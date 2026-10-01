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
use std::time::{Duration, Instant};

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

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

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

/// Whether this turn is forced to zero tools: pure-conversation meta card /
/// caller per-turn request / an `aux-` auxiliary conversation — any one
/// forces it.
/// `aux-` is server-side enforced (same idea as the `sched-` prefix guard):
/// the bridge layer always passes `restrictTools: true`, but `restrict_tools`
/// is only an optional call parameter of `chat` / `web_access_chat` — a
/// browser passing false, or any caller bypassing the first-party bridge,
/// could get a full-tool turn in an aux session; hence it is pinned at the
/// pool send chokepoint regardless of the caller's value.
/// edit_last_turn resends do not go through this function (the foundation's
/// Op::EditLastTurn carries no tool surface and reuses the engine config);
/// their zero-tool state is backstopped by the spawn config — see the `aux-`
/// branch of `bridge::build_engine_config_for_session_roots` — and the
/// zero-tool *reminder* is merged into the resent message by
/// `edit_last_turn_reserved` (round-14 minor-1).
pub(crate) fn turn_restrict_tools(
    session_id: &str,
    persona_conversational: bool,
    caller_restrict: bool,
) -> bool {
    persona_conversational
        || caller_restrict
        || crate::features::sessions::is_aux_session_id(session_id)
}

/// Zero-tool leakage guard for aux turns. The empty tool table removes the
/// tool *declarations* from the request, but tool-trained models (DeepSeek
/// emits its native DSML invoke markup) still "call" the removed tools by
/// writing the call syntax into the answer as plain text — the user then sees
/// raw tool-call markup in the aux panel. The per-turn reminder channel (the
/// same one persona anchors ride; the reservation's host-side
/// TranscriptSanitizationRule swaps it for the display copy before the
/// transcript is persisted) tells the model the turn is tool-less up front. The zero-tool
/// guarantee itself is unchanged: this only makes the model aware of it.
/// Kept as defense-in-depth after round-31 M8 isolated the aux engine
/// configuration (minimal instructions, no MCP/subagents/memory/vision):
/// markup emission is trained behavior that no prompt change fully removes,
/// and the reminder restates the boundary next to the user message at a
/// small fixed per-turn cost.
pub(crate) const AUX_ZERO_TOOL_REMINDER: &str = "You are answering in an auxiliary Q&A session. This turn has NO tools: the tool list is empty. Do not attempt to call tools or run commands, and never emit tool-call markup or invoke blocks as text. Answer directly in plain text from the conversation and your own knowledge; if an action is truly needed, explain how the user can do it instead.";

/// Merge the aux zero-tool boundary into the per-turn reminder (aux sessions
/// only; persona anchors, when present, keep their text ahead of it).
pub(crate) fn merge_aux_zero_tool_reminder(
    session_id: &str,
    persona_reminder: Option<String>,
) -> Option<String> {
    if !crate::features::sessions::is_aux_session_id(session_id) {
        return persona_reminder;
    }
    Some(match persona_reminder {
        Some(existing) => format!("{existing}\n\n{AUX_ZERO_TOOL_REMINDER}"),
        None => AUX_ZERO_TOOL_REMINDER.to_string(),
    })
}

/// Per-turn tool restriction, mintable only through the policy above.
///
/// PR #433 review round-10 (S2(b)): `AppEngine::send_reserved_user_message`'s
/// per-turn restriction used to be a bare `bool`, so replacing this composition
/// with the caller's `restrict_tools_for_turn` kept the whole suite green while
/// aux sessions regained full tools — the headline zero-tool wiring rested on
/// review alone. The engine's per-turn send entry now takes this token: the
/// field is private to this module, so outside it a token can only be obtained
/// from [`TurnToolRestrict::forced`], which folds in `turn_restrict_tools`.
/// Handing the caller's `bool` straight to the engine is therefore a compile
/// error, not a silent regression.
pub(crate) mod turn_tool_restrict {
    /// See the module documentation.
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct TurnToolRestrict(bool);

    impl TurnToolRestrict {
        /// Sole constructor: no path from raw flags skips the policy.
        pub(super) fn forced(
            session_id: &str,
            persona_conversational: bool,
            caller_restrict: bool,
        ) -> Self {
            Self(super::turn_restrict_tools(
                session_id,
                persona_conversational,
                caller_restrict,
            ))
        }

        /// The composed per-turn decision (`turn_restrict_tools`: caller
        /// request | pure-conversation meta card | `aux-` prefix).
        pub(crate) fn restricts_tools(self) -> bool {
            self.0
        }

        /// The restriction to apply on the engine that owns `engine_session_id`.
        ///
        /// The `aux-` test runs again against the engine's own id, so a token
        /// minted with an unrelated (or empty) session id can never hand an
        /// aux session a full-tool turn — the headless wrapper mints with the
        /// engine's own id through [`forward_forced_turn_restrict`].
        pub(crate) fn restricts_tools_for(self, engine_session_id: &str) -> bool {
            self.restricts_tools()
                || crate::features::sessions::is_aux_session_id(engine_session_id)
        }
    }
}

/// The "last mile" from decision to dispatch is folded into one function: the
/// forced result computed by `send_reserved_user_message` is handed to the
/// engine's per-turn send entry as a [`turn_tool_restrict::TurnToolRestrict`],
/// whose only constructor is this path, and the outgoing per-turn reminder is
/// produced here by [`merge_aux_zero_tool_reminder`]. The doc comment above
/// records why the parameter is a token instead of the caller's `bool`; the
/// reminder rides the same seam because a bare `Option<String>` assembled at
/// the call site was exactly as unpinned (round-31 M9-rust: deleting the
/// merge call kept the whole suite green) — through this function, removing
/// either the token policy or the reminder merge turns an executing test red.
pub(crate) fn forward_forced_turn_restrict<F>(
    session_id: &str,
    persona_conversational: bool,
    caller_restrict: bool,
    persona_reminder: Option<String>,
    send: impl FnOnce(turn_tool_restrict::TurnToolRestrict, Option<String>) -> F,
) -> F {
    send(
        turn_tool_restrict::TurnToolRestrict::forced(
            session_id,
            persona_conversational,
            caller_restrict,
        ),
        merge_aux_zero_tool_reminder(session_id, persona_reminder),
    )
}

/// The edit-resend counterpart of [`forward_forced_turn_restrict`]'s last
/// mile: `edit_last_turn_reserved` bypasses the send path, so its aux
/// zero-tool reminder is merged into the resent message here — the merge and
/// the engine dispatch are folded into one function so the wiring (not only
/// the pure helper) is covered by an executing test (round-31 M9-rust: this
/// was the second unpinned `merge_aux_zero_tool_reminder` call site).
pub(crate) fn forward_edit_resend_with_reminder<F>(
    session_id: &str,
    new_message: String,
    send: impl FnOnce(String) -> F,
) -> F {
    send(match merge_aux_zero_tool_reminder(session_id, None) {
        Some(reminder) => {
            format!("<system-reminder>\n{reminder}\n</system-reminder>\n\n{new_message}")
        }
        None => new_message,
    })
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
/// Emptiness probe for the guarded stub delete. The store method is
/// headless-surface (`benchmark-hooks`/test); the gate function also serves
/// GUI builds, where the guarded variant has no caller and the probe is
/// stubbed to "delete unconditionally" so the branch can never fire.
#[cfg(any(feature = "benchmark-hooks", test))]
fn record_is_message_free(store: &SessionStore, session_id: &str) -> bool {
    matches!(store.chat_session_has_messages(session_id), Ok(false))
}

#[cfg(not(any(feature = "benchmark-hooks", test)))]
fn record_is_message_free(_store: &SessionStore, _session_id: &str) -> bool {
    true
}

/// Adoption probe for the one-shot delete gate. Same headless-surface split
/// as [`record_is_message_free`]: the store method only exists for the
/// headless builds, and GUI builds stub it to "titled", so the guarded
/// branch can never fire there (a GUI user deleting a renamed chat through
/// `delete_chat_session` takes the unconditional arm anyway).
#[cfg(any(feature = "benchmark-hooks", test))]
fn record_is_factory_titled(store: &SessionStore, session_id: &str) -> bool {
    store
        .chat_session_factory_titled(session_id)
        .unwrap_or(false)
}

#[cfg(not(any(feature = "benchmark-hooks", test)))]
fn record_is_factory_titled(_store: &SessionStore, _session_id: &str) -> bool {
    true
}

/// What the delete gate re-verifies before the durable delete fires. Every
/// disposition that reaches this gate sampled its fact OUTSIDE the turn
/// lock, and a live turn can hold that lock for the turn's whole wall
/// clock — so each sampled fact must be re-verified under the gate before
/// it may destroy the record.
enum DeleteGateRecheck {
    /// An explicit deletion (GUI chat delete, eval temp session): whatever
    /// the caller asked to delete may be deleted.
    Unconditional,
    /// Headless stub cleanup: keep unless the record is still a message-free
    /// stub wearing the factory title — a turn admitted under the gate made
    /// it a started transcript, and a rename under the gate made it an
    /// adopted (GUI-owned) session.
    StillAStub,
    /// One-shot falsy cleanup: keep unless the record still wears the
    /// factory title. A read error keeps too: deleting on unknown state is
    /// the unsafe direction.
    StillFactoryTitled,
}

/// Turn-gated durable delete. Under the gate the sampled fact is re-checked
/// (per [`DeleteGateRecheck`]), and a record that no longer matches is kept
/// (`Ok(false)`, engine reclaimed, nothing deleted) instead of destroyed.
async fn delete_chat_session_with_gate<F, Fut, G>(
    turn_locks: &SessionTurnLocks,
    store: &SessionStore,
    session_id: &str,
    recheck: DeleteGateRecheck,
    evict_locked: F,
    forget: G,
) -> Result<bool>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
    G: FnOnce(),
{
    let turn_lock = turn_locks.for_session(session_id).await;
    let _turn = turn_lock.lock().await;
    let holds = match recheck {
        DeleteGateRecheck::Unconditional => true,
        DeleteGateRecheck::StillAStub => {
            record_is_message_free(store, session_id) && record_is_factory_titled(store, session_id)
        }
        DeleteGateRecheck::StillFactoryTitled => record_is_factory_titled(store, session_id),
    };
    if !holds {
        // The engine teardown below is everything the keep path would have
        // done, so the skip reports success — the caller's cleanup-failure
        // lane stays reserved for genuine delete faults. The skip can only
        // fire on the headless builds (the GUI stubs both probes to "holds"),
        // so the notice is gated with them; note_stderr keeps a closed
        // stderr from panicking the await chain under the report.
        #[cfg(any(feature = "benchmark-hooks", test))]
        crate::features::assistant::product_runtime::note_stderr(&format!(
            "[agent-task] cleanup skipped under the delete gate: the record no \
             longer matches the sampled disposition; keeping the session"
        ));
        evict_locked().await;
        return Ok(false);
    }
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
    Ok(true)
}

/// Aux-aware chat delete: deleting a main chat first deletes its aux session
/// through the same supplied gated delete (depth 1 — aux sessions never own
/// another aux, so no recursion guard is needed beyond the prefix check).
/// `SessionStore::delete`'s record-level cascade alone cannot reach the
/// engine/forwarder and would leave a still-running aux engine as a
/// handle-less orphan, so every deletion of a chat session must go through
/// this wrapper rather than bare `store.delete` (round-13 M-B: the eval
/// close path and the web-session rollback both bypassed the command-layer
/// cascade before this wrapper existed).
async fn delete_chat_session_with_aux_cascade<De, DeFut>(
    store: &SessionStore,
    session_id: &str,
    mut delete: De,
) -> Result<bool>
where
    De: FnMut(&str) -> DeFut,
    DeFut: Future<Output = Result<bool>>,
{
    if !crate::features::sessions::is_aux_session_id(session_id) {
        if let Some(aux_id) = store.aux_session_id(session_id) {
            delete(&aux_id)
                .await
                .map(|_| ())
                .context("delete the aux session before its main session")?;
        }
    }
    delete(session_id).await
}

/// The gated delete half of the atomic aux reset (M6): resolve the task's
/// derived aux id and delete it through the exact turn gate used by lazy
/// spawn and send (`delete_chat_session_with_gate`), so a queued sender
/// observes the completed delete instead of resurrecting the session. Returns
/// the deleted aux id, or `None` when no aux record exists (idempotent —
/// the create half then simply makes the fresh session). Never substitute a
/// bare `store.delete`: the mutation test
/// `aux_reset_delete_waits_for_the_turn_gate` holds the gate and goes red if
/// this body proceeds without it.
async fn reset_aux_session_delete_with_gate<F, Fut, G>(
    turn_locks: &SessionTurnLocks,
    store: &SessionStore,
    main_id: &str,
    evict_locked: F,
    forget: G,
) -> Result<Option<String>>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = ()>,
    G: FnOnce(&str),
{
    let Some(aux_id) = store.aux_session_id(main_id) else {
        return Ok(None);
    };
    let evict_id = aux_id.clone();
    delete_chat_session_with_gate(
        turn_locks,
        store,
        &aux_id,
        // An explicit aux reset is an explicit deletion (the aux id is
        // task-derived and cannot be user-renamed), so the gate recheck
        // stays unconditional under the round-26 enum.
        DeleteGateRecheck::Unconditional,
        || evict_locked(evict_id),
        || forget(aux_id.as_str()),
    )
    .await?;
    Ok(Some(aux_id))
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

/// A reused engine's pending native-window re-check (see
/// `EngineEntry::native_window_pending`): which server kind to ask and for
/// which model name. No URL or credential — the re-probe runs against the
/// freshly prepared bridge's base_url/key, which for a fresh entry is the
/// same model config the engine was spawned from.
#[derive(Clone, Debug, PartialEq)]
struct PendingNativeWindow {
    model_name: String,
    kind: crate::core::model_endpoint::LocalServerKind,
}

impl PendingNativeWindow {
    /// Captured from the finalized spawn bridge: only routes that are
    /// native-probeable (Ollama / LM Studio), ended the spawn without a
    /// served-window fact, and had the adoption path arm the re-check
    /// (`bridge.native_window_recheck` — true exactly when that route's own
    /// native fetch ran and served no fact). Arming is part of the
    /// construction so a fact the route can never adopt (a borrowed roster
    /// name, a non-operator-owned endpoint) or a declared route (whose own
    /// fetch is skipped, so it never produces a fact to re-check) can never
    /// mark the entry: an
    /// armed re-probe that keeps serving a fact the rebuild path refuses to
    /// adopt would drop and respawn the engine on every send, forever. A
    /// fact present at spawn means the normal frozen-until-rebuild
    /// discipline applies (no re-checking).
    fn from_finalized(bridge: &Pinvou3Bridge) -> Option<Self> {
        use crate::core::model_endpoint::LocalServerKind;
        if !bridge.native_window_recheck || bridge.probed_context_tokens.is_some() {
            return None;
        }
        let kind = bridge.probed_local_kind?;
        if !matches!(kind, LocalServerKind::Ollama | LocalServerKind::LmStudio) {
            return None;
        }
        let model_name = bridge.effective_model_owned()?.model;
        Some(Self { model_name, kind })
    }
}

/// The reuse-vs-self-heal outcome for a fresh cached entry (see
/// `EngineEntry::native_window_pending`). Standalone so the decision is
/// unit-testable without an AppHandle-backed pool (same idiom as
/// `require_live_engine_for_steer` / `delegate_steer_withdrawal`).
#[derive(Debug, PartialEq, Eq)]
enum CachedEntryReuse {
    /// Keep the cached engine for this turn.
    Keep,
    /// The pending window fact materialized since spawn: reclaim the entry
    /// and fall through to the rebuild path so the fact is adopted.
    RebuildForWindowFact,
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
    /// `mark_mcp_config_updated`; the next engine fetch safely reclaims the
    /// old instance and lazily rebuilds. A plain session's engine reads the
    /// session-derived mcp config (rewritten from the global mcp.json only at
    /// spawn); without a rebuild, servers installed midway would never be
    /// visible to the live engine.
    mcp_config_revision: u64,
    /// Set at spawn when the route is native-probeable (kind Ollama / LM
    /// Studio, local) but no served-window fact could be adopted — the
    /// common shape of a model that was never JIT-loaded yet. While this is
    /// Some, engine reuse re-checks the native API once per turn
    /// (`reprobe_missing_native_window`): the first post-load check finds
    /// `/api/ps` / `/api/v0/models` serving the real window and the entry is
    /// dropped for a rebuild that adopts it — instead of the collapsed
    /// 8192-fallback budget surviving the whole first engine lifetime
    /// (2026-09-30 user report). Holds only the model name and server kind
    /// (no credentials — the re-probe resolves the key from the freshly
    /// prepared bridge, which for a fresh entry is the same model config).
    native_window_pending: Option<PendingNativeWindow>,
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

    /// Whole-struct equality on purpose: the derived `PartialEq` makes every field of
    /// `PreparedRuntimeState` participate automatically, so a field added later cannot be
    /// forgotten here and silently leave a session running on a stale engine (#253/#385).
    /// A hand-written field list would have to be kept in sync by review alone.
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
                // No session id in the log: even a digested rendering trips
                // the cleartext-logging scanner (its source is tainted and
                // sanitizer-less, same as checkpoints.rs:556/564 on main).
                eprintln!(
                    "[engine_pool] a session idle for over {IDLE_EVICT_AFTER_SECS}s had its engine reclaimed (lazily respawned on the next message)"
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
            // Aux engines are isolated (no skill surface); recreating the
            // composed directory here would make a `## Skills` block appear
            // on the next aux turn (round-31 M8).
            if crate::features::sessions::is_aux_session_id(&sid) {
                continue;
            }
            let scope = self.bridge.session_policy(&sid).mode();
            let project_workspace = self.project_workspace_for(&sid);
            let sid_for_log = sid.clone();
            if let Err(join_error) = tokio::task::spawn_blocking(move || {
                crate::features::assistant::skill_materialization::rewrite_session_skills(
                    &sid,
                    scope,
                    project_workspace.as_deref(),
                );
            })
            .await
            {
                // Best-effort refresh: a panicked rewrite leaves the composed
                // dirs stale until the next materialization, but the join
                // failure itself must not vanish silently — every other
                // spawn_blocking join in the engine fails closed or logs.
                // eprintln, not log: the headless host installs no logger
                // (`run_windowless_host` builds a bare Tauri app), so a
                // `log::warn!` here is dropped in exactly the process this
                // path was hardened for. Every other diagnostic in this file
                // uses eprintln for the same reason.
                eprintln!(
                    "[engine_pool] session {sid_for_log} skills rewrite join failed: {join_error}"
                );
            }
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

    /// Prepare an unsent composer's model (the current default model) without
    /// borrowing any existing chat session or creating a synthetic one. It goes
    /// through the same passthrough preparation and spawn-time finalization as
    /// a session bridge, so served-name correction and probed endpoint facts
    /// match what chat would use.
    pub(crate) async fn fresh_bridge_for_draft(&self) -> Result<Pinvou3Bridge> {
        let mut bridge = self.bridge.clone();
        bridge.prefs = UserPrefs::load();
        let model = bridge
            .prefs
            .active_model()
            .cloned()
            .context("No effective model is available for draft preparation")?;
        let prepared = PreparedRuntimeModel::unchanged(model);
        Ok(Self::finalize_runtime_bridge(bridge, &prepared, false).await)
    }

    async fn prepare_runtime_model(
        &self,
        session_id: &str,
        scheduled_unattended: bool,
        explicit_model_override: Option<SavedModel>,
    ) -> Result<(Pinvou3Bridge, PreparedRuntimeModel, bool)> {
        Self::prepare_runtime_model_with(
            &self.store,
            self.bridge.clone(),
            session_id,
            scheduled_unattended,
            explicit_model_override,
        )
        .await
    }

    /// The preparation body behind [`Self::prepare_runtime_model`], as an
    /// associated function so tests can drive the real funnel — including the
    /// session-affinity latch — without a pool (a real EnginePool needs a
    /// Tauri AppHandle; see `install_session_affinity_key`).
    async fn prepare_runtime_model_with(
        store: &SessionStore,
        base_bridge: Pinvou3Bridge,
        session_id: &str,
        scheduled_unattended: bool,
        explicit_model_override: Option<SavedModel>,
    ) -> Result<(Pinvou3Bridge, PreparedRuntimeModel, bool)> {
        let mut bridge = base_bridge;
        bridge.prefs = UserPrefs::load();
        Self::install_session_affinity_key(&mut bridge, session_id);
        let scheduled_profile = store.scheduled_profile(session_id);
        // Same caliber as the command layer chat.rs's is_scheduled (a
        // scheduled_profile existing is enough): a scheduled session's images
        // always take the image_analyze hard rule, even with an interactive
        // model override, so the always flag must not use the narrower
        // pins_scheduled_model.
        bridge.image_analyze_always = scheduled_profile.is_some();
        let interactive_model_override = store.session_model_override(session_id);
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

    /// One OpenCode gateway session-affinity ID per conversation: key the
    /// `x-opencode-session` header by session id so engine respawns of the
    /// same session keep a single stable value
    /// (`core::model_endpoint::opencode_session_id_for`). Associated function
    /// (no pool state); the funnel wiring — this latch being reached on every
    /// spawn — is pinned by driving the real preparation body
    /// (`prepare_runtime_model_with`) in tests.
    fn install_session_affinity_key(bridge: &mut Pinvou3Bridge, session_id: &str) {
        bridge.session_affinity_key = Some(session_id.to_string());
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
        // Local endpoints (OpenAI-compatible presets and LocalVllm presets
        // pointing at local/intranet services): probe the service type
        // (Ollama / vLLM / LM Studio / generic). An OpenaiCompatible route's
        // thinking control follows the detected foundation wire protocol; a
        // LocalVllm preset resolves provider from the preset arm instead, so
        // for those routes the kind feeds only the native served-window
        // follow-up below. LocalVllm presets must probe too: discovery saves
        // Ollama/LM Studio endpoints under that preset, and `/v1/models`
        // there never carries a window fact — without the kind probe the
        // native fetch below can never fire and the route keeps the 128K
        // fallback while the monitor shows the real window. The probe result
        // cannot flip the wire route for these presets (`provider()` resolves
        // LocalVllm to "vllm" from the preset arm before ever consulting the
        // probed kind), so a real vLLM classified `Vllm` is inert here.
        // A probe failure (service not started/timeout/auth failure) is
        // classified as generic, keeping the existing openai wire route. The
        // probe request carries a credential from the same origin as real
        // inference (bridge.api_key()): authenticated vLLM (--api-key) 401s
        // on /v1/models without credentials, and misclassifying it as a
        // generic endpoint loses the local default thinking tier and the
        // vLLM tiers (inference itself still succeeds with the configured
        // key).
        if (bridge.provider() == "openai" || bridge.provider() == "vllm")
            && base_url_uses_local_or_private(&bridge.base_url())
        {
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
        // facts only when the sent name is the served one
        // (`adopts_probed_facts`): a kept multi-entry name trivially is
        // (served == configured), while a single-entry "borrowed name"
        // returns facts belonging to another model and must not be
        // misattributed. Cloud presets and coding_plan are not
        // operator-owned and are not probed.
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
    ///
    /// Ollama / LM Studio exception: their OpenAI-compatible `/v1/models`
    /// never carries a window fact, so an endpoint probed as one of them
    /// gets a native follow-up for the name actually sent — Ollama's
    /// `/api/ps` effective context → `/api/show` Modelfile `num_ctx`
    /// declaration, LM Studio's `/api/v0/models` served window
    /// (`loaded_context_length` on a loaded entry). Only provable runtime
    /// facts are adopted: a model that was never loaded and declares no
    /// `num_ctx` stays without a window (the GGUF trained cap is a
    /// capability ceiling — the server actually serves its 4096-class
    /// default, which no API exposes — and adopting it would loosen budgets
    /// into silent upstream truncation). Without any fact the route declares
    /// no window and the foundation derives its budget from the 8192
    /// unknown-Ollama fallback — compaction floors at 4096 and the
    /// post-compaction input budget collapses to 1024 (2026-09-30 user
    /// report: Ollama actually serving 131072); engine reuse re-checks the
    /// missing fact (`native_window_pending`), so the first JIT load is
    /// picked up on the next send instead of at the idle reap.
    ///
    /// Arms the reuse re-check (`bridge.native_window_recheck`) exactly when
    /// this route's own native fetch ran and served no fact. A route whose
    /// fact can never be adopted (`adopts` false on a borrowed roster name,
    /// or not operator-owned) never runs the fetch and never arms — the
    /// re-probe would keep serving a fact the rebuild refuses to adopt and
    /// respawn the engine on every send; a declared route skips the fetch
    /// entirely — its budget is the declaration by construction, and the
    /// per-turn re-check plus an adopting rebuild must not second-guess an
    /// explicit declaration (the monitor display still shows the smaller
    /// native fact where one exists; a stale declaration is the user's to
    /// fix).
    async fn adopt_probed_endpoint_facts(
        bridge: &mut Pinvou3Bridge,
        mut model: SavedModel,
        is_vllm_route: bool,
        pins_scheduled_model: bool,
    ) {
        // Only the route's own missed native fetch sets this below; every
        // other path (adopt-less, adopted, declared) leaves it cleared, so
        // reusing a bridge never inherits a stale arming.
        bridge.native_window_recheck = false;
        if !(is_vllm_route || model.is_operator_owned_endpoint()) {
            return;
        }
        // Read before `renamed` below moves `model` into `session_model`.
        let declared_window = model.context_window_tokens;
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
        let renamed = is_vllm_route && served != model.model && !pins_scheduled_model;
        let sent_name = if renamed {
            served.clone()
        } else {
            model.model.clone()
        };
        if renamed {
            model.model = served;
            bridge.session_model = Some(model);
        }
        if adopts {
            bridge.probed_context_tokens = max_len;
            bridge.probed_output_tokens = max_output;
            // Native window fact for probed-Ollama / probed-LM Studio
            // endpoints, only when the list probe had none and the facts are
            // adoptable (the queried name is the one actually sent, so the
            // fact belongs to this route by construction). Ollama's
            // `/api/ps`→`/api/show` cascade and LM Studio's
            // `/api/v0/models` served window are the only native facts these
            // servers expose; every other kind keeps the listing (vLLM and
            // friends list `max_model_len`) or no fact (generic). A model
            // that was never loaded and declares no `num_ctx` stays without
            // an Ollama window (the GGUF trained cap is a capability
            // ceiling — the server actually serves its 4096-class default,
            // which no API exposes — and adopting it would loosen budgets
            // into silent upstream truncation); engine reuse re-checks the
            // missing fact once per turn (`native_window_pending`), so the
            // first load is picked up by the next send without waiting for
            // the idle reap. For non-vLLM routes the `adopts` gate is an
            // exact-name match on the sent name; a tagless configured name
            // still resolves its own `/api/ps` entry via Ollama's
            // `name:latest` canonicalization, while a genuinely different
            // name (case- or tag-different) on a multi-entry list fires the
            // native fetch and safely misses the keyed native maps
            // (conservative, already a Mismatch display state). Adopted
            // native values carry no sanity band — deliberately the same
            // trust class as user declarations and vLLM `max_model_len`: a
            // tiny honest value (a Modelfile `num_ctx 512` typo, a small
            // global default) is adopted as-is and preflight fails per turn
            // with the visible context-recovery error — loud-and-true
            // instead of the 8192-pretense silently truncating upstream.
            // There is no in-app override for local presets (the
            // declaration field is hidden and declarations only min-clamp),
            // so the escape is the real fix — the server-side context
            // configuration; the adopted value stays visible on the monitor
            // card / progress denominator.
            if max_len.is_none() && declared_window.is_none() {
                let native = crate::core::model_endpoint::fetch_native_served_context(
                    bridge.probed_local_kind,
                    &bridge.base_url(),
                    Some(api_key.as_str()),
                    &sent_name,
                )
                .await;
                match native {
                    Some(Some(ctx)) => bridge.probed_context_tokens = Some(ctx),
                    Some(None) => {
                        // The route asked its own native API and got no fact
                        // — exactly the shape the reuse re-check exists for.
                        bridge.native_window_recheck = true;
                    }
                    None => {}
                }
            }
        }
    }

    /// Re-check for a reused engine whose spawn ended without a native
    /// served-window fact (`EngineEntry::native_window_pending`):
    /// re-classify the endpoint (cache-peeked; the battery re-runs at most
    /// once per TTL) and ask the native API that classification selects.
    /// Returns the window once the server can actually serve it (model
    /// JIT-loaded since), None while it still can't (unloaded / endpoint
    /// down / no declaration) — the caller keeps the engine in that case.
    /// `bridge` is the freshly prepared (unfinalized) bridge of this same
    /// model config; the fact is deliberately not written here — adoption
    /// goes through the normal finalize so the route limits derive in one
    /// place.
    ///
    /// The marker's frozen kind is deliberately not trusted for dispatch:
    /// the stack behind the port can swap while the engine lives (dev
    /// restart onto a different server class), and re-dialing the stale
    /// kind's API would 404 on every send without ever arming a fact — the
    /// 8192-fallback budget would persist past the point where the new
    /// server could serve the real window. A re-classification to a
    /// non-probeable kind lands on the helper's `None` arm and keeps (a
    /// Generic result is never cached, so that shape re-runs the battery
    /// per send — small local GETs, bounded, and the marker heals or drops
    /// at the next rebuild).
    ///
    /// Native probes are local-only by construction (the marker is armed
    /// inside the locality-gated finalize block, and a config edit that
    /// changes the base_url reclaims the entry via whole-struct equality).
    /// The direct guard below keeps that property local to this function —
    /// a weakening of the provenance chain upstream cannot silently turn
    /// the reuse path into a native prober of remote endpoints.
    async fn reprobe_missing_native_window(
        bridge: &Pinvou3Bridge,
        pending: &PendingNativeWindow,
    ) -> Option<u32> {
        if !base_url_uses_local_or_private(&bridge.base_url()) {
            return None;
        }
        let api_key = bridge.api_key();
        let kind = crate::core::model_endpoint::probe_local_server_kind(
            &bridge.base_url(),
            Some(api_key.as_str()),
        )
        .await;
        crate::core::model_endpoint::fetch_native_served_context(
            Some(kind),
            &bridge.base_url(),
            Some(api_key.as_str()),
            &pending.model_name,
        )
        .await
        .unwrap_or(None)
    }

    /// The reuse-path self-heal decision (see `EngineEntry::
    /// native_window_pending`): no pending marker → keep without probing;
    /// pending but the native API still serves no fact → keep (the engine
    /// stays, the next turn re-asks); pending and the fact materialized →
    /// rebuild so the fact is adopted. Extracted from `get_or_spawn_with_policy`
    /// verbatim so the decision — dropping it would let the collapsed
    /// 8192-fallback budget survive the engine's whole lifetime — is pinned
    /// by `pending_window_reuse_decision_rebuilds_once_fact_materializes`.
    async fn cached_entry_reuse_decision(
        pending: Option<&PendingNativeWindow>,
        bridge: &Pinvou3Bridge,
    ) -> CachedEntryReuse {
        match pending {
            None => CachedEntryReuse::Keep,
            Some(pending) => match Self::reprobe_missing_native_window(bridge, pending).await {
                Some(_) => CachedEntryReuse::RebuildForWindowFact,
                None => CachedEntryReuse::Keep,
            },
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
        let acquisition_started = Instant::now();
        let runtime_lock_started = Instant::now();
        let runtime_lock = self.runtime_model_locks.for_session(session_id).await;
        let _runtime = runtime_lock.lock().await;
        let runtime_lock_ms = elapsed_ms(runtime_lock_started);
        let prepare_model_started = Instant::now();
        let (bridge, prepared, pins_scheduled_model) = self
            .prepare_runtime_model(session_id, scheduled_unattended, explicit_model_override)
            .await?;
        let prepare_model_ms = elapsed_ms(prepare_model_started);
        let model_update_revision = self.model_update_revisions.current(&prepared.model.id);
        let prepared = PreparedRuntimeState::new(prepared, model_update_revision);
        let mcp_config_revision = self.mcp_config_revision.load(Ordering::Acquire);

        let (fresh_engine, pending_window, stale) = {
            let mut entries = self.entries.lock().await;
            if let Some(entry) = entries.get(session_id) {
                if entry_is_fresh(
                    prepared.requires_rebuild_from(&entry.runtime_model),
                    entry.mcp_config_revision,
                    mcp_config_revision,
                ) {
                    (
                        Some(entry.engine.clone()),
                        entry.native_window_pending.clone(),
                        None,
                    )
                } else {
                    (None, None, entries.remove(session_id))
                }
            } else {
                (None, None, None)
            }
        };
        let mut self_healed = false;
        if let Some(engine) = fresh_engine {
            // Missing-native-window re-adoption (see
            // `EngineEntry::native_window_pending`): while the spawn ended
            // without a served-window fact — the shape of a model that was
            // never JIT-loaded yet — every reuse re-classifies the endpoint
            // (cache-peeked, so the 7-probe battery re-runs at most once per
            // 60s TTL) and re-asks the selected native API once (the
            // `/api/show` leg is 60s-cached in core, `/api/ps` /
            // `/api/v0/models` are one small local GET each). Worst case
            // while pending is ~9s inline on this send path — a cold kind
            // cache against a hung-but-accepting server (3s battery + 3s ps
            // timeout +, when ps answered Ollama-shaped-but-missing, an
            // uncached 3s show; `Unreachable` is never cached); with the
            // kind cached this is ~6s. The re-check ends
            // once the fact materializes — and only then, not at the idle
            // reap: an entry in active use never goes idle (every send
            // refreshes `last_active`), so on a shape that can never prove
            // a fact (the model unloads between turns on a keep_alive=0
            // server, or a pre-0.6 Ollama whose `/api/ps` carries no
            // `context_length`) the re-check rides each send for the
            // engine's lifetime until an unrelated rebuild drops the
            // marker (disclosed residual; healthy servers pay ~ms). The
            // first post-load
            // check finds the real window and the entry is dropped for the
            // rebuild below, which re-finalizes with the fact adopted —
            // instead of the collapsed 8192-fallback budget surviving the
            // whole first engine lifetime (2026-09-30 user report). Runs
            // outside the entries lock.
            if matches!(
                Self::cached_entry_reuse_decision(pending_window.as_ref(), &bridge).await,
                CachedEntryReuse::Keep
            ) {
                crate::features::assistant::timing::record_engine_ready(
                    session_id,
                    crate::features::assistant::timing::EngineAcquireTiming {
                        kind: "reused",
                        total_ms: elapsed_ms(acquisition_started),
                        runtime_lock_ms,
                        prepare_model_ms,
                        reclaim_ms: 0,
                        finalize_bridge_ms: 0,
                        tool_setup_ms: 0,
                        materialize_skills_ms: 0,
                        spawn_engine_ms: 0,
                        load_session_ms: 0,
                        sync_session_ms: 0,
                    },
                );
                return Ok(engine);
            }
            // The model loaded since this engine spawned: reclaim and fall
            // through to the rebuild path (mirrors the revision-mismatch
            // reclaim; a mid-session rebuild is the same path a model edit
            // takes, including history hydration).
            self_healed = true;
            let removed = self.entries.lock().await.remove(session_id);
            if let Some(entry) = removed {
                self.reclaim_engine_entry(session_id, entry).await;
            }
        }
        // A pending-window self-heal drops the entry inline above (so
        // `stale` is None) but is a mid-session rebuild, not a cold spawn.
        let acquire_kind = if stale.is_some() || self_healed {
            "rebuilt"
        } else {
            "cold"
        };
        let reclaim_started = Instant::now();
        if let Some(entry) = stale {
            self.reclaim_engine_entry(session_id, entry).await;
        }
        let reclaim_ms = elapsed_ms(reclaim_started);

        let is_scheduled = self.store.scheduled_profile(session_id).is_some();
        let finalize_bridge_started = Instant::now();
        let bridge =
            Self::finalize_runtime_bridge(bridge, &prepared.prepared, pins_scheduled_model).await;
        let finalize_bridge_ms = elapsed_ms(finalize_bridge_started);
        // Pending re-adoption marker for engine reuse (see
        // `EngineEntry::native_window_pending`): armed by the adoption inside
        // finalize only when this route's native fetch actually ran and
        // served no fact — adopt-less routes (`adopts` false / not
        // operator-owned) and declared routes never arm, so a served fact
        // that can never be adopted cannot trigger a rebuild on every send.
        let native_window_pending = PendingNativeWindow::from_finalized(&bridge);
        let tool_setup_started = Instant::now();
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
        let tool_setup_ms = elapsed_ms(tool_setup_started);
        // Skill dual-scope governance: compose the composed directory fully
        // at spawn (materialization opportunity one, V-7). The composed
        // directory is the discovery root of EngineConfig.skills_dir (the
        // path injected by build_engine_config_for_session_roots) and must
        // exist before spawn, otherwise the first turn's prompt has no
        // `## Skills` block.
        let materialize_skills_started = Instant::now();
        // Aux sessions are isolated pure-Q&A engines (zero tools, minimal
        // instructions): they get no skill surface, so no composed directory
        // is materialized — the send path and the toggle hot refresh skip aux
        // on the same rule (round-31 M8). The #608 timing bracket stays
        // outside the guard: for aux it simply reports a near-zero span.
        if !crate::features::sessions::is_aux_session_id(session_id) {
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
        let materialize_skills_ms = elapsed_ms(materialize_skills_started);
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
        let spawn_engine_started = Instant::now();
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
        let spawn_engine_ms = elapsed_ms(spawn_engine_started);

        // Sync is mandatory even when messages is empty: SyncSession not only
        // injects history but also aligns the underlying Engine's internal
        // session id to the pre-created persisted session. Skipping it would
        // get the first turn's SessionUpdated rejected on an id mismatch,
        // ending with only the user message durable and the assistant reply
        // lost.
        let load_session_started = Instant::now();
        let loaded_session = self.store.load(session_id);
        let load_session_ms = elapsed_ms(load_session_started);
        let sync_session_started = Instant::now();
        match loaded_session {
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
        let sync_session_ms = elapsed_ms(sync_session_started);

        self.entries.lock().await.insert(
            session_id.to_string(),
            EngineEntry {
                engine: engine.clone(),
                forwarder,
                runtime_model: prepared,
                mcp_config_revision,
                native_window_pending,
                spawned_at_ms,
                steer_incarnation,
                last_active_epoch_ms: AtomicU64::new(Self::now_epoch_ms()),
            },
        );
        crate::features::assistant::timing::record_engine_ready(
            session_id,
            crate::features::assistant::timing::EngineAcquireTiming {
                kind: acquire_kind,
                total_ms: elapsed_ms(acquisition_started),
                runtime_lock_ms,
                prepare_model_ms,
                reclaim_ms,
                finalize_bridge_ms,
                tool_setup_ms,
                materialize_skills_ms,
                spawn_engine_ms,
                load_session_ms,
                sync_session_ms,
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

    /// Teardown-path reclaim with a bounded wait for the turn gate. The
    /// headless run has already finished (or hit its deadline) and must
    /// produce its report, so an admitted turn it does not know about must
    /// not block the process for up to the turn's wall clock — the same
    /// post-deadline bounding philosophy as the cancel settle window. On
    /// timeout the engine stays (the turn keeps running; the one-shot
    /// process exits right after) and the caller surfaces the skip.
    /// Returns `false` when the gate was not acquired within `wait`.
    pub(crate) async fn evict_bounded(&self, session_id: &str, wait: Duration) -> bool {
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let Ok(_turn) = tokio::time::timeout(wait, turn_lock.lock()).await else {
            return false;
        };
        self.evict_locked(session_id).await;
        true
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
    /// session was busy at recheck, when its turn gate could not be
    /// acquired within [`REBIND_EVICT_GATE_TIMEOUT`], or when the runtime
    /// lock itself timed out (the bounded-take third arm) — in every case
    /// nothing was touched and the command reports the session as post-busy.
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
    ///
    /// Aux-aware: deleting a main chat first deletes its aux session through
    /// this same gated path (depth 1 — aux sessions never own another aux), so
    /// callers that bypass the command-layer cascade (`delete_session`) still
    /// reclaim the aux engine instead of orphaning it. Never substitute a bare
    /// `store.delete` for this method on a chat session.
    pub(crate) async fn delete_chat_session(&self, session_id: &str) -> Result<()> {
        let deleted = delete_chat_session_with_aux_cascade(&self.store, session_id, |id| {
            let id = id.to_string();
            async move {
                delete_chat_session_with_gate(
                    &self.turn_locks,
                    &self.store,
                    &id,
                    DeleteGateRecheck::Unconditional,
                    || self.evict_locked(&id),
                    || self.forget_session(&id),
                )
                .await
            }
        })
        .await?;
        // The bare `agent` is available to **all** sessions (not only those
        // with the multi-agent toggle on), and the background ledger write
        // after the foundation cancels sub-agents (write_json_atomic recreates
        // the parent directory) can resurrect the just-deleted sessions/<id>/.
        // A missing directory is the common case and costs nothing; once
        // Shutdown is processed no new writes happen, so it always converges.
        // Aux sessions skip the sweep (round-30 B8): their id is derived
        // (`aux-{parent_id}`), so a reset (reopen-topic) recreate within the
        // 2s/6s delay window reuses the same directory the stale sweep would
        // remove — and the sweep's premise is structurally false for aux
        // anyway (zero tools ⇒ no subagents, no shell, no background ledger
        // writer that could resurrect the directory). The `deleted` leg
        // (#504) also keeps a keep-disposition (stub cleanup) from sweeping.
        if deleted && !crate::features::sessions::is_aux_session_id(session_id) {
            Self::schedule_late_sweep(
                crate::platform::paths::sessions_root().join(session_id),
                "late sweep of deleted chat",
            );
        }
        Ok(())
    }

    /// Headless stub-cleanup delete: like [`Self::delete_chat_session`], but
    /// the durable delete is guarded — under the turn gate a record that
    /// carries messages (or whose state is unloadable) is a started
    /// transcript, not a stub, and is kept with only the engine reclaimed
    /// ([`DeleteGateRecheck::StillAStub`]).
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) async fn delete_chat_session_if_still_empty(&self, session_id: &str) -> Result<()> {
        let deleted = delete_chat_session_with_gate(
            &self.turn_locks,
            &self.store,
            session_id,
            DeleteGateRecheck::StillAStub,
            || self.evict_locked(session_id),
            || self.forget_session(session_id),
        )
        .await;
        let deleted = match deleted {
            Ok(deleted) => deleted,
            // Same failed-delete backstop as `delete_eval_session`: the
            // in-memory entry must not linger on a failed delete, and the
            // late sweep retries the disk cleanup. The sweep is
            // adoption-aware, same as the one-shot twin: the record
            // SURVIVED the failed delete, so a GUI rename landing inside
            // the sweep window turns the directory into a live session's
            // workspace that must not be destroyed.
            Err(error) => {
                self.forget_session(session_id);
                Self::schedule_late_sweep_unless_adopted(
                    crate::platform::paths::sessions_root().join(session_id),
                    "late sweep of failed stub cleanup",
                    self.store.clone(),
                    session_id.to_string(),
                );
                return Err(error);
            }
        };
        if deleted {
            Self::schedule_late_sweep(
                crate::platform::paths::sessions_root().join(session_id),
                "late sweep of deleted stub",
            );
        }
        Ok(())
    }

    /// Atomically reset a task's auxiliary conversation (M6): delete the
    /// existing aux session through the same turn gate as
    /// `discard_aux_session` / chat delete, then create a fresh one and
    /// return its metadata together with the deleted aux id (so the caller
    /// can emit `session:deleted`). One primitive, one outcome — the
    /// frontend's old two-invoke restart (discard, then ensure) had no
    /// server-side ordering, and under the web relay's non-FIFO premise an
    /// orphaned discard could execute after the recreate and destroy the
    /// fresh transcript.
    ///
    /// Critical section: the aux session's turn gate, held across engine
    /// reclaim + disk delete (inside `delete_chat_session_with_gate`). The
    /// create half deliberately runs outside the gate:
    /// `SessionStore::get_or_create_aux_session` is commutative — concurrent
    /// creators converge on the same derived id with the same content
    /// (store.rs documents why no creation lock is needed) — so a concurrent
    /// `ensure` landing between the two halves yields the same end state as
    /// any serialized order: exactly one fresh aux session. Deadlock-freedom:
    /// the only graph lock this method acquires is the aux id's turn_lock
    /// (taken and released inside the delete half); the create half takes no
    /// turn lock, and the store-internal locks it touches are leaf locks
    /// downstream of `turn_lock` in the documented order (turn_lock → pool →
    /// scheduled_mutation → sidecar writes). A concurrent `ensure` takes no
    /// locks at all, and a concurrent `discard` contends for the same single
    /// turn_lock without holding a second lock, so no wait-cycle can form.
    pub(crate) async fn reset_aux_chat_session(
        &self,
        main_id: &str,
    ) -> Result<(
        Option<String>,
        Result<deepseek_tui::session_manager::SessionMetadata>,
    )> {
        // The outer Result covers the delete half only: its failure means
        // nothing committed and there is nothing to report. The inner one
        // carries the create half, whose failure must still let the caller
        // emit session:deleted for the committed delete (see below).
        let deleted_aux = reset_aux_session_delete_with_gate(
            &self.turn_locks,
            &self.store,
            main_id,
            |aux_id| async move {
                self.evict_locked(&aux_id).await;
            },
            |aux_id| self.forget_session(aux_id),
        )
        .await?;
        // The create result rides next to `deleted_aux` instead of behind
        // one outer Result (round-32 review minor 8): when the delete half
        // committed and the create half then failed (e.g. a concurrent
        // parent deletion), the aux record is durably gone and the command
        // must still emit session:deleted — gating the event on overall
        // success left clients buffering a stale transcript for a session
        // that no longer exists.
        let created = self.store.get_or_create_aux_session(main_id);
        Ok((deleted_aux, created))
    }

    /// One-shot falsy delete for the teardown arms: like
    /// [`Self::delete_chat_session_if_still_empty`], the durable delete is
    /// guarded under the turn gate — the disposition sampled the adoption
    /// marker outside the gate, and a live turn can hold that gate for the
    /// turn's whole wall clock, so a rename landing during the wait makes
    /// the record an adopted (GUI-owned) session that must be kept. The
    /// late sweep only fires on the delete outcome (or the failed-delete
    /// backstop), never on the keep outcome.
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) async fn delete_chat_session_unless_adopted(&self, session_id: &str) -> Result<()> {
        let deleted = delete_chat_session_with_gate(
            &self.turn_locks,
            &self.store,
            session_id,
            DeleteGateRecheck::StillFactoryTitled,
            || self.evict_locked(session_id),
            || self.forget_session(session_id),
        )
        .await;
        let deleted = match deleted {
            Ok(deleted) => deleted,
            // Same failed-delete backstop as the stub twin: the in-memory
            // entry must not linger on a failed delete, and the late sweep
            // retries the disk cleanup. The sweep is adoption-aware: the
            // record SURVIVED the failed delete, so a GUI rename landing
            // inside the sweep window turns the directory into a live
            // session's workspace that must not be destroyed.
            Err(error) => {
                self.forget_session(session_id);
                Self::schedule_late_sweep_unless_adopted(
                    crate::platform::paths::sessions_root().join(session_id),
                    "late sweep of failed one-shot delete",
                    self.store.clone(),
                    session_id.to_string(),
                );
                return Err(error);
            }
        };
        if deleted {
            Self::schedule_late_sweep(
                crate::platform::paths::sessions_root().join(session_id),
                "late sweep of deleted one-shot session",
            );
        }
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

    /// Adoption guard for the failed-delete backstop sweep: the record
    /// survived the failed delete, so sweep the directory only while it is
    /// still ours — an absent record means the delete converged (the sweep
    /// then cleans the resurrected-orphan directory as before), a record
    /// still reading factory-titled means the session was never adopted,
    /// and a renamed (adopted) or unreadable record keeps the directory
    /// (unknown keeps — the same rule the delete gate applies).
    #[cfg(any(feature = "benchmark-hooks", test))]
    fn sweep_may_remove_session_dir(store: &SessionStore, session_id: &str) -> bool {
        !store.chat_session_record_exists(session_id) || record_is_factory_titled(store, session_id)
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

    /// [`Self::schedule_late_sweep`] for the failed-delete backstop, gated on
    /// the adoption state at each attempt: a GUI adoption landing inside the
    /// sweep window must not lose the session directory under its surviving
    /// record.
    #[cfg(any(feature = "benchmark-hooks", test))]
    fn schedule_late_sweep_unless_adopted(
        dir: std::path::PathBuf,
        label: &'static str,
        store: SessionStore,
        session_id: String,
    ) {
        tauri::async_runtime::spawn(async move {
            for delay_ms in [2000u64, 6000] {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                if !Self::sweep_may_remove_session_dir(&store, &session_id) {
                    continue;
                }
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

    /// Resolve and privately pin the complete SavedModel while returning only a
    /// non-sensitive opaque selection to the evaluation layer. Callers that do
    /// not pass the selection to `prepare_eval_session` must explicitly discard it.
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) fn pin_eval_model_selection(&self, model_id: &str) -> Result<EvalModelSelection> {
        let prefs = UserPrefs::load();
        let (saved, identity) = resolve_eval_model_selection_from(
            &self.bridge,
            &prefs.advanced.saved_models,
            model_id,
        )?;
        Ok(self.eval_model_snapshots.pin(saved, identity))
    }

    /// Creates and loads a one-off eval session. The eval runner decides the
    /// session ID in advance so reports and cleanup can be correlated exactly;
    /// ordinary GUI sessions keep using the SessionStore-generated ID.
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) async fn prepare_eval_session(
        &self,
        session_id: &str,
        model_selection: Option<&EvalModelSelection>,
        workspace: Option<&std::path::Path>,
    ) -> Result<()> {
        // The caller's task directory (when provided) lands in the session's
        // `metadata.workspace` so the GUI list/detail shows the directory the
        // session actually works in; the durable binding sidecar is written
        // separately by the caller.
        let metadata_workspace = workspace.map(std::path::Path::to_path_buf);
        match model_selection {
            None => {
                let (model, model_id) = self.default_model_for_new_session();
                self.store.create_empty_with_id(
                    session_id.to_string(),
                    model,
                    model_id,
                    metadata_workspace
                        .clone()
                        .unwrap_or_else(|| self.bridge.workspace.clone()),
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
                    metadata_workspace.unwrap_or_else(|| self.bridge.workspace.clone()),
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
        // 基准轮与生产发送走同一匹配通道：这里 `content` 就是用户原文，
        // 与生产 match_source 同源。传空候选会让模型在真实有匹配时收到
        // 「无相关候选」的假话（候选行是快照的唯一下游）。评测策略轮
        // 刻意不带专家材料，见 `send_eval_user_message`。
        let expert_candidates = expert_snapshot
            .as_ref()
            .map(|snapshot| snapshot.available_role_lines(&content))
            .unwrap_or_default();
        self.send_reserved_user_message(
            session_id,
            content,
            display_message,
            mode,
            restrict_tools_for_turn,
            expert_snapshot,
            expert_candidates,
            reservation,
        )
        .await
    }

    /// 评测策略轮刻意不带专家快照/候选：评测度量的是被测策略本身，候选
    /// 匹配属产品行为，混入会让评测结果与产品行为互相污染。
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
    /// `expert_candidates` must come from the same
    /// [`ExpertRosterSnapshot::capture`] as `expert_snapshot`
    /// (prepare_delegation_turn)，bridge 把它放进 `<system-reminder>` 信封。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn send_reserved_user_message(
        &self,
        session_id: &str,
        content: String,
        display_message: Message,
        mode: AppMode,
        restrict_tools_for_turn: bool,
        expert_snapshot: Option<std::sync::Arc<ExpertRosterSnapshot>>,
        expert_candidates: Vec<String>,
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
        if scheduled_profile.is_none() && crate::features::sessions::is_sched_session_id(session_id)
        {
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
        let persona_conversational = active_card.as_ref().is_some_and(|c| c.conversational_only);
        let engine = self.get_or_spawn(session_id).await?;
        forward_forced_turn_restrict(
            session_id,
            persona_conversational,
            restrict_tools_for_turn,
            persona_reminder,
            |restrict_tools, reminder| {
                engine.send_reserved_user_message(
                    content,
                    mode,
                    reminder,
                    restrict_tools,
                    expert_snapshot,
                    expert_candidates,
                    reservation,
                )
            },
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
            crate::features::assistant::timing::start_turn(session_id);
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
        // Reclaim first: if submission succeeded but the scheduler callback or
        // terminal wait failed, the forwarder owns the authoritative terminal
        // and consumes this timing turn while eviction cancels it. The calls
        // below are idempotent fallbacks for failures before submission and
        // cancellation before send.
        match &result {
            Err(error) => crate::features::assistant::timing::finish_turn(
                session_id,
                "send_error",
                Some(&format!("{error:#}")),
            ),
            Ok(completion) if completion.turn_id.is_empty() => {
                crate::features::assistant::timing::finish_turn(session_id, "Interrupted", None);
            }
            Ok(_) => {}
        }
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
        if scheduled_profile.is_none() && crate::features::sessions::is_sched_session_id(session_id)
        {
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
        // Aux zero-tool reminder for edit resends (round-14 minor-1): this
        // path bypasses send_reserved_user_message, so the reminder is merged
        // into the resent message by forward_edit_resend_with_reminder —
        // otherwise an aux edit-resend can regress to literal tool-call
        // markup in the answer. The block is stripped from the stored
        // context host-side — the reservation's TranscriptSanitizationRule
        // swaps the raw prompt for the display copy before the transcript is
        // persisted, same as on the send path. The tool *surface* stays zero
        // via the spawn config; persona anchors share the pre-existing gap
        // and are unchanged. Folding the merge into the dispatch seam keeps
        // the wiring itself pinned by an executing test (round-31 M9-rust).
        let engine = self.get_or_spawn(session_id).await?;
        forward_edit_resend_with_reminder(session_id, new_message, |message| {
            engine.edit_last_turn_reserved(message, reservation)
        })
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

// Snapshot resolution. Under `test` this backs the session-continuation
// tests; under `benchmark-hooks` it backs the production eval pin path in
// `pin_eval_model_selection` — that gate must match the caller's, or a plain
// `--features benchmark-hooks` build (no test cfg) fails to compile.
#[cfg(any(feature = "benchmark-hooks", test))]
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
        AUX_ZERO_TOOL_REMINDER, BoundedJoinOutcome, DeleteGateRecheck, EvalModelSnapshots,
        ModelIdentity, ModelUpdateRevisions, Op, Pinvou3Bridge, PreparedRuntimeState,
        REBIND_EVICT_GATE_TIMEOUT, SESSION_MODEL_BINDING_STALE_ERROR, ScheduledUnattendedGuard,
        SessionShellManagers, SessionTurnLifecycles, SessionTurnLocks, SessionTurnShellTasks,
        TURN_GATE_AWAIT_TIMEOUT, TranscriptOperation, TurnIdentity,
        bounded_join_while_holding_turn_gate, bounded_shutdown_sends, cancel_turn_with_gates,
        default_model_for_new_session_from, delete_chat_session_with_aux_cascade,
        delete_chat_session_with_gate, delete_scheduled_run_with_gate, delete_then_forget,
        dispatch_turn_bound_cancel, entry_is_fresh, evict_if_idle_with_gates,
        forward_edit_resend_with_reminder, forward_forced_turn_restrict, generation_matches,
        identity_for_active_model, identity_for_saved_model, merge_aux_zero_tool_reminder,
        quiesce_engine_before_reclaim, rebind_evict_with_gates, rebind_evictable,
        reset_aux_session_delete_with_gate, resolve_eval_model_selection_from,
        resolve_runtime_model_override, resolve_scheduled_model, resolve_spawn_model,
        retry_shutdown_sends, scheduled_profile_after_turn_gate, should_still_reap_after_snapshot,
        turn_restrict_tools, user_display_message,
    };
    use crate::features::assistant::engine::TurnBoundCancelOps;
    use crate::features::assistant::runtime_model::PreparedRuntimeModel;
    use crate::features::sessions::{ScheduledRunMode, ScheduledRunProfile, SessionStore};
    use crate::platform::credential_store::{CredentialEditAction, CredentialState};
    use crate::platform::paths::tests::ENV_LOCK;
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

    /// Round-32 review minor 8: the reset's delete half can commit and the
    /// create half then fail (a concurrent parent deletion between the
    /// command layer's existence check and the create leg). The gate must
    /// still report the deleted aux id, and the store's create leg must
    /// error — the exact `(Some, Err)` shape the command layer needs to
    /// emit `session:deleted` for the committed delete before failing.
    #[tokio::test]
    async fn reset_reports_the_deleted_aux_when_the_create_half_fails() {
        let guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-reset-create-fault-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        // SAFETY: the test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled"))
            .expect("boot isolated store");
        let main = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create main");
        let aux = store
            .create_aux_session(&main.metadata.id)
            .expect("create aux");
        // The concurrent parent deletion: the main record leaves the disk
        // after the command layer checked it, before the create leg runs.
        std::fs::remove_file(
            crate::platform::paths::sessions_root().join(format!("{}.json", main.metadata.id)),
        )
        .expect("remove the parent record out of band");

        let turn_locks = super::SessionTurnLocks::default();
        let deleted = reset_aux_session_delete_with_gate(
            &turn_locks,
            &store,
            &main.metadata.id,
            |_aux_id| async move {},
            |_aux_id| {},
        )
        .await
        .expect("the delete half commits: the gate does not require the parent");
        assert_eq!(
            deleted.as_deref(),
            Some(aux.id.as_str()),
            "the gate reports the committed delete's aux id"
        );
        let created = store.get_or_create_aux_session(&main.metadata.id);
        assert!(
            created.is_err(),
            "the create half must fail: the parent record is gone"
        );
        assert!(
            store.durable_session_record_is_absent(&aux.id),
            "the old aux record is durably gone once the delete half committed"
        );
        drop(guard);
    }

    /// PR #433 review (MAJOR): `restrict_tools` is only an optional call
    /// parameter of `chat` / `web_access_chat`, so an aux session's tool
    /// restriction cannot rely on caller discipline — the `aux-` prefix
    /// forces zero tools at the pool send chokepoint, and a caller passing
    /// false is restricted all the same; ordinary sessions are unchanged.
    #[test]
    fn aux_session_turn_is_tool_free_regardless_of_caller() {
        // aux session: caller passes false / true, with or without a meta
        // card — always zero tools.
        assert!(turn_restrict_tools("aux-1", false, false));
        assert!(turn_restrict_tools("aux-1", false, true));
        assert!(turn_restrict_tools("aux-1", true, false));
        // ordinary session: keep the existing semantics (restricted only by
        // a caller per-turn request / a pure-conversation meta card).
        assert!(!turn_restrict_tools("sess-plain", false, false));
        assert!(turn_restrict_tools("sess-plain", false, true));
        assert!(turn_restrict_tools("sess-plain", true, false));
        // sched- and other prefixed sessions do not take the aux rule.
        assert!(!turn_restrict_tools("sched-1", false, false));
    }

    /// The zero-tool table alone does not stop tool-trained models from
    /// emitting their native tool-call markup as answer text (live repro: a
    /// DeepSeek aux turn answered with a literal DSML invoke block). Aux
    /// turns must therefore carry the zero-tool boundary in the per-turn
    /// reminder, merged after any persona anchor; non-aux sessions must not
    /// gain a reminder they never had.
    #[test]
    fn aux_turn_carries_zero_tool_boundary_reminder() {
        // Plain session: reminder untouched (None stays None, persona text
        // passes through verbatim — no aux boundary appended).
        assert_eq!(merge_aux_zero_tool_reminder("sess-plain", None), None);
        let persona = "persona anchor".to_string();
        assert_eq!(
            merge_aux_zero_tool_reminder("sess-plain", Some(persona.clone())),
            Some(persona)
        );
        // Aux session without a persona: boundary alone.
        assert_eq!(
            merge_aux_zero_tool_reminder("aux-1", None),
            Some(AUX_ZERO_TOOL_REMINDER.to_string())
        );
        // Aux session with a persona: anchor first, boundary second.
        let merged =
            merge_aux_zero_tool_reminder("aux-1", Some("persona anchor".to_string())).unwrap();
        assert!(merged.starts_with("persona anchor\n\n"));
        assert!(merged.ends_with(AUX_ZERO_TOOL_REMINDER));
        // Case-insensitive aux prefix, same as the tool gate.
        assert!(merge_aux_zero_tool_reminder("AUX-1", None).is_some());
        assert!(merge_aux_zero_tool_reminder("sched-1", None).is_none());
    }

    /// PR #433 review round-8 (M-1): the is-aux decision is a prefix test on
    /// the client-supplied id string, but id validation allows uppercase and
    /// ids resolve to files without case canonicalization — on
    /// case-insensitive filesystems an `AUX-<suffix>` alias loads the real
    /// aux record. The gate must therefore be case-insensitive, or the alias
    /// runs a full-tool turn over the aux session.
    #[test]
    fn aux_tool_gate_is_case_insensitive_against_id_aliases() {
        assert!(turn_restrict_tools("AUX-1", false, false));
        assert!(turn_restrict_tools("Aux-1", false, false));
        assert!(turn_restrict_tools("aUx-1", false, false));
        // A normal id can never collide: the generator emits lowercase
        // base36, and a non-prefixed id stays caller-driven either way.
        assert!(!turn_restrict_tools("SESS-plain", false, false));
        assert!(!turn_restrict_tools("AUXILIARY-1", false, false));
        // Round-9 MAJOR-1: multibyte ids must not panic the prefix helpers
        // (byte slicing at a non-char boundary panics; these guards run on
        // client-supplied ids before charset validation).
        assert!(!turn_restrict_tools("aux帮", false, false));
        assert!(!turn_restrict_tools("sched计划", false, false));
    }

    /// Round-9 MAJOR-1 regression pin: the case-insensitive helpers must be
    /// boundary-safe — byte slicing panics inside a multibyte char, and the
    /// guards run on client-supplied ids before any charset validation.
    #[test]
    fn aux_sched_prefix_helpers_never_panic_on_multibyte_ids() {
        use crate::features::sessions::{is_aux_session_id, is_sched_session_id};
        for id in ["aux帮", "sched计划", "au€", "sched-", "aux-é", "日", ""] {
            // The assertion is the absence of a panic; ASCII-correct results
            // are covered by the gate truth table above.
            let _ = is_aux_session_id(id);
            let _ = is_sched_session_id(id);
        }
        assert!(is_aux_session_id("AUX-1"));
        assert!(!is_aux_session_id("aux帮"));
        assert!(!is_sched_session_id("sched计划"));
    }

    /// PR #433 review round-6 (MAJOR) + round-10 (S2(b)): the "last mile" from
    /// decision to dispatch — `send_reserved_user_message` hands the forced
    /// result of `turn_restrict_tools` to the engine's per-turn send entry.
    /// Round-6 pinned the value with this capture closure but left the caller's
    /// `bool` an equally valid argument type, so passing it straight through
    /// kept every test green. The entry now takes
    /// [`TurnToolRestrict`](super::turn_tool_restrict::TurnToolRestrict), which
    /// only `forward_forced_turn_restrict` can mint: this test pins the value
    /// the engine reads, and the type makes "hand the caller's `bool` to the
    /// engine" a compile error instead of a silent regression.
    #[test]
    fn send_dispatch_forwards_forced_restrict_to_engine_entry() {
        let captured = std::cell::Cell::new(None);
        {
            let captured = &captured;
            forward_forced_turn_restrict(
                "aux-1",
                false,
                false,
                None,
                |turn_tool_restrict, _reminder| {
                    captured.set(Some(turn_tool_restrict));
                },
            );
        }
        let aux = captured
            .get()
            .expect("aux session must yield a restrict token");
        assert!(
            aux.restricts_tools(),
            "for an aux session the combined result must restrict (zero tools) even when the caller passes false"
        );
        assert!(
            aux.restricts_tools_for("aux-1"),
            "the per-turn restrict reaching the engine for an aux session must be true (zero tools)"
        );

        captured.set(None);
        {
            let captured = &captured;
            forward_forced_turn_restrict(
                "sess-plain",
                false,
                false,
                None,
                |turn_tool_restrict, _reminder| {
                    captured.set(Some(turn_tool_restrict));
                },
            );
        }
        let plain = captured.get().expect("a plain session must yield a token");
        assert!(
            !plain.restricts_tools(),
            "control: a plain session with no caller restriction and no meta card stays unrestricted in the combined result"
        );
        assert!(
            !plain.restricts_tools_for("sess-plain"),
            "control: a plain session with no caller restriction and no meta card keeps restrict=false at the engine"
        );

        captured.set(None);
        {
            let captured = &captured;
            forward_forced_turn_restrict(
                "sess-plain",
                false,
                true,
                None,
                |turn_tool_restrict, _reminder| {
                    captured.set(Some(turn_tool_restrict));
                },
            );
        }
        let caller_forced = captured
            .get()
            .expect("caller-forced restriction must yield a token");
        assert!(
            caller_forced.restricts_tools(),
            "control: a caller's per-turn restriction request is passed through as-is"
        );
        assert!(caller_forced.restricts_tools_for("sess-plain"));

        // A pure-conversation meta card forces the restriction on its own.
        captured.set(None);
        {
            let captured = &captured;
            forward_forced_turn_restrict(
                "sess-plain",
                true,
                false,
                None,
                |turn_tool_restrict, _reminder| {
                    captured.set(Some(turn_tool_restrict));
                },
            );
        }
        assert!(
            captured
                .get()
                .expect("a conversational-only meta card must yield a token")
                .restricts_tools(),
            "the combined result must restrict while a conversational-only meta card is in effect"
        );

        // Even a token minted for another session (wrong/empty id — the
        // headless harness uses "") cannot un-restrict the aux session: the
        // engine entry re-checks the aux prefix against its OWN session id.
        let mismatched =
            super::turn_tool_restrict::TurnToolRestrict::forced("sess-plain", false, false);
        assert!(
            !mismatched.restricts_tools(),
            "the token's own combined value comes from the session it was minted for and does not change with the engine id"
        );
        assert!(
            mismatched.restricts_tools_for("aux-1"),
            "the engine-side aux prefix re-check must catch tokens whose session id does not match"
        );
        assert!(
            !mismatched.restricts_tools_for(""),
            "an empty id (headless engine) is not an aux session; the token's own value applies"
        );
    }

    /// PR #433 review round-31 (M9-rust): the reminder leg of the send "last
    /// mile". The pure `merge_aux_zero_tool_reminder` helper had test
    /// coverage, but its CALL at the send dispatch did not — deleting the
    /// merge left the whole suite green while aux turns lost the zero-tool
    /// reminder and regressed to literal tool-call markup in the answer. The
    /// merge now lives inside `forward_forced_turn_restrict`, the only path
    /// to the engine's per-turn send entry: this capture closure pins the
    /// reminder the engine actually receives, for aux and non-aux ids.
    #[test]
    fn send_dispatch_merges_aux_zero_tool_reminder_into_outgoing_reminder() {
        // Aux id: the outgoing reminder carries the persona anchor first and
        // the zero-tool boundary after it.
        let reminder = forward_forced_turn_restrict(
            "aux-reminder-wire",
            false,
            false,
            Some("persona anchor".to_string()),
            |_token, reminder| reminder,
        )
        .expect("an aux turn must always carry a reminder");
        assert!(
            reminder.starts_with("persona anchor\n\n"),
            "the persona anchor keeps its lead position: {reminder}"
        );
        assert!(
            reminder.contains(AUX_ZERO_TOOL_REMINDER),
            "the zero-tool reminder must reach the outgoing aux turn"
        );

        // Aux id without a persona anchor: the reminder is the boundary alone.
        let reminder =
            forward_forced_turn_restrict("aux-reminder-wire", false, false, None, |_t, r| r);
        assert_eq!(
            reminder.as_deref(),
            Some(AUX_ZERO_TOOL_REMINDER),
            "with no persona anchor the aux reminder is the zero-tool boundary itself"
        );

        // Control: a plain session's reminder passes through untouched —
        // no zero-tool block is ever attached to a normal turn.
        let reminder = forward_forced_turn_restrict(
            "sess-plain",
            false,
            false,
            Some("persona anchor".to_string()),
            |_t, r| r,
        );
        assert_eq!(reminder.as_deref(), Some("persona anchor"));
        let reminder = forward_forced_turn_restrict("sess-plain", false, false, None, |_t, r| r);
        assert!(
            reminder.is_none(),
            "a plain session with no persona anchor sends no reminder"
        );
    }

    /// PR #433 review round-31 (M9-rust): the second call site.
    /// `edit_last_turn_reserved` bypasses the send path, so its reminder is
    /// merged into the resent message — a call that was likewise unpinned
    /// (deleting it kept the suite green). The merge now lives inside
    /// `forward_edit_resend_with_reminder`, the only path from the pool's
    /// edit-resend entry to the engine: this capture closure pins the exact
    /// message the engine receives, for aux and non-aux ids.
    #[test]
    fn edit_resend_dispatch_merges_aux_zero_tool_reminder_into_outgoing_message() {
        let message = forward_edit_resend_with_reminder(
            "aux-reminder-wire",
            "edited question".to_string(),
            |message| message,
        );
        assert!(
            message.starts_with("<system-reminder>\n"),
            "the aux edit resend must carry the reminder block: {message}"
        );
        assert!(
            message.contains(AUX_ZERO_TOOL_REMINDER),
            "the zero-tool reminder must reach the outgoing aux edit resend"
        );
        assert!(
            message.ends_with("\n</system-reminder>\n\nedited question"),
            "the user text rides after the reminder block: {message}"
        );

        // Control: a plain session's resent message is forwarded verbatim.
        let message = forward_edit_resend_with_reminder(
            "sess-plain",
            "edited question".to_string(),
            |message| message,
        );
        assert_eq!(message, "edited question");
    }

    /// The spawn funnel installs the session id as the OpenCode gateway
    /// affinity key, so every respawn of one session reuses the same
    /// `x-opencode-session` value instead of silently falling back to the
    /// shared `engine-default` conversation.
    #[test]
    fn install_session_affinity_key_pins_session_id_on_bridge() {
        let mut bridge = Pinvou3Bridge::test_fixture(None);
        assert!(
            bridge.session_affinity_key.is_none(),
            "fixture bridges start without a session affinity key"
        );
        super::EnginePool::install_session_affinity_key(&mut bridge, "session-a");
        assert_eq!(bridge.session_affinity_key.as_deref(), Some("session-a"));
        let mut respawned = Pinvou3Bridge::test_fixture(None);
        super::EnginePool::install_session_affinity_key(&mut respawned, "session-a");
        assert_eq!(
            respawned.session_affinity_key, bridge.session_affinity_key,
            "a respawned bridge for the same session must reuse the same key"
        );
    }

    /// Drives the real spawn-funnel preparation body (the same code
    /// `prepare_runtime_model` runs for every spawn/respawn) and asserts the
    /// produced bridge carries the session id as the gateway affinity key.
    /// Without the latch this fails: an unlatched bridge keys every gateway
    /// request onto the shared default conversation.
    #[tokio::test]
    async fn prepare_runtime_model_keys_bridge_by_session_id() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = EnvRestore::capture(&["PINVOU3_HOME"]);
        let home = std::env::temp_dir().join(format!(
            "pinvou3-runtime-model-affinity-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };

        let store = SessionStore::boot().expect("session store");
        let model = SavedModel {
            id: "affinity-wiring-model".into(),
            name: "Affinity Wiring".into(),
            alias: None,
            preset: ModelPreset::OpenaiCompatible,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: "affinity-model".into(),
            base_url: String::new(),
            provider_kind: Some("custom".into()),
            vendor: None,
            endpoint_mode: None,
            image_capability_override: ImageCapabilityOverride::default(),
            vision_model_id: None,
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        };
        let bridge = Pinvou3Bridge::test_fixture(Some(model));
        let (prepared, _prepared_model, _pins) = super::EnginePool::prepare_runtime_model_with(
            &store,
            bridge,
            "session-affinity-under-test",
            false,
            None,
        )
        .await
        .expect("runtime model preparation succeeds");
        assert_eq!(
            prepared.session_affinity_key.as_deref(),
            Some("session-affinity-under-test"),
            "every spawned bridge must be keyed by its session id for the gateway affinity header"
        );
        let _ = std::fs::remove_dir_all(&home);
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

    #[tokio::test(start_paused = true)]
    async fn rebind_eviction_runtime_lock_timeout_leaves_session_untouched() {
        // Round-24 minor 6 (the previously untested sibling arm): the
        // runtime lock is held across a cold spawn for many seconds — far
        // beyond any turn-gate wait — so it carries its own bounded
        // timeout. A runtime lock held past REBIND_EVICT_GATE_TIMEOUT must
        // give the eviction up exactly like the turn-gate arm: not idle
        // (post-busy), no take, no reclaim, no shell-state reset.
        let turn_locks = SessionTurnLocks::default();
        let runtime_locks = SessionTurnLocks::default();
        let shell_managers = SessionShellManagers::default();
        let turn_shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-rebind-evict-runtime-timeout";
        let manager = shell_managers.for_session(sid, PathBuf::from("D:/old-root"));
        turn_shell_tasks.for_session(sid, manager);

        // The turn gate is free (the eviction acquires it immediately); a
        // cold spawn holds the runtime lock for its whole duration.
        let runtime = runtime_locks.for_session(sid).await;
        let _cold_spawn = runtime.lock().await;

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
            "a runtime lock held past the timeout counts as not idle (reported post-busy)"
        );
        assert!(
            started.elapsed() >= REBIND_EVICT_GATE_TIMEOUT,
            "the eviction waited the bounded timeout on the runtime lock"
        );
        assert!(
            !take_ran.load(Ordering::Acquire),
            "the take closure must not run without the runtime lock"
        );
        assert!(
            shell_managers.get(sid).is_some(),
            "shell state survives a runtime-lock timeout"
        );
        assert!(
            turn_shell_tasks.has_registry(sid),
            "turn-scope registry survives a runtime-lock timeout"
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

        // The other half of the predicate: switching to a different model at an
        // unchanged revision must rebuild too. Without this, dropping the model
        // comparison from requires_rebuild_from would leave the session running on
        // the previous model's engine and no test would notice.
        let other_model_state = PreparedRuntimeState::new(
            PreparedRuntimeModel::unchanged(model("model-2", "wire-model-2")),
            revisions.current("model-1"),
        );
        assert!(
            other_model_state.requires_rebuild_from(&next_turn_state),
            "a different runtime model must force a rebuild even at the same revision"
        );
        assert!(
            !next_turn_state.requires_rebuild_from(&next_turn_state.clone()),
            "an unchanged model at an unchanged revision must reuse the engine"
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

    /// PR #433 review round-13 (M-B): the pool-level chat delete must cascade
    /// to the aux session through the gated delete *first* (depth 1), so
    /// callers that bypass the command-layer cascade (eval close, web-session
    /// rollback) cannot leave a running aux engine as a handle-less orphan.
    /// Reordering the wrapper (main before aux) or dropping the aux leg must
    /// fail this test.
    #[tokio::test]
    async fn chat_delete_cascades_to_aux_before_main() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = std::env::temp_dir().join(format!(
            "pinvou3-engine-pool-aux-cascade-{}",
            std::process::id()
        ));
        let previous_home = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&home);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };

        let store = SessionStore::boot().expect("session store");
        let main = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create main");
        let aux = store
            .get_or_create_aux_session(&main.metadata.id)
            .expect("create aux");

        let deleted = Arc::new(StdMutex::new(Vec::new()));
        let recording_delete = |id: &str| {
            let id = id.to_string();
            let store = store.clone();
            let deleted = deleted.clone();
            async move {
                deleted.lock().unwrap().push(id.clone());
                store.delete(&id).map(|_| true)
            }
        };
        delete_chat_session_with_aux_cascade(&store, &main.metadata.id, recording_delete)
            .await
            .expect("cascaded delete");

        assert_eq!(
            deleted.lock().unwrap().as_slice(),
            &[aux.id.clone(), main.metadata.id.clone()],
            "the aux session must be deleted strictly before its main session"
        );
        assert!(store.load(&aux.id).is_err());
        assert!(store.load(&main.metadata.id).is_err());

        // Depth 1: deleting an aux session directly must not look up a mapping
        // (aux sessions never own another aux) — a single delete, no cascade.
        let main_b = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create main B");
        let aux_b = store
            .get_or_create_aux_session(&main_b.metadata.id)
            .expect("create aux B");
        let deleted_b = Arc::new(StdMutex::new(Vec::new()));
        let recording_delete_b = |id: &str| {
            let id = id.to_string();
            let store = store.clone();
            let deleted = deleted_b.clone();
            async move {
                deleted.lock().unwrap().push(id.clone());
                store.delete(&id).map(|_| true)
            }
        };
        delete_chat_session_with_aux_cascade(&store, &aux_b.id, recording_delete_b)
            .await
            .expect("direct aux delete");
        assert_eq!(
            deleted_b.lock().unwrap().as_slice(),
            std::slice::from_ref(&aux_b.id)
        );

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

    /// The failed-delete backstop sweep must be adoption-aware: it arms on a
    /// delete that FAILED (the record survives), so a GUI rename landing in
    /// the sweep window must stop the directory removal — while an absent
    /// record (delete converged) and a still-factory-titled record (never
    /// adopted) both stay sweepable. `NEW_CHAT_TITLE` is the factory title
    /// the adoption probe compares against.
    #[test]
    fn failed_delete_sweep_skips_adopted_records_but_keeps_converged_and_fresh_ones() {
        use super::EnginePool;
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = std::env::temp_dir().join(format!(
            "pinvou3-engine-pool-sweep-guard-{}",
            std::process::id()
        ));
        let previous_home = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&home);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };

        let store = SessionStore::boot().expect("session store");
        let session_id = store
            .create_new(
                "wire-model".to_string(),
                Some(crate::features::sessions::NEW_CHAT_TITLE.to_string()),
                home.join("workspace"),
            )
            .expect("chat session")
            .metadata
            .id;

        // Factory-titled record (never adopted) → sweepable.
        assert!(EnginePool::sweep_may_remove_session_dir(
            &store,
            &session_id
        ));

        // Adopted (renamed out of the factory title) inside the window → keep.
        store
            .set_title(&session_id, "user renamed me".to_string())
            .expect("adopting rename");
        assert!(!EnginePool::sweep_may_remove_session_dir(
            &store,
            &session_id
        ));

        // Record deleted after all (the retried store delete converged) →
        // the sweep still cleans the resurrected-orphan directory.
        store.delete(&session_id).expect("record delete");
        assert!(EnginePool::sweep_may_remove_session_dir(
            &store,
            &session_id
        ));

        match previous_home {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&home);
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
                DeleteGateRecheck::Unconditional,
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

    /// M6 mutation pin: the aux reset's delete half must run under the aux
    /// session's turn gate — replacing the gated delete with a bare
    /// `store.delete` lets the body complete while a turn (the blocker below)
    /// still owns the gate, and the "must wait" assertions go red.
    #[tokio::test]
    async fn aux_reset_delete_waits_for_the_turn_gate() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = std::env::temp_dir().join(format!(
            "pinvou3-engine-pool-aux-reset-gate-{}",
            std::process::id()
        ));
        let previous_home = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&home);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };

        let store = SessionStore::boot().expect("session store");
        let main_id = store
            .create_new("wire-model".to_string(), None, home.join("workspace"))
            .expect("chat session")
            .metadata
            .id;
        let aux_id = store
            .get_or_create_aux_session(&main_id)
            .expect("aux session")
            .id;
        let locks = SessionTurnLocks::default();
        let gate = locks.for_session(&aux_id).await;
        // A running aux turn owns the gate; the reset's delete half must queue behind it.
        let blocker = gate.lock().await;
        let evicted = Arc::new(AtomicBool::new(false));
        let forgotten = Arc::new(AtomicBool::new(false));

        let reset_locks = locks.clone();
        let reset_store = store.clone();
        let reset_main = main_id.clone();
        let reset_evicted = evicted.clone();
        let reset_forgotten = forgotten.clone();
        let reset_aux = aux_id.clone();
        let reset = tokio::spawn(async move {
            reset_aux_session_delete_with_gate(
                &reset_locks,
                &reset_store,
                &reset_main,
                |evict_id| {
                    assert_eq!(
                        evict_id, reset_aux,
                        "the gated delete must target the derived aux id"
                    );
                    let flag = reset_evicted.clone();
                    async move {
                        flag.store(true, Ordering::Release);
                    }
                },
                move |_forget_id| {
                    reset_forgotten.store(true, Ordering::Release);
                },
            )
            .await
        });
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        assert!(
            !evicted.load(Ordering::Acquire),
            "the reset's engine reclaim must wait for the aux turn gate"
        );
        assert!(
            store.load(&aux_id).is_ok(),
            "the aux record must survive while the turn gate is held"
        );
        drop(blocker);
        drop(gate);

        let deleted = reset
            .await
            .expect("reset task joins")
            .expect("gated delete");
        assert_eq!(deleted.as_deref(), Some(aux_id.as_str()));
        assert!(evicted.load(Ordering::Acquire));
        assert!(forgotten.load(Ordering::Acquire));
        assert!(
            store.load(&aux_id).is_err(),
            "the aux record must be deleted once the gate frees"
        );

        match previous_home {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(home);
    }

    /// M6: with no aux record the reset's delete half is an idempotent no-op
    /// (the create half then simply makes the fresh session).
    #[tokio::test]
    async fn aux_reset_delete_is_a_no_op_without_an_aux() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = std::env::temp_dir().join(format!(
            "pinvou3-engine-pool-aux-reset-absent-{}",
            std::process::id()
        ));
        let previous_home = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&home);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };

        let store = SessionStore::boot().expect("session store");
        let main_id = store
            .create_new("wire-model".to_string(), None, home.join("workspace"))
            .expect("chat session")
            .metadata
            .id;
        let locks = SessionTurnLocks::default();
        let deleted = reset_aux_session_delete_with_gate(
            &locks,
            &store,
            &main_id,
            |_evict_id| async { panic!("no aux engine to reclaim") },
            |_forget_id| panic!("no aux session to forget"),
        )
        .await
        .expect("no-op reset delete");
        assert_eq!(deleted, None);

        match previous_home {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(home);
    }

    /// PR #433 review regression: `delete_session`'s Chat branch cascades the
    /// auxiliary session through the gate (same pipeline as
    /// `discard_aux_session`) — first `pool.delete_chat_session(aux)`
    /// (reclaim the engine + store.delete + late sweep inside the turn gate),
    /// then delete the main session; `store.delete`'s on-disk cascade only
    /// removes the record and would leave a running aux engine as a
    /// handle-less orphan.
    /// The command body itself is now covered directly: the extracted,
    /// Tauri-free `delete_chat_session_cascade` pins the command's exact
    /// order at the command layer. This test keeps its own value: it drives
    /// the pool side (`delete_chat_session_with_gate`) over a real store and
    /// engine, verifying the pool delete path reclaims the aux engine and
    /// deletes the aux record, strictly before the main session's deletion.
    #[tokio::test]
    async fn chat_delete_cascades_aux_engine_reclaim_before_main_delete() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = std::env::temp_dir().join(format!(
            "pinvou3-engine-pool-aux-cascade-{}",
            std::process::id()
        ));
        let previous_home = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&home);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };

        let store = SessionStore::boot().expect("session store");
        let main_id = store
            .create_new("wire-model".to_string(), None, home.join("workspace"))
            .expect("chat session")
            .metadata
            .id;
        let aux_id = store
            .get_or_create_aux_session(&main_id)
            .expect("aux session")
            .id;
        let locks = SessionTurnLocks::default();
        let aux_engine_present = Arc::new(AtomicBool::new(true));
        let main_engine_present = Arc::new(AtomicBool::new(true));
        let order = Arc::new(StdMutex::new(Vec::new()));

        // An in-order replay of the delete_session command's Chat branch
        // (app/commands/sessions.rs): resolve the mapping first, gated-delete
        // the aux (reclaim engine + delete record), then gated-delete the
        // main session.
        let resolved_aux = store.aux_session_id(&main_id).expect("aux mapping");
        assert_eq!(resolved_aux, aux_id);
        {
            let engine = aux_engine_present.clone();
            let steps = order.clone();
            delete_chat_session_with_gate(
                &locks,
                &store,
                &resolved_aux,
                DeleteGateRecheck::Unconditional,
                || async move {
                    engine.store(false, Ordering::Release);
                    steps.lock().unwrap().push("evict-aux");
                },
                || {},
            )
            .await
            .expect("delete aux session");
        }
        {
            let engine = main_engine_present.clone();
            let steps = order.clone();
            delete_chat_session_with_gate(
                &locks,
                &store,
                &main_id,
                DeleteGateRecheck::Unconditional,
                || async move {
                    engine.store(false, Ordering::Release);
                    steps.lock().unwrap().push("evict-main");
                },
                || {},
            )
            .await
            .expect("delete main session");
        }

        assert!(
            !aux_engine_present.load(Ordering::Acquire),
            "the cascade delete must reclaim the aux engine and must not leave a handle-less orphan"
        );
        assert!(!main_engine_present.load(Ordering::Acquire));
        assert_eq!(
            *order.lock().unwrap(),
            vec!["evict-aux", "evict-main"],
            "aux engine reclamation must happen strictly before the main session delete"
        );
        assert!(
            store.load(&aux_id).is_err(),
            "the aux session record must be deleted"
        );
        assert!(store.load(&main_id).is_err());
        assert!(
            store.aux_session_id(&main_id).is_none(),
            "the main -> aux mapping must not remain after the cascade delete"
        );

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
        delete_chat_session_with_gate(
            &locks,
            &store,
            &session_id,
            DeleteGateRecheck::Unconditional,
            || async {},
            || {},
        )
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

    /// The headless stub-cleanup guard: `StillMessageFree` re-checks
    /// emptiness UNDER the turn gate, so a record whose transcript landed
    /// between the disposition's outside-the-gate sample and the delete is
    /// kept (started transcript, the only copy) while a genuine zero-message
    /// stub still deletes.
    #[tokio::test]
    async fn guarded_stub_delete_keeps_a_record_that_has_messages() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // RAII env restore: a failing assertion unwinds past the straight-line
        // restore this replaces and would leave PINVOU3_HOME stale for every
        // later test in the process.
        let _env = EnvRestore::capture(&["PINVOU3_HOME"]);
        let home = std::env::temp_dir().join(format!(
            "pinvou3-engine-pool-stub-guard-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&home);
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };

        let store = SessionStore::boot().expect("session store");
        let session_id = store
            .create_new("wire-model".to_string(), None, home.join("workspace"))
            .expect("chat session")
            .metadata
            .id;
        // Simulate a transcript admitted after the disposition sampled an
        // empty record: the durable record now carries messages.
        let mut started = deepseek_tui::session_manager::create_saved_session_with_id_and_mode(
            session_id.clone(),
            &[deepseek_tui::models::Message {
                role: "user".into(),
                content: vec![deepseek_tui::models::ContentBlock::Text {
                    text: "admitted before the gate".into(),
                    cache_control: None,
                }],
            }],
            "wire-model",
            &home,
            0,
            None,
            None,
        );
        started.metadata.updated_at = chrono::Utc::now();
        store.save_session_atomic(&started).expect("seed messages");

        let locks = SessionTurnLocks::default();
        let deleted = delete_chat_session_with_gate(
            &locks,
            &store,
            &session_id,
            DeleteGateRecheck::StillAStub,
            || async {},
            || {},
        )
        .await
        .expect("guarded delete");
        assert!(
            !deleted,
            "a record that carries messages under the gate must be kept"
        );
        assert!(
            store.load(&session_id).is_ok(),
            "the started transcript survives the guarded stub delete"
        );

        // A genuine zero-message stub remains cleanup-eligible.
        let empty_id = store
            .create_new("wire-model".to_string(), None, home.join("workspace"))
            .expect("empty session")
            .metadata
            .id;
        let deleted = delete_chat_session_with_gate(
            &locks,
            &store,
            &empty_id,
            DeleteGateRecheck::StillAStub,
            || async {},
            || {},
        )
        .await
        .expect("guarded delete");
        assert!(deleted, "a zero-message stub is still deleted");
        assert!(store.load(&empty_id).is_err());

        let _ = std::fs::remove_dir_all(home);
    }

    /// The one-shot adoption guard: `DeleteGateRecheck::StillFactoryTitled`
    /// re-checks the `NEW_CHAT_TITLE` marker UNDER the turn gate, so a
    /// rename landing while the delete waited on a live turn keeps the
    /// record (a GUI user adopted it), while a record that still wears the
    /// factory title deletes.
    #[tokio::test]
    async fn one_shot_delete_gate_rechecks_the_adoption_marker() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvRestore::capture(&["PINVOU3_HOME"]);
        let home = std::env::temp_dir().join(format!(
            "pinvou3-engine-pool-adoption-guard-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&home);
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };

        let store = SessionStore::boot().expect("session store");
        let adopted_id = store
            .create_new("wire-model".to_string(), None, home.join("workspace"))
            .expect("chat session")
            .metadata
            .id;
        store
            .set_title(&adopted_id, "Adopted by the GUI user".to_owned())
            .expect("rename the session away from the factory title");

        let locks = SessionTurnLocks::default();
        let deleted = delete_chat_session_with_gate(
            &locks,
            &store,
            &adopted_id,
            DeleteGateRecheck::StillFactoryTitled,
            || async {},
            || {},
        )
        .await
        .expect("guarded delete");
        assert!(
            !deleted,
            "a record renamed under the gate was adopted and must be kept"
        );
        assert!(
            store.load(&adopted_id).is_ok(),
            "the adopted session survives the guarded one-shot delete"
        );

        // A record that still wears the factory title deletes normally.
        let fresh_id = store
            .create_new("wire-model".to_string(), None, home.join("workspace"))
            .expect("factory-titled session")
            .metadata
            .id;
        let deleted = delete_chat_session_with_gate(
            &locks,
            &store,
            &fresh_id,
            DeleteGateRecheck::StillFactoryTitled,
            || async {},
            || {},
        )
        .await
        .expect("guarded delete");
        assert!(
            deleted,
            "a record that still wears the factory title is not adopted"
        );
        assert!(store.load(&fresh_id).is_err());

        // The stub twin re-checks BOTH stub facts under the gate: a renamed
        // zero-message record is an adopted session, not a stub.
        let adopted_stub_id = store
            .create_new("wire-model".to_string(), None, home.join("workspace"))
            .expect("stub session")
            .metadata
            .id;
        store
            .set_title(&adopted_stub_id, "Renamed empty chat".to_owned())
            .expect("rename the stub away from the factory title");
        let deleted = delete_chat_session_with_gate(
            &locks,
            &store,
            &adopted_stub_id,
            DeleteGateRecheck::StillAStub,
            || async {},
            || {},
        )
        .await
        .expect("guarded stub delete");
        assert!(
            !deleted,
            "a renamed zero-message record is adopted, not a stub"
        );
        assert!(store.load(&adopted_stub_id).is_ok());

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

    /// Production-wiring probe (review #463 round-14 R2): every behavioral
    /// test of `rebind_evict_with_gates` injects a hand-copied take closure
    /// that hard-codes the scheduled arm away (`rebind_evictable(active,
    /// false)`), so deleting `scheduled_running_sessions` from the PRODUCTION
    /// closure in `evict_if_idle_for_rebind` shipped green. The pool itself
    /// needs an AppHandle and cannot be unit-constructed, so pin the reads
    /// in the production body — the layer the pure-predicate tests cannot
    /// cover.
    fn production_body<'a>(src: &'a str, signature: &str) -> &'a str {
        let start = src.find(signature).expect("production fn must exist");
        let rest = &src[start + signature.len()..];
        // End at the next fn ITEM, not the next `pub ` token (review #463
        // round-17 SF-3): a `pub(crate)`/`pub(super)` item would otherwise
        // extend the span past the function under test, silently diluting
        // (and eventually neutralizing) the contains-assertions.
        let end = [
            "\n    pub ",
            "\n    pub(crate) ",
            "\n    pub(super) ",
            "\n    ///",
        ]
        .iter()
        .filter_map(|marker| rest.find(marker))
        .min()
        .map_or(src.len(), |offset| start + signature.len() + offset);
        &src[start..end]
    }

    #[test]
    fn rebind_engine_take_production_closure_reads_turn_and_scheduled_state() {
        let src = include_str!("engine_pool.rs");
        // Round-26 MAJOR-3: cut at the test module exactly like projects.rs's
        // `production_source` (round-18) — an unanchored find over the WHOLE
        // file lands on this probe's own string argument once the production
        // function is deleted or renamed, and the span then contains the
        // probe's own assertion text, which can never fail.
        let src = &src[..src
            .find("#[allow(clippy::await_holding_lock)]\nmod scheduled_model_tests {")
            .expect("the test module marker must exist")];
        let body = production_body(src, "pub async fn evict_if_idle_for_rebind");
        assert!(
            body.contains("is_turn_active(session_id)"),
            "the take must recheck the turn state before reclaiming",
        );
        assert!(
            body.contains("scheduled_running_sessions"),
            "the take must refuse eviction while a scheduled turn is running",
        );
        // review #463 round-19 SF-4: contains-only is order-insensitive —
        // moving the entry removal BEFORE the evictability recheck would
        // evict a session that just turned busy. Pin the order: the
        // rebind_evictable gate must precede the removal.
        let gate_at = body
            .find("rebind_evictable(")
            .expect("the take must gate on rebind_evictable");
        let remove_at = body
            .find("entries.lock().await.remove(session_id)")
            .expect("the take must remove the pool entry");
        assert!(
            gate_at < remove_at,
            "the evictability recheck must precede the entry removal"
        );
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
    use super::{
        CachedEntryReuse, EnginePool, PendingNativeWindow, Pinvou3Bridge, PreparedRuntimeModel,
    };
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

    /// Ollama's `/v1/models` never carries a window fact; an endpoint probed
    /// as Ollama must follow up on the native API. The loaded model's
    /// `/api/ps` effective context (the deployment ground truth, e.g. a
    /// 131072 OLLAMA_CONTEXT_LENGTH serving) fills `probed_context_tokens`,
    /// so route limits no longer fall to the foundation's 8192
    /// unknown-Ollama fallback (compaction floor 4096 / input budget 1024).
    #[tokio::test]
    async fn ollama_probed_route_adopts_native_ps_context() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            ("/v1/models", 200, r#"{"data":[{"id":"my-model"}]}"#.into()),
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"my-model","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        bridge.probed_local_kind = Some(LocalServerKind::Ollama);
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, false, false).await;
        assert_eq!(
            bridge.probed_context_tokens,
            Some(131_072),
            "the native effective context must ride the probed facts"
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            0,
            "the /api/ps fact wins; /api/show stays unqueried"
        );
        assert_eq!(bridge.session_model.as_ref().unwrap().model, "my-model");
    }

    /// A hand-typed tagless configured name (`ollama run llama3` shape)
    /// against a canonical roster: Ollama reports the entry as
    /// `llama3:latest` in `/api/ps`, so the route's own native fetch must
    /// still resolve it — otherwise the first-load self-heal never lands
    /// and the 8192-fallback collapse (a global `OLLAMA_CONTEXT_LENGTH`,
    /// the 2026-09-30 report's fatality) survives the engine's whole
    /// lifetime with the marker armed forever. This is the pin whose
    /// absence let that shape slip through round 5: dropping the
    /// `:latest` fallback turns all three assertions red.
    #[tokio::test]
    async fn tagless_configured_name_adopts_canonical_ps_window() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            (
                "/v1/models",
                200,
                r#"{"data":[{"id":"llama3:latest"},{"id":"deepseek-r1:14b"}]}"#.into(),
            ),
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"llama3:latest","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "llama3", Some("custom"));
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        bridge.probed_local_kind = Some(LocalServerKind::Ollama);
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, false, false).await;
        assert_eq!(
            bridge.probed_context_tokens,
            Some(131_072),
            "the tagless configured name resolves its own canonical /api/ps entry"
        );
        assert!(
            !bridge.native_window_recheck,
            "a fact was adopted — the reuse re-check must not arm"
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            0,
            "the ps fact wins; /api/show stays unqueried"
        );
    }

    /// Same tagless shape on a single-model server (a fresh install with
    /// one downloaded model): `resolve_served_model_from_entries` follows
    /// the single listed id, so served is the canonical `llama3:latest`
    /// while the route stays the hand-typed `llama3` — the adoption gate
    /// must fold the Ollama canonical form. Without the fold the whole
    /// native block is skipped (no adoption AND no arming, since the
    /// fetch and the re-check marker share the gate) and the
    /// 8192-fallback collapse survives the engine's whole lifetime with
    /// no self-heal.
    #[tokio::test]
    async fn tagless_single_entry_roster_adopts_canonical_ps_window() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            (
                "/v1/models",
                200,
                r#"{"data":[{"id":"llama3:latest"}]}"#.into(),
            ),
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"llama3:latest","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "llama3", Some("custom"));
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        bridge.probed_local_kind = Some(LocalServerKind::Ollama);
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, false, false).await;
        assert_eq!(
            bridge.probed_context_tokens,
            Some(131_072),
            "the tagless configured name adopts its canonical entry even on a single-entry roster"
        );
        assert!(
            !bridge.native_window_recheck,
            "a fact was adopted — the reuse re-check must not arm"
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            0,
            "the ps fact wins; /api/show stays unqueried"
        );
    }

    /// Model downloaded but not loaded (no /api/ps entry) with a Modelfile
    /// num_ctx declaration: the declared value is the window fact (it is the
    /// effective value once the model loads).
    #[tokio::test]
    async fn ollama_probed_route_falls_back_to_show_context() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            ("/v1/models", 200, r#"{"data":[{"id":"my-model"}]}"#.into()),
            ("/api/ps", 200, r#"{"models":[]}"#.into()),
            (
                "/api/show",
                200,
                r#"{"parameters":"num_ctx 32768","model_info":{"general.architecture":"qwen3","qwen3.context_length":40960}}"#.into(),
            ),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        bridge.probed_local_kind = Some(LocalServerKind::Ollama);
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, false, false).await;
        assert_eq!(bridge.probed_context_tokens, Some(32_768));
    }

    /// Never loaded and no num_ctx declaration: `/api/show` only carries the
    /// GGUF *trained* context (a capability ceiling — an unloaded model
    /// actually serves at the server default, e.g. 4096, which no API
    /// exposes). The trained cap must not become the route limit: adopting
    /// it would loosen compaction budgets beyond what the server accepts and
    /// let upstream silently truncate, and the value is frozen for the
    /// pooled engine's lifetime (no refresh after the first load). The route
    /// keeps no probed window instead, i.e. the conservative foundation
    /// fallback, until a respawn after the model has loaded (`/api/ps` then
    /// reports the effective value).
    #[tokio::test]
    async fn ollama_probed_route_refuses_trained_context_without_num_ctx() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            ("/v1/models", 200, r#"{"data":[{"id":"my-model"}]}"#.into()),
            ("/api/ps", 200, r#"{"models":[]}"#.into()),
            (
                "/api/show",
                200,
                r#"{"model_info":{"general.architecture":"qwen3","qwen3.context_length":131072}}"#
                    .into(),
            ),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        bridge.probed_local_kind = Some(LocalServerKind::Ollama);
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, false, false).await;
        assert_eq!(
            mock.hits_for("/api/show"),
            1,
            "the /api/show follow-up ran and still must refuse the trained cap"
        );
        assert_eq!(
            bridge.probed_context_tokens, None,
            "a 128K trained cap with no runtime declaration must not become the route limit"
        );
    }

    /// The native follow-up is gated on the endpoint being probed as Ollama:
    /// any other kind (Generic / vLLM / LM Studio / unprobed) never issues
    /// the native requests.
    #[tokio::test]
    async fn non_ollama_kind_skips_native_context_fetch() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            ("/v1/models", 200, r#"{"data":[{"id":"my-model"}]}"#.into()),
            ("/api/ps", 200, r#"{"models":[]}"#.into()),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        bridge.probed_local_kind = Some(LocalServerKind::Generic);
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, false, false).await;
        assert_eq!(bridge.probed_context_tokens, None);
        assert_eq!(
            mock.hits_for("/api/ps"),
            0,
            "non-Ollama kinds must not receive Ollama native probes"
        );
        assert_eq!(mock.hits_for("/api/show"), 0);
    }

    /// Facts never borrow across names: the configured name missing from the
    /// list is *kept* (served == configured → adopts holds), so the native
    /// follow-up fires — but it queries `/api/ps` + `/api/show` **by that
    /// name**, and another loaded model's context ("a" here) cannot be
    /// misattributed to it.
    #[tokio::test]
    async fn kept_unlisted_name_fetches_only_its_own_native_fact() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            (
                "/v1/models",
                200,
                r#"{"data":[{"id":"a"},{"id":"b"}]}"#.into(),
            ),
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"a","context_length":131072}]}"#.into(),
            ),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "gone", Some("custom"));
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        bridge.probed_local_kind = Some(LocalServerKind::Ollama);
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, false, false).await;
        assert_eq!(
            mock.hits_for("/api/ps"),
            1,
            "the native follow-up fires for the kept name"
        );
        assert_eq!(
            bridge.probed_context_tokens, None,
            "another model's /api/ps fact must not be lent to the unlisted name"
        );
        assert_eq!(
            bridge.session_model.as_ref().unwrap().model,
            "gone",
            "non-vLLM routes keep the configured name"
        );
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

    /// The real finalize_runtime_bridge injection block (Ollama side):
    /// the kind probe completes as Ollama from the live `/api/tags`
    /// signature (no hand-set `probed_local_kind`), and the adoption
    /// follow-up then fetches the native window for the name actually sent
    /// — pinning the production ordering "kind probe before adopt" that
    /// the whole feature depends on: reordering those two blocks must turn
    /// this red.
    #[tokio::test]
    async fn finalize_runtime_bridge_adopts_native_window_for_ollama_kind() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            // Ollama's OpenAI shim shape: the listing carries no window fact.
            ("/v1/models", 200, r#"{"data":[{"id":"my-model"}]}"#.into()),
            // The kind signature that classifies the endpoint as Ollama.
            (
                "/api/tags",
                200,
                r#"{"models":[{"name":"my-model"}]}"#.into(),
            ),
            // The native effective context the follow-up must adopt.
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"my-model","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        let bridge = wiring_bridge(model.clone());
        let prepared = PreparedRuntimeModel::unchanged(model);
        let bridge = EnginePool::finalize_runtime_bridge(bridge, &prepared, false).await;
        assert_eq!(
            bridge.probed_local_kind,
            Some(LocalServerKind::Ollama),
            "the live /api/tags signature must classify the endpoint before adoption"
        );
        assert_eq!(
            bridge.probed_context_tokens,
            Some(131_072),
            "the native effective context must ride the probed facts"
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            0,
            "the /api/ps fact wins; /api/show stays unqueried"
        );
        assert_eq!(
            bridge.session_model.as_ref().unwrap().model,
            "my-model",
            "non-vLLM routes do no served-name correction"
        );
    }

    /// Round-3 R1: a discovery-added Ollama endpoint is saved under the
    /// LocalVllm preset (provider "vllm"). The kind probe must run for
    /// vllm-provider local routes too, so the native follow-up fires and
    /// the route carries the real window — instead of the 128K fallback
    /// while the monitor shows the truth. Driven through the real finalize
    /// block (no hand-set kind): the live `/api/tags` signature classifies
    /// the endpoint, then the adoption adopts the `/api/ps` fact.
    #[tokio::test]
    async fn finalize_runtime_bridge_probes_local_vllm_route_for_ollama_window() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            // Ollama's OpenAI shim shape: the listing carries no window fact.
            ("/v1/models", 200, r#"{"data":[{"id":"my-model"}]}"#.into()),
            // The kind signature that classifies the endpoint as Ollama.
            (
                "/api/tags",
                200,
                r#"{"models":[{"name":"my-model"}]}"#.into(),
            ),
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"my-model","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let mut model = saved_model(ModelPreset::LocalVllm, "my-model", None);
        model.base_url = mock.base_url.clone();
        let bridge = wiring_bridge(model.clone());
        let prepared = PreparedRuntimeModel::unchanged(model);
        let bridge = EnginePool::finalize_runtime_bridge(bridge, &prepared, false).await;
        assert_eq!(
            bridge.probed_local_kind,
            Some(LocalServerKind::Ollama),
            "a local vllm-provider route must be kind-probed (discovery saves Ollama under this preset)"
        );
        assert_eq!(
            bridge.probed_context_tokens,
            Some(131_072),
            "the native effective context must reach the route instead of the 128K fallback"
        );
        assert_eq!(mock.hits_for("/api/show"), 0, "the /api/ps fact wins");
    }

    /// Adoption-level pin for the same path: a hand-set Ollama kind on a
    /// vllm-provider route adopts the `/api/ps` effective context for the
    /// name actually sent.
    #[tokio::test]
    async fn local_vllm_preset_ollama_kind_adopts_native_ps_context() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            ("/v1/models", 200, r#"{"data":[{"id":"my-model"}]}"#.into()),
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"my-model","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let mut model = saved_model(ModelPreset::LocalVllm, "my-model", None);
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        bridge.probed_local_kind = Some(LocalServerKind::Ollama);
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, true, false).await;
        assert_eq!(
            bridge.probed_context_tokens,
            Some(131_072),
            "a vllm-preset route with an Ollama kind must adopt the native window"
        );
        assert_eq!(mock.hits_for("/api/show"), 0);
    }

    /// Round-3 R1 (LM Studio side): a probed-LM Studio endpoint adopts the
    /// native `/api/v0/models` served window (`loaded_context_length` of
    /// the loaded entry — bug #726's 131072-vs-12918 divergence), never the
    /// capability cap, and never the Ollama native APIs.
    #[tokio::test]
    async fn lmstudio_kind_adopts_v0_served_window() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            // The listing carries no window fact.
            ("/v1/models", 200, r#"{"data":[{"id":"my-model"}]}"#.into()),
            (
                "/api/v0/models",
                200,
                r#"{"data":[{"id":"my-model","state":"loaded","max_context_length":131072,"loaded_context_length":12918}]}"#
                    .into(),
            ),
            ("/api/ps", 200, r#"{"models":[]}"#.into()),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        bridge.probed_local_kind = Some(LocalServerKind::LmStudio);
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, false, false).await;
        assert_eq!(
            bridge.probed_context_tokens,
            Some(12_918),
            "the served window (12918), not the capability cap (131072)"
        );
        assert_eq!(
            mock.hits_for("/api/ps"),
            0,
            "an LM Studio endpoint must not be asked the Ollama native API"
        );
        assert_eq!(mock.hits_for("/api/show"), 0);
    }

    /// Round-3 R3: the reuse-path re-adoption marker. `from_finalized`
    /// captures only native-probeable routes that ended the spawn without a
    /// served-window fact (a fact at spawn means the normal
    /// frozen-until-rebuild discipline applies; non-native kinds never
    /// re-check), and only when the adoption path armed the re-check (see
    /// `pending_window_not_armed_when_fact_cannot_be_adopted` /
    /// `pending_window_not_armed_for_declared_route` — a fact the route can
    /// never adopt, or a declaration that min-clamps any fact, must never
    /// mark the entry). `reprobe_missing_native_window` returns the window
    /// once the server can actually serve it (model JIT-loaded since) and
    /// None while it can't. The get_or_spawn glue applies the extracted
    /// `cached_entry_reuse_decision` to drop the entry, reclaim, and let
    /// the rebuild path re-finalize and adopt — the decision itself is
    /// pinned by `pending_window_reuse_decision_rebuilds_once_fact_
    /// materializes`; the surrounding pool plumbing (EnginePool needs an
    /// AppHandle) stays the unit-test boundary of the rest of the spawn
    /// wiring.
    #[tokio::test]
    async fn pending_native_window_marks_factless_native_routes() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        // Ollama route, model never loaded, no num_ctx → pending.
        let mock = models_mock::spawn(&[
            ("/v1/models", 200, r#"{"data":[{"id":"my-model"}]}"#.into()),
            (
                "/api/tags",
                200,
                r#"{"models":[{"name":"my-model"}]}"#.into(),
            ),
            ("/api/ps", 200, r#"{"models":[]}"#.into()),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        let bridge = wiring_bridge(model.clone());
        let prepared = PreparedRuntimeModel::unchanged(model);
        let bridge = EnginePool::finalize_runtime_bridge(bridge, &prepared, false).await;
        let pending = PendingNativeWindow::from_finalized(&bridge)
            .expect("a factless Ollama route must be marked pending");
        assert_eq!(pending.model_name, "my-model");
        // Arming is part of the construction: without it nothing is marked,
        // so a route that never asked its native API can never mark itself.
        let mut unarmed = bridge.clone();
        unarmed.native_window_recheck = false;
        assert_eq!(
            PendingNativeWindow::from_finalized(&unarmed),
            None,
            "an unarmed spawn must never mark the entry, however factless it is"
        );
        // The re-probe against the still-unloaded server: no fact (engine
        // stays).
        assert_eq!(
            EnginePool::reprobe_missing_native_window(&bridge, &pending).await,
            None
        );
        // After the model loads, the re-probe finds the effective window.
        // The re-probe re-classifies the endpoint first, so the mock must
        // carry the Ollama signature the battery looks for.
        let loaded = models_mock::spawn(&[
            (
                "/api/tags",
                200,
                r#"{"models":[{"name":"my-model"}]}"#.into(),
            ),
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"my-model","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let mut loaded_model =
            saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        loaded_model.base_url = loaded.base_url.clone();
        let loaded_bridge = wiring_bridge(loaded_model);
        assert_eq!(
            EnginePool::reprobe_missing_native_window(&loaded_bridge, &pending).await,
            Some(131_072),
            "the post-load fact must be found so the caller can drop the entry for a rebuild"
        );
        // A route that ended the spawn WITH a fact is not pending.
        let loaded_finalize = models_mock::spawn(&[
            ("/v1/models", 200, r#"{"data":[{"id":"my-model"}]}"#.into()),
            (
                "/api/tags",
                200,
                r#"{"models":[{"name":"my-model"}]}"#.into(),
            ),
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"my-model","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let mut with_fact_model =
            saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        with_fact_model.base_url = loaded_finalize.base_url.clone();
        let with_fact_bridge = wiring_bridge(with_fact_model.clone());
        let with_fact_bridge = EnginePool::finalize_runtime_bridge(
            with_fact_bridge,
            &PreparedRuntimeModel::unchanged(with_fact_model),
            false,
        )
        .await;
        assert_eq!(
            PendingNativeWindow::from_finalized(&with_fact_bridge),
            None,
            "a fact adopted at spawn means the normal rebuild discipline applies"
        );
        // A Generic (no-signature) route is never pending.
        let generic =
            models_mock::spawn(&[("/v1/models", 200, r#"{"data":[{"id":"my-model"}]}"#.into())]);
        let mut generic_model =
            saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        generic_model.base_url = generic.base_url.clone();
        let generic_bridge = wiring_bridge(generic_model.clone());
        let generic_bridge = EnginePool::finalize_runtime_bridge(
            generic_bridge,
            &PreparedRuntimeModel::unchanged(generic_model),
            false,
        )
        .await;
        assert_eq!(
            bridge.probed_local_kind,
            Some(LocalServerKind::Ollama),
            "sanity: the first fixture classified Ollama"
        );
        assert_eq!(
            PendingNativeWindow::from_finalized(&generic_bridge),
            None,
            "non-native kinds have no native API to re-check"
        );
    }

    /// Round-5 MAJOR pin: a route whose fact can materialize but can never
    /// be adopted must not arm the reuse re-check. The roster lists only
    /// `server-name` while the route is configured `my-model`, so `adopts`
    /// is false and the spawn never runs the native fetch — but `/api/show`
    /// resolves the loose name server-side and would serve `num_ctx` on
    /// every re-probe. Arming here would drop and respawn the engine on
    /// every send, forever, with the window never landing (the rebuild hits
    /// the same `adopts=false`); the arming flag is what keeps the
    /// reuse path on `Keep`.
    #[tokio::test]
    async fn pending_window_not_armed_when_fact_cannot_be_adopted() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            (
                "/v1/models",
                200,
                r#"{"data":[{"id":"server-name"}]}"#.into(),
            ),
            (
                "/api/tags",
                200,
                r#"{"models":[{"name":"server-name"}]}"#.into(),
            ),
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"server-name","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"parameters":"num_ctx 4096"}"#.into()),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        let mut bridge = wiring_bridge(model.clone());
        // The kind probe lives in finalize (unit boundary, see the module
        // doc); the direct adoption call hand-sets the classified kind like
        // every sibling test.
        bridge.probed_local_kind = Some(LocalServerKind::Ollama);
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, false, false).await;
        assert!(
            !bridge.native_window_recheck,
            "a route whose native fact can never be adopted must not arm the re-check"
        );
        // The trap itself: the native API serves a fact for the configured
        // name on every re-probe — which is exactly why an armed marker
        // would have looped rebuilds (adoption refuses the borrowed roster
        // name on the rebuild too).
        let pending = PendingNativeWindow {
            model_name: "my-model".into(),
            kind: LocalServerKind::Ollama,
        };
        assert_eq!(
            EnginePool::reprobe_missing_native_window(&bridge, &pending).await,
            Some(4096),
            "sanity: the server resolves the loose name — arming this shape is the bug"
        );
    }

    /// A user declaration min-clamps any adopted fact, so the native leg
    /// cannot change a declared route: it must not even run, and nothing
    /// arms (otherwise the route would pay a per-turn native GET plus one
    /// rebuild whose outcome the declaration clamps right back).
    #[tokio::test]
    async fn pending_window_not_armed_for_declared_route() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            ("/v1/models", 200, r#"{"data":[{"id":"my-model"}]}"#.into()),
            ("/api/ps", 200, r#"{"models":[]}"#.into()),
            ("/api/show", 200, r#"{"parameters":"num_ctx 4096"}"#.into()),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        model.context_window_tokens = Some(8192);
        let mut bridge = wiring_bridge(model.clone());
        bridge.probed_local_kind = Some(LocalServerKind::Ollama);
        EnginePool::adopt_probed_endpoint_facts(&mut bridge, model, false, false).await;
        assert!(
            !bridge.native_window_recheck,
            "a declared route never arms the reuse re-check"
        );
        assert_eq!(
            mock.hits_for("/api/ps"),
            0,
            "the declaration min-clamps every adopted fact — the native leg must not even run"
        );
        assert_eq!(mock.hits_for("/api/show"), 0);
        assert_eq!(
            bridge.probed_context_tokens, None,
            "no fact is adopted over the declaration's head"
        );
    }

    /// The reuse-path self-heal decision (`cached_entry_reuse_decision`,
    /// extracted verbatim from the `get_or_spawn_with_policy` glue —
    /// EnginePool itself needs an AppHandle, the same unit-test boundary as
    /// `require_live_engine_for_steer`): no pending marker → keep without
    /// probing; pending but still no fact → keep (re-ask next turn); the
    /// fact materialized → rebuild so it is adopted. A mutation that always
    /// keeps the entry would let the collapsed 8192-fallback budget survive
    /// the engine's whole lifetime — the reported fatality — and must turn
    /// this red.
    #[tokio::test]
    async fn pending_window_reuse_decision_rebuilds_once_fact_materializes() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        // No pending marker: keep, without dialing anything (the bridge has
        // no base_url to reach).
        let no_pending_bridge = wiring_bridge(saved_model(
            ModelPreset::OpenaiCompatible,
            "my-model",
            Some("custom"),
        ));
        assert_eq!(
            EnginePool::cached_entry_reuse_decision(None, &no_pending_bridge).await,
            CachedEntryReuse::Keep,
            "a factless non-pending route must keep its engine without any re-probe"
        );
        let pending = PendingNativeWindow {
            model_name: "my-model".into(),
            kind: LocalServerKind::Ollama,
        };
        // Pending, model still unloaded: keep (the next turn re-asks).
        // The re-probe re-classifies first, so the mock carries the Ollama
        // signature.
        let mock = models_mock::spawn(&[
            (
                "/api/tags",
                200,
                r#"{"models":[{"name":"my-model"}]}"#.into(),
            ),
            ("/api/ps", 200, r#"{"models":[]}"#.into()),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        let unloaded_bridge = wiring_bridge(model);
        assert_eq!(
            EnginePool::cached_entry_reuse_decision(Some(&pending), &unloaded_bridge).await,
            CachedEntryReuse::Keep,
            "no post-load fact yet — the engine stays and the next turn re-asks"
        );
        // The model loaded since spawn: the decision must demand a rebuild
        // so the rebuild path adopts the fact.
        let loaded = models_mock::spawn(&[
            (
                "/api/tags",
                200,
                r#"{"models":[{"name":"my-model"}]}"#.into(),
            ),
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"my-model","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let mut loaded_model =
            saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        loaded_model.base_url = loaded.base_url.clone();
        let loaded_bridge = wiring_bridge(loaded_model);
        assert_eq!(
            EnginePool::cached_entry_reuse_decision(Some(&pending), &loaded_bridge).await,
            CachedEntryReuse::RebuildForWindowFact,
            "the post-load fact must drop the entry for the adopting rebuild"
        );
    }

    /// The marker's frozen kind must not drive the re-probe dispatch: the
    /// stack behind the port can swap while the engine lives (dev restart
    /// onto a different server class), and dispatching on the stale kind
    /// would re-dial the wrong native API on every send — a 404 that never
    /// arms a fact — so the real served window would never land until an
    /// unrelated rebuild. The re-probe must re-classify (cache-peeked
    /// battery) and follow the fresh kind. Deleting the re-classification
    /// lands the frozen-Ollama dispatch on the doomed `/api/ps` (hits > 0,
    /// window None) and turns both assertions red.
    #[tokio::test]
    async fn reprobe_reclassifies_swapped_server_kind() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        // The re-classification below runs the real battery and the
        // hygiene clear at the end resets the shared probe state, both of
        // which must run serially against the core probe tests (their
        // in-flight registrations live in the same process-global registry;
        // see PROBE_STATE_TEST_MUTEX's doc).
        let _probe_state = crate::core::model_endpoint::tests::PROBE_STATE_TEST_MUTEX
            .lock()
            .await;
        // The port now serves LM Studio's v0 API; the Ollama ps endpoint the
        // stale marker would dial is a registered 404 so the miss is
        // countable.
        let mock = models_mock::spawn(&[
            (
                "/api/v0/models",
                200,
                r#"{"data":[{"id":"my-model","state":"loaded","max_context_length":131072,"loaded_context_length":12918}]}"#
                    .into(),
            ),
            ("/api/ps", 404, "not found".into()),
        ]);
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = mock.base_url.clone();
        let bridge = wiring_bridge(model);
        // A marker frozen by the pre-swap spawn: kind Ollama.
        let pending = PendingNativeWindow {
            model_name: "my-model".into(),
            kind: LocalServerKind::Ollama,
        };
        assert_eq!(
            EnginePool::reprobe_missing_native_window(&bridge, &pending).await,
            Some(12_918),
            "the re-probe must follow the swapped-in LM Studio served window"
        );
        assert_eq!(
            mock.hits_for("/api/ps"),
            0,
            "the frozen Ollama kind must not re-dial the doomed /api/ps"
        );
        // Cross-test hygiene: the re-classification cached this mock's live
        // port for the TTL; clear it under ENV_LOCK so a port-reusing later
        // test cannot inherit the stale kind.
        crate::core::model_endpoint::clear_probe_kind_cache();
    }

    /// The reuse re-probe is locality-gated in its own right. Pinned without
    /// touching the network via the v4-mapped loopback trick: the URL
    /// classifies non-local while routing to the same mock, so deleting the
    /// guard lets the fetch land on the mock (which serves a loaded fact →
    /// `RebuildForWindowFact`) and turns both assertions red.
    #[tokio::test]
    async fn reprobe_refuses_non_local_base_url() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"my-model","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"parameters":"num_ctx 4096"}"#.into()),
        ]);
        let port = mock
            .base_url
            .rsplit(':')
            .next()
            .expect("mock base_url carries a port")
            .to_string();
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "my-model", Some("custom"));
        model.base_url = format!("http://[::ffff:127.0.0.1]:{port}");
        let bridge = wiring_bridge(model);
        let pending = PendingNativeWindow {
            model_name: "my-model".into(),
            kind: LocalServerKind::Ollama,
        };
        assert_eq!(
            EnginePool::cached_entry_reuse_decision(Some(&pending), &bridge).await,
            CachedEntryReuse::Keep,
            "a non-local base_url must not be natively re-probed"
        );
        assert_eq!(
            mock.hits_for("/api/ps"),
            0,
            "the locality guard must run before any request"
        );
    }

    /// The finalize kind probe is locality-gated. Pinned without touching
    /// the network via a v4-mapped loopback URL: `::ffff:127.0.0.1`
    /// classifies as a non-local IPv6 literal (the conservative arm of
    /// `base_url_uses_local_or_private`) while still routing to the same
    /// loopback mock — so a hit on the mock proves the gate ran, and
    /// deleting the gate turns both assertions red (the battery classifies
    /// the live `/api/tags` signature and `probed_local_kind` stops being
    /// None).
    #[tokio::test]
    async fn finalize_skips_kind_probe_for_public_host() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _env = isolate_model_env();
        let mock = models_mock::spawn(&[
            ("/v1/models", 200, r#"{"data":[{"id":"my-model"}]}"#.into()),
            (
                "/api/tags",
                200,
                r#"{"models":[{"name":"my-model"}]}"#.into(),
            ),
        ]);
        let port = mock
            .base_url
            .rsplit(':')
            .next()
            .expect("mock base_url carries a port")
            .to_string();
        let mut model = saved_model(ModelPreset::LocalVllm, "my-model", None);
        model.base_url = format!("http://[::ffff:127.0.0.1]:{port}");
        let bridge = wiring_bridge(model.clone());
        let prepared = PreparedRuntimeModel::unchanged(model);
        let bridge = EnginePool::finalize_runtime_bridge(bridge, &prepared, false).await;
        assert_eq!(
            bridge.probed_local_kind, None,
            "a non-local base_url must not be kind-probed"
        );
        assert_eq!(
            mock.hits_for("/api/tags"),
            0,
            "the signature battery must not reach the host the gate refuses"
        );
    }

    /// The real finalize_runtime_bridge injection block (vLLM side):
    /// provider() derives "vllm" and a single entry renames to the served
    /// name and adopts facts. The kind probe now also runs for vllm-provider
    /// routes (see `finalize_runtime_bridge_probes_local_vllm_route_for_
    /// ollama_window`); this mock carries no kind signature, so the probe
    /// lands Generic — inert for the listing-facts path.
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
