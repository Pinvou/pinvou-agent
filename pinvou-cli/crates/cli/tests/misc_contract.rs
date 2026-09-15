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
    assert!(outcome.stdout.contains("Kind: text"));
    assert!(outcome.stdout.contains("Tokens: "));
    let written = std::fs::read_to_string(&output_file).unwrap();
    assert!(written.contains("Heading"));
    assert!(written.contains("Body line with 内容."));

    // Plain text → stdout: the extracted text is printed after header lines.
    let outcome = run(&["pinvou", "files", "ingest", text_fixture.to_str().unwrap()])
        .expect("text ingest must succeed");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(outcome.stdout.contains("Kind: text"));
    assert!(outcome.stdout.contains("alpha\nbeta"));

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
    assert!(parse_args(["pinvou", "voice", "asr-install", "--yes"]).is_ok());
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
        // The install consent flag is accepted at most once.
        vec!["pinvou", "voice", "asr-install", "--yes", "--yes"],
    ];
    for arguments in invalid {
        let error = parse_args(&arguments).expect_err(arguments.join(" ").as_str());
        assert_usage(&error, &arguments.join(" "));
    }
}

/// The ASR env overrides (like `PINVOU3_ASR_CMD`) configure engines without
/// PATH; a hermetic voice test must clear them for the duration and restore
/// them after. Caller holds ENV_LOCK.
struct AsrEnvGuard {
    saved: Vec<(String, Option<std::ffi::OsString>)>,
}

impl AsrEnvGuard {
    const NAMES: [&str; 3] = [
        "PINVOU3_ASR_CMD",
        "PINVOU3_DEEPSPEECH2_CMD",
        "PADDLESPEECH_BIN",
    ];

    fn new() -> Self {
        let mut saved: Vec<(String, Option<std::ffi::OsString>)> = Vec::new();
        for name in Self::NAMES {
            saved.push((name.to_owned(), std::env::var_os(name)));
            // SAFETY: ENV_LOCK is held by the owning test.
            unsafe { std::env::remove_var(name) };
        }
        Self { saved }
    }
}

impl Drop for AsrEnvGuard {
    fn drop(&mut self) {
        for (name, value) in &self.saved {
            // SAFETY: ENV_LOCK is held by the owning test.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

/// Sets explicit env values for the duration of a test, restoring the
/// previous values (or absence) on drop, including during panic unwinding.
/// Caller holds ENV_LOCK.
struct EnvOverrideGuard {
    saved: Vec<(String, Option<OsString>)>,
}

impl EnvOverrideGuard {
    fn set(pairs: &[(&str, &str)]) -> Self {
        let mut saved = Vec::new();
        for (name, value) in pairs {
            saved.push(((*name).to_owned(), std::env::var_os(name)));
            // SAFETY: ENV_LOCK is held by the owning test.
            unsafe { std::env::set_var(name, value) };
        }
        Self { saved }
    }
}

impl Drop for EnvOverrideGuard {
    fn drop(&mut self) {
        for (name, value) in &self.saved {
            // SAFETY: ENV_LOCK is held by the owning test.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

/// Empties `PATH` for the duration of machine-dependent probes (ffmpeg,
/// external ASR CLIs) so the sandbox stays deterministic; restores the
/// previous value on drop, including during panic unwinding. Caller holds
/// ENV_LOCK.
struct PathGuard {
    saved: Option<OsString>,
}

impl PathGuard {
    fn empty() -> Self {
        let saved = std::env::var_os("PATH");
        // SAFETY: ENV_LOCK is held by the owning test.
        unsafe { std::env::set_var("PATH", "") };
        Self { saved }
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        // SAFETY: ENV_LOCK is held by the owning test.
        unsafe {
            match self.saved.take() {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
    }
}

#[test]
fn voice_asr_status_reports_hermetic_zero_state() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("voice-status");
    let _asr_env = AsrEnvGuard::new();
    // Empty PATH makes the machine-dependent probes deterministic: no ffmpeg
    // on PATH and no external ASR CLI anywhere. Env writes are safe here
    // because ENV_LOCK serializes all tests in this process.
    let value = {
        let _path = PathGuard::empty();
        run_json(&["pinvou", "voice", "asr-status"])
    };
    // macOS reports the host Speech runtime (present by definition, exactly
    // like the GUI status); on other platforms engine and model live under
    // `$PINVOU3_HOME`, so a fresh sandbox is a zero state.
    if cfg!(target_os = "macos") {
        assert_eq!(
            value["engine"], true,
            "macOS status mirrors the system Speech runtime"
        );
        assert_eq!(value["model"], true);
        assert_eq!(value["ready"], true);
    } else {
        assert_eq!(value["engine"], false, "fresh sandbox has no engine");
        assert_eq!(value["model"], false, "fresh sandbox has no model");
        assert_eq!(value["ready"], false);
        let missing = value["missing"].as_array().expect("missing list");
        assert!(missing.contains(&serde_json::json!("model")));
        assert!(missing.contains(&serde_json::json!("ffmpeg")));
        assert!(missing.contains(&serde_json::json!("engine")));
    }
    // Only Linux has a CLI install route; Windows reports gui_install_only
    // because its engine ships inside the desktop app's MSI.
    assert_eq!(value["installable"], cfg!(target_os = "linux"));
    assert_eq!(
        value["gui_install_only"].as_bool().unwrap_or(false),
        cfg!(target_os = "windows")
    );
    // Neither lane of the CLI itself can transcribe in this sandbox (the
    // macOS `ready` flag describes GUI capability, not CLI capability).
    assert_eq!(
        value["cli_transcribe_ready"], false,
        "no engine, model, or external ASR CLI"
    );
    assert!(
        value["asr_dir"]
            .as_str()
            .unwrap_or_default()
            .starts_with(home.root.to_str().unwrap())
    );
    // The human output mirrors the same fields.
    let outcome = {
        let _path = PathGuard::empty();
        run(&["pinvou", "voice", "asr-status"]).unwrap()
    };
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
    let _asr_env = AsrEnvGuard::new();
    // Empty PATH too: a developer with a real `pinvou-asr` on PATH must not
    // have an arbitrary external binary executed by a default test run.
    let error = {
        let _path = PathGuard::empty();
        let wav = home.root.join("capture.wav");
        // 44-byte header-only WAV: large enough to pass the empty-audio gate.
        std::fs::write(&wav, vec![0u8; 44]).unwrap();
        run(&["pinvou", "voice", "transcribe", wav.to_str().unwrap()])
            .expect_err("no ASR runtime exists in the sandbox")
    };
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

/// The ASR child is spawned as a process-group leader, so the timeout kill
/// takes its descendants with it instead of orphaning them (the same
/// contract as `pinvou connectors`'s vendor CLI spawns). The fake engine
/// backgrounds a long `sleep` and records its pid, then hangs; after the CLI
/// reports the timeout, the descendant must be dead.
#[cfg(unix)]
#[test]
fn voice_transcribe_timeout_kills_the_external_asr_process_tree() {
    use std::os::unix::fs::PermissionsExt;

    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("voice-transcribe-tree-kill");
    let _asr_env = AsrEnvGuard::new();
    let marker = home.root.join("descendant.pid");
    let engine = home.root.join("fake-asr.sh");
    std::fs::write(
        &engine,
        format!(
            "#!/bin/sh\nsleep 300 &\necho $! > {}\nsleep 600\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _overrides = EnvOverrideGuard::set(&[
        ("PINVOU3_ASR_CMD", engine.to_str().unwrap()),
        ("PINVOU3_ASR_TIMEOUT_SECS", "1"),
    ]);
    let wav = home.root.join("capture.wav");
    // 44-byte header-only WAV: large enough to pass the empty-audio gate.
    std::fs::write(&wav, vec![0u8; 44]).unwrap();

    let error = run(&["pinvou", "voice", "transcribe", wav.to_str().unwrap()])
        .expect_err("the wedged fake engine must time out");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("asr_timeout"),
        "expected the timeout code, got: {error}"
    );

    let descendant: i32 = std::fs::read_to_string(&marker)
        .expect("the fake engine records its descendant pid")
        .trim()
        .parse()
        .expect("the descendant pid is numeric");
    let mut reaped = false;
    for _ in 0..50 {
        // Safety: `kill(pid, 0)` only probes for existence; the pid came from
        // this test's own fake engine seconds ago.
        if unsafe { libc::kill(descendant, 0) } != 0 {
            reaped = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        reaped,
        "the timed-out ASR child orphaned its descendant process {descendant}"
    );
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
    assert!(
        outcome.stdout.contains("Source: llm"),
        "postprocess output missing the llm source line"
    );
}

/// OPT-IN: boots the windowless host (display required), like the test above.
/// The mock endpoint answers the first postprocess call with an unusable
/// output (empty on the Anthropic wire, `finish_reason: "length"` on the
/// OpenAI wire), which triggers the retry, and the retry call with HTTP 500.
/// GUI parity (`app/commands/voice.rs`): the failed retry must fail the
/// command (exit 1, `voice postprocess failed: …`) — the known-bad first
/// output must never be returned as the result. Run with: cargo test -p
/// pinvou-cli --test misc_contract -- --ignored voice_postprocess_retry
#[test]
#[ignore = "needs display host: cargo test --test misc_contract -- --ignored voice_postprocess_retry"]
fn voice_postprocess_retry_failure_is_an_error() {
    use std::io::{Read as _, Write as _};

    // Loopback mock model endpoint: request 1 forces the retry, request 2
    // fails it. `DEEPSEEK_*` env overrides pin the bridge to this endpoint
    // without touching the machine's settings.
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind the loopback mock endpoint");
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for round in 0..2 {
            let (mut stream, _) = listener.accept().expect("mock accepts a request");
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && head.len() < 64 * 1024 {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head);
            let path = head
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_owned();
            let response = if round == 0 {
                let body = if path.contains("/v1/messages") {
                    // Anthropic wire: empty content is the retry trigger.
                    r#"{"id":"msg_mock","model":"mock","role":"assistant","content":[{"type":"text","text":""}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#
                } else {
                    // OpenAI wire: a "length" finish marks the output truncated.
                    r#"{"choices":[{"message":{"role":"assistant","content":"partial first output"},"finish_reason":"length"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#
                };
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                     {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            } else {
                "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: \
                 close\r\n\r\n"
                    .to_owned()
            };
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    let base_url = format!("http://{address}/v1");

    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _overrides = EnvOverrideGuard::set(&[
        ("DEEPSEEK_BASE_URL", base_url.as_str()),
        ("DEEPSEEK_PROVIDER", "openai"),
        ("DEEPSEEK_API_KEY", "mock-key"),
    ]);
    let error = run(&[
        "pinvou",
        "voice",
        "postprocess",
        "--mode",
        "task",
        "--text",
        "查一下今日金价并生成数据分析",
    ])
    .expect_err("a failed postprocess retry must fail the command");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    let message = error.to_string();
    assert!(
        message.starts_with("voice postprocess failed"),
        "expected the GUI-parity failure prefix, got: {message}"
    );
    assert!(
        !message.contains("partial first output"),
        "the known-bad first output must not leak into the result: {message}"
    );
    // The server exits after its two rounds; if the host failed before even
    // the first request, the accept loop would block the join forever —
    // give the thread a bounded window and detach it (the test process is
    // short-lived; a leaked listener thread dies with it).
    for _ in 0..100 {
        if server.is_finished() {
            server.join().expect("the mock endpoint thread finishes");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// `voice asr-install` mutates the system (pkexec/apt ffmpeg install), so
/// like `deps install` it must refuse with a usage error naming `--yes`
/// before any probe, install, or download step. Compile-time Linux gate:
/// the unsupported-platform check is a runtime `cfg!` in the command, and
/// on macOS/Windows this invocation legitimately exits 1 instead.
#[cfg(target_os = "linux")]
#[test]
fn voice_asr_install_without_yes_exits_two_before_touching_the_system() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("voice-install-no-yes");
    let error = run(&["pinvou", "voice", "asr-install"])
        .expect_err("install without --yes must be refused");
    assert_usage(&error, "voice asr-install without --yes");
    assert!(
        error.to_string().contains("--yes"),
        "the refusal must name the --yes flag: {error}"
    );
}

/// Input validation must precede the ASR-availability gate: `/dev/zero`
/// reports len 0, so the old stat-then-read pair streamed it unbounded into
/// memory, and on a machine with no ASR the refusal must still be about the
/// input, not the missing engine. Unix-only because of the `/dev` path.
#[cfg(unix)]
#[test]
fn voice_transcribe_rejects_special_and_oversized_files_before_the_asr_gate() {
    use std::io::Write as _;
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("voice-transcribe-input-gate");

    let error = run(&["pinvou", "voice", "transcribe", "/dev/zero"])
        .expect_err("a character device must be refused");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("not a regular audio file"),
        "the refusal must name the regular-file rule: {error}"
    );

    // A grown regular file over the cap takes the same early rejection.
    let big =
        std::env::temp_dir().join(format!("pinvou-voice-over-cap-{}.wav", std::process::id()));
    let mut file = std::fs::File::create(&big).unwrap();
    file.write_all(&[0u8; 4 * 1024 * 1024 + 1]).unwrap();
    drop(file);
    let error = run(&["pinvou", "voice", "transcribe", big.to_str().unwrap()])
        .expect_err("an over-cap recording must be refused");
    assert!(
        error.to_string().contains("recording_too_long"),
        "the refusal must be the transcription cap: {error}"
    );
    let _ = std::fs::remove_file(&big);
}

/// OPT-IN: `voice asr-install` downloads the SenseVoice model (network) and
/// may install ffmpeg through pkexec/apt. Run with: cargo test -p pinvou-cli
/// --test misc_contract -- --ignored voice_asr_install
#[test]
#[ignore = "needs network + pkexec: cargo test --test misc_contract -- --ignored"]
fn voice_asr_install_downloads_model_with_network_and_pkexec() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("voice-install");
    // The consent gate is part of the command: the live install passes
    // --yes explicitly.
    let outcome = run(&["pinvou", "voice", "asr-install", "--yes"])
        .expect("install should run with network and policy agent available");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(outcome.stdout.contains("Model: true"));
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
    assert_eq!(lines, items.len());
    assert!(outcome.stdout.contains("voice_asr\t"));
    // macOS carries the GUI's i18n key `email_manual` as the email row hint;
    // the CLI boundary maps it to English copy instead of leaking the key.
    #[cfg(target_os = "macos")]
    assert!(
        !outcome.stdout.contains("email_manual"),
        "the raw email_manual i18n key must not reach CLI output"
    );
}

/// `deps install` refuses packages outside every platform allowlist before
/// any system mutation (the whitelist gate precedes pkexec/brew/installer
/// spawn on all platforms), so this execute-level refusal is hermetic and
/// guards the install path's failure shape.
#[test]
fn deps_install_rejects_packages_outside_the_allowlist() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("deps-install-refusal");
    let error = run(&["pinvou", "deps", "install", "cowsay", "--yes"])
        .expect_err("a package outside every allowlist must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
    assert!(error.to_string().contains("deps install failed"), "{error}");
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

#[test]
fn feedback_submit_refuses_a_body_file_over_the_read_limit() {
    // The 64 KiB read cap exists so a multi-GB file or an endless device
    // cannot be loaded into memory before the 5000-char validation runs;
    // an over-cap file must be a clean CLI failure, not an OOM.
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("feedback-oversize");
    let body = home.root.join("huge.md");
    std::fs::write(&body, vec![b'a'; 64 * 1024 + 1]).unwrap();
    let error = run(&[
        "pinvou",
        "feedback",
        "submit",
        "--type",
        "issue",
        "--title",
        "t",
        "--body-file",
        body.to_str().unwrap(),
    ])
    .expect_err("over-cap body file must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("read limit"), "{error}");
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
        "monitor status output missing the offline line"
    );
    // The zero state carries the same key set as the model-present branch,
    // with null values where there is no snapshot, so scripts parse one
    // stable shape.
    let value = run_json(&["pinvou", "monitor", "status"]);
    assert_eq!(value["vllm_online"], false);
    assert_eq!(value["health_status"], "unavailable");
    for key in [
        "max_model_len",
        "status",
        "provider",
        "model",
        "configured_model",
        "upstream",
        "target_kind",
        "diagnostic",
    ] {
        assert!(
            value.get(key).is_some_and(serde_json::Value::is_null),
            "{key} must be present and null in the zero state"
        );
    }
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
    assert!(outcome.stdout.contains("Ram: "));
    assert!(outcome.stdout.contains("Backend: "));
}
