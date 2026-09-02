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

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use deepseek_tui::tui::app::AppMode;
use serde::{Deserialize, Serialize};

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::assistant::product_runtime::headless_bridge::run_windowless_host;
use crate::features::assistant::product_runtime::{
    EnginePoolRuntime, ProductChatRuntime, SessionSpec, TurnHandle, TurnInput, TurnResult,
};
use crate::features::sessions::{ExecutionRootResolver, SessionStore};

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

/// One agentic task's input. `prompt` enters the product send path verbatim
/// (no eval envelope).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgenticTaskRequest {
    pub prompt: String,
    /// Task working directory; None = session-private directory (the same
    /// isolated scratch as eval sessions).
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
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

/// Drive one agentic turn: bind the execution root → pin the active model →
/// Yolo submit → timeout watchdog → collect the report → clean up the
/// temporary session. A report is always returned (internal failures land in
/// the `error` field); only host-level faults propagate as `Err`.
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
    let session_id = fresh_session_id();

    // Execution root binding: the closure only matches this run's session id;
    // resolution for every other session stays unchanged.
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
    let runtime = EnginePoolRuntime::new(Arc::new(pool));

    let outcome = run_turn(&runtime, &session_id, request, timeout_secs).await;

    // Reclaim regardless of outcome: engine resources go through the eval
    // cleanup channel, the persisted session is deleted, and the model suite
    // pin is returned by the guard's Drop. With PINVOU3_AGENT_TASK_KEEP_SESSION
    // the session artifacts stay under the sessions root for harness-side
    // debugging (this host is one-shot, so nothing else holds the session).
    let keep_session = std::env::var("PINVOU3_AGENT_TASK_KEEP_SESSION")
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false);
    if !keep_session {
        runtime.schedule_eval_cleanup(&session_id);
        let _ = runtime.close_eval_session_result(&session_id).await;
    }
    outcome
}

async fn run_turn(
    runtime: &EnginePoolRuntime,
    session_id: &str,
    request: AgenticTaskRequest,
    timeout_secs: u64,
) -> Result<AgenticTaskReport> {
    let guard = runtime
        .capture_eval_suite_model()
        .context("active evaluation model is not configured")?;
    let selection = guard.derive_case_selection()?;
    // The deadline covers the whole task, including session prepare/submit:
    // a hang in either phase must still produce a timeout report, never an
    // unbounded wait.
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let prompt = request.prompt;
    let setup = async {
        runtime
            .prepare(&SessionSpec {
                session_id: session_id.to_owned(),
                model_selection: Some(selection),
            })
            .await
            .context("prepare agentic session")?;
        runtime
            .submit(&TurnInput {
                session_id: session_id.to_owned(),
                content: prompt,
                mode: AppMode::Yolo,
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
    drop(guard);

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
        WaitFailed(anyhow::Error),
    }
    let turn_outcome = if timed_out && runtime.is_turn_active(session_id) {
        TurnOutcome::AbandonedAfterCancel
    } else {
        // Read failures (read_timeline/load_eval_transcript etc.) have nothing
        // to do with the timeout: disguising them as one would swallow the
        // real root cause, so they get an explicit error report.
        match runtime.wait_for_completion(&handle).await {
            Ok(turn) => TurnOutcome::Done(turn),
            Err(error) => TurnOutcome::WaitFailed(error),
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
        TurnOutcome::WaitFailed(error) => Ok(AgenticTaskReport {
            session_id: session_id.to_owned(),
            status: "error".to_string(),
            timed_out: false,
            completed_after_deadline: false,
            assistant_text: String::new(),
            tool_events: Vec::new(),
            usage: None,
            error: Some(format!("failed to read turn result: {error:#}")),
        }),
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
        AgenticTaskReport, AgenticTaskRequest, AgenticToolEvent, DEFAULT_TIMEOUT_SECS,
        MAX_TIMEOUT_SECS,
    };

    #[test]
    fn request_defaults_timeout_and_workspace() {
        let request: AgenticTaskRequest = serde_json::from_str(r#"{"prompt":"do it"}"#).unwrap();
        assert_eq!(request.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(request.workspace.is_none());

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
