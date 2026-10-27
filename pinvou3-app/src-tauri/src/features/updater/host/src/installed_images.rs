//! Administrator's read-only installed-byte diagnostic. No source-profile,
//! current qualification, store mutation, preparation lock or installation right.
use crate::{
    error::{Error, Result},
    platform,
};
use serde::Serialize;

/// Privately constructed live observation retaining all actual inventory files,
/// role images, protected ancestry and the observed SCM process. No Clone,
/// Deserialize, raw path/handle or privilege-bearing JSON representation exists.
pub struct InstalledImageObservation {
    inner: platform::InstalledImages,
}
impl InstalledImageObservation {
    /// Recheck held objects, complete current inventory, registration and service
    /// incarnation. Cancellation is mandatory and latched; no deadline is reset.
    pub fn recheck(&mut self, mut cancelled: impl FnMut() -> bool) -> Result<()> {
        let mut stopped = false;
        self.inner
            .recheck(&mut || {
                stopped |= cancelled();
                stopped
            })
            .map_err(inspection_error)
    }
    /// Minimal nonsecret facts from the last completed inspection. These bytes
    /// are a historical diagnostic projection, NOT an OS lease or SourceFacts.
    pub fn diagnostic_json(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self.inner.diagnostic()?).map_err(|_| Error::Unavailable)
    }
}

/// Inspect ONLY this independently compiled host's registered installation.
/// Requires a real elevated local administrator, does not request elevation and
/// accepts no paths, scope overrides, version claims or other caller facts.
pub fn inspect_installed_images(
    mut cancelled: impl FnMut() -> bool,
) -> Result<InstalledImageObservation> {
    let mut stopped = false;
    let mut stop = || {
        stopped |= cancelled();
        stopped
    };
    crate::download_stage::poll(&mut stop).map_err(inspection_error)?;
    let inner = platform::inspect_images(&mut stop).map_err(inspection_error)?;
    crate::download_stage::poll(&mut stop).map_err(inspection_error)?;
    Ok(InstalledImageObservation { inner })
}

fn inspection_error(error: Error) -> Error {
    if error == Error::StagingCanceled {
        Error::InspectionCanceled
    } else {
        error
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImageDiagnostic {
    pub(crate) size: u64,
    pub(crate) sha256: String,
    pub(crate) manifest_declared_version: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HostDiagnostic {
    pub(crate) os: &'static str,
    pub(crate) architecture: &'static str,
    pub(crate) os_version: String,
    pub(crate) workstation: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstalledImageDiagnostic {
    pub(crate) observation_version: u8,
    pub(crate) observation_kind: &'static str,
    pub(crate) registration_revision: u64,
    pub(crate) host_epoch: u64,
    pub(crate) manifest_sha256: String,
    pub(crate) application: ImageDiagnostic,
    pub(crate) observed_application_version: String,
    pub(crate) observed_host: Option<HostDiagnostic>,
    pub(crate) helper: ImageDiagnostic,
    pub(crate) launcher: ImageDiagnostic,
    pub(crate) checked_inventory_files: usize,
    pub(crate) checked_inventory_bytes: u64,
    pub(crate) complete_inventory_checked: bool,
    pub(crate) json_retains_leases: bool,
    pub(crate) source_facts_produced: bool,
    pub(crate) current_qualification: bool,
    pub(crate) installation_permission: bool,
}

/// Private read-comparison data, never a native context or serialized authority.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ObservationStamp {
    pub(crate) registration_digest: String,
    pub(crate) scope: String,
    pub(crate) revision: u64,
    pub(crate) host_epoch: u64,
    pub(crate) service: crate::identity::ProcessSelector,
}
pub(crate) fn require_unchanged(before: &ObservationStamp, after: &ObservationStamp) -> Result<()> {
    if before != after {
        return Err(Error::ScopeConflict);
    }
    Ok(())
}

pub(crate) fn require_observed_app_version(
    observed: &str,
    manifest: &str,
    seed: &str,
) -> Result<()> {
    if observed != manifest || observed != seed {
        return Err(Error::ImageRefused);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_registration_scope_or_service_creation_cannot_reuse_inspection() {
        let before = ObservationStamp {
            registration_digest: "1".repeat(64),
            scope: "private-scope".into(),
            revision: 4,
            host_epoch: 2,
            service: crate::identity::ProcessSelector {
                process_id: 17,
                creation_file_time: crate::identity::FileTime::from_native(0, 19),
            },
        };
        assert!(require_unchanged(&before, &before.clone()).is_ok());
        let changes: &[fn(&mut ObservationStamp)] = &[
            |v| v.registration_digest = "2".repeat(64),
            |v| v.scope = "other".into(),
            |v| v.revision += 1,
            |v| v.host_epoch += 1,
            |v| v.service.process_id += 1,
            |v| v.service.creation_file_time = crate::identity::FileTime::from_native(0, 20),
        ];
        for change in changes {
            let mut after = before.clone();
            change(&mut after);
            assert!(require_unchanged(&before, &after) == Err(Error::ScopeConflict));
        }
    }
    #[test]
    fn diagnostic_projection_is_not_private_identity_or_current_source_facts() {
        let image = || ImageDiagnostic {
            size: 10,
            sha256: "a".repeat(64),
            manifest_declared_version: "1.2.3".into(),
        };
        let diagnostic = InstalledImageDiagnostic {
            observation_version: 1,
            observation_kind: "installed-images-physical-diagnostic",
            registration_revision: 4,
            host_epoch: 2,
            manifest_sha256: "b".repeat(64),
            application: image(),
            observed_application_version: "1.2.3".into(),
            observed_host: Some(HostDiagnostic {
                os: "windows",
                architecture: "x86_64",
                os_version: "10.0.26100".into(),
                workstation: true,
            }),
            helper: image(),
            launcher: image(),
            checked_inventory_files: 5,
            checked_inventory_bytes: 50,
            complete_inventory_checked: true,
            json_retains_leases: false,
            source_facts_produced: false,
            current_qualification: false,
            installation_permission: false,
        };
        let value = serde_json::to_value(diagnostic).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 17);
        assert_eq!(value["application"].as_object().unwrap().len(), 3);
        assert_eq!(value["application"]["manifestDeclaredVersion"], "1.2.3");
        assert_eq!(value["observedApplicationVersion"], "1.2.3");
        assert_eq!(
            value["observedHost"],
            serde_json::json!({"os":"windows", "architecture":"x86_64", "osVersion":"10.0.26100", "workstation":true})
        );
        for key in [
            "jsonRetainsLeases",
            "sourceFactsProduced",
            "currentQualification",
            "installationPermission",
        ] {
            assert_eq!(value[key], false);
        }
        for forbidden in [
            "installationScopeId",
            "instanceNonce",
            "processId",
            "fileIdentity",
            "path",
            "canonicalAppVersion",
            "stableFacts",
        ] {
            assert!(!serde_json::to_string(&value).unwrap().contains(forbidden));
        }
    }
    #[test]
    fn observed_version_must_match_both_installation_expectations() {
        assert!(require_observed_app_version("1.2.3", "1.2.3", "1.2.3").is_ok());
        assert_eq!(
            require_observed_app_version("1.2.3", "1.2.4", "1.2.3"),
            Err(Error::ImageRefused)
        );
        assert_eq!(
            require_observed_app_version("1.2.3", "1.2.3", "1.2.4"),
            Err(Error::ImageRefused)
        );
    }
    #[test]
    fn inspection_preserves_refusals_and_has_a_distinct_cancellation() {
        assert_eq!(
            inspection_error(Error::StagingCanceled),
            Error::InspectionCanceled
        );
        for error in [
            Error::SetupIncomplete,
            Error::ScopeConflict,
            Error::SecurityRefused,
            Error::StateCorrupt,
            Error::SourceUnavailable,
            Error::Unsupported,
        ] {
            assert_eq!(inspection_error(error), error);
        }
        assert_eq!(
            Error::from_code(Error::InspectionCanceled.code()),
            Some(Error::InspectionCanceled)
        );
    }
}
