//! Native-authenticated finite command adapter. No install/prepare/consume port.
use super::{
    binding,
    installed::StartedServiceContext,
    layout::ordinal_equal,
    peer::Peer,
    pipe::{Pipe, nonce},
    process::{RetainedProcess, prove_prior_installer_gone},
    registry::Store,
};
use crate::{
    error::{Error, Result},
    handoff::{Actions, ConnectionId, Role, Snapshot, State},
    ipc, json,
    registration::{HostRegistration, Phase},
};
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};
pub(super) struct ActionLease {
    old: Arc<RetainedProcess>,
    launcher: Option<Arc<RetainedProcess>>,
}
pub(super) struct Core {
    pub(super) host: StartedServiceContext,
    actions: Mutex<Actions<ActionLease>>,
    pub(super) time: Arc<super::time_service::RefreshControl>,
}
impl Core {
    pub(super) fn new(host: StartedServiceContext) -> Self {
        Self {
            host,
            actions: Mutex::new(Actions::new()),
            time: Arc::new(super::time_service::RefreshControl::new()),
        }
    }
    fn actions(&self) -> Result<MutexGuard<'_, Actions<ActionLease>>> {
        self.actions.lock().map_err(|_| Error::Unavailable)
    }
    fn time_status(&self) -> Result<crate::TrustedTimeStatus> {
        // The native diagnostic always runs, including after worker failure;
        // cached activity/success never substitutes for actual protected state.
        let native = Store::new(&self.host.context.seed).diagnostic_time(&self.host);
        self.time.diagnostic(native)
    }
    pub(super) fn record(&self) -> Result<HostRegistration> {
        let record = Store::new(&self.host.context.seed)
            .read(&self.host.context.seed)?
            .ok_or(Error::StateCorrupt)?;
        self.host.authenticate_running(&record)?;
        Ok(record)
    }
    pub(super) fn connect(
        self: &Arc<Self>,
        peer: Peer,
        session: String,
        server_nonce: String,
        bootstrap: bool,
        operation: Option<String>,
    ) -> Result<Connection> {
        let id = self
            .actions()?
            .connect(peer.key(), peer.role, server_nonce)?;
        Ok(Connection {
            core: self.clone(),
            peer,
            id,
            session,
            bootstrap,
            operation,
        })
    }
    fn image_binding(&self, record: &HostRegistration) -> Result<String> {
        binding::image(
            &record.public(),
            &self.host.context.app,
            &self.host.context.process.selector,
            &self
                .host
                .startup
                .startup_attestation
                .as_ref()
                .ok_or(Error::StateCorrupt)?
                .instance_nonce,
        )
    }
}
pub(super) struct Connection {
    pub(super) core: Arc<Core>,
    pub(super) peer: Peer,
    id: ConnectionId,
    pub(super) session: String,
    bootstrap: bool,
    operation: Option<String>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Ok(mut actions) = self.core.actions() {
            let _ = actions.disconnect(self.id);
        }
    }
}
fn state_reply(id: &str, snapshot: Snapshot, epoch: u64) -> ipc::StateReply {
    ipc::StateReply {
        restart_id: id.into(),
        state: snapshot.state.name().into(),
        generation: snapshot.generation,
        host_epoch: epoch,
    }
}
impl Connection {
    pub(super) fn authorize(&self, pipe: &Pipe) -> Result<HostRegistration> {
        self.peer.recheck(pipe)?;
        let record = self.core.record()?;
        if self.bootstrap {
            if self.peer.role != Role::SourceInstaller
                || record.phase != Phase::ServiceConfigured
                || self.operation.as_deref()
                    != Some(record.installer_identity.operation_id.as_str())
            {
                return Err(Error::SetupIncomplete);
            }
        } else if record.phase != Phase::Ready {
            return Err(Error::SetupIncomplete);
        }
        Ok(record)
    }
    fn reply<T: serde::Serialize>(
        &self,
        request: &ipc::Request,
        body: T,
    ) -> Result<(Vec<u8>, bool)> {
        Ok((
            ipc::success(&self.session, request.sequence, &request.kind, body)?,
            false,
        ))
    }
    pub(super) fn dispatch(
        &self,
        request: &ipc::Request,
        record: &HostRegistration,
    ) -> Result<(Vec<u8>, bool)> {
        let epoch = record.host_epoch;
        self.core.actions()?.reap_expired(Instant::now())?;
        if self.bootstrap {
            if request.kind != "bootstrap-probe" {
                return Err(Error::OperationUnsupported);
            }
            let body: ipc::ProbeBody = request.body()?;
            let startup = record
                .startup_attestation
                .as_ref()
                .ok_or(Error::SetupIncomplete)?;
            if body.operation_id != record.installer_identity.operation_id
                || body.expected_revision != record.revision
                || !json::digest(&body.challenge)
            {
                return Err(Error::ScopeConflict);
            }
            return self.reply(
                request,
                ipc::ProbeReply {
                    operation_id: body.operation_id,
                    installation_scope_id: record.installation_scope_id.clone(),
                    phase: record.phase,
                    registration_revision: record.revision,
                    host_epoch: epoch,
                    instance_nonce: startup.instance_nonce.clone(),
                    process_id: startup.process_id,
                    creation_file_time: startup.creation_file_time,
                    helper_image_sha256: startup.helper_image_sha256.clone(),
                    manifest_sha256: startup.manifest_sha256.clone(),
                    challenge: body.challenge,
                    probe_state: "checked".into(),
                },
            );
        }
        if matches!(
            request.kind.as_str(),
            "time-status" | "request-time-refresh"
        ) {
            require_time_app(self.peer.role)?;
            let _: ipc::Empty = request.body()?;
            if request.kind == "time-status" {
                return self.reply(request, self.core.time_status()?);
            }
            self.core.time.request()?;
            return self.reply(request, crate::TrustedTimeRefreshReply::accepted());
        }
        if request.kind == "status" {
            let _: ipc::Empty = request.body()?;
            return self.reply(
                request,
                ipc::StatusReply {
                    phase: record.phase,
                    helper_build_id: record.helper.build_id.clone(),
                    launcher_build_id: record.launcher.build_id.clone(),
                    app_build_id: record.app.build_id.clone(),
                    host_epoch: epoch,
                    registration_revision: record.revision,
                    capabilities: ipc::Capabilities::ready(self.peer.role != Role::SourceInstaller),
                },
            );
        }
        if self.peer.role == Role::SourceInstaller {
            return Err(Error::OperationUnsupported);
        }
        match request.kind.as_str() {
            "begin-restart" => {
                let body: ipc::BeginBody = request.body()?;
                let digest = request.body_digest()?;
                if let Some(original) = self.core.actions()?.begin_restart_replay(
                    self.id,
                    &body.client_intent_id,
                    &digest,
                )? {
                    return self.reply(
                        request,
                        ipc::BeginReply {
                            restart_id: original.id,
                            generation: original.generation,
                            host_epoch: epoch,
                        },
                    );
                }
                let reservation = self.core.actions()?.reserve_restart(
                    self.id,
                    &body.client_intent_id,
                    &digest,
                    nonce()?,
                    nonce()?,
                )?;
                let prepared = (|| {
                    let old = self.peer.process.clone();
                    if old.exited()? {
                        return Err(Error::PeerRefused);
                    }
                    Ok((
                        ActionLease {
                            old,
                            launcher: None,
                        },
                        self.core.image_binding(record)?,
                    ))
                })();
                let (lease, binding) = match prepared {
                    Ok(value) => value,
                    Err(error) => {
                        self.core.actions()?.fail_reservation(reservation, error);
                        return Err(error);
                    }
                };
                let started = self
                    .core
                    .actions()?
                    .finish_restart(reservation, lease, binding)?;
                self.reply(
                    request,
                    ipc::BeginReply {
                        restart_id: started.id,
                        generation: started.generation,
                        host_epoch: epoch,
                    },
                )
            }
            "begin-launch" => {
                let body: ipc::BeginBody = request.body()?;
                let reservation = self.core.actions()?.reserve_launch(
                    self.id,
                    &body.client_intent_id,
                    &request.body_digest()?,
                    nonce()?,
                )?;
                let prepared = (|| {
                    let old = self.peer.process.clone();
                    if old.exited()? {
                        return Err(Error::PeerRefused);
                    }
                    Ok((
                        ActionLease {
                            old,
                            launcher: None,
                        },
                        self.core.image_binding(record)?,
                    ))
                })();
                let (lease, binding) = match prepared {
                    Ok(value) => value,
                    Err(error) => {
                        self.core.actions()?.fail_reservation(reservation, error);
                        return Err(error);
                    }
                };
                let release = self
                    .core
                    .actions()?
                    .finish_launch(reservation, lease, binding)?;
                let (bytes, _) = self.reply(
                    request,
                    ipc::LaunchReply {
                        handoff_id: release.handoff_id,
                        image_binding_sha256: release.image_binding_sha256,
                        host_epoch: epoch,
                    },
                )?;
                Ok((bytes, true))
            }
            "join-restart" => {
                let body: ipc::JoinBody = request.body()?;
                let mut actions = self.core.actions()?;
                let joined = actions.join(
                    self.id,
                    &body.restart_id,
                    &body.client_intent_id,
                    &request.body_digest()?,
                    nonce()?,
                )?;
                actions.with_launcher_leases_mut(self.id, &body.restart_id, |lease| {
                    if lease
                        .launcher
                        .as_ref()
                        .is_some_and(|old| old.selector != self.peer.process.selector)
                    {
                        return Err(Error::PeerRefused);
                    }
                    lease.launcher = Some(self.peer.process.clone());
                    Ok(())
                })?;
                drop(actions);
                self.reply(
                    request,
                    ipc::JoinReply {
                        wait_association_id: joined.wait_id,
                        old_process_id: joined.original_process.process_id,
                        old_process_creation_time: joined.original_process.creation_file_time,
                        old_image_binding_sha256: joined.image_binding_sha256,
                        generation: joined.generation,
                        host_epoch: epoch,
                    },
                )
            }
            "restart-ready" => {
                let body: ipc::ReadyBody = request.body()?;
                let state = self.core.actions()?.ready(
                    self.id,
                    &body.restart_id,
                    &body.wait_association_id,
                    body.expected_generation,
                )?;
                self.reply(request, state_reply(&body.restart_id, state, epoch))
            }
            "restart-arm" => {
                let body: ipc::ArmBody = request.body()?;
                let mut actions = self.core.actions()?;
                require_live_launcher(
                    &mut actions,
                    self.id,
                    &body.restart_id,
                    &self.peer.process,
                    &self.core.host.context.layout.launcher_path(),
                )?;
                let state = actions.arm(self.id, &body.restart_id, body.expected_generation)?;
                drop(actions);
                self.reply(request, state_reply(&body.restart_id, state, epoch))
            }
            "restart-status" => {
                let body: ipc::RestartBody = request.body()?;
                let (state, selector, held) = {
                    let mut actions = self.core.actions()?;
                    let state = actions.status(self.id, &body.restart_id, Instant::now())?;
                    let selector = actions.original_process(self.id, &body.restart_id)?;
                    let held = actions
                        .with_leases(self.id, &body.restart_id, |lease| lease.old.exited())
                        .ok();
                    (state, selector, held)
                };
                let exited = match held {
                    Some(value) => value,
                    None => prove_prior_installer_gone(&selector)?,
                };
                self.reply(
                    request,
                    ipc::RestartStatusReply {
                        restart_id: body.restart_id,
                        state: state.state.name().into(),
                        generation: state.generation,
                        host_epoch: epoch,
                        old_process_exited: exited,
                        exit_commit_id: state.exit_commit_id,
                        remaining_exit_grace_ms: state.remaining_exit_grace_ms,
                    },
                )
            }
            "restart-commit-exit" => {
                let body: ipc::CommitBody = request.body()?;
                let mut actions = self.core.actions()?;
                require_live_launcher(
                    &mut actions,
                    self.id,
                    &body.restart_id,
                    &self.peer.process,
                    &self.core.host.context.layout.launcher_path(),
                )?;
                let state = actions.commit_exit(
                    self.id,
                    &body.restart_id,
                    body.expected_generation,
                    &body.commit_id,
                    &request.body_digest()?,
                    Instant::now(),
                )?;
                drop(actions);
                self.reply(
                    request,
                    ipc::CommitReply {
                        restart_id: body.restart_id,
                        state: state.state.name().into(),
                        generation: state.generation,
                        host_epoch: epoch,
                        exit_commit_id: state.exit_commit_id.ok_or(Error::RestartConflict)?,
                        remaining_exit_grace_ms: state
                            .remaining_exit_grace_ms
                            .ok_or(Error::RestartConflict)?,
                    },
                )
            }
            "restart-cancel" => {
                let body: ipc::RestartBody = request.body()?;
                let state =
                    self.core
                        .actions()?
                        .cancel(self.id, &body.restart_id, Instant::now())?;
                let outcome = if matches!(
                    state.state,
                    State::LaunchReleased | State::Observed | State::OutcomeUnknown
                ) {
                    "unknown"
                } else {
                    "none"
                };
                self.reply(
                    request,
                    ipc::CancelReply {
                        restart_id: body.restart_id,
                        state: state.state.name().into(),
                        generation: state.generation,
                        host_epoch: epoch,
                        outcome: outcome.into(),
                    },
                )
            }
            "restart-take-launch" => {
                let body: ipc::TakeBody = request.body()?;
                let mut actions = self.core.actions()?;
                let signaled =
                    actions.with_leases(self.id, &body.restart_id, |lease| lease.old.exited())?;
                let release = actions.take(
                    self.id,
                    &body.restart_id,
                    body.expected_generation,
                    &body.exit_commit_id,
                    signaled,
                    Instant::now(),
                )?;
                drop(actions);
                let (bytes, _) = self.reply(
                    request,
                    ipc::TakeReply {
                        restart_id: body.restart_id,
                        state: "launch_released".into(),
                        generation: release.generation,
                        host_epoch: epoch,
                        handoff_id: release.handoff_id,
                        image_binding_sha256: release.image_binding_sha256,
                    },
                )?;
                Ok((bytes, true))
            }
            "launch-observed" => {
                let body: ipc::ObservedBody = request.body()?;
                let matched = self.observe(&body)?;
                self.core
                    .actions()?
                    .observed(self.id, &body.handoff_id, matched)?;
                self.reply(
                    request,
                    ipc::ObservedReply {
                        observation: if matched { "matched" } else { "unknown" }.into(),
                    },
                )
            }
            _ => Err(Error::OperationUnsupported),
        }
    }
    fn observe(&self, body: &ipc::ObservedBody) -> Result<bool> {
        if self.peer.role != Role::Launcher
            || !json::digest(&body.handoff_id)
            || body.child_process_id == 0
            || body.child_creation_time.ticks() == 0
        {
            return Err(Error::IpcInvalid);
        }
        let Ok(child) = RetainedProcess::open(body.child_process_id, None) else {
            return Ok(false);
        };
        if child.selector.creation_file_time != body.child_creation_time
            || child.token.user != self.peer.process.token.user
            || child.selector.creation_file_time.ticks()
                < self.peer.process.selector.creation_file_time.ticks()
        {
            return Ok(false);
        }
        if !ordinal_equal(
            child.image.to_str().ok_or(Error::ImageRefused)?,
            self.core
                .host
                .context
                .layout
                .app_path()
                .to_str()
                .ok_or(Error::ImageRefused)?,
        )? {
            return Ok(false);
        }
        let Ok(parent) = child.parent() else {
            return Ok(false);
        };
        Ok(parent.selector == self.peer.process.selector && !child.exited()?)
    }
    /// Sequence replay never re-acquires native resources or recreates a cell.
    /// Time-sensitive ACKs are checked against the SAME current association.
    pub(super) fn replay(
        &self,
        request: &ipc::Request,
        record: &HostRegistration,
        cached: &[u8],
    ) -> Result<Vec<u8>> {
        if !ipc::cached_success(cached)? {
            return Ok(cached.to_vec());
        }
        match request.kind.as_str() {
            "time-status" => {
                require_time_app(self.peer.role)?;
                let _: ipc::Empty = request.body()?;
                Ok(self.reply(request, self.core.time_status()?)?.0)
            }
            "request-time-refresh" => {
                require_time_app(self.peer.role)?;
                let _: ipc::Empty = request.body()?;
                // The same original response only; never re-enter the latch.
                Ok(cached.to_vec())
            }
            "begin-restart" => {
                let (_, original) = ipc::response::<ipc::BeginReply>(
                    cached,
                    Some(&self.session),
                    request.sequence,
                    &request.kind,
                )?;
                let body: ipc::BeginBody = request.body()?;
                let current = self
                    .core
                    .actions()?
                    .begin_restart_replay(self.id, &body.client_intent_id, &request.body_digest()?)?
                    .ok_or(Error::HandoffOutcomeUnknown)?;
                if current.id != original.restart_id
                    || current.generation < original.generation
                    || record.host_epoch != original.host_epoch
                {
                    return Err(Error::RestartCanceled);
                }
                Ok(cached.to_vec())
            }
            "join-restart" => {
                let (_, original) = ipc::response::<ipc::JoinReply>(
                    cached,
                    Some(&self.session),
                    request.sequence,
                    &request.kind,
                )?;
                let body: ipc::JoinBody = request.body()?;
                let current =
                    self.core
                        .actions()?
                        .status(self.id, &body.restart_id, Instant::now())?;
                if !matches!(
                    current.state,
                    State::Joined | State::Ready | State::Armed | State::ExitCommitted
                ) || current.generation < original.generation
                    || record.host_epoch != original.host_epoch
                {
                    return Err(Error::RestartCanceled);
                }
                Ok(cached.to_vec())
            }
            "restart-commit-exit" | "restart-status" | "restart-cancel" => {
                self.dispatch(request, record).map(|(bytes, _)| bytes)
            }
            "restart-ready" | "restart-arm" => {
                let (_, original) = ipc::response::<ipc::StateReply>(
                    cached,
                    Some(&self.session),
                    request.sequence,
                    &request.kind,
                )?;
                let mut actions = self.core.actions()?;
                require_live_launcher(
                    &mut actions,
                    self.id,
                    &original.restart_id,
                    &self.peer.process,
                    &self.core.host.context.layout.launcher_path(),
                )?;
                let current = actions.status(self.id, &original.restart_id, Instant::now())?;
                if current.state.name() != original.state
                    || current.generation != original.generation
                    || record.host_epoch != original.host_epoch
                {
                    return Err(Error::RestartCanceled);
                }
                Ok(cached.to_vec())
            }
            _ => Ok(cached.to_vec()),
        }
    }
}

/// The OS wait/token observation and arm/commit occur under the same finite
/// cell lock. The launcher cannot be replaced by a wire PID or an asynchronously
/// delayed disconnect callback. Wait0/token queries do not wait for user IO.
fn require_live_launcher(
    actions: &mut Actions<ActionLease>,
    connection: ConnectionId,
    id: &str,
    app: &RetainedProcess,
    path: &std::path::Path,
) -> Result<()> {
    let result = actions.with_leases(connection, id, |lease| {
        let launcher = lease.launcher.as_ref().ok_or(Error::RestartNotReady)?;
        if launcher.exited()? {
            return Err(Error::RestartCanceled);
        }
        let token = launcher.fresh_token(None)?;
        if token.user != app.token.user
            || token.integrity != app.token.integrity
            || token.elevated != app.token.elevated
            || token.app_container
            || token.network
            || !ordinal_equal(
                launcher.image.to_str().ok_or(Error::ImageRefused)?,
                path.to_str().ok_or(Error::ImageRefused)?,
            )?
        {
            return Err(Error::PeerRefused);
        }
        Ok(())
    });
    if let Err(error) = result {
        let _ = actions.cancel(connection, id, Instant::now());
        return Err(error);
    }
    Ok(())
}

fn require_time_app(role: Role) -> Result<()> {
    if role != Role::App {
        return Err(Error::OperationUnsupported);
    }
    Ok(())
}
#[cfg(test)]
mod time_role_tests {
    use super::*;
    #[test]
    fn time_route_rule_rejects_launcher_and_source_installer() {
        // Authorization-rule control only. No process/pipe identity is minted.
        require_time_app(Role::App).unwrap();
        assert_eq!(
            require_time_app(Role::Launcher),
            Err(Error::OperationUnsupported)
        );
        assert_eq!(
            require_time_app(Role::SourceInstaller),
            Err(Error::OperationUnsupported)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::PeerKey;
    use std::process::{Child, Command, Stdio};
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn hex(value: u32) -> String {
        format!("{value:064x}")
    }
    fn peer(process: &RetainedProcess) -> PeerKey {
        PeerKey {
            process: process.selector.clone(),
            user_sid: process.token.user.sid.clone(),
            logon: process.token.user.logon,
            session_id: process.token.user.session_id,
        }
    }
    #[test]
    fn actual_launcher_exit_refuses_arm_and_commit_before_logical_disconnect() {
        // Ordinary owned native processes exercise the real adapter wait/token
        // gate. No SourceInstallerContext/SCM/protected-host fact is fabricated.
        let app = Arc::new(RetainedProcess::current().unwrap());
        for commit in [false, true] {
            let mut child = OwnedChild(
                Command::new(std::env::var_os("COMSPEC").unwrap())
                    .args(["/Q", "/C", "pause >NUL"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            let launcher = Arc::new(RetainedProcess::open(child.0.id(), None).unwrap());
            let path = launcher.image.clone();
            let mut actions = Actions::new();
            let a = actions.connect(peer(&app), Role::App, hex(1)).unwrap();
            let l = actions
                .connect(peer(&launcher), Role::Launcher, hex(2))
                .unwrap();
            let reserved = actions
                .reserve_restart(
                    a,
                    &format!("{}-{}", hex(1), "1".repeat(32)),
                    &hex(3),
                    hex(4),
                    hex(5),
                )
                .unwrap();
            let begin = actions
                .finish_restart(
                    reserved,
                    ActionLease {
                        old: app.clone(),
                        launcher: None,
                    },
                    hex(6),
                )
                .unwrap();
            let joined = actions
                .join(
                    l,
                    &begin.id,
                    &format!("{}-{}", hex(2), "2".repeat(32)),
                    &hex(7),
                    hex(8),
                )
                .unwrap();
            actions
                .with_launcher_leases_mut(l, &begin.id, |lease| {
                    lease.launcher = Some(launcher.clone());
                    Ok(())
                })
                .unwrap();
            let ready = actions
                .ready(l, &begin.id, &joined.wait_id, joined.generation)
                .unwrap();
            require_live_launcher(&mut actions, a, &begin.id, &app, &path).unwrap();
            if commit {
                actions.arm(a, &begin.id, ready.generation).unwrap();
            }
            child.0.kill().unwrap();
            child.0.wait().unwrap();
            assert!(launcher.exited().unwrap());
            let before = actions.status(a, &begin.id, Instant::now()).unwrap();
            assert!(matches!(before.state, State::Ready | State::Armed));
            // Deliberately do NOT deliver launcher disconnect: its native exit
            // must be sufficient at the original decision point.
            assert_eq!(
                require_live_launcher(&mut actions, a, &begin.id, &app, &path),
                Err(Error::RestartCanceled)
            );
            assert!(matches!(
                actions.status(a, &begin.id, Instant::now()).unwrap().state,
                State::Canceled
            ));
            assert!(actions.arm(a, &begin.id, before.generation).is_err());
            assert!(
                actions
                    .commit_exit(
                        a,
                        &begin.id,
                        before.generation,
                        &hex(9),
                        &hex(10),
                        Instant::now()
                    )
                    .is_err()
            );
        }
    }
}
