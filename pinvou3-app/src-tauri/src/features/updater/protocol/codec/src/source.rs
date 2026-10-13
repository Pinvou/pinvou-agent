//! Closed stable-fact projection of supplied JSON, not actual source detection.
use crate::shape::{TARGETS, closed, identifier, one_of, positive_safe_integer, sha256_hex};
use crate::{Document, Error};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const STABLE_FIELDS: [&str; 9] = [
    "product",
    "component",
    "targetKey",
    "canonicalAppVersion",
    "application",
    "helper",
    "launcher",
    "installation",
    "dataMigration",
];
const NATIVE_FIELDS: [&str; 5] = ["size", "sha256", "identity", "publisher", "format"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceProfileError {
    Json(Error),
    MissingStableFacts,
    InvalidStableFacts,
}
impl std::fmt::Display for SourceProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::MissingStableFacts => f.write_str("SOURCE_FACTS_MISSING"),
            Self::InvalidStableFacts => f.write_str("SOURCE_FACTS_SCHEMA_INVALID"),
        }
    }
}
impl std::error::Error for SourceProfileError {}

/// Shape-validated supplied facts and SHA256(JCS) identity only. The owner still
/// proves actual native/installation/data facts and current Registry matching.
/// No native probe, signature verification, trustworthy time or rights is implied.
/// Private constructor and no Debug/Serialize avoid accidental raw fact logging.
pub struct SourceProfile {
    canonical: Vec<u8>,
    id: String,
}
impl SourceProfile {
    pub fn canonical_projection(&self) -> &[u8] {
        &self.canonical
    }
    pub fn source_profile_id(&self) -> &str {
        &self.id
    }
}
impl Document {
    /// Freeze exactly the nine stable fields from already bounded, duplicate-free
    /// JSON. Extra outer fields are excluded as in the frozen reference function;
    /// unknown nested fields are refused by closed Schema shape validation.
    /// This does not validate the complete dynamic runtime-facts document.
    pub fn source_profile(&self) -> Result<SourceProfile, SourceProfileError> {
        let facts = self
            .value
            .as_object()
            .ok_or(SourceProfileError::MissingStableFacts)?;
        if !STABLE_FIELDS.iter().all(|name| facts.contains_key(*name)) {
            return Err(SourceProfileError::MissingStableFacts);
        }
        let valid = identifier(&facts["product"])
            && identifier(&facts["component"])
            && one_of(&facts["targetKey"], &TARGETS)
            && version(&facts["canonicalAppVersion"])
            && ["application", "helper", "launcher"]
                .iter()
                .all(|name| native(&facts[*name]))
            && installation(&facts["installation"])
            && data_migration(&facts["dataMigration"]);
        if !valid {
            return Err(SourceProfileError::InvalidStableFacts);
        }
        let projection: Map<String, Value> = STABLE_FIELDS
            .iter()
            .map(|name| ((*name).to_owned(), facts[*name].clone()))
            .collect();
        // Only this fixed validated projection may use the internal Document
        // constructor; no arbitrary Value/Serialize input is exposed publicly.
        let canonical = Document {
            value: Value::Object(projection),
        }
        .canonical_bytes()
        .map_err(SourceProfileError::Json)?;
        let digest = Sha256::digest(&canonical);
        let id = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(SourceProfile { canonical, id })
    }
}
fn version(value: &Value) -> bool {
    let Some(text) = value.as_str() else {
        return false;
    };
    let mut parts = text.split('.');
    for _ in 0..3 {
        let Some(part) = parts.next() else {
            return false;
        };
        // Schema has no integer magnitude bound for version components. A long
        // valid component must not fail through integer parsing/overflow.
        if part.is_empty()
            || !part.bytes().all(|b| b.is_ascii_digit())
            || (part.starts_with('0') && part != "0")
        {
            return false;
        }
    }
    parts.next().is_none()
}
fn native(value: &Value) -> bool {
    let Some(facts) = closed(value, &NATIVE_FIELDS) else {
        return false;
    };
    let size = positive_safe_integer(&facts["size"]);
    let hash = sha256_hex(&facts["sha256"]);
    size && hash
        && identifier(&facts["identity"])
        && identifier(&facts["publisher"])
        && one_of(
            &facts["format"],
            &["nsis", "deb", "dmg", "helper", "launcher"],
        )
}
fn installation(value: &Value) -> bool {
    let Some(facts) = closed(value, &["format", "installScope", "ownershipProtocol"]) else {
        return false;
    };
    one_of(&facts["format"], &["nsis", "deb", "dmg"])
        && facts["installScope"].as_str() == Some("perMachine")
        && identifier(&facts["ownershipProtocol"])
}
fn data_migration(value: &Value) -> bool {
    let Some(facts) = closed(value, &["dataFormat", "migrationProtocol"]) else {
        return false;
    };
    identifier(&facts["dataFormat"]) && identifier(&facts["migrationProtocol"])
}
