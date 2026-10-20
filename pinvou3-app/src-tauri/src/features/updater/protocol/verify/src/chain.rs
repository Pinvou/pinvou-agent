//! Complete pure Root traversal; protected chain storage remains caller-owned.
use crate::successor::{NextRootWindow, verify_root_transition_parts};
use crate::{ActiveMetadataInput, VerificationError};
use pinvou_upgrade_codec::{
    Document, RootPolicyShape, SchemaCheckedEnvelope, SuppliedProtocolTime,
};
use std::collections::BTreeSet;

/// Every original supplied step was checked. Only the final original envelope
/// and Root are retained; callers own original intermediate persistence and the
/// consistency/completeness of the active metadata snapshot. No current rights.
pub struct SignatureCheckedRootChain<'a> {
    initial_root: &'a RootPolicyShape,
    final_envelope: SchemaCheckedEnvelope,
    final_root: RootPolicyShape,
    final_signer_ids: BTreeSet<String>,
    checked_steps: usize,
    supplied_now: f64,
}
impl SignatureCheckedRootChain<'_> {
    pub fn supplied_initial_root(&self) -> &RootPolicyShape {
        self.initial_root
    }
    pub fn final_envelope(&self) -> &SchemaCheckedEnvelope {
        &self.final_envelope
    }
    pub fn checked_final_root(&self) -> &RootPolicyShape {
        &self.final_root
    }
    pub fn verified_final_signer_ids(&self) -> &BTreeSet<String> {
        &self.final_signer_ids
    }
    pub fn checked_steps(&self) -> usize {
        self.checked_steps
    }
    pub fn supplied_time_millis(&self) -> f64 {
        self.supplied_now
    }
}

/// One through 4096 continuous dual-threshold steps; only nonfinal Root expiry
/// is allowed. Active metadata is checked against the final fresh Root alone.
/// The original per-object input limits still apply. No new aggregate byte or
/// active-set size policy, protected acceptance or intermediate storage is added.
pub fn verify_root_chain<'a>(
    successors: &[&[u8]],
    initial_root: &'a RootPolicyShape,
    expected_root: &Document,
    now: &SuppliedProtocolTime,
    active_metadata: &[ActiveMetadataInput<'_>],
) -> Result<SignatureCheckedRootChain<'a>, VerificationError> {
    if !(1..=4096).contains(&successors.len()) {
        return Err(VerificationError::RootChainInvalid);
    }
    let mut current = None;
    for (index, bytes) in successors.iter().enumerate() {
        let old = current
            .as_ref()
            .map(|parts: &crate::successor::TransitionParts| &parts.next_root)
            .unwrap_or(initial_root);
        let final_step = index + 1 == successors.len();
        let parts = verify_root_transition_parts(
            bytes,
            old,
            expected_root,
            now,
            if final_step { active_metadata } else { &[] },
            if final_step {
                NextRootWindow::Fresh
            } else {
                NextRootWindow::HistoricalIntermediate
            },
        )?;
        // Parts own no old-Root borrow. Replacing current releases the previous
        // envelope/cache each step, without manufacturing an expired public proof.
        current = Some(parts);
    }
    let parts = current.ok_or(VerificationError::RootChainInvalid)?;
    Ok(SignatureCheckedRootChain {
        initial_root,
        final_envelope: parts.envelope,
        final_root: parts.next_root,
        final_signer_ids: parts.signer_ids,
        checked_steps: successors.len(),
        supplied_now: now.as_millis(),
    })
}
