//! Independently pinned, closed checkpoint data; no URL/credential persistence.
use super::{ResumeCheckpointPin, prefix};
use crate::{DOCUMENT_LIMIT, Error, Job, MAX_SAFE, hash, json, platform, valid_hash};
use pinvou_upgrade_transfer::StrongEtag;
use pinvou_upgrade_verifier::SignatureCheckedDownloadSource;
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Write};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Source {
    source_id: String,
    object_id: String,
    object_version: String,
    package_id: String,
    size: u64,
    sha256: String,
}
impl Source {
    fn from_checked(value: &SignatureCheckedDownloadSource) -> Self {
        Self {
            source_id: value.source_id().into(),
            object_id: value.object_id().into(),
            object_version: value.object_version().into(),
            package_id: value.package_id().into(),
            size: value.size(),
            sha256: value.sha256().into(),
        }
    }
    pub(super) fn matches(&self, value: &SignatureCheckedDownloadSource) -> bool {
        self.source_id == value.source_id()
            && self.object_id == value.object_id()
            && self.object_version == value.object_version()
            && self.package_id == value.package_id()
            && self.size == value.size()
            && self.sha256 == value.sha256()
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Checkpoint {
    protocol_version: u8,
    manifest_sha256: String,
    pub(super) attempt: u64,
    pub(super) identity: platform::Identity,
    pub(super) source: Source,
    pub(super) length: u64,
    pub(super) prefix_sha256: String,
    // Opaque validated HTTP octets, not UTF-8 or provider identity. The whole
    // private JSON is bounded at one MiB before deserializing this vector.
    pub(super) validator: Vec<u8>,
}
fn name(pin: &ResumeCheckpointPin) -> String {
    format!("resume-{}.json", pin.sha256())
}
pub(super) fn load(job: &Job, pin: &ResumeCheckpointPin) -> Result<Checkpoint, Error> {
    let mut file = job.directory.file(name(pin), false, false, true)?;
    let raw = platform::read(&mut file, DOCUMENT_LIMIT)?;
    if hash(&raw) != pin.sha256() {
        return Err(Error::PinMismatch);
    }
    let value: Checkpoint = serde_json::from_slice(&raw).map_err(|_| Error::InvalidInput)?;
    if json(&value)? != raw
        || value.protocol_version != 1
        || value.manifest_sha256 != job.pin
        || value.attempt == 0
        || value.attempt > MAX_SAFE
        || value.attempt >= job.journal.attempt
        || value.length == 0
        || value.length >= job.target.size
        || !valid_hash(&value.prefix_sha256)
        || value.source.package_id != job.target.id
        || value.source.size != job.target.size
        || value.source.sha256 != job.target.digest
        || StrongEtag::parse(&value.validator).is_err()
    {
        return Err(Error::BindingMismatch);
    }
    Ok(value)
}

/// Called by the original worker only after transfer has returned and its body
/// futures have closed. Sync, scan and source identity precede publication.
/// A returned digest is useful only if the caller actually retains it outside
/// this ordinary cache. No directory/power-loss durability is certified here.
pub(crate) fn publish(
    job: &Job,
    file: &mut File,
    source: &SignatureCheckedDownloadSource,
    validator: Option<&[u8]>,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<Option<ResumeCheckpointPin>, Error> {
    let Some(validator) = validator else {
        return Ok(None);
    };
    StrongEtag::parse(validator).map_err(|_| Error::InvalidInput)?;
    let length = file.metadata().map_err(|_| Error::IoUnavailable)?.len();
    if length == 0 || length >= job.target.size {
        return Ok(None);
    }
    if source.package_id() != job.target.id
        || source.size() != job.target.size
        || source.sha256() != job.target.digest
    {
        return Err(Error::BindingMismatch);
    }
    let identity = platform::identity(file)?;
    if Some(identity) != job.journal.identity {
        return Err(Error::UnsafeObject);
    }
    file.sync_all().map_err(|_| Error::IoUnavailable)?;
    let prefix_sha256 = prefix::scan(file, length, cancelled)?;
    if platform::identity(file)? != identity {
        return Err(Error::UnsafeObject);
    }
    let value = Checkpoint {
        protocol_version: 1,
        manifest_sha256: job.pin.clone(),
        attempt: job.journal.attempt,
        identity,
        source: Source::from_checked(source),
        length,
        prefix_sha256,
        validator: validator.to_vec(),
    };
    let raw = json(&value)?;
    if raw.len() > DOCUMENT_LIMIT {
        return Err(Error::InvalidInput);
    }
    let pin = ResumeCheckpointPin::from_independently_retained_sha256(&hash(&raw))?;
    if cancelled() {
        return Err(Error::Cancelled);
    }
    match job.directory.file(name(&pin), true, true, true) {
        Ok(mut destination) => {
            destination
                .write_all(&raw)
                .map_err(|_| Error::IoUnavailable)?;
            destination.sync_all().map_err(|_| Error::IoUnavailable)?;
        }
        Err(Error::AlreadyExists) => {
            let mut original = job.directory.file(name(&pin), true, false, true)?;
            if platform::read(&mut original, DOCUMENT_LIMIT)? != raw {
                return Err(Error::PinMismatch);
            }
            // Equal cached bytes alone do not prove that this actual object was
            // flushed. A prior interrupted publication may have created it.
            original.sync_all().map_err(|_| Error::IoUnavailable)?;
        }
        Err(error) => return Err(error),
    }
    if cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(Some(pin))
}
