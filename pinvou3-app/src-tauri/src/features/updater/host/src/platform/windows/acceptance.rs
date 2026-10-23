//! Two deterministic guest-only crash boundaries. No runtime switch exists.
//! Shipping builds contain no event lookup/wait. This does not create a host
//! capability or make an instrumented profile a shipping acceptance result.
use super::source::SourceInstallerContext as Source;
use crate::error::Result;
#[derive(Clone, Copy)]
pub(super) enum Stage {
    ServiceCreated,
    ServiceSealed,
}
#[cfg(not(feature = "native-acceptance-hooks"))]
pub(super) fn checkpoint(source: &Source, _: Stage) -> Result<()> {
    source.recheck()
}
#[cfg(feature = "native-acceptance-hooks")]
#[allow(unsafe_code)]
pub(super) fn checkpoint(source: &Source, stage: Stage) -> Result<()> {
    use super::{handle::Handle, security};
    use crate::error::Error;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, GetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Security::Authorization::SE_KERNEL_OBJECT,
        System::Threading::{
            EVENT_MODIFY_STATE, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
            WaitForSingleObject,
        },
    };
    source.recheck()?;
    if source.seed.profile_kind != "lab" || !source.seed.profile_id.contains("-fault-") {
        return Err(Error::SecurityRefused);
    }
    let stage = match stage {
        Stage::ServiceCreated => "service-created",
        Stage::ServiceSealed => "service-sealed",
    };
    let base = format!(
        "Global\\Pinvou.Host.Acceptance.{}.{}.{}",
        source.seed.profile_id, source.seed.build_id, stage
    );
    let open = |suffix: &str, access: u32| -> Result<Option<Handle>> {
        let name: Vec<u16> = format!("{base}.{suffix}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let raw = unsafe { OpenEventW(access | 0x00020000, 0, name.as_ptr()) };
        if raw.is_null() {
            return if unsafe { GetLastError() } == ERROR_FILE_NOT_FOUND {
                Ok(None)
            } else {
                Err(Error::SecurityRefused)
            };
        }
        let event = Handle::owned(raw)?;
        security::inspect_kernel(event.raw(), SE_KERNEL_OBJECT)?;
        Ok(Some(event))
    };
    let reached = open("Reached", EVENT_MODIFY_STATE)?;
    let release = open("Release", SYNCHRONIZATION_SYNCHRONIZE)?;
    let (reached, release) = match (reached, release) {
        (None, None) => return source.recheck(),
        (Some(a), Some(b)) => (a, b),
        _ => return Err(Error::SecurityRefused),
    };
    // A leftover signaled Release cannot silently turn a later crash probe into
    // a normal pass. Harness must create a fresh unsignaled pair each time.
    if unsafe { WaitForSingleObject(release.raw(), 0) } != WAIT_TIMEOUT {
        return Err(Error::SecurityRefused);
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    if unsafe { SetEvent(reached.raw()) } == 0 {
        return Err(Error::Unavailable);
    }
    loop {
        source.recheck()?;
        if Instant::now() >= deadline {
            return Err(Error::HandoffOutcomeUnknown);
        }
        match unsafe { WaitForSingleObject(release.raw(), 100) } {
            WAIT_OBJECT_0 => {
                if Instant::now() >= deadline {
                    return Err(Error::HandoffOutcomeUnknown);
                }
                return source.recheck();
            }
            WAIT_TIMEOUT => {}
            _ => return Err(Error::HandoffOutcomeUnknown),
        }
    }
}
