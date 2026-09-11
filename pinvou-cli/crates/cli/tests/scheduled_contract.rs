//! Contract tests for the `scheduled` family (`crates/cli/src/scheduled.rs`).
//!
//! Parse-level invalid-usage coverage (exit-code 2 errors for bad rrules incl.
//! minute-level recurrences, bad kind/mode, missing `--prompt-file`,
//! `delete` without `--yes`) lives here; the typed parse assertions live in
//! the module's own `#[cfg(test)]` block (the command enum is not re-exported
//! at the crate root). Execute-level tests run against a temp `PINVOU3_HOME`
//! (serialized through ENV_LOCK, following cli_contract.rs) and assert
//! through the same store files and `pinvou3_lib` types the GUI uses; they
//! never touch the network, a model, or a display (AGENTS.md rule).
//!
//! `scheduled run` is the only host path (windowless product host: display +
//! active model). The chat-kind refusal is tested hermetically; the
//! memory-organize execution is covered by an `#[ignore]`d opt-in test.

use pinvou_cli::{CliCommand, ExitCode, execute, parse_args};
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
            "pinvou-cli-scheduled-{label}-{}-{}",
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

    /// The `AutomationManager::open(<home>/automations)` layout the GUI uses:
    /// definitions under `automations/automations`, runs under
    /// `automations/runs/<task_id>`.
    fn def_path(&self, task_id: &str) -> PathBuf {
        self.root
            .join("automations")
            .join("automations")
            .join(format!("{task_id}.json"))
    }

    fn runs_dir(&self, task_id: &str) -> PathBuf {
        self.root.join("automations").join("runs").join(task_id)
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

fn parsed_scheduled(arguments: &[&str]) -> CliCommand {
    let owned: Vec<String> = std::iter::once("pinvou".to_owned())
        .chain(arguments.iter().map(|value| value.to_string()))
        .collect();
    match parse_args(owned) {
        Ok(parsed) => parsed.command().clone(),
        Err(error) => panic!("expected valid scheduled command: {error}"),
    }
}

fn run_json(arguments: &[&str]) -> serde_json::Value {
    let mut owned: Vec<String> = std::iter::once("pinvou".to_owned())
        .chain(arguments.iter().map(|value| value.to_string()))
        .collect();
    owned.push("--output".to_owned());
    owned.push("json".to_owned());
    let parsed = parse_args(owned).expect("valid scheduled command");
    let outcome = execute(parsed).expect("successful scheduled command");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    serde_json::from_str(&outcome.stdout).expect("single-line JSON output")
}

fn run_human(arguments: &[&str]) -> String {
    let mut owned: Vec<String> = std::iter::once("pinvou".to_owned())
        .chain(arguments.iter().map(|value| value.to_string()))
        .collect();
    owned.push("--output".to_owned());
    owned.push("human".to_owned());
    let parsed = parse_args(owned).expect("valid scheduled command");
    let outcome = execute(parsed).expect("successful scheduled command");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    outcome.stdout
}

fn expect_failed(arguments: &[&str]) -> String {
    let owned: Vec<String> = std::iter::once("pinvou".to_owned())
        .chain(arguments.iter().map(|value| value.to_string()))
        .collect();
    let parsed = parse_args(owned).expect("parseable scheduled command");
    let error = execute(parsed).expect_err("expected host/runtime failure");
    assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
    error.to_string()
}

fn assert_usage(arguments: &[&str]) {
    let mut owned: Vec<String> = std::iter::once("pinvou".to_owned())
        .chain(arguments.iter().map(|value| value.to_string()))
        .collect();
    let error = match parse_args(owned.drain(..)) {
        Err(error) => error,
        Ok(parsed) => execute(parsed).expect_err("expected usage error"),
    };
    assert_eq!(error.exit_code(), ExitCode::Usage, "{error}");
}

fn write_prompt_file(home: &TempHome, name: &str, prompt: &str) -> PathBuf {
    let path = home.path().join(name);
    std::fs::write(&path, prompt).unwrap();
    path
}

const VALID_RRULE: &str = "FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=9;BYMINUTE=30";

fn create_task(home: &TempHome, name: &str) -> serde_json::Value {
    let prompt = write_prompt_file(home, &format!("{name}.md"), "Summarize the reports.");
    run_json(&[
        "scheduled",
        "create",
        "--name",
        name,
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        VALID_RRULE,
    ])
}

// ---- parse level: invalid shapes are exit-code 2 usage errors ----

#[test]
fn rejects_invalid_usage_with_exit_code_two() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let invalid = [
        vec!["scheduled"],
        vec!["scheduled", "bogus"],
        vec!["scheduled", "list", "--extra"],
        vec!["scheduled", "show"],
        vec!["scheduled", "show", "t-1", "--full"],
        // create: required flags
        vec!["scheduled", "create"],
        vec!["scheduled", "create", "--name", "N"],
        vec!["scheduled", "create", "--name", "N", "--rrule", VALID_RRULE],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
        ],
        // create: bad rrules, including the minute-level recurrence the GUI
        // rejects (no FREQ=MINUTELY / DAILY grammar).
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            "FREQ=MINUTELY;INTERVAL=5",
        ],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            "FREQ=DAILY;BYHOUR=9;BYMINUTE=30",
        ],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            "FREQ=WEEKLY;BYDAY=XX;BYHOUR=9;BYMINUTE=30",
        ],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            "FREQ=HOURLY;INTERVAL=0",
        ],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            "FREQ=WEEKLY;BYDAY=MO;BYHOUR=24;BYMINUTE=30",
        ],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            "FREQ=ONCE",
        ],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            "FREQ=ONCE;AT=not-a-date",
        ],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            "FREQ=CRON;EXPR=61 * * * *",
        ],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            "FREQ=CRON;EXPR=* * * *",
        ],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            "FREQ=WEEKLY;WEEKDAY=MO;BYHOUR=9;BYMINUTE=30",
        ],
        // create: bad kind / mode / duplicates
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            VALID_RRULE,
            "--kind",
            "backup",
        ],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            VALID_RRULE,
            "--mode",
            "bogus",
        ],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--name",
            "M",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            VALID_RRULE,
        ],
        // update: no-op and bad flags
        vec!["scheduled", "update", "t-1"],
        vec!["scheduled", "update", "t-1", "--kind", "chat"],
        vec![
            "scheduled",
            "update",
            "t-1",
            "--rrule",
            "FREQ=MINUTELY;INTERVAL=1",
        ],
        // delete/runs/mark-viewed shapes
        vec!["scheduled", "delete"],
        vec!["scheduled", "delete", "t-1", "--yes", "--nope"],
        vec!["scheduled", "pause"],
        vec!["scheduled", "runs"],
        vec!["scheduled", "runs", "t-1", "--limit", "0"],
        vec!["scheduled", "runs", "t-1", "--limit", "x"],
        vec!["scheduled", "runs-all", "--limit"],
        vec!["scheduled", "mark-viewed", "t-1"],
        vec!["scheduled", "mark-viewed", "t-1", "r-1", "--force"],
        vec!["scheduled", "chat-prompt", "--extra"],
    ];
    for arguments in &invalid {
        assert_usage(arguments);
    }
}

#[test]
fn every_subcommand_shape_parses() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for arguments in [
        vec!["scheduled", "list"],
        vec!["scheduled", "show", "t-1"],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            VALID_RRULE,
        ],
        vec![
            "scheduled",
            "create",
            "--name",
            "N",
            "--prompt-file",
            "prompt.md",
            "--rrule",
            "FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
            "--kind",
            "memory-organize",
            "--model-id",
            "m-1",
            "--mode",
            "plan",
            "--paused",
        ],
        vec!["scheduled", "update", "t-1", "--name", "New"],
        vec!["scheduled", "pause", "t-1"],
        vec!["scheduled", "resume", "t-1"],
        vec!["scheduled", "pin", "t-1"],
        vec!["scheduled", "unpin", "t-1"],
        vec!["scheduled", "delete", "t-1"],
        vec!["scheduled", "delete", "t-1", "--yes"],
        vec!["scheduled", "run", "t-1"],
        vec!["scheduled", "runs", "t-1", "--limit", "5"],
        vec!["scheduled", "runs-all"],
        vec!["scheduled", "runs-all", "--limit", "3"],
        vec!["scheduled", "mark-viewed", "t-1", "r-1"],
        vec!["scheduled", "chat-prompt"],
    ] {
        assert!(
            matches!(parsed_scheduled(&arguments), CliCommand::Scheduled(_)),
            "{arguments:?}"
        );
    }
}

// ---- execute level (temp PINVOU3_HOME, no engine/network) ----

#[test]
fn empty_home_lists_nothing_and_chat_prompt_is_available() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("empty");
    let value = run_json(&["scheduled", "list"]);
    assert_eq!(value["tasks"].as_array().map(Vec::len), Some(0));
    assert!(run_human(&["scheduled", "list"]).contains("No scheduled tasks."));
    let runs = run_json(&["scheduled", "runs-all"]);
    assert_eq!(runs["runs"].as_array().map(Vec::len), Some(0));
    // chat-prompt mirrors the fixed GUI AI task-creation prompt.
    let prompt = run_json(&["scheduled", "chat-prompt"]);
    assert!(
        prompt["prompt"]
            .as_str()
            .unwrap()
            .contains("scheduled-task-draft"),
        "chat prompt must carry the GUI draft-block instructions"
    );
    let _ = home;
}

#[test]
fn runs_and_show_reject_unknown_task_ids() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("unknown-id");
    expect_failed(&["scheduled", "show", "does-not-exist"]);
    expect_failed(&["scheduled", "runs", "does-not-exist"]);
    expect_failed(&["scheduled", "mark-viewed", "does-not-exist", "run-1"]);
    expect_failed(&["scheduled", "delete", "does-not-exist", "--yes"]);
    let _ = home;
}

#[test]
fn create_list_show_update_pause_resume_pin_round_trip_and_delete() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("round-trip");

    // create: mirrors the persisted GUI shape (mode forced to yolo, trust and
    // auto-approve on, workspace-owned cwds kept out of the DTO).
    let created = create_task(&home, "Report");
    let task_id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["name"].as_str(), Some("Report"));
    assert_eq!(
        created["rrule"].as_str(),
        Some("FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=9;BYMINUTE=30")
    );
    assert_eq!(created["status"].as_str(), Some("active"));
    assert_eq!(created["mode"].as_str(), Some("yolo"));
    assert_eq!(created["trustMode"].as_bool(), Some(true));
    assert_eq!(created["autoApprove"].as_bool(), Some(true));
    // Model mirrors current_automation_model: the active saved model's wire
    // name (a fresh home migrates in the builtin default saved model).
    let expected_model = pinvou3_lib::platform::prefs::UserPrefs::load()
        .active_model()
        .map(|model| model.model.clone())
        .unwrap_or_else(|| "default-model".to_owned());
    assert_eq!(created["model"].as_str(), Some(expected_model.as_str()));
    assert_eq!(created["kind"], serde_json::Value::Null);
    assert_eq!(created["cwds"].as_array().map(Vec::len), Some(0));
    // Schedule math is the foundation scheduler's job (see module docs).
    assert_eq!(created["nextRunAt"], serde_json::Value::Null);
    // The definition file lives where the GUI's AutomationManager reads it.
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert_eq!(def["schema_version"].as_u64(), Some(2));
    assert_eq!(def["status"].as_str(), Some("active"));
    assert_eq!(def["cwds"].as_array().unwrap().len(), 1);

    // list
    let listed = run_json(&["scheduled", "list"]);
    let tasks = listed["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"].as_str(), Some(task_id.as_str()));

    // show: task detail plus its (empty) recent runs
    let shown = run_json(&["scheduled", "show", &task_id]);
    assert_eq!(shown["id"].as_str(), Some(task_id.as_str()));
    assert_eq!(shown["runs"].as_array().map(Vec::len), Some(0));

    // update: name + rrule (uppercased like the GUI), next slot deferred
    let updated = run_json(&[
        "scheduled",
        "update",
        &task_id,
        "--name",
        "Weekly report",
        "--rrule",
        "FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
    ]);
    assert_eq!(updated["name"].as_str(), Some("Weekly report"));
    assert_eq!(
        updated["rrule"].as_str(),
        Some("FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30")
    );
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert_eq!(def["next_run_at"], serde_json::Value::Null);

    // pause / resume flip the status like the GUI pause/resume commands
    let paused = run_json(&["scheduled", "pause", &task_id]);
    assert_eq!(paused["status"].as_str(), Some("paused"));
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert_eq!(def["next_run_at"], serde_json::Value::Null);
    let resumed = run_json(&["scheduled", "resume", &task_id]);
    assert_eq!(resumed["status"].as_str(), Some("active"));

    // pin / unpin write the UI-metadata sidecar the GUI reads back
    let pinned = run_json(&["scheduled", "pin", &task_id]);
    assert_eq!(pinned["action"].as_str(), Some("pinned"));
    let listed = run_json(&["scheduled", "list"]);
    assert_eq!(
        listed["tasks"][0]["pinned"].as_bool(),
        Some(true),
        "{listed}"
    );
    let shown = run_json(&["scheduled", "show", &task_id]);
    assert_eq!(shown["pinned"].as_bool(), Some(true));
    run_json(&["scheduled", "unpin", &task_id]);
    let listed = run_json(&["scheduled", "list"]);
    assert_eq!(listed["tasks"][0]["pinned"].as_bool(), Some(false));

    // runs on a task without runs
    let runs = run_json(&["scheduled", "runs", &task_id]);
    assert_eq!(runs["runs"].as_array().map(Vec::len), Some(0));

    // delete requires --yes at execute time, then archives the history the
    // way the GUI delete does (runs survive in history-archive.json).
    assert_usage(&["scheduled", "delete", &task_id]);
    let deleted = run_json(&["scheduled", "delete", &task_id, "--yes"]);
    assert_eq!(deleted["id"].as_str(), Some(task_id.as_str()));
    assert_eq!(
        deleted["deletedSessionIds"].as_array().map(Vec::len),
        Some(0)
    );
    assert!(!home.def_path(&task_id).exists());
    let listed = run_json(&["scheduled", "list"]);
    assert_eq!(listed["tasks"].as_array().map(Vec::len), Some(0));
    // The archived history keeps the run feed complete after deletion.
    let archive: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.path().join("automations/history-archive.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        archive["tasks"][&task_id]["task"]["id"].as_str(),
        Some(task_id.as_str())
    );
}

#[test]
fn create_rejects_memory_organize_kind_while_memory_is_disabled() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("memory-gate");
    // Fresh home: memory is disabled by default, exactly like the GUI create
    // dialog's refusal for memory-organize tasks.
    let prompt = write_prompt_file(&home, "prompt.md", "Organize the memory stores.");
    let arguments = [
        "scheduled",
        "create",
        "--name",
        "Nightly organize",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        "FREQ=HOURLY;INTERVAL=12",
        "--kind",
        "memory-organize",
    ];
    let error = expect_failed(&arguments);
    assert!(
        error.starts_with("scheduled_memory_organize_disabled"),
        "{error}"
    );
    let _ = home;
}

#[test]
fn chat_kind_run_refuses_headless_execution_with_stable_error() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("chat-run");
    let created = create_task(&home, "Chat task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    let error = expect_failed(&["scheduled", "run", &task_id]);
    assert!(
        error.starts_with("scheduled_chat_run_requires_product_host"),
        "{error}"
    );
    // No run record may be fabricated for the refused chat run.
    assert!(!home.runs_dir(&task_id).exists());
    let _ = home;
}

#[test]
fn delete_refuses_while_a_run_is_active() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("delete-active");
    let created = create_task(&home, "Busy task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    // A queued/running run left by the GUI-owned runtime (it carries the
    // foundation task id) blocks deletion, matching the GUI's
    // cancel-then-delete contract: headless, there is nothing to cancel with.
    std::fs::create_dir_all(home.runs_dir(&task_id)).unwrap();
    std::fs::write(
        home.runs_dir(&task_id).join("active-run.json"),
        serde_json::json!({
            "schema_version": 1,
            "id": "active-run",
            "automation_id": task_id,
            "task_id": "foundation-task-1",
            "scheduled_for": "2026-09-10T08:00:00.000Z",
            "status": "running",
            "created_at": "2026-09-10T08:00:00.000Z"
        })
        .to_string(),
    )
    .unwrap();
    let error = expect_failed(&["scheduled", "delete", &task_id, "--yes"]);
    assert!(error.starts_with("scheduled_delete_blocked"), "{error}");
    assert!(home.def_path(&task_id).exists());

    // A queued record with no task id is CLI bookkeeping (a CLI process
    // killed mid-run) that no runtime can cancel; it must not wedge the task
    // forever, so delete proceeds past it.
    std::fs::write(
        home.runs_dir(&task_id).join("stranded-run.json"),
        serde_json::json!({
            "schema_version": 1,
            "id": "stranded-run",
            "automation_id": task_id,
            "task_id": serde_json::Value::Null,
            "scheduled_for": "2026-09-10T08:00:00.000Z",
            "status": "queued",
            "created_at": "2026-09-10T08:00:00.000Z"
        })
        .to_string(),
    )
    .unwrap();
    std::fs::remove_file(home.runs_dir(&task_id).join("active-run.json")).unwrap();
    let value = run_json(&["scheduled", "delete", &task_id, "--yes"]);
    assert_eq!(value["id"], task_id.as_str());
    assert!(!home.def_path(&task_id).exists());
    let _ = home;
}

#[test]
fn run_reconciles_a_stranded_queued_record_before_running() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("run-reconcile");
    let created = create_task(&home, "Reconciled task");
    let task_id = created["id"].as_str().unwrap().to_owned();

    // A CLI process killed mid-run leaves a queued record with no task id.
    std::fs::create_dir_all(home.runs_dir(&task_id)).unwrap();
    std::fs::write(
        home.runs_dir(&task_id)
            .join("20260910T080000000Z-stranded.json"),
        serde_json::json!({
            "schema_version": 1,
            "id": "stranded",
            "automation_id": task_id,
            "task_id": serde_json::Value::Null,
            "scheduled_for": "2026-09-10T08:00:00.000Z",
            "status": "queued",
            "created_at": "2026-09-10T08:00:00.000Z"
        })
        .to_string(),
    )
    .unwrap();

    // The next run reconciles it to a terminal failed record instead of
    // accumulating undeletable state (this environment has no display/model,
    // so the new run itself then fails honestly; the runs list still shows
    // the reconciled record).
    let error = expect_failed(&["scheduled", "run", &task_id]);
    assert!(!error.is_empty(), "{error}");
    let reconciled = std::fs::read_to_string(
        home.runs_dir(&task_id)
            .join("20260910T080000000Z-stranded.json"),
    )
    .unwrap();
    assert!(
        reconciled.contains("\"failed\""),
        "stranded record must become terminal: {reconciled}"
    );
    let _ = home;
}

#[test]
fn once_at_rejects_calendar_overflow_like_the_foundation() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("once-at-calendar");
    let prompt = write_prompt_file(&home, "once.md", "Summarize the reports.");
    // 2026-02-30 cannot exist; the foundation parser rejects it and one such
    // record would fail the GUI scheduler's whole sweep.
    for bad in [
        "FREQ=ONCE;AT=2026-02-30T08:30",
        "FREQ=ONCE;AT=2025-02-29T08:30",
    ] {
        let error = assert_validation_fail(&[
            "scheduled",
            "create",
            "--name",
            "bad once",
            "--prompt-file",
            prompt.to_str().unwrap(),
            "--rrule",
            bad,
        ]);
        assert!(error.contains("calendar"), "{error}");
    }
    // A real date passes validation (the create then proceeds to the
    // next-run recompute).
    let _ = home;
}

fn assert_validation_fail(arguments: &[&str]) -> String {
    let mut owned: Vec<String> = std::iter::once("pinvou".to_owned())
        .chain(arguments.iter().map(|value| value.to_string()))
        .collect();
    match parse_args(owned.drain(..)) {
        Err(error) => error.to_string(),
        Ok(parsed) => {
            let error = execute(parsed).expect_err("expected validation failure");
            error.to_string()
        }
    }
}

#[test]
fn mark_viewed_round_trip_over_completed_run() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("mark-viewed");
    let created = create_task(&home, "Watched task");
    let task_id = created["id"].as_str().unwrap().to_owned();

    // A completed scheduled run with its own conversation, the way the GUI
    // executor leaves the stores after a chat run.
    let store = pinvou3_lib::features::sessions::SessionStore::boot().unwrap();
    let session = store
        .create_scheduled_run(pinvou3_lib::features::sessions::ScheduledRunProfile {
            task_id: task_id.clone(),
            model: "default-model".to_owned(),
            model_id: None,
            workspace: home
                .path()
                .join("scheduled")
                .join(&task_id)
                .join("workspace"),
            mode: pinvou3_lib::features::sessions::ScheduledRunMode::Agent,
            allow_shell: false,
            trust_mode: true,
            auto_approve: true,
        })
        .unwrap();
    let session_id = session.metadata.id.clone();
    let run_id = "completed-run-1";
    std::fs::create_dir_all(home.runs_dir(&task_id)).unwrap();
    std::fs::write(
        home.runs_dir(&task_id).join(format!("{run_id}.json")),
        serde_json::json!({
            "schema_version": 1,
            "id": run_id,
            "automation_id": task_id,
            "scheduled_for": "2026-09-10T08:00:00.000Z",
            "status": "completed",
            "created_at": "2026-09-10T08:00:00.000Z",
            "started_at": "2026-09-10T08:00:01.000Z",
            "ended_at": "2026-09-10T08:05:00.000Z",
            "task_id": "foundation-task-1",
            "thread_id": session_id,
            "turn_id": "turn-1",
            "error": null
        })
        .to_string(),
    )
    .unwrap();

    // The run is unread until viewed.
    let listed = run_json(&["scheduled", "list"]);
    assert_eq!(listed["tasks"][0]["hasUnreadRuns"].as_bool(), Some(true));
    let runs = run_json(&["scheduled", "runs", &task_id]);
    let run = &runs["runs"].as_array().unwrap()[0];
    assert_eq!(run["id"].as_str(), Some(run_id));
    assert_eq!(run["sessionId"].as_str(), Some(session_id.as_str()));
    assert_eq!(run["unread"].as_bool(), Some(true));

    // Wrong-run and not-completed lookups fail without touching the state.
    expect_failed(&["scheduled", "mark-viewed", &task_id, "other-run"]);
    std::fs::write(
        home.runs_dir(&task_id).join("queued-run.json"),
        serde_json::json!({
            "schema_version": 1,
            "id": "queued-run",
            "automation_id": task_id,
            "scheduled_for": "2026-09-10T09:00:00.000Z",
            "status": "queued",
            "created_at": "2026-09-10T09:00:00.000Z"
        })
        .to_string(),
    )
    .unwrap();
    let error = expect_failed(&["scheduled", "mark-viewed", &task_id, "queued-run"]);
    assert!(error.starts_with("scheduled_run_not_completed"), "{error}");

    // mark-viewed persists the read state the GUI shares and clears unread.
    let marked = run_json(&["scheduled", "mark-viewed", &task_id, run_id]);
    assert_eq!(marked["automationId"].as_str(), Some(task_id.as_str()));
    assert_eq!(marked["runId"].as_str(), Some(run_id));
    assert_eq!(marked["hasUnreadRuns"].as_bool(), Some(false));
    let read_state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.path().join("scheduled-runs/read-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        read_state["viewed_runs"][&task_id][0].as_str(),
        Some(run_id)
    );
    let listed = run_json(&["scheduled", "list"]);
    assert_eq!(listed["tasks"][0]["hasUnreadRuns"].as_bool(), Some(false));
    let runs = run_json(&["scheduled", "runs", &task_id]);
    assert_eq!(runs["runs"][0]["unread"].as_bool(), Some(false));
    // Marking again is idempotent.
    run_json(&["scheduled", "mark-viewed", &task_id, run_id]);
    let _ = home;
}

#[test]
fn runs_all_merges_active_and_archived_runs_with_limit() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("runs-all");
    let created = create_task(&home, "Feed task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    std::fs::create_dir_all(home.runs_dir(&task_id)).unwrap();
    for (index, status) in ["completed", "failed"].iter().enumerate() {
        std::fs::write(
            home.runs_dir(&task_id).join(format!("run-{index}.json")),
            serde_json::json!({
                "schema_version": 1,
                "id": format!("run-{index}"),
                "automation_id": task_id,
                "scheduled_for": format!("2026-09-1{index}T08:00:00.000Z"),
                "status": status,
                "created_at": format!("2026-09-1{index}T08:00:00.000Z"),
                "error": if *status == "failed" {
                    serde_json::json!("memory organize: boom")
                } else {
                    serde_json::Value::Null
                }
            })
            .to_string(),
        )
        .unwrap();
    }
    // Deleted tasks keep their runs visible through the history archive.
    let archived_task_id = "archived-task";
    std::fs::create_dir_all(home.path().join("automations")).unwrap();
    std::fs::write(
        home.path().join("automations/history-archive.json"),
        serde_json::json!({
            "schema_version": 2,
            "tasks": {
                archived_task_id: {
                    "task": { "id": archived_task_id, "name": "Old task" },
                    "runs": [{
                        "schema_version": 1,
                        "id": "archived-run",
                        "automation_id": archived_task_id,
                        "scheduled_for": "2026-09-05T08:00:00.000Z",
                        "status": "completed",
                        "created_at": "2026-09-05T08:00:00.000Z"
                    }],
                    "deleted_at": "2026-09-06T08:00:00.000Z"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let all = run_json(&["scheduled", "runs-all"]);
    let runs = all["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 3, "{all}");
    assert_eq!(runs[0]["id"].as_str(), Some("run-1"));
    assert_eq!(runs[0]["taskName"].as_str(), Some("Feed task"));
    assert_eq!(runs[1]["id"].as_str(), Some("run-0"));
    assert_eq!(runs[2]["id"].as_str(), Some("archived-run"));
    assert_eq!(runs[2]["taskName"].as_str(), Some("Old task"));

    let limited = run_json(&["scheduled", "runs-all", "--limit", "1"]);
    assert_eq!(limited["runs"].as_array().unwrap().len(), 1);

    let per_task = run_json(&["scheduled", "runs", &task_id]);
    let runs = per_task["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0]["error"].as_str(), Some("memory organize: boom"));
    let _ = home;
}

// ---- host paths (opt-in) ----

/// Opt-in smoke for the only engine-backed subcommand: `scheduled run` on a
/// memory-organize task boots the windowless product host and records a
/// terminal run. Requires a display (xvfb on headless Linux), an active,
/// configured model, memory enabled, and a reachable endpoint:
///   pinvou settings set memory_enabled true   # plus a configured model
///   pinvou scheduled create --name Organize --prompt-file p.md \
///     --rrule 'FREQ=HOURLY;INTERVAL=12' --kind memory-organize
///   pinvou scheduled run <task-id>
#[test]
#[ignore = "boots the windowless product host: needs a display/xvfb, an active model, and \
memory enabled; opt in by creating a memory-organize task and running pinvou scheduled run"]
fn run_executes_a_memory_organize_task_through_the_product_host() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("run-organize");
    // The GUI refuses memory-organize tasks while memory is disabled; enable
    // it through the same prefs the feature reads.
    std::fs::write(
        home.path().join("settings.json"),
        serde_json::json!({ "memory_enabled": true }).to_string(),
    )
    .unwrap();
    let prompt = write_prompt_file(&home, "prompt.md", "Organize the memory stores.");
    let created = run_json(&[
        "scheduled",
        "create",
        "--name",
        "Nightly organize",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        "FREQ=HOURLY;INTERVAL=12",
        "--kind",
        "memory-organize",
    ]);
    let task_id = created["id"].as_str().unwrap().to_owned();
    let value = run_json(&["scheduled", "run", &task_id]);
    let run_id = value["id"].as_str().unwrap().to_owned();
    // Memory organize runs own no conversation session.
    assert_eq!(value["sessionId"], serde_json::Value::Null);
    assert!(
        matches!(value["status"].as_str(), Some("completed") | Some("failed")),
        "{}",
        value
    );
    let runs = run_json(&["scheduled", "runs", &task_id]);
    let runs = runs["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["id"].as_str(), Some(run_id.as_str()));
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert!(def["last_run_at"].is_string(), "{def}");
    let _ = home;
}

/// A wrong-shaped but valid read-state payload (hand-edited or partially
/// written file) must normalize to the default instead of panicking with an
/// exit code outside the 0/1/2 contract.
#[test]
fn mark_viewed_normalizes_wrong_shaped_registry_payloads() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("mark-viewed-bad-shape");
    let created = create_task(&home, "Bad shape task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    // A completed run bound to a real scheduled-run session, so the run is
    // actually viewable and the command reaches the registry write.
    let store = pinvou3_lib::features::sessions::SessionStore::boot().unwrap();
    let session = store
        .create_scheduled_run(pinvou3_lib::features::sessions::ScheduledRunProfile {
            task_id: task_id.clone(),
            model: "default-model".to_owned(),
            model_id: None,
            workspace: home
                .path()
                .join("scheduled")
                .join(&task_id)
                .join("workspace"),
            mode: pinvou3_lib::features::sessions::ScheduledRunMode::Agent,
            allow_shell: false,
            trust_mode: true,
            auto_approve: true,
        })
        .unwrap();
    let session_id = session.metadata.id.clone();
    std::fs::create_dir_all(home.runs_dir(&task_id)).unwrap();
    std::fs::write(
        home.runs_dir(&task_id).join("done-1.json"),
        serde_json::json!({
            "schema_version": 1,
            "id": "done-1",
            "automation_id": task_id,
            "scheduled_for": "2026-09-10T08:00:00.000Z",
            "status": "completed",
            "created_at": "2026-09-10T08:00:00.000Z",
            "started_at": "2026-09-10T08:00:01.000Z",
            "ended_at": "2026-09-10T08:05:00.000Z",
            "task_id": "foundation-task-1",
            "thread_id": session_id,
            "turn_id": "turn-1",
            "error": null
        })
        .to_string(),
    )
    .unwrap();

    // `viewed_runs` as an array, and (after normalization) a per-task entry
    // as a string: both used to hit `expect` and crash the process.
    std::fs::create_dir_all(home.root.join("scheduled-runs")).unwrap();
    std::fs::write(
        home.root.join("scheduled-runs").join("read-state.json"),
        serde_json::json!({ "schema_version": 2, "viewed_runs": [] }).to_string(),
    )
    .unwrap();
    let outcome = run_json(&["scheduled", "mark-viewed", &task_id, "done-1"]);
    assert_eq!(outcome["runId"].as_str(), Some("done-1"));

    std::fs::write(
        home.root.join("scheduled-runs").join("read-state.json"),
        serde_json::json!({ "schema_version": 2, "viewed_runs": { &task_id: "bogus" } })
            .to_string(),
    )
    .unwrap();
    let outcome = run_json(&["scheduled", "mark-viewed", &task_id, "done-1"]);
    assert_eq!(outcome["runId"].as_str(), Some("done-1"));
    let _ = home;
}

/// A history archive that is a valid object but lacks the `tasks` key must
/// still receive the deleted task's run snapshot instead of silently
/// dropping the history.
#[test]
fn delete_archives_run_history_when_archive_tasks_key_is_missing() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("delete-archive-missing-tasks");
    let created = create_task(&home, "Archive task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    std::fs::create_dir_all(home.runs_dir(&task_id)).unwrap();
    std::fs::write(
        home.runs_dir(&task_id).join("old-run.json"),
        serde_json::json!({
            "schema_version": 1,
            "id": "old-run",
            "automation_id": task_id,
            "scheduled_for": "2026-09-10T08:00:00.000Z",
            "status": "completed",
            "created_at": "2026-09-10T08:00:00.000Z"
        })
        .to_string(),
    )
    .unwrap();
    // Valid JSON object, but no `tasks` key.
    std::fs::create_dir_all(home.root.join("automations")).unwrap();
    std::fs::write(
        home.root.join("automations").join("history-archive.json"),
        serde_json::json!({ "schema_version": 2 }).to_string(),
    )
    .unwrap();

    run_json(&["scheduled", "delete", &task_id, "--yes"]);

    let archive: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.root.join("automations").join("history-archive.json"))
            .unwrap(),
    )
    .unwrap();
    let snapshot = &archive["tasks"][&task_id];
    assert!(snapshot.is_object(), "deleted task must be archived");
    assert_eq!(snapshot["runs"].as_array().unwrap().len(), 1);
    assert!(!home.def_path(&task_id).exists());
    let _ = home;
}
