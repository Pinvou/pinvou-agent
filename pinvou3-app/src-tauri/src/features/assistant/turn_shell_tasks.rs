//! App-owned lifecycle for detached shell jobs.
//!
//! CodeWhale deliberately allows detached jobs and background sub-agents to
//! outlive the root tool call. Pinvou's main stop action is stronger: it stops
//! shell jobs owned by the interrupted root turn without changing CodeWhale's
//! generic detached-task contract.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::{anyhow, Result};
use deepseek_tui::tools::shell::{
    new_shared_shell_manager, SharedShellManager, ShellResult, ShellStatus,
};
use parking_lot::Mutex;
use tokio::sync::Notify;

const CLEANUP_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CLEANUP_RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(50), Duration::from_millis(200)];
const MAX_KILL_ATTEMPTS: u8 = 3;
const MAX_FAILED_SCOPE_TOMBSTONES: usize = 16;

pub(crate) type TurnShellScopeId = u64;
type ScopeId = TurnShellScopeId;

#[derive(Clone, Default)]
pub(crate) struct SessionShellManagers {
    managers: Arc<Mutex<HashMap<String, SharedShellManager>>>,
}

impl SessionShellManagers {
    pub(crate) fn for_session(&self, session_id: &str, workspace: PathBuf) -> SharedShellManager {
        let mut managers = self.managers.lock();
        managers
            .entry(session_id.to_string())
            .or_insert_with(|| new_shared_shell_manager(workspace))
            .clone()
    }

    pub(crate) fn get(&self, session_id: &str) -> Option<SharedShellManager> {
        self.managers.lock().get(session_id).cloned()
    }

    pub(crate) fn remove(&self, session_id: &str) {
        self.managers.lock().remove(session_id);
    }
}

#[derive(Debug, Clone)]
struct PendingKill {
    attempts: u8,
    last_error: String,
}

#[derive(Debug)]
struct TurnShellScope {
    id: ScopeId,
    turn_id: Option<String>,
    baseline_task_ids: HashSet<String>,
    registered_task_ids: HashSet<String>,
    live_agent_ids: HashSet<String>,
    pending_kills: HashMap<String, PendingKill>,
    cancel_requested: bool,
    root_terminal: bool,
    /// The baseline fallback is valid only while this root turn still owns the
    /// producer window. After its final terminal sweep, exact task/agent
    /// provenance remains valid but a later turn may create new ownerless jobs.
    allow_unowned_fallback: bool,
    cleanup_settled: bool,
    cleanup_error: Option<String>,
    cleanup_error_attempts: u8,
}

#[derive(Debug, Default)]
struct RegistryState {
    next_scope_id: ScopeId,
    active_scope_id: Option<ScopeId>,
    scopes: HashMap<ScopeId, TurnShellScope>,
    turn_scopes: HashMap<String, ScopeId>,
    agent_scopes: HashMap<String, ScopeId>,
    task_scopes: HashMap<String, ScopeId>,
    failed_scope_order: VecDeque<ScopeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellCleanupFailure {
    pub task_id: String,
    pub error: String,
}

#[derive(Debug, Default)]
pub(crate) struct ShellCleanupReport {
    pub killed: Vec<ShellResult>,
    pub failed: Vec<ShellCleanupFailure>,
}

impl ShellCleanupReport {
    pub(crate) fn failure_summary(&self) -> Option<String> {
        (!self.failed.is_empty()).then(|| {
            let details = self
                .failed
                .iter()
                .map(|failure| format!("{}: {}", failure.task_id, failure.error))
                .collect::<Vec<_>>()
                .join("; ");
            format!(
                "Failed to stop {} background shell task(s): {details}",
                self.failed.len()
            )
        })
    }

    fn merge(&mut self, mut other: Self) {
        self.killed.append(&mut other.killed);
        self.failed = other.failed;
    }
}

struct RegistryInner {
    shell_manager: SharedShellManager,
    state: Mutex<RegistryState>,
    cleanup_notify: Notify,
    worker_started: AtomicBool,
}

/// Session-scoped supervisor for root-turn shell ownership.
///
/// A provisional scope is opened before an Engine operation is submitted and
/// later bound to the authoritative `TurnStarted.turn_id`. Exact task ids and
/// sub-agent owner ids are retained as provenance. The baseline is only a
/// fallback for ownerless root jobs whose result event has not arrived yet.
#[derive(Clone)]
pub(crate) struct TurnShellTaskRegistry {
    inner: Arc<RegistryInner>,
}

#[derive(Clone, Default)]
pub(crate) struct SessionTurnShellTasks {
    registries: Arc<Mutex<HashMap<String, TurnShellTaskRegistry>>>,
}

#[derive(Clone)]
pub(crate) struct ShellReclaim {
    registry: Option<TurnShellTaskRegistry>,
    scope_id: Option<ScopeId>,
    cleanup_failed: Arc<AtomicBool>,
}

pub(crate) struct TurnShellCancellation {
    session_id: String,
    registry: TurnShellTaskRegistry,
    scope_id: ScopeId,
}

impl SessionTurnShellTasks {
    pub(crate) fn for_session(
        &self,
        session_id: &str,
        shell_manager: SharedShellManager,
    ) -> TurnShellTaskRegistry {
        let mut registries = self.registries.lock();
        registries
            .entry(session_id.to_string())
            .or_insert_with(|| TurnShellTaskRegistry::new(shell_manager))
            .clone()
    }

    pub(crate) fn remove(&self, session_id: &str) {
        self.registries.lock().remove(session_id);
    }

    pub(crate) fn begin_reclaim(&self, session_id: &str) -> ShellReclaim {
        let registry = self.registries.lock().get(session_id).cloned();
        let scope_id = registry
            .as_ref()
            .and_then(TurnShellTaskRegistry::active_scope_id);
        if let (Some(registry), Some(scope_id)) = (registry.as_ref(), scope_id) {
            registry.request_cancel_scope(scope_id);
        }
        ShellReclaim {
            registry,
            scope_id,
            cleanup_failed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn request_cancel(&self, session_id: &str) -> Option<TurnShellCancellation> {
        let registry = self.registries.lock().get(session_id).cloned()?;
        let scope_id = registry.request_cancel_active()?;
        Some(TurnShellCancellation {
            session_id: session_id.to_string(),
            registry,
            scope_id,
        })
    }
}

impl ShellReclaim {
    pub(crate) async fn finalize(&self) {
        let (Some(registry), Some(scope_id)) = (self.registry.as_ref(), self.scope_id) else {
            return;
        };
        match registry.finalize_scope(scope_id, true).await {
            Ok(report) => {
                if let Some(error) = report.failure_summary() {
                    self.cleanup_failed.store(true, Ordering::Release);
                    log::error!(
                        "[engine_pool] shell cleanup remained incomplete before reclaim scope={scope_id}: {error}"
                    );
                }
            }
            Err(error) => {
                self.cleanup_failed.store(true, Ordering::Release);
                log::error!(
                    "[engine_pool] failed to finalize shell tasks before reclaim scope={scope_id}: {error:#}"
                );
            }
        }
    }

    pub(crate) fn cleanup_failed(&self) -> bool {
        self.cleanup_failed.load(Ordering::Acquire)
    }
}

impl TurnShellCancellation {
    pub(crate) async fn cleanup(self) {
        match self
            .registry
            .cleanup_scope_with_retries(self.scope_id)
            .await
        {
            Ok(report) if !report.killed.is_empty() => log::info!(
                "[pinvou3][chat] canceled {} shell task(s) for sid={} turn={}",
                report.killed.len(),
                self.session_id,
                self.scope_id
            ),
            Ok(report) => {
                if let Some(error) = report.failure_summary() {
                    log::error!(
                        "[pinvou3][chat] shell cleanup remains pending sid={} scope={}: {}",
                        self.session_id,
                        self.scope_id,
                        error
                    );
                }
            }
            Err(error) => log::error!(
                "[pinvou3][chat] failed to cancel shell tasks sid={} scope={}: {error:#}",
                self.session_id,
                self.scope_id
            ),
        }
    }
}

/// Cancellation-safe ownership of a provisional submission scope.
///
/// Dropping the send future before the Engine accepts the operation abandons
/// only an unbound scope. Once the operation is accepted, `commit` transfers
/// lifecycle ownership to the event forwarder.
pub(crate) struct PreparedTurnShellScope {
    registry: TurnShellTaskRegistry,
    scope_id: ScopeId,
    committed: bool,
}

impl PreparedTurnShellScope {
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }

    #[cfg(test)]
    fn scope_id(&self) -> ScopeId {
        self.scope_id
    }
}

impl Drop for PreparedTurnShellScope {
    fn drop(&mut self) {
        if !self.committed {
            self.registry.abandon_unbound_scope(self.scope_id);
        }
    }
}

impl TurnShellTaskRegistry {
    pub(crate) fn new(shell_manager: SharedShellManager) -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                shell_manager,
                state: Mutex::new(RegistryState::default()),
                cleanup_notify: Notify::new(),
                worker_started: AtomicBool::new(false),
            }),
        }
    }

    /// Opens the root-turn scope before the Engine can start a tool. This
    /// closes the submitted-but-not-yet-forwarded `TurnStarted` race.
    pub(crate) async fn prepare_turn(&self) -> Result<ScopeId> {
        self.ensure_cleanup_worker();
        let shell_manager = self.inner.shell_manager.clone();
        let baseline_task_ids = tauri::async_runtime::spawn_blocking(move || {
            let mut manager = shell_manager
                .lock()
                .map_err(|_| anyhow!("Shell manager lock poisoned"))?;
            Ok::<_, anyhow::Error>(manager.list_jobs().into_iter().map(|job| job.id).collect())
        })
        .await
        .map_err(|error| anyhow!("prepare turn shell scope join failed: {error}"))??;

        let mut state = self.inner.state.lock();
        if state.active_scope_id.is_some() {
            anyhow::bail!("a root-turn shell scope is already active");
        }
        state.next_scope_id = state.next_scope_id.wrapping_add(1).max(1);
        let scope_id = state.next_scope_id;
        state.scopes.insert(
            scope_id,
            TurnShellScope {
                id: scope_id,
                turn_id: None,
                baseline_task_ids,
                registered_task_ids: HashSet::new(),
                live_agent_ids: HashSet::new(),
                pending_kills: HashMap::new(),
                cancel_requested: false,
                root_terminal: false,
                allow_unowned_fallback: true,
                cleanup_settled: true,
                cleanup_error: None,
                cleanup_error_attempts: 0,
            },
        );
        state.active_scope_id = Some(scope_id);
        Ok(scope_id)
    }

    pub(crate) async fn prepare_submission(&self) -> Result<PreparedTurnShellScope> {
        let scope_id = self.prepare_turn().await?;
        Ok(PreparedTurnShellScope {
            registry: self.clone(),
            scope_id,
            committed: false,
        })
    }

    /// Binds the authoritative Engine turn id to the provisional submission
    /// scope. The fallback exists for engine-created turns which bypass the
    /// ordinary AppEngine submission methods.
    pub(crate) async fn bind_or_prepare_turn(&self, turn_id: &str) -> Result<ScopeId> {
        if let Some(scope_id) = self.bind_active_turn(turn_id)? {
            return Ok(scope_id);
        }
        let scope_id = self.prepare_turn().await?;
        self.bind_active_turn(turn_id)?
            .filter(|bound| *bound == scope_id)
            .ok_or_else(|| anyhow!("failed to bind prepared shell scope to turn {turn_id}"))
    }

    fn bind_active_turn(&self, turn_id: &str) -> Result<Option<ScopeId>> {
        let mut state = self.inner.state.lock();
        if let Some(scope_id) = state.turn_scopes.get(turn_id).copied() {
            return Ok(Some(scope_id));
        }
        let Some(scope_id) = state.active_scope_id else {
            return Ok(None);
        };
        let scope = state
            .scopes
            .get_mut(&scope_id)
            .ok_or_else(|| anyhow!("active shell scope {scope_id} is missing"))?;
        match scope.turn_id.as_deref() {
            Some(existing) if existing != turn_id => {
                anyhow::bail!("shell scope {scope_id} already belongs to turn {existing}")
            }
            Some(_) => {}
            None => scope.turn_id = Some(turn_id.to_string()),
        }
        state.turn_scopes.insert(turn_id.to_string(), scope_id);
        Ok(Some(scope_id))
    }

    /// A failed mailbox submission may abandon only an unbound provisional
    /// scope. If `TurnStarted` already won the race, the forwarder owns it.
    pub(crate) fn abandon_unbound_scope(&self, scope_id: ScopeId) {
        let mut state = self.inner.state.lock();
        let can_abandon = state
            .scopes
            .get(&scope_id)
            .is_some_and(|scope| scope.turn_id.is_none());
        if can_abandon {
            remove_scope_locked(&mut state, scope_id);
        }
    }

    pub(crate) fn active_scope_id(&self) -> Option<ScopeId> {
        self.inner.state.lock().active_scope_id
    }

    /// Marks cancellation before the Engine token is triggered. It does not
    /// require a forwarded turn id, so immediate stop after submission works.
    pub(crate) fn request_cancel_active(&self) -> Option<ScopeId> {
        let scope_id = self.active_scope_id()?;
        self.request_cancel_scope(scope_id).then_some(scope_id)
    }

    pub(crate) fn request_cancel_scope(&self, scope_id: ScopeId) -> bool {
        let changed = {
            let mut state = self.inner.state.lock();
            let Some(scope) = state.scopes.get_mut(&scope_id) else {
                return false;
            };
            scope.cancel_requested = true;
            scope.cleanup_settled = false;
            true
        };
        if changed {
            self.inner.cleanup_notify.notify_one();
        }
        changed
    }

    /// Associates an exact root tool result with its scope. Cleanup is handled
    /// by the supervisor worker, never inline on the serial event forwarder.
    pub(crate) fn register_task(&self, turn_id: &str, task_id: &str) -> bool {
        let should_notify = {
            let mut state = self.inner.state.lock();
            let Some(scope_id) = state.turn_scopes.get(turn_id).copied() else {
                return false;
            };
            state.task_scopes.insert(task_id.to_string(), scope_id);
            let Some(scope) = state.scopes.get_mut(&scope_id) else {
                return false;
            };
            scope.registered_task_ids.insert(task_id.to_string());
            if scope.cancel_requested {
                scope.cleanup_settled = false;
            }
            scope.cancel_requested
        };
        if should_notify {
            self.inner.cleanup_notify.notify_one();
        }
        true
    }

    /// Records sub-agent provenance only while its root scope is active. Agent
    /// ids remain mapped after root completion so a detached older agent cannot
    /// be mistaken for the next turn merely because it starts a shell later.
    pub(crate) fn register_agent(&self, turn_id: &str, agent_id: &str) -> bool {
        let mut state = self.inner.state.lock();
        let Some(scope_id) = state.turn_scopes.get(turn_id).copied() else {
            return false;
        };
        if let Some(existing_scope_id) = state.agent_scopes.get(agent_id).copied() {
            return existing_scope_id == scope_id;
        }
        let Some(scope) = state.scopes.get_mut(&scope_id) else {
            return false;
        };
        if scope.root_terminal {
            return false;
        }
        scope.live_agent_ids.insert(agent_id.to_string());
        state.agent_scopes.insert(agent_id.to_string(), scope_id);
        true
    }

    /// Inherits a nested agent's scope from its authoritative mailbox parent.
    /// This remains valid after the root turn is terminal because detached
    /// parents may continue spawning children until their own terminal event.
    pub(crate) fn register_child_agent(&self, parent_id: &str, child_id: &str) -> bool {
        let should_notify = {
            let mut state = self.inner.state.lock();
            let Some(scope_id) = state.agent_scopes.get(parent_id).copied() else {
                return false;
            };
            if !state.scopes.contains_key(&scope_id) {
                return false;
            }
            if let Some(previous_scope_id) =
                state.agent_scopes.insert(child_id.to_string(), scope_id)
            {
                if previous_scope_id != scope_id {
                    if let Some(previous_scope) = state.scopes.get_mut(&previous_scope_id) {
                        previous_scope.live_agent_ids.remove(child_id);
                    }
                }
            }
            let scope = state
                .scopes
                .get_mut(&scope_id)
                .expect("agent scope checked above");
            scope.live_agent_ids.insert(child_id.to_string());
            if scope.cancel_requested {
                scope.cleanup_settled = false;
            }
            scope.cancel_requested
        };
        if should_notify {
            self.inner.cleanup_notify.notify_one();
        }
        self.retire_settled_scopes();
        true
    }

    pub(crate) fn complete_agent(&self, agent_id: &str) {
        let should_notify = {
            let mut state = self.inner.state.lock();
            let Some(scope_id) = state.agent_scopes.get(agent_id).copied() else {
                return;
            };
            let Some(scope) = state.scopes.get_mut(&scope_id) else {
                return;
            };
            scope.live_agent_ids.remove(agent_id);
            if scope.cancel_requested {
                // One last scan must run after the producer is known terminal.
                scope.cleanup_settled = false;
            }
            scope.cancel_requested
        };
        if should_notify {
            self.inner.cleanup_notify.notify_one();
        } else {
            self.retire_settled_scopes();
        }
    }

    pub(crate) async fn cleanup_scope_with_retries(
        &self,
        scope_id: ScopeId,
    ) -> Result<ShellCleanupReport> {
        let mut report = ShellCleanupReport::default();
        let mut last_worker_error = None;
        for retry_index in 0..=CLEANUP_RETRY_DELAYS.len() {
            match self.cleanup_scope_once(scope_id).await {
                Ok(attempt) => {
                    report.merge(attempt);
                    last_worker_error = None;
                }
                Err(error) => last_worker_error = Some(error),
            }
            if !self.scope_has_retryable_cleanup(scope_id) {
                break;
            }
            if let Some(delay) = CLEANUP_RETRY_DELAYS.get(retry_index) {
                tokio::time::sleep(*delay).await;
            }
        }
        if let Some(error) = last_worker_error {
            return Err(error);
        }
        report.failed = self.exhausted_failures(scope_id);
        self.retire_settled_scopes();
        Ok(report)
    }

    /// Marks the root producer terminal only after its final non-blocking sweep.
    /// Completed turns preserve their detached jobs; cancelled scopes remain as
    /// provenance tombstones while any detached agents are still alive.
    pub(crate) async fn finalize_turn(
        &self,
        turn_id: &str,
        interrupted: bool,
    ) -> Result<ShellCleanupReport> {
        let scope_id = {
            let state = self.inner.state.lock();
            state
                .turn_scopes
                .get(turn_id)
                .copied()
                .or(state.active_scope_id)
        };
        let Some(scope_id) = scope_id else {
            return Ok(ShellCleanupReport::default());
        };
        self.finalize_scope(scope_id, interrupted).await
    }

    /// Finalizes the still-unbound active scope. The event stream can stop
    /// before `TurnStarted` bound the provisional submission scope; without
    /// this path the stale active scope would block every later submission.
    pub(crate) async fn finalize_active_scope(
        &self,
        interrupted: bool,
    ) -> Result<ShellCleanupReport> {
        let Some(scope_id) = self.active_scope_id() else {
            return Ok(ShellCleanupReport::default());
        };
        self.finalize_scope(scope_id, interrupted).await
    }

    pub(crate) async fn finalize_scope(
        &self,
        scope_id: ScopeId,
        interrupted: bool,
    ) -> Result<ShellCleanupReport> {
        if interrupted {
            self.request_cancel_scope(scope_id);
        }
        let should_cleanup = self
            .inner
            .state
            .lock()
            .scopes
            .get(&scope_id)
            .is_some_and(|scope| scope.cancel_requested);
        let cleanup = if should_cleanup {
            self.cleanup_scope_with_retries(scope_id).await
        } else {
            Ok(ShellCleanupReport::default())
        };

        {
            let mut state = self.inner.state.lock();
            if let Some(scope) = state.scopes.get_mut(&scope_id) {
                scope.root_terminal = true;
                scope.allow_unowned_fallback = false;
                if cleanup.is_err() {
                    scope.cleanup_settled = false;
                }
            }
            if state.active_scope_id == Some(scope_id) {
                state.active_scope_id = None;
            }
        }
        self.retire_settled_scopes();
        cleanup
    }

    async fn cleanup_scope_once(&self, scope_id: ScopeId) -> Result<ShellCleanupReport> {
        let input = {
            let state = self.inner.state.lock();
            let Some(scope) = state.scopes.get(&scope_id) else {
                return Ok(ShellCleanupReport::default());
            };
            if !scope.cancel_requested {
                return Ok(ShellCleanupReport::default());
            }
            CleanupInput {
                baseline_task_ids: scope.baseline_task_ids.clone(),
                registered_task_ids: scope.registered_task_ids.clone(),
                owned_agent_ids: state
                    .agent_scopes
                    .iter()
                    .filter_map(|(agent_id, owner_scope)| {
                        (*owner_scope == scope_id).then_some(agent_id.clone())
                    })
                    .collect(),
                owned_task_ids: state
                    .task_scopes
                    .iter()
                    .filter_map(|(task_id, owner_scope)| {
                        (*owner_scope == scope_id).then_some(task_id.clone())
                    })
                    .collect(),
                pending_attempts: scope
                    .pending_kills
                    .iter()
                    .map(|(task_id, pending)| (task_id.clone(), pending.attempts))
                    .collect(),
                allow_unowned_fallback: scope.allow_unowned_fallback,
            }
        };

        let shell_manager = self.inner.shell_manager.clone();
        let attempt = tauri::async_runtime::spawn_blocking(move || {
            run_cleanup_attempt(&shell_manager, &input)
        })
        .await
        .map_err(|error| anyhow!("shell cleanup worker join failed: {error}"))?;

        let attempt = match attempt {
            Ok(attempt) => attempt,
            Err(error) => {
                let message = format!("{error:#}");
                if let Some(scope) = self.inner.state.lock().scopes.get_mut(&scope_id) {
                    scope.cleanup_error = Some(message.clone());
                    scope.cleanup_error_attempts = scope.cleanup_error_attempts.saturating_add(1);
                    scope.cleanup_settled = false;
                }
                return Err(anyhow!(message));
            }
        };

        let mut state = self.inner.state.lock();
        let Some(scope) = state.scopes.get_mut(&scope_id) else {
            return Ok(ShellCleanupReport {
                killed: attempt.killed,
                failed: Vec::new(),
            });
        };
        scope.cleanup_error = None;
        scope.cleanup_error_attempts = 0;
        for task_id in attempt.resolved {
            scope.pending_kills.remove(&task_id);
        }
        for failure in attempt.failed {
            let pending = scope
                .pending_kills
                .entry(failure.task_id)
                .or_insert(PendingKill {
                    attempts: 0,
                    last_error: String::new(),
                });
            pending.attempts = pending.attempts.saturating_add(1);
            pending.last_error = failure.error;
        }
        scope.cleanup_settled = scope.pending_kills.is_empty();
        let failed = scope
            .pending_kills
            .iter()
            .filter(|(_, pending)| pending.attempts >= MAX_KILL_ATTEMPTS)
            .map(|(task_id, pending)| ShellCleanupFailure {
                task_id: task_id.clone(),
                error: pending.last_error.clone(),
            })
            .collect();
        Ok(ShellCleanupReport {
            killed: attempt.killed,
            failed,
        })
    }

    fn scope_has_retryable_cleanup(&self, scope_id: ScopeId) -> bool {
        self.inner
            .state
            .lock()
            .scopes
            .get(&scope_id)
            .is_some_and(|scope| {
                (scope.cleanup_error.is_some() && scope.cleanup_error_attempts < MAX_KILL_ATTEMPTS)
                    || scope
                        .pending_kills
                        .values()
                        .any(|pending| pending.attempts < MAX_KILL_ATTEMPTS)
            })
    }

    fn exhausted_failures(&self, scope_id: ScopeId) -> Vec<ShellCleanupFailure> {
        self.inner
            .state
            .lock()
            .scopes
            .get(&scope_id)
            .map(|scope| {
                scope
                    .pending_kills
                    .iter()
                    .filter(|(_, pending)| pending.attempts >= MAX_KILL_ATTEMPTS)
                    .map(|(task_id, pending)| ShellCleanupFailure {
                        task_id: task_id.clone(),
                        error: pending.last_error.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn ensure_cleanup_worker(&self) {
        if self
            .inner
            .worker_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        tauri::async_runtime::spawn(async move {
            cleanup_worker_loop(weak).await;
        });
    }

    fn scopes_needing_background_cleanup(&self) -> Vec<ScopeId> {
        self.inner
            .state
            .lock()
            .scopes
            .values()
            .filter(|scope| {
                scope.cancel_requested
                    && (!scope.root_terminal
                        || !scope.live_agent_ids.is_empty()
                        || (scope.cleanup_error.is_some()
                            && scope.cleanup_error_attempts < MAX_KILL_ATTEMPTS)
                        || scope
                            .pending_kills
                            .values()
                            .any(|pending| pending.attempts < MAX_KILL_ATTEMPTS))
            })
            .map(|scope| scope.id)
            .collect()
    }

    fn retire_settled_scopes(&self) {
        let mut state = self.inner.state.lock();
        let retired = state
            .scopes
            .values()
            .filter(|scope| {
                scope.root_terminal
                    && scope.live_agent_ids.is_empty()
                    && if scope.cancel_requested {
                        scope.cleanup_settled
                            && scope.cleanup_error.is_none()
                            && scope.pending_kills.is_empty()
                    } else {
                        true
                    }
            })
            .map(|scope| scope.id)
            .collect::<Vec<_>>();
        for scope_id in retired {
            remove_scope_locked(&mut state, scope_id);
        }

        // Scope ids increase monotonically, so sorting makes eviction
        // genuinely oldest-first instead of depending on HashMap order.
        let mut failed = state
            .scopes
            .values()
            .filter(|scope| {
                scope.root_terminal
                    && scope.live_agent_ids.is_empty()
                    && ((scope.cleanup_error.is_some()
                        && scope.cleanup_error_attempts >= MAX_KILL_ATTEMPTS)
                        || scope
                            .pending_kills
                            .values()
                            .any(|pending| pending.attempts >= MAX_KILL_ATTEMPTS))
            })
            .map(|scope| scope.id)
            .collect::<Vec<_>>();
        failed.sort_unstable();
        for scope_id in failed {
            if !state.failed_scope_order.contains(&scope_id) {
                state.failed_scope_order.push_back(scope_id);
            }
        }
        while state.failed_scope_order.len() > MAX_FAILED_SCOPE_TOMBSTONES {
            if let Some(scope_id) = state.failed_scope_order.pop_front() {
                remove_scope_locked(&mut state, scope_id);
            }
        }
    }
}

#[derive(Clone)]
struct CleanupInput {
    baseline_task_ids: HashSet<String>,
    registered_task_ids: HashSet<String>,
    owned_agent_ids: HashSet<String>,
    owned_task_ids: HashSet<String>,
    pending_attempts: HashMap<String, u8>,
    allow_unowned_fallback: bool,
}

struct CleanupAttempt {
    killed: Vec<ShellResult>,
    resolved: Vec<String>,
    failed: Vec<ShellCleanupFailure>,
}

fn run_cleanup_attempt(
    shell_manager: &SharedShellManager,
    input: &CleanupInput,
) -> Result<CleanupAttempt> {
    let mut manager = shell_manager
        .lock()
        .map_err(|_| anyhow!("Shell manager lock poisoned"))?;
    let snapshots = manager.list_jobs();
    let running_ids = snapshots
        .iter()
        .filter(|job| job.status == ShellStatus::Running)
        .map(|job| job.id.clone())
        .collect::<HashSet<_>>();
    let mut resolved = input
        .pending_attempts
        .keys()
        .filter(|task_id| !running_ids.contains(*task_id))
        .cloned()
        .collect::<Vec<_>>();
    let candidates = snapshots
        .into_iter()
        .filter(|job| job.status == ShellStatus::Running)
        .filter(|job| is_cleanup_candidate(job, input))
        .filter(|job| input.pending_attempts.get(&job.id).copied().unwrap_or(0) < MAX_KILL_ATTEMPTS)
        .map(|job| job.id)
        .collect::<Vec<_>>();

    let mut killed = Vec::with_capacity(candidates.len());
    let mut failed = Vec::new();
    for task_id in candidates {
        match manager.kill(&task_id) {
            Ok(result) => {
                resolved.push(task_id);
                killed.push(result);
            }
            Err(error) => {
                // The process can exit between list_jobs and kill. Preserve
                // CodeWhale's generic kill contract while making host cleanup
                // idempotent for that terminal race.
                let became_terminal = manager
                    .inspect_job(&task_id)
                    .is_ok_and(|job| job.snapshot.status != ShellStatus::Running);
                if became_terminal {
                    resolved.push(task_id);
                } else {
                    failed.push(ShellCleanupFailure {
                        task_id,
                        error: format!("{error:#}"),
                    });
                }
            }
        }
    }
    Ok(CleanupAttempt {
        killed,
        resolved,
        failed,
    })
}

fn is_cleanup_candidate(
    job: &deepseek_tui::tools::shell::ShellJobSnapshot,
    input: &CleanupInput,
) -> bool {
    input.registered_task_ids.contains(&job.id)
        || input.owned_task_ids.contains(&job.id)
        || job
            .owner_agent_id
            .as_ref()
            .is_some_and(|owner| input.owned_agent_ids.contains(owner))
        || (input.allow_unowned_fallback
            && job.owner_agent_id.is_none()
            && !input.baseline_task_ids.contains(&job.id))
        || input.pending_attempts.contains_key(&job.id)
}

async fn cleanup_worker_loop(inner: Weak<RegistryInner>) {
    loop {
        let Some(strong) = inner.upgrade() else {
            break;
        };
        tokio::select! {
            () = strong.cleanup_notify.notified() => {}
            () = tokio::time::sleep(CLEANUP_POLL_INTERVAL) => {}
        }
        drop(strong);

        let Some(strong) = inner.upgrade() else {
            break;
        };
        let registry = TurnShellTaskRegistry { inner: strong };
        let scopes = registry.scopes_needing_background_cleanup();
        for scope_id in scopes {
            if let Err(error) = registry.cleanup_scope_once(scope_id).await {
                log::error!(
                    "[pinvou3][chat] background shell cleanup failed scope={scope_id}: {error:#}"
                );
            }
        }
        registry.retire_settled_scopes();
    }
}

fn remove_scope_locked(state: &mut RegistryState, scope_id: ScopeId) {
    state.scopes.remove(&scope_id);
    if state.active_scope_id == Some(scope_id) {
        state.active_scope_id = None;
    }
    state
        .turn_scopes
        .retain(|_, owner_scope| *owner_scope != scope_id);
    state
        .agent_scopes
        .retain(|_, owner_scope| *owner_scope != scope_id);
    state
        .task_scopes
        .retain(|_, owner_scope| *owner_scope != scope_id);
    state
        .failed_scope_order
        .retain(|failed_scope_id| *failed_scope_id != scope_id);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use deepseek_tui::tools::shell::{new_shared_shell_manager, ShellJobOwner, ShellJobSnapshot};

    fn sleep_command() -> &'static str {
        if std::env::consts::OS == "windows" {
            "Start-Sleep -Seconds 30"
        } else {
            "sleep 30"
        }
    }

    fn start_background(manager: &SharedShellManager, owner: Option<ShellJobOwner>) -> String {
        manager
            .lock()
            .expect("shell manager lock")
            .execute_with_options_env_for_owner(
                sleep_command(),
                None,
                600_000,
                true,
                None,
                false,
                None,
                HashMap::new(),
                owner,
            )
            .expect("start background shell")
            .task_id
            .expect("background task id")
    }

    fn status(manager: &SharedShellManager, task_id: &str) -> ShellStatus {
        manager
            .lock()
            .expect("shell manager lock")
            .inspect_job(task_id)
            .expect("shell job")
            .snapshot
            .status
    }

    fn running_job(id: &str, owner_agent_id: Option<&str>) -> ShellJobSnapshot {
        ShellJobSnapshot {
            id: id.to_string(),
            job_id: id.to_string(),
            command: "sleep 30".to_string(),
            cwd: std::env::temp_dir(),
            status: ShellStatus::Running,
            exit_code: None,
            elapsed_ms: 0,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            stdout_len: 0,
            stderr_len: 0,
            stdin_available: false,
            stale: false,
            elapsed_since_output_ms: None,
            linked_task_id: None,
            owner_agent_id: owner_agent_id.map(str::to_string),
            owner_agent_name: owner_agent_id.map(str::to_string),
        }
    }

    #[test]
    fn candidate_ownership_preserves_an_older_agents_late_job() {
        let input = CleanupInput {
            baseline_task_ids: HashSet::new(),
            registered_task_ids: HashSet::new(),
            owned_agent_ids: HashSet::from(["agent-current".to_string()]),
            owned_task_ids: HashSet::new(),
            pending_attempts: HashMap::new(),
            allow_unowned_fallback: true,
        };
        let current = running_job("current", Some("agent-current"));
        let older = running_job("older-late", Some("agent-older"));
        let ownerless = running_job("root-current", None);

        let candidates = [current, older, ownerless]
            .into_iter()
            .filter(|job| is_cleanup_candidate(job, &input))
            .map(|job| job.id)
            .collect::<HashSet<_>>();

        assert_eq!(
            candidates,
            HashSet::from(["current".to_string(), "root-current".to_string()])
        );
        assert!(!candidates.contains("older-late"));
    }

    #[test]
    fn exhausted_cleanup_failure_keeps_scope_for_diagnostics_without_polling_forever() {
        let registry = TurnShellTaskRegistry::new(new_shared_shell_manager(std::env::temp_dir()));
        let scope_id = 1;
        let mut pending_kills = HashMap::new();
        pending_kills.insert(
            "task-still-running".to_string(),
            PendingKill {
                attempts: MAX_KILL_ATTEMPTS,
                last_error: "access denied".to_string(),
            },
        );
        registry.inner.state.lock().scopes.insert(
            scope_id,
            TurnShellScope {
                id: scope_id,
                turn_id: Some("turn-failed-cleanup".to_string()),
                baseline_task_ids: HashSet::new(),
                registered_task_ids: HashSet::from(["task-still-running".to_string()]),
                live_agent_ids: HashSet::new(),
                pending_kills,
                cancel_requested: true,
                root_terminal: true,
                allow_unowned_fallback: false,
                cleanup_settled: false,
                cleanup_error: None,
                cleanup_error_attempts: 0,
            },
        );

        registry.retire_settled_scopes();

        assert!(registry.inner.state.lock().scopes.contains_key(&scope_id));
        assert!(registry.scopes_needing_background_cleanup().is_empty());
        assert_eq!(
            registry.exhausted_failures(scope_id),
            vec![ShellCleanupFailure {
                task_id: "task-still-running".to_string(),
                error: "access denied".to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn provisional_submission_scope_is_abandoned_when_guard_drops() {
        let registry = TurnShellTaskRegistry::new(new_shared_shell_manager(std::env::temp_dir()));
        let guard = registry.prepare_submission().await.expect("prepare guard");
        let scope_id = guard.scope_id();

        drop(guard);

        let state = registry.inner.state.lock();
        assert_eq!(state.active_scope_id, None);
        assert!(!state.scopes.contains_key(&scope_id));
    }

    #[tokio::test]
    async fn reliable_child_lineage_is_not_overwritten_by_the_current_turn() {
        let registry = TurnShellTaskRegistry::new(new_shared_shell_manager(std::env::temp_dir()));
        let older_scope = registry.prepare_turn().await.expect("older scope");
        registry
            .bind_active_turn("turn-older")
            .expect("bind older turn");
        assert!(registry.register_agent("turn-older", "agent-parent"));
        registry
            .finalize_scope(older_scope, false)
            .await
            .expect("finish older root");

        let current_scope = registry.prepare_turn().await.expect("current scope");
        registry
            .bind_active_turn("turn-current")
            .expect("bind current turn");
        assert!(registry.register_child_agent("agent-parent", "agent-child"));
        assert!(!registry.register_agent("turn-current", "agent-child"));

        let state = registry.inner.state.lock();
        assert_eq!(state.agent_scopes.get("agent-child"), Some(&older_scope));
        assert!(state
            .scopes
            .get(&older_scope)
            .is_some_and(|scope| scope.live_agent_ids.contains("agent-child")));
        assert_eq!(state.active_scope_id, Some(current_scope));
    }

    #[test]
    fn failed_scope_tombstones_are_bounded() {
        let registry = TurnShellTaskRegistry::new(new_shared_shell_manager(std::env::temp_dir()));
        {
            let mut state = registry.inner.state.lock();
            for scope_id in 1..=(MAX_FAILED_SCOPE_TOMBSTONES as u64 + 1) {
                let task_id = format!("task-{scope_id}");
                state
                    .turn_scopes
                    .insert(format!("turn-{scope_id}"), scope_id);
                state.task_scopes.insert(task_id.clone(), scope_id);
                state.scopes.insert(
                    scope_id,
                    TurnShellScope {
                        id: scope_id,
                        turn_id: Some(format!("turn-{scope_id}")),
                        baseline_task_ids: HashSet::new(),
                        registered_task_ids: HashSet::from([task_id.clone()]),
                        live_agent_ids: HashSet::new(),
                        pending_kills: HashMap::from([(
                            task_id,
                            PendingKill {
                                attempts: MAX_KILL_ATTEMPTS,
                                last_error: "access denied".to_string(),
                            },
                        )]),
                        cancel_requested: true,
                        root_terminal: true,
                        allow_unowned_fallback: false,
                        cleanup_settled: false,
                        cleanup_error: None,
                        cleanup_error_attempts: 0,
                    },
                );
            }
        }

        registry.retire_settled_scopes();

        let state = registry.inner.state.lock();
        assert_eq!(state.failed_scope_order.len(), MAX_FAILED_SCOPE_TOMBSTONES);
        assert!(!state.scopes.contains_key(&1));
        assert!(!state.turn_scopes.contains_key("turn-1"));
        assert!(!state.task_scopes.contains_key("task-1"));
        assert!(state
            .scopes
            .contains_key(&(MAX_FAILED_SCOPE_TOMBSTONES as u64 + 1)));
    }

    #[tokio::test]
    async fn forkguard_interrupted_turn_preserves_preexisting_and_old_agent_jobs() {
        let manager = new_shared_shell_manager(std::env::temp_dir());
        let previous = start_background(&manager, None);
        let registry = TurnShellTaskRegistry::new(manager.clone());
        let older_scope = registry.prepare_turn().await.expect("older scope");
        registry
            .bind_active_turn("turn-older")
            .expect("bind older turn");
        assert!(registry.register_agent("turn-older", "agent-older"));
        registry
            .finalize_scope(older_scope, false)
            .await
            .expect("finish older turn");

        registry.prepare_turn().await.expect("current scope");
        registry
            .bind_active_turn("turn-current")
            .expect("bind current turn");
        let current = start_background(&manager, None);
        let older_late = start_background(
            &manager,
            Some(ShellJobOwner {
                agent_id: "agent-older".to_string(),
                agent_name: "older".to_string(),
            }),
        );

        let report = registry
            .finalize_turn("turn-current", true)
            .await
            .expect("finish interrupted turn");

        // The background supervisor started by `prepare_turn` may win the
        // reclaim race against the inline cleanup, so the report can record
        // the kill in either path. Only the terminal job status is
        // authoritative here.
        if !report
            .killed
            .iter()
            .any(|entry| entry.task_id.as_deref() == Some(current.as_str()))
        {
            tokio::time::timeout(Duration::from_secs(2), async {
                while status(&manager, &current) == ShellStatus::Running {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .await
            .expect("interrupted turn should reclaim its current job");
        }
        assert_eq!(status(&manager, &current), ShellStatus::Killed);
        assert_eq!(status(&manager, &previous), ShellStatus::Running);
        assert_eq!(status(&manager, &older_late), ShellStatus::Running);
        let mut manager = manager.lock().expect("shell manager lock");
        manager.kill(&previous).expect("cleanup previous job");
        manager.kill(&older_late).expect("cleanup older agent job");
    }

    #[tokio::test]
    async fn cancellation_before_turn_binding_still_kills_the_scope_job() {
        let manager = new_shared_shell_manager(std::env::temp_dir());
        let registry = TurnShellTaskRegistry::new(manager.clone());
        let scope_id = registry.prepare_turn().await.expect("prepare scope");
        assert_eq!(registry.request_cancel_active(), Some(scope_id));
        let late = start_background(&manager, None);

        // The background supervisor started by `prepare_turn` may win the
        // reclaim race against this explicit call, so only the terminal job
        // status is authoritative here.
        registry
            .cleanup_scope_with_retries(scope_id)
            .await
            .expect("clean unbound scope");
        tokio::time::timeout(Duration::from_secs(2), async {
            while status(&manager, &late) == ShellStatus::Running {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("cancelled unbound scope should reclaim its job");
        assert_eq!(status(&manager, &late), ShellStatus::Killed);
    }

    #[tokio::test]
    async fn task_registered_after_stop_is_reclaimed_by_supervisor() {
        let manager = new_shared_shell_manager(std::env::temp_dir());
        let baseline_job = start_background(&manager, None);
        let registry = TurnShellTaskRegistry::new(manager.clone());
        registry.prepare_turn().await.expect("prepare scope");
        registry
            .bind_active_turn("turn-late-registration")
            .expect("bind turn");
        assert!(registry.request_cancel_active().is_some());

        let initial = registry
            .cleanup_scope_with_retries(registry.active_scope_id().expect("active scope"))
            .await
            .expect("initial cleanup");
        assert!(initial.killed.is_empty());
        assert_eq!(status(&manager, &baseline_job), ShellStatus::Running);

        assert!(registry.register_task("turn-late-registration", &baseline_job));
        tokio::time::timeout(Duration::from_secs(2), async {
            while status(&manager, &baseline_job) == ShellStatus::Running {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("supervisor should reclaim the late registered task");
        assert_eq!(status(&manager, &baseline_job), ShellStatus::Killed);
        registry
            .finalize_turn("turn-late-registration", true)
            .await
            .expect("finalize turn");
    }

    #[tokio::test]
    async fn completed_turn_keeps_its_detached_job_running() {
        let manager = new_shared_shell_manager(std::env::temp_dir());
        let registry = TurnShellTaskRegistry::new(manager.clone());
        registry.prepare_turn().await.expect("prepare scope");
        registry
            .bind_active_turn("turn-complete")
            .expect("bind turn");
        let detached = start_background(&manager, None);
        assert!(registry.register_task("turn-complete", &detached));

        let report = registry
            .finalize_turn("turn-complete", false)
            .await
            .expect("finish completed turn");

        assert!(report.killed.is_empty());
        assert_eq!(status(&manager, &detached), ShellStatus::Running);
        manager
            .lock()
            .expect("shell manager lock")
            .kill(&detached)
            .expect("cleanup detached job");
    }

    #[tokio::test]
    async fn unbound_scope_is_reclaimed_when_the_stream_stops_before_turn_started() {
        let manager = new_shared_shell_manager(std::env::temp_dir());
        let registry = TurnShellTaskRegistry::new(manager.clone());
        registry.prepare_turn().await.expect("prepare scope");
        let job = start_background(&manager, None);

        registry
            .finalize_active_scope(true)
            .await
            .expect("finalize unbound scope");

        assert_eq!(registry.active_scope_id(), None);
        assert_eq!(status(&manager, &job), ShellStatus::Killed);
        // A later submission must be able to open a fresh scope.
        registry
            .prepare_turn()
            .await
            .expect("prepare a scope after the stale one closed");
        registry
            .finalize_active_scope(false)
            .await
            .expect("close the fresh scope");
    }
}
