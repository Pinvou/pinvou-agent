//! Earliest App writer barrier. Only an explicitly disabled embedded profile
//! bypasses the native host. Configured missing/corrupt/refused state is fatal.
use crate::{Error, error::Result, platform, profile::Seed};

/// A native authenticated startup admission for this actual process. The held
/// client/response remain private and cannot be supplied by a caller or DTO.
/// This is not a save, preparation ownership, freeze or installation grant.
#[must_use = "retain startup admission throughout the App runtime"]
pub struct AppWriterAdmission {
    _native: platform::AppWriterAdmission,
}

#[must_use = "complete the startup barrier before any App writer initialization"]
pub enum AppWriterStartup {
    /// Both embedded profile resources were deliberately absent at build time.
    DisabledProfile,
    Admitted(Box<AppWriterAdmission>),
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
    platform::admit_current_app_writer()
        .map(|native| AppWriterStartup::Admitted(Box::new(AppWriterAdmission { _native: native })))
}
