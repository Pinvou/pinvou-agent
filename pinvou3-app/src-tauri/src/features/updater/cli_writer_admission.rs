//! CLI and headless writer admission at their public entry points.
//! The value retains native custody for the entire guarded operation.

#[must_use = "retain admission until all guarded work has ended"]
pub struct CliWriterAdmission {
    #[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
    _native: pinvou_upgrade_host::AppWriterStartup,
}

/// The CLI binary either continues as the authenticated worker, returns the
/// supervised worker's exit status, or uses an explicitly disabled profile.
pub enum CliMainEntry {
    Worker(CliWriterAdmission),
    FrontendExit(i32),
    DisabledProfile,
}

/// Enter through the native launcher. It reads the current process itself;
/// no caller-supplied process identifier or handle can construct a worker.
pub fn enter_current_cli() -> Result<CliMainEntry, &'static str> {
    #[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
    {
        match pinvou_upgrade_host::enter_current_cli().map_err(pinvou_upgrade_host::Error::code)? {
            pinvou_upgrade_host::CliEntry::Worker(native) => {
                Ok(CliMainEntry::Worker(CliWriterAdmission {
                    _native: pinvou_upgrade_host::AppWriterStartup::Admitted(Box::new(native)),
                }))
            }
            pinvou_upgrade_host::CliEntry::FrontendExit(code) => {
                Ok(CliMainEntry::FrontendExit(code))
            }
            pinvou_upgrade_host::CliEntry::DisabledProfile => Ok(CliMainEntry::DisabledProfile),
        }
    }
    #[cfg(not(any(feature = "upgrade-host-lab", feature = "upgrade-host-production")))]
    {
        Ok(CliMainEntry::DisabledProfile)
    }
}

/// Finish the authenticated worker's output before terminating the process.
pub fn finish_current_cli(code: i32) -> Result<i32, &'static str> {
    #[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
    {
        pinvou_upgrade_host::finish_current_cli(code).map_err(pinvou_upgrade_host::Error::code)
    }
    #[cfg(not(any(feature = "upgrade-host-lab", feature = "upgrade-host-production")))]
    {
        Ok(code)
    }
}

/// Authenticate the actual CLI process before it may initialize writers.
/// Only an explicitly unconfigured build/profile may run without the host.
pub fn admit_current_cli_writer() -> Result<CliWriterAdmission, &'static str> {
    #[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
    {
        pinvou_upgrade_host::admit_current_cli_writer()
            .map(|native| CliWriterAdmission { _native: native })
            .map_err(pinvou_upgrade_host::Error::code)
    }
    #[cfg(not(any(feature = "upgrade-host-lab", feature = "upgrade-host-production")))]
    {
        Ok(CliWriterAdmission {})
    }
}

/// Read only the compiled public profile for build-time identity checks.
/// This reports neither a running host nor permission to write data.
pub fn compiled_cli_profile() -> Result<Vec<u8>, &'static str> {
    #[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
    {
        pinvou_upgrade_host::compiled_profile_diagnostic().map_err(pinvou_upgrade_host::Error::code)
    }
    #[cfg(not(any(feature = "upgrade-host-lab", feature = "upgrade-host-production")))]
    {
        Err("HOST_PROFILE_DISABLED")
    }
}
