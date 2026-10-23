//! Immutable independent bootstrap anchor, not current metadata acceptance.
//! No storage, system clock, native identity or online qualification is provided.
mod binding;
pub use binding::BootstrapBinding;

use pinvou_upgrade_codec::{Document, InputKind, decode_canonical_envelope};
use pinvou_upgrade_verifier::{VerificationInputs, verify_with_supplied_root};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const BOOTSTRAP_NAMESPACE: &str = "metadata-owner-bootstrap-v1";
pub const RECORD_MAXIMUM: usize = 4096;
pub const ROOT_MAXIMUM: usize = 1_048_576;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidBinding,
    RootPinMismatch,
    InvalidAnchor,
    InvalidRecord,
    ExistingStateMismatch,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidBinding => "METADATA_BOOTSTRAP_BINDING_INVALID",
            Self::RootPinMismatch => "METADATA_BOOTSTRAP_ROOT_PIN_MISMATCH",
            Self::InvalidAnchor => "METADATA_BOOTSTRAP_ANCHOR_INVALID",
            Self::InvalidRecord => "METADATA_BOOTSTRAP_RECORD_INVALID",
            Self::ExistingStateMismatch => "METADATA_BOOTSTRAP_EXISTING_STATE_MISMATCH",
        })
    }
}
impl std::error::Error for Error {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RootReference<'a> {
    size: u64,
    sha256: &'a str,
    version: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Record<'a> {
    metadata_bootstrap_version: u8,
    binding: &'a BootstrapBinding,
    initial_root: RootReference<'a>,
    integrity_basis: &'static str,
}

/// Original bytes with self-threshold integrity at their OWN signed issuedAt.
/// This is not an observed time, accepted current chain, rollback recovery,
/// fresh Root, installation permission or execution capability. Native storage
/// must bind the independently acquired pin to its actual first-install intent.
pub struct BootstrapAnchor {
    record: Vec<u8>,
    root: Vec<u8>,
}
impl BootstrapAnchor {
    pub fn new(
        binding: &BootstrapBinding,
        original_root: &[u8],
        expected_root_size: u64,
        expected_root_sha256: &str,
    ) -> Result<Self, Error> {
        binding.validate()?;
        if original_root.is_empty()
            || original_root.len() > ROOT_MAXIMUM
            || expected_root_size != original_root.len() as u64
            || !binding::digest(expected_root_sha256)
            || sha256(original_root) != expected_root_sha256
        {
            return Err(Error::RootPinMismatch);
        }
        let envelope = decode_canonical_envelope(original_root, InputKind::Metadata)
            .and_then(|value| {
                value
                    .check_full_schema()
                    .map_err(|_| pinvou_upgrade_codec::EnvelopeShapeError::InvalidEnvelope)
            })
            .map_err(|_| Error::InvalidAnchor)?;
        let signed = envelope.envelope().signed();
        let root = signed
            .root_policy_shape()
            .map_err(|_| Error::InvalidAnchor)?;
        if root.product() != binding.product() {
            return Err(Error::InvalidAnchor);
        }
        let expected = document(&serde_json::json!({
            "role": "root", "product": binding.product(), "component": null, "scope": {}
        }))?;
        // Reusing the signed timestamp is ONLY a deterministic integrity check.
        // It must never be recorded as a trusted current-clock observation.
        let issued_at = document(&signed.value()["issuedAt"])?;
        let integrity_time = issued_at
            .protocol_time_shape()
            .map_err(|_| Error::InvalidAnchor)?;
        verify_with_supplied_root(
            original_root,
            VerificationInputs::new(&root, &expected, &integrity_time),
        )
        .map_err(|_| Error::InvalidAnchor)?;
        let value = Record {
            metadata_bootstrap_version: 1,
            binding,
            initial_root: RootReference {
                size: expected_root_size,
                sha256: expected_root_sha256,
                version: root.version(),
            },
            integrity_basis: "self-threshold-at-signed-issued-at",
        };
        let record = document(&value)?
            .canonical_bytes()
            .map_err(|_| Error::InvalidRecord)?;
        if record.len() > RECORD_MAXIMUM {
            return Err(Error::InvalidRecord);
        }
        Ok(Self {
            record,
            root: original_root.to_vec(),
        })
    }
    /// The expected immutable genesis is recomputed through the same real
    /// signature checks. Unknown/partial/changed/noncanonical records refuse.
    pub fn parse(
        record: &[u8],
        original_root: &[u8],
        binding: &BootstrapBinding,
        expected_root_size: u64,
        expected_root_sha256: &str,
    ) -> Result<Self, Error> {
        if record.is_empty() || record.len() > RECORD_MAXIMUM {
            return Err(Error::InvalidRecord);
        }
        let expected = Self::new(
            binding,
            original_root,
            expected_root_size,
            expected_root_sha256,
        )?;
        expected.check_existing(record, original_root)?;
        Ok(expected)
    }
    pub fn record_bytes(&self) -> &[u8] {
        &self.record
    }
    pub fn root_bytes(&self) -> &[u8] {
        &self.root
    }
    pub fn check_existing(&self, record: &[u8], original_root: &[u8]) -> Result<(), Error> {
        if record.is_empty() || record.len() > RECORD_MAXIMUM {
            return Err(Error::InvalidRecord);
        }
        if record != self.record.as_slice() || original_root != self.root.as_slice() {
            return Err(Error::ExistingStateMismatch);
        }
        Ok(())
    }
}
fn document(value: &impl Serialize) -> Result<Document, Error> {
    let raw = serde_json::to_vec(value).map_err(|_| Error::InvalidRecord)?;
    Document::parse(&raw, InputKind::Metadata).map_err(|_| Error::InvalidRecord)
}
fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests;
