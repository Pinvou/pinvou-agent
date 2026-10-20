//! Authenticated immutable Release renewal; protected acceptance stays external.
use crate::{
    ArchivedEnvelopeIdentity, SignatureCheckedEnvelope, VerificationError, VerificationInputs,
    verify_archived_release, verify_with_supplied_root,
};
use pinvou_upgrade_codec::{
    Document, Error, InputKind, ReleaseRenewalError, RootPolicyShape, SuppliedProtocolTime,
    assert_supplied_release_renewal,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseRenewalVerificationError {
    Verification(VerificationError),
    Binding(ReleaseRenewalError),
    Json(Error),
}
impl std::fmt::Display for ReleaseRenewalVerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verification(e) => e.fmt(f),
            Self::Binding(e) => e.fmt(f),
            Self::Json(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for ReleaseRenewalVerificationError {}
/// Distinct caller-owned archived/current trust, independent identity and safe time.
pub struct ReleaseRenewalInputs<'a> {
    archived_root: &'a RootPolicyShape,
    current_root: &'a RootPolicyShape,
    context: &'a Document,
    now: &'a SuppliedProtocolTime,
    denied: &'a [String],
}
impl<'a> ReleaseRenewalInputs<'a> {
    pub fn new(
        archived_root: &'a RootPolicyShape,
        current_root: &'a RootPolicyShape,
        context: &'a Document,
        now: &'a SuppliedProtocolTime,
    ) -> Self {
        Self {
            archived_root,
            current_root,
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
/// Distinct historical and current authenticity plus immutable-business relation.
/// Root lineage/archive custody, high-water/cache replacement and full chain
/// publication/qualification remain separate; this grants no execution rights.
pub struct SignatureCheckedReleaseRenewal<'a> {
    previous: ArchivedEnvelopeIdentity<'a>,
    next: SignatureCheckedEnvelope<'a>,
}
impl<'a> SignatureCheckedReleaseRenewal<'a> {
    pub fn previous(&self) -> &ArchivedEnvelopeIdentity<'a> {
        &self.previous
    }
    pub fn next(&self) -> &SignatureCheckedEnvelope<'a> {
        &self.next
    }
}
/// Compose the three unchanged full operations. Historical identity uses each
/// original object's own issuedAt, without a retroactive current deny/window.
/// Next must be current and have strictly greater (not necessarily consecutive)
/// revision. Every other signed business field remains exactly unchanged.
pub fn verify_release_renewal<'a>(
    previous_bytes: &[u8],
    next_bytes: &[u8],
    inputs: ReleaseRenewalInputs<'a>,
) -> Result<SignatureCheckedReleaseRenewal<'a>, ReleaseRenewalVerificationError> {
    let context = inputs.context.value();
    let expected=Document::parse(format!(
        "{{\"role\":\"release\",\"product\":{},\"component\":{},\"scope\":{{\"releaseId\":{}}}}}",
        context["product"],context["component"],context["releaseId"],
    ).as_bytes(), InputKind::Metadata).map_err(ReleaseRenewalVerificationError::Json)?;
    let previous = verify_archived_release(previous_bytes, inputs.archived_root, &expected)
        .map_err(ReleaseRenewalVerificationError::Verification)?;
    let verified = verify_with_supplied_root(
        next_bytes,
        VerificationInputs::new(inputs.current_root, &expected, inputs.now)
            .with_denied_key_ids(inputs.denied),
    )
    .map_err(ReleaseRenewalVerificationError::Verification)?;
    assert_supplied_release_renewal(previous.envelope(), verified.envelope())
        .map_err(ReleaseRenewalVerificationError::Binding)?;
    // Ordinary proof stores only its owned envelope/signers/time and Root, never
    // the temporary expected context. Preserve those parts and the exact input
    // current Root lifetime; no reference to local expected identity escapes.
    let next = SignatureCheckedEnvelope {
        envelope: verified.envelope,
        signer_ids: verified.signer_ids,
        supplied_now: verified.supplied_now,
        root: inputs.current_root,
    };
    Ok(SignatureCheckedReleaseRenewal { previous, next })
}
