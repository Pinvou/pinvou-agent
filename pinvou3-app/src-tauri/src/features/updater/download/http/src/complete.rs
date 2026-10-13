//! One actual cache attempt followed, when complete, by an original-budget scan.
use crate::attempt::fetch_with_cache_observer_profile;
use crate::observer::NoopObserver;
use crate::stream::StallLimits;
use crate::{CacheFetchOutcome, CacheFetchRequest, CacheObserver, CacheReceiveError};
use pinvou_upgrade_byte_check::{ByteCheckError, ByteMatch, check_file};
use pinvou_upgrade_transfer::{Artifact, RestartReason, SegmentReceipt};
use std::fs::File;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationStop {
    Expired,
    Unproven,
    TimeFault,
    OwnerStopped,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationDecision {
    Continue,
    Stop(ValidationStop),
}

/// Trusted owner integration, not a time provider or budget proof. Begin must
/// start or resume the original workflow/transaction/target record without refreshing
/// its original deadline. Poll must stop on expired/unproven/fault evidence.
/// Both callbacks return promptly and preserve exclusive ownership of the file.
/// This is invoked only for whole-file validation, never ongoing downloads.
pub trait CacheValidationOwner {
    fn begin_or_resume(&mut self, target: Artifact<'_>) -> ValidationDecision;
    fn poll(&mut self, target: Artifact<'_>) -> ValidationDecision;
}

pub enum CheckedCacheOutcome<'a> {
    Partial(SegmentReceipt<'a>),
    Restart(RestartReason),
    /// Only observed whole-file bytes; no protection, native identity or rights.
    Complete(ByteMatch<'a>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheCheckError {
    Transfer(CacheReceiveError),
    Check(ByteCheckError),
    Validation(ValidationStop),
}
impl std::fmt::Display for CacheCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transfer(error) => error.fmt(f),
            Self::Check(error) => error.fmt(f),
            Self::Validation(reason) => f.write_str(match reason {
                ValidationStop::Expired => "CACHE_VALIDATION_EXPIRED",
                ValidationStop::Unproven => "CACHE_VALIDATION_UNPROVEN",
                ValidationStop::TimeFault => "CACHE_VALIDATION_TIME_FAULT",
                ValidationStop::OwnerStopped => "CACHE_VALIDATION_OWNER_STOPPED",
            }),
        }
    }
}
impl std::error::Error for CacheCheckError {}

/// Exactly one original attempt and, only on completeness, a scan of this same
/// open file from zero through real EOF. There are no retries, URL renewal or
/// internally generated clocks/deadlines. Owner blocking-worker/runtime rules
/// and explicit replacement permission are unchanged from the single attempt.
/// On any failure, output remains untrusted and caller-owned; no path is reopened.
pub fn fetch_and_check_cache_once_on_worker<'a>(
    runtime: &tokio::runtime::Handle,
    request: CacheFetchRequest<'_, 'a>,
    file: &mut File,
    mut cancelled: impl FnMut() -> bool,
    validation: &mut dyn CacheValidationOwner,
) -> Result<CheckedCacheOutcome<'a>, CacheCheckError> {
    fetch_and_check_with_profile(
        runtime,
        request,
        file,
        &mut cancelled,
        &mut NoopObserver,
        validation,
        StallLimits::default(),
    )
}
pub fn fetch_and_check_cache_once_with_observer_on_worker<'a>(
    runtime: &tokio::runtime::Handle,
    request: CacheFetchRequest<'_, 'a>,
    file: &mut File,
    mut cancelled: impl FnMut() -> bool,
    observer: &mut dyn CacheObserver,
    validation: &mut dyn CacheValidationOwner,
) -> Result<CheckedCacheOutcome<'a>, CacheCheckError> {
    fetch_and_check_with_profile(
        runtime,
        request,
        file,
        &mut cancelled,
        observer,
        validation,
        StallLimits::default(),
    )
}
pub(super) fn fetch_and_check_with_profile<'a>(
    runtime: &tokio::runtime::Handle,
    request: CacheFetchRequest<'_, 'a>,
    file: &mut File,
    cancelled: &mut impl FnMut() -> bool,
    observer: &mut dyn CacheObserver,
    validation: &mut dyn CacheValidationOwner,
    limits: StallLimits,
) -> Result<CheckedCacheOutcome<'a>, CacheCheckError> {
    let target = match fetch_with_cache_observer_profile(
        runtime, request, file, cancelled, observer, limits,
    )
    .map_err(CacheCheckError::Transfer)?
    {
        CacheFetchOutcome::Restart(reason) => return Ok(CheckedCacheOutcome::Restart(reason)),
        CacheFetchOutcome::Segment(receipt) if receipt.next_offset() != receipt.target().size() => {
            return Ok(CheckedCacheOutcome::Partial(receipt));
        }
        CacheFetchOutcome::Segment(receipt) => receipt.target(),
        CacheFetchOutcome::VerifyExisting(target) => target,
    };
    if cancelled() {
        return Err(CacheCheckError::Check(ByteCheckError::Cancelled));
    }
    if let ValidationDecision::Stop(reason) = validation.begin_or_resume(target) {
        return Err(CacheCheckError::Validation(reason));
    }
    let mut stopped = None;
    let checked = check_file(file, target, || {
        if stopped.is_some() || cancelled() {
            return true;
        }
        match validation.poll(target) {
            ValidationDecision::Continue => cancelled(),
            ValidationDecision::Stop(reason) => {
                stopped = Some(reason);
                true
            }
        }
    });
    if let Some(reason) = stopped {
        return Err(CacheCheckError::Validation(reason));
    }
    let matched = checked.map_err(CacheCheckError::Check)?;
    // Recheck the owner boundary after final size/hash computation. A receipt
    // cannot survive a new cancellation or unavailable original validation budget.
    if cancelled() {
        return Err(CacheCheckError::Check(ByteCheckError::Cancelled));
    }
    if let ValidationDecision::Stop(reason) = validation.poll(target) {
        return Err(CacheCheckError::Validation(reason));
    }
    // The final owner callback may itself publish cancellation while returning
    // Continue. Observe it before exposing this scan's match.
    if cancelled() {
        return Err(CacheCheckError::Check(ByteCheckError::Cancelled));
    }
    Ok(CheckedCacheOutcome::Complete(matched))
}
