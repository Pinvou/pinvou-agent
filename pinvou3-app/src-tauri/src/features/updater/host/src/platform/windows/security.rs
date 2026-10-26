//! Actual owner/DACL policy on retained native objects; never a serialized aclVerified flag.
use crate::error::{Error, Result};
use std::{ffi::c_void, fs::File, mem::size_of, os::windows::io::AsRawHandle, ptr};
use windows_sys::Win32::{
    Foundation::{HANDLE, LocalFree},
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        Authorization::{
            ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT, SE_OBJECT_TYPE,
            SE_REGISTRY_KEY, SE_SERVICE,
        },
        DACL_SECURITY_INFORMATION, GetAce, GetAclInformation, GetLengthSid,
        GetSecurityDescriptorControl, INHERIT_ONLY_ACE, IsValidSid, OWNER_SECURITY_INFORMATION,
        SE_DACL_PROTECTED,
    },
    System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE},
};

struct LocalDescriptor(*mut c_void);
impl Drop for LocalDescriptor {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}
#[allow(unsafe_code)]
pub(super) fn sid_string(pointer: *mut c_void) -> Result<String> {
    if pointer.is_null() || unsafe { IsValidSid(pointer) } == 0 {
        return Err(Error::SecurityRefused);
    }
    let mut text = ptr::null_mut();
    // SAFETY: pointer is a native validated SID in the still-owned OS descriptor;
    // this API returns its own LocalAlloc string, immediately assigned one owner.
    if unsafe { ConvertSidToStringSidW(pointer, &mut text) } == 0 {
        return Err(Error::Unavailable);
    }
    let guard = LocalDescriptor(text.cast());
    if guard.0.is_null() {
        return Err(Error::Unavailable);
    }
    let mut len = 0usize;
    while len < 192 && unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    if len == 192 {
        return Err(Error::SecurityRefused);
    }
    String::from_utf16(unsafe { std::slice::from_raw_parts(text, len) })
        .map_err(|_| Error::SecurityRefused)
}
fn trusted(sid: &str, ancestor: bool, service_sid: Option<&str>) -> bool {
    sid == "S-1-5-18"
        || sid == "S-1-5-32-544"
        || service_sid == Some(sid)
        || ancestor && sid == "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"
}
#[allow(unsafe_code)]
pub(crate) fn inspect_file(file: &File, host_owned: bool, service_sid: Option<&str>) -> Result<()> {
    inspect(
        file.as_raw_handle(),
        SE_FILE_OBJECT,
        host_owned,
        !host_owned,
        service_sid,
        AccessPolicy::File { owned: host_owned },
    )
}
#[allow(unsafe_code)]
fn inspect(
    handle: HANDLE,
    object_type: SE_OBJECT_TYPE,
    require_protected: bool,
    allow_os_owner: bool,
    service_sid: Option<&str>,
    policy: AccessPolicy,
) -> Result<()> {
    let mut owner = ptr::null_mut();
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SAFETY: live retained kernel handle; all out-pointers are initialized and
    // SDK allocations are one LocalFree owner. No caller bytes/pointers enter.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            object_type,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(Error::Unavailable);
    }
    let _guard = LocalDescriptor(descriptor);
    if descriptor.is_null() || dacl.is_null() || !trusted(&sid_string(owner)?, allow_os_owner, None)
    {
        return Err(Error::SecurityRefused);
    }
    let mut control = 0u16;
    let mut revision = 0u32;
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
        || require_protected && control & SE_DACL_PROTECTED == 0
    {
        return Err(Error::SecurityRefused);
    }
    let mut information = ACL_SIZE_INFORMATION::default();
    if unsafe {
        GetAclInformation(
            dacl,
            (&mut information as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
        || information.AceCount > 4096
        || information.AclBytesInUse > 65536
    {
        return Err(Error::SecurityRefused);
    }
    for i in 0..information.AceCount {
        let mut raw = ptr::null_mut();
        if unsafe { GetAce(dacl, i, &mut raw) } == 0 || raw.is_null() {
            return Err(Error::SecurityRefused);
        }
        // SAFETY: GetAce gives this indexed native ACE inside the owned valid ACL.
        let header = unsafe { &*raw.cast::<ACE_HEADER>() };
        if (header.AceSize as usize) < size_of::<ACE_HEADER>() {
            return Err(Error::SecurityRefused);
        }
        if (header.AceFlags as u32) & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        if header.AceType as u32 == ACCESS_DENIED_ACE_TYPE {
            continue;
        }
        // Conditional/object/callback semantics are not silently approximated.
        if header.AceType as u32 != ACCESS_ALLOWED_ACE_TYPE
            || (header.AceSize as usize) < size_of::<ACCESS_ALLOWED_ACE>() + 4
        {
            return Err(Error::SecurityRefused);
        }
        let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
        let sid = ptr::addr_of!(ace.SidStart).cast_mut().cast::<c_void>();
        let offset = std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        let sid_capacity = header.AceSize as usize - offset;
        if sid_capacity < 8 {
            return Err(Error::SecurityRefused);
        }
        // Check the native SID header's complete extent before passing it to SID APIs.
        let subauthorities = unsafe { *sid.cast::<u8>().add(1) } as usize;
        if subauthorities > 15
            || 8 + subauthorities * 4 > sid_capacity
            || unsafe { IsValidSid(sid) } == 0
            || unsafe { GetLengthSid(sid) } as usize > sid_capacity
        {
            return Err(Error::SecurityRefused);
        }
        let sid = sid_string(sid)?;
        if trusted(&sid, allow_os_owner, service_sid) {
            continue;
        }
        if !policy.permits_untrusted(ace.Mask) {
            return Err(Error::SecurityRefused);
        }
    }
    Ok(())
}

/// Close each object's access vocabulary instead of treating every generic
/// access bit as a file permission. In particular SERVICE_GENERIC_EXECUTE
/// includes START/STOP while FILE_GENERIC_EXECUTE is ordinary image reading.
#[derive(Clone, Copy)]
enum AccessPolicy {
    File { owned: bool },
    Registry,
    Service { quarantine: bool },
    Mutex,
    Pipe,
    ProcessQuery,
    TokenQuery,
}
impl AccessPolicy {
    fn permits_untrusted(self, mut mask: u32) -> bool {
        let allowed = match self {
            Self::File { owned } => {
                if owned {
                    0xa012_00a9
                } else {
                    0xa012_00af
                }
            }
            Self::Registry => 0xa002_0019,
            Self::Service { quarantine } => {
                if mask & 0x8000_0000 != 0 {
                    mask = (mask & !0x8000_0000) | 0x0002_008d;
                }
                if quarantine { 0x0002_018d } else { 0x0002_0085 }
            }
            Self::Mutex => 0,
            Self::Pipe => 0x0012_0083,
            Self::ProcessQuery => 0x0012_1000,
            Self::TokenQuery => 0x0002_0008,
        };
        mask & !allowed == 0
    }
}
pub(super) fn inspect_registry(handle: HANDLE, owned: bool) -> Result<()> {
    inspect(
        handle,
        SE_REGISTRY_KEY,
        owned,
        !owned,
        None,
        AccessPolicy::Registry,
    )
}
pub(super) fn inspect_service(handle: HANDLE) -> Result<()> {
    inspect(
        handle,
        SE_SERVICE,
        true,
        false,
        None,
        AccessPolicy::Service { quarantine: false },
    )
}
pub(super) fn inspect_service_quarantine(handle: HANDLE) -> Result<()> {
    inspect(
        handle,
        SE_SERVICE,
        false,
        false,
        None,
        AccessPolicy::Service { quarantine: true },
    )
}
pub(super) fn inspect_host_process(handle: HANDLE) -> Result<()> {
    inspect(
        handle,
        windows_sys::Win32::Security::Authorization::SE_KERNEL_OBJECT,
        true,
        false,
        None,
        AccessPolicy::ProcessQuery,
    )
}
pub(super) fn inspect_host_token(handle: HANDLE) -> Result<()> {
    inspect(
        handle,
        windows_sys::Win32::Security::Authorization::SE_KERNEL_OBJECT,
        true,
        false,
        None,
        AccessPolicy::TokenQuery,
    )
}
pub(super) fn inspect_pipe(handle: HANDLE) -> Result<()> {
    inspect(
        handle,
        windows_sys::Win32::Security::Authorization::SE_KERNEL_OBJECT,
        true,
        false,
        None,
        AccessPolicy::Pipe,
    )
}
pub(super) fn inspect_kernel(handle: HANDLE, kind: SE_OBJECT_TYPE) -> Result<()> {
    inspect(handle, kind, true, false, None, AccessPolicy::Mutex)
}
/// Native creation attributes for distinct fixed host object policies. These
/// are plain descriptors, never a bootstrap/installed capability constructor.
pub(super) struct NativeSecurity {
    descriptor: LocalDescriptor,
}
impl NativeSecurity {
    #[allow(unsafe_code)]
    fn from_aces(users: &str) -> Result<Self> {
        Self::from_sddl(&format!("O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA){users}"))
    }
    #[allow(unsafe_code)]
    fn from_sddl(sddl: &str) -> Result<Self> {
        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };
        let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut pointer = ptr::null_mut();
        let mut size = 0;
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut pointer,
                &mut size,
            )
        } == 0
        {
            return Err(Error::Unavailable);
        }
        let value = Self {
            descriptor: LocalDescriptor(pointer),
        };
        if pointer.is_null() || size == 0 || size > 65536 {
            return Err(Error::SecurityRefused);
        }
        Ok(value)
    }
    pub(super) fn registry(public_read: bool) -> Result<Self> {
        // Private parent SDs are inspectable by ordinary clients (READ_CONTROL
        // only), without granting query/enumeration of private record values.
        Self::from_aces(if public_read {
            "(A;;KR;;;BU)"
        } else {
            "(A;;RC;;;BU)"
        })
    }
    pub(super) fn host_process() -> Result<Self> {
        Self::from_aces("(A;;0x00121000;;;IU)")
    }
    pub(super) fn host_token() -> Result<Self> {
        Self::from_aces("(A;;0x00020008;;;IU)")
    }
    pub(super) fn mutex() -> Result<Self> {
        Self::from_aces("")
    }
    /// Exact writer/keeper policy. OW intentionally replaces the implicit owner
    /// WRITE_DAC grant with explicit READ_CONTROL; native behavior is not yet
    /// qualified by this source. No general owner/trusted-SID rule applies.
    pub(super) fn writer_job(service_sid: &str) -> Result<Self> {
        Self::from_sddl(&writer_job_sddl(service_sid)?)
    }
    #[allow(unsafe_code)]
    pub(super) fn inspect_writer_job(&self, handle: HANDLE) -> Result<()> {
        use windows_sys::Win32::Security::{
            Authorization::SE_KERNEL_OBJECT, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
        };
        let mut owner = ptr::null_mut();
        let mut actual_acl = ptr::null_mut();
        let mut actual = ptr::null_mut();
        if unsafe {
            GetSecurityInfo(
                handle,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut actual_acl,
                ptr::null_mut(),
                &mut actual,
            )
        } != 0
        {
            return Err(Error::Unavailable);
        }
        let _actual = LocalDescriptor(actual);
        if actual.is_null() || actual_acl.is_null() || sid_string(owner)? != "S-1-5-18" {
            return Err(Error::SecurityRefused);
        }
        let mut control = 0;
        let mut revision = 0;
        if unsafe { GetSecurityDescriptorControl(actual, &mut control, &mut revision) } == 0
            || control & SE_DACL_PROTECTED == 0
        {
            return Err(Error::SecurityRefused);
        }
        let mut expected_owner = ptr::null_mut();
        let mut defaulted_owner = 0;
        if unsafe {
            GetSecurityDescriptorOwner(self.descriptor.0, &mut expected_owner, &mut defaulted_owner)
        } == 0
            || sid_string(expected_owner)? != "S-1-5-18"
        {
            return Err(Error::SecurityRefused);
        }
        let mut present = 0;
        let mut expected_acl = ptr::null_mut();
        let mut defaulted = 0;
        if unsafe {
            GetSecurityDescriptorDacl(
                self.descriptor.0,
                &mut present,
                &mut expected_acl,
                &mut defaulted,
            )
        } == 0
            || present == 0
            || expected_acl.is_null()
        {
            return Err(Error::SecurityRefused);
        }
        let expected_info = writer_job_acl_information(expected_acl)?;
        let actual_info = writer_job_acl_information(actual_acl)?;
        // Compare raw native ACE bytes in exact order. This includes OW as
        // OWNER RIGHTS (S-1-3-4), flags, masks and dedicated service SID; it must
        // NOT pass through the generic trusted-owner policy above.
        for index in 0..3 {
            if writer_job_ace_bytes(expected_acl, &expected_info, index)?
                != writer_job_ace_bytes(actual_acl, &actual_info, index)?
            {
                return Err(Error::SecurityRefused);
            }
        }
        Ok(())
    }
    pub(super) fn pipe() -> Result<Self> {
        // Exact data IO/query/synchronize; omit FILE_APPEND_DATA=4 because it
        // is FILE_CREATE_PIPE_INSTANCE on named pipes. Never GENERIC_WRITE.
        Self::from_aces("(A;;0x00120083;;;IU)")
    }
    fn service() -> Result<Self> {
        Self::from_aces("(A;;0x00020085;;;IU)")
    }
    pub(super) fn attributes(&self) -> windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
        windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: 0,
        }
    }
    #[allow(unsafe_code)]
    pub(super) fn seal_dacl(&self, handle: HANDLE, kind: SE_OBJECT_TYPE) -> Result<()> {
        use windows_sys::Win32::Security::{
            Authorization::SetSecurityInfo, GetSecurityDescriptorDacl,
            PROTECTED_DACL_SECURITY_INFORMATION,
        };
        let mut present = 0;
        let mut dacl = ptr::null_mut();
        let mut defaulted = 0;
        if unsafe {
            GetSecurityDescriptorDacl(self.descriptor.0, &mut present, &mut dacl, &mut defaulted)
        } == 0
            || present == 0
            || dacl.is_null()
        {
            return Err(Error::SecurityRefused);
        }
        if unsafe {
            SetSecurityInfo(
                handle,
                kind,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                dacl,
                ptr::null_mut(),
            )
        } != 0
        {
            return Err(Error::Unavailable);
        }
        Ok(())
    }
}
fn writer_job_sddl(service_sid: &str) -> Result<String> {
    let parts: Vec<_> = service_sid
        .strip_prefix("S-1-5-80-")
        .ok_or(Error::SecurityRefused)?
        .split('-')
        .collect();
    if parts.len() != 5
        || parts.iter().any(|part| {
            part.parse::<u32>()
                .ok()
                .is_none_or(|value| value.to_string() != *part)
        })
    {
        return Err(Error::SecurityRefused);
    }
    Ok(format!(
        "O:SYG:SYD:P(A;;RC;;;OW)(A;;0x00020007;;;SY)(A;;0x00020004;;;{service_sid})"
    ))
}
#[allow(unsafe_code)]
fn writer_job_acl_information(acl: *mut ACL) -> Result<ACL_SIZE_INFORMATION> {
    let mut information = ACL_SIZE_INFORMATION::default();
    if unsafe {
        GetAclInformation(
            acl,
            (&mut information as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
        || information.AceCount != 3
        || information.AclBytesInUse > 1024
    {
        return Err(Error::SecurityRefused);
    }
    Ok(information)
}
#[allow(unsafe_code)]
fn writer_job_ace_bytes(
    acl: *mut ACL,
    information: &ACL_SIZE_INFORMATION,
    index: u32,
) -> Result<Vec<u8>> {
    let mut raw = ptr::null_mut();
    if unsafe { GetAce(acl, index, &mut raw) } == 0 || raw.is_null() {
        return Err(Error::SecurityRefused);
    }
    let start = acl as usize;
    let end = start
        .checked_add(information.AclBytesInUse as usize)
        .ok_or(Error::SecurityRefused)?;
    let address = raw as usize;
    if address < start
        || address
            .checked_add(size_of::<ACE_HEADER>())
            .is_none_or(|v| v > end)
    {
        return Err(Error::SecurityRefused);
    }
    // SAFETY: GetAce returned a native ACL member; header and complete ACE
    // extent are bounded against the retained descriptor before making a slice.
    let length = unsafe { (*raw.cast::<ACE_HEADER>()).AceSize } as usize;
    if length < size_of::<ACE_HEADER>() || address.checked_add(length).is_none_or(|v| v > end) {
        return Err(Error::SecurityRefused);
    }
    Ok(unsafe { std::slice::from_raw_parts(raw.cast::<u8>(), length) }.to_vec())
}
pub(super) fn seal_service(handle: HANDLE) -> Result<()> {
    NativeSecurity::service()?.seal_dacl(handle, SE_SERVICE)?;
    inspect_service(handle)
}
#[cfg(test)]
mod policy_tests {
    use super::*;
    #[test]
    fn writer_job_descriptor_is_exact_owner_rights_system_and_query_service_policy() {
        assert_eq!(
            writer_job_sddl("S-1-5-80-1-2-3-4-5").unwrap(),
            "O:SYG:SYD:P(A;;RC;;;OW)(A;;0x00020007;;;SY)(A;;0x00020004;;;S-1-5-80-1-2-3-4-5)"
        );
        for invalid in [
            "S-1-5-18",
            "S-1-5-80-1-2-3-4",
            "S-1-5-80-01-2-3-4-5",
            "S-1-5-80-4294967296-2-3-4-5",
            "S-1-5-80-1-2-3-4-5)(A;;GA;;;WD)",
        ] {
            assert_eq!(writer_job_sddl(invalid), Err(Error::SecurityRefused));
        }
        // This checks DATA construction only. Owner-rights access behavior,
        // native ACE normalization/order and restricted-token effects are UNRUN.
    }
    #[test]
    fn service_default_quarantine_is_closed_and_final_policy_removes_custom_control() {
        let before = AccessPolicy::Service { quarantine: true };
        let after = AccessPolicy::Service { quarantine: false };
        assert!(before.permits_untrusted(0x2018d));
        assert!(!after.permits_untrusted(0x2018d));
        assert!(after.permits_untrusted(0x20085));
        for bad in [
            2, 0x10, 0x20, 0x40, 0x8000, 0x10000, 0x40000, 0x80000, 0x10000000, 0x20000000,
            0x40000000, 0x01000000,
        ] {
            assert!(!before.permits_untrusted(bad), "{bad:x}");
        }
        assert!(before.permits_untrusted(0x80000000));
        assert!(!AccessPolicy::Mutex.permits_untrusted(0x100000));
        assert!(AccessPolicy::File { owned: true }.permits_untrusted(0xa01200a9));
        assert!(!AccessPolicy::File { owned: true }.permits_untrusted(0x01000000));
    }
}

// Candidate addition to append inside the actual windows/security.rs module.
// This file is a source block, not a new production module or test runner.
#[cfg(test)]
mod writer_job_owned_policy_tests {
    use super::super::{handle::Handle, pipe::nonce, process::RetainedProcess};
    use super::*;
    use windows_sys::Win32::{
        Foundation::{
            CompareObjectHandles, DuplicateHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS,
            GetHandleInformation, GetLastError, SetLastError,
        },
        Security::Authorization::SE_KERNEL_OBJECT,
        Storage::FileSystem::WRITE_DAC,
        System::{
            JobObjects::{
                CreateJobObjectW, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
                JobObjectBasicAccountingInformation, OpenJobObjectW, QueryInformationJobObject,
            },
            Threading::GetCurrentProcess,
        },
    };

    const QUERY_AND_READ_CONTROL: u32 = 0x0002_0004;
    const FIXTURE_SERVICE_SID: &str = "S-1-5-80-1-2-3-4-5";

    #[allow(unsafe_code)]
    fn owned_empty_job(sddl: &str, label: &str) -> (Handle, Vec<u16>) {
        let security = NativeSecurity::from_sddl(sddl).unwrap();
        let attributes = security.attributes();
        let name = format!("Local\\PinvouWriterOwnedAcl-{}-{label}", nonce().unwrap());
        let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
        unsafe { SetLastError(0) };
        let raw = unsafe { CreateJobObjectW(&attributes, name.as_ptr()) };
        let status = unsafe { GetLastError() };
        let job = Handle::owned(raw).unwrap();
        // Never adopt, alter or enumerate an existing object. These fixtures
        // assign no process and all handles remain ordinary owned test handles.
        assert_ne!(status, ERROR_ALREADY_EXISTS, "exclusive owned job required");
        (job, name)
    }

    #[test]
    #[allow(unsafe_code)]
    fn ordinary_owner_rights_blocks_implicit_write_dac_but_query_copy_remains_usable() {
        let current = RetainedProcess::current().unwrap();
        let sid = &current.token.user.sid;
        assert_ne!(
            sid.as_str(),
            "S-1-5-18",
            "ordinary-user fixture; this is not SYSTEM qualification"
        );
        assert_ne!(sid.as_str(), FIXTURE_SERVICE_SID);
        // Keep the exact OW/mask policy, substituting the actual ordinary user
        // for SYSTEM owner/ACE only. Omit explicit group assignment so Windows
        // uses the real token's default group without any group/owner privilege.
        let policy = format!(
            "O:{sid}D:P(A;;RC;;;OW)(A;;0x00020007;;;{sid})(A;;0x00020004;;;{FIXTURE_SERVICE_SID})"
        );
        let (job, name) = owned_empty_job(&policy, "owner-rights");

        let mut owner = ptr::null_mut();
        let mut acl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        let status = unsafe {
            GetSecurityInfo(
                job.raw(),
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        let _descriptor = LocalDescriptor(descriptor);
        assert_eq!(status, 0);
        assert_eq!(sid_string(owner).unwrap(), *sid);
        assert!(!descriptor.is_null() && !acl.is_null());
        let mut control = 0;
        let mut revision = 0;
        assert_ne!(
            unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
            0
        );
        assert_ne!(control & SE_DACL_PROTECTED, 0);

        // This is a real access check while the original object remains held.
        // If unexpectedly allowed, own that returned handle before asserting so
        // failure still closes only the fixture's own handles.
        unsafe { SetLastError(0) };
        let raw = unsafe { OpenJobObjectW(WRITE_DAC, 0, name.as_ptr()) };
        let denied_status = unsafe { GetLastError() };
        let unexpected_write = if raw.is_null() {
            None
        } else {
            Some(Handle::owned(raw).unwrap())
        };
        assert!(
            unexpected_write.is_none(),
            "OW READ_CONTROL must suppress implicit owner WRITE_DAC"
        );
        assert_eq!(denied_status, ERROR_ACCESS_DENIED);

        let query =
            Handle::owned(unsafe { OpenJobObjectW(QUERY_AND_READ_CONTROL, 0, name.as_ptr()) })
                .unwrap();
        assert_ne!(unsafe { CompareObjectHandles(query.raw(), job.raw()) }, 0);
        let mut flags = 0;
        assert_ne!(unsafe { GetHandleInformation(query.raw(), &mut flags) }, 0);
        assert_eq!(flags & 1, 0);

        let mut copied = ptr::null_mut();
        assert_ne!(
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    job.raw(),
                    GetCurrentProcess(),
                    &mut copied,
                    QUERY_AND_READ_CONTROL,
                    0,
                    0,
                )
            },
            0
        );
        let copied = Handle::owned(copied).unwrap();
        assert_ne!(unsafe { CompareObjectHandles(copied.raw(), job.raw()) }, 0);
        assert_ne!(unsafe { GetHandleInformation(copied.raw(), &mut flags) }, 0);
        assert_eq!(flags & 1, 0);
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let mut written = 0;
        assert_ne!(
            unsafe {
                QueryInformationJobObject(
                    copied.raw(),
                    JobObjectBasicAccountingInformation,
                    (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    &mut written,
                )
            },
            0
        );
        assert_eq!(
            written as usize,
            size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>()
        );
        assert_eq!(accounting.ActiveProcesses, 0); // This owned fixture is empty; no production proof is minted.

        // Small positive control distinguishes OWNER RIGHTS from an environment
        // that simply denies every WRITE_DAC open. The explicit user mask still
        // omits WRITE_DAC; without OW, the actual owner receives it implicitly.
        let without_ow =
            format!("O:{sid}D:P(A;;0x00020007;;;{sid})(A;;0x00020004;;;{FIXTURE_SERVICE_SID})");
        let (_control, control_name) = owned_empty_job(&without_ow, "implicit-owner-control");
        let _owner_write =
            Handle::owned(unsafe { OpenJobObjectW(WRITE_DAC, 0, control_name.as_ptr()) }).unwrap();
        // No security setter, token privilege adjustment, impersonation, process
        // assignment, SYSTEM/service fixture or keeper ACK is exercised.
    }
}
