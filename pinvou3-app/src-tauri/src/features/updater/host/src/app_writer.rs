//! Earliest App writer barrier. Only an explicitly disabled embedded profile
//! bypasses the native host. Configured missing/corrupt/refused state is fatal.
use crate::{Error, error::Result, platform, profile::Seed};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// A native authenticated startup admission for this actual process. The held
/// client/response remain private and cannot be supplied by a caller or DTO.
/// This is not a save, preparation ownership, freeze or installation grant.
#[must_use = "retain startup admission throughout the App runtime"]
pub struct AppWriterAdmission {
    _native: Arc<platform::AppWriterAdmission>,
}

#[must_use = "complete the startup barrier before any App writer initialization"]
pub enum AppWriterStartup {
    /// Both embedded profile resources were deliberately absent at build time.
    DisabledProfile,
    Admitted(Box<AppWriterAdmission>),
}

/// Fixed CLI entry outcomes. Only a native authenticated admission can create
/// the Worker value. The frontend outcome is reserved for the service-owned
/// execution path; it conveys no writer authority.
#[must_use]
pub enum CliEntry {
    DisabledProfile,
    Worker(AppWriterAdmission),
    FrontendExit(i32),
}

/// A configured profile uses the authenticated native frontend or worker path.
/// Connection and identity failures remain failures, with no fallback.
pub fn enter_current_cli() -> Result<CliEntry> {
    match Seed::embedded() {
        Err(Error::ProfileDisabled) => return Ok(CliEntry::DisabledProfile),
        Err(other) => return Err(other),
        Ok(_) => {}
    }
    if !platform::is_current_cli_worker()? {
        return platform::run_current_cli_frontend().map(CliEntry::FrontendExit);
    }
    match admit_current_cli_writer()? {
        AppWriterStartup::DisabledProfile => Ok(CliEntry::DisabledProfile),
        AppWriterStartup::Admitted(admission) => {
            platform::start_current_cli_worker(&admission._native)?;
            Ok(CliEntry::Worker(*admission))
        }
    }
}

/// Explicit worker completion before process exit. Disabled profiles retain
/// the ordinary CLI exit code without creating any native writer state.
pub fn finish_current_cli(code: i32) -> Result<i32> {
    match Seed::embedded() {
        Err(Error::ProfileDisabled) => Ok(code),
        Err(other) => Err(other),
        Ok(_) => platform::finish_current_cli_worker(code),
    }
}

/// Synchronous and read-only in the App until the authenticated service performs
/// its actual admission protocol. No service start/install, generic PID/path,
/// capability downgrade, reconnect retry or ignored refusal is provided here.
pub fn admit_current_app_writer() -> Result<AppWriterStartup> {
    match Seed::embedded() {
        Err(Error::ProfileDisabled) => return Ok(AppWriterStartup::DisabledProfile),
        Err(other) => return Err(other),
        Ok(_) => {}
    }
    platform::admit_current_app_writer().map(|native| {
        AppWriterStartup::Admitted(Box::new(AppWriterAdmission {
            _native: Arc::new(native),
        }))
    })
}

/// Admission for the authenticated installed CLI, retained for its full work.
pub fn admit_current_cli_writer() -> Result<AppWriterStartup> {
    match Seed::embedded() {
        Err(Error::ProfileDisabled) => return Ok(AppWriterStartup::DisabledProfile),
        Err(other) => return Err(other),
        Ok(_) => {}
    }
    static CURRENT: OnceLock<Mutex<Weak<platform::AppWriterAdmission>>> = OnceLock::new();
    let mut current = CURRENT
        .get_or_init(|| Mutex::new(Weak::new()))
        .lock()
        .map_err(|_| Error::HandoffOutcomeUnknown)?;
    let native = match current.upgrade() {
        Some(native) => {
            // Reuse only the actual still-held authenticated native session.
            // A stale service incarnation is a refusal, not a fresh grant.
            native.recheck()?;
            native
        }
        None => {
            let native = Arc::new(platform::admit_current_cli_writer()?);
            *current = Arc::downgrade(&native);
            native
        }
    };
    Ok(AppWriterStartup::Admitted(Box::new(AppWriterAdmission {
        _native: native,
    })))
}
