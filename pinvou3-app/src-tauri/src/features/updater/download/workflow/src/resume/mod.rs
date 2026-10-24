//! Ordinary-cache continuity under an independently retained checkpoint digest.
//! Neither a cache record nor a caller string is native or installation authority.
mod checkpoint;
mod prefix;
use crate::{Error, Job, platform, valid_hash};
use pinvou_upgrade_transfer::{
    Artifact, PrefixState, RequestPlan, SourceContinuity, StrongEtag, plan_request,
};
use pinvou_upgrade_verifier::SignatureCheckedDownloadSource;
use std::fs::File;

/// Digest returned by an earlier actual flushed checkpoint. The real caller must
/// retain it independently of this untrusted cache. Reading a digest back from
/// the journal/checkpoint directory does not establish continuity or ownership.
/// This typed wrapper validates spelling only; it is not a native capability.
#[derive(Clone)]
pub struct ResumeCheckpointPin(String);
impl ResumeCheckpointPin {
    pub fn from_independently_retained_sha256(value: &str) -> Result<Self, Error> {
        if !valid_hash(value) {
            return Err(Error::InvalidInput);
        }
        Ok(Self(value.into()))
    }
    pub fn sha256(&self) -> &str {
        &self.0
    }
}

pub(crate) struct RetainedPrefix {
    length: u64,
    validator: Vec<u8>,
}
impl RetainedPrefix {
    pub(crate) fn plan<'a>(&'a self, artifact: Artifact<'a>) -> Result<RequestPlan<'a>, Error> {
        let validator = StrongEtag::parse(&self.validator).map_err(|_| Error::InvalidInput)?;
        let prefix = PrefixState::new(Some(artifact), self.length, Some(validator))
            .map_err(|_| Error::InvalidInput)?;
        Ok(plan_request(
            artifact,
            Some(prefix),
            SourceContinuity::SameImmutableEntity,
        ))
    }
}

/// Only a fresh verifier-created source plus the externally pinned original
/// checkpoint can reach Range. A changed source starts a new file from zero.
/// The old file is opened by its fixed numeric name relative to held ancestry,
/// never written/truncated, and remains held until the exact copy is checked.
pub(crate) fn prepare(
    job: &Job,
    pin: Option<&ResumeCheckpointPin>,
    fresh: &SignatureCheckedDownloadSource,
    destination: &mut File,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<Option<RetainedPrefix>, Error> {
    let Some(pin) = pin else {
        return Ok(None);
    };
    let record = checkpoint::load(job, pin)?;
    if !record.source.matches(fresh) {
        return Ok(None);
    }
    let mut source = job.directory.file(
        crate::store::attempt_name(record.attempt),
        false,
        false,
        true,
    )?;
    if platform::identity(&source)? != record.identity {
        return Err(Error::UnsafeObject);
    }
    prefix::copy_checked(
        &mut source,
        destination,
        record.length,
        &record.prefix_sha256,
        cancelled,
    )?;
    if platform::identity(&source)? != record.identity {
        return Err(Error::UnsafeObject);
    }
    Ok(Some(RetainedPrefix {
        length: record.length,
        validator: record.validator,
    }))
}

/// The same worker path is used for notices and tests: any scan/publication/
/// sync failure returns before the observer can receive a checkpoint digest.
pub(crate) fn publish_notice(
    job: &Job,
    file: &mut File,
    source: &SignatureCheckedDownloadSource,
    validator: Option<&[u8]>,
    cancelled: &mut impl FnMut() -> bool,
    observe: &mut impl FnMut(crate::Notice) -> Result<(), Error>,
) -> Result<(), Error> {
    if let Some(pin) = checkpoint::publish(job, file, source, validator, cancelled)? {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        observe(crate::Notice::Checkpoint {
            checkpoint_sha256: pin.sha256().into(),
        })?;
    }
    Ok(())
}
#[cfg(test)]
pub(crate) mod tests;
