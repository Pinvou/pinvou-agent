//! Structural transfer planning, not source trust, file ownership or verification.
//! No network, clock, retry cap, credentials or filesystem access is supplied.

mod headers;
mod identity;
mod response;

pub use headers::{ResponseHeaders, StrongEtag};
pub use identity::{
    Artifact, PrefixState, RequestKind, RequestPlan, SourceContinuity, plan_request,
};
pub use response::{
    BodyPlan, BodyProgress, ResponseDecision, RestartReason, SegmentReceipt, WriteMode,
    plan_response,
};

const MAX_SAFE_SIZE: u64 = 9_007_199_254_740_991;
// Local input profile only. Providers must be qualified against these bounds.
const MAX_HEADER_FIELDS: usize = 4096;
const MAX_HEADER_BYTES: usize = 1_048_576;
const MAX_HEADER_VALUE_BYTES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferError {
    InvalidArtifact,
    InvalidPrefixLength,
    InvalidEtag,
    HeadersAboveLimit,
    DuplicateHeader,
    UnsupportedEncoding,
    InvalidContentLength,
    InvalidContentRange,
    RangeMismatch,
    ValidatorMismatch,
    UnexpectedPartialResponse,
    HttpStatusRefused(u16),
    LocalVerificationRequired,
    BodyTooLong,
    BodyTruncated,
    BodyAborted,
    BodyClosed,
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidArtifact => "FILE_ARTIFACT_INVALID",
            Self::InvalidPrefixLength => "FILE_PREFIX_LENGTH_INVALID",
            Self::InvalidEtag => "FILE_ETAG_INVALID",
            Self::HeadersAboveLimit => "FILE_HEADERS_ABOVE_LIMIT",
            Self::DuplicateHeader => "FILE_HEADER_DUPLICATE",
            Self::UnsupportedEncoding => "FILE_ENCODING_UNSUPPORTED",
            Self::InvalidContentLength => "FILE_CONTENT_LENGTH_INVALID",
            Self::InvalidContentRange => "FILE_CONTENT_RANGE_INVALID",
            Self::RangeMismatch => "FILE_RANGE_MISMATCH",
            Self::ValidatorMismatch => "FILE_VALIDATOR_MISMATCH",
            Self::UnexpectedPartialResponse => "FILE_PARTIAL_RESPONSE_UNEXPECTED",
            Self::HttpStatusRefused(_) => "FILE_HTTP_STATUS_REFUSED",
            Self::LocalVerificationRequired => "FILE_LOCAL_VERIFICATION_REQUIRED",
            Self::BodyTooLong => "FILE_BODY_TOO_LONG",
            Self::BodyTruncated => "FILE_BODY_TRUNCATED",
            Self::BodyAborted => "FILE_BODY_ABORTED",
            Self::BodyClosed => "FILE_BODY_CLOSED",
        })
    }
}
impl std::error::Error for TransferError {}

fn decimal(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    let mut number = 0_u64;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        number = number
            .checked_mul(10)?
            .checked_add(u64::from(byte - b'0'))?;
        if number > MAX_SAFE_SIZE {
            return None;
        }
    }
    Some(number)
}

fn trim_ows(bytes: &[u8]) -> &[u8] {
    let first = bytes
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .unwrap_or(bytes.len());
    let bytes = &bytes[first..];
    let last = bytes
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(0, |position| position + 1);
    &bytes[..last]
}

fn split_pair(bytes: &[u8], separator: u8) -> Option<(&[u8], &[u8])> {
    let position = bytes.iter().position(|byte| *byte == separator)?;
    Some((&bytes[..position], &bytes[position + 1..]))
}
