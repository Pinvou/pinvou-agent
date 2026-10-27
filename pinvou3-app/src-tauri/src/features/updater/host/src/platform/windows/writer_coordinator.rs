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
use crate::error::Error;
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

// Only for destructive cleanup: retain the poison flag so all ordinary state
// operations continue to fail closed. This does not clear the poison flag.
fn cleanup_with_poisoned_state<T, R>(state: &Mutex<T>, take: impl FnOnce(&mut T) -> R) -> R {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    take(&mut state)
}
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
/// Native peer proof produced only after actual custody and keeper rechecks.
pub(super) struct AdmittedCliPeer(Arc<RetainedProcess>);
impl AdmittedCliPeer {
    #[allow(unsafe_code)]
    pub(super) fn matches(&self, process: &RetainedProcess) -> bool {
        unsafe { CompareObjectHandles(self.0.raw(), process.raw()) != 0 }
    }
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
    // The service retains the native stage until the CLI actually exits.
    cli: Option<super::cli_spawn::SuspendedCli>,
    bridge: Option<Arc<super::cli_bridge::CliBridge>>,
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
    /// Establish durable writer custody for the real suspended service-created
    /// CLI. This method does not resume it or grant it an IPC role.
    #[allow(unsafe_code)]
    pub(super) fn admit_spawned(
        &self,
        child: super::cli_spawn::SuspendedCli,
        requester: &Peer,
        pipe: &Pipe,
    ) -> Result<Participant> {
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        child
            .recheck(&self.host, requester, pipe)
            .map_err(CoordinatorFailure::refused)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        recovery::discover(&self.host, &mut state, &self.stop)?;
        keeper::ensure_active(&self.host, &mut state.keeper, &self.stop)?;
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        let selected = state.writers.iter().position(|slot| unsafe {
            CompareObjectHandles(slot.original.raw(), child.original().raw()) != 0
        });
        let index = match selected {
            Some(_) => return Err(CoordinatorFailure::unknown(Error::IpcConflict)),
            None => {
                if state.writers.len() + state.recovered.len() >= MAX_HELD_WRITERS {
                    return Err(CoordinatorFailure::refused(Error::Busy));
                }
                let job = ReservedWriterJob::reserve_spawned(&self.host, &child, requester, pipe)
                    .map_err(CoordinatorFailure::refused)?;
                let index = state.writers.len();
                state.writers.push(WriterSlot {
                    original: Arc::clone(child.original()),
                    cli: Some(child),
                    bridge: Some(super::cli_bridge::CliBridge::new()),
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
        let child = slot
            .cli
            .take()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::StateCorrupt))?;
        let result = writers::enroll_spawned(
            &self.host, keeper, slot, &child, requester, pipe, &self.stop,
        );
        if result.is_ok() {
            slot.cli = Some(child);
        } else {
            // The request has no transferable retry ticket yet. End this
            // still-suspended child while retaining its journal and Job
            // state for normal reconciliation; never spawn a replacement.
            drop(child);
        }
        result
    }
    /// Revalidate the durable writer and live keeper for this exact owned
    /// child before allowing its descendants. This is not thread resumption.
    #[allow(unsafe_code)]
    pub(super) fn prepare_spawned_resume(
        &self,
        ticket: &super::cli_spawn::CliLaunchTicket,
        requester: &Peer,
        pipe: &Pipe,
    ) -> Result<()> {
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        let State {
            keeper, writers, ..
        } = &mut *state;
        let slot = writers
            .iter_mut()
            .find(|slot| unsafe {
                CompareObjectHandles(slot.original.raw(), ticket.original().raw()) != 0
            })
            .ok_or_else(|| CoordinatorFailure::unknown(Error::PeerRefused))?;
        let child = slot
            .cli
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        let WriterCustody::Assigned(job) = &slot.custody else {
            return Err(CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown));
        };
        let ack = slot
            .keeper_ack
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        let intent =
            Store::resume_writer_custody(&self.host, job).map_err(CoordinatorFailure::unknown)?;
        let mut live = keeper.peer().map_err(|failure| failure)?;
        let published = intent
            .published_admission(job, ack, &mut live)
            .map_err(CoordinatorFailure::unknown)?;
        child
            .prepare_guarded_resume(&self.host, requester, pipe, &published, job, ack, &mut live)
            .map_err(CoordinatorFailure::unknown)?;
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)
    }
    /// Resume only the exact staged original after durable custody and after
    /// its single-use frontend data endpoint has actually been acquired.
    #[allow(unsafe_code)]
    pub(super) fn resume_spawned(
        &self,
        ticket: &super::cli_spawn::CliLaunchTicket,
        requester: &Peer,
        pipe: &Pipe,
        frontend: &super::cli_bridge::FrontendEndpoint,
    ) -> Result<()> {
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        let State {
            keeper, writers, ..
        } = &mut *state;
        let slot = writers
            .iter_mut()
            .find(|slot| unsafe {
                CompareObjectHandles(slot.original.raw(), ticket.original().raw()) != 0
            })
            .ok_or_else(|| CoordinatorFailure::unknown(Error::PeerRefused))?;
        let WriterSlot {
            cli,
            custody,
            keeper_ack,
            bridge,
            ..
        } = slot;
        let bridge = bridge
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::StateCorrupt))?;
        if !frontend.belongs_to(bridge) {
            return Err(CoordinatorFailure::unknown(Error::PeerRefused));
        }
        let child = cli
            .as_mut()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        let WriterCustody::Assigned(job) = custody else {
            return Err(CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown));
        };
        let ack = keeper_ack
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        let intent =
            Store::resume_writer_custody(&self.host, job).map_err(CoordinatorFailure::unknown)?;
        let mut live = keeper.peer()?;
        let published = intent
            .published_admission(job, ack, &mut live)
            .map_err(CoordinatorFailure::unknown)?;
        child
            .resume(&self.host, requester, pipe, &published, job, ack, &mut live)
            .map_err(CoordinatorFailure::unknown)
    }
    /// A failed pre-resume setup kills its exact staged processes but retains
    /// journal/keeper custody for normal reconciliation; this is not proof of
    /// immediate process exit or permission to start a replacement.
    #[allow(unsafe_code)]
    pub(super) fn abort_spawned_setup(
        &self,
        ticket: &super::cli_spawn::CliLaunchTicket,
    ) -> Result<()> {
        cleanup_with_poisoned_state(&self.state, |state| {
            let slot = state
                .writers
                .iter_mut()
                .find(|slot| unsafe {
                    CompareObjectHandles(slot.original.raw(), ticket.original().raw()) != 0
                })
                .ok_or_else(|| CoordinatorFailure::unknown(Error::PeerRefused))?;
            drop(slot.cli.take());
            Ok(())
        })
    }
    /// Wait for the original kernel process signal, then retrieve its exact
    /// exit code. A numeric 259 is a legitimate result after that signal.
    /// Service cancellation remains interruptible; no total runtime limit is
    /// imposed and no caller-provided PID can select the process.
    #[allow(unsafe_code)]
    pub(super) fn wait_cli_exit(&self, ticket: &super::cli_spawn::CliLaunchTicket) -> Result<i32> {
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0,
            System::Threading::{GetExitCodeProcess, WaitForMultipleObjects},
        };
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        let original = {
            let state = self
                .state
                .lock()
                .map_err(|_| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
            let slot = state
                .writers
                .iter()
                .find(|slot| unsafe {
                    CompareObjectHandles(slot.original.raw(), ticket.original().raw()) != 0
                })
                .ok_or_else(|| CoordinatorFailure::unknown(Error::PeerRefused))?;
            Arc::clone(&slot.original)
        };
        let handles = [original.raw(), self.stop.raw()];
        let result = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, u32::MAX) };
        if result != WAIT_OBJECT_0 {
            return Err(CoordinatorFailure::unknown(Error::RestartCanceled));
        }
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        let mut code = 0u32;
        if unsafe { GetExitCodeProcess(original.raw(), &mut code) } == 0 {
            return Err(CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown));
        }
        Ok(code as i32)
    }
    /// A CLI-shaped pipe peer is a writer only if this service retains that
    /// exact native child and can revalidate its durable admission and keeper.
    #[allow(unsafe_code)]
    pub(super) fn admitted_cli_process(
        &self,
        process: &RetainedProcess,
    ) -> Result<Option<AdmittedCliPeer>> {
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        let State {
            keeper, writers, ..
        } = &mut *state;
        let Some(slot) = writers
            .iter_mut()
            .find(|slot| unsafe { CompareObjectHandles(slot.original.raw(), process.raw()) != 0 })
        else {
            return Ok(None);
        };
        let child = slot
            .cli
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::PeerRefused))?;
        child
            .recheck_worker(&self.host, process)
            .map_err(CoordinatorFailure::unknown)?;
        let WriterCustody::Assigned(job) = &slot.custody else {
            return Err(CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown));
        };
        let ack = slot
            .keeper_ack
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        let intent =
            Store::resume_writer_custody(&self.host, job).map_err(CoordinatorFailure::unknown)?;
        let mut live = keeper.peer()?;
        let published = intent
            .published_admission(job, ack, &mut live)
            .map_err(CoordinatorFailure::unknown)?;
        published
            .recheck(job, ack, &mut live)
            .map_err(CoordinatorFailure::unknown)?;
        child
            .recheck_worker(&self.host, process)
            .map_err(CoordinatorFailure::unknown)?;
        check_stop(&self.stop).map_err(CoordinatorFailure::unknown)?;
        Ok(Some(AdmittedCliPeer(Arc::clone(&slot.original))))
    }
    /// Single-use frontend endpoint, selected only by the internal ticket and
    /// revalidated against the same live requester pipe.
    #[allow(unsafe_code)]
    pub(super) fn take_cli_frontend(
        &self,
        ticket: &super::cli_spawn::CliLaunchTicket,
        requester: &Peer,
        pipe: &Pipe,
    ) -> Result<super::cli_bridge::FrontendEndpoint> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        let State {
            keeper, writers, ..
        } = &mut *state;
        let slot = writers
            .iter()
            .find(|slot| unsafe {
                CompareObjectHandles(slot.original.raw(), ticket.original().raw()) != 0
            })
            .ok_or_else(|| CoordinatorFailure::unknown(Error::PeerRefused))?;
        let child = slot
            .cli
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::PeerRefused))?;
        child
            .recheck(&self.host, requester, pipe)
            .map_err(CoordinatorFailure::unknown)?;
        let WriterCustody::Assigned(job) = &slot.custody else {
            return Err(CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown));
        };
        let ack = slot
            .keeper_ack
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        let intent =
            Store::resume_writer_custody(&self.host, job).map_err(CoordinatorFailure::unknown)?;
        let mut live = keeper.peer()?;
        let published = intent
            .published_admission(job, ack, &mut live)
            .map_err(CoordinatorFailure::unknown)?;
        published
            .recheck(job, ack, &mut live)
            .map_err(CoordinatorFailure::unknown)?;
        slot.bridge
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::StateCorrupt))?
            .frontend()
            .map_err(CoordinatorFailure::unknown)
    }
    /// A worker endpoint is available only after role promotion and renewed
    /// native custody/keeper checks for this same pipe process object.
    pub(super) fn take_cli_worker(
        &self,
        peer: &Peer,
        pipe: &Pipe,
    ) -> Result<super::cli_bridge::WorkerEndpoint> {
        peer.recheck(pipe).map_err(CoordinatorFailure::unknown)?;
        if peer.role != crate::handoff::Role::Cli {
            return Err(CoordinatorFailure::unknown(Error::PeerRefused));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        let State {
            keeper, writers, ..
        } = &mut *state;
        let slot = writers
            .iter()
            .find(|slot| same_original(&slot.original, peer))
            .ok_or_else(|| CoordinatorFailure::unknown(Error::PeerRefused))?;
        let child = slot
            .cli
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::PeerRefused))?;
        child
            .recheck_worker(&self.host, &peer.process)
            .map_err(CoordinatorFailure::unknown)?;
        let WriterCustody::Assigned(job) = &slot.custody else {
            return Err(CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown));
        };
        let ack = slot
            .keeper_ack
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::HandoffOutcomeUnknown))?;
        let intent =
            Store::resume_writer_custody(&self.host, job).map_err(CoordinatorFailure::unknown)?;
        let mut live = keeper.peer()?;
        let published = intent
            .published_admission(job, ack, &mut live)
            .map_err(CoordinatorFailure::unknown)?;
        published
            .recheck(job, ack, &mut live)
            .map_err(CoordinatorFailure::unknown)?;
        peer.recheck(pipe).map_err(CoordinatorFailure::unknown)?;
        slot.bridge
            .as_ref()
            .ok_or_else(|| CoordinatorFailure::unknown(Error::StateCorrupt))?
            .worker()
            .map_err(CoordinatorFailure::unknown)
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
        if !peer.role.is_writer() {
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
                    cli: None,
                    bridge: None,
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
    fn cli_cleanup_after_poison_drops_only_selected_stage_and_keeps_poison() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Stage(Arc<AtomicUsize>);
        impl Drop for Stage {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicUsize::new(0));
        let state = Arc::new(Mutex::new(vec![
            (1, Some(Stage(dropped.clone()))),
            (2, Some(Stage(dropped.clone()))),
        ]));
        let other = state.clone();
        assert!(
            std::thread::spawn(move || {
                let _guard = other.lock().unwrap();
                panic!("poison for cleanup test");
            })
            .join()
            .is_err()
        );
        cleanup_with_poisoned_state(&state, |slots| {
            drop(slots.iter_mut().find(|slot| slot.0 == 1).unwrap().1.take());
        });
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert!(state.is_poisoned());
        assert!(state.lock().is_err());
        cleanup_with_poisoned_state(&state, |slots| {
            assert!(slots[0].1.is_none());
            assert!(slots[1].1.is_some());
        });
    }

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
