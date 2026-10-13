//! Exact supplied consume/authentication-claim binding, not consumption.
use crate::{
    BoundedOutput, ConsumeDigestError, Document, EnvelopeShapeError, Error, SchemaCheckedEnvelope,
    SchemaError,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsumeBindingError {
    Json(Error),
    Envelope(EnvelopeShapeError),
    Schema(SchemaError),
    Request(ConsumeDigestError),
    PurposeMismatch,
    BindingMismatch,
}
impl std::fmt::Display for ConsumeBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::Envelope(e) => e.fmt(f),
            Self::Schema(e) => e.fmt(f),
            Self::Request(e) => e.fmt(f),
            Self::PurposeMismatch => f.write_str("CONSUME_PURPOSE_INVALID"),
            Self::BindingMismatch => f.write_str("CONSUME_BINDING_INVALID"),
        }
    }
}
impl std::error::Error for ConsumeBindingError {}

/// Exact closed semantic request. Transport/self fields accepted by the digest
/// projection are forbidden here. No public constructor, Debug or Serialize.
pub struct ConsumeRequestShape<'a> {
    value: &'a Value,
}
impl Document {
    pub fn consume_request_shape(&self) -> Result<ConsumeRequestShape<'_>, ConsumeDigestError> {
        if !crate::consume::valid_request(self.value()) {
            return Err(ConsumeDigestError::InvalidRequest);
        }
        Ok(ConsumeRequestShape {
            value: self.value(),
        })
    }
}

/// The frozen assertConsumeBinding post-verification relationships only.
/// This does not perform verifyEnvelope: the caller must independently establish
/// signature/Root/context/time/current qualification and atomically consume.
/// The hash binds the factory's original complete envelope, including signatures.
pub fn assert_supplied_consume_binding(
    request: &ConsumeRequestShape<'_>,
    authorization: &SchemaCheckedEnvelope,
) -> Result<(), ConsumeBindingError> {
    let request = request.value;
    let envelope = authorization.envelope();
    let signed = envelope.signed().value();
    let role = match request["purpose"].as_str() {
        Some("preinstall") => "authorization-preinstall",
        Some("install") => "authorization-install",
        Some("activate") => "authorization-activate",
        _ => return Err(ConsumeBindingError::PurposeMismatch),
    };
    if signed["role"] != role {
        return Err(ConsumeBindingError::PurposeMismatch);
    }
    let hash: String = Sha256::digest(envelope.raw_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if request["authorizationEnvelopeSha256"] != hash {
        return Err(ConsumeBindingError::BindingMismatch);
    }
    for field in [
        "product",
        "component",
        "purpose",
        "authorizationJti",
        "transactionId",
        "clientFactsDigest",
        "preparation",
        "freezeEpoch",
        "backup",
        "staged",
        "preinstallSlot",
    ] {
        if !same(&request[field], &signed[field])? {
            return Err(ConsumeBindingError::BindingMismatch);
        }
    }
    if request["installationScopeId"] != signed["scope"]["installationScopeId"]
        || !same(&request["plans"], &signed["update"]["plans"])?
    {
        return Err(ConsumeBindingError::BindingMismatch);
    }
    Ok(())
}
fn canonical(value: &Value) -> Result<Vec<u8>, ConsumeBindingError> {
    // Borrow bounded, already validated subtrees; no generic public entrance.
    let mut output = BoundedOutput {
        bytes: Vec::new(),
        exceeded: false,
    };
    if serde_jcs::to_writer(&mut output, value).is_err() {
        return Err(ConsumeBindingError::Json(if output.exceeded {
            Error::OutputLimit
        } else {
            Error::Canonicalization
        }));
    }
    Ok(output.bytes)
}
fn same(left: &Value, right: &Value) -> Result<bool, ConsumeBindingError> {
    Ok(canonical(left)? == canonical(right)?)
}
