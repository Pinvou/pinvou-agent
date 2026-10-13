//! Restricted file-plane GET transport. Approval, qualification, immutable source
//! continuity, physical file protection and final verification remain owner duties.

mod observer;
pub use observer::{
    CacheNotice, CacheObserver, ObserverControl, ProgressNotice, ResponseNotice, WaitPhase,
    WaitingNotice,
};

mod attempt;
pub use attempt::{
    CacheFetchOutcome, CacheFetchRequest, ReplacementPermission, fetch_cache_once_on_worker,
    fetch_cache_once_with_observer_on_worker,
};

mod complete;
pub use complete::{
    CacheCheckError, CacheValidationOwner, CheckedCacheOutcome, ValidationDecision, ValidationStop,
    fetch_and_check_cache_once_on_worker, fetch_and_check_cache_once_with_observer_on_worker,
};

mod cache;
pub use cache::{
    CacheReceiveError, receive_cache_body_on_worker, receive_cache_body_with_observer_on_worker,
};

mod destination;
mod stream;

pub use destination::{
    AddressResolver, DestinationPolicy, FileOrigin, ResolveError, SystemResolver,
};
pub use stream::{FetchResult, FileBody, FileChunk, fetch};

use pinvou_upgrade_transfer::TransferError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileHttpError {
    InvalidUrl,
    OriginDenied,
    DnsFailed,
    DnsTimeout,
    InvalidResolution,
    AddressDenied,
    ClientUnavailable,
    RequestTimeout,
    ConnectionFailed,
    NetworkFailed,
    InvalidFraming,
    ConsumerAborted,
    BodyClosed,
    Transfer(TransferError),
}

impl std::fmt::Display for FileHttpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = match self {
            Self::InvalidUrl => "FILE_URL_INVALID",
            Self::OriginDenied => "FILE_ORIGIN_DENIED",
            Self::DnsFailed => "FILE_DNS_FAILED",
            Self::DnsTimeout => "FILE_DNS_TIMEOUT",
            Self::InvalidResolution => "FILE_RESOLUTION_INVALID",
            Self::AddressDenied => "FILE_ADDRESS_DENIED",
            Self::ClientUnavailable => "FILE_CLIENT_UNAVAILABLE",
            Self::RequestTimeout => "FILE_REQUEST_STALLED",
            Self::ConnectionFailed => "FILE_CONNECTION_FAILED",
            Self::NetworkFailed => "FILE_NETWORK_FAILED",
            Self::InvalidFraming => "FILE_FRAMING_INVALID",
            Self::ConsumerAborted => "FILE_CONSUMER_ABORTED",
            Self::BodyClosed => "FILE_BODY_CLOSED",
            Self::Transfer(error) => return error.fmt(formatter),
        };
        formatter.write_str(code)
    }
}
impl std::error::Error for FileHttpError {}

fn network_error(error: reqwest::Error) -> FileHttpError {
    // Drop the SDK error: it can contain the complete URL/access query. Never
    // retain it as an Error::source or put its Display/Debug in diagnostics.
    if error.is_timeout() {
        FileHttpError::RequestTimeout
    } else if error.is_connect() {
        FileHttpError::ConnectionFailed
    } else {
        FileHttpError::NetworkFailed
    }
}
