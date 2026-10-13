//! Frozen semantic consume-request digest, not consumption or permission.
use crate::shape::{closed, identifier, one_of, positive_safe_integer, sha256_hex};
use crate::{Document, Error};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const EXCLUDED: [&str; 4] = [
    "consumeRequestDigest",
    "recoverySecret",
    "requestId",
    "headers",
];
const FIELDS: [&str; 17] = [
    "protocolVersion",
    "product",
    "component",
    "installationScopeId",
    "purpose",
    "authorizationJti",
    "consumeKey",
    "transactionId",
    "authorizationEnvelopeSha256",
    "recoveryHandleHash",
    "clientFactsDigest",
    "preparation",
    "freezeEpoch",
    "backup",
    "staged",
    "plans",
    "preinstallSlot",
];
const IDENTIFIERS: [&str; 6] = [
    "product",
    "component",
    "installationScopeId",
    "authorizationJti",
    "consumeKey",
    "transactionId",
];
const HASHES: [&str; 3] = [
    "authorizationEnvelopeSha256",
    "recoveryHandleHash",
    "clientFactsDigest",
];
const PLAN_FIELDS: [&str; 6] = [
    "installation",
    "migration",
    "backup",
    "helperMigration",
    "launcherMigration",
    "takeover",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsumeDigestError {
    Json(Error),
    InvalidRequest,
}
impl std::fmt::Display for ConsumeDigestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::InvalidRequest => f.write_str("CONSUME_REQUEST_SCHEMA_INVALID"),
        }
    }
}
impl std::error::Error for ConsumeDigestError {}

/// Shape-validated semantic JCS and digest only. No consumption, backup/slot
/// authenticity, current permission, signature or trustworthy time is attested.
/// No excluded top-level fields are retained; no Debug/Serialize is provided.
pub struct ConsumeRequestDigest {
    canonical: Vec<u8>,
    hash: String,
}
impl ConsumeRequestDigest {
    pub fn canonical_semantic(&self) -> &[u8] {
        &self.canonical
    }
    pub fn request_digest(&self) -> &str {
        &self.hash
    }
}
impl Document {
    /// Exclude only four exact top-level transport/self fields, then validate
    /// the complete remaining semantic object. All original JSON was already
    /// resource-bounded and duplicate-checked, even its excluded fields.
    pub fn consume_request_digest(&self) -> Result<ConsumeRequestDigest, ConsumeDigestError> {
        let request = self
            .value
            .as_object()
            .ok_or(ConsumeDigestError::InvalidRequest)?;
        let semantic = Value::Object(
            request
                .iter()
                .filter(|(key, _)| !EXCLUDED.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Map<String, Value>>(),
        );
        if !valid_request(&semantic) {
            return Err(ConsumeDigestError::InvalidRequest);
        }
        let canonical = Document { value: semantic }
            .canonical_bytes()
            .map_err(ConsumeDigestError::Json)?;
        let hash = Sha256::digest(&canonical)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(ConsumeRequestDigest { canonical, hash })
    }
}
fn valid_request(value: &Value) -> bool {
    let Some(request) = closed(value, &FIELDS) else {
        return false;
    };
    request["protocolVersion"].as_f64() == Some(1.0)
        && IDENTIFIERS.iter().all(|name| identifier(&request[*name]))
        && HASHES.iter().all(|name| sha256_hex(&request[*name]))
        && one_of(&request["purpose"], &["preinstall", "install", "activate"])
        && preparation(&request["preparation"])
        && nullable(&request["freezeEpoch"], positive_safe_integer)
        && nullable(&request["backup"], backup)
        && nullable(&request["staged"], staged)
        && plans(&request["plans"])
        && nullable(&request["preinstallSlot"], preinstall_slot)
}
fn nullable(value: &Value, validate: fn(&Value) -> bool) -> bool {
    value.is_null() || validate(value)
}
fn preparation(value: &Value) -> bool {
    closed(value, &["ownerEpoch", "scopeRevision"]).is_some_and(|object| {
        positive_safe_integer(&object["ownerEpoch"])
            && positive_safe_integer(&object["scopeRevision"])
    })
}
fn backup(value: &Value) -> bool {
    closed(
        value,
        &[
            "recordId",
            "freezeEpoch",
            "sha256",
            "dataScopeDigest",
            "preparationEpoch",
        ],
    )
    .is_some_and(|object| {
        identifier(&object["recordId"])
            && positive_safe_integer(&object["freezeEpoch"])
            && sha256_hex(&object["sha256"])
            && sha256_hex(&object["dataScopeDigest"])
            && positive_safe_integer(&object["preparationEpoch"])
    })
}
fn staged(value: &Value) -> bool {
    closed(
        value,
        &[
            "preinstallTransactionId",
            "stagedRevision",
            "slotIdentity",
            "stagedAt",
            "stagedValidUntil",
        ],
    )
    .is_some_and(|object| {
        identifier(&object["preinstallTransactionId"])
            && positive_safe_integer(&object["stagedRevision"])
            && identifier(&object["slotIdentity"])
            && nonnegative_safe_integer(&object["stagedAt"])
            && nonnegative_safe_integer(&object["stagedValidUntil"])
    })
}
fn nonnegative_safe_integer(value: &Value) -> bool {
    value.as_f64().is_some_and(|number| {
        (0.0..=9_007_199_254_740_991.0).contains(&number) && number.fract() == 0.0
    })
}
fn plans(value: &Value) -> bool {
    closed(value, &PLAN_FIELDS).is_some_and(|object| {
        plan_reference(&object["installation"])
            && PLAN_FIELDS[1..]
                .iter()
                .all(|name| nullable(&object[*name], plan_reference))
    })
}
fn plan_reference(value: &Value) -> bool {
    closed(value, &["planId", "revision", "sha256"]).is_some_and(|object| {
        identifier(&object["planId"])
            && positive_safe_integer(&object["revision"])
            && sha256_hex(&object["sha256"])
    })
}
fn preinstall_slot(value: &Value) -> bool {
    closed(value, &["slotIdentity", "slotRevision", "state"]).is_some_and(|object| {
        identifier(&object["slotIdentity"])
            && positive_safe_integer(&object["slotRevision"])
            && object["state"].as_str() == Some("inactive")
    })
}
