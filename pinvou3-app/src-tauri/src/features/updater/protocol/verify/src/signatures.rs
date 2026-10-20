use crate::VerificationError;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pinvou_upgrade_codec::{RootPolicyShape, SchemaCheckedEnvelope};
use std::collections::BTreeSet;

// Keys and exact role policy come only from the same supplied Root shape. There
// is no caller-selected projection, verifier callback or threshold-only shortcut.
pub(crate) fn verify_signatures(
    envelope: &SchemaCheckedEnvelope,
    root: &RootPolicyShape,
) -> Result<BTreeSet<String>, VerificationError> {
    verify_policy_roots(envelope, &[root])
}

pub(crate) fn verify_root_transition_signatures(
    envelope: &SchemaCheckedEnvelope,
    old: &RootPolicyShape,
    next: &RootPolicyShape,
) -> Result<BTreeSet<String>, VerificationError> {
    verify_policy_roots(envelope, &[old, next])
}

// Only whole validated Root shapes enter this private worker. The ordinary
// entry uses one Root; the transition uses old then next. Next descriptors win
// duplicate IDs just as in the frozen reference's merged map. Every policy is
// selected from that same Root and original context; no projected public keys.
fn verify_policy_roots(
    envelope: &SchemaCheckedEnvelope,
    roots: &[&RootPolicyShape],
) -> Result<BTreeSet<String>, VerificationError> {
    let original = envelope.envelope();
    let context = original
        .signed()
        .role_context_shape()
        .map_err(VerificationError::Policy)?;
    let policies = roots
        .iter()
        .map(|root| {
            root.declared_policy_for(&context)
                .map_err(VerificationError::Policy)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let allowed: BTreeSet<&str> = policies
        .iter()
        .flat_map(|policy| policy.key_ids().iter().map(String::as_str))
        .collect();
    let input = original
        .signed()
        .signing_input()
        .map_err(VerificationError::Binding)?;
    let mut verified = BTreeSet::new();
    for signature in original.signatures() {
        let id = signature.key_id();
        if !allowed.contains(id) {
            return Err(VerificationError::SigningKeyUnauthorized);
        }
        let descriptor = roots
            .iter()
            .rev()
            .find_map(|root| root.declared_key_for(id))
            .ok_or(VerificationError::SigningKeyUnauthorized)?;
        let key: [u8; 32] = URL_SAFE_NO_PAD
            .decode(descriptor.public_key())
            .map_err(|_| VerificationError::InvalidDescriptor)?
            .try_into()
            .map_err(|_| VerificationError::InvalidDescriptor)?;
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
            .verify(input.as_bytes(), signature.as_bytes())
            .map_err(|_| VerificationError::InvalidSignature)?;
        verified.insert(id.to_owned());
    }
    for policy in &policies {
        if policy
            .key_ids()
            .iter()
            .filter(|id| verified.contains(*id))
            .count()
            < usize::from(policy.threshold())
        {
            return Err(VerificationError::ThresholdUnmet);
        }
    }
    if let Some(id) = original.signed().value().get("signingKeyId")
        && id.as_str().is_none_or(|id| !verified.contains(id))
    {
        return Err(VerificationError::CredentialKeyBinding);
    }
    Ok(verified)
}
