//! Metadata reference shape and exact original wire identity, not trust or time.
use crate::shape::{closed, identifier, one_of, positive_safe_integer, sha256_hex};
use crate::{Document, Error, InputKind};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataReferenceError {
    Json(Error),
    InvalidReference,
    MissingEnvelopeFields,
    Mismatch,
}
impl std::fmt::Display for MetadataReferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::InvalidReference => f.write_str("METADATA_REFERENCE_INVALID"),
            Self::MissingEnvelopeFields => f.write_str("METADATA_REFERENCE_FIELDS_REQUIRED"),
            Self::Mismatch => f.write_str("METADATA_REFERENCE_MISMATCH"),
        }
    }
}
impl std::error::Error for MetadataReferenceError {}
/// Complete declared reference or identity derived from current original bytes.
/// No Debug/Serialize/public constructor; this proves no authentic envelope,
/// complete envelope Schema, expected product/component, current time or trust.
pub struct MetadataReferenceShape {
    canonical: Vec<u8>,
}
impl MetadataReferenceShape {
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical
    }
    /// Parse and derive from these current original wire bytes, never from JCS
    /// reserialization or an earlier digest; compare all six declared fields.
    pub fn assert_original_envelope(&self, bytes: &[u8]) -> Result<(), MetadataReferenceError> {
        let actual = derive_metadata_reference(bytes)?;
        if self.canonical == actual.canonical {
            Ok(())
        } else {
            Err(MetadataReferenceError::Mismatch)
        }
    }
}
impl Document {
    pub fn metadata_reference_shape(
        &self,
    ) -> Result<MetadataReferenceShape, MetadataReferenceError> {
        let reference = closed(
            &self.value,
            &[
                "role",
                "scope",
                "version",
                "expiresAt",
                "size",
                "envelopeSha256",
            ],
        )
        .ok_or(MetadataReferenceError::InvalidReference)?;
        if !one_of(
            &reference["role"],
            &[
                "root",
                "timestamp",
                "snapshot",
                "target",
                "release",
                "package",
            ],
        ) || !positive_safe_integer(&reference["size"])
            || reference["size"]
                .as_f64()
                .is_none_or(|size| size > 1_048_576.0)
            || !sha256_hex(&reference["envelopeSha256"])
        {
            return Err(MetadataReferenceError::InvalidReference);
        }
        let fields: &[&str] = match reference["role"].as_str() {
            Some("target") => &["channel", "targetKey"],
            Some("release") => &["releaseId"],
            Some("package") => &["targetKey"],
            _ => &[],
        };
        let scope =
            closed(&reference["scope"], fields).ok_or(MetadataReferenceError::InvalidReference)?;
        if !scope.values().all(identifier) {
            return Err(MetadataReferenceError::InvalidReference);
        }
        if reference["role"].as_str() == Some("package") {
            if !reference["version"].is_null() || !reference["expiresAt"].is_null() {
                return Err(MetadataReferenceError::InvalidReference);
            }
        } else if !positive_safe_integer(&reference["version"])
            || !nonnegative_safe_integer(&reference["expiresAt"])
        {
            return Err(MetadataReferenceError::InvalidReference);
        }
        Ok(MetadataReferenceShape {
            canonical: self
                .canonical_bytes()
                .map_err(MetadataReferenceError::Json)?,
        })
    }
}
/// Derive the frozen envelopeReference projection only, not full envelope checks.
/// Every raw field still passes original bounds/duplicate detection. Original
/// bytes, including whitespace and extra claims, bind the size and SHA. The owner
/// authenticates the enclosing expected reference and verifies all trust/context.
pub fn derive_metadata_reference(
    bytes: &[u8],
) -> Result<MetadataReferenceShape, MetadataReferenceError> {
    let envelope =
        Document::parse(bytes, InputKind::Metadata).map_err(MetadataReferenceError::Json)?;
    let signed = envelope
        .value
        .as_object()
        .and_then(|object| object.get("signed"))
        .and_then(Value::as_object)
        .ok_or(MetadataReferenceError::MissingEnvelopeFields)?;
    let field = |name| {
        signed
            .get(name)
            .ok_or(MetadataReferenceError::MissingEnvelopeFields)
    };
    let role = field("role")?;
    let scope = field("scope")?;
    let version = match role.as_str() {
        Some("package") => Value::Null,
        Some("release") => field("revision")?.clone(),
        _ => field("version")?.clone(),
    };
    let expires = signed.get("expiresAt").cloned().unwrap_or(Value::Null);
    let hash: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Document {
        value: json!({"role":role,"scope":scope,"version":version,"expiresAt":expires,
        "size":bytes.len(),"envelopeSha256":hash}),
    }
    .metadata_reference_shape()
}
fn nonnegative_safe_integer(value: &Value) -> bool {
    value.as_f64().is_some_and(|number| {
        (0.0..=9_007_199_254_740_991.0).contains(&number) && number.fract() == 0.0
    })
}
