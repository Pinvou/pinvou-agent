//! Closed native declaration selection. No installed role, version, publisher,
//! approval or execution authority follows from a signed Package declaration.
use crate::{Error, Limits, Observation, Report, inspect_selected};
use pinvou_upgrade_package_check::NativeMemberKind;
use serde::Serialize;
use std::{path::Path, sync::atomic::AtomicBool};

/// An ordinary request, not a verified role token. ExecutionHelper deliberately
/// has no caller-supplied ID: only signed.executionHelperId can choose it.
#[derive(Clone, Copy)]
pub enum Role<'a> {
    FinalInstaller,
    ExecutionHelper,
    Launcher,
    HelperArtifact(&'a str),
}
/// Ordinary caller selection plus an independent safe ZIP locator. This is not
/// a checked member or a capability; the shared pipeline validates both.
pub struct RoleRequest<'a> {
    pub role: Role<'a>,
    pub member_name: &'a str,
}
impl<'a> Role<'a> {
    pub(crate) fn member_kind(self) -> NativeMemberKind<'a> {
        match self {
            Self::FinalInstaller => NativeMemberKind::FinalInstaller,
            Self::ExecutionHelper => NativeMemberKind::ExecutionHelper,
            Self::Launcher => NativeMemberKind::Launcher,
            Self::HelperArtifact(id) => NativeMemberKind::HelperArtifact(id),
        }
    }
    pub(crate) fn selection(self, signed: &serde_json::Value) -> Result<RoleSelection, Error> {
        let (kind, helper_id) = match self {
            Self::FinalInstaller => ("finalInstaller", None),
            Self::Launcher => ("launcher", None),
            Self::ExecutionHelper => (
                "executionHelper",
                Some(signed["executionHelperId"].as_str().ok_or(Error::Package)?),
            ),
            Self::HelperArtifact(id) => {
                // Report the actual signed matching entry, not an echoed input.
                // Target, uniqueness and native binding are the existing Package
                // semantic/selector owners, already checked by the shared core.
                let helper = signed["helpers"]
                    .as_array()
                    .ok_or(Error::Package)?
                    .iter()
                    .find(|h| h["helperId"].as_str() == Some(id))
                    .ok_or(Error::Package)?;
                (
                    "helperArtifact",
                    Some(helper["helperId"].as_str().ok_or(Error::Package)?),
                )
            }
        };
        Ok(RoleSelection {
            kind,
            helper_id: helper_id.map(str::to_owned),
        })
    }
}
/// Projected only after Package semantics/actual FullPack-member binding. This
/// contains logical declarations; it does not attest installed/native versions.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleSelection {
    kind: &'static str,
    helper_id: Option<String>,
}
impl RoleSelection {
    pub fn kind(&self) -> &'static str {
        self.kind
    }
    pub fn helper_id(&self) -> Option<&str> {
        self.helper_id.as_deref()
    }
}
/// Keeps every source/output guard of the original observation until dropped.
pub struct RoleObservation {
    selection: RoleSelection,
    observation: Observation,
}
impl RoleObservation {
    pub fn selection(&self) -> &RoleSelection {
        &self.selection
    }
    pub fn report(&self) -> RoleReport<'_> {
        RoleReport {
            protocol_version: 1,
            selection: &self.selection,
            observation: self.observation.report(),
            publisher_qualification: false,
            current_qualification: false,
            installation_rights: false,
            verified_execution_object: false,
            trusted_deadline_evidence: false,
        }
    }
}
/// Separate wrapper: legacy inspect's report fields and meaning are unchanged.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleReport<'a> {
    protocol_version: u8,
    selection: &'a RoleSelection,
    observation: Report<'a>,
    publisher_qualification: bool,
    current_qualification: bool,
    installation_rights: bool,
    verified_execution_object: bool,
    trusted_deadline_evidence: bool,
}
/// Read/copy/observe one exact Package role using the original shared pipeline.
/// installationTarget is never consulted as a source path or write destination.
pub fn inspect_role(
    job: &Path,
    manifest_sha256: &str,
    attempt: u64,
    output: &Path,
    request: RoleRequest<'_>,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<RoleObservation, Error> {
    let (observation, selection) = inspect_selected(
        job,
        manifest_sha256,
        attempt,
        output,
        request,
        limits,
        cancel,
    )?;
    Ok(RoleObservation {
        selection,
        observation,
    })
}
