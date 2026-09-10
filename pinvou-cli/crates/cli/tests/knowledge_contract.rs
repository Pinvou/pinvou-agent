//! Contract tests for the `knowledge` family (`crates/cli/src/knowledge.rs`).
//!
//! Parse-level tests cover every subcommand plus the invalid shapes that must
//! map to exit-code 2 usage errors. Execute-level tests run against a temp
//! `PINVOU3_HOME` (serialized through ENV_LOCK, following cli_contract.rs).
//! The session-mount surface runs against a real `SessionStore` fixture; the
//! KnowledgeService-backed surface is behind the `pub(crate)` boundary
//! documented in knowledge.rs, so those tests freeze the stable
//! `knowledge_backend_unavailable` contract instead of touching data. No test
//! here reaches the network or a model (AGENTS.md rule); the model-download
//! path additionally needs the desktop model host and stays opt-in only.

use pinvou_cli::{ExitCode, execute, parse_args};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Serialises tests that mutate the process-global `PINVOU3_HOME` environment
/// variable, preventing data races when the parallel test runner executes them
/// concurrently.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct TempHome {
    root: PathBuf,
    previous: Option<std::ffi::OsString>,
}

impl TempHome {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pinvou-cli-knowledge-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let previous = std::env::var_os("PINVOU3_HOME");
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        Self { root, previous }
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn run_ok(arguments: &[&str]) -> String {
    let parsed = parse_args(arguments.to_vec()).expect("valid knowledge command");
    let outcome = execute(parsed).expect("successful knowledge command");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    outcome.stdout
}

fn run_json(arguments: &[&str]) -> serde_json::Value {
    let mut owned = arguments.to_vec();
    owned.push("--output");
    owned.push("json");
    let stdout = run_ok(&owned);
    assert!(!stdout.contains('\n'), "json output must stay single-line");
    serde_json::from_str(&stdout).expect("single-line json object")
}

fn execute_error(arguments: &[&str]) -> pinvou_cli::CliError {
    let parsed = parse_args(arguments.to_vec()).expect("valid knowledge command");
    execute(parsed).expect_err("expected execute failure")
}

/// Usage errors may surface from `parse` or from `execute`; both must carry
/// the exit-code 2 usage marker.
fn assert_usage(arguments: &[&str]) {
    let error = match parse_args(arguments.to_vec()) {
        Err(error) => error,
        Ok(parsed) => execute(parsed).expect_err("expected usage error"),
    };
    assert_eq!(error.exit_code(), ExitCode::Usage, "{error}");
}

/// The family command types are private to the crate, so parse-level
/// assertions freeze the derived Debug shape of the parsed command (same
/// convention as memory_contract.rs).
fn parsed_debug(arguments: &[&str]) -> String {
    format!(
        "{:?}",
        parse_args(arguments.to_vec())
            .expect("valid knowledge command")
            .command()
    )
}

/// Creates a real session in the sandboxed home so the mount surface (which
/// requires an existing session, like the GUI's open-session commands) has a
/// valid target.
fn create_session(home: &TempHome) -> String {
    let store = pinvou3_lib::features::sessions::SessionStore::boot().expect("boot session store");
    let workspace = home.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let session = store
        .create_new("test-model".to_owned(), None, workspace)
        .expect("create fixture session");
    session.metadata.id
}

// ---- parse-level coverage ----

#[test]
fn knowledge_parses_every_subcommand() {
    let cases: Vec<(&[&str], String)> = vec![
        (&["pinvou", "knowledge", "scan", "start"], "Knowledge(ScanStart { root: None })".into()),
        (
            &["pinvou", "knowledge", "scan", "start", "--root", "/tmp/docs"],
            r#"Knowledge(ScanStart { root: Some("/tmp/docs") })"#.into(),
        ),
        (&["pinvou", "knowledge", "scan", "status"], "Knowledge(ScanStatus)".into()),
        (&["pinvou", "knowledge", "scan", "cancel"], "Knowledge(ScanCancel)".into()),
        (&["pinvou", "knowledge", "stats"], "Knowledge(Stats)".into()),
        (&["pinvou", "knowledge", "type-counts"], "Knowledge(TypeCounts)".into()),
        (&["pinvou", "knowledge", "collections", "list"], "Knowledge(CollectionsList)".into()),
        (
            &["pinvou", "knowledge", "collections", "create", "--name", "papers"],
            r#"Knowledge(CollectionsCreate { name: "papers", category: None, description: None })"#.into(),
        ),
        (
            &[
                "pinvou",
                "knowledge",
                "collections",
                "create",
                "--name",
                "papers",
                "--category",
                "research",
                "--description",
                "deep work",
            ],
            r#"Knowledge(CollectionsCreate { name: "papers", category: Some("research"), description: Some("deep work") })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "collections", "update", "3", "--name", "renamed"],
            r#"Knowledge(CollectionsUpdate { id: 3, name: Some("renamed"), category: None, description: None })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "collections", "update", "3", "--description", "d"],
            r#"Knowledge(CollectionsUpdate { id: 3, name: None, category: None, description: Some("d") })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "collections", "delete", "5", "--yes"],
            r#"Knowledge(CollectionsDelete { id: 5, yes: true })"#.into(),
        ),
        // --yes stays parseable-but-unconfirmed: the exit-2 contract is
        // enforced by support::require_yes at execute time.
        (
            &["pinvou", "knowledge", "collections", "delete", "5"],
            r#"Knowledge(CollectionsDelete { id: 5, yes: false })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "collections", "add-sources", "7", "/a", "/b"],
            r#"Knowledge(CollectionsAddSources { id: 7, paths: ["/a", "/b"] })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "documents", "5"],
            r#"Knowledge(Documents { collection_id: 5, limit: None })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "documents", "5", "--limit", "10"],
            r#"Knowledge(Documents { collection_id: 5, limit: Some(10) })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "documents", "remove", "9", "--yes"],
            r#"Knowledge(DocumentsRemove { doc_id: 9, yes: true })"#.into(),
        ),
        (&["pinvou", "knowledge", "index", "status"], r#"Knowledge(IndexStatus { job_id: None })"#.into()),
        (
            &["pinvou", "knowledge", "index", "status", "job-1"],
            r#"Knowledge(IndexStatus { job_id: Some("job-1") })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "index", "cancel", "job-1"],
            r#"Knowledge(IndexCancel { job_id: "job-1" })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "index", "resume", "job-1"],
            r#"Knowledge(IndexResume { job_id: "job-1" })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "index", "retry", "job-1", "12"],
            r#"Knowledge(IndexRetry { job_id: "job-1", item_id: 12 })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "index", "failed", "job-1"],
            r#"Knowledge(IndexFailed { job_id: "job-1", offset: 0, limit: None })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "index", "failed", "job-1", "--offset", "5", "--limit", "10"],
            r#"Knowledge(IndexFailed { job_id: "job-1", offset: 5, limit: Some(10) })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "search", "hello", "world"],
            r#"Knowledge(Search { query: "hello world", limit: None, ext: None, after: None, before: None })"#.into(),
        ),
        (
            &[
                "pinvou",
                "knowledge",
                "search",
                "hello",
                "--limit",
                "5",
                "--ext",
                "pdf",
                "--after",
                "2026-01-01",
                "--before",
                "2026-09-01",
            ],
            r#"Knowledge(Search { query: "hello", limit: Some(5), ext: Some("pdf"), after: Some("2026-01-01"), before: Some("2026-09-01") })"#.into(),
        ),
        (&["pinvou", "knowledge", "model", "status"], "Knowledge(ModelStatus)".into()),
        (&["pinvou", "knowledge", "model", "download"], "Knowledge(ModelDownload)".into()),
        (&["pinvou", "knowledge", "model", "cancel"], "Knowledge(ModelCancel)".into()),
        (
            &["pinvou", "knowledge", "mounts", "s-1"],
            r#"Knowledge(Mounts { session_id: "s-1" })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "mount", "s-1", "42"],
            r#"Knowledge(Mount { session_id: "s-1", collection_id: 42 })"#.into(),
        ),
        (
            &["pinvou", "knowledge", "unmount", "s-1", "42"],
            r#"Knowledge(Unmount { session_id: "s-1", collection_id: 42 })"#.into(),
        ),
        (&["pinvou", "knowledge", "remote", "connections"], "Knowledge(RemoteConnections)".into()),
        (
            &["pinvou", "knowledge", "remote", "probe", "https://host:3210"],
            r#"Knowledge(RemoteProbe { url: "https://host:3210" })"#.into(),
        ),
        (&["pinvou", "knowledge", "remote", "collections"], "Knowledge(RemoteCollections)".into()),
        (
            &["pinvou", "knowledge", "remote", "search", "papers", "embeddings", "test"],
            r#"Knowledge(RemoteSearch { collection: "papers", query: "embeddings test" })"#.into(),
        ),
        (&["pinvou", "knowledge", "host", "status"], "Knowledge(HostStatus)".into()),
    ];
    for (arguments, expected) in cases {
        assert_eq!(parsed_debug(arguments), expected, "{arguments:?}");
    }
}

#[test]
fn knowledge_rejects_invalid_usage_with_exit_code_two() {
    let invalid = [
        vec!["pinvou", "knowledge"],
        vec!["pinvou", "knowledge", "bogus"],
        vec!["pinvou", "knowledge", "scan"],
        vec!["pinvou", "knowledge", "scan", "bogus"],
        vec!["pinvou", "knowledge", "scan", "start", "--bogus", "x"],
        vec!["pinvou", "knowledge", "scan", "start", "--root"],
        vec!["pinvou", "knowledge", "scan", "status", "--extra"],
        vec!["pinvou", "knowledge", "scan", "cancel", "--extra"],
        vec!["pinvou", "knowledge", "stats", "--extra"],
        vec!["pinvou", "knowledge", "type-counts", "--extra"],
        vec!["pinvou", "knowledge", "collections"],
        vec!["pinvou", "knowledge", "collections", "bogus"],
        vec!["pinvou", "knowledge", "collections", "list", "--extra"],
        vec!["pinvou", "knowledge", "collections", "create"],
        vec!["pinvou", "knowledge", "collections", "create", "--name"],
        vec![
            "pinvou",
            "knowledge",
            "collections",
            "create",
            "--name",
            "n",
            "--bogus",
            "x",
        ],
        vec!["pinvou", "knowledge", "collections", "update"],
        vec!["pinvou", "knowledge", "collections", "update", "3"],
        vec![
            "pinvou",
            "knowledge",
            "collections",
            "update",
            "abc",
            "--name",
            "n",
        ],
        vec![
            "pinvou",
            "knowledge",
            "collections",
            "update",
            "3",
            "--name",
            "n",
            "--extra",
        ],
        vec!["pinvou", "knowledge", "collections", "delete"],
        vec![
            "pinvou",
            "knowledge",
            "collections",
            "delete",
            "5",
            "--nope",
        ],
        vec!["pinvou", "knowledge", "collections", "add-sources"],
        vec!["pinvou", "knowledge", "collections", "add-sources", "7"],
        vec![
            "pinvou",
            "knowledge",
            "collections",
            "add-sources",
            "abc",
            "/a",
        ],
        vec![
            "pinvou",
            "knowledge",
            "collections",
            "add-sources",
            "7",
            "/a",
            "--limit",
            "3",
        ],
        vec!["pinvou", "knowledge", "documents"],
        vec!["pinvou", "knowledge", "documents", "abc"],
        vec!["pinvou", "knowledge", "documents", "5", "--limit"],
        vec!["pinvou", "knowledge", "documents", "5", "--limit", "0"],
        vec!["pinvou", "knowledge", "documents", "5", "--limit", "x"],
        vec!["pinvou", "knowledge", "documents", "remove"],
        vec!["pinvou", "knowledge", "documents", "remove", "9", "--nope"],
        vec!["pinvou", "knowledge", "documents", "remove", "abc", "--yes"],
        vec!["pinvou", "knowledge", "index"],
        vec!["pinvou", "knowledge", "index", "bogus"],
        vec!["pinvou", "knowledge", "index", "cancel"],
        vec!["pinvou", "knowledge", "index", "cancel", "j", "--extra"],
        vec!["pinvou", "knowledge", "index", "resume", "j", "extra"],
        vec!["pinvou", "knowledge", "index", "retry"],
        vec!["pinvou", "knowledge", "index", "retry", "job-1"],
        vec!["pinvou", "knowledge", "index", "retry", "job-1", "abc"],
        vec![
            "pinvou",
            "knowledge",
            "index",
            "retry",
            "job-1",
            "1",
            "extra",
        ],
        vec!["pinvou", "knowledge", "index", "failed"],
        vec!["pinvou", "knowledge", "index", "failed", "j", "--offset"],
        vec![
            "pinvou",
            "knowledge",
            "index",
            "failed",
            "j",
            "--offset",
            "-1",
        ],
        vec![
            "pinvou",
            "knowledge",
            "index",
            "failed",
            "j",
            "--limit",
            "0",
        ],
        vec!["pinvou", "knowledge", "search"],
        vec!["pinvou", "knowledge", "search", "--limit", "5"],
        vec!["pinvou", "knowledge", "search", "q", "--bogus", "v"],
        vec!["pinvou", "knowledge", "search", "q", "--limit", "0"],
        vec!["pinvou", "knowledge", "model"],
        vec!["pinvou", "knowledge", "model", "bogus"],
        vec!["pinvou", "knowledge", "model", "status", "--extra"],
        vec!["pinvou", "knowledge", "mounts"],
        vec!["pinvou", "knowledge", "mounts", "s-1", "--extra"],
        vec!["pinvou", "knowledge", "mount"],
        vec!["pinvou", "knowledge", "mount", "s-1"],
        vec!["pinvou", "knowledge", "mount", "s-1", "abc"],
        vec!["pinvou", "knowledge", "mount", "s-1", "42", "--extra"],
        vec!["pinvou", "knowledge", "unmount", "s-1"],
        vec!["pinvou", "knowledge", "remote"],
        vec!["pinvou", "knowledge", "remote", "bogus"],
        vec!["pinvou", "knowledge", "remote", "connections", "--extra"],
        vec!["pinvou", "knowledge", "remote", "probe"],
        vec!["pinvou", "knowledge", "remote", "collections", "--extra"],
        vec!["pinvou", "knowledge", "remote", "search"],
        vec!["pinvou", "knowledge", "remote", "search", "coll"],
        vec![
            "pinvou",
            "knowledge",
            "remote",
            "search",
            "coll",
            "q",
            "--extra",
        ],
        vec!["pinvou", "knowledge", "host"],
        vec!["pinvou", "knowledge", "host", "bogus"],
        vec!["pinvou", "knowledge", "host", "status", "--extra"],
    ];
    for arguments in invalid {
        assert_usage(&arguments);
    }
}

// ---- execute-level coverage ----

#[test]
fn destructive_subcommands_require_yes_before_anything_else() {
    for arguments in [
        vec!["pinvou", "knowledge", "collections", "delete", "5"],
        vec!["pinvou", "knowledge", "documents", "remove", "9"],
    ] {
        let error = execute_error(&arguments);
        assert_eq!(error.exit_code(), ExitCode::Usage, "{arguments:?}");
        assert!(error.to_string().contains("--yes"), "{arguments:?}");
    }
}

#[test]
fn mounts_reject_unknown_sessions() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = TempHome::new("mounts-unknown");
    let error = execute_error(&["pinvou", "knowledge", "mounts", "missing-session"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("session not found"), "{error}");
}

#[test]
fn mount_surfaces_the_gui_gate_errors_verbatim() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("mount-gate");
    let session_id = create_session(&home);

    // GUI gate step 1: invalid collection id, exact GUI message.
    let error = execute_error(&["pinvou", "knowledge", "mount", &session_id, "0"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert_eq!(error.to_string(), "知识集 id 无效");

    // GUI gate step 2: semantic readiness. A CLI process never has the
    // embedding model loaded, so the GUI's not-ready message is the faithful
    // outcome (verbatim, per the GUI gate in app/commands/knowledge.rs).
    let error = execute_error(&["pinvou", "knowledge", "mount", &session_id, "7"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert_eq!(error.to_string(), "embedding 模型未就绪,知识库暂不可用");
}

#[test]
fn mounts_and_unmount_round_trip_on_an_existing_session() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("mount-roundtrip");
    let session_id = create_session(&home);

    let snapshot = run_json(&["pinvou", "knowledge", "mounts", &session_id]);
    assert_eq!(snapshot["session_id"], serde_json::json!(session_id));
    assert_eq!(snapshot["revision"], serde_json::json!(0));
    assert_eq!(snapshot["collections"], serde_json::json!([]));

    // Removal of an absent mount is a no-op that still reports the fresh,
    // revision-bumped snapshot (GUI session_remove_mounted_collection
    // semantics; the GUI store keeps mount mutations in process memory and
    // persists them through other mode-state saves, so a later CLI invocation
    // starts from the persisted state again).
    let snapshot = run_json(&["pinvou", "knowledge", "unmount", &session_id, "42"]);
    assert_eq!(snapshot["session_id"], serde_json::json!(session_id));
    assert_eq!(snapshot["collections"], serde_json::json!([]));
    let revision = snapshot["revision"].as_u64().expect("revision");
    assert!(revision >= 1, "revision must bump after a mutation");
    let snapshot = run_json(&["pinvou", "knowledge", "mounts", &session_id]);
    assert_eq!(snapshot["collections"], serde_json::json!([]));
}

#[test]
fn service_backed_subcommands_report_the_stable_unavailable_code() {
    let cases = [
        vec!["pinvou", "knowledge", "scan", "start"],
        vec!["pinvou", "knowledge", "scan", "start", "--root", "/tmp"],
        vec!["pinvou", "knowledge", "scan", "status"],
        vec!["pinvou", "knowledge", "scan", "cancel"],
        vec!["pinvou", "knowledge", "stats"],
        vec!["pinvou", "knowledge", "type-counts"],
        vec!["pinvou", "knowledge", "collections", "list"],
        vec![
            "pinvou",
            "knowledge",
            "collections",
            "create",
            "--name",
            "n",
        ],
        vec![
            "pinvou",
            "knowledge",
            "collections",
            "update",
            "1",
            "--name",
            "n",
        ],
        vec!["pinvou", "knowledge", "collections", "delete", "1", "--yes"],
        vec![
            "pinvou",
            "knowledge",
            "collections",
            "add-sources",
            "1",
            "/tmp",
        ],
        vec!["pinvou", "knowledge", "documents", "1"],
        vec!["pinvou", "knowledge", "documents", "remove", "1", "--yes"],
        vec!["pinvou", "knowledge", "index", "status"],
        vec!["pinvou", "knowledge", "index", "cancel", "j"],
        vec!["pinvou", "knowledge", "index", "resume", "j"],
        vec!["pinvou", "knowledge", "index", "retry", "j", "1"],
        vec!["pinvou", "knowledge", "index", "failed", "j"],
        vec!["pinvou", "knowledge", "search", "hello"],
        vec!["pinvou", "knowledge", "model", "status"],
        vec!["pinvou", "knowledge", "model", "download"],
        vec!["pinvou", "knowledge", "model", "cancel"],
    ];
    for arguments in cases {
        let error = execute_error(&arguments);
        assert_eq!(error.exit_code(), ExitCode::Failed, "{arguments:?}");
        assert!(
            error
                .to_string()
                .starts_with("knowledge_backend_unavailable"),
            "{arguments:?}: {error}"
        );
    }
}

#[test]
fn remote_and_host_surfaces_name_their_own_boundaries() {
    for arguments in [
        vec!["pinvou", "knowledge", "remote", "connections"],
        vec![
            "pinvou",
            "knowledge",
            "remote",
            "probe",
            "https://host:3210",
        ],
        vec!["pinvou", "knowledge", "remote", "collections"],
        vec!["pinvou", "knowledge", "remote", "search", "papers", "query"],
    ] {
        let error = execute_error(&arguments);
        assert_eq!(error.exit_code(), ExitCode::Failed, "{arguments:?}");
        assert!(
            error
                .to_string()
                .starts_with("remote_knowledge_backend_unavailable"),
            "{arguments:?}: {error}"
        );
    }
    let error = execute_error(&["pinvou", "knowledge", "host", "status"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error
            .to_string()
            .starts_with("shared_knowledge_host_backend_unavailable"),
        "{error}"
    );
}
