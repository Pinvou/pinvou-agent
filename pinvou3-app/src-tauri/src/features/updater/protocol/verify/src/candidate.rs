//! Signed Target and committed candidate Release under supplied existing trust.
use crate::{VerificationError, VerificationInputs, verify_with_supplied_root};
use pinvou_upgrade_codec::{
    CandidateBindingError, Document, Error, InputKind, RootPolicyShape, SchemaCheckedEnvelope,
    SuppliedProtocolTime, assert_supplied_candidate_binding,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateVerificationError {
    Verification(VerificationError),
    Binding(CandidateBindingError),
    Json(Error),
}
impl std::fmt::Display for CandidateVerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verification(error) => error.fmt(f),
            Self::Binding(error) => error.fmt(f),
            Self::Json(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for CandidateVerificationError {}

/// Independent bounded product/component/channel/targetKey/expectedReleaseId
/// context, supplied existing trust/time and global denial facts.
pub struct CandidateVerificationInputs<'a> {
    root: &'a RootPolicyShape,
    context: &'a Document,
    now: &'a SuppliedProtocolTime,
    denied: &'a [String],
}
impl<'a> CandidateVerificationInputs<'a> {
    pub fn new(
        root: &'a RootPolicyShape,
        context: &'a Document,
        now: &'a SuppliedProtocolTime,
    ) -> Self {
        Self {
            root,
            context,
            now,
            denied: &[],
        }
    }
    pub fn with_denied_key_ids(mut self, ids: &'a [String]) -> Self {
        self.denied = ids;
        self
    }
}

/// Original signed pair with checked opening/membership, not a selection,
/// custody attestation, persistent acceptance or current online permission.
pub struct SignatureCheckedCandidateRelease<'a> {
    target: SchemaCheckedEnvelope,
    release: SchemaCheckedEnvelope,
    root: &'a RootPolicyShape,
    supplied_now: f64,
}
impl SignatureCheckedCandidateRelease<'_> {
    pub fn target(&self) -> &SchemaCheckedEnvelope {
        &self.target
    }
    pub fn release(&self) -> &SchemaCheckedEnvelope {
        &self.release
    }
    pub fn supplied_root(&self) -> &RootPolicyShape {
        self.root
    }
    pub fn supplied_time_millis(&self) -> f64 {
        self.supplied_now
    }
}

/// Complete supplied candidate operation with fixed independent Target/Release
/// contexts and actual ordinary verification. No valid partial pair is returned.
pub fn verify_candidate_release<'a>(
    target_bytes: &[u8],
    release_bytes: &[u8],
    opening: &Document,
    inputs: CandidateVerificationInputs<'a>,
) -> Result<SignatureCheckedCandidateRelease<'a>, CandidateVerificationError> {
    let context = inputs.context.value();
    let target_expected = expected(format!(
        "{{\"role\":\"target\",\"product\":{},\"component\":{},\"scope\":{{\"channel\":{},\"targetKey\":{}}}}}",
        context["product"], context["component"], context["channel"], context["targetKey"],
    ))?;
    let target = verify_with_supplied_root(
        target_bytes,
        VerificationInputs::new(inputs.root, &target_expected, inputs.now)
            .with_denied_key_ids(inputs.denied),
    )
    .map_err(CandidateVerificationError::Verification)?
    .envelope;
    let release_expected = expected(format!(
        "{{\"role\":\"release\",\"product\":{},\"component\":{},\"scope\":{{\"releaseId\":{}}}}}",
        context["product"], context["component"], context["expectedReleaseId"],
    ))?;
    let release = verify_with_supplied_root(
        release_bytes,
        VerificationInputs::new(inputs.root, &release_expected, inputs.now)
            .with_denied_key_ids(inputs.denied),
    )
    .map_err(CandidateVerificationError::Verification)?
    .envelope;
    assert_supplied_candidate_binding(&target, &release, opening)
        .map_err(CandidateVerificationError::Binding)?;
    Ok(SignatureCheckedCandidateRelease {
        target,
        release,
        root: inputs.root,
        supplied_now: inputs.now.as_millis(),
    })
}
fn expected(text: String) -> Result<Document, CandidateVerificationError> {
    // Only JSON-escaped values from the already bounded independent Document
    // reach this constructor; no incoming claims supply expected identity.
    Document::parse(text.as_bytes(), InputKind::Metadata).map_err(CandidateVerificationError::Json)
}
