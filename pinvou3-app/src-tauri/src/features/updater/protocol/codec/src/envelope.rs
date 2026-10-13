//! Original canonical envelope and signature encoding only, never verification.
use crate::shape::{ROLES, closed, identifier, sha256_hex};
use crate::{Document, Error, InputKind};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvelopeShapeError {
    Json(Error),
    NotCanonical,
    InvalidEnvelope,
    InvalidContext,
    WrongProfile,
    InvalidSignature,
    SignatureOrder,
}
impl std::fmt::Display for EnvelopeShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = match self {
            Self::Json(error) => return error.fmt(f),
            Self::NotCanonical => "ENVELOPE_NOT_CANONICAL",
            Self::InvalidEnvelope => "ENVELOPE_SHAPE_INVALID",
            Self::InvalidContext => "ENVELOPE_CONTEXT_SHAPE_INVALID",
            Self::WrongProfile => "ENVELOPE_INPUT_PROFILE_INVALID",
            Self::InvalidSignature => "SIGNATURE_ENCODING_INVALID",
            Self::SignatureOrder => "SIGNATURE_ORDER_INVALID",
        };
        f.write_str(code)
    }
}
impl std::error::Error for EnvelopeShapeError {}

/// Encoding facts only. No public constructor, Debug or Serialize.
pub struct SignatureRecordShape {
    key_id: String,
    bytes: [u8; 64],
}
impl SignatureRecordShape {
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.bytes
    }
}
/// Whole original canonical wire bytes with a partially checked signed context.
/// Full role Schema, expected context, curve/signature validity, key authority,
/// threshold satisfaction, time and protected Root lineage remain external.
pub struct CanonicalEnvelopeShape {
    raw: Vec<u8>,
    signed: Document,
    signatures: Vec<SignatureRecordShape>,
}
impl CanonicalEnvelopeShape {
    pub fn raw_bytes(&self) -> &[u8] {
        &self.raw
    }
    pub fn signed(&self) -> &Document {
        &self.signed
    }
    pub fn signatures(&self) -> &[SignatureRecordShape] {
        &self.signatures
    }
}
pub fn decode_canonical_envelope(
    bytes: &[u8],
    kind: InputKind,
) -> Result<CanonicalEnvelopeShape, EnvelopeShapeError> {
    let mut document = Document::parse(bytes, kind).map_err(EnvelopeShapeError::Json)?;
    if document
        .canonical_bytes()
        .map_err(EnvelopeShapeError::Json)?
        != bytes
    {
        return Err(EnvelopeShapeError::NotCanonical);
    }
    let envelope = closed(&document.value, &["signed", "signatures"])
        .ok_or(EnvelopeShapeError::InvalidEnvelope)?;
    let signed = envelope["signed"]
        .as_object()
        .ok_or(EnvelopeShapeError::InvalidContext)?;
    let role = signed
        .get("role")
        .and_then(serde_json::Value::as_str)
        .filter(|role| ROLES.contains(role))
        .ok_or(EnvelopeShapeError::InvalidContext)?;
    if signed
        .get("protocolVersion")
        .and_then(serde_json::Value::as_f64)
        != Some(1.0)
        || !signed.get("product").is_some_and(identifier)
        || !signed.get("component").is_some_and(|component| {
            if role == "root" {
                component.is_null()
            } else {
                identifier(component)
            }
        })
        || !signed
            .get("scope")
            .is_some_and(serde_json::Value::is_object)
    {
        return Err(EnvelopeShapeError::InvalidContext);
    }
    // The frozen role order has exactly the six metadata roles first.
    let expected_kind = if ROLES[..6].contains(&role) {
        InputKind::Metadata
    } else {
        InputKind::Credential
    };
    if kind != expected_kind {
        return Err(EnvelopeShapeError::WrongProfile);
    }
    let records = envelope["signatures"]
        .as_array()
        .filter(|records| !records.is_empty())
        .ok_or(EnvelopeShapeError::InvalidEnvelope)?;
    // Whole Document parsing already enforces the 4096-member bound.
    let mut signatures: Vec<SignatureRecordShape> = Vec::with_capacity(records.len());
    for record in records {
        let record =
            closed(record, &["keyId", "signature"]).ok_or(EnvelopeShapeError::InvalidSignature)?;
        if !sha256_hex(&record["keyId"]) {
            return Err(EnvelopeShapeError::InvalidSignature);
        }
        let key_id = record["keyId"]
            .as_str()
            .ok_or(EnvelopeShapeError::InvalidSignature)?;
        if signatures
            .last()
            .is_some_and(|previous| previous.key_id.as_str() >= key_id)
        {
            return Err(EnvelopeShapeError::SignatureOrder);
        }
        let text = record["signature"]
            .as_str()
            .filter(|text| {
                text.len() == 86
                    && text
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            })
            .ok_or(EnvelopeShapeError::InvalidSignature)?;
        let decoded = URL_SAFE_NO_PAD
            .decode(text)
            .map_err(|_| EnvelopeShapeError::InvalidSignature)?;
        if decoded.len() != 64 || URL_SAFE_NO_PAD.encode(&decoded) != text {
            return Err(EnvelopeShapeError::InvalidSignature);
        }
        let bytes = decoded
            .try_into()
            .map_err(|_| EnvelopeShapeError::InvalidSignature)?;
        signatures.push(SignatureRecordShape {
            key_id: key_id.to_owned(),
            bytes,
        });
    }
    // Move the original parsed subtree only after all whole-envelope checks.
    let signed = document
        .value
        .as_object_mut()
        .and_then(|envelope| envelope.remove("signed"))
        .ok_or(EnvelopeShapeError::InvalidEnvelope)?;
    Ok(CanonicalEnvelopeShape {
        raw: bytes.to_vec(),
        signed: Document { value: signed },
        signatures,
    })
}
