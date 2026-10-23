//! Retained native process/token facts. No requested PID/SID becomes an authenticated peer.
use super::{handle::Handle, security::sid_string};
use crate::{
    error::{Error, Result},
    identity::{FileTime, LogonId, ProcessSelector},
};
use std::{
    mem::{offset_of, size_of},
    path::PathBuf,
    ptr,
};
use windows_sys::{
    Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation},
    Win32::{
        Foundation::{
            ERROR_INSUFFICIENT_BUFFER, FILETIME, GetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Security::{
            GetTokenInformation, SID_AND_ATTRIBUTES, TOKEN_GROUPS, TOKEN_INFORMATION_CLASS,
            TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_STATISTICS, TOKEN_USER, TokenElevation,
            TokenGroups, TokenIntegrityLevel, TokenIsAppContainer, TokenSessionId, TokenStatistics,
            TokenUser,
        },
        System::{
            SystemServices::{SE_GROUP_ENABLED, SE_GROUP_USE_FOR_DENY_ONLY},
            Threading::{
                GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, OpenProcess,
                OpenProcessToken, PROCESS_BASIC_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject,
            },
        },
    },
};

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct NativeUser {
    pub(crate) sid: String,
    pub(crate) logon: LogonId,
    pub(crate) session_id: u32,
}
pub(super) struct TokenFacts {
    pub(super) user: NativeUser,
    pub(super) integrity: u32,
    pub(super) app_container: bool,
    pub(super) elevated: bool,
    pub(super) administrator: bool,
    pub(super) interactive: bool,
    pub(super) network: bool,
    pub(super) service_sid_enabled: bool,
}
struct TokenBuffer {
    words: Vec<usize>,
    len: usize,
}
impl TokenBuffer {
    #[allow(unsafe_code)]
    fn typed<T>(&self) -> Result<&T> {
        if self.len < size_of::<T>()
            || self.words.as_ptr().align_offset(std::mem::align_of::<T>()) != 0
        {
            return Err(Error::PeerRefused);
        }
        // SAFETY: aligned initialized output from the SDK for this exact requested
        // token class. Call sites pair the class with its documented T structure.
        Ok(unsafe { &*self.words.as_ptr().cast::<T>() })
    }
}
#[allow(unsafe_code)]
fn information(token: &Handle, class: TOKEN_INFORMATION_CLASS) -> Result<TokenBuffer> {
    let mut needed = 0u32;
    let result =
        unsafe { GetTokenInformation(token.raw(), class, ptr::null_mut(), 0, &mut needed) };
    let probe_error = unsafe { GetLastError() };
    if result != 0 || probe_error != ERROR_INSUFFICIENT_BUFFER || needed == 0 || needed > 65536 {
        return Err(Error::PeerRefused);
    }
    let mut words = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    let capacity = words.len() * size_of::<usize>();
    if unsafe {
        GetTokenInformation(
            token.raw(),
            class,
            words.as_mut_ptr().cast(),
            capacity as u32,
            &mut needed,
        )
    } == 0
        || needed as usize > capacity
    {
        return Err(Error::PeerRefused);
    }
    Ok(TokenBuffer {
        words,
        len: needed as usize,
    })
}
#[allow(unsafe_code)]
fn sid_in(buffer: &TokenBuffer, pointer: *mut std::ffi::c_void) -> Result<String> {
    let start = buffer.words.as_ptr() as usize;
    let end = start.checked_add(buffer.len).ok_or(Error::PeerRefused)?;
    let p = pointer as usize;
    if p < start || p.checked_add(8).is_none_or(|v| v > end) {
        return Err(Error::PeerRefused);
    }
    let count = unsafe { *pointer.cast::<u8>().add(1) } as usize;
    if count > 15 || p.checked_add(8 + count * 4).is_none_or(|v| v > end) {
        return Err(Error::PeerRefused);
    }
    sid_string(pointer)
}
#[allow(unsafe_code)]
fn scalar_information(token: &Handle, class: TOKEN_INFORMATION_CLASS) -> Result<u32> {
    if class != TokenSessionId && class != TokenIsAppContainer && class != TokenElevation {
        return Err(Error::PeerRefused);
    }
    let mut value = 0u32;
    let mut written = 0u32;
    // SAFETY: these three documented classes return one DWORD. Query their
    // exact initialized output directly: TokenElevation's zero-length probe
    // returns ERROR_BAD_LENGTH on supported Windows, unlike variable classes.
    // No error/result is interpreted as an elevated or non-container identity.
    if unsafe {
        GetTokenInformation(
            token.raw(),
            class,
            (&mut value as *mut u32).cast(),
            size_of::<u32>() as u32,
            &mut written,
        )
    } == 0
        || written != size_of::<u32>() as u32
    {
        return Err(Error::PeerRefused);
    }
    Ok(value)
}
pub(super) fn token_facts(token: &Handle, service_sid: Option<&str>) -> Result<TokenFacts> {
    let user = information(token, TokenUser)?;
    let sid = sid_in(&user, user.typed::<TOKEN_USER>()?.User.Sid)?;
    let stats = information(token, TokenStatistics)?;
    let stats = stats.typed::<TOKEN_STATISTICS>()?;
    let logon = LogonId {
        high: stats.AuthenticationId.HighPart as u32,
        low: stats.AuthenticationId.LowPart,
    };
    let session_id = scalar_information(token, TokenSessionId)?;
    let integrity_buffer = information(token, TokenIntegrityLevel)?;
    let mandatory = integrity_buffer.typed::<TOKEN_MANDATORY_LABEL>()?;
    let integrity_sid = sid_in(&integrity_buffer, mandatory.Label.Sid)?;
    let integrity = integrity_sid
        .rsplit('-')
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or(Error::PeerRefused)?;
    let app_container = scalar_information(token, TokenIsAppContainer)? != 0;
    let elevated = scalar_information(token, TokenElevation)? != 0;
    let buffer = information(token, TokenGroups)?;
    let groups = buffer.typed::<TOKEN_GROUPS>()?;
    let count = groups.GroupCount as usize;
    let required = offset_of!(TOKEN_GROUPS, Groups)
        .checked_add(
            count
                .checked_mul(size_of::<SID_AND_ATTRIBUTES>())
                .ok_or(Error::PeerRefused)?,
        )
        .ok_or(Error::PeerRefused)?;
    if required > buffer.len {
        return Err(Error::PeerRefused);
    }
    let mut administrator = false;
    let mut interactive = false;
    let mut network = false;
    let mut service_sid_enabled = false;
    for i in 0..count {
        // SAFETY: complete flexible-array extent was checked against the actual
        // SDK output buffer above; no pointer outlives that buffer.
        #[allow(unsafe_code)]
        let group = unsafe { &*groups.Groups.as_ptr().add(i) };
        if group.Attributes & (SE_GROUP_ENABLED as u32) == 0
            || group.Attributes & (SE_GROUP_USE_FOR_DENY_ONLY as u32) != 0
        {
            continue;
        }
        let sid = sid_in(&buffer, group.Sid)?;
        administrator |= sid == "S-1-5-32-544";
        interactive |= sid == "S-1-5-4" || sid == "S-1-5-14";
        network |= sid == "S-1-5-2" || sid == "S-1-5-7";
        service_sid_enabled |= service_sid == Some(sid.as_str());
    }
    Ok(TokenFacts {
        user: NativeUser {
            sid,
            logon,
            session_id,
        },
        integrity,
        app_container,
        elevated,
        administrator,
        interactive,
        network,
        service_sid_enabled,
    })
}

#[allow(unsafe_code)]
pub(super) fn creation_time(handle: windows_sys::Win32::Foundation::HANDLE) -> Result<FileTime> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } == 0 {
        return Err(Error::Unavailable);
    }
    let value = FileTime::from_native(creation.dwHighDateTime, creation.dwLowDateTime);
    if value.ticks() == 0 {
        return Err(Error::Unavailable);
    }
    Ok(value)
}
fn absence_from_open_error(id: u32, error: u32) -> Result<bool> {
    // OpenProcess documents ERROR_INVALID_PARAMETER for a nonexistent PID.
    // PID zero is never a valid protected installer identity. AccessDenied or
    // any unclassified query/native failure is uncertainty, NEVER death.
    if id != 0 && error == windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER {
        Ok(true)
    } else {
        Err(Error::Unavailable)
    }
}
#[allow(unsafe_code)]
pub(super) fn prove_prior_installer_gone(selector: &ProcessSelector) -> Result<bool> {
    selector.validate()?;
    let raw = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            selector.process_id,
        )
    };
    if raw.is_null() {
        return absence_from_open_error(selector.process_id, unsafe { GetLastError() });
    }
    let handle = Handle::owned(raw)?;
    let creation = creation_time(handle.raw())?;
    if creation != selector.creation_file_time {
        return Ok(true);
    }
    match unsafe { WaitForSingleObject(handle.raw(), 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(Error::Unavailable),
    }
}

pub(super) struct RetainedProcess {
    handle: Handle,
    pub(super) selector: ProcessSelector,
    pub(super) token: TokenFacts,
    pub(super) image: PathBuf,
}
impl RetainedProcess {
    #[allow(unsafe_code)]
    pub(super) fn open(id: u32, service_sid: Option<&str>) -> Result<Self> {
        if id == 0 {
            return Err(Error::PeerRefused);
        }
        // SAFETY: query/synchronize only; never request terminate/injection/dup.
        let handle = Handle::owned(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | 0x00020000,
                0,
                id,
            )
        })?;
        Self::observe(handle, id, service_sid)
    }
    #[allow(unsafe_code)]
    fn observe(handle: Handle, id: u32, service_sid: Option<&str>) -> Result<Self> {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if unsafe {
            GetProcessTimes(
                handle.raw(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } == 0
        {
            return Err(Error::PeerRefused);
        }
        let selector = ProcessSelector {
            process_id: id,
            creation_file_time: FileTime::from_native(
                creation.dwHighDateTime,
                creation.dwLowDateTime,
            ),
        };
        selector.validate()?;
        let mut token = ptr::null_mut();
        if unsafe { OpenProcessToken(handle.raw(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(Error::PeerRefused);
        }
        let token = token_facts(&Handle::owned(token)?, service_sid)?;
        let mut image = vec![0u16; 32768];
        let mut length = image.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle.raw(), 0, image.as_mut_ptr(), &mut length) }
            == 0
            || length == 0
            || length as usize >= image.len()
        {
            return Err(Error::PeerRefused);
        }
        let image = PathBuf::from(
            String::from_utf16(&image[..length as usize]).map_err(|_| Error::PeerRefused)?,
        );
        let value = Self {
            handle,
            selector,
            token,
            image,
        };
        if value.exited()? {
            return Err(Error::PeerRefused);
        }
        Ok(value)
    }
    #[allow(unsafe_code)]
    pub(super) fn current() -> Result<Self> {
        Self::open(unsafe { GetCurrentProcessId() }, None)
    }
    #[allow(unsafe_code)]
    pub(super) fn verify_host_query_security(&self) -> Result<()> {
        super::security::inspect_host_process(self.handle.raw())?;
        let mut token = ptr::null_mut();
        if unsafe { OpenProcessToken(self.handle.raw(), TOKEN_QUERY | 0x00020000, &mut token) } == 0
        {
            return Err(Error::PeerRefused);
        }
        let token = Handle::owned(token)?;
        super::security::inspect_host_token(token.raw())
    }
    #[allow(unsafe_code)]
    pub(super) fn fresh_token(&self, service_sid: Option<&str>) -> Result<TokenFacts> {
        if self.exited()? {
            return Err(Error::PeerRefused);
        }
        let mut token = ptr::null_mut();
        if unsafe { OpenProcessToken(self.handle.raw(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(Error::PeerRefused);
        }
        let token = Handle::owned(token)?;
        let facts = token_facts(&token, service_sid)?;
        if self.exited()? {
            return Err(Error::PeerRefused);
        }
        Ok(facts)
    }
    #[allow(unsafe_code)]
    pub(super) fn parent(&self) -> Result<Self> {
        let mut value = PROCESS_BASIC_INFORMATION::default();
        let mut length = 0;
        if unsafe {
            NtQueryInformationProcess(
                self.raw(),
                ProcessBasicInformation,
                (&mut value as *mut PROCESS_BASIC_INFORMATION).cast(),
                size_of::<PROCESS_BASIC_INFORMATION>() as u32,
                &mut length,
            )
        } < 0
            || length as usize != size_of::<PROCESS_BASIC_INFORMATION>()
        {
            return Err(Error::PeerRefused);
        }
        Self::open(
            u32::try_from(value.InheritedFromUniqueProcessId).map_err(|_| Error::PeerRefused)?,
            None,
        )
    }
    pub(super) fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.handle.raw()
    }
    #[allow(unsafe_code)]
    pub(super) fn exited(&self) -> Result<bool> {
        match unsafe { WaitForSingleObject(self.handle.raw(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(Error::Unavailable),
        }
    }
    #[allow(unsafe_code)]
    pub(super) fn actual_parent() -> Result<Self> {
        let mut value = PROCESS_BASIC_INFORMATION::default();
        let mut length = 0u32;
        // SAFETY: OS-provided actual current process basic information, exact
        // pinned SDK structure; this is not a caller-selected parent PID.
        if unsafe {
            NtQueryInformationProcess(
                GetCurrentProcess(),
                ProcessBasicInformation,
                (&mut value as *mut PROCESS_BASIC_INFORMATION).cast(),
                size_of::<PROCESS_BASIC_INFORMATION>() as u32,
                &mut length,
            )
        } < 0
            || length as usize != size_of::<PROCESS_BASIC_INFORMATION>()
        {
            return Err(Error::InstallerContextRefused);
        }
        let id = u32::try_from(value.InheritedFromUniqueProcessId)
            .map_err(|_| Error::InstallerContextRefused)?;
        Self::open(id, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unsafe_code)]
    fn handles() -> u32 {
        let mut count = 0u32;
        assert_ne!(
            unsafe {
                windows_sys::Win32::System::Threading::GetProcessHandleCount(
                    GetCurrentProcess(),
                    &mut count,
                )
            },
            0
        );
        count
    }
    #[test]
    fn actual_self_process_identity_is_lossless_and_query_handles_close() {
        let current = RetainedProcess::current().unwrap();
        assert_eq!(current.selector.process_id, std::process::id());
        assert!(current.selector.creation_file_time.ticks() > 9_007_199_254_740_991);
        assert!(!current.exited().unwrap());
        assert!(!current.token.user.sid.is_empty());
        let before = handles();
        for _ in 0..32 {
            let query = RetainedProcess::current().unwrap();
            assert!(query.selector == current.selector);
            assert!(query.token.user == current.token.user);
        }
        assert_eq!(handles(), before);
        // Native process/token observation only; no installer, role or host context.
    }
    #[test]
    #[allow(unsafe_code)]
    fn actual_held_prior_process_exit_and_pid_incarnation_are_distinct() {
        use std::{
            io::Write,
            os::windows::io::AsRawHandle,
            process::{Command, Stdio},
        };
        let mut directory = [0u16; 32768];
        let length = unsafe {
            windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
                directory.as_mut_ptr(),
                directory.len() as u32,
            )
        } as usize;
        assert!(length > 0 && length < directory.len());
        let executable =
            std::path::PathBuf::from(String::from_utf16(&directory[..length]).unwrap())
                .join("cmd.exe");
        struct JoinedChild(std::process::Child);
        impl std::ops::Deref for JoinedChild {
            type Target = std::process::Child;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
        impl std::ops::DerefMut for JoinedChild {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
        impl Drop for JoinedChild {
            fn drop(&mut self) {
                self.0.stdin.take();
                let _ = self.0.wait();
            }
        }
        let mut child = JoinedChild(
            Command::new(executable)
                .args(["/d", "/q", "/c", "set /p _pinvou_gate="])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let selector = ProcessSelector {
            process_id: child.id(),
            creation_file_time: creation_time(child.as_raw_handle()).unwrap(),
        };
        assert!(!prove_prior_installer_gone(&selector).unwrap());
        let ticks = selector.creation_file_time.ticks() + 1;
        let other = ProcessSelector {
            process_id: selector.process_id,
            creation_file_time: FileTime::from_native((ticks >> 32) as u32, ticks as u32),
        };
        assert!(prove_prior_installer_gone(&other).unwrap());
        child.stdin.take().unwrap().write_all(b"done\r\n").unwrap();
        child.wait().unwrap();
        assert!(prove_prior_installer_gone(&selector).unwrap());
        for code in [5, 6, 0, 1168] {
            assert!(absence_from_open_error(selector.process_id, code).is_err());
        }
        assert!(absence_from_open_error(0, 87).is_err());
    }
}

/// Service-only process/token DACL sealing after proving actual System + service
/// SID. Query access is explicit; no standard user obtains injection, token
/// adjustment/duplication, termination, or arbitrary service impersonation.
#[allow(unsafe_code)]
pub(super) fn seal_host_query_access(service_sid: &str) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{LUID, SetLastError},
        Security::{
            AdjustTokenPrivileges, Authorization::SE_KERNEL_OBJECT, LUID_AND_ATTRIBUTES,
            LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
        },
    };
    let current = RetainedProcess::open(unsafe { GetCurrentProcessId() }, Some(service_sid))?;
    if current.token.user.sid != "S-1-5-18"
        || current.token.user.session_id != 0
        || !current.token.service_sid_enabled
        || current.token.app_container
        || current.token.network
    {
        return Err(Error::ServiceIdentityRefused);
    }
    let mut token = ptr::null_mut();
    if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES | 0x00060000,
            &mut token,
        )
    } == 0
    {
        return Err(Error::Unavailable);
    }
    let token = Handle::owned(token)?;
    if unsafe {
        AdjustTokenPrivileges(
            token.raw(),
            1,
            ptr::null(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(Error::Unavailable);
    }
    let name: Vec<u16> = "SeChangeNotifyPrivilege\0".encode_utf16().collect();
    let mut luid = LUID::default();
    if unsafe { LookupPrivilegeValueW(ptr::null(), name.as_ptr(), &mut luid) } == 0 {
        return Err(Error::Unavailable);
    }
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    unsafe {
        SetLastError(0);
    }
    if unsafe {
        AdjustTokenPrivileges(
            token.raw(),
            0,
            &privileges,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    } == 0
        || unsafe { GetLastError() } != 0
    {
        return Err(Error::Unavailable);
    }
    super::security::NativeSecurity::host_token()?.seal_dacl(token.raw(), SE_KERNEL_OBJECT)?;
    super::security::inspect_host_token(token.raw())?;
    super::security::NativeSecurity::host_process()?
        .seal_dacl(unsafe { GetCurrentProcess() }, SE_KERNEL_OBJECT)?;
    super::security::inspect_host_process(unsafe { GetCurrentProcess() })?;
    Ok(())
}
