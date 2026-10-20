//! Durable isolated QA acceptance, never protected bootstrap or installation authority.
mod coordinator;
mod graph;
mod history;
mod input;
mod platform;
mod state;

pub use coordinator::{Acceptance, QaStore};
pub use input::{Bundle, read_bounded};
pub use state::{Inspection, Original};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    InvalidStore,
    Verification,
    Rollback,
    Equivocation,
    Conflict,
    Unavailable,
    Uncertain,
    Unsupported,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "METADATA_QA_INPUT_INVALID",
            Self::InvalidStore => "METADATA_QA_STORE_INVALID",
            Self::Verification => "METADATA_QA_VERIFICATION_FAILED",
            Self::Rollback => "METADATA_ROLLBACK",
            Self::Equivocation => "METADATA_EQUIVOCATION",
            Self::Conflict => "METADATA_QA_CONCURRENT_CHANGE",
            Self::Unavailable => "METADATA_QA_STORAGE_UNAVAILABLE",
            Self::Uncertain => "METADATA_QA_COMMIT_UNCERTAIN",
            Self::Unsupported => "PROTECTED_METADATA_ACCEPTANCE_UNSUPPORTED",
        })
    }
}
impl std::error::Error for Error {}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::Unavailable
    }
}
pub(crate) fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub(crate) fn document(bytes: &[u8]) -> Result<pinvou_upgrade_codec::Document, Error> {
    pinvou_upgrade_codec::Document::parse(bytes, pinvou_upgrade_codec::InputKind::Metadata)
        .map_err(|_| Error::InvalidInput)
}
pub(crate) fn canonical(value: &serde_json::Value) -> Result<Vec<u8>, Error> {
    document(&serde_json::to_vec(value).map_err(|_| Error::InvalidInput)?)?
        .canonical_bytes()
        .map_err(|_| Error::InvalidInput)
}
