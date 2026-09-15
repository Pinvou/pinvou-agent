//! Contract tests for the `knowledge` family (`crates/cli/src/knowledge.rs`).
//!
//! Parse-level tests cover every subcommand plus the invalid shapes that must
//! map to exit-code 2 usage errors. Execute-level tests run against a temp
//! `PINVOU3_HOME` (serialized through ENV_LOCK, following cli_contract.rs):
//! the KnowledgeService round-trips (collections CRUD, document listing, the
//! scan trio, add-sources indexing, L0 stats/type-counts/search, model
//! status) are offline — an empty `index.db` needs no model and no network,
//! because the import thread skips embedder loading when the model is not
//! installed and degrades to full-text. The L0 search round-trip seeds rows
//! through a completed `scan start` (still offline) and asserts the NL-rule
//! merge ("上周的 pdf" → ext + mtime filter + residual text) indirectly.
//! The session-mount surface refuses honestly (per-process app memory); the
//! remaining families run against real stores. A real `SessionStore`
//! fixture and freezes the GUI's verbatim gate errors. Paths that need the
//! network, a model download, a configured remote server or the windowless
//! product host stay behind `#[ignore]` with their opt-in command named
//! (AGENTS.md rule: never touch the network in default tests); `model
//! download` keeps its stable `knowledge_backend_unavailable` code with the
//! documented upstream blocker.

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
            "-3",
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
        vec!["pinvou", "knowledge", "documents", "-3"],
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
        vec!["pinvou", "knowledge", "unmount", "s-1", "-3"],
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
fn mount_family_refuses_honestly_as_product_host_bound() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("mount-gate");
    let session_id = create_session(&home);

    // Mounted collections live in the desktop app's per-process memory and
    // are deliberately not persisted, so every CLI mount-surface command
    // refuses instead of reporting success that changes nothing.
    for arguments in [
        vec!["pinvou", "knowledge", "mounts", &session_id],
        vec!["pinvou", "knowledge", "mount", &session_id, "7"],
        vec!["pinvou", "knowledge", "unmount", &session_id, "42"],
    ] {
        let error = execute_error(&arguments);
        assert_eq!(error.exit_code(), ExitCode::Failed, "{arguments:?}");
        let message = error.to_string();
        assert!(
            message.contains("_requires_product_host") && message.contains("process memory"),
            "{arguments:?}: {message}"
        );
    }
}

/// `model download` keeps its stable unavailable code: the orchestration and
/// its cancel/downloading state live behind model_download.rs privates (see
/// the CLI module docs for the exact blocker). stats/type-counts/search and
/// remote probe are real now and covered by the round-trip/opt-in tests.
#[test]
fn model_download_keeps_its_stable_unavailable_code() {
    let error = execute_error(&["pinvou", "knowledge", "model", "download"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error
            .to_string()
            .starts_with("knowledge_backend_unavailable"),
        "{error}"
    );
    assert!(error.to_string().contains("model_download.rs"), "{error}");
}

#[test]
fn stats_type_counts_and_search_answer_zero_state_offline() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = TempHome::new("l0-zero");

    let stats = run_json(&["pinvou", "knowledge", "stats"]);
    assert_eq!(stats["totalFiles"], serde_json::json!(0));
    assert_eq!(stats["totalBytes"], serde_json::json!(0));
    assert_eq!(stats["duplicateGroups"], serde_json::json!(0));

    let counts = run_json(&["pinvou", "knowledge", "type-counts"]);
    assert_eq!(counts["typeCounts"], serde_json::json!([]));

    let hits = run_json(&["pinvou", "knowledge", "search", "hello"]);
    assert_eq!(hits["query"], serde_json::json!("hello"));
    assert_eq!(hits["hits"], serde_json::json!([]));
}

// ---- execute-level coverage: real hermetic round-trips ----

#[test]
fn collections_crud_round_trip_on_a_temp_home() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = TempHome::new("collections-crud");

    let first = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "papers",
    ]);
    let first_id = first["id"].as_i64().expect("created collection id");
    let second = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "books",
        "--category",
        "reading",
        "--description",
        "long form",
    ]);
    let second_id = second["id"].as_i64().expect("created collection id");

    let listed = run_json(&["pinvou", "knowledge", "collections", "list"]);
    let collections = listed["collections"].as_array().expect("collections array");
    assert_eq!(collections.len(), 2);
    assert!(
        collections
            .iter()
            .any(|collection| collection["name"] == serde_json::json!("papers"))
    );

    let updated = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "update",
        &first_id.to_string(),
        "--name",
        "renamed",
    ]);
    assert_eq!(updated["name"], serde_json::json!("renamed"));
    // The unspecified flags keep the stored values (GUI replace semantics
    // with a CLI-side merge).
    assert_eq!(updated["id"], serde_json::json!(first_id));

    let error = execute_error(&[
        "pinvou",
        "knowledge",
        "collections",
        "update",
        "999",
        "--name",
        "ghost",
    ]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("not found"), "{error}");

    run_ok(&[
        "pinvou",
        "knowledge",
        "collections",
        "delete",
        &second_id.to_string(),
        "--yes",
    ]);
    let listed = run_json(&["pinvou", "knowledge", "collections", "list"]);
    let collections = listed["collections"].as_array().expect("collections array");
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0]["id"], serde_json::json!(first_id));
}

#[test]
fn documents_zero_state_and_remove_no_op() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = TempHome::new("documents-zero");

    let created = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "empty",
    ]);
    let id = created["id"].as_i64().expect("created collection id");

    let documents = run_json(&["pinvou", "knowledge", "documents", &id.to_string()]);
    assert_eq!(documents["collection_id"], serde_json::json!(id));
    assert_eq!(documents["documents"], serde_json::json!([]));

    // Unlike the GUI's silent no-op delete, removing an unknown document
    // refuses with exit 1 like every other unknown-id command in the CLI.
    let error = execute_error(&["pinvou", "knowledge", "documents", "remove", "123", "--yes"]);
    assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
    assert!(
        error.to_string().contains("knowledge_document_not_found"),
        "{error}"
    );
}

/// Index jobs are DB-persisted and polled (`kb_index_status`), so the
/// background import thread that `add-sources` spawns is observable across
/// fresh CLI invocations. The embedder load is skipped (model not installed
/// in the sandbox home), keeping this fully offline.
#[test]
fn add_sources_indexes_a_text_file_end_to_end() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("index-roundtrip");

    let created = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "notes",
    ]);
    let id = created["id"].as_i64().expect("created collection id");

    let docs = home.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    let source = docs.join("hello.txt");
    std::fs::write(
        &source,
        "Pinvou knowledge stores local notes about embeddings.",
    )
    .unwrap();

    let started = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "add-sources",
        &id.to_string(),
        source.to_str().unwrap(),
    ]);
    assert_eq!(started["collectionId"], serde_json::json!(id));
    let job_id = started["jobId"]
        .as_str()
        .expect("started job id")
        .to_owned();

    // The import runs on the `add-sources` invocation's background thread —
    // here the test process. Let it finish before the first fresh service
    // boot: every new invocation runs the GUI's startup recovery, which
    // recovers an in-flight job to `interrupted` (crash semantics), so
    // polling mid-flight would disturb the live import.
    std::thread::sleep(std::time::Duration::from_secs(1));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let state = loop {
        let state = run_json(&["pinvou", "knowledge", "index", "status"]);
        let phase = state["phase"].as_str().unwrap_or_default().to_owned();
        if phase == "done" || phase == "done_with_errors" {
            break state;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "index job did not finish in time; last state: {state}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(state["jobId"], serde_json::json!(job_id));
    assert!(
        state["completed"].as_u64().unwrap_or(0) >= 1,
        "expected one completed item: {state}"
    );

    let documents = run_json(&["pinvou", "knowledge", "documents", &id.to_string()]);
    let documents = documents["documents"].as_array().expect("documents array");
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0]["name"], serde_json::json!("hello.txt"));
    assert_eq!(documents[0]["parseStatus"], serde_json::json!("parsed"));

    let failed = run_ok(&["pinvou", "knowledge", "index", "failed", &job_id]);
    assert!(
        failed.contains("no failed files"),
        "index failed should report no failed files"
    );

    // Per-job live state is not addressable headlessly: an unknown/latest
    // mismatch names the boundary instead of silently returning another job.
    let error = execute_error(&["pinvou", "knowledge", "index", "status", "other-job"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("per-job state"), "{error}");

    // Cancel targets the active/latest job; on a finished job it is a
    // signal-only no-op that still succeeds.
    run_ok(&["pinvou", "knowledge", "index", "cancel", &job_id]);

    // Resuming an unknown job surfaces the upstream error verbatim.
    let error = execute_error(&["pinvou", "knowledge", "index", "resume", "bogus-job"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
}

/// A second `add-sources` behind an unfinished job for the SAME collection
/// must refuse instead of reporting the stale job as success: the one-shot
/// CLI process exits right after printing, which leaves its import job
/// mid-flight, and the next invocation's startup recovery turns that job
/// `interrupted` (= resumable) — exactly the upstream short-circuit that
/// silently drops the newly requested sources. Driven through the real
/// binary because the in-process helpers keep the import thread alive and
/// would complete the first job instead of stranding it.
#[test]
fn add_sources_refuses_to_drop_sources_behind_a_resumable_job() {
    let bin = env!("CARGO_BIN_EXE_pinvou");
    let root = std::env::temp_dir().join(format!(
        "pinvou-cli-knowledge-resumable-guard-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let run = |args: &[&str]| {
        let mut command = std::process::Command::new(bin);
        command
            .args(args)
            .env("PINVOU3_HOME", &root)
            .env("PINVOU_NO_COLOR", "1");
        command.output().expect("binary runs")
    };
    let created = run(&[
        "knowledge",
        "collections",
        "create",
        "--name",
        "guarded",
        "--output",
        "json",
    ]);
    assert!(created.status.success(), "collection create must succeed");
    let created: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let id = created["id"].as_i64().expect("created collection id");

    let first = root.join("first.txt");
    std::fs::write(
        &first,
        "Pinvou knowledge guards the first enqueued source.".repeat(64),
    )
    .unwrap();
    let second = root.join("second.txt");
    std::fs::write(
        &second,
        "Pinvou knowledge must not silently drop this source.",
    )
    .unwrap();

    let started = run(&[
        "knowledge",
        "collections",
        "add-sources",
        &id.to_string(),
        first.to_str().unwrap(),
        "--output",
        "json",
    ]);
    assert!(started.status.success(), "first add-sources must succeed");

    // The one-shot process of the first add-sources killed its import
    // thread mid-flight; this invocation's recovery marks the job
    // resumable, so the second batch must be refused loudly — the stale
    // "success" used to hide files that were never enqueued.
    let blocked = run(&[
        "knowledge",
        "collections",
        "add-sources",
        &id.to_string(),
        second.to_str().unwrap(),
    ]);
    assert!(!blocked.status.success(), "second add-sources must fail");
    let stderr = String::from_utf8_lossy(&blocked.stderr);
    assert!(
        stderr.contains("NOT enqueued"),
        "second add-sources must name the sources it did not enqueue"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// `index failed` for an unknown job must not leak the raw rusqlite driver
/// message ("Query returned no rows") that upstream uses to signal a missing
/// job id: the CLI names the cause, like `index cancel` does.
#[test]
fn index_failed_names_an_unknown_job() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = TempHome::new("index-failed-unknown");
    let error = execute_error(&["pinvou", "knowledge", "index", "failed", "missing-job"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("knowledge_index_job_not_found"),
        "{error}"
    );
}

/// Green-path `index resume`: a job stranded by a dead one-shot
/// `add-sources` process is re-armed in a fresh process — that child's boot
/// recovery flips the stranded job to `interrupted`, which is exactly the
/// state `resume` requires, and the resume re-marks it running. Polled
/// read-only (never recovering) through `index status`. Note: the resume
/// child kills its own import thread at exit like every one-shot invocation,
/// so the deterministic assertion is the interrupted → running progression;
/// when the child's import does beat process exit, the document-level
/// `parsed` end state is asserted too.
#[test]
fn index_resume_rearms_a_job_stranded_by_a_dead_process() {
    let bin = env!("CARGO_BIN_EXE_pinvou");
    let root = std::env::temp_dir().join(format!(
        "pinvou-cli-knowledge-resume-roundtrip-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let run = |args: &[&str]| {
        let mut command = std::process::Command::new(bin);
        command
            .args(args)
            .env("PINVOU3_HOME", &root)
            .env("PINVOU_NO_COLOR", "1");
        command.output().expect("binary runs")
    };
    let created = run(&[
        "knowledge",
        "collections",
        "create",
        "--name",
        "resumable",
        "--output",
        "json",
    ]);
    assert!(created.status.success(), "collection create must succeed");
    let created: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let id = created["id"].as_i64().expect("created collection id");

    let source = root.join("resume-me.txt");
    std::fs::write(
        &source,
        "Pinvou knowledge resume progression probe.".repeat(64),
    )
    .unwrap();
    let started = run(&[
        "knowledge",
        "collections",
        "add-sources",
        &id.to_string(),
        source.to_str().unwrap(),
        "--output",
        "json",
    ]);
    assert!(started.status.success(), "add-sources must succeed");
    let started: serde_json::Value = serde_json::from_slice(&started.stdout).unwrap();
    let job_id = started["jobId"]
        .as_str()
        .expect("started job id")
        .to_owned();

    // The add-sources one-shot process killed its import thread mid-flight;
    // `index resume` in a fresh process must re-arm the job (its boot
    // recovery marks the stranded job interrupted, then the resume flips it
    // back to running).
    let resumed = run(&["knowledge", "index", "resume", &job_id]);
    assert!(resumed.status.success(), "index resume must succeed");

    // Read-only poll (`index status` opens without recovery): the job must
    // have left `interrupted` for the active progression — or already be
    // done when the resume child's import thread beat process exit.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let state = loop {
        let polled = run(&["knowledge", "index", "status", "--output", "json"]);
        assert!(polled.status.success(), "index status must succeed");
        let state: serde_json::Value = serde_json::from_slice(&polled.stdout).unwrap();
        let phase = state["phase"].as_str().unwrap_or_default().to_owned();
        if phase != "interrupted" {
            break state;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "resumed job never left interrupted; last state: {state}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(state["jobId"], serde_json::json!(job_id));
    assert_eq!(state["resumable"], serde_json::json!(false));
    assert!(
        state["phase"] == serde_json::json!("preparing")
            || state["phase"] == serde_json::json!("running")
            || state["phase"] == serde_json::json!("parsing")
            || state["phase"] == serde_json::json!("done")
            || state["phase"] == serde_json::json!("done_with_errors"),
        "expected the active progression after resume: {state}"
    );
    // The resume child may exit (stranding the import thread again) before
    // it re-claims the item, so `total` can transiently read 0 during the
    // parsing phase; the deterministic resume contract is the re-armed state
    // above (same job id, running, not resumable).
    assert_eq!(state["running"], serde_json::json!(true), "state: {state}");
    if state["phase"] == serde_json::json!("done") {
        // Only reachable when the resume child's import finished before the
        // process exited; assert the document-level end state when it is.
        let documents = run(&[
            "knowledge",
            "documents",
            &id.to_string(),
            "--output",
            "json",
        ]);
        assert!(documents.status.success(), "documents listing must succeed");
        let documents: serde_json::Value = serde_json::from_slice(&documents.stdout).unwrap();
        let documents = documents["documents"].as_array().expect("documents array");
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0]["parseStatus"], serde_json::json!("parsed"));
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// The scan state is in-process, but its completion marker
/// (`last_scan_finished_at`) persists in `index.db` — so `scan status` from a
/// fresh CLI invocation converges to `done` once the background scan thread
/// of `scan start` finishes.
#[test]
fn scan_start_persists_its_completion_marker() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("scan-roundtrip");
    let root = home.path().join("docs");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("note.txt"), "pinvou scan fixture").unwrap();

    let started = run_json(&[
        "pinvou",
        "knowledge",
        "scan",
        "start",
        "--root",
        root.to_str().unwrap(),
    ]);
    // The background thread may finish before the returned snapshot for a
    // tiny root, so both phases are valid starts.
    assert!(
        started["phase"] == serde_json::json!("scanning")
            || started["phase"] == serde_json::json!("done"),
        "{started}"
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let state = run_json(&["pinvou", "knowledge", "scan", "status"]);
        if state["phase"] == serde_json::json!("done") {
            assert!(
                state["finishedAt"].as_i64().unwrap_or(0) > 0,
                "finished scan must persist its marker: {state}"
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "scan did not finish in time; last state: {state}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // Cancel is a real signal on the service (harmless once the scan is done).
    let cancelled = run_json(&["pinvou", "knowledge", "scan", "cancel"]);
    assert_eq!(cancelled["cancelled"], serde_json::json!(true));
}

/// L0 seeding is scan-driven and offline: `scan start` upserts file metadata
/// on its background thread (in-process, like add-sources) and the persisted
/// rows answer later invocations. The GUI `kb_search` NL-rule merge is
/// asserted indirectly through the CLI: "上周的 pdf" must degrade to an
/// ext + last-week-mtime filter (residual text stripped) that hits the
/// freshly seeded pdf — a raw FTS over the whole phrase would find nothing.
#[test]
fn search_hits_scan_seeded_files_with_nl_merge_and_explicit_flags() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("l0-seeded");
    let docs = home.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("季度报告.pdf"), b"pinvou knowledge fixture").unwrap();
    std::fs::write(docs.join("notes.txt"), b"plain text").unwrap();

    let started = run_json(&[
        "pinvou",
        "knowledge",
        "scan",
        "start",
        "--root",
        docs.to_str().unwrap(),
    ]);
    assert!(
        started["phase"] == serde_json::json!("scanning")
            || started["phase"] == serde_json::json!("done"),
        "{started}"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let state = run_json(&["pinvou", "knowledge", "scan", "status"]);
        if state["phase"] == serde_json::json!("done") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "scan did not finish in time; last state: {state}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let stats = run_json(&["pinvou", "knowledge", "stats"]);
    assert_eq!(stats["totalFiles"], serde_json::json!(2));
    let counts = run_json(&["pinvou", "knowledge", "type-counts"]);
    let counts = counts["typeCounts"].as_array().expect("typeCounts array");
    assert_eq!(counts.len(), 2);
    assert!(
        counts
            .iter()
            .any(|count| count["ext"] == serde_json::json!("pdf"))
    );

    // Plain substring over name/path (2 CJK chars take the LIKE fallback).
    let hits = run_json(&["pinvou", "knowledge", "search", "报告"]);
    let hits = hits["hits"].as_array().expect("hits array");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["name"], serde_json::json!("季度报告.pdf"));
    assert_eq!(hits[0]["ext"], serde_json::json!("pdf"));

    // NL merge without any explicit flag: ext + mtime filter only.
    let hits = run_json(&[
        "pinvou",
        "knowledge",
        "search",
        "上周的 pdf",
        "--limit",
        "5",
    ]);
    let hits = hits["hits"].as_array().expect("hits array");
    assert_eq!(hits.len(), 1, "NL-merged query must hit the seeded pdf");
    assert_eq!(hits[0]["ext"], serde_json::json!("pdf"));

    // Explicit --ext composes with the text query.
    let hits = run_json(&["pinvou", "knowledge", "search", "notes", "--ext", "txt"]);
    let hits = hits["hits"].as_array().expect("hits array");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["name"], serde_json::json!("notes.txt"));
    let hits = run_json(&["pinvou", "knowledge", "search", "notes", "--ext", "pdf"]);
    assert_eq!(hits["hits"], serde_json::json!([]));

    // Invalid date flags are usage errors (exit 2), like bad ids.
    assert_usage(&[
        "pinvou",
        "knowledge",
        "search",
        "q",
        "--after",
        "not-a-date",
    ]);
    assert_usage(&[
        "pinvou",
        "knowledge",
        "search",
        "q",
        "--before",
        "2026-02-30",
    ]);
}

#[test]
fn model_status_reports_disk_state_and_model_cancel_succeeds() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = TempHome::new("model-status");

    let status = run_json(&["pinvou", "knowledge", "model", "status"]);
    assert_eq!(status["version"], serde_json::json!("bge-m3"));
    assert_eq!(status["installed"], serde_json::json!(false));
    assert_eq!(status["ready"], serde_json::json!(false));
    let model_dir = status["model_dir"].as_str().expect("model dir");
    assert!(
        model_dir.ends_with("knowledge/models/bge-m3"),
        "{model_dir}"
    );

    let cancelled = run_json(&["pinvou", "knowledge", "model", "cancel"]);
    assert_eq!(cancelled["cancelled"], serde_json::json!(true));
}

#[test]
fn remote_connections_answer_offline_without_configured_servers() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = TempHome::new("remote-empty");

    let listed = run_json(&["pinvou", "knowledge", "remote", "connections"]);
    assert_eq!(listed["connections"], serde_json::json!([]));

    for arguments in [
        vec!["pinvou", "knowledge", "remote", "collections"],
        vec!["pinvou", "knowledge", "remote", "search", "papers", "query"],
    ] {
        let error = execute_error(&arguments);
        assert_eq!(error.exit_code(), ExitCode::Failed, "{arguments:?}");
        assert!(
            error
                .to_string()
                .contains("no remote knowledge connections configured"),
            "{arguments:?}: {error}"
        );
    }
}

// ---- opt-in paths (network / display / windowless host) ----

#[test]
#[ignore = "opt-in: cargo test -p pinvou-cli --test knowledge_contract -- --ignored — \
           boots the windowless product host (needs a display/xvfb); \
           command: pinvou knowledge host status"]
fn host_status_reports_the_shared_host_snapshot() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = TempHome::new("host-status");
    let status = run_json(&["pinvou", "knowledge", "host", "status"]);
    assert_eq!(
        status["endpoint"],
        serde_json::json!("https://127.0.0.1:3210")
    );
    assert!(status["supported"].is_boolean());
    assert!(status["app_version"].is_string());
}

#[test]
#[ignore = "opt-in: cargo test -p pinvou-cli --test knowledge_contract -- --ignored — \
           boots the windowless product host (needs a display/xvfb) and dials the \
           endpoint; command: pinvou knowledge remote probe <url>"]
fn remote_probe_reports_a_dead_endpoint_as_a_failed_host_call() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = TempHome::new("remote-probe-dead");
    // The TLS-pinned identity handshake is a real network call now; a dead
    // endpoint must surface as a failed command, not a crash.
    let error = execute_error(&[
        "pinvou",
        "knowledge",
        "remote",
        "probe",
        "https://127.0.0.1:9",
    ]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("remote probe"), "{error}");
}

#[test]
#[ignore = "opt-in: needs a display (windowless host boot) plus real network; \
            command: pinvou knowledge remote connections (configured server)"]
fn remote_connections_probe_a_configured_server() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("remote-configured");
    let knowledge_dir = home.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();
    // A configured-but-dead endpoint must surface as an offline connection
    // status, not a crash (GUI `remote_kb_connections` semantics).
    std::fs::write(
        knowledge_dir.join("remote-connections.json"),
        r#"{"version":3,"connections":[{"serverId":"srv-test","name":"lan","endpoint":"https://127.0.0.1:9","scope":"read","deviceId":"dev-test","tlsCa":"","legacyInsecureHttp":false}]}"#,
    )
    .unwrap();
    let listed = run_json(&["pinvou", "knowledge", "remote", "connections"]);
    let connections = listed["connections"].as_array().expect("connections array");
    assert_eq!(connections.len(), 1);
    assert_eq!(connections[0]["serverId"], serde_json::json!("srv-test"));
    assert_eq!(connections[0]["online"], serde_json::json!(false));
}

/// `index cancel` on a resumable (interrupted) job is an effective cancel —
/// the store's cancel is synchronous and also deletes the staged chunks —
/// and must be reported as signalled. The post-cancel status never shows
/// `running`, so deciding the message from it printed "nothing was
/// signalled" on exactly the cancels that landed. Driven through the real
/// binary so the stranded-job recovery is the production path.
#[test]
fn index_cancel_reports_a_signalled_resumable_job() {
    let bin = env!("CARGO_BIN_EXE_pinvou");
    let root = std::env::temp_dir().join(format!(
        "pinvou-cli-knowledge-cancel-report-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let run = |args: &[&str]| {
        let mut command = std::process::Command::new(bin);
        command
            .args(args)
            .env("PINVOU3_HOME", &root)
            .env("PINVOU_NO_COLOR", "1");
        command.output().expect("binary runs")
    };
    let created = run(&[
        "knowledge",
        "collections",
        "create",
        "--name",
        "cancellable",
        "--output",
        "json",
    ]);
    assert!(created.status.success(), "collection create must succeed");
    let created: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let id = created["id"].as_i64().expect("created collection id");

    let source = root.join("cancel-me.txt");
    std::fs::write(
        &source,
        "Pinvou knowledge cancel reporting probe.".repeat(64),
    )
    .unwrap();
    let started = run(&[
        "knowledge",
        "collections",
        "add-sources",
        &id.to_string(),
        source.to_str().unwrap(),
        "--output",
        "json",
    ]);
    assert!(started.status.success(), "add-sources must succeed");
    let started: serde_json::Value = serde_json::from_slice(&started.stdout).unwrap();
    let job_id = started["jobId"]
        .as_str()
        .expect("started job id")
        .to_owned();

    // The one-shot add-sources process stranded its import thread; this
    // invocation's recovery marks the job interrupted (= resumable), and the
    // cancel must report the signal it actually landed.
    let cancelled = run(&["knowledge", "index", "cancel", &job_id]);
    assert!(cancelled.status.success(), "index cancel must succeed");
    let stdout = String::from_utf8_lossy(&cancelled.stdout);
    assert!(stdout.contains("signalled"), "{stdout}");
    assert!(!stdout.contains("nothing was signalled"), "{stdout}");

    // The cancelled job is finished, so a second cancel honestly reports
    // there was nothing left to signal.
    let again = run(&["knowledge", "index", "cancel", &job_id]);
    assert!(again.status.success(), "second index cancel must succeed");
    let stdout = String::from_utf8_lossy(&again.stdout);
    assert!(stdout.contains("nothing was signalled"), "{stdout}");

    let _ = std::fs::remove_dir_all(&root);
}
