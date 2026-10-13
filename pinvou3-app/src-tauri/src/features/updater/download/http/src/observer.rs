//! Transient owner observations, not durable cache, time or quality evidence.
use crate::{CacheReceiveError, FileBody};
use pinvou_upgrade_transfer::{Artifact, WriteMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObserverControl {
    Continue,
    Stop,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitPhase {
    Fetch,
    Body,
}

/// Actual validated header/plan shape, before writer start. Optional strong ETag
/// bytes remain opaque; they prove neither source continuity nor retained bytes.
/// No Debug/serialization or automatic logging of these facts is provided.
pub struct ResponseNotice<'target, 'borrow> {
    target: Artifact<'target>,
    mode: WriteMode,
    span: u64,
    validator: Option<&'borrow [u8]>,
}
impl<'target> ResponseNotice<'target, '_> {
    pub fn target(&self) -> Artifact<'target> {
        self.target
    }
    pub fn write_mode(&self) -> WriteMode {
        self.mode
    }
    pub fn response_span(&self) -> u64 {
        self.span
    }
    pub fn strong_validator_bytes(&self) -> Option<&[u8]> {
        self.validator
    }
}
/// Counts only fully successful actual write calls. A failed write may already
/// have left partial bytes without a progress notice. No sync/hash is implied.
pub struct ProgressNotice<'target> {
    target: Artifact<'target>,
    response_bytes: u64,
    next_offset: u64,
}
impl<'target> ProgressNotice<'target> {
    pub fn target(&self) -> Artifact<'target> {
        self.target
    }
    pub fn response_bytes(&self) -> u64 {
        self.response_bytes
    }
    pub fn next_offset(&self) -> u64 {
        self.next_offset
    }
}
pub struct WaitingNotice<'target> {
    target: Artifact<'target>,
    phase: WaitPhase,
}
impl<'target> WaitingNotice<'target> {
    pub fn target(&self) -> Artifact<'target> {
        self.target
    }
    pub fn phase(&self) -> WaitPhase {
        self.phase
    }
}
pub enum CacheNotice<'target, 'borrow> {
    Response(ResponseNotice<'target, 'borrow>),
    Progress(ProgressNotice<'target>),
    Waiting(WaitingNotice<'target>),
}
/// Callbacks must return promptly, without reentering this operation or nesting
/// block_on. Waiting callbacks may execute inside a runtime future poll. Stop
/// consumes the transfer without receipt; it is not a signed business outcome.
/// Borrowed fields are temporary and never establish ownership or authority.
pub trait CacheObserver {
    fn observe(&mut self, notice: CacheNotice<'_, '_>) -> ObserverControl;
}
pub(super) struct NoopObserver;
impl CacheObserver for NoopObserver {
    fn observe(&mut self, _: CacheNotice<'_, '_>) -> ObserverControl {
        ObserverControl::Continue
    }
}
fn notify(
    observer: &mut dyn CacheObserver,
    notice: CacheNotice<'_, '_>,
) -> Result<(), CacheReceiveError> {
    match observer.observe(notice) {
        ObserverControl::Continue => Ok(()),
        ObserverControl::Stop => Err(CacheReceiveError::OwnerStopped),
    }
}
pub(super) fn response(
    observer: &mut dyn CacheObserver,
    body: &FileBody<'_>,
) -> Result<(), CacheReceiveError> {
    let plan = body.plan();
    notify(
        observer,
        CacheNotice::Response(ResponseNotice {
            target: plan.target(),
            mode: plan.write_mode(),
            span: plan.expected_response_length(),
            validator: body.strong_validator_bytes(),
        }),
    )
}
pub(super) fn progress(
    observer: &mut dyn CacheObserver,
    target: Artifact<'_>,
    response_bytes: u64,
    next_offset: u64,
) -> Result<(), CacheReceiveError> {
    notify(
        observer,
        CacheNotice::Progress(ProgressNotice {
            target,
            response_bytes,
            next_offset,
        }),
    )
}
pub(super) fn waiting(
    observer: &mut dyn CacheObserver,
    target: Artifact<'_>,
    phase: WaitPhase,
) -> Result<(), CacheReceiveError> {
    notify(
        observer,
        CacheNotice::Waiting(WaitingNotice { target, phase }),
    )
}
