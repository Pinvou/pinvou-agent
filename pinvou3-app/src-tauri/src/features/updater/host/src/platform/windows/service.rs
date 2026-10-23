//! SCM-owned lifetime. Only authenticated local IPC carries business commands.
use super::{installed::InstalledServiceContext, pipe::Event, registry::BootstrapGuard, server};
use crate::{
    error::{Error, Result},
    profile::Seed,
};
use std::{
    ptr,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{ERROR_CALL_NOT_IMPLEMENTED, NO_ERROR},
    System::Services::*,
};

struct Status {
    handle: usize,
    value: SERVICE_STATUS,
}
struct Control {
    stop: Arc<Event>,
    status: Mutex<Option<Status>>,
    result: Mutex<Option<Result<()>>>,
}
static CONTROL: OnceLock<Arc<Control>> = OnceLock::new();
// Its private field is constructed only in the successful registered SCM
// callback. A CLI argument or parsed registration cannot construct this port.
pub(super) struct ScmDispatch {
    _private: (),
}
impl Control {
    #[allow(unsafe_code)]
    fn report(&self, state: u32, exit: u32) -> Result<()> {
        let mut slot = self.status.lock().map_err(|_| Error::Unavailable)?;
        let status = slot.as_mut().ok_or(Error::Unavailable)?;
        status.value.dwCurrentState = state;
        status.value.dwControlsAccepted = if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP
        } else {
            0
        };
        status.value.dwWin32ExitCode = exit;
        status.value.dwCheckPoint = if matches!(state, SERVICE_START_PENDING | SERVICE_STOP_PENDING)
        {
            status
                .value
                .dwCheckPoint
                .checked_add(1)
                .ok_or(Error::Unavailable)?
        } else {
            0
        };
        status.value.dwWaitHint = if matches!(state, SERVICE_START_PENDING | SERVICE_STOP_PENDING) {
            30_000
        } else {
            0
        };
        if unsafe { SetServiceStatus(status.handle as SERVICE_STATUS_HANDLE, &status.value) } == 0 {
            return Err(Error::Unavailable);
        }
        Ok(())
    }
    #[allow(unsafe_code)]
    fn interrogate(&self) -> Result<()> {
        let slot = self.status.lock().map_err(|_| Error::Unavailable)?;
        let status = slot.as_ref().ok_or(Error::Unavailable)?;
        if unsafe { SetServiceStatus(status.handle as SERVICE_STATUS_HANDLE, &status.value) } == 0 {
            return Err(Error::Unavailable);
        }
        Ok(())
    }
}
fn control_kind(control: u32) -> u8 {
    match control {
        SERVICE_CONTROL_STOP => 1,
        SERVICE_CONTROL_INTERROGATE => 2,
        _ => 0,
    }
}
#[allow(unsafe_code)]
unsafe extern "system" fn handler(
    control: u32,
    _event: u32,
    _data: *mut std::ffi::c_void,
    _context: *mut std::ffi::c_void,
) -> u32 {
    let Some(owner) = CONTROL.get() else {
        return ERROR_CALL_NOT_IMPLEMENTED;
    };
    match control_kind(control) {
        1 => {
            let _ = owner.report(SERVICE_STOP_PENDING, 0);
            match owner.stop.set() {
                Ok(()) => NO_ERROR,
                Err(_) => ERROR_CALL_NOT_IMPLEMENTED,
            }
        }
        2 => match owner.interrogate() {
            Ok(()) => NO_ERROR,
            Err(_) => ERROR_CALL_NOT_IMPLEMENTED,
        },
        // Including every 128..255 code: old pre-seal SC_HANDLE grants are
        // irrevocable, but they never become semantic business capabilities.
        _ => ERROR_CALL_NOT_IMPLEMENTED,
    }
}
fn run(owner: &Control) -> Result<()> {
    let seed = Seed::embedded()?;
    let _lifetime = BootstrapGuard::acquire(&seed, true)?;
    let complete = Arc::new(AtomicBool::new(false));
    let signal = complete.clone();
    let control = CONTROL.get().ok_or(Error::Unavailable)?.clone();
    let ticker = std::thread::Builder::new()
        .name("upgrade-start-checkpoint".into())
        .spawn(move || {
            while !signal.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_secs(1));
                if !signal.load(Ordering::Acquire) {
                    let _ = control.report(SERVICE_START_PENDING, 0);
                }
            }
        })
        .map_err(|_| Error::Unavailable)?;
    let context = InstalledServiceContext::from_scm_dispatch(&ScmDispatch { _private: () });
    complete.store(true, Ordering::Release);
    let joined = ticker.join();
    if joined.is_err() {
        return Err(Error::Unavailable);
    }
    let context = context?;
    if owner.stop.is_set()? {
        return Err(Error::RestartCanceled);
    }
    // QueryServiceStatusEx PID is valid only after RUNNING. There is still no
    // pipe; startup CAS and setup-only authentication follow this observation.
    owner.report(SERVICE_RUNNING, 0)?;
    let started = context.commit_startup()?;
    server::serve(started, owner.stop.clone())
}
#[allow(unsafe_code)]
unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
    let Some(owner) = CONTROL.get() else { return };
    let result = (|| {
        let seed = Seed::embedded()?;
        let name = wide(&seed.service_name());
        let handle =
            unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(handler), ptr::null()) };
        if handle.is_null() {
            return Err(Error::Unavailable);
        }
        let value = SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: SERVICE_START_PENDING,
            dwControlsAccepted: 0,
            dwWin32ExitCode: 0,
            dwServiceSpecificExitCode: 0,
            dwCheckPoint: 0,
            dwWaitHint: 30_000,
        };
        *owner.status.lock().map_err(|_| Error::Unavailable)? = Some(Status {
            handle: handle as usize,
            value,
        });
        owner.report(SERVICE_START_PENDING, 0)?;
        run(owner)
    })();
    let _ = owner.stop.set();
    // run()/serve() join all owned ticker/pipe workers before STOPPED.
    let _ = owner.report(SERVICE_STOPPED, if result.is_ok() { 0 } else { 1066 });
    if let Ok(mut slot) = owner.result.lock() {
        *slot = Some(result);
    }
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
#[allow(unsafe_code)]
pub(super) fn dispatch() -> Result<()> {
    let seed = Seed::embedded()?;
    let name = wide(&seed.service_name());
    let control = Arc::new(Control {
        stop: Arc::new(Event::new()?),
        status: Mutex::new(None),
        result: Mutex::new(None),
    });
    CONTROL.set(control.clone()).map_err(|_| Error::Busy)?;
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_ptr() as *mut u16,
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: ptr::null_mut(),
            lpServiceProc: None,
        },
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        return Err(Error::InstallerContextRefused);
    }
    control
        .result
        .lock()
        .map_err(|_| Error::Unavailable)?
        .take()
        .ok_or(Error::Unavailable)?
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_scm_custom_control_handles_never_select_business_actions() {
        for code in 128..=255 {
            assert_eq!(control_kind(code), 0);
        }
        assert_eq!(control_kind(SERVICE_CONTROL_INTERROGATE), 2);
        assert_eq!(control_kind(SERVICE_CONTROL_STOP), 1);
        assert_eq!(control_kind(SERVICE_CONTROL_PAUSE), 0);
        assert_eq!(control_kind(SERVICE_CONTROL_CONTINUE), 0);
    }
}
