use crate::{
    Error, MANIFEST_LIMIT, RECORD_LIMIT, hash,
    journal::{Journal, Phase},
    json,
    manifest::{self, Import, Manifest, Target},
    platform, valid_hash,
};
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Write, path::Path};

pub struct Job {
    pub(crate) directory: platform::Directory,
    pub(crate) manifest: Manifest,
    pub(crate) target: Target,
    pub(crate) journal: Journal,
    pub(crate) pin: String,
    // This actual share-mode-zero handle, not a PID/journal flag, owns the job.
    _writer: File,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub protocol_version: u8,
    pub manifest_sha256: String,
    pub recorded_phase: &'static str,
    pub attempt: u64,
    pub run_id: Option<String>,
    pub actual_length: Option<u64>,
    pub observed_bytes_match: bool,
    pub current_qualification_checked: bool,
    pub native_authenticated: bool,
    pub verified_execution_object: bool,
}
/// A current whole-byte observation holding the actual read-only file and job
/// custody. This does not seal content, confer a remote stage or install right.
/// No ordinary path/JSON constructor, Clone or serialization is provided.
pub struct ObservedCache {
    file: File,
    facts: Snapshot,
    package: RetainedPackage,
    _directory: platform::Directory,
    _writer: File,
}
impl ObservedCache {
    pub fn file(&self) -> &File {
        &self.file
    }
    pub fn facts(&self) -> &Snapshot {
        &self.facts
    }
    /// Original inputs from the independently pinned manifest, rechecked by
    /// observe_retained. These historical observations grant no current rights.
    pub fn package(&self) -> &RetainedPackage {
        &self.package
    }
}
/// Immutable original Package inputs, not a protected trust/eligibility token.
/// FileKind is necessarily FullPack: the job target is signed.fullPackage.
pub struct RetainedPackage {
    manifest: Manifest,
    target: Target,
}
impl RetainedPackage {
    pub fn job_id(&self) -> &str {
        &self.manifest.job_id
    }
    pub fn root_sha256(&self) -> &str {
        &self.manifest.root_sha256
    }
    pub fn envelope_bytes(&self) -> &[u8] {
        self.manifest.package.as_bytes()
    }
    pub fn expected_context_bytes(&self) -> &[u8] {
        self.manifest.expected.as_bytes()
    }
    pub fn supplied_time_millis(&self) -> u64 {
        self.manifest.initial_supplied_time_millis
    }
    pub fn artifact(&self) -> Result<pinvou_upgrade_transfer::Artifact<'_>, Error> {
        self.target.artifact()
    }
    pub fn target_key(&self) -> &str {
        &self.target.target
    }
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StopRequest {
    Pause,
    Cancel,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Control {
    manifest_sha256: String,
    run_id: String,
    request: StopRequest,
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Created => "created",
        Phase::Running => "running",
        Phase::Opened => "opened",
        Phase::Paused => "paused",
        Phase::Cancelled => "cancelled",
        Phase::NetworkUnknown => "networkUnknown",
        Phase::SourceRefused => "sourceRefused",
        Phase::IoUnknown => "ioUnknown",
        Phase::BytesMatch => "bytesMatch",
        Phase::BytesMismatch => "bytesMismatch",
    }
}
fn write_new(directory: &platform::Directory, name: &str, bytes: &[u8]) -> Result<File, Error> {
    let mut file = directory.file(name, true, true, true)?;
    file.write_all(bytes).map_err(|_| Error::IoUnavailable)?;
    file.sync_all().map_err(|_| Error::IoUnavailable)?;
    Ok(file)
}
fn manifest_at(directory: &platform::Directory, pin: &str) -> Result<(Manifest, Target), Error> {
    if !valid_hash(pin) {
        return Err(Error::InvalidInput);
    }
    let mut file = directory.file("job.json", false, false, false)?;
    let raw = platform::read(&mut file, MANIFEST_LIMIT)?;
    if hash(&raw) != pin {
        return Err(Error::PinMismatch);
    }
    let manifest: Manifest = serde_json::from_slice(&raw).map_err(|_| Error::InvalidInput)?;
    if json(&manifest)? != raw {
        return Err(Error::InvalidInput);
    }
    let target = manifest.target()?;
    Ok((manifest, target))
}

/// Create an exclusive job directory. The returned digest must be retained
/// separately by its operator; reading a digest back from the journal is no pin.
/// Interrupted creation is left intact and is not automatically overwritten.
pub fn create(
    path: &Path,
    root: String,
    root_sha256: String,
    package: String,
    expected: String,
    import: &Import,
) -> Result<String, Error> {
    let mut manifest = manifest::new(root, root_sha256, package, expected, import)?;
    let absolute = std::path::absolute(path).map_err(|_| Error::InvalidInput)?;
    let parent = absolute.parent().ok_or(Error::InvalidInput)?;
    let parent = platform::directory(parent)?;
    let directory = parent.create_dir(absolute.file_name().ok_or(Error::InvalidInput)?)?;
    let writer = directory.file("writer.lock", true, true, true)?;
    writer.sync_all().map_err(|_| Error::IoUnavailable)?;
    manifest.job_id = hash(&json(&platform::identity(&writer)?)?);
    let bytes = json(&manifest)?;
    if bytes.len() > MANIFEST_LIMIT {
        return Err(Error::InvalidInput);
    }
    let pin = hash(&bytes);
    let file = directory.file("journal.jsonl", true, true, true)?;
    let mut journal = Journal::open(file, &pin)?;
    journal.append(Phase::Created, 0, None)?;
    let pending = write_new(&directory, "manifest.pending", &bytes)?;
    // Publish the already-flushed actual source handle into the actual parent.
    // No path is resolved again and no cleanup may fail after this commit point.
    directory.publish(&pending, "job.json")?;
    // The manifest has two links by design. Unlike payload/lock/journal, it is
    // immutable public input; its separate pin authenticates every read.
    Ok(pin)
}

impl Job {
    pub fn open(path: &Path, pin: &str) -> Result<Self, Error> {
        let directory = platform::directory(path)?;
        let (manifest, target) = manifest_at(&directory, pin)?;
        let writer = directory.file("writer.lock", true, false, true)?;
        if hash(&json(&platform::identity(&writer)?)?) != manifest.job_id {
            return Err(Error::UnsafeObject);
        }
        let file = directory.file("journal.jsonl", true, false, true)?;
        let journal = Journal::open(file, pin)?;
        if journal.sequence == 0 {
            return Err(Error::JournalDamaged);
        }
        Ok(Self {
            directory,
            manifest,
            target,
            journal,
            pin: pin.into(),
            _writer: writer,
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            protocol_version: 1,
            manifest_sha256: self.pin.clone(),
            recorded_phase: phase_name(self.journal.phase),
            attempt: self.journal.attempt,
            run_id: matches!(self.journal.phase, Phase::Running | Phase::Opened)
                .then(|| self.journal.run_id.clone())
                .flatten(),
            actual_length: None,
            observed_bytes_match: false,
            current_qualification_checked: false,
            native_authenticated: false,
            verified_execution_object: false,
        }
    }
    pub fn check(&mut self, mut cancelled: impl FnMut() -> bool) -> Result<Snapshot, Error> {
        if self.journal.attempt == 0 || self.journal.identity.is_none() {
            return Err(Error::NotFound);
        }
        let mut file = self.attempt_file()?;
        pinvou_upgrade_byte_check::check_file(&mut file, self.target.artifact()?, &mut cancelled)
            .map_err(|error| match error {
            pinvou_upgrade_byte_check::ByteCheckError::Cancelled => Error::Cancelled,
            pinvou_upgrade_byte_check::ByteCheckError::IoUnavailable => Error::IoUnavailable,
            _ => Error::ByteMismatch,
        })?;
        let mut result = self.snapshot();
        result.observed_bytes_match = true;
        result.actual_length = Some(self.target.size);
        Ok(result)
    }
    pub(crate) fn attempt_file(&self) -> Result<File, Error> {
        let file = self
            .directory
            .file(attempt_name(self.journal.attempt), false, false, true)?;
        if Some(platform::identity(&file)?) != self.journal.identity {
            return Err(Error::UnsafeObject);
        }
        Ok(file)
    }
    pub(crate) fn control(&self, run_id: &str) -> Result<Option<StopRequest>, Error> {
        read_control(&self.directory, &self.pin, run_id)
    }
}
pub(crate) fn attempt_name(attempt: u64) -> String {
    format!("attempt-{attempt:020}.bin")
}

/// Independently recover observed complete bytes when dynamic association is
/// missing or damaged. The fixed numeric attempt selects no arbitrary path.
/// No journal is read, repaired, written or adopted as an authorization source.
pub fn observe_retained(
    path: &Path,
    pin: &str,
    attempt: u64,
    mut cancelled: impl FnMut() -> bool,
) -> Result<ObservedCache, Error> {
    if attempt == 0 {
        return Err(Error::InvalidInput);
    }
    let directory = platform::directory(path)?;
    let (manifest, target) = manifest_at(&directory, pin)?;
    let writer = directory.file("writer.lock", true, false, true)?;
    if hash(&json(&platform::identity(&writer)?)?) != manifest.job_id {
        return Err(Error::UnsafeObject);
    }
    let mut file = directory.file(attempt_name(attempt), false, false, true)?;
    let identity = platform::identity(&file)?;
    pinvou_upgrade_byte_check::check_file(&mut file, target.artifact()?, &mut cancelled).map_err(
        |error| match error {
            pinvou_upgrade_byte_check::ByteCheckError::Cancelled => Error::Cancelled,
            pinvou_upgrade_byte_check::ByteCheckError::IoUnavailable => Error::IoUnavailable,
            _ => Error::ByteMismatch,
        },
    )?;
    if platform::identity(&file)? != identity {
        return Err(Error::UnsafeObject);
    }
    let facts = Snapshot {
        protocol_version: 1,
        manifest_sha256: pin.into(),
        recorded_phase: "journalNotConsulted",
        attempt,
        run_id: None,
        actual_length: Some(target.size),
        observed_bytes_match: true,
        current_qualification_checked: false,
        native_authenticated: false,
        verified_execution_object: false,
    };
    Ok(ObservedCache {
        file,
        facts,
        package: RetainedPackage { manifest, target },
        _directory: directory,
        _writer: writer,
    })
}
fn read_control(
    directory: &platform::Directory,
    pin: &str,
    run_id: &str,
) -> Result<Option<StopRequest>, Error> {
    let mut file = match directory.file(format!("control-{run_id}.json"), false, false, false) {
        Ok(file) => file,
        Err(Error::NotFound) => return Ok(None),
        Err(error) => return Err(error),
    };
    let raw = platform::read(&mut file, RECORD_LIMIT)?;
    let control: Control = serde_json::from_slice(&raw).map_err(|_| Error::JournalDamaged)?;
    if json(&control)? != raw || control.manifest_sha256 != pin || control.run_id != run_id {
        return Err(Error::JournalDamaged);
    }
    Ok(Some(control.request))
}
/// Stop requests name exactly one attempt. A stale pause/cancel cannot affect
/// the next attempt, and its existence never proves that the writer has stopped.
pub fn request_stop(
    path: &Path,
    pin: &str,
    run_id: &str,
    request: StopRequest,
) -> Result<(), Error> {
    if !valid_hash(run_id) {
        return Err(Error::InvalidInput);
    }
    let directory = platform::directory(path)?;
    manifest_at(&directory, pin)?;
    let raw = json(&Control {
        manifest_sha256: pin.into(),
        run_id: run_id.into(),
        request,
    })?;
    let pending = format!("control-{run_id}.pending");
    let source = match write_new(&directory, &pending, &raw) {
        Ok(source) => source,
        Err(Error::AlreadyExists) => {
            return match read_control(&directory, pin, run_id)? {
                Some(original) if original == request => Ok(()),
                _ => Err(Error::AlreadyExists),
            };
        }
        Err(error) => return Err(error),
    };
    directory.publish(&source, format!("control-{run_id}.json"))?;
    Ok(())
}
