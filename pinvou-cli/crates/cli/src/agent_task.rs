//! `agent run` family: prompt-file driven agentic task execution.
//!
//! Extracted from the former monolithic `lib.rs`: the historical parse
//! defaults and the exit contract are preserved, while the parity flags
//! (`--session/--mode/--model/--attach`) layered on top of the engine's
//! session persistence are new in this PR.

use std::path::{Path, PathBuf};

use crate::{CliError, CliOutcome, OutputMode};

/// Exit code for the success exit of the featureful run path; the import
/// lives under the feature with its only consumer so the featureless lib
/// target does not carry an unused name (tests import their own).
#[cfg(feature = "product-backend")]
use crate::ExitCode;

/// Parse-time cap for `agent run --timeout-secs` (7 days): an unbounded u64
/// would overflow `Instant + Duration`, exiting 101 with no report. Must stay
/// in lockstep with the library clamp `pinvou_product_backend::MAX_TIMEOUT_SECS`
/// (asserted equal by `agent_timeout_cap_matches_library_clamp`).
const AGENT_TIMEOUT_SECS_MAX: u64 = 7 * 24 * 60 * 60;

/// Byte cap for `--prompt-file`. The read is bounded (`Read::take`) so an
/// unbounded source cannot be pulled into memory before the engine ever sees
/// it; 4 MiB is far above any real task prompt and far below an OOM.
/// Feature-gated with the capped read that enforces it; the featureless
/// stub never reads the file, so the usage text below carries the "4 MiB"
/// figure and the usage test asserts the same literal.
#[cfg(feature = "product-backend")]
const PROMPT_FILE_MAX_BYTES: usize = 4 * 1024 * 1024;

/// The family's own usage text. It spells out the two input rules the parse
/// layer cannot enforce from argv alone, because they are the only places
/// where `agent run` refuses something a plain `read_to_string` accepted:
/// the prompt file must be a REGULAR file (a symlink to one is fine; a FIFO,
/// a character device, `/dev/stdin` or a `<(...)` process substitution is
/// refused, because their read blocks before any cap or deadline could act),
/// and it must fit in [`PROMPT_FILE_MAX_BYTES`].
const RUN_USAGE: &str = "usage: pinvou agent run --prompt-file <FILE> [--workspace <DIR>] \
     [--timeout-secs <SECONDS>] [--session <ID>] [--mode plan|agent] [--model <ID>] \
     [--attach <PATH>]...\n  --prompt-file must be a regular file (symlinks are followed; \
     FIFOs, character devices and /dev/stdin are refused) of at most 4 MiB";

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
            let prompt_file = prompt_file.ok_or_else(|| {
                CliError::usage(format!("agent run requires --prompt-file\n{RUN_USAGE}"))
            })?;
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
        _ => Err(CliError::usage(RUN_USAGE)),
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
    // Same absolute-path contract as every other family — and this one most
    // of all, since it spawns a real shell agent: a relative PINVOU3_HOME
    // would silently materialize the session store under the cwd.
    crate::support::sandbox_home()?;
    // Consistent with the other read failures in lib.rs (read_to_string ->
    // failed): an unreadable file is a host-level failure (exit 1), not an
    // argument usage error — the documented exit-code contract also lists
    // read failures under host-level. The two extra refusals this helper adds
    // over a plain `read_to_string` (over PROMPT_FILE_MAX_BYTES, and anything
    // that is not a regular file) are stated in RUN_USAGE, because they are a
    // narrowing of what `--prompt-file` used to accept.
    let prompt =
        crate::support::read_text_file_capped(prompt_file, PROMPT_FILE_MAX_BYTES, "agent run")?;
    // Consumed persona (round-18 wiring): a `pinvou personas equip` on this
    // session staged a one-shot persona body on the per-session sidecar
    // `persona_equipped.json`; this lane now delivers it through
    // [`prompt_with_persona_injection`] — the same injection point the GUI
    // chat send uses. No staged persona → the prompt passes through
    // verbatim, byte for byte (the hard no-behavior-change requirement).
    // Fresh sessions never consult the sidecar (nothing can be equipped on a
    // session that does not exist yet).
    let prompt = prompt_with_persona_injection(
        prompt,
        session.and_then(crate::personas::pending_persona_injection),
    );
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
    // `--session` resumes a store the desktop app may be actively writing:
    // both engines load-modify-save the same transcript JSON, so a run
    // against a GUI-open session is last-writer-wins on the whole file (the
    // store's `set_title` comment names the same hazard). A run without
    // `--mode` sends Agent (full-write) turns even into a session the GUI
    // has in Plan mode — pass `--mode plan` to keep its read-only contract.
    let mode = mode.map(|mode| {
        if mode == "plan" {
            pinvou3_lib::agentic_task::AgenticTaskMode::Plan
        } else {
            pinvou3_lib::agentic_task::AgenticTaskMode::Agent
        }
    });
    let attachments = attachments
        .into_iter()
        .map(|path| pinvou3_lib::agentic_task::AgenticTaskAttachment {
            path,
            remove_after_ingest: false,
        })
        .collect();
    let request = pinvou_product_backend::AgenticTaskRequest {
        // The consumed persona body travels inside the prompt (prepended
        // above, same point the GUI injects): the headless engine call has
        // no separate persona lane, and the GUI's own one-shot body reaches
        // the engine the same way — as part of the submitted user message.
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
    // Retention warning: the engine arms the real eviction observer (this
    // build has `benchmark-hooks`), so an at-cap fresh run prints the exact
    // evicted count on stderr even when the run later fails — a pre-run
    // count heuristic here would only duplicate that warning and cry wolf
    // on runs that fail before the evicting prepare-time save, so the CLI
    // does not add one of its own.
    let report = pinvou_product_backend::run_agentic_task(request)
        .map_err(|error| CliError::failed(format!("agent_run_failed: {error:#}")))?;
    // The consumed persona was one-shot and the turn that consumed it has
    // now been submitted (setup faults above propagate and skip this). Clear
    // the staged body, keeping the persona_id on the sidecar — `personas
    // active` keeps reporting the card until `unequip`, the same split the
    // GUI's in-memory take leaves behind. A failure here is warned, not
    // fatal: the report exists, the turn ran, and failing the whole run
    // after a submitted turn would misreport it as never-started. A
    // re-run against the same session then sees no staged body (the
    // already-consumed state), which is exactly the one-shot contract.
    if let Some(session_id) = session {
        if let Err(error) = crate::personas::consume_pending_persona_injection(session_id) {
            crate::note!(
                "warning: agent run: could not clear the consumed persona sidecar: {error}"
            );
        }
    }
    // A fresh run's session is deliberately left under the shared new-chat
    // placeholder, and the CLI does NOT rename it. Two reasons, both from this
    // PR's own contracts: the placeholder is the exact sentinel the GUI's
    // auto-rename triggers on, so writing a literal label here would freeze
    // the session name forever — and it would be untranslated English in the
    // zh/en/ja UI, which `features/sessions/store.rs` refuses for headless
    // sessions and AGENTS.md §4 forbids for UI-visible copy. Second, renaming
    // from here meant a SECOND `SessionStore` instance whose write serializer
    // is per-instance, so its load-modify-persist-whole-file raced the desktop
    // app's. The session id below identifies the run; naming it is the GUI's
    // job (or the caller's, via `pinvou sessions rename`).
    //
    // The reported id is resolvable: the engine only deletes a fresh session
    // when the run returned no report at all (exit 1, nothing printed). The
    // one exception is the caller's own `PINVOU3_AGENT_TASK_KEEP_SESSION=0`,
    // whose entire purpose is to remove this run's session afterwards.
    //
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
/// Prepends the session's staged one-shot persona body to the turn's prompt,
/// at the exact injection point the GUI chat send uses
/// (`app/commands/chat.rs`: `full = format!("{body}\n\n---\n\n{full}")` after
/// `take_pending_turn_injections` — the persona body first, then the
/// `\n\n---\n\n` separator, then the user's message). `None` passes the
/// prompt through unchanged, byte for byte: a session without an equipped
/// persona (the only case before this wiring) must behave identically.
///
/// Pure on purpose: the composition is the testable part of the wiring. The
/// full `run_agent` path needs a real engine (deliberately out of scope for
/// the hermetic contract tests), so this helper is what the unit tests pin —
/// including the separator, so the headless lane cannot drift from the GUI's
/// injection point.
fn prompt_with_persona_injection(prompt: String, injection: Option<String>) -> String {
    match injection {
        Some(injection) => format!("{injection}\n\n---\n\n{prompt}"),
        None => prompt,
    }
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

    /// The report renderer is pure; pin its human layout and its JSON round
    /// trip (the only execution-path piece testable without an engine).
    #[cfg(feature = "product-backend")]
    #[test]
    fn agent_report_renders_human_layout_and_json_envelope() {
        let report = pinvou_product_backend::AgenticTaskReport {
            session_id: "s-1".to_owned(),
            status: "completed".to_owned(),
            timed_out: false,
            completed_after_deadline: false,
            assistant_text: "done\n".to_owned(),
            tool_events: Vec::new(),
            usage: Some(pinvou3_lib::agentic_task::AgenticUsageReport {
                input_tokens: 10,
                output_tokens: 5,
                cache_hit_tokens: 0,
                cache_miss_tokens: 0,
                cache_write_tokens: 0,
                reasoning_tokens: 0,
                context_window: 0,
            }),
            error: None,
        };
        let human = render_agent_report(&report, OutputMode::Human).unwrap();
        assert!(
            human.contains("session: s-1 status: completed"),
            "human report must lead with the session line: {human}"
        );
        assert!(
            human.contains("tokens: input=10 output=5 tools=0"),
            "human report must carry the usage line: {human}"
        );
        assert!(
            human.ends_with("done"),
            "the assistant text ends the report: {human}"
        );

        let json = render_agent_report(&report, OutputMode::Json).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&json).expect("json mode must render a single-line JSON report");
        assert_eq!(value["session_id"], serde_json::json!("s-1"));
        assert_eq!(value["status"], serde_json::json!("completed"));
    }

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

    /// `--prompt-file` accepts strictly less than a plain `read_to_string`
    /// did (a byte cap, and regular files only). A narrowing a user can hit
    /// with `--prompt-file /dev/stdin` or `<(generate)` has to be readable
    /// somewhere the user already looks, so the family usage text carries it
    /// and this test keeps the two from drifting apart.
    #[test]
    fn agent_run_usage_states_the_prompt_file_input_rules() {
        // The figure is stated twice on purpose: as the featureful path's
        // enforced cap (PROMPT_FILE_MAX_BYTES) and here, so the usage text
        // cannot drift from the cap either way. Featureless builds see the
        // same literal; the cap itself rides the feature with its reader.
        #[cfg(feature = "product-backend")]
        let enforced_cap = PROMPT_FILE_MAX_BYTES;
        #[cfg(not(feature = "product-backend"))]
        let enforced_cap: usize = 4 * 1024 * 1024;
        assert_eq!(enforced_cap, 4 * 1024 * 1024);
        assert!(RUN_USAGE.contains("4 MiB"), "{RUN_USAGE}");
        assert!(RUN_USAGE.contains("regular file"), "{RUN_USAGE}");
        assert!(RUN_USAGE.contains("/dev/stdin"), "{RUN_USAGE}");
        // Every flag the parser accepts is named in the usage text, so the
        // text cannot silently fall behind RUN_OPTIONS.
        for option in RUN_OPTIONS {
            assert!(
                RUN_USAGE.contains(option),
                "{option} is accepted but missing from the usage text"
            );
        }
        // Both usage errors route through it.
        let error = parse_args(["pinvou", "agent", "status"]).unwrap_err();
        assert!(error.to_string().contains("--prompt-file"), "{error}");
        let error = parse_args(["pinvou", "agent", "run"]).unwrap_err();
        assert!(error.to_string().contains("4 MiB"), "{error}");
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
            _other => panic!(
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

    /// The persona wiring composes the turn prompt offline exactly the way
    /// the GUI chat send does online: persona body first, `\n\n---\n\n`,
    /// then the user's message (chat.rs injects
    /// `format!("{body}\n\n---\n\n{full}")`). The separator is pinned byte
    /// for byte so the headless lane cannot drift from the GUI's injection
    /// point, and the staged text is embedded verbatim (the wrapper around
    /// the card body already happened at `personas equip` time, via
    /// `equip_body_injection`).
    #[cfg(feature = "product-backend")]
    #[test]
    fn prompt_with_persona_injection_matches_the_gui_injection_point() {
        let injection = "【你被加持了一张专家面具:Test】\n\n====== 专家人设开始 ======\n\
                         # Persona\n\nbody\n====== 专家人设结束 ======";
        let composed =
            prompt_with_persona_injection("do the task".to_owned(), Some(injection.to_owned()));
        let expected = format!("{injection}\n\n---\n\ndo the task");
        assert_eq!(composed, expected);
        // Composition is prepend-only: the user's message survives untouched
        // at the tail of the prompt.
        assert!(composed.ends_with("\n\n---\n\ndo the task"));
    }

    /// The no-persona case is the hard requirement from the wiring review:
    /// `None` must leave the prompt bit-identical (no injection markers, no
    /// separator, no whitespace drift) — every pre-wiring invocation took
    /// this arm, so its behavior is a contract.
    #[cfg(feature = "product-backend")]
    #[test]
    fn prompt_without_persona_injection_is_byte_identical() {
        let prompt = "# Task\n\nmulti-line\nbody with --- separators\n\nend\n";
        let passthrough = prompt_with_persona_injection(prompt.to_owned(), None);
        assert_eq!(passthrough, prompt);
        assert_eq!(passthrough.len(), prompt.len());
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
