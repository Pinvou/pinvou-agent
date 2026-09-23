//! Headless single-task agentic entry for external harnesses
//! (Terminal-Bench/Harbor).
//!
//! Unlike the eval backend in [`super::headless_bridge`], this runs a
//! **product-equivalent** agentic turn: `TurnInput::eval_tool_policy = None` →
//! `EnginePool::send_user_message`, i.e. the exact path the GUI uses (Yolo
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
use std::sync::atomic::{AtomicU64, Ordering};
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
    ExecutionRootResolver, MAX_SESSIONS_PER_KIND, SessionKind, SessionStore,
    validate_user_workspace_path,
};
use crate::platform::prefs::UserPrefs;

const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// Upper bound for `timeout_secs`, mirroring the CLI parse cap: an unclamped
/// `u64` would overflow the internal `Instant + Duration` and panic before any
/// report is produced. The CLI enforces the same cap at parse time.
pub const MAX_TIMEOUT_SECS: u64 = 7 * 24 * 60 * 60;
/// Settle window after cancel: give the engine time to finish persisting;
/// past the window, give up waiting for a full turn result.
const CANCEL_SETTLE_SECS: u64 = 30;
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
    /// isolated scratch as eval sessions). With `session_id`, this binds the
    /// existing session to the directory only when the caller provides it;
    /// without a `workspace`, the session keeps its own resolution (its
    /// private scratch), never overriding a pre-existing binding.
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
    /// cleanup mode. Errors: unknown session → `agent_session_not_found`;
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
    /// Pin the session's model for this run, like the GUI per-session model
    /// switch: validated against the configured model list
    /// (`UserPrefs::model_by_id`, the same check the `set_session_model`
    /// command performs), then bound through the eval model-selection route
    /// for a fresh session or the GUI chip-switch path (sidecar write +
    /// engine evict) for an existing session. Unknown model →
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

impl Default for AgenticTaskRequest {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            workspace: None,
            timeout_secs: default_timeout_secs(),
            session_id: None,
            mode: None,
            model_id: None,
            attachments: Vec::new(),
        }
    }
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
/// instead — the CLI surfaces those as exit 1 without a report.
///
/// Persisting counts against the shared 50-session retention cap: when a
/// fresh run's prepare-time save evicts chat sessions at the cap (pinned
/// sessions are exempt from retention), the store's real eviction events
/// drive a stderr warning, so a batch harness pointed at the desktop's
/// default `PINVOU3_HOME` is not silent about the data loss — even when the
/// run errors after the save.
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
        // Fresh ids persist for good now, so a recycled pid replaying the
        // same counter must not silently overwrite a kept session's record
        // (transcript loss, and the old pin would transfer to the new stub):
        // regenerate until the id is free.
        None => {
            let mut session_id = fresh_session_id();
            while store.chat_session_record_exists(&session_id) {
                session_id = fresh_session_id();
            }
            session_id
        }
    };

    // Execution root binding: the closure only matches this run's session id
    // (fresh or caller-provided); resolution for every other session stays
    // unchanged. A caller-provided session is bound to `workspace` only when
    // the request carries one — an unset workspace keeps the session's own
    // resolution (its private scratch) instead of overriding it.
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
    let resolver_workspace = bound_workspace.clone();
    let matched_session = session_id.clone();
    let resolver: ExecutionRootResolver = Arc::new(move |id: &str| {
        (id == matched_session)
            .then(|| resolver_workspace.clone())
            .flatten()
    });
    let mut pool = pool;
    pool.bridge.set_execution_root_resolver(resolver.clone());
    store.set_execution_root_resolver(resolver);
    let runtime = EnginePoolRuntime::new(Arc::new(pool));

    // Retention-eviction observation: the prepare-time save inside the turn
    // lands in the same 50-session store the GUI reads, and a fresh save at
    // the cap evicts the oldest unpinned chat session(s) (pinned sessions are
    // exempt from retention). The store reports its real sweep deletions into
    // this receiver, so the warning keys on the eviction event itself: a run
    // that errors after the save (attachment staging, submit) must still
    // surface the eviction, and a run that fails before saving evicts nothing
    // and stays silent — a count sampled around the run cannot see mid-run
    // forwarder evictions, and the store's own deletions can.
    let evictions = Arc::new(Mutex::new(Vec::new()));
    if let Some(stale) = store.set_retention_eviction_observer(Some(evictions.clone())) {
        // Single-flight normally guarantees the slot is empty here; a stale
        // observer means an earlier run skipped its disarm (an unwind between
        // arm and disarm would do it). Its record was never reported (and may
        // be empty) — say so instead of silently adopting a dead receiver.
        drop(stale);
        eprintln!(
            "[pinvou agent run] warning: replaced a stale retention-eviction \
             observer; any eviction record the previous run left unreported \
             was discarded"
        );
    }
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

    // Session lifecycle after the turn: sessions persist by default (GUI
    // parity — the 50-session retention cap applies), so the engine is
    // reclaimed while the transcript, artifacts and timeline stay under the
    // sessions root for later continuation through the request's `session_id`
    // (a library surface; the one-shot CLI keeps its defaults today). Only an
    // explicit `PINVOU3_AGENT_TASK_KEEP_SESSION=0|false|no|off` restores the
    // old one-shot cleanup for harnesses that want a clean sandbox (the
    // legacy truthy values "1"/"true"/"yes"/"on" keep meaning keep).
    //
    // A fresh session whose turn never started (attachment staging, submit,
    // or the setup timeout) carries no transcript to inspect: keeping it
    // would litter the shared store — and the GUI history — with zero-message
    // stubs, one eviction apiece in a failing batch. Those runs clean up
    // after themselves regardless of `KEEP_SESSION` — where "never started"
    // is decided on the durable record: a stub that already carries admitted
    // messages is a started transcript and stays inspectable instead (see
    // the cleanup branch below).
    //
    // A caller-provided `session_id` is never auto-deleted by THIS run, but
    // it is an ordinary chat session in the store: the 50-session retention
    // sweep can still evict it later exactly like any GUI chat session.
    // Only this run's eval observation mark is dropped, and the session is
    // left in place for the caller.
    let keep_session = keep_session_from_env();
    if existing_session {
        crate::features::assistant::timing::unregister_eval_observation(&session_id);
    } else if !submitted {
        crate::features::assistant::timing::unregister_eval_observation(&session_id);
        // The submit boundary is not atomic with transcript admission: the
        // engine lazily spawns on submit and can durably admit the user
        // message before the fault surfaces (a submit error, or the setup
        // timeout landing right after admission). A record that carries
        // messages has therefore started — its transcript is the only copy,
        // so it stays inspectable like any submitted run unless the caller
        // explicitly opted back into the legacy one-shot cleanup. Only a
        // truly zero-message stub is cleanup-eligible regardless of
        // `KEEP_SESSION`; an unloadable record also keeps (deleting on
        // unknown state is the unsafe direction).
        match never_started_disposition(
            store.chat_session_has_messages(&session_id).map_err(|_| ()),
            keep_session,
        ) {
            NeverStartedDisposition::CleanupStub => {
                runtime.schedule_eval_cleanup(&session_id);
                log_cleanup_delete(&runtime, &session_id).await;
            }
            NeverStartedDisposition::KeepInspectable => runtime.pool.evict(&session_id).await,
            NeverStartedDisposition::LegacyCleanupStarted => {
                runtime.schedule_eval_cleanup(&session_id);
                log_cleanup_delete(&runtime, &session_id).await;
            }
        }
    } else if keep_session {
        crate::features::assistant::timing::unregister_eval_observation(&session_id);
        runtime.pool.evict(&session_id).await;
    } else {
        runtime.schedule_eval_cleanup(&session_id);
        log_cleanup_delete(&runtime, &session_id).await;
    }
    // Disarm before reporting: the prepare-time save happened before any setup
    // fault could surface, so the evictions are real regardless of the final
    // outcome — the report may carry an error, and the run may have cleaned
    // its own session up afterwards.
    store.take_retention_eviction_observer();
    if let Some(warning) = retention_eviction_warning(&evictions.lock()) {
        eprintln!("{warning}");
    }
    outcome
}

/// Best-effort cleanup delete: a failed delete must not mask the run's own
/// outcome, but silently stranding the session in the shared store hides the
/// failure from the operator — log it instead of discarding the result.
/// The session id stays out of the message (boot logs persist to disk and
/// the CodeQL cleartext-logging gate flags ids on stderr): the store's own
/// error chain names the session (`delete_session({id})`), so only the root
/// cause (the underlying io/serialization error) is printed — the full
/// `{error:#}` chain would carry the id right back into the log.
async fn log_cleanup_delete(runtime: &EnginePoolRuntime, session_id: &str) {
    if let Err(error) = runtime.close_eval_session_result(session_id).await {
        eprintln!("[agent-task] cleanup delete failed: {}", error.root_cause());
    }
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
/// `keep_session` only splits the started case: with the legacy one-shot
/// opt-in (`KEEP_SESSION=0|false|no|off`), a started-but-unsubmitted run is
/// still cleaned up per the old contract.
enum NeverStartedDisposition {
    CleanupStub,
    KeepInspectable,
    LegacyCleanupStarted,
}

fn never_started_disposition(
    has_messages: Result<bool, ()>,
    keep_session: bool,
) -> NeverStartedDisposition {
    match has_messages {
        Ok(false) => NeverStartedDisposition::CleanupStub,
        Ok(true) if keep_session => NeverStartedDisposition::KeepInspectable,
        Ok(true) => NeverStartedDisposition::LegacyCleanupStarted,
        Err(_) => NeverStartedDisposition::KeepInspectable,
    }
}

/// The retention-eviction warning for a run's recorded sweep deletions:
/// `Some` copy when the prepare-time save evicted unpinned sessions at the
/// retention cap, `None` when nothing was evicted (stay silent). The decision
/// deliberately does not consult the turn outcome — the save happened before
/// any setup fault could surface, so the evictions are real however the run
/// ends; taking no outcome parameter is what keeps that invariant structural
/// instead of a code path that can regress behind an `is_ok()` gate.
fn retention_eviction_warning(evicted: &[String]) -> Option<String> {
    (!evicted.is_empty()).then(|| {
        format!(
            "[pinvou agent run] warning: persisting this run's session evicted \
             {} unpinned chat session(s) at the {MAX_SESSIONS_PER_KIND}-session \
             retention cap (pinned sessions are exempt). Point PINVOU3_HOME at \
             a sandbox or prune the session store (PINVOU3_AGENT_TASK_KEEP_\
             SESSION=0 only removes this run's session afterwards; the \
             save-time eviction still happens).",
            evicted.len()
        )
    })
}

/// Validate the static attachment limits of an agentic request: at most
/// [`MAX_ATTACHMENTS`] entries, each resolving to a regular file of at most
/// [`MAX_ATTACHMENT_BYTES`] bytes (the staged attachment caps shared with
/// `ProductHeadlessBackend`). Symlinks to regular files are accepted,
/// matching the GUI staging path (`stage_file_in_workspace` copies content).
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
/// automation authority and never host external agentic turns; ACP sessions
/// do not live in this store, so they fail the existence check first.
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
/// setup timeout) the caller's lifecycle handling cleans up as stubs.
#[allow(clippy::too_many_arguments)]
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
    // Round-12 review: the Plan flip below persists BEFORE the turn
    // exists. If the setup then fails (attachment staging, submit), the
    // turn never started, so a caller-provided session must keep its
    // pre-run mode — the restore happens on the setup-error arm. The
    // timeout arm deliberately never restores: at the deadline edge the
    // submit may already have landed, so the pre-run mode is no longer
    // known to be the truth. Fresh sessions need no restore (the stub
    // cleanup deletes the whole record, mode sidecar included).
    let mut plan_restore: Option<SerializableMode> = None;
    let setup = async {
        if existing_session {
            // Continue the caller's chat session: never re-create it (session
            // creation would overwrite the transcript). A model pin goes
            // through the GUI chip-switch path (per-session sidecar write +
            // engine evict); the engine itself lazily spawns on submit,
            // exactly like a GUI send.
            if let Some(model_id) = request.model_id.as_deref() {
                runtime
                    .pool
                    .switch_session_model(session_id, Some(model_id.to_owned()))
                    .await
                    .context("pin session model")?;
            }
            // Mirror the fresh branch below: an explicit Plan request must
            // persist on a caller-provided session too, or the session
            // reopens in its stale mode (the unbound default is Yolo) — the
            // same unsafe divergence the fresh branch refuses. Failure is
            // fatal like the fresh branch; an existing session is never
            // cleaned up, so the caller's record stays untouched.
            if matches!(request.mode, Some(AgenticTaskMode::Plan)) {
                let previous = store.mode_state(session_id).mode;
                store
                    .set_mode_and_persist(session_id, SerializableMode::Plan)
                    .context("persist session mode")?;
                if previous != SerializableMode::Plan {
                    plan_restore = Some(previous);
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
        let content = prompt_with_attachments(store, session_id, request, existing_session).await?;
        runtime
            .submit(&TurnInput {
                session_id: session_id.to_owned(),
                content,
                mode: app_mode,
                restrict_tools: false,
                eval_tool_policy: None,
            })
            .await
            .context("submit agentic turn")
    };
    // Bound separately: a match scrutinee temporary would keep the future
    // (and its mutable borrow of `plan_restore`) alive into the arms.
    let setup_result =
        tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), setup).await;
    let handle = match setup_result {
        Ok(submitted) => match submitted {
            Ok(handle) => handle,
            Err(error) => {
                if let Some(previous) = plan_restore.take() {
                    if let Err(restore_error) = store.set_mode_and_persist(session_id, previous) {
                        eprintln!(
                            "[pinvou agent run] warning: failed to restore the \
                                 pre-run session mode after the setup failure: {restore_error:#}"
                        );
                    }
                }
                return (false, Err(error));
            }
        },
        Err(_elapsed) => {
            return (
                false,
                Ok(AgenticTaskReport {
                    session_id: session_id.to_owned(),
                    status: "timeout".to_string(),
                    timed_out: true,
                    completed_after_deadline: false,
                    assistant_text: String::new(),
                    tool_events: Vec::new(),
                    usage: None,
                    error: Some(
                        "agentic session setup did not finish within the timeout".to_string(),
                    ),
                }),
            );
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
            eprintln!(
                "[pinvou agent run] turn still active, {}s elapsed",
                started.elapsed().as_secs()
            );
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
async fn prompt_with_attachments(
    store: &SessionStore,
    session_id: &str,
    request: &AgenticTaskRequest,
    existing_session: bool,
) -> Result<String> {
    if request.attachments.is_empty() {
        return Ok(request.prompt.clone());
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
    let ingested = tokio::task::spawn_blocking(move || -> Result<Vec<IngestResult>> {
        let mut results = Vec::with_capacity(attachments.len());
        // Sources marked remove_after_ingest are deleted only after the whole
        // batch ingests successfully — deleting per-attachment would destroy
        // a caller file and then abort the run on a later failure.
        let mut consumed_sources: Vec<std::path::PathBuf> = Vec::new();
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
                staged_total = ensure_stage_size(&attachment.path, staged_total)?;
                // The re-stat above caps the validated size, but the source
                // can still grow while the copy streams — bound the staged
                // bytes too, exactly like the eval pipeline's staging.
                let relative = stage_file_in_workspace_with_copier(
                    &attachment.path.to_string_lossy(),
                    &basename,
                    &staging_root,
                    "attachments",
                    |source, destination| copy_bounded(source, destination, MAX_ATTACHMENT_BYTES),
                )
                .context(
                    "agent_attachment_stage_failed: staging into the session workspace failed",
                )?;
                let result = crate::features::files::file_ingest::ingest_attachment(
                    &staging_root.join(&relative),
                )
                .map_err(|code| anyhow::anyhow!("agent_attachment_ingest_failed: {code}"))?;
                if attachment.remove_after_ingest {
                    consumed_sources.push(attachment.path.clone());
                }
                results.push(result);
            }
            Ok(results)
        })();
        if batch.is_err() {
            return batch;
        }
        for source in &consumed_sources {
            if let Err(error) = std::fs::remove_file(source) {
                eprintln!(
                    "[pinvou agent run] remove_after_ingest could not delete {}: {error}",
                    source.display()
                );
            }
        }
        Ok(batch?)
    })
    .await
    .context("attachment staging task")??;
    Ok(build_message_with_attachments_in_dir(
        prompt,
        ingested,
        &ledger_root,
        "attachments",
        reference_absolute,
    ))
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

fn fresh_session_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "agentic_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AgenticTaskAttachment, AgenticTaskMode, AgenticTaskReport, AgenticTaskRequest,
        AgenticToolEvent, DEFAULT_TIMEOUT_SECS, MAX_ATTACHMENT_BYTES, MAX_ATTACHMENTS,
        MAX_ATTACHMENTS_TOTAL_BYTES, MAX_TIMEOUT_SECS, ensure_existing_chat_session,
        ensure_model_exists, ensure_stage_size, keep_session_from_env, retention_eviction_warning,
        validate_attachments,
    };
    use crate::features::assistant::attachments::{
        copy_bounded, stage_file_in_workspace_with_copier,
    };
    use crate::features::sessions::{
        MAX_SESSIONS_PER_KIND, ScheduledRunMode, ScheduledRunProfile, SessionStore,
    };
    use crate::platform::test_support::locked_env;
    use std::path::PathBuf;

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

    /// Sessions persist by default; only the explicit falsy values restore
    /// the legacy one-shot cleanup, and the legacy truthy values still mean
    /// keep.
    #[test]
    fn never_started_disposition_branch_order_is_pinned() {
        use super::{NeverStartedDisposition::*, never_started_disposition};
        // Zero-message stub: cleanup-eligible regardless of KEEP_SESSION.
        assert!(matches!(
            never_started_disposition(Ok(false), true),
            CleanupStub
        ));
        assert!(matches!(
            never_started_disposition(Ok(false), false),
            CleanupStub
        ));
        // Started transcript (durable record carries admitted messages):
        // stays inspectable under the default, legacy-cleanup only on the
        // explicit falsy opt-in.
        assert!(matches!(
            never_started_disposition(Ok(true), true),
            KeepInspectable
        ));
        assert!(matches!(
            never_started_disposition(Ok(true), false),
            LegacyCleanupStarted
        ));
        // Unloadable record: keep — deleting on unknown state is the unsafe
        // direction, regardless of KEEP_SESSION.
        assert!(matches!(
            never_started_disposition(Err(()), true),
            KeepInspectable
        ));
        assert!(matches!(
            never_started_disposition(Err(()), false),
            KeepInspectable
        ));
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
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let error = ensure_model_exists("definitely-missing-model").unwrap_err();
        assert!(error.to_string().contains("agent_model_not_found"));
        // `_env` restores the captured PINVOU3_HOME on return or panic.
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

    /// The store-side half of the eviction-warning contract: retention sweep
    /// deletions are recorded as real eviction events and a save below the
    /// cap records nothing. The runner's own arm/report half is pinned by
    /// `retention_eviction_warning_keys_on_the_record_regardless_of_outcome`
    /// below.
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
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot");

        let mut ids = Vec::new();
        for _ in 0..MAX_SESSIONS_PER_KIND {
            let session = store
                .create_new("test-model".to_string(), None, tmp.clone())
                .unwrap();
            ids.push(session.metadata.id);
        }

        // Arm the same receiver `run_agentic_task` installs around the turn.
        let evictions = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        store.set_retention_eviction_observer(Some(evictions.clone()));

        // The prepare-time save of a fresh run at the cap — the exact call
        // `prepare_eval_session` makes — evicts the oldest session...
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
        assert_eq!(evictions.lock().as_slice(), &[oldest]);

        // Disarm exactly like the runner does before reporting.
        let evicted = store.take_retention_eviction_observer().unwrap();
        assert_eq!(evicted.lock().as_slice(), &[ids[0].clone()]);

        // Below the cap a fresh save evicts nothing and records nothing.
        store.delete(&ids[MAX_SESSIONS_PER_KIND - 1]).unwrap();
        let evictions = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
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
            evictions.lock().is_empty(),
            "a save below the cap must not be reported as an eviction"
        );
        store.take_retention_eviction_observer();
        // `_env` restores the captured PINVOU3_HOME on return or panic; the
        // scratch home itself is removed here so repeated runs don't litter
        // the shared temp dir.
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The runner's warning decision is a pure function of the recorded
    /// evictions: a non-empty record warns (the copy carries the cap, the
    /// pin exemption and the KEEP_SESSION pointer) and an empty record stays
    /// silent. The helper takes no turn outcome, so "a run that errors after
    /// the prepare-time save still surfaces the eviction" cannot regress
    /// behind an outcome gate — there is no outcome to gate on.
    #[test]
    fn retention_eviction_warning_keys_on_the_record_regardless_of_outcome() {
        let warning = retention_eviction_warning(&["evicted-id".to_string()])
            .expect("a non-empty eviction record must warn");
        assert!(warning.contains("1 unpinned chat session"), "{warning}");
        assert!(warning.contains("pinned sessions are exempt"), "{warning}");
        assert!(
            warning.contains("PINVOU3_AGENT_TASK_KEEP_SESSION=0"),
            "{warning}"
        );
        // Nothing evicted — a below-cap save, or a run that failed before the
        // prepare-time save — must stay silent.
        assert!(retention_eviction_warning(&[]).is_none());
    }

    #[test]
    fn report_roundtrips_without_leaking_tool_payloads() {
        let report = AgenticTaskReport {
            session_id: "agentic_1_0".to_string(),
            status: "Completed".to_string(),
            timed_out: true,
            completed_after_deadline: true,
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
        // defaulting the marker to false.
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
        assert_eq!(parsed.status, "timeout");
    }

    #[test]
    fn max_timeout_secs_matches_the_cli_parse_cap() {
        // 7 days; the CLI parse cap and the library clamp must stay in lockstep
        // so `Instant + Duration` can never overflow.
        assert_eq!(MAX_TIMEOUT_SECS, 7 * 24 * 60 * 60);
    }
}
