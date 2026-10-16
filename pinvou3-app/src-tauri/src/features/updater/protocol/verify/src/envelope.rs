//! The original role-specific wire profile, full Schema and independent context.
use crate::{METADATA_ROLES, VerificationError};
use pinvou_upgrade_codec::{
    ContextBindingError, Document, InputKind, SchemaCheckedEnvelope,
    assert_supplied_expected_context, decode_canonical_envelope,
};
pub(crate) fn parse_bound_envelope(
    bytes: &[u8],
    expected: &Document,
) -> Result<SchemaCheckedEnvelope, VerificationError> {
    let expected_role = expected
        .value()
        .get("role")
        .and_then(|value| value.as_str())
        .ok_or(VerificationError::Context(
            ContextBindingError::InvalidExpected,
        ))?;
    let kind = if METADATA_ROLES.contains(&expected_role) {
        InputKind::Metadata
    } else {
        InputKind::Credential
    };
    let envelope = decode_canonical_envelope(bytes, kind)
        .map_err(VerificationError::Envelope)?
        .check_full_schema()
        .map_err(VerificationError::Schema)?;
    assert_supplied_expected_context(&envelope, expected).map_err(VerificationError::Context)?;
    Ok(envelope)
}
