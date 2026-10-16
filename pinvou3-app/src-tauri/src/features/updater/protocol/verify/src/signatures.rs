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
    let original = envelope.envelope();
    let context = original
        .signed()
        .role_context_shape()
        .map_err(VerificationError::Policy)?;
    let policy = root
        .declared_policy_for(&context)
        .map_err(VerificationError::Policy)?;
    let allowed: BTreeSet<&str> = policy.key_ids().iter().map(String::as_str).collect();
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
        let descriptor = root
            .declared_key_for(id)
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
    if verified.len() < usize::from(policy.threshold()) {
        return Err(VerificationError::ThresholdUnmet);
    }
    if let Some(id) = original.signed().value().get("signingKeyId")
        && id.as_str().is_none_or(|id| !verified.contains(id))
    {
        return Err(VerificationError::CredentialKeyBinding);
    }
    Ok(verified)
}
