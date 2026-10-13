//! Frozen consume-status/secret hash math; no query or consumption authority.
use crate::shape::{closed, identifier, one_of, sha256_hex};
use crate::{Document, Error};
use sha2::{Digest, Sha256};

const STATUS_DOMAIN: &[u8] = b"pinvou-consume-status-v1\0";
const STATUS_FIELDS: [&str; 9] = [
    "recoveryHandleHash",
    "product",
    "component",
    "installationScopeId",
    "purpose",
    "authorizationJti",
    "consumeKey",
    "transactionId",
    "consumeRequestDigest",
];
const IDENTIFIER_FIELDS: [&str; 6] = [
    "product",
    "component",
    "installationScopeId",
    "authorizationJti",
    "consumeKey",
    "transactionId",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusBindingError {
    Json(Error),
    InvalidBinding,
    InvalidSecretLength,
}
impl std::fmt::Display for StatusBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::InvalidBinding => f.write_str("CONSUME_STATUS_BINDING_INVALID"),
            Self::InvalidSecretLength => f.write_str("RECOVERY_SECRET_SIZE_INVALID"),
        }
    }
}
impl std::error::Error for StatusBindingError {}

/// Exact shape-validated preimage/hash, not secret possession, a persisted
/// server binding, signature validity, consume result or installation rights.
/// Private constructor and no Debug/Serialize avoid automatic raw binding logs.
pub struct StatusBinding {
    input: Vec<u8>,
    hash: String,
}
impl StatusBinding {
    pub fn input_bytes(&self) -> &[u8] {
        &self.input
    }
    pub fn binding_hash(&self) -> &str {
        &self.hash
    }
}
/// Only SHA256 of the caller's exact 32 bytes; no entropy, generation, custody
/// or ownership claim. The result retains no raw secret and has no Debug/Serialize.
pub struct RecoveryHandleHash {
    hash: String,
}
impl RecoveryHandleHash {
    pub fn as_hex(&self) -> &str {
        &self.hash
    }
}

pub fn recovery_handle_hash(secret: &[u8]) -> Result<RecoveryHandleHash, StatusBindingError> {
    if secret.len() != 32 {
        return Err(StatusBindingError::InvalidSecretLength);
    }
    Ok(RecoveryHandleHash {
        hash: digest_hex(secret),
    })
}
impl Document {
    /// Validate the complete root object, with no field exclusion or defaults,
    /// then hash fixed ASCII domain + one NUL + complete bounded JCS payload.
    /// Unlike source projection, extra transport/self fields must be refused.
    pub fn status_binding(&self) -> Result<StatusBinding, StatusBindingError> {
        let binding =
            closed(&self.value, &STATUS_FIELDS).ok_or(StatusBindingError::InvalidBinding)?;
        if !IDENTIFIER_FIELDS
            .iter()
            .all(|name| identifier(&binding[*name]))
            || !sha256_hex(&binding["recoveryHandleHash"])
            || !sha256_hex(&binding["consumeRequestDigest"])
            || !one_of(&binding["purpose"], &["preinstall", "install", "activate"])
        {
            return Err(StatusBindingError::InvalidBinding);
        }
        let payload = self.canonical_bytes().map_err(StatusBindingError::Json)?;
        let mut input = Vec::with_capacity(STATUS_DOMAIN.len() + payload.len());
        input.extend_from_slice(STATUS_DOMAIN);
        input.extend_from_slice(&payload);
        let hash = digest_hex(&input);
        Ok(StatusBinding { input, hash })
    }
}
fn digest_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
