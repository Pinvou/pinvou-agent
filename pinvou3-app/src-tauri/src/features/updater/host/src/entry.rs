//! Closed process entry contracts. Arguments select semantics, never paths,
//! trust inputs, arbitrary commands, credentials, identities or native handles.
use crate::{Error, error::Result};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HelperMode {
    Profile,
    InstallLocal,
    InspectInstalledImages,
    Service,
    PrepareInstall,
    PrepareLocalInstall,
    PrepareMetadataUpgrade,
    VerifyFiles,
    StartService,
    PrepareRemove,
    RemoveTarget,
    LaunchDesktop,
}
fn arguments(values: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Vec<String>> {
    let mut result = Vec::new();
    for value in values {
        if result.len() >= 2 {
            return Err(Error::IpcInvalid);
        }
        let value = value.into_string().map_err(|_| Error::IpcInvalid)?;
        if value.len() > 128 || value.contains('\0') {
            return Err(Error::IpcInvalid);
        }
        result.push(value);
    }
    Ok(result)
}
fn helper(values: &[String]) -> Result<HelperMode> {
    match values
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--profile"] => Ok(HelperMode::Profile),
        ["--install-local"] => Ok(HelperMode::InstallLocal),
        ["--inspect-installed-images"] => Ok(HelperMode::InspectInstalledImages),
        ["--service"] => Ok(HelperMode::Service),
        ["--installer-stage", stage] => match *stage {
            "prepare-install" => Ok(HelperMode::PrepareInstall),
            "prepare-local-install" => Ok(HelperMode::PrepareLocalInstall),
            "prepare-metadata-upgrade" => Ok(HelperMode::PrepareMetadataUpgrade),
            "verify-files" => Ok(HelperMode::VerifyFiles),
            "start-service" => Ok(HelperMode::StartService),
            "prepare-remove" => Ok(HelperMode::PrepareRemove),
            "remove-target" => Ok(HelperMode::RemoveTarget),
            "launch-desktop" => Ok(HelperMode::LaunchDesktop),
            _ => Err(Error::IpcInvalid),
        },
        _ => Err(Error::IpcInvalid),
    }
}
pub(crate) fn exit_code(error: Error) -> u8 {
    match error {
        Error::Unsupported
        | Error::ProfileDisabled
        | Error::InitialTrustUnavailable
        | Error::OperationUnsupported => 10,
        Error::IpcInvalid
        | Error::IpcConflict
        | Error::InstallerContextRefused
        | Error::ScopeConflict
        | Error::SecurityRefused
        | Error::ServiceIdentityRefused
        | Error::PeerRefused
        | Error::ImageRefused
        | Error::RestartConflict => 11,
        Error::Busy | Error::Referenced => 12,
        Error::StateCorrupt | Error::RepairRequired => 13,
        Error::StateCommitUnknown | Error::HandoffOutcomeUnknown => 14,
        _ => 15,
    }
}
fn finish(result: Result<()>) -> std::process::ExitCode {
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::from(exit_code(error))
        }
    }
}
pub fn helper_main(values: impl IntoIterator<Item = std::ffi::OsString>) -> std::process::ExitCode {
    finish((|| {
        let mode = helper(&arguments(values)?)?;
        crate::platform::harden_role_process()?;
        if mode == HelperMode::InstallLocal {
            // The native function has returned and released every package,
            // process and file owner before this exact original Win32 status.
            let original_status = crate::platform::install_local()?;
            std::process::exit(original_status as i32);
        }
        if mode == HelperMode::Profile {
            use std::io::Write;
            let bytes = crate::compiled_profile_diagnostic()?;
            std::io::stdout()
                .lock()
                .write_all(&bytes)
                .map_err(|_| Error::Unavailable)?;
            return Ok(());
        }
        if mode == HelperMode::InspectInstalledImages {
            use std::io::Write;
            // The observation stays alive while its ordinary diagnostic bytes
            // are written; stdout itself never holds a file/process lease.
            let observation = crate::inspect_installed_images(|| false)?;
            let bytes = observation.diagnostic_json()?;
            std::io::stdout()
                .lock()
                .write_all(&bytes)
                .map_err(|_| Error::Unavailable)?;
            return Ok(());
        }
        crate::platform::execute_helper(mode)
    })())
}
pub fn launcher_main(
    values: impl IntoIterator<Item = std::ffi::OsString>,
) -> std::process::ExitCode {
    finish((|| {
        let values = arguments(values)?;
        let id = match values
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            [] => None,
            ["--restart-handoff", id] if crate::json::digest(id) => Some((*id).to_owned()),
            _ => return Err(Error::IpcInvalid),
        };
        crate::platform::harden_role_process()?;
        crate::platform::execute_launcher(id.as_deref())
    })())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn role_arguments_are_closed_before_native_effects() {
        assert_eq!(
            helper(&["--installer-stage".into(), "prepare-install".into()]),
            Ok(HelperMode::PrepareInstall)
        );
        assert_eq!(
            helper(&["--installer-stage".into(), "prepare-local-install".into()]),
            Ok(HelperMode::PrepareLocalInstall)
        );
        assert_eq!(
            helper(&[
                "--installer-stage".into(),
                "prepare-metadata-upgrade".into()
            ]),
            Ok(HelperMode::PrepareMetadataUpgrade)
        );
        assert_eq!(
            helper(&["--install-local".into()]),
            Ok(HelperMode::InstallLocal)
        );
        assert_eq!(
            helper(&["--inspect-installed-images".into()]),
            Ok(HelperMode::InspectInstalledImages)
        );
        for values in [
            vec!["--inspect-installed-images".into(), r"C:\override".into()],
            vec!["--inspect-installed-images".into(), "--repair".into()],
            vec!["--inspect-installed-images=production".into()],
            vec![
                "--installer-stage".into(),
                "inspect-installed-images".into(),
            ],
        ] {
            assert_eq!(helper(&values), Err(Error::IpcInvalid));
        }
        for values in [
            vec!["--install-local".into(), r"C:\package".into()],
            vec!["--install-local".into(), "/S".into()],
            vec!["--installer-stage".into(), "install-local".into()],
            vec!["--install-local=production".into()],
        ] {
            assert_eq!(helper(&values), Err(Error::IpcInvalid));
        }
        // These select fixed entry semantics, never caller-supplied authority.
        for values in [
            vec![
                "--installer-stage".into(),
                "prepare-local-install /S".into(),
            ],
            vec!["--installer-stage".into(), "upgradeType=silent".into()],
        ] {
            assert_eq!(helper(&values), Err(Error::IpcInvalid));
        }
        for values in [
            vec![],
            vec!["--manifest".into(), "x".into()],
            vec!["--installer-stage".into(), "install-arbitrary".into()],
            vec!["--service=x".into()],
        ] {
            assert_eq!(helper(&values), Err(Error::IpcInvalid));
        }
        assert!(arguments(["a".into(), "b".into(), "c".into()]).is_err());
        assert!(arguments(["a".repeat(129).into()]).is_err());
        assert_eq!(exit_code(Error::StateCommitUnknown), 14);
        assert_eq!(exit_code(Error::Referenced), 12);
    }
}
