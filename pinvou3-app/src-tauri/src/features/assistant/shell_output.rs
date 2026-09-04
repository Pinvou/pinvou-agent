//! Host-side shell output observation for the fixed CodeWhale runtime.
//!
//! CodeWhale exposes one [`SharedShellManager`] per engine session.  The
//! manager already executes bounded foreground commands through its tracked
//! job table, so the host can observe both foreground and detached processes
//! without adding an engine event or replacing the built-in shell tool.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};
use std::time::Duration;

use deepseek_tui::tools::shell::{
    SharedShellManager, ShellJobDetail, ShellJobSnapshot, ShellStatus,
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter};

// The poll interval partially mitigates the engine's missing cursor API:
// every tick `inspect_job` clones and decodes a job's entire output (a real
// fix needs a cursor read in CodeWhale, which cannot change here), so a
// longer period amortizes that constant cost. Paired with the byte-length
// short-circuit in `observe_jobs` and the bounded emission mirror below.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Tail of already-emitted output kept per tracked stream. The tail acts as
/// the reconciliation cursor: delta computation runs against the last
/// `EMITTED_TAIL_KEEP_BYTES` bytes of emitted text plus the byte offset
/// where that tail starts inside the job's full output buffer, so memory
/// and per-tick compare costs stay bounded while live deltas keep flowing
/// for arbitrarily verbose jobs. A cursor read API upstream remains the
/// real fix for the per-tick full clone+decode.
const EMITTED_TAIL_KEEP_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub(crate) struct ShellOutputMonitor {
    state: Arc<Mutex<MonitorState>>,
}

#[derive(Debug)]
struct TrackedTool {
    command: String,
    order: u64,
    task_id: Option<String>,
    stdout: StreamMirror,
    stderr: StreamMirror,
    keep_after_tool_end: bool,
}

/// Bounded tail of one stream's already-emitted text plus the byte offset
/// where that tail starts inside the job's full output buffer.
#[derive(Debug, Default)]
struct StreamMirror {
    /// Bytes of the full stream output before `text` begins.
    skip: usize,
    /// Already-emitted tail, at most `EMITTED_TAIL_KEEP_BYTES` bytes.
    text: String,
}

impl StreamMirror {
    /// Return the not-yet-emitted suffix of `current`, or `None` when
    /// `current` does not extend the already-emitted text (hold the mirror
    /// and retry on the next snapshot). While `running`, a trailing U+FFFD
    /// is held back so a code point split across reader chunks is emitted
    /// exactly once (`inspect_job` lossily decodes the whole buffer every
    /// observation, so an incomplete trailing code point shows up as a
    /// temporary replacement character).
    fn delta(&mut self, current: &str, running: bool) -> Option<String> {
        let stable = if running {
            current.trim_end_matches('\u{fffd}')
        } else {
            current
        };
        let emitted_tail = stable.get(self.skip..)?;
        let delta = emitted_tail.strip_prefix(self.text.as_str())?;
        if emitted_tail.len() <= EMITTED_TAIL_KEEP_BYTES {
            self.text.push_str(delta);
        } else {
            // `text + delta == emitted_tail`, so the bounded mirror is the
            // last keep-window bytes of `emitted_tail`. Rebuild from that
            // suffix instead of push+drain: `drain` shortens the string
            // without releasing capacity, so one large observation would
            // keep a whole-delta allocation resident for the job's
            // lifetime. `skip` advances with the dropped head to keep
            // `text` aligned with `stable[self.skip..]`, cutting only at
            // char boundaries.
            let mut trim = emitted_tail.len() - EMITTED_TAIL_KEEP_BYTES;
            while !emitted_tail.is_char_boundary(trim) {
                trim += 1;
            }
            self.skip += trim;
            self.text = String::from(&emitted_tail[trim..]);
        }
        Some(delta.to_string())
    }
}

#[derive(Debug, Default)]
struct MonitorState {
    tools: HashMap<String, TrackedTool>,
    claimed_tasks: HashSet<String>,
    next_order: u64,
    /// task_id -> cumulative output bytes (snapshot stdout_len+stderr_len)
    /// observed last round. While a job is running and the byte count is
    /// unchanged, the decoded output must equal the previous round and the
    /// delta must be empty, so the full clone+decode in `inspect_job` is
    /// skipped entirely.
    last_output_bytes: HashMap<String, usize>,
}

#[derive(Debug)]
struct ObservedJob {
    task_id: String,
    status: ShellStatus,
    exit_code: Option<i64>,
    stdout: String,
    stderr: String,
    stdout_tail: String,
    stderr_tail: String,
}

#[derive(Debug, PartialEq, Eq)]
enum MonitorEmission {
    Delta {
        tool_id: String,
        stream: &'static str,
        content: String,
    },
    BackgroundFinished {
        tool_id: String,
        task_id: String,
        status: ShellStatus,
        exit_code: Option<i64>,
        stdout_tail: String,
        stderr_tail: String,
    },
}

pub(super) fn is_shell_execution_tool(name: &str) -> bool {
    matches!(name, "bash" | "exec_shell" | "task_shell_start" | "Bash")
}

impl ShellOutputMonitor {
    pub(crate) fn spawn(
        app: AppHandle,
        shell_manager: &SharedShellManager,
        session_id: String,
    ) -> Self {
        // EnginePool intentionally reuses a session's manager across engine
        // rebuilds.  Existing jobs belong to an earlier tool card and must not
        // be rebound merely because a later command has the same text.
        let claimed_tasks = shell_manager
            .lock()
            .map(|mut manager| manager.list_jobs().into_iter().map(|job| job.id).collect())
            .unwrap_or_default();
        let state = Arc::new(Mutex::new(MonitorState {
            claimed_tasks,
            ..MonitorState::default()
        }));
        spawn_monitor_loop(
            app,
            Arc::downgrade(shell_manager),
            session_id,
            Arc::downgrade(&state),
        );
        Self { state }
    }

    pub(crate) fn tool_started(&self, tool_id: &str, name: &str, input: &Value) {
        // v0.9.12 exposes lowercase `bash`; retain the older names so restored
        // transcripts and in-flight sessions can still reconnect to their jobs.
        if !is_shell_execution_tool(name) {
            return;
        }
        let Some(command) = input.get("command").and_then(Value::as_str) else {
            return;
        };
        let mut state = self.state.lock();
        let order = state.next_order;
        state.next_order = state.next_order.saturating_add(1);
        state.tools.insert(
            tool_id.to_string(),
            TrackedTool {
                command: command.to_string(),
                order,
                task_id: None,
                stdout: StreamMirror::default(),
                stderr: StreamMirror::default(),
                keep_after_tool_end: false,
            },
        );
    }

    /// Mark the Engine tool call complete. Detached work remains observed
    /// after `chat:tool_end`; foreground work stops because that event already
    /// carries the authoritative complete output.
    pub(crate) fn tool_completed(&self, tool_id: &str, background_task_id: Option<&str>) {
        let mut state = self.state.lock();
        let Some(task_id) = background_task_id else {
            if let Some(task_id) = state.tools.remove(tool_id).and_then(|tool| tool.task_id) {
                state.last_output_bytes.remove(&task_id);
            }
            return;
        };
        state.claimed_tasks.insert(task_id.to_string());
        if let Some(tool) = state.tools.get_mut(tool_id) {
            tool.task_id = Some(task_id.to_string());
            tool.keep_after_tool_end = true;
        }
    }
}

fn spawn_monitor_loop(
    app: AppHandle,
    shell_manager: Weak<std::sync::Mutex<deepseek_tui::tools::shell::ShellManager>>,
    session_id: String,
    state: Weak<Mutex<MonitorState>>,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            let Some(state) = state.upgrade() else {
                break;
            };
            if state.lock().tools.is_empty() {
                continue;
            }
            let Some(shell_manager) = shell_manager.upgrade() else {
                emit_owner_reclaimed(&app, &session_id, &mut state.lock());
                break;
            };

            let observed = match observe_jobs(&shell_manager, &mut state.lock()) {
                Ok(observed) => observed,
                Err(error) => {
                    log::warn!("[shell_output] observation stopped: {error}");
                    emit_owner_reclaimed(&app, &session_id, &mut state.lock());
                    break;
                }
            };
            let emissions = state.lock().reconcile(observed);
            for emission in emissions {
                emit_monitor_event(&app, &session_id, emission);
            }
        }
    });
}

fn observe_jobs(
    shell_manager: &SharedShellManager,
    state: &mut MonitorState,
) -> Result<Vec<ObservedJob>, String> {
    let mut manager = shell_manager
        .lock()
        .map_err(|_| "shell manager lock poisoned".to_string())?;
    let snapshots = manager.list_jobs();
    state.assign_unclaimed_tasks(&snapshots);
    let tracked = state
        .tools
        .values()
        .filter_map(|tool| tool.task_id.clone())
        .collect::<Vec<_>>();
    Ok(tracked
        .into_iter()
        .filter_map(|task_id| {
            // `inspect_job` clones and decodes a job's entire output on every
            // call (the engine has no cursor API). `list_jobs` snapshots
            // carry cumulative byte counts and were already polled within
            // this call: while a job is running and its total byte count is
            // unchanged, the decoded output must equal the previous round
            // and the delta must be empty, so skip the full read. Terminal
            // snapshots are never short-circuited so the closing event is
            // always emitted.
            if let Some(snapshot) = snapshots.iter().find(|job| job.id == task_id) {
                let total = snapshot.stdout_len.saturating_add(snapshot.stderr_len);
                if snapshot.status == ShellStatus::Running
                    && state.last_output_bytes.get(&task_id) == Some(&total)
                {
                    return None;
                }
            }
            let detail = manager.inspect_job(&task_id).ok()?;
            let total = detail
                .snapshot
                .stdout_len
                .saturating_add(detail.snapshot.stderr_len);
            state.last_output_bytes.insert(task_id, total);
            Some(ObservedJob::from(detail))
        })
        .collect())
}

impl MonitorState {
    fn assign_unclaimed_tasks(&mut self, snapshots: &[ShellJobSnapshot]) {
        let mut pending = self
            .tools
            .iter()
            .filter(|(_, tool)| tool.task_id.is_none())
            .map(|(tool_id, tool)| (tool.order, tool_id.clone(), tool.command.clone()))
            .collect::<Vec<_>>();
        pending.sort_by_key(|(order, _, _)| *order);

        for (_, tool_id, command) in pending {
            let candidate = snapshots.iter().find(|job| {
                job.owner_agent_id.is_none()
                    && job.origin_tool_call_id.as_deref() == Some(tool_id.as_str())
                    && !self.claimed_tasks.contains(&job.id)
            });
            // Compatibility for runtimes predating stable task origins. Never
            // bind a job that carries a different explicit origin merely
            // because its rendered command text happens to match.
            let candidate = candidate.or_else(|| {
                snapshots.iter().find(|job| {
                    job.owner_agent_id.is_none()
                        && job.origin_tool_call_id.is_none()
                        && job.command == command
                        && !self.claimed_tasks.contains(&job.id)
                })
            });
            let Some(candidate) = candidate else {
                continue;
            };
            self.claimed_tasks.insert(candidate.id.clone());
            if let Some(tool) = self.tools.get_mut(&tool_id) {
                tool.task_id = Some(candidate.id.clone());
            }
        }
    }

    fn reconcile(&mut self, observed: Vec<ObservedJob>) -> Vec<MonitorEmission> {
        let mut emissions = Vec::new();
        let by_task = observed
            .into_iter()
            .map(|job| (job.task_id.clone(), job))
            .collect::<HashMap<_, _>>();
        let tool_ids = self.tools.keys().cloned().collect::<Vec<_>>();
        let mut finished = Vec::new();

        for tool_id in tool_ids {
            let Some(tool) = self.tools.get_mut(&tool_id) else {
                continue;
            };
            let Some(task_id) = tool.task_id.as_deref() else {
                continue;
            };
            let Some(job) = by_task.get(task_id) else {
                continue;
            };

            if let Some(delta) = tool
                .stdout
                .delta(&job.stdout, job.status == ShellStatus::Running)
            {
                if !delta.is_empty() {
                    emissions.push(MonitorEmission::Delta {
                        tool_id: tool_id.clone(),
                        stream: "stdout",
                        content: delta,
                    });
                }
            }
            if let Some(delta) = tool
                .stderr
                .delta(&job.stderr, job.status == ShellStatus::Running)
            {
                if !delta.is_empty() {
                    emissions.push(MonitorEmission::Delta {
                        tool_id: tool_id.clone(),
                        stream: "stderr",
                        content: delta,
                    });
                }
            }

            if job.status != ShellStatus::Running && tool.keep_after_tool_end {
                emissions.push(MonitorEmission::BackgroundFinished {
                    tool_id: tool_id.clone(),
                    task_id: task_id.to_string(),
                    status: job.status.clone(),
                    exit_code: job.exit_code,
                    stdout_tail: job.stdout_tail.clone(),
                    stderr_tail: job.stderr_tail.clone(),
                });
                finished.push(tool_id);
            }
        }
        for tool_id in finished {
            if let Some(task_id) = self.tools.remove(&tool_id).and_then(|tool| tool.task_id) {
                self.last_output_bytes.remove(&task_id);
            }
        }
        emissions
    }
}

impl From<ShellJobDetail> for ObservedJob {
    fn from(detail: ShellJobDetail) -> Self {
        Self {
            task_id: detail.snapshot.id,
            status: detail.snapshot.status,
            // Windows process exit codes are unsigned 32-bit values surfaced by
            // CodeWhale as i64 (for example 0xC0000005 = 3221225477).  Narrowing
            // to i32 silently dropped those NTSTATUS values from UI events.
            exit_code: detail.snapshot.exit_code,
            stdout: detail.stdout,
            stderr: detail.stderr,
            stdout_tail: detail.snapshot.stdout_tail,
            stderr_tail: detail.snapshot.stderr_tail,
        }
    }
}

fn emit_monitor_event(app: &AppHandle, session_id: &str, emission: MonitorEmission) {
    match emission {
        MonitorEmission::Delta {
            tool_id,
            stream,
            content,
        } => {
            let payload = json!({
                "session_id": session_id,
                "id": tool_id,
                "stream": stream,
                "content": content,
            });
            let _ = app.emit("chat:tool_delta", payload.clone());
            crate::platform::app_events::forward_app_event(app, "chat:tool_delta", payload);
        }
        MonitorEmission::BackgroundFinished {
            tool_id,
            task_id,
            status,
            exit_code,
            stdout_tail,
            stderr_tail,
        } => emit_shell_task_status(
            app,
            session_id,
            &tool_id,
            &task_id,
            status,
            exit_code,
            &stdout_tail,
            &stderr_tail,
        ),
    }
}

fn emit_owner_reclaimed(app: &AppHandle, session_id: &str, state: &mut MonitorState) {
    let background = state
        .tools
        .iter()
        .filter(|(_, tool)| tool.keep_after_tool_end)
        .filter_map(|(tool_id, tool)| {
            tool.task_id
                .as_ref()
                .map(|task_id| (tool_id.clone(), task_id.clone()))
        })
        .collect::<Vec<_>>();
    state.tools.clear();
    state.last_output_bytes.clear();
    for (tool_id, task_id) in background {
        emit_shell_task_status(
            app,
            session_id,
            &tool_id,
            &task_id,
            ShellStatus::Killed,
            None,
            "",
            "Shell task owner was reclaimed",
        );
    }
}

fn emit_shell_task_status(
    app: &AppHandle,
    session_id: &str,
    tool_id: &str,
    task_id: &str,
    status: ShellStatus,
    exit_code: Option<i64>,
    stdout_tail: &str,
    stderr_tail: &str,
) {
    let payload = json!({
        "session_id": session_id,
        "tool_id": tool_id,
        "task_id": task_id,
        "status": format!("{status:?}"),
        "exit_code": exit_code,
        "stdout_tail": stdout_tail,
        "stderr_tail": stderr_tail,
    });
    let _ = app.emit("chat:shell_task_status", payload.clone());
    crate::platform::app_events::forward_app_event(app, "chat:shell_task_status", payload);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn snapshot(id: &str, command: &str, status: ShellStatus) -> ShellJobSnapshot {
        ShellJobSnapshot {
            id: id.to_string(),
            job_id: id.to_string(),
            command: command.to_string(),
            cwd: PathBuf::from("workspace"),
            status,
            exit_code: None,
            elapsed_ms: 1,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            stdout_len: 0,
            stderr_len: 0,
            stdin_available: false,
            stale: false,
            elapsed_since_output_ms: None,
            linked_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            origin_tool_call_id: None,
            origin_turn_id: None,
            owner_session_id: String::new(),
        }
    }

    fn observed(id: &str, status: ShellStatus, stdout: &str) -> ObservedJob {
        ObservedJob {
            task_id: id.to_string(),
            status,
            exit_code: Some(0),
            stdout: stdout.to_string(),
            stderr: String::new(),
            stdout_tail: stdout.to_string(),
            stderr_tail: String::new(),
        }
    }

    fn track(state: &mut MonitorState, tool_id: &str, command: &str) {
        let order = state.next_order;
        state.next_order = state.next_order.saturating_add(1);
        state.tools.insert(
            tool_id.to_string(),
            TrackedTool {
                command: command.to_string(),
                order,
                task_id: None,
                emitted_stdout: String::new(),
                emitted_stderr: String::new(),
                keep_after_tool_end: false,
            },
        );
    }

    #[test]
    fn tracks_v0912_bash_and_keeps_replay_aliases() {
        for name in ["bash", "exec_shell", "task_shell_start", "Bash"] {
            assert!(
                is_shell_execution_tool(name),
                "shell tool not tracked: {name}"
            );
        }
        assert!(!is_shell_execution_tool("read"));
    }

    #[test]
    fn assigns_new_job_by_command_and_coalesces_all_unseen_output() {
        let mut state = MonitorState::default();
        state.tools.insert(
            "tool-1".to_string(),
            TrackedTool {
                command: "cargo check".to_string(),
                order: 0,
                task_id: None,
                stdout: StreamMirror::default(),
                stderr: StreamMirror::default(),
                keep_after_tool_end: false,
            },
        );
        state.assign_unclaimed_tasks(&[snapshot("job-1", "cargo check", ShellStatus::Running)]);
        assert_eq!(state.tools["tool-1"].task_id.as_deref(), Some("job-1"));

        assert_eq!(
            state.reconcile(vec![observed("job-1", ShellStatus::Running, "one\ntwo\n")]),
            vec![MonitorEmission::Delta {
                tool_id: "tool-1".to_string(),
                stream: "stdout",
                content: "one\ntwo\n".to_string(),
            }]
        );
        assert_eq!(
            state.reconcile(vec![observed(
                "job-1",
                ShellStatus::Running,
                "one\ntwo\nthree\n"
            )]),
            vec![MonitorEmission::Delta {
                tool_id: "tool-1".to_string(),
                stream: "stdout",
                content: "three\n".to_string(),
            }]
        );
    }

    #[test]
    fn forkguard_shell_monitor_assigns_identical_commands_by_stable_origin() {
        let mut state = MonitorState::default();
        for tool_id in ["tool-1", "tool-2"] {
            track(&mut state, tool_id, "same command");
        }
        let mut second = snapshot("job-2", "same command", ShellStatus::Running);
        second.origin_tool_call_id = Some("tool-2".to_string());
        let mut first = snapshot("job-1", "same command", ShellStatus::Running);
        first.origin_tool_call_id = Some("tool-1".to_string());

        state.assign_unclaimed_tasks(&[second, first]);

        assert_eq!(state.tools["tool-1"].task_id.as_deref(), Some("job-1"));
        assert_eq!(state.tools["tool-2"].task_id.as_deref(), Some("job-2"));
    }

    #[test]
    fn explicit_origin_never_falls_back_to_a_matching_command() {
        let mut state = MonitorState::default();
        track(&mut state, "tool-current", "same command");
        let mut old = snapshot("job-old", "same command", ShellStatus::Running);
        old.origin_tool_call_id = Some("tool-old".to_string());

        state.assign_unclaimed_tasks(&[old]);

        assert_eq!(state.tools["tool-current"].task_id, None);
    }

    #[test]
    fn holds_incomplete_utf8_replacement_until_a_stable_snapshot() {
        let mut mirror = StreamMirror::default();
        assert_eq!(mirror.delta("中\u{fffd}", true).as_deref(), Some("中"));
        assert_eq!(mirror.delta("中文", true).as_deref(), Some("文"));
    }

    #[test]
    fn preserves_windows_ntstatus_exit_code() {
        let mut job = snapshot("job-windows", "crashing.exe", ShellStatus::Failed);
        job.exit_code = Some(0xC000_0005_u32 as i64);
        let observed = ObservedJob::from(ShellJobDetail {
            snapshot: job,
            stdout: String::new(),
            stderr: String::new(),
        });
        assert_eq!(observed.exit_code, Some(3_221_225_477));
    }

    #[test]
    fn preserves_existing_jobs_and_subagent_jobs_from_rebinding() {
        let mut state = MonitorState {
            claimed_tasks: HashSet::from(["old".to_string()]),
            ..MonitorState::default()
        };
        state.tools.insert(
            "tool-1".to_string(),
            TrackedTool {
                command: "same".to_string(),
                order: 0,
                task_id: None,
                stdout: StreamMirror::default(),
                stderr: StreamMirror::default(),
                keep_after_tool_end: false,
            },
        );
        let mut child = snapshot("child", "same", ShellStatus::Running);
        child.owner_agent_id = Some("agent-1".to_string());
        state.assign_unclaimed_tasks(&[
            snapshot("old", "same", ShellStatus::Completed),
            child,
            snapshot("new", "same", ShellStatus::Running),
        ]);
        assert_eq!(state.tools["tool-1"].task_id.as_deref(), Some("new"));
    }

    #[test]
    fn bounded_mirror_keeps_streaming_after_the_window_overflows() {
        let mut state = MonitorState::default();
        state.tools.insert(
            "tool-1".to_string(),
            TrackedTool {
                command: "build".to_string(),
                order: 0,
                task_id: Some("job-1".to_string()),
                stdout: StreamMirror::default(),
                stderr: StreamMirror::default(),
                keep_after_tool_end: true,
            },
        );
        // The first observation coalesces the whole backlog into one delta;
        // the mirror then trims to the keep window.
        let backlog = "x".repeat(EMITTED_TAIL_KEEP_BYTES + 4096);
        assert_eq!(
            state.reconcile(vec![observed("job-1", ShellStatus::Running, &backlog)]),
            vec![MonitorEmission::Delta {
                tool_id: "tool-1".to_string(),
                stream: "stdout",
                content: backlog.clone(),
            }]
        );
        assert_eq!(state.tools["tool-1"].stdout.skip, 4096);
        assert_eq!(
            state.tools["tool-1"].stdout.text.len(),
            EMITTED_TAIL_KEEP_BYTES
        );

        // Progress appended after the window overflowed still reaches the
        // display while the job keeps running.
        let progressed = backlog + "still alive\n";
        assert_eq!(
            state.reconcile(vec![observed("job-1", ShellStatus::Running, &progressed)]),
            vec![MonitorEmission::Delta {
                tool_id: "tool-1".to_string(),
                stream: "stdout",
                content: "still alive\n".to_string(),
            }]
        );
        assert_eq!(
            state.tools["tool-1"].stdout.text.len(),
            EMITTED_TAIL_KEEP_BYTES
        );

        // The terminal snapshot still fires the closing event with the
        // authoritative tails.
        let completed = progressed + "done\n";
        assert_eq!(
            state.reconcile(vec![observed("job-1", ShellStatus::Completed, &completed)]),
            vec![
                MonitorEmission::Delta {
                    tool_id: "tool-1".to_string(),
                    stream: "stdout",
                    content: "done\n".to_string(),
                },
                MonitorEmission::BackgroundFinished {
                    tool_id: "tool-1".to_string(),
                    task_id: "job-1".to_string(),
                    status: ShellStatus::Completed,
                    exit_code: Some(0),
                    stdout_tail: completed.clone(),
                    stderr_tail: String::new(),
                }
            ]
        );
        assert!(!state.tools.contains_key("tool-1"));
    }

    #[test]
    fn large_burst_trims_without_retaining_the_whole_allocation() {
        let mut mirror = StreamMirror::default();
        // A first observation after substantial output accumulation: the
        // burst crosses the keep window with a multibyte code point
        // straddling the trim cut, so the trim must advance to the next
        // char boundary.
        let burst = format!(
            "{}中{}",
            "x".repeat(EMITTED_TAIL_KEEP_BYTES),
            "y".repeat(EMITTED_TAIL_KEEP_BYTES - 2)
        );
        assert_eq!(mirror.delta(&burst, true).as_deref(), Some(burst.as_str()));
        // The mirror keeps only the bounded suffix and, unlike push+drain,
        // does not retain the whole-burst capacity afterwards.
        assert_eq!(mirror.text, "y".repeat(EMITTED_TAIL_KEEP_BYTES - 2));
        assert_eq!(mirror.skip, EMITTED_TAIL_KEEP_BYTES + 3);
        assert!(mirror.text.capacity() <= EMITTED_TAIL_KEEP_BYTES);

        // Progress after the burst still reaches the display while the
        // job keeps running, and retained capacity stays bounded.
        let progressed = format!("{burst}still alive\n");
        assert_eq!(
            mirror.delta(&progressed, true).as_deref(),
            Some("still alive\n")
        );
        assert!(mirror.text.capacity() <= EMITTED_TAIL_KEEP_BYTES);

        // Completion emits the remainder, still without growing the
        // retained allocation.
        let completed = format!("{progressed}done\n");
        assert_eq!(mirror.delta(&completed, false).as_deref(), Some("done\n"));
        assert!(mirror.text.capacity() <= EMITTED_TAIL_KEEP_BYTES);
    }

    #[test]
    fn reports_detached_completion_after_the_engine_tool_has_returned() {
        let mut state = MonitorState::default();
        state.tools.insert(
            "tool-1".to_string(),
            TrackedTool {
                command: "build".to_string(),
                order: 0,
                task_id: Some("job-1".to_string()),
                stdout: StreamMirror {
                    skip: 0,
                    text: "building\n".to_string(),
                },
                stderr: StreamMirror::default(),
                keep_after_tool_end: true,
            },
        );
        assert_eq!(
            state.reconcile(vec![observed(
                "job-1",
                ShellStatus::Completed,
                "building\ndone\n"
            )]),
            vec![
                MonitorEmission::Delta {
                    tool_id: "tool-1".to_string(),
                    stream: "stdout",
                    content: "done\n".to_string(),
                },
                MonitorEmission::BackgroundFinished {
                    tool_id: "tool-1".to_string(),
                    task_id: "job-1".to_string(),
                    status: ShellStatus::Completed,
                    exit_code: Some(0),
                    stdout_tail: "building\ndone\n".to_string(),
                    stderr_tail: String::new(),
                }
            ]
        );
        assert!(!state.tools.contains_key("tool-1"));
    }
}
