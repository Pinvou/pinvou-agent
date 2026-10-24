//! Explicit QA cache acquisition, not a protected upgrade owner or install API.
//! Every persisted byte is untrusted. Callers retain the independently returned
//! manifest digest; no journal field can confer source continuity or authority.
mod control;
mod journal;
mod manifest;
mod platform;
mod resume;
mod runner;
mod store;

pub use control::{ControlAttempt, run_control, run_control_with_resume};
pub use manifest::{Import, NetworkConsent};
pub use resume::ResumeCheckpointPin;
pub use runner::{Notice, run, run_with_resume};
pub use store::{
    Job, ObservedCache, RetainedPackage, Snapshot, StopRequest, create, observe_retained,
    request_stop,
};

use sha2::{Digest, Sha256};

const DOCUMENT_LIMIT: usize = 1_048_576;
// Two independently bounded public JSON documents may be escaped inside strings.
const MANIFEST_LIMIT: usize = 16 * DOCUMENT_LIMIT;
const RECORD_LIMIT: usize = 4096;
const MAX_SAFE: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    InvalidInput,
    PinMismatch,
    SignatureRefused,
    BindingMismatch,
    Busy,
    AlreadyExists,
    NotFound,
    IoUnavailable,
    UnsafeObject,
    JournalDamaged,
    Cancelled,
    ByteMismatch,
    NetworkUnknown,
    SourceRefused,
    ControlUnavailable,
    ControlRefused,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "DOWNLOAD_JOB_UNSUPPORTED",
            Self::InvalidInput => "DOWNLOAD_JOB_INPUT_INVALID",
            Self::PinMismatch => "DOWNLOAD_JOB_PIN_MISMATCH",
            Self::SignatureRefused => "DOWNLOAD_JOB_SIGNATURE_REFUSED",
            Self::BindingMismatch => "DOWNLOAD_JOB_BINDING_MISMATCH",
            Self::Busy => "DOWNLOAD_JOB_BUSY",
            Self::AlreadyExists => "DOWNLOAD_JOB_ALREADY_EXISTS",
            Self::NotFound => "DOWNLOAD_JOB_NOT_FOUND",
            Self::IoUnavailable => "DOWNLOAD_JOB_IO_UNAVAILABLE",
            Self::UnsafeObject => "DOWNLOAD_JOB_OBJECT_REFUSED",
            Self::JournalDamaged => "DOWNLOAD_JOB_JOURNAL_DAMAGED",
            Self::Cancelled => "DOWNLOAD_JOB_CANCELLED",
            Self::ByteMismatch => "DOWNLOAD_JOB_BYTES_MISMATCH",
            Self::NetworkUnknown => "DOWNLOAD_JOB_NETWORK_UNKNOWN",
            Self::SourceRefused => "DOWNLOAD_JOB_SOURCE_REFUSED",
            Self::ControlUnavailable => "DOWNLOAD_JOB_CONTROL_UNAVAILABLE",
            Self::ControlRefused => "DOWNLOAD_JOB_CONTROL_REFUSED",
        })
    }
}
impl std::error::Error for Error {}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(value).map_err(|_| Error::InvalidInput)
}

/// Bounded same-handle ordinary input for this explicit tool. This rejects
/// ancestor reparses/streams and does not turn a file into protected trust.
pub fn read_input(path: &std::path::Path, maximum: usize) -> Result<String, Error> {
    if maximum == 0 || maximum > MANIFEST_LIMIT {
        return Err(Error::InvalidInput);
    }
    let path = std::path::absolute(path).map_err(|_| Error::InvalidInput)?;
    let parent = platform::directory(path.parent().ok_or(Error::InvalidInput)?)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(Error::InvalidInput)?;
    if name.contains(':') || name.ends_with(['.', ' ']) {
        return Err(Error::InvalidInput);
    }
    let mut file = parent.file(name, false, false, true)?;
    String::from_utf8(platform::read(&mut file, maximum)?).map_err(|_| Error::InvalidInput)
}

#[cfg(test)]
mod tests;
