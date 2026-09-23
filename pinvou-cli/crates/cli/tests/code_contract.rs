//! Contract tests for the `code` family (GUI-parity project).
//!
//! Parse-level coverage runs against the typed command tree. Execute-level
//! coverage only touches pure-storage paths: every hermetic test runs against
//! a throwaway `PINVOU3_HOME` (plus an isolated `CODEWHALE_HOME`-style secrets
//! root and `$HOME` so the provider config writers never touch the developer's
//! real CLI configuration) and boots the feature-layer stores directly — the
//! same standalone constructors the CLI uses. The workspace tests shell out to
//! the `git` binary (a build prerequisite) to build fixture repositories; no
//! engine, network, model, or display is involved in the default tests.
//!
//! Network/engine paths (login against a real vendor CLI, ACP installs and
//! model probes, one-shot ACP turns) are covered by `#[ignore]` opt-in tests
//! or by asserting the stable honest error codes.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use pinvou_cli::{CliError, CliOutcome, ExitCode, OutputMode, execute, parse_args};
use pinvou3_lib::features::code_checkpoints as checkpoints;
use pinvou3_lib::features::codex_acp::workspace as app_workspace;
use pinvou3_lib::features::codex_acp::{CodexWorkspaceKind, SessionAgentStore};
use pinvou3_lib::features::sessions::SessionStore;

/// Serialises tests that mutate the process-global environment variables
/// (`PINVOU3_HOME`, `CODEWHALE_HOME`, `HOME`), preventing data races when the
/// parallel test runner executes them concurrently.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Sets `PINVOU3_HOME`, `CODEWHALE_HOME`, and `HOME` to fresh throwaway
/// directories for the duration of the test and restores the previous values
/// on drop. `HOME` isolation keeps `ProviderManager`'s per-CLI config writers
/// (`~/.codex`, `~/.claude`) and the kimi data root inside the sandbox.
struct HomeGuard {
    previous: (Option<OsString>, Option<OsString>, Option<OsString>),
    root: PathBuf,
}

impl HomeGuard {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pinvou-cli-code-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let previous = (
            std::env::var_os("PINVOU3_HOME"),
            std::env::var_os("CODEWHALE_HOME"),
            std::env::var_os("HOME"),
        );
        // SAFETY: the caller holds ENV_LOCK for the whole test, so env writes
        // are serialized in-process.
        unsafe {
            std::env::set_var("PINVOU3_HOME", &root);
            std::env::set_var("CODEWHALE_HOME", root.join("codewhale"));
            std::env::set_var("HOME", root.join("home"));
        }
        std::fs::create_dir_all(root.join("home")).unwrap();
        Self { previous, root }
    }

    fn sessions_root(&self) -> PathBuf {
        self.root.join("sessions")
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        let (pinvou, codewhale, home) = (&self.previous.0, &self.previous.1, &self.previous.2);
        // SAFETY: ENV_LOCK is held by the owning test.
        unsafe {
            match pinvou {
                Some(value) => std::env::set_var("PINVOU3_HOME", value),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
            match codewhale {
                Some(value) => std::env::set_var("CODEWHALE_HOME", value),
                None => std::env::remove_var("CODEWHALE_HOME"),
            }
            match home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Captures one process environment variable and restores it on drop, so a
/// failing assertion cannot leak an override into sibling tests.
#[cfg(unix)]
struct EnvVarGuard {
    name: &'static str,
    previous: Option<OsString>,
}

#[cfg(unix)]
impl EnvVarGuard {
    fn capture(name: &'static str) -> Self {
        Self {
            name,
            previous: std::env::var_os(name),
        }
    }
}

#[cfg(unix)]
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: the owning test holds ENV_LOCK, so env writes are
        // serialized in-process.
        unsafe {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }
}

/// Removes a scratch directory on drop (panic-safe cleanup for fake CLI bins).
#[cfg(unix)]
struct ScratchDir(PathBuf);

#[cfg(unix)]
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(arguments: &[&str]) -> Result<CliOutcome, CliError> {
    let parsed =
        parse_args(arguments).unwrap_or_else(|error| panic!("{arguments:?} must parse: {error}"));
    execute(parsed)
}

fn run_json(arguments: &[&str]) -> serde_json::Value {
    let mut owned = arguments.to_vec();
    owned.extend(["--output", "json"]);
    let outcome = run(&owned).expect("execute must succeed");
    serde_json::from_str(&outcome.stdout).expect("json output must be a single serde_json line")
}

fn usage_error(arguments: &[&str]) -> CliError {
    parse_args(arguments.to_vec()).expect_err(arguments.join(" ").as_str())
}

/// Creates one chat session and binds it as a native code session (the same
/// feature-layer calls the GUI `create_codex_acp_session` command makes for
/// the "pinvou" backend) with an optional bound project directory. Temporary
/// sessions get their private execution directory created, mirroring the
/// GUI's `ensure_codex_workspace_root`.
fn create_code_session_fixture(project: Option<&Path>) -> String {
    let store = SessionStore::boot().expect("boot session store");
    let session = store
        .create_new("test-model".to_owned(), None, std::env::temp_dir())
        .expect("create session");
    let agents = SessionAgentStore::load_or_empty();
    match project {
        Some(project) => agents
            .bind_code_native_session(
                &session.metadata.id,
                CodexWorkspaceKind::Project,
                Some(project.to_path_buf()),
            )
            .expect("bind project code session"),
        None => {
            agents
                .bind_code_native_session(&session.metadata.id, CodexWorkspaceKind::Temporary, None)
                .expect("bind temporary code session");
            let workspace = store.session_roots(&session.metadata.id).unwrap().execution;
            std::fs::create_dir_all(workspace).expect("create temporary workspace");
        }
    }
    session.metadata.id
}

/// Creates one chat session bound to the codex ACP backend with a project
/// workspace (the same `set_acp_workspace` call the GUI session creation
/// makes for external agents).
fn create_acp_session_fixture(project: &Path) -> String {
    let store = SessionStore::boot().expect("boot session store");
    let session = store
        .create_new("Codex (ACP)".to_owned(), None, project.to_path_buf())
        .expect("create session");
    let agents = SessionAgentStore::load_or_empty();
    agents
        .set_acp_workspace(
            &session.metadata.id,
            pinvou3_lib::features::codex_acp::AgentBackend::CodexAcp,
            CodexWorkspaceKind::Project,
            Some(project.to_path_buf()),
        )
        .expect("bind acp workspace");
    session.metadata.id
}

/// Seeds a two-turn transcript by editing the persisted `SavedSession` JSON
/// (the CLI counts user turns from the same store file).
fn seed_two_turn_transcript(id: &str) {
    let home = PathBuf::from(std::env::var_os("PINVOU3_HOME").unwrap());
    let path = home.join("sessions").join(format!("{id}.json"));
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    value["messages"] = serde_json::json!([
        {"role": "user", "content": [{"type": "text", "text": "turn one"}]},
        {"role": "assistant", "content": [{"type": "text", "text": "answer one"}]},
        {"role": "user", "content": [{"type": "text", "text": "turn two"}]},
        {"role": "assistant", "content": [{"type": "text", "text": "answer two"}]},
    ]);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
}

/// Initializes a git repository with `main` + `feature` branches and one
/// committed file; returns `None` when git is unavailable, printing a visible
/// skip line so a green run cannot hide an environment gap.
fn init_git_repo(label: &str) -> Option<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "pinvou-cli-code-repo-{label}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    // Same ambient-gitconfig isolation as the production `git_command`: a
    // developer's global/system config (aliases, hooks, credential helpers)
    // must not decide whether the fixture repository builds.
    let devnull = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", devnull)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    };
    let skip = |reason: &str| -> Option<PathBuf> {
        eprintln!("skipping: git unavailable ({reason})");
        None
    };
    if !run(&["init", "-b", "main"]) {
        return skip("git init failed");
    }
    if !run(&["config", "user.email", "test@example.com"]) || !run(&["config", "user.name", "test"])
    {
        return skip("repo config failed");
    }
    std::fs::write(root.join("tracked.txt"), "v1\n").unwrap();
    for args in [
        &["add", "."][..],
        &["commit", "-m", "init"][..],
        &["branch", "feature"][..],
    ] {
        if !run(args) {
            return skip("fixture commit failed");
        }
    }
    Some(root)
}

// ── parse-level coverage ────────────────────────────────────────────────────

#[test]
fn every_code_subcommand_parses() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let valid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "code", "agents", "list"],
        vec!["pinvou", "code", "agents", "status", "codex"],
        vec!["pinvou", "code", "agents", "install", "claude"],
        vec!["pinvou", "code", "login", "kimi"],
        vec!["pinvou", "code", "login", "claude", "--code", "abcd-1234"],
        vec!["pinvou", "code", "logout", "codex"],
        vec!["pinvou", "code", "providers", "list"],
        vec!["pinvou", "code", "providers", "list", "--agent", "codex"],
        vec![
            "pinvou",
            "code",
            "providers",
            "add",
            "--agent",
            "codex",
            "--name",
            "Relay",
            "--base-url",
            "https://api.example.com/v1",
            "--wire-api",
            "openai",
            "--model",
            "gpt-test",
            "--context-window",
            "128000",
        ],
        vec![
            "pinvou",
            "code",
            "providers",
            "add",
            "--agent",
            "claude",
            "--name",
            "Relay",
            "--base-url",
            "https://api.example.com",
            "--model-slot",
            "opus=o1",
            "--model-slot",
            "sonnet=s1",
            "--model-slot",
            "haiku=h1",
            "--model-slot",
            "fable=f1",
            "--model-slot",
            "subagent=su1",
            "--api-key-env",
            "MY_KEY",
        ],
        vec![
            "pinvou",
            "code",
            "providers",
            "add",
            "--agent",
            "kimi",
            "--name",
            "Relay",
            "--base-url",
            "https://api.example.com",
            "--api-key-stdin",
        ],
        vec![
            "pinvou",
            "code",
            "providers",
            "update",
            "pv-1234567890ab",
            "--agent",
            "codex",
            "--name",
            "Renamed",
        ],
        vec![
            "pinvou",
            "code",
            "providers",
            "update",
            "pv-1234567890ab",
            "--agent",
            "claude",
            "--delete-key",
        ],
        vec![
            "pinvou",
            "code",
            "providers",
            "remove",
            "pv-1234567890ab",
            "--agent",
            "kimi",
            "--yes",
        ],
        vec![
            "pinvou",
            "code",
            "providers",
            "switch",
            "codex",
            "pv-1234567890ab",
        ],
        vec!["pinvou", "code", "providers", "switch-official", "claude"],
        vec![
            "pinvou",
            "code",
            "providers",
            "export",
            "--agent",
            "codex",
            "--output",
            "out.json",
        ],
        vec![
            "pinvou",
            "code",
            "providers",
            "import",
            "--agent",
            "codex",
            "in.json",
        ],
        vec![
            "pinvou",
            "code",
            "providers",
            "update",
            "pv-1234567890ab",
            "--agent",
            "kimi",
            "--context-window",
            "64000",
        ],
        vec!["pinvou", "code", "sessions", "list"],
        vec!["pinvou", "code", "sessions", "info", "s-1"],
        vec!["pinvou", "code", "sessions", "timeline", "s-1"],
        vec!["pinvou", "code", "workspace", "list", "s-1"],
        vec!["pinvou", "code", "workspace", "list", "s-1", "src"],
        vec!["pinvou", "code", "workspace", "search", "s-1", "main"],
        vec!["pinvou", "code", "workspace", "preview", "s-1", "README.md"],
        vec!["pinvou", "code", "workspace", "changes", "s-1"],
        vec!["pinvou", "code", "workspace", "diff", "s-1"],
        vec!["pinvou", "code", "workspace", "diff", "s-1", "src/main.rs"],
        vec!["pinvou", "code", "workspace", "branches", "s-1"],
        vec![
            "pinvou",
            "code",
            "workspace",
            "checkout",
            "s-1",
            "feature",
            "--mode",
            "carry",
        ],
        vec![
            "pinvou",
            "code",
            "workspace",
            "checkout",
            "s-1",
            "feature",
            "--mode",
            "stash",
        ],
        vec![
            "pinvou",
            "code",
            "workspace",
            "checkout",
            "s-1",
            "feature",
            "--mode",
            "commit",
            "--message",
            "wip",
        ],
        vec!["pinvou", "code", "checkpoints", "list", "s-1"],
        vec!["pinvou", "code", "checkpoints", "diff", "s-1", "c1-123"],
        vec![
            "pinvou",
            "code",
            "checkpoints",
            "rewind",
            "s-1",
            "1",
            "--yes",
        ],
        vec!["pinvou", "code", "checkpoints", "rewind", "s-1", "0"],
        vec!["pinvou", "code", "checkpoints", "undo", "s-1"],
        vec![
            "pinvou",
            "code",
            "run",
            "codex",
            "--workspace",
            ".",
            "--prompt",
            "hello",
        ],
        vec![
            "pinvou",
            "code",
            "run",
            "claude",
            "--workspace",
            "/tmp",
            "--prompt-file",
            "p.md",
            "--timeout-secs",
            "30",
        ],
        vec!["pinvou", "code", "permissions", "s-1"],
        vec!["pinvou", "code", "respond", "s-1", "req-1", "allow"],
        vec!["pinvou", "code", "respond", "s-1", "req-1", "deny"],
    ];
    for arguments in &valid {
        let parsed = parse_args(arguments.clone())
            .unwrap_or_else(|error| panic!("{arguments:?} must parse: {error}"));
        let debug = format!("{:?}", parsed.command());
        assert!(debug.starts_with("Code("), "{arguments:?} -> {debug}");
    }

    // --output json flows through the family dispatch.
    let parsed = parse_args(["pinvou", "--output", "json", "code", "agents", "list"].to_vec())
        .expect("global output flag");
    assert_eq!(parsed.output(), OutputMode::Json);
}

#[test]
fn invalid_code_usage_exits_two_and_names_valid_values() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let invalid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "code"],
        vec!["pinvou", "code", "bogus"],
        // Unknown agent names must be refused with the valid set.
        vec!["pinvou", "code", "agents", "status", "gemini"],
        vec!["pinvou", "code", "login", "deepseek"],
        vec!["pinvou", "code", "logout"],
        vec![
            "pinvou",
            "code",
            "providers",
            "add",
            "--agent",
            "gpt",
            "--name",
            "n",
            "--base-url",
            "https://a.com",
        ],
        vec!["pinvou", "code", "providers", "add", "--agent", "codex"],
        vec![
            "pinvou",
            "code",
            "providers",
            "add",
            "--agent",
            "codex",
            "--name",
            "n",
        ],
        vec![
            "pinvou",
            "code",
            "providers",
            "add",
            "--agent",
            "codex",
            "--name",
            "n",
            "--base-url",
            "https://a.com",
            "--wire-api",
            "grpc",
        ],
        vec![
            "pinvou",
            "code",
            "providers",
            "add",
            "--agent",
            "codex",
            "--name",
            "n",
            "--base-url",
            "https://a.com",
            "--api-key-env",
            "A",
            "--api-key-stdin",
        ],
        vec![
            "pinvou",
            "code",
            "providers",
            "update",
            "--agent",
            "codex",
            "--name",
            "n",
        ],
        vec![
            "pinvou",
            "code",
            "providers",
            "remove",
            "pv-1",
            "--agent",
            "codex",
            "--bogus",
        ],
        vec!["pinvou", "code", "providers", "switch", "codex"],
        vec![
            "pinvou",
            "code",
            "providers",
            "export",
            "--output",
            "o.json",
        ],
        vec!["pinvou", "code", "providers", "import"],
        vec!["pinvou", "code", "providers", "probe", "pv-1"],
        vec!["pinvou", "code", "providers", "bogus"],
        vec!["pinvou", "code", "sessions"],
        vec!["pinvou", "code", "sessions", "bogus"],
        vec!["pinvou", "code", "sessions", "info"],
        vec!["pinvou", "code", "sessions", "timeline", "s-1", "--full"],
        vec!["pinvou", "code", "workspace"],
        vec!["pinvou", "code", "workspace", "list"],
        vec![
            "pinvou",
            "code",
            "workspace",
            "list",
            "s-1",
            "src",
            "--bogus",
        ],
        vec!["pinvou", "code", "workspace", "search", "s-1"],
        vec!["pinvou", "code", "workspace", "preview", "s-1"],
        // checkout without --mode is a usage error naming the valid values
        // (the GUI always sends an explicit mode from its selector).
        vec!["pinvou", "code", "workspace", "checkout", "s-1", "feature"],
        vec![
            "pinvou",
            "code",
            "workspace",
            "checkout",
            "s-1",
            "feature",
            "--mode",
            "discard",
        ],
        vec![
            "pinvou",
            "code",
            "workspace",
            "checkout",
            "s-1",
            "feature",
            "--mode",
            "commit",
        ],
        // Option-like branch names are refused at parse time so they can never
        // reach git as options (mirror of the GUI branch-name validation).
        vec![
            "pinvou",
            "code",
            "workspace",
            "checkout",
            "s-1",
            "--force",
            "--mode",
            "carry",
        ],
        vec!["pinvou", "code", "workspace", "branches"],
        // rewind without --yes parses but execute refuses (exit 2 via
        // require_yes); everything else here is a parse error.
        vec!["pinvou", "code", "checkpoints", "list"],
        vec!["pinvou", "code", "checkpoints", "diff", "s-1"],
        vec![
            "pinvou",
            "code",
            "checkpoints",
            "rewind",
            "s-1",
            "x",
            "--yes",
        ],
        vec!["pinvou", "code", "checkpoints", "bogus"],
        vec!["pinvou", "code", "run", "codex"],
        vec!["pinvou", "code", "run", "codex", "--workspace", "."],
        vec![
            "pinvou",
            "code",
            "run",
            "codex",
            "--workspace",
            ".",
            "--prompt",
            "a",
            "--prompt-file",
            "b",
        ],
        vec![
            "pinvou",
            "code",
            "run",
            "codex",
            "--workspace",
            ".",
            "--prompt-file",
            "b",
            "--timeout-secs",
            "x",
        ],
        vec!["pinvou", "code", "permissions"],
        vec!["pinvou", "code", "respond", "s-1", "req-1"],
        vec!["pinvou", "code", "respond", "s-1", "req-1", "maybe"],
        vec!["pinvou", "code", "../escape", "x"],
    ];
    for arguments in &invalid {
        let error = usage_error(arguments);
        assert_eq!(error.exit_code(), ExitCode::Usage, "{arguments:?}");
    }

    // The agent error message names the valid agents.
    let error = usage_error(&["pinvou", "code", "agents", "status", "gemini"]);
    let message = error.to_string();
    assert!(
        message.contains("codex") && message.contains("claude") && message.contains("kimi"),
        "unknown agent error must name codex|claude|kimi: {message}"
    );
    // The checkout mode error names the valid modes.
    let error = usage_error(&[
        "pinvou",
        "code",
        "workspace",
        "checkout",
        "s-1",
        "feature",
        "--mode",
        "discard",
    ]);
    let message = error.to_string();
    assert!(
        message.contains("carry") && message.contains("stash") && message.contains("commit"),
        "mode error must name carry|stash|commit: {message}"
    );
}

#[test]
fn rewind_without_yes_is_a_usage_error() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("rewind-needs-yes");
    let id = create_code_session_fixture(None);
    let parsed = parse_args(["pinvou", "code", "checkpoints", "rewind", &id, "0"].to_vec())
        .expect("rewind parses without --yes");
    let error = execute(parsed).expect_err("rewind without --yes must refuse");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("--yes"));
}

#[test]
fn logout_and_undo_without_yes_are_usage_errors() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("logout-undo-need-yes");
    let id = create_code_session_fixture(None);

    // logout erases the vendor CLI's stored credentials, so it gates on --yes.
    let parsed = parse_args(["pinvou", "code", "logout", "codex"].to_vec())
        .expect("logout parses without --yes");
    let error = execute(parsed).expect_err("logout without --yes must refuse");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("--yes"));

    // undo restores the working tree and rewrites the transcript, like rewind.
    let parsed = parse_args(["pinvou", "code", "checkpoints", "undo", &id].to_vec())
        .expect("undo parses without --yes");
    let error = execute(parsed).expect_err("undo without --yes must refuse");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("--yes"));
}

// ── execute-level coverage (pure storage, temp PINVOU3_HOME) ────────────────

#[test]
fn code_sessions_list_zero_state_and_filters_code_sessions() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("sessions-list");

    // Zero state: empty listing, valid JSON shape.
    let outcome = run(&["pinvou", "code", "sessions", "list"]).expect("zero-state list");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert_eq!(outcome.stdout, "");
    let value = run_json(&["pinvou", "code", "sessions", "list"]);
    assert_eq!(value["sessions"].as_array().unwrap().len(), 0);

    // A plain chat session and an archived code session are filtered out; the
    // freshly bound code session is listed with its agent + workspace.
    let store = SessionStore::boot().unwrap();
    let _plain = store
        .create_new("plain".to_owned(), None, std::env::temp_dir())
        .unwrap()
        .metadata
        .id;
    drop(store);
    let code_id = create_code_session_fixture(None);
    run_json(&["pinvou", "sessions", "archive", &code_id]);
    let value = run_json(&["pinvou", "code", "sessions", "list"]);
    assert_eq!(value["sessions"].as_array().unwrap().len(), 0);
    run_json(&["pinvou", "sessions", "restore", &code_id]);
    let value = run_json(&["pinvou", "code", "sessions", "list"]);
    let rows = value["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], code_id.as_str());
    assert_eq!(rows[0]["agent_id"], "pinvou");
    assert_eq!(rows[0]["workspace_available"], true);
    assert_eq!(
        rows[0]["workspace_path"],
        home.sessions_root()
            .join(&code_id)
            .join("workspace")
            .display()
            .to_string()
    );

    // Unknown session ids fail with a host error.
    let error = run(&["pinvou", "code", "sessions", "info", "missing-session"]).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Failed);
}

#[test]
fn code_sessions_info_and_timeline_read_persisted_state() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("sessions-info");
    let project = home.root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let id = create_acp_session_fixture(&project);

    let value = run_json(&["pinvou", "code", "sessions", "info", &id]);
    assert_eq!(value["agent_id"], "codex");
    assert_eq!(value["agent_name"], "Codex");
    assert_eq!(value["workspace_kind"], "project");
    assert_eq!(value["workspace_path"], project.display().to_string());
    assert_eq!(value["workspace_available"], true);

    // Timeline reads the same append-only JSONL the GUI's AcpPool writes;
    // a session without one yields an empty listing.
    let outcome = run(&["pinvou", "code", "sessions", "timeline", &id]).expect("empty timeline");
    assert_eq!(outcome.stdout, "");
    let session_dir = home.sessions_root().join(&id);
    std::fs::create_dir_all(&session_dir).unwrap();
    // The first two lines use the exact envelope the GUI's AcpPool journal
    // writes (AcpEventEnvelope: camelCase fields, `event.type`); the last
    // event uses the legacy snake_case spelling so the human renderer's
    // fallback is pinned too.
    std::fs::write(
        session_dir.join("acp-timeline.jsonl"),
        format!(
            "{{\"version\":1,\"sessionId\":\"{id}\",\"seq\":2,\"timestamp\":\"t2\",\"event\":{{\"type\":\"turn_finished\",\"data\":{{}}}}}}\n\
             {{\"version\":1,\"sessionId\":\"{id}\",\"seq\":1,\"timestamp\":\"t1\",\"turnId\":\"turn-1\",\"event\":{{\"type\":\"turn_started\",\"data\":{{}}}}}}\n\
             {{\"version\":1,\"session_id\":\"{id}\",\"seq\":3,\"timestamp\":\"t3\",\"turn_id\":\"turn-2\",\"event\":{{\"event_type\":\"turn_finished\",\"data\":{{}}}}}}\n\
             not-json-at-all\n"
        ),
    )
    .unwrap();
    let value = run_json(&["pinvou", "code", "sessions", "timeline", &id]);
    let events = value["events"].as_array().unwrap();
    assert_eq!(events.len(), 3, "malformed lines are skipped");
    assert_eq!(events[0]["seq"], 1);
    assert_eq!(events[0]["turnId"], "turn-1");
    assert_eq!(
        events[1].pointer("/event/type"),
        Some(&serde_json::json!("turn_finished"))
    );
    let outcome = run(&["pinvou", "code", "sessions", "timeline", &id]).expect("human timeline");
    // Both the event-type and turn-id columns render for the camelCase
    // envelope and the legacy snake_case spelling (a missing turnId renders
    // "-" — envelopes without an active turn exist).
    assert_eq!(outcome.stdout.lines().count(), 3);
    assert!(
        outcome.stdout.contains("turn_started"),
        "timeline should contain the turn_started event"
    );
    assert!(
        outcome.stdout.contains("turn-1"),
        "timeline should contain the turn id column"
    );
    assert!(
        outcome
            .stdout
            .lines()
            .all(|line| line.split('\t').nth(2).is_some_and(|kind| !kind.is_empty())),
        "timeline event-type column must never be silently empty"
    );
}

/// A session whose persisted JSON model is an ACP model name but whose
/// session-agents sidecar record is missing (the sidecar-loss fallback) must
/// be accepted everywhere `code sessions list` accepts it: listed with the
/// model-derived agent label, readable through `info`, and its missing
/// journal reported as an empty timeline success.
#[test]
fn sidecar_lost_acp_session_is_listed_and_readable() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("sidecar-lost-acp");
    let project = home.root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    // A plain chat session whose model name is the ACP sentinel: no
    // session-agents.json record is ever written for it.
    let store = SessionStore::boot().unwrap();
    let id = store
        .create_new("Codex (ACP)".to_owned(), None, project.clone())
        .unwrap()
        .metadata
        .id;
    drop(store);

    // list shows it with the degraded workspace and the derived agent.
    let value = run_json(&["pinvou", "code", "sessions", "list"]);
    let rows = value["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], id.as_str());
    assert_eq!(rows[0]["agent_id"], "codex");
    assert_eq!(rows[0]["workspace_available"], false);

    // info succeeds on the same fallback with the identical label.
    let value = run_json(&["pinvou", "code", "sessions", "info", &id]);
    assert_eq!(value["agent_id"], "codex");
    assert_eq!(value["agent_name"], "Codex");
    assert_eq!(value["workspace_available"], false);

    // and the missing journal is an empty success, not an error.
    let outcome = run(&["pinvou", "code", "sessions", "timeline", &id]).expect("empty timeline");
    assert_eq!(outcome.stdout, "");
    let value = run_json(&["pinvou", "code", "sessions", "timeline", &id]);
    assert_eq!(value["events"].as_array().unwrap().len(), 0);
}

/// Plain (non-code) chat sessions are refused by `info` and `timeline` with
/// the same stable error the workspace commands use (timeline used to report
/// an empty success for any existing session).
#[test]
fn sessions_info_and_timeline_refuse_plain_chat_sessions() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("info-timeline-gate");
    let store = SessionStore::boot().unwrap();
    let plain = store
        .create_new("plain".to_owned(), None, std::env::temp_dir())
        .unwrap()
        .metadata
        .id;
    drop(store);
    for action in ["info", "timeline"] {
        let error = run(&["pinvou", "code", "sessions", action, &plain]).unwrap_err();
        assert_eq!(error.exit_code(), ExitCode::Failed, "{action}: {error}");
        assert!(
            error.to_string().contains("code_session_not_found"),
            "{action}: {error}"
        );
    }
}

#[test]
fn workspace_list_search_preview_round_trip_with_fixture_session() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("workspace-read");
    let project = home.root.join("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::create_dir_all(project.join("node_modules")).unwrap();
    std::fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(project.join("README.md"), "# fixture\n").unwrap();
    std::fs::write(project.join("node_modules/index.js"), "ignored").unwrap();
    let id = create_code_session_fixture(Some(&project));

    // list hides ignored directories and reports entries with kinds.
    let value = run_json(&["pinvou", "code", "workspace", "list", &id]);
    let entries = value["entries"].as_array().unwrap();
    let names: Vec<&str> = entries
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(names.contains(&"src"));
    assert!(names.contains(&"README.md"));
    assert!(!names.contains(&"node_modules"), "ignored dirs are hidden");
    assert_eq!(value["relativePath"], "");

    // Sub-directory listing.
    let value = run_json(&["pinvou", "code", "workspace", "list", &id, "src"]);
    assert_eq!(value["entries"].as_array().unwrap().len(), 1);
    assert_eq!(value["entries"][0]["name"], "main.rs");

    // search matches relative paths case-insensitively.
    let value = run_json(&["pinvou", "code", "workspace", "search", &id, "readme"]);
    assert_eq!(value["results"].as_array().unwrap().len(), 1);
    assert_eq!(value["results"][0]["relativePath"], "README.md");

    // preview returns text content for text files.
    let value = run_json(&["pinvou", "code", "workspace", "preview", &id, "README.md"]);
    assert_eq!(value["kind"], "text");
    assert_eq!(value["text"], "# fixture\n");
    assert_eq!(value["relativePath"], "README.md");

    // Path escapes are refused; missing paths fail with host errors.
    let error = run(&["pinvou", "code", "workspace", "list", &id, "../escape"]).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Usage);
    let error = run(&["pinvou", "code", "workspace", "preview", &id, "missing.md"]).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Failed);
}

/// `workspace search` must report when the app module's SEARCH_LIMIT cut the
/// result list, mirroring the `truncated` flag of `workspace list`.
#[test]
fn workspace_search_reports_truncation_past_the_limit() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("search-truncated");
    let project = home.root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    // One match past the app module's SEARCH_LIMIT (300) plus one non-match.
    for index in 0..301 {
        std::fs::write(project.join(format!("needle-{index:03}.txt")), "x\n").unwrap();
    }
    std::fs::write(project.join("other.md"), "x\n").unwrap();
    let id = create_code_session_fixture(Some(&project));

    let value = run_json(&["pinvou", "code", "workspace", "search", &id, "needle"]);
    assert_eq!(value["results"].as_array().unwrap().len(), 300);
    assert_eq!(value["truncated"], serde_json::json!(true));

    // A short result set is not reported as truncated.
    let value = run_json(&["pinvou", "code", "workspace", "search", &id, "other"]);
    assert_eq!(value["results"].as_array().unwrap().len(), 1);
    assert_eq!(value["truncated"], serde_json::json!(false));
}

#[test]
fn workspace_git_changes_diff_branches_against_fixture_repo() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("workspace-git");
    let Some(project) = init_git_repo("changes") else {
        return; // git unavailable in the environment
    };
    let id = create_code_session_fixture(Some(&project));

    // branches: both local branches listed, main current, clean tree.
    let value = run_json(&["pinvou", "code", "workspace", "branches", &id]);
    assert_eq!(value["git"], true);
    assert_eq!(value["current"], "main");
    let branches: Vec<&str> = value["branches"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    assert!(branches.contains(&"main") && branches.contains(&"feature"));
    assert_eq!(value["dirtyCount"], 0);

    // changes: an untracked file shows up with its origin classified.
    std::fs::write(project.join("new.txt"), "added by fixture\n").unwrap();
    let value = run_json(&["pinvou", "code", "workspace", "changes", &id]);
    assert_eq!(value["git"], true);
    assert_eq!(value["baselineAvailable"], false);
    let changes = value["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["relativePath"], "new.txt");
    assert_eq!(changes[0]["status"], "untracked");
    assert_eq!(changes[0]["staged"], false);

    // diff of the untracked file renders the synthetic new-file diff.
    let value = run_json(&["pinvou", "code", "workspace", "diff", &id, "new.txt"]);
    assert!(
        value["text"]
            .as_str()
            .unwrap()
            .contains("+added by fixture"),
        "untracked diff must carry the synthetic new-file hunk"
    );
    assert_eq!(value["relativePath"], "new.txt");

    // diff of a modified tracked file contains the '+' hunk.
    std::fs::write(project.join("tracked.txt"), "v2\n").unwrap();
    let value = run_json(&["pinvou", "code", "workspace", "diff", &id, "tracked.txt"]);
    assert!(value["text"].as_str().unwrap().contains("+v2"));

    // diff without a file concatenates the per-file diffs.
    let value = run_json(&["pinvou", "code", "workspace", "diff", &id]);
    let text = value["text"].as_str().unwrap();
    assert!(
        text.contains("+v2") && text.contains("+added by fixture"),
        "the combined diff must contain both per-file hunks"
    );

    // checkout without a dirty tree: carry switches branches cleanly.
    let value = run_json(&[
        "pinvou",
        "code",
        "workspace",
        "checkout",
        &id,
        "feature",
        "--mode",
        "carry",
    ]);
    assert_eq!(value["checkedOut"], "feature");
    assert_eq!(value["branches"]["current"], "feature");
    let value = run_json(&["pinvou", "code", "workspace", "branches", &id]);
    assert_eq!(value["current"], "feature");

    // checkout validation: unknown branches fail at execute level.
    for branch in ["missing-branch"] {
        let error = run(&[
            "pinvou",
            "code",
            "workspace",
            "checkout",
            &id,
            branch,
            "--mode",
            "carry",
        ])
        .unwrap_err();
        assert_eq!(error.exit_code(), ExitCode::Failed, "{branch}");
    }

    // commit mode requires a non-empty --message (parse-level exit 2).
    let error = usage_error(&[
        "pinvou",
        "code",
        "workspace",
        "checkout",
        &id,
        "main",
        "--mode",
        "commit",
    ]);
    assert_eq!(error.exit_code(), ExitCode::Usage);
}

// The whole-workspace diff must stop diffing once the payload is over
// DIFF_LIMIT instead of accumulating every per-file diff (500 × 1 MiB)
// before the final truncation.
#[test]
fn whole_workspace_diff_truncates_without_accumulating_over_the_cap() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("workspace-diff-cap");
    let Some(project) = init_git_repo("diff-cap") else {
        return; // git unavailable in the environment
    };
    let id = create_code_session_fixture(Some(&project));

    // Two modified tracked files whose diffs each exceed DIFF_LIMIT (1 MiB):
    // the per-file path caps each text, and the whole-workspace loop must
    // stop diffing once the payload is over the cap — the second file's
    // content must never enter the payload, which distinguishes the early
    // break from a post-hoc truncation of the full accumulation.
    let big = format!("v2 {}\n", "x".repeat(1024 * 1024 + 4096));
    std::fs::write(project.join("tracked.txt"), &big).unwrap();
    let big2 = format!("v3 second-file {}\n", "y".repeat(1024 * 1024 + 4096));
    std::fs::write(project.join("tracked2.txt"), &big2).unwrap();
    let value = run_json(&["pinvou", "code", "workspace", "diff", &id]);
    assert_eq!(value["truncated"], serde_json::json!(true));
    let text = value["text"].as_str().unwrap();
    assert!(text.len() <= 1024 * 1024, "len={}", text.len());
    // Exactly one over-cap hunk may be in the payload, whichever file the
    // status list orders first: the loop must stop diffing once the payload
    // is over the cap, not accumulate every per-file diff before cutting.
    let has_first = text.contains("+v2");
    let has_second = text.contains("second-file");
    assert!(
        has_first ^ has_second,
        "the payload must contain exactly one over-cap hunk (first={has_first}, \
         second={has_second}) — accumulating both before the cut would fail this"
    );
}

/// The capped reader must keep draining past the cap: a diff that exceeds
/// the cap by MORE than one pipe buffer used to park git on a full pipe
/// forever (the reader had stopped, the stderr thread never saw EOF, and the
/// lane has no deadline). The sibling test's overhang is ~4 KiB — under the
/// pipe buffer — so it cannot distinguish draining from stopping; this one
/// overhangs by 512 KiB and fails fast on a watchdog instead of hanging CI.
#[test]
fn whole_workspace_diff_drains_a_diff_exceeding_the_cap_by_more_than_a_pipe() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("diff-cap-overhang");
    let Some(project) = init_git_repo("diff-cap-overhang") else {
        return; // git unavailable in the environment
    };
    let id = create_code_session_fixture(Some(&project));
    let big = format!("v2 {}\n", "x".repeat(1024 * 1024 + 512 * 1024));
    std::fs::write(project.join("tracked.txt"), &big).unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = parse_args([
            "pinvou",
            "code",
            "workspace",
            "diff",
            &id,
            "tracked.txt",
            "--output",
            "json",
        ])
        .and_then(|parsed| pinvou_cli::execute(parsed));
        let _ = tx.send(result);
    });
    let result = rx
        .recv_timeout(std::time::Duration::from_secs(90))
        .expect("the diff must finish — a hang here means the capped reader \
                 stopped draining past the cap and parked git on a full pipe");
    let outcome = result.expect("over-cap diff executes");
    let value: serde_json::Value =
        serde_json::from_str(&outcome.stdout).expect("single-line json");
    assert_eq!(value["truncated"], serde_json::json!(true));
    // The body is cut at DIFF_LIMIT; the section header rides on top.
    assert!(
        value["text"].as_str().unwrap().len() <= 1024 * 1024 + 4096,
        "the capped body must stay at the cap plus framing"
    );
}

/// The per-file diff lane composes the staged and unstaged sections for one
/// file ("# staged" first, then "# unstaged") — the composition branch is
/// distinct from the unstaged-only and untracked cases pinned above and was
/// historically untested.
#[test]
fn per_file_diff_composes_staged_and_unstaged_sections() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("diff-staged");
    let Some(project) = init_git_repo("diff-staged") else {
        return; // git unavailable in the environment
    };
    let id = create_code_session_fixture(Some(&project));

    // Stage one version, then modify the file further: the per-file diff
    // must render both sections against the committed baseline.
    std::fs::write(project.join("tracked.txt"), "v2 staged\n").unwrap();
    let devnull = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&project)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", devnull)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    };
    assert!(git(&["add", "tracked.txt"]), "git add must succeed");
    std::fs::write(project.join("tracked.txt"), "v2 staged\nv3 unstaged\n").unwrap();

    let value = run_json(&["pinvoy", "code", "workspace", "diff", &id, "tracked.txt"]);
    assert_eq!(value["truncated"], serde_json::json!(false));
    let text = value["text"].as_str().unwrap();
    let staged_at = text.find("# staged").expect("staged section header");
    let unstaged_at = text.find("# unstaged").expect("unstaged section header");
    assert!(
        staged_at < unstaged_at,
        "the staged section must come before the unstaged section"
    );
    assert!(text.contains("+v2 staged"), "staged hunk: {text}");
    assert!(text.contains("+v3 unstaged"), "unstaged hunk: {text}");
}

/// The per-file diff lane stays a CLI mirror on purpose (bounded untracked
/// reads, English section copy, whole-workspace composition are CLI-specific),
/// so its output is differentially pinned against the app module on the same
/// fixture tree: identical `truncated` flags and identical git diff bodies.
/// Unix-only like the env-var guard it shares with the other git lanes.
#[cfg(unix)]
#[test]
fn workspace_diff_stays_pinned_to_the_app_module_on_a_fixture() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("diff-pin");
    // The CLI's git lane pins the ambient gitconfig away while the app module
    // honors it; pin it here too so both render the same diff for the same
    // tree regardless of the developer's global diff settings.
    let _git_global = EnvVarGuard::capture("GIT_CONFIG_GLOBAL");
    let _git_nosystem = EnvVarGuard::capture("GIT_CONFIG_NOSYSTEM");
    unsafe {
        std::env::set_var(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        );
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    let Some(project) = init_git_repo("diff-pin") else {
        return; // git unavailable in the environment
    };
    let id = create_code_session_fixture(Some(&project));
    std::fs::write(project.join("tracked.txt"), "v2\n").unwrap();
    std::fs::write(project.join("untracked.txt"), "brand new\n").unwrap();

    // Modified tracked file: both render one section-header line followed by
    // the same git diff.
    let cli = run_json(&["pinvou", "code", "workspace", "diff", &id, "tracked.txt"]);
    let app = app_workspace::workspace_diff(&id, &project, "tracked.txt")
        .expect("app diff for the modified tracked file");
    assert_eq!(cli["relativePath"], app.relative_path);
    assert_eq!(cli["truncated"], app.truncated);
    let body = |text: &str| text.lines().skip(1).collect::<Vec<_>>().join("\n");
    assert_eq!(
        body(cli["text"].as_str().unwrap()),
        body(&app.text),
        "the CLI diff body must match the app module's line for line"
    );

    // Untracked file: both synthesize the same new-file diff, byte for byte.
    let cli = run_json(&["pinvou", "code", "workspace", "diff", &id, "untracked.txt"]);
    let app = app_workspace::workspace_diff(&id, &project, "untracked.txt")
        .expect("app diff for the untracked file");
    assert_eq!(cli["truncated"], app.truncated);
    assert_eq!(
        cli["text"].as_str().unwrap(),
        app.text,
        "untracked synthetic diffs must be identical"
    );
}

#[test]
fn workspace_stash_mode_round_trips_dirty_changes() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("workspace-stash");
    let Some(project) = init_git_repo("stash") else {
        return;
    };
    let id = create_code_session_fixture(Some(&project));
    std::fs::write(project.join("tracked.txt"), "dirty\n").unwrap();
    let value = run_json(&[
        "pinvou",
        "code",
        "workspace",
        "checkout",
        &id,
        "feature",
        "--mode",
        "stash",
    ]);
    assert_eq!(value["checkedOut"], "feature");
    // The stash was popped after the switch: the dirty content survived.
    assert_eq!(
        std::fs::read_to_string(project.join("tracked.txt")).unwrap(),
        "dirty\n"
    );
}

#[test]
fn workspace_requires_a_code_session() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("workspace-gate");
    // A plain (non-code) session is refused.
    let store = SessionStore::boot().unwrap();
    let plain = store
        .create_new("plain".to_owned(), None, std::env::temp_dir())
        .unwrap()
        .metadata
        .id;
    drop(store);
    let error = run(&["pinvou", "code", "workspace", "branches", &plain]).unwrap_err();
    assert!(
        error.to_string().contains("code_session_not_found"),
        "{error}"
    );
    // Missing sessions are refused before any workspace resolution.
    let error = run(&["pinvou", "code", "workspace", "branches", "missing-session"]).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Failed);
}

#[test]
fn checkpoints_list_zero_state_and_full_rewind_undo_round_trip() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("checkpoints");
    let project = home.root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let id = create_code_session_fixture(Some(&project));
    seed_two_turn_transcript(&id);

    // Zero state: no checkpoints yet.
    let outcome = run(&["pinvou", "code", "checkpoints", "list", &id]).expect("zero checkpoints");
    assert_eq!(outcome.stdout, "");
    let value = run_json(&["pinvou", "code", "checkpoints", "list", &id]);
    assert_eq!(value["checkpoints"].as_array().unwrap().len(), 0);

    // Snapshot the execution root at turn boundaries (the same feature calls
    // the GUI chat flow makes): the turn-1 snapshot captures turn-one.txt, the
    // turn-2 snapshot captures both files. The store gets the same execution
    // root resolver the app startup and the CLI install, so the two roots
    // match the CLI's resolution exactly.
    let store = SessionStore::boot().unwrap();
    let agents = SessionAgentStore::load_or_empty();
    store.set_execution_root_resolver(std::sync::Arc::new(move |session_id: &str| {
        agents.code_project_workspace(session_id)
    }));
    let roots = store.session_roots(&id).unwrap();
    // The GUI snapshots at the START of each turn, so the turn-1 snapshot
    // captures the pre-turn state and the turn-2 snapshot captures turn 1's
    // changes ("rewind to the end of turn 1" restores the turn-2 snapshot).
    let turn1 = checkpoints::create_checkpoint(
        &roots.ledger,
        &roots.execution,
        Some(1),
        checkpoints::CheckpointKind::Turn,
        "turn one",
    )
    .expect("create turn-1 checkpoint");
    std::fs::write(project.join("turn-one.txt"), "first\n").unwrap();
    let turn2 = checkpoints::create_checkpoint(
        &roots.ledger,
        &roots.execution,
        Some(2),
        checkpoints::CheckpointKind::Turn,
        "turn two",
    )
    .expect("create turn-2 checkpoint");
    std::fs::write(project.join("turn-two.txt"), "second\n").unwrap();
    drop(store);

    let value = run_json(&["pinvou", "code", "checkpoints", "list", &id]);
    let listed = value["checkpoints"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["id"], turn1.id.as_str());
    assert_eq!(listed[1]["kind"], "turn");

    // diff reports what a rewind to the turn-1 snapshot (the pre-turn state)
    // would undo: both turns' files.
    let value = run_json(&["pinvou", "code", "checkpoints", "diff", &id, &turn1.id]);
    let diff = &value["diff"];
    assert_eq!(diff["checkpoint"]["id"], turn1.id.as_str());
    let paths: Vec<&str> = diff["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|change| change["path"].as_str())
        .collect();
    assert!(
        paths.contains(&"turn-one.txt") && paths.contains(&"turn-two.txt"),
        "{paths:?}"
    );

    // rewind to the end of turn 1: restores the snapshot and truncates the
    // transcript to one user turn. The result carries the forced PreRestore
    // rollback point (undo bookkeeping), not the consumed target snapshot.
    let value = run_json(&["pinvou", "code", "checkpoints", "rewind", &id, "1", "--yes"]);
    assert_eq!(value["rewoundTurns"], 1);
    assert_eq!(value["degraded"], false);
    let pre_restore = value["restoredCheckpoint"]["id"]
        .as_str()
        .expect("rewind reports its PreRestore rollback point")
        .to_owned();
    assert_eq!(value["restoredCheckpoint"]["kind"], "preRestore");
    assert_ne!(pre_restore, turn1.id.as_str());
    assert!(
        !project.join("turn-two.txt").exists(),
        "rewind removed the file"
    );
    assert_eq!(
        std::fs::read_to_string(project.join("turn-one.txt")).unwrap(),
        "first\n"
    );
    let store = SessionStore::boot().unwrap();
    let session = store.load(&id).unwrap();
    assert_eq!(session.messages.len(), 2, "transcript truncated to turn 1");
    drop(store);

    // The rewind bookkeeping left an undoable record bound to its PreRestore
    // snapshot; undo restores both code and transcript.
    let value = run_json(&["pinvou", "code", "checkpoints", "undo", &id, "--yes"]);
    assert_eq!(value["restoredMessages"], 2);
    assert_eq!(
        value["restoredCheckpoint"].as_str().map(str::to_string),
        Some(pre_restore.clone()),
        "undo restores the rewind's bound rollback point"
    );
    assert_eq!(
        std::fs::read_to_string(project.join("turn-two.txt")).unwrap(),
        "second\n"
    );
    let store = SessionStore::boot().unwrap();
    let session = store.load(&id).unwrap();
    assert_eq!(session.messages.len(), 4, "transcript fully restored");
    drop(store);

    // A second undo is honestly refused: the record was consumed.
    let error = run(&["pinvou", "code", "checkpoints", "undo", &id, "--yes"]).unwrap_err();
    assert!(error.to_string().contains("no_undoable_rewind"), "{error}");

    // Rewinding past the current turn count fails before touching anything.
    let error = run(&["pinvou", "code", "checkpoints", "rewind", &id, "9", "--yes"]).unwrap_err();
    assert!(error.to_string().contains("cannot_rewind"), "{error}");

    // Rewinding to a turn without a snapshot fails honestly.
    let error = run(&["pinvou", "code", "checkpoints", "rewind", &id, "1", "--yes"]).unwrap_err();
    assert!(error.to_string().contains("checkpoint_missing"), "{error}");

    let _ = turn2;
}

#[test]
fn checkpoints_refuse_non_native_code_sessions() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("checkpoints-gate");
    // ACP sessions have no native checkpoints (by design in ACP).
    let project =
        std::env::temp_dir().join(format!("pinvou-cli-code-acp-gate-{}", std::process::id()));
    std::fs::create_dir_all(&project).unwrap();
    let id = create_acp_session_fixture(&project);
    let error = run(&["pinvou", "code", "checkpoints", "list", &id]).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("code_checkpoints_requires_native_code_session"),
        "{error}"
    );
    // Invalid checkpoint ids are usage errors.
    let native = create_code_session_fixture(None);
    let error = run(&[
        "pinvou",
        "code",
        "checkpoints",
        "diff",
        &native,
        "../escape",
    ])
    .unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Usage);
}

#[test]
fn providers_round_trip_against_temp_home() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("providers");
    let store_path = home.root.join("acp-providers.json");

    // Zero-state list is valid JSON for every agent.
    let value = run_json(&["pinvou", "code", "providers", "list"]);
    assert_eq!(value["agents"].as_array().unwrap().len(), 3);

    // add persists into acp-providers.json (GUI DTO: id, name, base_url...).
    let value = run_json(&[
        "pinvou",
        "code",
        "providers",
        "add",
        "--agent",
        "codex",
        "--name",
        "Relay A",
        "--base-url",
        "https://api.example.com/v1/",
        "--wire-api",
        "openai",
        "--model",
        "gpt-test",
        "--context-window",
        "128000",
    ]);
    assert_eq!(value["action"], "added");
    let added_id = value["provider"]["id"].as_str().unwrap().to_owned();
    assert!(
        added_id.starts_with("pv-"),
        "code providers add should print a pv_-prefixed GUI id"
    );
    assert_eq!(value["provider"]["name"], "Relay A");
    assert_eq!(value["provider"]["base_url"], "https://api.example.com/v1");
    let raw = std::fs::read_to_string(&store_path).unwrap();
    assert!(raw.contains("Relay A"));
    assert!(
        !raw.to_lowercase().contains("api_key"),
        "the store never persists plaintext keys"
    );

    // list shows the stored entry.
    let value = run_json(&["pinvou", "code", "providers", "list", "--agent", "codex"]);
    let providers = value["providers"]["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["id"], added_id.as_str());
    assert_eq!(providers[0]["hasCredential"], false);
    assert_eq!(
        value["providers"]["currentProviderId"],
        serde_json::Value::Null
    );

    // update renames the entry (merge with existing fields).
    let value = run_json(&[
        "pinvou",
        "code",
        "providers",
        "update",
        &added_id,
        "--agent",
        "codex",
        "--name",
        "Relay B",
    ]);
    assert_eq!(value["action"], "updated");
    assert_eq!(value["provider"]["name"], "Relay B");
    assert_eq!(
        value["provider"]["base_url"], "https://api.example.com/v1",
        "unspecified fields keep their stored values"
    );

    // switch without a stored key fails honestly and leaves the state alone.
    let error = run(&["pinvou", "code", "providers", "switch", "codex", &added_id]).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Failed);
    let raw = std::fs::read_to_string(&store_path).unwrap();
    assert!(
        !raw.contains("current_provider_id"),
        "a failed switch must not persist a current provider"
    );

    // import merges entries (no keys in the fixture file: hermetic).
    let import_file = home.root.join("import.json");
    std::fs::write(
        &import_file,
        r#"[{"name":"Imported","baseUrl":"https://relay.example.com","wireApi":"anthropic"}]"#,
    )
    .unwrap();
    let value = run_json(&[
        "pinvou",
        "code",
        "providers",
        "import",
        "--agent",
        "codex",
        import_file.to_str().unwrap(),
    ]);
    assert_eq!(value["result"]["imported"], 1);
    let raw = std::fs::read_to_string(&store_path).unwrap();
    assert!(raw.contains("Imported"));

    // export writes the JSON payload to --output.
    let export_file = home.root.join("export.json");
    let value = run_json(&[
        "pinvou",
        "code",
        "providers",
        "export",
        "--agent",
        "codex",
        "--output",
        export_file.to_str().unwrap(),
    ]);
    assert_eq!(value["output"], export_file.display().to_string());
    assert_eq!(value["containsPlaintextKeys"], true);
    let exported = std::fs::read_to_string(&export_file).unwrap();
    assert!(exported.contains("Relay B"));

    // remove --yes deletes the entry; the store stays parseable.
    let value = run_json(&[
        "pinvou",
        "code",
        "providers",
        "remove",
        &added_id,
        "--agent",
        "codex",
        "--yes",
    ]);
    assert_eq!(value["removed"], true);
    let value = run_json(&["pinvou", "code", "providers", "list", "--agent", "codex"]);
    let providers = value["providers"]["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1, "only the imported entry remains");
    assert_eq!(providers[0]["name"], "Imported");

    // remove without --yes is refused before any mutation.
    let error = run(&[
        "pinvou",
        "code",
        "providers",
        "remove",
        &added_id,
        "--agent",
        "codex",
    ])
    .unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("--yes"));
}

#[test]
fn providers_switch_official_is_hermetic_without_state() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("providers-official");
    // With no provider ever switched, restoring the official login is a
    // stateless no-op against the isolated CLI home (never the developer's).
    let outcome = run(&["pinvou", "code", "providers", "switch-official", "codex"])
        .expect("switch-official succeeds on a fresh home");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    let value = run_json(&["pinvou", "code", "providers", "switch-official", "claude"]);
    assert_eq!(value["action"], "switched_official");
}

// ── honest host-bound error contracts ───────────────────────────────────────

#[test]
fn engine_bound_paths_return_stable_honest_errors() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("host-bound");
    let id = create_code_session_fixture(None);

    for (arguments, expected_code) in [
        (
            vec![
                "pinvou",
                "code",
                "run",
                "codex",
                "--workspace",
                ".",
                "--prompt",
                "hi",
            ],
            "code_run_requires_product_host",
        ),
        (
            vec!["pinvou", "code", "agents", "install", "codex"],
            "code_install_requires_product_host",
        ),
        (
            vec![
                "pinvou",
                "code",
                "providers",
                "probe",
                "pv-1",
                "--agent",
                "codex",
            ],
            "code_probe_requires_product_host",
        ),
        (
            vec!["pinvou", "code", "permissions", id.as_str()],
            "code_permissions_requires_product_host",
        ),
        (
            vec!["pinvou", "code", "respond", id.as_str(), "req-1", "allow"],
            "code_respond_requires_product_host",
        ),
    ] {
        let error = run(&arguments).expect_err(&arguments.join(" "));
        assert_eq!(error.exit_code(), ExitCode::Failed, "{arguments:?}");
        assert!(
            error.to_string().contains(expected_code),
            "{arguments:?} -> {error}"
        );
    }

    // permissions/respond still gate on session existence first.
    let error = run(&["pinvou", "code", "permissions", "missing-session"]).unwrap_err();
    assert!(
        !error
            .to_string()
            .contains("code_permissions_requires_product_host"),
        "unknown sessions must fail before the host-bound error: {error}"
    );
}

// ── opt-in network/engine paths ─────────────────────────────────────────────

/// Opt-in (requires the real vendor CLI on PATH and network access):
/// `cargo test -p pinvou-cli --test code_contract -- --ignored login_streams_vendor_cli_flow`
/// Spawns the same `codex login`-style command the GUI's login flow runs and
/// verifies the URL/device-code extraction pipeline against live output.
#[test]
#[ignore = "spawns the real vendor CLI and needs network; run explicitly with --ignored"]
fn login_streams_vendor_cli_flow() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("ignored-login");
    let outcome = run(&["pinvou", "code", "login", "codex"]);
    match outcome {
        Ok(outcome) => assert!(outcome.stdout.contains("login codex")),
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("code_login_cli_missing")
                    || message.contains("code_login_failed")
                    || message.contains("code_login_timeout"),
                "unexpected login error: {message}"
            );
        }
    }
}

/// Opt-in: a one-shot ACP turn requires the product host (adapter process +
/// async protocol client). The default assertion pins the stable error.
#[test]
#[ignore = "the real one-shot turn path is product-host-bound; kept to document the gap"]
fn run_one_shot_turn_requires_product_host() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("ignored-run");
    let error = run(&[
        "pinvou",
        "code",
        "run",
        "codex",
        "--workspace",
        ".",
        "--prompt-file",
        "prompt.md",
    ])
    .unwrap_err();
    assert!(error.to_string().contains("code_run_requires_product_host"));
}

/// Re-derives the execution-root lock filename (FNV-1a over the canonical
/// root). This pins the on-disk lock keying: two CLI processes mutating the
/// same project directory through *different* sessions must collide on the
/// same lock file.
fn root_lock_path(canonical_root: &Path) -> PathBuf {
    fn stable(root: &Path) -> String {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in root.to_string_lossy().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }
    std::env::var_os("PINVOU3_HOME")
        .map(PathBuf::from)
        .expect("PINVOU3_HOME sandboxed")
        .join("locks")
        .join(format!("code-root-{}.lock", stable(canonical_root)))
}

/// Opens the lock file; the test keeps the write guard in its own frame (the
/// guard must outlive the command under test or the lock is already free).
fn open_lock(path: &Path) -> fd_lock::RwLock<std::fs::File> {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .unwrap();
    fd_lock::RwLock::new(file)
}

#[test]
fn session_lock_reports_busy_for_every_mutating_command() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("lock-session-busy");
    let id = create_code_session_fixture(None);
    seed_two_turn_transcript(&id);

    // Hold the advisory lock a second CLI process would hold.
    let lock_path = home
        .root
        .join("locks")
        .join(format!("code-session-{id}.lock"));
    let mut lock = open_lock(&lock_path);
    let _guard = lock.try_write().expect("test acquires the contended lock");

    for (arguments, code) in [
        (
            vec!["pinvou", "code", "checkpoints", "rewind", &id, "1", "--yes"],
            "rewind_busy",
        ),
        (
            vec!["pinvou", "code", "checkpoints", "undo", &id, "--yes"],
            "undo_busy",
        ),
        (
            vec!["pinvou", "code", "checkpoints", "diff", &id, "abc"],
            "diff_busy",
        ),
        (
            vec![
                "pinvou",
                "code",
                "workspace",
                "checkout",
                &id,
                "main",
                "--mode",
                "stash",
            ],
            "checkout_busy",
        ),
    ] {
        let error = run(&arguments).expect_err("the held lock must fail the mutation");
        assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
        assert!(
            error.to_string().contains(code),
            "expected {code} in: {error}"
        );
    }
    drop(_guard);
    drop(lock);

    // With the lock released the mutation passes the busy gate (and fails
    // later on the empty rewind state) — proving the busy error came from the
    // lock and that the failed attempts mutated nothing.
    let error = run(&["pinvou", "code", "checkpoints", "undo", &id, "--yes"])
        .expect_err("no rewind record exists");
    assert!(error.to_string().contains("no_undoable_rewind"), "{error}");
    let store = SessionStore::boot().unwrap();
    let session = store.load(&id).unwrap();
    assert_eq!(session.messages.len(), 4, "transcript untouched");
}

#[test]
fn execution_root_lock_blocks_a_second_session_on_the_same_project() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("lock-root-busy");
    let project = home.root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let id_a = create_code_session_fixture(Some(&project));
    let id_b = create_code_session_fixture(Some(&project));
    seed_two_turn_transcript(&id_a);
    seed_two_turn_transcript(&id_b);

    // Session A's rewind holds the project's root lock while it works. In this
    // single-process test the lock file is held externally, which is exactly
    // what a concurrent `pinvou code checkpoints rewind A` looks like to a
    // second process rewinding session B on the same directory.
    let canonical = std::fs::canonicalize(&project).unwrap();
    let mut lock = open_lock(&root_lock_path(&canonical));
    let _guard = lock.try_write().expect("test acquires the contended lock");

    let error = run(&[
        "pinvou",
        "code",
        "checkpoints",
        "rewind",
        &id_b,
        "1",
        "--yes",
    ])
    .expect_err("the held root lock must fail the rewind");
    assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
    assert!(error.to_string().contains("rewind_busy"), "{error}");
    assert!(
        error.to_string().contains("project directory"),
        "the busy error must name the shared project directory: {error}"
    );

    // Session A's own lock is free, so the session gate passes and only the
    // root gate trips — the two locks are independent.
    drop(_guard);
    drop(lock);
    let error = run(&[
        "pinvou",
        "code",
        "checkpoints",
        "rewind",
        &id_b,
        "1",
        "--yes",
    ])
    .expect_err("no checkpoints exist yet");
    assert!(error.to_string().contains("checkpoint_missing"), "{error}");
}

// ---- round-3 fixes: export permissions, login code-source guards ----

#[test]
#[cfg(unix)]
fn providers_export_tightens_permissions_on_an_existing_file() {
    use std::os::unix::fs::PermissionsExt;
    struct KeyVar(Option<std::ffi::OsString>);
    impl Drop for KeyVar {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => unsafe { std::env::set_var("PINVOU_CLI_TEST_EXPORT_KEY", value) },
                None => unsafe { std::env::remove_var("PINVOU_CLI_TEST_EXPORT_KEY") },
            }
        }
    }
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("export-perms");
    let _key = KeyVar(std::env::var_os("PINVOU_CLI_TEST_EXPORT_KEY"));
    unsafe {
        std::env::set_var(
            "PINVOU_CLI_TEST_EXPORT_KEY",
            "sk-test-export-key-1234567890",
        );
    }

    let value = run_json(&[
        "pinvou",
        "code",
        "providers",
        "add",
        "--agent",
        "codex",
        "--name",
        "Exported relay",
        "--base-url",
        "https://api.example.com/v1/",
        "--wire-api",
        "openai",
        "--model",
        "gpt-test",
        "--api-key-env",
        "PINVOU_CLI_TEST_EXPORT_KEY",
    ]);
    assert_eq!(value["action"], "added");

    // A pre-existing world-readable destination (an earlier 0644 export, a
    // shell redirect) must be tightened before plaintext keys land in it —
    // `OpenOptions::mode` alone only applies at create time.
    let target = _home.root.join("pre-existing-export.json");
    std::fs::write(&target, "stale").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
    let value = run_json(&[
        "pinvou",
        "code",
        "providers",
        "export",
        "--agent",
        "codex",
        "--output",
        target.to_str().unwrap(),
    ]);
    assert_eq!(value["containsPlaintextKeys"], true);
    let mode = std::fs::metadata(&target).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "exported key file must be 0600");
}

#[test]
fn code_login_rejects_conflicting_code_sources() {
    let error = parse_args([
        "pinvou",
        "code",
        "login",
        "claude",
        "--code",
        "C",
        "--code-stdin",
    ])
    .expect_err("conflicting code sources must be a usage error");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("only one of"), "{error}");
}

#[test]
fn code_login_rejects_a_missing_code_env_and_non_claude_codes() {
    // The execute path reads environment variables (`--code-env` resolution)
    // while sibling tests set_var under ENV_LOCK, so this test must hold the
    // same lock.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let error = parse_args([
        "pinvou",
        "code",
        "login",
        "claude",
        "--code-env",
        "PINVOU_CLI_TEST_UNSET_CODE_VAR",
    ])
    .expect("parse accepts --code-env");
    let error = pinvou_cli::execute(error)
        .expect_err("an unset env var must fail before spawning anything");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("is not set"), "{error}");

    let error = parse_args(["pinvou", "code", "login", "codex", "--code", "C"])
        .expect("parse accepts --code for any agent");
    let error = pinvou_cli::execute(error)
        .expect_err("only the claude flow consumes an authorization code");
    assert_eq!(error.exit_code(), ExitCode::Usage);
}

/// Writes a scripted `codex` stand-in into a fresh bin directory (argv logged
/// to `seen-args.txt` for spawn assertions) and returns the directory and the
/// script path. `version_snippet` runs for `--version` and `login_snippet`
/// for `login` (each must end in its own exit); any other subcommand exits 1.
#[cfg(unix)]
fn write_fake_codex(label: &str, version_snippet: &str, login_snippet: &str) -> (PathBuf, PathBuf) {
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-code-fake-codex-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    let args_file = bin.join("seen-args.txt");
    use std::os::unix::fs::PermissionsExt as _;
    let script = bin.join("codex");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\nif [ \"$1\" = \"--version\" ]; then {}\nfi\nif [ \"$1\" = \"login\" ]; then {}\nfi\nexit 1\n",
            args_file.display(),
            version_snippet,
            login_snippet
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (bin, script)
}

#[test]
#[cfg(unix)]
fn login_drives_the_real_vendor_spawn_path() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("fake-login");
    // A scripted codex stand-in driven through the real override-resolution
    // and spawn path: the override must pass the version gate (a real
    // `--version` probe), the login child must receive its argv exactly once
    // (the doubled-argv regression was invisible to empty-PATH tests), and
    // the allow-listed login URL must be captured into the JSON result.
    // Cleanup runs through drop guards so a failed assertion cannot leak the
    // PATH override or the scratch bin into sibling tests.
    let (bin, script) = write_fake_codex(
        "login",
        "echo \"codex 1.2.0\"; exit 0",
        "echo \"signin with this URL:\"; echo \"https://auth.openai.com/authorize?o=fake\"; \
         echo \"user code: ABCD-EFGH\"; exit 0",
    );
    let args_file = bin.join("seen-args.txt");
    let _bin = ScratchDir(bin);
    let _codex_path = EnvVarGuard::capture("PINVOU3_CODEX_PATH");
    unsafe { std::env::set_var("PINVOU3_CODEX_PATH", &script) };

    let outcome = run(&["pinvou", "code", "login", "codex", "--output", "json"])
        .expect("login against the fake codex must succeed");
    let value: serde_json::Value =
        serde_json::from_str(&outcome.stdout).expect("single-line JSON login result");
    assert_eq!(
        value["status"], "completed",
        "the login result must report status completed"
    );
    assert_eq!(
        value["login_url"], "https://auth.openai.com/authorize?o=fake",
        "the allow-listed login URL must be captured"
    );

    // The version-gate probe and the post-login authenticated probe are
    // separate spawns; a doubled argv (Command::args appends, so a second
    // .args call would double every argument) would repeat each spawn's
    // arguments: the gate probe would log "--version" twice and the login
    // and probe spawns would log "login" three times instead of two.
    let seen = std::fs::read_to_string(&args_file).unwrap();
    let count = |needle: &str| seen.lines().filter(|line| *line == needle).count();
    assert_eq!(
        count("--version"),
        1,
        "gate probe argv must not double: {seen}"
    );
    assert_eq!(
        count("login"),
        2,
        "login argv must be passed exactly once: {seen}"
    );
    assert_eq!(count("status"), 1, "probe argv must not double: {seen}");
}

/// A vendor CLI that prints the login URL and then exits non-zero must not
/// lose the URL: it is the only actionable part of the failed flow (the
/// timeout path already keeps it).
#[test]
#[cfg(unix)]
fn login_failure_keeps_the_captured_login_url() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("login-failed-url");
    let (bin, script) = write_fake_codex(
        "login-failure",
        "echo \"codex 1.2.0\"; exit 0",
        "echo \"signin with this URL:\"; echo \"https://auth.openai.com/authorize?o=fake\"; exit 3",
    );
    let _bin = ScratchDir(bin);
    let _codex_path = EnvVarGuard::capture("PINVOU3_CODEX_PATH");
    unsafe { std::env::set_var("PINVOU3_CODEX_PATH", &script) };

    let error = run(&["pinvou", "code", "login", "codex"]).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Failed);
    let message = error.to_string();
    assert!(
        message.contains("code_login_failed"),
        "the login failure must carry the code_login_failed prefix"
    );
    assert!(
        message.contains("https://auth.openai.com/authorize?o=fake"),
        "the captured login URL must survive the failure path: {message}"
    );
}

/// A vendor CLI whose `--version` probe fails is a distinct state from a
/// genuinely too-old version: the JSON must say `version_probe_failed` and
/// the install gate must stay closed.
#[test]
#[cfg(unix)]
fn version_probe_failure_is_reported_and_fails_the_gate() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("version-probe-failed");
    let (bin, _script) = write_fake_codex("probe-failure", "exit 7", "exit 1");
    // Resolve through PATH: the override route is rejected by the gate
    // itself before a probe failure could ever be reported.
    let _path = EnvVarGuard::capture("PATH");
    unsafe { std::env::set_var("PATH", &bin) };
    let _bin = ScratchDir(bin);

    let value = run_json(&["pinvou", "code", "agents", "status", "codex"]);
    assert_eq!(value["cli_found"], true);
    assert_eq!(value["version"], serde_json::Value::Null);
    assert_eq!(value["version_probe_failed"], true);
    assert_eq!(value["version_supported"], false);
    assert_eq!(value["installed"], false);
}

/// The claude login must survive a child that floods its own stdout around
/// consuming stdin: the code write runs on its own thread and the drains own
/// the pipes from the start, so no stdin/stdout interleaving can park the
/// flow past the deadline. The fake answers `auth login` (the claude argv),
/// reads a stdin line, emits far more than the OS pipe buffer, then exits —
/// the watchdog assertion below trips if any stdin/stdout coordination
/// regresses into a hang. (It cannot discriminate the historical
/// write-before-drains ordering on unix — a ≤4097-byte write always fits the
/// pipe buffer — which is why that ordering was eliminated structurally: the
/// write is detached, so even a blocked write cannot stall the deadline
/// loop, and the loop kills the child.)
#[test]
#[cfg(unix)]
fn claude_login_completes_when_the_child_floods_stdout_around_stdin() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("claude-login-stdin-flood");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-code-fake-claude-stdin-flood-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    let script = bin.join("claude");
    std::fs::write(
        &script,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"2.1.163 (Claude Code)\"; exit 0; fi\nif [ \"$1\" = \"auth\" ] && [ \"$2\" = \"login\" ]; then\n  read -r line\n  awk 'BEGIN{for(i=0;i<20000;i++) printf \"%s\", \"012345678901234567890123456789\"}'\n  echo\n  echo \"flood done\"\n  exit 0\nfi\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _bin = ScratchDir(bin);
    let _claude_path = EnvVarGuard::capture("PINVOU3_CLAUDE_CLI_PATH");
    unsafe { std::env::set_var("PINVOU3_CLAUDE_CLI_PATH", &script) };

    // The claude flow consumes `--code` (written to stdin post-drain); the
    // post-login auth probe fails against the fake, which is fine — any
    // terminal outcome proves the child exited instead of deadlocking.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = parse_args([
            "pinvou",
            "code",
            "login",
            "claude",
            "--code",
            "SRCRT-LOGIN-CODE-42",
        ])
        .and_then(|parsed| pinvou_cli::execute(parsed));
        let _ = tx.send(result);
    });
    let result = rx
        .recv_timeout(std::time::Duration::from_secs(90))
        .expect("claude login must complete well under the 600 s deadline — a hang here is an stdin/stdout coordination regression");
    // The flooded transcript must not carry the raw code anywhere a harness
    // could re-read (the JSON outcome is the script-visible part).
    match result {
        Ok(outcome) => assert!(
            !outcome.stdout.contains("SRCRT-LOGIN-CODE-42"),
            "the authorization code must not surface in the report: {}",
            outcome.stdout
        ),
        Err(error) => assert!(
            !error.to_string().contains("SRCRT-LOGIN-CODE-42"),
            "the authorization code must not surface in the error: {error}"
        ),
    }
}
