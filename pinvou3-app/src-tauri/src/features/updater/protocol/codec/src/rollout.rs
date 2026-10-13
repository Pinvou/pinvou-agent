//! Frozen rollout commitment math; not membership, freshness or qualification.
use crate::shape::{TARGETS, closed, identifier, one_of, positive_safe_integer, sha256_hex};
use crate::{Document, Error};
use serde_json::Value;
use sha2::{Digest, Sha256};

const DOMAIN: &[u8] = b"pinvou-rollout-set-v1\0";
const FIELDS: [&str; 7] = [
    "targetKey",
    "deploymentId",
    "deploymentRevision",
    "rolloutId",
    "rolloutRevision",
    "releaseEnvelopeSha256",
    "leafSalt",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RolloutCommitmentError {
    Json(Error),
    InvalidOpening,
}
impl std::fmt::Display for RolloutCommitmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::InvalidOpening => f.write_str("ROLLOUT_OPENING_INVALID"),
        }
    }
}
impl std::error::Error for RolloutCommitmentError {}

/// Exact validated preimage/hash only. Leaf salt shape does not attest entropy
/// or freshness. No current membership, signature, eligibility or rights follows.
/// Private construction and no Debug/Serialize avoid automatic opening logs.
pub struct RolloutCommitment {
    input: Vec<u8>,
    hash: String,
}
impl RolloutCommitment {
    pub fn input_bytes(&self) -> &[u8] {
        &self.input
    }
    pub fn commitment_hash(&self) -> &str {
        &self.hash
    }
}
impl Document {
    /// Only literal JSON null means the empty set; otherwise require one closed
    /// opening. The fixed array is constructed here, never accepted from callers.
    pub fn rollout_commitment(&self) -> Result<RolloutCommitment, RolloutCommitmentError> {
        let entries = if self.value.is_null() {
            Vec::new()
        } else {
            let opening =
                closed(&self.value, &FIELDS).ok_or(RolloutCommitmentError::InvalidOpening)?;
            if !one_of(&opening["targetKey"], &TARGETS)
                || !identifier(&opening["deploymentId"])
                || !identifier(&opening["rolloutId"])
                || !positive_safe_integer(&opening["deploymentRevision"])
                || !positive_safe_integer(&opening["rolloutRevision"])
                || !sha256_hex(&opening["releaseEnvelopeSha256"])
                || !sha256_hex(&opening["leafSalt"])
            {
                return Err(RolloutCommitmentError::InvalidOpening);
            }
            vec![self.value.clone()]
        };
        // Reuse the bounded JCS writer for this fixed, validated wrapper. The
        // additional array is also included in canonical output resource bounds.
        let payload = Document {
            value: Value::Array(entries),
        }
        .canonical_bytes()
        .map_err(RolloutCommitmentError::Json)?;
        let mut input = Vec::with_capacity(DOMAIN.len() + payload.len());
        input.extend_from_slice(DOMAIN);
        input.extend_from_slice(&payload);
        let hash = Sha256::digest(&input)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(RolloutCommitment { input, hash })
    }
}
