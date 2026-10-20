//! Compose retained FullPack/Package binding with same-object native observations.
//! Ordinary output cache, not a protected host, execution handoff or permission.
mod platform;
mod role;
use pinvou_upgrade_archive::{ArchiveLimits, materialize_member};
use pinvou_upgrade_codec::{Document, InputKind, derive_metadata_reference};
use pinvou_upgrade_custody::{Directory, Identity};
use pinvou_upgrade_download_job::{ObservedCache, observe_retained};
use pinvou_upgrade_native_evidence::{ExpectedFile, Observation as NativeObservation};
use pinvou_upgrade_package_check::{PackageInspection, select_native_member};
pub use role::{Role, RoleObservation, RoleReport, RoleRequest, RoleSelection, inspect_role};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    path::Path,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    InvalidInput,
    Job,
    Package,
    Bytes,
    UnsafeObject,
    IoUnavailable,
    Cancelled,
    Native,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "RETAINED_NATIVE_UNSUPPORTED",
            Self::InvalidInput => "RETAINED_NATIVE_INPUT_INVALID",
            Self::Job => "RETAINED_NATIVE_JOB_REFUSED",
            Self::Package => "RETAINED_NATIVE_PACKAGE_REFUSED",
            Self::Bytes => "RETAINED_NATIVE_BYTES_REFUSED",
            Self::UnsafeObject => "RETAINED_NATIVE_OBJECT_REFUSED",
            Self::IoUnavailable => "RETAINED_NATIVE_IO_UNAVAILABLE",
            Self::Cancelled => "RETAINED_NATIVE_CANCELLED",
            Self::Native => "RETAINED_NATIVE_COLLECTION_UNAVAILABLE",
        })
    }
}
impl std::error::Error for Error {}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    pub file_bytes: u64,
    pub expanded_bytes: u64,
    pub entries: u16,
    pub metadata_bytes: u64,
    pub path_state_bytes: u64,
    pub ratio: u64,
}
impl Limits {
    fn archive(&self) -> Result<ArchiveLimits, Error> {
        ArchiveLimits::new(
            self.file_bytes,
            self.expanded_bytes,
            self.entries,
            self.metadata_bytes,
            self.path_state_bytes,
            self.ratio,
        )
        .map_err(|_| Error::InvalidInput)
    }
}
/// Holds the original job reservation, source, destination directory and native
/// readonly object throughout use. Nothing persists an execution authority.
pub struct Observation {
    source: ObservedCache,
    native: NativeObservation,
    _destination: Directory,
    destination_name: String,
    package_id: String,
    package_sha256: String,
    package_size: u64,
    envelope_sha256: String,
    member_name: String,
    declared_native: serde_json::Value,
    source_identity: Identity,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report<'a> {
    protocol_version: u8,
    job_id: &'a str,
    root_sha256: &'a str,
    target_key: &'a str,
    original_supplied_time_millis: u64,
    retained: &'a pinvou_upgrade_download_job::Snapshot,
    file_kind: &'static str,
    package_id: &'a str,
    package_size: u64,
    package_sha256: &'a str,
    package_envelope_sha256: &'a str,
    source_identity: &'a Identity,
    member_name: &'a str,
    declared_native: &'a serde_json::Value,
    destination_name: &'a str,
    native: &'a pinvou_upgrade_native_evidence::Report,
    publisher_qualification: bool,
    current_qualification: bool,
    installation_rights: bool,
    verified_execution_object: bool,
    trusted_deadline_evidence: bool,
}
impl Observation {
    pub fn report(&self) -> Report<'_> {
        Report {
            protocol_version: 1,
            job_id: self.source.package().job_id(),
            root_sha256: self.source.package().root_sha256(),
            target_key: self.source.package().target_key(),
            original_supplied_time_millis: self.source.package().supplied_time_millis(),
            retained: self.source.facts(),
            file_kind: "FullPack",
            package_id: &self.package_id,
            package_size: self.package_size,
            package_sha256: &self.package_sha256,
            package_envelope_sha256: &self.envelope_sha256,
            source_identity: &self.source_identity,
            member_name: &self.member_name,
            declared_native: &self.declared_native,
            destination_name: &self.destination_name,
            native: self.native.report(),
            publisher_qualification: false,
            current_qualification: false,
            installation_rights: false,
            verified_execution_object: false,
            trusted_deadline_evidence: false,
        }
    }
}
fn poll(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}
fn digest(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
fn job_error(error: pinvou_upgrade_download_job::Error) -> Error {
    match error {
        pinvou_upgrade_download_job::Error::Unsupported => Error::Unsupported,
        pinvou_upgrade_download_job::Error::Cancelled => Error::Cancelled,
        _ => Error::Job,
    }
}
fn custody(error: pinvou_upgrade_custody::Error) -> Error {
    match error {
        pinvou_upgrade_custody::Error::Unsupported => Error::Unsupported,
        pinvou_upgrade_custody::Error::UnsafeObject => Error::UnsafeObject,
        _ => Error::IoUnavailable,
    }
}
fn document(bytes: &[u8]) -> Result<Document, Error> {
    Document::parse(bytes, InputKind::Metadata).map_err(|_| Error::Package)
}
fn fresh(directory: &Directory) -> Result<(String, File), Error> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // Collision only refuses: no existing name is ever replaced/truncated.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error::IoUnavailable)?
        .as_nanos();
    let name = format!(
        "native-{}-{nonce}-{}.exe",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let file = directory.file(&name, true, true, true).map_err(custody)?;
    Ok((name, file))
}
fn reopen(directory: &Directory, name: &str, identity: Identity) -> Result<File, Error> {
    let file = directory.file(name, false, false, false).map_err(custody)?;
    if pinvou_upgrade_custody::identity(&file).map_err(custody)? != identity {
        return Err(Error::UnsafeObject);
    }
    Ok(file)
}

/// Synchronous collection; cancellation joins native work before releasing files.
/// The source is always signed.fullPackage, never the outer upload ZIP. Only the
/// Windows NSIS finalInstaller is supported. Capacities do not set a deadline.
pub fn inspect(
    job: &Path,
    manifest_sha256: &str,
    attempt: u64,
    output: &Path,
    member_name: &str,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<Observation, Error> {
    inspect_selected(
        job,
        manifest_sha256,
        attempt,
        output,
        RoleRequest {
            role: Role::FinalInstaller,
            member_name,
        },
        limits,
        cancel,
    )
    .map(|(observation, _)| observation)
}

// One actual byte/custody/native pipeline for the old API and every role.
fn inspect_selected(
    job: &Path,
    manifest_sha256: &str,
    attempt: u64,
    output: &Path,
    request: RoleRequest<'_>,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<(Observation, RoleSelection), Error> {
    let RoleRequest { role, member_name } = request;
    poll(cancel)?;
    platform::require_supported()?;
    let limits = limits.archive()?;
    let source = observe_retained(job, manifest_sha256, attempt, || {
        cancel.load(Ordering::Acquire)
    })
    .map_err(job_error)?;
    if source.package().target_key() != "windows-x86_64-nsis-perMachine" {
        return Err(Error::Unsupported);
    }
    let original = source.package();
    let artifact = original.artifact().map_err(job_error)?;
    let expected = document(original.expected_context_bytes())?;
    let package = document(original.envelope_bytes())?;
    let version = document(
        &serde_json::to_vec(&package.value()["signed"]["appVersion"])
            .map_err(|_| Error::Package)?,
    )?
    .app_version_shape()
    .map_err(|_| Error::Package)?;
    let time = document(original.supplied_time_millis().to_string().as_bytes())?
        .protocol_time_shape()
        .map_err(|_| Error::Package)?;
    let reference =
        derive_metadata_reference(original.envelope_bytes()).map_err(|_| Error::Package)?;
    let input = PackageInspection {
        artifact,
        manifest_bytes: original.envelope_bytes(),
        reference: &reference,
        expected_context: &expected,
        expected_version: &version,
        supplied_time: &time,
        limits,
    };
    let mut reader = source
        .file()
        .try_clone()
        .map_err(|_| Error::IoUnavailable)?;
    let source_identity = pinvou_upgrade_custody::identity(&reader).map_err(custody)?;
    let selected =
        select_native_member(&mut reader, &input, role.member_kind(), member_name, || {
            cancel.load(Ordering::Acquire)
        })
        .map_err(|_| {
            if cancel.load(Ordering::Acquire) {
                Error::Cancelled
            } else {
                Error::Package
            }
        })?;
    // select_native_member has rechecked complete Package semantics, including
    // executionHelperId -> helpers[].nativeIdentity == Package.helper. The
    // selection is a report of that exact declaration, never a current owner.
    let selection = role.selection(package.value().get("signed").ok_or(Error::Package)?)?;
    poll(cancel)?;
    let directory = pinvou_upgrade_custody::directory(output).map_err(custody)?;
    let (destination_name, mut destination) = fresh(&directory)?;
    let destination_identity = pinvou_upgrade_custody::identity(&destination).map_err(custody)?;
    materialize_member(
        &mut reader,
        &mut destination,
        artifact,
        selected.expected_member().map_err(|_| Error::Package)?,
        limits,
        || cancel.load(Ordering::Acquire),
    )
    .map_err(|_| {
        if cancel.load(Ordering::Acquire) {
            Error::Cancelled
        } else {
            Error::Bytes
        }
    })?;
    if pinvou_upgrade_custody::identity(&destination).map_err(custody)? != destination_identity
        || pinvou_upgrade_custody::identity(&reader).map_err(custody)? != source_identity
    {
        return Err(Error::UnsafeObject);
    }
    // No duplicate survives this write object. Ordinary close/reopen can fail;
    // this is deliberately not an atomic protected execution-object handoff.
    drop(destination);
    poll(cancel)?;
    let destination = reopen(&directory, &destination_name, destination_identity)?;
    let native_expected = ExpectedFile::new(selected.size(), &digest(selected.sha256()))
        .map_err(|_| Error::InvalidInput)?
        .with_identity_sha256(&hash(
            &serde_json::to_vec(&destination_identity).map_err(|_| Error::IoUnavailable)?,
        ))
        .map_err(|_| Error::InvalidInput)?;
    let native =
        pinvou_upgrade_native_evidence::observe_file(&destination, &native_expected, cancel)
            .map_err(|error| match error {
                pinvou_upgrade_native_evidence::Error::Cancelled => Error::Cancelled,
                pinvou_upgrade_native_evidence::Error::Unsupported => Error::Unsupported,
                _ => Error::Native,
            })?;
    poll(cancel)?;
    let package_id = artifact.package_id().to_owned();
    let package_size = artifact.size();
    let package_sha256 = digest(&artifact.sha256());
    let envelope_sha256 = hash(original.envelope_bytes());
    let member_name = selected.name().to_owned();
    let declared_native = serde_json::from_slice(selected.identity().canonical_bytes())
        .map_err(|_| Error::Package)?;
    drop(reader);
    Ok((
        Observation {
            source,
            native,
            _destination: directory,
            destination_name,
            package_id,
            package_size,
            package_sha256,
            envelope_sha256,
            member_name,
            declared_native,
            source_identity,
        },
        selection,
    ))
}

#[cfg(test)]
mod tests;
