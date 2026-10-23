//! Exact fixed LocalSystem SCM object; installation-only mutators are private.
//! CreateService cannot receive an SD: the two-nonauthor-passed quarantine
//! protocol creates DISABLED/STOPPED, inspects default security, seals it, THEN
//! enables. Old granted user-control handles remain harmless in runtime handler.
use super::{
    layout::{Layout, ordinal_equal},
    process::RetainedProcess,
    security,
};
use crate::{
    error::{Error, Result},
    profile::Seed,
};
use std::{mem::size_of, ptr};
use windows_sys::Win32::{
    Foundation::{
        ERROR_INSUFFICIENT_BUFFER, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DOES_NOT_EXIST,
        ERROR_SERVICE_NOT_ACTIVE, GetLastError,
    },
    Security::LookupAccountNameW,
    System::Services::*,
};
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
struct ServiceHandle(SC_HANDLE);
impl Drop for ServiceHandle {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}
#[allow(unsafe_code)]
fn manager(create: bool) -> Result<ServiceHandle> {
    let handle = unsafe {
        OpenSCManagerW(
            ptr::null(),
            ptr::null(),
            SC_MANAGER_CONNECT | if create { SC_MANAGER_CREATE_SERVICE } else { 0 },
        )
    };
    if handle.is_null() {
        Err(Error::Unavailable)
    } else {
        Ok(ServiceHandle(handle))
    }
}
#[allow(unsafe_code)]
fn open(manager: &ServiceHandle, seed: &Seed, mutate: bool) -> Result<Option<ServiceHandle>> {
    let name = wide(&seed.service_name());
    let rights = SERVICE_QUERY_CONFIG
        | SERVICE_QUERY_STATUS
        | 0x00020000
        | if mutate {
            SERVICE_CHANGE_CONFIG | SERVICE_START | SERVICE_STOP | 0x00010000 | 0x00040000
        } else {
            0
        };
    let handle = unsafe { OpenServiceW(manager.0, name.as_ptr(), rights) };
    if handle.is_null() {
        if unsafe { GetLastError() } == ERROR_SERVICE_DOES_NOT_EXIST {
            Ok(None)
        } else {
            Err(Error::Unavailable)
        }
    } else {
        Ok(Some(ServiceHandle(handle)))
    }
}
struct Config {
    storage: Vec<usize>,
    len: usize,
}
impl Config {
    #[allow(unsafe_code)]
    fn get(service: &ServiceHandle) -> Result<Self> {
        let mut needed = 0;
        if unsafe { QueryServiceConfigW(service.0, ptr::null_mut(), 0, &mut needed) } != 0
            || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
            || (needed as usize) < size_of::<QUERY_SERVICE_CONFIGW>()
            || needed > 65536
        {
            return Err(Error::ServiceIdentityRefused);
        }
        let mut storage = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
        let capacity = storage.len() * size_of::<usize>();
        if unsafe {
            QueryServiceConfigW(
                service.0,
                storage.as_mut_ptr().cast(),
                capacity as u32,
                &mut needed,
            )
        } == 0
            || needed as usize > capacity
        {
            return Err(Error::ServiceIdentityRefused);
        }
        Ok(Self {
            storage,
            len: needed as usize,
        })
    }
    #[allow(unsafe_code)]
    fn fields(&self) -> &QUERY_SERVICE_CONFIGW {
        unsafe { &*self.storage.as_ptr().cast() }
    }
    #[allow(unsafe_code)]
    fn text(&self, pointer: *const u16, nullable: bool) -> Result<String> {
        if pointer.is_null() {
            return if nullable {
                Ok(String::new())
            } else {
                Err(Error::ServiceIdentityRefused)
            };
        }
        let start = self.storage.as_ptr() as usize;
        let end = start + self.len;
        let p = pointer as usize;
        if p < start || p >= end || p % 2 != 0 {
            return Err(Error::ServiceIdentityRefused);
        }
        let max = (end - p) / 2;
        let mut len = 0;
        while len < max && unsafe { *pointer.add(len) } != 0 {
            len += 1;
        }
        if len == max {
            return Err(Error::ServiceIdentityRefused);
        }
        String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, len) })
            .map_err(|_| Error::ServiceIdentityRefused)
    }
    fn validate(&self, seed: &Seed, layout: &Layout, disabled: bool) -> Result<()> {
        let f = self.fields();
        let expected = format!(
            "\"{}\" --service",
            layout
                .helper_path(seed)
                .to_str()
                .ok_or(Error::ServiceIdentityRefused)?
        );
        if f.dwServiceType != SERVICE_WIN32_OWN_PROCESS
            || f.dwStartType
                != if disabled {
                    SERVICE_DISABLED
                } else {
                    SERVICE_AUTO_START
                }
            || f.dwErrorControl != SERVICE_ERROR_NORMAL
            || self.text(f.lpBinaryPathName, false)? != expected
            || !ordinal_equal(&self.text(f.lpServiceStartName, false)?, "LocalSystem")?
            || !self.text(f.lpLoadOrderGroup, true)?.is_empty()
            || !self.text(f.lpDependencies, true)?.is_empty()
            || self.text(f.lpDisplayName, false)? != seed.service_name()
        {
            return Err(Error::ServiceIdentityRefused);
        }
        Ok(())
    }
}
#[allow(unsafe_code)]
fn query_status(service: &ServiceHandle) -> Result<SERVICE_STATUS_PROCESS> {
    let mut value = SERVICE_STATUS_PROCESS::default();
    let mut size = 0;
    if unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            (&mut value as *mut SERVICE_STATUS_PROCESS).cast(),
            size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut size,
        )
    } == 0
        || size as usize != size_of::<SERVICE_STATUS_PROCESS>()
    {
        return Err(Error::ServiceIdentityRefused);
    }
    Ok(value)
}
fn status(service: &ServiceHandle) -> Result<SERVICE_STATUS_PROCESS> {
    let value = query_status(service)?;
    if value.dwServiceType != SERVICE_WIN32_OWN_PROCESS {
        return Err(Error::ServiceIdentityRefused);
    }
    Ok(value)
}

#[allow(unsafe_code)]
fn sid_type(service: &ServiceHandle) -> Result<u32> {
    let mut sid = SERVICE_SID_INFO::default();
    let mut needed = 0;
    if unsafe {
        QueryServiceConfig2W(
            service.0,
            SERVICE_CONFIG_SERVICE_SID_INFO,
            (&mut sid as *mut SERVICE_SID_INFO).cast(),
            size_of::<SERVICE_SID_INFO>() as u32,
            &mut needed,
        )
    } == 0
        || needed as usize != size_of::<SERVICE_SID_INFO>()
    {
        return Err(Error::ServiceIdentityRefused);
    }
    Ok(sid.dwServiceSidType)
}
#[allow(unsafe_code)]
pub(super) fn resolve_service_sid(seed: &Seed) -> Result<String> {
    let name = wide(&format!("NT SERVICE\\{}", seed.service_name()));
    let mut sid_size = 0;
    let mut domain_size = 0;
    let mut kind = 0;
    if unsafe {
        LookupAccountNameW(
            ptr::null(),
            name.as_ptr(),
            ptr::null_mut(),
            &mut sid_size,
            ptr::null_mut(),
            &mut domain_size,
            &mut kind,
        )
    } != 0
        || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
        || !(8..=68).contains(&sid_size)
        || domain_size > 256
    {
        return Err(Error::ServiceIdentityRefused);
    }
    let mut sid = vec![0usize; (sid_size as usize).div_ceil(size_of::<usize>())];
    let mut domain = vec![0u16; domain_size as usize];
    if unsafe {
        LookupAccountNameW(
            ptr::null(),
            name.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut sid_size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut kind,
        )
    } == 0
        || sid_size as usize > sid.len() * size_of::<usize>()
    {
        return Err(Error::ServiceIdentityRefused);
    }
    let value = security::sid_string(sid.as_mut_ptr().cast())?;
    let parts: Vec<_> = value.split('-').collect();
    if parts.len() != 9
        || !value.starts_with("S-1-5-80-")
        || parts[4..].iter().any(|p| p.parse::<u32>().is_err())
    {
        return Err(Error::ServiceIdentityRefused);
    }
    Ok(value)
}
/// Read-only discovery is permitted before intent. It never creates/adopts.
pub(super) fn exists(seed: &Seed) -> Result<bool> {
    Ok(open(&manager(false)?, seed, false)?.is_some())
}

/// Called only with original service-create-intent, exact verified target
/// images and native source-installer binding. Not an IPC operation.
#[allow(unsafe_code)]
pub(super) fn configure_original(
    seed: &Seed,
    layout: &Layout,
    may_resume_original: bool,
    checkpoint: impl Fn(super::acceptance::Stage) -> Result<()>,
) -> Result<String> {
    let manager = manager(true)?;
    let service = match open(&manager, seed, true)? {
        Some(service) if may_resume_original => service,
        Some(_) => return Err(Error::ScopeConflict),
        None => {
            let name = wide(&seed.service_name());
            let image = wide(&format!(
                "\"{}\" --service",
                layout
                    .helper_path(seed)
                    .to_str()
                    .ok_or(Error::ServiceIdentityRefused)?
            ));
            let handle = unsafe {
                CreateServiceW(
                    manager.0,
                    name.as_ptr(),
                    name.as_ptr(),
                    SERVICE_QUERY_CONFIG
                        | SERVICE_QUERY_STATUS
                        | SERVICE_CHANGE_CONFIG
                        | SERVICE_START
                        | SERVICE_STOP
                        | 0x00070000,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_DISABLED,
                    SERVICE_ERROR_NORMAL,
                    image.as_ptr(),
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                )
            };
            if handle.is_null() {
                return Err(Error::Unavailable);
            }
            let service = ServiceHandle(handle);
            checkpoint(super::acceptance::Stage::ServiceCreated)?;
            service
        }
    };
    let state = status(&service)?;
    if state.dwCurrentState != SERVICE_STOPPED {
        return Err(Error::Busy);
    }
    let config = Config::get(&service)?;
    if config.fields().dwStartType == SERVICE_DISABLED {
        config.validate(seed, layout, true)?;
        security::inspect_service_quarantine(service.0.cast())?;
        security::seal_service(service.0.cast())?;
        checkpoint(super::acceptance::Stage::ServiceSealed)?;
    } else {
        // Only this original previously recorded creation can resume, and an
        // enabled existing object must ALREADY have the exact sealed policy.
        config.validate(seed, layout, false)?;
        security::inspect_service(service.0.cast())?;
    }
    let sid = SERVICE_SID_INFO {
        dwServiceSidType: SERVICE_SID_TYPE_UNRESTRICTED,
    };
    if unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_SERVICE_SID_INFO,
            (&sid as *const SERVICE_SID_INFO).cast(),
        )
    } == 0
    {
        return Err(Error::Unavailable);
    }
    let mut privileges = wide("SeChangeNotifyPrivilege");
    privileges.push(0);
    let privilege_info = SERVICE_REQUIRED_PRIVILEGES_INFOW {
        pmszRequiredPrivileges: privileges.as_mut_ptr(),
    };
    if unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO,
            (&privilege_info as *const SERVICE_REQUIRED_PRIVILEGES_INFOW).cast(),
        )
    } == 0
    {
        return Err(Error::Unavailable);
    }
    if unsafe {
        ChangeServiceConfigW(
            service.0,
            SERVICE_NO_CHANGE,
            SERVICE_AUTO_START,
            SERVICE_NO_CHANGE,
            ptr::null(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
        )
    } == 0
    {
        return Err(Error::Unavailable);
    }
    Config::get(&service)?.validate(seed, layout, false)?;
    security::inspect_service(service.0.cast())?;
    if sid_type(&service)? != SERVICE_SID_TYPE_UNRESTRICTED {
        return Err(Error::ServiceIdentityRefused);
    }
    verify_privileges(&service)?;
    resolve_service_sid(seed)
}
#[allow(unsafe_code)]
fn verify_privileges(service: &ServiceHandle) -> Result<()> {
    let mut needed = 0;
    if unsafe {
        QueryServiceConfig2W(
            service.0,
            SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO,
            ptr::null_mut(),
            0,
            &mut needed,
        )
    } != 0
        || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
        || (needed as usize) < size_of::<SERVICE_REQUIRED_PRIVILEGES_INFOW>()
        || needed > 65536
    {
        return Err(Error::ServiceIdentityRefused);
    }
    let mut words = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    let capacity = words.len() * size_of::<usize>();
    if unsafe {
        QueryServiceConfig2W(
            service.0,
            SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO,
            words.as_mut_ptr().cast(),
            capacity as u32,
            &mut needed,
        )
    } == 0
        || needed as usize > capacity
    {
        return Err(Error::ServiceIdentityRefused);
    }
    let info = unsafe { &*words.as_ptr().cast::<SERVICE_REQUIRED_PRIVILEGES_INFOW>() };
    let p = info.pmszRequiredPrivileges as usize;
    let start = words.as_ptr() as usize;
    if p < start || p >= start + needed as usize || p % 2 != 0 {
        return Err(Error::ServiceIdentityRefused);
    }
    let expected: Vec<u16> = "SeChangeNotifyPrivilege\0\0".encode_utf16().collect();
    if p + expected.len() * 2 > start + needed as usize
        || unsafe { std::slice::from_raw_parts(info.pmszRequiredPrivileges, expected.len()) }
            != expected
    {
        return Err(Error::ServiceIdentityRefused);
    }
    Ok(())
}
#[allow(unsafe_code)]
pub(super) fn start(seed: &Seed, layout: &Layout) -> Result<()> {
    let service = open(&manager(false)?, seed, true)?.ok_or(Error::ServiceIdentityRefused)?;
    Config::get(&service)?.validate(seed, layout, false)?;
    security::inspect_service(service.0.cast())?;
    if sid_type(&service)? != SERVICE_SID_TYPE_UNRESTRICTED {
        return Err(Error::ServiceIdentityRefused);
    }
    verify_privileges(&service)?;
    if unsafe { StartServiceW(service.0, 0, ptr::null()) } == 0
        && unsafe { GetLastError() } != ERROR_SERVICE_ALREADY_RUNNING
    {
        return Err(Error::Unavailable);
    }
    Ok(())
}
/// Fixed query-only client check, not a service-owned GUI launch.
pub(super) fn live_process(seed: &Seed, layout: &Layout) -> Result<RetainedProcess> {
    let service = open(&manager(false)?, seed, false)?.ok_or(Error::ServiceIdentityRefused)?;
    Config::get(&service)?.validate(seed, layout, false)?;
    security::inspect_service(service.0.cast())?;
    if sid_type(&service)? != SERVICE_SID_TYPE_UNRESTRICTED {
        return Err(Error::ServiceIdentityRefused);
    }
    verify_privileges(&service)?;
    let process_id = running_process_id(&status(&service)?)?;
    let sid = resolve_service_sid(seed)?;
    let process = RetainedProcess::open(process_id, Some(&sid))?;
    if process.token.user.sid != "S-1-5-18"
        || !process.token.service_sid_enabled
        || process.token.app_container
        || !ordinal_equal(
            process
                .image
                .to_str()
                .ok_or(Error::ServiceIdentityRefused)?,
            layout
                .helper_path(seed)
                .to_str()
                .ok_or(Error::ServiceIdentityRefused)?,
        )?
    {
        return Err(Error::ServiceIdentityRefused);
    }
    Ok(process)
}
fn running_process_id(state: &SERVICE_STATUS_PROCESS) -> Result<u32> {
    // QueryServiceStatusEx explicitly does not promise a valid PID during
    // START_PENDING. SCM RUNNING only means this process may be authenticated;
    // bootstrap capabilities still require configured/probe/native-ready CAS.
    if state.dwCurrentState != SERVICE_RUNNING {
        return Err(Error::SetupIncomplete);
    }
    if state.dwProcessId == 0 {
        return Err(Error::ServiceIdentityRefused);
    }
    Ok(state.dwProcessId)
}

fn original_stopped_mode(state: u32, start_type: u32, creation_provenance: bool) -> Result<bool> {
    if !creation_provenance {
        return Err(Error::ServiceIdentityRefused);
    }
    if state != SERVICE_STOPPED {
        return Err(Error::Referenced);
    }
    match start_type {
        SERVICE_DISABLED => Ok(true),
        SERVICE_AUTO_START => Ok(false),
        _ => Err(Error::ServiceIdentityRefused),
    }
}
fn validate_original_stopped(
    service: &ServiceHandle,
    seed: &Seed,
    layout: &Layout,
    creation_provenance: bool,
) -> Result<()> {
    let state = status(service)?;
    let config = Config::get(service)?;
    if original_stopped_mode(
        state.dwCurrentState,
        config.fields().dwStartType,
        creation_provenance,
    )? {
        config.validate(seed, layout, true)?;
        security::inspect_service_quarantine(service.0.cast())?;
        // This object is still disabled. SID/privilege setup may not have run;
        // only configure_original may finish sealing/configuration before AUTO.
    } else {
        config.validate(seed, layout, false)?;
        security::inspect_service(service.0.cast())?;
        if sid_type(service)? != SERVICE_SID_TYPE_UNRESTRICTED {
            return Err(Error::ServiceIdentityRefused);
        }
        verify_privileges(service)?;
    }
    Ok(())
}
/// Read-only interrupted original creation classification. A parsed record is
/// never sufficient: sole native Source caller proves original creation scope.
pub(super) fn original_stopped(
    seed: &Seed,
    layout: &Layout,
    creation_provenance: bool,
) -> Result<bool> {
    let Some(service) = open(&manager(false)?, seed, false)? else {
        return Ok(true);
    };
    if !creation_provenance {
        return Err(Error::ServiceIdentityRefused);
    }
    if status(&service)?.dwCurrentState != SERVICE_STOPPED {
        return Ok(false);
    }
    validate_original_stopped(&service, seed, layout, creation_provenance)?;
    Ok(true)
}
#[allow(unsafe_code)]
pub(super) fn request_stop(seed: &Seed, layout: &Layout, creation_provenance: bool) -> Result<()> {
    let Some(service) = open(&manager(false)?, seed, true)? else {
        return Ok(());
    };
    if !creation_provenance {
        return Err(Error::ServiceIdentityRefused);
    }
    if status(&service)?.dwCurrentState == SERVICE_STOPPED {
        return validate_original_stopped(&service, seed, layout, creation_provenance);
    }
    Config::get(&service)?.validate(seed, layout, false)?;
    security::inspect_service(service.0.cast())?;
    let mut state = SERVICE_STATUS::default();
    if unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut state) } == 0
        && unsafe { GetLastError() } != ERROR_SERVICE_NOT_ACTIVE
    {
        return Err(Error::Unavailable);
    }
    Ok(())
}
pub(super) fn stopped(seed: &Seed) -> Result<bool> {
    let Some(service) = open(&manager(false)?, seed, false)? else {
        return Ok(true);
    };
    Ok(status(&service)?.dwCurrentState == SERVICE_STOPPED)
}
#[allow(unsafe_code)]
pub(super) fn remove_stopped(
    seed: &Seed,
    layout: &Layout,
    creation_provenance: bool,
) -> Result<()> {
    let Some(service) = open(&manager(false)?, seed, true)? else {
        return Ok(());
    };
    if !creation_provenance {
        return Err(Error::ServiceIdentityRefused);
    }
    validate_original_stopped(&service, seed, layout, creation_provenance)?;
    if unsafe { DeleteService(service.0) } == 0 {
        return Err(Error::Unavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[allow(unsafe_code)]
    fn actual_os_service_queries_prove_sdk_buffer_extents_only() {
        let manager = manager(false).unwrap();
        let name = wide("EventLog");
        let handle = unsafe {
            OpenServiceW(
                manager.0,
                name.as_ptr(),
                SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS,
            )
        };
        assert!(!handle.is_null());
        let service = ServiceHandle(handle);
        let config = Config::get(&service).unwrap();
        assert!(
            !config
                .text(config.fields().lpBinaryPathName, false)
                .unwrap()
                .is_empty()
        );
        query_status(&service).unwrap();
        sid_type(&service).unwrap();
        // Built-in OS QUERY ONLY. This does not install, change, stop, start,
        // qualify or adopt this service as a Pinvou host or trusted profile.
    }
    #[test]
    fn interrupted_original_creation_is_only_disabled_stopped_with_creation_provenance() {
        assert_eq!(
            original_stopped_mode(SERVICE_STOPPED, SERVICE_DISABLED, true),
            Ok(true)
        );
        assert_eq!(
            original_stopped_mode(SERVICE_STOPPED, SERVICE_AUTO_START, true),
            Ok(false)
        );
        assert_eq!(
            original_stopped_mode(SERVICE_STOPPED, SERVICE_DISABLED, false),
            Err(Error::ServiceIdentityRefused)
        );
        assert_eq!(
            original_stopped_mode(SERVICE_STOPPED, SERVICE_AUTO_START, false),
            Err(Error::ServiceIdentityRefused)
        );
        for state in [SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STOP_PENDING] {
            assert_eq!(
                original_stopped_mode(state, SERVICE_DISABLED, true),
                Err(Error::Referenced)
            );
        }
        assert_eq!(
            original_stopped_mode(SERVICE_STOPPED, SERVICE_DEMAND_START, true),
            Err(Error::ServiceIdentityRefused)
        );
    }
    #[test]
    fn only_running_state_has_a_pid_usable_by_this_profile() {
        for current in [
            SERVICE_START_PENDING,
            SERVICE_STOP_PENDING,
            SERVICE_STOPPED,
            SERVICE_PAUSED,
            SERVICE_PAUSE_PENDING,
            SERVICE_CONTINUE_PENDING,
            0,
            u32::MAX,
        ] {
            let state = SERVICE_STATUS_PROCESS {
                dwCurrentState: current,
                dwProcessId: 123,
                ..Default::default()
            };
            assert!(matches!(
                running_process_id(&state),
                Err(Error::SetupIncomplete)
            ));
        }
        let mut state = SERVICE_STATUS_PROCESS {
            dwCurrentState: SERVICE_RUNNING,
            dwProcessId: 123,
            ..Default::default()
        };
        assert_eq!(running_process_id(&state).unwrap(), 123);
        state.dwProcessId = 0;
        assert!(matches!(
            running_process_id(&state),
            Err(Error::ServiceIdentityRefused)
        ));
    }
}
