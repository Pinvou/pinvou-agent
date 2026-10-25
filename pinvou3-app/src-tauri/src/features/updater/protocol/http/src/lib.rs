//! One fixed-origin client POST. Transport evidence is not signed/current authority.
use pinvou_upgrade_codec::{
    ClientHttpResponseError, ClientRequestError, ClientRequestOperation, Document, Error, InputKind,
};
use std::{collections::BTreeMap, sync::OnceLock, time::Duration};
mod routes;
pub use pinvou_upgrade_codec::ClientRequestOperation as Operation;

pub const CONTROL_ORIGIN: &str = "https://update.pinvou.com";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlHttpError {
    RoutesUnavailable,
    RequestJson(Error),
    RequestContract(ClientRequestError),
    ClientUnavailable,
    RequestStalled,
    ConnectionFailed,
    NetworkFailed,
    InvalidHeaders,
    ResponseTooLarge,
    ResponseJson(Error),
    ResponseContract(ClientHttpResponseError),
}
impl std::fmt::Display for ControlHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RequestJson(e) | Self::ResponseJson(e) => e.fmt(f),
            Self::RequestContract(e) => e.fmt(f),
            Self::ResponseContract(e) => e.fmt(f),
            Self::RoutesUnavailable => f.write_str("CONTROL_ROUTES_UNAVAILABLE"),
            Self::ClientUnavailable => f.write_str("CONTROL_CLIENT_UNAVAILABLE"),
            Self::RequestStalled => f.write_str("CONTROL_REQUEST_STALLED"),
            Self::ConnectionFailed => f.write_str("CONTROL_CONNECTION_FAILED"),
            Self::NetworkFailed => f.write_str("CONTROL_NETWORK_FAILED"),
            Self::InvalidHeaders => f.write_str("CONTROL_HEADERS_INVALID"),
            Self::ResponseTooLarge => f.write_str("CONTROL_RESPONSE_TOO_LARGE"),
        }
    }
}
impl std::error::Error for ControlHttpError {}
fn network_error(error: reqwest::Error) -> ControlHttpError {
    // Discard SDK errors, which may carry URLs or sensitive request details.
    if error.is_timeout() {
        ControlHttpError::RequestStalled
    } else if error.is_connect() {
        ControlHttpError::ConnectionFailed
    } else {
        ControlHttpError::NetworkFailed
    }
}

/// A received, contract-valid HTTP body, including declared 4xx/5xx errors.
/// No signature, trusted time, correlation, ACK persistence or rights are proved.
pub struct ReceivedResponse {
    operation: Operation,
    status: u16,
    document: Document,
    bytes: Vec<u8>,
}
impl ReceivedResponse {
    pub fn operation(&self) -> Operation {
        self.operation
    }
    pub fn status(&self) -> u16 {
        self.status
    }
    /// Original complete wire body, retained under the one MiB metadata limit.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn document(&self) -> &Document {
        &self.document
    }
}

struct Profile {
    connect: Duration,
    read_idle: Duration,
    #[cfg(test)]
    fixture: Option<(std::net::SocketAddr, reqwest::Certificate)>,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            read_idle: Duration::from_secs(30),
            #[cfg(test)]
            fixture: None,
        }
    }
}

/// Validate and canonically send exactly one client request to the frozen control
/// origin. No retry/cookie/auth header/custom endpoint API. Dropping the future
/// cancels I/O but does not prove absence of a committed remote result.
pub async fn post_once(
    operation: Operation,
    raw_request: &[u8],
) -> Result<ReceivedResponse, ControlHttpError> {
    post_with_profile(operation, raw_request, Profile::default()).await
}
async fn post_with_profile(
    operation: Operation,
    raw_request: &[u8],
    profile: Profile,
) -> Result<ReceivedResponse, ControlHttpError> {
    let kind = if operation == Operation::Time {
        InputKind::Credential
    } else {
        InputKind::Metadata
    };
    let response_limit = if operation == Operation::Time {
        65_536
    } else {
        MAX_RESPONSE_BYTES
    };
    let document = Document::parse(raw_request, kind).map_err(ControlHttpError::RequestJson)?;
    document
        .assert_supplied_client_request_contract(operation)
        .map_err(ControlHttpError::RequestContract)?;
    let body = document
        .canonical_bytes()
        .map_err(ControlHttpError::RequestJson)?;
    let path = routes::path(operation)?;
    let builder = reqwest::Client::builder()
        .https_only(true)
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_gzip()
        .no_brotli()
        .no_zstd()
        .no_deflate()
        .pool_max_idle_per_host(0)
        .connect_timeout(profile.connect)
        .read_timeout(profile.read_idle);
    #[cfg(test)]
    let (builder, origin) = match profile.fixture {
        Some((address, cert)) => (
            builder
                .add_root_certificate(cert)
                .resolve("update.pinvou.com", address),
            format!("{CONTROL_ORIGIN}:{}", address.port()),
        ),
        None => (builder, CONTROL_ORIGIN.to_owned()),
    };
    #[cfg(not(test))]
    let origin = CONTROL_ORIGIN;
    let client = builder
        .build()
        .map_err(|_| ControlHttpError::ClientUnavailable)?;
    let mut response = client
        .post(format!("{origin}{path}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .header(reqwest::header::CACHE_CONTROL, "no-store")
        .body(body)
        .send()
        .await
        .map_err(network_error)?;
    let status = response.status().as_u16();
    check_headers(response.headers())?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if chunk.len() > response_limit - bytes.len() {
            return Err(ControlHttpError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    let document = Document::parse(&bytes, kind).map_err(ControlHttpError::ResponseJson)?;
    if operation == Operation::Time
        && status == 200
        && document
            .canonical_bytes()
            .map_err(ControlHttpError::ResponseJson)?
            != bytes
    {
        return Err(ControlHttpError::ResponseJson(Error::Canonicalization));
    }
    document
        .assert_supplied_client_http_response_contract(operation, status)
        .map_err(ControlHttpError::ResponseContract)?;
    Ok(ReceivedResponse {
        operation,
        status,
        document,
        bytes,
    })
}
fn ows(text: &str) -> &str {
    text.trim_matches([' ', '\t'])
}
fn check_headers(headers: &reqwest::header::HeaderMap) -> Result<(), ControlHttpError> {
    let mut types = headers.get_all(reqwest::header::CONTENT_TYPE).iter();
    let content_type = types
        .next()
        .and_then(|v| v.to_str().ok())
        .ok_or(ControlHttpError::InvalidHeaders)?;
    // HTTP optional whitespace is SP/HTAB, not general Unicode whitespace.
    let mut parts = content_type.split(';');
    if types.next().is_some()
        || !ows(parts.next().unwrap_or("")).eq_ignore_ascii_case("application/json")
    {
        return Err(ControlHttpError::InvalidHeaders);
    }
    if let Some(parameter) = parts.next() {
        let (name, value) = parameter
            .split_once('=')
            .ok_or(ControlHttpError::InvalidHeaders)?;
        let value = ows(value);
        if !ows(name).eq_ignore_ascii_case("charset")
            || !(value.eq_ignore_ascii_case("utf-8") || value.eq_ignore_ascii_case("\"utf-8\""))
            || parts.next().is_some()
        {
            return Err(ControlHttpError::InvalidHeaders);
        }
    }
    let mut encodings = headers.get_all(reqwest::header::CONTENT_ENCODING).iter();
    if let Some(encoding) = encodings.next()
        && (encodings.next().is_some() || !encoding.as_bytes().eq_ignore_ascii_case(b"identity"))
    {
        return Err(ControlHttpError::InvalidHeaders);
    }
    let mut transfers = headers.get_all(reqwest::header::TRANSFER_ENCODING).iter();
    if let Some(encoding) = transfers.next()
        && (transfers.next().is_some()
            || !encoding.as_bytes().eq_ignore_ascii_case(b"chunked")
            || headers.contains_key(reqwest::header::CONTENT_LENGTH))
    {
        return Err(ControlHttpError::InvalidHeaders);
    }
    let mut lengths = headers.get_all(reqwest::header::CONTENT_LENGTH).iter();
    if let Some(length) = lengths.next() {
        let spelling = length.as_bytes();
        if lengths.next().is_some()
            || spelling.is_empty()
            || !spelling.iter().all(u8::is_ascii_digit)
        {
            return Err(ControlHttpError::InvalidHeaders);
        }
        let length = length
            .to_str()
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or(ControlHttpError::InvalidHeaders)?;
        if length > MAX_RESPONSE_BYTES as u64 {
            return Err(ControlHttpError::ResponseTooLarge);
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests;
