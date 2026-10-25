//! Root distribution DATA only. No signature, trusted clock, lineage or currentness.
use crate::{
    ControlHttpError, MAX_RESPONSE_BYTES, Profile, check_headers, client_with_profile,
    network_error,
};
use pinvou_upgrade_codec::{Document, InputKind, SchemaCheckedEnvelope, decode_canonical_envelope};

const MAX_VERSION: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootGetError {
    InvalidSelector,
    Transport(ControlHttpError),
    UnexpectedStatus,
    InvalidRoot,
    InvalidMissing,
}
impl std::fmt::Display for RootGetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSelector => f.write_str("ROOT_GET_SELECTOR_INVALID"),
            Self::Transport(error) => error.fmt(f),
            Self::UnexpectedStatus => f.write_str("ROOT_GET_STATUS_INVALID"),
            Self::InvalidRoot => f.write_str("ROOT_GET_BODY_INVALID"),
            Self::InvalidMissing => f.write_str("ROOT_GET_MISSING_INVALID"),
        }
    }
}
impl std::error::Error for RootGetError {}
impl From<ControlHttpError> for RootGetError {
    fn from(error: ControlHttpError) -> Self {
        Self::Transport(error)
    }
}

/// The original bounded canonical Root and exact requested context, without trust.
/// Expired and time-less Roots are deliberately accepted for continuity recovery.
/// Pass these bytes to the dedicated Root/time verification path with a separately
/// protected anchor. The server's choice of Root is never a new trust anchor.
pub struct ReceivedRoot {
    envelope: SchemaCheckedEnvelope,
    product: String,
    version: u64,
}
impl ReceivedRoot {
    pub fn bytes(&self) -> &[u8] {
        self.envelope.envelope().raw_bytes()
    }
    pub fn envelope(&self) -> &SchemaCheckedEnvelope {
        &self.envelope
    }
    pub fn product(&self) -> &str {
        &self.product
    }
    pub fn version(&self) -> u64 {
        self.version
    }
}
/// Missing is a received 404 DATA observation, never proof of chain tail/latest.
pub enum RootGetResult {
    Found(ReceivedRoot),
    Missing,
}

fn path(product: &str, version: u64) -> Result<String, RootGetError> {
    let mut chars = product.bytes();
    if product.len() > 128
        || !chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'-'))
        || version == 0
        || version > MAX_VERSION
    {
        return Err(RootGetError::InvalidSelector);
    }
    // The closed alphabet cannot add a slash, query, fragment or URL escape.
    // Formatting the safe integer gives its unique positive decimal spelling.
    Ok(format!("/v1/upgrade/roots/{product}/{version}.json"))
}
fn received(product: &str, version: u64, bytes: &[u8]) -> Result<ReceivedRoot, RootGetError> {
    let envelope = decode_canonical_envelope(bytes, InputKind::Metadata)
        .map_err(|_| RootGetError::InvalidRoot)?
        .check_full_schema()
        .map_err(|_| RootGetError::InvalidRoot)?;
    let signed = envelope.envelope().signed();
    let root = signed
        .root_policy_shape()
        .map_err(|_| RootGetError::InvalidRoot)?;
    if root.product() != product || root.version() != version {
        return Err(RootGetError::InvalidRoot);
    }
    Ok(ReceivedRoot {
        envelope,
        product: product.to_owned(),
        version,
    })
}
fn missing(bytes: &[u8]) -> Result<(), RootGetError> {
    let document =
        Document::parse(bytes, InputKind::Metadata).map_err(|_| RootGetError::InvalidMissing)?;
    let value = document
        .value()
        .as_object()
        .ok_or(RootGetError::InvalidMissing)?;
    if value.len() != 2
        || value
            .get("protocolVersion")
            .and_then(serde_json::Value::as_f64)
            != Some(1.0)
        || value.get("code").and_then(serde_json::Value::as_str) != Some("ROOT_NOT_FOUND")
    {
        return Err(RootGetError::InvalidMissing);
    }
    Ok(())
}

/// One fixed-origin GET with the POST transport's TLS/no-proxy/no-redirect/no-retry
/// and connect/idle limits. No total elapsed download deadline, cookies or auth.
/// Dropping the future cancels I/O. No latest probe or automatic successor loop.
pub async fn get_root_once(product: &str, version: u64) -> Result<RootGetResult, RootGetError> {
    get_with_profile(product, version, Profile::default()).await
}
async fn get_with_profile(
    product: &str,
    version: u64,
    profile: Profile,
) -> Result<RootGetResult, RootGetError> {
    let path = path(product, version)?;
    let (client, origin) = client_with_profile(profile)?;
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
        return Err(RootGetError::UnexpectedStatus);
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
        200 => received(product, version, &bytes).map(RootGetResult::Found),
        404 => {
            missing(&bytes)?;
            Ok(RootGetResult::Missing)
        }
        _ => unreachable!("status checked before reading"),
    }
}
#[cfg(test)]
mod tests;
