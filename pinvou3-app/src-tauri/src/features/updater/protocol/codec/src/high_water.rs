//! Supplied high-water comparison only. No protected acceptance or persistence.
use crate::shape::{positive_safe_integer, sha256_hex};
use crate::{BoundedOutput, CanonicalEnvelopeShape, Document, EnvelopeShapeError, Error};
use serde::{Serialize, Serializer, ser::SerializeStruct};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighWaterError {
    Json(Error),
    Envelope(EnvelopeShapeError),
    UnsupportedRole,
    InvalidVersion,
    InvalidRecord,
    ScopeMismatch,
    Rollback,
    Equivocation,
}
impl std::fmt::Display for HighWaterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Json(error) => return error.fmt(f),
            Self::Envelope(error) => return error.fmt(f),
            Self::UnsupportedRole => "HIGH_WATER_ROLE_INVALID",
            Self::InvalidVersion => "HIGH_WATER_VERSION_INVALID",
            Self::InvalidRecord => "HIGH_WATER_RECORD_INVALID",
            Self::ScopeMismatch => "HIGH_WATER_SCOPE_INVALID",
            Self::Rollback => "METADATA_ROLLBACK",
            Self::Equivocation => "METADATA_EQUIVOCATION",
        })
    }
}
impl std::error::Error for HighWaterError {}

struct Context<'a> {
    role: &'a Value,
    product: &'a Value,
    component: &'a Value,
    scope: &'a Value,
}

impl Serialize for Context<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut context = serializer.serialize_struct("HighWaterContext", 4)?;
        context.serialize_field("role", self.role)?;
        context.serialize_field("product", self.product)?;
        context.serialize_field("component", self.component)?;
        context.serialize_field("scope", self.scope)?;
        context.end()
    }
}

/// A required supplied record is compared, never treated as trusted or advanced.
/// The envelope remains a partial shape, not a verified accepted candidate.
/// Release uses revision only. Package and credential roles are unsupported.
pub fn assert_supplied_high_water(
    envelope: &CanonicalEnvelopeShape,
    high_water: &Document,
) -> Result<(), HighWaterError> {
    let signed = envelope.signed().value();
    let role = signed["role"]
        .as_str()
        .ok_or(HighWaterError::UnsupportedRole)?;
    let version_field = match role {
        "root" | "timestamp" | "snapshot" | "target" => "version",
        "release" => "revision",
        _ => return Err(HighWaterError::UnsupportedRole),
    };
    let version = signed
        .get(version_field)
        .filter(|v| positive_safe_integer(v))
        .and_then(Value::as_f64)
        .ok_or(HighWaterError::InvalidVersion)?;
    let record = high_water
        .value()
        .as_object()
        .ok_or(HighWaterError::InvalidRecord)?;
    let scope = record.get("scope").ok_or(HighWaterError::InvalidRecord)?;
    let context = Context {
        role: &signed["role"],
        product: &signed["product"],
        component: &signed["component"],
        scope: &signed["scope"],
    };
    // Both subtrees were completely resource-checked in their original input.
    // This four-field wrapper is shallower than the original full envelope.
    if canonical(&context)? != canonical(scope)? {
        return Err(HighWaterError::ScopeMismatch);
    }
    let previous = record
        .get("version")
        .filter(|v| positive_safe_integer(v))
        .and_then(Value::as_f64)
        .ok_or(HighWaterError::InvalidRecord)?;
    if version < previous {
        return Err(HighWaterError::Rollback);
    }
    if version == previous {
        let hash = record
            .get("envelopeSha256")
            .filter(|v| sha256_hex(v))
            .and_then(Value::as_str)
            .ok_or(HighWaterError::Equivocation)?;
        // Hash the factory's own complete original bytes, never a caller-supplied
        // digest or a reserialized signed body with signatures removed.
        let actual: String = Sha256::digest(envelope.raw_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if hash != actual {
            return Err(HighWaterError::Equivocation);
        }
    }
    // Frozen assertHighWater does not consult the old hash for a newer version.
    // Extra record fields remain ignored, but raw bounds/duplicates still apply.
    Ok(())
}
fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>, HighWaterError> {
    let mut output = BoundedOutput {
        bytes: Vec::new(),
        exceeded: false,
    };
    if serde_jcs::to_writer(&mut output, value).is_err() {
        return Err(HighWaterError::Json(if output.exceeded {
            Error::OutputLimit
        } else {
            Error::Canonicalization
        }));
    }
    Ok(output.bytes)
}
