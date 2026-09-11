//! `scheduled` family: GUI-parity surface over the scheduled-task stores the
//! GUI writes (`pinvou3-app/src-tauri/src/app/commands/scheduled.rs` ->
//! `features::scheduled::tasks`), mapped onto the same persisted files.
//!
//! Why a store-level mirror: `features::scheduled` is `pub(crate)` to
//! `pinvou3_lib` (features/mod.rs) and the automation registry lives in the
//! CodeWhale `codewhale-tui` crate (`AutomationManager`), which is not a CLI
//! dependency, so neither `ScheduledTaskState` nor `AutomationManager` can be
//! constructed from the CLI. Following the `sessions` family precedent, this
//! module operates on the exact same JSON stores the GUI/foundation use:
//! - task definitions: `~/.pinvou3/automations/automations/{id}.json`
//!   (`AutomationRecord`, schema 2, `AutomationManager::open(root)` layout)
//! - run records: `~/.pinvou3/automations/runs/{task_id}/{stamp}-{run}.json`
//! - sidecars: `model-bindings.json`, `task-kinds.json`,
//!   `task-ui-metadata.json` (under `automations/`), read state under
//!   `scheduled-runs/read-state.json`, history archive under
//!   `automations/history-archive.json` (same schemas as
//!   `features::scheduled::stores`)
//! - run sessions: `pinvou3_lib::features::sessions::SessionStore` (public),
//!   the same calls the GUI mapper makes (`scheduled_profile`, `is_pinned`,
//!   `pinned_at`, `is_hidden`, `list_scheduled`).
//!
//! Disclosed deviations from the GUI (all deferred to the foundation, never
//! duplicated half-way here):
//! - `next_run_at` is left unset on create/rrule-update/resume. The GUI
//!   computes it from the local-timezone schedule; the foundation's scheduler
//!   sweep initializes a missing `next_run_at` on its next tick
//!   (`automation_manager::collect_due_runs`), so the end state converges.
//! - Run-status reconciliation needs the foundation `TaskManager`; runs are
//!   reported exactly as persisted.
//! - `scheduled run` executes only `memory_organize` tasks (app-side, no
//!   engine conversation; same wiring as `memory organize` in this crate,
//!   minus the session-bound bridge which is `pub(crate)` to `pinvou3_lib`).
//!   Chat-kind run-now drives the GUI's `ScheduledChatExecutor` +
//!   `TaskManager`, which are not exposed headlessly, and is refused with a
//!   stable error instead of being faked.

use std::path::{Path, PathBuf};

use serde_json::Value;

use pinvou3_lib::features::memory as memory_feature;
use pinvou3_lib::features::sessions::SessionStore;
use pinvou3_lib::platform::prefs::UserPrefs;

use crate::support::{render, require_yes, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};

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

/// Approval mode. The GUI validates these three values and then always
/// persists `yolo` (scheduled execution cannot show an approval prompt), so
/// the CLI mirrors the validation and the fixed persisted value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskMode {
    Agent,
    Plan,
    Yolo,
}

impl TaskMode {
    fn parse_value(value: &str) -> Result<Self, CliError> {
        match value {
            "agent" => Ok(Self::Agent),
            "plan" => Ok(Self::Plan),
            "yolo" => Ok(Self::Yolo),
            other => Err(CliError::usage(format!(
                "scheduled task mode must be exactly one of agent|plan|yolo, got '{other}'"
            ))),
        }
    }

    /// The GUI always persists this mode for scheduled execution.
    const PERSISTED: &'static str = "yolo";
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
        mode: Option<TaskMode>,
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
/// command (`features::scheduled::tasks::SCHEDULED_TASK_CHAT_PROMPT`, mirrored
/// verbatim: it is GUI data the model consumes, not CLI copy).
const SCHEDULED_TASK_CHAT_PROMPT: &str = r#"我想创建一个 Pinvou 定时任务。请通过提问帮我确定方案，回复保持简短，不要长篇解释。

这是一个纯对话收集流程。不要调用任何工具，不要写文件，不要读写 ~/.pinvou3，也不要手动创建 automations JSON。信息完整后只输出给前端解析的任务参数，前端会通过 create_scheduled_task 创建并打开任务详情，不再要求用户二次确认。

严禁使用 schtasks、Windows Task Scheduler、任务计划程序、cron、crontab、systemd timer 或任何系统级计划任务。错误做法：使用 schtasks 创建 Windows 任务。正确做法：返回 scheduled-task-draft JSON，由 Pinvou 前端调用 create_scheduled_task。

请一次只问我一个问题，并依次确认这些信息：
1. 任务要做什么。
2. 什么时候运行。支持每 N 小时（可指定起始时间）、每天指定时间、每周指定星期和时间。不支持分钟级规则；如果用户要求“每 5 分钟”等分钟级频率，必须询问用户改成每 N 小时、每天指定时间或每周指定时间，不要输出草稿。

每次运行创建独立对话；同一个定时任务的所有运行对话共享该任务的专属工作间，不同任务互不共享。产物仍归属各次运行对话。不需要询问工作目录或权限设置。

整理草稿时，请把时间转换成 rrule：
- 每 6 小时一次，从 08:30 起算：FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30
- 每天 08:30：FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30
- 每周一、三 09:30：FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=30

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
    validate_rrule(&rrule)?;
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
        paused: flags.contains(&"--paused"),
    })
}

fn parse_update(rest: &[String]) -> Result<ScheduledCommand, CliError> {
    let id = require_id(rest.first(), "scheduled update")?;
    // The kind is a one-time creation property (the GUI's UpdateScheduledTaskInput
    // has no such field); reject it with the reason before the generic parser.
    if rest[1..].iter().any(|token| token == "--kind") {
        return Err(CliError::usage(
            "scheduled update does not accept --kind: kind is settable only at create time",
        ));
    }
    let (options, _) = parse_flags(
        &rest[1..],
        &["--name", "--prompt-file", "--rrule", "--model-id", "--mode"],
        &[],
    )?;
    let name = option(&options, "--name").map(str::to_owned);
    let prompt_file = option(&options, "--prompt-file").map(PathBuf::from);
    let rrule = match option(&options, "--rrule") {
        Some(value) => {
            validate_rrule(value)?;
            Some(value.to_owned())
        }
        None => None,
    };
    let model_id = option(&options, "--model-id").map(str::to_owned);
    let mode = match option(&options, "--mode") {
        Some(value) => Some(TaskMode::parse_value(value)?),
        None => None,
    };
    if name.is_none()
        && prompt_file.is_none()
        && rrule.is_none()
        && model_id.is_none()
        && mode.is_none()
    {
        return Err(CliError::usage(
            "scheduled update requires at least one of --name, --prompt-file, --rrule, \
--model-id, --mode (kind is settable only at create time)",
        ));
    }
    Ok(ScheduledCommand::Update {
        id,
        name,
        prompt_file,
        rrule,
        model_id,
        mode,
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

/// Mirrors the pair-based option parser used by the other families: valued
/// `--name value` pairs (each at most once), bare flags, and positional
/// arguments; unknown options are rejected.
fn parse_flags<'a>(
    values: &'a [String],
    value_flags: &[&str],
    boolean_flags: &[&str],
) -> Result<(Vec<(&'a str, &'a str)>, Vec<&'a str>), CliError> {
    let mut options = Vec::new();
    let mut flags = Vec::new();
    let mut index = 0;
    while index < values.len() {
        let token = values[index].as_str();
        if boolean_flags.contains(&token) {
            if flags.contains(&token) {
                return Err(CliError::usage(format!(
                    "duplicate scheduled option {token}"
                )));
            }
            flags.push(token);
            index += 1;
            continue;
        }
        if !value_flags.contains(&token) {
            return Err(CliError::usage(format!(
                "unsupported scheduled option: {token}"
            )));
        }
        if options.iter().any(|(name, _)| *name == token) {
            return Err(CliError::usage(format!(
                "duplicate scheduled option {token}"
            )));
        }
        let value = values
            .get(index + 1)
            .ok_or_else(|| CliError::usage(format!("scheduled option {token} requires a value")))?;
        if value.is_empty() || value.starts_with("--") {
            return Err(CliError::usage(format!(
                "scheduled option {token} requires a value"
            )));
        }
        options.push((token, value.as_str()));
        index += 2;
    }
    Ok((options, flags))
}

fn option<'a>(options: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    options
        .iter()
        .find_map(|(candidate, value)| (*candidate == name).then_some(*value))
}

fn parse_limit(options: &[(&str, &str)], name: &str) -> Result<Option<usize>, CliError> {
    match option(options, name) {
        None => Ok(None),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|count| *count > 0)
            .map(Some)
            .ok_or_else(|| CliError::usage(format!("scheduled {name} must be a positive integer"))),
    }
}

// ---- rrule validation (mirror of codewhale-tui AutomationSchedule::parse_rrule) ----

/// Validates an rrule with the same grammar the foundation scheduler applies:
/// FREQ=ONCE (AT), FREQ=HOURLY (INTERVAL, BYDAY, BYHOUR, BYMINUTE),
/// FREQ=WEEKLY (BYDAY, BYHOUR, BYMINUTE), FREQ=CRON (EXPR, 5 fields).
/// Minute-level recurrences have no FREQ and are rejected here, exactly like
/// the GUI. Full next-run evaluation (local timezone, DST) stays with the
/// foundation's scheduler sweep.
fn validate_rrule(rrule: &str) -> Result<(), CliError> {
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
    let value = |name: &str| {
        parts
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    let unsupported = |key: &str, allowed: &str| {
        CliError::usage(format!(
            "unsupported rrule field '{key}' (allowed: {allowed})"
        ))
    };
    // The foundation uppercases the FREQ value before matching
    // (parse_rrule: `value.trim().to_ascii_uppercase()`).
    match value("FREQ")
        .map(|freq| freq.to_ascii_uppercase())
        .as_deref()
    {
        None => Err(CliError::usage(
            "rrule must include FREQ (valid: ONCE, HOURLY, WEEKLY, CRON)",
        )),
        Some("ONCE") => {
            for (key, _) in &parts {
                if key != "FREQ" && key != "AT" {
                    return Err(unsupported(key, "FREQ,AT for FREQ=ONCE"));
                }
            }
            let at = value("AT").ok_or_else(|| {
                CliError::usage(
                    "ONCE rrules require AT (local \
YYYY-MM-DDTHH:MM[:SS] or RFC3339)",
                )
            })?;
            validate_once_at(&at)
        }
        Some("HOURLY") => {
            const ALLOWED: &str = "FREQ,INTERVAL,BYDAY,BYHOUR,BYMINUTE";
            for (key, _) in &parts {
                if key != "FREQ"
                    && key != "INTERVAL"
                    && key != "BYDAY"
                    && key != "BYHOUR"
                    && key != "BYMINUTE"
                {
                    return Err(unsupported(key, ALLOWED));
                }
            }
            if let Some(interval) = value("INTERVAL") {
                let interval = parse_number(&interval, "INTERVAL")?;
                if interval == 0 {
                    return Err(CliError::usage(
                        "INTERVAL must be >= 1 for HOURLY schedules",
                    ));
                }
            }
            if let Some(byday) = value("BYDAY") {
                parse_byday(&byday)?;
            }
            if let Some(hour) = value("BYHOUR") {
                if parse_number(&hour, "BYHOUR")? > 23 {
                    return Err(CliError::usage("BYHOUR must be between 0 and 23"));
                }
            }
            if let Some(minute) = value("BYMINUTE") {
                if parse_number(&minute, "BYMINUTE")? > 59 {
                    return Err(CliError::usage("BYMINUTE must be between 0 and 59"));
                }
            }
            Ok(())
        }
        Some("WEEKLY") => {
            const ALLOWED: &str = "FREQ,BYDAY,BYHOUR,BYMINUTE";
            for (key, _) in &parts {
                if key != "FREQ" && key != "BYDAY" && key != "BYHOUR" && key != "BYMINUTE" {
                    return Err(unsupported(key, ALLOWED));
                }
            }
            let byday = value("BYDAY")
                .ok_or_else(|| CliError::usage("WEEKLY rrules require BYDAY (e.g. MO,WE)"))?;
            parse_byday(&byday)?;
            let hour = value("BYHOUR")
                .ok_or_else(|| CliError::usage("WEEKLY rrules require BYHOUR (0-23)"))?;
            if parse_number(&hour, "BYHOUR")? > 23 {
                return Err(CliError::usage("BYHOUR must be between 0 and 23"));
            }
            let minute = value("BYMINUTE")
                .ok_or_else(|| CliError::usage("WEEKLY rrules require BYMINUTE (0-59)"))?;
            if parse_number(&minute, "BYMINUTE")? > 59 {
                return Err(CliError::usage("BYMINUTE must be between 0 and 59"));
            }
            Ok(())
        }
        Some("CRON") => {
            for (key, _) in &parts {
                if key != "FREQ" && key != "EXPR" {
                    return Err(unsupported(key, "FREQ,EXPR for FREQ=CRON"));
                }
            }
            let expr = value("EXPR").ok_or_else(|| {
                CliError::usage(
                    "CRON rrules require EXPR (minute hour day-of-month month day-of-week)",
                )
            })?;
            validate_cron_expr(&expr)
        }
        Some(other) => Err(CliError::usage(format!(
            "unsupported rrule FREQ '{other}' (valid: ONCE, HOURLY, WEEKLY, CRON; \
minute-level recurrences such as FREQ=MINUTELY are not supported)"
        ))),
    }
}

fn parse_number(value: &str, field: &str) -> Result<u32, CliError> {
    value
        .parse::<u32>()
        .map_err(|_| CliError::usage(format!("failed to parse rrule {field} '{value}'")))
}

fn parse_byday(value: &str) -> Result<Vec<&'static str>, CliError> {
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
            other => {
                return Err(CliError::usage(format!(
                    "invalid BYDAY value '{other}' (valid: MO, TU, WE, TH, FR, SA, SU)"
                )));
            }
        };
        if !days.contains(&day) {
            days.push(day);
        }
    }
    Ok(days)
}

/// Structural mirror of `parse_once_at`: RFC3339 with offset, or a naive
/// local `YYYY-MM-DDTHH:MM[:SS]` stamp. Calendar-overflow checks (leap days,
/// month lengths) stay with the foundation's datetime parser; the scheduler
/// sweep re-validates every record it touches.
fn validate_once_at(at: &str) -> Result<(), CliError> {
    if parse_rfc3339(at).is_some() {
        return Ok(());
    }
    let bytes = at.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        bytes
            .get(range.clone())
            .map(|slice| std::str::from_utf8(slice).unwrap_or(""))
            .unwrap_or("")
            .parse::<u32>()
            .ok()
    };
    let numeric = |range: std::ops::Range<usize>| digits(range).is_some();
    let shape_ok = bytes.len() == 16 || bytes.len() == 19;
    let separators = bytes.len() > 15
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && (bytes.len() == 16 || (bytes.len() == 19 && bytes[16] == b':'));
    let valid = shape_ok
        && separators
        && numeric(0..4)
        && numeric(5..7)
        && numeric(8..10)
        && numeric(11..13)
        && numeric(14..16)
        && (bytes.len() == 16 || numeric(17..19));
    if !valid {
        return Err(CliError::usage(format!(
            "failed to parse ONCE AT '{at}'. Use local YYYY-MM-DDTHH:MM[:SS] or RFC3339"
        )));
    }
    let month = digits(5..7).unwrap_or(0);
    let day = digits(8..10).unwrap_or(0);
    let hour = digits(11..13).unwrap_or(0);
    let minute = digits(14..16).unwrap_or(0);
    let second = digits(17..19).unwrap_or(0);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(CliError::usage(format!(
            "ONCE AT '{at}' is not a valid calendar time"
        )));
    }
    Ok(())
}

/// Structural mirror of the foundation `ParsedCronExpr` grammar: five
/// whitespace-separated fields with `*`, lists, `a-b` ranges, `/step`, and
/// month/weekday names.
fn validate_cron_expr(expr: &str) -> Result<(), CliError> {
    const MONTH_NAMES: &[(&str, u32)] = &[
        ("JAN", 1),
        ("FEB", 2),
        ("MAR", 3),
        ("APR", 4),
        ("MAY", 5),
        ("JUN", 6),
        ("JUL", 7),
        ("AUG", 8),
        ("SEP", 9),
        ("OCT", 10),
        ("NOV", 11),
        ("DEC", 12),
    ];
    const WEEKDAY_NAMES: &[(&str, u32)] = &[
        ("SUN", 0),
        ("MON", 1),
        ("TUE", 2),
        ("WED", 3),
        ("THU", 4),
        ("FRI", 5),
        ("SAT", 6),
    ];
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(CliError::usage(
            "CRON EXPR must have exactly 5 fields: minute hour day-of-month month day-of-week",
        ));
    }
    let specs = [
        (fields[0], 0, 59, &[] as &[(&str, u32)], "minute"),
        (fields[1], 0, 23, &[], "hour"),
        (fields[2], 1, 31, &[], "day-of-month"),
        (fields[3], 1, 12, MONTH_NAMES, "month"),
        (fields[4], 0, 7, WEEKDAY_NAMES, "day-of-week"),
    ];
    let mut day_of_month_wildcard = false;
    let mut day_of_month_max = 0u32;
    let mut month_values = Vec::new();
    for (index, (raw, min, max, names, field)) in specs.iter().enumerate() {
        let mut values = Vec::new();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(CliError::usage(format!(
                "CRON {field} field must not be empty"
            )));
        }
        let wildcard = trimmed == "*";
        if index == 2 {
            day_of_month_wildcard = wildcard;
        }
        for part in trimmed.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return Err(CliError::usage(format!(
                    "CRON {field} field contains an empty list item"
                )));
            }
            let (base, step) = match part.split_once('/') {
                Some((base, step)) => {
                    let step = step.trim().parse::<u32>().map_err(|_| {
                        CliError::usage(format!("failed to parse CRON {field} step '{step}'"))
                    })?;
                    if step == 0 {
                        return Err(CliError::usage(format!("CRON {field} step must be >= 1")));
                    }
                    (base.trim(), step)
                }
                None => (part, 1),
            };
            let range = if base == "*" {
                (*min, *max)
            } else if let Some((start, end)) = base.split_once('-') {
                let start = cron_atom(start.trim(), *min, *max, names, field)?;
                let end = cron_atom(end.trim(), *min, *max, names, field)?;
                if start > end {
                    return Err(CliError::usage(format!(
                        "CRON {field} range start must be <= end"
                    )));
                }
                (start, end)
            } else {
                let start = cron_atom(base, *min, *max, names, field)?;
                if part.contains('/') {
                    (start, *max)
                } else {
                    (start, start)
                }
            };
            let mut current = range.0;
            while current <= range.1 {
                if !values.contains(&current) {
                    values.push(current);
                }
                match current.checked_add(step) {
                    Some(next) => current = next,
                    None => break,
                }
            }
        }
        if index == 2 {
            day_of_month_max = values.iter().copied().max().unwrap_or(0);
        }
        if index == 3 {
            month_values = values;
        }
    }
    // Mirror of validate_date_space: reject day-of-month values no month can
    // ever produce (leap February included via the 2024 probe).
    if !day_of_month_wildcard {
        let valid = month_values.iter().any(|month| {
            let common = days_in_month(2025, *month);
            let leap = days_in_month(2024, *month);
            day_of_month_max > 0 && (day_of_month_max <= common || day_of_month_max <= leap)
        });
        if !valid {
            return Err(CliError::usage(
                "CRON EXPR day-of-month/month combination can never occur",
            ));
        }
    }
    Ok(())
}

fn cron_atom(
    raw: &str,
    min: u32,
    max: u32,
    names: &[(&str, u32)],
    field: &str,
) -> Result<u32, CliError> {
    let value = names
        .iter()
        .find(|(name, _)| *name == raw.trim().to_ascii_uppercase())
        .map(|(_, value)| *value)
        .or_else(|| raw.trim().parse::<u32>().ok())
        .ok_or_else(|| CliError::usage(format!("invalid CRON {field} value '{raw}'")))?;
    if !(min..=max).contains(&value) {
        return Err(CliError::usage(format!(
            "CRON {field} value {value} is out of range {min}-{max}"
        )));
    }
    Ok(value)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap { 29 } else { 28 }
        }
        _ => 0,
    }
}

// ---- wall-clock helpers (no chrono in the CLI crate) ----

fn now_epoch() -> (i64, u32) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (now.as_secs() as i64, now.subsec_nanos())
}

/// Days-to-civil conversion (Howard Hinnant's algorithm), sufficient to
/// render UTC timestamps without a date-time crate.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 } as i64;
    era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100
        + (153 * mp + 2) / 5
        + day as i64
        - 1
        - 719_468
}

fn format_rfc3339_millis(secs: i64, nanos: u32) -> String {
    let millis = nanos / 1_000_000;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// `{sortable-created-at}` stamp for run file names, mirroring the
/// foundation's `%Y%m%dT%H%M%S%3fZ` format so directory listings stay
/// chronologically sorted next to GUI-created runs.
fn run_file_stamp(secs: i64, nanos: u32) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!(
        "{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}{:03}Z",
        nanos / 1_000_000
    )
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fff][Z|±HH:MM]` into epoch seconds plus
/// subsecond nanos; returns None for anything else. Used only to order
/// records, mirroring the foundation's chrono-based sorts.
fn parse_rfc3339(value: &str) -> Option<(i64, u32)> {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| {
        std::str::from_utf8(bytes.get(range)?)
            .ok()?
            .parse::<i64>()
            .ok()
    };
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = num(0..4)?;
    let month = num(5..7)? as u32;
    let day = num(8..10)? as u32;
    let hour = num(11..13)? as u32;
    let minute = num(14..16)? as u32;
    let second = num(17..19)? as u32;
    if month == 0 || month > 12 || day == 0 || day > 31 || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = &trimmed[19..];
    let mut nanos = 0u32;
    if let Some(fractional) = rest.strip_prefix('.') {
        let digits_end = fractional
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(fractional.len());
        if digits_end == 0 {
            return None;
        }
        let mut scaled = fractional[..digits_end].to_owned();
        while scaled.len() < 9 {
            scaled.push('0');
        }
        nanos = scaled[..9].parse().ok()?;
        rest = &fractional[digits_end..];
    }
    let offset_secs = match rest {
        "" | "Z" | "z" => 0,
        offset => {
            let offset = offset.as_bytes();
            if offset.len() != 6 || (offset[0] != b'+' && offset[0] != b'-') || offset[3] != b':' {
                return None;
            }
            let sign: i64 = if offset[0] == b'-' { -1 } else { 1 };
            let hours: i64 = std::str::from_utf8(&offset[1..3]).ok()?.parse().ok()?;
            let minutes: i64 = std::str::from_utf8(&offset[4..6]).ok()?.parse().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
    };
    let epoch = days_from_civil(year, month, day) * 86_400
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second)
        - i64::from(offset_secs);
    Some((epoch, nanos))
}

fn record_time(value: &serde_json::Value, field: &str) -> (i64, u32) {
    value
        .get(field)
        .and_then(|value| value.as_str())
        .and_then(parse_rfc3339)
        .unwrap_or((i64::MIN, 0))
}

// ---- store layout (mirrors AutomationManager::open(root) + app sidecars) ----

fn safe_storage_id(kind: &str, value: &str) -> Result<(), CliError> {
    let path = Path::new(value);
    let mut components = path.components();
    match components.next() {
        Some(std::path::Component::Normal(_)) if components.next().is_none() => Ok(()),
        _ => Err(CliError::failed(format!(
            "{kind} must be a single path component: {value}"
        ))),
    }
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

    /// One task definition; a missing file maps to the stable
    /// `scheduled_task_not_found` failure, like the GUI command errors.
    fn read_def(&self, id: &str) -> Result<serde_json::Value, CliError> {
        let path = self.def_path(id)?;
        let raw = std::fs::read_to_string(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CliError::failed(format!("scheduled_task_not_found: {id}"))
            } else {
                CliError::failed(format!(
                    "scheduled_storage_unavailable: cannot read {}: {error}",
                    path.display()
                ))
            }
        })?;
        let value: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
            CliError::failed(format!(
                "scheduled_storage_unavailable: cannot parse {}: {error}",
                path.display()
            ))
        })?;
        ensure_supported_schema(&value, 2)?;
        Ok(value)
    }

    /// Every task definition, newest `updated_at` first, like
    /// `AutomationManager::list_automations`.
    fn list_defs(&self) -> Result<Vec<serde_json::Value>, CliError> {
        let dir = self.defs_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(CliError::failed(format!(
                    "scheduled_storage_unavailable: cannot read {}: {error}",
                    dir.display()
                )));
            }
        };
        let mut defs = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|error| {
                    CliError::failed(format!(
                        "scheduled_storage_unavailable: cannot list {}: {error}",
                        dir.display()
                    ))
                })?
                .path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let raw = std::fs::read_to_string(&path).map_err(|error| {
                CliError::failed(format!(
                    "scheduled_storage_unavailable: cannot read {}: {error}",
                    path.display()
                ))
            })?;
            let value: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
                CliError::failed(format!(
                    "scheduled_storage_unavailable: cannot parse {}: {error}",
                    path.display()
                ))
            })?;
            ensure_supported_schema(&value, 2)?;
            defs.push(value);
        }
        defs.sort_by(|a, b| record_time(b, "updated_at").cmp(&record_time(a, "updated_at")));
        Ok(defs)
    }

    /// Atomic pretty-JSON write, same shape as the foundation's
    /// `write_json_atomic` (`.json.tmp` sibling + rename).
    fn write_def(&self, def: &serde_json::Value) -> Result<(), CliError> {
        let id = def
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| CliError::failed("scheduled_storage_unavailable: task id missing"))?
            .to_owned();
        write_json_atomic(&self.def_path(&id)?, def)
    }

    /// Run records for one task, newest first (created_at, ties broken by the
    /// sortable file name), legacy `{run_id}.json` files merged in — the same
    /// ordering rules as `AutomationManager::list_runs`.
    fn list_runs(
        &self,
        id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<serde_json::Value>, CliError> {
        let dir = self.runs_dir_for(id)?;
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(CliError::failed(format!(
                    "scheduled_storage_unavailable: cannot read {}: {error}",
                    dir.display()
                )));
            }
        };
        let mut sortable = Vec::new();
        let mut legacy = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|error| {
                    CliError::failed(format!(
                        "scheduled_storage_unavailable: cannot list {}: {error}",
                        dir.display()
                    ))
                })?
                .path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            match path.file_stem().and_then(|stem| stem.to_str()) {
                Some(stem) if has_sortable_run_stem(stem) => sortable.push(path),
                _ => legacy.push(path),
            }
        }
        sortable.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        // No early truncate here: dedup happens after the created_at re-sort
        // (a legacy duplicate may not be adjacent), so truncating before
        // reading could drop records that dedup would have kept.
        let mut runs = Vec::new();
        for path in sortable.into_iter().chain(legacy) {
            let raw = std::fs::read_to_string(&path).map_err(|error| {
                CliError::failed(format!(
                    "scheduled_storage_unavailable: cannot read {}: {error}",
                    path.display()
                ))
            })?;
            let run: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
                CliError::failed(format!(
                    "scheduled_storage_unavailable: cannot parse {}: {error}",
                    path.display()
                ))
            })?;
            ensure_supported_schema(&run, 1)?;
            runs.push(run);
        }
        runs.sort_by(|a, b| record_time(b, "created_at").cmp(&record_time(a, "created_at")));
        runs.dedup_by(|a, b| a.get("id") == b.get("id"));
        if let Some(limit) = limit {
            runs.truncate(limit);
        }
        Ok(runs)
    }

    /// Persist one run record under its sortable name, mirroring
    /// `AutomationManager::save_run`.
    fn save_run(&self, run: &serde_json::Value) -> Result<(), CliError> {
        let id = run
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| CliError::failed("scheduled_storage_unavailable: run id missing"))?
            .to_owned();
        let automation_id = run
            .get("automation_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                CliError::failed("scheduled_storage_unavailable: run automation id missing")
            })?
            .to_owned();
        let dir = self.runs_dir_for(&automation_id)?;
        std::fs::create_dir_all(&dir).map_err(|error| {
            CliError::failed(format!(
                "scheduled_storage_unavailable: cannot create {}: {error}",
                dir.display()
            ))
        })?;
        let (secs, nanos) = record_time(run, "created_at");
        let path = dir.join(format!("{}-{id}.json", run_file_stamp(secs, nanos)));
        write_json_atomic(&path, run)
    }
}

fn ensure_supported_schema(value: &serde_json::Value, supported: u32) -> Result<(), CliError> {
    let version = value
        .get("schema_version")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    if version > u64::from(supported) {
        return Err(CliError::failed(format!(
            "scheduled_storage_unavailable: record schema v{version} is newer than supported v{supported}"
        )));
    }
    Ok(())
}

fn has_sortable_run_stem(stem: &str) -> bool {
    const RUN_STAMP_LEN: usize = "20260705T142530123Z".len();
    let Some((stamp, rest)) = stem.split_at_checked(RUN_STAMP_LEN) else {
        return false;
    };
    if !rest.starts_with('-') || rest.len() < 2 {
        return false;
    }
    stamp.char_indices().all(|(index, ch)| match index {
        8 => ch == 'T',
        18 => ch == 'Z',
        _ => ch.is_ascii_digit(),
    })
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
    let content = serde_json::to_string_pretty(value).map_err(|error| {
        CliError::failed(format!("scheduled_storage_unavailable: serialize: {error}"))
    })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, content).map_err(|error| {
        CliError::failed(format!(
            "scheduled_storage_unavailable: cannot write {}: {error}",
            tmp.display()
        ))
    })?;
    std::fs::rename(&tmp, path).map_err(|error| {
        CliError::failed(format!(
            "scheduled_storage_unavailable: cannot move {} to {}: {error}",
            tmp.display(),
            path.display()
        ))
    })
}

/// Reads a versioned sidecar registry (model bindings, task kinds, UI
/// metadata, read state, history archive); a missing file is the empty
/// default and an unreadable payload degrades to the default for this
/// process, mirroring the GUI stores' quarantine-then-default behavior.
fn read_registry(path: &Path) -> serde_json::Value {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null),
        Err(_) => serde_json::Value::Null,
    }
}

fn registry_tasks_mut<'a>(
    registry: &'a mut serde_json::Value,
    schema_version: u32,
) -> &'a mut serde_json::Map<String, serde_json::Value> {
    if !registry.is_object() {
        *registry = serde_json::json!({ "schema_version": schema_version, "tasks": {} });
    }
    let object = registry.as_object_mut().expect("registry is object");
    object
        .entry("schema_version")
        .or_insert_with(|| serde_json::json!(schema_version));
    // A wrong-shaped `tasks` value normalizes to the default instead of
    // panicking (same quarantine-then-default behavior as read_registry).
    if !object.get("tasks").is_some_and(Value::is_object) {
        object.insert("tasks".to_owned(), serde_json::json!({}));
    }
    object
        .get_mut("tasks")
        .and_then(Value::as_object_mut)
        .expect("tasks normalized to an object above")
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

fn now_string() -> String {
    let (secs, nanos) = now_epoch();
    format_rfc3339_millis(secs, nanos)
}

/// Random UUIDv4-shaped task/run id (hex, hyphens, safe as a single path
/// component), standing in for the foundation's `Uuid::new_v4`.
fn new_storage_id() -> String {
    let mut bytes = [0u8; 16];
    let random = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut bytes))
        .is_ok();
    if !random {
        let (secs, nanos) = now_epoch();
        let seed = (secs as u128) << 64 | nanos as u128 | u128::from(std::process::id());
        bytes = seed.to_be_bytes();
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[6..8].concat(),
        hex[8..10].concat(),
        hex[10..16].concat()
    )
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
    let mut parts: Vec<(String, String)> = Vec::new();
    for raw in rrule.split(';') {
        if let Some((key, value)) = raw.trim().split_once('=') {
            let key = key.trim().to_ascii_uppercase();
            if let Some(existing) = parts.iter_mut().find(|(name, _)| *name == key) {
                existing.1 = value.trim().to_string();
            } else {
                parts.push((key, value.trim().to_string()));
            }
        }
    }
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
    match parse_byday(byday) {
        Ok(days) if !days.is_empty() => {
            let workdays = ["MO", "TU", "WE", "TH", "FR"];
            let days_label = if days == workdays {
                "workdays".to_owned()
            } else {
                days.join(",")
            };
            format!("{days_label} {label}")
        }
        _ => label.to_owned(),
    }
}

fn unread_and_running(
    def: &serde_json::Value,
    runs: &[serde_json::Value],
    store: &SessionStore,
    read_state: &serde_json::Value,
) -> (bool, bool) {
    let task_id = str_field(def, "id").unwrap_or("");
    let is_running = runs
        .iter()
        .any(|run| matches!(str_field(run, "status").unwrap_or(""), "queued" | "running"));
    let viewed = viewed_runs(read_state, task_id);
    let has_unread = runs.iter().any(|run| {
        str_field(run, "status") == Some("completed")
            && owned_session_id(run, task_id, store).is_some_and(|session_id| {
                !store.is_hidden(&session_id)
                    && !viewed.contains(&str_field(run, "id").unwrap_or(""))
            })
    });
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
    store: &SessionStore,
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

fn map_run(
    run: &serde_json::Value,
    store: &SessionStore,
    titles: &std::collections::HashMap<String, String>,
    read_state: &serde_json::Value,
    task_name: Option<&str>,
    task_model: Option<&str>,
) -> serde_json::Value {
    let task_id = str_field(run, "automation_id").unwrap_or("").to_owned();
    let session_id = owned_session_id_from_snapshot(run, &task_id, store, titles);
    let session_title = session_id
        .as_deref()
        .and_then(|id| titles.get(id))
        .filter(|title| *title != "Scheduled run")
        .cloned();
    let archived = session_id.as_deref().is_some_and(|id| store.is_hidden(id));
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
        "pinned": session_id.as_deref().is_some_and(|id| store.is_pinned(id)),
        "pinnedAt": session_id.as_deref().and_then(|id| store.pinned_at(id)),
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
            let value = map_run(run, store, &titles, read_state, task_name, task_model);
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
            mode,
        } => update(&id, name, prompt_file, rrule, model_id, mode, output),
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
    let read_state = read_registry(&store_holder.read_state_path());
    let bindings = read_registry(&store_holder.model_bindings_path());
    let kinds = read_registry(&store_holder.task_kinds_path());
    let ui_metadata = read_registry(&store_holder.ui_metadata_path());
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
                &sessions,
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
    let read_state = read_registry(&store_holder.read_state_path());
    let bindings = read_registry(&store_holder.model_bindings_path());
    let kinds = read_registry(&store_holder.task_kinds_path());
    let ui_metadata = read_registry(&store_holder.ui_metadata_path());
    let runs = store_holder.list_runs(id, None)?;
    let task = map_task(
        &def,
        &runs,
        &sessions,
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
    let prompt = std::fs::read_to_string(prompt_file).map_err(|error| {
        CliError::failed(format!(
            "scheduled_prompt_file_unreadable: {}: {error}",
            prompt_file.display()
        ))
    })?;
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(CliError::usage(
            "scheduled create requires a non-empty prompt file",
        ));
    }
    if kind == TaskKind::MemoryOrganize && !memory_feature::memory_enabled() {
        return Err(CliError::failed(
            "scheduled_memory_organize_disabled: memory organize tasks require memory to be \
enabled in settings",
        ));
    }
    let _ = mode;
    // The workspace is allocated from the automation id exactly like the GUI
    // (`ensure_automation_workspace`); clients cannot provide a path.
    let id = new_storage_id();
    let now = now_string();
    let workspace = store_holder.workspace_dir(&id);
    std::fs::create_dir_all(&workspace).map_err(|error| {
        CliError::failed(format!(
            "scheduled_workspace_unavailable: cannot create {}: {error}",
            workspace.display()
        ))
    })?;
    let (secs, nanos) = now_epoch();
    let created_at = format_rfc3339_millis(secs, nanos);
    // The foundation's scheduler sweep fills next_run_at on its next tick;
    // see the module docs for the deferred-schedule deviation.
    let def = serde_json::json!({
        "schema_version": 2,
        "id": id,
        "name": name,
        "prompt": prompt,
        "rrule": rrule.trim().to_ascii_uppercase(),
        "cwds": [workspace.display().to_string()],
        "model": default_automation_model(),
        "mode": TaskMode::PERSISTED,
        "allow_shell": current_allow_shell(),
        "trust_mode": true,
        "auto_approve": true,
        "status": if paused { "paused" } else { "active" },
        "created_at": created_at,
        "updated_at": now,
        "next_run_at": serde_json::Value::Null,
        "last_run_at": serde_json::Value::Null,
    });
    store_holder.write_def(&def)?;
    if let Err(error) = persist_model_binding(&store_holder, &id, model_id.as_deref()) {
        // Roll back the just-created task so no kind-less/binding-less task
        // lingers, mirroring the GUI create rollback.
        if let Ok(path) = store_holder.def_path(&id) {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir_all(store_holder.workspace_dir(&id));
        return Err(error);
    }
    if let Some(stored_kind) = kind.stored_kind() {
        if let Err(error) = persist_task_kind(&store_holder, &id, Some(stored_kind)) {
            if let Ok(path) = store_holder.def_path(&id) {
                let _ = std::fs::remove_file(path);
            }
            let _ = std::fs::remove_dir_all(store_holder.workspace_dir(&id));
            return Err(error);
        }
    }
    let sessions = open_sessions()?;
    let value = map_task(
        &def,
        &[],
        &sessions,
        &serde_json::Value::Null,
        &read_registry(&store_holder.model_bindings_path()),
        &read_registry(&store_holder.task_kinds_path()),
        &read_registry(&store_holder.ui_metadata_path()),
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
    mode: Option<TaskMode>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let store_holder = TaskStore::new()?;
    let mut def = store_holder.read_def(id)?;
    let mut schedule_changed = false;
    if let Some(name) = name {
        let name = name.trim();
        if name.is_empty() {
            return Err(CliError::usage(
                "scheduled update requires a non-empty --name",
            ));
        }
        def["name"] = serde_json::json!(name);
    }
    if let Some(prompt_file) = prompt_file {
        let prompt = std::fs::read_to_string(&prompt_file).map_err(|error| {
            CliError::failed(format!(
                "scheduled_prompt_file_unreadable: {}: {error}",
                prompt_file.display()
            ))
        })?;
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(CliError::usage(
                "scheduled update requires a non-empty prompt file",
            ));
        }
        def["prompt"] = serde_json::json!(prompt);
    }
    if let Some(rrule) = rrule {
        def["rrule"] = serde_json::json!(rrule.trim().to_ascii_uppercase());
        schedule_changed = true;
    }
    let _ = mode;
    if schedule_changed {
        // Active or paused, the next slot is recomputed by the foundation
        // scheduler sweep (paused tasks keep it unset, like the GUI).
        def["next_run_at"] = serde_json::Value::Null;
    }
    def["updated_at"] = serde_json::json!(now_string());
    ensure_workspace(&store_holder, &mut def)?;
    store_holder.write_def(&def)?;
    if model_id.is_some() {
        persist_model_binding(&store_holder, id, model_id.as_deref())?;
    }
    let sessions = open_sessions()?;
    let runs = store_holder.list_runs(id, None)?;
    let value = map_task(
        &def,
        &runs,
        &sessions,
        &read_registry(&store_holder.read_state_path()),
        &read_registry(&store_holder.model_bindings_path()),
        &read_registry(&store_holder.task_kinds_path()),
        &read_registry(&store_holder.ui_metadata_path()),
    );
    Ok(success(render(
        output,
        format!("Updated scheduled task: {id}"),
        &value,
    )))
}

/// Mirrors `ensure_automation_workspace`: the durable execution workspace is
/// derived from the task id and persisted as the single cwd entry.
fn ensure_workspace(store_holder: &TaskStore, def: &mut serde_json::Value) -> Result<(), CliError> {
    let id = str_field(def, "id").unwrap_or("").to_owned();
    let workspace = store_holder.workspace_dir(&id);
    std::fs::create_dir_all(&workspace).map_err(|error| {
        CliError::failed(format!(
            "scheduled_workspace_unavailable: cannot create {}: {error}",
            workspace.display()
        ))
    })?;
    def["cwds"] = serde_json::json!([workspace.display().to_string()]);
    Ok(())
}

fn persist_model_binding(
    store_holder: &TaskStore,
    id: &str,
    model_id: Option<&str>,
) -> Result<(), CliError> {
    let model_id = model_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let mut registry = read_registry(&store_holder.model_bindings_path());
    let tasks = registry_tasks_mut(&mut registry, 1);
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
    let mut registry = read_registry(&store_holder.task_kinds_path());
    let tasks = registry_tasks_mut(&mut registry, 1);
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
    let mut def = store_holder.read_def(id)?;
    let action = if pause { "paused" } else { "resumed" };
    def["status"] = serde_json::json!(if pause { "paused" } else { "active" });
    // Both branches clear the next slot: pause must not fire, and resume lets
    // the foundation scheduler sweep recompute it in the local timezone.
    def["next_run_at"] = serde_json::Value::Null;
    def["updated_at"] = serde_json::json!(now_string());
    if !pause {
        ensure_workspace(&store_holder, &mut def)?;
    }
    store_holder.write_def(&def)?;
    let sessions = open_sessions()?;
    let runs = store_holder.list_runs(id, None)?;
    let value = map_task(
        &def,
        &runs,
        &sessions,
        &read_registry(&store_holder.read_state_path()),
        &read_registry(&store_holder.model_bindings_path()),
        &read_registry(&store_holder.task_kinds_path()),
        &read_registry(&store_holder.ui_metadata_path()),
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
    let mut registry = read_registry(&store_holder.ui_metadata_path());
    let tasks = registry_tasks_mut(&mut registry, 1);
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
    let def = store_holder.read_def(id)?;
    let runs = store_holder.list_runs(id, None)?;
    // The GUI cancels queued/running runs through the foundation TaskManager
    // before deleting; headlessly there is no engine runtime to cancel with,
    // so deletion refuses instead of stranding an active run.
    if let Some(active) = runs
        .iter()
        .find(|run| matches!(str_field(run, "status").unwrap_or(""), "queued" | "running"))
    {
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
    let mut archive = read_registry(&store_holder.history_archive_path());
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
    let existing = archive
        .get("tasks")
        .and_then(|value| value.as_object())
        .expect("tasks normalized to an object above");
    {
        let mut merged = existing.clone();
        if let Some(new_tasks) = snapshot["tasks"].as_object() {
            for (key, value) in new_tasks {
                merged.insert(key.clone(), value.clone());
            }
        }
        archive["tasks"] = serde_json::Value::Object(merged);
    }
    write_json_atomic(&store_holder.history_archive_path(), &archive)?;
    let def_path = store_holder.def_path(id)?;
    match std::fs::remove_file(&def_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CliError::failed(format!(
                "scheduled_delete_failed: cannot remove {}: {error}",
                def_path.display()
            )));
        }
    }
    let runs_dir = store_holder.runs_dir_for(id)?;
    if runs_dir.exists() {
        std::fs::remove_dir_all(&runs_dir).map_err(|error| {
            CliError::failed(format!(
                "scheduled_delete_failed: cannot remove {}: {error}",
                runs_dir.display()
            ))
        })?;
    }
    for path in [
        store_holder.model_bindings_path(),
        store_holder.task_kinds_path(),
        store_holder.ui_metadata_path(),
    ] {
        let mut registry = read_registry(&path);
        if registry.is_null() {
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
    let sessions = open_sessions()?;
    let task = map_task(
        &def,
        &runs,
        &sessions,
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
    let kind = kind_for(&read_registry(&store_holder.task_kinds_path()), id);
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
    // Run-record bookkeeping first: the run exists on disk before the work,
    // mirroring run_now_shared's persist-then-execute ordering.
    let run_id = new_storage_id();
    let (secs, nanos) = now_epoch();
    let now = format_rfc3339_millis(secs, nanos);
    let mut record = serde_json::json!({
        "schema_version": 1,
        "id": run_id,
        "automation_id": id,
        "scheduled_for": now,
        "status": "queued",
        "created_at": now,
        "started_at": serde_json::Value::Null,
        "ended_at": serde_json::Value::Null,
        "task_id": serde_json::Value::Null,
        "thread_id": serde_json::Value::Null,
        "turn_id": serde_json::Value::Null,
        "error": serde_json::Value::Null,
    });
    store_holder.save_run(&record)?;
    let organize = organize_headless();
    let (status, error) = match &organize {
        Ok(_) => ("completed", serde_json::Value::Null),
        Err(error) => ("failed", serde_json::json!(error)),
    };
    record["status"] = serde_json::json!(status);
    record["started_at"] = serde_json::json!(now);
    record["ended_at"] = serde_json::json!(now_string());
    record["error"] = error;
    store_holder.save_run(&record).map_err(|error| {
        // Without the terminal record the run would stay queued forever and
        // `scheduled delete` refuses queued runs — an unrecoverable task.
        // Surface a message that names the remedy instead.
        CliError::failed(format!(
            "scheduled_run_unterminated: the run finished but its terminal record could not be \
             written ({error}); the task will refuse deletion until the record is repaired or \
             removed manually at {}",
            store_holder
                .runs_dir_for(id)
                .map(|dir| dir.display().to_string())
                .unwrap_or_default()
        ))
    })?;
    if let Ok(mut latest) = store_holder.read_def(id) {
        latest["updated_at"] = serde_json::json!(now_string());
        latest["last_run_at"] = record["ended_at"].clone();
        let _ = store_holder.write_def(&latest);
    }
    let sessions = open_sessions()?;
    let value = map_run(
        &record,
        &sessions,
        &session_titles(&sessions),
        &read_registry(&store_holder.read_state_path()),
        str_field(&def, "name"),
        str_field(&def, "model"),
    );
    let human = format!(
        "Run: {}\nTask: {}\nSession: -\nStatus: {}",
        run_id, id, status
    );
    Ok(success(render(output, human, &value)))
}

/// One memory-organize pass through the windowless product host, the same
/// wiring as `memory organize` in this crate and the GUI scheduled executor's
/// shared-bridge fallback: requires a display and a configured active model.
fn organize_headless() -> Result<(), String> {
    pinvou3_lib::headless_bridge::run_windowless_host(|pool, _store| async move {
        let mut bridge = pool.bridge.clone();
        bridge.prefs = UserPrefs::load();
        bridge.session_model = None;
        memory_feature::organize_memory_with_llm(&bridge, None)
            .await
            .map(|_| ())
            .map_err(|error| {
                anyhow::anyhow!(
                    "{}",
                    pinvou3_lib::platform::credential_store::redact_secret(&format!("{error:#}"))
                )
            })
    })
    .map_err(|error| pinvou3_lib::platform::credential_store::redact_secret(&format!("{error:#}")))
}

fn runs(id: &str, limit: Option<usize>, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store_holder = TaskStore::new()?;
    let def = store_holder.read_def(id)?;
    let sessions = open_sessions()?;
    let read_state = read_registry(&store_holder.read_state_path());
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
    let read_state = read_registry(&store_holder.read_state_path());
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
    let archive = read_registry(&store_holder.history_archive_path());
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
    // Active runs first, then the deleted-task history archive, exactly like
    // the GUI mark command's lookup order.
    let runs = match store_holder.list_runs(task_id, None) {
        Ok(runs) => runs,
        Err(error) => {
            let archive = read_registry(&store_holder.history_archive_path());
            let archived = archive
                .get("tasks")
                .and_then(|value| value.get(task_id))
                .and_then(|task| task.get("runs"))
                .and_then(|value| value.as_array())
                .cloned()
                .ok_or_else(|| error)?;
            archived
        }
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
    let mut read_state = read_registry(&store_holder.read_state_path());
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
        let refreshed = read_registry(&store_holder.read_state_path());
        let def = serde_json::json!({ "id": task_id });
        unread_and_running(&def, &runs, &sessions, &refreshed)
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
            other => panic!("unexpected command: {other:?}"),
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
                "plan",
                "--paused",
            ])
            .unwrap(),
            ScheduledCommand::Create {
                name: "Report".into(),
                prompt_file: PathBuf::from("prompt.md"),
                rrule: "FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30".into(),
                kind: TaskKind::MemoryOrganize,
                model_id: Some("m-1".into()),
                mode: Some(TaskMode::Plan),
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
                mode: None,
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

    #[test]
    fn rrule_validation_mirrors_the_foundation_grammar() {
        for valid in [
            "FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
            "freq=hourly",
            "FREQ=HOURLY;BYDAY=MO,WE;INTERVAL=2",
            "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30",
            "FREQ=ONCE;AT=2026-09-10T08:30",
            "FREQ=ONCE;AT=2026-09-10T08:30:00Z",
            "FREQ=ONCE;AT=2026-09-10T08:30:00+02:00",
            "FREQ=CRON;EXPR=30 8 * * MON-FRI",
            "FREQ=CRON;EXPR=*/15 0 * JAN,DEC SUN",
        ] {
            if let Err(error) = validate_rrule(valid) {
                panic!("rejected valid rrule '{valid}': {error}");
            }
        }
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
            "FREQ=ONCE;AT=2026-09-10T08:30:00;BYDAY=MO",
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
    fn wall_clock_helpers_round_trip_utc_timestamps() {
        // 2026-09-10T12:34:56.123Z == epoch 1789118096.123
        let (secs, nanos) = parse_rfc3339("2026-09-10T12:34:56.123Z").unwrap();
        assert_eq!(
            format_rfc3339_millis(secs, nanos),
            "2026-09-10T12:34:56.123Z"
        );
        assert_eq!(run_file_stamp(secs, nanos), "20260910T123456123Z");
        // Offset normalization: +02:00 means the UTC instant is two hours earlier.
        let (offset_secs, _) = parse_rfc3339("2026-09-10T14:34:56+02:00").unwrap();
        assert_eq!(offset_secs, secs);
        // Sort ordering: later instants compare greater.
        let later = parse_rfc3339("2026-09-10T12:34:57Z").unwrap();
        assert!(later > (secs, nanos));
        assert!(parse_rfc3339("not-a-date").is_none());
        assert!(parse_rfc3339("2026-09-10 12:34:56").is_none());
    }

    #[test]
    fn schedule_labels_stay_english_and_sorted_run_stems_match_the_foundation() {
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
            humanize_rrule("FREQ=ONCE;AT=2026-09-10T08:30"),
            "FREQ=ONCE;AT=2026-09-10T08:30"
        );
        assert!(has_sortable_run_stem("20260910T123456123Z-run-1"));
        assert!(!has_sortable_run_stem("run-1"));
        assert!(!has_sortable_run_stem("20260910T123456123-run-1"));
        // Storage ids must be single path components (the foundation's
        // ensure_safe_storage_id rule).
        assert!(safe_storage_id("task id", "abc-def").is_ok());
        assert!(safe_storage_id("task id", "../escape").is_err());
        assert!(safe_storage_id("task id", "").is_err());
    }
}
