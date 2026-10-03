//! Headless single-task agentic entry for external harnesses
//! (Terminal-Bench/Harbor).
//!
//! Unlike the eval backend in [`super::headless_bridge`], this runs a
//! **product-equivalent** agentic turn: `TurnInput::eval_tool_policy = None` →
//! `EnginePool::send_user_message` (reserve_turn + send_reserved_user_message,
//! the same submission path the GUI chat command uses; Yolo
//! mode, product tool allowlist, Bash/File write access, real shell). Eval
//! read-only isolation is unaffected: the GAIA path still enforces its eval
//! policy, and this entry never goes through `HeadlessAgentBackend` nor
//! touches any eval tool policy.
//!
//! The session execution root is bound to the caller-provided task directory
//! through `ExecutionRootResolver` — the same mechanism that binds native code
//! sessions to a project directory, so the shell/File cwd is the task
//! directory. The resolver closure only recognizes the session id generated
//! for this run and leaves resolution for every other session unchanged.
//!
//! CLI feature parity: `AgenticTaskRequest` additionally carries optional
//! `session_id` (continue an existing chat session), `mode` (Agent/Plan turn
//! mode), `model_id` (per-session model pin), and `attachments` (files staged
//! and ingested through the GUI attachment pipeline). Every field defaults to
//! today's behavior; see the struct docs.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use deepseek_tui::AppMode;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::core::mode_state::SerializableMode;
use crate::features::assistant::attachments::{
    build_message_with_attachments_in_dir, copy_bounded, stage_file_in_workspace_with_copier,
};
use crate::features::assistant::engine_pool::EnginePool;
use crate::features::assistant::product_runtime::headless_bridge::run_windowless_host;
use crate::features::assistant::product_runtime::{
    EnginePoolRuntime, SessionSpec, TurnHandle, TurnInput, TurnResult,
};
use crate::features::files::file_ingest::IngestResult;
use crate::features::sessions::{
    ExecutionRootResolver, MAX_HEADLESS_SESSIONS, RetentionEvictionRecord, SessionKind,
    SessionStore, transcript_revision, validate_user_workspace_path,
};
use crate::platform::prefs::UserPrefs;

/// The engine-facing half of the run teardown, as a contract. The runner uses
/// it to distinguish "reclaim an inspectable session's engine" from "delete
/// the session record"; tests drive the SAME orchestration `run_agentic_task`
/// executes (instead of re-implementing it) by supplying an instrumented
/// implementation. The production implementation is [`EnginePoolRuntime`];
/// pooling in `EnginePool` requires a Tauri `AppHandle` (no mock runtime
/// exists in this repo), which is why the KEEP_SESSION disposition cascade
/// had no reachable test entry before this contract existed.
///
/// Implementations must stay one-line delegations — no behavior may live
/// here, or the contract stops testing the production path:
/// - `teardown_turn_active` → `EnginePoolRuntime::is_turn_active`
/// - `teardown_delete` → `EnginePoolRuntime::close_eval_session_result`
///   (turn gate → engine evict → `store.delete` → forget; the durable delete
///   happens INSIDE this call)
/// - `teardown_schedule_delete` → the timing unregister followed by the
///   adoption-guarded durable delete: under the turn gate the record's
///   factory title is RE-CHECKED, and a record renamed between the
///   disposition's outside-the-gate sample and the delete (a live turn can
///   hold the gate for the turn's whole wall clock) is kept as an adopted
///   session — only the engine is reclaimed. Like the stub twin, the late
///   sweep must NOT be pre-armed here: when the guard keeps the record, a
///   pre-armed sweep would delete `sessions/<id>/` under the kept session.
///   The guarded delete schedules the sweep itself on its own delete and
///   failed-delete outcomes.
/// - `teardown_delete_stub_if_still_empty` → the timing unregister followed
///   by the guarded stub delete: under the turn gate the record's emptiness
///   is RE-CHECKED, and a record that gained messages (a turn admitted
///   between the disposition's outside-the-gate samples and the delete) is
///   kept — only the engine is reclaimed. The late sweep must NOT be
///   pre-armed on this path: when the guard keeps the record, a pre-armed
///   sweep would delete `sessions/<id>/` (timeline, staged attachments,
///   workspace) under a transcript the disposition just decided to keep.
///   The guarded delete schedules the sweep itself on its own delete and
///   failed-delete outcomes.
/// - `teardown_evict` → `EnginePool::evict_bounded` (in-memory engine only;
///   the durable record stays; bounded wait so a live turn cannot withhold
///   the run's report for up to the turn's wall clock)
pub(crate) trait AgenticTeardownExecutor {
    /// Whether an engine turn is running for the session right now.
    fn teardown_turn_active(&self, session_id: &str) -> bool;
    /// Delete the session (record + directory), returning best-effort errors.
    ///
    /// UNCONDITIONAL by design: no adoption or stub guard. Only the test
    /// executor calls this (the production lifecycle routes every lane
    /// through [`Self::teardown_schedule_delete`] /
    /// [`Self::teardown_delete_stub_if_still_empty`], whose gates ARE the
    /// adoption contract). Wiring this into a production lane would silently
    /// regress the round-24/25 adoption guarantees — the trait requires the
    /// method, so the footgun is documented here instead.
    async fn teardown_delete(&self, session_id: &str) -> std::result::Result<(), anyhow::Error>;
    /// Delete the session through the one-shot lane, returning best-effort
    /// errors. Delete-on-entry is deliberate: the caller has already
    /// consulted the adoption and unreadable-record guards, so a transcript
    /// that still wears the factory title is the legacy one-shot contract's
    /// to delete. The gate re-checks the adoption marker under the turn
    /// lock, because a live turn can hold that lock long enough for a GUI
    /// rename to land after the sample.
    async fn teardown_schedule_delete(
        &self,
        session_id: &str,
    ) -> std::result::Result<(), anyhow::Error>;
    /// Stub-cleanup twin of [`AgenticTeardownExecutor::teardown_schedule_delete`]:
    /// the durable delete only fires when the record is still message-free
    /// under the turn gate. Unlike the unconditional arm, no late sweep may
    /// be armed before the guarded delete runs — on the keep outcome the
    /// sweep would destroy the kept session's on-disk directory.
    async fn teardown_delete_stub_if_still_empty(
        &self,
        session_id: &str,
    ) -> std::result::Result<(), anyhow::Error>;
    /// Reclaim the in-memory engine; the durable session record stays.
    async fn teardown_evict(&self, session_id: &str);
}

impl AgenticTeardownExecutor for EnginePoolRuntime {
    fn teardown_turn_active(&self, session_id: &str) -> bool {
        self.is_turn_active(session_id)
    }

    async fn teardown_delete(&self, session_id: &str) -> std::result::Result<(), anyhow::Error> {
        self.close_eval_session_result(session_id).await
    }

    async fn teardown_schedule_delete(
        &self,
        session_id: &str,
    ) -> std::result::Result<(), anyhow::Error> {
        // Timing teardown only — no pre-armed late sweep. The guarded delete
        // schedules the sweep itself when it deletes (or fails to); on the
        // keep outcome a pre-armed sweep would delete `sessions/<id>/`
        // (timeline, staged attachments, workspace) under the kept session.
        crate::features::assistant::timing::unregister_eval_observation(session_id);
        self.delete_headless_session_unless_adopted(session_id)
            .await
    }

    async fn teardown_delete_stub_if_still_empty(
        &self,
        session_id: &str,
    ) -> std::result::Result<(), anyhow::Error> {
        // Timing teardown only — no pre-armed late sweep. The guarded delete
        // schedules the sweep itself when it deletes (or fails to); on the
        // keep outcome a pre-armed sweep would delete `sessions/<id>/`
        // (timeline, staged attachments, workspace) under a kept transcript.
        crate::features::assistant::timing::unregister_eval_observation(session_id);
        self.delete_headless_stub_session(session_id).await
    }

    async fn teardown_evict(&self, session_id: &str) {
        if !self
            .pool
            .evict_bounded(session_id, Duration::from_secs(TEARDOWN_GATE_WAIT_SECS))
            .await
        {
            // No session id here (CodeQL cleartext-logging gate, same as the
            // cleanup-delete failure below): the store's own errors name it.
            // `evict_bounded` reports a failure to acquire the session's turn
            // gate, not necessarily a live turn — a long in-flight submit
            // holds it too.
            super::note_stderr(&format!(
                "[agent-task] engine reclaim skipped: the session's turn gate did not \
                 free up within {TEARDOWN_GATE_WAIT_SECS}s; the session stays inspectable"
            ));
        }
    }
}

const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// Upper bound for `timeout_secs`, mirroring the CLI parse cap: an unclamped
/// `u64` would overflow the internal `Instant + Duration` and panic before any
/// report is produced. The CLI enforces the same cap at parse time.
pub const MAX_TIMEOUT_SECS: u64 = 7 * 24 * 60 * 60;
/// Settle window after cancel: give the engine time to finish persisting;
/// past the window, give up waiting for a full turn result.
const CANCEL_SETTLE_SECS: u64 = 30;

/// The teardown evict's turn-gate wait bound. Same value as
/// `CANCEL_SETTLE_SECS`, but a distinct constant: one bounds how long an
/// interrupt's settle path waits, the other how long a teardown's engine
/// reclaim may hold the turn gate — they move independently.
const TEARDOWN_GATE_WAIT_SECS: u64 = 30;
/// Period of the stderr liveness heartbeat while the turn is running, so
/// harnesses with an output-inactivity watchdog do not kill long tasks.
const HEARTBEAT_SECS: u64 = 10;

// Attachment caps are the same constants `ProductHeadlessBackend` enforces on
// its staged attachments — re-exported under the request-path names so the
// two headless pipelines cannot drift apart silently.
pub(crate) use super::headless_bridge::{
    MAX_STAGED_ATTACHMENT_BYTES as MAX_ATTACHMENT_BYTES, MAX_STAGED_ATTACHMENTS as MAX_ATTACHMENTS,
    MAX_STAGED_ATTACHMENTS_TOTAL_BYTES as MAX_ATTACHMENTS_TOTAL_BYTES,
};

/// Turn mode for one agentic task. Serialized as a snake_case string
/// (`"agent"` / `"plan"`). The default (`None` on the request) is
/// [`AgenticTaskMode::Agent`] — today's behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgenticTaskMode {
    /// Full product Yolo turn (shell/file write access, product tool
    /// allowlist).
    Agent,
    /// Read-only plan turn: the same `Op::SendMessage` mode the GUI produces
    /// for a Plan-mode send (base `tool_setup` switches the sandbox to
    /// ReadOnly and the tool whitelist to the read-only set; the per-turn
    /// Plan reminder is injected by the shared send path).
    Plan,
}

impl AgenticTaskMode {
    fn to_app_mode(self) -> AppMode {
        match self {
            Self::Agent => AppMode::Agent,
            Self::Plan => AppMode::Plan,
        }
    }
}

/// One file attached to an agentic task. `path` is the caller-side source
/// file; it is copied into the run session's ledger `attachments/` directory,
/// ingested through `features/files::file_ingest` (the GUI chip ingest), and
/// rendered into the prompt with the same attachment text the GUI chat send
/// builds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgenticTaskAttachment {
    pub path: PathBuf,
    /// Best-effort removal of the caller-side source file after it has been
    /// staged and ingested. Removal failures are reported on stderr and never
    /// fail the run. Defaults to `false`.
    #[serde(default)]
    pub remove_after_ingest: bool,
}

/// One agentic task's input. `prompt` enters the product send path verbatim
/// (no eval envelope).
///
/// Optional parity fields (`session_id` / `mode` / `model_id` /
/// `attachments`) all default to today's behavior: a request without them
/// runs one fresh Yolo turn on a temporary session exactly as before, and
/// serializing such a request keeps the historical field set (unset optional
/// fields are skipped).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgenticTaskRequest {
    pub prompt: String,
    /// Task working directory; None = session-private directory (the same
    /// isolated scratch as eval sessions). For a FRESH session the directory
    /// is additionally persisted as the session's durable workspace binding.
    /// With `session_id`, the directory scopes this run only — the existing
    /// session's persisted binding is untouched, and a later run without
    /// `workspace` resolves to that session's own binding or scratch again.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Continue an existing chat session instead of creating a new one.
    /// The session must exist in the `SessionStore` and must be an ordinary
    /// chat session (scheduled-run sessions are rejected; ACP sessions do not
    /// live in this store and fail the existence check first). Sessions
    /// persist by default (see [`keep_session_from_env`]); a caller-provided
    /// session is additionally **never** auto-deleted regardless of the
    /// cleanup mode. Budget note: retention buckets on the durable
    /// `agentic_` id prefix — a caller-provided id without that prefix counts
    /// against the GUI chat budget, not the headless budget, so reusing
    /// plain chat ids in batch runs can evict GUI conversations at the chat
    /// cap. Errors: unknown session → `agent_session_not_found`;
    /// non-chat session → `agent_session_not_chat`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Turn mode. `None` = [`AgenticTaskMode::Agent`] = today's behavior.
    /// `Plan` submits the same read-only plan turn the GUI produces in Plan
    /// mode and persists the session mode through the GUI's per-session
    /// lane, so reopening the session restores Plan for fresh and
    /// caller-provided sessions alike (a persistence failure fails the run
    /// before submit — reopening in the stale mode is the unsafe
    /// divergence).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<AgenticTaskMode>,
    /// Pin the session's model, like the GUI per-session model switch:
    /// validated against the configured model list
    /// (`UserPrefs::model_by_id`, the same check the `set_session_model`
    /// command performs), then bound through the eval model-selection route
    /// for a fresh session or the GUI chip-switch path (per-session sidecar
    /// write + engine evict) for an existing session. The chip-switch
    /// binding is written during setup; a setup that fails, or times out
    /// before the submit, puts the session's previous model back, while a
    /// submitted run leaves the pin in force. Unknown model →
    /// `agent_model_not_found`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// Files attached to the prompt, processed by the GUI attachment
    /// pipeline: staged into the session ledger `attachments/` directory,
    /// ingested via `features/files::file_ingest`, and rendered with the same
    /// product attachment text the GUI chat send uses. Limits are the staged
    /// attachment caps shared with `ProductHeadlessBackend`: at most
    /// [`MAX_ATTACHMENTS`] files, at most [`MAX_ATTACHMENT_BYTES`] each.
    /// Missing path →
    /// `agent_attachment_not_found`; over limits →
    /// `agent_attachment_too_many` / `agent_attachment_too_large`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AgenticTaskAttachment>,
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

/// Tool-call summary: names and success flags only, never any
/// arguments/results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgenticToolEvent {
    pub name: String,
    pub failed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgenticUsageReport {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit_tokens: u64,
    pub cache_miss_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub context_window: u64,
}

/// Final report of one agentic task. `assistant_text` is the last turn's
/// assistant text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgenticTaskReport {
    pub session_id: String,
    pub status: String,
    pub timed_out: bool,
    /// Timeout race marker: the turn finished naturally after the deadline
    /// but before the cancel took effect (a full terminal result arrived), so
    /// `status` keeps the engine's real state instead of `timeout`. Graders
    /// can use this to tell "finished, but past the line" from "cancelled".
    #[serde(default)]
    pub completed_after_deadline: bool,
    /// Whether the turn was durably submitted to the engine. The only `Ok`
    /// report that can carry `false` is the setup-timeout arm, and only when
    /// the deadline fired BEFORE the submit was entered — there the turn
    /// provably never ran and the CLI's one-shot persona consume skips, so a
    /// staged body survives for the next run. Once the submit was entered the
    /// outcome is genuinely ambiguous (the awaited future includes the lazy
    /// engine spawn, and the turn may have been durably admitted and keep
    /// running), so the report carries `submitted: true`: the one-shot
    /// consume spends the staged body rather than risking a double injection
    /// on the next run, the same at-least-once direction as every submitted
    /// run.
    #[serde(default)]
    pub submitted: bool,
    pub assistant_text: String,
    pub tool_events: Vec<AgenticToolEvent>,
    pub usage: Option<AgenticUsageReport>,
    pub error: Option<String>,
}

/// Run one agentic task in the windowed Tauri host and return a structured
/// report.
///
/// Host bootstrap reuses
/// [`run_windowless_host`](super::headless_bridge::run_windowless_host) (the
/// same implementation as the eval backend); the only difference is that the
/// work closure receives `EnginePool` + `SessionStore` instead of the eval
/// backend.
pub fn run_agentic_task_headless(request: AgenticTaskRequest) -> Result<AgenticTaskReport> {
    run_windowless_host(|pool, store| run_agentic_task(pool, store, request))
}

/// Drive one agentic turn: validate the request → bind the execution root →
/// pin the model → Yolo/Plan submit → timeout
/// watchdog → collect the report → persist the session (only an explicit
/// `PINVOU3_AGENT_TASK_KEEP_SESSION=0|false|no|off` deletes it). Once the
/// turn is submitted, a report is
/// always returned (internal failures land in the `error` field); setup faults
/// (request validation, model pin, session prepare, submit) propagate as `Err`
/// instead — the CLI surfaces those as exit 1 without a report. A session
/// freshly created by such a failed run is deleted best-effort with the same
/// eval cleanup as `KEEP_SESSION=0`, so no empty placeholder-titled chat is
/// left behind — unless the record was adopted meanwhile (a rename, an
/// admitted user message, or a turn the engine is still running), which
/// keeps. The setup TIMEOUT is not a failed run in this sense: it is the one
/// never-submitted path that still returns an `Ok` report, and a reported
/// `session_id` must stay resolvable — so under the default keep contract
/// its session is kept, and only the explicit one-shot opt-in deletes it
/// (under the same adoption rule as a submitted run).
/// Caller-provided sessions are never auto-deleted.
///
/// Persisting counts against the headless 50-session retention budget — a
/// bucket separate from the GUI chat budget — so a fresh run's prepare-time
/// save evicts only previous headless sessions (pinned sessions are exempt
/// from retention). The store's real eviction events drive a stderr warning
/// counting the evicted non-pinned headless-budget sessions — boot-time
/// sweep evictions included (they buffer in the store and flush on arm) —
/// even when the run errors after the save. One path CAN evict GUI chats: a
/// caller-provided `session_id`
/// without the `agentic_` prefix bills against the chat budget (see
/// [`AgenticTaskRequest::session_id`]); those evictions are deliberately not
/// counted in the warning, so prefer fresh runs against the desktop's
/// default `PINVOU3_HOME`.
///
/// The execution root resolver must be registered before the pool enters an
/// `Arc` (the bridge setter needs `&mut self`), which is why this function
/// takes `EnginePool` by value.
pub async fn run_agentic_task(
    pool: EnginePool,
    store: SessionStore,
    request: AgenticTaskRequest,
) -> Result<AgenticTaskReport> {
    let timeout_secs = request.timeout_secs.clamp(1, MAX_TIMEOUT_SECS);
    validate_attachments(&request.attachments)?;
    refuse_ingest_without_a_surviving_copy(
        &request.attachments,
        keep_session_from_env(),
        request.session_id.is_none(),
    )?;
    if let Some(model_id) = request.model_id.as_deref() {
        ensure_model_exists(model_id)?;
    }
    let existing_session = match request.session_id.as_deref() {
        Some(session_id) => {
            ensure_existing_chat_session(&store, session_id)?;
            true
        }
        None => false,
    };
    let session_id = match request.session_id.as_deref() {
        Some(session_id) => session_id.to_owned(),
        // Regenerate until the id is free (see `mint_fresh_session_id`).
        None => mint_fresh_session_id(
            |id| store.chat_session_record_exists(id),
            fresh_session_id(),
        ),
    };

    // Execution root binding: the closure only matches this run's session id
    // (fresh or caller-provided); resolution for every other session stays
    // unchanged. A caller-provided session is bound to `workspace` only when
    // the request carries one — an unset workspace keeps the session's own
    // resolution chain instead of overriding it: the session's stored
    // working-directory binding when it has one (the GUI bind), else its
    // private scratch. A CLI resume of a GUI-bound session therefore runs
    // in the bound directory.
    //
    // Normalize ONCE and feed the same string to the run-scoped resolver and
    // the durable binding below: a raw `--workspace` and its normalized form
    // are different strings on Windows (`\\?\` verbatim paths), and the
    // binding-keyed gates compare strings on reopen. Failing the validation
    // here (directory gone since the caller checked) fails the run loud
    // before any record is prepared, mirroring the GUI create path.
    let bound_workspace = match request.workspace.as_ref() {
        Some(workspace) => Some(
            validate_user_workspace_path(&workspace.to_string_lossy())
                .context("validate agent workspace binding")?,
        ),
        None => None,
    };
    // The resolver owns its copy (it must be 'static); run_turn persists the
    // same validated string as the durable binding (passed below).
    //
    // It CHAINS to the durable binding rather than replacing it, mirroring
    // the GUI composition root (`lib.rs`, `code_project_workspace(id)
    // .or_else(|| store.session_workspace_binding(id))`). Replacing it would
    // make a caller-provided session that is already bound to a project
    // resolve to `None` here: `SessionStore::session_roots` has its own
    // binding fallback and would still report `bound = true` with the project
    // root, while `Pinvou3Bridge::session_roots` has none and would run the
    // engine, the shell and the prompt's working-directory layer in the
    // session's private scratch. Two answers to one question inside a single
    // run, and the user's files silently untouched.
    // Read once rather than capturing the store: the resolver is stored back
    // INTO the store, so holding a clone of it here would be a reference
    // cycle. Only this run's own session is resolvable, and its binding can
    // only be changed by this run (which is what `bound_workspace` is).
    let durable_binding = store.session_workspace_binding(&session_id);
    let resolver_workspace = bound_workspace.clone();
    let matched_session = session_id.clone();
    let resolver: ExecutionRootResolver = Arc::new(move |id: &str| {
        if id != matched_session {
            return None;
        }
        resolver_workspace
            .clone()
            .or_else(|| durable_binding.clone())
    });
    let mut pool = pool;
    pool.bridge.set_execution_root_resolver(resolver.clone());
    store.set_execution_root_resolver(resolver);
    let runtime = EnginePoolRuntime::new(Arc::new(pool));

    let evictions = arm_retention_eviction_observer(&store);
    let (submitted, outcome) = run_turn(
        &runtime,
        &store,
        &session_id,
        &request,
        bound_workspace,
        timeout_secs,
        existing_session,
    )
    .await;

    // Session lifecycle after the turn — full contract documented on
    // [`run_session_lifecycle`]. The setup timeout is the one
    // never-submitted outcome that produces a report, so it is named at this
    // boundary: its printed `session_id` must stay resolvable (see
    // [`run_session_lifecycle`]).
    let setup_timed_out = !submitted && outcome.is_ok();
    run_session_lifecycle(
        &runtime,
        &store,
        &session_id,
        existing_session,
        submitted,
        setup_timed_out,
    )
    .await;
    // Disarm before reporting: the prepare-time save happened before any setup
    // fault could surface, so the evictions are real regardless of the final
    // outcome — the report may carry an error, and the run may have cleaned
    // its own session up afterwards.
    if let Some(warning) = disarm_retention_eviction_observer(&store, &evictions) {
        super::note_stderr(&warning);
    }
    outcome
}

/// Session lifecycle after the turn — the KEEP_SESSION disposition cascade.
/// Sessions persist by default (GUI parity — the 50-session retention cap
/// applies), so the engine is reclaimed while the transcript, artifacts and
/// timeline stay under the sessions root for later continuation through the
/// request's `session_id` (a library surface; the one-shot CLI keeps its
/// defaults today). Only an explicit
/// `PINVOU3_AGENT_TASK_KEEP_SESSION=0|false|no|off` restores the old one-shot
/// cleanup for harnesses that want a clean sandbox (the legacy truthy values
/// "1"/"true"/"yes"/"on" keep meaning keep).
///
/// A fresh session whose turn never started through a setup FAULT (an `Err`
/// outcome: no report was produced, so nothing ever handed this id to the
/// caller) carries no transcript to
/// inspect: keeping it would
/// litter the shared store — and the GUI history — with zero-message stubs,
/// one eviction apiece in a failing batch. Those runs clean up after
/// themselves regardless of `KEEP_SESSION` — where "never started" is decided
/// on the durable record: a stub that already carries admitted messages is a
/// started transcript and stays inspectable instead (see
/// [`never_started_disposition`]), and a record a GUI user renamed is
/// adopted and keeps on every path (a rename is ownership).
///
/// The setup TIMEOUT is different: it is the one never-submitted path that
/// returns an `Ok` report, and that report carries the run's `session_id` —
/// so under the default keep contract its session is kept and the reported
/// id stays resolvable (no silent stub cleanup). Only the explicit one-shot
/// opt-in may still clean it up, under the same adoption rule as every
/// submitted run.
///
/// A caller-provided `session_id` (existing_session) is never auto-deleted by
/// THIS run, but it is an ordinary chat session in the store: the 50-session
/// retention sweep can still evict it later exactly like any GUI chat
/// session. Only this run's eval observation mark is dropped, and the session
/// is left in place for the caller.
///
/// Extracted behind [`AgenticTeardownExecutor`] so the disposition matrix is
/// pinned by tests driving this exact orchestration (`run_agentic_task`
/// itself needs an `EnginePool`, which needs a Tauri `AppHandle`).
async fn run_session_lifecycle(
    executor: &impl AgenticTeardownExecutor,
    store: &SessionStore,
    session_id: &str,
    existing_session: bool,
    submitted: bool,
    setup_timed_out: bool,
) {
    let keep_session = keep_session_from_env();
    if existing_session {
        crate::features::assistant::timing::unregister_eval_observation(session_id);
        // Reclaim the engine like the fresh+keep branch below. The process
        // exits right after this (`run_windowless_host` calls `exit(0)`), and
        // without the reclaim there is no shell-scope finalize, no Shutdown
        // and no forwarder drain: background shell jobs the turn started are
        // orphaned onto the user's machine, and a pending ledger/artifact
        // write is dropped mid-flight. The session record is untouched —
        // eviction only tears down the in-memory engine.
        executor.teardown_evict(session_id).await;
        return;
    }
    // The setup timeout produced a report that names this session: under the
    // default keep contract the record must stay resolvable, so the engine
    // alone is reclaimed. Under the one-shot opt-in the caller asked for a
    // clean sandbox and `one_shot_cleanup_decision` applies unchanged — the
    // same adoption and unreadable-record guards as for a submitted run.
    if setup_timed_out {
        crate::features::assistant::timing::unregister_eval_observation(session_id);
        if keep_session {
            executor.teardown_evict(session_id).await;
            return;
        }
        match one_shot_cleanup_decision(
            store.chat_session_has_messages(session_id).map_err(|_| ()),
            store
                .chat_session_factory_titled(session_id)
                .map_err(|_| ()),
        ) {
            FreshSessionDisposition::Cleanup => {
                if let Err(error) = executor.teardown_schedule_delete(session_id).await {
                    super::note_stderr(&format!(
                        "[agent-task] cleanup delete failed: {}",
                        error.root_cause()
                    ));
                }
            }
            FreshSessionDisposition::Keep => executor.teardown_evict(session_id).await,
        }
        return;
    }
    // The submit boundary is not atomic with transcript admission: the engine
    // lazily spawns on submit and can durably admit the user message before
    // the fault surfaces (a submit error, or the setup timeout landing right
    // after admission). A record that carries messages has therefore started
    // — its transcript is the only copy, so it stays inspectable like any
    // submitted run unless the caller explicitly opted back into the legacy
    // one-shot cleanup. Only a truly zero-message stub is cleanup-eligible
    // regardless of `KEEP_SESSION`; an unloadable record also keeps (deleting
    // on unknown state is the unsafe direction).
    if !submitted {
        crate::features::assistant::timing::unregister_eval_observation(session_id);
        // A rename away from the factory title is adoption — the one-shot
        // lane must not delete a record a GUI user took over. An unreadable
        // record is not proven factory-titled and keeps for the same reason.
        let factory_titled = store
            .chat_session_factory_titled(session_id)
            .map_err(|_| ());
        match never_started_disposition(
            store.chat_session_has_messages(session_id).map_err(|_| ()),
            executor.teardown_turn_active(session_id),
            keep_session,
            factory_titled,
        ) {
            NeverStartedDisposition::CleanupStub => {
                // Best-effort delete: a failure must not mask the run's own
                // outcome, but silently stranding the session hides it from
                // the operator. Only the root cause is printed — the full
                // `{error:#}` chain would carry the session id into
                // persistent logs (CodeQL cleartext-logging gate); the
                // store's own error chain already names the session.
                // The guarded twin re-checks emptiness under the turn gate:
                // the samples above were taken outside it, so a turn admitted
                // in between lands as a kept started transcript, not a
                // deleted stub.
                if let Err(error) = executor
                    .teardown_delete_stub_if_still_empty(session_id)
                    .await
                {
                    super::note_stderr(&format!(
                        "[agent-task] cleanup delete failed: {}",
                        error.root_cause()
                    ));
                }
            }
            NeverStartedDisposition::LegacyCleanupStarted => {
                // The legacy one-shot contract deletes a started transcript
                // deliberately — but only one that still wears the factory
                // title: the disposition has already classified a renamed
                // record as adopted and kept it.
                if let Err(error) = executor.teardown_schedule_delete(session_id).await {
                    super::note_stderr(&format!(
                        "[agent-task] cleanup delete failed: {}",
                        error.root_cause()
                    ));
                }
            }
            NeverStartedDisposition::KeepInspectable => {
                executor.teardown_evict(session_id).await;
            }
        }
        return;
    }
    crate::features::assistant::timing::unregister_eval_observation(session_id);
    if keep_session {
        executor.teardown_evict(session_id).await;
    } else {
        // One-shot opt-in on a submitted run: the legacy contract deletes the
        // run's session — unless it was adopted (renamed away from the
        // factory title, which means a GUI user owns it now) or its state is
        // unreadable (deleting on unknown state is the unsafe direction).
        match one_shot_cleanup_decision(
            store.chat_session_has_messages(session_id).map_err(|_| ()),
            store
                .chat_session_factory_titled(session_id)
                .map_err(|_| ()),
        ) {
            FreshSessionDisposition::Cleanup => {
                if let Err(error) = executor.teardown_schedule_delete(session_id).await {
                    super::note_stderr(&format!(
                        "[agent-task] cleanup delete failed: {}",
                        error.root_cause()
                    ));
                }
            }
            FreshSessionDisposition::Keep => executor.teardown_evict(session_id).await,
        }
    }
}

/// Mint a headless session id that is free in `exists`'s store. Fresh ids
/// persist for good now, so a recycled pid replaying the same counter must
/// not silently overwrite a kept session's record (transcript loss, and the
/// old pin would transfer to the new stub): regenerate until the id is free.
/// The seed id comes from [`fresh_session_id`].
fn mint_fresh_session_id(exists: impl Fn(&str) -> bool, mut session_id: String) -> String {
    while exists(&session_id) {
        session_id = fresh_session_id();
    }
    session_id
}

/// `PINVOU3_AGENT_TASK_KEEP_SESSION`: sessions are kept by default; only the
/// explicit falsy values restore the legacy one-shot cleanup. An unset or
/// empty variable both mean keep; values are compared ASCII
/// case-insensitively without trimming (so `"0 "` with trailing whitespace
/// counts as keep).
fn keep_session_from_env() -> bool {
    match std::env::var("PINVOU3_AGENT_TASK_KEEP_SESSION") {
        Ok(value) => !matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// Lifecycle decision for a fresh session whose turn never submitted: the
/// branch order decides between deleting the run's own stub and keeping it,
/// so it is pinned as a pure disposition instead of living only inline in
/// the run teardown.
///
/// - `Ok(false)` — a truly zero-message stub: cleanup-eligible regardless of
///   `KEEP_SESSION` (it would litter the shared store with eviction bait).
/// - `Ok(true)` — the durable record already carries admitted messages (the
///   engine lazily spawned mid-submit): a started transcript, the only copy,
///   stays inspectable like any submitted run.
/// - `Err(_)` — unloadable record: keep (deleting on unknown state is the
///   unsafe direction).
///
/// `engine_active` overrides a `Ok(false)` sample. The disk snapshot is
/// written by the engine's forwarder some time AFTER admission, so a submit
/// that failed late — or a setup deadline that fired while the op was already
/// queued — can read "no messages" for a turn the engine is at that moment
/// running and billing. The engine's own liveness is the authority on whether
/// a turn started; the file only says whether the first write has landed yet.
/// Residual: a turn admitted AFTER both samples (forwarder lag can outlast
/// them both) still classifies as a stub here — the disposition cannot see
/// it. The guarded stub delete NARROWS that window at the action level, not
/// by this pure function: it re-checks emptiness under the turn gate and
/// keeps a record whose first write has landed by then. A turn whose
/// forwarder write is still in flight at that re-check is still deleted with
/// its engine reclaimed — that last sliver is bounded by the forwarder's own
/// lag, not closed.
///
/// `keep_session` only splits the started case: with the legacy one-shot
/// opt-in (`KEEP_SESSION=0|false|no|off`), a started-but-unsubmitted run is
/// still cleaned up per the old contract — but only if the record still
/// wears the factory title. A rename is ownership: a GUI user who renamed
/// the session (or whose first send already triggered the auto-rename)
/// adopted it, and `factory_titled != Ok(true)` keeps it on the delete lane
/// too — the stub arm above applies the same rule, so an adopted record
/// keeps on EVERY never-started path. An unreadable record is not proven
/// factory-titled and keeps for the same reason.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NeverStartedDisposition {
    CleanupStub,
    KeepInspectable,
    LegacyCleanupStarted,
}

fn never_started_disposition(
    has_messages: Result<bool, ()>,
    engine_active: bool,
    keep_session: bool,
    factory_titled: Result<bool, ()>,
) -> NeverStartedDisposition {
    let started = match has_messages {
        Ok(has) => has || engine_active,
        // Unloadable record: keep (deleting on unknown state is the unsafe
        // direction), and the legacy opt-in must not override that.
        Err(_) => return NeverStartedDisposition::KeepInspectable,
    };
    match (started, keep_session) {
        // A stub is the run's to delete only while it still wears the
        // factory title: a rename is ownership on this lane too, the same
        // rule the started lane below and the one-shot decision apply. An
        // unreadable title is not proven factory-titled and keeps. Both
        // facts are re-checked UNDER the turn gate at the guarded delete
        // (`DeleteGateRecheck::StillAStub`), so a rename or a first message
        // landing in the sample→delete window keeps the record; the
        // residual window is only the store read racing the rename inside
        // the gate, the same cross-process store race the delete lane's
        // docs already disclose.
        (false, _) if factory_titled == Ok(true) => NeverStartedDisposition::CleanupStub,
        (false, _) => NeverStartedDisposition::KeepInspectable,
        (true, true) => NeverStartedDisposition::KeepInspectable,
        // The one-shot lane deletes a started transcript only while it still
        // wears the factory title; adopted (renamed) and unreadable records
        // are not the run's to delete.
        (true, false) if factory_titled == Ok(true) => {
            NeverStartedDisposition::LegacyCleanupStarted
        }
        (true, false) => NeverStartedDisposition::KeepInspectable,
    }
}

/// The pre-run mode state a failed setup must put back: either the durable
/// value the session had, or absence (the session was following its resolved
/// default, so the failed Plan persist added an entry it must not keep).
enum PlanModeRestore {
    Value(SerializableMode),
    Absent,
}

impl PlanModeRestore {
    fn apply(self, store: &SessionStore, session_id: &str) -> Result<()> {
        match self {
            PlanModeRestore::Value(mode) => store.set_mode_and_persist(session_id, mode),
            PlanModeRestore::Absent => store.clear_mode_and_persist(session_id),
        }
    }
}

/// The disposition of the one-shot delete lane: a run-owned session is
/// deleted, everything else (adopted, unreadable) is kept.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FreshSessionDisposition {
    Cleanup,
    Keep,
}

/// Lifecycle decision for the one-shot opt-in
/// (`PINVOU3_AGENT_TASK_KEEP_SESSION=0|false|no|off`) on a FRESH session whose
/// run produced a report — a completed turn, an in-turn error, or the setup
/// timeout. The legacy contract deletes the run's session, with two keeps:
///
/// - `factory_titled != Ok(true)` — a rename is ownership (the adoption
///   exception, the same rule the never-started path applies): a record a
///   GUI user took over mid-run is not the run's to delete, and an
///   unreadable title is not proven factory-titled.
/// - `Err(_)` — unknown state: keep, because deleting on unknown state is
///   the unsafe direction.
///
/// Kept as a named decision rather than an inline `if` so the cases that are
/// NOT a delete stay pinned by a test; they are the whole reason the caller
/// spends store reads it otherwise has no use for.
fn one_shot_cleanup_decision(
    record_readable: Result<bool, ()>,
    factory_titled: Result<bool, ()>,
) -> FreshSessionDisposition {
    match (record_readable, factory_titled) {
        (Ok(_), Ok(true)) => FreshSessionDisposition::Cleanup,
        _ => FreshSessionDisposition::Keep,
    }
}

/// The panic arm of the setup await, extracted verbatim so its SEQUENCING —
/// gate on `submit_entered`, run BOTH pin restores to completion, and only
/// then resume the unwind — is pinned by a test that drives a real panicking
/// future through it (round-25 review: the previous test called the restore
/// helper directly, so deleting this arm or dropping its gate left the whole
/// suite green). A panic past the submit boundary must NOT roll the pins
/// back: the turn may already be admitted, and a mid-turn pin rollback would
/// evict the engine under it — there the pins stay, like those of any
/// submitted run.
async fn resume_unwind_after_pinned_restore<Fut>(
    submit_entered: &AtomicBool,
    panic_payload: Box<dyn std::any::Any + Send>,
    restore: impl FnOnce(&'static str) -> Fut,
) -> !
where
    Fut: std::future::Future<Output = ()>,
{
    if !submit_entered.load(Ordering::SeqCst) {
        // Best-effort: a failing restore must not mask the panic (the
        // unwind is resumed regardless).
        restore("panic").await;
    }
    std::panic::resume_unwind(panic_payload)
}

/// Put a caller-provided session's `--mode plan` and `--model` pins back after
/// a setup that provably never reached the turn: a setup failure, or a setup
/// timeout that fired before the submit was entered. Nothing ran, so leaving
/// the user's GUI session repinned would be a permanent change made by a run
/// that did no work. Best-effort — a failed restore must not mask the setup
/// error or timeout report being returned, but it is worth a stderr note.
/// `phase` names the setup outcome in that note.
async fn restore_pre_run_pins(
    runtime: &EnginePoolRuntime,
    store: &SessionStore,
    session_id: &str,
    plan: Option<PlanModeRestore>,
    model: Option<Option<String>>,
    phase: &str,
) {
    restore_plan_mode(store, session_id, plan, phase);
    let Some(previous) = model else {
        return;
    };
    if let Err(error) = runtime
        .pool
        .switch_session_model(session_id, previous)
        .await
    {
        super::note_stderr(&format!(
            "[pinvou agent run] warning: failed to restore the pre-run session model \
             after the setup {phase}: {}",
            error.root_cause()
        ));
    }
}

/// Whether a failed submit may have durably admitted the user message: the
/// caller-provided session's pins must then stay (the turn owns the session,
/// like the timeout arm past `submit_entered`), and only a provably
/// pre-append failure restores them. A failure before `submit_entered` was
/// set (attachment staging, session roots — the slow, commonly failing setup
/// steps) provably never polled the submit, so nothing can have been
/// admitted and the pins restore even though no pre-submit snapshot exists
/// yet. Past that gate the decision comes from record facts — the transcript
/// revision snapshotted just before the submit against the record read after
/// the error — because the submit boundary itself is not atomic with the
/// append: the engine lazily spawns on submit and appends the user message
/// before several later fallible steps (reservation liveness, engine spawn,
/// the send itself). A missing snapshot or an unloadable record means
/// unknown, and unknown keeps the pins: restoring over an admitted turn is
/// the unsafe direction.
///
/// The `run_turn` Err arm is the only production caller; this helper is split
/// out (like the panic arm's `resume_unwind_after_pinned_restore`) so the
/// decision is unit-pinnable against a real store without an
/// `EnginePoolRuntime`.
fn submit_err_admitted_turn(
    store: &SessionStore,
    session_id: &str,
    submit_entered: bool,
    pre_submit_revision: Option<&str>,
) -> bool {
    if !submit_entered {
        // The setup failed before the submit was ever polled, so no turn can
        // have been admitted — regardless of the missing snapshot.
        return false;
    }
    let Some(pre) = pre_submit_revision else {
        return true;
    };
    match store.load(session_id) {
        Ok(record) => transcript_revision(&record.messages)
            .map(|now| now != pre)
            .unwrap_or(true),
        // Unloadable record: unknown state; restoring pins over a possibly
        // admitted turn is the unsafe direction (the same rule the
        // never-started disposition applies to deletes).
        Err(_) => true,
    }
}

/// The mode half of [`restore_pre_run_pins`], split out because it needs no
/// engine and is pinned by a store-level test.
fn restore_plan_mode(
    store: &SessionStore,
    session_id: &str,
    restore: Option<PlanModeRestore>,
    phase: &str,
) {
    let Some(restore) = restore else {
        return;
    };
    if let Err(error) = restore.apply(store, session_id) {
        super::note_stderr(&format!(
            "[pinvou agent run] warning: failed to restore the pre-run session mode \
             after the setup {phase}: {}",
            error.root_cause()
        ));
    }
}

/// `Some` copy when the prepare-time save evicted unpinned headless sessions
/// at the retention cap — whether through the save itself or the host's
/// boot-time sweep — and `None` when nothing was evicted (stay silent). The
/// decision deliberately does not consult the turn outcome — the deletions
/// happened before any setup fault could surface, so the evictions are real
/// however the run ends; taking no outcome parameter is what keeps that
/// invariant structural instead of a code path that can regress behind an
/// `is_ok()` gate. The store only forwards headless-budget deletions to the
/// observer (chat-budget deletions by the same sweep are the chat budget's
/// own enforcement), so the count below is exactly headless evictions. The
/// copy stays neutral about who triggered the sweep: a boot-time eviction
/// predates this run and must not be blamed on it.
fn retention_eviction_warning(evicted: &RetentionEvictionRecord) -> Option<String> {
    (evicted.total > 0).then(|| {
        format!(
            "[pinvou agent run] warning: the session store's retention sweep \
             evicted {} non-pinned session(s) at the \
             {MAX_HEADLESS_SESSIONS}-session headless retention cap (pinned \
             sessions are exempt; the desktop app's own chat sessions live on \
             a separate budget). Point PINVOU3_HOME at a \
             sandbox or prune the session store (PINVOU3_AGENT_TASK_KEEP_\
             SESSION=0 only removes this run's session afterwards; the \
             eviction at the cap still happens).",
            evicted.total
        )
    })
}

/// Arm the store's retention-eviction observer for one run: the prepare-time
/// save lands in the same 50-session store the GUI reads, and a fresh save at
/// the cap evicts the oldest unpinned headless session(s) (pinned sessions
/// are exempt). The store reports its real sweep deletions into the returned
/// receiver, so the warning keys on the eviction event itself — a run that
/// errors after the save (attachment staging, submit) still surfaces the
/// eviction, and a run that fails before saving evicts nothing and stays
/// silent. A count sampled around the run cannot see mid-run forwarder
/// evictions; the store's own deletions can.
fn arm_retention_eviction_observer(store: &SessionStore) -> Arc<Mutex<RetentionEvictionRecord>> {
    let evictions = Arc::new(Mutex::new(RetentionEvictionRecord::default()));
    if let Some(stale) = store.set_retention_eviction_observer(Some(evictions.clone())) {
        // Single-flight normally guarantees the slot is empty here; a stale
        // observer means an earlier run skipped its disarm (an unwind between
        // arm and disarm would do it). Its record was never reported (and may
        // be empty) — say so instead of silently adopting a dead receiver.
        drop(stale);
        super::note_stderr(
            "[pinvou agent run] warning: replaced a stale retention-eviction \
             observer; any eviction record the previous run left unreported \
             was discarded",
        );
    }
    evictions
}

/// Disarm the observer and decide the warning for the recorded evictions.
/// Deliberately outcome-independent (see [`retention_eviction_warning`]):
/// extraction keeps the arm/disarm pairing and the warn-on-nonempty-decision
/// pinned separately from `run_agentic_task`, which cannot enter tests (it
/// needs an `EnginePool` → Tauri `AppHandle`). The caller owns printing.
fn disarm_retention_eviction_observer(
    store: &SessionStore,
    evictions: &Arc<Mutex<RetentionEvictionRecord>>,
) -> Option<String> {
    store.take_retention_eviction_observer();
    retention_eviction_warning(&evictions.lock())
}

/// Validate the static attachment limits of an agentic request: at most
/// [`MAX_ATTACHMENTS`] entries, each resolving to a regular file of at most
/// [`MAX_ATTACHMENT_BYTES`] bytes (the staged attachment caps shared with
/// `ProductHeadlessBackend`). Symlinks to regular files are accepted,
/// matching the GUI staging path (`stage_file_in_workspace` copies content).
/// Refuses `remove_after_ingest` when the run is also configured to delete its
/// own session afterwards, because together they destroy both copies of the
/// caller's file.
///
/// `remove_after_ingest` is safe only because the staged copy outlives the
/// call: staging writes into `sessions/<id>/workspace/attachments/`, and the
/// source is unlinked *after* submit returns, so the file still exists inside
/// a transcript the caller can open. Under the legacy one-shot cleanup
/// (`PINVOU3_AGENT_TASK_KEEP_SESSION` falsy — the setting batch runs are told
/// to use) that premise is false for a FRESH session: the
/// `submitted && !keep_session` arm `remove_dir_all`s the whole session
/// directory, taking the staged copy with it moments after the source was
/// deleted. The file is then gone from both places, unrecoverably. A
/// caller-provided session is never auto-deleted by the run, so its staged
/// copy always survives and the refusal does not apply to it.
///
/// The two flags express contradictory intents — "hand this file over and keep
/// only your copy" versus "keep nothing" — so this refuses up front rather
/// than picking one silently. Checked before anything is staged or deleted.
fn refuse_ingest_without_a_surviving_copy(
    attachments: &[AgenticTaskAttachment],
    keep_session: bool,
    fresh_session: bool,
) -> Result<()> {
    if keep_session || !fresh_session {
        return Ok(());
    }
    if let Some(attachment) = attachments.iter().find(|a| a.remove_after_ingest) {
        anyhow::bail!(
            "agent_attachment_ingest_would_lose_the_file: '{}' sets remove_after_ingest, but \
             this run creates its own session and PINVOU3_AGENT_TASK_KEEP_SESSION is falsy, so \
             the run deletes that session (and the staged copy) right after the turn — the \
             source and the copy would both be destroyed. Drop remove_after_ingest, or let the \
             session persist.",
            attachment.path.display()
        );
    }
    Ok(())
}

fn validate_attachments(attachments: &[AgenticTaskAttachment]) -> Result<()> {
    if attachments.len() > MAX_ATTACHMENTS {
        anyhow::bail!(
            "agent_attachment_too_many: at most {MAX_ATTACHMENTS} attachments are supported (got {})",
            attachments.len()
        );
    }
    let mut total_bytes = 0_u64;
    for attachment in attachments {
        let metadata = std::fs::metadata(&attachment.path).with_context(|| {
            format!(
                "agent_attachment_not_found: attachment {} does not exist",
                attachment.path.display()
            )
        })?;
        if !metadata.is_file() {
            anyhow::bail!(
                "agent_attachment_not_found: attachment {} is not a regular file",
                attachment.path.display()
            );
        }
        if metadata.len() > MAX_ATTACHMENT_BYTES {
            anyhow::bail!(
                "agent_attachment_too_large: attachment {} is {} bytes (limit {MAX_ATTACHMENT_BYTES})",
                attachment.path.display(),
                metadata.len()
            );
        }
        total_bytes += metadata.len();
    }
    // Same aggregate budget as `ProductHeadlessBackend`
    // (MAX_ATTACHMENTS_TOTAL_BYTES = 100 MiB) — the per-file cap alone
    // allowed 320 MiB of staged attachments.
    if total_bytes > MAX_ATTACHMENTS_TOTAL_BYTES {
        anyhow::bail!(
            "agent_attachment_too_large: attachments total {total_bytes} bytes (limit \
             {MAX_ATTACHMENTS_TOTAL_BYTES})"
        );
    }
    Ok(())
}

/// Stage-time cap enforcement for the attachment staging loop: re-reads the
/// source's size right before the copy and returns the running total,
/// refusing sources that grew past the per-file cap after validation or
/// pushed the aggregate past the total budget. The caps must bind the STAGED
/// size: the sources are caller-owned and can grow or be swapped between
/// validation and staging.
fn ensure_stage_size(path: &std::path::Path, staged_total: u64) -> Result<u64> {
    let staged_bytes = std::fs::metadata(path)
        .with_context(|| {
            format!(
                "agent_attachment_not_found: attachment {} vanished before staging",
                path.display()
            )
        })?
        .len();
    if staged_bytes > MAX_ATTACHMENT_BYTES {
        anyhow::bail!(
            "agent_attachment_too_large: attachment {} is {staged_bytes} bytes at \
             staging time (limit {MAX_ATTACHMENT_BYTES})",
            path.display()
        );
    }
    let total = staged_total + staged_bytes;
    if total > MAX_ATTACHMENTS_TOTAL_BYTES {
        anyhow::bail!(
            "agent_attachment_too_large: attachments total {total} bytes at \
             staging time (limit {MAX_ATTACHMENTS_TOTAL_BYTES})"
        );
    }
    Ok(total)
}

/// Validate a caller-provided model id against the configured model list —
/// the same `UserPrefs::model_by_id` check the GUI `set_session_model` command
/// performs before switching a session's model.
fn ensure_model_exists(model_id: &str) -> Result<()> {
    if UserPrefs::load().model_by_id(model_id).is_none() {
        anyhow::bail!("agent_model_not_found: model '{model_id}' is not configured");
    }
    Ok(())
}

/// Validate a caller-provided session target: it must exist in the store and
/// be an ordinary chat session. Scheduled-run sessions have their own
/// automation authority and never host external agentic turns; native code
/// sessions are rejected on their durable sidecar (see below) because this
/// host cannot resolve their scope; ACP sessions do not live in this store
/// and fail the existence check first.
fn ensure_existing_chat_session(store: &SessionStore, session_id: &str) -> Result<()> {
    if !store.chat_session_record_exists(session_id) {
        anyhow::bail!("agent_session_not_found: session '{session_id}' does not exist");
    }
    // The record exists on disk but could not be loaded: report the real
    // fault. A corrupt or unreadable transcript is a different problem from
    // a missing session, and "does not exist" would misdirect the harness
    // operator (the safe direction is unchanged: both fail loud).
    if let Err(error) = store.load(session_id) {
        anyhow::bail!(
            "agent_session_unreadable: session '{session_id}' exists but could not be loaded: {error:#}"
        );
    }
    // A native code session is an ordinary `sessions/<id>.json` record with a
    // `code-session.json` sidecar, so `session_kind` classifies it as Chat.
    // It must still be refused here: this host registers no code-session
    // predicate, so the bridge would resolve the session to the permissive
    // `Plain` scope for both the tool allowlist and the execpolicy deny
    // ruleset, and run the code lane's transcript under the plain
    // instruction layer. Running someone's code session with the wrong
    // consent scope is worse than refusing it.
    if store.has_code_session_marker(session_id) {
        anyhow::bail!(
            "agent_session_not_chat: session '{session_id}' is a native code session; \
             headless runs cannot resolve its scope"
        );
    }
    match store.session_kind(session_id)? {
        SessionKind::Chat => Ok(()),
        SessionKind::ScheduledRun => anyhow::bail!(
            "agent_session_not_chat: session '{session_id}' is a scheduled-run session"
        ),
    }
}

/// Run prepare → submit → wait, and report. `true` in the first tuple slot
/// means the turn was actually submitted: everything after submit (cancel,
/// wait, report building) belongs to a session whose transcript exists, while
/// `false` marks the never-started cases (attachment staging, submit failure,
/// a post-prepare setup fault, setup timeout) the caller's lifecycle handling
/// cleans up as stubs.
#[allow(clippy::too_many_arguments)]
/// The setup-timeout report. `submit_entered` decides `submitted` AND the
/// error text: before the submit the turn provably never ran (setup-only
/// wording); past that boundary the submit was in flight when the deadline
/// fired — the turn may have been durably admitted and keep running — so the
/// report claims submitted and names the ambiguity instead of the provable
/// never-ran wording. Extracted so the flag-derivation (the field the CLI's
/// one-shot persona consume keys off) is unit-pinned rather than buried in
/// the deadline arm.
fn setup_timeout_report(session_id: &str, submit_entered: bool) -> AgenticTaskReport {
    let error = if submit_entered {
        "agentic run did not finish within the timeout; the turn was being submitted when the \
         deadline fired and its outcome is unknown"
    } else {
        "agentic session setup did not finish within the timeout"
    };
    AgenticTaskReport {
        session_id: session_id.to_owned(),
        status: "timeout".to_string(),
        timed_out: true,
        completed_after_deadline: false,
        submitted: submit_entered,
        assistant_text: String::new(),
        tool_events: Vec::new(),
        usage: None,
        error: Some(error.to_string()),
    }
}

async fn run_turn(
    runtime: &EnginePoolRuntime,
    store: &SessionStore,
    session_id: &str,
    request: &AgenticTaskRequest,
    workspace_binding: Option<std::path::PathBuf>,
    timeout_secs: u64,
    existing_session: bool,
) -> (bool, Result<AgenticTaskReport>) {
    // Model selection: a fresh session without an explicit model keeps
    // today's behavior — the active evaluation model is captured, pinned, and
    // released by the guard's Drop. A caller-provided `model_id` replaces
    // that pin (validated up front), and an existing session keeps its own
    // per-session model binding (the GUI chat send semantics).
    // These failures happen before prepare, so no session record exists and
    // the run reports `(false, Err)`.
    let suite_guard = if !existing_session && request.model_id.is_none() {
        let guard = match runtime
            .capture_eval_suite_model()
            .context("active evaluation model is not configured")
        {
            Ok(guard) => guard,
            Err(error) => return (false, Err(error)),
        };
        Some(guard)
    } else {
        None
    };
    let model_selection = if let Some(guard) = suite_guard.as_ref() {
        match guard.derive_case_selection() {
            Ok(selection) => Some(selection),
            Err(error) => return (false, Err(error)),
        }
    } else if !existing_session {
        // Fresh session with an explicit model: bind it through the same eval
        // selection route `prepare_eval_session` consumes.
        match request
            .model_id
            .as_deref()
            .map(|model_id| runtime.pin_eval_model_selection(model_id))
            .transpose()
        {
            Ok(selection) => selection,
            Err(error) => return (false, Err(error)),
        }
    } else {
        None
    };
    let app_mode = request.mode.unwrap_or(AgenticTaskMode::Agent).to_app_mode();
    // The deadline covers the whole task, including session prepare/submit:
    // a hang in either phase must still produce a timeout report, never an
    // unbounded wait.
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    // The mode and model pins below persist BEFORE the turn exists, so a
    // caller-provided session must get back the values the user left it with
    // when the setup then fails (attachment staging, or a submit that failed
    // before its durable append — [`submit_err_admitted_turn`] tells the two
    // apart) or times out before the submit. Once the submit is entered the
    // turn may own the session and the pins stay, exactly like a GUI Plan
    // send. Fresh sessions arm neither: the record was created by this run,
    // so there is no pre-run state a user chose (and on the `Err` paths the
    // stub cleanup deletes the whole record, mode sidecar included).
    let mut plan_restore: Option<PlanModeRestore> = None;
    // The model half: `--model` rewrites the session's durable model sidecar
    // before the turn exists, and a run that never happened must not leave
    // the session permanently repinned.
    let mut model_restore: Option<Option<String>> = None;
    // The staged attachment copies, filled by the setup future once staging
    // lands. Read by the failure arms after the future is dropped: an
    // existing session's copies must be swept when the turn provably never
    // started, or they orphan under the caller's workspace.
    let mut staged_attachment_copies: Vec<std::path::PathBuf> = Vec::new();
    // The staging task's join handle, owned HERE — outside the deadline-
    // scoped setup future — and parked by `prompt_with_attachments` through
    // the mutable borrow below. A deadline firing while staging is in flight
    // drops the setup future at its staging await; a handle owned by that
    // future would detach the blocking task, whose writes then land AFTER
    // the teardown disposition (re-creating a deleted session directory, or
    // stranding unreferenced copies inside a kept one). With the handle
    // parked here, the arms that drop the setup join the task first (see
    // `join_staged_attachments`), so the sweep sees the real copies and no
    // write outlives the disposition.
    let mut staging_in_flight: Option<tokio::task::JoinHandle<anyhow::Result<StagedBatch>>> = None;
    // Set immediately before `runtime.submit`; read by the timeout arm to tell
    // "the deadline hit while staging attachments" (nothing ran, restore the
    // pins) from "the deadline hit around the submit" (a turn may have been
    // admitted, leave them).
    let submit_entered = AtomicBool::new(false);
    // Transcript revision of a caller-provided session, captured inside the
    // setup just before the submit. The Err arm compares it against the
    // post-error record: the durable user-message append inside the submit is
    // the only write this run can put on the messages between the two reads,
    // so a changed revision proves the turn was admitted. `None` (fresh
    // session, or the pre-submit read failed) means "no snapshot" — the Err
    // arm must then assume admission.
    let mut pre_submit_revision: Option<String> = None;
    let setup = async {
        if existing_session {
            // Continue the caller's chat session: never re-create it (session
            // creation would overwrite the transcript). A model pin goes
            // through the GUI chip-switch path (per-session sidecar write +
            // engine evict); the engine itself lazily spawns on submit,
            // exactly like a GUI send. The sidecar write lands during this
            // setup, so the previous model is remembered for the restore.
            if let Some(model_id) = request.model_id.as_deref() {
                // The DURABLE sidecar, not the boot-time cache: the cache is
                // this process's startup view, so a GUI model switch that
                // landed after boot would be silently reverted by the
                // restore below — the exact stale-restore divergence
                // `durable_mode_entry` prevents for the mode restore beside
                // this one.
                let previous = store.durable_session_model_id(session_id);
                runtime
                    .pool
                    .switch_session_model(session_id, Some(model_id.to_owned()))
                    .await
                    .context("pin session model")?;
                if previous.as_deref() != Some(model_id) {
                    model_restore = Some(previous);
                }
            }
            // Mirror the fresh branch below: an explicit Plan request must
            // persist on a caller-provided session too, or the session
            // reopens in its stale mode (the unbound default is Yolo) — the
            // same unsafe divergence the fresh branch refuses. Failure is
            // fatal like the fresh branch. The previous mode is remembered so
            // a setup that never reaches the turn can put the caller's
            // session back the way they left it.
            if matches!(request.mode, Some(AgenticTaskMode::Plan)) {
                // Capture the DURABLE entry, not `mode_state`'s resolved
                // fallback: the fallback is process-relative (this headless
                // process installs no code-session predicate, so it would
                // resolve Yolo for a code session the GUI defaults to Plan),
                // and pinning that misresolution durably is the exact unsafe
                // reopen divergence the persist below prevents. A session
                // with no durable entry restores to absent — re-persisting a
                // resolved default would freeze a value the session was only
                // borrowing.
                let previous = store.durable_mode_entry(session_id);
                store
                    .set_mode_and_persist(session_id, SerializableMode::Plan)
                    .context("persist session mode")?;
                if previous.as_ref() != Some(&SerializableMode::Plan) {
                    plan_restore = Some(match previous {
                        Some(mode) => PlanModeRestore::Value(mode),
                        None => PlanModeRestore::Absent,
                    });
                }
            }
            crate::features::assistant::timing::register_eval_observation(session_id);
        } else {
            runtime
                .prepare(&SessionSpec {
                    session_id: session_id.to_owned(),
                    model_selection,
                    // A `--workspace` run records the task directory in the
                    // session metadata too, so the GUI list/detail shows the
                    // directory the session actually works in (the durable
                    // binding below is the authoritative copy; this is the
                    // display half, matching GUI-created bound sessions).
                    workspace: workspace_binding.clone(),
                })
                .await
                .context("prepare agentic session")?;
            // Persist the requested workspace as the session's durable #445
            // binding: a later GUI reopen keeps working-directory continuity
            // and the binding-keyed gates (composer chip, YOLO confirmation)
            // apply exactly as for a GUI-created bound session. Without the
            // sidecar the reopened session silently falls back to its private
            // scratch while `metadata.workspace` records the task directory.
            // Caller-provided sessions keep their own binding — the run-scoped
            // resolver above overrides this run only. The binding is the same
            // validated/normalized string the resolver carries (validated
            // once, above); a bind failure still rolls the session back and
            // the stub cleanup removes the prepared record.
            if let Some(binding) = workspace_binding.clone() {
                store
                    .bind_session_workspace(session_id, binding)
                    .context("persist session workspace binding")?;
            }
            // Persist an explicit Plan request through the GUI's per-session
            // lane, so reopening the session restores its own last mode
            // instead of resolving the unbound default (Yolo). The failure is
            // fatal: the in-run mode is already applied via the send op, but
            // reporting success while the session would reopen in Yolo is the
            // unsafe direction of divergence. Failing here counts as setup —
            // the turn never started, so the stub cleanup removes the prepared
            // session, exactly like the bind failure above.
            if matches!(request.mode, Some(AgenticTaskMode::Plan)) {
                store
                    .set_mode_and_persist(session_id, SerializableMode::Plan)
                    .context("persist session mode")?;
            }
        }
        let (content, consumed_sources, staged_copies) = prompt_with_attachments(
            store,
            session_id,
            request,
            existing_session,
            &mut staging_in_flight,
        )
        .await?;
        staged_attachment_copies = staged_copies;
        // Marks the point past which a deadline hit is genuinely ambiguous:
        // the submit may already have admitted the turn, so the timeout arm
        // must not roll the mode/model pins back. Everything before this —
        // the pins themselves, the bind, and attachment staging, which is the
        // slow part and the usual reason a small timeout fires — provably
        // never reached the submit, and there the pins must be restored.
        // Snapshot the admission fact at the same boundary: from here on, the
        // submit's own durable append is the only transcript write this run
        // can make.
        pre_submit_revision = if existing_session {
            store
                .load(session_id)
                .ok()
                .and_then(|record| transcript_revision(&record.messages).ok())
        } else {
            None
        };
        submit_entered.store(true, Ordering::SeqCst);
        let handle = runtime
            .submit(&TurnInput {
                session_id: session_id.to_owned(),
                content,
                mode: app_mode,
                restrict_tools: false,
                eval_tool_policy: None,
            })
            .await
            .context("submit agentic turn")?;
        // Only now: the turn is admitted, so the staged copies belong to a
        // transcript that outlives this function. Consuming the caller's
        // originals any earlier can leave the user with neither copy when the
        // submit fails and the unadopted stub is cleaned up.
        remove_consumed_sources(&consumed_sources);
        Ok(handle)
    };
    // Bound separately: a match scrutinee temporary would keep the future —
    // and its mutable borrow of `plan_restore` — alive into the arms.
    //
    // Panic safety for the pins (round-21 review finding): a panic inside
    // the setup future (staging, ingest, prepare, submit) propagated at the
    // await and skipped every arm of the match below — the Err and timeout
    // restores never ran, so a caller-provided session stayed repinned in
    // a mode/model the user never asked to keep: the exact
    // permanent-change-made-by-a-run-that-did-no-work this block exists to
    // prevent. The pins are NOT moved across the await — they are restored
    // from the unwind arm after it — and the awaited future is wrapped in
    // futures_util::FutureExt::catch_unwind
    // (AssertUnwindSafe is sound here: this frame is not being unwound —
    // an unwind drops only the polled future's own state, and the borrowed
    // locals stay valid for the restores below), and the panic arm
    // restores both pins exactly like the pre-submit arms — under the same
    // `submit_entered` gate, so a panic in the post-submit window cannot
    // roll pins back under an admitted turn — before re-raising into this
    // task.
    //
    // Round-22 review finding: the first cut restored the model half with
    // `Handle::current().block_on` inside the unwind closure. This task only
    // ever runs on `run_windowless_host`'s multi-thread runtime worker, where
    // `block_on` panics ("Cannot start a runtime from within a runtime") — so
    // the model pin was never restored and the tokio panic masked the real
    // one. The panic case is now diverted out of the closure and handled as
    // ordinary async code after the await: the SAME
    // [`restore_pre_run_pins`] the Err and timeout arms use, then
    // `resume_unwind`. `resume_unwind` from this frame unwinds through the
    // poll exactly as the closure's re-raise did.
    //
    // The setup coroutine mutably borrows the pin locals until the future
    // is dropped; a panicked future is dropped inside catch_unwind, which
    // ENDS those borrows before the match below runs — so the pin values
    // it recorded (set inside the coroutine before the panic) are readable
    // exactly there, and only there. Neither pin is moved before the await.
    let setup_result: Result<Result<TurnHandle, anyhow::Error>, tokio::time::error::Elapsed> =
        match futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
            tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), setup),
        ))
        .await
        {
            Ok(setup_result) => setup_result,
            Err(panic) => {
                // The setup panicked. Under the same gate as the timeout arm:
                // before the submit boundary nothing ran that could own the
                // session — the same proof the Err arm relies on — so both
                // pins go back before the panic is resumed into this task.
                // Past the boundary (a panic between submit and report
                // construction) the turn may already be admitted, and rolling
                // the pins back mid-turn would evict the engine under it —
                // there the pins stay, like those of any submitted run. The
                // sequencing itself is extracted (and pinned) in
                // [`resume_unwind_after_pinned_restore`]. The staging task,
                // if the panic caught it mid-flight, is joined first so no
                // staged write outlives the unwind (the handle is owned
                // outside the dropped setup future — see
                // `staging_in_flight`).
                join_staged_attachments(&mut staging_in_flight).await;
                resume_unwind_after_pinned_restore(&submit_entered, panic, |reason| {
                    restore_pre_run_pins(
                        runtime,
                        store,
                        session_id,
                        plan_restore.take(),
                        model_restore.take(),
                        reason,
                    )
                })
                .await
            }
        };
    let handle = match setup_result {
        Ok(submitted) => match submitted {
            Ok(handle) => handle,
            Err(error) => {
                // The submit boundary is not atomic with transcript admission
                // (the `!submitted` lifecycle lane below documents the same
                // fact): the engine lazily spawns on submit and can durably
                // admit the user message before the fault surfaces. Rolling a
                // caller-provided session's pins back over an admitted
                // transcript would reopen it in the run's transient
                // mode/model — the exact unsafe divergence the timeout arm
                // gates on `submit_entered`. Failures before that flag was
                // set (staging) provably never polled the submit, so they
                // restore without a snapshot; past the flag, decide from
                // record facts instead: the pins restore only when the
                // transcript revision is unchanged since the pre-submit
                // snapshot (nothing landed); unknown keeps them.
                if !submit_err_admitted_turn(
                    store,
                    session_id,
                    submit_entered.load(Ordering::SeqCst),
                    pre_submit_revision.as_deref(),
                ) {
                    restore_pre_run_pins(
                        runtime,
                        store,
                        session_id,
                        plan_restore.take(),
                        model_restore.take(),
                        "failure",
                    )
                    .await;
                }
                sweep_unreferenced_staged_copies(
                    store,
                    session_id,
                    existing_session,
                    &staged_attachment_copies,
                );
                return (false, Err(error));
            }
        },
        Err(_elapsed) => {
            // A deadline that fired BEFORE the submit was entered provably
            // never ran a turn, so the pins are put back exactly like on a
            // setup failure, and the report says submitted: false. Past that
            // point the outcome is genuinely ambiguous (the turn may have
            // been admitted and keep running — nothing here cancels it), so
            // the pins stay, like those of any submitted run, and the report
            // carries submitted: true so consumers keying side effects off
            // it take the at-least-once direction instead of re-injecting a
            // staged body next run. Either way, the staging task the dropped
            // setup future left joinable is joined first (bounded): its
            // batch either lands with its copies adopted into the sweep
            // below, or the note names the detachment — the old shape, where
            // an in-flight staging detached immediately and wrote after the
            // teardown, is gone.
            if let Some(copies) = join_staged_attachments(&mut staging_in_flight).await {
                staged_attachment_copies = copies;
            }
            let submit_entered = submit_entered.load(Ordering::SeqCst);
            if !submit_entered {
                restore_pre_run_pins(
                    runtime,
                    store,
                    session_id,
                    plan_restore.take(),
                    model_restore.take(),
                    "timeout",
                )
                .await;
                sweep_unreferenced_staged_copies(
                    store,
                    session_id,
                    existing_session,
                    &staged_attachment_copies,
                );
            }
            return (false, Ok(setup_timeout_report(session_id, submit_entered)));
        }
    };
    drop(suite_guard);

    let mut timed_out = false;
    let started = Instant::now();
    let mut heartbeat = started;
    while runtime.is_turn_active(session_id) {
        if Instant::now() >= deadline {
            timed_out = true;
            runtime.cancel(session_id).await;
            let settle = Instant::now() + Duration::from_secs(CANCEL_SETTLE_SECS);
            while runtime.is_turn_active(session_id) && Instant::now() < settle {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            break;
        }
        if heartbeat.elapsed() >= Duration::from_secs(HEARTBEAT_SECS) {
            super::note_stderr(&format!(
                "[pinvou agent run] turn still active, {}s elapsed",
                started.elapsed().as_secs()
            ));
            heartbeat = Instant::now();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // `wait_for_completion` internally polls for as long as the turn is
    // active (an unbounded wait); on the timeout path the cancel may not
    // actually stop the engine, so the settle window bounds it: if the turn
    // is still active after cancel, give up on the full TurnResult and emit
    // a timeout report — never wait forever.
    enum TurnOutcome {
        Done(TurnResult),
        AbandonedAfterCancel,
        /// The `bool` records whether the deadline had already fired when the
        /// wait failed, so the report can keep the timeout classification.
        WaitFailed(bool, anyhow::Error),
    }
    let turn_outcome = if timed_out && runtime.is_turn_active(session_id) {
        TurnOutcome::AbandonedAfterCancel
    } else {
        // Read failures (read_timeline/load_eval_transcript etc.) have nothing
        // to do with the timeout: disguising them as one would swallow the
        // real root cause, so they get an explicit error report.
        match runtime.wait_for_completion(&handle).await {
            Ok(turn) => TurnOutcome::Done(turn),
            Err(error) => TurnOutcome::WaitFailed(timed_out, error),
        }
    };
    match turn_outcome {
        TurnOutcome::Done(turn) => {
            // Timeout race: the turn finished naturally after the deadline but
            // before the cancel took effect (cancel is a no-op on a finished
            // turn), so this is a full terminal result — keep the engine's
            // real status and mark completed_after_deadline for the grader.
            // Failed/Cancelled caused by the cancel remain reported as
            // timeout.
            let completed_after_deadline =
                timed_out && turn.status.eq_ignore_ascii_case("completed");
            let status = if timed_out && !completed_after_deadline {
                "timeout".to_string()
            } else {
                turn.status
            };
            (
                true,
                Ok(AgenticTaskReport {
                    session_id: session_id.to_owned(),
                    status,
                    timed_out,
                    completed_after_deadline,
                    submitted: true,
                    assistant_text: turn.assistant_text,
                    tool_events: turn
                        .tool_events
                        .into_iter()
                        .map(|event| AgenticToolEvent {
                            name: event.name,
                            failed: event.failed,
                        })
                        .collect(),
                    usage: turn.usage.map(|usage| AgenticUsageReport {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        cache_hit_tokens: usage.cache_hit_tokens,
                        cache_miss_tokens: usage.cache_miss_tokens,
                        cache_write_tokens: usage.cache_write_tokens,
                        reasoning_tokens: usage.reasoning_tokens,
                        context_window: usage.context_window,
                    }),
                    error: turn.error,
                }),
            )
        }
        TurnOutcome::AbandonedAfterCancel => {
            // The turn never settled after cancel; salvage whatever the
            // transcript already holds so the report keeps partial
            // observability instead of dropping every tool event.
            let (assistant_text, tool_events) = partial_turn_analysis(runtime, &handle);
            (
                true,
                Ok(AgenticTaskReport {
                    session_id: session_id.to_owned(),
                    status: "timeout".to_string(),
                    timed_out: true,
                    completed_after_deadline: false,
                    submitted: true,
                    assistant_text,
                    tool_events,
                    usage: None,
                    error: Some("agent turn did not settle after cancel".to_string()),
                }),
            )
        }
        TurnOutcome::WaitFailed(timed_out, error) => {
            // When the deadline had already fired, the task genuinely
            // overran: keep the timeout classification (matching the
            // abandoned-after-cancel arm) instead of downgrading it to a
            // generic error, and salvage the partial transcript the same
            // way. The read failure itself stays in `error`.
            let (assistant_text, tool_events) = if timed_out {
                partial_turn_analysis(runtime, &handle)
            } else {
                (String::new(), Vec::new())
            };
            let error = if timed_out {
                format!("agent turn timed out; failed to read turn result: {error:#}")
            } else {
                format!("failed to read turn result: {error:#}")
            };
            (
                true,
                Ok(AgenticTaskReport {
                    session_id: session_id.to_owned(),
                    status: if timed_out {
                        "timeout".to_string()
                    } else {
                        "error".to_string()
                    },
                    timed_out,
                    completed_after_deadline: false,
                    submitted: true,
                    assistant_text,
                    tool_events,
                    usage: None,
                    error: Some(error),
                }),
            )
        }
    }
}

/// Enrich the prompt with the request's attachments using the GUI attachment
/// pipeline: each file is staged into the session ledger root's
/// `attachments/` directory (the same secure staging the GUI dialog and the
/// eval bridge use), ingested through `features/files::file_ingest` (the same
/// chip ingest), and rendered by the same product message builder the GUI
/// chat command uses for the non-native-image path. `reference_absolute`
/// follows the GUI chat command: when the session is bound to a real
/// directory (`SessionRoots::bound` — the documented binding signal, not a
/// path comparison between the two roots), staged files are referenced
/// by absolute path. Images get the `image_analyze` hard-rule text, which the
/// product tool allowlist always provides, so no model image-capability probe
/// is needed.
/// One staging batch's output: the ingested attachments for the prompt, the
/// consumed sources, and the staged workspace copies the failure arms sweep.
type StagedBatch = (
    Vec<IngestResult>,
    Vec<std::path::PathBuf>,
    Vec<std::path::PathBuf>,
);

/// Upper bound on how long the failure arms wait for a staging task that was
/// still running when the deadline (or a panic) dropped the setup future.
/// Staging is capped at 16 × 20 MiB plus bounded ingest work, so this grace
/// covers every real completion; expiring it degrades to the pre-fix
/// behavior (detached task) rather than hanging the report on a wedged
/// filesystem read.
const STAGING_JOIN_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

/// Joins a staging task whose setup future was dropped before its staging
/// await completed (deadline or panic arm). On a clean join the copies list
/// is returned for the caller's sweep; a staging failure self-swept its own
/// partials and yields `None`; a grace expiry leaves the task detached with
/// a stderr note — the outcome a dropped handle used to produce instantly,
/// now only after a grace that real staging always fits inside.
async fn join_staged_attachments(
    staging_in_flight: &mut Option<tokio::task::JoinHandle<anyhow::Result<StagedBatch>>>,
) -> Option<Vec<std::path::PathBuf>> {
    let join = staging_in_flight.as_mut()?;
    match tokio::time::timeout(STAGING_JOIN_GRACE, &mut *join).await {
        Ok(Ok(Ok((_, _, copies)))) => Some(copies),
        Ok(Ok(Err(_))) => None,
        Ok(Err(join_error)) => {
            super::note_stderr(&format!(
                "agentic staging task ended abnormally after the deadline: {join_error}"
            ));
            None
        }
        Err(_elapsed) => {
            super::note_stderr(
                "agentic staging task did not finish within the post-deadline grace; \
                 its copies may land unreferenced",
            );
            None
        }
    }
}

async fn prompt_with_attachments(
    store: &SessionStore,
    session_id: &str,
    request: &AgenticTaskRequest,
    existing_session: bool,
    staging_in_flight: &mut Option<tokio::task::JoinHandle<anyhow::Result<StagedBatch>>>,
) -> Result<(String, Vec<std::path::PathBuf>, Vec<std::path::PathBuf>)> {
    if request.attachments.is_empty() {
        return Ok((request.prompt.clone(), Vec::new(), Vec::new()));
    }
    let roots = store
        .session_roots(session_id)
        .context("resolve attachment roots")?;
    let ledger_root = roots.ledger.clone();
    // `SessionRoots::bound` is the documented MUST for detecting the bound
    // state (`ledger != execution` stops implying binding once other dual-root
    // shapes appear) — same predicate as the GUI chat command. The second
    // disjunct is defense-in-depth for an explicit `--workspace` on a
    // caller-provided session: today the run-scoped resolver already marks
    // that session bound (so `roots.bound` covers it), but forcing the
    // absolute form costs nothing and keeps staged ledger files resolving
    // correctly if the resolver's bound propagation ever narrows.
    let reference_absolute = roots.bound || (existing_session && request.workspace.is_some());
    let attachments = request.attachments.clone();
    let prompt = request.prompt.clone();
    let staging_root = ledger_root.clone();
    let join =
        tokio::task::spawn_blocking(move || stage_and_ingest_batch(&attachments, &staging_root));
    // Park the handle in the caller's slot BEFORE awaiting, then await by
    // mutable borrow of the parked handle: a deadline firing at this await
    // drops only the borrow (and with it the setup future) — the blocking
    // task stays joinable for the failure arms instead of detaching
    // mid-write.
    *staging_in_flight = Some(join);
    let staged = match staging_in_flight.as_mut() {
        Some(join) => (&mut *join).await.context("attachment staging task")??,
        None => unreachable!("the staging handle was parked above"),
    };
    let (ingested, consumed_sources, staged_copies) = staged;
    Ok((
        build_message_with_attachments_in_dir(
            prompt,
            ingested,
            &ledger_root,
            "attachments",
            reference_absolute,
        ),
        consumed_sources,
        staged_copies,
    ))
}

/// Sweep this run's staged attachment copies after a setup failure that
/// provably never admitted the turn (the submit resolved to an error, or the
/// deadline fired before it was entered). A caller-provided session is never
/// auto-deleted, so the fresh-run stub cleanup cannot reclaim its staged
/// directory; without this sweep every failed run leaves another
/// unreferenced batch under the caller's workspace. The sweep is gated on
/// the record still holding no messages — the same re-check the guarded stub
/// delete applies — because an admitted turn's transcript references these
/// copies; a read error keeps them (the safe direction). Fresh sessions are
/// skipped: their stub cleanup removes the whole record directory.
fn sweep_unreferenced_staged_copies(
    store: &SessionStore,
    session_id: &str,
    existing_session: bool,
    staged: &[std::path::PathBuf],
) {
    if !existing_session || staged.is_empty() {
        return;
    }
    if !matches!(store.chat_session_has_messages(session_id), Ok(false)) {
        return;
    }
    for path in staged {
        let _ = std::fs::remove_file(path);
    }
}

/// Stage-and-ingest one attachment batch into the session workspace.
///
/// Sources marked `remove_after_ingest` are reported to the caller, not
/// deleted here. Deleting at ingest time destroys the only remaining copy
/// whenever the run does not go on to start: the staged copy lives under the
/// session directory, and a submit failure or a setup timeout on a fresh run
/// classifies the record as a never-started stub, whose cleanup removes that
/// directory. Source gone, staged copy gone, no turn. The caller deletes them
/// once the turn is admitted, which is the first moment the staged copy is
/// part of something durable.
///
/// On ANY mid-batch failure the staged copies of this batch are swept: the
/// run never submits a turn, so no message references them, and a
/// caller-provided session (which is never auto-deleted) would otherwise
/// accumulate unreferenced orphans across repeated failed batches. The
/// caller's originals are untouched — `consumed_sources` is only returned on
/// success, so the sweep can never leave the user with neither file.
fn stage_and_ingest_batch(
    attachments: &[AgenticTaskAttachment],
    staging_root: &std::path::Path,
) -> Result<(
    Vec<IngestResult>,
    Vec<std::path::PathBuf>,
    Vec<std::path::PathBuf>,
)> {
    let mut results = Vec::with_capacity(attachments.len());
    let mut consumed_sources: Vec<std::path::PathBuf> = Vec::new();
    // Every staged copy of this batch, for the failure sweep below.
    let mut staged_paths: Vec<std::path::PathBuf> = Vec::new();
    let batch = (|| -> Result<Vec<IngestResult>> {
        // Re-stat at staging time: the caps were enforced at validation,
        // but the sources are caller-owned and can grow or be swapped
        // between validation and this copy — the enforced cap must be
        // the staged size, not the validated one.
        let mut staged_total = 0_u64;
        for attachment in attachments {
            let basename = attachment
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .context(anyhow::anyhow!(
                    "agent_attachment_invalid_name: {}",
                    attachment.path.display()
                ))?;
            let before = staged_total;
            staged_total = ensure_stage_size(&attachment.path, staged_total)?;
            // What the pre-copy stat predicted this file would add;
            // swapped for the landed size once the copy finishes.
            let predicted = staged_total.saturating_sub(before);
            // The re-stat above caps the validated size, but the source
            // can still grow while the copy streams — bound the staged
            // bytes too, exactly like the eval pipeline's staging.
            let relative = match stage_file_in_workspace_with_copier(
                &attachment.path.to_string_lossy(),
                &basename,
                staging_root,
                "attachments",
                |source, destination| copy_bounded(source, destination, MAX_ATTACHMENT_BYTES),
            ) {
                Some(relative) => relative,
                None => {
                    // The stager collapses every refusal into `None`.
                    // The one case worth its own error code is a
                    // caller-owned source that grew past the per-file
                    // cap while the copy streamed — the exact
                    // condition `copy_bounded` exists for; re-stat
                    // the source to tell it apart from a staging
                    // infrastructure failure.
                    if stage_refusal_means_cap_growth(&attachment.path) {
                        anyhow::bail!(
                            "agent_attachment_too_large: {} grew past the per-file \
                                 cap during staging",
                            attachment.path.display()
                        );
                    }
                    anyhow::bail!(
                        "agent_attachment_stage_failed: staging into the session \
                             workspace failed for {}",
                        attachment.path.display()
                    );
                }
            };
            // The aggregate budget must count what actually landed,
            // not what the pre-copy stat predicted. `copy_bounded`
            // binds each file on its own, but a caller-owned source
            // that is tiny at `ensure_stage_size` and large by the
            // time the copy streams contributes its stat'ed size to
            // the running total — sixteen of those pass a 100 MiB
            // budget while writing 320 MiB into the session. Re-stat
            // the destination and charge the real figure.
            let staged_path = staging_root.join(&relative);
            staged_paths.push(staged_path.clone());
            let landed = std::fs::metadata(&staged_path)
                .with_context(|| {
                    format!(
                        "agent_attachment_stage_failed: cannot stat the staged copy of {}",
                        attachment.path.display()
                    )
                })?
                .len();
            staged_total = staged_total
                .saturating_sub(predicted)
                .saturating_add(landed);
            if staged_total > MAX_ATTACHMENTS_TOTAL_BYTES {
                anyhow::bail!(
                    "agent_attachment_total_too_large: staged attachments exceed \
                     {MAX_ATTACHMENTS_TOTAL_BYTES} bytes in total"
                );
            }
            let result = crate::features::files::file_ingest::ingest_attachment(&staged_path)
                .map_err(|code| anyhow::anyhow!("agent_attachment_ingest_failed: {code}"))?;
            if attachment.remove_after_ingest {
                consumed_sources.push(attachment.path.clone());
            }
            results.push(result);
        }
        Ok(results)
    })();
    let results = match batch {
        Ok(results) => results,
        Err(error) => {
            // Best-effort sweep of this batch's staged copies. The caller's
            // originals are untouched: `consumed_sources` is only returned on
            // success, so removing the staged copies can never leave the user
            // with neither file.
            for path in &staged_paths {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }
    };
    // The staged copies ride back to the caller: a failure AFTER this
    // function — the submit resolving to an error, or a deadline firing
    // before it was entered — leaves them unreferenced, and a
    // caller-provided session is never auto-deleted, so the fresh-run stub
    // cleanup cannot reclaim them there. The caller sweeps them through
    // `sweep_unreferenced_staged_copies`.
    Ok((results, consumed_sources, staged_paths))
}

/// Whether a staging refusal's source re-stat shows it now past the per-file
/// cap — the condition `copy_bounded` exists for (a caller-owned source that
/// grew while the copy streamed), reported as `agent_attachment_too_large`
/// instead of the generic staging-failure code.
fn stage_refusal_means_cap_growth(source: &std::path::Path) -> bool {
    std::fs::metadata(source)
        .map(|meta| meta.len() > MAX_ATTACHMENT_BYTES)
        .unwrap_or(false)
}

/// Deletes the `remove_after_ingest` sources once the turn has been admitted.
/// Best-effort: the staged copies are already in the transcript, so a source
/// that cannot be removed is a cosmetic leftover, not a lost file.
fn remove_consumed_sources(sources: &[std::path::PathBuf]) {
    for source in sources {
        if let Err(error) = std::fs::remove_file(source) {
            super::note_stderr(&format!(
                "[pinvou agent run] remove_after_ingest could not delete {}: {error}",
                source.display()
            ));
        }
    }
}

/// Best-effort salvage of the assistant text and tool events already recorded
/// for `handle`'s turn; used when the turn is abandoned after cancel. Any
/// read failure yields empty data — the report's `error` field carries the
/// root cause.
fn partial_turn_analysis(
    runtime: &EnginePoolRuntime,
    handle: &TurnHandle,
) -> (String, Vec<AgenticToolEvent>) {
    let transcript = match runtime.pool.load_eval_transcript(&handle.session_id) {
        Ok(transcript) => transcript,
        Err(_) => return (String::new(), Vec::new()),
    };
    let (assistant_text, events) = super::extract_turn_analysis(&transcript);
    // Scope to this turn's recorded milestones when the timeline is readable;
    // the session runs exactly one turn, so an unreadable timeline keeps all
    // events.
    let tool_events = match crate::features::assistant::timing::read_timeline(&handle.session_id) {
        Ok(timeline) => {
            EnginePoolRuntime::scope_turn_analysis_to_turn(&timeline, &handle.turn_id, events).1
        }
        Err(_) => events,
    };
    (
        assistant_text,
        tool_events
            .into_iter()
            .map(|event| AgenticToolEvent {
                name: event.name,
                failed: event.failed,
            })
            .collect(),
    )
}

/// Fresh session id for one agentic run:
/// `{HEADLESS_SESSION_PREFIX}{pid}_{unix_millis}_{counter}`.
///
/// The prefix comes from `HEADLESS_SESSION_PREFIX` rather than a literal:
/// retention keys the separate headless eviction budget on it, so changing the
/// format here without changing the sweep would silently put these sessions
/// back in competition with the user's GUI chats.
///
/// The pid alone is not unique across time: OS pid reuse can hand a later
/// process the same pid while the per-process counter restarts at 0, so a
/// `{pid}_{counter}` shape could reproduce an id that is still persisted weeks
/// later. The unix-millisecond component bounds a collision to
/// same-millisecond reuse of both the pid and the counter (and the caller
/// still regenerates while the record exists). The id stays inside the
/// session id alphabet `[A-Za-z0-9_-]` (see `features/sessions/validators.rs`),
/// so the store accepts it unchanged.
fn fresh_session_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let unix_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!(
        "{}{}_{}_{}",
        crate::features::sessions::HEADLESS_SESSION_PREFIX,
        std::process::id(),
        unix_millis,
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AgenticTaskAttachment, AgenticTaskMode, AgenticTaskReport, AgenticTaskRequest,
        AgenticToolEvent, DEFAULT_TIMEOUT_SECS, MAX_ATTACHMENT_BYTES, MAX_ATTACHMENTS,
        MAX_ATTACHMENTS_TOTAL_BYTES, MAX_TIMEOUT_SECS, NeverStartedDisposition, PlanModeRestore,
        arm_retention_eviction_observer, ensure_existing_chat_session, ensure_model_exists,
        ensure_stage_size, fresh_session_id, keep_session_from_env, never_started_disposition,
        one_shot_cleanup_decision, refuse_ingest_without_a_surviving_copy, restore_plan_mode,
        resume_unwind_after_pinned_restore, retention_eviction_warning, setup_timeout_report,
        stage_and_ingest_batch, stage_refusal_means_cap_growth, sweep_unreferenced_staged_copies,
        validate_attachments,
    };
    use crate::features::assistant::attachments::{
        copy_bounded, stage_file_in_workspace_with_copier,
    };
    use crate::features::sessions::{
        MAX_HEADLESS_SESSIONS, MAX_SESSIONS_PER_KIND, NEW_CHAT_TITLE, RetentionEvictionRecord,
        ScheduledRunMode, ScheduledRunProfile, SerializableMode, SessionStore,
    };
    use crate::platform::test_support::locked_env;
    use deepseek_tui::models::{ContentBlock, Message};
    use std::path::PathBuf;

    /// RAII cleanup for a test scratch directory under `std::env::temp_dir()`:
    /// removed best-effort on drop (normal return or panic unwind). Removal
    /// failures are ignored on purpose — a leaked temp dir must never fail a
    /// test, and an OS that still holds the directory open simply skips it.
    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new(path: PathBuf) -> Self {
            Self { path }
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn default_request(prompt: &str) -> AgenticTaskRequest {
        AgenticTaskRequest {
            prompt: prompt.to_string(),
            workspace: None,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            session_id: None,
            mode: None,
            model_id: None,
            attachments: Vec::new(),
        }
    }

    /// The one-shot opt-in on a fresh session that produced a report:
    /// the caller owns the run's own record, so a readable
    /// factory-titled record is deleted. Two keeps: an adopted record
    /// (renamed away from the factory title — a GUI user owns it now) and an
    /// unreadable one, because deleting on unknown state is the unsafe
    /// direction.
    #[test]
    fn one_shot_cleanup_deletes_only_the_runs_own_factory_titled_record() {
        use super::FreshSessionDisposition::*;
        assert!(matches!(
            one_shot_cleanup_decision(Ok(true), Ok(true)),
            Cleanup
        ));
        assert!(matches!(
            one_shot_cleanup_decision(Ok(false), Ok(true)),
            Cleanup
        ));
        assert!(
            matches!(one_shot_cleanup_decision(Ok(true), Ok(false)), Keep),
            "a renamed record is adopted — the one-shot lane must not delete it"
        );
        assert!(
            matches!(one_shot_cleanup_decision(Ok(false), Err(())), Keep),
            "an unreadable title is not proven factory-titled and must keep"
        );
        assert!(
            matches!(one_shot_cleanup_decision(Err(()), Ok(true)), Keep),
            "an unreadable record is unknown state and must not be deleted"
        );
        assert!(matches!(one_shot_cleanup_decision(Err(()), Err(())), Keep));
    }

    /// The never-started decision, pinned over the branch order that is the
    /// contract: a zero-message FACTORY-TITLED stub cleans up regardless of
    /// `KEEP_SESSION`, a renamed (adopted) zero-message record keeps — the
    /// rename is ownership on this lane too — a started transcript keeps
    /// under the default contract, the one-shot opt-in deletes it — but only
    /// while the record still wears the factory title — and an unloadable
    /// record keeps on every path.
    #[test]
    fn never_started_disposition_matrix_is_pinned() {
        // (has_messages, engine_active, keep_session, factory_titled).
        let matrix = [
            // Zero-message factory-titled stub: cleanup-eligible regardless
            // of KEEP_SESSION (it would litter the shared store with
            // eviction bait).
            (
                Ok(false),
                false,
                true,
                Ok(true),
                NeverStartedDisposition::CleanupStub,
            ),
            (
                Ok(false),
                false,
                false,
                Ok(true),
                NeverStartedDisposition::CleanupStub,
            ),
            // Zero-message + adopted (renamed): the rename is ownership on
            // the stub lane too — the record keeps.
            (
                Ok(false),
                false,
                true,
                Ok(false),
                NeverStartedDisposition::KeepInspectable,
            ),
            (
                Ok(false),
                false,
                false,
                Ok(false),
                NeverStartedDisposition::KeepInspectable,
            ),
            // Zero-message + unreadable title: not proven factory-titled,
            // keep (deleting on unknown state is the unsafe direction).
            (
                Ok(false),
                false,
                true,
                Err(()),
                NeverStartedDisposition::KeepInspectable,
            ),
            // Zero-message + engine ACTIVE + one-shot + factory title: a
            // live engine outranks the stale zero-message disk sample, so
            // the record takes the legacy delete arm — the row the replaced
            // test pinned ("the two arms differ for KEEP_SESSION unset").
            (
                Ok(false),
                true,
                false,
                Ok(true),
                NeverStartedDisposition::LegacyCleanupStarted,
            ),
            // Started (durable messages) + default keep: inspectable.
            (
                Ok(true),
                false,
                true,
                Ok(true),
                NeverStartedDisposition::KeepInspectable,
            ),
            // Started + one-shot + factory title: the legacy delete.
            (
                Ok(true),
                false,
                false,
                Ok(true),
                NeverStartedDisposition::LegacyCleanupStarted,
            ),
            // Started + one-shot + adopted (renamed): the rename is
            // ownership, the one-shot lane does not own this record.
            (
                Ok(true),
                false,
                false,
                Ok(false),
                NeverStartedDisposition::KeepInspectable,
            ),
            // Started + one-shot + unreadable title: not proven
            // factory-titled, keep.
            (
                Ok(true),
                false,
                false,
                Err(()),
                NeverStartedDisposition::KeepInspectable,
            ),
            // Engine liveness overrides a stale zero-message sample.
            (
                Ok(false),
                true,
                true,
                Ok(true),
                NeverStartedDisposition::KeepInspectable,
            ),
            // Unloadable record keeps even under the one-shot opt-in.
            (
                Err(()),
                false,
                false,
                Ok(true),
                NeverStartedDisposition::KeepInspectable,
            ),
        ];
        for (has_messages, engine_active, keep_session, factory_titled, expected) in matrix {
            assert_eq!(
                never_started_disposition(
                    has_messages,
                    engine_active,
                    keep_session,
                    factory_titled
                ),
                expected,
                "has_messages={has_messages:?} engine_active={engine_active} \
                 keep_session={keep_session} factory_titled={factory_titled:?}"
            );
        }
    }

    #[test]
    fn keep_session_env_defaults_to_keeping() {
        let (_lock, _env) = locked_env(&["PINVOU3_AGENT_TASK_KEEP_SESSION"]);
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::remove_var("PINVOU3_AGENT_TASK_KEEP_SESSION") };
        assert!(keep_session_from_env(), "absent env must keep the session");
        for value in ["1", "true", "yes", "on", "anything-else"] {
            // SAFETY: see above.
            unsafe { std::env::set_var("PINVOU3_AGENT_TASK_KEEP_SESSION", value) };
            assert!(keep_session_from_env(), "{value} must keep the session");
        }
        // Pinned exactness: comparison is ASCII case-insensitive WITHOUT
        // trimming, and empty means keep — only the bare falsy tokens delete.
        for value in [" 0", "0 ", "\tfalse", ""] {
            // SAFETY: see above.
            unsafe { std::env::set_var("PINVOU3_AGENT_TASK_KEEP_SESSION", value) };
            assert!(keep_session_from_env(), "{value:?} must keep the session");
        }
        for value in ["0", "false", "no", "off", "FALSE", "Off"] {
            // SAFETY: see above.
            unsafe { std::env::set_var("PINVOU3_AGENT_TASK_KEEP_SESSION", value) };
            assert!(!keep_session_from_env(), "{value} must delete the session");
        }
        // `_env` restores the captured value on return or panic.
    }

    #[test]
    fn request_defaults_timeout_and_workspace() {
        let request: AgenticTaskRequest = serde_json::from_str(r#"{"prompt":"do it"}"#).unwrap();
        assert_eq!(request.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(request.workspace.is_none());
        assert!(request.session_id.is_none());
        assert!(request.mode.is_none());
        assert!(request.model_id.is_none());
        assert!(request.attachments.is_empty());

        let request: AgenticTaskRequest =
            serde_json::from_str(r#"{"prompt":"p","workspace":"/tmp/task","timeout_secs":42}"#)
                .unwrap();
        assert_eq!(request.timeout_secs, 42);
        assert_eq!(
            request.workspace,
            Some(std::path::PathBuf::from("/tmp/task"))
        );
    }

    #[test]
    fn default_request_serialization_keeps_todays_field_set() {
        // New parity fields must vanish while unset so serialized requests
        // stay byte-identical to the historical schema.
        let json = serde_json::to_value(default_request("do it")).unwrap();
        let keys = json.as_object().unwrap();
        assert_eq!(keys.len(), 3, "unexpected fields: {keys:?}");
        assert!(keys.contains_key("prompt"));
        assert!(keys.contains_key("workspace"));
        assert!(keys.contains_key("timeout_secs"));
    }

    #[test]
    fn request_parses_new_parity_fields() {
        let request: AgenticTaskRequest = serde_json::from_str(
            r#"{
                "prompt":"p",
                "session_id":"sess_1",
                "mode":"plan",
                "model_id":"model-7",
                "attachments":[{"path":"/tmp/a.txt","remove_after_ingest":true}]
            }"#,
        )
        .unwrap();
        assert_eq!(request.session_id.as_deref(), Some("sess_1"));
        assert_eq!(request.mode, Some(AgenticTaskMode::Plan));
        assert_eq!(request.model_id.as_deref(), Some("model-7"));
        assert_eq!(request.attachments.len(), 1);
        assert_eq!(request.attachments[0].path, PathBuf::from("/tmp/a.txt"));
        assert!(request.attachments[0].remove_after_ingest);
    }

    #[test]
    fn attachment_struct_defaults_remove_after_ingest() {
        let attachment: AgenticTaskAttachment =
            serde_json::from_str(r#"{"path":"/tmp/a.txt"}"#).unwrap();
        assert_eq!(attachment.path, PathBuf::from("/tmp/a.txt"));
        assert!(!attachment.remove_after_ingest);
    }

    /// `to_app_mode` decides what the headless turn is actually allowed to do,
    /// and nothing asserted it: `Plan => AppMode::Agent` would silently hand a
    /// `--mode plan` run full shell and file-write access while every other
    /// test (which only covers the serde names) stayed green.
    #[test]
    fn task_mode_maps_to_the_matching_app_mode() {
        assert_eq!(
            AgenticTaskMode::Plan.to_app_mode(),
            deepseek_tui::AppMode::Plan,
            "a Plan request must run under the Plan app mode, not an executing one"
        );
        assert_eq!(
            AgenticTaskMode::Agent.to_app_mode(),
            deepseek_tui::AppMode::Agent
        );
    }

    /// The headless retention budget is keyed on the id prefix, so the runner's
    /// id format and the sweep's predicate must not drift into two literals.
    #[test]
    fn fresh_ids_carry_the_prefix_retention_buckets_on() {
        assert!(
            fresh_session_id().starts_with(crate::features::sessions::HEADLESS_SESSION_PREFIX),
            "retention buckets headless sessions by this prefix; a format change here \
             silently puts them back in competition with the user's GUI chats"
        );
    }

    /// `remove_after_ingest` is only safe because the staged copy survives in
    /// the transcript. Under the legacy one-shot cleanup a FRESH run's session
    /// directory is deleted right after the turn, so the source and the copy
    /// both go — the combination must be refused before anything is staged or
    /// unlinked. A caller-provided session is never auto-deleted, so its
    /// staged copy always survives and the refusal does not apply.
    #[test]
    fn ingest_that_would_lose_both_copies_is_refused() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let attachments = vec![AgenticTaskAttachment {
            path: file.path().to_path_buf(),
            remove_after_ingest: true,
        }];
        let error = refuse_ingest_without_a_surviving_copy(&attachments, false, true)
            .expect_err("remove_after_ingest + one-shot cleanup destroys both copies");
        assert!(
            format!("{error}").contains("agent_attachment_ingest_would_lose_the_file"),
            "unexpected error: {error}"
        );
        // Either half alone is fine.
        refuse_ingest_without_a_surviving_copy(&attachments, true, true).unwrap();
        let keepers = vec![AgenticTaskAttachment {
            path: file.path().to_path_buf(),
            remove_after_ingest: false,
        }];
        refuse_ingest_without_a_surviving_copy(&keepers, false, true).unwrap();
        // A caller-provided session is never auto-deleted: its staged copy
        // survives even under the falsy opt-in, so no refusal.
        refuse_ingest_without_a_surviving_copy(&attachments, false, false).unwrap();
    }

    #[test]
    fn mode_enum_uses_snake_case_names() {
        assert_eq!(
            serde_json::to_string(&AgenticTaskMode::Agent).unwrap(),
            "\"agent\""
        );
        assert_eq!(
            serde_json::to_string(&AgenticTaskMode::Plan).unwrap(),
            "\"plan\""
        );
        let agent: AgenticTaskMode = serde_json::from_str("\"agent\"").unwrap();
        let plan: AgenticTaskMode = serde_json::from_str("\"plan\"").unwrap();
        assert_eq!(agent, AgenticTaskMode::Agent);
        assert_eq!(plan, AgenticTaskMode::Plan);
        // GUI names are not accepted: the request vocabulary is agent/plan.
        assert!(serde_json::from_str::<AgenticTaskMode>("\"yolo\"").is_err());
        assert!(serde_json::from_str::<AgenticTaskMode>("\"Agent\"").is_err());
    }

    #[test]
    fn validate_attachments_enforces_existence_and_limits() {
        let workspace = tempfile::tempdir().unwrap();
        let file_path = workspace.path().join("input.txt");
        std::fs::write(&file_path, b"hello").unwrap();

        assert!(
            validate_attachments(&[AgenticTaskAttachment {
                path: file_path.clone(),
                remove_after_ingest: false,
            }])
            .is_ok()
        );

        // Missing path.
        let error = validate_attachments(&[AgenticTaskAttachment {
            path: workspace.path().join("missing.txt"),
            remove_after_ingest: false,
        }])
        .unwrap_err();
        assert!(error.to_string().contains("agent_attachment_not_found"));

        // A directory is not an attachment.
        let error = validate_attachments(&[AgenticTaskAttachment {
            path: workspace.path().to_path_buf(),
            remove_after_ingest: false,
        }])
        .unwrap_err();
        assert!(error.to_string().contains("agent_attachment_not_found"));

        // Over the per-file size cap (sparse file, no real 20 MiB write).
        let oversized = workspace.path().join("big.bin");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(MAX_ATTACHMENT_BYTES + 1)
            .unwrap();
        let error = validate_attachments(&[AgenticTaskAttachment {
            path: oversized,
            remove_after_ingest: false,
        }])
        .unwrap_err();
        assert!(error.to_string().contains("agent_attachment_too_large"));

        // Over the attachment count cap.
        let too_many: Vec<_> = std::iter::repeat(AgenticTaskAttachment {
            path: file_path.clone(),
            remove_after_ingest: false,
        })
        .take(MAX_ATTACHMENTS + 1)
        .collect();
        let error = validate_attachments(&too_many).unwrap_err();
        assert!(error.to_string().contains("agent_attachment_too_many"));
    }

    #[test]
    fn validate_attachments_enforces_the_total_budget() {
        let workspace = tempfile::tempdir().unwrap();
        // Six files each just under the per-file cap sum past the aggregate
        // budget (6 × 18 MiB = 108 MiB > 100 MiB): the per-file check alone
        // allowed 320 MiB of staged attachments, so this branch is the
        // load-bearing one. Sparse files keep the test instant.
        let attachments: Vec<_> = (0..6)
            .map(|i| {
                let path = workspace.path().join(format!("part-{i}.bin"));
                std::fs::File::create(&path)
                    .unwrap()
                    .set_len(MAX_ATTACHMENT_BYTES - 2 * 1024 * 1024)
                    .unwrap();
                AgenticTaskAttachment {
                    path,
                    remove_after_ingest: false,
                }
            })
            .collect();
        let error = validate_attachments(&attachments).unwrap_err();
        assert!(
            error.to_string().contains("attachments total"),
            "the aggregate budget must reject the batch: {error}"
        );
    }

    #[test]
    fn ensure_stage_size_binds_the_staged_size_not_the_validated_one() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("grower.bin");

        // Validation-size file passes.
        std::fs::write(&path, b"tiny").unwrap();
        assert_eq!(ensure_stage_size(&path, 0).unwrap(), 4);

        // The caller-owned source grows past the per-file cap between
        // validation and staging: the copy must refuse, not stage it.
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_ATTACHMENT_BYTES + 1)
            .unwrap();
        let error = ensure_stage_size(&path, 0).unwrap_err();
        assert!(
            error.to_string().contains("at staging time"),
            "the refusal must name the stage-time re-check: {error}"
        );

        // A source that is fine on its own still refuses when it pushes the
        // running total past the aggregate budget.
        std::fs::write(&path, b"tiny").unwrap();
        let error = ensure_stage_size(&path, MAX_ATTACHMENTS_TOTAL_BYTES).unwrap_err();
        assert!(
            error.to_string().contains("at staging time"),
            "the aggregate budget must also bind at staging time: {error}"
        );

        // A vanished source refuses loudly instead of failing deep inside
        // the copy.
        std::fs::remove_file(&path).unwrap();
        let error = ensure_stage_size(&path, 0).unwrap_err();
        assert!(error.to_string().contains("vanished before staging"));
    }

    #[test]
    fn staged_copy_bounds_a_source_that_grows_while_streaming() {
        // The stage-time re-stat closes the stat→copy gap, but a caller-owned
        // source can still grow DURING the copy; `copy_bounded` caps the
        // staged bytes (take(MAX + 1) + over-check), and the staging helper
        // removes the partial destination when the copier refuses.
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("grower-mid-copy.bin");
        std::fs::write(&source, vec![0_u8; 64]).unwrap();

        let staged = stage_file_in_workspace_with_copier(
            source.to_str().unwrap(),
            "grower-mid-copy.bin",
            workspace.path(),
            "attachments",
            |source, destination| copy_bounded(source, destination, 8),
        );
        assert!(
            staged.is_none(),
            "a source over the copier's cap must refuse to stage"
        );
        let leftovers: Vec<_> = std::fs::read_dir(workspace.path().join("attachments"))
            .unwrap()
            .collect();
        assert!(
            leftovers.is_empty(),
            "the partial staged copy must be removed on refusal"
        );

        // A source within the cap stages normally through the same copier.
        std::fs::write(&source, b"tiny").unwrap();
        let staged = stage_file_in_workspace_with_copier(
            source.to_str().unwrap(),
            "grower-mid-copy.bin",
            workspace.path(),
            "attachments",
            |source, destination| copy_bounded(source, destination, 8),
        );
        let relative = staged.expect("a within-cap source stages through the bounded copier");
        assert_eq!(
            std::fs::read(workspace.path().join(&relative)).unwrap(),
            b"tiny",
            "the staged bytes are the bounded copy's exact contents"
        );
    }

    #[test]
    fn ensure_model_exists_rejects_unknown_model() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-agentic-model-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _tmp_cleanup = TempDirGuard::new(tmp.clone());
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let error = ensure_model_exists("definitely-missing-model").unwrap_err();
        assert!(error.to_string().contains("agent_model_not_found"));
        // `_tmp_cleanup` removes the scratch dir on return or panic;
        // `_env` restores the captured PINVOU3_HOME.
    }

    #[test]
    fn cap_growth_predicate_distinguishes_grown_sources() {
        let workspace = tempfile::tempdir().unwrap();
        let big = workspace.path().join("big.bin");
        std::fs::write(&big, vec![0_u8; MAX_ATTACHMENT_BYTES as usize + 1]).unwrap();
        assert!(
            stage_refusal_means_cap_growth(&big),
            "a source past the per-file cap at refusal time is the cap-growth case"
        );
        let tiny = workspace.path().join("tiny.txt");
        std::fs::write(&tiny, b"tiny").unwrap();
        assert!(
            !stage_refusal_means_cap_growth(&tiny),
            "a within-cap source is a staging failure, not a cap breach"
        );
        assert!(
            !stage_refusal_means_cap_growth(&workspace.path().join("missing.bin")),
            "a vanished source is a staging failure, not a cap breach"
        );
    }

    /// A mid-batch staging failure sweeps the whole batch's staged copies:
    /// the run never submits a turn, so no message references them, and a
    /// caller-provided session (never auto-deleted) would otherwise
    /// accumulate unreferenced orphans across repeated failed batches. The
    /// caller's originals are untouched.
    #[test]
    fn mid_batch_staging_failure_sweeps_the_batches_staged_copies() {
        let workspace = tempfile::tempdir().unwrap();
        let staging_root = workspace.path().join("ledger");
        std::fs::create_dir_all(staging_root.join("attachments")).unwrap();

        let good = workspace.path().join("good.txt");
        std::fs::write(&good, b"usable").unwrap();
        // A directory refuses staging AFTER the first file is already staged
        // (the copy fails on Unix, the open on Windows) and its stat stays
        // under the cap, so the batch takes the generic-failure arm — exactly
        // the orphan shape.
        let dir_source = workspace.path().join("stagedir");
        std::fs::create_dir(&dir_source).unwrap();
        let attachments = vec![
            AgenticTaskAttachment {
                path: good.clone(),
                remove_after_ingest: false,
            },
            AgenticTaskAttachment {
                path: dir_source,
                remove_after_ingest: false,
            },
        ];

        let error = stage_and_ingest_batch(&attachments, &staging_root).unwrap_err();
        assert!(
            error.to_string().contains("agent_attachment_stage_failed"),
            "a non-cap staging refusal keeps the generic code: {error:#}"
        );
        let leftovers: Vec<_> = std::fs::read_dir(staging_root.join("attachments"))
            .unwrap()
            .collect();
        assert!(
            leftovers.is_empty(),
            "the first file's staged copy must not survive the failed batch"
        );
        assert!(
            good.exists(),
            "the sweep removes staged copies, never the caller's originals"
        );
    }

    #[test]
    fn ensure_existing_chat_session_accepts_chat_rejects_unknown_and_scheduled() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-agentic-session-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _tmp_cleanup = TempDirGuard::new(tmp.clone());
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot");

        // Unknown session.
        let error = ensure_existing_chat_session(&store, "no_such_session").unwrap_err();
        assert!(error.to_string().contains("agent_session_not_found"));

        // Ordinary chat session is accepted.
        let chat = store
            .create_new("test-model".to_string(), None, tmp.clone())
            .unwrap();
        ensure_existing_chat_session(&store, &chat.metadata.id).unwrap();

        // A scheduled-run profile on the same id flips the kind: not chat.
        store.scheduled_profiles.write().insert(
            chat.metadata.id.clone(),
            ScheduledRunProfile {
                task_id: "task-1".to_string(),
                model: "test-model".to_string(),
                model_id: None,
                workspace: tmp.clone(),
                mode: ScheduledRunMode::Yolo,
                allow_shell: true,
                trust_mode: true,
                auto_approve: true,
            },
        );
        let error = ensure_existing_chat_session(&store, &chat.metadata.id).unwrap_err();
        assert!(error.to_string().contains("agent_session_not_chat"));

        // An existing-but-corrupt record reports unreadable, not "not found".
        let mut record_path = None;
        let wanted = std::ffi::OsString::from(format!("{}.json", chat.metadata.id));
        let mut stack = vec![crate::platform::paths::sessions_root()];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_name().is_some_and(|name| name == &wanted) {
                    record_path = Some(path);
                }
            }
        }
        std::fs::write(
            record_path.expect("chat session record file under the sessions root"),
            b"{corrupted",
        )
        .unwrap();
        let error = ensure_existing_chat_session(&store, &chat.metadata.id).unwrap_err();
        assert!(error.to_string().contains("agent_session_unreadable"));
        // `_env` restores the captured PINVOU3_HOME on return or panic.
    }

    /// A `--mode plan` run that never reaches the turn must leave a
    /// caller-provided session in the mode the user left it in: the run did no
    /// work, so it may not permanently flip their GUI session into Plan. Both
    /// captured shapes are restored — a durable value is written back, and a
    /// session that had no durable entry gets its entry removed again rather
    /// than a frozen copy of the resolved default. The restore is skipped when
    /// nothing was captured (a fresh session, or one already in Plan).
    #[test]
    fn setup_failure_restores_a_caller_sessions_pre_run_mode() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-agentic-plan-restore-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _tmp_cleanup = TempDirGuard::new(tmp.clone());
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot");
        let chat = store
            .create_new("test-model".to_string(), None, tmp.clone())
            .unwrap();
        let id = chat.metadata.id.clone();

        // No durable entry: the session follows its resolved default, and the
        // restore must put the absence back.
        assert_eq!(store.durable_mode_entry(&id), None);
        let resolved = store.mode_state(&id).mode;
        assert_ne!(
            resolved,
            SerializableMode::Plan,
            "fixture must start outside Plan or the restore is vacuous"
        );
        store
            .set_mode_and_persist(&id, SerializableMode::Plan)
            .unwrap();
        restore_plan_mode(&store, &id, Some(PlanModeRestore::Absent), "failure");
        assert_eq!(
            store.durable_mode_entry(&id),
            None,
            "the entry the failed run added must be gone"
        );
        assert_eq!(store.mode_state(&id).mode, resolved);

        // A durable value is written back as that value.
        store
            .set_mode_and_persist(&id, SerializableMode::Yolo)
            .unwrap();
        store
            .set_mode_and_persist(&id, SerializableMode::Plan)
            .unwrap();
        restore_plan_mode(
            &store,
            &id,
            Some(PlanModeRestore::Value(SerializableMode::Yolo)),
            "failure",
        );
        assert_eq!(
            store.durable_mode_entry(&id),
            Some(SerializableMode::Yolo),
            "a setup failure must put the caller's session back"
        );

        // No captured mode means nothing to restore.
        store
            .set_mode_and_persist(&id, SerializableMode::Plan)
            .unwrap();
        restore_plan_mode(&store, &id, None, "timeout");
        assert_eq!(
            store.mode_state(&id).mode,
            SerializableMode::Plan,
            "an unarmed restore must not touch the session"
        );
        // `_tmp_cleanup` removes the scratch dir; `_env` restores
        // PINVOU3_HOME.
    }

    /// The store-side half of the eviction-warning contract: retention sweep
    /// deletions are recorded as real eviction events and a save below the
    /// cap records nothing. The runner's own arm/report half is pinned by
    /// `retention_eviction_warning_keys_on_the_record_regardless_of_outcome`
    /// below.
    ///
    /// Seeds HEADLESS sessions, because headless runs are evicted against
    /// their own budget: a run can no longer delete one of the user's GUI
    /// chats (`headless_sessions_do_not_evict_gui_chats` pins that side), so
    /// filling the chat bucket here would evict nothing and the observer
    /// contract would go untested.
    #[test]
    fn retention_sweep_records_real_evictions_and_below_cap_stays_silent() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-agentic-retention-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _tmp_cleanup = TempDirGuard::new(tmp.clone());
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot");

        let mut ids = Vec::new();
        for index in 0..MAX_HEADLESS_SESSIONS {
            let id = format!("agentic_seed_{index}");
            store
                .create_empty_with_id(id.clone(), "test-model".to_string(), None, tmp.clone())
                .unwrap();
            ids.push(id);
        }

        // Arm the same receiver `run_agentic_task` installs around the turn.
        let evictions =
            std::sync::Arc::new(parking_lot::Mutex::new(RetentionEvictionRecord::default()));
        store.set_retention_eviction_observer(Some(evictions.clone()));

        // The prepare-time save of a fresh run at the headless cap — the
        // exact call `prepare_eval_session` makes — evicts the oldest
        // headless session...
        let oldest = ids[0].clone();
        store
            .create_empty_with_id(
                "agentic_probe_1".to_string(),
                "test-model".to_string(),
                None,
                tmp.clone(),
            )
            .unwrap();
        assert!(
            store.load(&oldest).is_err(),
            "oldest session must be evicted by the save at the cap"
        );
        // ...and the record stands on its own: an attachment/submit failure
        // after this point returns `Err`, but the eviction already happened
        // and the warning must still see it (no turn outcome consulted).
        assert_eq!(evictions.lock().ids.as_slice(), &[oldest]);
        assert_eq!(evictions.lock().total, 1);

        // Disarm exactly like the runner does before reporting.
        let evicted = store.take_retention_eviction_observer().unwrap();
        assert_eq!(evicted.lock().ids.as_slice(), &[ids[0].clone()]);

        // Below the cap a fresh save evicts nothing and records nothing.
        store.delete(&ids[MAX_HEADLESS_SESSIONS - 1]).unwrap();
        let evictions =
            std::sync::Arc::new(parking_lot::Mutex::new(RetentionEvictionRecord::default()));
        store.set_retention_eviction_observer(Some(evictions.clone()));
        store
            .create_empty_with_id(
                "agentic_probe_2".to_string(),
                "test-model".to_string(),
                None,
                tmp.clone(),
            )
            .unwrap();
        assert!(
            evictions.lock().total == 0,
            "a save below the cap must not be reported as an eviction"
        );
        store.take_retention_eviction_observer();
        // `_tmp_cleanup` removes the scratch dir; `_env` restores
        // PINVOU3_HOME.
    }

    /// Re-arming over an observer a previous run left behind (an unwind
    /// between its arm and disarm) must REPLACE it, not adopt it: the fresh
    /// receiver collects this run's evictions, and the stale receiver keeps
    /// its unreported record to itself — a dead receiver's contents silently
    /// bleeding into this run's warning would misreport evictions this run
    /// never caused.
    #[test]
    fn stale_retention_observer_is_replaced_and_its_record_stays_private() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-agentic-stale-observer-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot");

        // A previous run's receiver, still installed and holding content it
        // never reported.
        let stale = std::sync::Arc::new(parking_lot::Mutex::new(RetentionEvictionRecord {
            ids: vec!["agentic_stale_ghost".to_string()],
            total: 1,
        }));
        let previous = store.set_retention_eviction_observer(Some(stale.clone()));
        assert!(
            previous.is_none(),
            "precondition: no observer armed before the stale one"
        );

        // Re-arm exactly like the next run does. The slot now holds a fresh
        // receiver; the stale one is dropped by the arming and stays private.
        let fresh = arm_retention_eviction_observer(&store);

        // The next eviction lands in the fresh receiver only.
        let mut ids = Vec::new();
        for index in 0..MAX_HEADLESS_SESSIONS {
            let id = format!("agentic_stale_seed_{index}");
            store
                .create_empty_with_id(id.clone(), "test-model".to_string(), None, tmp.clone())
                .unwrap();
            ids.push(id);
        }
        let oldest = ids[0].clone();
        store
            .create_empty_with_id(
                "agentic_stale_probe".to_string(),
                "test-model".to_string(),
                None,
                tmp.clone(),
            )
            .unwrap();
        assert!(
            store.load(&oldest).is_err(),
            "oldest session must be evicted by the save at the cap"
        );
        assert_eq!(
            fresh.lock().ids.as_slice(),
            &[oldest],
            "the fresh receiver records this run's eviction"
        );
        assert_eq!(
            stale.lock().ids.as_slice(),
            ["agentic_stale_ghost".to_string()].as_slice(),
            "the replaced receiver must not adopt this run's evictions"
        );
        store.take_retention_eviction_observer();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The staged-copy sweep must reclaim copies only while the record
    /// provably holds no messages: an admitted turn's transcript references
    /// them, and a read error keeps them (the safe direction). Fresh sessions
    /// are the stub cleanup's job and are skipped entirely.
    #[test]
    fn staged_copy_sweep_gates_on_an_untouched_transcript() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-agentic-staged-sweep-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot");

        // An existing session with no messages: the staged copies are swept.
        store
            .create_empty_with_id(
                "agentic_sweep_empty".to_string(),
                "test-model".to_string(),
                None,
                tmp.clone(),
            )
            .unwrap();
        let swept = tmp.join("staged-swept.bin");
        std::fs::write(&swept, b"x").unwrap();
        sweep_unreferenced_staged_copies(&store, "agentic_sweep_empty", true, &[swept.clone()]);
        assert!(
            !swept.exists(),
            "an untouched transcript's staged copies must be reclaimed"
        );

        // An existing session whose transcript holds messages: the copies
        // stay — the admitted turn's transcript references them.
        store
            .create_empty_with_id(
                "agentic_sweep_talking".to_string(),
                "test-model".to_string(),
                None,
                tmp.clone(),
            )
            .unwrap();
        store
            .update_messages(
                "agentic_sweep_talking",
                vec![Message {
                    role: "user".into(),
                    content: vec![ContentBlock::Text {
                        text: "hi".into(),
                        cache_control: None,
                    }],
                }],
            )
            .unwrap();
        let kept = tmp.join("staged-kept.bin");
        std::fs::write(&kept, b"x").unwrap();
        sweep_unreferenced_staged_copies(&store, "agentic_sweep_talking", true, &[kept.clone()]);
        assert!(
            kept.exists(),
            "a transcript with messages may reference the staged copies"
        );

        // A fresh session: skipped — its stub cleanup removes the copies with
        // the whole record directory.
        let fresh = tmp.join("staged-fresh.bin");
        std::fs::write(&fresh, b"x").unwrap();
        sweep_unreferenced_staged_copies(&store, "agentic_never_created", false, &[fresh.clone()]);
        assert!(fresh.exists(), "fresh sessions are the stub cleanup's job");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The observer forwards HEADLESS-budget evictions only. A chat-budget
    /// eviction by the same sweep (possible when a caller-provided session id
    /// without the `agentic_` prefix pushes the chat bucket over its cap —
    /// the disclosed caller-id limitation) is the chat budget's own
    /// enforcement and must not be reported as headless retention pressure:
    /// the warning copy counts headless evictions only.
    #[test]
    fn retention_sweep_does_not_record_chat_budget_evictions() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-agentic-chat-eviction-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot");

        // Fill the CHAT budget to its cap with plain (non-`agentic_`) ids.
        let mut chat_ids = Vec::new();
        for index in 0..MAX_SESSIONS_PER_KIND {
            let id = format!("chat_seed_{index}");
            store
                .create_empty_with_id(id.clone(), "test-model".to_string(), None, tmp.clone())
                .unwrap();
            chat_ids.push(id);
        }

        let evictions =
            std::sync::Arc::new(parking_lot::Mutex::new(RetentionEvictionRecord::default()));
        store.set_retention_eviction_observer(Some(evictions.clone()));

        // One over the CHAT cap: the sweep evicts the oldest chat session...
        let oldest = chat_ids[0].clone();
        store
            .create_empty_with_id(
                "chat_probe_1".to_string(),
                "test-model".to_string(),
                None,
                tmp.clone(),
            )
            .unwrap();
        assert!(
            store.load(&oldest).is_err(),
            "oldest chat session must be evicted by the save at the chat cap"
        );
        // ...but the observer records nothing: chat-budget evictions are not
        // headless retention pressure.
        assert_eq!(
            evictions.lock().total,
            0,
            "chat-budget evictions must not be recorded as headless evictions"
        );
        store.take_retention_eviction_observer();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Seed `count` chat session records with the persist-only primitive
    /// (`save_session_atomic`, no per-save retention reconcile), so the home
    /// genuinely sits OVER the cap before the store under test boots. Going
    /// through `create_new`/`create_empty_with_id` would self-limit at the
    /// cap on every save and the boot-time sweep would have nothing to
    /// evict. Returns the seeded ids in creation order.
    fn seed_sessions_over_cap(tmp: &std::path::Path, count: usize) -> Vec<String> {
        use deepseek_tui::session_manager::create_saved_session_with_id_and_mode;
        let seeder =
            SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("seed boot");
        let mut ids = Vec::new();
        for i in 0..count {
            let id = format!("agentic_seed_{i:04}");
            let session = create_saved_session_with_id_and_mode(
                id.clone(),
                &[],
                "test-model",
                tmp,
                0,
                None,
                None,
            );
            seeder.save_session_atomic(&session).expect("seed save");
            ids.push(id);
        }
        ids
    }

    /// Boot-sweep evictions are no longer silent (the flush-on-arm half of
    /// the contract): the host boots — and sweeps — before the runner arms,
    /// so evictions a store over the cap makes at process start must sit in
    /// the pre-observer buffer and reach the run's warning once it arms. A
    /// disarm must keep buffering, so a later arm still sees what happened
    /// in between.
    #[test]
    fn boot_sweep_evictions_flush_into_the_observer_on_arm() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-agentic-boot-flush-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let seed_ids = seed_sessions_over_cap(&tmp, MAX_HEADLESS_SESSIONS + 3);

        // Boot on the over-cap home: the boot-time sweep evicts three
        // sessions with no observer installed — they must buffer.
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot");

        // Arming flushes the buffered boot sweep into the runner's record.
        let evictions =
            std::sync::Arc::new(parking_lot::Mutex::new(RetentionEvictionRecord::default()));
        store.set_retention_eviction_observer(Some(evictions.clone()));
        assert_eq!(
            evictions.lock().total,
            3,
            "boot-sweep evictions must flush on arm"
        );
        assert_eq!(evictions.lock().ids.len(), 3);
        let flushed_boot = evictions.lock().ids.clone();
        for id in &flushed_boot {
            assert!(seed_ids.contains(id), "flushed id must be a seeded one");
            assert!(
                store.load(id).is_err(),
                "flushed id must actually be deleted"
            );
        }

        // Disarm: a further save at the cap evicts again with no observer,
        // and the next arm must still see it (the buffer survives disarms).
        store.take_retention_eviction_observer();
        store
            .create_empty_with_id(
                "agentic_boot_flush_post_disarm".to_string(),
                "test-model".to_string(),
                None,
                tmp.clone(),
            )
            .unwrap();
        let rearmed =
            std::sync::Arc::new(parking_lot::Mutex::new(RetentionEvictionRecord::default()));
        store.set_retention_eviction_observer(Some(rearmed.clone()));
        assert_eq!(
            rearmed.lock().total,
            1,
            "an eviction between disarm and the next arm must survive in the buffer"
        );
        let flushed = rearmed.lock().ids.clone();
        assert_eq!(flushed.len(), 1);
        assert!(
            seed_ids.contains(&flushed[0]),
            "flushed id must be a seeded one"
        );
        assert!(
            store.load(&flushed[0]).is_err(),
            "flushed id must actually be deleted"
        );
        store.take_retention_eviction_observer();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `set_retention_eviction_observer(None)` is the disarm-without-
    /// installing form: it must flush nothing into anything and keep the
    /// pre-observer buffer intact, so the next real arm still sees what
    /// happened while disarmed. No production caller passes `None` today
    /// (the runner disarms via `take`), so this pins the documented contract
    /// of the defensive branch — silently turning it into a drop would
    /// otherwise pass the suite.
    #[test]
    fn disarming_with_none_keeps_buffering_for_the_next_arm() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-agentic-none-arm-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let seed_ids = seed_sessions_over_cap(&tmp, MAX_HEADLESS_SESSIONS + 1);

        // Boot one over the cap: the sweep evicted the oldest session with
        // no observer installed — it sits in the pending buffer.
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot");

        let previous = store.set_retention_eviction_observer(None);
        assert!(
            previous.is_none(),
            "nothing was installed before the None request"
        );

        let evictions =
            std::sync::Arc::new(parking_lot::Mutex::new(RetentionEvictionRecord::default()));
        store.set_retention_eviction_observer(Some(evictions.clone()));
        assert_eq!(
            evictions.lock().total,
            1,
            "a None arm must not drop the buffered boot eviction"
        );
        assert_eq!(
            evictions.lock().ids.as_slice(),
            &[seed_ids[0].clone()],
            "the buffered boot eviction must reach the next real arm"
        );
        store.take_retention_eviction_observer();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The runner's warning decision is a pure function of the recorded
    /// evictions: a non-empty record warns (the copy carries the pin
    /// exemption and the KEEP_SESSION pointer) and an empty record stays
    /// silent. The helper takes no turn outcome, so "a run that errors after
    /// the prepare-time save still surfaces the eviction" cannot regress
    /// behind an outcome gate — there is no outcome to gate on.
    #[test]
    fn retention_eviction_warning_keys_on_the_record_regardless_of_outcome() {
        let record = RetentionEvictionRecord {
            ids: vec!["evicted-id".to_string()],
            total: 1,
        };
        let warning =
            retention_eviction_warning(&record).expect("a non-empty eviction record must warn");
        assert!(warning.contains("1 non-pinned session"), "{warning}");
        assert!(warning.contains("pinned sessions are exempt"), "{warning}");
        assert!(
            warning.contains("PINVOU3_AGENT_TASK_KEEP_SESSION=0"),
            "{warning}"
        );
        // The count must come from `total`, never the windowed id vector:
        // a trimmed record still reports the full data loss. Also pins the
        // neutral attribution — the store's sweep is the actor, not this
        // run's persist (a boot-time eviction predates the run and must not
        // be blamed on it). The record itself only ever holds headless-budget
        // ids (chat-budget deletions never reach the observer), so the copy's
        // scope comes from the headless-cap qualifier, not a victim adjective.
        let trimmed = RetentionEvictionRecord {
            ids: vec!["windowed-id".to_string()],
            total: 270,
        };
        let warning = retention_eviction_warning(&trimmed).expect("total > 0 must warn");
        assert!(warning.contains("270"), "{warning}");
        assert!(
            !warning.contains("persisting this run's session evicted"),
            "the copy must not blame the run's persist: {warning}"
        );
        assert!(
            !warning.contains("headless run session"),
            "the count's scope is carried by the cap qualifier, not a victim label: {warning}"
        );
        // Nothing evicted — a below-cap save, or a run that failed before the
        // prepare-time save — must stay silent.
        assert!(retention_eviction_warning(&RetentionEvictionRecord::default()).is_none());
    }

    #[test]
    fn report_roundtrips_without_leaking_tool_payloads() {
        let report = AgenticTaskReport {
            session_id: "agentic_1_0".to_string(),
            status: "Completed".to_string(),
            timed_out: true,
            completed_after_deadline: true,
            submitted: true,
            assistant_text: "done".to_string(),
            tool_events: vec![AgenticToolEvent {
                name: "Bash".to_string(),
                failed: false,
            }],
            usage: None,
            error: None,
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"name\":\"Bash\""));
        assert!(json.contains("\"completed_after_deadline\":true"));
        // The no-payload contract is structural: a tool event serializes to
        // exactly the name/failed pair. The old `!json.contains("secret")`
        // assertion was vacuous — the fixture had no channel through which a
        // secret could reach the output, so it could never fail. Growing the
        // event struct a payload-bearing field (arguments, results) is
        // exactly the change that must turn this red and force a revisit of
        // the "safe to persist under /logs" contract.
        let event = serde_json::to_value(&report.tool_events[0]).unwrap();
        let event_keys: Vec<&str> = event
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            event_keys.len(),
            2,
            "tool events must carry no payload-bearing fields: {event_keys:?}"
        );
        assert!(event_keys.contains(&"name") && event_keys.contains(&"failed"));
        let parsed: AgenticTaskReport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, report);
    }

    #[test]
    fn report_deserializes_without_new_marker_field() {
        // Older reports (without completed_after_deadline) must still parse,
        // defaulting the marker to false — and `submitted` to false too:
        // the CLI's one-shot persona consume gates on that default, so a
        // flipped default would silently spend a staged persona body on a
        // timed-out setup.
        let json = r#"{
            "session_id":"agentic_1_0",
            "status":"timeout",
            "timed_out":true,
            "assistant_text":"",
            "tool_events":[],
            "usage":null,
            "error":null
        }"#;
        let parsed: AgenticTaskReport = serde_json::from_str(json).unwrap();
        assert!(!parsed.completed_after_deadline);
        assert!(!parsed.submitted, "the serde default must stay false");
        assert_eq!(parsed.status, "timeout");
    }

    #[test]
    fn max_timeout_secs_matches_the_cli_parse_cap() {
        // 7 days; the CLI parse cap and the library clamp must stay in lockstep
        // so `Instant + Duration` can never overflow.
        assert_eq!(MAX_TIMEOUT_SECS, 7 * 24 * 60 * 60);
    }

    /// The setup-timeout report's `submitted` flag must be derived from the
    /// submit admission boundary, not hard-coded: before the submit the turn
    /// provably never ran (`false`, setup-only wording); past it the submit
    /// was in flight and the turn may have been admitted (`true`, ambiguous
    /// wording) — the CLI's one-shot persona consume keys off this flag, so
    /// a hard `false` would stage a double injection on the next run.
    #[test]
    fn setup_timeout_report_carries_the_submit_boundary() {
        let before = setup_timeout_report("s-1", false);
        assert!(!before.submitted);
        assert_eq!(before.status, "timeout");
        assert!(before.timed_out);
        assert_eq!(
            before.error.as_deref(),
            Some("agentic session setup did not finish within the timeout"),
            "the provably-never-ran case keeps the setup-only wording"
        );

        let entered = setup_timeout_report("s-1", true);
        assert!(
            entered.submitted,
            "once the submit was entered the report must claim submitted — \
             the one-shot consume takes the at-least-once direction"
        );
        assert_eq!(entered.status, "timeout");
        let error = entered.error.as_deref().expect("error text present");
        assert!(
            error.contains("outcome is unknown"),
            "the entered case must name the ambiguity, not claim setup never finished: {error}"
        );
    }

    // -----------------------------------------------------------------------
    // Behavior-level pins for the run teardown. `run_agentic_task` itself
    // needs an `EnginePool` (→ Tauri `AppHandle`, no mock runtime in this
    // repo), so these tests drive the extracted orchestration directly with
    // a real `SessionStore` and a recording executor that mirrors the
    // production delegations. They pin what the 2026-09-25/26 reviews found
    // unprotected: the disposition→action WIRING (the enum was pinned, the
    // cascade was not), the default-keep teardown, and the stub cleanup.
    // -----------------------------------------------------------------------

    /// Mirrors [`AgenticTeardownExecutor for EnginePoolRuntime`]: deletes go
    /// to the real store (so assertions see durable state, not just calls),
    /// evictions/liveness are instrumented. `late_admission` simulates the
    /// stub-delete race: a transcript landing after the disposition sampled
    /// an empty record (injected at the liveness sample, the second of the
    /// two outside-the-gate samples).
    #[derive(Default)]
    struct RecordingTeardown {
        store: parking_lot::Mutex<Option<SessionStore>>,
        turn_active: parking_lot::Mutex<bool>,
        late_admission: parking_lot::Mutex<Option<String>>,
        deletes: parking_lot::Mutex<Vec<String>>,
        evictions: parking_lot::Mutex<Vec<String>>,
    }

    impl RecordingTeardown {
        fn new(store: &SessionStore) -> Self {
            Self {
                store: parking_lot::Mutex::new(Some(store.clone())),
                ..Self::default()
            }
        }
    }

    impl super::AgenticTeardownExecutor for RecordingTeardown {
        fn teardown_turn_active(&self, _session_id: &str) -> bool {
            if let Some(id) = self.late_admission.lock().clone() {
                let store = self.store.lock().as_ref().expect("store present").clone();
                seed_record(&store, &id, &[user_text("admitted after the samples")]);
            }
            *self.turn_active.lock()
        }

        async fn teardown_delete(&self, session_id: &str) -> anyhow::Result<()> {
            self.deletes.lock().push(session_id.to_owned());
            self.store
                .lock()
                .as_ref()
                .expect("store present")
                .delete(session_id)
        }

        async fn teardown_schedule_delete(&self, session_id: &str) -> anyhow::Result<()> {
            // Covers the DISPOSITION half only (this double performs the
            // unconditional store delete). Production `teardown_schedule_delete`
            // routes the durable delete through the adoption-gated
            // `delete_headless_session_unless_adopted` under the turn lock;
            // that gate half is pinned by
            // `one_shot_delete_gate_rechecks_the_adoption_marker` in
            // engine_pool, not here.
            self.teardown_delete(session_id).await
        }

        async fn teardown_delete_stub_if_still_empty(
            &self,
            session_id: &str,
        ) -> anyhow::Result<()> {
            // Covers the message-free half of the production guard in
            // `EnginePool::delete_chat_session_if_still_empty`; the
            // production DeleteGateRecheck also requires the record to still
            // read factory-titled, which this test double deliberately does
            // not reproduce — production is strictly more conservative, so
            // every directory this double deletes is one production may
            // delete too.
            let store = self.store.lock().as_ref().expect("store present").clone();
            if !matches!(store.chat_session_has_messages(session_id), Ok(false)) {
                self.evictions.lock().push(session_id.to_owned());
                return Ok(());
            }
            self.teardown_delete(session_id).await
        }

        async fn teardown_evict(&self, session_id: &str) {
            self.evictions.lock().push(session_id.to_owned());
        }
    }

    fn lifecycle_home(tag: &str) -> (SessionStore, std::path::PathBuf) {
        // The caller holds ENV_LOCK via `locked_env` and has already set
        // PINVOU3_HOME to the scratch home — `boot_inner_with` resolves
        // `paths::sessions_root()` from it at boot, so the store and every
        // seeded record land inside the scratch home. SAFETY: ENV_LOCK held;
        // env writes are serialized across tests.
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-agentic-lifecycle-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let store =
            SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot store");
        (store, tmp)
    }

    fn seed_record(store: &SessionStore, id: &str, messages: &[deepseek_tui::models::Message]) {
        let mut session = deepseek_tui::session_manager::create_saved_session_with_id_and_mode(
            id.to_owned(),
            messages,
            "lifecycle-model",
            &std::env::temp_dir(),
            0,
            None,
            None,
        );
        session.metadata.updated_at = chrono::Utc::now();
        store
            .save_session_atomic(&session)
            .expect("seed record without eager retention");
    }

    fn user_text(text: &str) -> deepseek_tui::models::Message {
        deepseek_tui::models::Message {
            role: "user".into(),
            content: vec![deepseek_tui::models::ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        }
    }

    /// The Err-arm admission decision, pinned against a real store: a
    /// pre-`submit_entered` failure restores even without a snapshot; only
    /// an unchanged transcript revision (nothing landed) is otherwise
    /// restore-eligible; a landed append (the late-fault submit), a missing
    /// pre-submit snapshot past the gate, or an unreadable record all count
    /// as admitted — restoring pins over a possibly admitted turn is the
    /// unsafe direction.
    #[test]
    fn submit_err_admission_decides_from_the_transcript_revision() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let (store, tmp) = lifecycle_home("submit-admission");
        // The scratch home must not leak per CI run (every sibling wraps it
        // in the guard; this test's binding was the one that didn't).
        let _tmp_cleanup = TempDirGuard::new(tmp);

        seed_record(&store, "adm_probe", &[user_text("first")]);
        let pre = super::transcript_revision(&store.load("adm_probe").unwrap().messages).unwrap();

        // Failure before the submit was polled (staging/roots — no snapshot
        // exists yet) → provably nothing admitted, the pins restore.
        assert!(!super::submit_err_admitted_turn(
            &store,
            "adm_probe",
            false,
            None
        ));

        // Submit failed before the durable append (nothing landed) → the
        // caller-provided session's pins restore.
        assert!(!super::submit_err_admitted_turn(
            &store,
            "adm_probe",
            true,
            Some(&pre)
        ));

        // Submit admitted the message and only then faulted (the engine can
        // append before its spawn/send fails) → the pins stay.
        seed_record(
            &store,
            "adm_probe",
            &[user_text("first"), user_text("second")],
        );
        assert!(super::submit_err_admitted_turn(
            &store,
            "adm_probe",
            true,
            Some(&pre)
        ));

        // No pre-submit snapshot (fresh session, or the snapshot read
        // failed) past the gate → assume admission.
        assert!(super::submit_err_admitted_turn(
            &store,
            "adm_probe",
            true,
            None
        ));

        // Record unreadable after the error → unknown ⇒ keep.
        seed_record(&store, "adm_probe", &[user_text("first")]);
        store.delete("adm_probe").unwrap();
        assert!(super::submit_err_admitted_turn(
            &store,
            "adm_probe",
            true,
            Some(&pre)
        ));
    }

    fn set_keep_session(value: Option<&str>) {
        // SAFETY: ENV_LOCK held via locked_env in the caller; env writes are
        // serialized across tests.
        unsafe {
            match value {
                Some(value) => std::env::set_var("PINVOU3_AGENT_TASK_KEEP_SESSION", value),
                None => std::env::remove_var("PINVOU3_AGENT_TASK_KEEP_SESSION"),
            }
        }
    }

    /// The KEEP_SESSION matrix for a SUBMITTED run (the headline breaking
    /// change): the session stays by default; only the explicit falsy opt-in
    /// restores the legacy one-shot delete.
    #[tokio::test]
    async fn submitted_run_keeps_session_by_default_and_deletes_on_falsy_opt_in() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME", "PINVOU3_AGENT_TASK_KEEP_SESSION"]);
        let (store, tmp) = lifecycle_home("submitted");

        // Default (unset) → keep: engine reclaimed, record survives.
        set_keep_session(None);
        seed_record(&store, "agentic_matrix_default", &[user_text("hi")]);
        // Production mints a fresh run session with the factory title; the
        // seeder derives a prompt title, so restore the run's own state.
        store
            .set_title("agentic_matrix_default", NEW_CHAT_TITLE.to_string())
            .unwrap();
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(
            &executor,
            &store,
            "agentic_matrix_default",
            false,
            true,
            false,
        )
        .await;
        assert!(
            store.chat_session_record_exists("agentic_matrix_default"),
            "default keep: the run's session record must stay for continuation"
        );
        assert_eq!(executor.evictions.lock().len(), 1, "engine reclaimed");
        assert!(
            executor.deletes.lock().is_empty(),
            "default keep: nothing may be deleted"
        );

        // Explicit falsy → legacy one-shot cleanup: the record is deleted,
        // whether the run completed or errored (the report carries the
        // outcome; the teardown does not consult it).
        for falsy in ["0", "false", "no", "off", "FALSE"] {
            set_keep_session(Some(falsy));
            seed_record(&store, "agentic_matrix_falsy", &[user_text("hi")]);
            store
                .set_title("agentic_matrix_falsy", NEW_CHAT_TITLE.to_string())
                .unwrap();
            let executor = RecordingTeardown::new(&store);
            super::run_session_lifecycle(
                &executor,
                &store,
                "agentic_matrix_falsy",
                false,
                true,
                false,
            )
            .await;
            assert!(
                !store.chat_session_record_exists("agentic_matrix_falsy"),
                "KEEP_SESSION={falsy}: the legacy one-shot cleanup must delete the record"
            );
            assert!(
                executor.evictions.lock().is_empty(),
                "KEEP_SESSION={falsy}: no engine reclaim on the delete lane"
            );
            store.delete("agentic_matrix_falsy").ok();
        }

        // Truthy legacy values still mean keep.
        set_keep_session(Some("0-nope"));
        seed_record(&store, "agentic_matrix_truthy", &[user_text("hi")]);
        store
            .set_title("agentic_matrix_truthy", NEW_CHAT_TITLE.to_string())
            .unwrap();
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(
            &executor,
            &store,
            "agentic_matrix_truthy",
            false,
            true,
            false,
        )
        .await;
        assert!(
            store.chat_session_record_exists("agentic_matrix_truthy"),
            "a non-falsy KEEP_SESSION keeps the session"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The never-started matrix: a zero-message stub is cleaned up regardless
    /// of KEEP_SESSION; a started-untracked transcript (durable messages, or
    /// the engine still running) stays inspectable unless the falsy opt-in;
    /// an unloadable record keeps (deleting on unknown state is unsafe).
    #[tokio::test]
    async fn never_started_stub_matrix_cleans_stubs_and_keeps_started_transcripts() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME", "PINVOU3_AGENT_TASK_KEEP_SESSION"]);
        let (store, tmp) = lifecycle_home("unsubmitted");

        // Zero-message FACTORY-TITLED stub + default keep → deleted anyway
        // (CleanupStub): a stub has no transcript to inspect and becomes
        // eviction bait. (The factory title is what makes it the run's to
        // delete — see the adopted-stub row below.)
        set_keep_session(None);
        seed_record(&store, "agentic_stub_keep", &[]);
        store
            .set_title("agentic_stub_keep", NEW_CHAT_TITLE.to_string())
            .unwrap();
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(&executor, &store, "agentic_stub_keep", false, false, false)
            .await;
        assert!(
            !store.chat_session_record_exists("agentic_stub_keep"),
            "a zero-message factory-titled stub must clean up even under default keep"
        );

        // Zero-message factory-titled stub + falsy → same delete (already
        // covered by CleanupStub before KEEP_SESSION is even consulted).
        seed_record(&store, "agentic_stub_falsy", &[]);
        store
            .set_title("agentic_stub_falsy", NEW_CHAT_TITLE.to_string())
            .unwrap();
        set_keep_session(Some("0"));
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(&executor, &store, "agentic_stub_falsy", false, false, false)
            .await;
        assert!(
            !store.chat_session_record_exists("agentic_stub_falsy"),
            "a zero-message stub is cleanup-eligible regardless of KEEP_SESSION"
        );

        // Zero-message ADOPTED stub (renamed before the run faults) + falsy →
        // KEEPS: the rename is ownership on the never-started lane too — the
        // round-25 review hole, pinned end-to-end (the pure matrix pins the
        // decision; this pins the whole lifecycle path).
        seed_record(&store, "agentic_stub_adopted", &[]);
        store
            .set_title("agentic_stub_adopted", "User renamed this".to_string())
            .unwrap();
        set_keep_session(Some("0"));
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(
            &executor,
            &store,
            "agentic_stub_adopted",
            false,
            false,
            false,
        )
        .await;
        assert!(
            store.chat_session_record_exists("agentic_stub_adopted"),
            "a renamed zero-message stub keeps even under the falsy opt-in"
        );

        // Started transcript (durable record carries admitted messages) +
        // default → stays inspectable.
        seed_record(&store, "agentic_started_keep", &[user_text("hi")]);
        store
            .set_title("agentic_started_keep", NEW_CHAT_TITLE.to_string())
            .unwrap();
        set_keep_session(None);
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(
            &executor,
            &store,
            "agentic_started_keep",
            false,
            false,
            false,
        )
        .await;
        assert!(
            store.chat_session_record_exists("agentic_started_keep"),
            "a started-but-unsubmitted transcript is the only copy — it stays"
        );
        assert_eq!(executor.evictions.lock().len(), 1, "engine reclaimed");

        // Same started transcript + falsy → legacy cleanup deletes it.
        seed_record(&store, "agentic_started_falsy", &[user_text("hi")]);
        store
            .set_title("agentic_started_falsy", NEW_CHAT_TITLE.to_string())
            .unwrap();
        set_keep_session(Some("off"));
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(
            &executor,
            &store,
            "agentic_started_falsy",
            false,
            false,
            false,
        )
        .await;
        assert!(
            !store.chat_session_record_exists("agentic_started_falsy"),
            "the falsy opt-in restores legacy cleanup for started-but-unsubmitted runs"
        );

        // Engine-activity override: zero durable messages yet the engine is
        // mid-turn → started, not a stub (the file write may lag admission).
        seed_record(&store, "agentic_engine_live", &[]);
        set_keep_session(None);
        let executor = RecordingTeardown::new(&store);
        *executor.turn_active.lock() = true;
        super::run_session_lifecycle(
            &executor,
            &store,
            "agentic_engine_live",
            false,
            false,
            false,
        )
        .await;
        assert!(
            store.chat_session_record_exists("agentic_engine_live"),
            "engine liveness overrides a stale zero-message snapshot"
        );

        // Unloadable record (no record at all) → keep: unknown state must
        // not be deleted, even under the falsy opt-in.
        set_keep_session(Some("0"));
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(
            &executor,
            &store,
            "agentic_ghost_never_seeded",
            false,
            false,
            false,
        )
        .await;
        assert!(
            executor.deletes.lock().is_empty(),
            "an unloadable/missing record must keep — deleting on unknown state is unsafe"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The setup timeout is the one never-submitted path that produces a
    /// report, and that report names the run's `session_id`: under the
    /// default keep contract the session must stay resolvable — no silent
    /// stub cleanup behind an id the caller was just handed. Only the
    /// explicit one-shot opt-in may still clean it up, under the same
    /// adoption rule as every submitted run.
    #[tokio::test]
    async fn a_setup_timeout_keeps_the_reported_session_resolvable() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME", "PINVOU3_AGENT_TASK_KEEP_SESSION"]);
        let (store, tmp) = lifecycle_home("setup-timeout");

        // Default keep: the reported id stays resolvable.
        set_keep_session(None);
        seed_record(&store, "agentic_timeout_keep", &[]);
        store
            .set_title("agentic_timeout_keep", NEW_CHAT_TITLE.to_string())
            .unwrap();
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(
            &executor,
            &store,
            "agentic_timeout_keep",
            false,
            false,
            true, // setup timeout: the report carries this session_id
        )
        .await;
        assert!(
            store.chat_session_record_exists("agentic_timeout_keep"),
            "the setup timeout's session_id was reported — the session must stay resolvable"
        );
        assert_eq!(executor.evictions.lock().len(), 1, "engine reclaimed");
        assert!(
            executor.deletes.lock().is_empty(),
            "no silent stub cleanup behind a reported session id"
        );

        // One-shot opt-in + factory title: the caller asked for a clean
        // sandbox, and the timeout session is still the run's own.
        set_keep_session(Some("0"));
        seed_record(&store, "agentic_timeout_falsy", &[]);
        store
            .set_title("agentic_timeout_falsy", NEW_CHAT_TITLE.to_string())
            .unwrap();
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(
            &executor,
            &store,
            "agentic_timeout_falsy",
            false,
            false,
            true,
        )
        .await;
        assert!(
            !store.chat_session_record_exists("agentic_timeout_falsy"),
            "the one-shot opt-in owns the run's own timeout session too"
        );

        // One-shot opt-in + adopted (renamed): the rename is ownership.
        set_keep_session(Some("0"));
        seed_record(&store, "agentic_timeout_adopted", &[user_text("hi")]);
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(
            &executor,
            &store,
            "agentic_timeout_adopted",
            false,
            false,
            true,
        )
        .await;
        assert!(
            store.chat_session_record_exists("agentic_timeout_adopted"),
            "KEEP_SESSION=0 must not delete an adopted session record"
        );
        assert!(
            executor.deletes.lock().is_empty(),
            "an adopted record must not reach the delete lane"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The adoption exception on the submitted and started one-shot lanes:
    /// `KEEP_SESSION=0` restores the legacy cleanup for the run's own
    /// factory-titled session, but a record a GUI user renamed away from the
    /// placeholder is adopted and keeps — only the engine is reclaimed.
    #[tokio::test]
    async fn the_one_shot_opt_in_preserves_an_adopted_session() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME", "PINVOU3_AGENT_TASK_KEEP_SESSION"]);
        let (store, tmp) = lifecycle_home("adopted");
        set_keep_session(Some("0"));

        // Submitted run + renamed record: kept.
        seed_record(&store, "agentic_adopted_submitted", &[user_text("hi")]);
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(
            &executor,
            &store,
            "agentic_adopted_submitted",
            false,
            true,
            false,
        )
        .await;
        assert!(
            store.chat_session_record_exists("agentic_adopted_submitted"),
            "KEEP_SESSION=0 must not delete a renamed (adopted) session"
        );
        assert_eq!(executor.evictions.lock().len(), 1, "engine reclaimed");
        assert!(executor.deletes.lock().is_empty());

        // Started-never-submitted run + renamed record: kept too.
        seed_record(&store, "agentic_adopted_started", &[user_text("hi")]);
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(
            &executor,
            &store,
            "agentic_adopted_started",
            false,
            false,
            false,
        )
        .await;
        assert!(
            store.chat_session_record_exists("agentic_adopted_started"),
            "the started one-shot lane respects adoption the same way"
        );
        assert!(executor.deletes.lock().is_empty());

        // Control: the same lane still deletes the factory-titled record
        // (the legacy contract for the run's own session — pinned above by
        // the falsy arms of the matrix tests; this assert documents the
        // pairing with the adopted cases).
        seed_record(&store, "agentic_adopted_control", &[user_text("hi")]);
        store
            .set_title("agentic_adopted_control", NEW_CHAT_TITLE.to_string())
            .unwrap();
        let executor = RecordingTeardown::new(&store);
        super::run_session_lifecycle(
            &executor,
            &store,
            "agentic_adopted_control",
            false,
            true,
            false,
        )
        .await;
        assert!(
            !store.chat_session_record_exists("agentic_adopted_control"),
            "the factory-titled counterpart is still the run's own record"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The stub-delete race: the disposition samples `has_messages` BEFORE
    /// `turn_active`, and a turn can be admitted after BOTH samples read
    /// negative (lazy spawn around the failed submit; forwarder lag outlasting
    /// them). The guarded stub delete must therefore re-check emptiness at
    /// action time: a record that gained messages between the samples is a
    /// started transcript and is kept, never destroyed as a stub.
    #[tokio::test]
    async fn stub_cleanup_keeps_a_record_that_gains_messages_after_the_disposition() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME", "PINVOU3_AGENT_TASK_KEEP_SESSION"]);
        let (store, tmp) = lifecycle_home("stub-race");

        set_keep_session(None);
        seed_record(&store, "agentic_stub_race", &[]);
        let executor = RecordingTeardown::new(&store);
        // Simulates the transcript landing after the disposition's message
        // sample (injected at the liveness sample, which runs second).
        *executor.late_admission.lock() = Some("agentic_stub_race".to_owned());
        super::run_session_lifecycle(&executor, &store, "agentic_stub_race", false, false, false)
            .await;
        assert!(
            store.chat_session_record_exists("agentic_stub_race"),
            "a transcript admitted between the samples and the delete must \
             survive the stub cleanup as a started transcript"
        );
        assert!(
            executor.deletes.lock().is_empty(),
            "the guarded stub delete must not issue a durable delete for a \
             record that gained messages"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A caller-provided session is never auto-deleted by this run — even
    /// under the falsy opt-in, only the engine is reclaimed.
    #[tokio::test]
    async fn existing_session_is_never_auto_deleted() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME", "PINVOU3_AGENT_TASK_KEEP_SESSION"]);
        let (store, tmp) = lifecycle_home("existing");
        seed_record(&store, "agentic_caller_session", &[user_text("hi")]);
        for falsy in [None, Some("0")] {
            set_keep_session(falsy);
            let executor = RecordingTeardown::new(&store);
            super::run_session_lifecycle(
                &executor,
                &store,
                "agentic_caller_session",
                true,  // existing (caller-provided) session
                false, // and even a never-started outcome must not delete it
                false, // and a setup timeout is no different for this arm
            )
            .await;
            assert!(
                store.chat_session_record_exists("agentic_caller_session"),
                "a caller-provided session is never auto-deleted by this run"
            );
            assert!(
                executor.deletes.lock().is_empty(),
                "no delete may be issued for a caller-provided session"
            );
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Fresh ids regenerate while the id is taken; a free seed is accepted
    /// unchanged (no busy regeneration loop on the happy path).
    #[test]
    fn mint_fresh_session_id_regenerates_while_taken() {
        let taken = "agentic_recycled_pid_0";
        let minted = super::mint_fresh_session_id(|id| id == taken, taken.to_owned());
        assert_ne!(
            minted, taken,
            "a taken id must be regenerated, never returned (PID-reuse overwrite)"
        );
        assert!(
            minted.starts_with("agentic_"),
            "regeneration mints real headless ids: {minted}"
        );
        // The free-seed path returns the seed itself — the run keeps using
        // the caller-visible counter, not a different id mid-run.
        let free = "agentic_free_seed_0";
        let minted = super::mint_fresh_session_id(|_| false, free.to_owned());
        assert_eq!(minted, free);
    }

    #[test]
    fn fresh_session_id_keeps_store_alphabet_and_components() {
        let first = fresh_session_id();
        let second = fresh_session_id();
        let prefix = crate::features::sessions::HEADLESS_SESSION_PREFIX;
        for id in [&first, &second] {
            assert!(
                id.starts_with(crate::features::sessions::HEADLESS_SESSION_PREFIX),
                "{id} must keep the headless prefix"
            );
            assert!(
                id.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                "{id} must stay inside the session id alphabet [A-Za-z0-9_-]"
            );
            // pid + unix-millis + counter: all three components must be
            // numeric so ids stay inspectable and sortable, and the millis
            // component keeps cross-restart collisions in one process rare.
            let parts: Vec<&str> = id
                .strip_prefix(prefix)
                .unwrap_or_else(|| panic!("{id} must carry the {prefix} prefix"))
                .split('_')
                .collect();
            assert_eq!(
                parts.len(),
                3,
                "{id} must be <prefix><pid>_<unix_millis>_<counter>"
            );
            assert!(
                parts[0].parse::<u32>().is_ok(),
                "{id} pid component must be numeric"
            );
            assert!(
                parts[1].parse::<u64>().is_ok(),
                "{id} unix-millis component must be numeric"
            );
            assert!(
                parts[2].parse::<u64>().is_ok(),
                "{id} counter component must be numeric"
            );
        }
        // The per-process counter keeps consecutive ids distinct even within
        // the same millisecond; cross-restart and cross-process collisions
        // are mint_fresh_session_id's job (see its pins above).
        assert_ne!(first, second);
    }

    #[test]
    fn a_panic_inside_setup_restores_the_caller_session_pins() {
        // Round-21 review finding (panic-unsafe restore arms): a panic
        // inside the setup future — anywhere between the pin writes and the
        // submit — used to skip the Err and timeout restore arms entirely,
        // so a caller-provided session stayed repinned in Plan / the new
        // model by a run that never did any work. The catch_unwind arm
        // restores both pins before re-raising; this test pins the arm's
        // restore composition on the store side, plus the unarmed case.
        //
        // Round-22 review finding: the arm's model half used an inline
        // `Handle::block_on`, which panics inside the runtime worker that
        // always drives this code — the model pin was never restored and the
        // original panic was masked. The arm now diverts the panic payload
        // out of the closure and calls the SAME async
        // `restore_pre_run_pins` the Err and timeout arms use, so the model
        // half is the already-covered pool path. The arm's own sequencing
        // (gate → restore → resume) is pinned separately by
        // `the_panic_arm_restores_before_resuming_and_gates_on_submit_entered`,
        // which drives a real panicking setup future.
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-agentic-panic-restore-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _tmp_cleanup = TempDirGuard::new(tmp.clone());
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot");
        let chat = store
            .create_new("test-model".to_string(), None, tmp.clone())
            .unwrap();
        let id = chat.metadata.id.clone();

        // Armed: the user left the session in Yolo with a durable entry.
        // After a "panic" — the arm sequenced exactly as the code sequences
        // it — the entry the run added (Plan) is gone and Yolo is back.
        store
            .set_mode_and_persist(&id, SerializableMode::Plan)
            .unwrap();
        restore_plan_mode(
            &store,
            &id,
            Some(PlanModeRestore::Value(SerializableMode::Yolo)),
            "panic",
        );
        assert_eq!(
            store.durable_mode_entry(&id),
            Some(SerializableMode::Yolo),
            "the panic arm must put the caller's durable mode back"
        );
        assert_eq!(store.mode_state(&id).mode, SerializableMode::Yolo);

        // Absent-pre-state variant: a session with no durable entry before
        // the run returns to no entry, not to a resolved default.
        store.clear_mode_and_persist(&id).unwrap();
        store
            .set_mode_and_persist(&id, SerializableMode::Plan)
            .unwrap();
        restore_plan_mode(&store, &id, Some(PlanModeRestore::Absent), "panic");
        assert_eq!(
            store.durable_mode_entry(&id),
            None,
            "the panic arm must return the session to no entry, the pre-run state"
        );

        // Unarmed: a panic whose setup never armed a restore changes nothing.
        store
            .set_mode_and_persist(&id, SerializableMode::Plan)
            .unwrap();
        restore_plan_mode(&store, &id, None, "panic");
        assert_eq!(
            store.mode_state(&id).mode,
            SerializableMode::Plan,
            "an unarmed panic restore must not touch the session"
        );
    }

    /// Round-25 review finding: the store-side test above calls the restore
    /// helper directly, so nothing pinned the arm's own sequencing — a
    /// deleted `catch_unwind` arm, a dropped `submit_entered` gate, or a
    /// `resume_unwind` moved ahead of the restore would all have left the
    /// suite green. This test drives a REAL panicking setup future through
    /// the extracted arm ([`resume_unwind_after_pinned_restore`]) and
    /// observes the gate, the restore-before-resume ordering, and the
    /// resumed payload.
    #[test]
    fn the_panic_arm_restores_before_resuming_and_gates_on_submit_entered() {
        let runtime = tokio::runtime::Runtime::new().expect("test runtime");
        // The quiet hook is installed around EACH block_on and restored
        // IMMEDIATELY after it, before any assert runs: a leaked silencing
        // hook would blind every LATER test's panic output. (The restore
        // type is deliberately not named — the hook type moved across
        // toolchains, so the save/restore relies on inference only.)

        // Pre-submit (gate clear): the restore must COMPLETE before the
        // payload surfaces, and the payload must be the original one — not
        // masked by a restore failure or replaced by a synthetic error.
        let gate = std::sync::atomic::AtomicBool::new(false);
        let restored = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = restored.clone();
        let quiet = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let first = runtime.block_on(futures_util::FutureExt::catch_unwind(
            std::panic::AssertUnwindSafe(async move {
                let inner =
                    futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
                        panic!("setup exploded before the submit");
                    }))
                    .await;
                let payload = inner.expect_err("the setup future must panic");
                resume_unwind_after_pinned_restore(&gate, payload, move |_phase| {
                    let flag = flag.clone();
                    async move {
                        flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                })
                .await
            }),
        ));
        std::panic::set_hook(quiet);
        let resumed = first.expect_err("the resumed panic must surface after the restore");
        assert!(
            restored.load(std::sync::atomic::Ordering::SeqCst),
            "the panic arm must run the restore to completion BEFORE resuming the unwind"
        );
        assert_eq!(
            resumed.downcast_ref::<&'static str>(),
            Some(&"setup exploded before the submit"),
            "the original panic payload must be resumed, not masked"
        );

        // Past the submit boundary (gate set): the pins stay — no restore —
        // and the payload still resumes untouched.
        let gate = std::sync::atomic::AtomicBool::new(true);
        let restored = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = restored.clone();
        let quiet = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let second = runtime.block_on(futures_util::FutureExt::catch_unwind(
            std::panic::AssertUnwindSafe(async move {
                let inner =
                    futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
                        panic!("setup exploded past the submit");
                    }))
                    .await;
                let payload = inner.expect_err("the setup future must panic");
                resume_unwind_after_pinned_restore(&gate, payload, move |_phase| {
                    let flag = flag.clone();
                    async move {
                        flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                })
                .await
            }),
        ));
        std::panic::set_hook(quiet);
        let resumed = second
            .expect_err("the resumed panic must surface even when the gate holds the restore");
        assert!(
            !restored.load(std::sync::atomic::Ordering::SeqCst),
            "a panic past the submit boundary must NOT roll the pins back"
        );
        assert_eq!(
            resumed.downcast_ref::<&'static str>(),
            Some(&"setup exploded past the submit"),
        );
    }
}
