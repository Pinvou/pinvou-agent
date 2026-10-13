//! Supplied candidate opening and target relationship only, not trusted selection.
use crate::{
    Document, EnvelopeShapeError, Error, OpeningBindingError, SchemaCheckedEnvelope, SchemaError,
};
use sha2::{Digest, Sha256};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateBindingError {
    Json(Error),
    Envelope(EnvelopeShapeError),
    Schema(SchemaError),
    Opening(OpeningBindingError),
    RoleInvalid,
    IsBaseline,
    TargetMissing,
}
impl std::fmt::Display for CandidateBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::Envelope(e) => e.fmt(f),
            Self::Schema(e) => e.fmt(f),
            Self::Opening(e) => e.fmt(f),
            Self::RoleInvalid => f.write_str("CANDIDATE_ROLE_INVALID"),
            Self::IsBaseline => f.write_str("CANDIDATE_IS_BASELINE"),
            Self::TargetMissing => f.write_str("CANDIDATE_TARGET_MISSING"),
        }
    }
}
impl std::error::Error for CandidateBindingError {}
/// The three frozen verifyCandidateRelease relationships, with full Schema/encoding.
/// This does not verify signatures, independent product/component/channel/release
/// context, current membership, time or permission. No candidate is selected for use.
pub fn assert_supplied_candidate_binding(
    target: &SchemaCheckedEnvelope,
    release: &SchemaCheckedEnvelope,
    opening: &Document,
) -> Result<(), CandidateBindingError> {
    let target = target.envelope().signed();
    let release_body = release.envelope().signed().value();
    if target.value()["role"] != "target" || release_body["role"] != "release" {
        return Err(CandidateBindingError::RoleInvalid);
    }
    let hash: String = Sha256::digest(release.envelope().raw_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if target.value()["baselineRelease"]["envelopeSha256"] == hash {
        return Err(CandidateBindingError::IsBaseline);
    }
    // Internal scalar from this exact factory's bytes; not a generic unchecked
    // public Document entrance. Reuse the unchanged seven-field opening checker.
    let hash = Document {
        value: serde_json::Value::String(hash),
    };
    crate::assert_rollout_opening(target, &hash, opening)
        .map_err(CandidateBindingError::Opening)?;
    let present = release_body["targets"].as_array().is_some_and(|targets| {
        targets
            .iter()
            .any(|entry| entry["targetKey"] == target.value()["scope"]["targetKey"])
    });
    if !present {
        return Err(CandidateBindingError::TargetMissing);
    }
    Ok(())
}
