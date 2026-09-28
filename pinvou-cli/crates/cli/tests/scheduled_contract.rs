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
use std::process::Command;
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
        let _ = std::fs::remove_dir_all(&self.root);
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

fn assert_usage(arguments: &[&str]) -> String {
    let mut owned: Vec<String> = std::iter::once("pinvou".to_owned())
        .chain(arguments.iter().map(|value| value.to_string()))
        .collect();
    let error = match parse_args(owned.drain(..)) {
        Err(error) => error,
        Ok(parsed) => execute(parsed).expect_err("expected usage error"),
    };
    assert_eq!(error.exit_code(), ExitCode::Usage, "{error}");
    error.to_string()
}

fn write_prompt_file(home: &TempHome, name: &str, prompt: &str) -> PathBuf {
    let path = home.path().join(name);
    std::fs::write(&path, prompt).unwrap();
    path
}

const VALID_RRULE: &str = "FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=9;BYMINUTE=30";

// Bounded read: an over-cap prompt file must fail cleanly (the reason
// `--prompt-file /dev/zero` cannot hang the CLI).
#[test]
fn create_refuses_an_oversized_prompt_file() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("prompt-cap");
    let prompt = write_prompt_file(&home, "big.md", &"x".repeat(4 * 1024 * 1024 + 1));
    let message = expect_failed(&[
        "scheduled",
        "create",
        "--name",
        "cap",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        VALID_RRULE,
    ]);
    assert!(
        message.contains("exceeds the 4194304-byte read limit"),
        "{message}"
    );
}

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

/// Writes the saved-model store `UserPrefs::load` reads: a default (active)
/// local-vllm record and a second DeepSeek record, mirroring the shape the
/// GUI's model list persists (`migrate_models`/`normalize` run on load, so a
/// raw settings.json fixture is the same store the app sees).
fn write_saved_models(home: &TempHome) {
    std::fs::write(
        home.path().join("settings.json"),
        serde_json::json!({
            "memory_enabled": true,
            "advanced": {
                "saved_models": [
                    {
                        "id": "default",
                        "name": "Local Qwen",
                        "preset": "local_vllm",
                        "model": "qwen36_35b_256k",
                        "base_url": "http://127.0.0.1:8000/v1"
                    },
                    {
                        "id": "sel-1",
                        "name": "DeepSeek Flash",
                        "preset": "deepseek",
                        "model": "deepseek-flash",
                        "base_url": "https://api.deepseek.com"
                    }
                ],
                "active_model_id": "default"
            }
        })
        .to_string(),
    )
    .unwrap();
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
        // `agent`/`plan` are refused like any other non-yolo value: the mode
        // is forced to yolo downstream, so accepting them would discard the
        // caller's choice. `scheduled update` already classifies the same
        // refusal as a usage error.
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
            "agent",
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
            "plan",
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
            // `yolo` is the only mode a scheduled task can have; `agent`/
            // `plan` are refused (see create_refuses_non_yolo_modes).
            "--mode",
            "yolo",
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
fn chat_prompt_mirrors_the_gui_once_scheduling_guidance() {
    // The served prompt is a verbatim copy of the GUI's
    // `SCHEDULED_TASK_CHAT_PROMPT` (`features::scheduled::tasks` is
    // `pub(crate)` to `pinvou3_lib`, so the const cannot be referenced from
    // the CLI); these pins make copy drift a CI failure instead of a silent
    // divergence from the GUI's ONCE guidance.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("chat-prompt-once");
    let prompt = run_json(&["scheduled", "chat-prompt"]);
    let prompt = prompt["prompt"].as_str().unwrap();
    assert!(
        prompt.contains("FREQ=ONCE;AT="),
        "the chat prompt must embed the ONCE rule"
    );
    assert!(
        prompt.contains("一次性定时的 AT 只用本地时刻 YYYY-MM-DDTHH:MM"),
        "the chat prompt must keep the ONCE AT guidance line"
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

/// pause/resume answer an unknown id with the family's stable
/// `scheduled_task_not_found` (like show/runs/pin/update/run), not the
/// foundation's raw `scheduled_update_failed: … Failed to read automation …`
/// read error.
#[test]
fn pause_and_resume_reject_unknown_task_ids_like_the_rest_of_the_family() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("unknown-id-pause-resume");
    for command in ["pause", "resume"] {
        let error = expect_failed(&["scheduled", command, "does-not-exist"]);
        assert!(
            error.starts_with("scheduled_task_not_found"),
            "{command}: {error}"
        );
    }
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
    // The foundation create resolves the next slot EAGERLY for an active
    // record (create_automation: next_after_with_anchor(now, now)) — the
    // old CLI deferred it to the app's sweep, which PAUSED a one-shot whose
    // AT had passed instead of running it late. The slot must be a real
    // RFC3339 stamp now.
    assert!(
        created["nextRunAt"]
            .as_str()
            .is_some_and(|stamp| stamp.ends_with('Z') || stamp.contains('+')),
        "active create must resolve a concrete next_run_at, got {}",
        created["nextRunAt"]
    );
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
    // rrule updates recompute the slot through the foundation update, so
    // the persisted def carries a resolved next_run_at (never null).
    assert!(def["next_run_at"].is_string(), "{}", def["next_run_at"]);

    // pause / resume flip the status like the GUI pause/resume commands;
    // pause clears the slot, resume re-resolves it eagerly.
    let paused = run_json(&["scheduled", "pause", &task_id]);
    assert_eq!(paused["status"].as_str(), Some("paused"));
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert!(def["next_run_at"].is_null(), "{}", def["next_run_at"]);
    let resumed = run_json(&["scheduled", "resume", &task_id]);
    assert_eq!(resumed["status"].as_str(), Some("active"));
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert!(def["next_run_at"].is_string(), "{}", def["next_run_at"]);
    // pin / unpin write the UI-metadata sidecar the GUI reads back
    let pinned = run_json(&["scheduled", "pin", &task_id]);
    assert_eq!(pinned["action"].as_str(), Some("pinned"));
    let listed = run_json(&["scheduled", "list"]);
    assert_eq!(
        listed["tasks"][0]["pinned"].as_bool(),
        Some(true),
        "list must report the pinned task"
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
fn create_refuses_non_yolo_modes_like_the_app() {
    // `build_create_request` runs every scheduled task through
    // `canonical_scheduled_mode`, which refuses anything but `yolo` because
    // the request overwrites `mode` downstream regardless: an accepted
    // `agent`/`plan` becomes a full-YOLO task (`trust_mode` and
    // `auto_approve` on) with the caller's choice discarded in silence. The
    // CLI used to accept both and throw them away, so pin the refusal, its
    // exit-code class, and the absence of a persisted task.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("mode-refusal");
    let prompt = write_prompt_file(&home, "mode.md", "Summarize the reports.");
    for refused in ["agent", "plan"] {
        let error = assert_validation_fail(&[
            "scheduled",
            "create",
            "--name",
            "Moded",
            "--prompt-file",
            prompt.to_str().unwrap(),
            "--rrule",
            VALID_RRULE,
            "--mode",
            refused,
        ]);
        assert!(error.contains(refused), "{refused}: {error}");
        assert!(error.contains("yolo"), "{refused}: {error}");
        let listed = run_json(&["scheduled", "list"]);
        assert_eq!(
            listed["tasks"].as_array().map(Vec::len),
            Some(0),
            "{refused}: a refused mode must not persist a task"
        );
    }

    // The one accepted value still creates, and persists the yolo shape.
    let created = run_json(&[
        "scheduled",
        "create",
        "--name",
        "Yolo task",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        VALID_RRULE,
        "--mode",
        "yolo",
    ]);
    assert_eq!(created["mode"].as_str(), Some("yolo"));
    assert_eq!(created["trustMode"].as_bool(), Some(true));
    assert_eq!(created["autoApprove"].as_bool(), Some(true));
    let _ = home;
}

/// A run record that is valid JSON but wrong-shaped (a required field
/// missing, typed wrong, or carrying a value the foundation's typed fields
/// cannot decode) must fail the listing with the malformed-record error
/// instead of rendering a phantom run — the gate implemented by
/// `require_object_run_record`.
#[test]
fn runs_listing_refuses_wrong_typed_run_records_as_malformed() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("run-record-type");
    let created = create_task(&home, "Typechecked task");
    let task_id = created["id"].as_str().unwrap().to_owned();

    // Missing `status` entirely.
    let runs = home.runs_dir(&task_id);
    std::fs::create_dir_all(&runs).unwrap();
    let missing = runs.join("20260923T000000-1.json");
    std::fs::write(
        &missing,
        r#"{"id":"run-1","automation_id":"t","scheduled_for":"2026-09-23T00:00:00Z","created_at":"2026-09-23T00:00:00Z"}"#,
    )
    .unwrap();
    let error = expect_failed(&["scheduled", "runs", &task_id]);
    assert!(
        error.contains("cannot list runs"),
        "the typed read must refuse the malformed record: {error}"
    );
    std::fs::remove_file(&missing).unwrap();

    // Wrong-typed `status` (number instead of string) is refused too.
    let wrong_type = runs.join("20260923T000000-2.json");
    std::fs::write(
        &wrong_type,
        r#"{"id":"run-2","automation_id":"t","scheduled_for":"x","status":3,"created_at":"y"}"#,
    )
    .unwrap();
    let error = expect_failed(&["scheduled", "runs", &task_id]);
    assert!(
        error.contains("cannot list runs"),
        "the typed read must refuse the malformed record: {error}"
    );
    std::fs::remove_file(&wrong_type).unwrap();

    // A *string* status that is not an `AutomationRunStatus` variant, and a
    // string timestamp that is not an RFC3339 instant, both used to pass the
    // string-only gate and render as a run with a garbage status, sorted last
    // by `record_time`'s epoch-floor fallback. They are the records that
    // hard-stop the GUI: `read_run_file` cannot deserialize them and
    // `collect_due_runs` aborts the sweep for every automation.
    let undecodable = runs.join("20260923T000000-3.json");
    for (label, payload) in [
        (
            "bogus status",
            r#"{"id":"run-3","automation_id":"t","scheduled_for":"2026-09-23T00:00:00Z","status":"bogus","created_at":"2026-09-23T00:00:00Z"}"#,
        ),
        (
            "unparseable scheduled_for",
            r#"{"id":"run-3","automation_id":"t","scheduled_for":"x","status":"completed","created_at":"2026-09-23T00:00:00Z"}"#,
        ),
        (
            "unparseable created_at",
            r#"{"id":"run-3","automation_id":"t","scheduled_for":"2026-09-23T00:00:00Z","status":"completed","created_at":"y"}"#,
        ),
        (
            // Offset-less: chrono's `DateTime<Utc>` deserializer requires one,
            // so this is undecodable for the GUI exactly like the above.
            "offset-less created_at",
            r#"{"id":"run-3","automation_id":"t","scheduled_for":"2026-09-23T00:00:00Z","status":"completed","created_at":"2026-09-23T00:00:00"}"#,
        ),
    ] {
        std::fs::write(&undecodable, payload).unwrap();
        let error = expect_failed(&["scheduled", "runs", &task_id]);
        assert!(error.contains("cannot list runs"), "{label}: {error}");
    }
    std::fs::remove_file(&undecodable).unwrap();

    // The same record with every field decodable lists cleanly, so the gate
    // rejects on the value and not merely on the shape.
    std::fs::write(
        runs.join("20260923T000000-4.json"),
        r#"{"id":"run-4","automation_id":"t","scheduled_for":"2026-09-23T00:00:00Z","status":"completed","created_at":"2026-09-23T00:00:00Z"}"#,
    )
    .unwrap();
    let listed = run_json(&["scheduled", "runs", &task_id]);
    assert_eq!(listed["runs"][0]["id"].as_str(), Some("run-4"));
    let _ = home;
}

/// The definition twin of the run-record gate: `status` is an
/// `AutomationStatus` enum and the two stamps are `DateTime<Utc>`, so a
/// hand-edited def carrying a well-typed but undecodable value must be
/// refused as malformed rather than listed with a status the GUI will never
/// agree with.
#[test]
fn read_commands_refuse_undecodable_definition_values_as_malformed() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("def-undecodable-values");
    let created = create_task(&home, "Decodable task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    let pristine: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();

    for (label, field, value) in [
        ("bogus status", "status", serde_json::json!("archived")),
        ("empty status", "status", serde_json::json!("")),
        (
            "unparseable created_at",
            "created_at",
            serde_json::json!("not-a-date"),
        ),
        (
            "offset-less updated_at",
            "updated_at",
            serde_json::json!("2026-09-23T00:00:00"),
        ),
    ] {
        let mut def = pristine.clone();
        def[field] = value;
        std::fs::write(home.def_path(&task_id), def.to_string()).unwrap();
        let shown = expect_failed(&["scheduled", "show", &task_id]);
        assert!(shown.contains("Failed to parse"), "{label}: {shown}");
        let listed = expect_failed(&["scheduled", "list"]);
        assert!(listed.contains("Failed to parse"), "{label}: {listed}");
    }

    // The untouched definition still reads, so the gate is value-specific.
    std::fs::write(home.def_path(&task_id), pristine.to_string()).unwrap();
    let shown = run_json(&["scheduled", "show", &task_id]);
    assert_eq!(shown["id"].as_str(), Some(task_id.as_str()));
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
        // 2026-02-30 cannot exist; the foundation's chrono parser rejects
        // it and one such record would fail the GUI scheduler's whole
        // sweep. Grammar rejection happens at parse time through the
        // foundation parser (exit 2), the same class as before — only the
        // message source changed (chrono's own error).
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
        assert!(
            error.contains("invalid rrule"),
            "the refusal must come from the foundation grammar: {error}"
        );
    }
    // A real date passes validation (the create then proceeds to the
    // next-run recompute).
    let _ = home;
}

/// Rrule validation (including ONCE AT) runs at parse time, so a regression
/// to execute-time rejection (exit 1) must fail the test: pin the usage exit
/// code here instead of accepting either layer.
fn assert_validation_fail(arguments: &[&str]) -> String {
    let mut owned: Vec<String> = std::iter::once("pinvou".to_owned())
        .chain(arguments.iter().map(|value| value.to_string()))
        .collect();
    let error = match parse_args(owned.drain(..)) {
        Err(error) => error,
        Ok(parsed) => execute(parsed).expect_err("expected validation failure"),
    };
    assert_eq!(error.exit_code(), ExitCode::Usage, "{error}");
    error.to_string()
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
    assert_eq!(
        runs.len(),
        3,
        "runs-all should contain the three seeded runs"
    );
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
        serde_json::json!({ "language": "zh-Hans", "memory_enabled": true }).to_string(),
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
    assert!(
        def["last_run_at"].is_string(),
        "the deleted task must stamp last_run_at for the history archive"
    );
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

// ---- round-3 fixes: RFC3339 ONCE AT calendar truth, delete pause semantics ----

#[test]
fn once_at_rejects_calendar_overflow_on_the_rfc3339_channel_too() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("once-at-rfc3339");
    let prompt = write_prompt_file(&home, "task.md", "Do the thing.");
    // The foundation parser (chrono) rejects these on both channels, and one
    // unparseable record stalls the GUI scheduler's whole sweep. Validation
    // happens at parse time, so the rejection is a usage error (exit 2).
    for bad_at in [
        "FREQ=ONCE;AT=2026-02-30T08:30:00Z",
        "FREQ=ONCE;AT=2025-02-29T08:30:00Z",
        "FREQ=ONCE;AT=2026-04-31T08:30:00+08:00",
    ] {
        let mut owned: Vec<String> = std::iter::once("pinvou".to_owned())
            .chain(
                [
                    "scheduled",
                    "create",
                    "--name",
                    "Overflow",
                    "--prompt-file",
                    prompt.to_str().unwrap(),
                    "--rrule",
                    bad_at,
                ]
                .iter()
                .map(|value| value.to_string()),
            )
            .collect();
        let error = match parse_args(owned.drain(..)) {
            Err(error) => error,
            Ok(parsed) => execute(parsed).expect_err("calendar overflow must be rejected"),
        };
        assert_eq!(error.exit_code(), ExitCode::Usage, "{bad_at}: {error}");
        assert!(
            error.to_string().contains("invalid rrule"),
            "the refusal must come from the foundation grammar: {bad_at}: {error}"
        );
    }
    // A real RFC3339 stamp on the same channels is still accepted (it must
    // also be in the future — see once_at_rejects_past_times_like_the_gui).
    let value = run_json(&[
        "scheduled",
        "create",
        "--name",
        "Real date",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        "FREQ=ONCE;AT=2030-02-28T08:30:00Z",
    ]);
    assert_eq!(value["name"], "Real date");
}

#[test]
fn leap_day_schedules_are_accepted_on_every_channel() {
    // Feb 29 exists in leap years: the naive ONCE channel, the RFC3339 ONCE
    // channel, and a cron February-29 expression (kept realizable by the
    // foundation's leap-year date-space probe) must all be accepted — only
    // the rejections are pinned elsewhere.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("leap-day");
    let prompt = write_prompt_file(&home, "leap.md", "Summarize the reports.");
    for rrule in [
        "FREQ=ONCE;AT=2028-02-29T08:30",
        "FREQ=ONCE;AT=2028-02-29T08:30:00Z",
        "FREQ=CRON;EXPR=0 0 29 2 *",
    ] {
        let created = run_json(&[
            "scheduled",
            "create",
            "--name",
            "Leap day",
            "--prompt-file",
            prompt.to_str().unwrap(),
            "--rrule",
            rrule,
        ]);
        assert_eq!(created["rrule"].as_str(), Some(rrule), "{rrule}");
    }
    let _ = home;
}

#[test]
fn once_at_rejects_past_times_like_the_gui() {
    // The GUI's create/update resolves ONCE through
    // `next_after_with_anchor(now, now)` and fails with "no future run" for
    // a past stamp; the CLI used to accept it and let the first sweep tick
    // pause the task instead. Both channels must refuse up front.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("once-at-past");
    let prompt = write_prompt_file(&home, "past.md", "Summarize the reports.");
    // An old absolute stamp (RFC3339 with offset) and a naive local stamp
    // that is unambiguously over (the test env's clock is well past 2030
    // only in the offset channel; the naive channel uses yesterday's date).
    let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);
    let naive_stamp = format!("FREQ=ONCE;AT={yesterday}T08:30");
    for bad in ["FREQ=ONCE;AT=2020-01-01T00:00:00Z", naive_stamp.as_str()] {
        // The refusal moved from the CLI's parse-time mirror to the
        // foundation's own eager create resolution (`create_automation`:
        // `next_after_with_anchor(now, now)`), which is where the GUI
        // rejects the same record — a state-dependent failure (exit 1),
        // like the GUI command's Err. A past one-shot must never be
        // persisted as a live task.
        let error = expect_failed(&[
            "scheduled",
            "create",
            "--name",
            "past once",
            "--prompt-file",
            prompt.to_str().unwrap(),
            "--rrule",
            bad,
        ]);
        assert!(error.contains("no future run"), "{bad}: {error}");
        let listed = run_json(&["scheduled", "list"]);
        assert_eq!(
            listed["tasks"].as_array().map(Vec::len),
            Some(0),
            "{bad}: a refused one-shot must not persist a task"
        );
    }
}

#[test]
fn paused_create_accepts_a_past_once_stamp_like_the_gui() {
    // The refusal above is the *active* rule only. The foundation resolves the
    // schedule solely for an active record (`create_automation` calls
    // `next_after_with_anchor` inside `if matches!(status, Active)`), so the
    // GUI creates a paused one-shot with an elapsed AT without complaint — a
    // drafted task the user edits and resumes later. `--paused` must not be
    // stricter in the CLI than on the surface it mirrors, while the very same
    // rrule without `--paused` stays a usage error.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("paused-past-once");
    let prompt = write_prompt_file(&home, "paused-past.md", "Summarize the reports.");
    let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);
    let naive_stamp = format!("FREQ=ONCE;AT={yesterday}T08:30");
    for past in ["FREQ=ONCE;AT=2020-01-01T00:00:00Z", naive_stamp.as_str()] {
        let created = run_json(&[
            "scheduled",
            "create",
            "--name",
            "paused once",
            "--prompt-file",
            prompt.to_str().unwrap(),
            "--rrule",
            past,
            "--paused",
        ]);
        assert_eq!(created["status"].as_str(), Some("paused"), "{past}");
        assert_eq!(created["rrule"].as_str(), Some(past), "{past}");
        // Persisted the same way, so the GUI reads back a paused record with
        // no next_run_at — exactly what its own paused create writes.
        let def: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.def_path(created["id"].as_str().unwrap())).unwrap(),
        )
        .unwrap();
        assert_eq!(def["status"].as_str(), Some("paused"), "{past}");
        assert!(def["next_run_at"].is_null(), "{past}");

        let error = expect_failed(&[
            "scheduled",
            "create",
            "--name",
            "active once",
            "--prompt-file",
            prompt.to_str().unwrap(),
            "--rrule",
            past,
        ]);
        assert!(error.contains("no future run"), "{past}: {error}");
    }
}

/// A grammar-invalid rrule must be a usage error (exit 2) on `create
/// --paused` too: `--paused` defers only the foundation's past-ONCE/next-slot
/// calendar resolution (a state-dependent failure, exit 1), never the argv
/// grammar — and the MAX_HOURLY_INTERVAL scheduler guard must not be
/// bypassed by pausing the create.
#[test]
fn paused_create_still_validates_the_rrule_grammar() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("paused-grammar");
    let prompt = write_prompt_file(&home, "paused-grammar.md", "Summarize the reports.");
    for bad in ["FREQ=BOGUS", "FREQ=ONCE", "FREQ=HOURLY;INTERVAL=4000000000"] {
        let error = assert_usage(&[
            "scheduled",
            "create",
            "--name",
            "Paused grammar",
            "--prompt-file",
            prompt.to_str().unwrap(),
            "--rrule",
            bad,
            "--paused",
        ]);
        assert!(!error.is_empty(), "{bad}: {error}");
        let listed = run_json(&["scheduled", "list"]);
        assert_eq!(
            listed["tasks"].as_array().map(Vec::len),
            Some(0),
            "{bad}: a usage-refused create must not persist a task"
        );
    }
    let _ = home;
}

#[test]
fn newer_schema_sidecars_are_refused_not_merged_and_written_back() {
    // The GUI's VersionedJsonStore quarantines a registry whose
    // schema_version is newer than supported; the CLI must refuse to
    // read-modify-write it instead of silently writing a hybrid format the
    // GUI would misread.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("sidecar-newer-schema");
    let created = create_task(&home, "Pinned task");
    let task_id = created["id"].as_str().unwrap().to_owned();

    let ui_metadata = home
        .path()
        .join("automations")
        .join("task-ui-metadata.json");
    std::fs::create_dir_all(ui_metadata.parent().unwrap()).unwrap();
    let future_format = r#"{"schema_version": 99, "tasks": {"future": {}}}"#;
    std::fs::write(&ui_metadata, future_format).unwrap();

    let error = expect_failed(&["scheduled", "pin", &task_id]);
    assert!(error.contains("scheduled_storage_unavailable"), "{error}");
    assert!(error.contains("newer than supported"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&ui_metadata).unwrap(),
        future_format,
        "the refused write must leave the future-format file untouched"
    );
}

#[test]
fn hourly_interval_beyond_scheduler_range_is_refused() {
    // The foundation sweep computes the first slot lazily for CLI-created
    // records (`next_run_at: null`) and its unanchored branch does an
    // unchecked `DateTime + Duration::hours(interval)` — an absurd INTERVAL
    // overflows chrono's range and panics the scheduler task, stalling every
    // GUI automation. The CLI must refuse the value up front.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("hourly-interval-bound");
    let prompt = write_prompt_file(&home, "interval.md", "Summarize the reports.");
    let error = assert_validation_fail(&[
        "scheduled",
        "create",
        "--name",
        "huge interval",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        "FREQ=HOURLY;INTERVAL=4000000000",
    ]);
    assert!(error.contains("INTERVAL must be <="), "{error}");

    // Just past the bound is refused, the bound itself is accepted.
    let error = assert_validation_fail(&[
        "scheduled",
        "create",
        "--name",
        "past bound",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        "FREQ=HOURLY;INTERVAL=1000001",
    ]);
    assert!(error.contains("INTERVAL must be <="), "{error}");
    let created = run_json(&[
        "scheduled",
        "create",
        "--name",
        "at bound",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        "FREQ=HOURLY;INTERVAL=1000000",
    ]);
    assert_eq!(created["rrule"], "FREQ=HOURLY;INTERVAL=1000000");
}

#[test]
fn rfc3339_once_accepts_lowercase_separator_like_the_foundation() {
    // RFC3339 §5.6 NOTE allows the lowercase date/time separator and the
    // foundation parses through chrono, which accepts it; the CLI must not
    // reject a stamp the GUI can create.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("once-lowercase-t");
    let prompt = write_prompt_file(&home, "lower.md", "Summarize the reports.");
    let created = run_json(&[
        "scheduled",
        "create",
        "--name",
        "lowercase separator",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        "FREQ=ONCE;AT=2030-02-28t08:30:00z",
    ]);
    assert_eq!(created["id"].as_str().map(str::len), Some(36));
}

#[test]
fn malformed_schema_version_is_refused_not_treated_as_legacy() {
    // A wrong-typed schema_version used to pass the gate as 0 while the
    // GUI's typed deserialization kept failing on the file: the CLI must
    // refuse to operate on a record the app cannot read instead of
    // reporting success.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("schema-malformed");
    let created = create_task(&home, "Stringy schema");
    let task_id = created["id"].as_str().unwrap().to_owned();
    let def_path = home.def_path(&task_id);
    let mut def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&def_path).unwrap()).unwrap();
    def["schema_version"] = serde_json::json!("2");
    std::fs::write(&def_path, def.to_string()).unwrap();

    let error = expect_failed(&["scheduled", "pause", &task_id]);
    assert!(error.contains("Failed to parse"), "{error}");
}

#[test]
fn newer_schema_read_state_is_refused_not_merged_and_written_back() {
    // The read-state registry carries the viewed history; a newer app's
    // format must never be merged and written back (the GUI's
    // VersionedJsonStore quarantines the same situation).
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("read-state-newer-schema");
    let created = create_task(&home, "Watched task");
    let task_id = created["id"].as_str().unwrap().to_owned();

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
            "thread_id": session.metadata.id,
            "turn_id": "turn-1",
            "error": null
        })
        .to_string(),
    )
    .unwrap();

    let read_state = home.path().join("scheduled-runs").join("read-state.json");
    std::fs::create_dir_all(read_state.parent().unwrap()).unwrap();
    let future_format = r#"{"schema_version": 99, "viewed_runs": {"other": ["r1"]}}"#;
    std::fs::write(&read_state, future_format).unwrap();

    let error = expect_failed(&["scheduled", "mark-viewed", &task_id, run_id]);
    assert!(error.contains("newer than supported"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&read_state).unwrap(),
        future_format,
        "the refused write must leave the future-format file untouched"
    );
}

#[test]
fn newer_schema_history_archive_blocks_delete_and_restores_the_task() {
    // The archive gate must both refuse the write-back AND restore the
    // provisional pause, like every other blocked delete path — a caller
    // retrying after upgrading must not find the task paused.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("archive-newer-schema");
    let created = create_task(&home, "Archived task");
    let task_id = created["id"].as_str().unwrap().to_owned();

    let archive = home.path().join("automations").join("history-archive.json");
    let future_format = r#"{"schema_version": 99, "tasks": {}}"#;
    std::fs::write(&archive, future_format).unwrap();

    let error = expect_failed(&["scheduled", "delete", &task_id, "--yes"]);
    assert!(error.contains("newer than supported"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&archive).unwrap(),
        future_format,
        "the refused write must leave the future-format file untouched"
    );
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert_eq!(def["status"], "active", "the provisional pause is restored");
}

#[test]
fn delete_blocked_restores_the_previous_status() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("delete-restore");
    let created = create_task(&home, "Restore task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    std::fs::create_dir_all(home.runs_dir(&task_id)).unwrap();
    std::fs::write(
        home.runs_dir(&task_id).join("active-run.json"),
        serde_json::json!({
            "schema_version": 1,
            "id": "active-run",
            "automation_id": task_id,
            "task_id": "foundation-task-1",
            "scheduled_for": "2026-09-10T08:00:00.000Z",
            "status": "queued",
            "created_at": "2026-09-10T08:00:00.000Z"
        })
        .to_string(),
    )
    .unwrap();
    let error = expect_failed(&["scheduled", "delete", &task_id, "--yes"]);
    assert!(error.starts_with("scheduled_delete_blocked"), "{error}");
    // The pause applied for the deletion check is rolled back: the sweep
    // field (`status`) is back to its previous value and no stray `paused`
    // bool is left behind.
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert_eq!(def["status"], "active");
    assert!(def.get("paused").is_none());
}

#[test]
fn delete_on_a_non_object_definition_fails_instead_of_panicking() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("delete-non-object");
    let created = create_task(&home, "Malformed task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    std::fs::write(home.def_path(&task_id), "5").unwrap();
    let error = expect_failed(&["scheduled", "delete", &task_id, "--yes"]);
    assert!(error.contains("Failed to parse"), "{error}");
}

#[test]
fn read_commands_refuse_non_object_definitions_uniformly() {
    // `read_def`/`list_defs` apply the object check, so the read-only
    // commands refuse a hand-edited non-object definition with the same
    // malformed message as the mutating commands (show used to render a
    // phantom empty task with exit 0 while list died on a confusing
    // safe_storage_id error).
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("non-object-read");
    let created = create_task(&home, "Malformed task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    std::fs::write(home.def_path(&task_id), "5").unwrap();
    let shown = expect_failed(&["scheduled", "show", &task_id]);
    assert!(
        shown.contains("Failed to parse"),
        "show must refuse a malformed definition"
    );
    let listed = expect_failed(&["scheduled", "list"]);
    assert!(
        listed.contains("Failed to parse"),
        "list must refuse a malformed definition"
    );
    let _ = home;
}

#[test]
fn read_commands_refuse_definitions_missing_required_fields() {
    // A def the GUI's typed `AutomationRecord` cannot deserialize (required
    // field missing) must refuse like a non-object one instead of rendering
    // a phantom task with exit 0.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("def-missing-fields");
    let created = create_task(&home, "Incomplete task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    std::fs::write(
        home.def_path(&task_id),
        serde_json::json!({ "schema_version": 2, "id": task_id }).to_string(),
    )
    .unwrap();
    let shown = expect_failed(&["scheduled", "show", &task_id]);
    assert!(
        shown.contains("Failed to parse"),
        "show must refuse a malformed definition"
    );
    let listed = expect_failed(&["scheduled", "list"]);
    assert!(
        listed.contains("Failed to parse"),
        "list must refuse a malformed definition"
    );
    let _ = home;
}

/// TZ-driven: chrono only reads the `TZ` variable on Unix.
#[cfg(unix)]
#[test]
fn once_at_rejects_dst_gap_times_like_the_foundation() {
    // The foundation resolves naive stamps through the system timezone and a
    // nonexistent local time fails its sweep every tick, so the CLI must
    // reject one at creation. chrono caches the local zone at first use, so
    // a runtime TZ switch inside this test process is unreliable — drive the
    // real binary with a pinned timezone instead (UTC runners have no gap,
    // so the zone must be pinned regardless).
    let bin = env!("CARGO_BIN_EXE_pinvou");
    let root = std::env::temp_dir().join(format!(
        "pinvou-cli-scheduled-once-at-dst-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let prompt = root.join("dst.md");
    std::fs::write(&prompt, "Summarize the reports.").unwrap();
    let run = |rrule: &str| {
        let mut command = std::process::Command::new(bin);
        command
            .args([
                "scheduled",
                "create",
                "--name",
                "DST task",
                "--prompt-file",
                prompt.to_str().unwrap(),
                "--rrule",
                rrule,
            ])
            .env("PINVOU3_HOME", &root)
            .env("TZ", "America/New_York");
        command.output().expect("binary runs")
    };
    // 2027-03-14 02:30 does not exist in America/New_York (clocks jump
    // 02:00 -> 03:00); the foundation's `resolve_local_datetime` returns
    // None for it. Both foundation naive shapes — `HH:MM` and `HH:MM:SS` —
    // must take that channel; an offset-less `HH:MM:SS` stamp used to fall
    // into the offset-free parse and skip the gap check entirely.
    for gap_stamp in ["2027-03-14T02:30", "2027-03-14T02:30:00"] {
        let gap = run(&format!("FREQ=ONCE;AT={gap_stamp}"));
        assert!(!gap.status.success(), "{gap_stamp} must be rejected");
        assert!(
            String::from_utf8_lossy(&gap.stderr).contains("does not exist"),
            "gap rejection must name the missing task"
        );
    }
    // Ordinary and ambiguous (fall-back) times still resolve — the
    // foundation picks the earliest occurrence for ambiguous stamps.
    for good in [
        "FREQ=ONCE;AT=2027-01-15T02:30",
        "FREQ=ONCE;AT=2027-11-07T01:30",
        "FREQ=ONCE;AT=2027-01-15T02:30:00",
    ] {
        let output = run(good);
        assert!(output.status.success(), "{good} must be accepted");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn mutations_fail_honestly_on_non_object_definitions() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for label in ["update", "pause", "resume"] {
        let home = TempHome::new(&format!("non-object-{label}"));
        let created = create_task(&home, "Malformed task");
        let task_id = created["id"].as_str().unwrap().to_owned();
        std::fs::write(home.def_path(&task_id), "[]").unwrap();
        let arguments: Vec<&str> = match label {
            "update" => vec!["scheduled", "update", &task_id, "--name", "Renamed"],
            "pause" => vec!["scheduled", "pause", &task_id],
            _ => vec!["scheduled", "resume", &task_id],
        };
        let error = expect_failed(&arguments);
        assert!(error.contains("Failed to parse"), "{label}: {error}");
        let _ = home;
    }
}

#[test]
fn run_writes_only_terminal_records() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("run-terminal-only");
    let created = create_task(&home, "Terminal task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    // No display/model here, so the run fails honestly — but it must never
    // leave a persisted `queued` record behind: the GUI cannot cancel,
    // complete, reconcile or delete around a queued record with no task id.
    let _ = expect_failed(&["scheduled", "run", &task_id]);
    let mut queued = Vec::new();
    if let Ok(entries) = std::fs::read_dir(home.runs_dir(&task_id)) {
        for entry in entries.flatten() {
            let raw = std::fs::read_to_string(entry.path()).unwrap();
            if raw.contains("\"queued\"") {
                queued.push(entry.path());
            }
        }
    }
    assert!(
        queued.is_empty(),
        "no run record may persist in queued state: {queued:?}"
    );
}

#[test]
fn failed_delete_restores_the_previous_status() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("delete-restore");
    let created = create_task(&home, "Blocked task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    // Make the archive commit fail: a directory at the archive path makes
    // every write fail. Delete must restore the pre-delete status instead of
    // leaving the task paused.
    let archive = home.path().join("automations").join("history-archive.json");
    std::fs::create_dir_all(&archive).unwrap();
    let error = expect_failed(&["scheduled", "delete", &task_id, "--yes"]);
    assert!(!error.is_empty(), "{error}");
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert_eq!(
        def["status"], "active",
        "a failed delete must leave the task as it was"
    );
}

#[test]
fn delete_with_an_unreadable_run_record_restores_the_paused_task() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("delete-run-record-restore");
    let created = create_task(&home, "Run-record task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    // A run record the store refuses (newer schema) makes list_runs fail
    // after the provisional pause was already committed: that failure is a
    // blocked delete like any other, so the pre-delete status must be
    // restored instead of leaving the task paused with no next run.
    std::fs::create_dir_all(home.runs_dir(&task_id)).unwrap();
    std::fs::write(
        home.runs_dir(&task_id).join("future-run.json"),
        serde_json::json!({
            "schema_version": 99,
            "id": "future-run",
            "automation_id": task_id,
            "scheduled_for": "2026-09-10T08:00:00.000Z",
            "status": "done",
            "created_at": "2026-09-10T08:00:00.000Z"
        })
        .to_string(),
    )
    .unwrap();
    expect_failed(&["scheduled", "delete", &task_id, "--yes"]);
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert_eq!(
        def["status"], "active",
        "a delete blocked by an unreadable run record must not leave the task paused"
    );
    assert!(home.def_path(&task_id).exists());
}

#[test]
fn unreadable_registries_are_quarantined_before_the_default_is_used() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("registry-quarantine");
    let created = create_task(&home, "Quarantine task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    let bindings = home.path().join("automations").join("model-bindings.json");
    std::fs::write(&bindings, "{not json").unwrap();
    // A command reading the registry must not silently destroy the
    // malformed file on the next write: it is quarantined next to the
    // original first. (`show` degrades to the default and succeeds.)
    let _ = run_human(&["scheduled", "show", &task_id]);
    let quarantined = std::fs::read_dir(bindings.parent().unwrap())
        .unwrap()
        .flatten()
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("model-bindings.json.invalid-")
        });
    assert!(quarantined, "the malformed registry must be quarantined");
}

#[test]
fn wrong_shaped_registries_are_quarantined_not_silently_overwritten() {
    // The GUI's VersionedJsonStore::open quarantines any payload its typed
    // deserialization rejects — valid JSON of the wrong shape included; the
    // CLI's read_registry must do the same instead of normalizing the value
    // in memory and letting the next write destroy the only on-disk copy.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for (label, payload) in [
        ("non-object", "[]"),
        ("wrong-typed-tasks", r#"{"tasks": []}"#),
    ] {
        let home = TempHome::new(&format!("registry-shape-quarantine-{label}"));
        let created = create_task(&home, "Shape task");
        let task_id = created["id"].as_str().unwrap().to_owned();
        // Seed a real saved model: `--model-id` is validated against
        // saved_models (round-18 blocker 2), so the binding write needs an
        // id the executor could actually resolve.
        let mut add = Command::new(env!("CARGO_BIN_EXE_pinvou"));
        add.args([
            "models",
            "add",
            "--preset",
            "deepseek",
            "--name",
            "Shape model",
            "--model",
            "shape-wire-name",
            "--base-url",
            "https://api.deepseek.com",
        ])
        .env("PINVOU3_HOME", home.path());
        let added = add.output().expect("models add runs");
        assert!(
            added.status.success(),
            "{}",
            String::from_utf8_lossy(&added.stderr)
        );
        let model_id = String::from_utf8_lossy(&added.stdout)
            .trim()
            .strip_prefix("id: ")
            .expect("models add prints the id")
            .trim()
            .to_owned();
        let bindings = home.path().join("automations").join("model-bindings.json");
        std::fs::write(&bindings, payload).unwrap();
        // A mutating command reading the registry must quarantine the
        // wrong-shaped file next to the original before degrading to the
        // default, so the only on-disk copy survives the write-back.
        let _ = run_json(&["scheduled", "update", &task_id, "--model-id", &model_id]);
        let quarantine_copies: Vec<_> = std::fs::read_dir(bindings.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("model-bindings.json.invalid-")
            })
            .map(|entry| std::fs::read_to_string(entry.path()).unwrap())
            .collect();
        assert_eq!(
            quarantine_copies,
            vec![payload.to_owned()],
            "{label}: exactly one quarantine copy of the original payload"
        );
        // The write itself still lands: the command degrades to the default
        // registry and persists the new binding.
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&bindings).unwrap()).unwrap();
        assert_eq!(
            written["tasks"][&task_id]["model_id"], model_id,
            "{label}: the binding write lands after the quarantine"
        );
        let _ = home;
    }
}

#[test]
fn read_state_with_a_stray_tasks_key_stays_readable() {
    // The per-registry shape gate (079bf9d43) checks only the keys the
    // caller's registry owns: the GUI's serde ignores unknown members, so a
    // read-state file carrying a stray `tasks` key must stay readable and
    // must NOT be quarantined (that would reset the user's viewed-run
    // state — the exact regression the per-type key split fixed).
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("read-state-stray-key");
    let created = create_task(&home, "Stray key task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    let read_state = home.path().join("scheduled-runs").join("read-state.json");
    std::fs::create_dir_all(read_state.parent().unwrap()).unwrap();
    std::fs::write(
        &read_state,
        serde_json::json!({ "viewed_runs": {}, "tasks": [] }).to_string(),
    )
    .unwrap();
    let listed = run_human(&["scheduled", "runs", &task_id]);
    assert!(!listed.is_empty(), "runs must stay readable");
    let quarantined = std::fs::read_dir(read_state.parent().unwrap())
        .unwrap()
        .flatten()
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("read-state.json.invalid-")
        });
    assert!(
        !quarantined,
        "the stray-key read state must not be quarantined"
    );
    let _ = home;
}

#[test]
fn read_commands_refuse_wrong_typed_required_fields_as_malformed() {
    // The required-field gate type-checks, not just presence-checks: a
    // hand-edited def with a numeric `name` or a numeric timestamp must be
    // refused as malformed (the GUI's typed serde rejects the whole record),
    // not rendered as a phantom task with empty strings.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("wrong-typed-def");
    let created = create_task(&home, "Type check task");
    let task_id = created["id"].as_str().unwrap().to_owned();

    let mut def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    def["name"] = serde_json::json!(42);
    std::fs::write(home.def_path(&task_id), def.to_string()).unwrap();
    let shown = expect_failed(&["scheduled", "show", &task_id]);
    assert!(shown.contains("Failed to parse"), "{shown}");

    let mut def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    def["name"] = serde_json::json!("Back to a string");
    def["created_at"] = serde_json::json!(1726000000);
    std::fs::write(home.def_path(&task_id), def.to_string()).unwrap();
    let listed = expect_failed(&["scheduled", "list"]);
    assert!(listed.contains("Failed to parse"), "{listed}");
    let _ = home;
}

// ---- round-18: model-id pairing, eager one-shot slots, run exit contract ----

/// BLOCKER 2 (round-18 review): `create --model-id X` used to pair X with the
/// ACTIVE model's wire name while the definition persisted that same active
/// wire name — a pairing the executor rejects on every run
/// (`resolve_scheduled_model`: "此任务绑定的 AI 模型配置已变更"). The GUI
/// sends `model: selected.model, modelId: selected.id`; the CLI resolves the
/// pair the same way now, and X is validated against the saved models before
/// anything is persisted (an unknown id is the family's state-dependent
/// refusal, exit 1, matching `models ...` "model not found:").
#[test]
fn create_with_model_id_binds_the_definition_to_that_models_wire_name() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("model-id-pair");

    // Seed a saved model the way the GUI does (models add prints the id).
    let prompt = write_prompt_file(&home, "pair.md", "Summarize the reports.");
    let mut add = Command::new(env!("CARGO_BIN_EXE_pinvou"));
    add.args([
        "models",
        "add",
        "--preset",
        "deepseek",
        "--name",
        "Pair test",
        "--model",
        "pair-wire-name",
        "--base-url",
        "https://api.deepseek.com",
    ])
    .env("PINVOU3_HOME", home.path());
    let added = add.output().expect("models add runs");
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let model_id = String::from_utf8_lossy(&added.stdout)
        .trim()
        .strip_prefix("id: ")
        .expect("models add prints the id")
        .trim()
        .to_owned();

    // Create with --model-id: definition model AND binding must both carry
    // the named model's wire name, exactly the GUI's pairing.
    let created = run_json(&[
        "scheduled",
        "create",
        "--name",
        "Pinned task",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        VALID_RRULE,
        "--model-id",
        &model_id,
    ]);
    assert_eq!(created["model"].as_str(), Some("pair-wire-name"));
    assert_eq!(created["modelId"].as_str(), Some(model_id.as_str()));
    let def: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.def_path(created["id"].as_str().unwrap())).unwrap(),
    )
    .unwrap();
    assert_eq!(def["model"].as_str(), Some("pair-wire-name"));
    let bindings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.path().join("automations/model-bindings.json")).unwrap(),
    )
    .unwrap();
    let task_id = created["id"].as_str().unwrap();
    assert_eq!(
        bindings["tasks"][task_id]["model_id"].as_str(),
        Some(model_id.as_str())
    );
    assert_eq!(
        bindings["tasks"][task_id]["model"].as_str(),
        Some("pair-wire-name")
    );
}

/// Unknown `--model-id` values are refused before anything is persisted —
/// the pairing cannot be constructed for a model that does not exist, and a
/// task bound to a phantom id fails on every run with the executor's
/// "配置已失效" refusal.
#[test]
fn create_refuses_an_unknown_model_id_without_persisting_a_task() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("model-id-unknown");
    let prompt = write_prompt_file(&home, "unknown.md", "Summarize the reports.");
    let error = expect_failed(&[
        "scheduled",
        "create",
        "--name",
        "Phantom pin",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        VALID_RRULE,
        "--model-id",
        "m-does-not-exist",
    ]);
    assert!(
        error.contains("model not found: m-does-not-exist"),
        "{error}"
    );
    let listed = run_json(&["scheduled", "list"]);
    assert_eq!(listed["tasks"].as_array().map(Vec::len), Some(0));
}

/// `update --model-id` re-binds the pair on the same rule (the GUI applies
/// input.model + modelId together; the old CLI only re-bound the pin and
/// left the definition's wire name alone, which the executor rejects).
#[test]
fn update_with_model_id_moves_both_the_definition_and_the_pin() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("model-id-update");
    let created = create_task(&home, "Rebind task");
    let task_id = created["id"].as_str().unwrap().to_owned();

    let prompt = write_prompt_file(&home, "add.md", "x");
    let mut add = Command::new(env!("CARGO_BIN_EXE_pinvou"));
    add.args([
        "models",
        "add",
        "--preset",
        "deepseek",
        "--name",
        "Other model",
        "--model",
        "other-wire-name",
        "--base-url",
        "https://api.deepseek.com",
    ])
    .env("PINVOU3_HOME", home.path());
    let added = add.output().expect("models add runs");
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let model_id = String::from_utf8_lossy(&added.stdout)
        .trim()
        .strip_prefix("id: ")
        .expect("models add prints the id")
        .trim()
        .to_owned();

    let updated = run_json(&["scheduled", "update", &task_id, "--model-id", &model_id]);
    assert_eq!(updated["modelId"].as_str(), Some(model_id.as_str()));
    assert_eq!(updated["model"].as_str(), Some("other-wire-name"));
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert_eq!(def["model"].as_str(), Some("other-wire-name"));

    let _ = prompt;
}

/// `update`/`resume` must PERSIST the workspace repair: the GUI's
/// `ensure_automation_workspace` writes a repaired `cwds` back through
/// `update_automation(cwds: …)`, so a definition whose stored `cwds` is empty
/// must not be repaired in memory only — the executor would run with a
/// different cwd than the command reported. A definition that already pins
/// the workspace stays untouched (the repair is idempotent).
#[test]
fn update_and_resume_persist_a_repaired_workspace_cwd() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("workspace-repair");
    let created = create_task(&home, "Repaired task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    let expected = pinvou3_lib::platform::paths::scheduled_task_workspace_dir(&task_id);
    let persisted_cwds = || -> Vec<String> {
        let def: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap())
                .unwrap();
        def["cwds"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    // Strip the persisted cwd the way a definition predating the workspace
    // pin (or a hand-edit) looks.
    let strip_cwds = || {
        let mut def: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap())
                .unwrap();
        def["cwds"] = serde_json::json!([]);
        std::fs::write(home.def_path(&task_id), def.to_string()).unwrap();
    };

    // update repairs the definition on disk, not only in memory.
    strip_cwds();
    run_json(&["scheduled", "update", &task_id, "--name", "Repaired"]);
    assert_eq!(
        persisted_cwds(),
        vec![expected.display().to_string()],
        "update must persist the repaired cwd"
    );

    // resume performs the same repair.
    run_json(&["scheduled", "pause", &task_id]);
    strip_cwds();
    run_json(&["scheduled", "resume", &task_id]);
    assert_eq!(
        persisted_cwds(),
        vec![expected.display().to_string()],
        "resume must persist the repaired cwd too"
    );

    // The repair is idempotent: a definition already pinning the workspace is
    // left with exactly one entry (never rewritten or duplicated).
    run_json(&["scheduled", "update", &task_id, "--name", "Repaired again"]);
    assert_eq!(
        persisted_cwds(),
        vec![expected.display().to_string()],
        "an already-pinned workspace must not be rewritten or duplicated"
    );
    let _ = home;
}

/// BLOCKER 3 (round-18 review): a CLI one-shot used to persist
/// `next_run_at: null` and rely on the app's sweep; a one-shot whose AT had
/// already passed was silently PAUSED by the sweep's "no future run" and
/// never ran. The foundation's create/resume resolve the slot eagerly — the
/// CLI routes through them, so an active one-shot (here: a near-future
/// stamp the sweep would fire late) carries a concrete next_run_at from the
/// moment it is created, and a resumed one-shot is re-resolved (a past AT
/// is refused at resume time with the scheduler's own "no future run").
#[test]
fn one_shot_tasks_get_an_eager_next_run_slot_on_create_pause_and_resume() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("once-eager");
    let prompt = write_prompt_file(&home, "once-eager.md", "Summarize the reports.");
    let at = chrono::Local::now() + chrono::Duration::hours(26);
    let rrule = format!("FREQ=ONCE;AT={}", at.format("%Y-%m-%dT%H:%M"));

    let created = run_json(&[
        "scheduled",
        "create",
        "--name",
        "Eager once",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        &rrule,
    ]);
    let task_id = created["id"].as_str().unwrap().to_owned();
    assert!(
        created["nextRunAt"].as_str().is_some(),
        "an active one-shot must carry its slot eagerly: {}",
        created["nextRunAt"]
    );
    // …and the persisted record agrees with the DTO.
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert!(def["next_run_at"].is_string(), "{}", def["next_run_at"]);

    // Pause clears it (nothing fires while paused), resume re-resolves it.
    run_json(&["scheduled", "pause", &task_id]);
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert!(def["next_run_at"].is_null());
    let resumed = run_json(&["scheduled", "resume", &task_id]);
    assert!(
        resumed["nextRunAt"].as_str().is_some(),
        "resume must re-resolve the one-shot slot eagerly: {}",
        resumed["nextRunAt"]
    );

    // The silent-pause trap: resuming a one-shot whose AT has passed is the
    // exact case the sweep used to pause silently; the foundation resume
    // refuses it up front instead.
    let past = chrono::Local::now() - chrono::Duration::hours(24);
    let past_rrule = format!("FREQ=ONCE;AT={}", past.format("%Y-%m-%dT%H:%M"));
    let staged = run_json(&[
        "scheduled",
        "create",
        "--name",
        "Staged once",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        &past_rrule,
        "--paused",
    ]);
    let staged_id = staged["id"].as_str().unwrap().to_owned();
    let error = expect_failed(&["scheduled", "resume", &staged_id]);
    assert!(
        error.contains("no future run"),
        "a past one-shot must be refused at resume, not silently paused: {error}"
    );
}

/// S8 (round-18 review): `scheduled run` exited 0 when the memory-organize
/// pass failed (`status:"failed"` inside a success outcome). The exit code
/// is the verdict for scripts; a failed run must fail the command (exit 1)
/// while its record stays durable. This environment has no display/model,
/// so the run fails for real — the exact lane the old contract misreported.
#[test]
fn run_reports_a_failed_memory_organize_pass_as_a_failed_command() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("run-exit-contract");
    // The gate needs a memory-organize task; memory must be enabled to get
    // past the create-guard, and the run then fails at the host lane.
    std::fs::write(
        home.path().join("settings.json"),
        serde_json::json!({ "language": "zh-Hans", "memory_enabled": true }).to_string(),
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
    let error = expect_failed(&["scheduled", "run", &task_id]);
    assert!(
        error.starts_with("scheduled_run_failed"),
        "a failed organize pass must fail the command: {error}"
    );
    // The durable record still exists and is terminal (`failed`).
    let runs = run_json(&["scheduled", "runs", &task_id]);
    let runs = runs["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["status"].as_str(), Some("failed"));
}

/// `runs --limit N` routes through the foundation's bounded `list_runs`:
/// sortable run files are truncated to the newest N before reading. With N
/// legacy-named files the legacy lane still reads everything (ordering
/// requires created_at), exactly like the GUI.
#[test]
fn runs_limit_returns_the_newest_records_in_order() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("runs-limit");
    let created = create_task(&home, "Limited task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    let runs = home.runs_dir(&task_id);
    std::fs::create_dir_all(&runs).unwrap();
    for index in 0..10 {
        // 19-char sortable stamp: YYYYMMDDTHHMMSSmmmZ (created_at's millis
        // mirrored into the name, foundation run_file_stamp shape). The
        // created_at/scheduled_for stamps are real RFC3339 instants the
        // foundation's chrono decode accepts — seconds 00..09, matching the
        // file name's ordering.
        let stamp = format!("20260910T08000{index}000Z");
        assert_eq!(stamp.len(), 19, "{stamp}");
        let instant = format!("2026-09-10T08:00:{index:02}.000Z");
        std::fs::write(
            runs.join(format!("{stamp}-run-{index}.json")),
            serde_json::json!({
                "schema_version": 1,
                "id": format!("run-{index}"),
                "automation_id": task_id,
                "scheduled_for": instant,
                "status": "completed",
                "created_at": instant,
            })
            .to_string(),
        )
        .unwrap();
    }
    let limited = run_json(&["scheduled", "runs", &task_id, "--limit", "3"]);
    let values = limited["runs"].as_array().unwrap();
    assert_eq!(values.len(), 3);
    assert_eq!(values[0]["id"].as_str(), Some("run-9"));
    assert_eq!(values[1]["id"].as_str(), Some("run-8"));
    assert_eq!(values[2]["id"].as_str(), Some("run-7"));
}

#[test]
fn read_commands_refuse_a_path_shaped_file_supplied_id_as_malformed() {
    // The id doubles as the on-disk workspace directory name, so a def file
    // whose id is not a single path component is malformed (the store is
    // broken), not a usage error and never a directory join.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("path-shaped-id");
    let created = create_task(&home, "Escaped id task");
    let task_id = created["id"].as_str().unwrap().to_owned();
    let mut def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    def["id"] = serde_json::json!("../escape");
    std::fs::write(home.def_path(&task_id), def.to_string()).unwrap();

    let shown = expect_failed(&["scheduled", "show", &task_id]);
    assert!(shown.contains("Failed to parse"), "{shown}");
    assert!(
        !home.root.join("escape").exists(),
        "the workspace join must never escape the scheduled root"
    );
    let _ = home;
}

// ---- model binding pairing ("--model-id" resolves the selected record) ----

#[test]
fn create_model_id_round_trips_the_selected_records_wire_name() {
    // The pair the executor's `resolve_scheduled_model` later checks is
    // (`selected.model`, `selected.id`) — the binding must carry the
    // selected record's own wire name for the modelId, not the active
    // model's, and `update --model-id` must resolve the same lookup so a
    // dangling id cannot be persisted. The GUI's create sends
    // `model: selected.model, modelId: selected.id`; pre-fix, the CLI wrote
    // the active model's wire name against sel-1's id, and the task failed
    // on every run at the model-resolution check.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("model-id-round-trip");
    write_saved_models(&home);
    let prompt = write_prompt_file(&home, "model-id.md", "Summarize the reports.");
    let created = run_json(&[
        "scheduled",
        "create",
        "--name",
        "Bound task",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        VALID_RRULE,
        "--model-id",
        "sel-1",
    ]);
    assert_eq!(created["modelId"].as_str(), Some("sel-1"));
    assert_eq!(created["model"].as_str(), Some("deepseek-flash"));
    // On-disk definition: the selected record's wire name in the `model`
    // field, and the anchor the sweep would otherwise have to guess.
    let def: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.def_path(created["id"].as_str().unwrap())).unwrap(),
    )
    .unwrap();
    assert_eq!(def["model"].as_str(), Some("deepseek-flash"));
    assert_ne!(
        def["model"].as_str(),
        Some("qwen36_35b_256k"),
        "with --model-id the definition's model must be the selected record's wire name, \
         never the active model's"
    );
    // The binding sidecar persists exactly that pair and nothing else.
    let bindings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.path().join("automations/model-bindings.json")).unwrap(),
    )
    .unwrap();
    let binding = &bindings["tasks"][created["id"].as_str().unwrap()];
    assert_eq!(binding["model_id"].as_str(), Some("sel-1"));
    assert_eq!(binding["model"].as_str(), Some("deepseek-flash"));
    let _ = home;
}

#[test]
fn create_model_id_unknown_id_is_a_state_error() {
    // An unknown id must be refused before anything is persisted; the id is
    // only decidable against `saved_models` (disk state), so the refusal is
    // exit 1 Failed like every other state-dependent gap on the family — not
    // Usage, which argv alone must decide (the family's exit-code rule).
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("model-id-unknown");
    write_saved_models(&home);
    let prompt = write_prompt_file(&home, "model-id.md", "Summarize the reports.");
    let error = expect_failed(&[
        "scheduled",
        "create",
        "--name",
        "Dangling task",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        VALID_RRULE,
        "--model-id",
        "missing-1",
    ]);
    assert!(
        error.contains("model not found") && error.contains("missing-1"),
        "{error}"
    );
    // Nothing was persisted — no task, no binding.
    let listed = run_json(&["scheduled", "list"]);
    assert_eq!(listed["tasks"].as_array().map(Vec::len), Some(0));
    assert!(
        !home.path().join("automations/model-bindings.json").exists(),
        "a refused create must not write a binding sidecar"
    );
    let _ = home;
}

// ---- one-shot anchors (create/resume persist next_run_at = AT) ----

#[test]
fn once_tasks_persist_their_anchor_on_create_and_resume() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("once-anchor");
    let prompt = write_prompt_file(&home, "once.md", "Summarize the reports.");
    let at = "2027-06-01T09:30";
    let created = run_json(&[
        "scheduled",
        "create",
        "--name",
        "Once reminder",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        &format!("FREQ=ONCE;AT={at}"),
    ]);
    let task_id = created["id"].as_str().unwrap().to_owned();
    // The anchor landed on disk as the GUI would persist it (RFC3339 UTC
    // millis): the sweep never needs to initialize it, so a first tick that
    // finds the AT elapsed fires the run late instead of failing with
    // "no future run" and silently pausing the task. The exact stamp is
    // computed in-test (not hardcoded) so the test stays correct in any
    // local timezone the runner provides.
    let def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    let expected_anchor = {
        use chrono::TimeZone as _;
        let resolved = chrono::Local
            .from_local_datetime(
                &chrono::NaiveDate::from_ymd_opt(2027, 6, 1)
                    .unwrap()
                    .and_hms_opt(9, 30, 0)
                    .unwrap(),
            )
            .earliest()
            .unwrap();
        // Parsed as an instant, not compared as a string: the foundation
        // renders the anchor at its own sub-second precision, and the
        // contract that matters is the right instant, not the right string.
        resolved.with_timezone(&chrono::Utc)
    };
    assert!(
        def["next_run_at"].is_string(),
        "a one-shot must carry its AT anchor: {}",
        def["next_run_at"]
    );
    let written =
        chrono::DateTime::parse_from_rfc3339(def["next_run_at"].as_str().expect("anchor"))
            .expect("anchor parses as RFC3339");
    assert_eq!(
        written.with_timezone(&chrono::Utc),
        expected_anchor,
        "the anchor must be the once AT (local 09:30 resolved in the local zone, rendered UTC): {}",
        def["next_run_at"]
    );

    // Pause then resume: resume must recompute the anchor, not leave null.
    run_json(&["scheduled", "pause", &task_id]);
    let paused_def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert!(paused_def["next_run_at"].is_null());
    run_json(&["scheduled", "resume", &task_id]);
    let resumed_def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert_eq!(
        resumed_def["next_run_at"], def["next_run_at"],
        "resuming a once task must restore its anchor, not leave it null"
    );
    let _ = home;
}

#[test]
fn once_tasks_with_a_past_anchor_are_refused_at_resume() {
    // The `resume` gap: a paused one-shot whose AT already elapsed used to be
    // restored with the sweep later silently pausing it ("no future run").
    // The foundation's resume refuses the past anchor up front instead — the
    // same refusal `create` applies to a past AT on an active task — so the
    // user sees the honest error rather than a task that never fires.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("once-past-resume");
    let prompt = write_prompt_file(&home, "once-past.md", "Summarize the reports.");
    let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);
    let created = run_json(&[
        "scheduled",
        "create",
        "--name",
        "Past once",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--rrule",
        &format!("FREQ=ONCE;AT={yesterday}T08:30"),
        "--paused",
    ]);
    let task_id = created["id"].as_str().unwrap().to_owned();
    let paused_def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert_eq!(paused_def["status"].as_str(), Some("paused"));
    assert!(paused_def["next_run_at"].is_null());
    // Resume of an elapsed one-shot: the foundation refuses the past anchor
    // ("no future run") instead of restoring it and letting the sweep pause
    // the task silently. The task stays paused and untouched.
    let error = expect_failed(&["scheduled", "resume", &task_id]);
    assert!(
        error.contains("no future run"),
        "a past one-shot must be refused at resume, not silently paused: {error}"
    );
    let resumed_def: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.def_path(&task_id)).unwrap()).unwrap();
    assert_eq!(
        resumed_def["status"].as_str(),
        Some("paused"),
        "the refused resume must leave the task paused: {}",
        resumed_def["status"]
    );
    assert!(
        resumed_def["next_run_at"].is_null(),
        "nothing fires without a future run: {}",
        resumed_def["next_run_at"]
    );
    let _ = home;
}

// ---- run exit contract ----

#[test]
fn run_reports_a_failed_run_through_a_nonzero_exit() {
    // A completed command reporting a failed result exits 1 instead of 0,
    // following the family's completed-command-failed-result convention —
    // pre-fix the success line printed unconditionally, so a failed run
    // looked like a success to scripts.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("run-failed-exit");
    // zh-Hans as in the sibling run-exit test (line ~706): the memory gate is
    // locale-gated and a settings file without a language defaults to a
    // non-memory locale, which silently turns the fixture's memory_enabled
    // off and fails the child before it ever renders a report.
    std::fs::write(
        home.path().join("settings.json"),
        serde_json::json!({ "language": "zh-Hans", "memory_enabled": true }).to_string(),
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
    // Deterministic failed run: a corrupted pending store makes the organize
    // pass fail inside the host (the snapshot load checks every jsonl line;
    // `_pending.jsonl` with junk lines has no valid `.bak`/`.tmp-` recovery
    // candidate, so the read surfaces InvalidData). The failure happens
    // before any LLM call, so no display, model endpoint, or network is
    // involved — the same class of failure a real broken store produces.
    // The real binary runs as a child so the event loop owns a true main
    // thread and the assertion checks the process exit code users observe.
    let memory_dir = home.path().join("user").join("memory");
    std::fs::create_dir_all(&memory_dir).unwrap();
    std::fs::write(memory_dir.join("_pending.jsonl"), "{not json\n").unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pinvou"))
        .args(["scheduled", "run", &task_id, "--output", "json"])
        .env("PINVOU3_HOME", home.path())
        .output()
        .expect("spawn pinvou scheduled run");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a failed run must exit 1, not 0; stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    // Exit-1 verdicts never carry a JSON body on stdout: the family's
    // convention (and the CLI-wide contract in main.rs) renders verdicts on
    // stdout only for successful outcomes and puts failures on stderr as
    // plain text. Keep pinned both halves: the child names the failure on
    // stderr, and its durable run record reads back terminal `failed`.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("scheduled_run_failed"),
        "the failed run must name its verdict on stderr: {stderr}"
    );
    // The record half also runs as a child: this test owns a real tao main
    // thread for the windowless host, so any in-process invocation in the
    // same test would hit `EventLoop must be created on the main thread` /
    // `runtime already initialized` and fail the suite spuriously.
    let runs_output = std::process::Command::new(env!("CARGO_BIN_EXE_pinvou"))
        .args(["scheduled", "runs", &task_id, "--output", "json"])
        .env("PINVOU3_HOME", home.path())
        .output()
        .expect("spawn pinvou scheduled runs");
    assert_eq!(runs_output.status.code(), Some(0));
    let runs: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&runs_output.stdout))
            .expect("single-line JSON runs listing from the child");
    let runs = runs["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["status"].as_str(), Some("failed"));
    assert!(
        runs[0]["error"].as_str().is_some(),
        "the durable run record carries its error: {runs:?}"
    );
    let _ = home;
}
