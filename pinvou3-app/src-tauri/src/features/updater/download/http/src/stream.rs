use crate::destination::{DenyFallbackResolver, FileDestination, MAX_ADDRESSES};
use crate::{AddressResolver, DestinationPolicy, FileHttpError, ResolveError, network_error};
use bytes::Bytes;
use pinvou_upgrade_transfer::{
    Artifact, BodyPlan, BodyProgress, RequestKind, RequestPlan, ResponseDecision, ResponseHeaders,
    RestartReason, SegmentReceipt, plan_response,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const DELIVERED_CHUNK_BYTES: usize = 65_536;

struct StallLimits {
    connect: Duration,
    read_idle: Duration,
    #[cfg(test)]
    fixture_root: Option<reqwest::Certificate>,
}
impl Default for StallLimits {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            read_idle: Duration::from_secs(30),
            #[cfg(test)]
            fixture_root: None,
        }
    }
}

pub enum FetchResult<'a> {
    Transfer(Box<FileBody<'a>>),
    Restart(RestartReason),
    VerifyExisting(Artifact<'a>),
}
pub enum FileChunk<'a> {
    Data(Vec<u8>),
    SegmentFinished(SegmentReceipt<'a>),
}

pub async fn fetch<'a>(
    raw_url: &str,
    request: RequestPlan<'a>,
    policy: &dyn DestinationPolicy,
    resolver: &dyn AddressResolver,
) -> Result<FetchResult<'a>, FileHttpError> {
    fetch_with_profile(raw_url, request, policy, resolver, StallLimits::default()).await
}

async fn fetch_with_profile<'a>(
    raw_url: &str,
    request: RequestPlan<'a>,
    policy: &dyn DestinationPolicy,
    resolver: &dyn AddressResolver,
    limits: StallLimits,
) -> Result<FetchResult<'a>, FileHttpError> {
    if matches!(request.kind(), RequestKind::VerifyExisting) {
        return Ok(FetchResult::VerifyExisting(request.target()));
    }
    let destination = FileDestination::parse(raw_url)?;
    if !policy.permits_origin(&destination.origin) {
        return Err(FileHttpError::OriginDenied);
    }
    let addresses = if let Some(ip) = destination.literal {
        vec![SocketAddr::new(ip, destination.origin.port())]
    } else {
        tokio::time::timeout(limits.connect, resolver.resolve(&destination.origin))
            .await
            .map_err(|_| FileHttpError::DnsTimeout)?
            .map_err(|error| match error {
                ResolveError::Failed => FileHttpError::DnsFailed,
                ResolveError::AboveLimit => FileHttpError::InvalidResolution,
            })?
    };
    if addresses.is_empty()
        || addresses.len() > MAX_ADDRESSES
        || addresses.iter().any(|address| {
            address.port() != destination.origin.port()
                || address.ip().is_unspecified()
                || address.ip().is_multicast()
        })
    {
        return Err(FileHttpError::InvalidResolution);
    }
    // Recheck the same current immutable policy after awaiting DNS. Every
    // approved address is the URI's actual port; literal hosts bypass DNS.
    if !policy.permits_origin(&destination.origin) {
        return Err(FileHttpError::OriginDenied);
    }
    if addresses
        .iter()
        .any(|address| !policy.permits_address(&destination.origin, *address))
    {
        return Err(FileHttpError::AddressDenied);
    }
    let builder = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .no_gzip()
        .no_brotli()
        .no_zstd()
        .no_deflate()
        .pool_max_idle_per_host(0)
        .connect_timeout(limits.connect)
        .read_timeout(limits.read_idle)
        .dns_resolver(Arc::new(DenyFallbackResolver))
        .resolve_to_addrs(destination.origin.host(), &addresses);
    #[cfg(test)]
    let builder = match limits.fixture_root {
        Some(root) => builder.add_root_certificate(root),
        None => builder,
    };
    let client = builder
        .build()
        .map_err(|_| FileHttpError::ClientUnavailable)?;
    let mut get = client
        .get(destination.url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity");
    if let RequestKind::Range { offset, validator } = request.kind() {
        get = get
            .header(reqwest::header::RANGE, format!("bytes={offset}-"))
            .header(
                reqwest::header::IF_RANGE,
                reqwest::header::HeaderValue::from_bytes(validator.as_bytes()).map_err(|_| {
                    FileHttpError::Transfer(pinvou_upgrade_transfer::TransferError::InvalidEtag)
                })?,
            );
    }
    let response = get.send().await.map_err(network_error)?;
    let headers = ResponseHeaders::from_fields(
        response
            .headers()
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_bytes())),
    )
    .map_err(FileHttpError::Transfer)?;
    validate_framing(response.headers())?;
    let decision = plan_response(request, response.status().as_u16(), headers)
        .map_err(FileHttpError::Transfer)?;
    // A valid opaque validator is still not immutable-source evidence. Weak or
    // malformed optional tags on a full response cannot be used for resumption.
    let validator = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| pinvou_upgrade_transfer::StrongEtag::parse(value.as_bytes()).ok())
        .map(|value| value.as_bytes().to_vec());
    match decision {
        ResponseDecision::Transfer(plan) => Ok(FetchResult::Transfer(Box::new(FileBody {
            response: Some(response),
            plan,
            progress: plan.receive(),
            pending: Bytes::new(),
            validator,
            state: StreamState::Receiving,
        }))),
        ResponseDecision::Restart(reason) => Ok(FetchResult::Restart(reason)),
        ResponseDecision::VerifyExisting(target) => Ok(FetchResult::VerifyExisting(target)),
    }
}

enum StreamState {
    Receiving,
    Failed(FileHttpError),
    Finished,
}

fn validate_framing(headers: &reqwest::header::HeaderMap) -> Result<(), FileHttpError> {
    let mut encodings = headers.get_all(reqwest::header::TRANSFER_ENCODING).iter();
    if let Some(encoding) = encodings.next() {
        // The SDK frames chunked bodies but accepts other transfer codings and
        // gives Transfer-Encoding precedence over Content-Length. The qualified
        // profile permits exactly one chunked coding, with no length conflict.
        if encodings.next().is_some()
            || !encoding.as_bytes().eq_ignore_ascii_case(b"chunked")
            || headers.contains_key(reqwest::header::CONTENT_LENGTH)
        {
            return Err(FileHttpError::InvalidFraming);
        }
    }
    Ok(())
}

/// Only output fragments are capped at 64 KiB. SDK parsing/buffering has its own
/// limits; this transport never collects the complete body before splitting it.
pub struct FileBody<'a> {
    response: Option<reqwest::Response>,
    plan: BodyPlan<'a>,
    progress: BodyProgress<'a>,
    pending: Bytes,
    validator: Option<Vec<u8>>,
    state: StreamState,
}
impl<'a> FileBody<'a> {
    /// The validated response's original target, write mode and exact span.
    /// Inspect before selecting the protected writer; a 200 replaces from zero.
    pub fn plan(&self) -> BodyPlan<'a> {
        self.plan
    }
    fn fail(&mut self, error: FileHttpError) -> FileHttpError {
        let _ = self.progress.abort();
        self.pending = Bytes::new();
        self.response = None;
        self.state = StreamState::Failed(error);
        error
    }
    pub fn abort_by_consumer(&mut self) -> Result<(), FileHttpError> {
        match self.state {
            StreamState::Receiving => Err(self.fail(FileHttpError::ConsumerAborted)),
            StreamState::Failed(error) => Err(error),
            StreamState::Finished => Err(FileHttpError::BodyClosed),
        }
    }
    pub fn strong_validator_bytes(&self) -> Option<&[u8]> {
        self.validator.as_deref()
    }
    pub async fn next_chunk(&mut self) -> Result<FileChunk<'a>, FileHttpError> {
        match self.state {
            StreamState::Failed(error) => return Err(error),
            StreamState::Finished => return Err(FileHttpError::BodyClosed),
            StreamState::Receiving => {}
        }
        loop {
            if !self.pending.is_empty() {
                let count = self.pending.len().min(DELIVERED_CHUNK_BYTES);
                return Ok(FileChunk::Data(self.pending.split_to(count).to_vec()));
            }
            let chunk = match self.response.as_mut() {
                Some(response) => response.chunk().await,
                None => return Err(self.fail(FileHttpError::BodyClosed)),
            };
            match chunk {
                Ok(Some(chunk)) => {
                    if let Err(error) = self.progress.observe_bytes(chunk.len() as u64) {
                        return Err(self.fail(FileHttpError::Transfer(error)));
                    }
                    self.pending = chunk;
                }
                Ok(None) => match self.progress.finish() {
                    Ok(receipt) => {
                        self.response = None;
                        self.state = StreamState::Finished;
                        return Ok(FileChunk::SegmentFinished(receipt));
                    }
                    Err(error) => return Err(self.fail(FileHttpError::Transfer(error))),
                },
                Err(error) => return Err(self.fail(network_error(error))),
            }
        }
    }
}

#[cfg(test)]
mod tests;
