//! Private native object queries. GetHandleInformation establishes flags ONLY;
//! NtQueryObject supplies actual GrantedAccess/type/name, and duplicated-back
//! CompareObjectHandles establishes identity. No wire field is a handle proof.
mod challenge;
use super::*;
use crate::json;
pub(super) use challenge::{
    ValidatedChallenge, cleanup_orphan_challenges, signal_challenge, take_challenge,
};
use challenge::{event_name, inspect_event, next_serial};
use std::{collections::BTreeSet, ffi::c_void, mem::size_of, ptr};
use windows_sys::{
    Wdk::Foundation::{NtQueryObject, ObjectBasicInformation, ObjectTypeInformation},
    Win32::{
        Foundation::{
            CompareObjectHandles, DuplicateHandle, ERROR_ALREADY_EXISTS, GetHandleInformation,
            GetLastError, LocalFree, SetHandleInformation, UNICODE_STRING, WAIT_OBJECT_0,
            WAIT_TIMEOUT,
        },
        Security::{
            ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
                SDDL_REVISION_1, SE_KERNEL_OBJECT,
            },
            DACL_SECURITY_INFORMATION, GetAce, GetAclInformation, GetSecurityDescriptorControl,
            GetSecurityDescriptorDacl, OWNER_SECURITY_INFORMATION, SE_DACL_PROTECTED,
            SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
        },
        System::{
            Pipes::{
                CreateNamedPipeW, GetNamedPipeInfo, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_SERVER_END, PIPE_TYPE_BYTE, PIPE_WAIT,
            },
            Threading::{
                CREATE_EVENT_MANUAL_RESET, CreateEventExW, GetCurrentProcess, WaitForSingleObject,
            },
        },
    },
};

const PIPE_IO: u32 = 0x0012_0083;
const JOB_QUERY: u32 = 0x0002_0004;
const EVENT_SIGNAL_QUERY: u32 = 3;
const EVENT_WAIT_QUERY: u32 = 0x0010_0001;
const OBJECT_NAME_INFORMATION: i32 = 1;
const KEEPER_DESCRIPTOR: &str = "O:SYG:SYD:P(A;;RC;;;OW)(A;;GA;;;SY)";

#[repr(C)]
#[derive(Default)]
struct PublicObjectBasicInformation {
    attributes: u32,
    granted_access: u32,
    handle_count: u32,
    pointer_count: u32,
    reserved: [u32; 10],
}
struct Descriptor(*mut c_void);
impl Drop for Descriptor {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
impl Descriptor {
    #[allow(unsafe_code)]
    fn keeper() -> Result<Self> {
        let text: Vec<_> = KEEPER_DESCRIPTOR.encode_utf16().chain(Some(0)).collect();
        let mut raw = ptr::null_mut();
        let mut length = 0;
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut raw,
                &mut length,
            )
        } == 0
        {
            return Err(Error::SecurityRefused);
        }
        let value = Self(raw);
        if raw.is_null() || length == 0 || length > 65536 {
            return Err(Error::SecurityRefused);
        }
        Ok(value)
    }
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}
/// Exact mask and inheritance. No generic or SAME_ACCESS duplication is used.
#[allow(unsafe_code)]
fn duplicate_local(raw: HANDLE, rights: u32, inherit: bool) -> Result<Handle> {
    let mut copy = ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            raw,
            GetCurrentProcess(),
            &mut copy,
            rights,
            i32::from(inherit),
            0,
        )
    } == 0
    {
        return Err(Error::HandoffOutcomeUnknown);
    }
    Handle::owned(copy)
}
#[allow(unsafe_code)]
fn flags(raw: HANDLE, inherited: bool) -> Result<()> {
    let mut flags = 0;
    if unsafe { GetHandleInformation(raw, &mut flags) } == 0 || flags != u32::from(inherited) {
        return Err(Error::SecurityRefused);
    }
    Ok(())
}
#[allow(unsafe_code)]
fn granted(raw: HANDLE, expected: u32) -> Result<()> {
    let mut basic = PublicObjectBasicInformation::default();
    let mut length = 0;
    if unsafe {
        NtQueryObject(
            raw,
            ObjectBasicInformation,
            (&mut basic as *mut PublicObjectBasicInformation).cast(),
            size_of::<PublicObjectBasicInformation>() as u32,
            &mut length,
        )
    } < 0
        || length != size_of::<PublicObjectBasicInformation>() as u32
        || basic.granted_access != expected
    {
        return Err(Error::SecurityRefused);
    }
    Ok(())
}
#[allow(unsafe_code)]
fn object_string(raw: HANDLE, class: i32) -> Result<String> {
    object_text(raw, class)?.ok_or(Error::SecurityRefused)
}
#[allow(unsafe_code)]
fn object_text(raw: HANDLE, class: i32) -> Result<Option<String>> {
    // Both documented information layouts begin with UNICODE_STRING. The rest
    // of ObjectTypeInformation is deliberately neither redefined nor trusted.
    let mut buffer = vec![0usize; 8192 / size_of::<usize>()];
    let mut length = 0;
    if unsafe { NtQueryObject(raw, class, buffer.as_mut_ptr().cast(), 8192, &mut length) } < 0
        || length < size_of::<UNICODE_STRING>() as u32
        || length > 8192
    {
        return Err(Error::SecurityRefused);
    }
    let value = unsafe { &*buffer.as_ptr().cast::<UNICODE_STRING>() };
    let start = buffer.as_ptr() as usize;
    let end = start + length as usize;
    let address = value.Buffer as usize;
    if value.Length == 0 {
        // Unnamed IO events must be excluded, not treated as challenge events.
        // Only ObjectNameInformation has a legitimate empty Unicode result.
        if class != OBJECT_NAME_INFORMATION {
            return Err(Error::SecurityRefused);
        }
        if value.MaximumLength == 0 && value.Buffer.is_null() {
            return Ok(None);
        }
        if value.MaximumLength % 2 != 0
            || address % 2 != 0
            || address < start
            || address
                .checked_add(value.MaximumLength as usize)
                .is_none_or(|v| v > end)
        {
            return Err(Error::SecurityRefused);
        }
        return Ok(None);
    }
    if value.Length % 2 != 0
        || value.MaximumLength < value.Length
        || address % 2 != 0
        || address < start
        || address
            .checked_add(value.Length as usize)
            .is_none_or(|v| v > end)
    {
        return Err(Error::SecurityRefused);
    }
    String::from_utf16(unsafe {
        std::slice::from_raw_parts(value.Buffer, value.Length as usize / 2)
    })
    .map(Some)
    .map_err(|_| Error::SecurityRefused)
}
pub(super) fn inspect_job(raw: HANDLE, name: &str, sid: &str) -> Result<()> {
    if !valid_job_name(name) {
        return Err(Error::SecurityRefused);
    }
    flags(raw, false)?;
    granted(raw, JOB_QUERY)?;
    if object_string(raw, ObjectTypeInformation)? != "Job"
        || object_string(raw, OBJECT_NAME_INFORMATION)? != native_global_name(name)?
    {
        return Err(Error::SecurityRefused);
    }
    super::super::security::NativeSecurity::writer_job(sid)?.inspect_writer_job(raw)?;
    query_job(raw).map(|_| ())
}
pub(super) fn valid_job_name(name: &str) -> bool {
    name.strip_prefix("Global\\PinvouWriter-")
        .is_some_and(json::digest)
}
fn native_global_name(name: &str) -> Result<String> {
    let leaf = name
        .strip_prefix("Global\\")
        .ok_or(Error::SecurityRefused)?;
    if leaf.contains(['\\', '/', '\0']) {
        return Err(Error::SecurityRefused);
    }
    Ok(format!("\\BaseNamedObjects\\{leaf}"))
}
#[allow(unsafe_code)]
pub(super) fn query_job(raw: HANDLE) -> Result<u32> {
    use windows_sys::Win32::System::JobObjects::{
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
        QueryInformationJobObject,
    };
    let mut value = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
    let mut length = 0;
    if unsafe {
        QueryInformationJobObject(
            raw,
            JobObjectBasicAccountingInformation,
            (&mut value as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
            size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
            &mut length,
        )
    } == 0
        || length != size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32
    {
        return Err(Error::SecurityRefused);
    }
    Ok(value.ActiveProcesses)
}
#[allow(unsafe_code)]
pub(super) fn create_listener(name: &str) -> Result<Handle> {
    let descriptor = Descriptor::keeper()?;
    let attributes = descriptor.attributes();
    let name_wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
    let original = Handle::owned(unsafe {
        CreateNamedPipeW(
            name_wide.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            (json::FRAME_MAX + 4) as u32,
            (json::FRAME_MAX + 4) as u32,
            15000,
            &attributes,
        )
    })?;
    let reduced = duplicate_local(original.raw(), PIPE_IO, false)?;
    if unsafe { CompareObjectHandles(original.raw(), reduced.raw()) } == 0 {
        return Err(Error::SecurityRefused);
    }
    Ok(reduced)
}
pub(super) fn duplicate_listener(raw: HANDLE) -> Result<Handle> {
    duplicate_local(raw, PIPE_IO, true)
}
#[allow(unsafe_code)]
pub(super) fn inspect_listener(raw: HANDLE, name: &str, inherited: bool) -> Result<()> {
    flags(raw, inherited)?;
    granted(raw, PIPE_IO)?;
    let expected = name
        .strip_prefix("\\\\.\\pipe\\")
        .ok_or(Error::SecurityRefused)?;
    if object_string(raw, ObjectTypeInformation)? != "File"
        || !super::super::layout::ordinal_equal(
            &object_string(raw, OBJECT_NAME_INFORMATION)?,
            &format!("\\Device\\NamedPipe\\{expected}"),
        )?
    {
        return Err(Error::SecurityRefused);
    }
    let mut kind = 0;
    let mut out = 0;
    let mut input = 0;
    let mut maximum = 0;
    if unsafe { GetNamedPipeInfo(raw, &mut kind, &mut out, &mut input, &mut maximum) } == 0
        || kind != PIPE_SERVER_END
        || maximum != 1
        || out < (json::FRAME_MAX + 4) as u32
        || input < (json::FRAME_MAX + 4) as u32
        || out > (json::FRAME_MAX + 65540) as u32
        || input > (json::FRAME_MAX + 65540) as u32
    {
        return Err(Error::SecurityRefused);
    }
    inspect_descriptor(raw)
}
#[allow(unsafe_code)]
pub(super) fn inherited_listener(seed: &Seed) -> Result<Pipe> {
    use windows_sys::Win32::System::Threading::{
        GetStartupInfoW, STARTF_USESTDHANDLES, STARTUPINFOW,
    };
    let mut startup = STARTUPINFOW::default();
    unsafe {
        GetStartupInfoW(&mut startup);
    }
    if startup.dwFlags & STARTF_USESTDHANDLES == 0
        || !startup.hStdOutput.is_null()
        || !startup.hStdError.is_null()
    {
        return Err(Error::SecurityRefused);
    }
    let name = super::pipe_name(seed);
    inspect_listener(startup.hStdInput, &name, true)?;
    if unsafe { SetHandleInformation(startup.hStdInput, 1, 0) } == 0 {
        return Err(Error::SecurityRefused);
    }
    inspect_listener(startup.hStdInput, &name, false)?;
    Ok(Pipe::keeper_from_owned_server(Handle::owned(
        startup.hStdInput,
    )?))
}

/// Exactly one fresh, service-created challenge. Only this module constructs
/// it; fixed RunningKeeper code may direct it into its held actual process.
pub(in crate::platform::windows) struct NativeChallengeEvent {
    handle: Handle,
    nonce: String,
    host_epoch: u64,
    serial: u64,
}
pub(in crate::platform::windows) struct PendingNativeChallenge {
    wait_query: Handle,
    nonce: String,
    remote: usize,
    keeper: ProcessSelector,
    host_epoch: u64,
    serial: u64,
}
impl NativeChallengeEvent {
    #[allow(unsafe_code)]
    pub(super) fn new(host: &StartedServiceContext) -> Result<Self> {
        let record = super::super::registry::Store::new(&host.context.seed)
            .read(&host.context.seed)?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&record)?;
        let host_epoch = record.host_epoch;
        let serial = next_serial()?;
        let nonce = super::super::pipe::nonce()?;
        let name: Vec<_> = event_name(host_epoch, serial, &nonce)?
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let descriptor = Descriptor::keeper()?;
        let attributes = descriptor.attributes();
        let raw = unsafe {
            CreateEventExW(
                &attributes,
                name.as_ptr(),
                CREATE_EVENT_MANUAL_RESET,
                EVENT_SIGNAL_QUERY | 0x0012_0000,
            )
        };
        let code = unsafe { GetLastError() };
        let handle = Handle::owned(raw)?;
        if code == ERROR_ALREADY_EXISTS {
            return Err(Error::ScopeConflict);
        }
        flags(handle.raw(), false)?;
        Ok(Self {
            handle,
            nonce,
            host_epoch,
            serial,
        })
    }
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn duplicate_into(
        self,
        actual: &Handle,
        selector: &ProcessSelector,
    ) -> Result<PendingNativeChallenge> {
        selector.validate()?;
        let wait_query = duplicate_local(self.handle.raw(), EVENT_WAIT_QUERY, false)?;
        let mut remote = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.handle.raw(),
                actual.raw(),
                &mut remote,
                EVENT_SIGNAL_QUERY,
                0,
                0,
            )
        } == 0
        {
            return Err(Error::HandoffOutcomeUnknown);
        }
        // Query through a native duplicate-back, then compare the actual object.
        let mut roundtrip = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                actual.raw(),
                remote,
                GetCurrentProcess(),
                &mut roundtrip,
                EVENT_SIGNAL_QUERY,
                0,
                0,
            )
        } == 0
        {
            return Err(Error::HandoffOutcomeUnknown);
        }
        let roundtrip = Handle::owned(roundtrip)?;
        inspect_event(roundtrip.raw(), self.host_epoch, self.serial, &self.nonce)?;
        if unsafe { CompareObjectHandles(wait_query.raw(), roundtrip.raw()) } == 0
            || unsafe { WaitForSingleObject(wait_query.raw(), 0) } != WAIT_TIMEOUT
        {
            return Err(Error::SecurityRefused);
        }
        let result = PendingNativeChallenge {
            wait_query,
            nonce: self.nonce,
            remote: remote as usize,
            keeper: selector.clone(),
            host_epoch: self.host_epoch,
            serial: self.serial,
        };
        // Both original modifying handle and duplicate-back close BEFORE the
        // address is sent. The service retains only wait/query; it cannot cause
        // its own event observation by accidentally calling SetEvent locally.
        drop(roundtrip);
        drop(self.handle);
        granted(result.wait_query.raw(), EVENT_WAIT_QUERY)?;
        Ok(result)
    }
}
impl PendingNativeChallenge {
    pub(super) fn host_epoch(&self) -> u64 {
        self.host_epoch
    }
    pub(super) fn serial(&self) -> u64 {
        self.serial
    }
    pub(super) fn nonce(&self) -> &str {
        &self.nonce
    }
    pub(super) fn remote(&self) -> usize {
        self.remote
    }
    pub(super) fn keeper(&self) -> &ProcessSelector {
        &self.keeper
    }
    #[allow(unsafe_code)]
    pub(super) fn observed_signal(&self) -> Result<()> {
        flags(self.wait_query.raw(), false)?;
        granted(self.wait_query.raw(), EVENT_WAIT_QUERY)?;
        if unsafe { WaitForSingleObject(self.wait_query.raw(), 0) } != WAIT_OBJECT_0 {
            return Err(Error::HandoffOutcomeUnknown);
        }
        Ok(())
    }
}
/// A bounded observation of THIS process's actual kernel handle table. This
/// covers job copies duplicated by a service that died before sending retain.
/// Unknown query/type/rights/name, unsupported class or capacity failure is
/// uncertainty. The service must serialize this with all job duplications.
pub(super) struct NativeJobHandle {
    pub(super) raw: HANDLE,
    pub(super) name: String,
}
#[repr(C)]
struct HandleSnapshotHeader {
    count: usize,
    reserved: usize,
}
#[repr(C)]
struct HandleSnapshotEntry {
    handle: HANDLE,
    handle_count: usize,
    pointer_count: usize,
    granted_access: u32,
    object_type_index: u32,
    attributes: u32,
    reserved: u32,
}
/// Snapshot DATA stays private to the native object adapters. It cannot open,
/// own or authorize closing a handle without a separate exact object check.
struct NativeHandle {
    raw: HANDLE,
    granted_access: u32,
    attributes: u32,
}
#[allow(unsafe_code)]
fn native_handles() -> Result<Vec<NativeHandle>> {
    use windows_sys::Wdk::System::Threading::{
        NtQueryInformationProcess, ProcessHandleInformation,
    };
    const LIMIT: usize = 4096;
    let capacity = size_of::<HandleSnapshotHeader>() + LIMIT * size_of::<HandleSnapshotEntry>();
    if capacity > 1_048_576 {
        return Err(Error::SecurityRefused);
    }
    let mut buffer = vec![0usize; capacity.div_ceil(size_of::<usize>())];
    let mut length = 0;
    if unsafe {
        NtQueryInformationProcess(
            GetCurrentProcess(),
            ProcessHandleInformation,
            buffer.as_mut_ptr().cast(),
            capacity as u32,
            &mut length,
        )
    } < 0
        || (length as usize) < size_of::<HandleSnapshotHeader>()
        || length as usize > capacity
    {
        return Err(Error::HandoffOutcomeUnknown);
    }
    let header = unsafe { &*buffer.as_ptr().cast::<HandleSnapshotHeader>() };
    if header.count > LIMIT
        || size_of::<HandleSnapshotHeader>()
            .checked_add(
                header
                    .count
                    .checked_mul(size_of::<HandleSnapshotEntry>())
                    .ok_or(Error::SecurityRefused)?,
            )
            .is_none_or(|n| n > length as usize)
    {
        return Err(Error::SecurityRefused);
    }
    let entries = unsafe {
        buffer
            .as_ptr()
            .cast::<u8>()
            .add(size_of::<HandleSnapshotHeader>())
            .cast::<HandleSnapshotEntry>()
    };
    let mut result = Vec::with_capacity(header.count);
    let mut addresses = BTreeSet::new();
    for index in 0..header.count {
        let entry = unsafe { &*entries.add(index) };
        if entry.handle.is_null() || !addresses.insert(entry.handle as usize) {
            return Err(Error::HandoffOutcomeUnknown);
        }
        result.push(NativeHandle {
            raw: entry.handle,
            granted_access: entry.granted_access,
            attributes: entry.attributes,
        });
    }
    Ok(result)
}
pub(super) fn native_jobs(sid: &str) -> Result<Vec<NativeJobHandle>> {
    let mut result = Vec::new();
    for entry in native_handles()? {
        if object_string(entry.raw, ObjectTypeInformation)? == "Job" {
            if entry.granted_access != JOB_QUERY || entry.attributes != 0 {
                return Err(Error::SecurityRefused);
            }
            let native_name = object_string(entry.raw, OBJECT_NAME_INFORMATION)?;
            let leaf = native_name
                .strip_prefix("\\BaseNamedObjects\\")
                .ok_or(Error::SecurityRefused)?;
            let name = format!("Global\\{leaf}");
            inspect_job(entry.raw, &name, sid)?;
            result.push(NativeJobHandle {
                raw: entry.raw,
                name,
            });
        }
    }
    Ok(result)
}

#[allow(unsafe_code)]
fn inspect_descriptor(raw: HANDLE) -> Result<()> {
    let expected = Descriptor::keeper()?;
    let mut owner = ptr::null_mut();
    let mut actual_acl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    if unsafe {
        GetSecurityInfo(
            raw,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut actual_acl,
            ptr::null_mut(),
            &mut descriptor,
        )
    } != 0
    {
        return Err(Error::SecurityRefused);
    }
    let _descriptor = Descriptor(descriptor);
    if descriptor.is_null()
        || actual_acl.is_null()
        || super::super::security::sid_string(owner)? != "S-1-5-18"
    {
        return Err(Error::SecurityRefused);
    }
    let mut control = 0;
    let mut revision = 0;
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
    {
        return Err(Error::SecurityRefused);
    }
    let mut present = 0;
    let mut defaulted = 0;
    let mut expected_acl = ptr::null_mut();
    if unsafe {
        GetSecurityDescriptorDacl(expected.0, &mut present, &mut expected_acl, &mut defaulted)
    } == 0
        || present == 0
        || expected_acl.is_null()
    {
        return Err(Error::SecurityRefused);
    }
    if acl_bytes(actual_acl)? != acl_bytes(expected_acl)? {
        return Err(Error::SecurityRefused);
    }
    Ok(())
}
#[allow(unsafe_code)]
fn acl_bytes(acl: *mut ACL) -> Result<Vec<Vec<u8>>> {
    let mut information = ACL_SIZE_INFORMATION::default();
    if unsafe {
        GetAclInformation(
            acl,
            (&mut information as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
        || information.AceCount > 16
        || information.AclBytesInUse > 65536
    {
        return Err(Error::SecurityRefused);
    }
    let start = acl as usize;
    let end = start
        .checked_add(information.AclBytesInUse as usize)
        .ok_or(Error::SecurityRefused)?;
    let mut result = Vec::new();
    for index in 0..information.AceCount {
        let mut raw = ptr::null_mut();
        if unsafe { GetAce(acl, index, &mut raw) } == 0
            || raw.is_null()
            || (raw as usize) < start
            || (raw as usize)
                .checked_add(size_of::<ACE_HEADER>())
                .is_none_or(|n| n > end)
        {
            return Err(Error::SecurityRefused);
        }
        let size = unsafe { (*raw.cast::<ACE_HEADER>()).AceSize } as usize;
        if size < size_of::<ACE_HEADER>()
            || (raw as usize).checked_add(size).is_none_or(|n| n > end)
        {
            return Err(Error::SecurityRefused);
        }
        result.push(unsafe { std::slice::from_raw_parts(raw.cast::<u8>(), size) }.to_vec());
    }
    Ok(result)
}
