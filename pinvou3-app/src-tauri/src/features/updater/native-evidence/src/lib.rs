//! Read-only, same-object Windows embedded Authenticode evidence. No execution,
//! protected bootstrap, publisher mapping or current installation authority.
mod platform;
use pinvou_upgrade_custody::{Directory, Identity};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    InvalidInput,
    Busy,
    NotFound,
    UnsafeObject,
    IoUnavailable,
    ByteMismatch,
    IdentityMismatch,
    Cancelled,
    EvidenceUnavailable,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "NATIVE_EVIDENCE_UNSUPPORTED",
            Self::InvalidInput => "NATIVE_EVIDENCE_INPUT_INVALID",
            Self::Busy => "NATIVE_EVIDENCE_BUSY",
            Self::NotFound => "NATIVE_EVIDENCE_NOT_FOUND",
            Self::UnsafeObject => "NATIVE_EVIDENCE_OBJECT_REFUSED",
            Self::IoUnavailable => "NATIVE_EVIDENCE_IO_UNAVAILABLE",
            Self::ByteMismatch => "NATIVE_EVIDENCE_BYTES_MISMATCH",
            Self::IdentityMismatch => "NATIVE_EVIDENCE_IDENTITY_MISMATCH",
            Self::Cancelled => "NATIVE_EVIDENCE_CANCELLED",
            Self::EvidenceUnavailable => "NATIVE_EVIDENCE_UNAVAILABLE",
        })
    }
}
impl std::error::Error for Error {}
impl From<pinvou_upgrade_custody::Error> for Error {
    fn from(e: pinvou_upgrade_custody::Error) -> Self {
        use pinvou_upgrade_custody::Error as E;
        match e {
            E::Unsupported => Self::Unsupported,
            E::InvalidInput => Self::InvalidInput,
            E::Busy => Self::Busy,
            E::AlreadyExists | E::IoUnavailable => Self::IoUnavailable,
            E::NotFound => Self::NotFound,
            E::UnsafeObject => Self::UnsafeObject,
        }
    }
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn identity_hash(value: &Identity) -> Result<String, Error> {
    Ok(hash(
        &serde_json::to_vec(value).map_err(|_| Error::EvidenceUnavailable)?,
    ))
}

/// Explicit byte expectations; these ordinary inputs prove no publisher policy.
pub struct ExpectedFile {
    size: u64,
    sha256: String,
    identity_sha256: Option<String>,
}
impl ExpectedFile {
    pub fn new(size: u64, sha256: &str) -> Result<Self, Error> {
        if size == 0 || size > 9_007_199_254_740_991 || !valid_hash(sha256) {
            return Err(Error::InvalidInput);
        }
        Ok(Self {
            size,
            sha256: sha256.to_owned(),
            identity_sha256: None,
        })
    }
    pub fn with_identity_sha256(mut self, value: &str) -> Result<Self, Error> {
        if !valid_hash(value) {
            return Err(Error::InvalidInput);
        }
        self.identity_sha256 = Some(value.to_owned());
        Ok(self)
    }
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
/// OS/provider observation for this one embedded signature only. `Verified`
/// is not V1 Publisher/SPKI/EKU/RFC3161 mapping or fresh qualification.
pub enum Verification {
    Verified,
    Unsigned,
    InvalidSignature,
    Refused,
    Unknown,
}
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CertificateEvidence {
    pub certificate_sha256: String,
    pub encoded_bytes: u32,
    // Decimal FILETIME strings preserve all 100ns ticks beyond JS's safe range.
    pub not_before_file_time: String,
    pub not_after_file_time: String,
    pub provider_error: u32,
    pub chain_error_status: Option<u32>,
    pub chain_info_status: Option<u32>,
    pub revocation_error: Option<u32>,
    pub trusted_root: bool,
}
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChainEvidence {
    pub signer_type: u32,
    pub provider_error: u32,
    pub verification_as_of_file_time: String,
    pub chain_error_status: Option<u32>,
    pub chain_info_status: Option<u32>,
    pub certificates: Vec<CertificateEvidence>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureEvidence {
    pub index: u32,
    pub native_status: String,
    pub verification: Verification,
    pub provider_error: Option<u32>,
    pub provider_final_error: Option<u32>,
    pub trust_step_errors: Vec<u32>,
    pub signer: Option<ChainEvidence>,
    pub timestamps: Vec<ChainEvidence>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub protocol_version: u8,
    pub size: u64,
    pub sha256: String,
    pub file_identity: Identity,
    pub identity_sha256: String,
    pub policy: &'static str,
    pub signatures: Vec<SignatureEvidence>,
    pub publisher_qualification: bool,
    pub current_qualification: bool,
    pub installation_rights: bool,
    pub verified_execution_object: bool,
    pub trusted_deadline_evidence: bool,
}
/// Holds the read-only, write/delete-denying actual handle until dropped.
/// The report is an observation, never an execution/permission constructor.
pub struct Observation {
    report: Report,
    _file: File,
    _directory: Option<Directory>,
}
impl Observation {
    pub fn report(&self) -> &Report {
        &self.report
    }
}
fn cancelled(cancel: &mut dyn FnMut() -> bool) -> Result<(), Error> {
    if cancel() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
fn full_hash(
    file: &mut File,
    expected: &ExpectedFile,
    cancel: &mut dyn FnMut() -> bool,
) -> Result<String, Error> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| Error::IoUnavailable)?;
    let mut sha = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        cancelled(cancel)?;
        let n = file.read(&mut buffer).map_err(|_| Error::IoUnavailable)?;
        cancelled(cancel)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or(Error::ByteMismatch)?;
        if total > expected.size {
            return Err(Error::ByteMismatch);
        }
        sha.update(&buffer[..n]);
    }
    let value: String = sha.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if total != expected.size || value != expected.sha256 {
        return Err(Error::ByteMismatch);
    }
    Ok(value)
}
fn require_pe(file: &mut File, size: u64) -> Result<(), Error> {
    let mut dos = [0u8; 64];
    file.seek(SeekFrom::Start(0))
        .map_err(|_| Error::IoUnavailable)?;
    file.read_exact(&mut dos).map_err(|_| Error::Unsupported)?;
    if &dos[..2] != b"MZ" {
        return Err(Error::Unsupported);
    }
    let offset = u32::from_le_bytes(dos[60..64].try_into().map_err(|_| Error::Unsupported)?) as u64;
    if offset < 64 || offset.checked_add(26).is_none_or(|end| end > size) {
        return Err(Error::Unsupported);
    }
    let mut header = [0u8; 26];
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| Error::IoUnavailable)?;
    file.read_exact(&mut header)
        .map_err(|_| Error::Unsupported)?;
    if &header[..4] != b"PE\0\0"
        || !matches!(u16::from_le_bytes([header[24], header[25]]), 0x10b | 0x20b)
    {
        return Err(Error::Unsupported);
    }
    Ok(())
}
/// Reopens the supplied actual object with stricter sharing, never its path.
/// Existing write/delete handles cause refusal; no try_clone share-mode claim.
pub fn observe_file(
    file: &File,
    expected: &ExpectedFile,
    cancel: &AtomicBool,
) -> Result<Observation, Error> {
    observe_file_with_poll(file, expected, || cancel.load(Ordering::Acquire))
}

/// Cooperative cancellation for callers with an original validation budget.
/// Polls surround native calls and every hash chunk; a native call is never
/// forcefully interrupted. This creates no deadline or execution authority.
pub fn observe_file_with_poll(
    file: &File,
    expected: &ExpectedFile,
    mut poll: impl FnMut() -> bool,
) -> Result<Observation, Error> {
    let mut stopped = false;
    observe_file_polled(file, expected, &mut || {
        stopped |= poll();
        stopped
    })
}

fn observe_file_polled(
    file: &File,
    expected: &ExpectedFile,
    cancel: &mut dyn FnMut() -> bool,
) -> Result<Observation, Error> {
    cancelled(cancel)?;
    let mut locked = platform::lock(file)?;
    let identity = pinvou_upgrade_custody::identity(&locked)?;
    let identity_sha256 = identity_hash(&identity)?;
    if expected
        .identity_sha256
        .as_ref()
        .is_some_and(|e| e != &identity_sha256)
    {
        return Err(Error::IdentityMismatch);
    }
    if locked.metadata().map_err(|_| Error::IoUnavailable)?.len() != expected.size {
        return Err(Error::ByteMismatch);
    }
    let sha256 = full_hash(&mut locked, expected, cancel)?;
    require_pe(&mut locked, expected.size)?;
    let signatures = platform::inspect(&locked, cancel)?;
    cancelled(cancel)?;
    // Also detect mapped writes or metadata changes that are observable during
    // collection. This is not a protected host or proof against privileged ABA.
    full_hash(&mut locked, expected, cancel)?;
    if pinvou_upgrade_custody::identity(&locked)? != identity {
        return Err(Error::UnsafeObject);
    }
    Ok(Observation {
        report: Report {
            protocol_version: 1,
            size: expected.size,
            sha256,
            file_identity: identity,
            identity_sha256,
            policy: "embedded-authenticode/cache-only/whole-chain-exclude-root/no-ui/no-md2-md4",
            signatures,
            publisher_qualification: false,
            current_qualification: false,
            installation_rights: false,
            verified_execution_object: false,
            trusted_deadline_evidence: false,
        },
        _file: locked,
        _directory: None,
    })
}
/// Raw-file CLI entry: each namespace component is opened relative to retained
/// real directory handles. UNC, streams, special/reparse objects are refused.
pub fn observe_path(
    path: &Path,
    expected: &ExpectedFile,
    cancel: &AtomicBool,
) -> Result<Observation, Error> {
    cancelled(&mut || cancel.load(Ordering::Acquire))?;
    let path = std::path::absolute(path).map_err(|_| Error::InvalidInput)?;
    let parent = pinvou_upgrade_custody::directory(path.parent().ok_or(Error::InvalidInput)?)?;
    let file = parent.file(
        path.file_name().ok_or(Error::InvalidInput)?,
        false,
        false,
        false,
    )?;
    let mut observation = observe_file(&file, expected, cancel)?;
    observation._directory = Some(parent);
    Ok(observation)
}

#[cfg(test)]
mod hash_poll_tests {
    use super::*;
    use std::{
        fs::{self, OpenOptions},
        io::Write,
    };

    #[test]
    fn actual_hash_checks_cancellation_between_chunks_and_keeps_input_intact() {
        let path = std::env::temp_dir().join(format!(
            "pinvou-native-hash-poll-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bytes = vec![23u8; 2 * 65_536];
        let hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let expected = ExpectedFile::new(bytes.len() as u64, &hash).unwrap();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        let mut polls = 0;
        assert!(matches!(
            full_hash(&mut file, &expected, &mut || {
                polls += 1;
                polls >= 3
            }),
            Err(Error::Cancelled)
        ));
        assert_eq!(file.stream_position().unwrap(), 65_536);
        assert_eq!(
            full_hash(&mut file, &expected, &mut || false).unwrap(),
            hash
        );
        assert!(matches!(
            observe_file_with_poll(&file, &expected, || true),
            Err(Error::Cancelled)
        ));
        drop(file);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::remove_file(path).unwrap();
    }
}

/// CLI-only process cancellation registration. Native verification is synchronous:
/// cancellation is returned after the call finishes and its state is closed.
pub use platform::ConsoleCancellation;
