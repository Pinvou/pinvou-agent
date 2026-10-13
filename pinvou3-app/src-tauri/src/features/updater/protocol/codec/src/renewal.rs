//! Supplied Release renewal relationship only; no publication or key trust.
use crate::{BoundedOutput, EnvelopeShapeError, Error, SchemaCheckedEnvelope, SchemaError};
use serde::{
    Serialize,
    ser::{SerializeMap, Serializer},
};
use serde_json::Value;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseRenewalError {
    Json(Error),
    Envelope(EnvelopeShapeError),
    Schema(SchemaError),
    RevisionInvalid,
    BusinessChanged,
}
impl std::fmt::Display for ReleaseRenewalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::Envelope(e) => e.fmt(f),
            Self::Schema(e) => e.fmt(f),
            Self::RevisionInvalid => f.write_str("RELEASE_REVISION_INVALID"),
            Self::BusinessChanged => f.write_str("RELEASE_BUSINESS_CHANGED"),
        }
    }
}
impl std::error::Error for ReleaseRenewalError {}
/// Exact frozen assertReleaseRenewal over two full-Schema original envelopes.
/// Revision must increase, without a new consecutive-revision constraint.
/// Only revision/issuedAt/expiresAt are excluded; every other signed field binds.
/// Signature validity, trusted Root/history/time and atomic publication remain
/// separate obligations. This operation does not modify or publish either input.
pub fn assert_supplied_release_renewal(
    previous: &SchemaCheckedEnvelope,
    next: &SchemaCheckedEnvelope,
) -> Result<(), ReleaseRenewalError> {
    let previous = previous.envelope().signed().value();
    let next = next.envelope().signed().value();
    if previous["role"] != "release" || next["role"] != "release" {
        return Err(ReleaseRenewalError::RevisionInvalid);
    }
    let before = previous["revision"]
        .as_f64()
        .ok_or(ReleaseRenewalError::RevisionInvalid)?;
    let after = next["revision"]
        .as_f64()
        .ok_or(ReleaseRenewalError::RevisionInvalid)?;
    if after <= before {
        return Err(ReleaseRenewalError::RevisionInvalid);
    }
    if canonical(previous)? != canonical(next)? {
        return Err(ReleaseRenewalError::BusinessChanged);
    }
    Ok(())
}
struct Business<'a>(&'a Value);
impl Serialize for Business<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let object = self
            .0
            .as_object()
            .ok_or_else(|| serde::ser::Error::custom("release business object required"))?;
        let mut map = s.serialize_map(None)?;
        for (key, value) in object {
            if !["revision", "issuedAt", "expiresAt"].contains(&key.as_str()) {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}
fn canonical(value: &Value) -> Result<Vec<u8>, ReleaseRenewalError> {
    let mut output = BoundedOutput {
        bytes: Vec::new(),
        exceeded: false,
    };
    if serde_jcs::to_writer(&mut output, &Business(value)).is_err() {
        return Err(ReleaseRenewalError::Json(if output.exceeded {
            Error::OutputLimit
        } else {
            Error::Canonicalization
        }));
    }
    Ok(output.bytes)
}
