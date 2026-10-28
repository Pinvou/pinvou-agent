//! Dedicated no-now time-response verification. The caller owns protected Root,
//! deny/high-water storage, live nonce ownership and real monotonic observations.
//! Successful DATA is not an installed clock, current rights or execution proof.
use crate::{VerificationError, signatures};
use pinvou_upgrade_codec::{
    InputKind, RootPolicyShape, SchemaCheckedEnvelope, TimeContractError, TimeRequestShape,
    decode_canonical_envelope,
};
use std::{borrow::Cow, collections::BTreeSet};

const MAXIMUM: u64 = 9_007_199_254_740_991;
const ROOT_LIFETIME: u64 = 365 * 86_400_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeVerificationError {
    Contract(TimeContractError),
    Verification(VerificationError),
    InvalidObservation,
    RootBinding,
    RootWindow,
    Interval,
    Rollback,
}
impl std::fmt::Display for TimeVerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Contract(error) => return error.fmt(f),
            Self::Verification(error) => return error.fmt(f),
            Self::InvalidObservation => "TIME_OBSERVATION_INVALID",
            Self::RootBinding => "TIME_ROOT_BINDING_INVALID",
            Self::RootWindow => "TIME_ROOT_WINDOW_INVALID",
            Self::Interval => "TIME_INTERVAL_INVALID",
            Self::Rollback => "TIME_FLOOR_ROLLBACK",
        })
    }
}
impl std::error::Error for TimeVerificationError {}
impl From<VerificationError> for TimeVerificationError {
    fn from(error: VerificationError) -> Self {
        Self::Verification(error)
    }
}

/// Mandatory caller-observed DATA in milliseconds, not proof of a clock source,
/// freshness policy or a live host incarnation. No default policy is inferred.
/// sent is the actual send-floor bound; received is an actual receive/verification-
/// ceil bound, never two independently truncated timestamps. A zero interval is
/// a numeric boundary case, not evidence of a zero-duration network exchange.
/// The owner must advance this interval with its own proven counter after asOf;
/// this pure verifier does not measure or erase processing/persistence latency.
#[derive(Clone, Copy)]
pub struct SuppliedTimeObservation {
    pub sent_monotonic_millis: u64,
    pub received_monotonic_millis: u64,
    pub max_round_trip_millis: u64,
    pub max_uncertainty_millis: u64,
    pub trusted_time_floor_millis: u64,
}

pub struct TimeVerificationInputs<'a> {
    trusted_root_bytes: &'a [u8],
    successors: &'a [&'a [u8]],
    request: &'a TimeRequestShape,
    observation: SuppliedTimeObservation,
    denied_key_ids: &'a [String],
}
impl<'a> TimeVerificationInputs<'a> {
    /// Root bytes must be the caller's independently protected accepted Root,
    /// never a Root supplied by the incoming time response. Successors are exact
    /// canonical originals. Neither this constructor nor a parsed nonce proves
    /// custody, independent high-water/deny freshness or entropy.
    pub fn new(
        trusted_root_bytes: &'a [u8],
        successors: &'a [&'a [u8]],
        request: &'a TimeRequestShape,
        observation: SuppliedTimeObservation,
        denied_key_ids: &'a [String],
    ) -> Self {
        Self {
            trusted_root_bytes,
            successors,
            request,
            observation,
            denied_key_ids,
        }
    }
}

/// Dedicated signature-checked interval DATA only. No public constructor,
/// Debug, Serialize, Clone, trusted-clock or execution-capability entrance.
pub struct SignatureCheckedTimeResponse {
    envelope: SchemaCheckedEnvelope,
    earliest: u64,
    latest: u64,
    as_of_monotonic: u64,
    root_version: u64,
    root_sha256: String,
    signer_ids: BTreeSet<String>,
    checked_steps: usize,
}
impl SignatureCheckedTimeResponse {
    pub fn envelope(&self) -> &SchemaCheckedEnvelope {
        &self.envelope
    }
    pub fn earliest_millis(&self) -> u64 {
        self.earliest
    }
    pub fn latest_millis(&self) -> u64 {
        self.latest
    }
    pub fn as_of_monotonic_millis(&self) -> u64 {
        self.as_of_monotonic
    }
    pub fn root_version(&self) -> u64 {
        self.root_version
    }
    pub fn root_sha256(&self) -> &str {
        &self.root_sha256
    }
    pub fn verified_signer_ids(&self) -> &BTreeSet<String> {
        &self.signer_ids
    }
    pub fn checked_root_steps(&self) -> usize {
        self.checked_steps
    }
}

struct RootCandidate {
    root: RootPolicyShape,
    issued: u64,
    expires: u64,
}

/// Authenticate the dedicated body and continuous Root lineage without treating
/// response serverTime as an already trusted now. Window checks happen only in
/// this private candidate path, against the complete derived interval below.
pub fn verify_time_response(
    bytes: &[u8],
    inputs: TimeVerificationInputs<'_>,
) -> Result<SignatureCheckedTimeResponse, TimeVerificationError> {
    verify_time_response_source(
        bytes,
        inputs.trusted_root_bytes,
        inputs.successors.len(),
        |index| Ok(Cow::Borrowed(inputs.successors[index])),
        inputs.request,
        inputs.observation,
        inputs.denied_key_ids,
    )
}

/// Verify the same time response with one original successor at a time. The
/// source is caller-owned DATA: it cannot establish protected custody, latest
/// Root, a trusted clock, or completeness. `count` is the exact batch length;
/// source errors abort verification and are never interpreted as end-of-chain.
/// Per-object codec limits and the existing 4096-step batch limit still apply.
pub fn verify_time_response_source<'a>(
    bytes: &[u8],
    trusted_root_bytes: &[u8],
    count: usize,
    mut read: impl FnMut(usize) -> Result<Cow<'a, [u8]>, TimeVerificationError>,
    request: &TimeRequestShape,
    observation: SuppliedTimeObservation,
    denied_key_ids: &[String],
) -> Result<SignatureCheckedTimeResponse, TimeVerificationError> {
    let round_trip = observed_round_trip(observation)?;
    if count > 4096 || denied_key_ids.len() > 4096 {
        return Err(TimeVerificationError::InvalidObservation);
    }
    let mut denied = BTreeSet::new();
    for id in denied_key_ids {
        if !hash_shape(id) || !denied.insert(id.as_str()) {
            return Err(TimeVerificationError::InvalidObservation);
        }
    }
    // The initial original is independently protected, not newly self-signed.
    // Retain only window facts needed after the response interval is known.
    let (_, mut current) = root_candidate(trusted_root_bytes, request.product())?;
    let mut latest_issue = current.issued;
    let mut invalid_lifetime = current.expires <= current.issued
        || current.expires.saturating_sub(current.issued) > ROOT_LIFETIME;
    let mut final_sha = sha256(trusted_root_bytes);
    for index in 0..count {
        let next_bytes = read(index)?;
        let (envelope, next) = root_candidate(&next_bytes, request.product())?;
        if current.root.version() >= MAXIMUM || next.root.version() != current.root.version() + 1 {
            return Err(VerificationError::RootContinuityInvalid.into());
        }
        reject_denied(&envelope, &denied)?;
        signatures::verify_root_transition_signatures(&envelope, &current.root, &next.root)?;
        latest_issue = latest_issue.max(next.issued);
        invalid_lifetime |=
            next.expires <= next.issued || next.expires.saturating_sub(next.issued) > ROOT_LIFETIME;
        final_sha = sha256(&next_bytes);
        current = next;
    }
    if request.root_version() != current.root.version() || request.root_sha256() != final_sha {
        return Err(TimeVerificationError::RootBinding);
    }
    let envelope = decode_canonical_envelope(bytes, InputKind::Credential)
        .map_err(VerificationError::Envelope)?
        .check_full_schema()
        .map_err(VerificationError::Schema)?;
    let response = envelope
        .envelope()
        .signed()
        .time_response_shape()
        .map_err(TimeVerificationError::Contract)?;
    if response.product() != request.product()
        || response.component() != request.component()
        || response.request_nonce() != request.request_nonce()
        || response.root_version() != request.root_version()
        || response.root_sha256() != request.root_sha256()
    {
        return Err(TimeVerificationError::RootBinding);
    }
    reject_denied(&envelope, &denied)?;
    let signer_ids = signatures::verify_signatures(&envelope, &current.root)?;
    let uncertainty = response.uncertainty_millis();
    if uncertainty > observation.max_uncertainty_millis {
        return Err(TimeVerificationError::Interval);
    }
    let earliest = response
        .server_time_millis()
        .checked_sub(uncertainty)
        .ok_or(TimeVerificationError::Interval)?;
    let latest = response
        .server_time_millis()
        .checked_add(uncertainty)
        .and_then(|n| n.checked_add(round_trip))
        .filter(|n| *n <= MAXIMUM)
        .ok_or(TimeVerificationError::Interval)?;
    if earliest < observation.trusted_time_floor_millis {
        return Err(TimeVerificationError::Rollback);
    }
    // Historical Roots may have expired; none may be future-issued or have an
    // invalid original lifetime. Only the final Root must cover both edges.
    if latest_issue > earliest || invalid_lifetime || current.expires <= latest {
        return Err(TimeVerificationError::RootWindow);
    }
    Ok(SignatureCheckedTimeResponse {
        envelope,
        earliest,
        latest,
        as_of_monotonic: observation.received_monotonic_millis,
        root_version: current.root.version(),
        root_sha256: final_sha,
        signer_ids,
        checked_steps: count,
    })
}
fn observed_round_trip(o: SuppliedTimeObservation) -> Result<u64, TimeVerificationError> {
    if [
        o.sent_monotonic_millis,
        o.received_monotonic_millis,
        o.max_round_trip_millis,
        o.max_uncertainty_millis,
        o.trusted_time_floor_millis,
    ]
    .iter()
    .any(|v| *v > MAXIMUM)
    {
        return Err(TimeVerificationError::InvalidObservation);
    }
    o.received_monotonic_millis
        .checked_sub(o.sent_monotonic_millis)
        .filter(|elapsed| *elapsed <= o.max_round_trip_millis)
        .ok_or(TimeVerificationError::InvalidObservation)
}
fn root_candidate(
    bytes: &[u8],
    product: &str,
) -> Result<(SchemaCheckedEnvelope, RootCandidate), TimeVerificationError> {
    let envelope = decode_canonical_envelope(bytes, InputKind::Metadata)
        .map_err(VerificationError::Envelope)?
        .check_full_schema()
        .map_err(VerificationError::Schema)?;
    let signed = envelope.envelope().signed();
    let root = signed
        .root_policy_shape()
        .map_err(VerificationError::Root)?;
    if root.product() != product {
        return Err(TimeVerificationError::RootBinding);
    }
    // root_policy_shape already checks integer shape without validating now.
    let issued = signed.value()["issuedAt"]
        .as_f64()
        .ok_or(TimeVerificationError::RootWindow)? as u64;
    let expires = signed.value()["expiresAt"]
        .as_f64()
        .ok_or(TimeVerificationError::RootWindow)? as u64;
    Ok((
        envelope,
        RootCandidate {
            root,
            issued,
            expires,
        },
    ))
}
fn reject_denied(
    envelope: &SchemaCheckedEnvelope,
    denied: &BTreeSet<&str>,
) -> Result<(), TimeVerificationError> {
    if envelope
        .envelope()
        .signatures()
        .iter()
        .any(|signature| denied.contains(signature.key_id()))
    {
        return Err(VerificationError::SigningKeyDenied.into());
    }
    Ok(())
}
fn hash_shape(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn sha256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
