//! One caller-supplied Root successor; no installation or read-set completeness proof.
use crate::{VerificationError, parse_bound_envelope, signatures};
use pinvou_upgrade_codec::{
    Document, RootPolicyShape, SchemaCheckedEnvelope, SuppliedProtocolTime,
};
use std::collections::BTreeSet;

/// An original active metadata envelope and its independently expected complete
/// context. The caller owns snapshot consistency and read-set completeness.
pub struct ActiveMetadataInput<'a> {
    bytes: &'a [u8],
    expected: &'a Document,
}
impl<'a> ActiveMetadataInput<'a> {
    pub fn new(bytes: &'a [u8], expected: &'a Document) -> Self {
        Self { bytes, expected }
    }
}

/// Actual dual-threshold checks under the exact supplied old Root. This neither
/// installs the next Root nor attests that the caller supplied all active data.
pub struct SignatureCheckedRootSuccessor<'a> {
    envelope: SchemaCheckedEnvelope,
    old_root: &'a RootPolicyShape,
    next_root: RootPolicyShape,
    signer_ids: BTreeSet<String>,
    supplied_now: f64,
}
impl SignatureCheckedRootSuccessor<'_> {
    pub fn envelope(&self) -> &SchemaCheckedEnvelope {
        &self.envelope
    }
    pub fn supplied_old_root(&self) -> &RootPolicyShape {
        self.old_root
    }
    pub fn checked_next_root(&self) -> &RootPolicyShape {
        &self.next_root
    }
    pub fn verified_signer_ids(&self) -> &BTreeSet<String> {
        &self.signer_ids
    }
    pub fn supplied_time_millis(&self) -> f64 {
        self.supplied_now
    }
}

/// Verify exactly one continuous, fresh successor against independently supplied
/// complete Root context (role root, product, component null, empty scope).
/// Old expiry is allowed only here; original window bounds and issued time still apply. The active read set
/// is mandatory, may explicitly be empty/repeated, and is checked in full. Its
/// entries undergo only full Schema/context and next-key signature checks: no
/// current body time, Production restriction or new aggregate limit is imposed.
pub fn verify_root_successor<'a>(
    bytes: &[u8],
    old_root: &'a RootPolicyShape,
    expected_root: &Document,
    now: &SuppliedProtocolTime,
    active_metadata: &[ActiveMetadataInput<'_>],
) -> Result<SignatureCheckedRootSuccessor<'a>, VerificationError> {
    let parts = verify_root_transition_parts(
        bytes,
        old_root,
        expected_root,
        now,
        active_metadata,
        NextRootWindow::Fresh,
    )?;
    Ok(SignatureCheckedRootSuccessor {
        envelope: parts.envelope,
        old_root,
        next_root: parts.next_root,
        signer_ids: parts.signer_ids,
        supplied_now: now.as_millis(),
    })
}

// Private owned transition parts carry no old-Root borrow and no public claim
// that an expired intermediate is a fresh successor. Only these fixed callers
// choose the mode: the single operation always Fresh; chain uses history solely
// for nonfinal steps. No mode is supplied by an envelope or public API option.
pub(super) enum NextRootWindow {
    Fresh,
    HistoricalIntermediate,
}
pub(super) struct TransitionParts {
    pub(super) envelope: SchemaCheckedEnvelope,
    pub(super) next_root: RootPolicyShape,
    pub(super) signer_ids: BTreeSet<String>,
}
pub(super) fn verify_root_transition_parts(
    bytes: &[u8],
    old_root: &RootPolicyShape,
    expected_root: &Document,
    now: &SuppliedProtocolTime,
    active_metadata: &[ActiveMetadataInput<'_>],
    next_window: NextRootWindow,
) -> Result<TransitionParts, VerificationError> {
    if expected_root.value().get("role").and_then(|v| v.as_str()) != Some("root") {
        return Err(VerificationError::Context(
            pinvou_upgrade_codec::ContextBindingError::InvalidExpected,
        ));
    }
    let envelope = parse_bound_envelope(bytes, expected_root)?;
    old_root
        .assert_supplied_historical_window(now)
        .map_err(VerificationError::Time)?;
    let next_root = envelope
        .envelope()
        .signed()
        .root_policy_shape()
        .map_err(VerificationError::Root)?;
    if old_root.product() != next_root.product()
        || old_root.version() >= 9_007_199_254_740_991
        || next_root.version() != old_root.version() + 1
    {
        return Err(VerificationError::RootContinuityInvalid);
    }
    let signer_ids =
        signatures::verify_root_transition_signatures(&envelope, old_root, &next_root)?;
    match next_window {
        NextRootWindow::Fresh => next_root.assert_supplied_current_window(now),
        NextRootWindow::HistoricalIntermediate => next_root.assert_supplied_historical_window(now),
    }
    .map_err(VerificationError::Time)?;
    for current in active_metadata {
        let active = parse_bound_envelope(current.bytes, current.expected)?;
        if !matches!(
            active.envelope().signed().value()["role"].as_str(),
            Some("timestamp" | "snapshot" | "target" | "release" | "package")
        ) {
            return Err(VerificationError::ActiveReadSetInvalid);
        }
        signatures::verify_signatures(&active, &next_root)?;
    }
    Ok(TransitionParts {
        envelope,
        next_root,
        signer_ids,
    })
}
