//! App-side original-session readiness/keepalive/definite-exit coordination.
use super::{
    client::Client,
    launch::{self, Target},
    pipe::nonce,
    process::RetainedProcess,
};
use crate::{
    error::{Error, Result},
    handoff::Role,
    ipc, json,
    restart::{CommittedRestart, Control, PreparedRestart},
};
use std::{
    sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError},
    time::{Duration, Instant},
};
struct Association {
    client: Client,
    id: String,
    armed_generation: u64,
    committed: bool,
    _launcher: RetainedProcess,
}
impl Drop for Association {
    fn drop(&mut self) {
        if !self.committed {
            let _: Result<ipc::CancelReply> = self.client.request(
                "restart-cancel",
                &ipc::RestartBody {
                    restart_id: self.id.clone(),
                },
            );
        }
    }
}
fn status(association: &mut Association) -> Result<ipc::RestartStatusReply> {
    let reply: ipc::RestartStatusReply = association.client.request(
        "restart-status",
        &ipc::RestartBody {
            restart_id: association.id.clone(),
        },
    )?;
    if reply.restart_id != association.id
        || reply.host_epoch != association.client.record.host_epoch
        || reply.generation == 0
        || reply.generation > json::SAFE_MAX
        || !matches!(
            reply.state.as_str(),
            "created"
                | "joined"
                | "ready"
                | "armed"
                | "exit_committed"
                | "launch_released"
                | "observed"
                | "outcome_unknown"
                | "canceled"
                | "failed"
        )
        || reply
            .exit_commit_id
            .as_ref()
            .is_some_and(|id| !json::digest(id))
        || reply.remaining_exit_grace_ms.is_some_and(|ms| ms > 30_000)
    {
        return Err(Error::IpcInvalid);
    }
    Ok(reply)
}
const READY_WAIT: Duration = Duration::from_secs(30);
fn require_ready_wait(deadline: Instant, now: Instant) -> Result<()> {
    if now >= deadline {
        Err(Error::RestartCanceled)
    } else {
        Ok(())
    }
}
fn exit_deadline(started: Instant, grace_ms: u64) -> Result<Instant> {
    started
        .checked_add(Duration::from_millis(grace_ms))
        .ok_or(Error::RestartCanceled)
}
fn prepare(controls: Receiver<Control>, ready: SyncSender<Result<()>>) -> Result<CommittedRestart> {
    let mut client = Client::connect(Role::App)?;
    let intent = client.intent()?;
    let begin: ipc::BeginReply = client.request(
        "begin-restart",
        &ipc::BeginBody {
            client_intent_id: intent,
        },
    )?;
    if !json::digest(&begin.restart_id)
        || begin.generation == 0
        || begin.generation > json::SAFE_MAX
        || begin.host_epoch != client.record.host_epoch
    {
        return Err(Error::IpcInvalid);
    }
    let launcher = launch::spawn(&client, Target::RestartLauncher(&begin.restart_id))?;
    let mut association = Association {
        client,
        id: begin.restart_id,
        armed_generation: 0,
        committed: false,
        _launcher: launcher,
    };
    let ready_deadline = Instant::now()
        .checked_add(READY_WAIT)
        .ok_or(Error::RestartCanceled)?;
    loop {
        require_ready_wait(ready_deadline, Instant::now())?;
        match controls.try_recv() {
            Err(TryRecvError::Empty) => {}
            _ => return Err(Error::RestartCanceled),
        }
        if association._launcher.exited()? {
            return Err(Error::RestartCanceled);
        }
        let current = status(&mut association)?;
        require_ready_wait(ready_deadline, Instant::now())?;
        match current.state.as_str() {
            "created" | "joined" => std::thread::sleep(Duration::from_millis(100)),
            "ready" => {
                let armed: ipc::StateReply = association.client.request(
                    "restart-arm",
                    &ipc::ArmBody {
                        restart_id: association.id.clone(),
                        expected_generation: current.generation,
                    },
                )?;
                require_ready_wait(ready_deadline, Instant::now())?;
                if armed.restart_id != association.id
                    || armed.state != "armed"
                    || armed.generation != current.generation + 1
                    || armed.host_epoch != association.client.record.host_epoch
                {
                    return Err(Error::IpcInvalid);
                }
                association.armed_generation = armed.generation;
                break;
            }
            _ => return Err(Error::RestartCanceled),
        }
    }
    ready.send(Ok(())).map_err(|_| Error::RestartCanceled)?;
    loop {
        match controls.recv_timeout(Duration::from_secs(2)) {
            Ok(Control::Cancel) | Err(RecvTimeoutError::Disconnected) => {
                return Err(Error::RestartCanceled);
            }
            Err(RecvTimeoutError::Timeout) => {
                if association._launcher.exited()? {
                    return Err(Error::RestartCanceled);
                }
                let current = status(&mut association)?;
                if current.state != "armed" || current.generation != association.armed_generation {
                    return Err(Error::RestartCanceled);
                }
            }
            Ok(Control::Commit) => {
                let commit = nonce()?;
                let started = Instant::now();
                let reply: ipc::CommitReply = association
                    .client
                    .request(
                        "restart-commit-exit",
                        &ipc::CommitBody {
                            restart_id: association.id.clone(),
                            expected_generation: association.armed_generation,
                            commit_id: commit.clone(),
                        },
                    )
                    .map_err(|error| match error {
                        Error::RestartCanceled
                        | Error::RestartNotReady
                        | Error::RestartConflict
                        | Error::PeerRefused => error,
                        _ => Error::HandoffOutcomeUnknown,
                    })?;
                if reply.restart_id != association.id
                    || reply.state != "exit_committed"
                    || reply.generation != association.armed_generation + 1
                    || reply.host_epoch != association.client.record.host_epoch
                    || reply.exit_commit_id != commit
                    || reply.remaining_exit_grace_ms == 0
                    || reply.remaining_exit_grace_ms > 30_000
                {
                    return Err(Error::HandoffOutcomeUnknown);
                }
                // Request-start elapsed is conservative: it includes time
                // before the server committed, so never extends its original
                // monotonic grace. No trusted clock/installation flag follows.
                let deadline = exit_deadline(started, reply.remaining_exit_grace_ms)?;
                let marker = CommittedRestart::checked_until(deadline)?;
                association.committed = true;
                return Ok(marker);
            }
        }
    }
}
pub(super) fn begin() -> Result<PreparedRestart> {
    PreparedRestart::start(prepare)
}

#[cfg(test)]
mod timing_tests {
    use super::*;
    #[test]
    fn original_start_fences_grace_without_resampling_now() {
        let started = Instant::now();
        let deadline = exit_deadline(started, 30_000).unwrap();
        assert_eq!(deadline.duration_since(started), Duration::from_secs(30));
        let delayed = started + Duration::from_secs(31);
        assert!(deadline < delayed);
        assert_eq!(
            exit_deadline(started, 1).unwrap(),
            started + Duration::from_millis(1)
        );
    }
    #[test]
    fn readiness_at_or_after_original_deadline_cannot_arm() {
        let start = Instant::now();
        let deadline = start + READY_WAIT;
        assert!(require_ready_wait(deadline, deadline - Duration::from_nanos(1)).is_ok());
        assert_eq!(
            require_ready_wait(deadline, deadline),
            Err(Error::RestartCanceled)
        );
        assert_eq!(
            require_ready_wait(deadline, deadline + Duration::from_secs(60)),
            Err(Error::RestartCanceled)
        );
    }
}
