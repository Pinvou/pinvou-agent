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
use serde::{Deserialize, Serialize};

use crate::features::assistant::attachments::{
    build_message_with_attachments_in_dir, stage_file_in_workspace,
};
use crate::features::assistant::engine_pool::EnginePool;
use crate::features::assistant::product_runtime::headless_bridge::run_windowless_host;
use crate::features::assistant::product_runtime::{
    EnginePoolRuntime, ProductChatRuntime, SessionSpec, TurnHandle, TurnInput, TurnResult,
};
use crate::features::files::file_ingest::IngestResult;
use crate::features::sessions::{ExecutionRootResolver, SessionKind, SessionStore};
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

/// Upper bound on `AgenticTaskRequest::attachments`, mirroring the staged
/// attachment limit of `ProductHeadlessBackend` in `headless_bridge.rs`.
pub const MAX_ATTACHMENTS: usize = 16;
/// Per-attachment size cap in bytes (20 MiB), mirroring
/// `ProductHeadlessBackend`'s staged attachment limit.
pub const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

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
    /// mode (the mode is carried on the send op itself, so no session-mode
    /// sidecar write is required).
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
    /// product attachment text the GUI chat send uses. Limits mirror
    /// `ProductHeadlessBackend`: at most [`MAX_ATTACHMENTS`] files, at most
    /// [`MAX_ATTACHMENT_BYTES`] each. Missing path →
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
/// lift the tool-call cap → pin the model → Yolo/Plan submit → timeout
/// watchdog → collect the report → persist the session (only an explicit
/// `PINVOU3_AGENT_TASK_KEEP_SESSION=0|false|no|off` deletes it). Once the
/// turn is submitted, a report is
/// always returned (internal failures land in the `error` field); setup faults
/// (request validation, model pin, session prepare, submit) propagate as `Err`
/// instead — the CLI surfaces those as exit 1 without a report.
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
        None => fresh_session_id(),
    };

    // Execution root binding: the closure only matches this run's session id
    // (fresh or caller-provided); resolution for every other session stays
    // unchanged. A caller-provided session is bound to `workspace` only when
    // the request carries one — an unset workspace keeps the session's own
    // resolution (its private scratch) instead of overriding it.
    let bound_workspace = request.workspace.clone();
    let matched_session = session_id.clone();
    let resolver: ExecutionRootResolver = Arc::new(move |id: &str| {
        (id == matched_session)
            .then(|| bound_workspace.clone())
            .flatten()
    });
    let mut pool = pool;
    pool.bridge.set_execution_root_resolver(resolver.clone());
    store.set_execution_root_resolver(resolver);
    // Long-horizon product work must not inherit the benchmark-hooks build's
    // per-turn tool-call cap of 8 (the GAIA runaway guard): an agentic run is
    // unlimited unless the caller pinned an explicit PINVOU3_MAX_TOOL_CALLS.
    if std::env::var_os("PINVOU3_MAX_TOOL_CALLS").is_none() {
        pool.bridge
            .set_session_tool_budget(session_id.clone(), None);
    }
    let runtime = EnginePoolRuntime::new(Arc::new(pool));

    let outcome = run_turn(
        &runtime,
        &store,
        &session_id,
        &request,
        timeout_secs,
        existing_session,
    )
    .await;

    // Session lifecycle after the turn: sessions persist by default (GUI
    // parity — the 50-session retention cap applies), so the engine is
    // reclaimed while the transcript, artifacts and timeline stay under the
    // sessions root for continuation via `--session`. Only an explicit
    // `PINVOU3_AGENT_TASK_KEEP_SESSION=0|false|no|off` restores the old
    // one-shot cleanup for harnesses that want a clean sandbox (the legacy
    // truthy values "1"/"true"/"yes"/"on" keep meaning keep).
    //
    // A caller-provided `session_id` is never auto-deleted and never swept:
    // only this run's eval observation mark is dropped, and the session is
    // left in place for the caller.
    let keep_session = keep_session_from_env();
    if existing_session {
        crate::features::assistant::timing::unregister_eval_observation(&session_id);
    } else if keep_session {
        crate::features::assistant::timing::unregister_eval_observation(&session_id);
        runtime.pool.evict(&session_id).await;
    } else {
        runtime.schedule_eval_cleanup(&session_id);
        let _ = runtime.close_eval_session_result(&session_id).await;
    }
    outcome
}

/// `PINVOU3_AGENT_TASK_KEEP_SESSION`: sessions are kept by default; only the
/// explicit falsy values restore the legacy one-shot cleanup.
fn keep_session_from_env() -> bool {
    match std::env::var("PINVOU3_AGENT_TASK_KEEP_SESSION") {
        Ok(value) => !matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// Validate the static attachment limits of an agentic request: at most
/// [`MAX_ATTACHMENTS`] entries, each resolving to a regular file of at most
/// [`MAX_ATTACHMENT_BYTES`] bytes (mirroring `ProductHeadlessBackend`'s staged
/// attachment limits). Symlinks to regular files are accepted, matching the
/// GUI staging path (`stage_file_in_workspace` copies content).
fn validate_attachments(attachments: &[AgenticTaskAttachment]) -> Result<()> {
    if attachments.len() > MAX_ATTACHMENTS {
        anyhow::bail!(
            "agent_attachment_too_many: at most {MAX_ATTACHMENTS} attachments are supported (got {})",
            attachments.len()
        );
    }
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
    }
    Ok(())
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
    if store.load(session_id).is_err() {
        anyhow::bail!("agent_session_not_found: session '{session_id}' does not exist");
    }
    match store.session_kind(session_id)? {
        SessionKind::Chat => Ok(()),
        SessionKind::ScheduledRun => anyhow::bail!(
            "agent_session_not_chat: session '{session_id}' is a scheduled-run session"
        ),
    }
}

async fn run_turn(
    runtime: &EnginePoolRuntime,
    store: &SessionStore,
    session_id: &str,
    request: &AgenticTaskRequest,
    timeout_secs: u64,
    existing_session: bool,
) -> Result<AgenticTaskReport> {
    // Model selection: a fresh session without an explicit model keeps
    // today's behavior — the active evaluation model is captured, pinned, and
    // released by the guard's Drop. A caller-provided `model_id` replaces
    // that pin (validated up front), and an existing session keeps its own
    // per-session model binding (the GUI chat send semantics).
    let suite_guard = if !existing_session && request.model_id.is_none() {
        let guard = runtime
            .capture_eval_suite_model()
            .context("active evaluation model is not configured")?;
        Some(guard)
    } else {
        None
    };
    let model_selection = if let Some(guard) = suite_guard.as_ref() {
        Some(guard.derive_case_selection()?)
    } else if !existing_session {
        // Fresh session with an explicit model: bind it through the same eval
        // selection route `prepare_eval_session` consumes.
        request
            .model_id
            .as_deref()
            .map(|model_id| runtime.pin_eval_model_selection(model_id))
            .transpose()?
    } else {
        None
    };
    let app_mode = request.mode.unwrap_or(AgenticTaskMode::Agent).to_app_mode();
    // The deadline covers the whole task, including session prepare/submit:
    // a hang in either phase must still produce a timeout report, never an
    // unbounded wait.
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
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
            crate::features::assistant::timing::register_eval_observation(session_id);
        } else {
            runtime
                .prepare(&SessionSpec {
                    session_id: session_id.to_owned(),
                    model_selection,
                })
                .await
                .context("prepare agentic session")?;
        }
        let content = prompt_with_attachments(store, session_id, request).await?;
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
    let handle =
        match tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), setup).await
        {
            Ok(submitted) => submitted?,
            Err(_elapsed) => {
                return Ok(AgenticTaskReport {
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
                });
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
            })
        }
        TurnOutcome::AbandonedAfterCancel => {
            // The turn never settled after cancel; salvage whatever the
            // transcript already holds so the report keeps partial
            // observability instead of dropping every tool event.
            let (assistant_text, tool_events) = partial_turn_analysis(runtime, &handle);
            Ok(AgenticTaskReport {
                session_id: session_id.to_owned(),
                status: "timeout".to_string(),
                timed_out: true,
                completed_after_deadline: false,
                assistant_text,
                tool_events,
                usage: None,
                error: Some("agent turn did not settle after cancel".to_string()),
            })
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
            })
        }
    }
}

/// Enrich the prompt with the request's attachments using the GUI attachment
/// pipeline: each file is staged into the session ledger root's
/// `attachments/` directory (the same secure staging the GUI dialog and the
/// eval bridge use), ingested through `features/files::file_ingest` (the same
/// chip ingest), and rendered by the same product message builder the GUI
/// chat command uses for the non-native-image path. `reference_absolute`
/// follows the GUI chat command: when the ledger root and the engine
/// execution root diverge (workspace-bound run), staged files are referenced
/// by absolute path. Images get the `image_analyze` hard-rule text, which the
/// product tool allowlist always provides, so no model image-capability probe
/// is needed.
async fn prompt_with_attachments(
    store: &SessionStore,
    session_id: &str,
    request: &AgenticTaskRequest,
) -> Result<String> {
    if request.attachments.is_empty() {
        return Ok(request.prompt.clone());
    }
    let roots = store
        .session_roots(session_id)
        .context("resolve attachment roots")?;
    let ledger_root = roots.ledger.clone();
    let reference_absolute = roots.ledger != roots.execution;
    let attachments = request.attachments.clone();
    let prompt = request.prompt.clone();
    let staging_root = ledger_root.clone();
    let ingested = tokio::task::spawn_blocking(move || -> Result<Vec<IngestResult>> {
        let mut results = Vec::with_capacity(attachments.len());
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
            let relative = stage_file_in_workspace(
                &attachment.path.to_string_lossy(),
                &basename,
                &staging_root,
                "attachments",
            )
            .context("agent_attachment_stage_failed: staging into the session workspace failed")?;
            let result = crate::features::files::file_ingest::ingest_attachment(
                &staging_root.join(&relative),
            )
            .map_err(|code| anyhow::anyhow!("agent_attachment_ingest_failed: {code}"))?;
            if attachment.remove_after_ingest {
                if let Err(error) = std::fs::remove_file(&attachment.path) {
                    eprintln!(
                        "[pinvou agent run] remove_after_ingest could not delete {}: {error}",
                        attachment.path.display()
                    );
                }
            }
            results.push(result);
        }
        Ok(results)
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
            let turn_tool_ids: std::collections::HashSet<_> = timeline
                .iter()
                .filter(|event| {
                    event.turn_id == handle.turn_id
                        && !matches!(event.event.as_str(), "user_start" | "assistant_done")
                })
                .filter_map(|event| event.tool_id.clone())
                .collect();
            events
                .into_iter()
                .filter(|tool| turn_tool_ids.contains(&tool.id))
                .collect()
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
        MAX_TIMEOUT_SECS, ensure_existing_chat_session, ensure_model_exists, keep_session_from_env,
        validate_attachments,
    };
    use crate::features::sessions::{ScheduledRunMode, ScheduledRunProfile, SessionStore};
    use crate::platform::paths::tests::ENV_LOCK;
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
    fn keep_session_env_defaults_to_keeping() {
        let _lock = ENV_LOCK.lock().unwrap();
        // SAFETY: ENV_LOCK held; env writes are serialized across tests.
        unsafe { std::env::remove_var("PINVOU3_AGENT_TASK_KEEP_SESSION") };
        assert!(keep_session_from_env(), "absent env must keep the session");
        for value in ["1", "true", "yes", "on", "anything-else"] {
            // SAFETY: see above.
            unsafe { std::env::set_var("PINVOU3_AGENT_TASK_KEEP_SESSION", value) };
            assert!(keep_session_from_env(), "{value} must keep the session");
        }
        for value in ["0", "false", "no", "off", "FALSE", "Off"] {
            // SAFETY: see above.
            unsafe { std::env::set_var("PINVOU3_AGENT_TASK_KEEP_SESSION", value) };
            assert!(!keep_session_from_env(), "{value} must delete the session");
        }
        // SAFETY: see above.
        unsafe { std::env::remove_var("PINVOU3_AGENT_TASK_KEEP_SESSION") };
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
    fn ensure_model_exists_rejects_unknown_model() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-agentic-model-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let error = ensure_model_exists("definitely-missing-model").unwrap_err();
        assert!(error.to_string().contains("agent_model_not_found"));
    }

    #[test]
    fn ensure_existing_chat_session_accepts_chat_rejects_unknown_and_scheduled() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-agentic-session-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
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
        assert!(!json.contains("secret"));
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
