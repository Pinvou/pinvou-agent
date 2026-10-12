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
//! Paths that need a display host (`voice postprocess`), a model endpoint,
//! a local ASR engine, GPU probing, or a system package manager are
//! `#[ignore]` opt-in tests; each names its opt-in command in a comment.
//! (`monitor status|snapshot` run on `run_bare_host` — no display — but
//! still probe a live endpoint/GPU, so they stay opt-in.)

mod common;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use common::{EnvGuard, HomeGuard, Isolation, RestoreCwd, command_error, run, run_json};
use pinvou_cli::{CliError, ExitCode, OutputMode, execute, parse_args};

// Tests that mutate the process-global `PINVOU3_HOME` environment variable
// hold [`common::env_lock`] for their whole body; the product-home preset
// ([`common::Isolation::ProductHome`]) is the throwaway `PINVOU3_HOME`
// sandbox with panic-safe restore and cleanup.

/// A scratch directory under the real `$HOME` for fixtures that must pass the
/// feature's upload-location policy (`files ingest`). Cleaned up on drop.
/// Stays local ON PURPOSE: unlike the sandbox guards it touches the
/// developer's real home directory, so it must never be folded into the
/// shared `HomeGuard` family.
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

fn assert_usage(error: &CliError, context: &str) {
    assert_eq!(
        error.exit_code(),
        ExitCode::Usage,
        "{context}: expected exit 2, got {error}"
    );
}

/// The batch form of [`assert_usage`] for the parse lane: every argv in
/// `invalid` must be refused by `parse_args` with the family's exit-2 usage
/// class, the failure naming the offending line.
fn assert_all_rejected_as_usage(invalid: Vec<Vec<&str>>) {
    for arguments in invalid {
        let error = parse_args(&arguments).expect_err(arguments.join(" ").as_str());
        assert_usage(&error, &arguments.join(" "));
    }
}

/// Stages the fake-credential fixture the credential-path refusal tests pin
/// against: creates `<home>/<component>/`, writes `content` into `file`
/// inside it, and returns the file's path. The refusals fire on the path
/// component before any content is read, so the bodies are placeholders.
fn write_credential_fixture(
    home: &std::path::Path,
    component: &str,
    file: &str,
    content: &str,
) -> PathBuf {
    let dir = home.join(component);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(file);
    std::fs::write(&path, content).unwrap();
    path
}

/// The GUI's `warning` chip and sibling i18n copy is Simplified Chinese; none
/// of it may reach CLI output in either mode.
fn assert_no_cjk_output(stdout: &str) {
    assert!(
        !stdout
            .chars()
            .any(|value| ('\u{4e00}'..='\u{9fff}').contains(&value)),
        "no CJK GUI copy may reach CLI output: {stdout}"
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
    assert_all_rejected_as_usage(invalid);
}

#[test]
fn files_ingest_round_trips_markdown_and_text_fixtures() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "files-ingest", Isolation::ProductHome);
    let scoped = ScopedHomeDir::new("files-ingest");
    let markdown_fixture = scoped.dir.join("notes.md");
    std::fs::write(&markdown_fixture, "# Heading\n\nBody line with 内容.\n").unwrap();
    let text_fixture = scoped.dir.join("plain.txt");
    std::fs::write(&text_fixture, "alpha\nbeta\n").unwrap();

    // Markdown → --output file: extracted text lands in the file, summary on
    // stdout.
    let output_file = home.path().join("extracted.md");
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
        "markdown ingest summary must carry the fixture header"
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
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "files-missing", Isolation::ProductHome);
    let scoped = ScopedHomeDir::new("files-missing");
    let missing = scoped.dir.join("does-not-exist.md");
    let error = run(&["pinvou", "files", "ingest", missing.to_str().unwrap()])
        .expect_err("missing file must fail at execute");
    assert_eq!(error.exit_code(), ExitCode::Failed);
}

/// `validate_path` requires an absolute path — free for the GUI's file dialog,
/// fatal for `pinvou files ingest report.md`. The positional resolves against
/// the cwd first, like every other path input in the crate.
#[test]
fn files_ingest_resolves_a_relative_path_against_the_current_directory() {
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "files-relative", Isolation::ProductHome);
    let scoped = ScopedHomeDir::new("files-relative");
    std::fs::write(scoped.dir.join("notes.md"), "# Relative\n").unwrap();

    // The cwd is process-global; ENV_LOCK (held for this whole test) is the
    // same gate every execute-level test in this file uses, and the parse-only
    // tests that may run concurrently name absolute paths.
    let _restore_cwd = RestoreCwd::new();
    std::env::set_current_dir(&scoped.dir).unwrap();
    let outcome = run(&["pinvou", "files", "ingest", "notes.md"])
        .expect("a relative path must resolve against the cwd");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(
        outcome.stdout.contains("File: notes.md"),
        "{}",
        outcome.stdout
    );
    assert!(outcome.stdout.contains("# Relative"), "{}", outcome.stdout);
}

/// Resolving relatives must not weaken the rest of the upload policy: the
/// `$HOME` confinement is GUI parity and stays, and the refusal has to explain
/// itself in English rather than echoing the feature's `not under $HOME`.
#[test]
fn files_ingest_refuses_a_file_outside_the_home_directory() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "files-outside-home", Isolation::ProductHome);
    // `HomeGuard` roots live under the system temp dir, which is outside the
    // real `$HOME` on unix — but on Windows `%TEMP%` normally sits under
    // `%USERPROFILE%`, which would turn the fixture into a false negative.
    // This test pins the CLI's refusal wording, not the platform predicate,
    // so skip when the temp root is home-confined on whatever platform.
    let temp = std::env::temp_dir();
    let real_home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    if let Some(home) = real_home
        && temp.starts_with(std::path::PathBuf::from(home))
    {
        eprintln!("skipping: the system temp dir lives under the real home on this machine");
        return;
    }
    let outside = home.path().join("outside.md");
    std::fs::write(&outside, "# Outside\n").unwrap();

    let error = run(&["pinvou", "files", "ingest", outside.to_str().unwrap()])
        .expect_err("a file outside $HOME must be refused");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    let message = error.to_string();
    assert!(
        message.contains("must live under your home directory"),
        "the refusal must explain the constraint: {message}"
    );
    assert!(
        !message.contains("not under $HOME"),
        "the raw feature wording must not reach CLI output: {message}"
    );
}

/// Round-28 review: the credential-component arm of the ingest refusal had
/// no contract test — if upstream rewords `crosses sensitive component`,
/// the English translation silently degrades to the raw feature string
/// (the same regression class round-27 pinned for feedback's body-file
/// gate). The `$HOME` leg passes because the fixture lives under the
/// scoped home directory.
#[test]
fn files_ingest_translates_the_credential_component_refusal() {
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "files-credential", Isolation::ProductHome);
    let scoped = ScopedHomeDir::new("files-credential");
    let key = write_credential_fixture(&scoped.dir, ".ssh", "leaked.md", "# pretend notes\n");

    let error = run(&["pinvou", "files", "ingest", key.to_str().unwrap()])
        .expect_err("a credential path must be refused");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    let message = error.to_string();
    assert!(
        message.contains("crosses the credential path component"),
        "the credential arm must translate: {message}"
    );
    assert!(
        !message.contains(" crosses sensitive component "),
        "the raw feature wording must not reach CLI output: {message}"
    );
}

/// The `warning` chip is GUI i18n copy. Printing it verbatim put Chinese into
/// an English tool; the boundary translator turns the known cases into English
/// in both output modes.
#[test]
fn files_ingest_translates_the_warning_chip_to_english() {
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "files-warning", Isolation::ProductHome);
    let scoped = ScopedHomeDir::new("files-warning");
    // Unknown extension + a NUL byte: the feature's content sniffer classifies
    // this as binary and returns a warning placeholder. No external tool is
    // involved, so the assertion is hermetic on every platform. The length is
    // odd on purpose — the sniffer checks for BOM-less UTF-16 first, and that
    // detector only considers even-length buffers, so an odd one cannot be
    // mistaken for wide text.
    let blob = scoped.dir.join("opaque.bin");
    std::fs::write(&blob, [0x89u8, 0x50, 0x4e, 0x47, 0x00, 0x1a, 0x0a]).unwrap();

    let outcome = run(&["pinvou", "files", "ingest", blob.to_str().unwrap()])
        .expect("a binary placeholder is still a successful ingest");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(
        outcome.stdout.contains("Warning: unsupported file type"),
        "{}",
        outcome.stdout
    );
    assert_no_cjk_output(&outcome.stdout);

    let value = run_json(&["pinvou", "files", "ingest", blob.to_str().unwrap()]);
    assert_eq!(
        value["warning"],
        serde_json::json!("unsupported file type (binary)")
    );
}

/// Round-44 review: the content hard-wall's SECRET-EXTENSION lane (a `.key`
/// classified to `redacted_secret_placeholder` — the layer that exists
/// precisely because the path policy cannot stop `~/certs/server.key`) had
/// no test on either surface, so a dropped translation-table entry would
/// silently degrade to raw Chinese on this lane. Both channels are pinned:
/// the English chip in human output and the `markdown: null` +
/// secret-placeholder warning shape in JSON.
#[test]
fn files_ingest_redacts_a_secret_extension_with_the_english_chip() {
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "files-secret-ext", Isolation::ProductHome);
    let scoped = ScopedHomeDir::new("files-secret-ext");
    let key = scoped.dir.join("server.key");
    // PEM-ish body: the class the extension classifier exists to keep out
    // of the ledger.
    std::fs::write(
        &key,
        "-----BEGIN PRIVATE KEY-----\nMIIEvQ\n-----END PRIVATE KEY-----\n",
    )
    .unwrap();

    let outcome = run(&["pinvou", "files", "ingest", key.to_str().unwrap()])
        .expect("a redacted secret placeholder is still a successful ingest");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(
        outcome.stdout.contains("private-key file"),
        "the human chip must name the key/private-key class: {}",
        outcome.stdout
    );
    assert_no_cjk_output(&outcome.stdout);
    assert!(
        !outcome.stdout.contains("BEGIN PRIVATE KEY"),
        "the secret body must never surface: {}",
        outcome.stdout
    );

    let value = run_json(&["pinvou", "files", "ingest", key.to_str().unwrap()]);
    assert!(
        value
            .get("markdown")
            .is_none_or(|markdown| markdown.is_null()),
        "a secret placeholder must not carry markdown: {value}"
    );
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
    assert!(
        parse_args([
            "pinvou",
            "voice",
            "postprocess",
            "--mode",
            "task",
            "--text-file",
            "/tmp/t.txt"
        ])
        .is_ok()
    );
    // The draft is what the edit prompt rewrites (the GUI's `draft_text`), so
    // `--mode edit` only parses with one; both spellings are accepted, and
    // the other modes may carry one too (the GUI sends it on every lane).
    for draft in [["--draft", "existing body"], ["--draft-file", "/tmp/d.txt"]] {
        assert!(
            parse_args([
                "pinvou",
                "voice",
                "postprocess",
                "--mode",
                "edit",
                "--text-file",
                "/tmp/t.txt",
                draft[0],
                draft[1]
            ])
            .is_ok()
        );
        assert!(
            parse_args([
                "pinvou",
                "voice",
                "postprocess",
                "--mode",
                "dictation",
                "--text",
                "hello",
                draft[0],
                draft[1]
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
        // `edit` rewrites the input-box draft; without one the shipped prompt
        // would describe a DRAFT_TEXT section that is never transmitted.
        vec![
            "pinvou",
            "voice",
            "postprocess",
            "--mode",
            "edit",
            "--text",
            "make it three bullets",
        ],
        // At most one draft source, and each needs a value.
        vec![
            "pinvou",
            "voice",
            "postprocess",
            "--mode",
            "edit",
            "--text",
            "a",
            "--draft",
            "body",
            "--draft-file",
            "/tmp/d",
        ],
        vec![
            "pinvou",
            "voice",
            "postprocess",
            "--mode",
            "edit",
            "--text",
            "a",
            "--draft",
        ],
        vec![
            "pinvou",
            "voice",
            "postprocess",
            "--mode",
            "edit",
            "--text",
            "a",
            "--draft-file",
        ],
        vec!["pinvou", "voice", "asr-status", "--extra"],
        vec!["pinvou", "voice", "asr-install", "--extra"],
        // The install consent flag is accepted at most once.
        vec!["pinvou", "voice", "asr-install", "--yes", "--yes"],
    ];
    assert_all_rejected_as_usage(invalid);
}

#[test]
fn voice_transcribe_refuses_option_shaped_positional_at_parse() {
    // A truncated flag pair like `voice transcribe --text` must die in the
    // parse layer with the family's exit-2 usage class, naming the token —
    // not parse as a PATH and die at execute with exit 1 ("cannot read
    // --text"), the same argv-decidable posture as `files ingest`. Because
    // parse_args rejects it, execute is never reached and nothing runs.
    // (A bare trailing `--output` never reaches this family: the dispatch
    // spine answers it with the global incomplete-pair usage error at
    // lib.rs, so the family-level pin uses tokens the family really sees.)
    for token in ["-o", "--text", "--output-file"] {
        let arguments = ["pinvou", "voice", "transcribe", token];
        let error = parse_args(arguments).expect_err(token);
        assert_usage(&error, token);
        assert!(
            error
                .to_string()
                .contains(&format!("option-like token {token:?}")),
            "the usage error must name the offending token {token:?}: {error}"
        );
    }
}

/// The ASR env overrides (like `PINVOU3_ASR_CMD`) configure engines without
/// PATH; a hermetic voice test must clear them for the duration and restore
/// them after (the common [`EnvGuard`] both sets and removes with panic-safe
/// restore). Caller holds ENV_LOCK.
const ASR_ENV_NAMES: [&str; 3] = [
    "PINVOU3_ASR_CMD",
    "PINVOU3_DEEPSPEECH2_CMD",
    "PADDLESPEECH_BIN",
];

#[test]
fn voice_asr_status_reports_hermetic_zero_state() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "voice-status", Isolation::ProductHome);
    let _asr_env = EnvGuard::remove(&ASR_ENV_NAMES);
    // Empty PATH makes the machine-dependent probes deterministic: no ffmpeg
    // on PATH and no external ASR CLI anywhere. Env writes are safe here
    // because ENV_LOCK serializes all tests in this process.
    let value = {
        let _path = EnvGuard::set(&[("PATH", Some(""))]);
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
    assert!(
        value["gui_install_only"].is_boolean(),
        "gui_install_only must be present (round-37: unwrap_or(false) made a missing \
         field indistinguishable from false on non-Windows legs)"
    );
    assert_eq!(
        value["gui_install_only"].as_bool().unwrap(),
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
            .starts_with(home.path().to_str().unwrap())
    );
    // The human output mirrors the same fields.
    let outcome = {
        let _path = EnvGuard::set(&[("PATH", Some(""))]);
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
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "voice-transcribe", Isolation::ProductHome);
    let _asr_env = EnvGuard::remove(&ASR_ENV_NAMES);
    // Empty PATH too: a developer with a real `pinvou-asr` on PATH must not
    // have an arbitrary external binary executed by a default test run.
    let error = {
        let _path = EnvGuard::set(&[("PATH", Some(""))]);
        let wav = stage_wav(home.path());
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
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "voice-transcribe-live", Isolation::ProductHome);
    let wav = home.path().join("capture.wav");
    std::fs::write(&wav, vec![0u8; 8000]).unwrap();
    let outcome = run(&["pinvou", "voice", "transcribe", wav.to_str().unwrap()])
        .expect("transcription should run with an installed ASR runtime");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(outcome.stdout.contains("Text: "));
}

/// Writes a 44-byte header-only `capture.wav` under `home` — no audio
/// frames, but large enough to pass the empty-audio gate — and returns its
/// path.
fn stage_wav(home: &std::path::Path) -> PathBuf {
    let wav = home.join("capture.wav");
    std::fs::write(&wav, vec![0u8; 44]).unwrap();
    wav
}

/// Writes and chmods +x a fake ASR engine script under `home` and returns its
/// path; the process-tree tests point `PINVOU3_ASR_CMD` at it.
#[cfg(unix)]
fn stage_fake_asr(home: &std::path::Path, script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let engine = home.join("fake-asr.sh");
    std::fs::write(&engine, script).unwrap();
    std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
    engine
}

/// Reads back the descendant pid the fake engine recorded in `marker`.
#[cfg(unix)]
fn descendant_pid(marker: &std::path::Path) -> i32 {
    std::fs::read_to_string(marker)
        .expect("the fake engine records its descendant pid")
        .trim()
        .parse()
        .expect("the descendant pid is numeric")
}

/// The ASR child is spawned as a process-group leader, so the timeout kill
/// takes its descendants with it instead of orphaning them (the same
/// contract as `pinvou connectors`'s vendor CLI spawns). The fake engine
/// backgrounds a long `sleep` and records its pid, then hangs; after the CLI
/// reports the timeout, the descendant must be dead.
#[cfg(unix)]
#[test]
fn voice_transcribe_timeout_kills_the_external_asr_process_tree() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "voice-transcribe-tree-kill", Isolation::ProductHome);
    let _asr_env = EnvGuard::remove(&ASR_ENV_NAMES);
    let marker = home.path().join("descendant.pid");
    let engine = stage_fake_asr(
        home.path(),
        &format!(
            "#!/bin/sh\nsleep 300 &\necho $! > {}\nsleep 600\n",
            marker.display()
        ),
    );
    let _overrides = EnvGuard::set(&[
        ("PINVOU3_ASR_CMD", Some(engine.to_str().unwrap())),
        ("PINVOU3_ASR_TIMEOUT_SECS", Some("1")),
    ]);
    let wav = stage_wav(home.path());

    let error = run(&["pinvou", "voice", "transcribe", wav.to_str().unwrap()])
        .expect_err("the wedged fake engine must time out");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("asr_timeout"),
        "expected the timeout code, got: {error}"
    );

    let descendant = descendant_pid(&marker);
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

/// A child the wait loop already reaped must never have its process group
/// killed: the pid is free for reuse the moment it is reaped, so `kill(-pgid)`
/// could SIGKILL an unrelated group with the user's privileges. Exit code 6 is
/// the documented "no speech" answer of the external ASR CLI, so this fires on
/// an ordinary user action. The fake engine leaves a live descendant in its own
/// process group before exiting 6; that descendant is the observable proof —
/// a group kill after the reap would take it down with it.
#[cfg(unix)]
#[test]
fn voice_transcribe_no_speech_exit_does_not_kill_the_reaped_process_group() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "voice-transcribe-no-speech", Isolation::ProductHome);
    let _asr_env = EnvGuard::remove(&ASR_ENV_NAMES);
    let marker = home.path().join("descendant.pid");
    // The descendant's own pipes go to /dev/null so it cannot hold the CLI's
    // capture pipes open past the exit; only the process-group membership
    // matters for this test.
    let engine = stage_fake_asr(
        home.path(),
        &format!(
            "#!/bin/sh\nsleep 300 >/dev/null 2>&1 &\necho $! > {}\nexit 6\n",
            marker.display()
        ),
    );
    let _overrides = EnvGuard::set(&[("PINVOU3_ASR_CMD", Some(engine.to_str().unwrap()))]);
    let wav = stage_wav(home.path());

    let error = run(&["pinvou", "voice", "transcribe", wav.to_str().unwrap()])
        .expect_err("exit code 6 is the no-speech failure");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().starts_with("asr_no_speech"),
        "expected the no-speech code, got: {error}"
    );

    let descendant = descendant_pid(&marker);
    // Safety: `kill(pid, 0)` only probes for existence; the pid came from this
    // test's own fake engine seconds ago.
    let alive = unsafe { libc::kill(descendant, 0) } == 0;
    if alive {
        // Safety: the descendant is verifiably still this test's own `sleep`,
        // so the sandbox leaves nothing running behind. A dead pid is left
        // alone rather than signalled — that is the very reuse hazard under
        // test here.
        unsafe { libc::kill(descendant, libc::SIGKILL) };
    }
    assert!(
        alive,
        "the reaped child's process group was killed: descendant {descendant} is gone"
    );
}

/// The edit prompt rewrites the input-box draft, so a draft that resolves to
/// nothing is refused before the windowless host is booted — the parser only
/// sees that an option was passed, not that its file was blank. Hermetic:
/// the refusal precedes every host/model step.
#[test]
fn voice_postprocess_edit_refuses_a_blank_draft_before_booting_the_host() {
    let _env = common::env_lock();
    let home = HomeGuard::new(
        "misc",
        "voice-postprocess-blank-draft",
        Isolation::ProductHome,
    );
    let blank = home.path().join("draft.txt");
    std::fs::write(&blank, "  \n\t\n").unwrap();
    for draft in [
        vec!["--draft", "   "],
        vec!["--draft-file", blank.to_str().unwrap()],
    ] {
        let mut arguments = vec![
            "pinvou",
            "voice",
            "postprocess",
            "--mode",
            "edit",
            "--text",
            "把它改成三条要点",
        ];
        arguments.extend(draft.iter().copied());
        let error = run(&arguments).expect_err("a blank draft must be refused");
        assert_usage(&error, &arguments.join(" "));
        assert!(
            error.to_string().contains("draft"),
            "the refusal must name the draft: {error}"
        );
    }
}

/// `--draft-file` gets the same input hygiene as `--text-file`: regular files
/// only, so a character device or FIFO cannot stream into the prompt. Unix
/// only because of the `/dev` path.
#[cfg(unix)]
#[test]
fn voice_postprocess_draft_file_refuses_a_special_file() {
    let _env = common::env_lock();
    let _home = HomeGuard::new(
        "misc",
        "voice-postprocess-draft-special",
        Isolation::ProductHome,
    );
    let error = run(&[
        "pinvou",
        "voice",
        "postprocess",
        "--mode",
        "edit",
        "--text",
        "把它改成三条要点",
        "--draft-file",
        "/dev/zero",
    ])
    .expect_err("a character device must be refused");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("not a regular file"),
        "the refusal must name the regular-file rule: {error}"
    );
}

/// Pure-noise input short-circuits without touching the model (GUI parity),
/// and every postprocess result names the GUI pipeline stages the CLI does
/// NOT run — the deterministic rule corrections and the shrink/protected-term
/// validator — so the difference is disclosed instead of silent: `omitted_stages`
/// in JSON and the trailing `Note:` line in human output, on the model path
/// AND on the empty short-circuit alike. Hermetic: the empty short-circuit
/// precedes the host boot.
#[test]
fn voice_postprocess_empty_input_reports_the_omitted_pipeline_stages() {
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "voice-postprocess-empty", Isolation::ProductHome);
    let arguments = [
        "pinvou",
        "voice",
        "postprocess",
        "--mode",
        "edit",
        "--text",
        "   ",
        "--draft",
        "整理会议纪要。",
    ];
    let value = run_json(&arguments);
    assert_eq!(value["source"], "empty");
    assert_eq!(value["text"], "");
    let omitted = value["omitted_stages"]
        .as_array()
        .expect("every postprocess result discloses the omitted GUI stages");
    assert!(omitted.contains(&serde_json::json!("deterministic-rule-corrections")));
    assert!(omitted.contains(&serde_json::json!("shrink-and-protected-term-validation")));
    // The human output carries the same disclosure as its trailing Note line
    // — the module header promises it on EVERY postprocess result, the
    // empty short-circuit included.
    let outcome = run(&arguments).expect("the empty short-circuit must succeed");
    assert!(
        outcome
            .stdout
            .contains("Note: the model output is returned as written"),
        "the human output must carry the trailing Note disclosure: {}",
        outcome.stdout
    );
    // The human line set is branch-independent: the model path always prints
    // `Truncated:`, so the empty short-circuit prints it as `false` instead
    // of silently dropping the line.
    assert!(
        outcome.stdout.contains("Truncated: false"),
        "the empty short-circuit must keep the human schema stable: {}",
        outcome.stdout
    );
}

/// Round-40 review MAJOR: `voice postprocess --text-file`/`--draft-file`
/// ship their content to the configured model endpoint, so they carry the
/// same credential-path refusal as `files ingest`/`feedback --body-file`.
/// The refusal fires BEFORE the windowless host boots, so this is hermetic:
/// a file under a credential component is refused with the artifact-crosses
/// wording and no model call can have happened (there is no display here —
/// a boot would fail the test differently).
#[test]
fn voice_postprocess_refuses_credential_path_files() {
    // Round-48 review: `HomeGuard` mutates process-global env, so this test
    // must hold ENV_LOCK like every other HomeGuard user in this file.
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "voice-postprocess-gate", Isolation::ProductHome);
    let secret = write_credential_fixture(_home.path(), ".ssh", "id_rsa.txt", "PRIVATE KEY")
        .to_str()
        .unwrap()
        .to_owned();

    for lane in [
        vec![
            "pinvou",
            "voice",
            "postprocess",
            "--mode",
            "task",
            "--text-file",
            &secret,
        ],
        vec![
            "pinvou",
            "voice",
            "postprocess",
            "--mode",
            "edit",
            "--text",
            "polish this",
            "--draft-file",
            &secret,
        ],
    ] {
        let error = command_error(&lane);
        let rendered = error.to_string();
        assert!(
            rendered.contains("crosses the credential path component"),
            "the refusal must name the credential-path rule: {rendered}"
        );
    }
}

/// OPT-IN: `voice postprocess` boots the windowless host (display required)
/// and calls the configured model endpoint. Run with: cargo test -p
/// pinvou-cli --test misc_contract -- --ignored voice_postprocess
#[test]
#[ignore = "needs display host + configured model: cargo test --test misc_contract -- --ignored"]
fn voice_postprocess_calls_the_active_model() {
    // Round-37 review: sandbox the home — a windowless-host boot runs the
    // retention sweep against whatever PINVOU3_HOME names.
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "ignored-voice-postprocess", Isolation::ProductHome);
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
/// output must never be returned as the result.
///
/// The mock records both request bodies, because the failure prefix alone
/// proves nothing: `voice postprocess failed: timeout budget exhausted before
/// retry` matches it just as well, and that is exactly what a budget measured
/// across the windowless-host boot produced — a structurally dead retry that
/// this test could not see. Asserting that a SECOND request arrived, and that
/// it carried the retry system prompt, is what pins the fix. Run with: cargo
/// test -p pinvou-cli --test misc_contract -- --ignored voice_postprocess_retry
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
    let bodies: std::sync::Arc<Mutex<Vec<String>>> =
        std::sync::Arc::new(Mutex::new(Vec::with_capacity(2)));
    let recorded = std::sync::Arc::clone(&bodies);
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
            let head = String::from_utf8_lossy(&head).into_owned();
            let path = head
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_owned();
            // Read the JSON body too: which prompt each attempt carried is
            // the observable difference between a real retry and a first
            // attempt that merely failed twice as loudly.
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.trim().eq_ignore_ascii_case("content-length") {
                        value.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            let mut body = vec![0u8; length];
            if length > 0 && stream.read_exact(&mut body).is_err() {
                body.clear();
            }
            recorded
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(String::from_utf8_lossy(&body).into_owned());
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

    let _env = common::env_lock();
    let _overrides = EnvGuard::set(&[
        ("DEEPSEEK_BASE_URL", Some(base_url.as_str())),
        ("DEEPSEEK_PROVIDER", Some("openai")),
        ("DEEPSEEK_API_KEY", Some("mock-key")),
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
    // The failure must be the retry's HTTP 500, not a starved budget: a clock
    // started before `run_windowless_host` spends the whole 8 s task budget on
    // the boot and reports this instead, with no second request ever issued.
    assert!(
        !message.contains("budget exhausted"),
        "the model-call clock must start after the host is up: {message}"
    );
    // The server exits after its two rounds; if the host failed before even
    // the first request, the accept loop would block the join forever —
    // give the thread a bounded window and detach it (the test process is
    // short-lived; a leaked listener thread dies with it).
    for _ in 0..100 {
        if server.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let bodies = bodies.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        bodies.len(),
        2,
        "the retry must actually be issued; the endpoint saw {} request(s)",
        bodies.len()
    );
    assert!(
        bodies[0].contains("你是 Pinvou 的语音任务纠错器"),
        "the first attempt must carry the task system prompt: {}",
        bodies[0]
    );
    // "禁止空输出" appears only in `postprocess_retry_prompt`, never in the
    // first-attempt system prompt.
    assert!(
        bodies[1].contains("禁止空输出"),
        "the second attempt must carry the retry prompt: {}",
        bodies[1]
    );
}

/// `voice asr-install` mutates the system (pkexec/apt ffmpeg install), so
/// like `deps install` it must refuse with a usage error naming `--yes`
/// before any probe, install, or download step. Compile-time Linux gate:
/// the unsupported-platform check is a runtime `cfg!` in the command, and
/// on macOS/Windows this invocation legitimately exits 1 instead.
#[cfg(target_os = "linux")]
#[test]
fn voice_asr_install_without_yes_exits_two_before_touching_the_system() {
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "voice-install-no-yes", Isolation::ProductHome);
    let error = run(&["pinvou", "voice", "asr-install"])
        .expect_err("install without --yes must be refused");
    assert_usage(&error, "voice asr-install without --yes");
    assert!(
        error.to_string().contains("--yes"),
        "the refusal must name the --yes flag: {error}"
    );
}

/// Two concurrent `voice asr-install --yes` processes would both
/// `File::create` the same `.part` file and interleave their writes; the
/// GUI's equivalent guard (`voice_asr::begin_asr_install` swapping
/// `ASR_INSTALLING`) is process-local and cannot see a second CLI. The
/// cross-process fd-lock must refuse the later entrant with a stable busy
/// code, before any ffmpeg probe, pkexec install or model download runs — so
/// this test is hermetic even though the command it drives is not. Linux-only
/// because elsewhere the platform gate fails the command first.
#[cfg(target_os = "linux")]
#[test]
fn voice_asr_install_refuses_while_another_process_holds_the_install_lock() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "voice-install-lock", Isolation::ProductHome);
    // Hold the advisory lock a second CLI process would hold.
    let lock_path = home.path().join("locks").join("voice-asr-install.lock");
    std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .unwrap();
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock.try_write().expect("test acquires the contended lock");

    let error = run(&["pinvou", "voice", "asr-install", "--yes"])
        .expect_err("a contended install lock must refuse the second entrant");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().starts_with("asr_install_busy"),
        "expected the stable busy code, got: {error}"
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
    let _env = common::env_lock();
    let _home = HomeGuard::new(
        "misc",
        "voice-transcribe-input-gate",
        Isolation::ProductHome,
    );

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
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "voice-install", Isolation::ProductHome);
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
        // The single-dash consent form is not a package name either: the
        // parse layer refuses any option-shaped token with the usage class
        // (round-48), so pin the `-y` arm next to the `--`-arm above.
        vec!["pinvou", "deps", "install", "ffmpeg", "-y"],
    ];
    assert_all_rejected_as_usage(invalid);
}

#[test]
fn deps_install_without_yes_exits_two_before_touching_the_system() {
    // require_yes runs before any package-manager interaction, so this is a
    // hermetic usage-error check.
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "deps-no-yes", Isolation::ProductHome);
    let error = run(&["pinvou", "deps", "install", "ffmpeg"])
        .expect_err("install without --yes must be refused");
    assert_usage(&error, "deps install without --yes");
}

#[test]
fn deps_check_reports_the_platform_capability_table() {
    // check_dependencies is pure local detection; the sandbox keeps any state
    // reads off the developer's real home. Installed flags are
    // machine-dependent, so assertions target structure, not values.
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "deps-check", Isolation::ProductHome);
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
    // the CLI boundary maps it to English copy instead of leaking the key —
    // on EVERY platform (the mapping is not cfg-gated), so the assertion is
    // not either (round-37: a macOS-gated assert executed nowhere — CI's
    // cli-test leg is Linux, macos-cli-check only compiles).
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
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "deps-install-refusal", Isolation::ProductHome);
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
    assert_all_rejected_as_usage(invalid);
}

/// A concluded submission leaves exactly one file: the receipt, carrying the
/// request it answers. The bundle used to stay under `feedback/pending/`
/// forever — the community `submit_feedback` never uploads and no retry lane
/// exists — so a directory documented for transient packages accumulated
/// permanent copies of the user's free text.
#[test]
fn feedback_submit_keeps_only_a_receipt_after_a_concluded_submission() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "feedback-submit", Isolation::ProductHome);
    let body = home.path().join("body.md");
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
    // `status` is human copy and is always `failed_validation` on this
    // (successful, exit-0) path, so it can carry no script signal at all.
    // `uploaded` is the field a guard can branch on.
    assert_eq!(value["uploaded"], serde_json::json!(false));
    assert_eq!(
        value["issue_url"],
        "https://github.com/Pinvou/pinvou-agent/issues"
    );
    // No `--attach`: the key is still present, so a consumer never has to
    // distinguish "absent" from "none".
    assert_eq!(value["attachments"], serde_json::json!([]));

    // Nothing is left staged: the run concluded, so no package is pending.
    let pending_dir = home.path().join("feedback").join("pending");
    assert!(
        !pending_dir
            .join(format!("{feedback_id}.json"))
            .try_exists()
            .unwrap(),
        "a concluded submission must not leave a pending bundle"
    );
    if pending_dir.exists() {
        let left: Vec<_> = std::fs::read_dir(&pending_dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert!(left.is_empty(), "feedback/pending must be empty: {left:?}");
    }

    // The receipt is the record, and it carries the request it answers so the
    // user still has their text to paste into the issue tracker.
    let receipt_path = home
        .path()
        .join("feedback")
        .join("receipts")
        .join(format!("{feedback_id}.json"));
    let receipt: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&receipt_path).expect("receipt must exist"))
            .unwrap();
    assert_eq!(receipt["status"], "failed_validation");
    assert_eq!(receipt["retryable"], false);
    assert_eq!(receipt["feedback_id"], feedback_id);
    assert_eq!(receipt["request"]["type"], "suggestion");
    assert_eq!(receipt["request"]["title"], "cli smoke");
    assert_eq!(
        receipt["request"]["description"],
        "Reproduction steps go here.\n"
    );
    assert_eq!(receipt["request"]["entry_point"], "settings");
    // The JSON no longer advertises a path that was just removed.
    assert!(value.get("pending_path").is_none(), "{value}");
    assert_eq!(value["receipt_path"], receipt_path.display().to_string());

    // Free text the user wrote is not world-readable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&receipt_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "receipt mode must be 0600, got {mode:o}");
    }

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
    assert!(outcome.stdout.contains("Uploaded: false"));
    assert!(
        outcome
            .stdout
            .contains("https://github.com/Pinvou/pinvou-agent/issues")
    );
    assert!(
        !outcome.stdout.contains("Pending:"),
        "human mode must not point at a staged file that is gone: {}",
        outcome.stdout
    );

    // Attachments are registered in the request the receipt embeds.
    let attachment = home.path().join("trace.log");
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
    let receipt: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            home.path()
                .join("feedback")
                .join("receipts")
                .join(format!("{feedback_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["request"]["attachments"][0]["name"], "trace.log");
    assert_eq!(
        receipt["request"]["attachments"][0]["media_type"],
        "text/plain"
    );
    // …and echoed back, so `--attach` is visibly not a no-op.
    assert_eq!(value["attachments"][0]["name"], "trace.log");
    assert_eq!(value["attachments"][0]["media_type"], "text/plain");
    assert_eq!(value["attachments"][0]["size_bytes"], "log line\n".len());
    assert_eq!(
        value["attachments"][0]["path"],
        attachment.display().to_string()
    );
    let outcome = run(&[
        "pinvou",
        "feedback",
        "submit",
        "--type",
        "issue",
        "--title",
        "attach human",
        "--body-file",
        body.to_str().unwrap(),
        "--attach",
        attachment.to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        outcome
            .stdout
            .contains("Attachment: trace.log (text/plain,"),
        "{}",
        outcome.stdout
    );
}

/// Nothing is read or uploaded from an attachment, but its path is written
/// into a receipt that lives forever — and the GUI picker refuses credential
/// locations outright. The CLI must not be the way around that rule.
///
/// Round-27 review: the `--body-file` credential gate (the same
/// `check_sensitive_path` rule `--attach` applies, per the disclosure and
/// docs) had no contract test — deleting the gate passed the whole suite.
/// This pins it on its own input.
#[test]
fn feedback_submit_refuses_a_credential_path_on_attach_and_body_file() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "feedback-credential-gates", Isolation::ProductHome);
    let body = home.path().join("body.md");
    std::fs::write(&body, "Reproduction steps go here.\n").unwrap();
    let key = write_credential_fixture(home.path(), ".ssh", "id_rsa", "PRIVATE KEY");

    for (tail, needle) in [
        (
            vec![
                "--body-file",
                body.to_str().unwrap(),
                "--attach",
                key.to_str().unwrap(),
            ],
            "refusing attachment",
        ),
        (
            vec!["--body-file", key.to_str().unwrap()],
            "refusing body file",
        ),
    ] {
        let mut arguments = vec![
            "pinvou", "feedback", "submit", "--type", "issue", "--title", "secret",
        ];
        arguments.extend(tail);
        let error = run(&arguments).expect_err("a credential path must be refused");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        assert!(
            error.to_string().contains(needle),
            "the refusal must name the gated input ({needle}): {error}"
        );
        // The refusal happens before anything is staged.
        let pending_dir = home.path().join("feedback").join("pending");
        assert!(!pending_dir.exists(), "nothing may be staged: {error}");
    }
}

/// Round-37 review: the migration note tells users `agent run --prompt-file`
/// is "a regular file of at most 4 MiB", but the shared helper's caps were
/// pinned only through the feedback/voice lanes — nothing failed if this
/// lane's delegation reverted to `read_to_string` or swapped the cap. The
/// refusal must land pre-boot like the attachment refusals above.
#[cfg(feature = "product-backend")]
#[test]
fn agent_run_refuses_an_oversized_prompt_file_before_any_host_boot() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "agent-prompt-cap", Isolation::ProductHome);
    let prompt = home.path().join("big-prompt.txt");
    std::fs::write(&prompt, vec![b'x'; 4 * 1024 * 1024 + 1]).unwrap();

    let error = run(&[
        "pinvou",
        "agent",
        "run",
        "--prompt-file",
        prompt.to_str().unwrap(),
    ])
    .expect_err("an over-cap prompt file must be refused before any host boot");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("4194304-byte read limit"),
        "the cap is the documented 4 MiB: {error}"
    );
}

/// Round-28 review: the `agent run --attach` credential gate (the same
/// `check_sensitive_path` rule `feedback --attach` applies) had no contract
/// test — the identical gap round-27 closed for `feedback --body-file`.
/// Deleting the gate in `agent_task.rs` passes the whole suite without this.
/// The gate runs after the prompt read and before any host boot, so the
/// refusal is hermetic.
// Round-38 review: the round-37 cap test's `#[test]` had been stacked onto
// this doc comment, leaving this function attribute-less and never run —
// the exact regression the round-28 note describes was uncaught again.
// Round-39 review: the same gating discipline as the personas seam tests —
// both agent refusal legs live behind the product-backend stub, so the
// featureless `cargo test` run failed here instead of exercising them.
#[cfg(feature = "product-backend")]
#[test]
fn agent_run_refuses_an_attachment_under_a_credential_path() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "agent-attach-secret", Isolation::ProductHome);
    let key = write_credential_fixture(home.path(), ".ssh", "id_rsa", "PRIVATE KEY");
    let prompt = home.path().join("prompt.txt");
    std::fs::write(&prompt, "summarize the report").unwrap();

    let error = run(&[
        "pinvou",
        "agent",
        "run",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--attach",
        key.to_str().unwrap(),
    ])
    .expect_err("a credential attachment must be refused before any host boot");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("refusing attachment"), "{error}");
}

/// Round-46 review: `--prompt-file` is model context exactly like `--attach`
/// is, and it was the one model-context file lane without the
/// credential-path gate — a scripted `--prompt-file ~/.aws/credentials`
/// would ship the secret verbatim to the model endpoint and into the
/// session transcript. The gate runs pre-boot (before the canonicalize it
/// performs, nothing else touches the store), so the refusal is hermetic.
#[cfg(feature = "product-backend")]
#[test]
fn agent_run_refuses_a_prompt_file_under_a_credential_path() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "agent-promptfile-secret", Isolation::ProductHome);
    let credentials = write_credential_fixture(
        home.path(),
        ".aws",
        "credentials",
        "[default]\naws_access_key_id = AKIAFAKE",
    );

    let error = run(&[
        "pinvou",
        "agent",
        "run",
        "--prompt-file",
        credentials.to_str().unwrap(),
    ])
    .expect_err("a credential prompt file must be refused before any host boot");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("refusing prompt file"),
        "{error}"
    );
}

/// The receipt write used to `?` out before the cleanup below it, leaving the
/// staged bundle under `feedback/pending/` forever — the exact state the
/// module docs argue must never exist, since nothing retries it.
#[test]
fn feedback_submit_drops_the_staged_bundle_when_the_receipt_cannot_be_written() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "feedback-receipt-fail", Isolation::ProductHome);
    let body = home.path().join("body.md");
    std::fs::write(&body, "Reproduction steps go here.\n").unwrap();
    // A regular file where the receipts directory belongs: `create_dir_all`
    // fails, so the receipt cannot land.
    let feedback_dir = home.path().join("feedback");
    std::fs::create_dir_all(&feedback_dir).unwrap();
    std::fs::write(feedback_dir.join("receipts"), "not a directory").unwrap();

    let error = run(&[
        "pinvou",
        "feedback",
        "submit",
        "--type",
        "issue",
        "--title",
        "receipt failure",
        "--body-file",
        body.to_str().unwrap(),
    ])
    .expect_err("an unwritable receipt must fail the command");
    assert_eq!(error.exit_code(), ExitCode::Failed);

    let pending_dir = feedback_dir.join("pending");
    if pending_dir.exists() {
        let left: Vec<_> = std::fs::read_dir(&pending_dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert!(
            left.is_empty(),
            "a failed receipt write must not leave a permanent pending bundle: {left:?}"
        );
    }
}

#[test]
fn feedback_submit_fails_cleanly_on_missing_body_file() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "feedback-missing", Isolation::ProductHome);
    let error = run(&[
        "pinvou",
        "feedback",
        "submit",
        "--type",
        "issue",
        "--title",
        "t",
        "--body-file",
        home.path().join("nope.md").to_str().unwrap(),
    ])
    .expect_err("missing body file must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed);
}

#[test]
fn feedback_submit_refuses_a_body_file_over_the_read_limit() {
    // The 64 KiB read cap exists so a multi-GB file or an endless device
    // cannot be loaded into memory before the 5000-char validation runs;
    // an over-cap file must be a clean CLI failure, not an OOM.
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "feedback-oversize", Isolation::ProductHome);
    let body = home.path().join("huge.md");
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
    assert_all_rejected_as_usage(invalid);
}

/// OPT-IN: `monitor status` (run_bare_host — no display needed) probes the
/// configured model endpoint; without a model it must report a clean offline
/// zero state. Run with: cargo test -p pinvou-cli --test
/// misc_contract -- --ignored monitor_status
///
/// Only the *probe* needs the opt-in. The stable-shape guard this test used to
/// carry was hermetic all along and is now a plain unit test next to the
/// payload builder (`monitor::tests::status_json_has_one_key_set_in_both_branches`
/// and friends, `cargo test -p pinvou-cli --lib monitor`), so a branch that
/// drifts out of shape fails on every run instead of only when someone
/// remembers `--ignored` on a machine with a display.
#[test]
#[ignore = "needs a model endpoint probe: cargo test --test misc_contract -- --ignored"]
fn monitor_status_reports_clean_zero_state_without_a_model() {
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "monitor-status", Isolation::ProductHome);
    let parsed = parse_args(["pinvou", "monitor", "status"]).unwrap();
    let outcome = execute(parsed).expect("status must not hard-fail without a model");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(
        outcome.stdout.contains("Online: false"),
        "monitor status output missing the offline line"
    );
    // The zero state carries the same fields the model-present branch does;
    // the stable key set and human line set are pinned hermetically by the
    // monitor unit tests (see the doc comment above), so this opt-in probe
    // only pins the live zero-state values.
    let value = run_json(&["pinvou", "monitor", "status"]);
    assert_eq!(value["vllm_online"], false);
    assert_eq!(value["health_status"], "unavailable");
}

/// OPT-IN: `monitor snapshot` (run_bare_host — no display needed) queries
/// nvidia-smi/local resources and probes the model endpoint.
/// Run with: cargo test -p pinvou-cli --test misc_contract -- --ignored
///   monitor_snapshot
///
/// The stable-shape rules (key sets, process-local accumulators reported as
/// not-applicable) are pinned hermetically by `monitor::tests::snapshot_*`
/// (`cargo test -p pinvou-cli --lib monitor`); this opt-in test only checks
/// that a live sample still dispatches and carries the headers.
#[test]
#[ignore = "needs GPU/endpoint probing: cargo test --test misc_contract -- --ignored"]
fn monitor_snapshot_produces_a_one_shot_sample() {
    let _env = common::env_lock();
    let _home = HomeGuard::new("misc", "monitor-snapshot", Isolation::ProductHome);
    let parsed = parse_args(["pinvou", "monitor", "snapshot"]).unwrap();
    let outcome = execute(parsed).expect("snapshot must produce a sample");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(
        outcome.stdout.contains("GeneratedAt: "),
        "monitor snapshot must carry the GeneratedAt header"
    );
    assert!(outcome.stdout.contains("Ram: "));
    assert!(outcome.stdout.contains("Cpu: "));
    assert!(outcome.stdout.contains("Backend: "));
}

/// Round-41 review: `feedback submit --attach` used to accept a DIRECTORY —
/// `metadata().len()` on one reports the inode size, and the entry lands in a
/// receipt that stays on disk forever. Every sibling ingest lane enforces the
/// regular-file rule; the attachment lane now does too.
#[test]
fn feedback_attach_refuses_directories() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "feedback-dir-attach", Isolation::ProductHome);
    let scoped = ScopedHomeDir::new("feedback-dir-attach");
    let attachment_dir = scoped.dir.join("not-a-file");
    std::fs::create_dir_all(&attachment_dir).unwrap();
    let body = scoped.dir.join("body.md");
    std::fs::write(&body, "body text\n").unwrap();

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
        "--attach",
        attachment_dir.to_str().unwrap(),
    ])
    .unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("feedback_submit_not_a_file"),
        "the refusal must carry the stable code: {error}"
    );
    // Nothing was registered: no receipt exists for a refused submit.
    let receipts = home.path().join("feedback").join("receipts");
    assert!(
        !receipts.exists(),
        "a refused attachment must not leave a receipt: {}",
        receipts.display()
    );
}

/// Round-41 review: `files ingest --output` no-overwrite contract was
/// implemented and documented but untested — the only export lane without
/// the refusal pin, so deleting the refusal would have passed the suite.
#[test]
fn files_ingest_output_refuses_overwrite_and_cleans_up_on_failure() {
    let _env = common::env_lock();
    let home = HomeGuard::new("misc", "files-ingest-no-overwrite", Isolation::ProductHome);
    let scoped = ScopedHomeDir::new("files-ingest-no-overwrite");
    let markdown_fixture = scoped.dir.join("notes.md");
    std::fs::write(&markdown_fixture, "# Heading\n\nBody.\n").unwrap();
    let output_file = home.path().join("extracted.md");
    std::fs::write(&output_file, "PRECIOUS EXISTING CONTENT\n").unwrap();

    let error = run(&[
        "pinvou",
        "files",
        "ingest",
        markdown_fixture.to_str().unwrap(),
        "--output",
        output_file.to_str().unwrap(),
    ])
    .unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("refusing to overwrite"),
        "the refusal must match the sibling export lanes' wording: {error}"
    );
    assert_eq!(
        std::fs::read_to_string(&output_file).unwrap(),
        "PRECIOUS EXISTING CONTENT\n",
        "the existing destination must be untouched"
    );
}
