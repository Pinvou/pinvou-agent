//! Closed dedicated time DATA shapes. Parsing never establishes clock authority.
use crate::shape::{closed, identifier, positive_safe_integer, sha256_hex};
use crate::{Document, Error};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

const MAXIMUM: f64 = 9_007_199_254_740_991.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeContractError {
    Json(Error),
    InvalidRequest,
    InvalidResponse,
    InvalidNonce,
}
impl std::fmt::Display for TimeContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::InvalidRequest => f.write_str("TIME_REQUEST_INVALID"),
            Self::InvalidResponse => f.write_str("TIME_RESPONSE_INVALID"),
            Self::InvalidNonce => f.write_str("TIME_NONCE_INVALID"),
        }
    }
}
impl std::error::Error for TimeContractError {}

/// Parsed request DATA only. A random-looking nonce does not prove entropy,
/// a live host incarnation, protected Root storage or monotonic observations.
/// No public constructor, Debug or Serialize exposes a bypass of exact parsing.
pub struct TimeRequestShape {
    canonical: Vec<u8>,
    product: String,
    component: String,
    nonce: String,
    root_version: u64,
    root_sha256: String,
}
impl TimeRequestShape {
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical
    }
    pub fn product(&self) -> &str {
        &self.product
    }
    pub fn component(&self) -> &str {
        &self.component
    }
    pub fn request_nonce(&self) -> &str {
        &self.nonce
    }
    pub fn root_version(&self) -> u64 {
        self.root_version
    }
    pub fn root_sha256(&self) -> &str {
        &self.root_sha256
    }
}

/// Exact unsigned time body DATA; not a signature result or usable time anchor.
pub struct TimeResponseShape {
    request: TimeRequestShape,
    server_time: u64,
    uncertainty: u64,
}
impl TimeResponseShape {
    pub fn product(&self) -> &str {
        self.request.product()
    }
    pub fn component(&self) -> &str {
        self.request.component()
    }
    pub fn request_nonce(&self) -> &str {
        self.request.request_nonce()
    }
    pub fn root_version(&self) -> u64 {
        self.request.root_version()
    }
    pub fn root_sha256(&self) -> &str {
        self.request.root_sha256()
    }
    pub fn server_time_millis(&self) -> u64 {
        self.server_time
    }
    pub fn uncertainty_millis(&self) -> u64 {
        self.uncertainty
    }
}

impl Document {
    /// Supplied API shape only; no signature, trusted time or request acceptance.
    pub fn assert_supplied_time_response_contract(&self) -> Result<(), TimeContractError> {
        crate::schema::assert_role_value("time", self.value())
            .map_err(|_| TimeContractError::InvalidResponse)?;
        let signed = Document {
            value: self.value()["signed"].clone(),
        };
        signed.time_response_shape().map(|_| ())
    }

    pub fn time_request_shape(&self) -> Result<TimeRequestShape, TimeContractError> {
        let error = TimeContractError::InvalidRequest;
        let fields = closed(
            self.value(),
            &[
                "protocolVersion",
                "product",
                "component",
                "requestNonce",
                "root",
            ],
        )
        .ok_or(error)?;
        if fields["protocolVersion"].as_f64() != Some(1.0) {
            return Err(error);
        }
        common(self, error)
    }

    pub fn time_response_shape(&self) -> Result<TimeResponseShape, TimeContractError> {
        let error = TimeContractError::InvalidResponse;
        let fields = closed(
            self.value(),
            &[
                "protocolVersion",
                "role",
                "product",
                "component",
                "scope",
                "purpose",
                "requestNonce",
                "root",
                "serverTime",
                "uncertaintyMs",
            ],
        )
        .ok_or(error)?;
        if fields["protocolVersion"].as_f64() != Some(1.0)
            || fields["role"].as_str() != Some("time")
            || fields["purpose"].as_str() != Some("trusted-time.v1")
            || closed(&fields["scope"], &[]).is_none()
        {
            return Err(error);
        }
        Ok(TimeResponseShape {
            request: common(self, error)?,
            server_time: integer(&fields["serverTime"]).ok_or(error)?,
            uncertainty: integer(&fields["uncertaintyMs"]).ok_or(error)?,
        })
    }
}
fn common(
    document: &Document,
    error: TimeContractError,
) -> Result<TimeRequestShape, TimeContractError> {
    let body = document.value();
    let root = closed(&body["root"], &["version", "sha256"]).ok_or(error)?;
    let nonce = body["requestNonce"].as_str().ok_or(error)?;
    if !identifier(&body["product"])
        || !identifier(&body["component"])
        || !positive_safe_integer(&root["version"])
        || !sha256_hex(&root["sha256"])
    {
        return Err(error);
    }
    if nonce.len() != 43
        || !nonce
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(TimeContractError::InvalidNonce);
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(nonce)
        .map_err(|_| TimeContractError::InvalidNonce)?;
    if decoded.len() != 32 || URL_SAFE_NO_PAD.encode(decoded) != nonce {
        return Err(TimeContractError::InvalidNonce);
    }
    Ok(TimeRequestShape {
        canonical: document
            .canonical_bytes()
            .map_err(TimeContractError::Json)?,
        product: body["product"].as_str().ok_or(error)?.to_owned(),
        component: body["component"].as_str().ok_or(error)?.to_owned(),
        nonce: nonce.to_owned(),
        root_version: integer(&root["version"]).ok_or(error)?,
        root_sha256: root["sha256"].as_str().ok_or(error)?.to_owned(),
    })
}
fn integer(value: &serde_json::Value) -> Option<u64> {
    value
        .as_f64()
        .filter(|n| (0.0..=MAXIMUM).contains(n) && n.fract() == 0.0)
        .map(|n| n as u64)
}
