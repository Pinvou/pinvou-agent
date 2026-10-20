//! Historical signed identities only; protected archive lineage stays caller-owned.
use crate::{VerificationError, parse_bound_envelope, signatures};
use pinvou_upgrade_codec::{
    Document, InputKind, RootPolicyShape, RootWindowMode, SchemaCheckedEnvelope,
    SuppliedProtocolTime, assert_supplied_metadata_semantics,
};
use std::collections::BTreeSet;

/// Original signature-checked historical identity, separate from a fresh ordinary
/// envelope proof. This neither attests archive custody nor grants present rights.
pub struct ArchivedEnvelopeIdentity<'a> {
    envelope: SchemaCheckedEnvelope,
    archived_root: &'a RootPolicyShape,
    signer_ids: BTreeSet<String>,
}
impl ArchivedEnvelopeIdentity<'_> {
    pub fn envelope(&self) -> &SchemaCheckedEnvelope {
        &self.envelope
    }
    pub fn supplied_archived_root(&self) -> &RootPolicyShape {
        self.archived_root
    }
    pub fn verified_signer_ids(&self) -> &BTreeSet<String> {
        &self.signer_ids
    }
}

/// Production Package identity under supplied protected archive material. Only
/// Root expiry is permitted; original window and issued<=suppliednow still apply.
/// No current qualification, high-water, denial or execution authority is added.
pub fn verify_archived_package<'a>(
    bytes: &[u8],
    archived_root: &'a RootPolicyShape,
    expected: &Document,
    now: &SuppliedProtocolTime,
) -> Result<ArchivedEnvelopeIdentity<'a>, VerificationError> {
    let envelope = parse_archive_envelope(bytes, expected, "package")?;
    archived_root
        .assert_supplied_historical_window(now)
        .map_err(VerificationError::Time)?;
    let signer_ids = signatures::verify_signatures(&envelope, archived_root)?;
    assert_supplied_metadata_semantics(&envelope, now, RootWindowMode::Current)
        .map_err(VerificationError::Metadata)?;
    Ok(ArchivedEnvelopeIdentity {
        envelope,
        archived_root,
        signer_ids,
    })
}

/// Release original identity using each original object's own issuedAt as its
/// window-check point, matching the frozen operation. No current-now parameter
/// or cross-object issuance ordering is introduced. Protected lineage is supplied.
pub fn verify_archived_release<'a>(
    bytes: &[u8],
    archived_root: &'a RootPolicyShape,
    expected: &Document,
) -> Result<ArchivedEnvelopeIdentity<'a>, VerificationError> {
    let envelope = parse_archive_envelope(bytes, expected, "release")?;
    let root_claims = Document::parse(archived_root.canonical_bytes(), InputKind::Metadata)
        .map_err(VerificationError::Json)?;
    let root_issued = original_issue_time(&root_claims)?;
    archived_root
        .assert_supplied_current_window(&root_issued)
        .map_err(VerificationError::Time)?;
    let signer_ids = signatures::verify_signatures(&envelope, archived_root)?;
    let release_issued = original_issue_time(envelope.envelope().signed())?;
    assert_supplied_metadata_semantics(&envelope, &release_issued, RootWindowMode::Current)
        .map_err(VerificationError::Metadata)?;
    Ok(ArchivedEnvelopeIdentity {
        envelope,
        archived_root,
        signer_ids,
    })
}
fn parse_archive_envelope(
    bytes: &[u8],
    expected: &Document,
    role: &str,
) -> Result<SchemaCheckedEnvelope, VerificationError> {
    if expected
        .value()
        .get("role")
        .and_then(|value| value.as_str())
        != Some(role)
    {
        return Err(VerificationError::ArchiveRoleInvalid);
    }
    parse_bound_envelope(bytes, expected)
}
// Only a Schema-checked Release or this same validated Root's canonical claims
// reach here. The JSON number is bounded and safe; this derives a claim's time,
// never a current clock observation or a public unbounded time-input factory.
fn original_issue_time(claims: &Document) -> Result<SuppliedProtocolTime, VerificationError> {
    let text = claims.value()["issuedAt"].to_string();
    Document::parse(text.as_bytes(), InputKind::Metadata)
        .map_err(VerificationError::Json)?
        .protocol_time_shape()
        .map_err(VerificationError::Time)
}
