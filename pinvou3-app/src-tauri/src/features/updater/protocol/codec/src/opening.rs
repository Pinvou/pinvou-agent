//! Supplied Target/hash/opening consistency only, not trusted membership.
use crate::{Document, Error, RolloutCommitmentError};
use serde_json::Value;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpeningBindingError {
    Json(Error),
    Commitment(RolloutCommitmentError),
    Mismatch,
}
impl std::fmt::Display for OpeningBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::Commitment(error) => error.fmt(f),
            Self::Mismatch => f.write_str("ROLLOUT_OPENING_MISMATCH"),
        }
    }
}
impl std::error::Error for OpeningBindingError {}
/// Check supplied signed-body Target, JSON hash string and nonnull opening.
/// All three documents have original complete raw bounds/duplicate validation.
/// Target extras are ignored here, not closed or full-Schema/trust validated.
/// A success proves only supplied-value consistency, never current membership,
/// signatures, product/component context, selection, freshness or execution rights.
pub fn assert_rollout_opening(
    target: &Document,
    release_hash: &Document,
    opening: &Document,
) -> Result<(), OpeningBindingError> {
    if opening.value.is_null() {
        return Err(OpeningBindingError::Mismatch);
    }
    let commitment = opening
        .rollout_commitment()
        .map_err(OpeningBindingError::Commitment)?;
    let target_key = target
        .value
        .get("scope")
        .and_then(|scope| scope.get("targetKey"))
        .and_then(Value::as_str);
    // Closed opening validation ensures these two fields are valid ASCII strings;
    // equality with its lower64 hash / computed hash also rejects noncanonical
    // counterpart types, case, padding and line endings without coercion.
    if target_key != opening.value["targetKey"].as_str()
        || release_hash.value.as_str() != opening.value["releaseEnvelopeSha256"].as_str()
        || target
            .value
            .get("rolloutSetCommitment")
            .and_then(Value::as_str)
            != Some(commitment.commitment_hash())
    {
        return Err(OpeningBindingError::Mismatch);
    }
    Ok(())
}
