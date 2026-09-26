//! Contract tests for the `knowledge` family (`crates/cli/src/knowledge.rs`).
//!
//! Parse-level tests cover every subcommand plus the invalid shapes that must
//! map to exit-code 2 usage errors. Execute-level tests run against a temp
//! `PINVOU3_HOME` (serialized through ENV_LOCK, following cli_contract.rs):
//! the KnowledgeService round-trips (collections CRUD, document listing, the
//! scan pair, add-sources indexing, L0 stats/type-counts/search, model
//! status) are offline — an empty `index.db` needs no model and no network,
//! because the import thread skips embedder loading when the model is not
//! installed and degrades to full-text. The L0 search round-trip seeds rows
//! through a completed `scan start` (still offline) and asserts the NL-rule
//! merge ("上周的 pdf" → ext + mtime filter + residual text) indirectly.
//!
//! Import-job semantics: `add-sources`/`resume`/`retry` WAIT for their job
//! to finish inside the invocation (a one-shot process kills its import
//! thread at exit, which used to strand the job `running` with no owner),
//! report the final state with a phase-honest exit code, and a stall past
//! the no-progress bound INTERRUPTS the job first
//! (`PINVOU_KB_IMPORT_STALL_MILLIS` overrides the bound for automation)
//! so it is left `interrupted`/resumable, not `running`-with-no-owner. The
//! stranded-job scenarios are driven by SIGKILLing a real `add-sources`
//! child mid-import (`strand_running_job`): the surviving DB row is exactly
//! what a hard-killed process leaves behind, and no CLI lane may pretend to
//! reconcile it — re-enqueue/resume/retry refuse with exit 1 naming the
//! running job, read lanes leave it untouched, and only `index cancel` (a
//! real, targeted, DB-level cancel of the named job) and `collections
//! delete` of its own collection may act on it. The desktop-app-only boot
//! recovery is mirrored in-process by opening `KnowledgeService::new` (the
//! recovering constructor the CLI never calls), which is what turns a
//! stranded job into the `interrupted` state `index resume` legitimately
//! continues.
//!
//! `scan cancel` and `model cancel` refuse honestly with exit 1
//! (`knowledge_{scan,model}_cancel_requires_product_host`): the only scan
//! this process could signal is always already over (`scan start` blocks),
//! and model downloads only ever run inside the desktop app process
//! (process-local cancel channels) — a subcommand that could only report a
//! no-op signal is placeholder capability (AGENTS.md §4). The session-mount surface refuses honestly
//! (per-process app memory), and `collections delete` never boots the
//! session store (the mount sweep was always empty in a one-shot process;
//! the boot would run the 50-sessions-per-kind retention). Paths that need
//! the network, a model download, a configured remote server or the
//! windowless product host stay behind `#[ignore]` with their opt-in
//! command named (AGENTS.md rule: never touch the network in default
//! tests); `model download` keeps its stable `knowledge_backend_unavailable`
//! code with the documented upstream blocker.

use pinvou_cli::{ExitCode, execute, parse_args};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
        let _ = std::fs::remove_dir_all(&self.root);
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

/// Writes `count` bulk text sources under `home` — large enough that the
/// import thread spends seconds per file (thousands of chunks with the
/// per-batch checkpoint sleeps), so a SIGKILL mid-import deterministically
/// strands the job with work still pending.
fn write_bulk_sources(home: &TempHome, count: usize) -> Vec<PathBuf> {
    (0..count)
        .map(|index| {
            let path = home.path().join(format!("bulk-{index}.txt"));
            std::fs::write(&path, "Pinvou knowledge strand probe. ".repeat(60_000)).unwrap();
            path
        })
        .collect()
}

/// Spawns a real `pinvou knowledge collections add-sources` child over
/// `home`, polls the DB-backed job state through read-only invocations until
/// the import is demonstrably mid-run (phase `running`, one item completed,
/// more still pending), then SIGKILLs the child — reproducing exactly what a
/// hard-killed process (power loss, `kill -9`) leaves behind: a job row
/// stranded `running` with no owner, the state no CLI lane may pretend to
/// reconcile. Returns the stranded job's id.
fn strand_running_job(home: &TempHome, id: i64, sources: &[PathBuf]) -> String {
    let bin = env!("CARGO_BIN_EXE_pinvou");
    let mut child = std::process::Command::new(bin)
        .arg("knowledge")
        .arg("collections")
        .arg("add-sources")
        .arg(id.to_string())
        .args(sources.iter().map(|path| path.to_str().unwrap()))
        .env("PINVOU3_HOME", home.path())
        .env("PINVOU_NO_COLOR", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn add-sources child");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let job_id = loop {
        let polled = std::process::Command::new(bin)
            .args(["knowledge", "index", "status", "--output", "json"])
            .env("PINVOU3_HOME", home.path())
            .env("PINVOU_NO_COLOR", "1")
            .output()
            .expect("index status child runs");
        assert!(
            polled.status.success(),
            "index status must succeed while the import runs"
        );
        let state: serde_json::Value = serde_json::from_slice(&polled.stdout).unwrap();
        let done = state["done"].as_u64().unwrap_or(0);
        let total = state["total"].as_u64().unwrap_or(0);
        if state["running"] == serde_json::json!(true) && done >= 1 && total > done {
            break state["jobId"]
                .as_str()
                .expect("running job carries its id")
                .to_owned();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the import child never reached a killable mid-run state \
             (last status: {state})"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    child.kill().expect("SIGKILL the importing child");
    child.wait().expect("reap the killed child");
    job_id
}

/// Seeds a real import job through the feature service inside THIS process
/// (its import thread runs here, exactly like an add-sources invocation's
/// would) — the "some other owner" position a CLI lane must refuse to race.
/// The file count sets the import's lower time bound; returns (collection
/// id, job id).
fn seed_running_job(home: &TempHome, label: &str, files: usize) -> (i64, String) {
    let db = pinvou3_lib::features::knowledge::default_db_path();
    let service = pinvou3_lib::features::knowledge::KnowledgeService::new_without_recovery(&db)
        .expect("seed service");
    let collection = service
        .l1()
        .create_collection(label, None, None)
        .expect("seed collection");
    let dir = home.path().join(format!("{label}-src"));
    std::fs::create_dir_all(&dir).unwrap();
    for index in 0..files {
        std::fs::write(
            dir.join(format!("item-{index:03}.txt")),
            "seed prose for the importer to parse and chunk. ".repeat(64),
        )
        .unwrap();
    }
    let state = service.start_index(collection, vec![dir]);
    let job = state.job_id.expect("seed job id").to_owned();
    (collection, job)
}

/// Polls `knowledge index status` (read-only, non-recovering) until the
/// phase is one of `phases`, failing after `timeout_secs`.
fn poll_index_phase(phases: &[&str], timeout_secs: u64) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let state = run_json(&["pinvou", "knowledge", "index", "status"]);
        let phase = state["phase"].as_str().unwrap_or_default().to_owned();
        if phases.contains(&phase.as_str()) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "index job never reached {phases:?} (last state: {state})"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
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
        // `scan cancel` is kept as an honest exit-1 refusal: the only scan
        // this process could signal is always already over (scan start
        // blocks), so its parse shape survives like every other subcommand.
        (
            &["pinvou", "knowledge", "scan", "cancel"],
            "Knowledge(ScanCancel)".into(),
        ),        (&["pinvou", "knowledge", "stats"], "Knowledge(Stats)".into()),
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
        // `model cancel` is kept as an honest exit-1 refusal: the download
        // cancel flag is process-local to the desktop app's orchestration,
        // so a one-shot CLI can never aim it at a real download — see the
        // refusal test.
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
        // `scan cancel`/`model cancel` stay PARSEABLE subcommands (their
        // honest exit-1 refusals are execute-level, pinned below); malformed
        // shapes of them remain usage errors.
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

/// The mount surface refuses before opening the session store. The refusal
/// cannot depend on the session (mounts are app-process memory), and
/// `SessionStore::boot()` is not a read: it enforces the 50-sessions-per-kind
/// retention policy and irreversibly evicts the user's oldest non-pinned
/// sessions. A command that can only refuse must not destroy chat history on
/// the way to saying so.
///
/// Proof that no store is opened: the sessions root is occupied by a regular
/// file, so `SessionStore::boot()` would fail loudly with "session store
/// unavailable" — the product-host refusal comes back instead, for a session
/// id that does not exist either.
#[test]
fn mount_surface_refuses_without_booting_the_session_store() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("mounts-no-boot");
    std::fs::write(home.path().join("sessions"), b"not a directory").unwrap();

    for arguments in [
        vec!["pinvou", "knowledge", "mounts", "missing-session"],
        vec!["pinvou", "knowledge", "mount", "missing-session", "7"],
        vec!["pinvou", "knowledge", "unmount", "missing-session", "7"],
    ] {
        let error = execute_error(&arguments);
        assert_eq!(error.exit_code(), ExitCode::Failed, "{arguments:?}");
        let message = error.to_string();
        assert!(
            message.contains("_requires_product_host"),
            "{arguments:?}: {message}"
        );
        assert!(
            !message.contains("session store unavailable")
                && !message.contains("session not found"),
            "the refusal must be returned before any store boot: {message}"
        );
    }

    // The charset gate still runs first (exit 2), so a traversal-shaped id
    // never reaches the refusal.
    assert_usage(&["pinvou", "knowledge", "mounts", "../escape"]);
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

/// Index jobs are DB-persisted, and `add-sources` waits for the import to
/// finish INSIDE the invocation (mirroring `scan start`'s wait-loop
/// contract): a one-shot process that returned immediately would kill its
/// import thread and strand the job `running` with no owner. So the printed
/// state is the job's FINAL state, a fresh `index status` invocation agrees
/// with it, and the command fails unless the job ended `done`. The embedder
/// load is skipped (model not installed in the sandbox home), keeping this
/// fully offline.
#[test]
fn add_sources_indexes_a_text_file_and_waits_for_the_final_state() {
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

    // The same wait-for-terminal contract the scan-start tests assert
    // (`scan_start_persists_its_completion_marker`): the returned phase is
    // terminal, and the persisted state a fresh invocation reads matches it.
    let finished = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "add-sources",
        &id.to_string(),
        source.to_str().unwrap(),
    ]);
    assert_eq!(finished["collectionId"], serde_json::json!(id));
    let job_id = finished["jobId"]
        .as_str()
        .expect("finished job id")
        .to_owned();
    assert_eq!(
        finished["phase"],
        serde_json::json!("done"),
        "add-sources must wait for the import and report its final phase"
    );
    assert_eq!(finished["running"], serde_json::json!(false));
    assert!(
        finished["done"].as_u64().unwrap_or(0) >= 1,
        "expected one processed item"
    );

    // No stranded running job: a fresh invocation (which opens WITHOUT boot
    // recovery) sees the same terminal state under the same job id.
    let state = run_json(&["pinvou", "knowledge", "index", "status"]);
    assert_eq!(state["jobId"], serde_json::json!(job_id));
    assert_eq!(state["phase"], serde_json::json!("done"));
    assert_eq!(state["resumable"], serde_json::json!(false));

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

    // A named job is read per-job, not through the latest-job ordering: the
    // finished job answers under its own id, and an id that does not exist
    // gets the family's stable not-found code instead of another job's state.
    let named = run_json(&["pinvou", "knowledge", "index", "status", &job_id]);
    assert_eq!(named["jobId"], serde_json::json!(job_id));
    let error = execute_error(&["pinvou", "knowledge", "index", "status", "other-job"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("knowledge_index_job_not_found"),
        "{error}"
    );
    assert!(
        !error.to_string().contains("Query returned no rows"),
        "{error}"
    );

    // Cancel targets the active/latest job; on a finished job it is a
    // signal-only no-op that still succeeds.
    run_ok(&["pinvou", "knowledge", "index", "cancel", &job_id]);

    // Resume/retry validate the named id against the latest job first: a
    // mistyped id refuses with exit 1 without touching the store (no CLI
    // lane runs boot recovery anymore).
    let error = execute_error(&["pinvou", "knowledge", "index", "resume", "bogus-job"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("active/latest"), "{error}");
    let error = execute_error(&["pinvou", "knowledge", "index", "retry", "bogus-job", "1"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("active/latest"), "{error}");

    // The finished job is the latest but not resumable: resume/retry name
    // that with the friendly job-not-found code instead of leaking the raw
    // rusqlite "Query returned no rows" driver message.
    for arguments in [
        vec!["pinvou", "knowledge", "index", "resume", &job_id],
        vec!["pinvou", "knowledge", "index", "retry", &job_id, "1"],
    ] {
        let error = execute_error(&arguments);
        assert_eq!(error.exit_code(), ExitCode::Failed, "{arguments:?}");
        let message = error.to_string();
        assert!(
            message.contains("knowledge_index_job_not_found"),
            "{arguments:?}: {message}"
        );
        assert!(!message.contains("Query returned no rows"), "{message}");
    }
}

/// A second `add-sources` behind a job that is still preparing/running must
/// refuse with exit 1 (the round-18/19 stranding fix): no CLI lane runs boot
/// recovery anymore, so the second enqueue cannot "recover" the stranded job
/// into resumable — it must see it running (it cannot tell a live desktop-app
/// owner from a dead process's orphan; the store carries no owner heartbeat)
/// and refuse. The refusal names the running job, and the job's state is
/// untouched afterwards. The strand is produced by SIGKILLing a real
/// add-sources child mid-import, exactly what a hard-killed process leaves
/// behind.
#[test]
fn add_sources_refuses_to_race_a_running_job() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("index-resumable-guard");
    let created = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "guarded",
    ]);
    let id = created["id"].as_i64().expect("created collection id");

    let sources = write_bulk_sources(&home, 2);
    let job_id = strand_running_job(&home, id, &sources);

    // The second enqueue must refuse (exit 1) naming the stranded running
    // job, and must not turn it into anything else.
    let extra = home.path().join("second.txt");
    std::fs::write(
        &extra,
        "Pinvou knowledge must not silently drop this source.",
    )
    .unwrap();
    let error = execute_error(&[
        "pinvou",
        "knowledge",
        "collections",
        "add-sources",
        &id.to_string(),
        extra.to_str().unwrap(),
    ]);
    assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
    let message = error.to_string();
    assert!(
        message.contains("still running") && message.contains(&job_id),
        "the refusal must name the running job it refused to race: {message}"
    );

    // The stranded job is still exactly what the killed child left behind:
    // running, not resumable, same id.
    let state = run_json(&["pinvou", "knowledge", "index", "status"]);
    assert_eq!(state["jobId"], serde_json::json!(job_id));
    assert_eq!(state["phase"], serde_json::json!("running"));
    assert_eq!(state["resumable"], serde_json::json!(false));
}

/// The CLI never runs the GUI's boot recovery: a job a live worker (here:
/// a SIGKILLed add-sources child's orphan, indistinguishable from a live
/// desktop-app import to the store) left preparing/running must still read
/// running after every read and maintenance lane ran — only `index cancel`
/// can end it, and it must do so purely by cancelling the named job.
#[test]
fn index_resume_and_retry_refuse_a_running_job_and_leave_it_untouched() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("index-resume-precheck");
    let bin = env!("CARGO_BIN_EXE_pinvou");
    let run_child = |args: &[&str]| {
        let mut command = std::process::Command::new(bin);
        command
            .args(args)
            .env("PINVOU3_HOME", home.path())
            .env("PINVOU_NO_COLOR", "1");
        command.output().expect("binary runs")
    };

    let created = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "live",
    ]);
    let id = created["id"].as_i64().expect("created collection id");

    let sources = write_bulk_sources(&home, 2);
    let source_str = sources[0].to_str().unwrap().to_owned();
    let job_id = strand_running_job(&home, id, &sources);

    // Mistyped ids fail against the latest job without touching the store.
    for arguments in [
        vec!["pinvou", "knowledge", "index", "resume", "typo-job"],
        vec!["pinvou", "knowledge", "index", "retry", "typo-job", "1"],
    ] {
        let error = execute_error(&arguments);
        assert_eq!(error.exit_code(), ExitCode::Failed, "{arguments:?}");
        let message = error.to_string();
        assert!(
            message.contains("active/latest") && message.contains(&job_id),
            "{arguments:?}: {message}"
        );
    }

    // The collection commands gate the same way: a mistyped id must fail on
    // the plain open.
    for arguments in [
        vec![
            "pinvou",
            "knowledge",
            "collections",
            "delete",
            "999",
            "--yes",
        ],
        vec![
            "pinvou",
            "knowledge",
            "collections",
            "add-sources",
            "999",
            source_str.as_str(),
        ],
    ] {
        let error = execute_error(&arguments);
        assert_eq!(error.exit_code(), ExitCode::Failed, "{arguments:?}");
        assert!(
            error.to_string().contains("collection 999 not found"),
            "{arguments:?}: the unknown id must be named"
        );
    }

    // resume/retry on the NAMED stranded job refuse with exit 1: the job
    // may be owned by a live process, and there is no cross-process owner
    // heartbeat that could prove otherwise. No boot recovery may run to
    // wedge it into interrupted.
    for arguments in [
        vec!["pinvou", "knowledge", "index", "resume", &job_id],
        vec!["pinvou", "knowledge", "index", "retry", &job_id, "1"],
    ] {
        let error = execute_error(&arguments);
        assert_eq!(error.exit_code(), ExitCode::Failed, "{arguments:?}");
        let message = error.to_string();
        assert!(
            message.contains("still running") && message.contains(&job_id),
            "{arguments:?}: {message}"
        );
    }

    // `scan start` never touches the import-job store (its open is the
    // family's plain non-recovering one), so the stranded job must still
    // read running after a scan lane ran.
    let scan_root = home.path().join("scan-root");
    std::fs::create_dir_all(&scan_root).unwrap();
    let scan = run_json(&[
        "pinvou",
        "knowledge",
        "scan",
        "start",
        "--root",
        scan_root.to_str().unwrap(),
    ]);
    assert!(scan.is_object(), "scan start must emit a JSON object");
    let polled = run_child(&["knowledge", "index", "status", "--output", "json"]);
    let state: serde_json::Value = serde_json::from_slice(&polled.stdout).unwrap();
    assert_eq!(
        state["phase"],
        serde_json::json!("running"),
        "scan start must not run boot recovery or otherwise touch the stranded job"
    );
    assert_eq!(state["jobId"], serde_json::json!(job_id));
}

/// `add-sources` pre-flights every source path before any store mutation: a
/// FIFO would hang a blocking open (upstream's expansion silently skips
/// non-regular files), and a nonexistent path would silently enqueue
/// nothing. Both must be an honest exit 1 that names the path — and no
/// import job may start.
#[test]
#[cfg(unix)]
fn add_sources_preflights_the_source_paths() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("add-sources-preflight");
    let created = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "preflight",
    ]);
    let id = created["id"].as_i64().expect("created collection id");

    let fifo = home.path().join("preflight.fifo");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo failed");
    let error = execute_error(&[
        "pinvou",
        "knowledge",
        "collections",
        "add-sources",
        &id.to_string(),
        fifo.to_str().unwrap(),
    ]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("not a regular file"),
        "the FIFO path must be named as the rejection cause: {error}"
    );

    let missing = home.path().join("missing.md");
    let error = execute_error(&[
        "pinvou",
        "knowledge",
        "collections",
        "add-sources",
        &id.to_string(),
        missing.to_str().unwrap(),
    ]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("does not exist"),
        "the missing path must be named as the rejection cause: {error}"
    );

    // The pre-flight rejects run before any job is created, so no import
    // job may exist afterwards.
    let state = run_json(&["pinvou", "knowledge", "index", "status"]);
    assert_eq!(state["jobId"], serde_json::Value::Null);
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

/// Green-path `index resume` against a genuinely interrupted job. A real
/// `add-sources` child is SIGKILLed mid-import (the hard-crash shape), then
/// the DESKTOP APP's own boot recovery is replayed in-process through the
/// recovering constructor the CLI never calls (`KnowledgeService::new`) —
/// that, not a CLI lane, is what owns reconciling an orphaned job. While
/// the job is still stranded `running`, every re-arm lane (resume/retry,
/// and a second add-sources behind it) must REFUSE (exit 1) without
/// flipping it — the CLI cannot tell a live owner from a dead process's
/// orphan. The recovery turns the stranded job `interrupted` with its
/// staged per-item progress preserved; the CLI `index resume` then re-arms
/// it and, like `scan start`/`add-sources`, WAITS inside the invocation
/// for the job to reach its terminal phase (a one-shot process that
/// returned immediately would strand the re-armed job running again).
/// Asserted through the real binary.
#[test]
fn index_resume_refuses_a_stranded_running_job_then_continues_after_recovery() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("index-resume-roundtrip");
    let bin = env!("CARGO_BIN_EXE_pinvou");
    let run = |args: &[&str]| {
        let mut command = std::process::Command::new(bin);
        command
            .args(args)
            .env("PINVOU3_HOME", home.path())
            .env("PINVOU_NO_COLOR", "1");
        command.output().expect("binary runs")
    };
    let created = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "resumable",
    ]);
    let id = created["id"].as_i64().expect("created collection id");

    // Two bulk sources: the strand needs a window with one item completed
    // and another still pending, and the resume below then has genuine
    // remaining work to finish inside its wait.
    let sources = write_bulk_sources(&home, 2);
    let job_id = strand_running_job(&home, id, &sources);

    // While the job still reads `running`, resume must refuse honestly: the
    // CLI cannot distinguish a dead owner from a live desktop-app import
    // (no cross-process owner heartbeat), and the app's next boot — not a
    // CLI lane — is what relabels it.
    let refused = run(&["knowledge", "index", "resume", &job_id]);
    assert!(
        !refused.status.success(),
        "resume on a running stranded job must refuse"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("still running"), "{stderr}");
    assert!(stderr.contains("index resume"), "{stderr}");

    // A second add-sources against ANOTHER collection is refused the same
    // way while the store's latest job is running, and the new sources must
    // not be silently enqueued (the upstream short-circuit would drop them
    // behind the unfinished job while reporting success).
    let other = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "other",
    ]);
    let other_id = other["id"].as_i64().expect("other collection id");
    let extra = home.path().join("second.txt");
    std::fs::write(&extra, "must not be enqueued.").unwrap();
    let refused_enqueue = run(&[
        "knowledge",
        "collections",
        "add-sources",
        &other_id.to_string(),
        extra.to_str().unwrap(),
    ]);
    assert!(
        !refused_enqueue.status.success(),
        "add-sources behind a running latest job must refuse"
    );
    let enqueue_stderr = String::from_utf8_lossy(&refused_enqueue.stderr);
    assert!(
        enqueue_stderr.contains("still running") && enqueue_stderr.contains("NOT enqueued"),
        "{enqueue_stderr}"
    );
    let other_documents = run_json(&["pinvou", "knowledge", "documents", &other_id.to_string()]);
    assert_eq!(
        other_documents["documents"],
        serde_json::json!([]),
        "refused sources must not be enqueued"
    );

    // The refusals must not have flipped the strand: it still reads running
    // and not resumable exactly as the killed child left it.
    let stranded = run_json(&["pinvou", "knowledge", "index", "status"]);
    assert_eq!(stranded["jobId"], serde_json::json!(job_id));
    assert_eq!(stranded["phase"], serde_json::json!("running"));
    assert_eq!(stranded["resumable"], serde_json::json!(false));

    // The GUI's startup recovery, replayed exactly as the desktop app boots
    // it (the recovering constructor) — the orphaned running job becomes
    // interrupted/resumable with its staged per-item progress preserved.
    // The CLI itself must never do this.
    {
        let db = pinvou3_lib::features::knowledge::default_db_path();
        let service = pinvou3_lib::features::knowledge::KnowledgeService::new(&db).expect("boot");
        let state = service.index_job_state(&job_id).expect("recovered job");
        assert!(
            state.resumable,
            "boot recovery must turn the orphaned job interrupted (got {:?})",
            state
        );
        // The item completed before the kill survives the recovery:
        // `interrupt` re-claims only in-flight items, never finished ones.
        assert!(
            state.done >= 1,
            "staged progress must survive the kill (got {:?})",
            state
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    // The resume re-arms the job and waits for it: the printed state is the
    // job's FINAL state under the same id, never a stranded `running`.
    let resumed = run(&["knowledge", "index", "resume", &job_id, "--output", "json"]);
    assert!(
        resumed.status.success(),
        "index resume must succeed: {}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let final_state: serde_json::Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(final_state["jobId"], serde_json::json!(job_id));
    assert_eq!(
        final_state["phase"],
        serde_json::json!("done"),
        "resume must wait for the re-armed job to finish and report its final phase"
    );
    assert_eq!(final_state["running"], serde_json::json!(false));
    assert_eq!(final_state["resumable"], serde_json::json!(false));
    assert_eq!(
        final_state["total"].as_u64().unwrap_or(0),
        final_state["done"].as_u64().unwrap_or(1),
        "the resume must finish every item"
    );

    // Document-level end state: the resumed import really completed — the
    // item finished before the kill and the one still pending both parsed.
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
    assert_eq!(documents.len(), 2);
    assert!(
        documents
            .iter()
            .all(|document| document["parseStatus"] == serde_json::json!("parsed")),
        "every resumed item must have completed: {documents:?}"
    );
}

/// A running latest job must not be re-armed — not by `add-sources` (a
/// second import would run items concurrently against the same store) nor
/// by `index resume`/`index retry` (re-arming a job its owner still
/// executes). The running job is seeded through the real feature service
/// in-process (its import thread runs in this test process, which is
/// precisely the "some other owner" position the CLI must refuse); the
/// refusals must arrive without flipping the job or enqueueing the new
/// sources. Complements the SIGKILL-driven strand tests with an
/// owner that is genuinely LIVE while the refusals land.
#[test]
fn running_jobs_refuse_resume_retry_and_second_add_sources() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("running-job-refusal");
    let (_collection, job_id) = seed_running_job(&home, "guarded", 400);

    // The CLI sees the in-flight job as the (running) latest one and must
    // refuse both re-arming commands with the owner explanation.
    for arguments in [
        vec!["pinvou", "knowledge", "index", "resume", &job_id],
        vec!["pinvou", "knowledge", "index", "retry", &job_id, "1"],
    ] {
        let error = execute_error(&arguments);
        assert_eq!(error.exit_code(), ExitCode::Failed, "{arguments:?}");
        let message = error.to_string();
        assert!(
            message.contains("still running") && message.contains("another process"),
            "{arguments:?}: {message}"
        );
    }

    // A second add-sources against a DIFFERENT collection is likewise
    // refused while the store's latest job is running, and the new sources
    // must not be enqueued.
    let other = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "other",
    ]);
    let other_id = other["id"].as_i64().expect("other collection id");
    let second_source = home.path().join("second.txt");
    std::fs::write(&second_source, "must not be enqueued.").unwrap();
    let error = execute_error(&[
        "pinvou",
        "knowledge",
        "collections",
        "add-sources",
        &other_id.to_string(),
        second_source.to_str().unwrap(),
    ]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    let message = error.to_string();
    assert!(
        message.contains("still running") && message.contains("NOT enqueued"),
        "{message}"
    );

    // The refused commands must not have flipped the seeded job: it stays
    // on its active progression (interrupted would mean something ran the
    // boot-recovery UPDATE against a live owner).
    let state = poll_index_phase(&["running", "done", "done_with_errors"], 180);
    assert_eq!(state["jobId"], serde_json::json!(job_id));
    assert_ne!(
        state["phase"],
        serde_json::json!("interrupted"),
        "no refusal may run boot recovery against a live owner"
    );

    let documents = run_json(&["pinvou", "knowledge", "documents", &other_id.to_string()]);
    assert_eq!(
        documents["documents"],
        serde_json::json!([]),
        "refused sources must not be enqueued"
    );
}

/// The import-owning commands' no-progress timeout leaves the job
/// `interrupted` (immediately resumable), not `running`-with-no-owner: the
/// timeout path interrupts through the feature layer's `interrupt_index`
/// before failing. The stall is driven for real: the source "tree" is 110k
/// empty directories, so the import thread spends seconds in the pre-item
/// WALK phase (nothing indexed, no counters moving, no DB lock held), while
/// the invoking child runs with `PINVOU_KB_IMPORT_STALL_MILLIS=300` — the
/// test/automation override knob. The child must exit 1, report the
/// interrupted/resumable remedy, and leave the job `interrupted` on disk
/// for a fresh invocation to read back.
#[test]
fn a_stalled_import_timeout_interrupts_the_job_for_resume() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("stall-timeout");

    let created = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "stall",
    ]);
    let id = created["id"].as_i64().expect("created collection id");

    // A wide-and-deep tree of EMPTY directories: every entry is traversed
    // by the importer's walk (with the excluder's per-entry name checks)
    // but yields zero files, so the observable job signature stays frozen
    // at (done 0, total 0, failed 0, no current item) until long past the
    // injected 300 ms bound.
    let root = home.path().join("stall-tree");
    for a in 0..100 {
        let band = root.join(format!("a{a:03}"));
        for b in 0..100 {
            let cell = band.join(format!("b{b:03}"));
            std::fs::create_dir_all(&cell).expect("create cell dir");
            for c in 0..10 {
                std::fs::create_dir(cell.join(format!("c{c:02}"))).expect("create leaf dir");
            }
        }
    }

    // The add-sources child owns the import, stalls on the bound during the
    // walk, and must interrupt its own job before failing.
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_pinvou"));
    command
        .args([
            "knowledge",
            "collections",
            "add-sources",
            &id.to_string(),
            root.to_str().unwrap(),
        ])
        .env("PINVOU3_HOME", home.path())
        .env("PINVOU_NO_COLOR", "1")
        .env("PINVOU_KB_IMPORT_STALL_MILLIS", "300");
    let outcome = command.output().expect("stalled add-sources child runs");
    assert!(
        !outcome.status.success(),
        "the stalled import must exit 1, got {:?} with stdout {}",
        outcome.status.code(),
        String::from_utf8_lossy(&outcome.stdout)
    );
    let stderr = String::from_utf8_lossy(&outcome.stderr);
    assert!(
        stderr.contains("no progress") && stderr.contains("resumable now"),
        "the timeout must report the interrupted/resumable remedy: {stderr}"
    );

    // The job on disk is interrupted (resumable), not stranded running:
    // read from a fresh invocation. The stalled job is the only one (fresh
    // store), so the latest-job read names it.
    let settled = run_json(&["pinvou", "knowledge", "index", "status"]);
    assert!(settled["jobId"].is_string(), "stalled job missing its id");
    assert_eq!(settled["phase"], serde_json::json!("interrupted"));
    assert_eq!(settled["resumable"], serde_json::json!(true));
}

/// `scan start` waits for the scan to finish inside the invocation (a
/// fire-and-forget scan would be killed by process exit before doing any
/// work), so the returned state is already terminal and the completion
/// marker (`last_scan_finished_at`) is persisted in `index.db` by the time
/// the command exits — `scan status` from a fresh CLI invocation reports
/// `done` deterministically.
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
    assert_eq!(started["phase"], serde_json::json!("done"));
    assert_eq!(started["running"], serde_json::json!(false));
    assert!(
        started["finishedAt"].as_i64().unwrap_or(0) > 0,
        "a completed scan must carry its persisted marker"
    );

    let state = run_json(&["pinvou", "knowledge", "scan", "status"]);
    assert_eq!(state["phase"], serde_json::json!("done"));
    assert!(state["finishedAt"].as_i64().unwrap_or(0) > 0);

    // `scan cancel` refuses honestly: the only scan this process could
    // signal is always already over (`scan start` blocks), and an app-side
    // scan lives in the app's process.
    let error = execute_error(&["pinvou", "knowledge", "scan", "cancel"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    let message = error.to_string();
    assert!(
        message.contains("knowledge_scan_cancel_requires_product_host"),
        "{message}"
    );
}

/// A missing or non-directory `--root` is refused before the scan starts: a
/// root that walks to nothing would otherwise report a cheerful `done` for a
/// scan that indexed zero files, so the typo must fail loudly and name the
/// path. This is the usability guard; index safety against an unrelated root
/// is the sweep's own contract, asserted by the regression test below.
#[test]
fn scan_start_refuses_a_missing_or_non_directory_root_before_any_wipe() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("scan-preflight");
    let root = home.path().join("docs");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("note.txt"), "pinvou scan fixture").unwrap();

    let seeded = run_json(&[
        "pinvou",
        "knowledge",
        "scan",
        "start",
        "--root",
        root.to_str().unwrap(),
    ]);
    assert_eq!(seeded["phase"], serde_json::json!("done"));

    let missing = home.path().join("docs-typo");
    let error = execute_error(&[
        "pinvou",
        "knowledge",
        "scan",
        "start",
        "--root",
        missing.to_str().unwrap(),
    ]);
    assert!(
        error.to_string().contains("does not exist"),
        "the refusal must name the missing root, got: {error}"
    );

    let file = home.path().join("plain.txt");
    std::fs::write(&file, "not a directory").unwrap();
    let error = execute_error(&[
        "pinvou",
        "knowledge",
        "scan",
        "start",
        "--root",
        file.to_str().unwrap(),
    ]);
    assert!(
        error.to_string().contains("not a directory"),
        "the refusal must name the non-directory root, got: {error}"
    );

    // The seeded index must be intact: the refused scans never started, so
    // the stale sweep never ran.
    let stats = run_json(&["pinvou", "knowledge", "stats"]);
    assert_eq!(
        stats["totalFiles"],
        serde_json::json!(1),
        "a refused scan must not wipe the index"
    );
}

/// Scanning one directory must never delete what another directory put in
/// the index. The incremental sweep deletes the entries it did not re-visit,
/// which is correct for the GUI (it always scans the user home, so every
/// indexed path is in scope) but catastrophic for `--root`: an existing
/// directory passes every pre-flight, so before the sweep was scoped to the
/// walked roots, `scan start --root <B>` reported `done` with exit 0 while
/// silently deleting everything indexed from A.
#[test]
fn scan_start_on_an_unrelated_root_keeps_the_entries_indexed_from_another_root() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("scan-root-scope");
    let docs_a = home.path().join("docsA");
    let docs_b = home.path().join("docsB");
    std::fs::create_dir_all(&docs_a).unwrap();
    std::fs::create_dir_all(&docs_b).unwrap();
    for index in 0..5 {
        std::fs::write(docs_a.join(format!("rootA-{index}.txt")), "from root A").unwrap();
    }
    std::fs::write(docs_b.join("rootB.txt"), "from root B").unwrap();

    let seeded = run_json(&[
        "pinvou",
        "knowledge",
        "scan",
        "start",
        "--root",
        docs_a.to_str().unwrap(),
    ]);
    assert_eq!(seeded["phase"], serde_json::json!("done"));
    let stats = run_json(&["pinvou", "knowledge", "stats"]);
    assert_eq!(
        stats["totalFiles"],
        serde_json::json!(5),
        "root A must seed its five files"
    );

    let second = run_json(&[
        "pinvou",
        "knowledge",
        "scan",
        "start",
        "--root",
        docs_b.to_str().unwrap(),
    ]);
    assert_eq!(second["phase"], serde_json::json!("done"));

    // Root A was never walked this time, so none of its entries may be
    // classified as stale: the index is A's five plus B's one.
    let stats = run_json(&["pinvou", "knowledge", "stats"]);
    assert_eq!(
        stats["totalFiles"],
        serde_json::json!(6),
        "scanning root B must add to the index, never wipe root A"
    );
    let hits = run_json(&["pinvou", "knowledge", "search", "rootA"]);
    let hits = hits["hits"].as_array().expect("hits array");
    assert!(
        hits.iter()
            .any(|hit| hit["name"] == serde_json::json!("rootA-0.txt")),
        "a file indexed from root A must still be searchable after scanning root B"
    );

    // A file genuinely removed from a root that IS walked is still swept,
    // so scoping the sweep did not disable it.
    std::fs::remove_file(docs_b.join("rootB.txt")).unwrap();
    let third = run_json(&[
        "pinvou",
        "knowledge",
        "scan",
        "start",
        "--root",
        docs_b.to_str().unwrap(),
    ]);
    assert_eq!(third["phase"], serde_json::json!("done"));
    let stats = run_json(&["pinvou", "knowledge", "stats"]);
    assert_eq!(
        stats["totalFiles"],
        serde_json::json!(5),
        "a file that vanished under a walked root must still be swept"
    );
}

/// The walker does not follow symlinks, so a symlinked `--root` would key the
/// same files a second time under the link's path — and, one scan later,
/// sweep the originals as stale. `scan start` canonicalizes the root first,
/// so scanning through the link is scanning the directory itself: no
/// duplicates, no wipe.
#[test]
#[cfg(unix)]
fn scan_start_through_a_symlinked_root_neither_duplicates_nor_wipes_entries() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("scan-symlink-root");
    let docs = home.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("linked-one.txt"), "first").unwrap();
    std::fs::write(docs.join("linked-two.txt"), "second").unwrap();
    let link = home.path().join("docs-link");
    std::os::unix::fs::symlink(&docs, &link).unwrap();

    let seeded = run_json(&[
        "pinvou",
        "knowledge",
        "scan",
        "start",
        "--root",
        docs.to_str().unwrap(),
    ]);
    assert_eq!(seeded["phase"], serde_json::json!("done"));
    let stats = run_json(&["pinvou", "knowledge", "stats"]);
    assert_eq!(stats["totalFiles"], serde_json::json!(2));

    let through_link = run_json(&[
        "pinvou",
        "knowledge",
        "scan",
        "start",
        "--root",
        link.to_str().unwrap(),
    ]);
    assert_eq!(through_link["phase"], serde_json::json!("done"));
    let stats = run_json(&["pinvou", "knowledge", "stats"]);
    assert_eq!(
        stats["totalFiles"],
        serde_json::json!(2),
        "scanning through a symlink must re-visit the same keys, not mint new ones"
    );
    let hits = run_json(&["pinvou", "knowledge", "search", "linked-one"]);
    let hits = hits["hits"].as_array().expect("hits array");
    let named: Vec<&serde_json::Value> = hits
        .iter()
        .filter(|hit| hit["name"] == serde_json::json!("linked-one.txt"))
        .collect();
    assert_eq!(named.len(), 1, "the file must be indexed exactly once");
    // The surviving key is the canonical one, not a second key minted under
    // the link: re-keying under the link would look identical in the totals
    // while having deleted every original entry as stale.
    let canonical = std::fs::canonicalize(&docs).unwrap();
    assert_eq!(
        named[0]["path"],
        serde_json::json!(canonical.join("linked-one.txt").to_str().unwrap()),
        "the entry must stay keyed by its canonical path"
    );
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
        "scan start must begin scanning or finish immediately"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let state = run_json(&["pinvou", "knowledge", "scan", "status"]);
        if state["phase"] == serde_json::json!("done") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "scan did not finish in time"
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
fn model_status_reports_disk_state_and_model_cancel_refuses_honestly() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = TempHome::new("model-status");

    let status = run_json(&["pinvou", "knowledge", "model", "status"]);
    assert_eq!(status["version"], serde_json::json!("bge-m3"));
    assert_eq!(status["installed"], serde_json::json!(false));
    assert_eq!(status["ready"], serde_json::json!(false));
    // `ready` is `semantic_ready()` — "is the ~570MB model loaded in THIS
    // process" — and a one-shot CLI never loads it, so it reads false even
    // when the desktop app has it resident. JSON must carry the same
    // process-local marker, otherwise a script gating on `.ready` can never
    // proceed and cannot tell why.
    assert_eq!(status["scope"], serde_json::json!("process-local"));
    let model_dir = status["model_dir"].as_str().expect("model dir");
    // Compare on separators-normalized text: the assertion must hold on
    // Windows too (this suite only runs on ubuntu in CI, but the CLI itself
    // is cross-platform and developers run it locally there).
    let normalized = model_dir.replace(std::path::MAIN_SEPARATOR, "/");
    assert!(
        normalized.ends_with("knowledge/models/bge-m3"),
        "{model_dir}"
    );

    // `model cancel` refuses honestly: the cancel flag is process-local to
    // the desktop app's download orchestration and a CLI process never has
    // a download of its own in flight, so the old `{"cancelled":true}` was
    // a no-op reported as success.
    let error = execute_error(&["pinvou", "knowledge", "model", "cancel"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    let message = error.to_string();
    assert!(
        message.contains("knowledge_model_cancel_requires_product_host")
            && message.contains("nothing in this process"),
        "{message}"
    );
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

/// `index cancel` on a stranded RUNNING job is a real, targeted cancel: the
/// store's `ImportJobStore::cancel` is synchronous and transactional — the
/// job row flips to `cancelled`, its pending items to `cancelled`, and its
/// staged chunks are deleted inside the call — so a CLI process can end a
/// job another process orphaned (or is still running; the live worker
/// observes the cancel at its next per-item checkpoint) without touching any
/// other job. The cancel must be reported as signalled, and the job must
/// actually read `cancelled` afterwards. Driven through the real binary
/// against a SIGKILLed add-sources child's stranded job.
#[test]
fn index_cancel_targets_a_stranded_running_job() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("index-cancel-stranded");
    let bin = env!("CARGO_BIN_EXE_pinvou");
    let run = |args: &[&str]| {
        let mut command = std::process::Command::new(bin);
        command
            .args(args)
            .env("PINVOU3_HOME", home.path())
            .env("PINVOU_NO_COLOR", "1");
        command.output().expect("binary runs")
    };
    let created = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "cancellable",
    ]);
    let id = created["id"].as_i64().expect("created collection id");

    let sources = write_bulk_sources(&home, 2);
    let job_id = strand_running_job(&home, id, &sources);

    let cancelled = run(&["knowledge", "index", "cancel", &job_id]);
    assert!(cancelled.status.success(), "index cancel must succeed");
    let stdout = String::from_utf8_lossy(&cancelled.stdout);
    assert!(
        stdout.contains("signalled"),
        "the cancel must report the signal it landed"
    );
    assert!(!stdout.contains("nothing was signalled"));

    // The job really is cancelled in the DB (read per-job, not through the
    // latest-job ordering that ranks `cancelled` last).
    let state = run_json(&["pinvou", "knowledge", "index", "status", &job_id]);
    assert_eq!(state["jobId"], serde_json::json!(job_id));
    assert_eq!(
        state["phase"],
        serde_json::json!("cancelled"),
        "the stranded job must actually be cancelled"
    );
    assert_eq!(state["running"], serde_json::json!(false));
    assert_eq!(state["resumable"], serde_json::json!(false));

    // The cancelled job is finished, so a second cancel honestly reports
    // there was nothing left to signal.
    let again = run(&["knowledge", "index", "cancel", &job_id]);
    assert!(again.status.success(), "second index cancel must succeed");
    let stdout = String::from_utf8_lossy(&again.stdout);
    assert!(
        stdout.contains("nothing was signalled"),
        "the second cancel must report nothing was signalled"
    );
}

/// `collections delete` must never boot the session store: the GUI's mount
/// sweep is always empty in a one-shot CLI process, while the boot would
/// enforce the 50-sessions-per-kind retention and irreversibly evict the
/// user's oldest non-pinned sessions as a side effect of a delete. Proof by
/// occupation: the sessions root is a regular file, so any
/// `SessionStore::boot()` would fail loudly ("session store unavailable") —
/// the delete must succeed anyway, and must never report a mount sweep.
#[test]
fn collections_delete_never_boots_the_session_store() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("delete-no-session-store");
    std::fs::write(home.path().join("sessions"), b"not a directory").unwrap();

    let created = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "doomed",
    ]);
    let id = created["id"].as_i64().expect("created collection id");

    let outcome = run_ok(&[
        "pinvou",
        "knowledge",
        "collections",
        "delete",
        &id.to_string(),
        "--yes",
    ]);
    assert!(
        !outcome.contains("session store unavailable"),
        "delete must not open the session store: {outcome}"
    );
    let listed = run_json(&["pinvou", "knowledge", "collections", "list"]);
    let collections = listed["collections"].as_array().expect("collections array");
    assert!(
        collections.iter().all(|c| c["id"] != serde_json::json!(id)),
        "the collection must actually be gone"
    );
}

/// `collections delete` of a collection whose import was stranded running by
/// a hard-killed process must cancel THAT job (a real, targeted,
/// DB-level cancel) and delete the collection — a one-shot CLI is exactly
/// the janitor for a job its sibling process orphaned. The stranded state is
/// produced by SIGKILLing a real add-sources child mid-import.
#[test]
fn collections_delete_cancels_its_own_stranded_job_and_deletes() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new("delete-stranded-job");

    let created = run_json(&[
        "pinvou",
        "knowledge",
        "collections",
        "create",
        "--name",
        "stranded",
    ]);
    let id = created["id"].as_i64().expect("created collection id");
    let sources = write_bulk_sources(&home, 2);
    strand_running_job(&home, id, &sources);

    run_ok(&[
        "pinvou",
        "knowledge",
        "collections",
        "delete",
        &id.to_string(),
        "--yes",
    ]);

    // The job rows are gone with the collection (delete_collection removes
    // the job's staged chunks and the cascade clears the rest), so the
    // import-job store must be back to its idle no-job state — never a
    // stranded running job left behind.
    let state = run_json(&["pinvou", "knowledge", "index", "status"]);
    assert_eq!(state["jobId"], serde_json::Value::Null);
    assert_eq!(state["phase"], serde_json::json!("idle"));

    let listed = run_json(&["pinvou", "knowledge", "collections", "list"]);
    let collections = listed["collections"].as_array().expect("collections array");
    assert!(
        collections.iter().all(|c| c["id"] != serde_json::json!(id)),
        "the collection must be deleted"
    );
}
