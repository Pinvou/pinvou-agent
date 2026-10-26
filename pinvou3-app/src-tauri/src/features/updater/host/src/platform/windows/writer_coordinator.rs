//! Service-internal App startup admission. The caller keeps the real App write
//! and child-spawn barrier closed until the reply has been validated by the App.
//! This component does not produce Saved receipts, preparation/freeze authority,
//! filesystem fencing, CLI admission, or retroactive descendant containment.
//!
//! Lock order is state -> native BootstrapGuard. Native protected intents and
//! ClosedWriterRead guards are stack-only and never enter the Send state below.
//! All failed native consuming operations return their custody to this state.
mod keeper;
mod recovery;
mod writers;

use super::{
    installed::StartedServiceContext,
    keeper_spawn::{
        KeeperChild, KeeperPeer, KeeperRecoveryCustody, KeeperSpawnFailure, ParkedQueryCopy,
        PreparedKeeperSpawn, RecoveredKeeper, RunningKeeper,
    },
    peer::Peer,
    pipe::{Event, Pipe},
    process::RetainedProcess,
    registry::{Store, writers::WriterReference},
    writer_job::{PendingWriterJob, RecoveredWriterJob, ReservedWriterJob, WriterJob},
    writer_keeper::{KeeperJobAck, KeeperReadyAck, KeeperReleasedAck},
};
use crate::{error::Error, handoff::Role};
use pinvou_upgrade_ownership::live::Participant;
use std::sync::{Arc, Mutex};
use windows_sys::Win32::Foundation::CompareObjectHandles;

/// Finite concurrently held admissions, including uncertain and unreleased
/// admissions. Closed entries leave memory only after actual keeper release.
/// There is no lifetime admission limit or history deletion.
const MAX_HELD_WRITERS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FailureKind {
    /// A determined request refusal. Earlier custody, if any, remains held;
    /// this is never evidence that an older attempt had no native effects.
    Refused,
    /// Native/protected effects may exist; admission remains closed.
    Unknown,
    /// A protected publication/readback outcome was not determined.
    StateCommitUnknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CoordinatorFailure {
    pub(super) kind: FailureKind,
    pub(super) error: Error,
}
type Result<T> = std::result::Result<T, CoordinatorFailure>;
impl CoordinatorFailure {
    fn refused(error: Error) -> Self {
        Self::at(error, FailureKind::Refused)
    }
    fn unknown(error: Error) -> Self {
        Self::at(error, FailureKind::Unknown)
    }
    fn at(error: Error, otherwise: FailureKind) -> Self {
        Self {
            kind: if error == Error::StateCommitUnknown {
                FailureKind::StateCommitUnknown
            } else {
                otherwise
            },
            error,
        }
    }
}

/// Internal worker result, not a save/freeze/cancellation receipt.
pub(super) struct ReapReport {
    pub(super) released: usize,
    pub(super) retained: usize,
    pub(super) first_failure: Option<CoordinatorFailure>,
}

pub(super) struct WriterCoordinator {
    host: Arc<StartedServiceContext>,
    stop: Arc<Event>,
    state: Mutex<State>,
}
struct State {
    keeper: KeeperState,
    writers: Vec<WriterSlot>,
    recovered: Vec<RecoveredSlot>,
    recovery_active_loaded: bool,
    recovery_pending_loaded: bool,
}
enum KeeperState {
    Unselected,
    Prepared(PreparedKeeperSpawn),
    SpawnFailed(KeeperSpawnFailure),
    Child(KeeperChild),
    Running {
        keeper: RunningKeeper,
        ready: Option<KeeperReadyAck>,
    },
    Recovery(KeeperRecoveryCustody),
    Recovered(RecoveredKeeper),
    /// Only a synchronous ownership-transfer marker. Never returned as ready.
    Transition,
}
struct WriterSlot {
    original: Arc<RetainedProcess>,
    custody: WriterCustody,
    reference: Option<WriterReference>,
    keeper_copy: Option<ParkedQueryCopy>,
    keeper_ack: Option<KeeperJobAck>,
    release_ack: Option<KeeperReleasedAck>,
    close_attempted: bool,
}
enum WriterCustody {
    Reserved(ReservedWriterJob),
    Pending(PendingWriterJob),
    Assigned(WriterJob),
    Transition,
}
struct RecoveredSlot {
    reference: WriterReference,
    job: Option<RecoveredWriterJob>,
    close_attempted: bool,
    release_ack: Option<KeeperReleasedAck>,
}

impl WriterCoordinator {
    pub(super) fn new(host: Arc<StartedServiceContext>, stop: Arc<Event>) -> Self {
        Self {
            host,
            stop,
            state: Mutex::new(State {
                keeper: KeeperState::Unselected,
                writers: Vec::new(),
                recovered: Vec::new(),
                recovery_active_loaded: false,
                recovery_pending_loaded: false,
            }),
        }
    }
    /// Returns public participant DATA from the actual protected admission;
    /// private job names/handles/paths and keeper capabilities stay in service.
    pub(super) fn admit(&self, peer: &Peer, pipe: &Pipe) -> Result<Participant> {
        self.enroll(peer, pipe)
    }
    pub(super) fn maintenance(&self) -> Result<()> {
        let report = self.reap_exited()?;
        match report.first_failure {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
    }
    /// Called only by the actual App startup barrier RPC handler, after native
    /// Peer authentication on this same Pipe. No caller selector or flags enter.
    fn enroll(&self, peer: &Peer, pipe: &Pipe) -> Result<Participant> {
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        peer.recheck(pipe).map_err(CoordinatorFailure::refused)?;
        if peer.role != Role::App {
            return Err(CoordinatorFailure::refused(Error::PeerRefused));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        recovery::discover(&self.host, &mut state, &self.stop)?;
        // Complete keeper bootstrap BEFORE the first Reserved writer record:
        // fresh keeper readiness legitimately requires an empty writer journal.
        keeper::ensure_active(&self.host, &mut state.keeper, &self.stop)?;
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        let selected = state
            .writers
            .iter()
            .position(|slot| same_original(&slot.original, peer));
        let index = match selected {
            Some(index) => index,
            None => {
                if state.writers.len() + state.recovered.len() >= MAX_HELD_WRITERS {
                    return Err(CoordinatorFailure::refused(Error::Busy));
                }
                let job = ReservedWriterJob::reserve(&self.host, peer, pipe)
                    .map_err(CoordinatorFailure::refused)?;
                let index = state.writers.len();
                state.writers.push(WriterSlot {
                    original: Arc::clone(&peer.process),
                    custody: WriterCustody::Reserved(job),
                    reference: None,
                    keeper_copy: None,
                    keeper_ack: None,
                    release_ack: None,
                    close_attempted: false,
                });
                index
            }
        };
        let State {
            keeper, writers, ..
        } = &mut *state;
        let slot = &mut writers[index];
        writers::enroll(&self.host, keeper, slot, peer, pipe, &self.stop)
    }

    /// Root's service worker may call this between requests. A Busy original/job
    /// remains held. No process kill, cancellation, actor-count zero inference,
    /// preparation retirement, Saved acknowledgement, or admission reopening.
    pub(super) fn reap_exited(&self) -> Result<ReapReport> {
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        recovery::discover(&self.host, &mut state, &self.stop)?;
        if !state.recovered.is_empty()
            && !matches!(
                &state.keeper,
                KeeperState::Running { .. } | KeeperState::Recovered(_)
            )
        {
            // Initial recovery must authenticate the actual Active keeper.
            // Afterwards maintenance also needs the held original when it has
            // actually exited; requiring new live authentication would wedge
            // the sealed NativeKeeperExit cleanup path.
            keeper::ensure_active(&self.host, &mut state.keeper, &self.stop)?;
        }
        let mut released = 0;
        let mut first_failure = None;
        let State {
            keeper,
            writers,
            recovered,
            ..
        } = &mut *state;
        let mut index = 0;
        while index < writers.len() {
            check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
            match writers::close_and_release(&self.host, keeper, &mut writers[index], &self.stop) {
                Ok(()) => {
                    // Native job/original/query backup stay owned until the
                    // guarded Closed read and actual release completion succeed
                    // (typed ACK or the separately verified original exit).
                    writers.remove(index);
                    released += 1;
                }
                Err(failure) => {
                    if failure.error == Error::RestartCanceled {
                        return Err(failure);
                    }
                    if failure.error != Error::Busy && first_failure.is_none() {
                        first_failure = Some(failure);
                    }
                    index += 1;
                }
            }
        }
        let mut index = 0;
        while index < recovered.len() {
            check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
            match recovery::close_and_release(&self.host, keeper, &mut recovered[index], &self.stop)
            {
                Ok(()) => {
                    recovered.remove(index);
                    released += 1;
                }
                Err(failure) => {
                    if failure.error == Error::RestartCanceled {
                        return Err(failure);
                    }
                    if failure.error != Error::Busy && first_failure.is_none() {
                        first_failure = Some(failure);
                    }
                    index += 1;
                }
            }
        }
        if writers.is_empty() && recovered.is_empty() {
            if let Err(failure) = keeper::retire_observed_exit(&self.host, keeper, &self.stop) {
                if failure.error == Error::RestartCanceled {
                    return Err(failure);
                }
                if first_failure.is_none() {
                    first_failure = Some(failure);
                }
            }
        }
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        Ok(ReapReport {
            released,
            retained: writers.len() + recovered.len(),
            first_failure,
        })
    }
}

/// Read the actual shared manual-reset service stop event. Cancellation never
/// proves no effects, releases custody, or creates a native/serialized ACK.
fn check_stop(stop: &Event) -> crate::error::Result<()> {
    if stop.is_set()? {
        Err(Error::RestartCanceled)
    } else {
        Ok(())
    }
}

#[allow(unsafe_code)]
fn same_original(original: &RetainedProcess, peer: &Peer) -> bool {
    // Both handles are already held native objects. This performs no OpenProcess
    // and makes no inference from PID/creation-time/Arc address equality.
    unsafe { CompareObjectHandles(original.raw(), peer.process.raw()) != 0 }
}

impl KeeperState {
    fn peer(&mut self) -> Result<KeeperPeer<'_>> {
        match self {
            Self::Running { keeper, .. } => Ok(KeeperPeer::Running(keeper)),
            Self::Recovered(keeper) => Ok(KeeperPeer::Recovered(keeper)),
            _ => Err(CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_state_can_cross_threads_without_a_native_operation_guard() {
        fn require_send_sync<T: Send + Sync>() {}
        fn require_send<T: Send>() {}
        // A future accidental ProtectedIntent/ClosedWriterRead field makes this
        // fail to compile: BootstrapGuard is intentionally thread-affine !Send.
        // This creates no host/process/job, privileged token, or grant proof.
        require_send_sync::<WriterCoordinator>();
        require_send::<State>();
    }
}
