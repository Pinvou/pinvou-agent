//! `agent run` family: prompt-file driven agentic task execution.
//!
//! Moved verbatim from the former monolithic `lib.rs`; the parse defaults and
//! the exit contract must not change.

use std::path::{Path, PathBuf};

use crate::{CliError, CliOutcome, ExitCode, OutputMode};

/// Parse-time cap for `agent run --timeout-secs` (7 days): an unbounded u64
/// would overflow `Instant + Duration`, exiting 101 with no report. Must stay
/// in lockstep with the library clamp `pinvou_product_backend::MAX_TIMEOUT_SECS`
/// (asserted equal by `agent_timeout_cap_matches_library_clamp`).
const AGENT_TIMEOUT_SECS_MAX: u64 = 7 * 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentCommand {
    Run {
        prompt_file: PathBuf,
        workspace: Option<PathBuf>,
        timeout_secs: u64,
        /// `--session <id>`: continue an existing chat session
        /// (`AgenticTaskRequest.session_id`). None = historical behavior
        /// (fresh temporary session).
        session: Option<String>,
        /// `--mode plan|agent` (`AgenticTaskRequest.mode`). None = historical
        /// behavior (agent turn).
        mode: Option<String>,
        /// `--model <model-id>` (`AgenticTaskRequest.model_id`).
        model: Option<String>,
        /// `--attach <PATH>`, repeatable (`AgenticTaskRequest.attachments`
        /// with `remove_after_ingest: false`).
        attachments: Vec<PathBuf>,
    },
}

/// Flags that carry a value. `--attach` is intentionally repeatable; every
/// other flag rejects duplicates.
const RUN_OPTIONS: &[&str] = &[
    "--prompt-file",
    "--workspace",
    "--timeout-secs",
    "--session",
    "--mode",
    "--model",
    "--attach",
];

pub(crate) fn parse(values: &[String]) -> Result<AgentCommand, CliError> {
    match values.get(1).map(String::as_str) {
        Some("run") => {
            let mut prompt_file = None;
            let mut workspace = None;
            let mut timeout_secs: Option<u64> = None;
            let mut session = None;
            let mut mode = None;
            let mut model = None;
            let mut attachments = Vec::new();
            let mut index = 2;
            while index < values.len() {
                let token = values[index].as_str();
                if !RUN_OPTIONS.contains(&token) {
                    return Err(CliError::usage(format!(
                        "unsupported agent run option: {token}"
                    )));
                }
                let duplicate = match token {
                    "--prompt-file" => prompt_file.is_some(),
                    "--workspace" => workspace.is_some(),
                    "--timeout-secs" => timeout_secs.is_some(),
                    "--session" => session.is_some(),
                    "--mode" => mode.is_some(),
                    "--model" => model.is_some(),
                    _ => false,
                };
                if duplicate {
                    return Err(CliError::usage(format!(
                        "duplicate agent run option {token}"
                    )));
                }
                let value = values.get(index + 1).ok_or_else(|| {
                    CliError::usage(format!("agent run option {token} requires a value"))
                })?;
                if value.is_empty() || value.starts_with("--") {
                    return Err(CliError::usage(format!(
                        "agent run option {token} requires a value"
                    )));
                }
                match token {
                    "--prompt-file" => prompt_file = Some(PathBuf::from(value)),
                    "--workspace" => workspace = Some(PathBuf::from(value)),
                    "--timeout-secs" => {
                        timeout_secs = Some(value.parse::<u64>().ok().filter(|seconds| {
                            *seconds > 0 && *seconds <= AGENT_TIMEOUT_SECS_MAX
                        }).ok_or_else(|| {
                            CliError::usage(format!(
                                "agent run requires --timeout-secs to be a positive integer \
                                 no greater than {AGENT_TIMEOUT_SECS_MAX}"
                            ))
                        })?);
                    }
                    "--session" => {
                        // Same charset gate as the sessions family: an id the
                        // store would reject is a usage error, not a
                        // not-found host failure after the fact.
                        if !crate::support::valid_session_id(value) {
                            return Err(CliError::usage(
                                "agent run --session requires a valid session id ([A-Za-z0-9_-])",
                            ));
                        }
                        session = Some(value.clone());
                    }
                    "--mode" => match value.as_str() {
                        "plan" | "agent" => mode = Some(value.clone()),
                        other => {
                            return Err(CliError::usage(format!(
                                "agent run --mode must be plan or agent (got {other})"
                            )));
                        }
                    },
                    "--model" => model = Some(value.clone()),
                    _ => attachments.push(PathBuf::from(value)),
                }
                index += 2;
            }
            let prompt_file =
                prompt_file.ok_or_else(|| CliError::usage("agent run requires --prompt-file"))?;
            Ok(AgentCommand::Run {
                prompt_file,
                workspace,
                timeout_secs: timeout_secs.unwrap_or(600),
                session,
                mode,
                model,
                attachments,
            })
        }
        _ => Err(CliError::usage("usage: pinvou agent run")),
    }
}

pub(crate) fn execute(command: AgentCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    match command {
        AgentCommand::Run {
            prompt_file,
            workspace,
            timeout_secs,
            session,
            mode,
            model,
            attachments,
        } => run_agent(
            &prompt_file,
            workspace.as_deref(),
            timeout_secs,
            session.as_deref(),
            mode.as_deref(),
            model,
            attachments,
            output,
        ),
    }
}

#[cfg(not(feature = "product-backend"))]
#[allow(clippy::too_many_arguments)]
fn run_agent(
    _prompt_file: &Path,
    _workspace: Option<&Path>,
    _timeout_secs: u64,
    _session: Option<&str>,
    _mode: Option<&str>,
    _model: Option<String>,
    _attachments: Vec<PathBuf>,
    _output: OutputMode,
) -> Result<CliOutcome, CliError> {
    Err(CliError::failed("product_backend_not_enabled"))
}

#[cfg(feature = "product-backend")]
#[allow(clippy::too_many_arguments)]
fn run_agent(
    prompt_file: &Path,
    workspace: Option<&Path>,
    timeout_secs: u64,
    session: Option<&str>,
    mode: Option<&str>,
    model: Option<String>,
    attachments: Vec<PathBuf>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    // Consistent with the other read failures in lib.rs (read_to_string ->
    // failed): an unreadable file is a host-level failure (exit 1), not an
    // argument usage error — the documented exit-code contract also lists
    // read failures under host-level.
    let prompt = std::fs::read_to_string(prompt_file)
        .map_err(|_| CliError::failed("agent run cannot read --prompt-file"))?;
    // Canonicalize so the engine receives an absolute path regardless of cwd
    // changes, and fail fast on a missing/non-directory workspace instead of
    // letting a typo'd path get silently created deeper in the stack.
    let workspace = match workspace {
        Some(path) => {
            let resolved = std::fs::canonicalize(path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    CliError::failed(format!(
                        "agent run --workspace does not exist: {}",
                        path.display()
                    ))
                } else {
                    CliError::failed(format!(
                        "agent run --workspace cannot be accessed: {} ({error})",
                        path.display()
                    ))
                }
            })?;
            if !resolved.is_dir() {
                return Err(CliError::failed(format!(
                    "agent run --workspace is not a directory: {}",
                    resolved.display()
                )));
            }
            Some(resolved)
        }
        None => None,
    };
    let mode = mode.map(|mode| {
        if mode == "plan" {
            pinvou_product_backend::AgenticTaskMode::Plan
        } else {
            pinvou_product_backend::AgenticTaskMode::Agent
        }
    });
    let attachments = attachments
        .into_iter()
        .map(|path| pinvou_product_backend::AgenticTaskAttachment {
            path,
            remove_after_ingest: false,
        })
        .collect();
    let request = pinvou_product_backend::AgenticTaskRequest {
        prompt,
        workspace,
        timeout_secs,
        // New optional parity fields default to the historical behavior when
        // the flags are absent; the app-side request treats None exactly as
        // before the fields were introduced.
        session_id: session.map(str::to_owned),
        mode,
        model_id: model,
        attachments,
    };
    let report = pinvou_product_backend::run_agentic_task(request)
        .map_err(|error| CliError::failed(format!("agent_run_failed: {error:#}")))?;
    // A fresh run persists its session under the eval-session factory title
    // ("临时评测"), which then reads as a stray user chat in the GUI's
    // session list. Give CLI-created sessions an honest label; best-effort —
    // a failed rename is cosmetic and must not fail the report. A
    // caller-provided session keeps its own title.
    if session.is_none() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let store = pinvou3_lib::features::sessions::SessionStore::boot();
        if let Ok(store) = store {
            let _ = store.set_title(&report.session_id, format!("CLI agent run <{stamp}>"));
        }
    }
    // TB/harness semantics: exit 0 whenever a report is produced (timeouts and
    // in-turn errors live in the report fields and are settled by the
    // harness grader); non-zero exit codes are reserved for host-level
    // failures (unreadable file, unusable backend, ...). Otherwise a timed-out
    // task would be recorded as an exception instead of a 0-reward run and the
    // mean would only cover surviving tasks, skewing scores.
    Ok(CliOutcome {
        exit_code: ExitCode::Success,
        stdout: render_agent_report(&report, output)?,
    })
}

#[cfg(feature = "product-backend")]
fn render_agent_report(
    report: &pinvou_product_backend::AgenticTaskReport,
    output: OutputMode,
) -> Result<String, CliError> {
    match output {
        OutputMode::Json => serde_json::to_string(report).map_err(|error| {
            CliError::failed(format!("agent report serialization failed: {error}"))
        }),
        OutputMode::Human => {
            let mut lines = vec![format!(
                "session: {} status: {}",
                report.session_id, report.status
            )];
            if let Some(error) = &report.error {
                lines.push(format!("error: {error}"));
            }
            if let Some(usage) = &report.usage {
                lines.push(format!(
                    "tokens: input={} output={} tools={}",
                    usage.input_tokens,
                    usage.output_tokens,
                    report.tool_events.len()
                ));
            }
            lines.push(String::new());
            lines.push(report.assistant_text.trim_end().to_string());
            Ok(lines.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CliCommand, ExitCode, parse_args};

    /// The flag-less contract: only the three historical fields are set;
    /// every parity field defaults to "unset" (byte-identical request).
    fn historical(
        prompt_file: PathBuf,
        workspace: Option<PathBuf>,
        timeout_secs: u64,
    ) -> AgentCommand {
        AgentCommand::Run {
            prompt_file,
            workspace,
            timeout_secs,
            session: None,
            mode: None,
            model: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn parse_args_accepts_agent_run_with_options() {
        let parsed = parse_args([
            "pinvou",
            "agent",
            "run",
            "--prompt-file",
            "task.txt",
            "--workspace",
            "/tmp/task",
            "--timeout-secs",
            "900",
        ])
        .unwrap();
        assert_eq!(
            parsed.command(),
            &CliCommand::Agent(historical(
                PathBuf::from("task.txt"),
                Some(PathBuf::from("/tmp/task")),
                900,
            ))
        );
        assert_eq!(parsed.output(), OutputMode::Human);
    }

    #[test]
    fn parse_args_defaults_agent_run_timeout_and_workspace() {
        let parsed = parse_args(["pinvou", "agent", "run", "--prompt-file", "task.txt"]).unwrap();
        assert_eq!(
            parsed.command(),
            &CliCommand::Agent(historical(PathBuf::from("task.txt"), None, 600))
        );
    }

    #[test]
    fn parse_args_rejects_agent_run_without_prompt_file() {
        let error = parse_args(["pinvou", "agent", "run"]).unwrap_err();
        assert_eq!(error.exit_code(), ExitCode::Usage);
        assert!(error.to_string().contains("--prompt-file"));
    }

    #[test]
    fn parse_args_rejects_non_positive_agent_timeout() {
        let error = parse_args([
            "pinvou",
            "agent",
            "run",
            "--prompt-file",
            "task.txt",
            "--timeout-secs",
            "0",
        ])
        .unwrap_err();
        assert_eq!(error.exit_code(), ExitCode::Usage);
        assert!(error.to_string().contains("positive integer"));
    }

    #[test]
    fn parse_args_rejects_oversized_agent_timeout() {
        // u64::MAX parses fine, but Instant + Duration would overflow and
        // panic; the parse layer must reject it with a usage error carrying
        // the cap.
        let error = parse_args([
            "pinvou",
            "agent",
            "run",
            "--prompt-file",
            "task.txt",
            "--timeout-secs",
            &(u64::MAX.to_string()),
        ])
        .unwrap_err();
        assert_eq!(error.exit_code(), ExitCode::Usage);
        assert!(error.to_string().contains("no greater than"));
        assert_eq!(AGENT_TIMEOUT_SECS_MAX, 7 * 24 * 60 * 60);
    }

    /// The CLI parse cap and the library clamp guard the same
    /// `Instant + Duration` overflow; they must never diverge.
    #[cfg(feature = "product-backend")]
    #[test]
    fn agent_timeout_cap_matches_library_clamp() {
        assert_eq!(
            AGENT_TIMEOUT_SECS_MAX,
            pinvou_product_backend::MAX_TIMEOUT_SECS
        );
    }

    #[test]
    fn parse_args_rejects_unknown_agent_subcommand() {
        let error = parse_args(["pinvou", "agent", "status"]).unwrap_err();
        assert_eq!(error.exit_code(), ExitCode::Usage);
    }

    #[test]
    fn parse_args_keeps_output_flag_for_agent_run() {
        let parsed = parse_args([
            "pinvou",
            "--output",
            "json",
            "agent",
            "run",
            "--prompt-file",
            "task.txt",
        ])
        .unwrap();
        assert_eq!(parsed.output(), OutputMode::Json);
    }

    #[test]
    fn parse_args_accepts_every_new_parity_flag() {
        let parsed = parse_args([
            "pinvou",
            "agent",
            "run",
            "--prompt-file",
            "task.txt",
            "--session",
            "sess-1",
            "--mode",
            "plan",
            "--model",
            "model-1",
            "--attach",
            "a.md",
            "--attach",
            "/tmp/b.md",
        ])
        .unwrap();
        assert_eq!(
            parsed.command(),
            &CliCommand::Agent(AgentCommand::Run {
                prompt_file: PathBuf::from("task.txt"),
                workspace: None,
                timeout_secs: 600,
                session: Some("sess-1".into()),
                mode: Some("plan".into()),
                model: Some("model-1".into()),
                attachments: vec![PathBuf::from("a.md"), PathBuf::from("/tmp/b.md")],
            })
        );
    }

    #[test]
    fn parse_args_accepts_agent_mode_explicitly() {
        let parsed = parse_args([
            "pinvou",
            "agent",
            "run",
            "--prompt-file",
            "task.txt",
            "--mode",
            "agent",
        ])
        .unwrap();
        match parsed.command() {
            CliCommand::Agent(AgentCommand::Run { mode, .. }) => {
                assert_eq!(mode.as_deref(), Some("agent"));
            }
            other => panic!(
                "parsed an unexpected command family; the fixture argv does not match the test"
            ),
        }
    }

    #[test]
    fn parse_args_rejects_unknown_mode_with_usage_exit_code() {
        let error = parse_args([
            "pinvou",
            "agent",
            "run",
            "--prompt-file",
            "task.txt",
            "--mode",
            "bogus",
        ])
        .unwrap_err();
        assert_eq!(error.exit_code(), ExitCode::Usage);
        assert!(error.to_string().contains("plan or agent"));
    }

    #[test]
    fn parse_args_rejects_invalid_new_flag_values_and_duplicates() {
        let invalid = [
            vec![
                "pinvou",
                "agent",
                "run",
                "--prompt-file",
                "task.txt",
                "--mode",
            ],
            vec![
                "pinvou",
                "agent",
                "run",
                "--prompt-file",
                "task.txt",
                "--session",
            ],
            vec![
                "pinvou",
                "agent",
                "run",
                "--prompt-file",
                "task.txt",
                "--model",
            ],
            vec![
                "pinvou",
                "agent",
                "run",
                "--prompt-file",
                "task.txt",
                "--attach",
            ],
            vec![
                "pinvou",
                "agent",
                "run",
                "--prompt-file",
                "task.txt",
                "--session",
                "a",
                "--session",
                "b",
            ],
            vec![
                "pinvou",
                "agent",
                "run",
                "--prompt-file",
                "task.txt",
                "--model",
                "a",
                "--model",
                "b",
            ],
            vec![
                "pinvou",
                "agent",
                "run",
                "--prompt-file",
                "task.txt",
                "--keep-session",
            ],
            vec![
                "pinvou",
                "agent",
                "run",
                "--prompt-file",
                "task.txt",
                "--bogus",
            ],
            vec![
                "pinvou",
                "agent",
                "run",
                "--prompt-file",
                "task.txt",
                "--session",
                "--model",
            ],
        ];
        for arguments in invalid {
            let error = parse_args(&arguments).expect_err(arguments.join(" ").as_str());
            assert_eq!(error.exit_code(), ExitCode::Usage, "{arguments:?}");
        }
    }
}
