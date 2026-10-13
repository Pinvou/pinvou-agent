//! One original restricted GET/cache attempt; qualification and URL renewal are external.
use crate::cache::await_or_cancel;
use crate::stream::{StallLimits, fetch_with_profile};
use crate::{
    AddressResolver, CacheReceiveError, DestinationPolicy, FetchResult,
    receive_cache_body_on_worker,
};
use pinvou_upgrade_byte_check::{ByteCheckError, CacheWritePermission, SegmentWriteError};
use pinvou_upgrade_transfer::{Artifact, RequestPlan, RestartReason, SegmentReceipt, WriteMode};
use std::fs::File;

/// Separate owner approval to discard existing bytes on a legal 200 response.
/// An attempted original range already requests append on a legal 206 response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplacementPermission {
    Refuse,
    Allow,
}

/// Immutable attempt inputs; no second target, arbitrary headers or client.
/// The owner supplies current approval/qualification, protected prefix evidence
/// and actual file ownership. Constructor shape does not establish those facts.
pub struct CacheFetchRequest<'request, 'target> {
    url: &'request str,
    plan: RequestPlan<'target>,
    policy: &'request dyn DestinationPolicy,
    resolver: &'request dyn AddressResolver,
    replacement: ReplacementPermission,
}
impl<'request, 'target> CacheFetchRequest<'request, 'target> {
    pub fn new(
        url: &'request str,
        plan: RequestPlan<'target>,
        policy: &'request dyn DestinationPolicy,
        resolver: &'request dyn AddressResolver,
        replacement: ReplacementPermission,
    ) -> Self {
        Self {
            url,
            plan,
            policy,
            resolver,
            replacement,
        }
    }
}

/// None of these outcomes certifies a whole-file hash or installation rights.
pub enum CacheFetchOutcome<'a> {
    Segment(SegmentReceipt<'a>),
    Restart(RestartReason),
    VerifyExisting(Artifact<'a>),
}

/// Exactly one GET (or B1's complete-cache verification request), with no retry,
/// qualification refresh, URL renewal or deadline reset. Run only on an owner
/// blocking worker with live IO/timer driving, as required by the B8 receiver.
/// Cancellation also observes DNS/header waits without restarting their futures.
/// Restart/VerifyExisting leave file bytes, length and cursor untouched; existing
/// cache verification is still required later under the original validation budget.
pub fn fetch_cache_once_on_worker<'a>(
    runtime: &tokio::runtime::Handle,
    request: CacheFetchRequest<'_, 'a>,
    file: &mut File,
    mut cancelled: impl FnMut() -> bool,
) -> Result<CacheFetchOutcome<'a>, CacheReceiveError> {
    fetch_with_cache_profile(
        runtime,
        request,
        file,
        &mut cancelled,
        StallLimits::default(),
    )
}

pub(super) fn fetch_with_cache_profile<'a>(
    runtime: &tokio::runtime::Handle,
    request: CacheFetchRequest<'_, 'a>,
    file: &mut File,
    cancelled: &mut impl FnMut() -> bool,
    limits: StallLimits,
) -> Result<CacheFetchOutcome<'a>, CacheReceiveError> {
    let fetched = runtime.block_on(await_or_cancel(
        fetch_with_profile(
            request.url,
            request.plan,
            request.policy,
            request.resolver,
            limits,
        ),
        cancelled,
    ))?;
    // Even an immediate local outcome or a ready fetch must observe cancellation
    // before a write or returning a future-action projection.
    if cancelled() {
        return Err(CacheReceiveError::Write(SegmentWriteError::File(
            ByteCheckError::Cancelled,
        )));
    }
    match fetched {
        FetchResult::Restart(reason) => Ok(CacheFetchOutcome::Restart(reason)),
        FetchResult::VerifyExisting(target) => Ok(CacheFetchOutcome::VerifyExisting(target)),
        FetchResult::Transfer(body) => {
            let permission = match body.plan().write_mode() {
                WriteMode::Append { .. } => CacheWritePermission::AppendExisting,
                WriteMode::ReplaceFromZero
                    if request.replacement == ReplacementPermission::Allow =>
                {
                    CacheWritePermission::ReplaceExisting
                }
                WriteMode::ReplaceFromZero => {
                    return Err(CacheReceiveError::Write(
                        SegmentWriteError::WritePermissionMismatch,
                    ));
                }
            };
            receive_cache_body_on_worker(runtime, *body, file, permission, cancelled)
                .map(CacheFetchOutcome::Segment)
        }
    }
}
