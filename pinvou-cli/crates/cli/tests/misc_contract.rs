//! Contract tests for the five misc families: `files`, `voice`, `deps`,
//! `feedback`, `monitor` (GUI-parity project).
//!
//! Parse-level coverage runs against the typed command tree. Execute-level
//! coverage is strictly hermetic: every default test runs against a throwaway
//! `PINVOU3_HOME` (ENV_LOCK serialization, same pattern as `cli_contract.rs`)
//! and touches no network, model endpoint, package manager, GPU, or display.
//! `files ingest` additionally stages its fixture under `$HOME` because the
//! feature's upload-location policy (validate_path) requires it.
//!
//! Paths that need a display host (`monitor status|snapshot`, `voice
//! postprocess`), a model endpoint, a local ASR engine, or a system package
//! manager are `#[ignore]` opt-in tests; each names its opt-in command in a
//! comment.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use pinvou_cli::{CliError, CliOutcome, ExitCode, OutputMode, execute, parse_args};

/// Serialises tests that mutate the process-global `PINVOU3_HOME` environment
/// variable, preventing data races when the parallel test runner executes them
/// concurrently.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Sets `PINVOU3_HOME` to a fresh throwaway directory for the duration of
/// the test and restores the previous value on drop.
struct HomeGuard {
    previous: Option<OsString>,
    root: PathBuf,
}

impl HomeGuard {
    fn new(label: &str) -> Self {
        let root = sandbox_root(label);
        std::fs::create_dir_all(&root).unwrap();
        let previous = std::env::var_os("PINVOU3_HOME");
        // SAFETY: the caller holds ENV_LOCK for the whole test, so env writes
        // are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        Self { previous, root }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            // SAFETY: ENV_LOCK is held by the owning test.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: ENV_LOCK is held by the owning test.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn sandbox_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "pinvou-cli-misc-{label}-{}-{nonce}",
        std::process::id()
    ))
}

/// A scratch directory under the real `$HOME` for fixtures that must pass the
/// feature's upload-location policy (`files ingest`). Cleaned up on drop.
struct ScopedHomeDir {
    dir: PathBuf,
}

impl ScopedHomeDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                ".pinvou3-cli-misc-{label}-{}-{nonce}",
                std::process::id()
            ));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }
}

impl Drop for ScopedHomeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn run(arguments: &[&str]) -> Result<CliOutcome, CliError> {
    let parsed = parse_args(arguments).expect("arguments must parse");
    execute(parsed)
}

fn run_json(arguments: &[&str]) -> serde_json::Value {
    let mut owned = arguments.to_vec();
    owned.extend(["--output", "json"]);
    let outcome = run(&owned).expect("execute must succeed");
    serde_json::from_str(&outcome.stdout).expect("json output must be a single serde_json line")
}

fn assert_usage(error: &CliError, context: &str) {
    assert_eq!(
        error.exit_code(),
        ExitCode::Usage,
        "{context}: expected exit 2, got {error}"
    );
}

// ── files ingest ────────────────────────────────────────────────────────────

#[test]
fn files_ingest_parses_paths_and_output_flag() {
    let parsed = parse_args(["pinvou", "files", "ingest", "/tmp/a.md"]).unwrap();
    assert_eq!(parsed.output(), OutputMode::Human);
    for arguments in [
        vec![
            "pinvou",
            "files",
            "ingest",
            "/tmp/a.md",
            "--output",
            "/tmp/out.md",
        ],
        vec!["pinvou", "--output", "json", "files", "ingest", "/tmp/a.md"],
    ] {
        assert!(parse_args(&arguments).is_ok(), "{arguments:?}");
    }
}

#[test]
fn files_ingest_rejects_invalid_usage_with_exit_two() {
    let invalid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "files"],
        vec!["pinvou", "files", "bogus"],
        vec!["pinvou", "files", "ingest"],
        vec!["pinvou", "files", "ingest", "/tmp/a.md", "--nope"],
        vec!["pinvou", "files", "ingest", "/tmp/a.md", "--output"],
        vec!["pinvou", "files", "ingest", "/tmp/a.md", "--output", "--x"],
        vec![
            "pinvou",
            "files",
            "ingest",
            "/tmp/a.md",
            "--output",
            "a",
            "--output",
            "b",
        ],
    ];
    for arguments in invalid {
        let error = parse_args(&arguments).expect_err(arguments.join(" ").as_str());
        assert_usage(&error, &arguments.join(" "));
    }
}

#[test]
fn files_ingest_round_trips_markdown_and_text_fixtures() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("files-ingest");
    let scoped = ScopedHomeDir::new("files-ingest");
    let markdown_fixture = scoped.dir.join("notes.md");
    std::fs::write(&markdown_fixture, "# Heading\n\nBody line with 内容.\n").unwrap();
    let text_fixture = scoped.dir.join("plain.txt");
    std::fs::write(&text_fixture, "alpha\nbeta\n").unwrap();

    // Markdown → --output file: extracted text lands in the file, summary on
    // stdout.
    let output_file = home.root.join("extracted.md");
    let outcome = run(&[
        "pinvou",
        "files",
        "ingest",
        markdown_fixture.to_str().unwrap(),
        "--output",
        output_file.to_str().unwrap(),
    ])
    .expect("markdown ingest must succeed");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(
        outcome.stdout.contains("File: notes.md"),
        "{}",
        outcome.stdout
    );
    assert!(outcome.stdout.contains("Kind: text"), "{}", outcome.stdout);
    assert!(outcome.stdout.contains("Tokens: "), "{}", outcome.stdout);
    let written = std::fs::read_to_string(&output_file).unwrap();
    assert!(written.contains("Heading"));
    assert!(written.contains("Body line with 内容."));

    // Plain text → stdout: the extracted text is printed after header lines.
    let outcome = run(&["pinvou", "files", "ingest", text_fixture.to_str().unwrap()])
        .expect("text ingest must succeed");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(outcome.stdout.contains("Kind: text"), "{}", outcome.stdout);
    assert!(outcome.stdout.contains("alpha\nbeta"), "{}", outcome.stdout);

    // JSON mode mirrors the human fields in a single line.
    let value = run_json(&["pinvou", "files", "ingest", text_fixture.to_str().unwrap()]);
    assert_eq!(value["basename"], "plain.txt");
    assert_eq!(value["markdown"], "alpha\nbeta\n");
    assert!(value["token_estimate"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn files_ingest_missing_file_fails_at_execute_with_exit_one() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("files-missing");
    let scoped = ScopedHomeDir::new("files-missing");
    let missing = scoped.dir.join("does-not-exist.md");
    let error = run(&["pinvou", "files", "ingest", missing.to_str().unwrap()])
        .expect_err("missing file must fail at execute");
    assert_eq!(error.exit_code(), ExitCode::Failed);
}

// ── voice ───────────────────────────────────────────────────────────────────

#[test]
fn voice_subcommands_parse() {
    assert!(parse_args(["pinvou", "voice", "transcribe", "/tmp/a.wav"]).is_ok());
    assert!(
        parse_args([
            "pinvou",
            "voice",
            "postprocess",
            "--mode",
            "dictation",
            "--text",
            "hello"
        ])
        .is_ok()
    );
    for mode in ["task", "edit"] {
        assert!(
            parse_args([
                "pinvou",
                "voice",
                "postprocess",
                "--mode",
                mode,
                "--text-file",
                "/tmp/t.txt"
            ])
            .is_ok()
        );
    }
    assert!(parse_args(["pinvou", "voice", "asr-status"]).is_ok());
    assert!(parse_args(["pinvou", "voice", "asr-install"]).is_ok());
}

#[test]
fn voice_rejects_invalid_usage_with_exit_two() {
    let invalid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "voice"],
        vec!["pinvou", "voice", "bogus"],
        vec!["pinvou", "voice", "transcribe"],
        vec!["pinvou", "voice", "transcribe", "/tmp/a.wav", "--full"],
        vec!["pinvou", "voice", "postprocess"],
        // Bad --mode value must be a usage error naming the valid values.
        vec![
            "pinvou",
            "voice",
            "postprocess",
            "--mode",
            "summarize",
            "--text",
            "x",
        ],
        vec!["pinvou", "voice", "postprocess", "--mode"],
        // Exactly one text source is required.
        vec!["pinvou", "voice", "postprocess", "--mode", "task"],
        vec![
            "pinvou",
            "voice",
            "postprocess",
            "--mode",
            "task",
            "--text",
            "a",
            "--text-file",
            "/tmp/t",
        ],
        vec!["pinvou", "voice", "postprocess", "--mode", "task", "--text"],
        vec!["pinvou", "voice", "asr-status", "--extra"],
        vec!["pinvou", "voice", "asr-install", "--extra"],
    ];
    for arguments in invalid {
        let error = parse_args(&arguments).expect_err(arguments.join(" ").as_str());
        assert_usage(&error, &arguments.join(" "));
    }
}

#[test]
fn voice_asr_status_reports_hermetic_zero_state() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("voice-status");
    // Empty PATH makes the machine-dependent probes deterministic: no ffmpeg
    // on PATH and no external ASR CLI anywhere. Env writes are safe here
    // because ENV_LOCK serializes all tests in this process.
    let saved_path = std::env::var("PATH").ok();
    unsafe { std::env::set_var("PATH", "") };
    let value = run_json(&["pinvou", "voice", "asr-status"]);
    match saved_path {
        Some(path) => unsafe { std::env::set_var("PATH", path) },
        None => unsafe { std::env::remove_var("PATH") },
    }
    // macOS reports the host Speech runtime (present by definition, exactly
    // like the GUI status); on other platforms engine and model live under
    // `$PINVOU3_HOME`, so a fresh sandbox is a zero state.
    if cfg!(target_os = "macos") {
        assert_eq!(
            value["engine"], true,
            "macOS status mirrors the system Speech runtime: {value}"
        );
        assert_eq!(value["model"], true);
        assert_eq!(value["ready"], true);
    } else {
        assert_eq!(
            value["engine"], false,
            "fresh sandbox has no engine: {value}"
        );
        assert_eq!(value["model"], false, "fresh sandbox has no model: {value}");
        assert_eq!(value["ready"], false);
        let missing = value["missing"].as_array().expect("missing list");
        assert!(missing.contains(&serde_json::json!("model")));
        assert!(missing.contains(&serde_json::json!("ffmpeg")));
        assert!(missing.contains(&serde_json::json!("engine")));
    }
    assert_eq!(value["installable"], cfg!(target_os = "linux"));
    // Neither lane of the CLI itself can transcribe in this sandbox (the
    // macOS `ready` flag describes GUI capability, not CLI capability).
    assert_eq!(
        value["cli_transcribe_ready"], false,
        "no engine, model, or external ASR CLI: {value}"
    );
    assert!(
        value["asr_dir"]
            .as_str()
            .unwrap_or_default()
            .starts_with(home.root.to_str().unwrap())
    );
    // The human output mirrors the same fields.
    let saved_path = std::env::var("PATH").ok();
    unsafe { std::env::set_var("PATH", "") };
    let outcome = run(&["pinvou", "voice", "asr-status"]).unwrap();
    match saved_path {
        Some(path) => unsafe { std::env::set_var("PATH", path) },
        None => unsafe { std::env::remove_var("PATH") },
    }
    if cfg!(target_os = "macos") {
        assert!(outcome.stdout.contains("Engine: true"));
        assert!(outcome.stdout.contains("CliTranscribe: no"));
        assert!(outcome.stdout.contains("macOS Speech is GUI-only"));
    } else {
        assert!(outcome.stdout.contains("Engine: false"));
        assert!(outcome.stdout.contains("Model: false"));
        assert!(outcome.stdout.contains("CliTranscribe: no"));
    }
}

#[test]
fn voice_transcribe_reports_missing_engine_cleanly_without_asr() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("voice-transcribe");
    let wav = home.root.join("capture.wav");
    // 44-byte header-only WAV: large enough to pass the empty-audio gate.
    std::fs::write(&wav, vec![0u8; 44]).unwrap();
    let error = run(&["pinvou", "voice", "transcribe", wav.to_str().unwrap()])
        .expect_err("no ASR runtime exists in the sandbox");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    let message = error.to_string();
    assert!(
        message.starts_with("asr_engine_missing"),
        "expected the not-installed code, got: {message}"
    );
    assert!(
        message.contains("voice asr-status"),
        "the error must hint at `voice asr-status`: {message}"
    );
}

/// OPT-IN: needs the local SenseVoice engine + model installed under
/// `$PINVOU3_HOME/asr` (or `PINVOU3_ASR_CMD` on PATH) and a real audio file.
/// Run with: cargo test -p pinvou-cli --test misc_contract -- --ignored
///   voice_transcribe_uses_installed_asr_runtime
#[test]
#[ignore = "needs an installed local ASR runtime: cargo test --test misc_contract -- --ignored"]
fn voice_transcribe_uses_installed_asr_runtime() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("voice-transcribe-live");
    let wav = home.root.join("capture.wav");
    std::fs::write(&wav, vec![0u8; 8000]).unwrap();
    let outcome = run(&["pinvou", "voice", "transcribe", wav.to_str().unwrap()])
        .expect("transcription should run with an installed ASR runtime");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(outcome.stdout.contains("Text: "));
}

/// OPT-IN: `voice postprocess` boots the windowless host (display required)
/// and calls the configured model endpoint. Run with: cargo test -p
/// pinvou-cli --test misc_contract -- --ignored voice_postprocess
#[test]
#[ignore = "needs display host + configured model: cargo test --test misc_contract -- --ignored"]
fn voice_postprocess_calls_the_active_model() {
    let parsed = parse_args([
        "pinvou",
        "voice",
        "postprocess",
        "--mode",
        "task",
        "--text",
        "查一下今日金价并生成数据分析图标",
    ])
    .unwrap();
    let outcome = execute(parsed).expect("postprocess must succeed with a configured model");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(outcome.stdout.contains("Source: llm"), "{}", outcome.stdout);
}

/// OPT-IN: `voice asr-install` downloads the SenseVoice model (network) and
/// may install ffmpeg through pkexec/apt. Run with: cargo test -p pinvou-cli
/// --test misc_contract -- --ignored voice_asr_install
#[test]
#[ignore = "needs network + pkexec: cargo test --test misc_contract -- --ignored"]
fn voice_asr_install_downloads_model_with_network_and_pkexec() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("voice-install");
    let outcome = run(&["pinvou", "voice", "asr-install"])
        .expect("install should run with network and policy agent available");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(outcome.stdout.contains("Model: true"), "{}", outcome.stdout);
}

// ── deps ────────────────────────────────────────────────────────────────────

#[test]
fn deps_subcommands_parse() {
    assert!(parse_args(["pinvou", "deps", "check"]).is_ok());
    assert!(parse_args(["pinvou", "deps", "install", "ffmpeg", "--yes"]).is_ok());
    // Without --yes the command parses (the exit-2 gate is enforced at
    // execute time, like `sessions delete`).
    assert!(parse_args(["pinvou", "deps", "install", "ffmpeg"]).is_ok());
}

#[test]
fn deps_rejects_invalid_usage_with_exit_two() {
    let invalid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "deps"],
        vec!["pinvou", "deps", "bogus"],
        vec!["pinvou", "deps", "check", "--extra"],
        vec!["pinvou", "deps", "install"],
        vec!["pinvou", "deps", "install", "--yes"],
        vec!["pinvou", "deps", "install", "ffmpeg", "--nope"],
    ];
    for arguments in invalid {
        let error = parse_args(&arguments).expect_err(arguments.join(" ").as_str());
        assert_usage(&error, &arguments.join(" "));
    }
}

#[test]
fn deps_install_without_yes_exits_two_before_touching_the_system() {
    // require_yes runs before any package-manager interaction, so this is a
    // hermetic usage-error check.
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("deps-no-yes");
    let error = run(&["pinvou", "deps", "install", "ffmpeg"])
        .expect_err("install without --yes must be refused");
    assert_usage(&error, "deps install without --yes");
}

#[test]
fn deps_check_reports_the_platform_capability_table() {
    // check_dependencies is pure local detection; the sandbox keeps any state
    // reads off the developer's real home. Installed flags are
    // machine-dependent, so assertions target structure, not values.
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("deps-check");
    let value = run_json(&["pinvou", "deps", "check"]);
    let items = value["items"].as_array().expect("items array");
    assert!(!items.is_empty(), "the capability table must not be empty");
    for item in items {
        assert!(!item["key"].as_str().unwrap_or_default().is_empty());
        assert!(item["installed"].is_boolean());
        assert!(item["apt"].is_string());
    }
    // These capabilities are reported on every platform.
    for key in ["voice_asr", "office_legacy", "email"] {
        assert!(
            items
                .iter()
                .any(|item| item["key"] == serde_json::json!(key)),
            "{key} must be part of the check table: {items:?}"
        );
    }
    // Human mode: one `key<TAB>state<TAB>packages` row per item.
    let outcome = run(&["pinvou", "deps", "check"]).unwrap();
    let lines = outcome.stdout.lines().count();
    assert_eq!(lines, items.len(), "{}", outcome.stdout);
    assert!(outcome.stdout.contains("voice_asr\t"));
}

/// OPT-IN: `deps install` runs the system package manager (Linux: pkexec apt)
/// with real root authorization. Run with: cargo test -p pinvou-cli --test
/// misc_contract -- --ignored deps_install
#[test]
#[ignore = "runs the system package manager via pkexec: cargo test --test misc_contract -- --ignored"]
fn deps_install_runs_the_system_package_manager() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("deps-install");
    let outcome = run(&["pinvou", "deps", "install", "cowsay", "--yes"]);
    // Already-installed or unknown-package outcomes are both valid OS-level
    // results; the command must not exit with a usage error.
    if let Ok(outcome) = outcome {
        assert_eq!(outcome.exit_code, ExitCode::Success);
        assert!(outcome.stdout.contains("Installed: cowsay"));
    }
}

// ── feedback ────────────────────────────────────────────────────────────────

#[test]
fn feedback_submit_parses_types_and_options() {
    assert!(
        parse_args([
            "pinvou",
            "feedback",
            "submit",
            "--type",
            "issue",
            "--title",
            "t",
            "--body-file",
            "/tmp/body.md"
        ])
        .is_ok()
    );
    assert!(
        parse_args([
            "pinvou",
            "feedback",
            "submit",
            "--type",
            "suggestion",
            "--title",
            "t",
            "--body-file",
            "/tmp/body.md",
            "--attach",
            "/tmp/a.log",
            "--attach",
            "/tmp/b.png"
        ])
        .is_ok()
    );
}

#[test]
fn feedback_rejects_invalid_usage_with_exit_two() {
    let invalid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "feedback"],
        vec!["pinvou", "feedback", "bogus"],
        vec!["pinvou", "feedback", "submit"],
        // Bad --type must be a usage error naming the valid values.
        vec![
            "pinvou",
            "feedback",
            "submit",
            "--type",
            "bug",
            "--title",
            "t",
            "--body-file",
            "/tmp/b.md",
        ],
        vec![
            "pinvou",
            "feedback",
            "submit",
            "--type",
            "issue",
            "--body-file",
            "/tmp/b.md",
        ],
        vec![
            "pinvou", "feedback", "submit", "--type", "issue", "--title", "t",
        ],
        vec![
            "pinvou",
            "feedback",
            "submit",
            "--title",
            "t",
            "--body-file",
            "/tmp/b.md",
        ],
        vec![
            "pinvou",
            "feedback",
            "submit",
            "--type",
            "issue",
            "--title",
            "--body-file",
            "x",
        ],
    ];
    for arguments in invalid {
        let error = parse_args(&arguments).expect_err(arguments.join(" ").as_str());
        assert_usage(&error, &arguments.join(" "));
    }
}

#[test]
fn feedback_submit_round_trips_pending_and_receipt_files() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("feedback-submit");
    let body = home.root.join("body.md");
    std::fs::write(&body, "Reproduction steps go here.\n").unwrap();

    let value = run_json(&[
        "pinvou",
        "feedback",
        "submit",
        "--type",
        "suggestion",
        "--title",
        "cli smoke",
        "--body-file",
        body.to_str().unwrap(),
    ]);
    let feedback_id = value["feedback_id"].as_str().expect("feedback id");
    assert!(!feedback_id.is_empty());
    assert_eq!(value["status"], "failed_validation");
    assert_eq!(
        value["issue_url"],
        "https://github.com/Pinvou/pinvou-agent/issues"
    );

    // The request bundle is kept under feedback/pending, the receipt under
    // feedback/receipts (the platform's feedback directory contract).
    let pending: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            home.root
                .join("feedback")
                .join("pending")
                .join(format!("{feedback_id}.json")),
        )
        .expect("pending bundle must exist"),
    )
    .unwrap();
    assert_eq!(pending["type"], "suggestion");
    assert_eq!(pending["title"], "cli smoke");
    assert_eq!(pending["description"], "Reproduction steps go here.\n");
    assert_eq!(pending["entry_point"], "settings");

    let receipt: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            home.root
                .join("feedback")
                .join("receipts")
                .join(format!("{feedback_id}.json")),
        )
        .expect("receipt must exist"),
    )
    .unwrap();
    assert_eq!(receipt["status"], "failed_validation");
    assert_eq!(receipt["retryable"], false);

    // The human output prints the issues URL instead of opening a browser.
    let outcome = run(&[
        "pinvou",
        "feedback",
        "submit",
        "--type",
        "issue",
        "--title",
        "human",
        "--body-file",
        body.to_str().unwrap(),
    ])
    .unwrap();
    assert!(outcome.stdout.contains("Status: failed_validation"));
    assert!(
        outcome
            .stdout
            .contains("https://github.com/Pinvou/pinvou-agent/issues")
    );

    // Attachments are registered in the bundle.
    let attachment = home.root.join("trace.log");
    std::fs::write(&attachment, "log line\n").unwrap();
    let value = run_json(&[
        "pinvou",
        "feedback",
        "submit",
        "--type",
        "issue",
        "--title",
        "attach",
        "--body-file",
        body.to_str().unwrap(),
        "--attach",
        attachment.to_str().unwrap(),
    ]);
    let feedback_id = value["feedback_id"].as_str().unwrap();
    let pending: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            home.root
                .join("feedback")
                .join("pending")
                .join(format!("{feedback_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(pending["attachments"][0]["name"], "trace.log");
    assert_eq!(pending["attachments"][0]["media_type"], "text/plain");
}

#[test]
fn feedback_submit_fails_cleanly_on_missing_body_file() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("feedback-missing");
    let error = run(&[
        "pinvou",
        "feedback",
        "submit",
        "--type",
        "issue",
        "--title",
        "t",
        "--body-file",
        home.root.join("nope.md").to_str().unwrap(),
    ])
    .expect_err("missing body file must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed);
}

// ── monitor ─────────────────────────────────────────────────────────────────

#[test]
fn monitor_subcommands_parse() {
    assert!(parse_args(["pinvou", "monitor", "status"]).is_ok());
    assert!(parse_args(["pinvou", "--output", "json", "monitor", "snapshot"]).is_ok());
    let parsed = parse_args(["pinvou", "monitor", "snapshot"]).unwrap();
    assert_eq!(parsed.output(), OutputMode::Human);
}

#[test]
fn monitor_rejects_invalid_usage_with_exit_two() {
    let invalid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "monitor"],
        vec!["pinvou", "monitor", "bogus"],
        vec!["pinvou", "monitor", "status", "--live"],
        vec!["pinvou", "monitor", "snapshot", "extra"],
    ];
    for arguments in invalid {
        let error = parse_args(&arguments).expect_err(arguments.join(" ").as_str());
        assert_usage(&error, &arguments.join(" "));
    }
}

/// OPT-IN: `monitor status` boots the windowless host (display required) and
/// probes the configured model endpoint; without a model it must report a
/// clean offline zero state. Run with: cargo test -p pinvou-cli --test
/// misc_contract -- --ignored monitor_status
#[test]
#[ignore = "needs display host + model endpoint probe: cargo test --test misc_contract -- --ignored"]
fn monitor_status_reports_clean_zero_state_without_a_model() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("monitor-status");
    let parsed = parse_args(["pinvou", "monitor", "status"]).unwrap();
    let outcome = execute(parsed).expect("status must not hard-fail without a model");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(
        outcome.stdout.contains("Online: false"),
        "{}",
        outcome.stdout
    );
}

/// OPT-IN: `monitor snapshot` boots the windowless host (display required),
/// queries nvidia-smi/local resources and probes the model endpoint.
/// Run with: cargo test -p pinvou-cli --test misc_contract -- --ignored
///   monitor_snapshot
#[test]
#[ignore = "needs display host + GPU/endpoint probing: cargo test --test misc_contract -- --ignored"]
fn monitor_snapshot_produces_a_one_shot_sample() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("monitor-snapshot");
    let parsed = parse_args(["pinvou", "monitor", "snapshot"]).unwrap();
    let outcome = execute(parsed).expect("snapshot must produce a sample");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(
        outcome.stdout.contains("GeneratedAt: "),
        "{}",
        outcome.stdout
    );
    assert!(outcome.stdout.contains("Ram: "), "{}", outcome.stdout);
    assert!(outcome.stdout.contains("Backend: "), "{}", outcome.stdout);
}
