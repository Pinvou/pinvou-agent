//! Engine pool for concurrent multi-session use.
//!
//! Old model: one Engine for the whole process; switching sessions replaced
//! the entire internal state via `Op::SyncSession`
//! → only one session could be served at a time, and switching away from a
//! running session crosstailed them.
//!
//! New model: **one independent Engine per session** (the foundation's
//! `spawn_engine` is an independent factory, see
//! [`AppEngine::spawn_for_session`]). This pool manages the lifecycle of
//! those engines by `session_id`:
//!  - **lazy spawn**: spawn only when a session gets its first message (with
//!    that session's own workspace + instructions); a session with existing
//!    on-disk history is hydrated with a one-shot `SyncSession` after spawn.
//!  - **idle reclaim (a tightening of the old keep-alive policy)**: after
//!    spawn an engine stays resident and background sessions keep running
//!    their turns, but no longer forever — each engine is an in-process task
//!    + its own channel/toolset, the pool is unbounded and memory grows
//!    linearly with session count. A background sweep (`start_idle_reaper`)
//!    reclaims engines that have been "idle for more than
//!    `IDLE_EVICT_AFTER_SECS` with no in-flight turn and not active";
//!    reclaiming only returns to lazy-spawn semantics — the next message
//!    rebuilds via `get_or_spawn` and rehydrates with `SyncSession`,
//!    losslessly.
//!  - **evict**: reclaim on session deletion (cancel the running turn +
//!    Shutdown the engine + abort the forwarder).
//!
//! The pool itself is Tauri State; chat / cancel / submit_user_input etc. in
//! `commands.rs` all route to the corresponding engine by `session_id`.
//!
//! Concurrency note: runtime model preparation may access an external
//! credential service and must not hold the global `entries` lock. Each
//! session first serializes prepare/compare/rebuild through its own runtime
//! lock, then briefly holds `entries` to complete the local spawn — avoiding
//! duplicate engines for the same session at the root, and never letting a
//! slow credential service block other sessions.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use deepseek_tui::AppMode;
use deepseek_tui::core::events::TurnOutcomeStatus;
use deepseek_tui::core::ops::Op;
use deepseek_tui::models::{ContentBlock, Message};
use deepseek_tui::tools::shell::{ShellJobSnapshot, ShellResult};
use deepseek_tui::tools::spec::ToolSpec;
use deepseek_tui::tools::user_input::UserInputResponse;
use parking_lot::Mutex as SyncMutex;
use serde::Serialize;
use tauri::AppHandle;
use tauri::async_runtime::JoinHandle;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::features::assistant::engine::{
    AppEngine, EngineTurnSignal, TranscriptOperation, TurnIdentity, TurnLifecycle, TurnReservation,
    dispatch_turn_bound_cancel,
};
#[cfg(any(feature = "benchmark-hooks", test))]
use crate::features::assistant::eval::{EvalModelSelection, EvalSuiteModelSnapshot, ModelIdentity};
use crate::features::assistant::expert_roster::ExpertRosterSnapshot;
use crate::features::assistant::platform::bridge::{Pinvou3Bridge, base_url_uses_local_or_private};
use crate::features::assistant::runtime_model::PreparedRuntimeModel;
use crate::features::assistant::turn_shell_tasks::{SessionShellManagers, SessionTurnShellTasks};
use crate::features::sessions::{ScheduledRunProfile, SessionStore, transcript_revision};
use crate::platform::prefs::{SavedModel, UserPrefs};

// The idle-reclaim threshold and sweep interval (IDLE_EVICT_AFTER_SECS /
// REAP_INTERVAL_SECS) converge into `core::reaper`: an engine is an
// in-process task (not a subprocess), but each one occupies a channel, a
// toolset, and a hydrated conversation context, the pool is unbounded, and
// memory grows linearly with session count. Reclaim engines that are idle
// beyond the threshold with no in-flight turn and not the active session
// (reusing `reclaim_engine_entry`'s reclaim sequence). Reclaiming returns
// to lazy-spawn semantics; the next message rebuilds + rehydrates via
// SyncSession, losslessly. 30 minutes is a deliberately conservative
// value: better to under-reclaim than to evict a session about to be used.
use crate::core::reaper::{IDLE_EVICT_AFTER_SECS, IdleReaperGuard};

/// Upper bound for side-effect awaits issued while the per-session turn gate
/// is held: the phase-two subagent-cascade sends in `cancel_turn_with_gates`,
/// the shell-scope cleanup join, the shell-reclaim finalize, and the reclaim
/// shutdown sends. A stalled engine run loop with the ops channel full and
/// never drained would otherwise hold the gate forever and block evict,
/// delete, and the next send of the whole session (issue #255). On timeout
/// the await is abandoned: a dropped cascade send is never enqueued (tokio
/// mpsc send is cancel-safe) and every gate-held sender is serialized with
/// the next turn's gate-submitted `SendMessage` on the same turn gate, so no
/// late cancel can be enqueued after it. The ordering argument covers turns
/// the app submits under the gate; engine-autonomous turns (idle child
/// completion, goal continuation) start without the gate and are adopted by
/// the forwarder only at `TurnStarted` — a pre-existing window outside this
/// guarantee. An abandoned cascade send leaves the old turn's subagents
/// alive on the stalled engine until it unsticks (their own step/time
/// budgets apply) or reclaim shuts it down.
///
/// Why 5s: the budget must absorb every ordinary gate-held side effect —
/// a cascade send is queue wait, not work — while keeping evict/delete
/// responsive. The shell finalize's legitimate worst case can genuinely
/// exceed it (one stubborn job can cost ~3.5s between TERM grace, reaper
/// wait and reader join, and a second ladder queues behind the
/// per-registry cleanup gate), so a healthy-but-slow finalize can
/// spuriously trip the conservative `cleanup_failed` preset; that is
/// accepted (the flag is diagnostics-only and the detached run still
/// records the true outcome).
const TURN_GATE_AWAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Total wait per op for the detached retry that re-delivers the reclaim
/// shutdown ops after the gate-held send gave up (issue #255). The retry
/// runs outside the turn gate, so this bound only decides how long the
/// process keeps trying to let the reclaimed engine exit through its normal
/// `Shutdown` path; if the engine is still stalled when it expires, the
/// engine task lingers until process exit (it would leak either way while
/// stalled — the retry only shrinks the window in the temporary-stall case,
/// which is the common one). On a delete/evict that gave up in this state,
/// the lingering engine's late terminal write can land under the already
/// removed session directory as orphaned files — harmless residue: session
/// listing only reads top-level `<id>.json` records, so no ghost session
/// appears.
const RECLAIM_SHUTDOWN_RETRY_PATIENCE: std::time::Duration = std::time::Duration::from_secs(60);

/// Idle-reclaim predicate (pure function, easy to unit test): a turn being
/// active (reservation held or terminal closing), a scheduled turn in
/// flight (the spawn→submit window of run_scheduled_turn where the
/// lifecycle is not yet active), and the currently active session are never
/// reclaimed. The predicate body is shared with the ACP side as
/// `core::reaper::should_reap_idle`; this side keeps assistant's
/// parameter-semantics naming (seconds + the two busy flags).
fn should_reap_idle_engine(
    turn_active: bool,
    scheduled_running: bool,
    is_active_session: bool,
    idle_for_secs: u64,
) -> bool {
    crate::core::reaper::should_reap_idle(
        turn_active,
        scheduled_running,
        is_active_session,
        std::time::Duration::from_secs(idle_for_secs),
    )
}

/// Rebind eviction recheck (pure function, unit-testable; review #463
/// eviction-tail TOCTOU): only genuine turn activity blocks the reclaim — an
/// in-flight/reserved turn (reserve occupies the lifecycle before the gate is
/// taken) or a running scheduled round. Unlike the idle reaper there is no
/// idle-duration or active-session gate: a rebound session must be reclaimed
/// even when recently active, so the next turn respawns in the new directory.
fn rebind_evictable(turn_active: bool, scheduled_running: bool) -> bool {
    !turn_active && !scheduled_running
}

/// `evict_if_idle`'s in-lock recheck (pure function, easy to unit test): the
/// activity clock must not have advanced since the snapshot (both turn
/// submission and terminal-state closing advance it), and the idle-reclaim
/// conditions must still hold by current values; if either fails, skip this
/// reclaim round and leave it to the next sweep.
fn should_still_reap_after_snapshot(
    turn_active: bool,
    scheduled_running: bool,
    is_active_session: bool,
    idle_for_secs: u64,
    current_last_active_ms: u64,
    snapshot_last_active_ms: u64,
) -> bool {
    current_last_active_ms <= snapshot_last_active_ms
        && should_reap_idle_engine(
            turn_active,
            scheduled_running,
            is_active_session,
            idle_for_secs,
        )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduledTurnCompletion {
    pub turn_id: String,
    pub status: TurnOutcomeStatus,
    pub error: Option<String>,
    pub cancel_requested: bool,
}

struct ScheduledUnattendedGuard(Arc<AtomicBool>);

impl ScheduledUnattendedGuard {
    fn enter(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::Release);
        Self(flag)
    }
}

impl Drop for ScheduledUnattendedGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Clone, Default)]
struct SessionTurnLocks {
    locks: Arc<Mutex<HashMap<String, Weak<Mutex<()>>>>>,
}

impl SessionTurnLocks {
    async fn for_session(&self, session_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        locks.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = locks.get(session_id).and_then(Weak::upgrade) {
            return gate;
        }

        let gate = Arc::new(Mutex::new(()));
        locks.insert(session_id.to_string(), Arc::downgrade(&gate));
        gate
    }
}

#[cfg(any(feature = "benchmark-hooks", test))]
#[derive(Clone, Default)]
struct EvalModelSnapshots {
    next_token: Arc<AtomicU64>,
    saved_models: Arc<SyncMutex<HashMap<String, SavedModel>>>,
    suite_models: Arc<SyncMutex<HashMap<String, SavedModel>>>,
    session_models: Arc<SyncMutex<HashMap<String, SavedModel>>>,
}

#[cfg(any(feature = "benchmark-hooks", test))]
impl EvalModelSnapshots {
    fn pin(&self, saved_model: SavedModel, identity: ModelIdentity) -> EvalModelSelection {
        let sequence = self.next_token.fetch_add(1, Ordering::Relaxed);
        let token = format!("eval-model-{}-{sequence}", std::process::id());
        let model_id = Some(saved_model.id.clone());
        self.saved_models.lock().insert(token.clone(), saved_model);
        EvalModelSelection::new(token, model_id, identity)
    }

    fn pin_suite(
        &self,
        saved_model: SavedModel,
        identity: ModelIdentity,
    ) -> EvalSuiteModelSnapshot {
        let sequence = self.next_token.fetch_add(1, Ordering::Relaxed);
        let token = format!("eval-suite-model-{}-{sequence}", std::process::id());
        self.suite_models.lock().insert(token.clone(), saved_model);
        EvalSuiteModelSnapshot::new(token, identity)
    }

    fn derive_case_selection(&self, suite: &EvalSuiteModelSnapshot) -> Result<EvalModelSelection> {
        let saved_model = self
            .suite_models
            .lock()
            .get(suite.token())
            .cloned()
            .context("evaluation suite model snapshot is missing")?;
        Ok(self.pin(saved_model, suite.identity().clone()))
    }

    fn discard_suite(&self, suite: &EvalSuiteModelSnapshot) {
        self.suite_models.lock().remove(suite.token());
    }

    fn bind_to_session(&self, session_id: &str, selection: &EvalModelSelection) -> Result<()> {
        let saved_model = self
            .saved_models
            .lock()
            .remove(selection.token())
            .with_context(|| "evaluation model selection is missing or already consumed")?;
        self.session_models
            .lock()
            .insert(session_id.to_string(), saved_model);
        Ok(())
    }

    fn for_session(&self, session_id: &str) -> Option<SavedModel> {
        self.session_models.lock().get(session_id).cloned()
    }

    fn forget_session(&self, session_id: &str) {
        self.session_models.lock().remove(session_id);
    }

    // Test-only: no benchmark-hooks or production caller consumes a pinned
    // saved-model selection outside the snapshot-map tests.
    #[cfg(test)]
    fn discard(&self, selection: &EvalModelSelection) {
        self.saved_models.lock().remove(selection.token());
    }
}

#[derive(Clone, Default)]
struct SessionTurnLifecycles {
    states: Arc<SyncMutex<HashMap<String, Arc<TurnLifecycle>>>>,
}

impl SessionTurnLifecycles {
    fn for_session(&self, session_id: &str) -> Arc<TurnLifecycle> {
        let mut states = self.states.lock();
        states
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(TurnLifecycle::default()))
            .clone()
    }

    fn get(&self, session_id: &str) -> Option<Arc<TurnLifecycle>> {
        self.states.lock().get(session_id).cloned()
    }

    fn remove(&self, session_id: &str) {
        self.states.lock().remove(session_id);
    }
}

fn scheduled_profile_after_turn_gate(
    store: &SessionStore,
    session_id: &str,
    expected_task_id: &str,
) -> Result<ScheduledRunProfile> {
    let profile = store.scheduled_profile(session_id).with_context(|| {
        format!("Scheduled session '{session_id}' was deleted before the follow-up could start")
    })?;
    if profile.task_id != expected_task_id {
        bail!(
            "Scheduled session '{session_id}' changed owner from '{expected_task_id}' to '{}'",
            profile.task_id
        );
    }
    if !store.scheduled_session_exists(session_id) {
        bail!("Scheduled session '{session_id}' no longer exists");
    }
    Ok(profile)
}

async fn delete_scheduled_run_with_gate<F, Fut>(
    turn_locks: &SessionTurnLocks,
    store: &SessionStore,
    session_id: &str,
    expected_task_id: &str,
    evict_locked: F,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
{
    let turn_lock = turn_locks.for_session(session_id).await;
    let _turn = turn_lock.lock().await;
    evict_locked().await;
    store.delete_scheduled_run(session_id, expected_task_id)
}

/// The shared delete path: ordinary chat deletion and the contract tests
/// both go through it, holding the exact same turn gate as lazy spawn /
/// send, so a queued send cannot resurrect the session between the engine
/// reclaim and the on-disk deletion.
async fn delete_chat_session_with_gate<F, Fut, G>(
    turn_locks: &SessionTurnLocks,
    store: &SessionStore,
    session_id: &str,
    evict_locked: F,
    forget: G,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
    G: FnOnce(),
{
    let turn_lock = turn_locks.for_session(session_id).await;
    let _turn = turn_lock.lock().await;
    evict_locked().await;
    store.delete(session_id)?;
    forget();
    // timing is process-level state within the same feature, so keys can
    // be cleared directly (no dependency inversion needed). This covers
    // paths that bypass the app composition root where the
    // SessionPurgedHook is not registered (eval teardown, tests): unpaired
    // turn-queue keys pinned by session id would otherwise grow unbounded
    // under create/delete cycles like GAIA. Idempotent overlap with the
    // hook cleanup fired from store.delete; no duplicated side effects.
    crate::features::assistant::timing::clear_session(session_id);
    Ok(())
}

#[cfg(test)]
fn delete_then_forget<D, G>(delete: D, forget: G) -> Result<()>
where
    D: FnOnce() -> Result<()>,
    G: FnOnce(),
{
    let delete_result = delete();
    if delete_result.is_err() {
        return delete_result;
    }
    forget();
    delete_result
}

async fn quiesce_engine_before_reclaim<C, S, SFut, F, Fut, T>(
    cancel_current: C,
    stop_forwarder: S,
    finish_reclaimed: F,
) -> T
where
    C: FnOnce(),
    S: FnOnce() -> SFut,
    SFut: Future<Output = ()>,
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    cancel_current();
    stop_forwarder().await;
    finish_reclaimed().await
}

/// `evict_if_idle`'s lock-order skeleton (same extraction idea as
/// `cancel_turn_with_gates`, for deterministic concurrency tests with bare
/// Default components + probe closures): take the turn gate first, then the
/// runtime lock, and execute `take_entry` under both (in-lock recheck +
/// atomic removal, returning `None` when activity appeared after the
/// snapshot and this round is skipped); only an actual removal proceeds to
/// `reclaim`.
async fn evict_if_idle_with_gates<T, Take, TakeFut, Reclaim, ReclaimFut>(
    turn_locks: &SessionTurnLocks,
    runtime_locks: &SessionTurnLocks,
    session_id: &str,
    take_entry: Take,
    reclaim: Reclaim,
) -> bool
where
    Take: FnOnce() -> TakeFut,
    TakeFut: Future<Output = Option<T>>,
    Reclaim: FnOnce(T) -> ReclaimFut,
    ReclaimFut: Future<Output = ()>,
{
    let turn_lock = turn_locks.for_session(session_id).await;
    let _turn = turn_lock.lock().await;
    let runtime_lock = runtime_locks.for_session(session_id).await;
    let _runtime = runtime_lock.lock().await;
    let Some(entry) = take_entry().await else {
        return false;
    };
    reclaim(entry).await;
    true
}

/// How long the rebind eviction tail waits for a session's turn gate before
/// giving up and leaving the session alone (review #463 round-8 minor 4).
/// A scheduled round holds that gate for its WHOLE duration, so delegating to
/// the unbounded [`evict_if_idle_with_gates`] would stall the rebind command —
/// and the process-wide rebind gate behind it — for minutes, and the round
/// would still not be reported afterwards. The eviction is best-effort by
/// design, so a gate that is still held after this bound is treated as "not
/// idle": nothing is touched and the command reports the session as post-busy.
/// Comfortably longer than any normal send's gate hold, so ordinary turns are
/// still observed rather than skipped.
const REBIND_EVICT_GATE_TIMEOUT: Duration = Duration::from_secs(2);

/// Rebind eviction tail skeleton (review #463 M1 + eviction-tail TOCTOU,
/// extended in round 8 by M2 and minor 4): the idle-gated take runs under the
/// turn gate + runtime lock, and a successful take ALSO drops the per-session
/// shell state under the same gates.
///
/// Unlike [`evict_if_idle_with_gates`] BOTH gates — the turn gate and the
/// runtime lock — are acquired under a timeout (see
/// [`REBIND_EVICT_GATE_TIMEOUT`]) and a timeout counts as "not evicted",
/// keeping the command responsive. The runtime lock needs its own timeout
/// because a cold spawn holds it for many seconds, far beyond any turn gate
/// wait (round-8 should-fix 4).
///
/// Both registries have to go. `SessionShellManagers::for_session` is
/// `entry().or_insert_with` and the manager's cwd is pinned at construction,
/// so a surviving manager would keep executing bare shell commands in the old
/// directory while the rebuilt engine runs in the new one — split-brain
/// inside one turn (M1). `SessionTurnShellTasks::for_session` is
/// `entry().or_insert_with` too and its registry pins that same shell
/// manager, so a surviving entry would resolve the next turn's scope against
/// the OLD manager: the baseline diff and the end-of-turn cleanup kills would
/// miss the jobs the turn actually started, leaving detached/background jobs
/// running (round-8 M2). This mirrors `forget_session`, which removes both;
/// the lifecycle is deliberately NOT touched here (an unsubmitted reservation
/// must survive and submit to the rebuilt engine), which is why
/// `forget_session` itself is not reused.
///
/// The reset happens inside the gated section so a new turn cannot slip in
/// between and rebuild either registry against the new workspace only to have
/// it dropped afterwards.
async fn rebind_evict_with_gates<T, Take, TakeFut, Reclaim, ReclaimFut>(
    turn_locks: &SessionTurnLocks,
    runtime_locks: &SessionTurnLocks,
    shell_managers: &SessionShellManagers,
    turn_shell_tasks: &SessionTurnShellTasks,
    session_id: &str,
    take_entry: Take,
    reclaim: Reclaim,
) -> bool
where
    Take: FnOnce() -> TakeFut,
    TakeFut: Future<Output = Option<T>>,
    Reclaim: FnOnce(T) -> ReclaimFut,
    ReclaimFut: Future<Output = ()>,
{
    let turn_lock = turn_locks.for_session(session_id).await;
    let Ok(_turn) = tokio::time::timeout(REBIND_EVICT_GATE_TIMEOUT, turn_lock.lock()).await else {
        return false;
    };
    let runtime_lock = runtime_locks.for_session(session_id).await;
    let Ok(_runtime) = tokio::time::timeout(REBIND_EVICT_GATE_TIMEOUT, runtime_lock.lock()).await
    else {
        // A spawn in flight holds this lock far longer than a turn gate wait;
        // skipping this round is the honest answer, same as a turn-gate
        // timeout.
        return false;
    };
    let Some(entry) = take_entry().await else {
        return false;
    };
    reclaim(entry).await;
    turn_shell_tasks.remove(session_id);
    shell_managers.remove(session_id);
    true
}

/// Whether the two epoch snapshots still refer to the same turn; used by the
/// cancel path's guard across the turn_lock boundary.
///
/// `(None, None)` (idle→idle) counts as a match: canceling an idle session is
/// a no-op anyway, and going through the original logic has no side effects;
/// `(Some, Some)` matches only when equal; crossing `Some`/`None` or unequal
/// means the target turn has ended and a new turn has been reserved — the
/// cancel must be a whole no-op.
fn generation_matches(target: Option<u64>, current: Option<u64>) -> bool {
    match (target, current) {
        (Some(a), Some(b)) => a == b,
        (None, None) => true,
        _ => false,
    }
}

/// Guard for retrying the cascade cancel: whether `CancelSubAgents` can still
/// be safely resent after a mismatch.
///
/// When the new turn has not been submitted yet (`SendMessage` needs the same
/// `turn_lock`, so while cancel holds the lock the engine still has only the
/// old turn's leftover sub-agents) or when currently idle, the resend cannot
/// hit sub-agents just started by the new turn; once the new turn has been
/// submitted (`submitted=true`) the engine may already have started new-turn
/// sub-agents and a resend would kill them by mistake — it must be skipped.
/// All mismatch discovery points (entry recheck / recheck after the
/// `get_engine` await / arm rejected) use this predicate uniformly, so the
/// resend path also takes effect at the G1 miss point (turn switching after
/// the `get_engine` await).
fn should_retry_cascade(lifecycle: Option<&TurnLifecycle>) -> bool {
    !lifecycle.is_some_and(|lc| lc.is_current_turn_submitted())
}

/// Bounds an await that runs while the caller holds the session turn gate
/// (see [`TURN_GATE_AWAIT_TIMEOUT`], issue #255). On timeout the future is
/// dropped (nothing further is enqueued) and the degradation is logged.
async fn bounded_while_holding_turn_gate<F>(what: &str, fut: F)
where
    F: Future<Output = ()>,
{
    match tokio::time::timeout(TURN_GATE_AWAIT_TIMEOUT, fut).await {
        Ok(()) => {}
        Err(_) => {
            eprintln!(
                "[engine_pool] {what} did not settle within {TURN_GATE_AWAIT_TIMEOUT:?} while holding the turn gate; abandoning it to keep the session gate responsive"
            );
        }
    }
}

/// Outcome of [`bounded_join_while_holding_turn_gate`].
enum BoundedJoinOutcome {
    /// The task finished within the budget.
    Settled,
    /// The task outlived the budget: the dropped join handle detached it and
    /// it keeps running in the background.
    Detached,
    /// The task panicked: nothing keeps running, so bookkeeping that only
    /// the task's normal completion performs must be redone by the caller.
    /// Nothing in the current tree aborts these side-effect tasks (the
    /// timeout path drops the join handle instead), so a `JoinError` here
    /// is always a real panic; if a future abort source appears, match on
    /// [`tokio::task::JoinError::is_cancelled`] before treating it as one.
    Panicked(tokio::task::JoinError),
}

/// Bounds the join of a detached side-effect task while the caller holds the
/// session turn gate (see [`TURN_GATE_AWAIT_TIMEOUT`], issue #255): a slow
/// task must not extend the gate hold. On timeout the join handle is
/// dropped, which detaches the task without aborting it, and the
/// degradation is logged. A panicked task is reported as [`BoundedJoinOutcome::Panicked`]
/// instead of silently counting as settled — a panic means the task will
/// never complete its own bookkeeping. The budget is a parameter so the
/// `Detached` outcome stays reachable from behavior tests without waiting
/// on the real 5s clock; production callers always pass
/// [`TURN_GATE_AWAIT_TIMEOUT`].
async fn bounded_join_while_holding_turn_gate<T>(
    what: &str,
    budget: std::time::Duration,
    task: tokio::task::JoinHandle<T>,
) -> BoundedJoinOutcome {
    match tokio::time::timeout(budget, task).await {
        Ok(Ok(_)) => BoundedJoinOutcome::Settled,
        Ok(Err(join_error)) => {
            eprintln!(
                "[engine_pool] {what} task panicked: {join_error}; treating it as not settled"
            );
            BoundedJoinOutcome::Panicked(join_error)
        }
        Err(_) => {
            eprintln!(
                "[engine_pool] {what} did not settle within {budget:?} while holding the turn gate; letting it finish in the background"
            );
            BoundedJoinOutcome::Detached
        }
    }
}

/// Static name for the shutdown-op diagnostics. The surrounding reclaim logs
/// must not carry the session id (CodeQL flags cleartext session ids in
/// newly added lines): the reclaimed-terminal path logs the session id, but
/// the delete/evict paths currently log none, so a failure line on those
/// paths is attributable only by ordering — a known diagnosability trade.
/// Message payloads are avoided on purpose: `Op`'s `Debug`
/// prints message contents, so a future payload variant sent through this
/// loop would leak them into the log.
fn shutdown_op_name(op: &Op) -> &'static str {
    match op {
        Op::CancelSubAgents => "CancelSubAgents",
        Op::Shutdown => "Shutdown",
        _ => "op",
    }
}

/// Delivers the reclaim shutdown ops in order, each send bounded by
/// [`TURN_GATE_AWAIT_TIMEOUT`] (issue #255): reclaim runs inside the session
/// turn gate, so a stalled engine with a full ops channel must not extend
/// the gate hold. Returns the number of ops delivered in order from the
/// front; the caller re-sends any remainder from a detached task (see
/// [`retry_shutdown_sends`]). A timed-out send is never enqueued (tokio
/// mpsc send is cancel-safe), so skipping ahead cannot duplicate an op.
async fn bounded_shutdown_sends<S, Fut>(mut send: S, ops: impl IntoIterator<Item = Op>) -> usize
where
    S: FnMut(Op) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let mut delivered = 0;
    for op in ops {
        let op_name = shutdown_op_name(&op);
        match tokio::time::timeout(TURN_GATE_AWAIT_TIMEOUT, send(op)).await {
            Ok(Ok(())) => delivered += 1,
            Ok(Err(e)) => {
                eprintln!(
                    "[engine_pool] shutdown {op_name} send failed: {e:#}; abandoning remaining shutdown ops"
                );
                break;
            }
            Err(_) => {
                eprintln!(
                    "[engine_pool] shutdown {op_name} send timed out after {TURN_GATE_AWAIT_TIMEOUT:?} while holding the turn gate; abandoning remaining shutdown ops"
                );
                break;
            }
        }
    }
    delivered
}

/// Retries the shutdown ops that [`bounded_shutdown_sends`] could not
/// deliver while the turn gate was held. Spawns detached (the caller drops
/// the returned join handle) and holds nothing but its own sender clone: it
/// never touches the turn gate, the pool, or the session maps, so it cannot
/// re-block evict/delete. Without this retry a timed-out reclaim would
/// never deliver `Shutdown` at all — the engine owns a `tx_op` clone that
/// keeps its ops channel open, so its run loop only exits through the
/// normal `Shutdown` path and would otherwise linger until process exit
/// (MCP shutdown, subagent flush included). Each send is bounded by
/// `patience`, so a permanently stalled engine stops the retry after at
/// most `pending.len() × patience` instead of waiting forever.
async fn retry_shutdown_sends<S, Fut>(mut send: S, pending: Vec<Op>, patience: std::time::Duration)
where
    S: FnMut(Op) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    for op in pending {
        let op_name = shutdown_op_name(&op);
        match tokio::time::timeout(patience, send(op)).await {
            Ok(Ok(())) => {
                eprintln!(
                    "[engine_pool] shutdown {op_name} retry delivered after the engine drained"
                );
            }
            Ok(Err(e)) => {
                eprintln!(
                    "[engine_pool] shutdown {op_name} retry stopped: engine channel closed ({e:#})"
                );
                break;
            }
            Err(_) => {
                eprintln!(
                    "[engine_pool] shutdown {op_name} retry timed out after {patience:?}; the engine task may linger until process exit"
                );
                break;
            }
        }
    }
}

/// The testable body of the cancel logic, extracted from
/// [`EnginePool::cancel`] so deterministic tests can use bare Default
/// components
/// (`SessionTurnLocks` / `SessionTurnLifecycles` / `SessionTurnShellTasks`) +
/// closure injection, bypassing `Pinvou3Bridge::boot` / `AppHandle` / a real
/// `EngineHandle` (private across crates, not constructible).
///
/// **Two-phase generation guard**: when a cancel is not bound to the turn
/// identity at its initiation, the later-queued of two concurrent cancel
/// requests (C1/C2) would read the "current lifecycle" after `turn_lock` is
/// released (which may already be a new turn) and cancel the new turn
/// indiscriminately. Here the epoch is compared once in each phase:
///
/// - before phase one (the lock-free `get_engine`), snapshot `target` and
///   compare with the immediate `current`;
/// - after phase two (holding `turn_lock`), compare again, and on mismatch
///   early-return before `request_cancel` / `claim_unsubmitted` /
///   `arm_pending_cancel` / `cancel_engine` all run, avoiding canceling the
///   new turn's engine and shell scope by mistake.
///
/// **TOCTOU protection for phase one**: the `get_engine` closure awaits
/// internally (`handle_for` takes the entries lock), and during the await the
/// old turn may end and a new turn may reserve and `reset_cancel_token()`.
/// Therefore the epoch check must be placed **after** `get_engine().await`
/// and **before** `cancel_current` (comparing the initiation-time snapshot
/// `target` with the `current` reread after the await; on mismatch the whole
/// thing is a no-op) — it must not check first, then await, then cancel,
/// otherwise `cancel_current` would hit the new turn's live token, and a
/// mismatch found in phase two could not be withdrawn.
///
/// **arm-order constraint**: `arm_pending_cancel` must run before every
/// `cancel_current` (see the [`TurnLifecycle::arm_pending_cancel`] docs). If
/// cancel ran before arm: cancel hits the old token → the engine
/// `reset_cancel_token()` and a concurrent TurnStarted → the forwarder does
/// not re-issue the cancel because nothing is armed → when arm runs here the
/// `turn_id` already exists and is rejected — the stop request is lost. With
/// arm first, when TurnStarted arrives between the two steps the forwarder
/// can `take_pending_cancel` and replay the cancel (the subsequent cancel is
/// just an idempotent no-op).
///
/// `get_engine` returns the in-pool engine handle (`None` means no engine —
/// take the unsubmitted-claim terminal-state path). `claim_unsubmitted`
/// receives the target epoch and, for an "unsubmitted reservation",
/// atomically checks `turn_epoch == target` inside the lifecycle state lock
/// before claiming the Interrupted terminal state and re-issuing `chat:done`,
/// returning whether it claimed; an epoch mismatch (the turn has switched)
/// must be a no-op and must not claim the new turn (reviewer point 7).
///
/// `arm_pending_cancel_and_cancel` folds "epoch check + arm + synchronous
/// cancel" into one atomic operation inside the same lifecycle state lock
/// critical section: `cancel_current` runs holding the lock, and
/// `reserve_turn` needs the same state lock, so no turn switch can be
/// inserted between "check/arm" and "cancel" (reviewer point 8 — with a
/// standalone arm the lock is already released, and the synchronous call
/// segment up to cancel can still be preempted by a multi-threaded runtime).
/// Returning `false` means the turn switched after the generation recheck
/// passed but before arm (a new turn has been reserved); in that case the
/// cancel closure does not run, and `cancel_current` / `cascade_cancel` must
/// be skipped — otherwise they would hit the live token the new turn has
/// `reset_cancel_token`ed (reviewer point 6).
///
/// `cancel_current` synchronously cancels the engine's current token
/// (idempotent; runs in both phase one and phase two; implementations may
/// best-effort `try_send` a cascade cancel alongside). It receives the
/// same-source turn identity that `arm_pending_cancel_and_cancel` captured
/// under the **same lifecycle state lock** (`Some`): the closure runs inside
/// that critical section and arbitrates turn-bound on that identity, never
/// on a stale view taken outside the lock. The defensive branches without a
/// lifecycle receive `None` and converge with the terminal-closing verdict:
/// disposition only, never fire (issue #254). `cascade_cancel` asynchronously
/// sends the cascade cancel (CancelSubAgents) and is **only awaited in phase
/// two while holding the `turn_lock`**:
/// guaranteeing the cascade cancel is enqueued before the turn gate is
/// released — the next turn's `SendMessage` must wait on the same
/// `turn_lock` (`send_reserved_user_message`), so the cascade cancel is
/// always enqueued before the new turn's message, and FIFO guarantees the
/// engine cancels the old turn's sub-agents before starting the new turn;
/// a late cascade cancel cannot mistakenly kill the new turn's just-started
/// sub-agents (reviewer point 4: an async spawn send loses the enqueue-order
/// guarantee relative to the next turn's SendMessage).
///
/// Every call is bounded by [`TURN_GATE_AWAIT_TIMEOUT`] (issue #255): a
/// stalled engine run loop (ops channel full and never draining) must not
/// hold the turn gate forever, or evict/delete/send all queue on the same
/// lock and the session pipeline wedges as a whole. On timeout this enqueue
/// is abandoned — the only cost is that the old turn's subagents on the
/// stalled engine stay alive until the engine unstalls or is reclaimed (the
/// subagents' own step/time budgets bound them). A late cascade cancel cannot
/// kill a new gate-submitted turn's subagents: the dropped send is guaranteed
/// never enqueued by tokio mpsc send's cancel-safety (the acquired permit is
/// returned when the future is dropped), and every gate-held sender plus the
/// next turn's `SendMessage` are serialized on the same turn gate, while
/// phase-1's `try_send` is covered by the in-lock epoch check under the
/// lifecycle state lock + FIFO — the ordering guarantee does not rely on "the
/// channel staying full". The guarantee covers turns the app submits under
/// the gate; engine-autonomous turns (idle-subagent finishing, goal
/// continuation) start without the gate and are adopted by the forwarder only
/// at `TurnStarted`, a pre-existing window outside this guarantee.
///
/// **Cascade-cancel delivery guard** (reviewer point 9 + G1 resend
/// convergence): phase 1's best-effort `try_send` can fail when the ops
/// channel is full (capacity 32) and is silently ignored — `CancelSubAgents`
/// is never enqueued. If the old turn then ends and a new turn completes
/// `reserve` before phase 2 acquires the turn gate, phase 2 returns directly
/// on a generation mismatch and `cascade_cancel` never runs — the old turn's
/// detached sub-agents keep running. Therefore **all** mismatch discovery
/// points (the entry recheck L330, the recheck after the `get_engine` await,
/// and `arm_pending_cancel_and_cancel` returning false) uniformly decide by
/// [`should_retry_cascade`]: when the new turn has not been submitted yet
/// (only reserved, not sent — `SendMessage` needs the same `turn_lock`, so
/// the engine still has only the old turn's leftover sub-agents) or when
/// currently idle, resend `cascade_cancel` once under the lock — it cannot
/// hit the new turn's sub-agents. The `cascade_queued` flag is no longer
/// needed: `CancelSubAgents` is idempotent, and resending once more after
/// phase 1 already enqueued is a no-op (simplification ③).
///
/// [`should_retry_cascade`]: fn@should_retry_cascade
/// Return of the `cancel_generation` command: the turn cancellation result,
/// letting the frontend `interruptAndSend` decide "whether to wait for a
/// `chat:done` event".
///
/// - `generation`: the epoch of the target turn being cancelled; **None**
///   covers two ambiguous cases — an idle session (no turn to cancel) and a
///   missing target session; callers should treat both uniformly as "no
///   specific turn was cancelled" (`terminal` is then always true and no
///   event wait is needed).
/// - `terminal`: **true** = the target turn's terminal is confirmed and the
///   reserve gate is reopened, so the frontend can send a new message without
///   waiting for events — three cases: the cancel completed the terminal by
///   itself via the unsubmitted-claim path (the claim path's chat:done is
///   emitted before the cancel command returns, so a frontend listener always
///   misses it and only the command's return value can confirm); the target
///   turn's reserve gate is already reopened (terminal closing finished,
///   including the mismatch case where the target turn already ended and a
///   new turn was reserved); or the session is idle.
///   **false** = the cancel took effect but the target turn's terminal is
///   still closing (reserve gate not open); the frontend should wait for the
///   `chat:done` event carrying that `generation`.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CancelOutcome {
    pub generation: Option<u64>,
    pub terminal: bool,
}

/// Returns `(target_generation, claimed_unsubmitted)`:
/// - `target_generation`: the target turn epoch snapshotted when the cancel
///   was initiated (None when idle).
/// - `claimed_unsubmitted`: whether the cancel **completed** the terminal by
///   itself via the unsubmitted-claim path (the claim path's chat:done is
///   emitted before the cancel command returns, so a frontend listener always
///   misses it; the caller must therefore mark the result terminal=true so the
///   frontend skips the event wait).
async fn cancel_turn_with_gates<G, E, EFut, X, F, FFut, C>(
    turn_locks: &SessionTurnLocks,
    turn_lifecycles: &SessionTurnLifecycles,
    turn_shell_tasks: &SessionTurnShellTasks,
    session_id: &str,
    steer_mode: deepseek_tui::core::engine::CancelMode,
    mut get_engine: E,
    mut cancel_current: X,
    mut cascade_cancel: F,
    claim_unsubmitted: C,
) -> (Option<u64>, bool)
where
    E: FnMut() -> EFut,
    EFut: Future<Output = Option<G>>,
    X: FnMut(&G, Option<TurnIdentity>),
    F: FnMut(&G) -> FFut,
    FFut: Future<Output = ()>,
    C: Fn(&Arc<TurnLifecycle>, u64) -> bool,
{
    // Phase one: trigger the cancel_token first, lock-free — only cancel when
    // it is still the same turn as at initiation, otherwise we would hit the
    // new turn's live token that has since been reset_cancel_tokened.
    let target = turn_lifecycles
        .get(session_id)
        .and_then(|lc| lc.current_turn_generation());
    // Simplification ①: the upfront check can be omitted — get_engine has no
    // side effects, and the recheck after the await +
    // arm_pending_cancel_and_cancel's in-lock atomic check already close the
    // same TOCTOU window; the upfront check only costs one extra entries lock
    // read.
    if let Some(engine) = get_engine().await {
        // get_engine awaits internally (handle_for takes the entries lock),
        // and the turn may switch during the await: the epoch must be
        // re-validated after the await and before the cancel (TOCTOU).
        if generation_matches(
            target,
            turn_lifecycles
                .get(session_id)
                .and_then(|lc| lc.current_turn_generation()),
        ) {
            // Arm first, then cancel, both atomically inside the same state
            // lock critical section: arm_pending_cancel_and_cancel checks
            // turn_epoch == target under the lock, sets pending (when the
            // conditions hold), and executes cancel_current — reserve_turn
            // needs the same state lock, so no turn switch can be inserted
            // between "check/arm" and "cancel", and the old cancel cannot hit
            // the new turn's reset_cancel_tokened live token (reviewer point
            // 8). On epoch mismatch it returns false without running the
            // cancel closure, and phase two's locked generation recheck makes
            // the whole thing a no-op (reviewer point 6).
            if let Some(lifecycle) = turn_lifecycles.get(session_id) {
                lifecycle.arm_pending_cancel_and_cancel(
                    target.unwrap_or(0),
                    steer_mode,
                    |identity| cancel_current(&engine, identity),
                );
            } else {
                cancel_current(&engine, None);
            }
        }
    }

    // Phase two: clean up turn state and shell tasks under the lock.
    let turn_lock = turn_locks.for_session(session_id).await;
    let _turn = turn_lock.lock().await;

    let lifecycle = turn_lifecycles.get(session_id);
    let current = lifecycle
        .as_ref()
        .and_then(|lc| lc.current_turn_generation());
    if !generation_matches(target, current) {
        // The target turn has ended (a new turn has been reserved): whole
        // no-op. This early-return must precede request_cancel /
        // claim_unsubmitted / cancel_engine — request_cancel cancels the
        // currently active shell scope, and if it is already a new turn it
        // would wrongly clean up the new turn's shell tasks.
        //
        // Exception (reviewer point 9 + G1): if phase 1's best-effort
        // try_send was not delivered because the ops channel was full, the
        // old turn's detached sub-agents were not canceled. If at this point
        // the new turn has not been submitted yet (only reserved, not sent:
        // SendMessage needs the same turn_lock, held by this function, so the
        // engine still has only the old turn's leftover sub-agents) or it is
        // currently idle, resending the cascade cancel once under the lock is
        // safe — it cannot hit the new turn's just-started sub-agents.
        // Idempotent: resending after phase 1 already enqueued is a no-op
        // (simplification ③, no cascade_queued flag needed).
        if should_retry_cascade(lifecycle.as_deref()) {
            if let Some(engine) = get_engine().await {
                bounded_while_holding_turn_gate("cascade subagent cancel", cascade_cancel(&engine))
                    .await;
            }
        }
        // The target turn already ended (its terminal was emitted
        // elsewhere): the caller derives terminal=true from comparing target
        // with current (no frontend event wait needed).
        return (target, false);
    }

    // generation matches, the target turn is still the one from initiation:
    // clean up its shell tasks.
    let shell_cancellation = turn_shell_tasks.request_cancel(session_id);
    // claim_unsubmitted atomically checks against the target epoch inside the
    // lifecycle state lock before claiming: between this line and the
    // generation recheck above another worker may already have ended the old
    // turn and reserved a new one (reserve_turn does not take the turn_lock),
    // so on an epoch mismatch the claim must be a no-op and must not misclaim
    // the new turn's reservation as Interrupted (reviewer point 7).
    let claimed_unsubmitted = lifecycle
        .as_ref()
        .is_some_and(|lc| claim_unsubmitted(lc, target.unwrap_or(0)));
    if !claimed_unsubmitted {
        // Recheck the engine under the lock: phase one may have failed to get
        // it because a send was spawning.
        // Idempotent: if phase one already canceled, another cancel is a
        // no-op; if phase one did not cancel, this one fills in.
        // Same as phase one: re-validate the epoch after get_engine's await,
        // then arm first and cancel after.
        if let Some(engine) = get_engine().await {
            if generation_matches(
                target,
                turn_lifecycles
                    .get(session_id)
                    .and_then(|lc| lc.current_turn_generation()),
            ) {
                if let Some(lifecycle) = lifecycle.as_ref() {
                    // Arm with the target snapshotted at initiation
                    // (already re-checked to still be the target turn).
                    // Forwarder-side consumption is gated solely by the
                    // submission token echo (no epoch gate: an autonomous
                    // lifecycle inside the submit→TurnStarted window may
                    // legitimately advance the epoch before the target's own
                    // echo arrives — see `take_pending_cancel`), so a stale
                    // pending never leaks across turns. arm + cancel_current
                    // complete atomically under the state lock (same as
                    // phase one, reviewer point 8): reserve_turn needs the
                    // same state lock and cannot interleave a turn switch
                    // between "verify/arm" and "cancel". Returning false =
                    // the turn already switched after the re-check passed:
                    // skip cancel and the cascade side effects to avoid
                    // hitting the new turn's live token (reviewer point 6).
                    let armed = lifecycle.arm_pending_cancel_and_cancel(
                        target.unwrap_or(0),
                        steer_mode,
                        |identity| cancel_current(&engine, identity),
                    );
                    if armed {
                        // The cascade cancel must finish enqueueing before
                        // the turn gate is released (reviewer point 4): the
                        // next turn's SendMessage needs the same turn_lock,
                        // so the cascade cancel is always enqueued first, and
                        // FIFO guarantees the engine cancels the old turn's
                        // sub-agents before starting the new turn.
                        bounded_while_holding_turn_gate(
                            "cascade subagent cancel",
                            cascade_cancel(&engine),
                        )
                        .await;
                    } else if should_retry_cascade(Some(lifecycle.as_ref())) {
                        // G1 miss point: arm rejected = the turn switched
                        // after the recheck passed. If phase-1's best-effort
                        // try_send was not delivered (channel full) and the
                        // new turn has not been submitted yet (the engine
                        // still has only the old turn's leftover sub-agents),
                        // resend the cascade cancel so the old turn's
                        // detached sub-agents do not keep running (same
                        // predicate as the entry mismatch branch, see
                        // should_retry_cascade).
                        bounded_while_holding_turn_gate(
                            "cascade subagent cancel",
                            cascade_cancel(&engine),
                        )
                        .await;
                    }
                    // Rejected: the turn has switched; the new turn's
                    // engine / sub-agents must not be canceled.
                } else {
                    cancel_current(&engine, None);
                    // When the lifecycle is missing (no active turn) the cascade is
                    // meaningless: the cascade cancel CancelSubAgents targets the engine's
                    // current sub-agents, an idle engine has no active sub-agents, and not
                    // sending is lossless (original behavior kept).
                }
            } else if should_retry_cascade(lifecycle.as_deref()) {
                // G1 miss point: the recheck after the get_engine await
                // mismatches (T2 reserved during the await). If phase-1's
                // try_send was not delivered and the new turn has not been
                // submitted yet, resend the cascade cancel — the entry
                // mismatch branch's resend is not evaluated at this discovery
                // point, so it must be added separately (the same class of
                // window as reviewer point 9).
                bounded_while_holding_turn_gate("cascade subagent cancel", cascade_cancel(&engine))
                    .await;
            }
        }
    }
    if let Some(cancellation) = shell_cancellation {
        // Killing the shells is side-effect-safe to finish later (the scope
        // belongs to the ended turn and the registry worker keeps sweeping
        // pending kills), so the cleanup runs detached and only the join is
        // bounded (issue #255): a slow cleanup must not extend the time the
        // turn gate is held. Dropping the join handle detaches the task.
        // A panicked result needs no re-finalize here, unlike the reclaim
        // site: the worker keeps sweeping pending kills and the still-
        // running forwarder retires the scope.
        let cleanup = tokio::spawn(async move { cancellation.cleanup().await });
        bounded_join_while_holding_turn_gate("shell cleanup", TURN_GATE_AWAIT_TIMEOUT, cleanup)
            .await;
    }
    (target, claimed_unsubmitted)
}

/// get_or_spawn's staleness predicate: either a runtime model change
/// (`requires_rebuild_from`) or an mcp config revision bump
/// (`mark_mcp_config_updated`) means the next turn must rebuild safely.
/// Extracted as a pure function: pins the behavior at the same layer as
/// `requires_rebuild_from` without needing a real engine.
fn entry_is_fresh(
    requires_model_rebuild: bool,
    entry_mcp_config_revision: u64,
    current_mcp_config_revision: u64,
) -> bool {
    !requires_model_rebuild && entry_mcp_config_revision == current_mcp_config_revision
}

/// A session's resident entry in the pool: the engine + its own event
/// forwarder task.
struct EngineEntry {
    engine: AppEngine,
    /// This engine's event forwarder; aborted on evict so a zombie task does
    /// not keep emitting.
    forwarder: JoinHandle<()>,
    /// The runtime model, provider version, and local model revision actually
    /// used when this engine was created.
    runtime_model: PreparedRuntimeState,
    /// MCP config revision. Changers of mcp.json (the marketplace
    /// install/uninstall/import/trash-restore commands) bump it via
    /// `mark_mcp_config_updated`; the next engine fetch safely reclaims the old
    /// instance and lazily rebuilds. A plain session's engine reads the
    /// session-derived mcp config (rewritten from the global mcp.json only at
    /// spawn); without a rebuild, servers installed midway would never be
    /// visible to the live engine.
    mcp_config_revision: u64,
    /// Engine epoch (UNIX ms): on the worker ledger, a "still running" record
    /// counts as truly alive only if it had activity within this epoch. The
    /// foundation's restart-load only flips in-memory state and does not
    /// rewrite the persisted running flag (the load path in subagent/mod.rs);
    /// without this vetting, after the parent session rebuilds its engine the
    /// previous process's zombie workers would show "working" again and be
    /// polled forever.
    spawned_at_ms: u64,
    /// Process-monotonic engine incarnation number (the steer-id generation
    /// source). The generation stamp disambiguates steer ids across engine
    /// rebuilds of one session and must be unique per rebuild within the
    /// process; `spawned_at_ms` is wall-clock milliseconds, so two rebuilds
    /// inside the same tick (or a clock rollback) collide and defeat the
    /// equality check (zhuowp re-review P1-2) — the incarnation sequence is
    /// time-independent and never repeats. Used only for steer-id stamping
    /// and matching, not for idle-reap timing.
    steer_incarnation: u64,
    /// The most recent turn-submit-side activity (UNIX ms): refreshed by
    /// spawn and by turn submissions (send / edit-resend / scheduled-turn
    /// entry). The turn terminal-closing clock lives in TurnLifecycle
    /// (`last_terminal_epoch_ms`, refreshed uniformly at the shared closing
    /// point of all terminal paths); idle reclaim takes the newer of the two
    /// to judge the true idle duration — a long turn that just ended must not
    /// be judged reclaimable immediately.
    /// Idle reclaim goes by this (rather than spawned_at_ms) — an engine
    /// just reclaimed and rebuilt must also be reclaimable again if it stays
    /// unused.
    last_active_epoch_ms: AtomicU64,
}

#[derive(Clone, Default)]
struct ModelUpdateRevisions {
    revisions: Arc<SyncMutex<HashMap<String, u64>>>,
}

impl ModelUpdateRevisions {
    fn current(&self, model_id: &str) -> u64 {
        self.revisions.lock().get(model_id).copied().unwrap_or(0)
    }

    fn bump(&self, model_id: &str) -> u64 {
        let mut revisions = self.revisions.lock();
        let revision = revisions.entry(model_id.to_string()).or_default();
        *revision = revision.saturating_add(1);
        *revision
    }
}

#[derive(Clone, PartialEq, Eq)]
struct PreparedRuntimeState {
    prepared: PreparedRuntimeModel,
    model_update_revision: u64,
}

impl PreparedRuntimeState {
    fn new(prepared: PreparedRuntimeModel, model_update_revision: u64) -> Self {
        Self {
            prepared,
            model_update_revision,
        }
    }

    fn requires_rebuild_from(&self, previous: &Self) -> bool {
        self != previous
    }
}

pub type EngineToolFactory =
    Arc<dyn Fn(&AppHandle, &str) -> Vec<Arc<dyn ToolSpec>> + Send + Sync + 'static>;
pub type ToolPolicy = Arc<dyn Fn(&AppHandle) -> Vec<String> + Send + Sync + 'static>;

/// The multi-session engine pool. Held as Tauri State; `Clone` is cheap
/// (everything inside is an Arc).
#[derive(Clone)]
pub struct EnginePool {
    entries: Arc<Mutex<HashMap<String, EngineEntry>>>,
    runtime_model_locks: SessionTurnLocks,
    model_update_revisions: ModelUpdateRevisions,
    mcp_config_revision: Arc<AtomicU64>,
    #[cfg(any(feature = "benchmark-hooks", test))]
    eval_model_snapshots: EvalModelSnapshots,
    turn_locks: SessionTurnLocks,
    turn_lifecycles: SessionTurnLifecycles,
    shell_managers: SessionShellManagers,
    turn_shell_tasks: SessionTurnShellTasks,
    app: AppHandle,
    store: SessionStore,
    tool_factory: EngineToolFactory,
    tool_policy: ToolPolicy,
    /// All sessions share one booted bridge (boot writes to disk / sets env,
    /// so it can happen only once). Commands reading model / workspace also go
    /// through here.
    pub bridge: Pinvou3Bridge,
    /// Idle-reclaim sweep task handle. Kept in an Arc shared by all pool
    /// clones: the pool itself is Clone (Tauri State hands out a clone per
    /// command), so it cannot impl Drop directly — any clone dropping would
    /// wrongly stop the sweep; the last clone dropping also drops the guard.
    idle_reaper: Arc<SyncMutex<Option<IdleReaperGuard>>>,
    /// The set of sessions currently running a scheduled turn (in
    /// run_scheduled_turn's spawn→submit window the lifecycle is not yet
    /// active; idle reclaim needs this as a second layer of protection).
    scheduled_running_sessions: Arc<SyncMutex<HashSet<String>>>,
    /// Steer-id engine-incarnation allocator: a process-monotonic AtomicU64
    /// sequence bumped on every engine spawn. Arc-shared so pool clones see
    /// one sequence (same idiom as every shared field here — EnginePool is a
    /// cheap Tauri State clone). Replaces wall-clock `spawned_at_ms` as the
    /// generation stamp source so that same-tick rebuilds (or clock
    /// rollbacks) can never collide generations (zhuowp re-review P1-2).
    steer_incarnation_seq: Arc<AtomicU64>,
    /// Rewind mutual-exclusion flags keyed by canonical execution root (a
    /// true value = that directory is mid-rewind/rollback).
    /// Shadow repositories are per-session dirs, so a same-root session's
    /// restore (checkout-index + clean) and an in-flight turn's file writes
    /// are unaware of each other — they cannot even collide on index.lock.
    /// Turn reservations (reserve_turn / run_scheduled_turn's in-flight
    /// registration) check inside this flag's lock, and the rewind side
    /// rechecks in-flight peers after setting it: both sides complete their
    /// own check-and-act on the same lock, eliminating the race.
    /// Known trade-off: entries only grow (one Arc<Mutex<bool>> per ever-bound
    /// execution root — byte-sized, no pressure at session scale); the flag
    /// itself is a "set/check" signal, not an exclusion lock — the real
    /// mutual exclusion is jointly guaranteed by the rewinder's session
    /// reservation + the in-flight peer recheck; when changing this
    /// structure, do not keep only the flag and lose the reservation.
    execution_root_rewind_flags: Arc<SyncMutex<HashMap<std::path::PathBuf, Arc<SyncMutex<bool>>>>>,
}

/// The holding token of an execution-root rewind exclusion: Drop releases it
/// automatically (including the panic/error early-exit path).
pub(crate) struct ExecutionRootRewindGuard {
    flag: Arc<SyncMutex<bool>>,
}

impl Drop for ExecutionRootRewindGuard {
    fn drop(&mut self) {
        *self.flag.lock() = false;
    }
}

// `IdleReaperGuard` (on Drop it cancels first, then aborts — a double
// safeguard for stopping the sweep) and the sweep loop converge into
// `core::reaper`, sharing one implementation with the ACP side.

/// M-7: a steer must have a live engine to deliver to. When the engine is
/// absent (the session is not running) return Err — otherwise no
/// steer_committed/steer_dropped event would ever arrive and the frontend's
/// queued chip hangs forever. Extracted into a function so the "absent → Err"
/// contract can be covered by unit tests without an AppHandle (EnginePool
/// construction depends on AppHandle and bridge boot).
fn require_live_engine_for_steer<T>(engine: Option<T>, session_id: &str) -> Result<T> {
    engine.with_context(|| format!("no live engine for session '{session_id}' to steer"))
}

/// Steer ids are generation-scoped at the pool boundary. The foundation mints
/// per-handle ordinal ids (`steer-{n}`) whose counter resets whenever an
/// engine is rebuilt (idle reclaim / model switch), so a raw id is ambiguous
/// across engine generations of one session: an old chip's withdrawal could
/// retire the NEW engine's unrelated `steer-1`, and a settlement event could
/// cross-wire two chips. The pool stamps a process-monotonic engine
/// incarnation (an AtomicU64 sequence allocated at spawn — NOT wall-clock
/// time, which collides for same-tick rebuilds or clock rollbacks, zhuowp
/// re-review P1-2) onto every id it returns, and the forwarder stamps the
/// `SteerCommitted`/`SteerDropped` payloads with the same incarnation,
/// keeping chip↔event correlation exact and withdrawals
/// generation-checked. The stamp is opaque to the frontend.
pub(crate) fn stamp_steer_generation(generation: u64, raw_steer_id: &str) -> String {
    format!("e{generation}-{raw_steer_id}")
}

/// Split a stamped id into `(generation, raw)`. `None` = not stamped (legacy
/// or foreign id) — callers pass it through untranslated.
fn parse_steer_generation(steer_id: &str) -> Option<(u64, &str)> {
    let rest = steer_id.strip_prefix('e')?;
    let dash = rest.find('-')?;
    let generation = rest.get(..dash)?.parse::<u64>().ok()?;
    Some((generation, rest.get(dash + 1..)?))
}

/// Pure withdrawal-delegation decision for a (possibly stamped) steer id
/// against the live engine's incarnation. Kept standalone so the collision
/// semantics are unit-testable without an AppHandle-backed pool.
#[derive(Debug, PartialEq, Eq)]
enum SteerWithdrawTarget<'a> {
    /// Withdraw this id on the live engine (current incarnation, or an
    /// unstamped legacy id delegated verbatim).
    Raw(&'a str),
    /// The id belongs to a previous engine incarnation — never delegate it to
    /// the live engine (see `withdraw_steer`).
    StaleGeneration,
}

fn delegate_steer_withdrawal(steer_id: &str, current_generation: u64) -> SteerWithdrawTarget<'_> {
    match parse_steer_generation(steer_id) {
        Some((stamped, raw)) if stamped == current_generation => SteerWithdrawTarget::Raw(raw),
        Some(_) => SteerWithdrawTarget::StaleGeneration,
        None => SteerWithdrawTarget::Raw(steer_id),
    }
}

impl EnginePool {
    pub fn new_with_dependencies(
        app: AppHandle,
        store: SessionStore,
        tool_factory: EngineToolFactory,
        tool_policy: ToolPolicy,
    ) -> Result<Self> {
        let bridge = Pinvou3Bridge::boot()?;
        Ok(Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            runtime_model_locks: SessionTurnLocks::default(),
            model_update_revisions: ModelUpdateRevisions::default(),
            mcp_config_revision: Arc::new(AtomicU64::new(0)),
            #[cfg(any(feature = "benchmark-hooks", test))]
            eval_model_snapshots: EvalModelSnapshots::default(),
            turn_locks: SessionTurnLocks::default(),
            turn_lifecycles: SessionTurnLifecycles::default(),
            shell_managers: SessionShellManagers::default(),
            turn_shell_tasks: SessionTurnShellTasks::default(),
            app,
            store,
            tool_factory,
            tool_policy,
            bridge,
            idle_reaper: Arc::new(SyncMutex::new(None)),
            scheduled_running_sessions: Arc::new(SyncMutex::new(HashSet::new())),
            steer_incarnation_seq: Arc::new(AtomicU64::new(0)),
            execution_root_rewind_flags: Arc::new(SyncMutex::new(HashMap::new())),
        })
    }

    /// Start the idle-reclaim sweep (idempotent): scan every
    /// REAP_INTERVAL_SECS seconds and reclaim engines that are "idle beyond
    /// IDLE_EVICT_AFTER_SECS with no in-flight turn and not active". Active
    /// is judged by `SessionStore::active_id` (the frontend's current
    /// session, maintained by the create/load session commands); in
    /// addition, reserve_turn (the mandatory precursor of a turn) refreshes
    /// last_active, and an engine whose turn is not yet reserved is
    /// genuinely idle — with this double safeguard, err on the conservative side.
    /// Reclaim reuses the delete path's `reclaim_engine_entry` (cascade-cancel
    /// the sub-agents first, then Shutdown) and returns to lazy-spawn
    /// semantics.
    ///
    /// The sweep task holds a pool clone (all Arc inside, cheap). The pool is
    /// Tauri managed state with a process-level lifetime, so the sweep
    /// naturally ends when the process exits; the guard's Drop cleanup is
    /// only defensive (the Arc cycle between clones means it does not
    /// normally trigger — and that is not a leak: what stays resident is only
    /// a light task waking every 5 minutes).
    pub fn start_idle_reaper(&self) {
        let mut slot = self.idle_reaper.lock();
        crate::core::reaper::start_idle_reaper(
            &mut slot,
            self.clone(),
            |pool| async move { pool.reap_idle_engines().await },
            "engine_pool",
        );
    }

    /// One round of idle reclaim. The decision first takes a read-only
    /// snapshot (entries / lifecycle / active id) to pick candidates; the
    /// actual reclaim goes through `evict_if_idle`: after acquiring the turn
    /// gate + runtime lock it rechecks that the session is still idle (a new
    /// turn may have been reserved / submitted between the snapshot and the
    /// lock — reserve_turn does not take the gate); if the recheck fails it
    /// skips and leaves it to the next sweep.
    async fn reap_idle_engines(&self) {
        let active_id = self.store.active_id();
        let now = Self::now_epoch_ms();
        let candidates: Vec<(String, u64)> = {
            let entries = self.entries.lock().await;
            entries
                .iter()
                .filter(|(sid, entry)| {
                    let last_active = self.last_activity_ms(sid, entry);
                    let idle_for_secs = now.saturating_sub(last_active) / 1000;
                    should_reap_idle_engine(
                        self.is_turn_active(sid),
                        self.scheduled_running_sessions.lock().contains(*sid),
                        active_id.as_deref() == Some(sid.as_str()),
                        idle_for_secs,
                    )
                })
                .map(|(sid, entry)| (sid.clone(), self.last_activity_ms(sid, entry)))
                .collect()
        };
        for (session_id, snapshot_last_active) in candidates {
            if self.evict_if_idle(&session_id, snapshot_last_active).await {
                eprintln!(
                    "[engine_pool] 会话空闲超过 {IDLE_EVICT_AFTER_SECS} 秒，回收 engine（下次发消息时 lazy 重建）"
                );
            }
        }
    }

    /// The session's last activity time (UNIX ms): the newer of the engine
    /// entry's spawn / turn-submit clock and the turn terminal-closing clock
    /// (TurnLifecycle, refreshed uniformly at the closing point shared by all
    /// terminal paths). When the lifecycle does not exist (a turn has never
    /// spawned) only the entry clock is available.
    fn last_activity_ms(&self, session_id: &str, entry: &EngineEntry) -> u64 {
        let submitted_side = entry.last_active_epoch_ms.load(Ordering::Acquire);
        let terminal_side = self
            .turn_lifecycles
            .get(session_id)
            .map(|lifecycle| lifecycle.last_terminal_epoch_ms())
            .unwrap_or(0);
        submitted_side.max(terminal_side)
    }

    /// Called after a model config or user-hosted credential save succeeds.
    /// Only bumps the non-sensitive in-memory revision; already-spawned engines
    /// are not interrupted, and the next turn safely reclaims and rebuilds
    /// before sending.
    pub(crate) fn mark_model_updated(&self, model_id: &str) {
        self.model_update_revisions.bump(model_id);
    }

    /// Called after an mcp.json atomic update succeeds. Does not interrupt an
    /// in-progress turn; on the next entry into `get_or_spawn` the revision
    /// difference is detected and the engine is safely rebuilt, rediscovering
    /// tools from the new config.
    pub(crate) fn mark_mcp_config_updated(&self) {
        self.mcp_config_revision.fetch_add(1, Ordering::AcqRel);
    }

    pub fn compute_disallowed_tools(&self) -> Vec<String> {
        (self.tool_policy)(&self.app)
    }

    pub async fn refresh_disallowed_tools(&self) -> Vec<String> {
        let tools = self.compute_disallowed_tools();
        self.set_disallowed_all(tools.clone()).await;
        tools
    }

    /// Project-skills source root for the session: returns the bound real
    /// directory only when the session is actually bound (a native code
    /// session's project directory, or a plain chat session's user workspace
    /// binding — the explicit `SessionRoots::bound` signal); unbound or
    /// resolution failure -> None (project-level skills stay out of play).
    fn project_workspace_for(&self, session_id: &str) -> Option<std::path::PathBuf> {
        self.store
            .session_roots(session_id)
            .ok()
            .and_then(|roots| roots.bound.then_some(roots.execution))
    }

    /// Skill dual-scope governance: event-driven **incremental rewrite** of
    /// every online session's composed directory (called after the skill
    /// toggle / install / uninstall commands persist, §2.3.2). Each session
    /// computes its enabled set by its own scope and only adds/removes the
    /// delta (diff idempotent); the foundation rescans each turn, so the next
    /// turn's prompt takes effect. Offline sessions are left alone (the next
    /// spawn composes fully, §2.3.1).
    pub async fn refresh_live_sessions_skills(&self) {
        let sids: Vec<String> = {
            let entries = self.entries.lock().await;
            entries.keys().cloned().collect()
        };
        for sid in sids {
            let scope = self.bridge.session_policy(&sid).mode();
            let project_workspace = self.project_workspace_for(&sid);
            let _ = tokio::task::spawn_blocking(move || {
                crate::features::assistant::skill_materialization::rewrite_session_skills(
                    &sid,
                    scope,
                    project_workspace.as_deref(),
                );
            })
            .await;
        }
    }

    /// Builds a bridge for the session on behalf of a standalone caller. Shares the
    /// EnginePool lazy-spawn runtime-model resolution (`prepare_runtime_model`) so
    /// bypass entry points such as review get consistent model-routing behavior with
    /// the formal spawn. Bypass entries deliberately take the interactive routing
    /// policy (`scheduled_unattended = false`); the scheduled organizer is the one
    /// unattended caller and intentionally mirrors the interactive path.
    pub(crate) async fn fresh_bridge_for(&self, session_id: &str) -> Result<Pinvou3Bridge> {
        #[cfg(any(feature = "benchmark-hooks", test))]
        let eval_model = self.eval_model_snapshots.for_session(session_id);
        #[cfg(not(any(feature = "benchmark-hooks", test)))]
        let eval_model = None;
        let (bridge, prepared, pins_scheduled_model) = self
            .prepare_runtime_model(session_id, false, eval_model)
            .await?;
        Ok(Self::finalize_runtime_bridge(bridge, &prepared, pins_scheduled_model).await)
    }

    async fn prepare_runtime_model(
        &self,
        session_id: &str,
        scheduled_unattended: bool,
        explicit_model_override: Option<SavedModel>,
    ) -> Result<(Pinvou3Bridge, PreparedRuntimeModel, bool)> {
        let mut bridge = self.bridge.clone();
        bridge.prefs = UserPrefs::load();
        let scheduled_profile = self.store.scheduled_profile(session_id);
        // Same caliber as the command layer chat.rs's is_scheduled (a
        // scheduled_profile existing is enough): a scheduled session's images
        // always take the image_analyze hard rule, even with an interactive
        // model override, so the always flag must not use the narrower
        // pins_scheduled_model.
        bridge.image_analyze_always = scheduled_profile.is_some();
        let interactive_model_override = self.store.session_model_override(session_id);
        let pins_scheduled_model = scheduled_profile.is_some()
            && (scheduled_unattended || interactive_model_override.is_none());
        bridge.session_model = resolve_runtime_model_override(explicit_model_override, || {
            resolve_spawn_model(
                &bridge.prefs.advanced.saved_models,
                scheduled_profile.as_ref(),
                interactive_model_override.as_deref(),
                scheduled_unattended,
            )
        })?;
        // The community default preparation path is a fixed passthrough: the model
        // is kept as-is, with no runtime credential/revision injection; credentials
        // still go through environment variables and the local credential store
        // (bridge.api_key()).
        let selected = bridge
            .effective_model_owned()
            .context("No effective model is available for runtime preparation")?;
        let prepared = PreparedRuntimeModel::unchanged(selected);
        Ok((bridge, prepared, pins_scheduled_model))
    }

    /// No `&self`: this function does not read pool state, it only
    /// orchestrates the spawn finish. Making it an associated function lets
    /// unit tests drive the real injection block directly (a real EnginePool
    /// cannot be constructed in unit tests; wiring coverage lives in
    /// `probed_facts_wiring_tests`).
    async fn finalize_runtime_bridge(
        mut bridge: Pinvou3Bridge,
        prepared: &PreparedRuntimeModel,
        pins_scheduled_model: bool,
    ) -> Pinvou3Bridge {
        bridge.session_model = Some(prepared.model.clone());
        // Local endpoints (OpenAI-compatible presets pointing at local/intranet
        // services): probe the service type (Ollama / vLLM / LM Studio /
        // generic) so thinking control follows the corresponding foundation
        // wire protocol.
        // A probe failure (service not started/timeout/auth failure) is
        // classified as generic, keeping the existing openai wire route. The
        // probe request carries a credential from the same origin as real
        // inference (bridge.api_key()): authenticated vLLM (--api-key) 401s
        // on /v1/models without credentials, and misclassifying it as a
        // generic endpoint loses default-off thinking and the vLLM tiers
        // (inference itself still succeeds with the configured key).
        if bridge.provider() == "openai" && base_url_uses_local_or_private(&bridge.base_url()) {
            let api_key = bridge.api_key();
            bridge.probed_local_kind = Some(
                crate::core::model_endpoint::probe_local_server_kind(
                    &bridge.base_url(),
                    Some(api_key.as_str()),
                )
                .await,
            );
        }
        // Operator-owned routes (local vLLM + custom OpenAI-compatible /
        // custom, see `SavedModel::is_operator_owned_endpoint`) probe
        // `/v1/models` once at spawn, bringing back the matched entry's own
        // context window and self-reported output limit for
        // `route_limits_for_model` to min-tighten (on probe failure both are
        // None, falling back to configured values / window tiers). vLLM
        // routes additionally correct the served name
        // (resolve_served_model): a configured name that the server lists
        // must be kept verbatim — LM Studio/Ollama list every downloaded
        // model, the first entry is unrelated to the user's pick, and
        // substituting it is exactly the reported "conversation names A,
        // engine loads B" chain break; only follow the served name on a
        // single-model server that does not expose the configured name.
        // Non-vLLM operator-owned routes do no name correction and adopt
        // facts only when the configured name exactly hits the list
        // (`adopts_probed_facts`) — a single-entry "borrowed name" returns
        // facts belonging to another model and must not be misattributed.
        // Cloud presets and coding_plan are not operator-owned and are not
        // probed.
        let is_vllm_route = bridge.provider() == "vllm";
        if let Some(model) = bridge.effective_model_owned() {
            Self::adopt_probed_endpoint_facts(
                &mut bridge,
                model,
                is_vllm_route,
                pins_scheduled_model,
            )
            .await;
        }
        bridge
    }

    /// Spawn-time probe and fact adoption (the testable core of
    /// `finalize_runtime_bridge`; wiring unit tests live in
    /// `probed_facts_wiring_tests` at the end of this file): operator-owned
    /// routes (or vLLM routes) probe `/v1/models`, and vLLM additionally
    /// corrects the served name (keeping the configured name when
    /// `pins_scheduled_model`); whether facts are adopted is decided by
    /// `adopts_probed_facts`, and on adoption both `probed_context_tokens`
    /// and `probed_output_tokens` are written. On probe failure (endpoint
    /// unreachable / name not matched) both facts are None and the route
    /// falls back to configured values / window tiers.
    async fn adopt_probed_endpoint_facts(
        bridge: &mut Pinvou3Bridge,
        mut model: SavedModel,
        is_vllm_route: bool,
        pins_scheduled_model: bool,
    ) {
        if !(is_vllm_route || model.is_operator_owned_endpoint()) {
            return;
        }
        // The probe carries the same credential as real inference
        // (authenticated endpoints 401 on `/v1/models` without credentials;
        // on probe failure the configured values are kept).
        let api_key = bridge.api_key();
        let (served, max_len, max_output) = crate::features::monitor::resolve_served_model(
            &bridge.base_url(),
            Some(api_key.as_str()),
            &model.model,
        )
        .await;
        let adopts =
            crate::features::monitor::adopts_probed_facts(is_vllm_route, &model.model, &served);
        if is_vllm_route && served != model.model && !pins_scheduled_model {
            model.model = served;
            bridge.session_model = Some(model);
        }
        if adopts {
            bridge.probed_context_tokens = max_len;
            bridge.probed_output_tokens = max_output;
        }
    }

    /// Get the session's engine, spawning one if absent. After spawn, if the
    /// session has on-disk history, hydrate the historical messages into the
    /// new engine with a one-shot `SyncSession` (the scenario of a cold start
    /// / reopening an old session after an app restart and sending a message).
    pub async fn get_or_spawn(&self, session_id: &str) -> Result<AppEngine> {
        #[cfg(any(feature = "benchmark-hooks", test))]
        let eval_model = self.eval_model_snapshots.for_session(session_id);
        #[cfg(not(any(feature = "benchmark-hooks", test)))]
        let eval_model = None;
        self.get_or_spawn_with_policy(session_id, false, eval_model)
            .await
    }

    /// Spawn policy for an unattended automation turn is deliberately distinct
    /// from an interactive continuation: the task profile remains authoritative
    /// even if the user temporarily selected another model while viewing it.
    async fn get_or_spawn_with_policy(
        &self,
        session_id: &str,
        scheduled_unattended: bool,
        explicit_model_override: Option<SavedModel>,
    ) -> Result<AppEngine> {
        let runtime_lock = self.runtime_model_locks.for_session(session_id).await;
        let _runtime = runtime_lock.lock().await;
        let (bridge, prepared, pins_scheduled_model) = self
            .prepare_runtime_model(session_id, scheduled_unattended, explicit_model_override)
            .await?;
        let model_update_revision = self.model_update_revisions.current(&prepared.model.id);
        let prepared = PreparedRuntimeState::new(prepared, model_update_revision);
        let mcp_config_revision = self.mcp_config_revision.load(Ordering::Acquire);

        let stale = {
            let mut entries = self.entries.lock().await;
            if let Some(entry) = entries.get(session_id) {
                if entry_is_fresh(
                    prepared.requires_rebuild_from(&entry.runtime_model),
                    entry.mcp_config_revision,
                    mcp_config_revision,
                ) {
                    return Ok(entry.engine.clone());
                }
            }
            entries.remove(session_id)
        };
        if let Some(entry) = stale {
            self.reclaim_engine_entry(session_id, entry).await;
        }

        let is_scheduled = self.store.scheduled_profile(session_id).is_some();
        let bridge =
            Self::finalize_runtime_bridge(bridge, &prepared.prepared, pins_scheduled_model).await;
        // The shell execution directory and the engine cwd share one source:
        // resolved uniformly via SessionStore::session_roots
        // (scheduled = automation workspace, a native code project-bound
        // session = the project directory).
        // On resolution failure (e.g. a scheduled session missing its
        // profile) keep the original fallback: bridge-side resolution.
        let shell_workspace = self
            .store
            .session_roots(session_id)
            .map(|roots| roots.execution)
            .unwrap_or_else(|_| bridge.session_workspace(session_id));
        let shell_manager = self.shell_managers.for_session(session_id, shell_workspace);
        let turn_shell_tasks = self
            .turn_shell_tasks
            .for_session(session_id, shell_manager.clone());
        let mut extra_tools = (self.tool_factory)(&self.app, session_id);
        extra_tools.push(Arc::new(
            crate::features::connectors::ima::ImaOpenApiTool::new(),
        ));
        // Skill dual-scope governance: compose the composed directory fully
        // at spawn (materialization opportunity one, V-7). The composed
        // directory is the discovery root of EngineConfig.skills_dir (the
        // path injected by build_engine_config_for_session_roots) and must
        // exist before spawn, otherwise the first turn's prompt has no
        // `## Skills` block.
        {
            let sid = session_id.to_string();
            let scope = self.bridge.session_policy(&sid).mode();
            let project_workspace = self.project_workspace_for(&sid);
            tokio::task::spawn_blocking(move || {
                crate::features::assistant::skill_materialization::materialize_session_skills(
                    &sid,
                    scope,
                    project_workspace.as_deref(),
                )
            })
            .await
            .map_err(|e| anyhow::anyhow!("materialize session skills join: {e}"))?
            .map_err(|e| anyhow::anyhow!("materialize session skills: {e}"))?;
        }
        let turn_lifecycle = self.turn_lifecycles.for_session(session_id);
        // One wall-clock epoch for the entry ledger plus a process-monotonic
        // incarnation for the steer-id generation stamp. The stamp on ids
        // returned by `steer` and on the forwarded SteerCommitted/SteerDropped
        // payloads must identify THIS engine build uniquely: the incarnation
        // sequence guarantees uniqueness even when two rebuilds land in the
        // same wall-clock millisecond (or the clock rolls back), where
        // `spawned_at_ms` would collide (zhuowp re-review P1-2).
        let spawned_at_ms = Self::now_epoch_ms();
        let steer_incarnation = self
            .steer_incarnation_seq
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let (engine, forwarder) = AppEngine::spawn_for_session(
            self.app.clone(),
            self.store.clone(),
            bridge,
            session_id,
            extra_tools,
            self.bridge
                .shape_disallowed_tools(session_id, self.compute_disallowed_tools()),
            turn_lifecycle.clone(),
            shell_manager,
            turn_shell_tasks,
            steer_incarnation,
        )
        .await?;

        // Sync is mandatory even when messages is empty: SyncSession not only
        // injects history but also aligns the underlying Engine's internal
        // session id to the pre-created persisted session. Skipping it would
        // get the first turn's SessionUpdated rejected on an id mismatch,
        // ending with only the user message durable and the assistant reply
        // lost.
        match self.store.load(session_id) {
            Ok(saved) => {
                if let Err(error) = engine
                    .sync_session(session_id.to_string(), saved.messages)
                    .await
                {
                    if is_scheduled {
                        let _ = engine.handle.send(Op::Shutdown).await;
                        forwarder.abort();
                        return Err(error).with_context(|| {
                            format!("sync scheduled session {session_id} before its first turn")
                        });
                    }
                    eprintln!("[engine_pool] sync history for {session_id} failed: {error:?}");
                } else {
                    // The hydrated history is forwarder-sanitized: old
                    // transcript rules can no longer match the new engine's
                    // snapshot, so prune them to stop rules accumulating per
                    // turn. The interactive path installs rules around spawn
                    // (reserve already marked active); the retain predicate
                    // guarantees the in-flight reservation's rule survives.
                    turn_lifecycle.prune_stale_transcript_rules();
                }
            }
            Err(error) => {
                let _ = engine.handle.send(Op::Shutdown).await;
                forwarder.abort();
                return Err(error).with_context(|| {
                    format!("load session {session_id} before spawning its engine")
                });
            }
        }

        self.entries.lock().await.insert(
            session_id.to_string(),
            EngineEntry {
                engine: engine.clone(),
                forwarder,
                runtime_model: prepared,
                mcp_config_revision,
                spawned_at_ms,
                steer_incarnation,
                last_active_epoch_ms: AtomicU64::new(Self::now_epoch_ms()),
            },
        );
        Ok(engine)
    }

    /// The epoch timestamp of this session's engine (UNIX ms). None = the
    /// engine is not running. The transcripts projection uses it to vet
    /// "running" entries the previous process left in the worker ledger.
    pub async fn engine_epoch_ms(&self, session_id: &str) -> Option<u64> {
        self.entries
            .lock()
            .await
            .get(session_id)
            .map(|e| e.spawned_at_ms)
    }

    /// Get an existing engine (no spawn). Used by cancel /
    /// submit_user_input etc.: the engine not running means the session is
    /// not active, so these operations are naturally no-ops.
    pub async fn handle_for(&self, session_id: &str) -> Option<AppEngine> {
        self.entries
            .lock()
            .await
            .get(session_id)
            .map(|e| e.engine.clone())
    }

    /// Reclaim a session's engine: cancel the running turn → Shutdown the
    /// engine → abort the forwarder. Called when a session is deleted.
    pub async fn evict(&self, session_id: &str) {
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        self.evict_locked(session_id).await;
    }

    /// The reclaim path used only by idle reclaim: acquires the turn gate +
    /// runtime lock just like [`evict`](Self::evict) (lock-order skeleton in
    /// [`evict_if_idle_with_gates`]), but rechecks the session is still idle
    /// before removing the engine (TOCTOU protection) — a new turn may have
    /// appeared between `reap_idle_engines`' candidate snapshot and the lock:
    /// `reserve_turn` can reserve first without taking the gate (lifecycle
    /// turns active), and `send_reserved_user_message` grabs the gate,
    /// submits, and refreshes the activity clock. Without this recheck a
    /// running turn would be closed into Interrupted by the reclaim (an
    /// unsubmitted reservation is kept per the semantics below and is not
    /// invalidated). The recheck requires the activity clock to have not
    /// advanced since the snapshot and the reclaim conditions to still hold
    /// by current values (predicate in
    /// [`should_still_reap_after_snapshot`]); otherwise it skips and leaves
    /// it to the next sweep.
    /// Returns whether a reclaim actually happened. The delete / model-switch
    /// paths still go through `evict` without the idle recheck; their reclaim
    /// likewise keeps unsubmitted reservations — on the delete path the
    /// sender's subsequent `store.load` guard still errors, and on the
    /// model-switch path the pending message is submitted to the rebuilt
    /// new-model engine. The only place that still invalidates an
    /// unsubmitted reservation is [`Self::evict_locked`]'s no-engine branch
    /// (the window where reserve precedes lazy spawn).
    ///
    /// The residual window (an unsubmitted reservation newly reserved in the
    /// tiny interval after the recheck and before the reclaim closes) is no
    /// longer invalidated: `claim_reclaimed_transition` only claims submitted
    /// turns; an unsubmitted reservation binds the session-level lifecycle,
    /// not the engine, and the sender submits normally after `get_or_spawn`
    /// rebuilds the engine (#352).
    async fn evict_if_idle(&self, session_id: &str, snapshot_last_active_ms: u64) -> bool {
        let active_id = self.store.active_id();
        evict_if_idle_with_gates(
            &self.turn_locks,
            &self.runtime_model_locks,
            session_id,
            || async {
                let now = Self::now_epoch_ms();
                let mut entries = self.entries.lock().await;
                let entry = entries.get(session_id)?;
                let last_active = self.last_activity_ms(session_id, entry);
                if should_still_reap_after_snapshot(
                    self.is_turn_active(session_id),
                    self.scheduled_running_sessions.lock().contains(session_id),
                    active_id.as_deref() == Some(session_id),
                    now.saturating_sub(last_active) / 1000,
                    last_active,
                    snapshot_last_active_ms,
                ) {
                    entries.remove(session_id)
                } else {
                    None
                }
            },
            |entry| self.reclaim_engine_entry(session_id, entry),
        )
        .await
    }

    /// Rebind eviction (review #463 M1 + eviction-tail TOCTOU, round-8 M2):
    /// same turn gate + runtime lock as [`evict`](Self::evict), but the entry
    /// is taken only when the session is still idle at recheck time — a turn
    /// that started after the command layer's post-migration recheck keeps its
    /// engine instead of being cancelled into an Interrupted terminal.
    /// A successful take also resets the per-session shell state (via
    /// [`rebind_evict_with_gates`]): the shell manager's cwd is pinned at
    /// construction, so a surviving manager would keep running bare shell
    /// commands in the old directory while the rebuilt engine runs in the new
    /// one, and the turn-scope registry pins that same manager, so it would
    /// diff the next turn's baseline and clean up its jobs against the old
    /// one. The lifecycle is deliberately NOT forgotten — an unsubmitted
    /// reservation must survive and submit to the rebuilt engine (same
    /// semantics as reclaim).
    ///
    /// The take yields `Some(None)` for an idle session with no resident
    /// engine: there is nothing to reclaim, but the shell state may still
    /// exist from an earlier turn and must be reset. Returns false when the
    /// session was busy at recheck OR when its turn gate could not be
    /// acquired within [`REBIND_EVICT_GATE_TIMEOUT`] — in both cases nothing
    /// was touched and the command reports the session as post-busy.
    pub async fn evict_if_idle_for_rebind(&self, session_id: &str) -> bool {
        rebind_evict_with_gates(
            &self.turn_locks,
            &self.runtime_model_locks,
            &self.shell_managers,
            &self.turn_shell_tasks,
            session_id,
            || async {
                if !rebind_evictable(
                    self.is_turn_active(session_id),
                    self.scheduled_running_sessions.lock().contains(session_id),
                ) {
                    return None;
                }
                Some(self.entries.lock().await.remove(session_id))
            },
            |entry| async move {
                if let Some(entry) = entry {
                    self.reclaim_engine_entry(session_id, entry).await;
                }
            },
        )
        .await
    }

    /// Delete an ordinary chat under the exact turn gate used by lazy spawn
    /// and send. No queued sender can slip between engine reclaim, disk delete,
    /// and lifecycle cleanup to resurrect the session.
    pub(crate) async fn delete_chat_session(&self, session_id: &str) -> Result<()> {
        delete_chat_session_with_gate(
            &self.turn_locks,
            &self.store,
            session_id,
            || self.evict_locked(session_id),
            || self.forget_session(session_id),
        )
        .await?;
        // The bare `agent` is available to **all** sessions (not only those
        // with the multi-agent toggle on), and the background ledger write
        // after the foundation cancels sub-agents (write_json_atomic recreates
        // the parent directory) can resurrect the just-deleted sessions/<id>/.
        // A missing directory is the common case and costs nothing; once
        // Shutdown is processed no new writes happen, so it always converges.
        Self::schedule_late_sweep(
            crate::platform::paths::sessions_root().join(session_id),
            "late sweep of deleted chat",
        );
        Ok(())
    }

    /// Eval-only deletion keeps ordinary delete semantics, but also schedules the existing
    /// late sweep when the immediate disk deletion fails. The error remains observable to the
    /// Judge adapter, while the sweep prevents a transient filesystem failure from silently
    /// retaining the temporary transcript forever.
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) async fn delete_eval_session(&self, session_id: &str) -> Result<()> {
        let result = self.delete_chat_session(session_id).await;
        crate::features::assistant::timing::unregister_eval_observation(session_id);
        if result.is_err() {
            self.forget_session(session_id);
            Self::schedule_late_sweep(
                crate::platform::paths::sessions_root().join(session_id),
                "late sweep of failed eval cleanup",
            );
        }
        result
    }

    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) fn schedule_eval_cleanup(&self, session_id: &str) {
        crate::features::assistant::timing::unregister_eval_observation(session_id);
        Self::schedule_late_sweep(
            crate::platform::paths::sessions_root().join(session_id),
            "background takeover of eval cleanup",
        );
    }

    /// Delayed sweep after deletion: after the foundation cancels sub-agents
    /// it writes the worker ledger asynchronously on a background thread
    /// (write_json_atomic recreates the parent directory), so a just-deleted
    /// directory can be resurrected as an orphan.
    /// Two delayed re-deletes as the backstop; a missing target counts as
    /// converged.
    fn schedule_late_sweep(dir: std::path::PathBuf, label: &'static str) {
        tauri::async_runtime::spawn(async move {
            for delay_ms in [2000u64, 6000] {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        eprintln!("[engine_pool] {label} {} failed: {error}", dir.display())
                    }
                }
            }
        });
    }

    /// Atomically closes the live engine and removes a scheduled session under
    /// the same per-session turn gate used by initial and follow-up turns.
    /// A follow-up already queued on that gate observes the deletion and fails
    /// instead of lazily respawning the id as an ordinary chat.
    pub(crate) async fn delete_scheduled_run(
        &self,
        session_id: &str,
        expected_task_id: &str,
    ) -> Result<()> {
        let result = delete_scheduled_run_with_gate(
            &self.turn_locks,
            &self.store,
            session_id,
            expected_task_id,
            || self.evict_locked(session_id),
        )
        .await;
        if result.is_ok() {
            self.forget_session(session_id);
        }
        result
    }

    /// The closing op sequence of an engine reclaim: **first** cascade-cancel
    /// all sub-agents, **then** shut the engine down.
    /// The order is semantic — the two ops travel the same mpsc channel, and
    /// FIFO guarantees the engine finishes processing the cancel before the
    /// Shutdown; reversing the order equals no cancel (Shutdown breaks out of
    /// the event loop directly).
    fn shutdown_cancel_cascade_ops() -> [Op; 2] {
        [Op::CancelSubAgents, Op::Shutdown]
    }

    async fn reclaim_engine_entry(&self, session_id: &str, entry: EngineEntry) {
        // Stop the producer before admission fallback awaits disk I/O.
        // An in-flight SessionUpdated save is serialized with the fallback
        // by SessionStore's mutation gate; no later delta/tool event can
        // overtake the authoritative transcript_committed + done pair.
        let EngineEntry {
            engine, forwarder, ..
        } = entry;
        let shell_reclaim = self.turn_shell_tasks.begin_reclaim(session_id);
        let shell_reclaim_for_drain = shell_reclaim.clone();
        let shell_reclaim_for_terminal = shell_reclaim.clone();
        let reclaimed = quiesce_engine_before_reclaim(
            || engine.cancel_current(),
            || async move {
                // finalize can legitimately run long (up to MAX_KILL_ATTEMPTS
                // kill retries), so it must not extend the gate hold (issue
                // #255). Unlike dropping the future — which would skip
                // finalize_scope's bookkeeping tail (root_terminal /
                // active_scope_id) and permanently bail every later
                // prepare_turn of this session once forwarder.abort() below
                // removes the last fallback finalizer — the run is spawned
                // detached and only the join is bounded, mirroring the
                // phase-two cleanup: the registry worker keeps sweeping
                // pending kills meanwhile and the scope closes when the
                // detached run settles. cleanup_failed is preset
                // conservatively on timeout: the reclaimed terminal must not
                // claim an unverified clean shell state, and the terminal is
                // emitted right after this closure, before a slow detached
                // run can settle. The detached run later records the
                // authoritative outcome in the flag; nothing re-reads it for
                // the already-persisted terminal — the flag stays truthful
                // for post-reclaim diagnostics instead. Until the detached
                // run settles, the session's next send can transiently fail
                // with the scope-still-active bail; a retry succeeds once
                // the run has closed the scope (rebind and delete reset the
                // registry instead and are immune). A panicked run would
                // never settle at all: one fresh detached attempt redoes the
                // idempotent finalize, and if that fails too the preset flag
                // keeps the terminal honest while the registry worker keeps
                // sweeping pending kills.
                let shell_reclaim_for_finalize = shell_reclaim_for_drain.clone();
                let finalize =
                    tokio::spawn(async move { shell_reclaim_for_finalize.finalize().await });
                match bounded_join_while_holding_turn_gate(
                    "shell reclaim finalize",
                    TURN_GATE_AWAIT_TIMEOUT,
                    finalize,
                )
                .await
                {
                    BoundedJoinOutcome::Settled => {}
                    BoundedJoinOutcome::Detached => shell_reclaim_for_drain.mark_cleanup_failed(),
                    BoundedJoinOutcome::Panicked(_) => {
                        shell_reclaim_for_drain.mark_cleanup_failed();
                        let shell_reclaim_after_panic = shell_reclaim_for_drain.clone();
                        tokio::spawn(async move { shell_reclaim_after_panic.finalize().await });
                    }
                }
                forwarder.abort();
                let _ = forwarder.await;
            },
            || {
                engine.finish_reclaimed_turn(
                    &self.app,
                    &self.store,
                    session_id,
                    shell_reclaim_for_terminal.cleanup_failed(),
                )
            },
        )
        .await;
        if reclaimed {
            log::warn!(
                "[engine_pool] emitted interrupted terminal before reclaim sid={}",
                session_id
            );
        }
        // After reclaim the engine transcript is destroyed with it,
        // removing the only live carrier of old raw prompts (the on-disk
        // history is sanitized). Drop rules that can no longer match,
        // stopping cross-engine-generation residency until session deletion;
        // the in-flight reservation's rule is kept as a backstop (normal
        // reclaim finalizes in-flight turns first).
        if let Some(lifecycle) = self.turn_lifecycles.get(session_id) {
            lifecycle.prune_stale_transcript_rules();
        }
        // Cascade-cancel all background sub-agents first, then shut the
        // engine down (ADR-0006). The two ops travel the same channel; FIFO
        // guarantees the cancel is processed before the shutdown — otherwise,
        // after a delete/model-switch reclaim, the session's bare sub-agents
        // would keep running as orphan tasks up to their own step/time
        // limits. Known limitation: the cancel is an abort without a join, so
        // independent shell subprocesses a sub-agent already started may
        // still linger.
        // Each send is bounded by [`TURN_GATE_AWAIT_TIMEOUT`] (issue #255):
        // reclaim runs under the turn gate, and a wedged engine must not hold
        // the gate forever — the entry is removed either way, and the timeout
        // only stops waiting for delivery under the gate. Undelivered ops move
        // to the detached [`retry_shutdown_sends`] retry: the engine holds a
        // tx_op clone of its own and removing the entry does not close its ops
        // channel, so without re-delivering `Shutdown` the run loop could only
        // stay alive until process exit; the retry holds no gate and
        // re-delivers in the original FIFO order once the engine unstalls,
        // letting the engine exit through the normal Shutdown path.
        let shutdown_ops = Self::shutdown_cancel_cascade_ops();
        let shutdown_total = shutdown_ops.len();
        let delivered = bounded_shutdown_sends(|op| engine.handle.send(op), shutdown_ops).await;
        if delivered < shutdown_total {
            let handle = engine.handle.clone();
            let pending: Vec<Op> = Self::shutdown_cancel_cascade_ops()
                .into_iter()
                .skip(delivered)
                .collect();
            let _ = tokio::spawn(retry_shutdown_sends(
                move |op| {
                    let handle = handle.clone();
                    async move { handle.send(op).await }
                },
                pending,
                RECLAIM_SHUTDOWN_RETRY_PATIENCE,
            ));
        }
    }

    async fn evict_locked(&self, session_id: &str) {
        let runtime_lock = self.runtime_model_locks.for_session(session_id).await;
        let _runtime = runtime_lock.lock().await;
        // Take the entry out of the map before matching: a temporary guard in
        // the match scrutinee would live until the whole match ends, and
        // reclaiming inside an arm would then hold the pool-wide entries lock
        // across the entire reclaim (worst ~15s after bounding), blocking
        // other sessions' handle_for / get_or_spawn on that lock.
        let entry = self.entries.lock().await.remove(session_id);
        match entry {
            Some(entry) => {
                self.reclaim_engine_entry(session_id, entry).await;
            }
            _ => {
                if let Some(lifecycle) = self.turn_lifecycles.get(session_id) {
                    // A caller can reserve before lazy spawn. Reclaim that Reserved
                    // phase without fabricating chat:done; the guard's eventual send
                    // will observe that its reservation was invalidated.
                    lifecycle.invalidate_unsubmitted_reservation();
                }
            }
        }
    }

    pub(crate) fn forget_session(&self, session_id: &str) {
        #[cfg(any(feature = "benchmark-hooks", test))]
        self.eval_model_snapshots.forget_session(session_id);
        self.turn_lifecycles.remove(session_id);
        self.turn_shell_tasks.remove(session_id);
        self.shell_managers.remove(session_id);
    }

    pub async fn list_shell_tasks(&self, session_id: &str) -> Result<Vec<ShellJobSnapshot>> {
        let Some(manager) = self.shell_managers.get(session_id) else {
            return Ok(Vec::new());
        };
        tauri::async_runtime::spawn_blocking(move || {
            let mut manager = manager
                .lock()
                .map_err(|_| anyhow::anyhow!("Shell manager lock poisoned"))?;
            Ok(manager.list_jobs())
        })
        .await
        .map_err(|error| anyhow::anyhow!("list shell tasks join failed: {error}"))?
    }

    pub async fn cancel_shell_task(&self, session_id: &str, task_id: &str) -> Result<ShellResult> {
        let manager = self
            .shell_managers
            .get(session_id)
            .with_context(|| format!("No shell runtime for session '{session_id}'"))?;
        let task_id = task_id.to_string();
        tauri::async_runtime::spawn_blocking(move || {
            let mut manager = manager
                .lock()
                .map_err(|_| anyhow::anyhow!("Shell manager lock poisoned"))?;
            manager.kill(&task_id)
        })
        .await
        .map_err(|error| anyhow::anyhow!("cancel shell task join failed: {error}"))?
    }

    // ── Model hot switching (called by commands.rs) ─────────────────────

    /// The default model for a new session: the (model name, id) of the
    /// global active model. Read fresh from disk (the GUI may have just
    /// changed the default); falls back to the boot snapshot on failure.
    pub fn default_model_for_new_session(&self) -> (String, Option<String>) {
        let prefs = UserPrefs::load();
        default_model_for_new_session_from(&prefs, &self.bridge)
    }

    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) fn pin_active_eval_suite_model(&self) -> Result<EvalSuiteModelSnapshot> {
        let prefs = UserPrefs::load();
        let saved_model = prefs
            .active_model()
            .cloned()
            .context("active evaluation model is not configured")?;
        let identity = identity_for_saved_model(&self.bridge, &saved_model);
        Ok(self.eval_model_snapshots.pin_suite(saved_model, identity))
    }

    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) fn derive_eval_suite_case_selection(
        &self,
        suite: &EvalSuiteModelSnapshot,
    ) -> Result<EvalModelSelection> {
        self.eval_model_snapshots.derive_case_selection(suite)
    }

    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) fn discard_eval_suite_model(&self, suite: &EvalSuiteModelSnapshot) {
        self.eval_model_snapshots.discard_suite(suite);
    }

    /// Creates and loads a one-off eval session. The eval runner decides the
    /// session ID in advance so reports and cleanup can be correlated exactly;
    /// ordinary GUI sessions keep using the SessionStore-generated ID.
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) async fn prepare_eval_session(
        &self,
        session_id: &str,
        model_selection: Option<&EvalModelSelection>,
    ) -> Result<()> {
        match model_selection {
            None => {
                let (model, model_id) = self.default_model_for_new_session();
                self.store.create_empty_with_id(
                    session_id.to_string(),
                    model,
                    model_id,
                    self.bridge.workspace.clone(),
                )?;
                self.get_or_spawn(session_id).await?;
            }
            Some(selection) => {
                self.eval_model_snapshots
                    .bind_to_session(session_id, selection)?;
                let prepare_result = self.store.create_empty_with_id(
                    session_id.to_string(),
                    selection.wire_model().to_string(),
                    selection.model_id().map(str::to_string),
                    self.bridge.workspace.clone(),
                );
                if let Err(error) = prepare_result {
                    self.eval_model_snapshots.forget_session(session_id);
                    return Err(error);
                }
                if let Err(error) = self.get_or_spawn(session_id).await {
                    self.eval_model_snapshots.forget_session(session_id);
                    return Err(error);
                }
            }
        }
        crate::features::assistant::timing::register_eval_observation(session_id);
        Ok(())
    }

    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) fn eval_session_execution_root(
        &self,
        session_id: &str,
    ) -> Result<std::path::PathBuf> {
        Ok(self.store.session_roots(session_id)?.execution)
    }

    /// Read the transcript snapshot of an eval temporary session without
    /// exposing a mutable store handle.
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) fn load_eval_transcript(&self, session_id: &str) -> Result<Vec<Message>> {
        Ok(self.store.load(session_id)?.messages)
    }

    /// Switch a session's model (chat chip hot switch): write the per-session
    /// binding + evict the session's engine. The next message rebuilds with
    /// the new model via get_or_spawn (the client is rebuilt across
    /// providers; history is hydrated by SyncSession). `model_id = None` =
    /// clear the binding and fall back to the global default.
    pub async fn switch_session_model(
        &self,
        session_id: &str,
        model_id: Option<String>,
    ) -> Result<()> {
        self.store.set_session_model_id(session_id, model_id)?;
        self.evict(session_id).await;
        Ok(())
    }

    // ── High-level routing (called by commands.rs) ──────────────────────

    /// Atomically switch the multi-agent resource policy: first occupy the
    /// same lifecycle slot as a send, then persist the state and reclaim the
    /// old engine inside the session turn gate. This way a send and a switch
    /// can never interleave into "new toggle + old engine" or "old toggle +
    /// new engine". The reclaim claims only submitted turns and produces no
    /// forged chat:done; the placeholder reservation held by this function
    /// returns the slot on Drop.
    pub(crate) async fn reconfigure_multi_agent_mode(
        &self,
        session_id: &str,
        enabled: bool,
    ) -> Result<()> {
        if enabled && !self.swarm_mode_available(session_id) {
            anyhow::bail!("当前会话不支持 Pinvou 蜂群模式");
        }
        let _reservation = self.turn_lifecycles.for_session(session_id).reserve()?;
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        self.store.set_multi_agent(session_id, enabled)?;
        self.evict_locked(session_id).await;
        Ok(())
    }

    /// Atomically reserve the single turn slot for a session before callers
    /// consume one-shot state, stage attachments, or perform other side effects.
    /// Dropping the returned guard before submission restores the slot.
    pub(crate) fn reserve_turn(&self, session_id: &str) -> Result<TurnReservation> {
        // Execution-root rewind gate: the flag lock spans "check + lifecycle
        // occupancy", on the opposite side of the rewinder's "set + in-flight
        // peer recheck" on the same lock — if the rewind set the flag first,
        // this reservation necessarily sees it and refuses; if this
        // reservation occupied first, the rewinder's busy recheck necessarily
        // sees active.
        // This gate is skipped when the root is unresolvable (directory
        // deleted, etc.) — the same degradation as the busy gate.
        let root_flag = self
            .store
            .session_roots(session_id)
            .ok()
            .map(|roots| self.execution_root_rewind_flag(&roots.execution));
        let _flag_guard = root_flag.as_ref().map(|flag| flag.lock());
        if _flag_guard.as_deref().is_some_and(|rewinding| *rewinding) {
            bail!("该会话绑定的项目目录正在回退/回滚，请稍后重试");
        }
        let mut reservation = self.turn_lifecycles.for_session(session_id).reserve()?;
        let baseline = self.store.load(session_id)?;
        reservation.set_base_transcript_revision(transcript_revision(&baseline.messages)?);
        Ok(reservation)
    }

    /// The execution-root rewind exclusion flag (deduplicated by canonical
    /// root; short-name/case differences on both ends are normalized).
    fn execution_root_rewind_flag(&self, execution_root: &std::path::Path) -> Arc<SyncMutex<bool>> {
        let canonical =
            std::fs::canonicalize(execution_root).unwrap_or_else(|_| execution_root.to_path_buf());
        self.execution_root_rewind_flags
            .lock()
            .entry(canonical)
            .or_default()
            .clone()
    }

    /// Enter the execution-root rewind/rollback critical section:
    /// check-and-set completes inside the same flag lock — already set means
    /// refuse ("this directory is mid-rewind"), and the winner owns the set
    /// exclusively. Previously the set did not check the old value and the
    /// Guard's Drop cleared unconditionally: two sessions on the same root
    /// setting in sequence would have the loser's Drop open the winner's
    /// critical section mid-flight (review M2). The refusal path creates no
    /// Guard, so the winner's Drop remains the only clearing point.
    /// After setting successfully, the caller must recheck in-flight peers
    /// (an already-active turn is not stopped by the reservation gate; see
    /// busy_peer_on_same_execution_root).
    pub(crate) fn begin_execution_root_rewind(
        &self,
        execution_root: &std::path::Path,
    ) -> Result<ExecutionRootRewindGuard, String> {
        let flag = self.execution_root_rewind_flag(execution_root);
        let mut guard = flag.lock();
        if *guard {
            return Err("该会话绑定的项目目录正在回退/回滚，请稍后再试".to_string());
        }
        *guard = true;
        drop(guard);
        Ok(ExecutionRootRewindGuard { flag })
    }

    /// Whether this session has a scheduled turn in flight (in the
    /// spawn→submit window the lifecycle is not yet active; the rewind gate's
    /// peer recheck needs to count it as busy).
    pub(crate) fn is_scheduled_turn_running(&self, session_id: &str) -> bool {
        self.scheduled_running_sessions.lock().contains(session_id)
    }

    /// Refresh the session engine's idle clock (called when a turn starts;
    /// async-context safe: the entries lock is taken momentarily and never
    /// overlaps the send path's long critical section). A no-op when the
    /// engine is absent — lazy spawn initializes last_active to now, which is
    /// already fresh.
    async fn touch_engine_activity(&self, session_id: &str) {
        if let Some(entry) = self.entries.lock().await.get(session_id) {
            entry
                .last_active_epoch_ms
                .store(Self::now_epoch_ms(), Ordering::Release);
        }
    }

    /// Whether this session currently has a turn in progress (for the
    /// frontend to restore the busy display after a remount).
    pub fn is_turn_active(&self, session_id: &str) -> bool {
        self.turn_lifecycles
            .get(session_id)
            .is_some_and(|lifecycle| lifecycle.is_active())
    }

    /// Product capability gate for Pinvou's multi-agent mode. This is shared
    /// by command, prompt, engine and roster paths so a hidden control cannot
    /// be bypassed by stale state or a direct IPC call.
    pub(crate) fn multi_agent_mode_available(&self, session_id: &str) -> bool {
        self.bridge.multi_agent_mode_available(session_id)
    }

    /// Whether the swarm regime (lifted delegation caps, expert roster, swarm
    /// prompt) may apply to this session. Scheduled sessions always assemble
    /// plain engine config (engine.rs's `scheduled_profile` gate), so the
    /// swarm regime must follow the same exclusion: turn assembly must not
    /// inject swarm copy the engine would not honor, and the toggle must not
    /// be offered. Kept separate from [`Self::multi_agent_mode_available`],
    /// which intentionally stays the sole gate for transcript listing —
    /// scheduled runs can still delegate through the bare `agent` tool and
    /// their records stay readable.
    pub(crate) fn swarm_mode_available(&self, session_id: &str) -> bool {
        Self::swarm_mode_available_for(
            self.multi_agent_mode_available(session_id),
            self.store.scheduled_profile(session_id).is_some(),
        )
    }

    /// Testable body of [`Self::swarm_mode_available`].
    fn swarm_mode_available_for(multi_agent_available: bool, scheduled: bool) -> bool {
        multi_agent_available && !scheduled
    }

    /// Resolve the session-owned delegated-agent runtime-state root.
    /// For project-bound Code sessions this is distinct from the execution root.
    pub(crate) fn session_state_root(
        &self,
        session_id: &str,
    ) -> std::result::Result<std::path::PathBuf, String> {
        self.store
            .session_roots(session_id)
            .map(|roots| roots.ledger)
            .map_err(|error| format!("解析会话状态根失败: {error:#}"))
    }

    /// Send a user message to a session's engine (lazy spawn if absent).
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub async fn send_user_message(
        &self,
        session_id: &str,
        content: String,
        mode: AppMode,
        restrict_tools_for_turn: bool,
    ) -> Result<()> {
        let reservation = self.reserve_turn(session_id)?;
        let display_message = user_display_message(content.clone());
        let expert_snapshot = (self.store.mode_state(session_id).multi_agent
            && self.swarm_mode_available(session_id))
        .then(ExpertRosterSnapshot::capture);
        self.send_reserved_user_message(
            session_id,
            content,
            display_message,
            mode,
            restrict_tools_for_turn,
            expert_snapshot,
            reservation,
        )
        .await
    }

    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) async fn send_eval_user_message(
        &self,
        session_id: &str,
        content: String,
        policy: &crate::features::assistant::product_runtime::eval_tool_policy::EvalTurnPolicy,
    ) -> Result<()> {
        let reservation = self.reserve_turn(session_id)?;
        let display_message = user_display_message(content.clone());
        self.send_reserved_eval_user_message(
            session_id,
            content,
            display_message,
            policy,
            reservation,
        )
        .await
    }

    #[cfg(any(feature = "benchmark-hooks", test))]
    async fn send_reserved_eval_user_message(
        &self,
        session_id: &str,
        content: String,
        display_message: Message,
        policy: &crate::features::assistant::product_runtime::eval_tool_policy::EvalTurnPolicy,
        mut reservation: TurnReservation,
    ) -> Result<()> {
        let baseline_revision = reservation
            .base_transcript_revision()
            .context("turn reservation has no base transcript revision")?
            .to_string();
        reservation.set_transcript_with_baseline(
            TranscriptOperation::Append,
            display_message,
            baseline_revision,
        )?;
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        self.store.load(session_id).with_context(|| {
            format!("Session '{session_id}' was deleted before the eval turn could start")
        })?;
        reservation.ensure_active()?;
        self.get_or_spawn(session_id)
            .await?
            .send_reserved_eval_message(content, policy, reservation)
            .await
    }

    /// Submit a previously admitted append operation. This is the entry point
    /// used by chat commands that must reserve before resolving attachments.
    pub(crate) async fn send_reserved_user_message(
        &self,
        session_id: &str,
        content: String,
        display_message: Message,
        mode: AppMode,
        restrict_tools_for_turn: bool,
        expert_snapshot: Option<std::sync::Arc<ExpertRosterSnapshot>>,
        mut reservation: TurnReservation,
    ) -> Result<()> {
        let baseline_revision = reservation
            .base_transcript_revision()
            .context("turn reservation has no base transcript revision")?
            .to_string();
        reservation.set_transcript_with_baseline(
            TranscriptOperation::Append,
            display_message,
            baseline_revision,
        )?;
        let scheduled_profile = self.store.scheduled_profile(session_id);
        if scheduled_profile.is_none() && session_id.starts_with("sched-") {
            bail!("Scheduled session '{session_id}' no longer exists");
        }
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        if let Some(profile) = scheduled_profile {
            scheduled_profile_after_turn_gate(&self.store, session_id, &profile.task_id)?;
        } else {
            self.store.load(session_id).with_context(|| {
                format!("Session '{session_id}' was deleted before the turn could start")
            })?;
        }
        reservation.ensure_active()?;
        // The turn is formally submitted: refresh the idle clock (attachment
        // resolution and other time-consuming steps sit between reserve and
        // send; keep the idle sweep from reclaiming the engine in this
        // window).
        self.touch_engine_activity(session_id).await;
        // Side B card pool: when this session is wearing an expert mask,
        // inject a light anchor (short) every turn to maintain the identity.
        // The full body was injected once with the first equip message
        // (commands::chat take_pending_turn_injections).
        // Resolved at the pool layer, so all upper-level calls (chat /
        // accept_plan) automatically carry the anchor.
        // The same card derives two per-turn states: ① the light anchor
        // (sticky identity) ② whether to empty the tool table (a
        // pure-conversation meta card such as the card-crafting expert → zero
        // tools this turn, preventing it from writing files by mistake). The
        // active persona is read live every turn: put it on and it restricts,
        // take it off and it recovers, switch cards and the new card applies —
        // no persisted state, no equip/unequip sync needed.
        let active_card = self
            .store
            .active_persona_id(session_id)
            .and_then(|pid| crate::features::personas::get(&pid));
        let persona_reminder = active_card
            .as_ref()
            .map(crate::features::personas::equip_anchor);
        let restrict_tools = active_card.as_ref().is_some_and(|c| c.conversational_only);
        let restrict_tools = restrict_tools || restrict_tools_for_turn;
        self.get_or_spawn(session_id)
            .await?
            .send_reserved_user_message(
                content,
                mode,
                persona_reminder,
                restrict_tools,
                expert_snapshot,
                reservation,
            )
            .await
    }

    /// Execute the initial turn for a pre-created scheduled session and wait
    /// for the authoritative terminal event produced by the existing engine
    /// forwarder. The engine is evicted afterwards, while the session itself
    /// remains durable and can later be opened or continued by the user.
    pub(crate) async fn run_scheduled_turn<F, Fut>(
        &self,
        session_id: &str,
        content: String,
        cancel: CancellationToken,
        mut on_started: F,
    ) -> Result<ScheduledTurnCompletion>
    where
        F: FnMut(&str) -> Fut + Send,
        Fut: Future<Output = Result<()>> + Send,
    {
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        // Scheduled-turn registration: in the spawn→submit window the
        // lifecycle is not yet active, and idle reclaim needs this explicit
        // protection layer; registration is revoked at the end regardless of
        // success or failure (a panic is covered by abort semantics — this
        // task itself is a spawned detached future).
        // Execution-root rewind gate: the flag lock spans "check + in-flight
        // registration" (the same race elimination as reserve_turn); with a
        // rewind in progress this scheduled run fails truthfully and the
        // scheduler records it.
        let root_flag = self
            .store
            .session_roots(session_id)
            .ok()
            .map(|roots| self.execution_root_rewind_flag(&roots.execution));
        match &root_flag {
            Some(flag) => {
                let flag_guard = flag.lock();
                if *flag_guard {
                    bail!("该会话绑定的项目目录正在回退/回滚，本次定时执行失败");
                }
                self.scheduled_running_sessions
                    .lock()
                    .insert(session_id.to_string());
            }
            None => {
                self.scheduled_running_sessions
                    .lock()
                    .insert(session_id.to_string());
            }
        }
        // Round-8 should-fix 5: the slot used to be removed only after the
        // round future resolved; a panic inside a round skipped the removal
        // and permanently wedged the new rebind fences for this session until
        // restart. A drop guard removes it on every exit path.
        struct ScheduledRunningSlotGuard<'a> {
            slots: &'a SyncMutex<HashSet<String>>,
            session_id: &'a str,
        }
        impl Drop for ScheduledRunningSlotGuard<'_> {
            fn drop(&mut self) {
                self.slots.lock().remove(self.session_id);
            }
        }
        let _running_slot = ScheduledRunningSlotGuard {
            slots: &self.scheduled_running_sessions,
            session_id,
        };
        let result = async {
            self.touch_engine_activity(session_id).await;
            let profile = self
                .store
                .scheduled_profile(session_id)
                .with_context(|| format!("Scheduled session '{session_id}' has no profile"))?;
            // A user may have opened this conversation since the previous run.
            // Scheduled execution must rebuild from the latest task profile and
            // global model/provider settings instead of reusing that old client.
            // Known limitation: a user reservation pending between reserve_turn
            // and the turn gate on this fresh run session survives the reclaim
            // (claim_reclaimed_transition preserves unsubmitted turns), so the
            // send below bails "session_turn_in_progress" and this tick fails
            // while the user's message goes through on the respawned engine.
            // The window is ms-scale after create_session; every run owns a
            // fresh session, so nothing propagates to the next run.
            self.evict_locked(session_id).await;
            let engine = match self
                .get_or_spawn_with_policy(session_id, true, None)
                .await
            {
                Ok(engine) => engine,
                Err(engine_error) => {
                    if let Err(seed_error) = persist_scheduled_prompt(
                        self.store.clone(),
                        session_id.to_string(),
                        content.clone(),
                    )
                    .await
                    {
                        bail!(
                            "{engine_error:#}; additionally failed to preserve the scheduled prompt: {seed_error:#}"
                        );
                    }
                    return Err(engine_error);
                }
            };
            let _unattended =
                ScheduledUnattendedGuard::enter(engine.scheduled_unattended.clone());
            let mut turn_events = engine.subscribe_turns();
            persist_scheduled_prompt(
                self.store.clone(),
                session_id.to_string(),
                content.clone(),
            )
            .await?;
            if cancel.is_cancelled() {
                return Ok(ScheduledTurnCompletion {
                    turn_id: String::new(),
                    status: TurnOutcomeStatus::Interrupted,
                    error: None,
                    cancel_requested: true,
                });
            }
            engine.send_scheduled_message(content, &profile).await?;
            wait_for_scheduled_terminal(
                &mut turn_events,
                &engine,
                cancel,
                &mut on_started,
            )
            .await
        }
        .await;

        drop(_running_slot);
        self.evict_locked(session_id).await;
        result
    }

    /// Cancel the specified session's in-progress reply, and cascade-cancel
    /// every background sub-agent it spawned. A no-op when the engine is not
    /// running.
    ///
    /// Executed in two phases, avoiding contention with
    /// `send_reserved_user_message` over `turn_lock` which would make the
    /// "stop button unresponsive": cancel_token is an independent atomic
    /// whose setting needs no turn_lock protection, so step one triggers it
    /// lock-free and turn_loop's biased select exits immediately and emits
    /// `TurnComplete` (→ chat:done) normally; step two then cleans up
    /// shell/lifecycle state under the lock.
    ///
    /// Step two takes the lifecycle's submission state (rather than the
    /// Engine's existence) as authoritative: when the reservation is in the
    /// "reserved, not submitted" stage (the message has not entered the
    /// engine queue), whether or not the session still holds the previous
    /// turn's idle Engine, immediately claim the unsubmitted Interrupted
    /// terminal state and re-emit `chat:done`, invalidating the reservation
    /// (the subsequent `ensure_active` fails and the message is never
    /// submitted) — guaranteeing the frontend busy flag always resets.
    ///
    /// The "stop" button is a sub-agent's only deterministic stop entry
    /// (cards have no cancel button and a natural-language instruction is
    /// only a suggestion); canceling only the host turn would leave
    /// background sub-agents burning money.
    ///
    /// `keep_inbox` (P0-A): when interrupting (true), un-injected steers are
    /// kept for the next turn; when stopping (false), clear them and emit
    /// SteerDropped so the frontend removes the chip with a notice —
    /// preventing a "gone from the UI but alive in the engine" hang.
    pub async fn cancel(&self, session_id: &str, keep_inbox: bool) -> CancelOutcome {
        // r10 foundation: the steer disposition
        // (InterruptKeepInbox/StopDropInbox) and the cancel token are
        // published atomically by cancel_with_mode; a separate keepInbox
        // toggle set before the cancel is no longer needed (the old two-step
        // write had a disposition cross-talk window under concurrent
        // cancels).
        let steer_mode = if keep_inbox {
            deepseek_tui::core::engine::CancelMode::InterruptKeepInbox
        } else {
            deepseek_tui::core::engine::CancelMode::StopDropInbox
        };
        // The two-phase generation guard is documented in
        // cancel_turn_with_gates: a cancel request binds the turn epoch at
        // its initiation; if the later-queued C2 of concurrent requests
        // finds after turn_lock is released that the target turn has ended
        // (a new turn has been reserved), the whole thing is a no-op and the
        // new turn is not canceled by mistake.
        let app = &self.app;
        // The turn-bound arbitration runs inside the cancel closure on the
        // identity that `arm_pending_cancel_and_cancel` snapshots under the
        // lifecycle state lock at dispatch time — not on a view taken here.
        // A snapshot at this point would be two lock acquisitions away from
        // the closure (the `get_engine` await sits in between): an engine
        // self-started follow-up turn (idle sub-agent completion /
        // background shell wake / goal continuation) swaps the shared token
        // before the forwarder observes `TurnStarted`, and a `TurnStarted`
        // for the target turn can land inside the await — only the
        // same-lock identity is fresh enough to arbitrate on (issue #254
        // review round).
        let (target, claimed_unsubmitted) = cancel_turn_with_gates(
            &self.turn_locks,
            &self.turn_lifecycles,
            &self.turn_shell_tasks,
            session_id,
            // steer_mode is passed to cancel_turn_with_gates as well: the
            // pending_cancel armed during the submit→TurnStarted window must
            // carry the same disposition mode so the forwarder replay cannot
            // degrade into a mode-less StopDropInbox.
            steer_mode,
            // get_engine: take the in-pool engine handle (does not cancel;
            // the epoch validation is executed by cancel_turn_with_gates
            // after the await and before the cancel, eliminating the TOCTOU
            // window while handle_for holds the entries lock).
            // handle_for takes the entries lock only momentarily (no conflict
            // with send's momentary entries lock) and does not wait for
            // turn_lock — phase one triggers lock-free first, and turn_loop's
            // biased select exits immediately.
            || async move { self.handle_for(session_id).await },
            // cancel_current: trigger a synchronous cancel on the present engine
            // (idempotent) and best-effort try_send the cascade cancel for the
            // engine's background sub-agents (multiagent, ADR-0006): the "stop"
            // button is a sub-agent's only deterministic stop entry (cards have no
            // cancel button; a natural-language instruction is only a suggestion), so
            // canceling only the host turn would leave background sub-agents burning
            // money. try_send does not block and enqueues immediately while the
            // channel has room (ahead of the next turn's SendMessage); when the
            // channel is full (capacity 32) it gives up, and the phase-two and
            // mismatch-resend paths guarantee delivery with a locked await (reviewer
            // point 9 + G1).
            |engine, identity| {
                // Turn-bound arbitration on the engine slot (issue #254),
                // shared with the wiring tests via `dispatch_turn_bound_cancel`.
                // The epoch re-checks alone still admit a stale view: a
                // delayed forwarder can leave the lifecycle looking like the
                // target turn while the slot already holds a self-started
                // follow-up turn's live token. The dispatch therefore fires
                // only when the identity names the slot's turn; unobserved
                // (submit→TurnStarted) and terminal-closing identities
                // converge on disposition-only, and the genuinely pending
                // target is delivered by the forwarder's turn-bound
                // pending_cancel replay.
                // Known boundary: the cascade cancel (try_send below) is not
                // converged with the arbitration — a bound skip still cancels
                // every subagent the engine currently hosts. Clearing turn
                // N's leftover subagents is the stop contract and N+1's
                // just-spawned subagents are indistinguishable app-side; see
                // the fork registration docs.
                dispatch_turn_bound_cancel(engine, identity.as_ref(), steer_mode);
                let _ = engine.handle.try_send(Op::CancelSubAgents);
            },
            // cascade_cancel: awaited in phase two while holding turn_lock,
            // guaranteeing the enqueue completes before the turn gate is
            // released — the next turn's SendMessage must wait for the same
            // turn_lock (send_reserved_user_message), so the cascade cancel
            // is always enqueued before the new turn's message; the engine
            // cancels the old turn's sub-agents before starting the new turn
            // and never kills the new turn's just-started sub-agents by
            // mistake (reviewer point 4). Sending twice is harmless: it is an
            // idempotent no-op relative to phase one's try_send and the
            // reclaim path's CancelSubAgents.
            |engine| {
                let handle = engine.handle.clone();
                let sid = session_id.to_string();
                async move {
                    if let Err(e) = handle.send(Op::CancelSubAgents).await {
                        eprintln!("[engine_pool] cancel subagents {sid} failed: {e:#}");
                    }
                }
            },
            // claim_unsubmitted: the claim-terminal path for an unsubmitted
            // reservation (claim + emit terminal synchronously).
            // Carries the target epoch: the claim and the turn_epoch check
            // complete atomically inside the state lock; when the turn has
            // switched after the recheck (a new turn has been reserved) the
            // claim is a no-op and the new turn is not killed by mistake.
            |lifecycle, target| {
                lifecycle.emit_unsubmitted_interrupted_terminal_for_epoch(app, session_id, target)
            },
        )
        .await;
        // Idle-window backstop for the "stop = clear" contract (third review
        // round, item 2): when the turn ended naturally, the lifecycle is
        // already idle (target=None) but the frontend busy flag has not reset,
        // a ⏹ press hits the idle guard in phase one of the gate function and
        // never runs the cancel closure — steers parked by the previous
        // round's keepInbox escape StopDropInbox, get injected next round as
        // usual, and no chat:steer_dropped is emitted. In that case re-issue
        // StopDropInbox once to the live engine — the foundation merely raises
        // the drop_through_generation barrier to retire parked steers (a no-op
        // when nothing is parked) with no side effects on the idle token. Only
        // the stop path does this (an interrupt's keepInbox intends to keep
        // parked steers); if a fresh turn got reserved between the two checks,
        // `idle_recheck` deliberately skips the re-issue — cancelling a
        // just-reserved turn here would break its admission contract, and
        // that turn's own step boundaries settle parked steers normally.
        //
        // Semantics boundary (registered for issue #254): this backstop only
        // triggers on a stop issued while the lifecycle is already idle at
        // entry — an intentional stop=clear (the user pressed ⏹ on a session
        // the frontend still shows as busy). If the engine has self-started a
        // follow-up turn the forwarder has not yet observed, this unbound
        // fire hits that turn's live token on purpose: no target turn exists
        // here, so there is no turn-bound arbitration to make. That is a
        // different contract from the #254 misfire shape (a stop aimed at a
        // turn that already ended), which the turn-bound dispatch intercepts:
        // aimed stops always snapshot a Some target and never reach this
        // branch.
        if !keep_inbox && target.is_none() {
            let still_idle = self
                .turn_lifecycles
                .get(session_id)
                .and_then(|lc| lc.current_turn_generation())
                .is_none();
            if still_idle {
                if let Some(engine) = self.handle_for(session_id).await {
                    // Re-check once after handle_for's await to narrow the window.
                    let idle_recheck = self
                        .turn_lifecycles
                        .get(session_id)
                        .and_then(|lc| lc.current_turn_generation())
                        .is_none();
                    if idle_recheck {
                        engine.cancel_current_with_mode(
                            deepseek_tui::core::engine::CancelMode::StopDropInbox,
                        );
                    }
                }
            }
        }
        // Assemble the turn result. `terminal=true` is allowed in exactly
        // three cases:
        //   1) claim path — the terminal was completed by the cancel itself
        //      (its chat:done is emitted before the cancel returns and before
        //      the frontend listener registers, so it is always missed and
        //      must be confirmed by the command's return value);
        //   2) idle (target=None) — there is no event to wait for;
        //   3) the target turn's reserve gate is already reopened
        //      (is_reserve_gate_open_for) — the terminal has finished closing:
        //      either the lifecycle is idle (in the authoritative terminal
        //      path, reopening the gate and emitting chat:done happen in the
        //      same synchronous block, finish before emit with no await in
        //      between, so observing an open gate ⇒ chat:done was already
        //      emitted), or a new turn is active with epoch != target (a
        //      reserve can only succeed after the old turn reopened the gate,
        //      so the old turn's terminal must have closed — terminal=true is
        //      mandatory here, otherwise the frontend waits for an already
        //      emitted-and-missed chat:done until timeout and then hits the
        //      new turn's reserve).
        // Everything else is terminal=false (the frontend waits for the
        // chat:done carrying the target generation): **engine turn loop exit
        // ≠ lifecycle released** — until the forwarder processes TurnComplete
        // and claims, the lifecycle is still active and a chat reserve_turn
        // would hit session_turn_in_progress. The only reliable "slot
        // released" signal for the frontend is chat:done (emitted only after
        // the gate reopens).
        // (M-6: is_terminal_emitted was previously used for terminal=true, but
        // between the claim setting terminal_emitted and
        // finish_terminal_emission reopening the gate, the forwarder has
        // several spawn_blocking persistence awaits; inside that window the
        // gate is still closed, and a frontend that skips the wait and calls
        // doSendFor directly hits session_turn_in_progress → intermittent zap
        // delivery failures. The criterion must be "gate open", not "terminal
        // claimed".)
        let reserve_gate_open = self
            .turn_lifecycles
            .get(session_id)
            .is_some_and(|lc| lc.is_reserve_gate_open_for(target));
        CancelOutcome {
            generation: target,
            terminal: claimed_unsubmitted || target.is_none() || reserve_gate_open,
        }
    }

    /// Pinvou tool toggles (globally persisted): broadcast the "full names of
    /// unavailable tools" (toggles off ∪ hidden; the model-visible full names,
    /// lowercased) to **all running session engines** → writing each one's
    /// config.disallowed_tools, taking effect on the next turn. Sessions whose
    /// engine is not running read their initial value from the persisted list
    /// at the next spawn (build_engine_config), so new windows / new
    /// conversations all inherit the same governance state.
    pub async fn set_disallowed_all(&self, tools: Vec<String>) {
        let targets = self
            .entries
            .lock()
            .await
            .iter()
            .map(|(sid, entry)| (sid.clone(), entry.engine.clone()))
            .collect::<Vec<_>>();
        for (sid, engine) in targets {
            // The global hot refresh is shaped per session as well (code
            // sessions keep present_artifact hidden), and the entries lock is
            // released before sending, avoiding holding the global engine
            // table lock across awaits.
            if let Err(e) = engine
                .handle
                .send(Op::SetDisallowedTools {
                    tools: Some(self.bridge.shape_disallowed_tools(&sid, tools.clone())),
                })
                .await
            {
                eprintln!("[engine_pool] set_disallowed_all {sid} failed: {e:#}");
            }
        }
    }

    /// execpolicy hard-deny hot refresh (scope gate channel ③ + safety net):
    /// after connector/skill toggles are persisted or the super-permission
    /// toggle changes, recompute the deny ruleset per session scope (CLI
    /// binary names + disabled skill script paths + sensitive-data/privilege
    /// rules) and broadcast it to every running engine so it hard-denies from
    /// the next turn. Newly spawned / rebuilt engines get the initial value
    /// from build_engine_config_for_session_roots — both share
    /// `bridge.scope_deny_ruleset`.
    pub async fn refresh_permission_rulesets(&self) {
        let targets = self
            .entries
            .lock()
            .await
            .iter()
            .map(|(sid, entry)| (sid.clone(), entry.engine.clone()))
            .collect::<Vec<_>>();
        for (sid, engine) in targets {
            if let Err(e) = engine
                .handle
                .send(Op::SetPermissionRuleset {
                    ruleset: self.bridge.scope_deny_ruleset(&sid),
                })
                .await
            {
                eprintln!("[engine_pool] refresh_permission_rulesets {sid} failed: {e:?}");
            }
        }
    }

    /// Current UNIX time (milliseconds). The engine epoch and the worker
    /// ledger's created_at_ms/updated_at_ms share the same source (both
    /// SystemTime) and can be compared directly.
    fn now_epoch_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Edit/resend the specified session's last user message. The caller
    /// passes the model content and the clean display message separately
    /// after reserving the turn, keeping runtime reminders out of the visible
    /// history.
    pub(crate) async fn edit_last_turn_reserved(
        &self,
        session_id: &str,
        new_message: String,
        display_message: Message,
        mut reservation: TurnReservation,
    ) -> Result<()> {
        let baseline_revision = reservation
            .base_transcript_revision()
            .context("turn reservation has no base transcript revision")?
            .to_string();
        reservation.set_transcript_with_baseline(
            TranscriptOperation::EditLast,
            display_message,
            baseline_revision,
        )?;
        let scheduled_profile = self.store.scheduled_profile(session_id);
        if scheduled_profile.is_none() && session_id.starts_with("sched-") {
            bail!("Scheduled session '{session_id}' no longer exists");
        }
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        if let Some(profile) = scheduled_profile {
            scheduled_profile_after_turn_gate(&self.store, session_id, &profile.task_id)?;
        } else {
            self.store.load(session_id).with_context(|| {
                format!("Session '{session_id}' was deleted before the edit could start")
            })?;
        }
        reservation.ensure_active()?;
        // A resend is also a turn submission: refresh the idle clock (same
        // reason as send_reserved_user_message).
        self.touch_engine_activity(session_id).await;
        self.get_or_spawn(session_id)
            .await?
            .edit_last_turn_reserved(new_message, reservation)
            .await
    }

    /// Manually compacts one session. Engines are spawned lazily, so a session
    /// that has not sent a message since restart has no context to compact.
    pub async fn compact_now(&self, session_id: &str) -> Result<()> {
        let Some(engine) = self.handle_for(session_id).await else {
            anyhow::bail!("session_engine_not_running");
        };
        engine.compact_now().await?;
        Ok(())
    }

    /// Submit the specified session's request_user_input choice.
    pub async fn submit_user_input(
        &self,
        session_id: &str,
        tool_call_id: String,
        response: UserInputResponse,
    ) -> Result<()> {
        if let Some(engine) = self.handle_for(session_id).await {
            engine.submit_user_input(tool_call_id, response).await?;
        }
        Ok(())
    }

    /// Cancel the specified session's request_user_input.
    pub async fn cancel_user_input(&self, session_id: &str, tool_call_id: String) -> Result<()> {
        if let Some(engine) = self.handle_for(session_id).await {
            engine.cancel_user_input(tool_call_id).await?;
        }
        Ok(())
    }

    /// Mid-turn inject: deliver a user message to the next step boundary of
    /// the current turn. The foundation's `EngineHandle::steer` already
    /// implements it — the turn loop drains the `rx_steer` channel after each
    /// tool result and before the next model call, appending to
    /// session.messages automatically (see foundation turn_loop.rs:493-510).
    /// The model sees the message on its next thinking pass, matching Claude
    /// Code's "insert while the main agent is idle" semantics.
    ///
    /// Returns the opaque steer id (`steer-{n}`) generated at enqueue time;
    /// the frontend uses it to associate the queued chip with
    /// `chat:steer_committed` / `chat:steer_dropped` events.
    ///
    /// Engine absent (session not running) → Err (M-7): there is nothing to
    /// deliver to and no committed/dropped event would ever come; a silent Ok
    /// would leave the frontend chip hanging forever. The frontend takes the
    /// Err into its failure-recovery path.
    pub async fn steer(&self, session_id: &str, content: String) -> Result<String> {
        // One atomic entry read: engine handle + its incarnation. If the pool
        // rebuilds the engine right after this read, we steer on the OLD
        // engine and stamp with ITS incarnation — the pair stays consistent,
        // which is exactly what the stamp exists for.
        let entry = self
            .entries
            .lock()
            .await
            .get(session_id)
            .map(|e| (e.engine.clone(), e.steer_incarnation));
        let (engine, generation) = require_live_engine_for_steer(entry, session_id)?;
        let raw = engine.handle.steer(content).await?;
        Ok(stamp_steer_generation(generation, &raw))
    }

    /// Withdraw a not-yet-injected steer (the ✕ on a frontend queued chip).
    ///
    /// Foundation semantics: a withdrawn steer_id never enters the transcript;
    /// when the engine meets it at any collection/injection point it skips it
    /// and emits exactly one `SteerDropped` (idempotent). Marks survive across
    /// turns; `SyncSession`/`Shutdown` clear them. The withdrawal returns an
    /// explicit outcome (review P1-1 / CodeWhale#30): `"retired"` = the engine
    /// copy is marked withdrawn and will never inject (the host may safely
    /// resend the same message through another path); `"not_pending"` =
    /// committed/settled/unknown (the injection may already be done — the
    /// host must not resend; it waits for steer_committed to render the
    /// bubble). An already-committed id has no side effects and no event —
    /// the frontend removes the chip optimistically and a late committed can
    /// still render the bubble.
    ///
    /// Engine absent → Err: the message never entered the engine, a purely
    /// local frontend removal suffices and no event needs waiting for.
    pub async fn withdraw_steer(&self, session_id: &str, steer_id: String) -> Result<&'static str> {
        let entry = self
            .entries
            .lock()
            .await
            .get(session_id)
            .map(|e| (e.engine.clone(), e.steer_incarnation));
        let (engine, generation) = require_live_engine_for_steer(entry, session_id)?;
        // Generation check before delegation (the raw-id match below is why):
        // a stamped id from a previous engine incarnation must NOT reach the
        // live engine — the foundation compares raw strings, so the live
        // engine's unrelated `steer-1` could be retired by a stale chip's
        // withdrawal. The stale incarnation's steers died with their engine
        // (foundation Drop drains, forwarder aborts first): they can never
        // inject again, but they may have committed before the drop —
        // `not_pending` ("no proof of delivery") is the honest outcome and
        // sends the frontend to its reconcile path instead of a resend.
        // Unstamped (legacy) ids delegate unchanged.
        let delegated = match delegate_steer_withdrawal(&steer_id, generation) {
            SteerWithdrawTarget::Raw(raw) => raw.to_string(),
            SteerWithdrawTarget::StaleGeneration => return Ok("not_pending"),
        };
        let outcome = engine.handle.withdraw_steer(&delegated);
        Ok(match outcome {
            deepseek_tui::core::engine::SteerWithdrawal::Retired => "retired",
            deepseek_tui::core::engine::SteerWithdrawal::NotPending => "not_pending",
        })
    }

    /// Called after a super-permission change. **No static-prompt hot
    /// refresh is needed** — sudo's on/off state is now injected by
    /// `build_send_message_op` every turn via `<system-reminder>`
    /// (see `super_permission::turn_reminder`); `is_enabled()` reads the disk
    /// live each time, so a toggle takes effect automatically on the next
    /// turn. The static prompt keeps only one neutral pointer (to the
    /// per-turn reminder); whether it is stale does not affect behavior.
    ///
    /// This function is kept as a no-op: the call site (set_super_permission)
    /// semantically "gives a notification", but the actual effect comes from
    /// the per-turn injection and does not depend on this.
    pub async fn refresh_all_instructions(&self) {
        let live_count = self.entries.lock().await.len();
        eprintln!(
            "[engine_pool] sudo permission changed; {live_count} live session(s) — \
             new state takes effect next turn via per-turn system-reminder"
        );
    }
}

#[cfg(any(feature = "benchmark-hooks", test))]
fn identity_for_saved_model(bridge: &Pinvou3Bridge, saved: &SavedModel) -> ModelIdentity {
    let effective = bridge.with_session_model(Some(saved.clone()));
    ModelIdentity::new(effective.provider(), effective.model())
}

// Test-only: the active-model identity shortcut has no benchmark-hooks or
// production caller; the eval paths resolve identities through
// `identity_for_saved_model`.
#[cfg(test)]
fn identity_for_active_model(bridge: &Pinvou3Bridge, prefs: &UserPrefs) -> ModelIdentity {
    match prefs.active_model() {
        Some(saved) => identity_for_saved_model(bridge, saved),
        None => ModelIdentity::new(bridge.provider(), bridge.model()),
    }
}

fn default_model_for_new_session_from(
    prefs: &UserPrefs,
    bridge: &Pinvou3Bridge,
) -> (String, Option<String>) {
    match prefs.active_model() {
        Some(model) => (model.model.clone(), Some(model.id.clone())),
        None => (bridge.model(), None),
    }
}

// Test-only snapshot resolution: production eval paths obtain the
// (SavedModel, identity) pair through the pinned selections instead.
#[cfg(test)]
fn resolve_eval_model_selection_from(
    bridge: &Pinvou3Bridge,
    models: &[SavedModel],
    model_id: &str,
) -> Result<(SavedModel, ModelIdentity)> {
    let saved = models
        .iter()
        .find(|model| model.id == model_id)
        .with_context(|| format!("evaluation model ID '{model_id}' was not found"))?;
    let identity = identity_for_saved_model(bridge, saved);
    Ok((saved.clone(), identity))
}

pub(crate) fn user_display_message(text: impl Into<String>) -> Message {
    Message {
        role: deepseek_tui::models::Role::User,
        content: vec![ContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }],
    }
}

async fn persist_scheduled_prompt(
    store: SessionStore,
    session_id: String,
    prompt: String,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let saved = store.load(&session_id)?;
        if !saved.messages.is_empty() {
            bail!(
                "Scheduled initial session '{}' already contains messages",
                session_id
            );
        }
        store.update_messages(
            &session_id,
            vec![Message {
                role: deepseek_tui::models::Role::User,
                content: vec![ContentBlock::Text {
                    text: prompt,
                    cache_control: None,
                }],
            }],
        )
    })
    .await
    .context("Scheduled prompt persistence task failed")??;
    Ok(())
}

async fn wait_for_scheduled_terminal<F, Fut>(
    receiver: &mut tokio::sync::broadcast::Receiver<EngineTurnSignal>,
    engine: &AppEngine,
    cancel: CancellationToken,
    on_started: &mut F,
) -> Result<ScheduledTurnCompletion>
where
    F: FnMut(&str) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let mut active_turn_id: Option<String> = None;
    let mut cancel_requested = false;
    let mut cancel_deadline: Option<tokio::time::Instant> = None;
    loop {
        let cancel_timeout = async {
            match cancel_deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            signal = receiver.recv() => match signal {
                Ok(EngineTurnSignal::Started { turn_id }) => {
                    if let Some(active) = active_turn_id.as_deref() {
                        bail!("Engine started overlapping scheduled turns '{active}' and '{turn_id}'");
                    }
                    active_turn_id = Some(turn_id.clone());
                    on_started(&turn_id).await?;
                    if cancel_requested {
                        engine.handle.cancel_with_reason(
                            deepseek_tui::core::engine::CancelReason::External,
                        );
                    }
                }
                Ok(EngineTurnSignal::Terminal {
                    turn_id,
                    status,
                    error,
                }) if active_turn_id.as_deref() == Some(turn_id.as_str()) => {
                    return Ok(ScheduledTurnCompletion {
                        turn_id,
                        status,
                        error,
                        cancel_requested,
                    });
                }
                Ok(EngineTurnSignal::Terminal { .. }) => {}
                Ok(EngineTurnSignal::ForwarderStopped { error }) => bail!(error),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    bail!("Engine event stream closed before the scheduled turn completed")
                }
            },
            _ = cancel.cancelled(), if !cancel_requested => {
                cancel_requested = true;
                cancel_deadline = Some(
                    tokio::time::Instant::now() + std::time::Duration::from_secs(30),
                );
                if active_turn_id.is_some() {
                    engine.handle.cancel_with_reason(
                        deepseek_tui::core::engine::CancelReason::External,
                    );
                }
            }
            _ = cancel_timeout, if cancel_requested => {
                bail!("Timed out waiting for the scheduled turn to stop");
            }
        }
    }
}

fn resolve_scheduled_model(
    models: &[SavedModel],
    profile: &ScheduledRunProfile,
) -> Result<SavedModel> {
    if let Some(model_id) = profile.model_id.as_deref() {
        let selected = models
            .iter()
            .find(|model| model.id == model_id)
            .with_context(|| {
                format!("此任务绑定的 AI 模型配置已失效，请重新选择 AI 模型并保存任务。缺失配置：{model_id}")
            })?;
        if selected.model != profile.model {
            bail!(
                "此任务绑定的 AI 模型配置已变更，请重新选择 AI 模型并保存任务。配置 {model_id} 从 '{}' 变为 '{}'",
                profile.model,
                selected.model
            );
        }
        return Ok(selected.clone());
    }

    let mut matches = models.iter().filter(|model| model.model == profile.model);
    let selected = matches.next().with_context(|| {
        format!(
            "此任务绑定的 AI 模型已不可用，请重新选择 AI 模型并保存任务。模型：{}",
            profile.model
        )
    })?;
    if matches.next().is_some() {
        bail!(
            "此任务绑定的 AI 模型配置不唯一，请重新选择 AI 模型并保存任务。模型：{}",
            profile.model
        );
    }
    Ok(selected.clone())
}

/// Stable error code surfaced when a session's bound saved model no longer
/// exists (deleted, or regenerated with a fresh id by a detection re-run).
/// The missing model id follows after ": "; the frontend maps this prefix to
/// tri-lingual copy (BT_TABLE `sessionModelStale`) instead of showing the raw
/// backend string.
const SESSION_MODEL_BINDING_STALE_ERROR: &str = "session_model_binding_stale";

fn resolve_spawn_model(
    models: &[SavedModel],
    scheduled_profile: Option<&ScheduledRunProfile>,
    interactive_model_override: Option<&str>,
    scheduled_unattended: bool,
) -> Result<Option<SavedModel>> {
    if scheduled_unattended {
        return scheduled_profile
            .map(|profile| resolve_scheduled_model(models, profile))
            .transpose();
    }
    if let Some(model_id) = interactive_model_override {
        // Same policy as scheduled tasks and the probe path: once a session's
        // bound model config is deleted or regenerated (re-running detection
        // mints fresh ids), never fall back silently to the global active
        // model — the user picked A while the engine would run B. Fail loudly
        // so the user re-selects; the chain must not substitute silently.
        let selected = models
            .iter()
            .find(|model| model.id == model_id)
            .cloned()
            .with_context(|| format!("{SESSION_MODEL_BINDING_STALE_ERROR}: {model_id}"))?;
        return Ok(Some(selected));
    }
    scheduled_profile
        .map(|profile| resolve_scheduled_model(models, profile))
        .transpose()
}

fn resolve_runtime_model_override<F>(
    explicit_model_override: Option<SavedModel>,
    resolve_normal: F,
) -> Result<Option<SavedModel>>
where
    F: FnOnce() -> Result<Option<SavedModel>>,
{
    match explicit_model_override {
        Some(model) => Ok(Some(model)),
        None => resolve_normal(),
    }
}

#[cfg(test)]
// Tests borrow platform::paths::tests::ENV_LOCK (std Mutex) to serialize global env access;
// cargo test runs test threads in parallel, but env-writing tests are mutually serialized, and
// the lock is held across await only inside a current_thread runtime with no reentrant path,
// so it cannot deadlock.
#[allow(clippy::await_holding_lock)]
mod scheduled_model_tests {
    const TEST_SUBMISSION: &str = "sub-test";
    use super::{
        BoundedJoinOutcome, EvalModelSnapshots, ModelIdentity, ModelUpdateRevisions, Op,
        Pinvou3Bridge, PreparedRuntimeState, REBIND_EVICT_GATE_TIMEOUT,
        SESSION_MODEL_BINDING_STALE_ERROR, ScheduledUnattendedGuard, SessionShellManagers,
        SessionTurnLifecycles, SessionTurnLocks, SessionTurnShellTasks, TURN_GATE_AWAIT_TIMEOUT,
        TranscriptOperation, TurnIdentity, bounded_join_while_holding_turn_gate,
        bounded_shutdown_sends, cancel_turn_with_gates, default_model_for_new_session_from,
        delete_chat_session_with_gate, delete_scheduled_run_with_gate, delete_then_forget,
        dispatch_turn_bound_cancel, entry_is_fresh, evict_if_idle_with_gates, generation_matches,
        identity_for_active_model, identity_for_saved_model, quiesce_engine_before_reclaim,
        rebind_evict_with_gates, rebind_evictable, resolve_eval_model_selection_from,
        resolve_runtime_model_override, resolve_scheduled_model, resolve_spawn_model,
        retry_shutdown_sends, scheduled_profile_after_turn_gate, should_still_reap_after_snapshot,
        user_display_message,
    };
    use crate::features::assistant::engine::TurnBoundCancelOps;
    use crate::features::assistant::runtime_model::PreparedRuntimeModel;
    use crate::features::sessions::{ScheduledRunMode, ScheduledRunProfile, SessionStore};
    use crate::platform::credential_store::{CredentialEditAction, CredentialState};
    use crate::platform::prefs::{ImageCapabilityOverride, ModelPreset, SavedModel};
    use crate::platform::test_support::EnvRestore;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    // `EnvRestore` (snapshot + Drop-restore of a set of env vars; the SAFETY
    // premise is that the test holds platform::paths::tests::ENV_LOCK
    // throughout) converges into `platform::test_support`, sharing one
    // implementation with the engine.rs / multiagent regression tests.

    fn isolated_eval_bridge() -> (Pinvou3Bridge, std::path::PathBuf, EnvRestore) {
        let restore = EnvRestore::capture(&[
            "PINVOU3_HOME",
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_MAX_OUTPUT_TOKENS",
            "PINVOU3_SESSION_ARTIFACTS",
        ]);
        let home = std::env::temp_dir().join(format!(
            "pinvou-eval-selection-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::remove_var("DEEPSEEK_MODEL") };
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::remove_var("DEEPSEEK_PROVIDER") };
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::remove_var("DEEPSEEK_BASE_URL") };
        let bridge = Pinvou3Bridge::boot().expect("boot isolated test bridge");
        (bridge, home, restore)
    }

    /// ADR-0006: an engine reclaim must cancel all sub-agents **first** and
    /// send Shutdown **after**. The two ops are FIFO on the same channel;
    /// reversing the order equals no cancel (Shutdown breaks out of the event
    /// loop directly and the session's bare sub-agents keep running as orphan
    /// tasks).
    #[test]
    fn reclaim_cascade_cancels_subagents_before_shutdown() {
        let ops = super::EnginePool::shutdown_cancel_cascade_ops();
        assert!(
            matches!(ops[0], deepseek_tui::core::ops::Op::CancelSubAgents),
            "级联取消必须先行"
        );
        assert!(
            matches!(ops[1], deepseek_tui::core::ops::Op::Shutdown),
            "Shutdown 必须殿后"
        );
    }

    #[test]
    fn idle_reap_keeps_active_turns_scheduled_and_active_sessions() {
        let idle = super::IDLE_EVICT_AFTER_SECS;
        // idle and not active → reclaim.
        assert!(super::should_reap_idle_engine(false, false, false, idle));
        assert!(super::should_reap_idle_engine(
            false,
            false,
            false,
            idle + 1
        ));
        // an in-flight turn (a reservation held or a terminal state being
        // closed) is never reclaimed.
        assert!(!super::should_reap_idle_engine(true, false, false, idle));
        // a scheduled turn in flight (the spawn→submit window) is not
        // reclaimed.
        assert!(!super::should_reap_idle_engine(false, true, false, idle));
        // the currently active session is not reclaimed.
        assert!(!super::should_reap_idle_engine(false, false, true, idle));
        // below the idle threshold → not reclaimed.
        assert!(!super::should_reap_idle_engine(
            false,
            false,
            false,
            idle - 1
        ));
    }

    #[test]
    fn idle_reap_recheck_requires_unchanged_clock_and_still_idle() {
        let idle = super::IDLE_EVICT_AFTER_SECS;
        // no activity since the snapshot: clock not advanced + still idle →
        // allow the reclaim.
        assert!(should_still_reap_after_snapshot(
            false, false, false, idle, 1000, 1000
        ));
        // the activity clock advanced after the snapshot (turn submission /
        // terminal-closing refresh) → skip, even if the idle duration by the
        // old clock still exceeds the threshold.
        assert!(!should_still_reap_after_snapshot(
            false, false, false, idle, 2000, 1000
        ));
        // a new turn was reserved after the snapshot (lifecycle active;
        // reserve_turn does not take the gate) → skip.
        assert!(!should_still_reap_after_snapshot(
            true, false, false, idle, 1000, 1000
        ));
        // a scheduled turn entered the spawn→submit window / the session was
        // opened as active → skip.
        assert!(!should_still_reap_after_snapshot(
            false, true, false, idle, 1000, 1000
        ));
        assert!(!should_still_reap_after_snapshot(
            false, false, true, idle, 1000, 1000
        ));
        // clock unchanged but the idle threshold is no longer met by current
        // values (defensive backstop) → skip.
        assert!(!should_still_reap_after_snapshot(
            false,
            false,
            false,
            idle - 1,
            1000,
            1000
        ));
    }

    #[tokio::test]
    async fn idle_reap_skips_session_with_activity_after_snapshot() {
        // Deterministic regression of the PR #318 review timeline:
        // reap_idle_engines judges the session idle when snapshotting
        // candidates; after the snapshot and before the reclaim takes the
        // lock, the user starts a new turn (reserve_turn can reserve first
        // without the gate; send_reserved_user_message grabs the gate,
        // submits, and refreshes the activity clock). The recheck must spot
        // the activity and skip the reclaim, otherwise the running turn would
        // be closed into Interrupted by the reclaim; the unsubmitted
        // reservation, though kept, would be needlessly resubmitted onto the
        // rebuilt engine.
        let turn_locks = SessionTurnLocks::default();
        let runtime_locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let sid = "session-idle-reap-race";
        let lifecycle = lifecycles.for_session(sid);

        // At snapshot time: idle, activity clock = 1000.
        let snapshot_last_active = 1000_u64;
        let fake_last_active = Arc::new(AtomicU64::new(snapshot_last_active));
        let entry_present = Arc::new(AtomicBool::new(true));
        let reclaimed = Arc::new(AtomicBool::new(false));

        // After the snapshot, send grabs the turn gate first
        // (send_reserved_user_message submits under the lock) while the
        // reaper queues outside the lock.
        let gate = turn_locks.for_session(sid).await;
        let blocker = gate.lock().await;

        let reaper_locks = turn_locks.clone();
        let reaper_runtime_locks = runtime_locks.clone();
        let reaper_lifecycles = lifecycles.clone();
        let probe_last_active = fake_last_active.clone();
        let probe_entry = entry_present.clone();
        let probe_reclaim = reclaimed.clone();
        let reaper = tokio::spawn(async move {
            evict_if_idle_with_gates(
                &reaper_locks,
                &reaper_runtime_locks,
                sid,
                // take_entry: the same recheck sequence as
                // EnginePool::evict_if_idle — recomputed by current values;
                // activity after the snapshot (clock advanced / turn active)
                // → None.
                move || {
                    let probe_last_active = probe_last_active.clone();
                    let probe_entry = probe_entry.clone();
                    let reaper_lifecycles = reaper_lifecycles.clone();
                    async move {
                        let current = probe_last_active.load(Ordering::Acquire);
                        let turn_active =
                            reaper_lifecycles.get(sid).is_some_and(|lc| lc.is_active());
                        if should_still_reap_after_snapshot(
                            turn_active,
                            false,
                            false,
                            // the turn kept running after the snapshot; the
                            // idle duration by the old clock still exceeds
                            // the threshold.
                            super::IDLE_EVICT_AFTER_SECS + 60,
                            current,
                            snapshot_last_active,
                        ) {
                            probe_entry.store(false, Ordering::Release);
                            Some(())
                        } else {
                            None
                        }
                    }
                },
                move |_| {
                    probe_reclaim.store(true, Ordering::Release);
                    async {}
                },
            )
            .await
        });
        tokio::task::yield_now().await;

        // Activity appears after the snapshot: a new turn reserves (no gate,
        // lifecycle turns active) + the submission refreshes the activity
        // clock.
        let reservation = lifecycle
            .reserve()
            .expect("new turn reserve after snapshot");
        fake_last_active.store(2000, Ordering::Release);

        // Release the turn gate; the reaper resumes its recheck.
        drop(blocker);
        drop(gate);
        let evicted = reaper.await.expect("reaper task joins");

        assert!(!evicted, "快照后有活动的会话必须跳过回收");
        assert!(
            entry_present.load(Ordering::Acquire),
            "复核不过时引擎条目不得被移除"
        );
        assert!(
            !reclaimed.load(Ordering::Acquire),
            "复核不过时 reclaim 不得执行"
        );
        assert!(
            reservation.ensure_active().is_ok(),
            "新轮 reservation 必须保持有效"
        );
    }

    #[tokio::test]
    async fn idle_reap_still_reclaims_when_no_activity_after_snapshot() {
        // Control test preventing an over-conservative recheck: with no
        // activity after the snapshot, the reclaim proceeds as usual.
        let turn_locks = SessionTurnLocks::default();
        let runtime_locks = SessionTurnLocks::default();
        let sid = "session-idle-reap-normal";
        let snapshot_last_active = 1000_u64;
        let entry_present = Arc::new(AtomicBool::new(true));
        let reclaimed = Arc::new(AtomicBool::new(false));
        let probe_entry = entry_present.clone();
        let probe_reclaim = reclaimed.clone();

        let evicted = evict_if_idle_with_gates(
            &turn_locks,
            &runtime_locks,
            sid,
            move || {
                let probe_entry = probe_entry.clone();
                async move {
                    if should_still_reap_after_snapshot(
                        false,
                        false,
                        false,
                        super::IDLE_EVICT_AFTER_SECS,
                        snapshot_last_active,
                        snapshot_last_active,
                    ) {
                        probe_entry.store(false, Ordering::Release);
                        Some(())
                    } else {
                        None
                    }
                }
            },
            move |_| {
                probe_reclaim.store(true, Ordering::Release);
                async {}
            },
        )
        .await;

        assert!(evicted, "快照后无活动的空闲会话必须照常回收");
        assert!(!entry_present.load(Ordering::Acquire));
        assert!(reclaimed.load(Ordering::Acquire));
    }

    #[test]
    fn rebind_evictable_blocks_only_real_activity() {
        // review #463: rebind eviction has no idle-duration or active-session
        // gate — only an in-flight/reserved turn or a running scheduled round
        // blocks the reclaim.
        assert!(rebind_evictable(false, false));
        assert!(!rebind_evictable(true, false));
        assert!(!rebind_evictable(false, true));
        assert!(!rebind_evictable(true, true));
    }

    #[tokio::test]
    async fn rebind_eviction_resets_shell_state_for_idle_session() {
        // review #463 M1/M2 regression: the per-session ShellManager pins its
        // cwd at construction and `for_session` is entry().or_insert_with, so a
        // manager surviving the rebind eviction would keep executing bare
        // shell commands in the OLD directory while the rebuilt engine runs in
        // the new one. The turn-scope registry pins that same manager, so it
        // must go too — a surviving registry would diff the next turn's
        // baseline and clean up its jobs against the old manager. The eviction
        // tail must drop both under the gates.
        let turn_locks = SessionTurnLocks::default();
        let runtime_locks = SessionTurnLocks::default();
        let shell_managers = SessionShellManagers::default();
        let turn_shell_tasks = SessionTurnShellTasks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let sid = "session-rebind-evict-idle";
        let _lifecycle = lifecycles.for_session(sid);
        let manager = shell_managers.for_session(sid, PathBuf::from("D:/old-root"));
        turn_shell_tasks.for_session(sid, manager);

        // Same take sequence as EnginePool::evict_if_idle_for_rebind: idle
        // recheck via rebind_evictable, then the entry removal. A resident
        // engine entry is reclaimed…
        let reclaimed = Arc::new(AtomicBool::new(false));
        let probe_reclaim = reclaimed.clone();
        let probe_lifecycles = lifecycles.clone();
        let evicted = rebind_evict_with_gates(
            &turn_locks,
            &runtime_locks,
            &shell_managers,
            &turn_shell_tasks,
            sid,
            move || {
                let probe_lifecycles = probe_lifecycles.clone();
                async move {
                    let turn_active = probe_lifecycles.get(sid).is_some_and(|lc| lc.is_active());
                    rebind_evictable(turn_active, false).then_some(Some(()))
                }
            },
            move |_| {
                probe_reclaim.store(true, Ordering::Release);
                async {}
            },
        )
        .await;

        assert!(evicted, "idle session must be evicted");
        assert!(reclaimed.load(Ordering::Acquire));
        assert!(
            shell_managers.get(sid).is_none(),
            "shell manager must be reset so the next turn rebuilds it against the rebound workspace"
        );
        assert!(
            !turn_shell_tasks.has_registry(sid),
            "turn-scope registry must be reset alongside the shell manager (round-8 M2)"
        );

        // …and an idle session WITHOUT a resident engine still gets both reset
        // (take yields Some(None)): they may exist from an earlier turn even
        // though the engine was already reclaimed.
        let manager = shell_managers.for_session(sid, PathBuf::from("D:/old-root"));
        turn_shell_tasks.for_session(sid, manager);
        let evicted = rebind_evict_with_gates(
            &turn_locks,
            &runtime_locks,
            &shell_managers,
            &turn_shell_tasks,
            sid,
            || async { Some(None::<()>) },
            |_| async {},
        )
        .await;
        assert!(evicted);
        assert!(
            shell_managers.get(sid).is_none(),
            "shell manager reset must not depend on a resident engine entry"
        );
        assert!(
            !turn_shell_tasks.has_registry(sid),
            "turn-scope registry reset must not depend on a resident engine entry"
        );
    }

    #[tokio::test]
    async fn rebind_eviction_skips_turn_started_after_recheck() {
        // review #463 eviction-tail TOCTOU regression: a turn that starts
        // between the command layer's post-migration recheck and the eviction
        // must NOT be killed — the idle recheck under the turn gate observes
        // the reservation and skips the session entirely (no reclaim, no
        // shell manager reset), leaving it for the post-busy report.
        let turn_locks = SessionTurnLocks::default();
        let runtime_locks = SessionTurnLocks::default();
        let shell_managers = SessionShellManagers::default();
        let turn_shell_tasks = SessionTurnShellTasks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let sid = "session-rebind-evict-busy";
        let lifecycle = lifecycles.for_session(sid);
        let manager = shell_managers.for_session(sid, PathBuf::from("D:/old-root"));
        turn_shell_tasks.for_session(sid, manager);

        // A new turn wins the turn gate before the eviction (send path holds
        // the gate while submitting); the eviction queues outside.
        let gate = turn_locks.for_session(sid).await;
        let blocker = gate.lock().await;

        let reclaimed = Arc::new(AtomicBool::new(false));
        let evict_locks = turn_locks.clone();
        let evict_runtime_locks = runtime_locks.clone();
        let evict_shell_managers = shell_managers.clone();
        let evict_turn_shell_tasks = turn_shell_tasks.clone();
        let evict_lifecycles = lifecycles.clone();
        let probe_reclaim = reclaimed.clone();
        let eviction = tokio::spawn(async move {
            rebind_evict_with_gates(
                &evict_locks,
                &evict_runtime_locks,
                &evict_shell_managers,
                &evict_turn_shell_tasks,
                sid,
                move || {
                    let evict_lifecycles = evict_lifecycles.clone();
                    async move {
                        let turn_active =
                            evict_lifecycles.get(sid).is_some_and(|lc| lc.is_active());
                        rebind_evictable(turn_active, false).then_some(())
                    }
                },
                move |_| {
                    probe_reclaim.store(true, Ordering::Release);
                    async {}
                },
            )
            .await
        });
        tokio::task::yield_now().await;

        // The turn starts after the command layer's recheck: reserve (does
        // not take the gate) flips the lifecycle to active.
        let reservation = lifecycle.reserve().expect("new turn reserve");
        drop(blocker);
        drop(gate);

        let evicted = eviction.await.expect("eviction task joins");
        assert!(!evicted, "a session with a fresh turn must be skipped");
        assert!(
            !reclaimed.load(Ordering::Acquire),
            "no reclaim for a session that became busy"
        );
        assert!(
            shell_managers.get(sid).is_some(),
            "shell manager survives a skipped eviction"
        );
        assert!(
            turn_shell_tasks.has_registry(sid),
            "turn-scope registry survives a skipped eviction"
        );
        assert!(
            reservation.ensure_active().is_ok(),
            "the in-flight reservation must stay valid"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn rebind_eviction_gate_timeout_leaves_session_untouched() {
        // review #463 round-8 minor 4 (the previously untested arm): a turn
        // gate held longer than REBIND_EVICT_GATE_TIMEOUT — a scheduled round
        // holds it for its WHOLE duration — must not stall the rebind command
        // (and the process-wide rebind gate behind it). The eviction gives
        // up, counts as not-idle so the command reports the session as
        // post-busy, and touches nothing: no take, no reclaim, no shell-state
        // reset.
        let turn_locks = SessionTurnLocks::default();
        let runtime_locks = SessionTurnLocks::default();
        let shell_managers = SessionShellManagers::default();
        let turn_shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-rebind-evict-gate-timeout";
        let manager = shell_managers.for_session(sid, PathBuf::from("D:/old-root"));
        turn_shell_tasks.for_session(sid, manager);

        // The scheduled round holds the gate for its whole duration; the
        // eviction queues outside until the bounded timeout fires.
        let gate = turn_locks.for_session(sid).await;
        let _blocker = gate.lock().await;

        let take_ran = Arc::new(AtomicBool::new(false));
        let probe_take = take_ran.clone();
        let started = tokio::time::Instant::now();
        let evicted = rebind_evict_with_gates(
            &turn_locks,
            &runtime_locks,
            &shell_managers,
            &turn_shell_tasks,
            sid,
            move || {
                probe_take.store(true, Ordering::Release);
                async { Some(()) }
            },
            |_| async {},
        )
        .await;

        assert!(
            !evicted,
            "a gate held past the timeout counts as not idle (reported post-busy)"
        );
        assert!(
            started.elapsed() >= REBIND_EVICT_GATE_TIMEOUT,
            "the eviction waited the bounded timeout rather than the whole turn"
        );
        assert!(
            !take_ran.load(Ordering::Acquire),
            "the take closure must not run without the gate"
        );
        assert!(
            shell_managers.get(sid).is_some(),
            "shell state survives a timed-out eviction"
        );
        assert!(
            turn_shell_tasks.has_registry(sid),
            "turn-scope registry survives a timed-out eviction"
        );
    }

    fn model(id: &str, wire_name: &str) -> SavedModel {
        SavedModel {
            id: id.to_string(),
            name: id.to_string(),
            alias: None,
            preset: ModelPreset::OpenaiCompatible,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: wire_name.to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: ImageCapabilityOverride::default(),
            vision_model_id: None,
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None::<CredentialEditAction>,
        }
    }

    #[test]
    fn missing_eval_model_error_names_only_the_requested_id() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (bridge, home, _env) = isolated_eval_bridge();
        let configured = model("configured", "wire-model");
        let error = resolve_eval_model_selection_from(&bridge, &[configured], "missing-judge")
            .expect_err("unknown model ID must fail");
        let message = format!("{error:#}");

        assert!(message.contains("missing-judge"));
        assert!(!message.to_ascii_lowercase().contains("api_key"));
        assert!(!message.to_ascii_lowercase().contains("secret"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn identity_uses_actual_bridge_provider_and_model_overrides() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (bridge, home, _env) = isolated_eval_bridge();
        // SAFETY: this test holds platform::paths::tests::ENV_LOCK; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_MODEL", "actual-model") };
        // SAFETY: this test holds platform::paths::tests::ENV_LOCK; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_PROVIDER", "actual-provider") };

        let first = identity_for_saved_model(&bridge, &model("first", "raw-one"));
        let second = identity_for_saved_model(&bridge, &model("second", "raw-two"));

        assert_eq!(first, second);
        assert!(
            crate::features::assistant::eval::validate_judge_identity(&first, &second).is_err()
        );

        // SAFETY: this test holds platform::paths::tests::ENV_LOCK; env writes are serialized.
        unsafe { std::env::remove_var("DEEPSEEK_MODEL") };
        let mut models = vec![model("judge", "snapshot-model")];
        let (saved, identity) = resolve_eval_model_selection_from(&bridge, &models, "judge")
            .expect("resolve saved model");
        let snapshots = EvalModelSnapshots::default();
        let selection = snapshots.pin(saved, identity);
        models[0].model = "changed-after-resolution".to_string();
        assert_eq!(selection.wire_model(), "snapshot-model");
        assert_eq!(selection.model_id(), Some("judge"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn pinned_eval_snapshot_binds_once_and_survives_later_config_changes() {
        let snapshots = EvalModelSnapshots::default();
        let saved = model("judge", "judge-wire");
        let selection = snapshots.pin(
            saved.clone(),
            super::ModelIdentity::new("openai", "judge-wire"),
        );

        snapshots
            .bind_to_session("judge-session", &selection)
            .expect("first bind");
        assert!(
            snapshots
                .bind_to_session("other-session", &selection)
                .is_err()
        );
        assert!(snapshots.saved_models.lock().is_empty());
        let mut latest_models = vec![model("judge", "changed-wire")];
        latest_models.clear();
        assert!(latest_models.is_empty());
        let resolved =
            resolve_runtime_model_override(snapshots.for_session("judge-session"), || {
                panic!("deleted prefs model must not be consulted for a pinned eval session");
                #[allow(unreachable_code)]
                Ok(None)
            })
            .expect("resolve lifecycle pin");
        assert_eq!(resolved, Some(saved));
        let debug = format!("{selection:?}");
        assert!(!debug.contains("example.invalid"));
        assert!(!debug.contains("api_key"));
    }

    #[test]
    fn discarded_eval_snapshot_releases_private_saved_model() {
        let snapshots = EvalModelSnapshots::default();
        let selection = snapshots.pin(
            model("judge", "judge-wire"),
            super::ModelIdentity::new("openai", "judge-wire"),
        );
        assert_eq!(snapshots.saved_models.lock().len(), 1);

        snapshots.discard(&selection);

        assert!(snapshots.saved_models.lock().is_empty());
    }

    #[test]
    fn suite_snapshot_derives_unique_case_selections_from_one_model() {
        let snapshots = EvalModelSnapshots::default();
        let suite = snapshots.pin_suite(
            model("tested-a", "wire-a"),
            super::ModelIdentity::new("provider-a", "wire-a"),
        );
        // Represents preferences switching to B after suite startup. Derivation must not use it.
        let _latest_active = model("tested-b", "wire-b");

        let first = snapshots
            .derive_case_selection(&suite)
            .expect("derive first case");
        let second = snapshots
            .derive_case_selection(&suite)
            .expect("derive second case");

        assert_ne!(first.token(), second.token());
        assert_eq!(first.model_id(), Some("tested-a"));
        assert_eq!(second.model_id(), Some("tested-a"));
        assert_eq!(first.identity(), suite.identity());
        assert_eq!(second.identity(), suite.identity());
    }

    #[test]
    fn discarded_suite_snapshot_cannot_derive_more_case_models() {
        let snapshots = EvalModelSnapshots::default();
        let suite = snapshots.pin_suite(
            model("tested-a", "wire-a"),
            super::ModelIdentity::new("provider-a", "wire-a"),
        );

        snapshots.discard_suite(&suite);

        assert!(snapshots.derive_case_selection(&suite).is_err());
        assert!(snapshots.suite_models.lock().is_empty());
    }

    #[test]
    fn forgetting_eval_session_releases_lifecycle_model_pin() {
        let snapshots = EvalModelSnapshots::default();
        let saved = model("judge", "judge-wire");
        let selection = snapshots.pin(
            saved.clone(),
            super::ModelIdentity::new("openai", "judge-wire"),
        );
        snapshots
            .bind_to_session("judge-session", &selection)
            .expect("bind session");

        snapshots.forget_session("judge-session");

        assert_eq!(snapshots.for_session("judge-session"), None);
        assert!(snapshots.session_models.lock().is_empty());
    }

    #[test]
    fn explicit_eval_override_wins_without_consulting_normal_resolution() {
        let explicit = model("pinned", "pinned-wire");
        let selected = resolve_runtime_model_override(Some(explicit.clone()), || {
            panic!("normal model resolution must not run for an explicit eval snapshot");
            #[allow(unreachable_code)]
            Ok(None)
        })
        .expect("resolve explicit model");

        assert_eq!(selected, Some(explicit));
    }

    #[test]
    fn tested_identity_uses_latest_active_model_instead_of_boot_bridge_model() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (bridge, home, _env) = isolated_eval_bridge();
        let boot_identity = ModelIdentity::new(bridge.provider(), bridge.model());
        let mut prefs = crate::platform::prefs::UserPrefs::default();
        let mut active = model("active-b", "active-model-b");
        active.vendor = Some("kimi".to_string());
        prefs.advanced.saved_models = vec![active];
        prefs.advanced.active_model_id = Some("active-b".to_string());

        let identity = identity_for_active_model(&bridge, &prefs);

        assert_eq!(identity.provider, "moonshot");
        assert_eq!(identity.model, "active-model-b");
        assert_ne!(identity, boot_identity);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn default_eval_metadata_keeps_raw_saved_model_despite_env_override() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (bridge, home, _env) = isolated_eval_bridge();
        // SAFETY: this test holds platform::paths::tests::ENV_LOCK; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_MODEL", "env-wire-override") };
        let mut prefs = crate::platform::prefs::UserPrefs::default();
        prefs.advanced.saved_models = vec![model("active", "raw-saved-wire")];
        prefs.advanced.active_model_id = Some("active".to_string());

        assert_eq!(
            default_model_for_new_session_from(&prefs, &bridge),
            ("raw-saved-wire".to_string(), Some("active".to_string()))
        );
        let _ = std::fs::remove_dir_all(home);
    }

    fn profile(model_id: Option<&str>, wire_name: &str) -> ScheduledRunProfile {
        ScheduledRunProfile {
            task_id: "task-1".to_string(),
            model: wire_name.to_string(),
            model_id: model_id.map(str::to_string),
            workspace: PathBuf::from("D:/workspace"),
            mode: ScheduledRunMode::Yolo,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
        }
    }

    #[test]
    fn configured_model_is_resolved_by_stable_id_and_wire_name() {
        let models = vec![model("other", "other-model"), model("wanted", "wire-model")];
        let selected = resolve_scheduled_model(&models, &profile(Some("wanted"), "wire-model"))
            .expect("configured model");
        assert_eq!(selected.id, "wanted");
    }

    #[test]
    fn unattended_spawn_uses_task_model_despite_interactive_override() {
        let models = vec![
            model("task-model", "task-wire"),
            model("interactive-model", "interactive-wire"),
        ];
        let scheduled = profile(Some("task-model"), "task-wire");
        let unattended =
            resolve_spawn_model(&models, Some(&scheduled), Some("interactive-model"), true)
                .expect("unattended model")
                .expect("selected unattended model");
        assert_eq!(unattended.id, "task-model");

        let interactive =
            resolve_spawn_model(&models, Some(&scheduled), Some("interactive-model"), false)
                .expect("interactive model")
                .expect("selected interactive model");
        assert_eq!(interactive.id, "interactive-model");
    }

    #[test]
    fn deleted_or_changed_configured_model_never_falls_back_to_active() {
        let models = vec![
            model("active", "active-model"),
            model("wanted", "renamed-model"),
        ];
        assert!(resolve_scheduled_model(&models, &profile(Some("missing"), "wire-model")).is_err());
        assert!(resolve_scheduled_model(&models, &profile(Some("wanted"), "wire-model")).is_err());
    }

    /// Session-scoped model selection follows the same policy as scheduled
    /// tasks: once the bound config is deleted or regenerated (a detection
    /// re-run mints fresh ids), the stale binding must surface a stable-code
    /// error instead of silently falling back to the global active model —
    /// otherwise the user picks A while the engine loads B (the local LM
    /// Studio chain break reported by users).
    #[test]
    fn interactive_override_missing_id_never_falls_back_to_active() {
        let models = vec![model("active", "active-model")];
        let error = resolve_spawn_model(&models, None, Some("deleted-id"), false)
            .expect_err("stale session binding must surface, not fall back to active");
        let message = error.to_string();
        assert!(
            message.starts_with(SESSION_MODEL_BINDING_STALE_ERROR)
                && message.contains("deleted-id"),
            "error must carry the stable code and the missing config id, got: {message}"
        );
        // An override that still exists resolves as usual.
        let models = vec![model("active", "active-model"), model("kept", "kept-wire")];
        assert_eq!(
            resolve_spawn_model(&models, None, Some("kept"), false)
                .expect("resolvable override")
                .expect("selected model")
                .id,
            "kept"
        );
    }

    #[test]
    fn legacy_profile_without_id_requires_one_unambiguous_wire_name() {
        let one = vec![model("one", "wire-model")];
        assert_eq!(
            resolve_scheduled_model(&one, &profile(None, "wire-model"))
                .expect("unique model")
                .id,
            "one"
        );
        let duplicates = vec![model("one", "wire-model"), model("two", "wire-model")];
        assert!(resolve_scheduled_model(&duplicates, &profile(None, "wire-model")).is_err());
    }

    #[test]
    fn unattended_policy_is_scoped_to_the_executor_turn() {
        let flag = Arc::new(AtomicBool::new(false));
        {
            let _guard = ScheduledUnattendedGuard::enter(flag.clone());
            assert!(flag.load(Ordering::Acquire));
        }
        assert!(!flag.load(Ordering::Acquire));
    }

    #[test]
    fn session_shell_manager_is_reused_across_engine_rebuilds() {
        let managers = SessionShellManagers::default();
        let first = managers.for_session("session-1", PathBuf::from("D:/workspace-a"));
        let rebuilt = managers.for_session("session-1", PathBuf::from("D:/workspace-b"));
        assert!(Arc::ptr_eq(&first, &rebuilt));
        drop(first);
        assert!(
            Arc::strong_count(&rebuilt) >= 2,
            "the session registry must keep detached jobs alive after an Engine entry drops"
        );
        managers.remove("session-1");
        assert!(managers.get("session-1").is_none());
    }

    #[test]
    fn mcp_config_revision_bump_forces_next_turn_rebuild() {
        // A pin for the mark_mcp_config_updated contract (the counterpart of the
        // model path's requires_rebuild_from predicate): when the model is
        // unchanged and only the mcp config revision is bumped, the old entry must
        // be judged stale for the next turn — a plain session's engine reads the
        // session-derived mcp config rewritten only at spawn; without a rebuild,
        // servers installed midway would never be visible.
        assert!(entry_is_fresh(false, 7, 7));
        assert!(!entry_is_fresh(false, 7, 8), "mcp 配置修订递增必须触发重建");
        assert!(!entry_is_fresh(true, 7, 7), "模型变更路径保持原有判定");
        // The uniquely discriminating combination: an XNOR-shaped mutant
        // (fresh = model unchanged ⇔ revision unchanged) slips past the three
        // asserts above; only this one catches it (a model and revision double
        // change must not be misjudged as fresh).
        assert!(
            !entry_is_fresh(true, 7, 8),
            "model change plus revision bump together must be judged stale"
        );
    }

    #[test]
    fn rebuild_does_not_swallow_inflight_reservation() {
        // #253 regression (deterministic composition, no real engine): saving
        // the model/credentials (the mark_model_updated revision bump) -> the
        // sender reserves -> the next get_or_spawn sees requires_rebuild_from
        // -> the old engine gets reclaimed. Before the fix, the !submitted
        // branch of claim_reclaimed_transition invalidated this freshly
        // reserved reservation, send_reserved_turn_op's ensure_active then
        // failed, and the user message was silently dropped; after the fix
        // (#385) the unsubmitted reservation survives the reclaim and the
        // message is submitted to the rebuilt engine.
        let revisions = ModelUpdateRevisions::default();
        let prepared = PreparedRuntimeModel::unchanged(model("model-1", "wire-model"));
        let entry_state = PreparedRuntimeState::new(prepared.clone(), revisions.current("model-1"));

        let lifecycles = SessionTurnLifecycles::default();
        let sid = "session-rebuild-reservation";
        let lifecycle = lifecycles.for_session(sid);
        let mut reservation = lifecycle.reserve().expect("reserve before model save");
        reservation
            .set_transcript(
                TranscriptOperation::Append,
                user_display_message("saved-model first message"),
            )
            .expect("display transcript");

        // Saving the model: after the revision bump the next turn's prepared
        // state necessarily differs -> a rebuild is required.
        revisions.bump("model-1");
        let next_turn_state = PreparedRuntimeState::new(prepared, revisions.current("model-1"));
        assert!(
            next_turn_state.requires_rebuild_from(&entry_state),
            "saved-model revision must force the next turn to rebuild the engine"
        );

        // The get_or_spawn rebuild path touches two lifecycle write points:
        // the authoritative reclaim claim must not hit an unsubmitted turn,
        // and rule pruning must not touch the in-flight reservation's rules.
        assert!(
            !lifecycle.claim_reclaimed_once(),
            "reclaim must not claim a turn that never reached the engine mailbox"
        );
        lifecycle.prune_stale_transcript_rules();

        // The respawned engine shares the session-scoped lifecycle
        // (spawn_for_session passes the same Arc), so the send-side
        // ensure_active must pass; the rebuild must also not reset busy —
        // the in-flight reservation still occupies the single turn slot.
        assert!(
            Arc::ptr_eq(&lifecycle, &lifecycles.for_session(sid)),
            "respawned engine must share the session-scoped lifecycle"
        );
        reservation
            .ensure_active()
            .expect("unsubmitted reservation must survive the model-update rebuild");
        assert!(lifecycle.reserve().is_err());
    }

    #[tokio::test]
    async fn runtime_preparation_locks_are_isolated_between_sessions() {
        let locks = SessionTurnLocks::default();
        let first = locks.for_session("session-1").await;
        let second = locks.for_session("session-2").await;
        assert!(!Arc::ptr_eq(&first, &second));

        let _first_guard = first.lock().await;
        let _second_guard = tokio::time::timeout(std::time::Duration::from_secs(1), second.lock())
            .await
            .expect("a slow provider for one session must not block another session");
    }

    #[test]
    fn turn_lifecycle_survives_engine_entry_removal_without_faking_idle_cancel() {
        let lifecycles = SessionTurnLifecycles::default();
        let engine_lifecycle = lifecycles.for_session("session-1");
        engine_lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string()));
        drop(engine_lifecycle);

        let pool_lifecycle = lifecycles.get("session-1").expect("session lifecycle");
        assert!(pool_lifecycle.finish_once(|| {}).is_some());
        assert_eq!(pool_lifecycle.finish_once(|| panic!("duplicate")), None);
        lifecycles.remove("session-1");
        assert!(lifecycles.get("session-1").is_none());
    }

    #[test]
    fn steer_without_live_engine_is_an_error() {
        // M-7: steer must return Err when the engine is absent (the session
        // is not running) — a silent Ok would leave the frontend queued chip
        // waiting forever for steer_committed/steer_dropped. EnginePool
        // construction depends on AppHandle and bridge boot and cannot be
        // instantiated in unit tests, so the contract is covered on
        // require_live_engine_for_steer instead.
        let err = super::require_live_engine_for_steer(None::<super::AppEngine>, "session-1")
            .err()
            .expect("steer without a live engine must fail");
        assert!(format!("{err:#}").contains("no live engine"));
    }

    #[test]
    fn swarm_mode_availability_excludes_scheduled_sessions() {
        // The swarm regime must track the engine-side scheduled_profile gate:
        // a scheduled session assembles plain engine config, so its switch —
        // even if still on — must not inject swarm copy, must refuse to
        // enable, and must report unavailable to the frontend. Availability
        // alone stays true for a plain session and false for an unsupported
        // lane; transcript listing keeps using multi_agent_mode_available
        // (scheduled runs delegate via bare `agent` and stay readable).
        assert!(super::EnginePool::swarm_mode_available_for(true, false));
        assert!(!super::EnginePool::swarm_mode_available_for(true, true));
        assert!(!super::EnginePool::swarm_mode_available_for(false, true));
        assert!(!super::EnginePool::swarm_mode_available_for(false, false));
    }

    #[test]
    fn steer_id_generation_stamp_parse_round_trip() {
        // Regression (eighth-review round): foundation steer ids are ordinals
        // (`steer-{n}`) that reset on engine rebuild. The pool stamps the
        // engine generation onto ids it returns; parse must recover the exact
        // pair so withdrawals can be generation-checked.
        let stamped = super::stamp_steer_generation(1_725_012_345_678, "steer-3");
        assert_eq!(stamped, "e1725012345678-steer-3");
        assert_eq!(
            super::parse_steer_generation(&stamped),
            Some((1_725_012_345_678, "steer-3"))
        );
        // Raw ids with dashes survive the round trip (the first dash splits).
        assert_eq!(
            super::parse_steer_generation("e42-e1725012345678-steer-3"),
            Some((42, "e1725012345678-steer-3"))
        );
    }

    #[test]
    fn unstamped_steer_ids_parse_as_legacy_passthrough() {
        // Legacy / foreign ids must not be misread as stamped: the pool
        // delegates them untranslated so an id that happens to look ordinal
        // still reaches the engine it was minted by.
        assert_eq!(super::parse_steer_generation("steer-3"), None);
        assert_eq!(super::parse_steer_generation(""), None);
        assert_eq!(super::parse_steer_generation("esteer-3"), None);
        assert_eq!(super::parse_steer_generation("e-nan-steer"), None);
        assert_eq!(
            super::parse_steer_generation("e18446744073709551616-x"),
            None
        );
    }

    #[test]
    fn steer_incarnations_stay_unique_across_same_tick_rebuilds() {
        // Regression (zhuowp re-review P1-2): the generation stamp source must
        // be unique per engine rebuild within the process. Wall-clock
        // milliseconds collide for two rebuilds inside one tick (or after a
        // clock rollback); the pool's incarnation sequence is time-independent,
        // so consecutive allocations — regardless of what the clock does —
        // mint distinct generations and therefore distinct stamped ids.
        use std::sync::atomic::AtomicU64;
        let seq = AtomicU64::new(0);
        let first = seq.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let second = seq.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        assert_ne!(first, second, "incarnations must never repeat");
        // Deterministic replay of the collision scenario: two engine rebuilds
        // with the SAME spawned_at_ms still get distinct steer generations,
        // so a chip minted by the first cannot match the second.
        let first_ids = (1..=2)
            .map(|n| super::stamp_steer_generation(first, &format!("steer-{n}")))
            .collect::<Vec<_>>();
        let second_ids = (1..=2)
            .map(|n| super::stamp_steer_generation(second, &format!("steer-{n}")))
            .collect::<Vec<_>>();
        assert_ne!(first_ids, second_ids);
        // Foundation ordinals restart at steer-1 on every rebuild; only the
        // incarnation keeps them apart.
        assert_eq!(first_ids[0], format!("e{first}-steer-1"));
        assert_eq!(second_ids[0], format!("e{second}-steer-1"));
        assert_ne!(first_ids[0], second_ids[0]);
    }

    #[test]
    fn stale_incarnation_withdrawal_never_delegates_on_same_tick_rebuild() {
        // Regression (zhuowp re-review P1-2): a chip stamped by engine
        // incarnation N must never have its withdrawal delegated into a
        // rebuilt engine N+1, even when both rebuilds share one
        // spawned_at_ms tick — the raw ordinal `steer-1` exists in BOTH
        // engines, and a mis-delegation would retire the new engine's
        // unrelated steer.
        let first_engine = 7u64;
        let second_engine = 8u64; // same spawned_at_ms, next incarnation
        let stale_chip = super::stamp_steer_generation(first_engine, "steer-1");
        assert_eq!(
            super::delegate_steer_withdrawal(&stale_chip, first_engine),
            super::SteerWithdrawTarget::Raw("steer-1")
        );
        assert_eq!(
            super::delegate_steer_withdrawal(&stale_chip, second_engine),
            super::SteerWithdrawTarget::StaleGeneration
        );
        // The rebuilt engine's own chip delegates normally.
        let live_chip = super::stamp_steer_generation(second_engine, "steer-1");
        assert_eq!(
            super::delegate_steer_withdrawal(&live_chip, second_engine),
            super::SteerWithdrawTarget::Raw("steer-1")
        );
        // Unstamped legacy ids keep delegating verbatim.
        assert_eq!(
            super::delegate_steer_withdrawal("steer-1", second_engine),
            super::SteerWithdrawTarget::Raw("steer-1")
        );
    }

    #[tokio::test]
    async fn scheduled_close_and_concurrent_followup_share_one_session_gate() {
        let locks = SessionTurnLocks::default();
        let scheduled_gate = locks.for_session("scheduled-session").await;
        let followup_gate = locks.for_session("scheduled-session").await;
        assert!(Arc::ptr_eq(&scheduled_gate, &followup_gate));

        let scheduled_guard = scheduled_gate.lock().await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), followup_gate.lock())
                .await
                .is_err()
        );
        drop(scheduled_guard);
        let _followup_guard =
            tokio::time::timeout(std::time::Duration::from_secs(1), followup_gate.lock())
                .await
                .expect("follow-up acquires only after scheduled close");
    }

    #[tokio::test]
    async fn scheduled_delete_wins_over_waiting_followup_without_resurrecting_state() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = std::env::temp_dir().join(format!(
            "pinvou3-engine-pool-delete-race-{}",
            std::process::id()
        ));
        let previous_home = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&home);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };

        let store = SessionStore::boot().expect("session store");
        let session_id = store
            .create_scheduled_run(ScheduledRunProfile {
                task_id: "task-delete-race".to_string(),
                model: "wire-model".to_string(),
                model_id: None,
                workspace: home.join("workspace"),
                mode: ScheduledRunMode::Agent,
                allow_shell: false,
                trust_mode: false,
                auto_approve: false,
            })
            .expect("scheduled session")
            .metadata
            .id;
        let locks = SessionTurnLocks::default();
        let gate = locks.for_session(&session_id).await;
        let blocker = gate.lock().await;
        let fake_engine_present = Arc::new(AtomicBool::new(true));

        let delete_locks = locks.clone();
        let delete_store = store.clone();
        let delete_id = session_id.clone();
        let delete_engine = fake_engine_present.clone();
        let delete = tokio::spawn(async move {
            delete_scheduled_run_with_gate(
                &delete_locks,
                &delete_store,
                &delete_id,
                "task-delete-race",
                || async move {
                    delete_engine.store(false, Ordering::Release);
                },
            )
            .await
        });
        tokio::task::yield_now().await;

        let followup_locks = locks.clone();
        let followup_store = store.clone();
        let followup_id = session_id.clone();
        let followup = tokio::spawn(async move {
            let gate = followup_locks.for_session(&followup_id).await;
            let _turn = gate.lock().await;
            scheduled_profile_after_turn_gate(&followup_store, &followup_id, "task-delete-race")
        });
        tokio::task::yield_now().await;
        drop(blocker);
        drop(gate);

        delete
            .await
            .expect("delete task joins")
            .expect("delete run");
        assert!(
            followup.await.expect("follow-up task joins").is_err(),
            "a follow-up already waiting on the gate must fail after deletion"
        );
        assert!(!fake_engine_present.load(Ordering::Acquire));
        assert!(!store.scheduled_session_exists(&session_id));
        assert!(store.scheduled_profile(&session_id).is_none());
        let probe = locks.for_session("turn-lock-prune-probe").await;
        assert!(!locks.locks.lock().await.contains_key(&session_id));
        drop(probe);

        match previous_home {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn engine_reclaim_quiesces_event_producer_before_persistence() {
        let order = Arc::new(StdMutex::new(Vec::new()));
        let cancel_order = order.clone();
        let abort_order = order.clone();
        let persist_order = order.clone();

        let result = quiesce_engine_before_reclaim(
            move || cancel_order.lock().unwrap().push("cancel"),
            move || async move {
                abort_order.lock().unwrap().push("abort");
                tokio::task::yield_now().await;
                abort_order.lock().unwrap().push("joined");
            },
            move || async move {
                persist_order.lock().unwrap().push("persist");
                42
            },
        )
        .await;

        assert_eq!(result, 42);
        assert_eq!(
            *order.lock().unwrap(),
            vec!["cancel", "abort", "joined", "persist"]
        );
    }

    #[test]
    fn chat_delete_failure_preserves_runtime_and_error() {
        let forget_count = Arc::new(AtomicUsize::new(0));
        let observed_count = forget_count.clone();

        let error = delete_then_forget(
            || Err(anyhow::anyhow!("sentinel delete failure")),
            move || {
                forget_count.fetch_add(1, Ordering::SeqCst);
            },
        )
        .expect_err("injected deletion must fail");

        assert_eq!(observed_count.load(Ordering::SeqCst), 0);
        assert_eq!(error.to_string(), "sentinel delete failure");
    }

    #[tokio::test]
    async fn chat_delete_keeps_evict_delete_and_forget_ahead_of_waiting_send() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = std::env::temp_dir().join(format!(
            "pinvou3-engine-pool-chat-delete-race-{}",
            std::process::id()
        ));
        let previous_home = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&home);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };

        let store = SessionStore::boot().expect("session store");
        let session_id = store
            .create_new("wire-model".to_string(), None, home.join("workspace"))
            .expect("chat session")
            .metadata
            .id;
        let locks = SessionTurnLocks::default();
        let gate = locks.for_session(&session_id).await;
        let blocker = gate.lock().await;
        let fake_engine_present = Arc::new(AtomicBool::new(true));
        let forgotten = Arc::new(AtomicBool::new(false));

        let delete_locks = locks.clone();
        let delete_store = store.clone();
        let delete_id = session_id.clone();
        let delete_engine = fake_engine_present.clone();
        let delete_forgotten = forgotten.clone();
        let delete = tokio::spawn(async move {
            delete_chat_session_with_gate(
                &delete_locks,
                &delete_store,
                &delete_id,
                || async move {
                    delete_engine.store(false, Ordering::Release);
                },
                || delete_forgotten.store(true, Ordering::Release),
            )
            .await
        });
        tokio::task::yield_now().await;

        let send_locks = locks.clone();
        let send_store = store.clone();
        let send_id = session_id.clone();
        let waiting_send = tokio::spawn(async move {
            let gate = send_locks.for_session(&send_id).await;
            let _turn = gate.lock().await;
            send_store.load(&send_id)
        });
        tokio::task::yield_now().await;
        drop(blocker);
        drop(gate);

        delete
            .await
            .expect("delete task joins")
            .expect("delete chat");
        assert!(
            waiting_send.await.expect("send task joins").is_err(),
            "a sender queued on the gate must observe the completed delete"
        );
        assert!(!fake_engine_present.load(Ordering::Acquire));
        assert!(forgotten.load(Ordering::Acquire));
        assert!(store.load(&session_id).is_err());

        match previous_home {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(home);
    }

    /// Regression: the eval teardown paths (`EnginePoolRuntime::close` /
    /// `delete_eval_session`) reuse delete_chat_session, but submit already
    /// ran `timing::start_turn`; deletion must clear the session's unpaired
    /// queue key in ACTIVE_TURNS, or a single GAIA pass's ~165 create/delete
    /// cycles would grow the process-level map unbounded.
    #[tokio::test]
    async fn chat_delete_clears_queued_turn_timing_state() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = std::env::temp_dir().join(format!(
            "pinvou3-engine-pool-chat-delete-timing-{}",
            std::process::id()
        ));
        let previous_home = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&home);
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };

        let store = SessionStore::boot().expect("session store");
        let session_id = store
            .create_new("wire-model".to_string(), None, home.join("workspace"))
            .expect("chat session")
            .metadata
            .id;
        // The eval submit path runs start_turn first; on submit failure or interruption the key stays resident, and deletion is the backstop.
        crate::features::assistant::timing::start_turn(&session_id);
        assert!(
            crate::features::assistant::timing::has_queued_active_turn(&session_id),
            "precondition: turn already queued"
        );

        let locks = SessionTurnLocks::default();
        delete_chat_session_with_gate(&locks, &store, &session_id, || async {}, || {})
            .await
            .expect("delete chat");

        assert!(
            !crate::features::assistant::timing::has_queued_active_turn(&session_id),
            "delete_chat_session must clear the session's unpaired turn queue"
        );

        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe {
            match previous_home {
                Some(value) => std::env::set_var("PINVOU3_HOME", value),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn one_shot_turn_locks_do_not_accumulate() {
        let locks = SessionTurnLocks::default();

        for index in 0..128 {
            let gate = locks.for_session(&format!("one-shot-{index}")).await;
            let _guard = gate.lock().await;
        }

        assert!(
            locks.locks.lock().await.len() <= 1,
            "dead per-session turn gates must be reclaimed"
        );
    }

    // ── cancel turn generation guard (review #4872749559 cross-turn
    //    mis-cancel regression) ──────
    // cancel_turn_with_gates extracts the cancel body from EnginePool::cancel
    // so these three test groups can choreograph C1/C2 timelines
    // deterministically with bare Default components + probe closures,
    // bypassing bridge / AppHandle / a real EngineHandle (private across
    // crates).

    #[test]
    fn generation_matches_treats_idle_to_idle_as_match() {
        // (None, None): canceling an idle session is a no-op anyway — counts
        // as a match and goes through the original logic.
        assert!(generation_matches(None, None));
        // same-turn epoch: match.
        assert!(generation_matches(Some(1), Some(1)));
        // different epochs (the target turn has ended and a new turn has been
        // reserved): mismatch → no-op.
        assert!(!generation_matches(Some(1), Some(2)));
        // crossing Some/None (target turn active, currently idle, or vice
        // versa): mismatch → no-op.
        assert!(!generation_matches(Some(1), None));
        assert!(!generation_matches(None, Some(1)));
    }

    #[tokio::test]
    async fn stale_cancel_after_turn_change_leaves_new_turn_intact() {
        // Positive verification of the reviewer's timeline (review
        // #4872749559):
        //   C1/C2 cancel turn1 concurrently. C1 runs to completion first
        //   (cancels turn1 + emits the terminal + cleans up shell), and turn2
        //   reserves ahead via reserve_turn (which does not take turn_lock)
        //   before C1 releases the lock.
        //   C2's **phase one** (lock-free cancel_current) still belongs to
        //   turn1 at that point (correctly, it cancels turn1);
        //   the real defect window is in **phase two**: after C2 takes the
        //   lock it is already turn2, and the original implementation would
        //   indiscriminately cancel/arm pending onto turn2. The generation
        //   guard makes phase two early-return.
        //
        // Here we choreograph "C2 snapshots turn1 → turn1 terminal → turn2
        // reserve → C2 phase two" directly:
        // first occupy turn_lock with a blocker so C2's phase one snapshots
        // turn1 and then suspends in phase two; the main thread advances to
        // turn2, then releases the lock to let C2's phase two resume.
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-stale-cancel";

        let lifecycle = lifecycles.for_session(sid);
        // turn1: on_submitted activates it (active+submitted+epoch incremented),
        // so phase one's cancel_engine can match the generation and finish_once
        // can claim (which requires submitted).
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));

        let gate = locks.for_session(sid).await;
        let blocker = gate.lock().await;

        // Phase-two-side probe: set if phase two executes the cancel_engine
        // recheck.
        let phase_two_cancel_called = Arc::new(AtomicBool::new(false));
        let phase_one_count = Arc::new(AtomicU64::new(0));
        let probe2 = phase_two_cancel_called.clone();
        let probe1 = phase_one_count.clone();
        // cascade probe: when phase two early-returns (a new turn has been
        // reserved), the cascade cancel must not run — a CancelSubAgents
        // emitted now would kill the new turn's just-started sub-agents by
        // mistake (reviewer point 4).
        let cascade_called = Arc::new(AtomicBool::new(false));
        let probe_cascade = cascade_called.clone();
        // phase 1's cascade cancel is treated as successfully enqueued (this
        // test does not cover reviewer point 9's try_send-failure scenario;
        // it keeps the existing assertion semantics of mismatch
        // early-return).
        // C2: phase one snapshots target=Some(1) lock-free → match →
        //     get_engine returns the in-pool engine
        //     → epoch still matches → arm + cancel_current (probe1++);
        //     phase two holds the lock (blocked by the blocker); after
        //     resuming, current ≠ target → early return.
        let cancel_task = tokio::spawn(async move {
            cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                deepseek_tui::core::engine::CancelMode::StopDropInbox,
                // get_engine: engine present (phase one and the phase-two
                // recheck both return Some).
                || async { Some(()) },
                // cancel_current: both the phase-one and phase-two rechecks come here;
                // distinguished by counters. Phase one (turn1) moves probe1 0→1; if phase
                // two mistakenly executes, probe2 is set.
                move |_engine: &(), _identity: Option<TurnIdentity>| {
                    let prev = probe1.fetch_add(1, Ordering::AcqRel);
                    if prev >= 1 {
                        probe2.store(true, Ordering::Release);
                    }
                },
                // cascade_cancel: phase two early-returns — must not be
                // called.
                move |_engine: &()| {
                    probe_cascade.store(true, Ordering::Release);
                    async {}
                },
                // claim_unsubmitted must not be called (turn1 is already
                // submitted).
                |_lc, _target| false,
            )
            .await
        });
        // Let C2 progress to: phase one complete, phase two suspended at
        // gate.lock().await.
        tokio::task::yield_now().await;

        // turn1 terminal (submitted → claim path) → turn2 reserve (epoch=2).
        assert!(lifecycle.finish_once(|| {}).is_some());
        let reservation2 = lifecycle.reserve().expect("turn2 reserve");

        // Release turn_lock; C2's phase two resumes: current=Some(2) ≠
        // target=Some(1) → early return.
        drop(blocker);
        drop(gate);
        cancel_task.await.expect("cancel task joins");

        // phase one ran exactly once (canceling turn1 — correct).
        assert_eq!(
            phase_one_count.load(Ordering::Acquire),
            1,
            "phase one cancel on the originating turn must run exactly once"
        );
        // phase two did not wrongly run cancel_engine (the guard held) and
        // turn2 is not canceled by mistake.
        assert!(
            !phase_two_cancel_called.load(Ordering::Acquire),
            "phase two must not cancel the engine of a turn that started after the cancel was issued"
        );
        // After simplification ③ (cascade_queued removed): when the new turn
        // is only reserved, not submitted, the mismatch branch resends the
        // cascade per should_retry_cascade — the engine still has only the
        // old turn's leftover sub-agents (SendMessage needs the same
        // turn_lock, held by this function), so the resend cancels only the
        // old turn's sub-agents and cannot hit the new turn
        // (submitted=false). CancelSubAgents is idempotent — safe.
        assert!(
            cascade_called.load(Ordering::Acquire),
            "stale cancel must still cascade old subagents when the new turn is only reserved (not submitted)"
        );
        assert!(
            reservation2.ensure_active().is_ok(),
            "new turn reservation must remain valid after a stale cancel's phase two recovered"
        );
    }

    // issue #255: phase two must not hold the turn gate unboundedly. The
    // cascade closure models a wedged engine (ops channel full, never
    // drained): `cancel_turn_with_gates` must return after the bound and
    // release the gate so evict / delete / send can proceed. On unbounded
    // code the cancel never settles; the 2× outer wrapper turns that
    // regression into a fast red instead of a hung test binary (the
    // bound-removal regression itself was reverse-verified red on main by
    // the original hanging form). The wait is the real 5s
    // TURN_GATE_AWAIT_TIMEOUT — deliberate: enabling tokio's test-util for
    // a paused clock would change the tokio feature set and invalidate the
    // whole CI test cache for one test. The forkguard_ prefix registers it
    // as a fork-guard layer-3 behavior test (fork-policy §3).
    #[tokio::test]
    async fn forkguard_cancel_holds_turn_lock_boundedly() {
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-cancel-bounded";

        let lifecycle = lifecycles.for_session(sid);
        assert!(lifecycle.on_submitted(None));

        // get_engine: None first (phase one — engine still spawning), Some
        // afterwards (phase two finds it). This deterministically routes the
        // cancel through phase two's first arm and the primary cascade send
        // without relying on the second-arm-on-the-same-turn semantics.
        let probe_calls = Arc::new(AtomicU64::new(0));
        let engine_calls = probe_calls.clone();
        let cascade_started = Arc::new(AtomicBool::new(false));
        let cascade_probe = cascade_started.clone();
        let gate_locks = locks.clone();
        // Measured from before the spawn: the production bound timer starts
        // when phase two first polls the cascade, so elapsed >= the bound is
        // guaranteed and pins "the cancel really waited the full budget"
        // (an unbounded regression hangs; a silently shrunken bound fails
        // this assert).
        let cancel_started = std::time::Instant::now();
        let cancel_task = tokio::spawn(async move {
            cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                deepseek_tui::core::engine::CancelMode::StopDropInbox,
                move || {
                    let call = engine_calls.fetch_add(1, Ordering::AcqRel);
                    async move { (call > 0).then_some(()) }
                },
                |_engine: &(), _identity: Option<TurnIdentity>| {},
                move |_engine: &()| {
                    cascade_probe.store(true, Ordering::Release);
                    std::future::pending::<()>()
                },
                |_lc, _target| false,
            )
            .await
        });
        // Phase two reached the primary cascade send, then parks on the
        // never-settling wedged engine. Bounded so a regression that never
        // reaches the cascade fails with a diagnostic instead of hanging.
        tokio::time::timeout(TURN_GATE_AWAIT_TIMEOUT, async {
            while !cascade_started.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("phase two must reach the primary cascade send");
        // The cascade is parked on the wedged engine: cancel must come back
        // via TURN_GATE_AWAIT_TIMEOUT (≈5s real time), not via the closure.
        // The 2× wrapper keeps an unbounded regression a fast red (with a
        // diagnostic) instead of a hung test binary.
        let (target, claimed) = tokio::time::timeout(2 * TURN_GATE_AWAIT_TIMEOUT, cancel_task)
            .await
            .expect(
                "cancel must settle within twice the gate bound; an unbounded regression would hang here",
            )
            .expect("cancel task joins");
        assert!(cancel_started.elapsed() >= TURN_GATE_AWAIT_TIMEOUT);
        assert_eq!(target, Some(1));
        assert!(!claimed);
        // The gate is free again: evict / delete / send queue on it and must
        // not be stuck behind the cancelled turn (issue #255). Bounded so a
        // gate-leak regression fails fast instead of hanging.
        tokio::time::timeout(TURN_GATE_AWAIT_TIMEOUT, async {
            let gate = gate_locks.for_session(sid).await;
            drop(gate.lock().await);
        })
        .await
        .expect("turn gate must be re-acquirable after the bound");
    }

    // issue #255 hardening: a panicked side-effect task must count as "not
    // settled". A JoinError silently counted as a timely completion would let
    // the reclaim terminal claim a verified-clean shell state that was never
    // verified, and — with forwarder.abort() removing the fallback finalizer —
    // leave finalize_scope's tail (active_scope_id retirement) unexecuted so
    // every later prepare_turn of the session bails. The forkguard_ prefix
    // registers it as a fork-guard layer-3 behavior test (fork-policy §3).
    #[tokio::test]
    async fn forkguard_bounded_join_reports_panicked_task_as_not_settled() {
        let panicked = bounded_join_while_holding_turn_gate(
            "test finalize",
            TURN_GATE_AWAIT_TIMEOUT,
            tokio::spawn(async { panic!("finalize exploded") }),
        )
        .await;
        assert!(matches!(panicked, BoundedJoinOutcome::Panicked(_)));

        let settled = bounded_join_while_holding_turn_gate(
            "test finalize",
            TURN_GATE_AWAIT_TIMEOUT,
            tokio::spawn(async {}),
        )
        .await;
        assert!(matches!(settled, BoundedJoinOutcome::Settled));
    }

    // issue #255: the reclaim shutdown sends must give up the turn gate
    // within TURN_GATE_AWAIT_TIMEOUT when the engine ops channel is full
    // and never drained, and the undelivered ops must still be re-delivered
    // in order by the detached retry once capacity frees up — otherwise a
    // timed-out reclaim would leak the engine run loop until process exit
    // (the engine only exits through its normal Shutdown path and its own
    // tx_op clone keeps the channel open). The forkguard_ prefix registers
    // it as a fork-guard layer-3 behavior test (fork-policy §3).
    #[tokio::test]
    async fn forkguard_reclaim_shutdown_sends_bounded_and_retried() {
        // Capacity 1, filled and never drained: the first gate-held send
        // parks exactly like on a wedged engine.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Op>(1);
        tx.send(Op::Shutdown).await.expect("wedged slot filled");
        let started = std::time::Instant::now();
        let delivered = tokio::time::timeout(
            2 * TURN_GATE_AWAIT_TIMEOUT,
            bounded_shutdown_sends(
                |op| {
                    let tx = tx.clone();
                    async move { tx.send(op).await.map_err(anyhow::Error::from) }
                },
                super::EnginePool::shutdown_cancel_cascade_ops(),
            ),
        )
        .await
        .expect(
            "bounded sends must settle within twice the gate bound; an unbounded regression would hang here",
        );
        assert_eq!(delivered, 0);
        assert!(started.elapsed() >= TURN_GATE_AWAIT_TIMEOUT);
        // Drain the filler so capacity frees: the detached retry must now
        // deliver CancelSubAgents then Shutdown, in that order.
        assert!(matches!(rx.recv().await, Some(Op::Shutdown)));
        let retry_tx = tx.clone();
        let retry = tokio::spawn(retry_shutdown_sends(
            move |op| {
                let tx = retry_tx.clone();
                async move { tx.send(op).await.map_err(anyhow::Error::from) }
            },
            super::EnginePool::shutdown_cancel_cascade_ops()
                .into_iter()
                .collect(),
            TURN_GATE_AWAIT_TIMEOUT,
        ));
        assert!(matches!(
            tokio::time::timeout(2 * TURN_GATE_AWAIT_TIMEOUT, rx.recv())
                .await
                .expect("retry CancelSubAgents must arrive within twice the gate bound"),
            Some(Op::CancelSubAgents)
        ));
        assert!(matches!(
            tokio::time::timeout(2 * TURN_GATE_AWAIT_TIMEOUT, rx.recv())
                .await
                .expect("retry Shutdown must arrive within twice the gate bound"),
            Some(Op::Shutdown)
        ));
        tokio::time::timeout(2 * TURN_GATE_AWAIT_TIMEOUT, retry)
            .await
            .expect("retry must join within twice the gate bound; a detached-retry regression would hang here")
            .expect("retry task joins");
    }

    // The detached retry must itself be bounded: on a permanently wedged
    // engine it stops after its patience (the engine task would linger
    // either way while stalled) instead of parking a task forever.
    #[tokio::test]
    async fn forkguard_reclaim_shutdown_retry_gives_up_within_patience() {
        // The receiver stays alive but is never drained, so the channel
        // stays full: the retry must give up on its patience, not on a
        // channel error.
        let (tx, _rx) = tokio::sync::mpsc::channel::<Op>(1);
        tx.send(Op::Shutdown).await.expect("wedged slot filled");
        let patience = std::time::Duration::from_millis(100);
        let started = std::time::Instant::now();
        let retry_tx = tx.clone();
        let retry = tokio::spawn(retry_shutdown_sends(
            move |op| {
                let tx = retry_tx.clone();
                async move { tx.send(op).await.map_err(anyhow::Error::from) }
            },
            super::EnginePool::shutdown_cancel_cascade_ops()
                .into_iter()
                .collect(),
            patience,
        ));
        tokio::time::timeout(2 * TURN_GATE_AWAIT_TIMEOUT, retry)
            .await
            .expect("retry must join within twice the gate bound; an unbounded-patience regression would hang here")
            .expect("retry task joins");
        assert!(started.elapsed() >= patience);
        assert!(started.elapsed() < TURN_GATE_AWAIT_TIMEOUT);
    }

    // The Detached arm must be distinguishable too: a task that outlives
    // the (parameterized) budget is detached — it keeps running while the
    // caller stops waiting — and must not be counted as settled or
    // panicked. A synthetic 50ms budget keeps this off the real 5s clock.
    // The forkguard_ prefix registers it as a fork-guard layer-3 behavior
    // test (fork-policy §3).
    #[tokio::test]
    async fn forkguard_bounded_join_detaches_task_that_outlives_budget() {
        let detached = bounded_join_while_holding_turn_gate(
            "test finalize",
            std::time::Duration::from_millis(50),
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }),
        )
        .await;
        assert!(matches!(detached, BoundedJoinOutcome::Detached));
    }

    #[tokio::test]
    async fn fresh_cancel_after_turn_change_still_cancels_new_turn() {
        // Control test preventing the generation guard from overshooting:
        // the user pressed stop **after** the new turn started, so the
        // snapshot holds the new turn's epoch — match → the new turn is
        // legitimately canceled (cancel_engine fires).
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-fresh-cancel";

        let lifecycle = lifecycles.for_session(sid);
        // turn1 reserve → terminal (unsubmitted claim path) → turn2 reserve
        // (epoch=2).
        let _reservation1 = lifecycle.reserve().expect("turn1 reserve");
        assert!(lifecycle.finish_unsubmitted_once());
        let _reservation2 = lifecycle.reserve().expect("turn2 reserve");

        let cancel_called = Arc::new(AtomicBool::new(false));
        let probe = cancel_called.clone();
        // cascade probe: the normal cancel path (fresh cancel) must execute
        // the cascade cancel.
        let cascade_called = Arc::new(AtomicBool::new(false));
        let probe_cascade = cascade_called.clone();
        // phase 1's cascade cancel is treated as successfully enqueued (this
        // test does not cover the reviewer point 9 scenario).
        cancel_turn_with_gates(
            &locks,
            &lifecycles,
            &shell_tasks,
            sid,
            deepseek_tui::core::engine::CancelMode::StopDropInbox,
            // get_engine: engine present.
            || async { Some(()) },
            // cancel_current: record that it fired.
            move |_engine: &(), _identity: Option<TurnIdentity>| {
                probe.store(true, Ordering::Release);
            },
            // cascade_cancel: a fresh cancel must be called after phase two's
            // generation match (the enqueue completes inside the turn gate
            // await, reviewer point 4).
            move |_engine: &()| {
                probe_cascade.store(true, Ordering::Release);
                async {}
            },
            |_lc, _target| false,
        )
        .await;

        // target=Some(2)=current → match → get_engine returns Some →
        // cancel_current fires.
        assert!(
            cancel_called.load(Ordering::Acquire),
            "a fresh cancel on the current turn must still cancel its engine"
        );
        assert!(
            cascade_called.load(Ordering::Acquire),
            "a fresh cancel on the current turn must also cascade-cancel its subagents inside the turn gate"
        );
    }

    #[tokio::test]
    async fn stale_cancel_skips_cascade_retry_when_new_turn_already_submitted() {
        // The resend boundary of reviewer point 9: phase 1 try_send failed +
        // the new turn is **already submitted** (SendMessage has entered the
        // engine and the new turn may already have started sub-agents) → the
        // mismatch branch must not resend the cascade cancel — otherwise
        // CancelSubAgents would kill the new turn's just-started sub-agents
        // by mistake.
        // The resend is safe only inside the window of "the new turn has not
        // been submitted yet" (the engine still has only the old turn's
        // leftover sub-agents).
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-try-send-failure-submitted";

        let lifecycle = lifecycles.for_session(sid);
        // turn1: on_submitted activates it (epoch=1).
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));

        let gate = locks.for_session(sid).await;
        let blocker = gate.lock().await;

        let cascade_called = Arc::new(AtomicBool::new(false));
        let probe_cascade = cascade_called.clone();
        let cancel_called = Arc::new(AtomicBool::new(false));
        let probe_cancel = cancel_called.clone();

        let cancel_task = tokio::spawn(async move {
            cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                deepseek_tui::core::engine::CancelMode::StopDropInbox,
                || async { Some(()) },
                move |_engine: &(), _identity: Option<TurnIdentity>| {
                    probe_cancel.store(true, Ordering::Release);
                },
                move |_engine: &()| {
                    probe_cascade.store(true, Ordering::Release);
                    async {}
                },
                |_lc, _target| false,
            )
            .await
        });
        // Let the cancel progress to: phase one complete, phase two
        // suspended at gate.lock().await.
        tokio::task::yield_now().await;

        // turn1 terminal → turn2 on_submitted activates (epoch=2,
        // submitted=true — SendMessage has entered the engine and new-turn
        // sub-agents may already have started).
        assert!(lifecycle.finish_once(|| {}).is_some());
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));

        // Release turn_lock: phase two current=Some(2) ≠ target=Some(1) →
        // mismatch; the new turn is already submitted → the cascade cancel
        // must not be resent.
        drop(blocker);
        drop(gate);
        cancel_task.await.expect("cancel task joins");

        assert!(
            !cascade_called.load(Ordering::Acquire),
            "cascade retry must be skipped when the new turn has already been submitted"
        );
        assert!(
            cancel_called.load(Ordering::Acquire),
            "phase one must still cancel the originating turn's engine"
        );
    }

    #[tokio::test]
    async fn stale_cancel_retries_cascade_when_mismatch_surfaces_after_phase_two_engine_lookup() {
        // Deterministic regression of G1: when the mismatch first surfaces
        // in the recheck **after phase two's get_engine await** (rather than
        // at the entry recheck), the cascade resend must still run.
        //
        // The old implementation only resent in phase two's entry mismatch
        // branch; if the entry recheck still matched (T1 still active,
        // current_turn_generation reporting Some(T1)), claim was a no-op (T1
        // already submitted), and T2 reserved during the subsequent
        // get_engine().await, the later recheck mismatch would skip directly —
        // with phase-1's best-effort try_send failed, the old turn's
        // detached sub-agents were silently dropped.
        //
        // Fix: at the recheck-mismatch point after the get_engine await,
        // resend the cascade with the same should_retry_cascade predicate as
        // the entry mismatch branch.
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-g1-mismatch-after-engine-lookup";

        let lifecycle = lifecycles.for_session(sid);
        // turn1: on_submitted activates it (active+submitted+epoch=1).
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));
        let target = lifecycle.current_turn_generation().expect("turn1 epoch");
        assert_eq!(target, 1_u64);

        // The suspension point of phase two's get_engine await: phase one's
        // first call returns directly; phase two's second call notifies
        // entered and then suspends, the main thread switches the turn inside
        // the await window, then releases it.
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let entered_main = entered.clone();
        let release_main = release.clone();
        let get_engine_calls = Arc::new(AtomicU64::new(0));
        let probe_engine = get_engine_calls.clone();

        let cascade_called = Arc::new(AtomicBool::new(false));
        let probe_cascade = cascade_called.clone();
        let cancel_calls = Arc::new(AtomicU64::new(0));
        let probe_cancel = cancel_calls.clone();

        let cancel_task = tokio::spawn(async move {
            cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                deepseek_tui::core::engine::CancelMode::StopDropInbox,
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    let probe = probe_engine.clone();
                    async move {
                        if probe.fetch_add(1, Ordering::SeqCst) == 1 {
                            // The get_engine before phase two's recheck:
                            // simulate handle_for's entries-lock await,
                            // suspended until the main thread switches the
                            // turn.
                            entered.notify_one();
                            release.notified().await;
                        }
                        Some(())
                    }
                },
                // cancel_current: fires once in phase one (canceling T1); phase two
                // should skip due to the later mismatch.
                move |_engine: &(), _identity: Option<TurnIdentity>| {
                    probe_cancel.fetch_add(1, Ordering::SeqCst);
                },
                // cascade_cancel: later-recheck mismatch + new turn not
                // submitted → must resend.
                move |_engine: &()| {
                    probe_cascade.store(true, Ordering::Release);
                    async {}
                },
                // claim_unsubmitted: T1 already submitted → no-op (no
                // claim).
                |_lc, _target| false,
            )
            .await
        });
        // Let the cancel finish phase one (the first get_engine returns
        // directly + arm+cancel), have phase two acquire turn_lock and finish
        // the entry recheck (current=Some(1)==target, match), then enter the
        // second get_engine suspension.
        entered_main.notified().await;

        // At this point phase two's entry recheck has passed and it is
        // suspended inside the get_engine await: switch the turn.
        // T1 terminal → T2 reserve (epoch=2, not submitted — SendMessage is
        // blocked by turn_lock).
        assert!(lifecycle.finish_once(|| {}).is_some());
        let reservation2 = lifecycle.reserve().expect("turn2 reserve");

        // Release get_engine: the later recheck current=Some(2)≠Some(1) →
        // mismatch → should_retry_cascade (new turn not submitted) → resend
        // the cascade.
        release_main.notify_one();
        cancel_task.await.expect("cancel task joins");

        assert!(
            get_engine_calls.load(Ordering::SeqCst) >= 2,
            "get_engine must be consulted in phase one and phase two"
        );
        assert!(
            cascade_called.load(Ordering::Acquire),
            "G1: cascade retry must fire when the mismatch is first observed after the phase-two engine lookup"
        );
        assert!(
            cancel_calls.load(Ordering::SeqCst) == 1,
            "phase one cancels turn1; phase two must skip cancel_current on mismatch"
        );
        assert!(
            reservation2.ensure_active().is_ok(),
            "new turn reservation must remain valid after the G1 cascade retry"
        );
    }

    #[tokio::test]
    async fn stale_cancel_skips_unsubmitted_claim_and_leaves_new_turn_intact() {
        // invalidate path: when cancel_engine returns false (simulating an
        // absent engine), a generation mismatch must block claim_unsubmitted
        // (claiming the unsubmitted terminal would invalidate the
        // reservation) — otherwise the new turn's unsubmitted reservation
        // would be wrongly cleared.
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-stale-invalidate";

        let lifecycle = lifecycles.for_session(sid);
        let _reservation1 = lifecycle.reserve().expect("turn1 reserve");
        let gate = locks.for_session(sid).await;
        let blocker = gate.lock().await;

        let claimed = Arc::new(AtomicBool::new(false));
        let probe = claimed.clone();
        // phase 1 has no engine, so the cascade cancel was not delivered; but
        // with the new turn reserved-not-sent the mismatch branch resends
        // (reviewer point 9) — with the engine absent the resend is a no-op.
        let cancel_task = tokio::spawn(async move {
            cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                deepseek_tui::core::engine::CancelMode::StopDropInbox,
                // engine absent → get_engine returns None → no cancel, take
                // the claim_unsubmitted branch.
                || async { None::<()> },
                // cancel_current: must not be called when the engine is absent.
                |_engine: &(), _identity: Option<TurnIdentity>| {},
                // cascade_cancel: the engine is absent, so phase two does not call it.
                |_engine: &()| async {},
                move |lc, _target| {
                    probe.store(true, Ordering::Release);
                    // Reuse the real unsubmitted claim path to observe side
                    // effects.
                    lc.claim_unsubmitted_terminal()
                },
            )
            .await
        });
        tokio::task::yield_now().await;

        // turn1 terminal (unsubmitted claim path) → turn2 unsubmitted
        // reservation (epoch=2).
        assert!(lifecycle.finish_unsubmitted_once());
        let reservation2 = lifecycle.reserve().expect("turn2 reserve");

        drop(blocker);
        drop(gate);
        cancel_task.await.expect("cancel task joins");

        // Assert: the claim_unsubmitted closure was not called, and the turn2
        // reservation is still valid.
        assert!(
            !claimed.load(Ordering::Acquire),
            "stale cancel must not claim an unsubmitted terminal on a new turn"
        );
        assert!(
            reservation2.ensure_active().is_ok(),
            "new turn unsubmitted reservation must survive a stale cancel"
        );
    }

    #[tokio::test]
    async fn stale_cancel_claim_with_epoch_guard_rejects_new_turn() {
        // Integration verification of reviewer point 7: when the turn
        // switches after phase two's generation check passes but before the
        // claim (reserve_turn does not take turn_lock), claim_unsubmitted
        // must atomically validate against the initiation-time snapshot
        // target inside the state lock — an epoch mismatch is a no-op and the
        // new turn's reservation must not be claimed as Interrupted. Here the
        // claim closure simulates "switch after the check" (the real scenario
        // is done by another worker), verifying for_epoch rejects a stale
        // target and the new turn's reservation stays valid.
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-claim-epoch-guard";

        let lifecycle = lifecycles.for_session(sid);
        // turn1: reserved, not submitted (epoch=1), engine absent → cancel
        // takes the claim branch.
        let _reservation1 = lifecycle.reserve().expect("turn1 reserve");

        let new_turn_intact = Arc::new(AtomicBool::new(false));
        let probe = new_turn_intact.clone();
        // phase 1 has no engine, so the cascade cancel was not delivered;
        // with the engine absent the mismatch branch's resend is a no-op and
        // does not affect this test's claim semantics.
        cancel_turn_with_gates(
            &locks,
            &lifecycles,
            &shell_tasks,
            sid,
            deepseek_tui::core::engine::CancelMode::StopDropInbox,
            // engine absent → get_engine returns None → no cancel, take
            // claim_unsubmitted.
            || async { None::<()> },
            // cancel_current: the engine is absent; must not be called.
            |_engine: &(), _identity: Option<TurnIdentity>| {},
            // cascade_cancel: the engine is absent, so phase two does not call it.
            |_engine: &()| async {},
            // claim closure: simulate switching the turn "after the
            // generation check passes, before the claim", then claim with the
            // initiation-time snapshot target — must be rejected (epoch
            // mismatch) and the new turn stays intact.
            move |lc, target| {
                assert!(lc.finish_unsubmitted_once(), "turn1 terminal");
                let new_reservation = lc.reserve().expect("turn2 reserve");
                let claimed = lc.claim_unsubmitted_terminal_for_epoch(target);
                probe.store(
                    !claimed && new_reservation.ensure_active().is_ok(),
                    Ordering::Release,
                );
                // At closure end new_reservation drops: not submitted →
                // on_reservation_failed rolls back turn2's active state. By
                // then the claim has been rejected and phase two touches the
                // lifecycle no further (engine absent), so the rollback is
                // idempotent and harmless.
                claimed
            },
        )
        .await;

        assert!(
            new_turn_intact.load(Ordering::Acquire),
            "stale claim must reject the new turn and leave its reservation intact"
        );
    }

    #[tokio::test]
    async fn stale_cancel_during_phase_one_engine_lookup_leaves_new_turn_intact() {
        // Deterministic regression of reviewer point 1 (phase-one TOCTOU):
        // the generation check must not happen only **before**
        // `get_engine().await` — `get_engine` awaits internally
        // (`handle_for` takes the entries lock), and during the await the old
        // turn may end and a new turn may reserve and
        // `reset_cancel_token()`; a subsequent `cancel_current()` would hit
        // the new turn's live token, and phase two finding the epoch mismatch
        // cannot withdraw the cancel that already happened.
        //
        // Here the turn switch is arranged **during** phase one's `get_engine`
        // await (the original test
        // `stale_cancel_after_turn_change_leaves_new_turn_intact` only covered
        // a switch after phase one completed, missing this window): the
        // get_engine probe suspends on a oneshot to simulate waiting for the
        // entries lock, the main thread advances turn1's terminal + turn2's
        // reserve meanwhile, then releases get_engine → phase one re-validates
        // the epoch after the await, finds the mismatch → no cancel; phase two
        // early-returns the same way, and turn2 stays intact.
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-phase1-toctou";

        let lifecycle = lifecycles.for_session(sid);
        // turn1: on_submitted activates it (active+submitted+epoch=1).
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));

        // Coordinate with Notify: the probe first notifies "entered the
        // get_engine await", then suspends waiting for release; the main
        // thread advances the turn after receiving entered, then releases the
        // probe.
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        // Main-thread-side copies (the spawned async move moves the
        // originals into the task).
        let entered_main = entered.clone();
        let release_main = release.clone();
        let cancel_called = Arc::new(AtomicBool::new(false));
        let probe = cancel_called.clone();
        let get_engine_calls = Arc::new(AtomicU64::new(0));
        let probe_calls = get_engine_calls.clone();
        // phase 1's cascade cancel is treated as successfully enqueued (this
        // test focuses on the phase-one TOCTOU guard and does not cover
        // reviewer point 9's try_send-failure resend path; it keeps the
        // existing assertion of no cascade on a mismatch early-return).

        let cancel_task = tokio::spawn(async move {
            cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                deepseek_tui::core::engine::CancelMode::StopDropInbox,
                // get_engine: the first call (phase one) notifies entered and
                // then suspends (simulating handle_for's entries-lock await),
                // returning "engine present" once released.
                // If the implementation regresses (phase two missing the
                // generation guard — the original #205 bug), phase two calls
                // get_engine again — returning "engine present" directly here
                // lets the cancel_current probe fire and the test go red; if
                // we parked on Notify here too, it would double-suspend into
                // a deadlock (showing up in CI as a timeout instead of an
                // assertion failure).
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    let calls = probe_calls.clone();
                    async move {
                        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                            // notify_one keeps a permit for future waiters:
                            // even if the spawned task reaches this before
                            // the main thread registers notified().await, the
                            // signal is not lost (reviewer point 5:
                            // notify_waiters keeps no notification for
                            // later-registered waiters — they would wait
                            // forever).
                            entered.notify_one();
                            release.notified().await;
                        }
                        Some(())
                    }
                },
                // cancel_current: must not be called (epoch mismatch after phase one's
                // await).
                move |_engine: &(), _identity: Option<TurnIdentity>| {
                    probe.store(true, Ordering::Release);
                },
                // cascade_cancel: generation mismatch — must not be called.
                |_engine: &()| async {},
                |_lc, _target| false,
            )
            .await
        });
        // Wait until C2's phase one enters the get_engine await (the
        // suspension point before taking the entries lock).
        entered_main.notified().await;

        // Switch the turn inside the await window: turn1 terminal (submitted
        // → claim path) → turn2 reserve.
        assert!(lifecycle.finish_once(|| {}).is_some());
        let reservation2 = lifecycle.reserve().expect("turn2 reserve");

        // Release get_engine: phase one re-validates the epoch after the
        // await → target=Some(1) ≠ current=Some(2)
        // → no cancel; phase two early-returns the same way.
        // notify_one, same as on the entered side: keeps the permit for
        // registered/future waiters.
        release_main.notify_one();
        cancel_task.await.expect("cancel task joins");

        assert!(
            !cancel_called.load(Ordering::Acquire),
            "phase one must not cancel the new turn when the turn switched during the engine lookup await"
        );
        assert!(
            reservation2.ensure_active().is_ok(),
            "new turn reservation must remain valid after a phase-one TOCTOU"
        );
    }

    #[tokio::test]
    async fn terminal_closing_cancel_reaches_the_closure_for_disposition_only() {
        // Guard face of the issue #254 terminal-closing residual window: when
        // the lifecycle is inside terminal_closing (claimed, not yet
        // finished), the arm's idle guard (`!active && !terminal_closing`)
        // must NOT skip the closure — the production closure publishes the
        // stop disposition in that window (foundation disposition-only: drop
        // parked steers, latch the cancel reason). If the guard skipped it
        // here, a ⏹ the user already pressed would lose its steer-loss
        // contract. The token fire itself is blocked by the closure's
        // DispositionOnly verdict (pure-function exhaustion in engine.rs);
        // this test pins "the closure must be reached".
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-closing-cancel";

        let lifecycle = lifecycles.for_session(sid);
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));
        // Terminal claim: active=false, submitted=false,
        // terminal_closing=true, turn_id taken into the EmittedTerminal.
        assert!(lifecycle.claim_terminal().is_some());

        let closure_calls = Arc::new(AtomicU64::new(0));
        let probe = closure_calls.clone();
        let (target, claimed_unsubmitted) = cancel_turn_with_gates(
            &locks,
            &lifecycles,
            &shell_tasks,
            sid,
            deepseek_tui::core::engine::CancelMode::StopDropInbox,
            || async { Some(()) },
            move |_engine: &(), _identity: Option<TurnIdentity>| {
                probe.fetch_add(1, Ordering::AcqRel);
            },
            |_engine: &()| async {},
            |_lc, _target| false,
        )
        .await;

        // The closing window keeps the target epoch visible to the gates
        // (same epoch accounting as current_turn_generation); the closure
        // must run at least in phase one (phase two may run it again when no
        // turn switch interleaves).
        assert_eq!(
            target,
            Some(1),
            "terminal closing must keep the target epoch visible to the gates"
        );
        assert!(
            !claimed_unsubmitted,
            "a submitted turn never takes the unsubmitted-claim path"
        );
        assert!(
            closure_calls.load(Ordering::Acquire) >= 1,
            "the cancel closure must run during terminal closing so the stop disposition is published"
        );
    }

    /// Fake of the foundation r13+ turn-bound cancel contract: a shared slot
    /// `{ turn_id, token }` swapped atomically at every turn start; a
    /// turn-bound cancel fires only the named turn's token and publishes the
    /// disposition only on match; the disposition-only entry never fires.
    struct FakeTurnSlotEngine {
        slot_turn_id: StdMutex<Option<String>>,
        fired: StdMutex<Vec<String>>,
        dispositions: AtomicU64,
    }

    impl FakeTurnSlotEngine {
        fn installed_on(turn_id: &str) -> Arc<Self> {
            Arc::new(Self {
                slot_turn_id: StdMutex::new(Some(turn_id.to_string())),
                fired: StdMutex::new(Vec::new()),
                dispositions: AtomicU64::new(0),
            })
        }

        fn fired_turns(&self) -> Vec<String> {
            self.fired.lock().expect("fired").clone()
        }

        fn disposition_count(&self) -> u64 {
            self.dispositions.load(Ordering::Acquire)
        }
    }

    impl TurnBoundCancelOps for FakeTurnSlotEngine {
        fn cancel_bound_turn(
            &self,
            turn_id: &str,
            _mode: deepseek_tui::core::engine::CancelMode,
        ) -> bool {
            // Identity check and token resolution happen under the same slot
            // lock (mirrors EngineHandle::cancel_turn).
            let slot = self.slot_turn_id.lock().expect("slot");
            if slot.as_deref() != Some(turn_id) {
                return false;
            }
            drop(slot);
            self.fired.lock().expect("fired").push(turn_id.to_string());
            self.dispositions.fetch_add(1, Ordering::AcqRel);
            true
        }

        fn publish_stop_disposition_only(&self, _mode: deepseek_tui::core::engine::CancelMode) {
            self.dispositions.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Drive `cancel_turn_with_gates` with the production dispatch closure
    /// (`dispatch_turn_bound_cancel`) against the fake slot engine, the same
    /// wiring `EnginePool::cancel` uses.
    async fn run_production_cancel_wiring(
        locks: &SessionTurnLocks,
        lifecycles: &SessionTurnLifecycles,
        shell_tasks: &SessionTurnShellTasks,
        sid: &str,
        engine: &Arc<FakeTurnSlotEngine>,
    ) -> (Option<u64>, bool) {
        cancel_turn_with_gates(
            locks,
            lifecycles,
            shell_tasks,
            sid,
            deepseek_tui::core::engine::CancelMode::StopDropInbox,
            || async { Some(engine.clone()) },
            |engine, identity| {
                dispatch_turn_bound_cancel(
                    engine.as_ref(),
                    identity.as_ref(),
                    deepseek_tui::core::engine::CancelMode::StopDropInbox,
                );
            },
            |_engine: &Arc<FakeTurnSlotEngine>| async {},
            |_lc, _target| false,
        )
        .await
    }

    #[tokio::test]
    async fn delayed_forwarder_stop_spares_the_self_started_followup_token() {
        // The review-round regression for issue #254's submitted-but-
        // unobserved window: the lifecycle still shows turn N as reserved and
        // submitted (a delayed forwarder has not processed its
        // `TurnStarted`), while the engine has already completed N and
        // self-started the autonomous follow-up whose live token now occupies
        // the slot. Issuing the stop here used to fall back to the unbound
        // cancel and killed that follow-up. It must not: the closure takes
        // the disposition-only verdict, the pending replay is armed, and the
        // replay itself is dropped by the foundation identity check.
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-delayed-forwarder";

        let lifecycle = lifecycles.for_session(sid);
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));
        let epoch = lifecycle.current_turn_generation().expect("reserved epoch");
        // The engine self-started N+1 before the forwarder observed anything.
        let engine = FakeTurnSlotEngine::installed_on("turn-n-plus-1");

        let (target, claimed_unsubmitted) =
            run_production_cancel_wiring(&locks, &lifecycles, &shell_tasks, sid, &engine).await;

        assert_eq!(target, Some(epoch), "the reserved turn stays the target");
        assert!(
            !claimed_unsubmitted,
            "a submitted turn never takes the unsubmitted-claim path"
        );
        assert!(
            engine.fired_turns().is_empty(),
            "the follow-up turn's live token must stay uncancelled"
        );
        assert!(
            engine.disposition_count() >= 1,
            "the stop disposition (steer loss, cancel reason) must still be published"
        );
        // The forwarder's turn-bound replay is armed under the same lock the
        // verdict was made in.
        assert!(
            lifecycle
                .take_pending_cancel(Some(TEST_SUBMISSION))
                .is_some(),
            "the pending replay must be armed so the genuinely pending target is still deliverable"
        );
        // Simulate the replay once the delayed `TurnStarted(N)` is finally
        // processed: the slot no longer names N, so the foundation drops it
        // wholesale — no fire, no disposition.
        let dispositions_before_replay = engine.disposition_count();
        assert!(
            !engine.cancel_bound_turn(
                "turn-n",
                deepseek_tui::core::engine::CancelMode::StopDropInbox
            ),
            "the replay for the already-finished turn must be dropped by the identity check"
        );
        assert!(
            engine.fired_turns().is_empty(),
            "the dropped replay must not fire any token"
        );
        assert_eq!(
            engine.disposition_count(),
            dispositions_before_replay,
            "a dropped replay publishes no disposition"
        );
    }

    #[tokio::test]
    async fn pending_stop_replay_cancels_the_genuinely_pending_turn() {
        // The other half of the submit→TurnStarted window (review round:
        // "keep coverage that a genuinely pending target is eventually
        // cancelled"): the engine has just admitted the pending turn N (its
        // token is installed, `TurnStarted` still queued). The stop must not
        // fire blind — but the armed pending replay delivers the cancel the
        // moment the forwarder processes that `TurnStarted`.
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-genuine-pending";

        let lifecycle = lifecycles.for_session(sid);
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));
        let epoch = lifecycle.current_turn_generation().expect("reserved epoch");
        // The engine admitted N; its `TurnStarted` is still queued.
        let engine = FakeTurnSlotEngine::installed_on("turn-n");

        let (target, _) =
            run_production_cancel_wiring(&locks, &lifecycles, &shell_tasks, sid, &engine).await;

        assert_eq!(target, Some(epoch));
        assert!(
            engine.fired_turns().is_empty(),
            "no unscoped fire may happen while the identity is unobserved"
        );
        assert!(
            engine.disposition_count() >= 1,
            "the stop disposition must still be published"
        );
        // The forwarder processes the queued `TurnStarted(N)`: it takes the
        // armed cancel and replays it bound to that turn id — the slot names
        // N, so it fires.
        assert!(
            lifecycle
                .take_pending_cancel(Some(TEST_SUBMISSION))
                .is_some(),
            "the armed pending cancel must be consumable by the forwarder"
        );
        assert!(
            engine.cancel_bound_turn(
                "turn-n",
                deepseek_tui::core::engine::CancelMode::StopDropInbox
            ),
            "the turn-bound replay must hit the genuinely pending turn"
        );
        assert_eq!(
            engine.fired_turns(),
            vec!["turn-n".to_string()],
            "the replay must fire exactly the pending turn's token"
        );
    }

    #[tokio::test]
    async fn overtaking_self_started_turn_started_cannot_consume_the_replay() {
        // The remaining #254 window (P1 review round, submission
        // correlation): the stop was armed inside the submit→TurnStarted
        // window, and a runtime self-started follow-up's `TurnStarted`
        // overtook the submitted turn's in the forwarder stream. Epoch alone
        // cannot tell the two events apart, so the first arrival used to
        // consume the pending replay and — the slot naming the overtaking
        // turn — cancelled N+1 while the stop intended for N was lost. The
        // foundation now echoes a host-supplied submission id on every
        // host-submitted turn's `TurnStarted` and never tags a self-started
        // one: the forwarder's consumption gate refuses the overtaking
        // arrival, the replay stays armed, and the submitted turn's own
        // echo delivers the cancel to exactly that turn.
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-overtaking-start";

        let lifecycle = lifecycles.for_session(sid);
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));
        let epoch = lifecycle.current_turn_generation().expect("reserved epoch");
        // The engine admitted the submitted turn N; a self-started N+1's
        // `TurnStarted` overtakes N's in the event stream.
        let engine = FakeTurnSlotEngine::installed_on("turn-n");

        let (target, _) =
            run_production_cancel_wiring(&locks, &lifecycles, &shell_tasks, sid, &engine).await;
        assert_eq!(target, Some(epoch));
        assert!(
            engine.fired_turns().is_empty(),
            "arming must not fire any token"
        );

        // The overtaking self-started `TurnStarted` carries no submission
        // id: the forwarder gate must refuse it — the replay is neither
        // consumed nor redirected onto the overtaking turn.
        assert!(
            lifecycle.take_pending_cancel(None).is_none(),
            "an overtaking self-started TurnStarted must not consume the replay"
        );
        assert!(
            engine.fired_turns().is_empty(),
            "no cancel may reach the overtaking turn through the replay"
        );
        // A foreign submitted id must not consume it either.
        assert!(
            lifecycle
                .take_pending_cancel(Some("sub-other-turn"))
                .is_none(),
            "a foreign submission echo must not consume the replay"
        );
        // The submitted turn's own `TurnStarted` arrives: the gate accepts
        // the matching echo and the forwarder replays bound to that turn —
        // the slot names N, so it fires exactly there.
        assert!(
            lifecycle
                .take_pending_cancel(Some(TEST_SUBMISSION))
                .is_some(),
            "the replay must stay armed for the submitted turn's own echo"
        );
        assert!(
            engine.cancel_bound_turn(
                "turn-n",
                deepseek_tui::core::engine::CancelMode::StopDropInbox
            ),
            "the replay must land on the submitted turn, not the overtake"
        );
        assert_eq!(
            engine.fired_turns(),
            vec!["turn-n".to_string()],
            "the user's stop for N must be delivered to N"
        );
    }

    #[tokio::test]
    async fn pending_stop_replay_survives_an_autonomous_lifecycle_before_the_target_starts() {
        // The full production sequence behind the #254 replay gate (review
        // round: "keep the pending submission's identity valid across
        // unrelated autonomous lifecycle events"): the stop is armed inside
        // the submit→TurnStarted window of the submitted turn N, and an
        // untagged autonomous turn not only starts before N (its
        // `TurnStarted` must not consume the replay) but also runs to
        // completion. Its terminal reopens the gate, so N's own
        // `TurnStarted` arrives at an idle lifecycle and advances the epoch
        // as newly-active. The consumption gate is the submission token, not
        // the epoch: N's echo must still deliver the user's stop even though
        // the arming-time epoch no longer equals the current one. Driving
        // the lifecycle methods in forwarder order (started → terminal →
        // target start) is what moves the epoch; handing `take_pending_cancel`
        // a hand-held arming epoch can never see it.
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-overtaking-full-lifecycle";

        let lifecycle = lifecycles.for_session(sid);
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));
        let armed_epoch = lifecycle.current_turn_generation().expect("armed epoch");
        // The engine admitted N; a self-started N+1's `TurnStarted` overtakes
        // N's in the event stream.
        let engine = FakeTurnSlotEngine::installed_on("turn-n");

        let (target, _) =
            run_production_cancel_wiring(&locks, &lifecycles, &shell_tasks, sid, &engine).await;
        assert_eq!(target, Some(armed_epoch));

        // Forwarder order 1: the overtaking self-started `TurnStarted`
        // (untagged) merges into the still-active submitted lifecycle and
        // must leave the replay armed.
        lifecycle.on_started("turn-n-plus-1".to_string());
        assert_eq!(
            lifecycle.current_turn_generation(),
            Some(armed_epoch),
            "merging the overtake into the active lifecycle must not advance the epoch"
        );
        assert!(
            lifecycle.take_pending_cancel(None).is_none(),
            "the untagged overtake must not consume the replay"
        );

        // Forwarder order 2: the autonomous turn runs to completion; its
        // terminal closes the merged lifecycle and reopens the reserve gate.
        assert!(
            lifecycle.finish_once(|| {}).is_some(),
            "the autonomous turn's terminal must close the lifecycle"
        );

        // Forwarder order 3: N's own `TurnStarted` arrives with the matching
        // echo. The lifecycle is idle again, so the start is newly-active and
        // bumps the epoch past the arming value — the token still delivers.
        lifecycle.on_started("turn-n".to_string());
        let replay_epoch = lifecycle.current_turn_generation().unwrap_or(0);
        assert_ne!(
            replay_epoch, armed_epoch,
            "the target's own start must be newly-active after the autonomous terminal"
        );
        let (replay_armed_epoch, mode) = lifecycle
            .take_pending_cancel(Some(TEST_SUBMISSION))
            .expect("the matching echo must deliver the armed stop across the epoch bump");
        assert_eq!(replay_armed_epoch, armed_epoch);
        assert_eq!(
            mode,
            deepseek_tui::core::engine::CancelMode::StopDropInbox,
            "the replay must carry the arming-time disposition mode"
        );
        assert!(
            engine.cancel_bound_turn(
                "turn-n",
                deepseek_tui::core::engine::CancelMode::StopDropInbox
            ),
            "the replay must land on the submitted turn N"
        );
        assert_eq!(
            engine.fired_turns(),
            vec!["turn-n".to_string()],
            "the user's stop must reach N, not the completed autonomous turn"
        );
    }

    #[tokio::test]
    async fn bound_stop_hits_the_observed_turn_and_skips_a_moved_on_slot() {
        // Bound-verdict wiring: with the turn id observed, the dispatch fires
        // exactly that turn through the foundation entry; when the slot has
        // already moved on, the identity check skips wholesale — no fire and
        // no disposition, because the newer turn belongs to a generation this
        // stop never aimed at.
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-bound-hit";

        let lifecycle = lifecycles.for_session(sid);
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));
        lifecycle.on_started("turn-n".to_string());

        // Hit: the slot still names the observed turn.
        let engine = FakeTurnSlotEngine::installed_on("turn-n");
        let (target, _) =
            run_production_cancel_wiring(&locks, &lifecycles, &shell_tasks, sid, &engine).await;
        assert_eq!(target, Some(1));
        // The gates run the cancel closure in both phases — idempotent on the
        // foundation (an already-cancelled token stays cancelled).
        let fired = engine.fired_turns();
        assert!(
            !fired.is_empty() && fired.iter().all(|id| id == "turn-n"),
            "the bound cancel must fire only the observed turn's token"
        );
        assert!(
            engine.disposition_count() >= 1,
            "a delivered bound cancel publishes the stop disposition"
        );

        // Skip: the engine already moved on to a self-started follow-up; the
        // stale bound cancel must be dropped wholesale. End the observed turn
        // through the authoritative terminal path first, then reserve and
        // start the next user turn.
        assert!(lifecycle.claim_terminal().is_some());
        lifecycle.finish_terminal_emission();
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));
        lifecycle.on_started("turn-n-plus-1".to_string());
        let engine = FakeTurnSlotEngine::installed_on("turn-n-plus-2-auto");
        let dispositions_before = engine.disposition_count();
        run_production_cancel_wiring(&locks, &lifecycles, &shell_tasks, sid, &engine).await;
        // The observed turn N+1 is the target but the slot names the
        // self-started N+2: the foundation identity check skips it.
        assert!(
            engine.fired_turns().is_empty(),
            "the follow-up turn's token must not be fired by a stale bound cancel"
        );
        assert_eq!(
            engine.disposition_count(),
            dispositions_before,
            "a skipped bound cancel publishes no disposition"
        );
    }

    #[tokio::test]
    async fn terminal_closing_stop_spares_the_followup_token_but_publishes_disposition() {
        // Terminal closing x engine at N+1 (the review-found P2 window): the
        // target turn already ended, the slot holds a self-started follow-up
        // turn's live token. The stop must publish its disposition (steer
        // loss contract) and never fire that token.
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-closing-vs-followup";

        let lifecycle = lifecycles.for_session(sid);
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));
        assert!(lifecycle.claim_terminal().is_some());
        let engine = FakeTurnSlotEngine::installed_on("turn-n-plus-1-auto");

        let identities_seen = Arc::new(StdMutex::new(Vec::new()));
        let seen = identities_seen.clone();
        cancel_turn_with_gates(
            &locks,
            &lifecycles,
            &shell_tasks,
            sid,
            deepseek_tui::core::engine::CancelMode::StopDropInbox,
            || async { Some(engine.clone()) },
            move |engine: &Arc<FakeTurnSlotEngine>, identity: Option<TurnIdentity>| {
                if let Some(seen_identity) = &identity {
                    seen.lock().expect("seen").push(seen_identity.clone());
                }
                dispatch_turn_bound_cancel(
                    engine.as_ref(),
                    identity.as_ref(),
                    deepseek_tui::core::engine::CancelMode::StopDropInbox,
                );
            },
            |_engine: &Arc<FakeTurnSlotEngine>| async {},
            |_lc, _target| false,
        )
        .await;

        let seen = identities_seen.lock().expect("seen");
        assert!(
            !seen.is_empty(),
            "the closure must run during terminal closing"
        );
        assert!(
            seen.iter().all(|identity| identity.closing),
            "the dispatch must see the closing discriminator"
        );
        assert!(
            engine.fired_turns().is_empty(),
            "no token may fire while the target turn has already ended"
        );
        assert!(
            engine.disposition_count() >= 1,
            "the stop disposition must be published during terminal closing"
        );
    }

    #[tokio::test]
    async fn pending_cancel_is_armed_before_cancel_current_in_phase_two() {
        // Deterministic regression of reviewer point 2: phase two must
        // `arm_pending_cancel` before `cancel_current`. With the order
        // reversed (cancel first, arm after):
        //   cancel hits the old token → engine reset_cancel_token +
        //   TurnStarted → forwarder does not re-issue the cancel because
        //   nothing is armed → when arm runs here the `turn_id` already
        //   exists and is rejected → the stop request is lost.
        //
        // After atomization (reviewer point 8) arm and cancel_current
        // complete inside the same state-lock critical section, the order
        // guaranteed by `arm_pending_cancel_and_cancel` internally, so
        // TurnStarted cannot be inserted between the two steps (the
        // forwarder needs the same lock to consume the pending).
        // Here we verify the delivery invariant: after the cancel runs, the
        // pending can still be consumed by the forwarder (take yields Some) —
        // i.e. the "arm before cancel" effect is preserved.
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-arm-order";

        let lifecycle = lifecycles.for_session(sid);
        // turn: on_submitted activates it (submitted but not started, turn_id
        // still None, epoch=1) — arm_pending_cancel's preconditions hold.
        assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));

        let cancel_calls = Arc::new(AtomicU64::new(0));
        let probe_calls = cancel_calls.clone();
        // phase 1's cascade cancel is treated as successfully enqueued (this
        // test focuses on the arm-order invariant and does not cover reviewer
        // point 9's try_send-failure resend path).
        cancel_turn_with_gates(
            &locks,
            &lifecycles,
            &shell_tasks,
            sid,
            deepseek_tui::core::engine::CancelMode::StopDropInbox,
            // get_engine: engine present.
            || async { Some(()) },
            // cancel_current probe: counting only. The cancel runs inside the state
            // lock, and the probe must not take the lifecycle lock again (std Mutex is
            // not reentrant — it would deadlock); instead, after the call returns,
            // verify that pending can still be consumed by the forwarder.
            move |_engine: &(), _identity: Option<TurnIdentity>| {
                probe_calls.fetch_add(1, Ordering::SeqCst);
            },
            // cascade_cancel: called in phase two on the normal cancel path;
            // a no-op probe here.
            |_engine: &()| async {},
            |_lc, _target| false,
        )
        .await;

        // one cancel each in phase one and phase two (engine present, epoch
        // matches).
        assert!(
            cancel_calls.load(Ordering::Acquire) >= 1,
            "cancel_current must run on the originating turn"
        );
        // arm before cancel: after the cancel runs, pending can still be consumed
        // by the forwarder (simulating take-and-replay when a TurnStarted arrives).
        assert!(
            lifecycle
                .take_pending_cancel(Some(TEST_SUBMISSION))
                .is_some(),
            "pending_cancel must be armed before cancel_current so a TurnStarted can be replayed"
        );
    }

    #[tokio::test]
    async fn cancel_outcome_assembly_inputs_cover_every_terminal_branch() {
        // Sixth review round regression: the CancelOutcome assembly of
        // `EnginePool::cancel` (`terminal: claimed_unsubmitted ||
        // target.is_none() || reserve_gate_open` at the end of cancel() in
        // this file) — any true → terminal=true, all false → false. EnginePool
        // construction depends on AppHandle (tauri's test feature is not
        // enabled and the repo has no mock_app precedent), so cancel() itself
        // is not unit-testable; this test executes the exact same assembly
        // expression branch by branch with the exact same input sources
        // (cancel_turn_with_gates' return value + TurnLifecycle::
        // is_reserve_gate_open_for), locking the truth table.

        // Branch "target.is_none()": idle session (no lifecycle) → no event
        // to wait for, terminal=true (the frontend must not wait for a
        // chat:done in vain).
        {
            let locks = SessionTurnLocks::default();
            let lifecycles = SessionTurnLifecycles::default();
            let shell_tasks = SessionTurnShellTasks::default();
            let sid = "session-outcome-idle";
            let (target, claimed_unsubmitted) = cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                deepseek_tui::core::engine::CancelMode::StopDropInbox,
                || async { None::<()> },
                |_engine: &(), _identity: Option<TurnIdentity>| {},
                |_engine: &()| async {},
                |_lc, _target| false,
            )
            .await;
            let reserve_gate_open = lifecycles
                .get(sid)
                .is_some_and(|lc| lc.is_reserve_gate_open_for(target));
            assert_eq!(target, None, "idle session has no target generation");
            assert!(!claimed_unsubmitted, "nothing to claim while idle");
            assert!(
                claimed_unsubmitted || target.is_none() || reserve_gate_open,
                "idle session must assemble terminal=true (no chat:done to wait for)"
            );
        }

        // Branch "claimed_unsubmitted": unsubmitted reservation + engine
        // absent → the claim path claims the terminal (its chat:done is
        // emitted before the cancel returns, so a frontend listener always
        // misses it) → claimed_unsubmitted=true → terminal=true.
        {
            let locks = SessionTurnLocks::default();
            let lifecycles = SessionTurnLifecycles::default();
            let shell_tasks = SessionTurnShellTasks::default();
            let sid = "session-outcome-claimed";
            let lifecycle = lifecycles.for_session(sid);
            let _reservation = lifecycle.reserve().expect("unsubmitted reservation");
            let (target, claimed_unsubmitted) = cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                deepseek_tui::core::engine::CancelMode::InterruptKeepInbox,
                || async { None::<()> },
                |_engine: &(), _identity: Option<TurnIdentity>| {},
                |_engine: &()| async {},
                |_lc, _target| true,
            )
            .await;
            let reserve_gate_open = lifecycles
                .get(sid)
                .is_some_and(|lc| lc.is_reserve_gate_open_for(target));
            assert_eq!(target, Some(1));
            assert!(
                claimed_unsubmitted,
                "the claim path must surface as claimed_unsubmitted"
            );
            assert!(
                claimed_unsubmitted || target.is_none() || reserve_gate_open,
                "claimed terminal must assemble terminal=true (its chat:done is unobservable)"
            );
        }

        // Branch "reserve_gate_open" and the all-false contrast: a submitted
        // turn still closing its terminal after cancel (the persistence window
        // between claim and finish, gate closed) → all three inputs false →
        // terminal=false (the frontend waits for chat:done); once the
        // forwarder finishes closing (finish_once = claim + emit + finish,
        // same order as the authoritative path) the gate reopens
        // → terminal=true.
        {
            let locks = SessionTurnLocks::default();
            let lifecycles = SessionTurnLifecycles::default();
            let shell_tasks = SessionTurnShellTasks::default();
            let sid = "session-outcome-gate";
            let lifecycle = lifecycles.for_session(sid);
            assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));
            let (target, claimed_unsubmitted) = cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                deepseek_tui::core::engine::CancelMode::StopDropInbox,
                || async { Some(()) },
                |_engine: &(), _identity: Option<TurnIdentity>| {},
                |_engine: &()| async {},
                |_lc, _target| false,
            )
            .await;
            assert_eq!(target, Some(1));
            assert!(
                !claimed_unsubmitted,
                "a submitted turn never takes the claim path"
            );
            let reserve_gate_open = lifecycles
                .get(sid)
                .is_some_and(|lc| lc.is_reserve_gate_open_for(target));
            assert!(
                !(claimed_unsubmitted || target.is_none() || reserve_gate_open),
                "terminal closing in progress: all inputs false -> terminal=false (frontend waits for chat:done)"
            );
            // Terminal closing finished (gate reopening precedes the
            // chat:done emit, same synchronous block) → gate reopened.
            assert!(lifecycle.finish_once(|| {}).is_some());
            let reserve_gate_open = lifecycles
                .get(sid)
                .is_some_and(|lc| lc.is_reserve_gate_open_for(target));
            assert!(
                claimed_unsubmitted || target.is_none() || reserve_gate_open,
                "reserve gate reopened: terminal=true is safe (chat:done already emitted)"
            );
        }
    }

    #[tokio::test]
    async fn cancel_gates_arm_pending_cancel_with_the_given_steering_mode() {
        // Lock the downstream half of the keep_inbox → CancelMode mapping
        // (sixth review round): EnginePool::cancel picks InterruptKeepInbox
        // (interrupt) or StopDropInbox (stop) per keep_inbox and passes it
        // into this function; this test locks "whichever mode is passed in,
        // the pending_cancel armed during the submit→TurnStarted window
        // carries that same mode for the forwarder replay" — preventing
        // cancel_turn_with_gates from internally degrading to a hard-coded
        // StopDropInbox (a ⚡ interrupt would wrongly clear un-injected queued
        // steers). The literal `if keep_inbox` mapping inside cancel() and the
        // idle StopDropInbox re-issue need an EnginePool (AppHandle) plus a
        // live engine, untestable in the current harness (consistent with the
        // note on require_live_engine_for_steer).
        for mode in [
            deepseek_tui::core::engine::CancelMode::InterruptKeepInbox,
            deepseek_tui::core::engine::CancelMode::StopDropInbox,
        ] {
            let locks = SessionTurnLocks::default();
            let lifecycles = SessionTurnLifecycles::default();
            let shell_tasks = SessionTurnShellTasks::default();
            let sid = if mode == deepseek_tui::core::engine::CancelMode::InterruptKeepInbox {
                "session-mode-keep-inbox"
            } else {
                "session-mode-stop-drop"
            };
            let lifecycle = lifecycles.for_session(sid);
            // Submitted but TurnStarted not yet arrived (turn_id=None) → the
            // arm preconditions hold.
            assert!(lifecycle.on_submitted(Some(TEST_SUBMISSION.to_string())));
            let epoch = lifecycle.current_turn_generation().expect("active epoch");
            cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                mode,
                || async { Some(()) },
                |_engine: &(), _identity: Option<TurnIdentity>| {},
                |_engine: &()| async {},
                |_lc, _target| false,
            )
            .await;
            assert_eq!(
                lifecycle.take_pending_cancel(Some(TEST_SUBMISSION)),
                Some((epoch, mode)),
                "armed pending_cancel must carry the CancelMode passed to cancel_turn_with_gates"
            );
            assert!(lifecycle.finish_once(|| {}).is_some());
        }
    }
}

/// Wiring tests for spawn-time probe adoption, in two layers:
/// `finalize_runtime_bridge` (the real production injection block, drivable
/// directly as an associated function) pins the provider() derivation, the
/// effective_model_owned gating, and the adopt call itself;
/// `adopt_probed_endpoint_facts` (the testable core) pins four paths through
/// a real HTTP mock (127.0.0.1:0) — non-vLLM single-entry "borrowed name" is
/// not adopted, exact match is adopted, vLLM renames + adopts, vLLM pinned
/// name still adopts (an intentional trade-off, see the
/// `adopts_probed_facts` docs), plus cloud presets are not probed. Review
/// round-2 found the production injection point had zero tests (pure-function
/// guarantees cannot cover the spawn wiring); round-3 added the finalize
/// layer — previously only the core had tests, so deleting the injection
/// block in finalize would not fail any test.
#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod probed_facts_wiring_tests {
    use super::{EnginePool, Pinvou3Bridge, PreparedRuntimeModel};
    use crate::core::model_endpoint::{LocalServerKind, models_mock};
    use crate::platform::credential_store::CredentialState;
    use crate::platform::paths::tests::ENV_LOCK;
    use crate::platform::prefs::{ImageCapabilityOverride, ModelPreset, SavedModel};
    use crate::platform::test_support::EnvRestore;

    // `EnvRestore` reuses the shared implementation from
    // `platform::test_support` (same comment above the tests module).

    /// Isolates env vars related to base_url/api_key (`Pinvou3Bridge::
    /// base_url`/`api_key` prioritize env over session model); the returned
    /// guard restores them when the test ends.
    fn isolate_model_env() -> EnvRestore {
        let restore = EnvRestore::capture(&["DEEPSEEK_BASE_URL", "DEEPSEEK_API_KEY"]);
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::remove_var("DEEPSEEK_BASE_URL") };
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };
        restore
    }

    fn saved_model(preset: ModelPreset, model: &str, provider_kind: Option<&str>) -> SavedModel {
        SavedModel {
            id: "wiring-model".into(),
            name: "Wiring".into(),
            alias: None,
            preset,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: model.into(),
            base_url: String::new(),
            provider_kind: provider_kind.map(Into::into),
            vendor: None,
            endpoint_mode: None,
            image_capability_override: ImageCapabilityOverride::default(),
            vision_model_id: None,
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        }
    }

    fn wiring_bridge(model: SavedModel) -> Pinvou3Bridge {
        Pinvou3Bridge::test_fixture(Some(model))
    }

    fn single_entry_json(id: &str) -> String {
        format!(
            r#"{{"data":[{{"id":"{id}","max_model_len":262144,"max_completion_tokens":4096}}]}}"#
        )
    }

    #[tokio::test]
    async fn non_vllm_operator_route_does_not_adopt_borrowed_single_entry() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[("/v1/models", 200, single_entry_json("served-only"))]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, false, false).await;
        assert_eq!(
            bridge.probed_context_tokens, None,
            "window facts from a single-entry borrowed name belong to another model and must not be adopted"
        );
        assert_eq!(
            bridge.probed_output_tokens, None,
            "the self-reported output limit is likewise not adopted"
        );
        assert_eq!(
            bridge.session_model.as_ref().unwrap().model,
            "my-model",
            "non-vLLM routes do no served-name correction"
        );
    }

    #[tokio::test]
    async fn non_vllm_operator_route_adopts_on_exact_name_match() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[("/v1/models", 200, single_entry_json("my-model"))]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, false, false).await;
        assert_eq!(bridge.probed_context_tokens, Some(262_144));
        assert_eq!(bridge.probed_output_tokens, Some(4_096));
        assert_eq!(bridge.session_model.as_ref().unwrap().model, "my-model");
    }

    #[tokio::test]
    async fn vllm_route_renames_to_served_name_and_adopts_facts() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[("/v1/models", 200, single_entry_json("served-actual"))]);
        let mut model = saved_model(ModelPreset::LocalVllm, "my-model", None);
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, true, false).await;
        assert_eq!(
            bridge.session_model.as_ref().unwrap().model,
            "served-actual",
            "a vLLM single entry follows the served name"
        );
        assert_eq!(bridge.probed_context_tokens, Some(262_144));
        assert_eq!(bridge.probed_output_tokens, Some(4_096));
    }

    #[tokio::test]
    async fn vllm_route_pins_scheduled_model_keeps_name_but_adopts_facts() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[("/v1/models", 200, single_entry_json("served-actual"))]);
        let mut model = saved_model(ModelPreset::LocalVllm, "my-model", None);
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, true, true).await;
        assert_eq!(
            bridge.session_model.as_ref().unwrap().model,
            "my-model",
            "name correction is suppressed while a scheduled model is pinned; the configured name goes live verbatim"
        );
        assert_eq!(
            bridge.probed_context_tokens,
            Some(262_144),
            "pinned name + single-entry borrow still adopts facts under vLLM semantics (intentional trade-off, see the adopts_probed_facts docs)"
        );
        assert_eq!(bridge.probed_output_tokens, Some(4_096));
    }

    #[tokio::test]
    async fn cloud_route_is_not_probed_at_all() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[("/v1/models", 200, single_entry_json("my-model"))]);
        let mut model = saved_model(ModelPreset::Deepseek, "my-model", None);
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, false, false).await;
        assert_eq!(
            mock.hits_for("/v1/models"),
            0,
            "cloud presets are not operator-owned; no probe request may be issued"
        );
        assert_eq!(bridge.probed_context_tokens, None);
        assert_eq!(bridge.probed_output_tokens, None);
    }

    /// The real finalize_runtime_bridge injection block (non-vLLM side):
    /// an OpenaiCompatible + custom route goes through the provider()
    /// derivation (local mock URL → kind probe completes as Generic →
    /// "openai") and the effective_model_owned gate to reach adopt; exact
    /// match adopts facts without renaming; the kind probe coexists (TTL
    /// cache is isolated per base_url).
    #[tokio::test]
    async fn finalize_runtime_bridge_injects_probed_facts_into_custom_route() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[("/v1/models", 200, single_entry_json("my-model"))]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        let bridge = wiring_bridge(model.clone());
        let prepared = PreparedRuntimeModel::unchanged(model);
        let bridge = EnginePool::finalize_runtime_bridge(bridge, &prepared, false).await;
        assert_eq!(
            bridge.probed_local_kind,
            Some(LocalServerKind::Generic),
            "the openai route's coexisting kind probe lands on Generic (mock has no kind signature)"
        );
        assert_eq!(bridge.probed_context_tokens, Some(262_144));
        assert_eq!(bridge.probed_output_tokens, Some(4_096));
        assert_eq!(
            bridge.session_model.as_ref().unwrap().model,
            "my-model",
            "non-vLLM routes do no served-name correction"
        );
    }

    /// The real finalize_runtime_bridge injection block (vLLM side):
    /// provider() derives "vllm" (skipping the kind probe) and a single
    /// entry renames to the served name and adopts facts.
    #[tokio::test]
    async fn finalize_runtime_bridge_renames_and_injects_vllm_route() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[("/v1/models", 200, single_entry_json("served-actual"))]);
        let mut model = saved_model(ModelPreset::LocalVllm, "my-model", None);
        model.base_url = mock.base_url.clone();
        let bridge = wiring_bridge(model.clone());
        let prepared = PreparedRuntimeModel::unchanged(model);
        let bridge = EnginePool::finalize_runtime_bridge(bridge, &prepared, false).await;
        assert_eq!(
            bridge.session_model.as_ref().unwrap().model,
            "served-actual",
            "a vLLM single entry follows the served name"
        );
        assert_eq!(bridge.probed_context_tokens, Some(262_144));
        assert_eq!(bridge.probed_output_tokens, Some(4_096));
    }
}
