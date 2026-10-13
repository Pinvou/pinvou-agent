//! Fixed signing bytes and descriptor identities only. Neither result proves
//! signature validity, authorized keys, Root trust or current execution rights.

use crate::{BoundedOutput, CONTAINER_MEMBERS, Document, Error, MAXIMUM_DEPTH, STRING_BYTES};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Serialize, Serializer, ser::SerializeStruct};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;

const SIGNED_DOMAIN: &[u8] = b"pinvou-upgrade-signed-v1\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingError {
    Json(Error),
    MissingClaims,
    InvalidKeyEncoding,
}

impl fmt::Display for BindingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => fmt::Display::fmt(error, formatter),
            Self::MissingClaims => formatter.write_str("SIGNED_BINDING_FIELDS_REQUIRED"),
            Self::InvalidKeyEncoding => formatter.write_str("KEY_ENCODING_INVALID"),
        }
    }
}
impl std::error::Error for BindingError {}

/// Exact domain-separated bytes; deliberately has no Debug/Serialize implementation.
pub struct SigningInput {
    bytes: Vec<u8>,
}
impl SigningInput {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

struct SignedBinding<'a> {
    protocol_version: &'a Value,
    role: &'a Value,
    product: &'a Value,
    component: &'a Value,
    scope: &'a Value,
    signed: &'a Value,
}

impl Serialize for SignedBinding<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut binding = serializer.serialize_struct("SignedBinding", 6)?;
        binding.serialize_field("protocolVersion", self.protocol_version)?;
        binding.serialize_field("role", self.role)?;
        binding.serialize_field("product", self.product)?;
        binding.serialize_field("component", self.component)?;
        binding.serialize_field("scope", self.scope)?;
        binding.serialize_field("signed", self.signed)?;
        binding.end()
    }
}

impl Document {
    /// Bind the complete signed object. Closed schema and trust checks belong to
    /// later verifiers; explicit null differs from an absent required property.
    pub fn signing_input(&self) -> Result<SigningInput, BindingError> {
        let object = self.value.as_object().ok_or(BindingError::MissingClaims)?;
        let field = |name| object.get(name).ok_or(BindingError::MissingClaims);
        let binding = SignedBinding {
            protocol_version: field("protocolVersion")?,
            role: field("role")?,
            product: field("product")?,
            component: field("component")?,
            scope: field("scope")?,
            signed: &self.value,
        };
        // The wrapper introduces a level and repeats five fields. Revalidate the
        // resulting shape instead of relying on the original parsed depth.
        for value in [
            binding.protocol_version,
            binding.role,
            binding.product,
            binding.component,
            binding.scope,
            binding.signed,
        ] {
            validate_value(value, 1).map_err(BindingError::Json)?;
        }
        let payload = canonical_binding(&binding)?;
        let mut bytes = Vec::with_capacity(SIGNED_DOMAIN.len() + payload.len());
        bytes.extend_from_slice(SIGNED_DOMAIN);
        bytes.extend_from_slice(&payload);
        Ok(SigningInput { bytes })
    }
}

// Only private fixed-shape callers may serialize. Do not expose a generic entry
// point that could bypass decoded duplicate keys or resource validation.
fn canonical_binding<T: Serialize>(value: &T) -> Result<Vec<u8>, BindingError> {
    let mut output = BoundedOutput {
        bytes: Vec::new(),
        exceeded: false,
    };
    if serde_jcs::to_writer(&mut output, value).is_err() {
        return Err(BindingError::Json(if output.exceeded {
            Error::OutputLimit
        } else {
            Error::Canonicalization
        }));
    }
    Ok(output.bytes)
}

fn validate_value(value: &Value, depth: usize) -> Result<(), Error> {
    if depth > MAXIMUM_DEPTH {
        return Err(Error::DepthLimit);
    }
    match value {
        Value::String(text) if text.len() > STRING_BYTES => Err(Error::StringLimit),
        Value::Array(items) => {
            if items.len() > CONTAINER_MEMBERS {
                return Err(Error::MemberLimit);
            }
            for item in items {
                validate_value(item, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(fields) => {
            if fields.len() > CONTAINER_MEMBERS {
                return Err(Error::MemberLimit);
            }
            for (key, item) in fields {
                if key.len() > STRING_BYTES {
                    return Err(Error::StringLimit);
                }
                validate_value(item, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// A byte descriptor identity, not a valid curve point or a trusted signing key.
pub struct KeyDescriptor {
    public_key: String,
    key_id: String,
}
impl KeyDescriptor {
    pub fn public_key(&self) -> &str {
        &self.public_key
    }
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}

struct DescriptorBinding<'a> {
    algorithm: &'static str,
    public_key: &'a str,
}

impl Serialize for DescriptorBinding<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut descriptor = serializer.serialize_struct("DescriptorBinding", 2)?;
        descriptor.serialize_field("algorithm", self.algorithm)?;
        descriptor.serialize_field("publicKey", self.public_key)?;
        descriptor.end()
    }
}

pub fn key_descriptor(public_key: &str) -> Result<KeyDescriptor, BindingError> {
    // A canonical unpadded 32-byte value has exactly 43 ASCII characters. Check
    // before allocating decoded bytes; never accept a caller-selected algorithm.
    if public_key.len() != 43
        || !public_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(BindingError::InvalidKeyEncoding);
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(public_key)
        .map_err(|_| BindingError::InvalidKeyEncoding)?;
    if decoded.len() != 32 || URL_SAFE_NO_PAD.encode(&decoded) != public_key {
        return Err(BindingError::InvalidKeyEncoding);
    }
    let payload = canonical_binding(&DescriptorBinding {
        algorithm: "ed25519",
        public_key,
    })?;
    Ok(KeyDescriptor {
        public_key: public_key.to_owned(),
        key_id: Sha256::digest(payload)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}
