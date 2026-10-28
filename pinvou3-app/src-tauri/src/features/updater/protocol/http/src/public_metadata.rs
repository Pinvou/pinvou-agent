//! Public metadata DATA only; callers must authenticate references and verify trust.
use crate::{
    ControlHttpError, MAX_RESPONSE_BYTES, Profile, check_headers, client_with_profile,
    network_error,
};
use pinvou_upgrade_codec::{
    Document, InputKind, MetadataReferenceShape, SchemaCheckedEnvelope, decode_canonical_envelope,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicMetadataError {
    InvalidSelector,
    Transport(ControlHttpError),
    UnexpectedStatus,
    InvalidObject,
    InvalidMissing,
}
impl std::fmt::Display for PublicMetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSelector => f.write_str("PUBLIC_METADATA_SELECTOR_INVALID"),
            Self::Transport(error) => error.fmt(f),
            Self::UnexpectedStatus => f.write_str("PUBLIC_METADATA_STATUS_INVALID"),
            Self::InvalidObject => f.write_str("PUBLIC_METADATA_OBJECT_INVALID"),
            Self::InvalidMissing => f.write_str("PUBLIC_METADATA_MISSING_INVALID"),
        }
    }
}
impl std::error::Error for PublicMetadataError {}
impl From<ControlHttpError> for PublicMetadataError {
    fn from(error: ControlHttpError) -> Self {
        Self::Transport(error)
    }
}

pub struct ReceivedPublicMetadata {
    envelope: SchemaCheckedEnvelope,
}
impl ReceivedPublicMetadata {
    pub fn bytes(&self) -> &[u8] {
        self.envelope.envelope().raw_bytes()
    }
    pub fn envelope(&self) -> &SchemaCheckedEnvelope {
        &self.envelope
    }
}
pub enum PublicMetadataResult {
    Found(ReceivedPublicMetadata),
    Missing,
}

fn identifier(value: &str) -> bool {
    let mut chars = value.bytes();
    value.len() <= 128
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'-'))
}
fn reference_hash(
    reference: &MetadataReferenceShape,
    allowed: &[&str],
) -> Result<String, PublicMetadataError> {
    let value: serde_json::Value = serde_json::from_slice(reference.canonical_bytes())
        .map_err(|_| PublicMetadataError::InvalidSelector)?;
    let role = value
        .get("role")
        .and_then(serde_json::Value::as_str)
        .ok_or(PublicMetadataError::InvalidSelector)?;
    let hash = value
        .get("envelopeSha256")
        .and_then(serde_json::Value::as_str)
        .ok_or(PublicMetadataError::InvalidSelector)?;
    if !allowed.contains(&role)
        || hash.len() != 64
        || !hash
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(PublicMetadataError::InvalidSelector);
    }
    Ok(hash.to_owned())
}
fn path(
    product: &str,
    component: &str,
    snapshot: &MetadataReferenceShape,
    object: &MetadataReferenceShape,
) -> Result<String, PublicMetadataError> {
    if !identifier(product) || !identifier(component) {
        return Err(PublicMetadataError::InvalidSelector);
    }
    let snapshot = reference_hash(snapshot, &["snapshot"])?;
    let object = reference_hash(
        object,
        &["timestamp", "snapshot", "target", "release", "package"],
    )?;
    Ok(format!(
        "/v1/upgrade/public-metadata/{product}/{component}/{snapshot}/{object}"
    ))
}
fn received(
    product: &str,
    component: &str,
    expected: &MetadataReferenceShape,
    bytes: &[u8],
) -> Result<ReceivedPublicMetadata, PublicMetadataError> {
    expected
        .assert_original_envelope(bytes)
        .map_err(|_| PublicMetadataError::InvalidObject)?;
    let envelope = decode_canonical_envelope(bytes, InputKind::Metadata)
        .map_err(|_| PublicMetadataError::InvalidObject)?
        .check_full_schema()
        .map_err(|_| PublicMetadataError::InvalidObject)?;
    let signed = envelope.envelope().signed().value();
    if signed["product"].as_str() != Some(product)
        || signed["component"].as_str() != Some(component)
    {
        return Err(PublicMetadataError::InvalidObject);
    }
    Ok(ReceivedPublicMetadata { envelope })
}
fn missing(bytes: &[u8]) -> Result<(), PublicMetadataError> {
    let document = Document::parse(bytes, InputKind::Metadata)
        .map_err(|_| PublicMetadataError::InvalidMissing)?;
    let value = document
        .value()
        .as_object()
        .ok_or(PublicMetadataError::InvalidMissing)?;
    if value.len() != 2
        || value
            .get("protocolVersion")
            .and_then(serde_json::Value::as_f64)
            != Some(1.0)
        || value.get("code").and_then(serde_json::Value::as_str)
            != Some("PUBLIC_METADATA_NOT_FOUND")
    {
        return Err(PublicMetadataError::InvalidMissing);
    }
    Ok(())
}

/// Fetch one public object using authenticated Snapshot/object references.
/// This verifies exact bytes and schema only: the caller must authenticate both
/// references, signatures, full public membership, time, deny and high-water.
/// Missing or a changed publication requires a new check, never "no update".
pub async fn get_public_metadata_once(
    product: &str,
    component: &str,
    snapshot: &MetadataReferenceShape,
    object: &MetadataReferenceShape,
) -> Result<PublicMetadataResult, PublicMetadataError> {
    let path = path(product, component, snapshot, object)?;
    let (client, origin) = client_with_profile(Profile::default())?;
    let mut response = client
        .get(format!("{origin}{path}"))
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .header(reqwest::header::CACHE_CONTROL, "no-store")
        .send()
        .await
        .map_err(network_error)?;
    let status = response.status().as_u16();
    if status != 200 && status != 404 {
        return Err(PublicMetadataError::UnexpectedStatus);
    }
    check_headers(response.headers())?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if chunk.len() > MAX_RESPONSE_BYTES - bytes.len() {
            return Err(ControlHttpError::ResponseTooLarge.into());
        }
        bytes.extend_from_slice(&chunk);
    }
    match status {
        200 => received(product, component, object, &bytes).map(PublicMetadataResult::Found),
        404 => {
            missing(&bytes)?;
            Ok(PublicMetadataResult::Missing)
        }
        _ => unreachable!("status checked before reading"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinvou_upgrade_codec::derive_metadata_reference;
    use serde_json::{Value, json};

    fn canonical(value: Value) -> Vec<u8> {
        let bytes = serde_json::to_vec(&value).unwrap();
        Document::parse(&bytes, InputKind::Metadata)
            .unwrap()
            .canonical_bytes()
            .unwrap()
    }
    fn fixture() -> (Vec<u8>, Vec<u8>) {
        let signature = json!([{ "keyId": "a".repeat(64), "signature": "A".repeat(86) }]);
        let reference = json!({ "role":"target", "scope":{}, "version":1, "expiresAt":1000,
            "size":10, "envelopeSha256":"b".repeat(64) });
        let snapshot = canonical(json!({ "signed": { "protocolVersion":1, "role":"snapshot",
            "product":"product", "component":"desktop", "scope":{}, "version":1,
            "issuedAt":1, "expiresAt":1000, "entries":[reference] }, "signatures":signature }));
        let snapshot_ref = derive_metadata_reference(&snapshot).unwrap();
        let snapshot_value: Value = serde_json::from_slice(snapshot_ref.canonical_bytes()).unwrap();
        let timestamp = canonical(json!({ "signed": { "protocolVersion":1, "role":"timestamp",
            "product":"product", "component":"desktop", "scope":{}, "version":1,
            "issuedAt":1, "expiresAt":1000, "snapshot":snapshot_value }, "signatures":signature }));
        (snapshot, timestamp)
    }

    #[test]
    fn selectors_and_original_object_identity_are_exact() {
        let (snapshot, timestamp) = fixture();
        let snapshot_ref = derive_metadata_reference(&snapshot).unwrap();
        let object_ref = derive_metadata_reference(&timestamp).unwrap();
        assert!(
            path("product", "desktop", &snapshot_ref, &object_ref)
                .unwrap()
                .starts_with("/v1/upgrade/public-metadata/product/desktop/")
        );
        assert!(path("../product", "desktop", &snapshot_ref, &object_ref).is_err());
        assert!(path("product", "desktop", &object_ref, &object_ref).is_err());
        assert_eq!(
            received("product", "desktop", &object_ref, &timestamp)
                .unwrap()
                .bytes(),
            timestamp
        );
        assert!(received("other", "desktop", &object_ref, &timestamp).is_err());
        assert!(received("product", "other", &object_ref, &timestamp).is_err());
        let mut changed = timestamp.clone();
        changed.push(b' ');
        assert!(received("product", "desktop", &object_ref, &changed).is_err());
    }

    #[test]
    fn missing_is_a_closed_data_observation() {
        assert!(missing(br#"{"code":"PUBLIC_METADATA_NOT_FOUND","protocolVersion":1}"#).is_ok());
        assert!(
            missing(br#"{"code":"PUBLIC_METADATA_NOT_FOUND","protocolVersion":1,"latest":1}"#)
                .is_err()
        );
        assert!(missing(br#"{"code":"ROOT_NOT_FOUND","protocolVersion":1}"#).is_err());
    }
}
