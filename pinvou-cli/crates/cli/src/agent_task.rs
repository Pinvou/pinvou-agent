//! `agent run` family: prompt-file driven agentic task execution.
//!
//! Moved verbatim from the former monolithic `lib.rs`; the parse defaults and
//! the exit contract must not change.

use std::path::{Path, PathBuf};

use crate::{CliError, CliOutcome, ExitCode, OutputMode, named_options, option};

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
    },
}

pub(crate) fn parse(values: &[String]) -> Result<AgentCommand, CliError> {
    match values.get(1).map(String::as_str) {
        Some("run") => {
            let options = named_options(
                &values[2..],
                &["--prompt-file", "--workspace", "--timeout-secs"],
            )?;
            let prompt_file = option(&options, "--prompt-file")
                .map(PathBuf::from)
                .ok_or_else(|| CliError::usage("agent run requires --prompt-file"))?;
            let workspace = option(&options, "--workspace").map(PathBuf::from);
            let timeout_secs = match option(&options, "--timeout-secs") {
                None => 600,
                Some(value) => value
                    .parse::<u64>()
                    .ok()
                    .filter(|seconds| *seconds > 0 && *seconds <= AGENT_TIMEOUT_SECS_MAX)
                    .ok_or_else(|| {
                        CliError::usage(format!(
                            "agent run requires --timeout-secs to be a positive integer \
                             no greater than {AGENT_TIMEOUT_SECS_MAX}"
                        ))
                    })?,
            };
            Ok(AgentCommand::Run {
                prompt_file,
                workspace,
                timeout_secs,
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
        } => run_agent(&prompt_file, workspace.as_deref(), timeout_secs, output),
    }
}

#[cfg(not(feature = "product-backend"))]
fn run_agent(
    _prompt_file: &Path,
    _workspace: Option<&Path>,
    _timeout_secs: u64,
    _output: OutputMode,
) -> Result<CliOutcome, CliError> {
    Err(CliError::failed("product_backend_not_enabled"))
}

#[cfg(feature = "product-backend")]
fn run_agent(
    prompt_file: &Path,
    workspace: Option<&Path>,
    timeout_secs: u64,
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
    let request = pinvou_product_backend::AgenticTaskRequest {
        prompt,
        workspace,
        timeout_secs,
        // New optional parity fields (session_id/mode/model_id/attachments)
        // default to the historical behavior until the CLI gains flags for
        // them.
        ..Default::default()
    };
    let report = pinvou_product_backend::run_agentic_task(request)
        .map_err(|error| CliError::failed(format!("agent_run_failed: {error:#}")))?;
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
            &CliCommand::Agent(AgentCommand::Run {
                prompt_file: PathBuf::from("task.txt"),
                workspace: Some(PathBuf::from("/tmp/task")),
                timeout_secs: 900,
            })
        );
        assert_eq!(parsed.output(), OutputMode::Human);
    }

    #[test]
    fn parse_args_defaults_agent_run_timeout_and_workspace() {
        let parsed = parse_args(["pinvou", "agent", "run", "--prompt-file", "task.txt"]).unwrap();
        assert_eq!(
            parsed.command(),
            &CliCommand::Agent(AgentCommand::Run {
                prompt_file: PathBuf::from("task.txt"),
                workspace: None,
                timeout_secs: 600,
            })
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
}
