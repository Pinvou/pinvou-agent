//! `scheduled` family: GUI-parity surface over the scheduled-task stores the
//! GUI writes (`pinvou3-app/src-tauri/src/app/commands/scheduled.rs` ->
//! `features::scheduled::tasks`), mapped onto the same persisted files.
//!
//! Definitions and run records go through the foundation's pub
//! automation-manager API (`AutomationManager`, `AutomationSchedule::parse_rrule`
//! and the record types, reached through the app crate's
//! `pinvou3_lib::automation_foundation` facade — the architecture guard
//! requires the CLI to consume the foundation through the pinvou3_lib
//! surface, not past it) — the same calls the GUI feature layer makes —
//! instead of a CLI-local mirror of that module; a mirror cannot see
//! upstream fixes (that is how this family ended up writing
//! `next_run_at: null` on create where the foundation resolves the
//! slot eagerly, silently pausing past one-shots). What stays CLI-local:
//! - atomic JSON writes with a per-pid/nanos-unique staging file
//!   (`write_json_atomic`); the foundation writer still stages under a
//!   fixed `.json.tmp` sibling name shared by all concurrent writers.
//! - run-record persistence for terminal CLI runs (`save_run` is private in
//!   the foundation; the CLI stores the identical record shape under the
//!   identical sortable file name, so the GUI's `list_runs` co-reads them).
//! - the app-owned sidecar registries (`model-bindings.json`,
//!   `task-kinds.json`, `task-ui-metadata.json`, read state, history
//!   archive; `features::scheduled::stores` domain), read-modify-written
//!   under the shared store lock.
//! - run sessions: `pinvou3_lib::features::sessions::SessionStore` (public),
//!   the same calls the GUI mapper makes (`scheduled_profile`, `is_pinned`,
//!   `pinned_at`, `is_hidden`, `list_scheduled`).
//!
//! Disclosed deviation from the GUI: run-status reconciliation needs the
//! foundation `TaskManager`; runs are reported exactly as persisted.
//!
//! `scheduled run` executes only `memory_organize` tasks (app-side, no
//! engine conversation; same wiring as `memory organize` in this crate,
//! minus the session-bound bridge which is `pub(crate)` to `pinvou3_lib`).
//! Chat-kind run-now drives the GUI's `ScheduledChatExecutor` +
//! `TaskManager`, which are not exposed headlessly, and is refused with a
//! stable error instead of being faked.
//! A run whose outcome status is `failed` exits 1 with the same output
//! body — the completed-command-failed-result convention the `models`
//! family established for `probe-local`; an exit-0 would tell scripts
//! the run succeeded.
//!
//! `update --model-id X` re-binds the model pin AND the definition's model
//! wire name as one pair, exactly like the GUI's update (`model:
//! selected.model, modelId: selected.id`) — the executor rejects any other
//! combination (`resolve_scheduled_model`). An unknown X is refused before
//! anything is persisted. Changing only the wire name (no id) is not
//! offered: the CLI has no flag that names a bare wire model — edit the
//! model in the GUI, or delete and recreate the task.

use std::path::{Path, PathBuf};

use serde_json::Value;

use pinvou3_lib::automation_foundation::{
    AutomationManager, AutomationRecord, AutomationRunRecord, AutomationSchedule, AutomationStatus,
    CreateAutomationRequest, UpdateAutomationRequest,
};
use pinvou3_lib::features::memory as memory_feature;
use pinvou3_lib::features::sessions::SessionStore;
use pinvou3_lib::platform::prefs::UserPrefs;

use crate::support::{render, require_yes, sandbox_home, success};
use crate::{CliError, CliOutcome, ExitCode, OutputMode};

const SCHEDULED_USAGE: &str = "usage: pinvou scheduled \
<list|show|create|update|pause|resume|pin|unpin|delete|run|runs|runs-all|mark-viewed|chat-prompt>";

const RUN_HELP_HOST_REQUIREMENT: &str = "scheduled run boots the windowless product host: it \
requires a display (xvfb on headless Linux), a configured active model, and the scheduled task \
runtime; only memory-organize tasks can run headlessly";

/// Task kind. `Chat` is the ordinary engine-conversation task (no sidecar
/// entry); `MemoryOrganize` persists `memory_organize` to `task-kinds.json`,
/// exactly like the GUI's create-side allow-list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskKind {
    Chat,
    MemoryOrganize,
}

impl TaskKind {
    fn parse_value(value: &str) -> Result<Self, CliError> {
        match value {
            "chat" => Ok(Self::Chat),
            "memory-organize" | "memory_organize" => Ok(Self::MemoryOrganize),
            other => Err(CliError::usage(format!(
                "unknown scheduled task kind '{other}' (valid: chat, memory-organize); \
kind can only be set at create time"
            ))),
        }
    }

    /// The value persisted in `task-kinds.json`; chat tasks get no entry.
    fn stored_kind(self) -> Option<&'static str> {
        match self {
            Self::Chat => None,
            Self::MemoryOrganize => Some("memory_organize"),
        }
    }
}

/// Approval mode. A scheduled run has no operator in front of it, so no
/// approval prompt can ever be answered: the app overwrites `mode` with
/// `yolo` unconditionally on both the create and the update request
/// (`features::scheduled::tasks::SCHEDULED_EXECUTION_MODE`), which turns any
/// other accepted value into a silently discarded user choice — the caller
/// asks for `plan` and is handed a full-YOLO task with `trust_mode` and
/// `auto_approve` on. The app therefore refuses `agent`/`plan` outright in
/// `canonical_scheduled_mode`, and this enum mirrors that decision by having
/// no variant to represent them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TaskMode {
    #[default]
    Yolo,
}

impl TaskMode {
    /// Mirrors the app's `canonical_scheduled_mode`: absent, empty (the flag
    /// carries no opinion) or an exact `yolo` passes; everything else is
    /// refused by name. The refusal is a usage error (exit 2) because the
    /// value is invalid at parse time — the same class `scheduled update`
    /// already uses for its blanket `--mode` rejection, so both subcommands
    /// classify the identical refusal identically.
    fn parse_value(value: &str) -> Result<Self, CliError> {
        match value.trim() {
            "" | Self::PERSISTED => Ok(Self::Yolo),
            other => Err(CliError::usage(format!(
                "scheduled tasks always run in '{}' mode, got '{other}': a scheduled run \
cannot answer an approval prompt, so the mode is forced to yolo (trust mode and auto-approve \
on) and an 'agent'/'plan' request would be discarded without notice",
                Self::PERSISTED
            ))),
        }
    }

    /// The mode persisted for every scheduled task.
    const PERSISTED: &'static str = "yolo";

    fn persisted(self) -> &'static str {
        match self {
            Self::Yolo => Self::PERSISTED,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScheduledCommand {
    List,
    Show {
        id: String,
    },
    Create {
        name: String,
        prompt_file: PathBuf,
        rrule: String,
        kind: TaskKind,
        model_id: Option<String>,
        mode: Option<TaskMode>,
        paused: bool,
    },
    Update {
        id: String,
        name: Option<String>,
        prompt_file: Option<PathBuf>,
        rrule: Option<String>,
        model_id: Option<String>,
    },
    Pause {
        id: String,
    },
    Resume {
        id: String,
    },
    Pin {
        id: String,
    },
    Unpin {
        id: String,
    },
    Delete {
        id: String,
        yes: bool,
    },
    Run {
        id: String,
    },
    Runs {
        id: String,
        limit: Option<usize>,
    },
    RunsAll {
        limit: Option<usize>,
    },
    MarkViewed {
        task_id: String,
        run_id: String,
    },
    ChatPrompt,
}

/// Fixed AI task-creation prompt served by the GUI `scheduled_task_chat_prompt`
/// command (`features::scheduled::tasks::SCHEDULED_TASK_CHAT_PROMPT`, copied
/// verbatim: the GUI feature is `pub(crate)` to `pinvou3_lib`, so the const
/// cannot be referenced from the CLI, and this is GUI data the model consumes,
/// not CLI copy). `chat_prompt_mirrors_the_gui_once_scheduling_guidance` in
/// scheduled_contract.rs pins the ONCE guidance so copy drift fails CI.
const SCHEDULED_TASK_CHAT_PROMPT: &str = r#"我想创建一个 Pinvou 定时任务。请通过提问帮我确定方案，回复保持简短，不要长篇解释。

这是一个纯对话收集流程。不要调用任何工具，不要写文件，不要读写 ~/.pinvou3，也不要手动创建 automations JSON。信息完整后只输出给前端解析的任务参数，前端会通过 create_scheduled_task 创建并打开任务详情，不再要求用户二次确认。

严禁使用 schtasks、Windows Task Scheduler、任务计划程序、cron、crontab、systemd timer 或任何系统级计划任务。错误做法：使用 schtasks 创建 Windows 任务。正确做法：返回 scheduled-task-draft JSON，由 Pinvou 前端调用 create_scheduled_task。

请一次只问我一个问题，并依次确认这些信息：
1. 任务要做什么。
2. 什么时候运行。支持每 N 小时（可指定起始时间）、每天指定时间、每周指定星期和时间，以及一次性定时（在指定时刻运行一次后自动结束，适合“明天 9 点提醒我一次”这类需求）。一次性定时的 AT 只用本地时刻 YYYY-MM-DDTHH:MM，不要带 Z 或时区偏移后缀。如果用户指定的一次性时刻已经过去，必须先和用户确认改成未来的时刻，不要输出草稿。不支持分钟级规则；如果用户要求“每 5 分钟”等分钟级频率，必须询问用户改成每 N 小时、每天指定时间或每周指定时间，不要输出草稿。

每次运行创建独立对话；同一个定时任务的所有运行对话共享该任务的专属工作间，不同任务互不共享。产物仍归属各次运行对话。不需要询问工作目录或权限设置。

整理草稿时，请把时间转换成 rrule：
- 每 6 小时一次，从 08:30 起算：FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30
- 每天 08:30：FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30
- 每周一、三 09:30：FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=30
- 2027-06-01 09:30 运行一次：FREQ=ONCE;AT=2027-06-01T09:30

当信息足够时，请直接给出最终任务参数，并使用下面这种完整代码块格式：
```scheduled-task-draft
{
  "name": "AI 招聘情报晨报",
  "prompt": "检索并汇总...",
  "rrule": "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30",
  "paused": false
}
```
输出代码块后不要继续提问，也不要假装自己调用了创建命令；前端会负责创建任务。"#;

pub fn parse(values: &[String]) -> Result<ScheduledCommand, CliError> {
    let subcommand = values
        .get(1)
        .ok_or_else(|| CliError::usage(SCHEDULED_USAGE))?;
    let rest = &values[2..];
    match subcommand.as_str() {
        "list" => {
            expect_no_arguments(rest, "scheduled list")?;
            Ok(ScheduledCommand::List)
        }
        "show" => {
            let id = require_id(rest.first(), "scheduled show")?;
            expect_no_arguments(&rest[1..], "scheduled show")?;
            Ok(ScheduledCommand::Show { id })
        }
        "create" => parse_create(rest),
        "update" => parse_update(rest),
        "run" => {
            let id = rest.first().ok_or_else(|| {
                CliError::usage(format!(
                    "scheduled run requires a task id; {RUN_HELP_HOST_REQUIREMENT}"
                ))
            })?;
            let id = require_id(Some(id), "scheduled run")?;
            expect_no_arguments(&rest[1..], "scheduled run")?;
            Ok(ScheduledCommand::Run { id })
        }
        "pause" | "resume" | "pin" | "unpin" => {
            let id = require_id(rest.first(), &format!("scheduled {subcommand}"))?;
            expect_no_arguments(&rest[1..], &format!("scheduled {subcommand}"))?;
            Ok(match subcommand.as_str() {
                "pause" => ScheduledCommand::Pause { id },
                "resume" => ScheduledCommand::Resume { id },
                "pin" => ScheduledCommand::Pin { id },
                _ => ScheduledCommand::Unpin { id },
            })
        }
        "delete" => {
            let id = require_id(rest.first(), "scheduled delete")?;
            let (_, flags) = parse_flags(&rest[1..], &[], &["--yes"])?;
            Ok(ScheduledCommand::Delete {
                id,
                yes: flags.contains(&"--yes"),
            })
        }
        "runs" => {
            let id = require_id(rest.first(), "scheduled runs")?;
            let (options, _) = parse_flags(&rest[1..], &["--limit"], &[])?;
            Ok(ScheduledCommand::Runs {
                id,
                limit: parse_limit(&options, "--limit")?,
            })
        }
        "runs-all" => {
            let (options, _) = parse_flags(rest, &["--limit"], &[])?;
            Ok(ScheduledCommand::RunsAll {
                limit: parse_limit(&options, "--limit")?,
            })
        }
        "mark-viewed" => {
            let task_id = require_id(rest.first(), "scheduled mark-viewed")?;
            let run_id = rest.get(1).cloned().ok_or_else(|| {
                CliError::usage("scheduled mark-viewed requires <task-id> <run-id>")
            })?;
            expect_no_arguments(&rest[2..], "scheduled mark-viewed")?;
            Ok(ScheduledCommand::MarkViewed { task_id, run_id })
        }
        "chat-prompt" => {
            expect_no_arguments(rest, "scheduled chat-prompt")?;
            Ok(ScheduledCommand::ChatPrompt)
        }
        other => Err(CliError::usage(format!(
            "unknown scheduled command '{other}'; {SCHEDULED_USAGE}"
        ))),
    }
}

fn parse_create(rest: &[String]) -> Result<ScheduledCommand, CliError> {
    let (options, flags) = parse_flags(
        rest,
        &[
            "--name",
            "--prompt-file",
            "--rrule",
            "--kind",
            "--model-id",
            "--mode",
        ],
        &["--paused"],
    )?;
    let name = option(&options, "--name")
        .ok_or_else(|| CliError::usage("scheduled create requires --name N"))?
        .to_owned();
    let prompt_file = PathBuf::from(
        option(&options, "--prompt-file")
            .ok_or_else(|| CliError::usage("scheduled create requires --prompt-file F"))?,
    );
    let rrule = option(&options, "--rrule")
        .ok_or_else(|| {
            CliError::usage(
                "scheduled create requires --rrule R (e.g. \
'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30')",
            )
        })?
        .to_owned();
    // Syntactic gate at parse time for EVERY create, `--paused` included:
    // the rrule string is argv, so a grammar-invalid value is a usage error
    // (exit 2) — skipping the check for a paused create would surface the
    // same input later as `scheduled_create_failed` (exit 1) and would
    // bypass the MAX_HOURLY_INTERVAL pre-check that keeps a
    // scheduler-breaking record out of the store. `--paused` only defers
    // the past-ONCE/next-slot calendar resolution, which stays the
    // foundation `create_automation`'s own eager policy for an ACTIVE task
    // (the CLI used to defer it and a past one-shot ended up Paused by the
    // first sweep, never running): grammar here (exit 2), calendar truth in
    // the layer that owns the rule (exit 1).
    validate_rrule(&rrule)?;
    let paused = flags.contains(&"--paused");
    let kind = match option(&options, "--kind") {
        Some(value) => TaskKind::parse_value(value)?,
        None => TaskKind::Chat,
    };
    let model_id = option(&options, "--model-id").map(str::to_owned);
    let mode = match option(&options, "--mode") {
        Some(value) => Some(TaskMode::parse_value(value)?),
        None => None,
    };
    Ok(ScheduledCommand::Create {
        name,
        prompt_file,
        rrule,
        kind,
        model_id,
        mode,
        paused,
    })
}

fn parse_update(rest: &[String]) -> Result<ScheduledCommand, CliError> {
    let id = require_id(rest.first(), "scheduled update")?;
    // `--kind` is a create-time property and `--mode` is forced to `yolo`
    // for every run (the GUI validates it and still persists yolo, so an
    // update can never change the effective value); reject them with the
    // reason before the generic parser instead of silently discarding the
    // user's flag.
    for rejected in ["--kind", "--mode"] {
        if rest[1..].iter().any(|token| token == rejected) {
            return Err(CliError::usage(format!(
                "scheduled update does not accept {rejected}: kind is settable only at \
                 create time, and every run is forced to yolo like the GUI"
            )));
        }
    }
    let (options, _) = parse_flags(
        &rest[1..],
        &["--name", "--prompt-file", "--rrule", "--model-id"],
        &[],
    )?;
    let name = option(&options, "--name").map(str::to_owned);
    let prompt_file = option(&options, "--prompt-file").map(PathBuf::from);
    let rrule = match option(&options, "--rrule") {
        Some(value) => {
            // Unconditionally active-grammar: `update` cannot see the stored
            // status from the parser, and the strict reading is the safe
            // default here — `resume` on an active-again task would otherwise
            // meet a one-shot the sweep can never schedule.
            validate_rrule(value)?;
            Some(value.to_owned())
        }
        None => None,
    };
    let model_id = option(&options, "--model-id").map(str::to_owned);
    if name.is_none() && prompt_file.is_none() && rrule.is_none() && model_id.is_none() {
        return Err(CliError::usage(
            "scheduled update requires at least one of --name, --prompt-file, --rrule, \
--model-id",
        ));
    }
    Ok(ScheduledCommand::Update {
        id,
        name,
        prompt_file,
        rrule,
        model_id,
    })
}

fn expect_no_arguments(values: &[String], command: &str) -> Result<(), CliError> {
    if values.is_empty() {
        Ok(())
    } else {
        Err(CliError::usage(format!(
            "{command} accepts no options or arguments"
        )))
    }
}

fn require_id(value: Option<&String>, command: &str) -> Result<String, CliError> {
    let id = value
        .ok_or_else(|| CliError::usage(format!("{command} requires a task id")))?
        .trim()
        .to_owned();
    if id.is_empty() {
        return Err(CliError::usage(format!("{command} requires a task id")));
    }
    Ok(id)
}

/// Shared implementation in `support::parse_family_flags`; `family`
/// only names this family in error messages.
fn parse_flags<'a>(
    values: &'a [String],
    value_flags: &[&str],
    boolean_flags: &[&str],
) -> Result<(Vec<(&'a str, &'a str)>, Vec<&'a str>), CliError> {
    crate::support::parse_family_flags(values, value_flags, boolean_flags, "scheduled")
}

fn option<'a>(options: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    crate::support::family_option(options, name)
}

fn parse_limit(options: &[(&str, &str)], name: &str) -> Result<Option<usize>, CliError> {
    crate::support::parse_family_positive::<usize>(options, name, "scheduled")
}

// ---- rrule validation ----
// The grammar belongs to the foundation (`AutomationSchedule::parse_rrule`,
// pub in codewhale-tui): the CLI validates by CALLING it, not by mirroring
// it. Mirrors drift — the previous copy had already diverged in the cron
// atom iteration (an added `next <= current` overflow guard the foundation
// later grew for negative steps), the weekly BYDAY emptiness rule and the
// ONCE AT loose-width acceptance (chrono `%H:%M` accepts 1-digit fields the
// old CLI mirror rejected). A failure here is a usage error (exit 2): the
// rrule string is argv, invalid at parse time, same class as the families'
// other input validation.

/// Upper bound for `FREQ=HOURLY;INTERVAL`. The foundation's unanchored
/// sweep branch does `DateTime + Duration::hours(interval)` for up to
/// `MAX_HOURLY_SEARCH_STEPS` iterations under a BYDAY filter; an absurd
/// INTERVAL overflows chrono's date range and errors out the entire sweep,
/// stalling every GUI automation until the record is removed by hand — and
/// the round-17 tree even observed it panic the scheduler task. The GUI
/// can never persist such a record (it evaluates the first slot eagerly at
/// create), so this pre-check keeps the guard for the records the CLI
/// itself writes. 1e6 hours (~114 years) per step keeps the worst-case
/// 504-step reach (~57k years) far inside chrono's ±262k-year range.
const MAX_HOURLY_INTERVAL: u32 = 1_000_000;

/// Validates an rrule through the foundation parser and converts its
/// failure into the CLI usage-error class. The per-field pre-checks below
/// duplicate nothing the parser already does — they keep only the guards
/// that must fire BEFORE a record can be created at all (the
/// scheduler-breaking INTERVAL bound and the fields the parser itself does
/// not bound).
fn validate_rrule(rrule: &str) -> Result<(), CliError> {
    let parts = parse_rrule_pairs(rrule)?;
    for (key, _) in &parts {
        if key == "INTERVAL" {
            // Only for the HOURLY grammar, where the sweep does interval
            // arithmetic; ONCE/WEEKLY/CRON ignore the field.
            if matches!(freq_of(&parts).as_deref(), Some("HOURLY")) {
                let interval = parts
                    .iter()
                    .find(|(name, _)| name == "INTERVAL")
                    .and_then(|(_, value)| value.parse::<u32>().ok())
                    .unwrap_or(1);
                if interval > MAX_HOURLY_INTERVAL {
                    return Err(CliError::usage(format!(
                        "INTERVAL must be <= {MAX_HOURLY_INTERVAL} for HOURLY schedules (a larger \
                         step cannot be evaluated by the scheduler without overflowing its \
                         date range)"
                    )));
                }
            }
        }
    }
    match AutomationSchedule::parse_rrule(rrule) {
        Ok(_) => Ok(()),
        Err(error) => Err(CliError::usage(format!(
            "invalid rrule '{rrule}': {error:#}"
        ))),
    }
}

/// KEY=VALUE pairs of an rrule (uppercased keys), the shape
/// `AutomationSchedule::parse_rrule` reads; also used by the
/// human-readable schedule label.
fn parse_rrule_pairs(rrule: &str) -> Result<Vec<(String, String)>, CliError> {
    let mut parts: Vec<(String, String)> = Vec::new();
    for raw in rrule.split(';') {
        let item = raw.trim();
        if item.is_empty() {
            continue;
        }
        let Some((key, value)) = item.split_once('=') else {
            return Err(CliError::usage(format!(
                "invalid rrule segment '{item}' (expected KEY=VALUE pairs separated by ';')"
            )));
        };
        let key = key.trim().to_ascii_uppercase();
        if let Some(existing) = parts.iter_mut().find(|(name, _)| *name == key) {
            existing.1 = value.trim().to_string();
        } else {
            parts.push((key, value.trim().to_string()));
        }
    }
    Ok(parts)
}

fn freq_of(parts: &[(String, String)]) -> Option<String> {
    parts
        .iter()
        .find(|(key, _)| key == "FREQ")
        .map(|(_, value)| value.trim().to_ascii_uppercase())
}

fn parse_byday(value: &str) -> Vec<&'static str> {
    // Parser is the foundation's; this renders the label (best effort).
    let mut days = Vec::new();
    for token in value.split(',') {
        let day = match token.trim().to_ascii_uppercase().as_str() {
            "MO" => "MO",
            "TU" => "TU",
            "WE" => "WE",
            "TH" => "TH",
            "FR" => "FR",
            "SA" => "SA",
            "SU" => "SU",
            _ => continue,
        };
        if !days.contains(&day) {
            days.push(day);
        }
    }
    days
}

/// Record ordering key from a JSON field: the foundation sorts by chrono
/// `DateTime<Utc>`; the CLI reads the same stamps through the foundation's
/// own parser semantics via `AutomationRecord`-equivalent decode — a value
/// that is not an RFC3339 instant sorts last (it can only be hand-edited
/// storage; every CLI writer writes chrono-rendered stamps and all read
/// gates refuse undecodable records before ordering runs).
fn record_time(value: &serde_json::Value, field: &str) -> chrono::DateTime<chrono::Utc> {
    // Sorting floor for undecodable stamps: the smallest chrono can
    // represent (year -262143 overflows for many chrono versions, so
    // MIN_DATETIME safe bound via from_timestamp at its documented
    // boundary). Real records never reach this — every read gate refuses
    // undecodable stamps first; only the lossy archive lane orders
    // untrusted data, and there it must sort LAST.
    const FLOOR_SECS: i64 = -62_135_596_800; // 0001-01-01T00:00:00Z
    value
        .get(field)
        .and_then(|value| value.as_str())
        .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
        .map(|stamp| stamp.with_timezone(&chrono::Utc))
        .unwrap_or(chrono::DateTime::from_timestamp(FLOOR_SECS, 0).expect("sorting floor"))
}

// ---- store layout (mirrors AutomationManager::open(root) + app sidecars) ----

fn safe_storage_id(kind: &str, value: &str) -> Result<(), CliError> {
    let path = Path::new(value);
    let mut components = path.components();
    match components.next() {
        Some(std::path::Component::Normal(_)) if components.next().is_none() => Ok(()),
        // A malformed id is invalid user input, not a runtime failure: the
        // refusal is a usage error (exit 2), matching the crate-wide input
        // validation convention (sessions id alphabet, knowledge integer
        // parse).
        _ => Err(CliError::usage(format!(
            "{kind} must be a single path component: {value}"
        ))),
    }
}

/// Defense in depth, kept from the pre-port read gate: the decoded record's
/// id doubles as the on-disk workspace directory name (`workspace_dir`,
/// `ensure_workspace`), so a hand-edited definition carrying a
/// non-component id must be refused on read instead of flowing into a
/// directory join. The typed `AutomationRecord` decode accepts any string
/// id, so this check rides alongside it, in the same malformed-definition
/// refusal class ("Failed to parse …"): the store is broken and only its
/// fix is user-actionable, never a usage error — the id came from the file,
/// not from argv.
fn require_safe_record_id(record: &AutomationRecord) -> Result<(), CliError> {
    if safe_storage_id("scheduled task id", &record.id).is_err() {
        return Err(CliError::failed(format!(
            "Failed to parse scheduled task definition: its stored id '{}' is not a single \
             path component; fix or remove the definition file manually",
            record.id
        )));
    }
    Ok(())
}

/// Task store rooted at the sandbox home, mirroring
/// `features::scheduled::tasks::open_scheduled_automation_manager`:
/// `AutomationManager::open(<home>/automations)` places definitions under
/// `<home>/automations/automations` and runs under `<home>/automations/runs`.
struct TaskStore {
    home: PathBuf,
}

impl TaskStore {
    fn new() -> Result<Self, CliError> {
        Ok(Self {
            home: sandbox_home()?,
        })
    }

    fn root(&self) -> PathBuf {
        self.home.join("automations")
    }

    fn defs_dir(&self) -> PathBuf {
        self.root().join("automations")
    }

    fn def_path(&self, id: &str) -> Result<PathBuf, CliError> {
        safe_storage_id("scheduled task id", id)?;
        Ok(self.defs_dir().join(format!("{id}.json")))
    }

    fn runs_dir_for(&self, id: &str) -> Result<PathBuf, CliError> {
        safe_storage_id("scheduled task id", id)?;
        Ok(self.root().join("runs").join(id))
    }

    fn model_bindings_path(&self) -> PathBuf {
        self.root().join("model-bindings.json")
    }

    fn task_kinds_path(&self) -> PathBuf {
        self.root().join("task-kinds.json")
    }

    fn ui_metadata_path(&self) -> PathBuf {
        self.root().join("task-ui-metadata.json")
    }

    fn history_archive_path(&self) -> PathBuf {
        self.root().join("history-archive.json")
    }

    fn read_state_path(&self) -> PathBuf {
        pinvou3_lib::platform::paths::scheduled_run_read_state_path()
    }

    fn workspace_dir(&self, id: &str) -> PathBuf {
        pinvou3_lib::platform::paths::scheduled_task_workspace_dir(id)
    }

    /// The foundation manager over the same root. Its `open` creates the
    /// directory layout (idempotent); all typed reads and writes of
    /// definitions and run listings go through it.
    fn manager(&self) -> Result<AutomationManager, CliError> {
        AutomationManager::open(self.root()).map_err(|error| {
            CliError::failed(format!(
                "scheduled_storage_unavailable: cannot open the automation store {}: {error:#}",
                self.root().display()
            ))
        })
    }

    /// One task definition; a missing file maps to the stable
    /// `scheduled_task_not_found` failure, like the GUI command errors.
    /// Typed decode: the foundation `AutomationRecord` deserialization, so
    /// a wrong-shaped or undecodable definition is refused exactly like the
    /// app's typed reader refuses it (instead of rendering a phantom task).
    fn read_def(&self, id: &str) -> Result<serde_json::Value, CliError> {
        let path = self.def_path(id)?;
        if !path.exists() {
            return Err(CliError::failed(format!("scheduled_task_not_found: {id}")));
        }
        let record = self.manager()?.get_automation(id).map_err(|error| {
            CliError::failed(format!(
                "scheduled_storage_unavailable: cannot read scheduled task {id}: {error:#}"
            ))
        })?;
        require_safe_record_id(&record)?;
        Ok(def_to_value(&record))
    }

    /// Every task definition, newest `updated_at` first, like
    /// `AutomationManager::list_automations`.
    fn list_defs(&self) -> Result<Vec<serde_json::Value>, CliError> {
        let records = self.manager()?.list_automations().map_err(|error| {
            CliError::failed(format!(
                "scheduled_storage_unavailable: cannot list scheduled tasks: {error:#}"
            ))
        })?;
        let mut defs = Vec::with_capacity(records.len());
        for record in &records {
            require_safe_record_id(record).map_err(|error| {
                CliError::failed(format!(
                    "scheduled_storage_unavailable: cannot list scheduled tasks: {error:#}"
                ))
            })?;
            defs.push(def_to_value(record));
        }
        Ok(defs)
    }

    /// Typed write through the foundation manager (same normalization and
    /// serialization the GUI's writes go through).
    fn write_def(&self, def: &serde_json::Value) -> Result<(), CliError> {
        let record = def_from_value(def)?;
        self.manager()?.save_automation(&record).map_err(|error| {
            CliError::failed(format!(
                "scheduled_storage_unavailable: cannot write scheduled task: {error:#}"
            ))
        })
    }

    /// Run records for one task, newest first, legacy `{run_id}.json` files
    /// merged in — the foundation `AutomationManager::list_runs`, which is
    /// bounded: sortable files are truncated to `limit` BEFORE reading when
    /// the limit is set.
    fn list_runs(
        &self,
        id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<serde_json::Value>, CliError> {
        let runs = self.manager()?.list_runs(id, limit).map_err(|error| {
            CliError::failed(format!(
                "scheduled_storage_unavailable: cannot list runs for scheduled task {id}: \
                     {error:#}"
            ))
        })?;
        Ok(runs.iter().map(run_to_value).collect())
    }

    /// Persist one terminal CLI run record. `AutomationManager::save_run` is
    /// private, so the CLI writes the same record through the same JSON
    /// shim to the same sortable file name (foundation
    /// `{stamp}-{run_id}.json`, chrono-based) and drops a legacy-named twin,
    /// mirroring `save_run`'s migration step.
    fn save_run(&self, run: &serde_json::Value) -> Result<(), CliError> {
        let record = run_from_value(run)?;
        let dir = self.runs_dir_for(&record.automation_id)?;
        std::fs::create_dir_all(&dir).map_err(|error| {
            CliError::failed(format!(
                "scheduled_storage_unavailable: cannot create {}: {error}",
                dir.display()
            ))
        })?;
        let stamp = record.created_at.format("%Y%m%dT%H%M%S%3fZ").to_string();
        let path = dir.join(format!("{stamp}-{}.json", record.id));
        write_json_atomic(&path, run)?;
        // Mirror the foundation: rewrites of a legacy-named run migrate it
        // to the sortable name; drop the old file so the run never exists
        // twice on disk (list dedups by id, but the twin still leaks stale
        // content to direct readers).
        let legacy = dir.join(format!("{}.json", record.id));
        if legacy != path && legacy.exists() {
            std::fs::remove_file(&legacy).map_err(|error| {
                CliError::failed(format!(
                    "scheduled_storage_unavailable: cannot remove legacy run {}: {error}",
                    legacy.display()
                ))
            })?;
        }
        Ok(())
    }
}

/// JSON <-> `AutomationRecord` shims. The wire JSON is the record's own
/// serde shape (`schema_version: 2`, snake_case fields, chrono RFC3339
/// stamps), so the mapping is a typed round-trip: a def carrying a missing
/// or wrong-typed required field fails the typed decode inside the shim
/// instead of rendering as a phantom task — the same class the previous
/// hand-rolled `require_object_definition` gate covered.
fn def_from_value(def: &serde_json::Value) -> Result<AutomationRecord, CliError> {
    serde_json::from_value(def.clone()).map_err(|error| {
        CliError::failed(format!(
            "scheduled_storage_unavailable: task definition is malformed: {error}"
        ))
    })
}

fn def_to_value(record: &AutomationRecord) -> serde_json::Value {
    serde_json::to_value(record).expect("AutomationRecord serializes")
}

fn run_from_value(run: &serde_json::Value) -> Result<AutomationRunRecord, CliError> {
    serde_json::from_value(run.clone()).map_err(|error| {
        CliError::failed(format!(
            "scheduled_storage_unavailable: run record is malformed: {error}"
        ))
    })
}

fn run_to_value(record: &AutomationRunRecord) -> serde_json::Value {
    serde_json::to_value(record).expect("AutomationRunRecord serializes")
}

fn write_json_atomic(path: &Path, value: &serde_json::Value) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CliError::failed(format!(
                "scheduled_storage_unavailable: cannot create {}: {error}",
                parent.display()
            ))
        })?;
    }
    let content = serde_json::to_vec_pretty(value).map_err(|error| {
        CliError::failed(format!("scheduled_storage_unavailable: serialize: {error}"))
    })?;
    // Unique per-pid/nanos staging name (round-16 fix, kept): two concurrent
    // writers sharing a fixed `*.json.tmp` name could rename each other's
    // content. Hidden sibling in the target's own directory: same filesystem
    // (so the rename is atomic) and never surfaced as a stray visible file.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CliError::failed(format!(
                "scheduled_storage_unavailable: {} has no usable file name",
                path.display()
            ))
        })?;
    let tmp = path.with_file_name(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));
    write_json_atomic_staged(path, &tmp, &content)
}

/// [`write_json_atomic`] with the staging path supplied by the caller.
///
/// Split out for the same reason the round-16 `artifacts::atomic_write_staged`
/// exists: the real staging name embeds a nanosecond timestamp, which makes
/// the `create_new` guarantee — the one the previous non-`O_EXCL` copy
/// lacked — untestable through the public entry point.
fn write_json_atomic_staged(path: &Path, tmp: &Path, content: &[u8]) -> Result<(), CliError> {
    let stage = (|| -> std::io::Result<()> {
        // create_new (O_EXCL): never truncate, and never follow a symlink
        // someone planted at the staging path — the non-O_EXCL pattern the
        // artifacts writer documented as removed (round-18 review).
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(tmp)?;
        std::io::Write::write_all(&mut file, content)?;
        // Propagated, not discarded: this is the step that makes the rename
        // safe to perform at all.
        file.sync_all()
    })();
    if let Err(error) = stage {
        // Only clean up staging failures that are NOT "something was already
        // there": removing a path we refused to open would delete exactly the
        // file (or symlink) `create_new` protected.
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            let _ = std::fs::remove_file(tmp);
        }
        return Err(CliError::failed(format!(
            "scheduled_storage_unavailable: cannot write {}: {error}",
            tmp.display()
        )));
    }
    if let Err(error) = std::fs::rename(tmp, path) {
        let _ = std::fs::remove_file(tmp);
        return Err(CliError::failed(format!(
            "scheduled_storage_unavailable: cannot move {} to {}: {error}",
            tmp.display(),
            path.display()
        )));
    }
    // Best-effort like the artifacts writer: the data is already durable,
    // this only shortens the window in which the directory entry is not.
    if let Ok(dir) = std::fs::File::open(path.parent().unwrap_or_else(|| Path::new("."))) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Cross-process write serialization for the scheduled store: every mutating
/// subcommand is a whole-file read-modify-write of the definition or a
/// sidecar registry, so two concurrent writers would each read the same base
/// and the second write silently drop the first (mark-viewed, pin, model
/// bindings, kind, delete/archive all race this way). The advisory fd-lock
/// serializes the CLI×CLI half; a concurrent GUI write takes no CLI lock
/// (the GUI keeps its own in-process registry and persists wholesale) and
/// stays a documented residual, like the code family's session locks.
fn scheduled_store_lock() -> Result<fd_lock::RwLock<std::fs::File>, CliError> {
    let dir = pinvou3_lib::platform::paths::pinvou3_home().join("locks");
    std::fs::create_dir_all(&dir).map_err(|error| {
        CliError::failed(format!(
            "scheduled_storage_unavailable: cannot create the lock directory {}: {error}",
            dir.display()
        ))
    })?;
    let path = dir.join("scheduled-store.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| {
            CliError::failed(format!(
                "scheduled_storage_unavailable: cannot open the store lock {}: {error}",
                path.display()
            ))
        })?;
    Ok(fd_lock::RwLock::new(file))
}

/// Reads a versioned sidecar registry (model bindings, task kinds, UI
/// metadata, read state, history archive); a missing file is the empty
/// default. An unreadable or wrong-shaped payload is quarantined next to the
/// original (`<name>.invalid-<timestamp>`, the GUI `VersionedJsonStore`
/// convention) before degrading to the default for this process — otherwise
/// the next write through this process would silently destroy the only copy
/// of the other tasks' data. Newer-schema files stay untouched: they take
/// the refusal path (`ensure_supported_schema`), not quarantine.
fn read_registry(path: &Path, keys: &[&str]) -> serde_json::Value {
    match std::fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(value) if registry_shape_valid(&value, keys) => value,
            _ => {
                quarantine_unreadable(path);
                serde_json::Value::Null
            }
        },
        Err(_) => serde_json::Value::Null,
    }
}

/// Shape gate mirroring the GUI `VersionedJsonStore`'s typed
/// deserialization: a payload that parses as JSON but can never deserialize
/// into the registry being read (a non-object top level like `[]`, or one of
/// the registry's own keys of the wrong type like `{"tasks": []}`) is
/// treated like a parse failure — quarantined, never normalized in place and
/// overwritten. Only the keys the caller's registry owns are checked: the
/// GUI's deserializer ignores unknown members, so a read-state file with a
/// stray `tasks` key must stay readable here too instead of being
/// quarantined into data loss.
fn registry_shape_valid(value: &serde_json::Value, keys: &[&str]) -> bool {
    value.is_object()
        && keys
            .iter()
            .all(|key| value.get(key).is_none_or(Value::is_object))
}

/// Best-effort `.invalid-<timestamp>` quarantine of a registry that failed
/// to parse; failure to quarantine is ignored (the write path still refuses
/// to treat the file as data — degrading to the default loses only the
/// malformed file's own content, as before). The file is renamed aside
/// (not copied, the GUI's `handle_invalid` semantics): a copy would leave
/// the malformed bytes at the canonical path and pile up a fresh quarantine
/// copy on every subsequent read. The degradation is announced on stderr
/// whether or not the file could be moved aside — silently resetting e.g. the
/// user's viewed-run state looks like success.
fn quarantine_unreadable(path: &Path) {
    let stamp = quarantine_stamp();
    let mut target = path.as_os_str().to_owned();
    target.push(format!(".invalid-{stamp}"));
    let target = PathBuf::from(target);
    if !path.is_file() {
        return;
    }
    // Same-directory rename; fall back to copy+remove for exotic mounts
    // where rename cannot serve.
    let moved = std::fs::rename(path, &target).or_else(|_| {
        let copied = std::fs::copy(path, &target).and_then(|_| std::fs::remove_file(path));
        if copied.is_err() {
            // A half-done fallback is worse than none: the malformed bytes
            // stay at the canonical path *and* a `.invalid-<stamp>` copy is
            // left behind, so every later read quarantines again under a fresh
            // stamp and grows the directory without bound — the unbounded
            // pile-up this rename-first policy exists to prevent. Drop the
            // useless copy (a partial one when the copy itself failed) and
            // leave the original alone for the user to fix.
            let _ = std::fs::remove_file(&target);
        }
        copied
    });
    // Announced either way: a failed quarantine still degrades the registry to
    // its default for this invocation, and silently resetting e.g. the user's
    // viewed-run state looks like success.
    match moved {
        Ok(()) => note!(
            "pinvou: warning: quarantined malformed registry {} to {}",
            path.display(),
            target.display()
        ),
        Err(error) => note!(
            "pinvou: warning: malformed registry {} could not be quarantined ({error}); it is \
             ignored for this command — fix or remove the file manually",
            path.display()
        ),
    }
}

fn registry_tasks_mut<'a>(
    registry: &'a mut serde_json::Value,
    schema_version: u32,
) -> Result<&'a mut serde_json::Map<String, serde_json::Value>, CliError> {
    // A registry written by a newer app version must be refused, never
    // merged and written back (the GUI's VersionedJsonStore quarantines the
    // same situation); the version check only gates objects that carry one.
    if registry.is_object() {
        ensure_sidecar_schema(registry, schema_version as u64, "registry")?;
    }
    if !registry.is_object() {
        *registry = serde_json::json!({ "schema_version": schema_version, "tasks": {} });
    }
    let object = registry.as_object_mut().expect("registry is object");
    object
        .entry("schema_version")
        .or_insert_with(|| serde_json::json!(schema_version));
    // Defense in depth: read_registry quarantines a wrong-shaped `tasks`
    // value before this point, so the normalize is only reachable for
    // registries built in memory.
    if !object.get("tasks").is_some_and(Value::is_object) {
        object.insert("tasks".to_owned(), serde_json::json!({}));
    }
    Ok(object
        .get_mut("tasks")
        .and_then(Value::as_object_mut)
        .expect("tasks normalized to an object above"))
}

/// Current instant in the RFC3339 shape the foundation writes
/// (`to_rfc3339()` on chrono stamps renders fractional seconds when the
/// nanosecond part is non-zero; the previous hand-rolled renderer emitted
/// fixed milliseconds — both decode identically through every reader, so
/// the foundation's own renderer is the parity choice now).
fn now_string() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Quarantine stamp: sortable, separator-free version of the same instant.
fn quarantine_stamp() -> String {
    chrono::Utc::now().to_rfc3339().replace([':', '.'], "-")
}

/// Registry schema gate for the CLI-owned sidecar registries (schema
/// 2 read-state/history archive, schema 1 bindings/kinds/ui-metadata): a
/// registry written by a newer app version must be refused, never merged
/// and written back (the GUI's VersionedJsonStore quarantines the same
/// situation); a present-but-wrong-typed version would pass as 0 while the
/// GUI's typed deserialization fails on the file, so it is refused too.
fn ensure_sidecar_schema(
    registry: &serde_json::Value,
    supported: u64,
    what: &str,
) -> Result<(), CliError> {
    if !registry.is_object() {
        return Ok(());
    }
    match registry.get("schema_version") {
        None => Ok(()), // legacy files carry no version; every CLI writer adds one
        Some(version) => {
            let version = version.as_u64().ok_or_else(|| {
                CliError::failed(format!(
                    "scheduled_storage_unavailable: {what} schema_version is malformed \
                     (expected a number); fix or remove the file manually"
                ))
            })?;
            if version > supported {
                return Err(CliError::failed(format!(
                    "scheduled_storage_unavailable: {what} schema v{version} is newer than \
                     supported v{supported}; upgrade pinvou to edit it"
                )));
            }
            Ok(())
        }
    }
}

fn registry_tasks_view<'a>(
    registry: &'a serde_json::Value,
) -> &'a serde_json::Map<String, serde_json::Value> {
    static EMPTY: std::sync::OnceLock<serde_json::Map<String, serde_json::Value>> =
        std::sync::OnceLock::new();
    registry
        .get("tasks")
        .and_then(|value| value.as_object())
        .unwrap_or_else(|| EMPTY.get_or_init(serde_json::Map::new))
}

/// Random task/run id — the same UUIDv4 generator as the foundation's
/// automation manager (hex, hyphens, safe as a single path component).
fn new_storage_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ---- DTO mapping (mirrors ScheduledTaskDto / ScheduledRunDto, camelCase) ----

fn str_field<'a>(value: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(|value| value.as_str())
}

fn bool_field(value: &serde_json::Value, field: &str, default: bool) -> bool {
    value
        .get(field)
        .and_then(|value| value.as_bool())
        .unwrap_or(default)
}

fn status_label(value: &serde_json::Value) -> String {
    str_field(value, "status").unwrap_or("active").to_owned()
}

/// English schedule label for human output; JSON `scheduleLabel` carries the
/// same value (the GUI renders Chinese labels; the CLI is an English tool).
fn humanize_rrule(rrule: &str) -> String {
    let Ok(parts) = parse_rrule_pairs(rrule) else {
        return rrule.to_owned();
    };
    let value = |name: &str| {
        parts
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    match value("FREQ").as_deref() {
        Some("HOURLY") => {
            let interval = value("INTERVAL")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(1);
            let base = if interval == 1 {
                "hourly".to_owned()
            } else {
                format!("every {interval} hours")
            };
            let anchor = match (
                value("BYHOUR").and_then(|value| value.parse::<u32>().ok()),
                value("BYMINUTE").and_then(|value| value.parse::<u32>().ok()),
            ) {
                (Some(hour), minute) => format!(" from {hour:02}:{:02}", minute.unwrap_or(0)),
                (None, Some(minute)) => format!(" at minute {minute:02}"),
                (None, None) => String::new(),
            };
            with_days(value("BYDAY").as_deref(), &format!("{base}{anchor}"))
        }
        Some("WEEKLY") => {
            let hour = value("BYHOUR")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            let minute = value("BYMINUTE")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            with_days(value("BYDAY").as_deref(), &format!("{hour:02}:{minute:02}"))
        }
        _ => rrule.to_owned(),
    }
}

fn with_days(byday: Option<&str>, label: &str) -> String {
    let Some(byday) = byday else {
        return label.to_owned();
    };
    let days = parse_byday(byday);
    if days.is_empty() {
        label.to_owned()
    } else {
        let workdays = ["MO", "TU", "WE", "TH", "FR"];
        let days_label = if days == workdays {
            "workdays".to_owned()
        } else {
            days.join(",")
        };
        format!("{days_label} {label}")
    }
}

/// `store` is `None` when the sessions store could not be opened for
/// best-effort enrichment (see `open_sessions_for_enrichment`): the task is
/// then rendered without session-derived fields instead of failing.
fn unread_and_running(
    def: &serde_json::Value,
    runs: &[serde_json::Value],
    store: Option<&SessionStore>,
    read_state: &serde_json::Value,
) -> (bool, bool) {
    let task_id = str_field(def, "id").unwrap_or("");
    let is_running = runs
        .iter()
        .any(|run| matches!(str_field(run, "status").unwrap_or(""), "queued" | "running"));
    let viewed = viewed_runs(read_state, task_id);
    let has_unread = match store {
        Some(store) => runs.iter().any(|run| {
            str_field(run, "status") == Some("completed")
                && owned_session_id(run, task_id, store).is_some_and(|session_id| {
                    !store.is_hidden(&session_id)
                        && !viewed.contains(&str_field(run, "id").unwrap_or(""))
                })
        }),
        // Without the store the unread computation cannot tell which
        // completed runs have live conversations; report none rather than
        // failing the command.
        None => false,
    };
    (has_unread, is_running)
}

/// Mirrors `features::scheduled::tasks::owned_session_id`: the run's thread
/// is a CLI-visible scheduled-run session owned by this exact task.
fn owned_session_id(
    run: &serde_json::Value,
    task_id: &str,
    store: &SessionStore,
) -> Option<String> {
    let thread_id = str_field(run, "thread_id")?;
    store
        .scheduled_profile(thread_id)
        .filter(|profile| profile.task_id == task_id && store.scheduled_session_exists(thread_id))
        .map(|_| thread_id.to_owned())
}

fn viewed_runs<'a>(read_state: &'a serde_json::Value, task_id: &str) -> Vec<&'a str> {
    read_state
        .get("viewed_runs")
        .and_then(|value| value.get(task_id))
        .and_then(|value| value.as_array())
        .map(|runs| runs.iter().filter_map(|value| value.as_str()).collect())
        .unwrap_or_default()
}

fn model_id_for(
    bindings: &serde_json::Value,
    task_id: &str,
    model: Option<&str>,
) -> Option<String> {
    let binding = registry_tasks_view(bindings).get(task_id)?;
    let bound_model = str_field(binding, "model")?;
    if bound_model == model? {
        str_field(binding, "model_id").map(str::to_owned)
    } else {
        None
    }
}

fn kind_for(kinds: &serde_json::Value, task_id: &str) -> Option<String> {
    let entry = registry_tasks_view(kinds).get(task_id)?;
    match str_field(entry, "kind") {
        Some("memory_organize") => Some("memory_organize".to_owned()),
        _ => None,
    }
}

fn pinned_for(ui_metadata: &serde_json::Value, task_id: &str) -> (bool, Option<String>) {
    match registry_tasks_view(ui_metadata).get(task_id) {
        Some(entry) if bool_field(entry, "pinned", false) => {
            (true, str_field(entry, "pinned_at").map(str::to_owned))
        }
        _ => (false, None),
    }
}

fn map_task(
    def: &serde_json::Value,
    runs: &[serde_json::Value],
    store: Option<&SessionStore>,
    read_state: &serde_json::Value,
    bindings: &serde_json::Value,
    kinds: &serde_json::Value,
    ui_metadata: &serde_json::Value,
) -> serde_json::Value {
    let task_id = str_field(def, "id").unwrap_or("").to_owned();
    let model = str_field(def, "model");
    let (has_unread_runs, is_running) = unread_and_running(def, runs, store, read_state);
    let (pinned, pinned_at) = pinned_for(ui_metadata, &task_id);
    serde_json::json!({
        "id": task_id,
        "name": str_field(def, "name").unwrap_or(""),
        "prompt": str_field(def, "prompt").unwrap_or(""),
        "rrule": str_field(def, "rrule").unwrap_or(""),
        "scheduleLabel": humanize_rrule(str_field(def, "rrule").unwrap_or("")),
        "status": status_label(def),
        "nextRunAt": def.get("next_run_at").cloned().unwrap_or(serde_json::Value::Null),
        "lastRunAt": def.get("last_run_at").cloned().unwrap_or(serde_json::Value::Null),
        // Workspace selection is no longer a user-facing setting (GUI keeps
        // the DTO field empty).
        "cwds": [],
        "model": model.map(str::to_owned),
        "modelId": model_id_for(bindings, &task_id, model),
        "kind": kind_for(kinds, &task_id),
        "mode": def.get("mode").cloned().unwrap_or(serde_json::Value::Null),
        "allowShell": bool_field(def, "allow_shell", false),
        "trustMode": bool_field(def, "trust_mode", false),
        "autoApprove": bool_field(def, "auto_approve", true),
        "hasUnreadRuns": has_unread_runs,
        "isRunning": is_running,
        "pinned": pinned,
        "pinnedAt": pinned_at,
    })
}

fn session_titles(store: &SessionStore) -> std::collections::HashMap<String, String> {
    store
        .list_scheduled()
        .map(|sessions| {
            sessions
                .into_iter()
                .map(|metadata| (metadata.id, metadata.title))
                .collect()
        })
        .unwrap_or_default()
}

/// `store` is optional for the same reason `map_task` takes an
/// `Option<&SessionStore>`: a command that already committed its write
/// enriches best-effort (`open_sessions_for_enrichment`), and without the
/// store there is simply no session to attribute — every session-derived
/// field degrades to its absent value rather than failing the command.
fn map_run(
    run: &serde_json::Value,
    store: Option<&SessionStore>,
    titles: &std::collections::HashMap<String, String>,
    read_state: &serde_json::Value,
    task_name: Option<&str>,
    task_model: Option<&str>,
) -> serde_json::Value {
    let task_id = str_field(run, "automation_id").unwrap_or("").to_owned();
    let session_id =
        store.and_then(|store| owned_session_id_from_snapshot(run, &task_id, store, titles));
    let session_title = session_id
        .as_deref()
        .and_then(|id| titles.get(id))
        .filter(|title| *title != "Scheduled run")
        .cloned();
    // The owned session paired with the store that vouched for it. Without a
    // store there is no session id either, so every session-derived field
    // below reads as absent instead of guessing.
    let session = session_id.as_deref().zip(store);
    let archived = session.is_some_and(|(id, store)| store.is_hidden(id));
    let unread = str_field(run, "status") == Some("completed")
        && session_id.is_some()
        && !archived
        && !viewed_runs(read_state, &task_id).contains(&str_field(run, "id").unwrap_or(""));
    serde_json::json!({
        "id": str_field(run, "id").unwrap_or(""),
        "automationId": task_id,
        "sessionId": session_id,
        "scheduledFor": str_field(run, "scheduled_for").unwrap_or(""),
        "status": str_field(run, "status").unwrap_or(""),
        "createdAt": str_field(run, "created_at").unwrap_or(""),
        "startedAt": run.get("started_at").cloned().unwrap_or(serde_json::Value::Null),
        "endedAt": run.get("ended_at").cloned().unwrap_or(serde_json::Value::Null),
        "taskId": run.get("task_id").cloned().unwrap_or(serde_json::Value::Null),
        "threadId": run.get("thread_id").cloned().unwrap_or(serde_json::Value::Null),
        "turnId": run.get("turn_id").cloned().unwrap_or(serde_json::Value::Null),
        "error": run.get("error").cloned().unwrap_or(serde_json::Value::Null),
        "unread": unread,
        "sessionTitle": session_title,
        "pinned": session.is_some_and(|(id, store)| store.is_pinned(id)),
        "pinnedAt": session.and_then(|(id, store)| store.pinned_at(id)),
        "archived": archived,
        "taskName": task_name.map(str::to_owned),
        "taskModel": task_model.map(str::to_owned),
    })
}

/// Snapshot variant of `owned_session_id` (the GUI's
/// `owned_session_id_from_snapshot`): ownership is checked against the
/// precomputed session-title map instead of a live existence probe.
fn owned_session_id_from_snapshot(
    run: &serde_json::Value,
    task_id: &str,
    store: &SessionStore,
    titles: &std::collections::HashMap<String, String>,
) -> Option<String> {
    let thread_id = str_field(run, "thread_id")?;
    store
        .scheduled_profile(thread_id)
        .filter(|profile| profile.task_id == task_id && titles.contains_key(thread_id))
        .map(|_| thread_id.to_owned())
}

fn render_runs(
    store: &SessionStore,
    runs: &[serde_json::Value],
    read_state: &serde_json::Value,
    task_names: &std::collections::HashMap<String, (String, Option<String>)>,
) -> (Vec<String>, Vec<serde_json::Value>) {
    let titles = session_titles(store);
    let mut lines = Vec::new();
    let values = runs
        .iter()
        .map(|run| {
            let automation_id = str_field(run, "automation_id").unwrap_or("");
            let (task_name, task_model) = task_names
                .get(automation_id)
                .map(|(name, model)| (Some(name.as_str()), model.as_deref()))
                .unwrap_or((None, None));
            let value = map_run(run, Some(store), &titles, read_state, task_name, task_model);
            lines.push(format!(
                "{}\t{}\t{}\t{}\t{}",
                value["id"].as_str().unwrap_or(""),
                value["status"].as_str().unwrap_or(""),
                value["scheduledFor"].as_str().unwrap_or(""),
                value["sessionId"].as_str().unwrap_or("-"),
                value["error"].as_str().unwrap_or("-"),
            ));
            value
        })
        .collect::<Vec<_>>();
    (lines, values)
}

/// Task name/model lookup used to enrich run DTOs, mirroring the GUI's
/// `list_scheduled_task_runs` / `list_scheduled_runs` attachments.
fn task_name_map(
    defs: &[serde_json::Value],
) -> std::collections::HashMap<String, (String, Option<String>)> {
    defs.iter()
        .filter_map(|def| {
            let id = str_field(def, "id")?;
            Some((
                id.to_owned(),
                (
                    str_field(def, "name").unwrap_or("").to_owned(),
                    str_field(def, "model").map(str::to_owned),
                ),
            ))
        })
        .collect()
}

// ---- command execution ----

pub fn execute(command: ScheduledCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Hold the cross-process store write lock across the whole mutating
    // command (see `scheduled_store_lock`); read-only commands skip it.
    // `run` holds it across the entire headless host execution (which can
    // take minutes) by design: concurrent `scheduled run` invocations must
    // serialize CLI×CLI so their store writes and run records stay ordered.
    let mutating = matches!(
        command,
        ScheduledCommand::Create { .. }
            | ScheduledCommand::Update { .. }
            | ScheduledCommand::Pause { .. }
            | ScheduledCommand::Resume { .. }
            | ScheduledCommand::Pin { .. }
            | ScheduledCommand::Unpin { .. }
            | ScheduledCommand::Delete { .. }
            | ScheduledCommand::Run { .. }
            | ScheduledCommand::MarkViewed { .. }
    );
    let mut store_lock = if mutating {
        Some(scheduled_store_lock()?)
    } else {
        None
    };
    let _store_write_guard = match store_lock.as_mut() {
        Some(lock) => Some(lock.write().map_err(|error| {
            CliError::failed(format!(
                "scheduled_storage_unavailable: cannot acquire the store write lock: {error}"
            ))
        })?),
        None => None,
    };
    match command {
        ScheduledCommand::List => list(output),
        ScheduledCommand::Show { id } => show(&id, output),
        ScheduledCommand::Create {
            name,
            prompt_file,
            rrule,
            kind,
            model_id,
            mode,
            paused,
        } => create(
            &name,
            &prompt_file,
            &rrule,
            kind,
            model_id,
            mode,
            paused,
            output,
        ),
        ScheduledCommand::Update {
            id,
            name,
            prompt_file,
            rrule,
            model_id,
        } => update(&id, name, prompt_file, rrule, model_id, output),
        ScheduledCommand::Pause { id } => pause_or_resume(&id, true, output),
        ScheduledCommand::Resume { id } => pause_or_resume(&id, false, output),
        ScheduledCommand::Pin { id } => set_pinned(&id, true, output),
        ScheduledCommand::Unpin { id } => set_pinned(&id, false, output),
        ScheduledCommand::Delete { id, yes } => delete(&id, yes, output),
        ScheduledCommand::Run { id } => run(&id, output),
        ScheduledCommand::Runs { id, limit } => runs(&id, limit, output),
        ScheduledCommand::RunsAll { limit } => runs_all(limit, output),
        ScheduledCommand::MarkViewed { task_id, run_id } => mark_viewed(&task_id, &run_id, output),
        ScheduledCommand::ChatPrompt => chat_prompt(output),
    }
}

fn list(output: OutputMode) -> Result<CliOutcome, CliError> {
    let store_holder = TaskStore::new()?;
    let sessions = open_sessions()?;
    let read_state = read_registry(&store_holder.read_state_path(), &["viewed_runs"]);
    let bindings = read_registry(&store_holder.model_bindings_path(), &["tasks"]);
    let kinds = read_registry(&store_holder.task_kinds_path(), &["tasks"]);
    let ui_metadata = read_registry(&store_holder.ui_metadata_path(), &["tasks"]);
    let defs = store_holder.list_defs()?;
    let mut lines = Vec::new();
    let tasks = defs
        .iter()
        .map(|def| {
            let task_id = str_field(def, "id").unwrap_or("").to_owned();
            let runs = store_holder.list_runs(&task_id, None)?;
            let value = map_task(
                def,
                &runs,
                Some(&sessions),
                &read_state,
                &bindings,
                &kinds,
                &ui_metadata,
            );
            lines.push(format!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                value["id"].as_str().unwrap_or(""),
                value["status"].as_str().unwrap_or(""),
                value["nextRunAt"].as_str().unwrap_or("-"),
                value["kind"].as_str().unwrap_or("chat"),
                if value["pinned"].as_bool().unwrap_or(false) {
                    "pinned"
                } else {
                    "-"
                },
                value["name"].as_str().unwrap_or(""),
            ));
            Ok(value)
        })
        .collect::<Result<Vec<_>, CliError>>()?;
    let value = serde_json::json!({ "tasks": tasks });
    let human = if lines.is_empty() {
        "No scheduled tasks.".to_owned()
    } else {
        lines.join("\n")
    };
    Ok(success(render(output, human, &value)))
}

fn show(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store_holder = TaskStore::new()?;
    let def = store_holder.read_def(id)?;
    let sessions = open_sessions()?;
    let read_state = read_registry(&store_holder.read_state_path(), &["viewed_runs"]);
    let bindings = read_registry(&store_holder.model_bindings_path(), &["tasks"]);
    let kinds = read_registry(&store_holder.task_kinds_path(), &["tasks"]);
    let ui_metadata = read_registry(&store_holder.ui_metadata_path(), &["tasks"]);
    let runs = store_holder.list_runs(id, None)?;
    let task = map_task(
        &def,
        &runs,
        Some(&sessions),
        &read_state,
        &bindings,
        &kinds,
        &ui_metadata,
    );
    let names = task_name_map(std::slice::from_ref(&def));
    let (run_lines, run_values) = render_runs(&sessions, &runs, &read_state, &names);
    let mut lines = vec![
        format!("id: {}", task["id"].as_str().unwrap_or("")),
        format!("name: {}", task["name"].as_str().unwrap_or("")),
        format!("status: {}", task["status"].as_str().unwrap_or("")),
        format!("schedule: {}", task["scheduleLabel"].as_str().unwrap_or("")),
        format!("rrule: {}", task["rrule"].as_str().unwrap_or("")),
        format!("kind: {}", task["kind"].as_str().unwrap_or("chat")),
        format!("model: {}", task["model"].as_str().unwrap_or("-")),
        format!("next_run: {}", task["nextRunAt"].as_str().unwrap_or("-")),
        format!("last_run: {}", task["lastRunAt"].as_str().unwrap_or("-")),
        format!("runs: {}", run_values.len()),
    ];
    lines.extend(run_lines);
    let mut value = task.clone();
    value["runs"] = serde_json::Value::Array(run_values);
    Ok(success(render(output, lines.join("\n"), &value)))
}

/// Resolves the model name a new task runs with, mirroring
/// `features::scheduled::tasks::current_automation_model`: the active saved
/// model's wire name, else the shared `default-model` fallback.
fn default_automation_model() -> String {
    UserPrefs::load()
        .active_model()
        .map(|model| model.model.clone())
        .unwrap_or_else(|| "default-model".to_owned())
}

/// Mirrors `Pinvou3Bridge::allow_shell_for_prefs` (crate-private in
/// `pinvou3_lib`): env > prefs.advanced > default true.
fn current_allow_shell() -> bool {
    if let Ok(value) = std::env::var("PINVOU3_ALLOW_SHELL") {
        return matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );
    }
    UserPrefs::load().advanced.allow_shell.unwrap_or(true)
}

fn create(
    name: &str,
    prompt_file: &Path,
    rrule: &str,
    kind: TaskKind,
    model_id: Option<String>,
    mode: Option<TaskMode>,
    paused: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let store_holder = TaskStore::new()?;
    let name = name.trim();
    if name.is_empty() {
        return Err(CliError::usage(
            "scheduled create requires a non-empty --name",
        ));
    }
    // Same 4 MiB cap as the agent prompt read: `--prompt-file /dev/zero`
    // must fail cleanly instead of reading forever.
    let prompt = crate::support::read_text_file_capped(
        &prompt_file,
        4 * 1024 * 1024,
        "scheduled create --prompt-file",
    )?;
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(CliError::usage(
            "scheduled create requires a non-empty prompt file",
        ));
    }
    // The model wire name and the saved-model pin are resolved as ONE pair,
    // exactly like the GUI's create (`model: selected.model, modelId:
    // selected.id`): the executor's `resolve_scheduled_model` rejects a task
    // whose binding's wire name does not match the id's current wire name,
    // so pinning X while persisting the ACTIVE model's wire name (the old
    // CLI behavior) produced a task that fails on every run. With
    // `--model-id` the definition carries the named model's own wire name;
    // without it, the active model's (the GUI's `current_automation_model`).
    // An unknown `--model-id` is refused before anything is persisted: it
    // names a saved model that does not exist, the same state-dependent
    // refusal class the family's conventions dictate (exit 1, like `models
    // ... model not found:`).
    let (model, validated_model_id) = match model_id.as_deref().map(str::trim) {
        Some(raw) if !raw.is_empty() => {
            let prefs = UserPrefs::load();
            let selected = prefs
                .model_by_id(raw)
                .ok_or_else(|| CliError::failed(format!("model not found: {raw}")))?;
            (selected.model.clone(), Some(raw.to_owned()))
        }
        _ => (default_automation_model(), None),
    };
    if kind == TaskKind::MemoryOrganize && !memory_feature::memory_enabled() {
        return Err(CliError::failed(
            "scheduled_memory_organize_disabled: memory organize tasks require memory to be \
enabled in settings",
        ));
    }
    // Foundation create: same id allocation, normalization and serialization
    // as the GUI, and — the reason this replaces the hand-rolled writer —
    // an ACTIVE record gets its `next_run_at` resolved EAGERLY
    // (`schedule.next_after_with_anchor(now, now)`). The CLI used to persist
    // `next_run_at: null` and rely on the app's sweep to fill it in; for a
    // one-shot whose AT had passed by the time the app opened, the sweep's
    // resolution fails with "no future run" and PAUSES the task — a
    // CLI-created one-shot could silently never run. A `paused` create keeps
    // `next_run_at` unset, matching the foundation for paused records.
    let created = store_holder
        .manager()?
        .create_automation(CreateAutomationRequest {
            name: name.to_owned(),
            prompt: prompt.to_owned(),
            rrule: rrule.to_owned(),
            cwds: Vec::new(),
            model: Some(model.clone()),
            // Pinvou's exact saved-model id is persisted in
            // model-bindings.json after the id exists; the CodeWhale
            // provider fields address a different registry and are not
            // interchangeable with that id (same as the GUI's request).
            model_provider: None,
            model_provider_id: None,
            mode: Some(mode.unwrap_or_default().persisted().to_owned()),
            allow_shell: Some(current_allow_shell()),
            trust_mode: Some(true),
            // 不可绕过的审批（rlm_eval/hook ask）仍由 force_prompt 拦截。
            auto_approve: Some(true),
            delivery_mode: None,
            status: Some(if paused {
                AutomationStatus::Paused
            } else {
                AutomationStatus::Active
            }),
        })
        .map_err(|error| {
            CliError::failed(format!(
                "scheduled_create_failed: cannot create the scheduled task: {error:#}"
            ))
        })?;
    let id = created.id.clone();
    // The workspace is allocated from the automation id exactly like the GUI
    // (`ensure_automation_workspace`); clients cannot provide a path.
    let workspace = store_holder.workspace_dir(&id);
    std::fs::create_dir_all(&workspace).map_err(|error| {
        CliError::failed(format!(
            "scheduled_workspace_unavailable: cannot create {}: {error}",
            workspace.display()
        ))
    })?;
    let def = if created.cwds.first().is_some_and(|cwd| cwd == &workspace) {
        def_to_value(&created)
    } else {
        let mut def = def_to_value(&created);
        def["cwds"] = serde_json::json!([workspace.display().to_string()]);
        store_holder.write_def(&def)?;
        def
    };
    // Only touch the shared bindings sidecar when a binding was actually
    // requested: an unconditional write widens the last-writer-wins window
    // against a concurrently persisting GUI and turns a sidecar-write
    // failure into a failed create where the GUI would succeed. The pair
    // written matches the definition's model by construction (resolved
    // together above), so the executor accepts it.
    if validated_model_id.as_deref().is_some() {
        if let Err(error) = write_model_binding(&store_holder, &id, validated_model_id.as_deref()) {
            // Roll back the just-created task so no kind-less/binding-less task
            // lingers, mirroring the GUI create rollback.
            if let Ok(path) = store_holder.def_path(&id) {
                let _ = std::fs::remove_file(path);
            }
            let _ = std::fs::remove_dir_all(store_holder.workspace_dir(&id));
            return Err(error);
        }
    }
    if let Some(stored_kind) = kind.stored_kind() {
        if let Err(error) = persist_task_kind(&store_holder, &id, Some(stored_kind)) {
            if let Ok(path) = store_holder.def_path(&id) {
                let _ = std::fs::remove_file(path);
            }
            let _ = std::fs::remove_dir_all(store_holder.workspace_dir(&id));
            // A binding written above must not outlive the rolled-back task:
            // clear it so no binding for a nonexistent id lingers in the
            // shared registry.
            if validated_model_id.as_deref().is_some() {
                let _ = write_model_binding(&store_holder, &id, None);
            }
            return Err(error);
        }
    }
    // The definition (and its sidecar records) are committed above; session
    // enrichment is best-effort so a sessions-store failure cannot misreport
    // the committed create as a failure (same rationale as the mutating
    // commands).
    let sessions = open_sessions_for_enrichment();
    let value = map_task(
        &def,
        &[],
        sessions.as_ref(),
        &serde_json::Value::Null,
        &read_registry(&store_holder.model_bindings_path(), &["tasks"]),
        &read_registry(&store_holder.task_kinds_path(), &["tasks"]),
        &read_registry(&store_holder.ui_metadata_path(), &["tasks"]),
    );
    Ok(success(render(
        output,
        format!("Created scheduled task: {id}"),
        &value,
    )))
}

fn update(
    id: &str,
    name: Option<String>,
    prompt_file: Option<PathBuf>,
    rrule: Option<String>,
    model_id: Option<String>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let store_holder = TaskStore::new()?;
    // The foundation update reads the current record itself; this pre-read
    // keeps the CLI's unknown-id contract stable (scheduled_task_not_found
    // for a missing task, before any sidecar is touched).
    store_holder.read_def(id)?;
    // Same pair-resolution rule as create (see there): `--model-id` X
    // re-binds the pin to X AND the definition to X's wire name — the GUI
    // sends `model: selected.model, modelId: selected.id`, and the executor
    // rejects any other combination. Unknown ids are refused before any
    // write. (Changing the model wire name without an id is not offered:
    // the GUI's update applies input.model, and the CLI has no flag that
    // names a bare wire model — the docs say to edit the model in the GUI
    // or recreate the task.)
    let mut model_update: Option<String> = None;
    let mut validated_model_id: Option<String> = None;
    match model_id.as_deref().map(str::trim) {
        Some(raw) if !raw.is_empty() => {
            let prefs = UserPrefs::load();
            let selected = prefs
                .model_by_id(raw)
                .ok_or_else(|| CliError::failed(format!("model not found: {raw}")))?;
            model_update = Some(selected.model.clone());
            validated_model_id = Some(raw.to_owned());
        }
        _ => {}
    }
    let mut request = UpdateAutomationRequest::default();
    if let Some(name) = name {
        let name = name.trim();
        if name.is_empty() {
            return Err(CliError::usage(
                "scheduled update requires a non-empty --name",
            ));
        }
        request.name = Some(name.to_owned());
    }
    if let Some(prompt_file) = prompt_file {
        let prompt = crate::support::read_text_file_capped(
            &prompt_file,
            4 * 1024 * 1024,
            "scheduled update --prompt-file",
        )?;
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(CliError::usage(
                "scheduled update requires a non-empty prompt file",
            ));
        }
        request.prompt = Some(prompt.to_owned());
    }
    if let Some(rrule) = rrule {
        request.rrule = Some(rrule.to_owned());
    }
    if let Some(model) = model_update.clone() {
        request.model = Some(model);
    }
    let updated = store_holder
        .manager()?
        .update_automation(id, request)
        .map_err(|error| {
            CliError::failed(format!(
                "scheduled_update_failed: cannot update scheduled task {id}: {error:#}"
            ))
        })?;
    // `update_automation` recomputed next_run_at for the final status when
    // the schedule or status changed (paused keeps it unset, active resolves
    // the slot eagerly — including the past-one-shot failure, mirroring the
    // GUI's own update refusal for a one-shot with no future run). Workspace
    // pinning follows the GUI's ensure_automation_workspace, including its
    // persistence of a repaired cwd.
    let mut def = def_to_value(&updated);
    ensure_workspace(&store_holder, &mut def)?;
    if validated_model_id.is_some() {
        write_model_binding(&store_holder, id, validated_model_id.as_deref())?;
    }
    // Enrichment is best-effort: the update is committed above, so a
    // sessions store boot failure must not report the update as failed.
    let sessions = open_sessions_for_enrichment();
    let runs = store_holder.list_runs(id, None)?;
    let value = map_task(
        &def,
        &runs,
        sessions.as_ref(),
        &read_registry(&store_holder.read_state_path(), &["viewed_runs"]),
        &read_registry(&store_holder.model_bindings_path(), &["tasks"]),
        &read_registry(&store_holder.task_kinds_path(), &["tasks"]),
        &read_registry(&store_holder.ui_metadata_path(), &["tasks"]),
    );
    Ok(success(render(
        output,
        format!("Updated scheduled task: {id}"),
        &value,
    )))
}

/// Mirrors `ensure_automation_workspace`: the durable execution workspace is
/// derived from the task id and persisted as the single cwd entry — including
/// the persistence half (the GUI writes the repair through
/// `update_automation(cwds: …)`): mutating only the in-memory def would let
/// the executor run with a different cwd than this command reported. A def
/// that already pins the workspace is left untouched, so the repair is
/// idempotent and writes only when the content actually changed.
fn ensure_workspace(store_holder: &TaskStore, def: &mut serde_json::Value) -> Result<(), CliError> {
    let id = str_field(def, "id").unwrap_or("").to_owned();
    // `require_object_definition` already rejected non-component ids at the
    // read gate; this join therefore only ever sees validated ids.
    let workspace = store_holder.workspace_dir(&id);
    std::fs::create_dir_all(&workspace).map_err(|error| {
        CliError::failed(format!(
            "scheduled_workspace_unavailable: cannot create {}: {error}",
            workspace.display()
        ))
    })?;
    let already_pinned = def
        .get("cwds")
        .and_then(Value::as_array)
        .is_some_and(|cwds| {
            cwds.len() == 1
                && cwds
                    .first()
                    .and_then(Value::as_str)
                    .is_some_and(|cwd| Path::new(cwd) == workspace)
        });
    if already_pinned {
        return Ok(());
    }
    def["cwds"] = serde_json::json!([workspace.display().to_string()]);
    store_holder.write_def(def)
}

/// Persist one (model_id, wire model) pair in the bindings sidecar. The
/// pair is the executor's acceptance contract (`resolve_scheduled_model`
/// rejects a task whose bound wire model diverges from the saved model id's
/// current wire name), so callers resolve BOTH halves together from the same
/// saved model before calling (see `create`/`update`); the wire name is
/// re-read from the definition only as a consistency fallback.
fn write_model_binding(
    store_holder: &TaskStore,
    id: &str,
    model_id: Option<&str>,
) -> Result<(), CliError> {
    let model_id = model_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let mut registry = read_registry(&store_holder.model_bindings_path(), &["tasks"]);
    let tasks = registry_tasks_mut(&mut registry, 1)?;
    match model_id {
        Some(model_id) => {
            let model = store_holder
                .read_def(id)?
                .get("model")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned();
            tasks.insert(
                id.to_owned(),
                serde_json::json!({
                    "model_id": model_id,
                    "model": model,
                    "updated_at": now_string(),
                }),
            );
        }
        // The GUI clears a binding when model id and model are not both
        // present; a bare --model-id always carries both here.
        None => {
            tasks.remove(id);
        }
    }
    write_json_atomic(&store_holder.model_bindings_path(), &registry)
}

fn persist_task_kind(
    store_holder: &TaskStore,
    id: &str,
    kind: Option<&str>,
) -> Result<(), CliError> {
    let mut registry = read_registry(&store_holder.task_kinds_path(), &["tasks"]);
    let tasks = registry_tasks_mut(&mut registry, 1)?;
    match kind {
        Some(kind) => {
            tasks.insert(
                id.to_owned(),
                serde_json::json!({ "kind": kind, "updated_at": now_string() }),
            );
        }
        None => {
            tasks.remove(id);
        }
    }
    write_json_atomic(&store_holder.task_kinds_path(), &registry)
}

fn pause_or_resume(id: &str, pause: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store_holder = TaskStore::new()?;
    // Same unknown-id pre-gate as update/show/runs/pin: the foundation
    // pause/resume would surface a missing task as `scheduled_update_failed`
    // wrapping the raw read error, so read the definition first and answer
    // with the family's stable `scheduled_task_not_found`.
    store_holder.read_def(id)?;
    let manager = store_holder.manager()?;
    // Foundation pause/resume: `update_automation` under the hood, which
    // recomputes `next_run_at` for the FINAL status — resume resolves the
    // next slot eagerly (`next_after_with_anchor(now, created_at)`), so a
    // resumed one-shot whose AT already passed fails HERE with "no future
    // run" instead of being paused by the first sweep and silently never
    // running (the round-18 blocker). Pause clears the slot so nothing
    // fires.
    let updated = if pause {
        manager.pause_automation(id)
    } else {
        manager.resume_automation(id)
    }
    .map_err(|error| {
        CliError::failed(format!(
            "scheduled_update_failed: cannot {} scheduled task {id}: {error:#}",
            if pause { "pause" } else { "resume" }
        ))
    })?;
    let mut def = def_to_value(&updated);
    let action = if pause { "paused" } else { "resumed" };
    if !pause {
        // Same GUI step as create/update: the durable workspace stays pinned
        // to the id-derived path, and a missing/empty stored cwd is
        // persisted like the GUI's `update_automation(cwds: …)` repair.
        ensure_workspace(&store_holder, &mut def)?;
    }
    // Enrichment is best-effort: the status flip is committed above, so a
    // sessions store boot failure must not report the command as failed.
    let sessions = open_sessions_for_enrichment();
    let runs = store_holder.list_runs(id, None)?;
    let value = map_task(
        &def,
        &runs,
        sessions.as_ref(),
        &read_registry(&store_holder.read_state_path(), &["viewed_runs"]),
        &read_registry(&store_holder.model_bindings_path(), &["tasks"]),
        &read_registry(&store_holder.task_kinds_path(), &["tasks"]),
        &read_registry(&store_holder.ui_metadata_path(), &["tasks"]),
    );
    Ok(success(render(
        output,
        format!("{action} scheduled task: {id}"),
        &value,
    )))
}

fn set_pinned(id: &str, pinned: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store_holder = TaskStore::new()?;
    // Same existence gate as the GUI: pin state never lingers for a deleted
    // task.
    store_holder.read_def(id)?;
    let mut registry = read_registry(&store_holder.ui_metadata_path(), &["tasks"]);
    let tasks = registry_tasks_mut(&mut registry, 1)?;
    if pinned {
        let now = now_string();
        tasks.insert(
            id.to_owned(),
            serde_json::json!({ "pinned": true, "pinned_at": now, "updated_at": now }),
        );
    } else {
        tasks.remove(id);
    }
    write_json_atomic(&store_holder.ui_metadata_path(), &registry)?;
    let action = if pinned { "pinned" } else { "unpinned" };
    let value = serde_json::json!({ "id": id, "action": action });
    Ok(success(render(
        output,
        format!("{action} scheduled task: {id}"),
        &value,
    )))
}

fn delete(id: &str, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    let store_holder = TaskStore::new()?;
    let mut def = store_holder.read_def(id)?;
    // Pause first, exactly like the GUI's destructive sequence: a concurrent
    // GUI scheduler tick must not enqueue a run between the active-run check
    // below and the removal. The pause lives in `status` — the field the
    // foundation sweep actually reads; a `paused` bool would be silently
    // ignored by serde and pause nothing.
    let previous_status = def
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("active")
        .to_owned();
    // Match the GUI pause shape exactly: a GUI pause always clears the next
    // slot, so the provisional pause must not leave a `next_run_at` no GUI
    // pause would ever produce. The original value is restored on every
    // blocked/failed path below, together with the status.
    let previous_next_run_at = def.get("next_run_at").cloned();
    def["status"] = Value::String("paused".into());
    def["next_run_at"] = Value::Null;
    store_holder.write_def(&def)?;
    // Every failure below restores the pre-delete status like the GUI
    // (`restore_task_status_if_present` runs on every failed delete): a
    // caller retrying after fixing the cause must not find the task paused.
    let restore_status = |def: &Value, status: &str| {
        let mut restored = def.clone();
        restored["status"] = Value::String(status.to_owned());
        match &previous_next_run_at {
            Some(value) => restored["next_run_at"] = value.clone(),
            None => {
                if let Some(object) = restored.as_object_mut() {
                    object.remove("next_run_at");
                }
            }
        }
        let _ = store_holder.write_def(&restored);
    };
    // list_runs can fail on a corrupt/unsupported run record; that failure
    // is a blocked delete like any other, so it must restore the pre-delete
    // status too instead of leaving the task provisionally paused.
    let runs = match store_holder.list_runs(id, None) {
        Ok(runs) => runs,
        Err(error) => {
            restore_status(&def, &previous_status);
            return Err(error);
        }
    };
    // The GUI cancels queued/running runs through the foundation TaskManager
    // before deleting; headlessly there is no engine runtime to cancel with,
    // so GUI-runtime-owned active runs refuse deletion. A `queued` record
    // with no task id is CLI-created bookkeeping (a CLI process killed
    // mid-run) that the GUI cannot cancel either — it must not wedge the
    // task forever, so it does not block deletion.
    if let Some(active) = runs.iter().find(|run| {
        // Records with no task id are CLI bookkeeping no runtime can cancel;
        // only GUI-runtime-owned active runs block deletion.
        !str_field(run, "task_id").unwrap_or("").is_empty()
            && matches!(str_field(run, "status").unwrap_or(""), "queued" | "running")
    }) {
        // Mirror the GUI's restore-on-blocked path: the task stays exactly
        // as it was, paused only for the duration of this check.
        restore_status(&def, &previous_status);
        return Err(CliError::failed(format!(
            "scheduled_delete_blocked: run {} is {} for task {id}; wait for it to finish \
(only the GUI runtime can cancel a scheduled run)",
            str_field(active, "id").unwrap_or(""),
            str_field(active, "status").unwrap_or(""),
        )));
    }
    // Archive first (commit before removal), then delete the definition and
    // its runs, then drop the sidecar entries — the GUI delete order minus
    // the engine-task cancellation step.
    let mut archive = read_registry(&store_holder.history_archive_path(), &["tasks"]);
    // Same newer-schema refusal as registry_tasks_mut: an archive written by
    // a newer app version must not be merged and written back. The refusal
    // restores the provisional pause like every other blocked path — a
    // caller retrying after upgrading must not find the task paused.
    if archive.is_object() {
        if let Err(error) = ensure_sidecar_schema(&archive, 2, "history archive") {
            restore_status(&def, &previous_status);
            return Err(error);
        }
    }
    if !archive.is_object() {
        archive = serde_json::json!({ "schema_version": 2, "tasks": {} });
    }
    // Missing or wrong-shaped "tasks" normalizes to an empty object so the
    // snapshot below is never silently dropped (run history would be lost
    // once the definition and runs are removed).
    if !archive.get("tasks").is_some_and(Value::is_object) {
        archive["tasks"] = serde_json::Value::Object(serde_json::Map::new());
    }
    let snapshot = serde_json::json!({
        "schema_version": 2,
        "tasks": {
            id: {
                "task": {
                    "id": str_field(&def, "id").unwrap_or(id),
                    "name": str_field(&def, "name").unwrap_or(""),
                    "model": def.get("model").cloned().unwrap_or(serde_json::Value::Null),
                },
                "runs": runs,
                "deleted_at": now_string(),
            }
        },
    });
    let existing_snapshot = archive
        .get("tasks")
        .and_then(|value| value.as_object())
        .expect("tasks normalized to an object above")
        .clone();
    let existing = &existing_snapshot;
    {
        let mut merged = existing.clone();
        if let Some(new_tasks) = snapshot["tasks"].as_object() {
            for (key, value) in new_tasks {
                merged.insert(key.clone(), value.clone());
            }
        }
        archive["tasks"] = serde_json::Value::Object(merged);
    }
    if let Err(error) = write_json_atomic(&store_holder.history_archive_path(), &archive) {
        restore_status(&def, &previous_status);
        return Err(error);
    }
    // If the removal below fails, roll the archive entry back so the task is
    // not left in the GUI's "live + archived" mixed state (mirror of the GUI
    // delete's archive rollback).
    let archive_rollback = {
        let mut rolled = archive.clone();
        rolled["tasks"] = serde_json::Value::Object(existing_snapshot);
        rolled
    };
    let def_path = match store_holder.def_path(id) {
        Ok(path) => path,
        Err(error) => {
            let _ = write_json_atomic(&store_holder.history_archive_path(), &archive_rollback);
            restore_status(&def, &previous_status);
            return Err(error);
        }
    };
    match std::fs::remove_file(&def_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            let _ = write_json_atomic(&store_holder.history_archive_path(), &archive_rollback);
            restore_status(&def, &previous_status);
            return Err(CliError::failed(format!(
                "scheduled_delete_failed: cannot remove {}: {error}",
                def_path.display()
            )));
        }
    }
    let runs_dir = store_holder.runs_dir_for(id)?;
    if runs_dir.exists() {
        if let Err(error) = std::fs::remove_dir_all(&runs_dir) {
            // The definition file is already removed — the delete is
            // committed, exactly like the GUI, whose
            // `restore_task_status_if_present` refuses to recreate a missing
            // definition ("a missing definition is a committed delete") and
            // whose own delete logs a warning and keeps the durable archive
            // snapshot. Restoring here would resurrect a live task whose
            // runs are gone AND leave it in the history archive, so the next
            // delete would overwrite the only history snapshot with an empty
            // run list.
            note!(
                "pinvou: warning: scheduled task {id} was deleted, but its run directory \
                 could not be removed: {error}; the history archive snapshot is kept"
            );
        }
    }
    for path in [
        store_holder.model_bindings_path(),
        store_holder.task_kinds_path(),
        store_holder.ui_metadata_path(),
    ] {
        // All three are `tasks`-keyed registries.
        let mut registry = read_registry(&path, &["tasks"]);
        if registry.is_null() {
            continue;
        }
        // Same newer-schema refusal as the write paths — but non-fatal: the
        // delete is already committed above, so a registry this CLI must not
        // rewrite only skips its cleanup (the GUI compaction drops the stale
        // entry); failing the whole delete here would strand a live-less
        // task with sidecar entries and no definition.
        if registry.is_object()
            && registry
                .get("schema_version")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 1
        {
            note!(
                "pinvou: warning: scheduled task {id} was deleted, but a sidecar registry has \
                 a newer schema; its stale entry is left for the desktop app to clean up"
            );
            continue;
        }
        if let Some(tasks) = registry
            .get_mut("tasks")
            .and_then(|value| value.as_object_mut())
        {
            if tasks.remove(id).is_some() {
                let _ = write_json_atomic(&path, &registry);
            }
        }
    }
    // Enrichment is best-effort: the delete is committed above, so a
    // sessions store boot failure must not report the delete as failed.
    let sessions = open_sessions_for_enrichment();
    let task = map_task(
        &def,
        &runs,
        sessions.as_ref(),
        &serde_json::Value::Null,
        &serde_json::Value::Null,
        &serde_json::Value::Null,
        &serde_json::Value::Null,
    );
    let mut value = task;
    value["deletedSessionIds"] = serde_json::json!([]);
    Ok(success(render(
        output,
        format!("Deleted scheduled task: {id}"),
        &value,
    )))
}

fn run(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store_holder = TaskStore::new()?;
    let def = store_holder.read_def(id)?;
    // Reconcile stranded bookkeeping first: a previous CLI process killed
    // mid-run leaves a queued record with no task id that no runtime can
    // cancel or complete (the foundation persists runs only after the
    // enqueue, so a persisted queued record is a CLI-only state). Mark it
    // failed/interrupted so list/delete/mark-viewed see a terminal record.
    let stranded = store_holder.list_runs(id, None)?;
    for mut stale in stranded {
        if str_field(&stale, "status") == Some("queued")
            && str_field(&stale, "task_id").unwrap_or("").is_empty()
        {
            stale["status"] = serde_json::json!("failed");
            stale["error"] = serde_json::json!(
                "interrupted: the pinvou CLI process was terminated before this run finished"
            );
            stale["ended_at"] = serde_json::json!(now_string());
            let _ = store_holder.save_run(&stale);
        }
    }
    let kind = kind_for(
        &read_registry(&store_holder.task_kinds_path(), &["tasks"]),
        id,
    );
    if kind.as_deref() != Some("memory_organize") {
        // Chat-kind run-now drives the GUI's ScheduledChatExecutor +
        // foundation TaskManager, which are not exposed headlessly; report a
        // stable failure instead of faking a run.
        return Err(CliError::failed(format!(
            "scheduled_chat_run_requires_product_host: task {id} is an engine conversation \
task; {RUN_HELP_HOST_REQUIREMENT} (run it from the Pinvou app)"
        )));
    }
    if !memory_feature::memory_enabled() {
        return Err(CliError::failed(
            "scheduled_memory_organize_disabled: memory organize tasks require memory to be \
enabled in settings",
        ));
    }
    // Run-record bookkeeping: only the terminal record is persisted. A
    // pre-work `queued` record with no task id is a state the GUI runtime can
    // neither cancel nor complete nor delete (its reconcile filters on
    // task_id and its delete errors on active runs without one), so a CLI
    // process killed mid-work would wedge the task for the GUI forever — the
    // same reason the foundation persists runs only after the enqueue. A
    // killed CLI therefore leaves no record; the reconcile above remains for
    // records written by earlier builds.
    let run_id = new_storage_id();
    let now = now_string();
    let organize = organize_headless();
    let (status, error) = match &organize {
        Ok(_) => ("completed", serde_json::Value::Null),
        Err(error) => ("failed", serde_json::json!(error)),
    };
    let record = serde_json::json!({
        "schema_version": 1,
        "id": run_id,
        "automation_id": id,
        "scheduled_for": now,
        "status": status,
        "created_at": now,
        "started_at": now,
        "ended_at": serde_json::json!(now_string()),
        "task_id": serde_json::Value::Null,
        "thread_id": serde_json::Value::Null,
        "turn_id": serde_json::Value::Null,
        "error": error,
    });
    store_holder.save_run(&record).map_err(|error| {
        // Without the record the run has no history trail; unlike a stranded
        // queued record it cannot wedge deletion or the GUI sweep. Surface
        // the loss honestly instead.
        CliError::failed(format!(
            "scheduled_run_unrecorded: the run finished but its record could not be \
             written ({error}); the run history for task {id} is not persisted"
        ))
    })?;
    if let Ok(mut latest) = store_holder.read_def(id) {
        if latest.is_object() {
            latest["updated_at"] = serde_json::json!(now_string());
            latest["last_run_at"] = record["ended_at"].clone();
            let _ = store_holder.write_def(&latest);
        }
    }
    // Enrichment is best-effort: the organize pass already ran and its run
    // record is committed above, so a sessions store boot failure must not
    // report a finished, irreversible run as failed.
    let sessions = open_sessions_for_enrichment();
    let titles = sessions.as_ref().map(session_titles).unwrap_or_default();
    let value = map_run(
        &record,
        sessions.as_ref(),
        &titles,
        &read_registry(&store_holder.read_state_path(), &["viewed_runs"]),
        str_field(&def, "name"),
        str_field(&def, "model"),
    );
    if !matches!(error, serde_json::Value::Null) {
        let error = error.as_str().unwrap_or("unknown failure");
        // Exit/reporting contract: a completed outcome (exit 0) asserts the
        // run succeeded. The organize pass FAILED here — the record says
        // `failed` — so exit 0 would report a successful command over a
        // failed run. The run itself is not retryable through this command
        // (memory organize is not transactional), but the honest verdict
        // still matters: scripts gating on the exit code would retry or
        // alert on a failure that was reported as success. The record is
        // durable either way; the payload is reported on stderr (the human
        // line names the run id and the error, redacted by organize_headless
        // upstream) and the command takes the failure class (exit 1), like
        // every other executing family.
        note!("pinvou: run {} (task {}) failed: {error}", run_id, id);
        let human = format!("Run failed: {}\nTask: {}\nError: {error}", run_id, id);
        let _ = render(output, human, &value);
        return Err(CliError::failed(format!(
            "scheduled_run_failed: the memory-organize run {run_id} failed: {error}"
        )));
    }
    let human = format!(
        "Run: {}\nTask: {}\nSession: -\nStatus: {}",
        run_id, id, status
    );
    let stdout = render(output, human, &value);
    // A completed command reporting a failed result exits 1, the convention
    // the `models` family set with `probe-local`: the output body (and the
    // persisted run record) stay exactly as before, but the exit code must
    // tell scripts the outcome was not a success.
    Ok(CliOutcome {
        exit_code: if status == "failed" {
            ExitCode::Failed
        } else {
            ExitCode::Success
        },
        stdout,
    })
}

/// One memory-organize pass through the windowless product host, the same
/// wiring as `memory organize` in this crate and the GUI scheduled executor's
/// shared-bridge fallback: requires a display and a configured active model.
/// The host bootstrap panics (tauri's EventLoop refuses a non-main thread)
/// rather than returning Err in some embedded environments — a panic here
/// would take the whole process down with exit 101, outside the exit-code
/// contract, so the boot is catch_unwind-wrapped and downgraded to the
/// command's ordinary failure lane.
fn organize_headless() -> Result<(), String> {
    let result = std::panic::catch_unwind(|| {
        pinvou3_lib::headless_bridge::run_windowless_host(|pool, _store| async move {
            let mut bridge = pool.bridge.clone();
            bridge.prefs = UserPrefs::load();
            bridge.session_model = None;
            // The work-closure bound (`WorkFuture: Future<Output =
            // anyhow::Result<T>>`) pins the error type by inference, exactly
            // like `memory organize`'s closure — so anyhow is never named
            // here and the `--no-default-features` build (no `dep:anyhow` in
            // the CLI) stays clean. Redaction happens once, in the
            // `Ok(Err(error))` arm below.
            memory_feature::organize_memory_with_llm(&bridge, None)
                .await
                .map(|_| ())
        })
    });
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(pinvou3_lib::platform::credential_store::redact_secret(
            &format!("{error:#}"),
        )),
        Err(panic) => {
            let reason = panic
                .downcast_ref::<&str>()
                .map(|reason| (*reason).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| {
                    "the product host could not boot in this environment".to_owned()
                });
            Err(format!(
                "scheduled_host_unavailable: {reason} (a display and an active model are required; run this task from the Pinvou app if the host cannot start here)"
            ))
        }
    }
}

fn runs(id: &str, limit: Option<usize>, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store_holder = TaskStore::new()?;
    let def = store_holder.read_def(id)?;
    let sessions = open_sessions()?;
    let read_state = read_registry(&store_holder.read_state_path(), &["viewed_runs"]);
    let records = store_holder.list_runs(id, limit)?;
    let names = task_name_map(std::slice::from_ref(&def));
    let (lines, values) = render_runs(&sessions, &records, &read_state, &names);
    let human = if lines.is_empty() {
        format!("No runs for scheduled task {id}.")
    } else {
        lines.join("\n")
    };
    let value = serde_json::json!({ "id": id, "runs": values });
    Ok(success(render(output, human, &value)))
}

fn runs_all(limit: Option<usize>, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store_holder = TaskStore::new()?;
    let sessions = open_sessions()?;
    let read_state = read_registry(&store_holder.read_state_path(), &["viewed_runs"]);
    let defs = store_holder.list_defs()?;
    let mut records: Vec<serde_json::Value> = Vec::new();
    let mut names = task_name_map(&defs);
    let mut active_keys = std::collections::HashSet::new();
    for def in &defs {
        let task_id = str_field(def, "id").unwrap_or("").to_owned();
        for run in store_holder.list_runs(&task_id, None)? {
            active_keys.insert((
                task_id.clone(),
                str_field(&run, "id").unwrap_or("").to_owned(),
            ));
            records.push(run);
        }
    }
    // Archived tasks (deleted through the GUI or this CLI) keep their run
    // history visible in runs-all, exactly like the GUI sidebar feed.
    let archive_path = store_holder.history_archive_path();
    let archive = read_registry(&archive_path, &["tasks"]);
    if let Some(tasks) = archive.get("tasks").and_then(|value| value.as_object()) {
        for (task_id, archived) in tasks {
            names.entry(task_id.clone()).or_insert_with(|| {
                (
                    str_field(&archived["task"], "name")
                        .unwrap_or("")
                        .to_owned(),
                    archived["task"]["model"].as_str().map(str::to_owned),
                )
            });
            if let Some(runs) = archived.get("runs").and_then(|value| value.as_array()) {
                for run in runs {
                    // Archived runs get the same typed gate as the active
                    // lane (`AutomationRunRecord` decode through the shim),
                    // but a failure skips the record instead of failing the
                    // command: that is what the GUI does with the same bytes
                    // (`deserialize_archived_runs_lossy` warns and drops),
                    // and one hand-edited entry in the shared archive must
                    // not be able to hide every healthy task's runs. Pushing
                    // it unchecked is the option that is not available —
                    // `map_run` renders it as a phantom row of empty fields.
                    if run_from_value(run).is_err() {
                        note!(
                            "pinvou: warning: ignoring invalid run in the scheduled history \
                             archive for task {task_id}"
                        );
                        continue;
                    }
                    let run_id = str_field(run, "id").unwrap_or("");
                    if !active_keys.contains(&(task_id.clone(), run_id.to_owned())) {
                        records.push(run.clone());
                    }
                }
            }
        }
    }
    records.sort_by(|a, b| record_time(b, "scheduled_for").cmp(&record_time(a, "scheduled_for")));
    if let Some(limit) = limit {
        records.truncate(limit);
    }
    let (lines, values) = render_runs(&sessions, &records, &read_state, &names);
    let human = if lines.is_empty() {
        "No scheduled runs.".to_owned()
    } else {
        lines.join("\n")
    };
    let value = serde_json::json!({ "runs": values });
    Ok(success(render(output, human, &value)))
}

fn mark_viewed(task_id: &str, run_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store_holder = TaskStore::new()?;
    let sessions = open_sessions()?;
    // Active runs first, then the deleted-task history archive — keyed like
    // the GUI on whether the task definition still exists (a deleted task's
    // runs directory is simply absent, which list_runs reports as empty, so
    // existence — not a read error — is the discriminator). A definition
    // that exists but cannot be read (corrupt JSON, unreadable file,
    // unsupported schema) is a storage failure, not a missing task: surface
    // it honestly instead of misreporting "task does not exist" from the
    // archive branch.
    let runs = match store_holder.read_def(task_id) {
        Ok(_) => store_holder.list_runs(task_id, None)?,
        Err(error) if error.to_string().starts_with("scheduled_task_not_found") => {
            let archive = read_registry(&store_holder.history_archive_path(), &["tasks"]);
            archive
                .get("tasks")
                .and_then(|value| value.get(task_id))
                .and_then(|task| task.get("runs"))
                .and_then(|value| value.as_array())
                .cloned()
                .ok_or(error)?
        }
        Err(error) => return Err(error),
    };
    let run = runs
        .iter()
        .find(|run| str_field(run, "id") == Some(run_id))
        .ok_or_else(|| {
            CliError::failed(format!(
                "scheduled_run_not_found: run {run_id} does not belong to task {task_id}"
            ))
        })?;
    if str_field(run, "status") != Some("completed") {
        return Err(CliError::failed(format!(
            "scheduled_run_not_completed: run {run_id} is not completed and cannot be marked \
viewed"
        )));
    }
    if owned_session_id(run, task_id, &sessions).is_none() {
        return Err(CliError::failed(format!(
            "scheduled_run_not_viewable: run {run_id} has no valid conversation to mark as \
viewed"
        )));
    }
    let mut read_state = read_registry(&store_holder.read_state_path(), &["viewed_runs"]);
    // Same newer-schema refusal as registry_tasks_mut: a read-state file
    // written by a newer app version must not be merged and written back.
    if read_state.is_object() {
        ensure_sidecar_schema(&read_state, 2, "read-state")?;
    }
    // A wrong-shaped but valid payload (hand-edited or partially written
    // file) normalizes to the default like the parse-failure path instead of
    // panicking with an exit code outside the 0/1/2 contract.
    if !read_state.is_object() {
        read_state = serde_json::json!({ "schema_version": 2, "viewed_runs": {} });
    }
    if !read_state.get("viewed_runs").is_some_and(Value::is_object) {
        read_state["viewed_runs"] = serde_json::json!({});
    }
    {
        let viewed = read_state["viewed_runs"]
            .as_object_mut()
            .expect("viewed_runs normalized to an object above");
        let entry = viewed
            .entry(task_id.to_owned())
            .or_insert_with(|| serde_json::json!([]));
        if !entry.is_array() {
            *entry = serde_json::json!([]);
        }
        let list = entry
            .as_array_mut()
            .expect("viewed list normalized to an array above");
        if !list.iter().any(|value| value.as_str() == Some(run_id)) {
            list.push(serde_json::json!(run_id));
        }
    }
    // Same opportunistic compaction as the GUI's `compact_viewed_runs`:
    // viewed ids outside the retained run list this command read (active
    // files or the deleted-task history archive) are dropped while writing.
    {
        let current: std::collections::HashSet<&str> =
            runs.iter().filter_map(|run| str_field(run, "id")).collect();
        if let Some(list) = read_state["viewed_runs"]
            .get_mut(task_id)
            .and_then(|value| value.as_array_mut())
        {
            list.retain(|value| {
                value
                    .as_str()
                    .is_some_and(|marked| current.contains(marked))
            });
        }
    }
    write_json_atomic(&store_holder.read_state_path(), &read_state)?;
    let (has_unread, _) = {
        let refreshed = read_registry(&store_holder.read_state_path(), &["viewed_runs"]);
        let def = serde_json::json!({ "id": task_id });
        unread_and_running(&def, &runs, Some(&sessions), &refreshed)
    };
    let value = serde_json::json!({
        "automationId": task_id,
        "runId": run_id,
        "hasUnreadRuns": has_unread,
    });
    Ok(success(render(
        output,
        format!("Marked run viewed: {task_id} {run_id}"),
        &value,
    )))
}

fn chat_prompt(output: OutputMode) -> Result<CliOutcome, CliError> {
    let value = serde_json::json!({ "prompt": SCHEDULED_TASK_CHAT_PROMPT });
    Ok(success(render(
        output,
        SCHEDULED_TASK_CHAT_PROMPT.to_owned(),
        &value,
    )))
}

fn open_sessions() -> Result<SessionStore, CliError> {
    SessionStore::boot()
        .map_err(|error| CliError::failed(format!("sessions store unavailable: {error:#}")))
}

/// Best-effort sessions store for enriching a response after the store write
/// is already committed (`update`/`pause`/`resume`/`delete`): the mutation is
/// irreversible, so a boot failure must not report the command as failed — it
/// prints a warning and the response degrades without session-derived fields.
fn open_sessions_for_enrichment() -> Option<SessionStore> {
    match open_sessions() {
        Ok(store) => Some(store),
        Err(error) => {
            note!(
                "pinvou: warning: could not open the sessions store to enrich this response: \
                 {error}"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<ScheduledCommand, CliError> {
        let mut owned: Vec<String> = arguments.iter().map(|value| value.to_string()).collect();
        // arguments start at the subcommand token; parse_args strips argv[0].
        owned.insert(0, "scheduled".to_owned());
        owned.insert(0, "pinvou".to_owned());
        match crate::parse_args(owned)?.command() {
            crate::CliCommand::Scheduled(command) => Ok(command.clone()),
            _other => panic!(
                "parsed an unexpected command family; the fixture argv does not match the test"
            ),
        }
    }

    #[test]
    fn parses_every_subcommand() {
        assert_eq!(parse(&["list"]).unwrap(), ScheduledCommand::List);
        assert_eq!(
            parse(&["show", "t-1"]).unwrap(),
            ScheduledCommand::Show { id: "t-1".into() }
        );
        assert_eq!(
            parse(&[
                "create",
                "--name",
                "Report",
                "--prompt-file",
                "prompt.md",
                "--rrule",
                "FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
                "--kind",
                "memory-organize",
                "--model-id",
                "m-1",
                "--mode",
                "yolo",
                "--paused",
            ])
            .unwrap(),
            ScheduledCommand::Create {
                name: "Report".into(),
                prompt_file: PathBuf::from("prompt.md"),
                rrule: "FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30".into(),
                kind: TaskKind::MemoryOrganize,
                model_id: Some("m-1".into()),
                mode: Some(TaskMode::Yolo),
                paused: true,
            }
        );
        assert_eq!(
            parse(&[
                "create",
                "--name",
                "N",
                "--prompt-file",
                "prompt.md",
                "--rrule",
                "FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=9;BYMINUTE=30",
            ])
            .unwrap(),
            ScheduledCommand::Create {
                name: "N".into(),
                prompt_file: PathBuf::from("prompt.md"),
                rrule: "FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=9;BYMINUTE=30".into(),
                kind: TaskKind::Chat,
                model_id: None,
                mode: None,
                paused: false,
            }
        );
        assert_eq!(
            parse(&["update", "t-1", "--name", "New"]).unwrap(),
            ScheduledCommand::Update {
                id: "t-1".into(),
                name: Some("New".into()),
                prompt_file: None,
                rrule: None,
                model_id: None,
            }
        );
        for (name, expected) in [
            ("pause", ScheduledCommand::Pause { id: "t-1".into() }),
            ("resume", ScheduledCommand::Resume { id: "t-1".into() }),
            ("pin", ScheduledCommand::Pin { id: "t-1".into() }),
            ("unpin", ScheduledCommand::Unpin { id: "t-1".into() }),
            ("run", ScheduledCommand::Run { id: "t-1".into() }),
        ] {
            assert_eq!(parse(&[name, "t-1"]).unwrap(), expected, "{name}");
        }
        // delete without --yes stays parseable: require_yes enforces it at
        // execute time (exit-code 2).
        assert_eq!(
            parse(&["delete", "t-1"]).unwrap(),
            ScheduledCommand::Delete {
                id: "t-1".into(),
                yes: false,
            }
        );
        assert_eq!(
            parse(&["delete", "t-1", "--yes"]).unwrap(),
            ScheduledCommand::Delete {
                id: "t-1".into(),
                yes: true,
            }
        );
        assert_eq!(
            parse(&["runs", "t-1", "--limit", "5"]).unwrap(),
            ScheduledCommand::Runs {
                id: "t-1".into(),
                limit: Some(5),
            }
        );
        assert_eq!(
            parse(&["runs-all"]).unwrap(),
            ScheduledCommand::RunsAll { limit: None }
        );
        assert_eq!(
            parse(&["runs-all", "--limit", "3"]).unwrap(),
            ScheduledCommand::RunsAll { limit: Some(3) }
        );
        assert_eq!(
            parse(&["mark-viewed", "t-1", "r-1"]).unwrap(),
            ScheduledCommand::MarkViewed {
                task_id: "t-1".into(),
                run_id: "r-1".into(),
            }
        );
        assert_eq!(
            parse(&["chat-prompt"]).unwrap(),
            ScheduledCommand::ChatPrompt
        );
    }

    /// `--mode` is validated, never discarded: the app's
    /// `canonical_scheduled_mode` refuses anything but `yolo` because the
    /// downstream request overwrites the field unconditionally, so accepting
    /// `agent`/`plan` would hand the caller a full-YOLO task under another
    /// name. The typed parse assertion lives here; the exit-code class is
    /// pinned in the contract tests.
    #[test]
    fn mode_accepts_only_the_yolo_scheduled_execution_mode() {
        for accepted in ["yolo", " yolo ", ""] {
            assert_eq!(TaskMode::parse_value(accepted).unwrap(), TaskMode::Yolo);
        }
        for refused in ["agent", "plan", "bogus"] {
            let error = TaskMode::parse_value(refused).expect_err(refused);
            assert_eq!(error.exit_code(), crate::ExitCode::Usage, "{refused}");
            assert!(error.to_string().contains(refused), "{refused}: {error}");
        }
    }

    #[test]
    fn rrule_validation_mirrors_the_foundation_grammar() {
        for valid in [
            "FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
            "freq=hourly",
            "FREQ=HOURLY;BYDAY=MO,WE;INTERVAL=2",
            "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30",
            "FREQ=ONCE;AT=2035-09-10T08:30",
            "FREQ=ONCE;AT=2035-09-10T08:30:00Z",
            "FREQ=ONCE;AT=2035-09-10T08:30:00+02:00",
            "FREQ=CRON;EXPR=30 8 * * MON-FRI",
            "FREQ=CRON;EXPR=*/15 0 * JAN,DEC SUN",
        ] {
            if let Err(error) = validate_rrule(valid) {
                panic!("rejected valid rrule '{valid}': {error}");
            }
        }
        // The cron atom loop iterated with `checked_add` under a step that
        // parsed as u32 but the mirror had already made negative steps
        // unreachable — the foundation's own `next <= current` guard is the
        // authority now, so this stays a pure foundation-grammar pin.
        for invalid in [
            "",
            "INTERVAL=6",
            "FREQ=MINUTELY;INTERVAL=5",
            "FREQ=DAILY;BYHOUR=9",
            "FREQ=YEARLY;BYMONTH=1",
            "FREQ=HOURLY;INTERVAL=0",
            "FREQ=HOURLY;SECOND=1",
            "FREQ=HOURLY;BYDAY=XX",
            "FREQ=HOURLY;BYHOUR=24",
            "FREQ=WEEKLY;BYDAY=MO",
            "FREQ=WEEKLY;BYDAY=MO;BYMINUTE=30",
            "FREQ=WEEKLY;BYDAY=;BYHOUR=9;BYMINUTE=30",
            "FREQ=WEEKLY;BYDAY=MO;BYHOUR=9;BYMINUTE=60",
            "FREQ=WEEKLY;WEEKDAY=MO;BYHOUR=9;BYMINUTE=30",
            "FREQ=ONCE;AT=never",
            "FREQ=ONCE;AT=2026-13-01T08:30",
            "FREQ=ONCE;AT=2035-09-10T08:30:00;BYDAY=MO",
            "FREQ=CRON;EXPR=* * * *",
            "FREQ=CRON;EXPR=61 * * * *",
            "FREQ=CRON;EXPR=* 25 * * *",
            "FREQ=CRON;EXPR=0 0 31 FEB *",
            "FREQ=CRON;EXPR=* * * * * /5",
            "NOT-A-PAIR",
        ] {
            let error = validate_rrule(invalid).expect_err(invalid);
            assert_eq!(error.exit_code(), crate::ExitCode::Usage, "{invalid}");
        }
    }

    #[test]
    fn hourly_intervals_beyond_the_scheduler_range_are_refused_before_the_foundation() {
        // The pre-check bound from the previous mirror, kept: the foundation
        // parser itself accepts any u32 INTERVAL; the scheduler's interval
        // arithmetic is what overflows. 1e6 passes both layers, 1e6+1 and
        // u32-overflow values are usage errors.
        assert!(validate_rrule("FREQ=HOURLY;INTERVAL=1000000").is_ok());
        for refused in [
            "FREQ=HOURLY;INTERVAL=1000001",
            "FREQ=HOURLY;INTERVAL=4000000000",
        ] {
            let error = validate_rrule(refused).expect_err(refused);
            assert_eq!(error.exit_code(), crate::ExitCode::Usage, "{refused}");
            assert!(
                error.to_string().contains("INTERVAL must be <="),
                "{refused}"
            );
        }
    }

    #[test]
    fn schedule_labels_stay_english_and_storage_ids_stay_single_components() {
        assert_eq!(
            humanize_rrule("FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30"),
            "every 6 hours from 08:30"
        );
        assert_eq!(
            humanize_rrule("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30"),
            "MO,TU,WE,TH,FR,SA,SU 08:30"
        );
        assert_eq!(
            humanize_rrule("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=30"),
            "workdays 09:30"
        );
        assert_eq!(
            humanize_rrule("FREQ=ONCE;AT=2035-09-10T08:30"),
            "FREQ=ONCE;AT=2035-09-10T08:30"
        );
        // Storage ids must be single path components (the foundation's
        // ensure_safe_storage_id rule).
        assert!(safe_storage_id("task id", "abc-def").is_ok());
        assert!(safe_storage_id("task id", "../escape").is_err());
        assert!(safe_storage_id("task id", "").is_err());
    }

    #[test]
    fn record_ordering_uses_the_foundation_chrono_semantics() {
        // record_time sorts by the foundation's own parser (chrono
        // RFC3339): offsets normalize, anything undecodable floors to the
        // epoch minimum so it sorts last — the read gates have already
        // refused such records before ordering runs; this pins the floor
        // for the archive lane, which skips instead of failing.
        let value = serde_json::json!({
            "a": "2026-09-10T12:34:56.123Z",
            "b": "2026-09-10T14:34:56.123+02:00",
        });
        // Offsets normalize to the same instant; compare timestamps, not
        // rendered strings (trailing fractional digits differ in display).
        assert_eq!(
            record_time(&value, "a").timestamp_nanos_opt(),
            record_time(&value, "b").timestamp_nanos_opt()
        );
        let later = serde_json::json!({ "t": "2026-09-10T12:34:57Z" });
        assert!(record_time(&later, "t") > record_time(&value, "a"));
        let undecodable = serde_json::json!({ "t": "not-a-date" });
        assert!(record_time(&undecodable, "t") < record_time(&value, "a"));
        let offsetless = serde_json::json!({ "t": "2026-09-10T12:34:56" });
        assert!(record_time(&offsetless, "t") < record_time(&value, "a"));
    }

    /// S9 (round-18 review): `write_json_atomic` used the staging pattern the
    /// round-16 `artifacts::atomic_write` documented as removed
    /// (`O_CREAT|O_TRUNC`, no `O_EXCL`). It is now aligned with that hardened
    /// writer — `create_new` staging, propagated `sync_all`, parent-directory
    /// fsync — while keeping the per-pid/nanos staging name: the foundation
    /// writer stages under a fixed `.json.tmp` sibling shared by all concurrent
    /// writers, and the CLI keeps the unique name so two CLI processes writing
    /// the same registry never rename each other's content. The staging path's
    /// nanos token makes the occupied-path guarantee untestable through the
    /// plain entry point, so this pins the stage lane directly.
    #[test]
    fn write_json_atomic_refuses_an_occupied_staging_path() {
        let dir = std::env::temp_dir().join(format!(
            "pinvou-scheduled-atomic-occupied-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("model-bindings.json");
        std::fs::write(&target, b"old").unwrap();
        let tmp = dir.join(".model-bindings.json.tmp-fixed");
        std::fs::write(&tmp, b"leftover").unwrap();
        let error = write_json_atomic_staged(&target, &tmp, br#"{"staged":true}"#)
            .map(|_unit| ())
            .expect_err("an occupied staging path must fail the write");
        assert!(
            error.to_string().contains(&tmp.display().to_string()),
            "the failure must name the staging path: {error}"
        );
        // The occupied staging file and the target's old content survive:
        // the cleanup must not delete the path create_new protected.
        assert_eq!(std::fs::read(&tmp).unwrap(), b"leftover");
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The happy path through the same writer: payload lands, no staging file
    /// is left behind. Reverting the `create_new` staging to `std::fs::write`
    /// fails the occupied-path test above; this one additionally catches a
    /// regressed rename/cleanup leaking a visible `.tmp` sibling.
    #[test]
    fn write_json_atomic_lands_the_payload_and_leaves_no_staging_file() {
        let dir = std::env::temp_dir().join(format!(
            "pinvou-scheduled-atomic-landed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("task-kinds.json");
        std::fs::write(&target, br#"{"old":true}"#).unwrap();
        write_json_atomic(&target, &serde_json::json!({ "tasks": {} })).unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            serde_json::to_string_pretty(&serde_json::json!({ "tasks": {} })).unwrap()
        );
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
