// architecture-guard: allow-target-cfg -- Unix filesystem boundary tests plant symlinks/FIFOs; the Linux-only invalid-UTF-8 filename fixture avoids Darwin filesystems that reject those names at creation. These OS fixtures cannot compile or run portably.
//! Scheduled-task request watcher — the app-side consumer of the
//! app-automations MCP family's create/update/delete spool
//! (docs/app-automations-定时任务创建工具-设计与验收.md + its CRUD addendum;
//! docs/builtin-toolset-contract.md §5 L1 / §6).
//!
//! The app-automations MCP server validates a `create_scheduled_task` /
//! `update_scheduled_task` / `delete_scheduled_task` call and spools it to
//! `<pinvou3 home>/task-requests/spool/<name>.json` (see that server.py's
//! `schedule_task_request` — the spool record schema is the contract between
//! the two sides; the file name is the idempotency identity: the sha256 of
//! `"<from_session>|<kind>|<task_id>|<idempotency_key>"` when a key is given (a
//! key requires `from_session`, so the namespace is never global), a
//! random uuid otherwise, so a retried operation replaces its own pending
//! record and can never clobber another session's nor another kind's). This
//! module is the app-side consumer:
//!
//! - a poll watcher picks spool files up, re-validates them (server-side
//!   checks are not trusted — the spool directory is user-writable), and
//!   dispatches by the record's `kind` to the panel's own domain function
//!   ([`ScheduledTaskState::create_task`] / `update_task` / `delete_task`:
//!   forced YOLO, per-task workspace, model sidecar, archive-then-delete —
//!   the exact pipelines the panel uses);
//! - a processed request leaves a result marker
//!   `spool/.done/<file-stem>.json` (`{"ok":true|false,...}`) so the MCP
//!   server's short synchronous wait can return the outcome. Delivery is
//!   **at-least-once**, not exactly-once: a success marker suppresses a
//!   replayed request (so a retried tool call cannot re-apply after the
//!   marker landed), and a failure marker lets the retry re-apply once the
//!   cause is fixed — but a crash between applying an operation and writing
//!   its marker can still re-apply it on the next boot (create can then
//!   duplicate; updates and ensure-absent deletes replay idempotently).
//!   Prompt/rrule replacements require panel submission. This is the same
//!   accepted window as features/messaging; it is bounded by the marker
//!   write happening directly after the apply;
//! - poison files (schema drift, hostile content, oversize) are quarantined
//!   under `spool/failed/` immediately with a `{"ok":false,"error"}`
//!   marker; *transient* failures retry up to [`MAX_CREATE_ATTEMPTS`] times
//!   and then quarantine with the same marker, so a waiting caller always
//!   receives a terminal outcome instead of hanging. Marker and quarantine
//!   state older than [`STATE_RETENTION`] is pruned (`.done/` and `failed/`
//!   would otherwise grow forever; stray `*.tmp` crash leftovers are swept
//!   with the same pass);
//! - every success appends a kind-specific audit record into the
//!   requesting session's audit root — the execution root for unbound
//!   sessions, the PRIVATE ledger root for bound ones (round-24 M4; the
//!   contract §5 L1 provenance note is unchanged: model-supplied
//!   `from_session` is the same unauthenticated provenance as messaging's
//!   — the Ask rules and the audit trail are the trust boundary) — and
//!   emits the panel refresh
//!   event `scheduled_task:run_updated` (same channel the file watcher uses;
//!   both frontends only refresh from the event name).
//!
//! The watcher is started once from [`ScheduledTaskState::boot_runtime`] and
//! cancelled from `ScheduledTaskState::Drop` via [`CreationWatchGuard`]. It
//! must be spawned through `tauri::async_runtime::spawn` (not `tokio::spawn`):
//! `boot_runtime` runs inside the tauri setup hook, outside any raw tokio
//! context (a6d135840 lesson).
//!
//! Dependency direction: this module is a child of `tasks`, so it reuses the
//! private domain entry points without widening them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::{
    CreateScheduledTaskInput, ScheduledTaskDto, ScheduledTaskState, UpdateScheduledTaskInput,
};
use crate::features::assistant::platform::bridge::{
    SCHEDULED_TASK_CREATE_TOOL, SCHEDULED_TASK_DELETE_TOOL, SCHEDULED_TASK_UPDATE_TOOL,
};
use crate::features::sessions::validators::is_aux_session_id;

/// Same bounds as the MCP server's caps — re-checked here because the spool
/// directory is user-writable.
const MAX_NAME_CHARS: usize = 200;
const MAX_PROMPT_CHARS: usize = 32 * 1024;
const MAX_MODEL_ID_CHARS: usize = 200;
const MAX_RRULE_CHARS: usize = 256;
const MAX_IDEMPOTENCY_KEY_CHARS: usize = 128;
const MAX_SESSION_ID_LEN: usize = 128;
const MAX_TASK_ID_LEN: usize = 128;
/// Spool file size cap: a legitimate record is bounded by the 32k-char prompt
/// (~96 KB as UTF-8 CJK) plus small metadata; anything bigger is hostile and
/// is quarantined before being read into memory.
const MAX_SPOOL_FILE_BYTES: u64 = 256 * 1024;
/// Transient creation failures retry on consecutive polls; quarantine only
/// after this many attempts (design C4: quarantine + failure marker).
const MAX_CREATE_ATTEMPTS: u32 = 3;
/// Round-27 minor 9: pending-file ceiling per drain pass (the messaging
/// sibling's 256; only the bounded batch's tail quarantines before processing).
const MAX_PENDING_FILES: usize = 256;
const MAX_TAIL_QUARANTINES: usize = 32;
/// Round-31 minor 5: per-pass poison-warn budget (detail for the first
/// few, one summary beyond — the failed/ evidence carries every record).
const POISON_WARN_PER_PASS: usize = 8;
/// Watch poll interval: task creations are rare; the MCP server's synchronous
/// wait covers up to 5s, so 1s keeps the typical create inside one poll.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Retention for terminal state: result markers are the cross-restart
/// idempotency window and `failed/` holds quarantine evidence, so keep a
/// bounded history (same shape as messaging's state pruning) instead of
/// letting `.done/` + `failed/` grow forever.
const STATE_RETENTION: Duration = Duration::from_secs(14 * 24 * 3600);
/// How often the watcher sweeps stale terminal state and stray `*.tmp`
/// crash leftovers (cheap enough at this rate, rare enough to be free).
const PRUNE_INTERVAL: Duration = Duration::from_secs(60);
/// Rotate the long-lived shadow audit at 2 MiB and keep one prior segment.
/// Checking before each append permits at most one line of normal overshoot.
const SHADOW_AUDIT_ROTATE_BYTES: u64 = 2 * 1024 * 1024;

/// rrule product subset (deliberately stricter than the domain parser, same
/// rules as the MCP server's `validate_rrule`): HOURLY/WEEKLY/ONCE only —
/// CRON and minute-granular frequencies are a product-level rejection, not a
/// parse error.
const WEEKDAY_TOKENS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

/// Guard for the creation-request watcher task: held in a
/// `Arc<SyncMutex<Option<_>>>` slot on [`ScheduledTaskState`] so the state can
/// derive `Clone` while the watcher keeps a single owner. Drop only cancels
/// the token — the loop observes it between passes and exits on its own
/// (graceful drain): aborting the task could interrupt an apply between
/// persisting the operation and writing its result marker, and that
/// apply→marker gap is exactly the at-least-once replay window.
pub(crate) struct CreationWatchGuard {
    cancel: CancellationToken,
}

impl Drop for CreationWatchGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Which domain operation a spool record asks for (`kind` on disk). Old
/// create-only records predate the field and default to [`SpoolRequestKind::Create`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpoolRequestKind {
    #[default]
    Create,
    Update,
    Delete,
}

impl SpoolRequestKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SpoolRequestKind::Create => "create",
            SpoolRequestKind::Update => "update",
            SpoolRequestKind::Delete => "delete",
        }
    }
}

/// One spooled task request (create / update / delete). Field names mirror
/// server.py's `_spool_payload` exactly (snake_case JSON on disk); unknown
/// fields are skipped on read (contract §4.4 drift defense). The `id` field
/// is informational only — the watcher keys its state on the directory-listed
/// file name, never on this field (a user-writable spool must not control
/// watcher paths). Payload fields are optional because update supplies only
/// the changed subset and delete supplies none; `validate` enforces the
/// per-kind requirements.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct SpooledCreationRequest {
    pub schema_version: u32,
    #[serde(default)]
    pub kind: SpoolRequestKind,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub rrule: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    /// `session_message` mode marker: when present, the task delivers its
    /// prompt into this session at each fire instead of starting new
    /// conversations.
    #[serde(default)]
    pub target_session: Option<String>,
    #[serde(default)]
    pub paused: Option<bool>,
    #[serde(default)]
    pub from_session: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

fn check_sender_session_id(session_id: Option<&String>) -> Result<()> {
    let Some(id) = session_id else {
        return Ok(());
    };
    if id.is_empty()
        || id.len() > MAX_SESSION_ID_LEN
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("invalid from_session id");
    }
    // from_session is unauthenticated audit provenance, not runtime posture.
    // A sched- conversation can be continued interactively; actual unattended
    // writes are denied by the run engine, even when this claim is omitted.
    // Auxiliary, benchmark, and headless origins remain isolated.
    // Round-15 REQUIRED-1: the eval_ leg folds case like the server's
    // `.lower()` gate — a planted `EVAL_b1` sender used to pass the
    // watcher while the server rejected it, and on case-insensitive
    // filesystems `session_roots("EVAL_b1")` aliases the real benchmark
    // session's audit root.
    // Headless `agentic_` runs join the isolated set (the server's
    // ISOLATED_SENDER_PREFIXES shape): a prompt-injected headless run must
    // not request task operations (case-folded like the eval_ leg).
    if is_aux_session_id(id)
        || id.to_ascii_lowercase().starts_with("eval_")
        || id.to_ascii_lowercase().starts_with("agentic_")
    {
        bail!("from_session {id} is an isolated session and cannot request task operations");
    }
    Ok(())
}

/// Charset + length validation for a target task id (update/delete): the id
/// flows back into storage paths on the domain side, so the same
/// anti-traversal discipline as session ids applies.
fn check_task_id(task_id: &str) -> Result<()> {
    if task_id.is_empty()
        || task_id.len() > MAX_TASK_ID_LEN
        || !task_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("invalid target task_id");
    }
    Ok(())
}

fn check_optional_field(value: &Option<String>, label: &str, max_chars: usize) -> Result<()> {
    if let Some(value) = value {
        let value = value.trim();
        if value.is_empty() {
            bail!("{label} is blank");
        }
        if value.chars().count() > max_chars {
            bail!("{label} exceeds the {max_chars} character limit");
        }
    }
    Ok(())
}

impl SpooledCreationRequest {
    /// Server-side re-validation of a spool record, per kind (contract §4.4:
    /// errors are explicit; §5: the L1 write re-checks everything it was
    /// told). Mirrors every MCP-server rule that is checkable without live
    /// store access (shape, caps, charsets, isolation prefixes); the live
    /// probes (task exists, target session exists) stay with the domain
    /// layer; explicit missing-target errors poison on attempt 1. The
    /// exact-name probe does not parse transcripts; runtime reads report corruption.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            bail!("unsupported spool schema_version {}", self.schema_version);
        }
        match self.kind {
            SpoolRequestKind::Create => {
                let name = self.name.as_deref().map(str::trim).unwrap_or("");
                if name.is_empty() {
                    bail!("task name is empty");
                }
                let prompt = self.prompt.as_deref().map(str::trim).unwrap_or("");
                if prompt.is_empty() {
                    bail!("task prompt is empty");
                }
                if self.task_id.is_some() {
                    bail!("create requests must not target an existing task");
                }
            }
            SpoolRequestKind::Update => {
                let task_id = self.task_id.as_deref().map(str::trim).unwrap_or("");
                check_task_id(task_id)?;
                if self.name.is_none()
                    && self.prompt.is_none()
                    && self.rrule.is_none()
                    && self.model_id.is_none()
                    && self.paused.is_none()
                    && self.target_session.is_none()
                {
                    bail!("update must provide at least one field to change");
                }
            }
            SpoolRequestKind::Delete => {
                let task_id = self.task_id.as_deref().map(str::trim).unwrap_or("");
                check_task_id(task_id)?;
                for (label, field) in [
                    ("name", &self.name),
                    ("prompt", &self.prompt),
                    ("rrule", &self.rrule),
                    ("model_id", &self.model_id),
                    ("target_session", &self.target_session),
                ] {
                    if field
                        .as_deref()
                        .map(str::trim)
                        .is_some_and(|v| !v.is_empty())
                    {
                        bail!("delete takes no extra fields ({label})");
                    }
                }
                if self.paused.is_some() {
                    bail!("delete takes no extra fields (paused)");
                }
            }
        }
        check_optional_field(&self.name, "task name", MAX_NAME_CHARS)?;
        check_optional_field(&self.prompt, "task prompt", MAX_PROMPT_CHARS)?;
        check_optional_field(&self.model_id, "model_id", MAX_MODEL_ID_CHARS)?;
        if let Some(key) = &self.idempotency_key {
            if key.trim().is_empty() {
                bail!("idempotency_key is blank");
            }
            if key.chars().count() > MAX_IDEMPOTENCY_KEY_CHARS {
                bail!("idempotency_key exceeds the {MAX_IDEMPOTENCY_KEY_CHARS} character limit");
            }
            if self.from_session.is_none() {
                // Without a sender the key's namespace would degrade to
                // global: two unattributed senders reusing one key would
                // clobber each other's pending request (mirrors the MCP
                // server's validation).
                bail!("idempotency_key requires from_session so the key is scoped to one sender");
            }
        }
        check_sender_session_id(self.from_session.as_ref())?;
        if let Some(rrule) = &self.rrule {
            validate_product_rrule(rrule)?;
        } else if self.kind == SpoolRequestKind::Create {
            bail!("rrule is empty");
        }
        check_optional_field(&self.target_session, "target_session", MAX_TASK_ID_LEN)?;
        if let Some(target) = self
            .target_session
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            // Same charset the server enforces: a hostile or padded target
            // must poison immediately, not burn three transient retries
            // inside the domain's own session lookup.
            crate::features::scheduled::message::validate_target_identity(target)
                .map_err(|error| anyhow::anyhow!("invalid target_session: {error}"))?;
        }
        Ok(())
    }
}

/// Validates one rrule against the product subset and returns the normalized
/// (trim + uppercase) form. Mirrors the MCP server's `validate_rrule` — the
/// watcher must reject exactly what the server rejects (B9: the spool
/// directory is user-writable, server-side validation is not trusted).
pub(crate) fn validate_product_rrule(rrule: &str) -> Result<String> {
    let normalized = rrule.trim().to_ascii_uppercase();
    if normalized.is_empty() {
        bail!("rrule is empty");
    }
    if normalized.chars().count() > MAX_RRULE_CHARS {
        bail!("rrule exceeds the {MAX_RRULE_CHARS} character limit");
    }
    let mut parts: Vec<(String, String)> = Vec::new();
    for raw in normalized.split(';') {
        let item = raw.trim();
        if item.is_empty() {
            continue;
        }
        let Some((key, value)) = item.split_once('=') else {
            bail!("invalid rrule segment '{item}'");
        };
        let key = key.trim().to_string();
        if parts.iter().any(|(existing, _)| *existing == key) {
            // Duplicate keys are ambiguous (the server's dict is last-wins,
            // this lookup is first-wins): reject instead of silently picking
            // one side's schedule (mirrors the MCP server's validation).
            bail!("invalid rrule: duplicate field '{key}'");
        }
        parts.push((key, value.trim().to_string()));
    }
    let get = |key: &str| {
        parts
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    let freq = get("FREQ").ok_or_else(|| anyhow::anyhow!("rrule must include FREQ"))?;
    match freq {
        "MINUTELY" | "SECONDLY" | "DAILY" => {
            bail!("minute-granular or daily FREQ '{freq}' is not supported by the product subset")
        }
        "CRON" => bail!("CRON rrule is not supported by the product subset"),
        "HOURLY" | "WEEKLY" | "ONCE" => {}
        other => bail!("unsupported FREQ '{other}'"),
    }
    let allowed: &[&str] = match freq {
        "HOURLY" => &["FREQ", "INTERVAL", "BYDAY", "BYHOUR", "BYMINUTE"],
        "WEEKLY" => &["FREQ", "BYDAY", "BYHOUR", "BYMINUTE"],
        _ => &["FREQ", "AT"],
    };
    for (key, _) in &parts {
        if !allowed.contains(&key.as_str()) {
            bail!("unsupported rrule field '{key}' for FREQ={freq}");
        }
    }
    let parse_int = |key: &str, value: &str, minimum: u32, maximum: u32| -> Result<u32> {
        // Round-27 minor 5: `u32::from_str` accepts a leading '+' — the
        // server's isdigit check rejects it, and the module's own B9 rule
        // is "reject exactly what the server rejects".
        if !value.bytes().all(|b| b.is_ascii_digit()) {
            bail!("invalid rrule: {key} must be an integer, got '{value}'");
        }
        let Ok(number) = value.parse::<u32>() else {
            bail!("invalid rrule: {key} must be an integer, got '{value}'");
        };
        if number < minimum || number > maximum {
            bail!("invalid rrule: {key} must be between {minimum} and {maximum}");
        }
        Ok(number)
    };
    let parse_byday = |value: &str| -> Result<()> {
        // Round-30 REQUIRED-1: an empty token (trailing/double comma) is
        // INVALID, not skippable — the domain parser bails on "", and the
        // module's B9 rule is to reject exactly what the server rejects
        // (which now rejects it too).
        let tokens: Vec<&str> = value.split(',').map(str::trim).collect();
        if tokens.iter().any(|t| t.is_empty()) {
            bail!("invalid rrule: BYDAY cannot be empty");
        }
        for token in tokens {
            if !WEEKDAY_TOKENS.contains(&token) {
                bail!("invalid rrule: BYDAY token '{token}' is not a weekday");
            }
        }
        Ok(())
    };
    match freq {
        "HOURLY" => {
            if let Some(interval) = get("INTERVAL") {
                parse_int("INTERVAL", interval, 1, 24 * 30)?;
            }
            if let Some(byhour) = get("BYHOUR") {
                parse_int("BYHOUR", byhour, 0, 23)?;
            }
            if let Some(byminute) = get("BYMINUTE") {
                parse_int("BYMINUTE", byminute, 0, 59)?;
            }
            if let Some(byday) = get("BYDAY") {
                parse_byday(byday)?;
            }
        }
        "WEEKLY" => {
            let Some(byday) = get("BYDAY") else {
                bail!("invalid rrule: FREQ=WEEKLY requires BYDAY");
            };
            parse_byday(byday)?;
            let Some(byhour) = get("BYHOUR") else {
                bail!("invalid rrule: FREQ=WEEKLY requires BYHOUR");
            };
            parse_int("BYHOUR", byhour, 0, 23)?;
            let Some(byminute) = get("BYMINUTE") else {
                bail!("invalid rrule: FREQ=WEEKLY requires BYMINUTE");
            };
            parse_int("BYMINUTE", byminute, 0, 59)?;
        }
        _ => {
            // ONCE: exactly the local wall-clock form YYYY-MM-DDTHH:MM — no
            // seconds, no 'Z', no UTC offset — and strictly in the future
            // (the domain would also reject a past moment at parse time; the
            // watcher rejects earlier so the failure lands in the marker).
            let Some(at) = get("AT") else {
                bail!("invalid rrule: FREQ=ONCE requires AT");
            };
            let bytes = at.as_bytes();
            let shape_ok = bytes.len() == 16
                && bytes[4] == b'-'
                && bytes[7] == b'-'
                && bytes[10] == b'T'
                && bytes[13] == b':'
                && bytes.iter().enumerate().all(|(index, byte)| {
                    (index == 4 || index == 7 || index == 10 || index == 13)
                        || byte.is_ascii_digit()
                });
            if !shape_ok {
                bail!(
                    "invalid rrule: ONCE AT must be the local time YYYY-MM-DDTHH:MM without a timezone suffix"
                );
            }
            let year: i32 = at[0..4]
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid rrule: ONCE AT year"))?;
            let month: u32 = at[5..7]
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid rrule: ONCE AT month"))?;
            let day: u32 = at[8..10]
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid rrule: ONCE AT day"))?;
            let hour: u32 = at[11..13]
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid rrule: ONCE AT hour"))?;
            let minute: u32 = at[14..16]
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid rrule: ONCE AT minute"))?;
            use chrono::TimeZone;
            // DST gap/ambiguous parity with the server is pinned on the
            // PYTHON side (validate_rrule + the gap/ambiguous helper with
            // its committed wall times); the watcher stays the runtime
            // authority — `.single()` refusing both shapes here is the
            // same decision, stated but not separately TZ-tested (a Rust
            // TZ test would need the IANA db the platform may lack).
            let local = chrono::Local
                .with_ymd_and_hms(year, month, day, hour, minute, 0)
                .single()
                .ok_or_else(|| {
                    anyhow::anyhow!("invalid rrule: ONCE AT '{at}' is not a real local minute")
                })?;
            if local <= chrono::Local::now() {
                bail!("invalid rrule: ONCE AT '{at}' is not in the future");
            }
        }
    }
    Ok(normalized)
}

fn spool_root() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("task-requests")
        .join("spool")
}

/// Resolve every app-owned component relative to a stable real directory.
/// Never let a planted spool/audit child redirect reads or mutations.
fn private_directory(
    path: &Path,
) -> std::io::Result<crate::platform::filesystem::PrivateFileDirectory> {
    let home = crate::platform::paths::pinvou3_home();
    let relative = path.strip_prefix(&home).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "request directory outside app home",
        )
    })?;
    let mut directory = crate::platform::filesystem::open_private_file_directory(&home)?;
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid request directory",
            ));
        };
        directory = directory.create_private_child_directory(name)?;
    }
    Ok(directory)
}

fn shadow_directory() -> std::io::Result<crate::platform::filesystem::PrivateFileDirectory> {
    let root = private_directory(&super::scheduled_automation_root())?;
    match root.create_private_child_directory(std::ffi::OsStr::new("audit")) {
        Ok(directory) => Ok(directory),
        Err(original) => {
            // Preserve a regular-file obstruction, never follow a link or
            // remove somebody's data. The next boot/append can recover too.
            if root
                .open_plain_file(std::ffi::OsStr::new("audit"))?
                .is_none()
            {
                return Err(original);
            }
            let backup = format!("audit-obstruction-{}", uuid::Uuid::new_v4());
            root.move_plain_file_to(
                std::ffi::OsStr::new("audit"),
                &root,
                std::ffi::OsStr::new(&backup),
            )?;
            root.create_private_child_directory(std::ffi::OsStr::new("audit"))
        }
    }
}

pub(crate) fn prepare_request_directories() -> Result<()> {
    private_directory(&spool_root())?;
    private_directory(&done_dir())?;
    private_directory(&failed_dir())?;
    shadow_directory()?;
    Ok(())
}

fn failed_dir() -> PathBuf {
    spool_root().join("failed")
}

fn done_dir() -> PathBuf {
    spool_root().join(".done")
}

/// Domain port: the real impl drives the state's own create/update/delete
/// pipelines; tests inject a stub (native async fn in trait, used only
/// through generic static dispatch — same pattern as messaging's
/// SpoolDelivery).
trait TaskCreator {
    async fn create(
        &self,
        input: CreateScheduledTaskInput,
    ) -> std::result::Result<ScheduledTaskDto, String>;
    async fn update(
        &self,
        task_id: &str,
        input: UpdateScheduledTaskInput,
    ) -> std::result::Result<ScheduledTaskDto, String>;
    async fn delete(&self, task_id: &str) -> std::result::Result<ScheduledTaskDto, String>;
}

struct StateCreator<'a>(&'a ScheduledTaskState);

impl TaskCreator for StateCreator<'_> {
    async fn create(
        &self,
        input: CreateScheduledTaskInput,
    ) -> std::result::Result<ScheduledTaskDto, String> {
        self.0.create_task(input).await
    }

    async fn update(
        &self,
        task_id: &str,
        input: UpdateScheduledTaskInput,
    ) -> std::result::Result<ScheduledTaskDto, String> {
        self.0.update_task(task_id.to_string(), input).await
    }

    async fn delete(&self, task_id: &str) -> std::result::Result<ScheduledTaskDto, String> {
        self.0.delete_task(task_id.to_string()).await
    }
}

/// Panel-refresh signal: production emits `scheduled_task:run_updated`
/// (desktop emit + web forward, same as the file watcher); tests count calls.
trait PanelNotifier: Send + Sync {
    fn notify(&self);
}

struct CoalescingNotifier<'a, N: PanelNotifier + ?Sized> {
    inner: &'a N,
    changed: std::sync::atomic::AtomicBool,
}

impl<'a, N: PanelNotifier + ?Sized> CoalescingNotifier<'a, N> {
    fn new(inner: &'a N) -> Self {
        Self {
            inner,
            changed: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl<N: PanelNotifier + ?Sized> PanelNotifier for CoalescingNotifier<'_, N> {
    fn notify(&self) {
        self.changed
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl<N: PanelNotifier + ?Sized> Drop for CoalescingNotifier<'_, N> {
    fn drop(&mut self) {
        if self.changed.load(std::sync::atomic::Ordering::Relaxed) {
            self.inner.notify();
        }
    }
}

struct EventNotifier<'a>(&'a tauri::AppHandle);

impl PanelNotifier for EventNotifier<'_> {
    fn notify(&self) {
        use tauri::Emitter;
        let _ = self
            .0
            .emit("scheduled_task:run_updated", serde_json::json!({}));
        crate::platform::app_events::forward_app_event(
            self.0,
            "scheduled_task:run_updated",
            serde_json::json!({}),
        );
    }
}

/// Terminal disposition of one spool file after processing. Parse/validate/
/// oversize problems are permanent (retrying cannot succeed); creation
/// failures are treated as transient and retried up to
/// [`MAX_CREATE_ATTEMPTS`].
enum Processed {
    /// Created (or an already-created skip): the marker exists, the spool
    /// file can be removed. Carries the APPLIED request's digest — the
    /// loop's unlink guard re-digests whatever sits at the path and removes
    /// it only when it still matches (round-11 MAJOR-4: a keyed retry whose
    /// corrected payload landed during the apply window must survive to the
    /// next poll, not be destroyed untried under the old body's receipt).
    Done(String),
    /// The domain already applied; only receipt publication may be retried.
    ReceiptPending(String, serde_json::Value),
    /// Permanent rejection: quarantine now.
    Poison(anyhow::Error),
    /// Transient creation failure: retry. Carries the attempted record's
    /// payload digest when one was computable — the parked episode's
    /// identity input (#680: the un-park is admitted by a payload CHANGE,
    /// so the identity must be the bytes the apply consumed, not whatever
    /// a post-apply re-read would find there).
    Retry(anyhow::Error, Option<String>),
}

fn build_create_input(request: &SpooledCreationRequest) -> CreateScheduledTaskInput {
    CreateScheduledTaskInput {
        name: request
            .name
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string(),
        prompt: request
            .prompt
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string(),
        rrule: request
            .rrule
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string(),
        cwds: Vec::new(),
        model: None,
        model_id: trimmed_non_empty(&request.model_id),
        target_session: trimmed_non_empty(&request.target_session),
        kind: None,
        mode: None,
        allow_shell: None,
        trust_mode: None,
        auto_approve: None,
        paused: Some(request.paused.unwrap_or(false)),
    }
}

fn build_update_input(request: &SpooledCreationRequest) -> UpdateScheduledTaskInput {
    UpdateScheduledTaskInput {
        name: trimmed_non_empty(&request.name),
        prompt: trimmed_non_empty(&request.prompt),
        rrule: trimmed_non_empty(&request.rrule),
        cwds: None,
        model: None,
        model_id: trimmed_non_empty(&request.model_id),
        target_session: trimmed_non_empty(&request.target_session),
        mode: None,
        allow_shell: None,
        trust_mode: None,
        auto_approve: None,
        paused: request.paused,
    }
}

fn trimmed_non_empty(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|trimmed| !trimmed.is_empty())
        .map(str::to_string)
}

/// Append the kind-specific L1 audit record into the requesting session's
/// execution root (failures inside `audit::append` only log, never panic).
/// Without a usable `from_session` the task record itself is the audit trail
/// (design D3).
/// Round-24 M4 (the messaging sibling's round-8 M1 rule): bound sessions'
/// audit lines route to the PRIVATE ledger root — appending to
/// `roots.execution` lands `workflow_audit.jsonl` inside the user's bound
/// project directory (git-status pollution), and the provenance line sits
/// inside the audited model's own working tree, where that session's model
/// can read and rewrite it with ordinary file tools. Unbound sessions keep
/// the execution root (both roots are the same private dir there).
fn audit_root_for(roots: &crate::features::sessions::SessionRoots) -> std::path::PathBuf {
    if roots.bound {
        roots.ledger.clone()
    } else {
        roots.execution.clone()
    }
}

fn audit_request(
    sessions: &crate::features::sessions::SessionStore,
    request: &SpooledCreationRequest,
    task_id: &str,
    task_name: &str,
) {
    let Some(from) = request.from_session.as_deref() else {
        return;
    };
    let (tool, kind) = match request.kind {
        SpoolRequestKind::Create => (SCHEDULED_TASK_CREATE_TOOL, "scheduled_task_create"),
        SpoolRequestKind::Update => (SCHEDULED_TASK_UPDATE_TOOL, "scheduled_task_update"),
        SpoolRequestKind::Delete => (SCHEDULED_TASK_DELETE_TOOL, "scheduled_task_delete"),
    };
    let mut detail = serde_json::json!({
        "tool": tool,
        "task_id": task_id,
        "task_name": task_name,
        "outcome": request.kind.as_str(),
        // Round-6 MAJOR 2: the per-session line carries the same honesty as
        // the shadow audit — the sender is a model-supplied claim, and a
        // bound session's execution root is that project's working tree;
        // planted evidence must be distinguishable from verified
        // provenance on the human-facing surface too.
        "claimed_from_session_verified": false,
    });
    if request.kind == SpoolRequestKind::Update {
        // Record which fields the update touched so the audit line says what
        // changed, not just that something did.
        let changed: Vec<&str> = [
            ("name", request.name.is_some()),
            ("prompt", request.prompt.is_some()),
            ("rrule", request.rrule.is_some()),
            ("model_id", request.model_id.is_some()),
            ("paused", request.paused.is_some()),
            ("target_session", request.target_session.is_some()),
        ]
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(label, _)| label)
        .collect();
        detail["changed"] = serde_json::json!(changed);
    }
    if let Some(target) = request
        .target_session
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        // A delivery target — set at create or moved by a retarget —
        // redirects where every future prompt fires: the audit line must
        // say where it goes either way. Round-13 minor 7: the TRIMMED
        // value (validation checks the trimmed form; writing the raw form
        // let padded ids into the trail).
        detail["target_session"] = serde_json::json!(target);
    }
    if let Ok(roots) = sessions.session_roots(from) {
        let audit_root = audit_root_for(&roots);
        append_session_audit(&audit_root, kind, detail);
    }
}

/// Keep this feature's hardened audit IO local. Other audit consumers retain
/// their existing append/open/logging behavior (scope separation for R33).
fn append_session_audit(root: &Path, kind: &str, detail: serde_json::Value) {
    use std::io::Write as _;
    let result = (|| -> std::io::Result<()> {
        let directory = private_directory(root)?;
        let mut file = directory.open_append_file(std::ffi::OsStr::new("workflow_audit.jsonl"))?;
        let line = serde_json::json!({"ts": chrono::Utc::now().to_rfc3339(), "kind": kind, "role": "app", "detail": detail});
        file.write_all(format!("{line}\n").as_bytes())
    })();
    if let Err(error) = result {
        log::warn!("[scheduled-creation] session audit append failed: {error}");
    }
}

/// Shadow audit (review R4-M3): an append-only trace under the automation
/// store root, written for EVERY applied request regardless of
/// `from_session`. The session-workspace audit above early-returns when the
/// model-supplied `from_session` is omitted — and an unattended session can
/// omit it precisely to skip the claimed-origin trace — so this
/// watcher-side trace covers requests applied through this watcher. It is
/// best effort, not tamper proof: same-user direct store writes bypass it.
/// The spool file NAME (not the record fields) keys the identity column,
/// and the claimed sender is recorded as explicitly unverified.
fn audit_request_shadow(
    request: &SpooledCreationRequest,
    spool_stem: &str,
    task_id: &str,
    task_name: &str,
    outcome: &str,
) {
    let tool = match request.kind {
        SpoolRequestKind::Create => SCHEDULED_TASK_CREATE_TOOL,
        SpoolRequestKind::Update => SCHEDULED_TASK_UPDATE_TOOL,
        SpoolRequestKind::Delete => SCHEDULED_TASK_DELETE_TOOL,
    };
    let detail = serde_json::json!({
        "tool": tool,
        "outcome": outcome,
        "task_id": task_id,
        "task_name": task_name,
        // The delivery destination belongs in the append-only trail too
        // (round-4 minor 7): a from_session-less retarget otherwise leaves
        // the destination only in mutable state. Round-13 minor 7: the
        // TRIMMED value (validation checks the trimmed form).
        "target_session": request
            .target_session
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty()),
        "spool_stem": spool_stem,
        "claimed_from_session": request.from_session,
        "claimed_from_session_verified": false,
    });
    append_shadow_audit("scheduled_task_request", detail);
}

fn append_shadow_audit(kind: &str, detail: serde_json::Value) {
    let prepared = (|| -> std::io::Result<_> {
        let directory = shadow_directory()?;
        let current = std::ffi::OsStr::new("workflow_audit.jsonl");
        let previous = std::ffi::OsStr::new("workflow_audit.jsonl.old");
        if let Some(file) = directory.open_plain_file(current)? {
            if file.metadata()?.len() > SHADOW_AUDIT_ROTATE_BYTES {
                let rotated = directory.remove_plain_file(previous).and_then(|_| {
                    directory
                        .move_plain_file_to(current, &directory, previous)
                        .map(|_| ())
                });
                if let Err(error) = rotated {
                    log::warn!(
                        "[scheduled-creation] shadow rotation failed; retaining current log and appending evidence: {error}"
                    );
                }
            }
        }
        directory.open_append_file(current)
    })();
    let mut writer = match prepared {
        Ok(writer) => writer,
        Err(error) => {
            log::warn!("[scheduled-creation] shadow audit unavailable: {error}");
            return;
        }
    };
    let line = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(), "kind": kind,
        "role": "app", "detail": detail,
    });
    use std::io::Write as _;
    if let Err(error) = writer.write_all(format!("{line}\n").as_bytes()) {
        log::warn!("[scheduled-creation] shadow audit append failed: {error}");
    }
}

/// Best-effort failure audit: a poison or retry-exhausted record with a
/// usable `from_session` leaves a trace in that session's execution root —
/// the audit trail must not be success-only. Runs before quarantine because
/// it re-reads the spool file.
fn audit_failure(
    sessions: &crate::features::sessions::SessionStore,
    bytes: Option<&[u8]>,
    error: &anyhow::Error,
) {
    let Some(bytes) = bytes else {
        return;
    };
    let Ok(request) = serde_json::from_slice::<SpooledCreationRequest>(&bytes) else {
        return;
    };
    let Some(from) = request.from_session.as_deref() else {
        return;
    };
    // Round-13 MAJOR-2: the per-session failure line is gated on the sender
    // EXACTLY like the success path's validate() — `session_roots`
    // charset-validates only, so a planted record claiming an isolated
    // prefix (or any sender validate() already rejected) must not land a
    // failure line in that session's execution root; such records stay
    // covered by the shadow trail alone.
    if check_sender_session_id(request.from_session.as_ref()).is_err() {
        return;
    }
    let Ok(roots) = sessions.session_roots(from) else {
        return;
    };
    let tool = match request.kind {
        SpoolRequestKind::Create => SCHEDULED_TASK_CREATE_TOOL,
        SpoolRequestKind::Update => SCHEDULED_TASK_UPDATE_TOOL,
        SpoolRequestKind::Delete => SCHEDULED_TASK_DELETE_TOOL,
    };
    let error_text = sanitize_marker_text(&format!("{error:#}"));
    let detail = serde_json::json!({
        "tool": tool,
        "task_id": request.task_id.as_deref().unwrap_or_default(),
        "outcome": "failed",
        // Round-13 MAJOR-2: same stamp as audit_request / the shadow audit
        // — planted evidence must be distinguishable from verified
        // provenance.
        "claimed_from_session_verified": false,
        "error": error_text.chars().take(500).collect::<String>(),
    });
    let audit_root = audit_root_for(&roots);
    append_session_audit(&audit_root, "scheduled_task_failed", detail);
}

/// Process one spool file: read → validate → (marker check) → create →
/// marker + audit + notify. The spool identity is the directory-listed file
/// name (the server names idempotent retries by their sender-scoped key
/// hash) — the JSON `id` field is never trusted for watcher paths.
async fn process_spool_file<C: TaskCreator, N: PanelNotifier + ?Sized>(
    path: &Path,
    creator: &C,
    sessions: &crate::features::sessions::SessionStore,
    notifier: &N,
) -> Processed {
    let poisoned = |error: anyhow::Error| Processed::Poison(error);
    // Round-12 MAJOR-2: the entry read goes through the same gated helper
    // as every other read — the collection-time is_file() filter is stale
    // by processing time (files drain sequentially across awaited applies),
    // so a FIFO or /dev/zero symlink swapped in after collection would
    // wedge the single drain task in a blocking open or grow an unbounded
    // Vec; stat+regular-file+cap now refuse it (poison: same class as
    // oversize).
    let bytes = match read_regular_bounded(path, MAX_SPOOL_FILE_BYTES as usize) {
        Some(bytes) => bytes,
        None => {
            return poisoned(anyhow::anyhow!(
                "spool file unreadable, not a regular file, or exceeds the {} byte cap",
                MAX_SPOOL_FILE_BYTES
            ));
        }
    };
    let request: SpooledCreationRequest = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(error) => return poisoned(anyhow::Error::new(error).context("parse spool file")),
    };
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let done_marker = done_dir().join(format!("{stem}.json"));
    let expected_digest = spool_request_digest(&request);
    // Round-7 MAJOR 1a: the success marker is probed BEFORE validation —
    // a leftover record that no longer validates (e.g. a FREQ=ONCE whose
    // AT passed) must suppress on its recorded success, not take the
    // poison arm whose failure-marker write would overwrite the ok:true
    // receipt and re-apply an already-succeeded key.
    if result_marker_suppresses(&done_marker, &expected_digest) {
        return Processed::Done(expected_digest);
    }
    if let Err(error) = request.validate() {
        return poisoned(error);
    }
    if request.kind == SpoolRequestKind::Update
        && (request.prompt.is_some() || request.rrule.is_some())
    {
        return poisoned(anyhow::anyhow!(
            "confirmation_required: use the Scheduled Tasks panel to review and submit prompt/rrule replacements; no change was applied"
        ));
    }
    if result_marker_suppresses(&done_marker, &expected_digest) {
        // Idempotent retry after a completed operation: the success marker
        // wins and the leftover file is dropped WITHOUT a second apply (C3).
        // A failure marker does NOT suppress: the retry re-applies so one
        // terminal failure cannot poison the key forever (the fixed cause
        // can succeed; success overwrites the marker).
        return Processed::Done(expected_digest);
    }
    // Drop the stale failure marker BEFORE applying: the server's first poll
    // must not replay the previous attempt's error while this fresh apply is
    // in flight. A failing apply writes a fresh marker on its terminal path
    // (inside the documented at-least-once window).
    if let Ok(directory) = private_directory(&done_dir()) {
        let _ = directory.remove_plain_file(done_marker.file_name().unwrap_or_default());
    }
    let applied = match request.kind {
        SpoolRequestKind::Create => creator.create(build_create_input(&request)).await,
        SpoolRequestKind::Update => {
            // Round-26 minor 1: validation checked the TRIMMED id —
            // dispatching the raw value sent whitespace-padded direct-spool
            // ids to a lookup that could never match, burning the full
            // retry budget before failing.
            let task_id = request
                .task_id
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            creator.update(&task_id, build_update_input(&request)).await
        }
        SpoolRequestKind::Delete => {
            let task_id = request
                .task_id
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            creator.delete(&task_id).await
        }
    };
    match applied {
        Ok(dto) => {
            let payload = serde_json::json!({
                "ok": true,
                "kind": request.kind.as_str(),
                "task_id": dto.id,
                "task_name": dto.name,
                // R5-M1: what was APPLIED, digested — the server compares
                // this against a retried call's payload to answer
                // payload_mismatch truthfully (the task_id alone cannot).
                "request_digest": spool_request_digest(&request),
            });
            // Round-8 minor 1: the domain mutation LANDED here — the
            // shadow line fires before anything can divert the arm (the
            // round-7 shape returned Retry on a marker-write failure with
            // NO audit at all, the exact path that re-applies on retry).
            audit_request_shadow(&request, &stem, &dto.id, &dto.name, "applied");
            audit_request(sessions, &request, &dto.id, &dto.name);
            notifier.notify();
            if let Err(error) =
                write_done_marker(&done_marker, &payload).context("write result marker")
            {
                // A missing marker means the MCP server's short wait times out
                // into "pending" and a retried call could re-apply: surface
                // this as a retryable failure instead of reporting success.
                log::warn!(
                    "[scheduled-creation] applied but receipt publication is pending: {error:#}"
                );
                return Processed::ReceiptPending(expected_digest, payload);
            }
            Processed::Done(expected_digest)
        }
        Err(error) => {
            audit_request_shadow(
                &request,
                &stem,
                request.task_id.as_deref().unwrap_or_default(),
                request.name.as_deref().unwrap_or_default(),
                "apply-failed",
            );
            let error = anyhow::anyhow!(error);
            // Domain rejections that can never succeed on retry poison
            // immediately instead of burning the transient budget
            // (review round-3 domain minor: three doomed attempts ate ~3s
            // of the MCP server's 5s sync window).
            if is_permanent_domain_error(&format!("{error:#}")) {
                Processed::Poison(error)
            } else {
                Processed::Retry(error, Some(expected_digest))
            }
        }
    }
}

/// Canonical digest of a spooled request's payload-bearing fields (R5-M1).
/// serde_json's default Map is a sorted BTreeMap; the golden-vector
/// tests pin the exact serialization (alphabetical field order here) — `server.py::spool_request_digest`
/// mirrors the exact construction (sorted keys, compact separators, raw
/// UTF-8) so both sides hash identically.
fn spool_request_digest(request: &SpooledCreationRequest) -> String {
    // `paused` omitted vs Some(false) digests identically for creates
    // (the domain coerces via unwrap_or(false)) — mirror the server's
    // coercion so the spelling difference cannot false-flag a mismatch.
    let normalized_paused = match request.kind {
        SpoolRequestKind::Create => Some(request.paused.unwrap_or(false)),
        _ => request.paused,
    };
    // Round-8 minor 20: serialize through a BTreeMap — serde_json resolves
    // with preserve_order (codewhale-tui), so the json! literal's
    // alphabetical order was the only anchor of byte parity with the
    // python twin's sort_keys; the map anchors it mechanically.
    let canonical = std::collections::BTreeMap::from([
        ("kind", serde_json::json!(request.kind.as_str())),
        (
            "model_id",
            serde_json::json!(trimmed_non_empty(&request.model_id)),
        ),
        ("name", serde_json::json!(trimmed_non_empty(&request.name))),
        ("paused", serde_json::json!(normalized_paused)),
        (
            "prompt",
            serde_json::json!(trimmed_non_empty(&request.prompt)),
        ),
        (
            "rrule",
            serde_json::json!(
                trimmed_non_empty(&request.rrule).map(|value| value.to_ascii_uppercase())
            ),
        ),
        (
            "task_id",
            serde_json::json!(trimmed_non_empty(&request.task_id)),
        ),
        (
            "target_session",
            serde_json::json!(trimmed_non_empty(&request.target_session)),
        ),
    ]);
    use sha2::Digest;
    let serialized = serde_json::to_string(&canonical).unwrap_or_default();
    let digest = sha2::Sha256::digest(serialized.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Round-11 MAJOR-2/3, re-cut at round-19 MAJOR-1: the gate moved from
/// stat-then-open to the platform's open-time-gated read. The old shape
/// (`symlink_metadata`, then a separate `File::open`) left a window where
/// a FIFO planted into the path opened BLOCKING and wedged the single
/// drain task forever — the exact failure the round-11 comment claimed
/// this helper prevents, but only for a FIFO present at stat time. The
/// platform primitive gates at the open itself (O_NOFOLLOW|O_NONBLOCK,
/// fd-level regular-file refusal) and re-bounds the cap past the gate.
/// `None` = missing/not a regular file/oversize/unreadable — callers
/// treat that as "no usable bytes". Residual: the fd pins the inode, so a
/// concurrent appender can still grow the file past the gate — the
/// primitive's capped read re-bounds that.
fn read_regular_bounded(path: &Path, cap: usize) -> Option<Vec<u8>> {
    read_regular_bounded_anchored(path, cap).map(|(bytes, _)| bytes)
}

fn read_regular_bounded_anchored(path: &Path, cap: usize) -> Option<(Vec<u8>, std::fs::Metadata)> {
    use std::io::Read as _;
    let directory = private_directory(path.parent()?).ok()?;
    let file = directory.open_plain_file(path.file_name()?).ok()??;
    let metadata = file.metadata().ok()?;
    if metadata.len() > cap as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(cap as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= cap).then_some((bytes, metadata))
}

/// Identity compare for the anchored Done-arm unlink (round-24 minor 1):
/// the open-handle fstat of the digested bytes vs the name's current
/// symlink stat. std has no cross-platform inode, so identity is
/// (regular file, len, modified-time) — a replacement payload virtually
/// always moves the mtime, and a same-name miss only keeps the record for
/// the next poll, where the digest gate re-verifies content regardless.
/// FAT's 2 s mtime granularity is the documented floor (same caveat as
/// the unlink gates in the design doc's borderline list).
fn same_file_identity(current: &std::fs::Metadata, applied: &std::fs::Metadata) -> bool {
    current.is_file()
        && current.len() == applied.len()
        && current.modified().ok() == applied.modified().ok()
        && crate::platform::filesystem::metadata_file_identity(current)
            == crate::platform::filesystem::metadata_file_identity(applied)
}

/// Validation-shaped domain errors (charset/isolation/caps/kind rules and
/// missing targets) — retrying the same record can never fix them.
fn is_permanent_domain_error(error: &str) -> bool {
    // Keep explicit, reachable domain dispositions here. Pre-spool validation
    // handles field shape, mode, schedule syntax and body limits. I/O, registry
    // recovery and index-read failures remain bounded retries. A typed error
    // migration is deferred; do not classify generic OS/parse text as poison.
    const PERMANENT_MARKERS: [&str; 5] = [
        "only scheduled-message tasks can change their target session",
        "target_session not found:",
        "scheduled task not found:",
        "ACP/code session",
        "Once schedule has no future run",
    ];
    PERMANENT_MARKERS
        .iter()
        .any(|marker| error.contains(marker))
}

fn write_done_marker(path: &Path, payload: &serde_json::Value) -> Result<()> {
    let directory = private_directory(path.parent().context("marker parent missing")?)?;
    // Existing primitive uses a unique exclusive temp, sync_all, and a
    // directory-handle-relative atomic publication on each supported OS.
    directory.atomic_write_private_file(
        path.file_name().context("marker name missing")?,
        payload.to_string().as_bytes(),
    )?;
    Ok(())
}

/// Whether a replayed request with an existing result marker should be
/// suppressed. Only a parseable `ok:true` marker whose `request_digest`
/// matches THIS request suppresses (C3 idempotent replay). Anything
/// else — `ok:false` (retry re-applies), unparseable/oversize/unreadable
/// (mid-write or hostile), or a digest mismatch (forged) — lets the
/// retry re-apply: the marker channel is untrusted, and the server's
/// poll surfaces the fresh outcome instead of a stale lie.
fn result_marker_suppresses(path: &Path, expected_digest: &str) -> bool {
    // Round-7 MAJOR 1b: the marker channel is a trust boundary like the
    // spool itself. Size-gate the read, REQUIRE the digest binding (a
    // forged {"ok":true} — with a wrong or absent `request_digest` —
    // must not silently swallow a queued request), and make every
    // non-NotFound outcome observable — suppression now says WHY via
    // the log.
    // Round-11 MAJOR-2/3: the gate+cap read replaced read-then-check —
    // the old shape read a planted sparse/oversize marker WHOLE before the
    // length check, and a FIFO or /dev/zero symlink opened blocking and
    // wedged the drain. NotFound stays silent (the normal no-marker case);
    // every other refusal is observable.
    let bytes = match read_regular_bounded(path, MAX_RESULT_MARKER_BYTES) {
        Some(bytes) => bytes,
        None => {
            if path.exists() {
                log::warn!(
                    "[scheduled-creation] result marker refused (not a regular file, oversize, or unreadable): {:?}",
                    path
                );
            }
            return false;
        }
    };
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(value) => {
            // Round-21 minor 1: require a LITERAL ok:true — the function's
            // own doc ("Only a parseable ok:true marker … suppresses") and
            // the receipt probe's `== Some(true)` both bind the truthiness:
            // ok:absent/"true"/1 used to suppress while the server's
            // truthiness check kept answering, splitting the outcome.
            if value.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
                return false;
            }
            match value
                .get("request_digest")
                .and_then(serde_json::Value::as_str)
            {
                Some(digest) if digest == expected_digest => {}
                Some(_) => {
                    log::warn!(
                        "[scheduled-creation] result marker digest mismatch: {:?}",
                        path
                    );
                    return false;
                }
                None => {
                    log::warn!(
                        "[scheduled-creation] result marker lacks digest binding: {:?}",
                        path
                    );
                    return false;
                }
            }
            log::info!(
                "[scheduled-creation] result marker suppresses replay: {:?}",
                path
            );
            true
        }
        Err(_) => {
            log::warn!("[scheduled-creation] result marker unparseable: {:?}", path);
            false
        }
    }
}

/// Bound for the result marker read (real markers are ~150 bytes).
const MAX_RESULT_MARKER_BYTES: usize = 64 * 1024;

fn quarantine(path: &Path) -> std::io::Result<()> {
    let source = private_directory(
        path.parent()
            .ok_or_else(|| std::io::Error::other("missing spool parent"))?,
    )?;
    let destination = private_directory(&failed_dir())?;
    let dir = failed_dir();
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "missing evidence filename",
        )
    })?;
    for counter in 0..u32::MAX {
        let target = if counter == 0 {
            dir.join(name)
        } else {
            let mut suffixed = path.file_stem().unwrap_or(name).to_os_string();
            suffixed.push(format!(".{counter}"));
            if let Some(extension) = path.extension() {
                suffixed.push(".");
                suffixed.push(extension);
            }
            dir.join(suffixed)
        };
        match source.move_plain_file_to(name, &destination, target.file_name().unwrap_or(name)) {
            Ok(crate::platform::filesystem::MovePlainFileOutcome::Moved) => {
                let evidence_name = target.file_name().unwrap_or(name);
                let stamp = std::fs::FileTimes::new().set_modified(std::time::SystemTime::now());
                let stamped = destination
                    .open_plain_file(evidence_name)?
                    .ok_or_else(|| std::io::Error::other("quarantined evidence disappeared"))?
                    .set_times(stamp)
                    .or_else(|_| {
                        destination
                            .open_append_file(evidence_name)?
                            .set_times(stamp)
                    });
                if let Err(error) = stamped {
                    // Never prune fresh evidence by a preserved old mtime if
                    // filesystem metadata permissions refuse both handle modes.
                    let held = format!(".untimed-{}", uuid::Uuid::new_v4());
                    destination.move_plain_file_to(
                        evidence_name,
                        &destination,
                        std::ffi::OsStr::new(&held),
                    )?;
                    log::warn!(
                        "[scheduled-creation] evidence timestamp unavailable; retained for manual cleanup as {held}: {error}"
                    );
                }
                return Ok(());
            }
            Ok(_) => {
                return Err(std::io::Error::other(
                    "spool evidence vanished before quarantine",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "evidence suffixes exhausted",
    ))
}

fn quarantine_with_warning(path: &Path, warnings: &mut usize) -> bool {
    match quarantine(path) {
        Ok(()) => true,
        Err(error) => {
            *warnings += 1;
            if *warnings <= POISON_WARN_PER_PASS {
                log::warn!(
                    "[scheduled-creation] cannot quarantine {:?}; source retained: {error}",
                    path
                );
            } else if *warnings == POISON_WARN_PER_PASS + 1 {
                log::warn!(
                    "[scheduled-creation] further quarantine failures this pass are summarized; sources retained"
                );
            }
            false
        }
    }
}

/// Failure markers must not leak absolute host paths into model-visible
/// context (the MCP server scrubs its own errors the same way): fold the
/// user home and the pinvou3 home prefixes into opaque placeholders.
fn sanitize_marker_text(text: &str) -> String {
    let pinvou3_home = crate::platform::paths::pinvou3_home();
    let home = crate::platform::paths::user_home_dir();
    // Longest prefix first so the pinvou3-home placeholder wins over "~".
    let longest_first = [(&pinvou3_home, "<pinvou3-home>"), (&home, "~")];
    let mut sanitized = text.to_string();
    for (prefix, placeholder) in longest_first {
        let prefix_text = prefix.to_string_lossy().to_string();
        if !prefix_text.is_empty() {
            sanitized = sanitized.replace(&prefix_text, placeholder);
        }
    }
    sanitized
}

// Round-19 MAJOR-2: the scrubber was mutation-green — the poison fixture's
// validation error contained no path, so deleting the whole body kept the
// suite green. Pin it with a path-bearing input (an IO-context apply error
// embeds exactly these prefixes).
#[test]
fn sanitize_marker_text_scrubs_home_prefixes() {
    // Round-36 review: the home paths are read here and re-read inside
    // sanitize_marker_text — a concurrent TempHome-style env flip between
    // the two reads folds nothing and fails the assert. Hold the env lock
    // for the whole test (the repo's own env-serialization contract).
    let _env_lock = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let pinvou3_home = crate::platform::paths::pinvou3_home();
    let user_home = crate::platform::paths::user_home_dir();
    let text = format!(
        "create workspace {}/workspaces/w-1 (under {}) failed: is a file",
        pinvou3_home.display(),
        user_home.display()
    );
    let scrubbed = sanitize_marker_text(&text);
    assert!(
        !scrubbed.contains(&pinvou3_home.display().to_string())
            && !scrubbed.contains(&user_home.display().to_string()),
        "no absolute prefix may survive: {scrubbed}"
    );
    assert!(
        scrubbed.contains("<pinvou3-home>") && scrubbed.contains('~'),
        "both prefixes fold to their placeholders: {scrubbed}"
    );
}

/// Write a failure marker for an already-consumed spool file (the caller has
/// quarantined it): a still-waiting MCP call receives the failure instead of
/// hanging until its 5s timeout (design C4). The marker also unblocks the
/// idempotency key: a later retry re-applies instead of replaying the stale
/// failure forever.
fn write_failure_marker(stem: &str, error: &anyhow::Error) {
    let marker = done_dir().join(format!("{stem}.json"));
    // Round-8 minor 2: NEVER overwrite a readable ok:true receipt — the
    // early poison arms (stat/read/parse/oversize) bypass the suppression
    // probe entirely, so a stuck spool file whose bytes later corrupt
    // would otherwise have its success receipt replaced with ok:false,
    // and the model's next retry re-applies a succeeded operation.
    // Round-11 MAJOR-2 (folded minor): the receipt probe is gated+bounded
    // like every other marker read.
    if let Some(existing) = read_regular_bounded(&marker, MAX_RESULT_MARKER_BYTES) {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&existing) {
            if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
                log::warn!(
                    "[scheduled-creation] refusing to overwrite an ok:true receipt for {stem}"
                );
                return;
            }
        }
    }
    let payload = serde_json::json!({
        "ok": false,
        "error": sanitize_marker_text(&format!("{error:#}")),
    });
    if let Err(marker_error) = write_done_marker(&marker, &payload) {
        log::warn!("[scheduled-creation] failure marker write failed for {stem}: {marker_error:#}");
    }
}

/// Whether the record's result marker is currently ABSENT. The server
/// unlinks a not-ok marker right after re-spooling the same name (its
/// stale-marker unlink), so a stuck episode's "once per episode" marker
/// dedupe must still restore a marker that vanished this way — the dedupe
/// exists to stop per-pass rewrites, never to leave a waiting call without
/// an answer (#680's ensure-presence). The ok:true refusal in
/// [`write_failure_marker`] keeps a restored marker from ever clobbering a
/// landed receipt.
fn failure_marker_missing(stem: &str) -> bool {
    // Round-12 minor 11: a MISSING or UNPARSEABLE marker both leave the
    // waiting call unanswered (the server's poll cannot read a corrupt
    // marker either, and the re-spool unlink never produces this shape) —
    // the healers repair both. A readable marker of any outcome answers
    // the call and keeps the once-per-episode dedupe.
    match read_regular_bounded(
        &done_dir().join(format!("{stem}.json")),
        MAX_RESULT_MARKER_BYTES,
    ) {
        None => true,
        Some(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes).is_err(),
    }
}

/// The payload digest of the record currently sitting at `path`, when one
/// is computable under the gated+bounded read (None: unreadable or
/// unparseable).
fn record_digest(path: &Path) -> Option<String> {
    read_regular_bounded(path, MAX_SPOOL_FILE_BYTES as usize)
        .and_then(|bytes| serde_json::from_slice::<SpooledCreationRequest>(&bytes).ok())
        .map(|request| spool_request_digest(&request))
}

/// Whether the record now sitting at a parked name is a payload CHANGE
/// against the parked episode's audited identity — the un-park gate's
/// input (#680): a same-bytes mtime bump does NOT rebind (the gate, not
/// the clock, admits a replacement), a corrected payload does, and a
/// record that parses while the audited episode stored no digest does
/// (the gate arm's round-4 m-A rule). No recorded identity (a park the
/// pre-#680 code left behind, or an externally seeded one) admits as
/// before.
fn parked_episode_rebinds(name: &str, root: &Path, retries: &RetryState) -> bool {
    let Some(audited) = retries.stuck_episode_audited.get(name) else {
        return true;
    };
    // The gate's admit predicate is the arms' fresh-episode predicate —
    // one copy, so the two decisions cannot desynchronize (round-12 nit).
    opens_fresh_episode(Some(audited), record_digest(&root.join(name)).as_deref())
}

/// Whether this poison/exhaustion opens a FRESH stuck episode for the name:
/// no recorded identity, or an identity that differs from the record's
/// current digest (a corrected re-spool, or a record that parses while the
/// audited episode was unparseable-empty, or one that stopped parsing while
/// the audited one had a digest). A same-bytes re-poison stays deduped.
fn opens_fresh_episode(audited: Option<&String>, current: Option<&str>) -> bool {
    match audited {
        None => true,
        Some(audited) => match current {
            Some(current) => audited.is_empty() || audited != current,
            None => !audited.is_empty(),
        },
    }
}

/// Age-based retention for terminal state (see [`STATE_RETENTION`]): prune
/// stale markers in `.done/` and evidence in `failed/`, and sweep stray
/// `*.tmp` files left by a crash between mkstemp and rename on the server
/// side (nothing else ever removes them).
fn prune_stale_state() {
    prune_stale_state_at(std::time::SystemTime::now());
}

/// Round-36: the retention sweep's starvation check — a bounded
/// enumeration window is "starved" when it holds entries that are
/// neither the app's own marker directories nor sweepable records, so
/// the sweep can never reach them. The app-owned `.done`/`failed`
/// directories are excluded: a healthy spool root holding only those
/// (and no records) is the normal steady state, not a defect.
fn spool_starved_by_non_records(
    names: &[std::ffi::OsString],
    owned_dirs: &[std::ffi::OsString],
    is_spool_root: bool,
) -> bool {
    if !is_spool_root {
        return false;
    }
    let non_owned: Vec<std::ffi::OsString> = names
        .iter()
        .filter(|name| {
            !owned_dirs
                .iter()
                .any(|owned| owned.as_os_str() == name.as_os_str())
        })
        .cloned()
        .collect();
    if non_owned.is_empty() {
        return false;
    }
    non_owned
        .iter()
        .all(|name| !name.to_string_lossy().ends_with(".json"))
}

/// The clock is injected so tests can age files without sleeping.
fn prune_stale_state_at(now: std::time::SystemTime) {
    for (dir, keep_pending) in [
        (done_dir(), true),
        (failed_dir(), false),
        (spool_root(), false),
    ] {
        let Ok(directory) = private_directory(&dir) else {
            continue;
        };
        let Ok(names) = directory.entry_names_up_to(MAX_PENDING_FILES + MAX_TAIL_QUARANTINES)
        else {
            continue;
        };
        // Round-36 minor 2: non-JSON spool entries can fill the bounded
        // enumeration window so no sweepable name is ever seen. The sweep
        // stays bounded (the platform primitive is shared), but a window
        // this degraded must say so once per prune instead of silently
        // starving the retention clock. The app's own `.done`/`failed`
        // directories always sit in the spool root and never count as
        // `.json` — they are excluded, so a HEALTHY empty spool never
        // warns (round-36 reviewer probe).
        let starvation = spool_starved_by_non_records(
            &names,
            &[
                done_dir().file_name().unwrap_or_default().to_os_string(),
                failed_dir().file_name().unwrap_or_default().to_os_string(),
            ],
            dir == spool_root(),
        );
        if starvation {
            log::warn!(
                "[scheduled-creation] spool enumeration window ({}) holds no .json record; retention sweeping for this directory is starved until the non-record entries are removed",
                names.len()
            );
        }
        for name in names {
            // Untimed evidence has no trustworthy retention clock.
            if name.to_string_lossy().starts_with(".untimed-") {
                continue;
            }
            let path = dir.join(&name);
            if dir == spool_root() && !path.extension().is_some_and(|ext| ext == "tmp") {
                continue;
            }
            let Ok(Some(file)) = directory.open_plain_file(&name) else {
                continue;
            };
            let stale = file
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= STATE_RETENTION);
            let pending = keep_pending
                && path.extension().is_some_and(|ext| ext == "json")
                && std::fs::symlink_metadata(spool_root().join(&name)).is_ok();
            if stale && !pending {
                let _ = directory.remove_plain_file(&name);
            }
        }
    }
}

struct RequestSnapshot {
    metadata: std::fs::Metadata,
    bytes: Option<Vec<u8>>,
}
impl RequestSnapshot {
    fn capture(path: &Path) -> Option<Self> {
        use std::io::Read as _;
        let directory = private_directory(path.parent()?).ok()?;
        let file = directory.open_plain_file(path.file_name()?).ok()??;
        let metadata = file.metadata().ok()?;
        let bytes = if metadata.len() <= MAX_SPOOL_FILE_BYTES {
            let mut bytes = Vec::new();
            file.take(MAX_SPOOL_FILE_BYTES + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            (bytes.len() as u64 <= MAX_SPOOL_FILE_BYTES).then_some(bytes)
        } else {
            None
        };
        Some(Self { metadata, bytes })
    }
    fn still_matches(&self, path: &Path) -> bool {
        Self::capture(path).is_some_and(|current| {
            same_file_identity(&current.metadata, &self.metadata) && current.bytes == self.bytes
        })
    }
}

/// Retry attempts belong to the filename, not its current payload. Replacing
/// queued bytes keeps the spent budget; a valid parked replacement only
/// un-parks, it does not grant a new budget. This bounds same-key replanting.
/// Retry bookkeeping per file: attempt count. Entries are removed on every
/// terminal path. `spool_dir_error_logged` keeps the unreadable-directory
/// warning to one line per streak instead of 1 Hz spam, and
/// `removal_error_logged` gives the Done arm's persisted removal failure
/// the same gate (round-24 minor 4: a stuck file warned at 1 Hz —
/// reset on a successful removal). `parked` holds names whose
/// terminal-path quarantine FAILED (round-16 MAJOR-1): the record is
/// still in the spool and could not leave it, so re-processing would
/// re-apply it — no marker suppresses a record whose marker write cannot
/// land either — with no budget left to bound it. Round-24 minor 3: a
/// park is no longer restart-shaped — the entry carries the park-time
/// wall clock and the name un-parks when the record at the
/// deterministic sha256 name is replaced after the park (the
/// retry-after-fix re-spool writes fresh bytes with a newer mtime) or
/// vanishes; the quarantine failure applied to the previous bytes, not
/// these. #680: the un-park is admitted by the record's PAYLOAD DIGEST
/// (see `stuck_episode_audited`), not by the clock alone — a same-bytes
/// mtime bump stays parked.
#[derive(Default)]
struct RetryState {
    attempts: HashMap<String, u32>,
    spool_dir_error_logged: bool,
    feature_gate_error_logged: bool,
    removal_error_logged: bool,
    /// Round-31 minor 5: the ceiling-quarantine warn is once per streak
    /// (the flag resets on a pass that does not hit the ceiling) — a
    /// sustained re-plant flood at 1 Hz otherwise yields ~10k log
    /// lines/s from this arm alone.
    ceiling_warn_logged: bool,
    parked: HashMap<String, std::time::SystemTime>,
    receipts: HashMap<String, (String, serde_json::Value)>,
    storage_backpressure_logged: bool,
    /// #680: stuck records whose episode already appended its audit line
    /// and wrote its failure marker — a stuck record used to append one
    /// jsonl line and rewrite the marker EVERY pass (a parked record
    /// re-spooled with the same bytes un-parks, re-poisons, and re-appends
    /// forever). The value is the audited record's PAYLOAD DIGEST where one
    /// was computable (poison episodes, park identity, retry-exhaustion
    /// episodes), and empty when no digest was computable (unparseable
    /// record): an empty identity rebinds as soon as the record at the name
    /// parses. The map is the UN-PARK GATE's input too — a same-bytes
    /// re-spool matches the parked identity and stays parked; a corrected
    /// payload differs and rebinds onto a fresh episode (fresh audit line +
    /// marker) instead of being silenced by the stale entry. Entries clear
    /// when the record finally moves (the quarantine/park arms and the
    /// maintenance move; the Done arm's receipt path and the
    /// receipts-republish leave the entry to the pass-start retain), and
    /// the retain drops entries whose file was deleted externally.
    stuck_episode_audited: HashMap<String, String>,
}

/// Retain active retries at the head. Parked records and unpublished receipts
/// have their own maintenance paths and never consume apply/tail slots.
fn pending_batch(root: &Path, retries: &RetryState, mut listed: Vec<PathBuf>) -> Vec<PathBuf> {
    // #680: a name whose file vanished this pass (the parked-maintenance
    // move above, or an external delete after the retains) must not enter
    // the batch — a ghost poisons as "unreadable" and would open a junk
    // episode for a name that may legitimately return. Round-11 M-1:
    // `symlink_metadata`, not `exists` — `exists` follows links, so a
    // DANGLING symlink at a record name would be silently skipped forever
    // (never reaching the Poison arm's terminal answer); a dangling link
    // has metadata, stays in the batch, and is refused by the gated open
    // like any non-regular entry.
    let mut selected: Vec<_> = retries
        .attempts
        .keys()
        .filter(|name| !retries.parked.contains_key(*name) && !retries.receipts.contains_key(*name))
        .map(|name| root.join(name))
        .filter(|path| path.symlink_metadata().is_ok())
        .collect();
    selected.sort();
    selected.dedup();
    listed.sort();
    for path in listed {
        if selected.len() >= MAX_PENDING_FILES + MAX_TAIL_QUARANTINES {
            break;
        }
        if path.symlink_metadata().is_err() {
            continue;
        }
        let skipped = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                retries.parked.contains_key(name) || retries.receipts.contains_key(name)
            });
        if !skipped && !selected.contains(&path) {
            selected.push(path);
        }
    }
    selected
}

/// Watch loop body: process every pending spool file (sorted names — uuid /
/// key-hash hex), quarantining poison files immediately and creation failures
/// after [`MAX_CREATE_ATTEMPTS`] attempts.
async fn process_pending_spool<C: TaskCreator, N: PanelNotifier + ?Sized>(
    creator: &C,
    sessions: &crate::features::sessions::SessionStore,
    notifier: &N,
    retries: &mut RetryState,
) {
    let root = spool_root();
    let directory = match private_directory(&root) {
        Ok(directory) => {
            retries.spool_dir_error_logged = false;
            directory
        }
        Err(error) => {
            if !retries.spool_dir_error_logged {
                log::warn!("[scheduled-creation] cannot open real spool directory: {error}");
                retries.spool_dir_error_logged = true;
            }
            return;
        }
    };
    // Publish applied outcomes even when their spool file vanished or is
    // outside this pass's selected batch. Bound cached unapplied receipts too.
    let receipts: Vec<_> = retries
        .receipts
        .iter()
        .take(MAX_PENDING_FILES)
        .map(|(name, (digest, payload))| (name.clone(), digest.clone(), payload.clone()))
        .collect();
    for (name, digest, payload) in receipts {
        if write_done_marker(&done_dir().join(&name), &payload).is_ok() {
            let path = root.join(&name);
            // Round-35 minor 2: the removal is identity-anchored like the
            // Done arm — a same-name replacement landing between the read
            // and the unlink keeps the new payload for the next poll
            // instead of being destroyed untried.
            let applied = read_regular_bounded_anchored(&path, MAX_SPOOL_FILE_BYTES as usize)
                .and_then(|(bytes, applied_meta)| {
                    serde_json::from_slice::<SpooledCreationRequest>(&bytes)
                        .ok()
                        .filter(|request| spool_request_digest(&request) == digest)
                        .map(|_| applied_meta)
                });
            if let Some(applied_meta) = applied {
                if std::fs::symlink_metadata(&path)
                    .map(|current| same_file_identity(&current, &applied_meta))
                    .unwrap_or(false)
                {
                    let _ = directory.remove_plain_file(std::ffi::OsStr::new(&name));
                }
            }
            retries.receipts.remove(&name);
        }
    }
    if retries.receipts.len() >= MAX_PENDING_FILES {
        return;
    }
    // Enumeration itself is bounded, including non-json and non-file entries.
    // No scan/HashSet proportional to an untrusted directory's total size.
    let names = match directory.entry_names_up_to(MAX_PENDING_FILES + MAX_TAIL_QUARANTINES + 2) {
        Ok(names) => names,
        Err(error) => {
            log::warn!("[scheduled-creation] spool listing failed: {error}");
            return;
        }
    };
    let listed: Vec<PathBuf> = names
        .into_iter()
        .map(|name| root.join(name))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    // #680 (the messaging sibling's round-4 C5 guard, ported): a non-UTF-8
    // name can never be processed or keyed, but it passed the extension
    // filter and consumes an enumeration window AND batch slot — planted
    // squatters starve legitimately named records of apply slots (the
    // bounded listing window itself is a further base residual, disclosed
    // in the PR body). Quarantine on sight, BEFORE the batch count; no
    // marker is possible (the stem is unnameable), failed/ is the trace.
    let mut sight_warnings: usize = 0;
    let mut sight_quarantines: usize = 0;
    let listed: Vec<PathBuf> = listed
        .into_iter()
        .filter(|path| {
            if path.file_name().and_then(|n| n.to_str()).is_some() {
                return true;
            }
            if quarantine_with_warning(path, &mut sight_warnings) {
                sight_quarantines += 1;
            }
            false
        })
        .collect();
    // Round-10 m6: a successful sweep is otherwise log-silent while 256
    // files land in failed/; one line per pass matches the round-31
    // per-pass budget pattern.
    if sight_quarantines > 0 {
        log::warn!(
            "[scheduled-creation] quarantined {sight_quarantines} non-UTF-8 spool name(s) on sight"
        );
    }
    // Round-12 MAJOR-2: all three retains are symlink_metadata-based — an
    // `exists` retain follows links, so a dangling link's park + identity
    // dissolved EVERY pass and the Poison arm re-opened a fresh episode per
    // pass (1 Hz marker rewrites forever); a dangling link has metadata and
    // keeps its parked episode.
    retries
        .attempts
        .retain(|name, _| root.join(name).symlink_metadata().is_ok());
    retries
        .parked
        .retain(|name, _| root.join(name).symlink_metadata().is_ok());
    // #680: the episode map joins the retain — a re-spool after an
    // externally deleted file starts clean instead of inheriting the stale
    // identity.
    retries
        .stuck_episode_audited
        .retain(|name, _| root.join(name).symlink_metadata().is_ok());
    // Retry only the terminal move, never the domain operation, when the
    // evidence directory recovers. Otherwise parked identities wedge forever.
    let parked: Vec<_> = retries
        .parked
        .iter()
        .map(|(name, at)| (name.clone(), *at))
        .collect();
    // The original terminal failure already logged the IO cause; maintenance
    // must not repeat one warning per parked record on every poll.
    let mut maintenance_warnings = POISON_WARN_PER_PASS + 1;
    for (name, parked_at) in parked {
        let path = root.join(&name);
        let Ok(mtime) = std::fs::metadata(&path).and_then(|meta| meta.modified()) else {
            continue;
        };
        let now = std::time::SystemTime::now();
        let bumped = mtime > parked_at && mtime < now;
        // #680: a mtime bump un-parks only on a payload CHANGE — a
        // same-bytes re-spool matches the parked episode's digest identity
        // and stays parked (the gate, not the clock, admits a
        // replacement; an mtime-keyed fresh apply would let a 1 Hz
        // mtime-bumper recreate one duplicate apply per pass forever).
        let admitted = bumped && parked_episode_rebinds(&name, &root, retries);
        if admitted {
            retries.parked.remove(&name);
        } else if mtime <= parked_at && mtime <= now || bumped {
            // Same bytes (or an unchanged-payload bump): the parked episode
            // continues — retry only the terminal move, never the domain
            // operation, so parked records still self-heal when the
            // evidence directory recovers.
            let mut moved = false;
            if let Some(snapshot) = RequestSnapshot::capture(&path) {
                if snapshot.metadata.modified().ok() == Some(mtime)
                    && snapshot.still_matches(&path)
                    && quarantine_with_warning(&path, &mut maintenance_warnings)
                {
                    retries.parked.remove(&name);
                    retries.attempts.remove(&name);
                    retries.stuck_episode_audited.remove(&name);
                    moved = true;
                }
            }
            // #680's ensure-presence on the gate path (round-10 R-2: on
            // every bump, moved or not — the record died unapplied either
            // way, so a truthful terminal answer is available in both
            // cases; round-12 minor 7: also when the quiet move SUCCEEDS —
            // a prior pass's marker-write IO failure must not leave the
            // moved record unanswered): the re-spool unlinked the failure
            // marker, or the pass that owned the marker write lost it to
            // IO — answer the waiting call from here. The SPECIFIC cause
            // lives in the audited episode and the Scheduled Tasks panel.
            if bumped || moved {
                let stem = name.strip_suffix(".json").unwrap_or(&name);
                if failure_marker_missing(stem) {
                    write_failure_marker(
                        stem,
                        &anyhow::anyhow!(
                            "the request could not be re-applied on the app side and its earlier failure marker was not delivered; check the Scheduled Tasks panel and the audit trail"
                        ),
                    );
                }
                // Round-12 minor 8: consume the bump — refresh the park
                // timestamp so one historical bump cannot keep `bumped`
                // (the probe, the restore attempt, and its warn) live on
                // every pass; a genuine later re-spool bumps again.
                if bumped {
                    retries
                        .parked
                        .insert(name.clone(), std::time::SystemTime::now());
                }
            }
        }
    }
    // Bounded in-memory evidence is backpressure, not a false terminal
    // ceiling failure for fresh work. Maintenance above releases it after IO
    // repair without an app restart or re-applying an exhausted operation.
    if retries.parked.len() + retries.receipts.len() >= MAX_PENDING_FILES {
        if !retries.storage_backpressure_logged {
            log::warn!(
                "[scheduled-creation] terminal evidence IO is blocked; fresh requests remain queued until storage recovers"
            );
            retries.storage_backpressure_logged = true;
        }
        return;
    }
    retries.storage_backpressure_logged = false;
    let mut files = pending_batch(&root, retries, listed);
    if files.is_empty() {
        retries.ceiling_warn_logged = false;
        return;
    }
    match crate::features::marketplace::builtin::feature_enabled_for_background_drain(
        "scheduled-task-automation",
    ) {
        Ok(enabled) => {
            retries.feature_gate_error_logged = false;
            if !enabled {
                return;
            }
        }
        Err(error) => {
            if !retries.feature_gate_error_logged {
                log::warn!(
                    "[scheduled-creation] cannot read feature switches; drain paused: {error:#}"
                );
                retries.feature_gate_error_logged = true;
            }
            return;
        }
    }
    // Selected-batch ceiling: at most 256 domain calls and 32 tail moves.
    // Later batches stay queued; there is no whole-directory sort/scan.
    // Round-27 originally mirrored the messaging sibling, which caps at 256 and
    // quarantines the sorted-tail excess — without it, a flood of planted
    // records becomes N real applies or N retained quarantines in one tick
    // (the design doc §7 "task explosion" mitigations lean on this bound).
    // Same ceiling and ordering: the first MAX drain in sorted order, the
    // excess quarantines. Round-30 MAJOR-4: the quarantine uses the shared
    // collision-suffix rename (never overwrite earlier evidence), and each
    // record gets a failure marker — a waiting caller whose record lands in
    // the tail must receive a terminal outcome, not time out into a
    // "pending" that is terminally dead.
    let mut quarantine_warnings = 0;
    let over_ceiling = files.len() > MAX_PENDING_FILES;
    if over_ceiling {
        let warn_this_pass = !retries.ceiling_warn_logged;
        let mut quarantine_failures = 0;
        // Round-31 minor 5: one warn per flood streak, not one per record.
        if !retries.ceiling_warn_logged {
            log::warn!(
                "[scheduled-creation] pending-file ceiling {MAX_PENDING_FILES} exceeded; quarantining the excess"
            );
            retries.ceiling_warn_logged = true;
        }
        for path in files
            .split_off(MAX_PENDING_FILES)
            .into_iter()
            .take(MAX_TAIL_QUARANTINES)
        {
            let Some(name) = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
            else {
                let _ = quarantine_with_warning(&path, &mut quarantine_warnings);
                continue;
            };
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            // #680: probe the REAL cause BEFORE the move (while the move
            // succeeds this is the only pass that handles the record; a
            // wedged rename re-probes ≤32 records per pass, cost disclosed).
            // A digest-bound recorded ok receipt
            // suppresses the failure marker entirely (the receipt answers
            // the waiting call); a corrupt tail record is "unparseable", an
            // unreadable/non-regular/oversize one says so — only the
            // genuinely-over-ceiling record keeps the ceiling label.
            let snapshot = RequestSnapshot::capture(&path);
            let probe = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.bytes.as_deref());
            let parsed = probe
                .and_then(|bytes| serde_json::from_slice::<SpooledCreationRequest>(bytes).ok());
            let ceiling_error = anyhow::anyhow!(
                "pending-file ceiling {MAX_PENDING_FILES} exceeded; the record was never applied"
            );
            let marker_reason: Option<anyhow::Error> = match (probe, &parsed) {
                (Some(_), Some(request)) => {
                    let suppressed = result_marker_suppresses(
                        &done_dir().join(format!("{stem}.json")),
                        &spool_request_digest(request),
                    );
                    if suppressed {
                        None
                    } else {
                        Some(ceiling_error)
                    }
                }
                (Some(_), None) => Some(anyhow::Error::msg(
                    "pending-ceiling tail record is unparseable",
                )),
                (None, _) => Some(anyhow::Error::msg(
                    "pending-ceiling tail record is unreadable, not a regular file, or oversize",
                )),
            };
            // Round-10 R-1 (#680): the sibling arms re-verify their snapshot
            // before the terminal move — so does the tail. A replacement
            // published between the probe and the rename must not be moved
            // and answered by the OLD bytes' marker/audit; it gets its own
            // fresh pass (an unreadable entry has no snapshot to diverge
            // and is moved with the honest label).
            if snapshot
                .as_ref()
                .is_some_and(|snapshot| !snapshot.still_matches(&path))
            {
                continue;
            }
            if quarantine_with_warning(&path, &mut quarantine_warnings) {
                // Best-effort trail: the disposition is audited when the
                // record parses AND carries a gate-valid sender (the tail
                // can hold legitimate-looking records — the trail must show
                // where they went and why); hostile bytes keep the failed/
                // evidence and the streak warn as the trace.
                if parsed.is_some() {
                    let audited_reason = marker_reason
                        .as_ref()
                        .map(|e| format!("{e:#}"))
                        .unwrap_or_else(|| "recorded ok receipt".to_string());
                    audit_failure(sessions, probe, &anyhow::anyhow!("{audited_reason}"));
                }
                if let Some(reason) = marker_reason {
                    write_failure_marker(&stem, &reason);
                }
                retries.attempts.remove(&name);
                retries.stuck_episode_audited.remove(&name);
            } else {
                // Persisted rename failure: leave the record queued and
                // say so; it stays outside this pass's budget either way.
                quarantine_failures += 1;
            }
        }
        if warn_this_pass && quarantine_failures > 0 {
            log::warn!(
                "[scheduled-creation] {quarantine_failures} excess records could not be quarantined and remain queued"
            );
        }
    }
    if !over_ceiling {
        // Round-31 minor 5: a pass under the ceiling ends the flood streak.
        retries.ceiling_warn_logged = false;
    }
    let mut poisons_this_pass: usize = 0;
    let pass_notifier = CoalescingNotifier::new(notifier);
    for path in files {
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            let _ = quarantine_with_warning(&path, &mut quarantine_warnings);
            continue;
        };
        if retries.receipts.contains_key(&name) {
            continue;
        }
        if retries.receipts.len() >= MAX_PENDING_FILES {
            break;
        }
        let tracked = retries.attempts.contains_key(&name) || retries.receipts.contains_key(&name);
        if !tracked
            && retries
                .attempts
                .keys()
                .chain(retries.parked.keys())
                .chain(retries.receipts.keys())
                .collect::<std::collections::HashSet<_>>()
                .len()
                >= MAX_PENDING_FILES
        {
            break;
        }
        // Round-24 minor 3: parked names skip until their record moves —
        // a re-spool at the same deterministic sha256 name (the
        // retry-after-fix shape) writes fresh bytes with a newer mtime
        // and un-parks; a vanished name un-parks too. The quarantine
        // failure that parked the name applied to the previous bytes.
        // #680: the un-park is admitted by the PAYLOAD DIGEST, not the
        // clock — a same-bytes mtime bump matches the parked episode's
        // identity and stays parked (the maintenance loop keeps retrying
        // only the terminal move for it); a corrected payload rebinds.
        if let Some(parked_at) = retries.parked.get(&name).copied() {
            match std::fs::metadata(&path).and_then(|meta| meta.modified()) {
                // A restored/future timestamp is not proof of a new request:
                // it would otherwise unpark again on every failed pass.
                Ok(mtime) if mtime > parked_at && mtime < std::time::SystemTime::now() => {
                    if parked_episode_rebinds(&name, &root, retries) {
                        retries.parked.remove(&name);
                    } else {
                        continue;
                    }
                }
                Ok(_) => continue,
                // The name left the spool; nothing to keep skipping.
                Err(_) => {
                    retries.parked.remove(&name);
                    continue;
                }
            }
        }
        // Round-21 M1 (the messaging sibling's round-8 M2 shape): the panic
        // guard is PER RECORD. The poll-level catch left a deterministic
        // panic at a fixed sorted position starving every later-sorted
        // record forever — silent, restart-surviving, and the record never
        // accrued budget (no quarantine, no failure marker, no audit). A
        // panicking record now burns its own budget and is quarantined
        // after MAX attempts, like any other persistent failure.
        let snapshot = RequestSnapshot::capture(&path);
        let outcome = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
            process_spool_file(path.as_path(), creator, sessions, &pass_notifier),
        ))
        .await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(panic) => {
                log::error!("[scheduled-creation] processing {name} panicked: {panic:?}");
                // Round-26 M3: catch_unwind converts the panic to
                // Processed::Retry HERE — past process_spool_file's own
                // Err arm, which is where the apply-failed shadow line
                // lives — so a panicking attempt used to be the one
                // failed-attempt shape the contract's "every failed
                // attempt leaves a shadow line" claim did not cover.
                // Re-read the record gated+bounded (the same trust
                // boundary as every spool read) and write the shadow
                // from it; an unreadable or diverged file keeps only the
                // log line above, which the contract row does not claim.
                if let Some((bytes, _meta)) =
                    read_regular_bounded_anchored(&path, MAX_SPOOL_FILE_BYTES as usize)
                {
                    if let Ok(request) = serde_json::from_slice::<SpooledCreationRequest>(&bytes) {
                        let stem = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or_default()
                            .to_string();
                        audit_request_shadow(
                            &request,
                            &stem,
                            request.task_id.as_deref().unwrap_or_default(),
                            request.name.as_deref().unwrap_or_default(),
                            "apply-panicked",
                        );
                    }
                }
                Processed::Retry(anyhow::anyhow!("processing panicked: {panic:?}"), None)
            }
        };
        match outcome {
            Processed::ReceiptPending(digest, payload) => {
                retries.receipts.insert(name, (digest, payload));
            }
            Processed::Done(applied_digest) => {
                // Round-16 MAJOR-1: the budget clears only after the record
                // actually left the spool. This arm used to clear it up
                // front, so a persisted removal failure re-ran the record's
                // removal loop on a fresh budget forever while the comment
                // below claimed the budget stays PUT.
                // Round-11 MAJOR-4: unlink only what was APPLIED. The
                // marker's digest binding answers a divergent retry
                // truthfully (payload_mismatch) only while the record is
                // alive; destroying whatever sits at the path untried left
                // the retrying call on a false "pending/queued" for its
                // whole wait window. An unreadable/diverged file stays for
                // the next poll (it fails the marker's digest gate and
                // applies afresh).
                // Round-12 MAJOR-1: this re-read runs exactly in the
                // replacement window the guard targets (after the awaited
                // apply) — the ungated std::fs::read would let a swap-to-
                // FIFO block the drain forever or a /dev/zero symlink grow
                // an unbounded Vec. Gated+bounded like every other read
                // (refusal counts as "not the applied record" → kept).
                // Round-24 minor 1: the re-read is also ANCHORED — the
                // unlink compares the name's current metadata against the
                // open handle's fstat, so a same-name replace landing
                // after the digest verified no longer deletes the retry's
                // corrected payload untried (the false-5 s-"pending"
                // degrade); it stays for the next poll. Residual: the
                // compare-then-unlink pair is not atomic — a replace in
                // that microscopic window still loses the new bytes, and
                // they retry; same-len same-mtime collisions are the FAT
                // granularity floor noted on `same_file_identity`.
                let applied = read_regular_bounded_anchored(&path, MAX_SPOOL_FILE_BYTES as usize)
                    .and_then(|(bytes, applied_meta)| {
                        serde_json::from_slice::<SpooledCreationRequest>(&bytes)
                            .ok()
                            .filter(|request| spool_request_digest(request) == applied_digest)
                            .map(|_| applied_meta)
                    });
                let Some(applied_meta) = applied else {
                    log::warn!(
                        "[scheduled-creation] {name} was replaced while its request was being applied; leaving the newest payload for the next poll"
                    );
                    continue;
                };
                match std::fs::symlink_metadata(&path) {
                    Ok(current) if same_file_identity(&current, &applied_meta) => {
                        if let Err(error) =
                            directory.remove_plain_file(path.file_name().unwrap_or_default())
                        {
                            // The marker exists, so the operation stays
                            // deduped and no poll can re-apply it — but
                            // the removal retries every poll until it
                            // lands. Round-15 REQUIRED-4, now actually
                            // true: the budget stays PUT on a persisted
                            // removal failure. Round-24 minor 4: the warn
                            // is once per streak (like the spool-dir
                            // path) — a stuck file warned at 1 Hz was
                            // 86k lines/day; the flag resets on success.
                            if !retries.removal_error_logged {
                                log::warn!(
                                    "[scheduled-creation] processed {name} but could not remove it: {error}"
                                );
                                retries.removal_error_logged = true;
                            }
                            continue;
                        }
                        retries.attempts.remove(&name);
                        retries.removal_error_logged = false;
                    }
                    // Replaced (or moved aside) after the digest verified:
                    // leave whatever sits at the name for the next poll.
                    Ok(_) => {
                        log::warn!(
                            "[scheduled-creation] {name} was replaced after its digest was verified; leaving the newest payload for the next poll"
                        );
                    }
                    Err(error) => {
                        log::warn!(
                            "[scheduled-creation] processed {name} but could not stat it for removal: {error}"
                        );
                    }
                }
            }
            Processed::Poison(error) => {
                // Round-31 minor 5: per-record detail up to the pass budget;
                // beyond it, one summary line. The failed/ evidence and each
                // record's failure marker carry the error either way, so a
                // re-plant flood cannot turn the drain into a log firehose.
                poisons_this_pass += 1;
                if poisons_this_pass <= POISON_WARN_PER_PASS {
                    log::warn!("[scheduled-creation] quarantining {name}: {error:#}");
                } else if poisons_this_pass == POISON_WARN_PER_PASS + 1 {
                    log::warn!(
                        "[scheduled-creation] further poison quarantines this pass are summarized; failed/ carries each record"
                    );
                }
                // #680: the audit line + failure marker fire ONCE per stuck
                // episode (a parked record re-spooled with the same bytes
                // un-parks, re-poisons, and used to re-append the jsonl and
                // rewrite the marker every pass). The episode identity is
                // the payload digest from the pre-read snapshot, and the
                // gate-arm rebind rule applies: a corrected same-name
                // re-spool — whose stale ok:false marker the SERVER unlinks
                // — differs and opens a fresh episode (fresh audit line +
                // marker) instead of being silenced by the stale entry.
                let current_digest = snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.bytes.as_deref())
                    .and_then(|bytes| serde_json::from_slice::<SpooledCreationRequest>(bytes).ok())
                    .map(|request| spool_request_digest(&request));
                let fresh_episode = opens_fresh_episode(
                    retries.stuck_episode_audited.get(&name),
                    current_digest.as_deref(),
                );
                if fresh_episode {
                    // The inserted identity is also the PARK's digest
                    // identity: the un-park gate (maintenance + skip
                    // branches) reads this same map.
                    retries
                        .stuck_episode_audited
                        .insert(name.clone(), current_digest.unwrap_or_default());
                    audit_failure(
                        sessions,
                        snapshot
                            .as_ref()
                            .and_then(|snapshot| snapshot.bytes.as_deref()),
                        &error,
                    );
                }
                // Round-11 M-1: skip a record whose entry VANISHED (nothing
                // at the name to answer), or one that diverged after the
                // snapshot (the replacement takes its own fresh pass). A
                // DANGLING LINK has an entry — it proceeds to the
                // quarantine/park and the marker below, the same terminal
                // answer base gave every unreadable record.
                if !path.symlink_metadata().is_ok()
                    || snapshot
                        .as_ref()
                        .is_some_and(|snapshot| !snapshot.still_matches(&path))
                {
                    continue;
                }
                // Round-16 MAJOR-1: the budget clears only when the record
                // actually left the spool; a persisted rename failure parks
                // the name so the drain skips it instead of re-poisoning
                // (and re-auditing) it every poll.
                if quarantine_with_warning(&path, &mut quarantine_warnings) {
                    retries.attempts.remove(&name);
                    retries.stuck_episode_audited.remove(&name);
                } else {
                    retries
                        .parked
                        .insert(name.clone(), std::time::SystemTime::now());
                }
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&name)
                    .to_string();
                if fresh_episode {
                    write_failure_marker(&stem, &error);
                } else if failure_marker_missing(&stem) {
                    // #680's ensure-presence: the deduped episode still
                    // repairs a MISSING marker. Reachable triggers: the
                    // PRIOR pass whose divergence/skip guards bypassed the
                    // write after inserting this identity (a mid-apply
                    // replacement, or the record vanished), and a prior
                    // marker-write IO failure; the audit line stays
                    // deduped, the waiting call does not
                    // (write_failure_marker still refuses a landed ok
                    // receipt).
                    write_failure_marker(&stem, &error);
                }
            }
            Processed::Retry(error, applied_digest) => {
                let count = {
                    let attempt = retries.attempts.entry(name.clone()).or_insert(0);
                    *attempt += 1;
                    *attempt
                };
                if count >= MAX_CREATE_ATTEMPTS {
                    log::warn!(
                        "[scheduled-creation] quarantining {name} after {count} create attempts: {error:#}"
                    );
                    // #680: the terminal audit + marker fire once per stuck
                    // episode (same rule as the Poison arm); the identity is
                    // the APPLIED bytes' digest, carried back from
                    // process_spool_file.
                    let current_digest = applied_digest.or_else(|| {
                        snapshot
                            .as_ref()
                            .and_then(|snapshot| snapshot.bytes.as_deref())
                            .and_then(|bytes| {
                                serde_json::from_slice::<SpooledCreationRequest>(bytes).ok()
                            })
                            .map(|request| spool_request_digest(&request))
                    });
                    let fresh_episode = opens_fresh_episode(
                        retries.stuck_episode_audited.get(&name),
                        current_digest.as_deref(),
                    );
                    if fresh_episode {
                        retries
                            .stuck_episode_audited
                            .insert(name.clone(), current_digest.clone().unwrap_or_default());
                        audit_failure(
                            sessions,
                            snapshot
                                .as_ref()
                                .and_then(|snapshot| snapshot.bytes.as_deref()),
                            &error,
                        );
                    }
                    // Round-11 M-1: see the Poison arm's guard — skip a
                    // VANISHED entry or a post-snapshot divergence; a
                    // dangling link proceeds to the terminal answer.
                    if !path.symlink_metadata().is_ok()
                        || snapshot
                            .as_ref()
                            .is_some_and(|snapshot| !snapshot.still_matches(&path))
                    {
                        continue;
                    }
                    // Round-16 MAJOR-1: the budget clears only when the
                    // record actually left the spool. Clearing before the
                    // rename let a persisted quarantine failure re-apply
                    // the record on a fresh budget every poll; a failure
                    // now parks the name instead (see RetryState.parked).
                    if quarantine_with_warning(&path, &mut quarantine_warnings) {
                        retries.attempts.remove(&name);
                        // #680: the episode entry clears on the move like
                        // every other terminal arm — a lingering identity
                        // would silence a fresh same-name episode that
                        // lands within this pass's retain window.
                        retries.stuck_episode_audited.remove(&name);
                    } else {
                        retries
                            .parked
                            .insert(name.clone(), std::time::SystemTime::now());
                        // The parked episode's identity: the applied bytes'
                        // digest — the un-park gate (not the mtime) decides
                        // what a re-spool admits.
                        retries
                            .stuck_episode_audited
                            .insert(name.clone(), current_digest.unwrap_or_default());
                    }
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&name)
                        .to_string();
                    if fresh_episode {
                        write_failure_marker(&stem, &error);
                    } else if failure_marker_missing(&stem) {
                        // #680's ensure-presence (see the Poison arm): the
                        // deduped episode repairs a marker a prior pass's
                        // diverged skip never wrote.
                        write_failure_marker(&stem, &error);
                    }
                } else {
                    log::warn!(
                        "[scheduled-creation] create attempt {count} for {name} failed (will retry): {error:#}"
                    );
                }
            }
        }
    }
}

impl ScheduledTaskState {
    /// Spawn the creation-request watcher and park its guard on `self`. Uses
    /// `tauri::async_runtime::spawn` (not `tokio::spawn`): `boot_runtime`
    /// runs inside the tauri setup hook, outside any raw tokio context.
    pub(crate) fn start_creation_watcher(&mut self, app: tauri::AppHandle) {
        let cancel = CancellationToken::new();
        let state = self.clone();
        let token = cancel.clone();
        // Round-8 minor 4 (truthful justification): GUARD-DROP cancellation
        // is observed between passes, so a Drop-triggered stop never splits
        // an apply from its marker. Process EXIT is different — the spawned
        // task drops at its next await on runtime shutdown, which can fall
        // inside the apply→marker window; that span is exactly the
        // documented at-least-once duplicate window, not a guarantee of
        // pass completion (the JoinHandle is dropped detached either way).
        tauri::async_runtime::spawn(async move {
            let mut retries = RetryState::default();
            // Round-10 M1: seed the clock at now() — the previous
            // `now() - PRUNE_INTERVAL` is a checked subtraction on the
            // CLOCK_MONOTONIC boot clock, so an app start within 60 s of
            // boot (desktop autostart on a fast machine) panicked on the
            // first loop pass BEFORE the catch_unwind below, and the
            // detached watcher died silently for the session. First prune
            // one interval in; with a 14-day retention the delay is
            // harmless (the messaging sibling seeds the same plain now()).
            let mut last_prune = tokio::time::Instant::now();
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(POLL_INTERVAL) => {}
                }
                if last_prune.elapsed() >= PRUNE_INTERVAL {
                    // Round-24 M2: prune runs inside its own sync
                    // catch_unwind — a bookkeeping panic must not kill the
                    // detached watcher task (whose Drop would cancel the
                    // panel scheduler app-wide). The per-record isolation
                    // inside process_pending_spool covers the drain path;
                    // this guard covers the prune.
                    if let Err(panic) =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(prune_stale_state))
                    {
                        log::error!("[scheduled-creation] prune panicked: {panic:?}");
                    }
                    last_prune = tokio::time::Instant::now();
                }
                // Round-6 minor 4 / round-21 M1: the panic isolation now
                // lives PER RECORD inside process_pending_spool (the
                // messaging sibling's round-8 M2 shape — the whole-poll
                // catch starved every later-sorted record of a
                // deterministic panicking record). This outer catch remains
                // as defense for panics outside the per-record path (prune,
                // loop bookkeeping). The state refs are bound outside the
                // future so the catch_unwind wrapper stays simple.
                let creator = StateCreator(&state);
                let notifier = EventNotifier(&app);
                let poll =
                    process_pending_spool(&creator, &state.sessions, &notifier, &mut retries);
                if let Err(panic) =
                    futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(poll)).await
                {
                    log::error!("[scheduled-creation] drain poll panicked: {panic:?}");
                }
            }
        });
        let mut slot = self
            .creation_watch
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        *slot = Some(CreationWatchGuard { cancel });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // tasks.rs's private imports (stores, ParkingMutex, AutomationManager)
    // are visible to this descendant module.
    use super::super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as SyncMutex};

    /// Round-6 MAJOR 4 (Rust side): the same committed golden vectors — a
    /// serializer/key-set/coercion drift on either side of the language
    /// boundary turns this red against the literal hex.
    #[test]
    fn spool_request_digest_golden_vectors() {
        let request = SpooledCreationRequest {
            schema_version: 1,
            kind: SpoolRequestKind::Create,
            id: "irrelevant".to_string(),
            task_id: None,
            target_session: None,
            name: Some("早报".to_string()),
            prompt: Some("汇总".to_string()),
            rrule: Some("FREQ=DAILY;BYHOUR=8".to_string()),
            model_id: None,
            paused: Some(false),
            from_session: Some("src".to_string()),
            created_at: String::new(),
            idempotency_key: Some("k".to_string()),
        };
        assert_eq!(
            spool_request_digest(&request),
            "e3ee5656710820acad7daa187ed50fe7a2295020d3d66f5bd37fc7afea42f755"
        );
        // Round-27 minor 1: the CREATE paused coercion is pinned — the
        // server coerces None→false before digesting, and this vector (the
        // same create with paused: None) must hash to the SAME hex as the
        // Some(false) vector above. The other vectors feed Some(...) or a
        // non-create kind, so deleting the unwrap_or(false) arm kept the
        // whole module green while a Rust regression would surface
        // cross-language as payload_mismatch on keyed replays.
        let mut omitted = request.clone();
        omitted.paused = None;
        assert_eq!(
            spool_request_digest(&omitted),
            "e3ee5656710820acad7daa187ed50fe7a2295020d3d66f5bd37fc7afea42f755",
            "a create with paused omitted must digest as paused:false (the server's coercion)"
        );
        let mut update = request;
        update.kind = SpoolRequestKind::Update;
        update.task_id = Some("t-9".to_string());
        update.name = None;
        update.prompt = None;
        update.rrule = None;
        update.model_id = Some("m1".to_string());
        update.paused = Some(true);
        assert_eq!(
            spool_request_digest(&update),
            "80d24d63000aa38aa3f4c1866155e42e91d57594189996443d336fc2754bfca6"
        );
        // Round-6 minor 6: a NON-NULL target_session vector — the first two
        // hashed only the null form, so a value-normalization divergence
        // (one side trimming/case-folding before hashing) stayed green.
        let mut retarget = update.clone();
        retarget.target_session = Some("sess-7777".to_string());
        assert_eq!(
            spool_request_digest(&retarget),
            "87f414a68c6034ca1d625fc05f682c8f0811aef01752f9b4cf1d140a5aac6af3"
        );
    }

    /// RAII temp PINVOU3_HOME; the ENV_LOCK guard is held across awaits —
    /// safe on the current-thread test runtime, and other suites' env use is
    /// serialized by the same lock.
    struct TempHome {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev: Option<String>,
        dir: std::path::PathBuf,
    }

    impl TempHome {
        fn new() -> Self {
            let lock = crate::platform::paths::tests::ENV_LOCK
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let dir = std::env::temp_dir().join(format!(
                "pinvou3-scheduled-creation-{}-{}",
                std::process::id(),
                crate::platform::paths::tests::unique_suffix()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var("PINVOU3_HOME").ok();
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
            TempHome {
                _lock: lock,
                prev,
                dir,
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.prev {
                // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
                Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
                // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
                None => unsafe { std::env::remove_var("PINVOU3_HOME") },
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn spool_record_json(overrides: &[(&str, serde_json::Value)]) -> String {
        let mut value = serde_json::json!({
            "schema_version": 1,
            "kind": "create",
            "id": "abc123",
            "task_id": null,
            "name": "早报任务",
            "prompt": "汇总今天的新闻",
            "rrule": "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30",
            "model_id": null,
            "paused": false,
            "from_session": "reqsrc01",
            "from_title": "请求来源",
            "created_at": "2026-09-29T00:00:00Z",
            "idempotency_key": null,
        });
        if let Some(object) = value.as_object_mut() {
            for (key, field) in overrides {
                object.insert((*key).to_string(), field.clone());
            }
        }
        value.to_string()
    }

    /// A minimal real state: the domain pipeline (AutomationManager + sidecar
    /// stores + fallback model) without the TaskManager/pool, which
    /// `create_task` never touches.
    async fn watcher_state() -> ScheduledTaskState {
        let sessions = crate::features::sessions::SessionStore::boot_for_process_startup()
            .expect("sessions store");
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("read state");
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())
            .expect("model bindings");
        let task_kinds =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("task kinds");
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())
            .expect("ui metadata");
        let history_archive = ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
            .expect("history archive");
        let manager = AutomationManager::open(scheduled_automation_root()).expect("automations");
        ScheduledTaskState {
            automations: Arc::new(tokio::sync::Mutex::new(manager)),
            task_manager: None,
            sessions,
            read_state,
            model_bindings,
            task_kinds,
            ui_metadata,
            history_archive,
            operation_locks: Arc::new(ParkingMutex::new(HashMap::new())),
            pool: None,
            fallback_model: "fallback-model".to_string(),
            scheduler_cancel: None,
            scheduler_handle: Arc::new(SyncMutex::new(None)),
            retention_handle: Arc::new(SyncMutex::new(None)),
            creation_watch: Arc::new(SyncMutex::new(None)),
        }
    }

    struct CountingNotifier(Arc<AtomicUsize>);

    impl PanelNotifier for CountingNotifier {
        fn notify(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Always-failing creator (the "disk read-only" of C4).
    struct FailingCreator;

    impl TaskCreator for FailingCreator {
        async fn create(
            &self,
            _input: CreateScheduledTaskInput,
        ) -> std::result::Result<ScheduledTaskDto, String> {
            Err("simulated persistent failure".to_string())
        }

        async fn update(
            &self,
            _task_id: &str,
            _input: UpdateScheduledTaskInput,
        ) -> std::result::Result<ScheduledTaskDto, String> {
            Err("simulated persistent failure".to_string())
        }

        async fn delete(&self, _task_id: &str) -> std::result::Result<ScheduledTaskDto, String> {
            Err("simulated persistent failure".to_string())
        }
    }

    fn valid_record() -> SpooledCreationRequest {
        serde_json::from_str(&spool_record_json(&[])).unwrap()
    }

    #[test]
    fn rrule_subset_accepts_product_shapes() {
        for rrule in [
            "FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
            "freq=weekly;byday=mo,we;byhour=9;byminute=30",
            " FREQ=ONCE;AT=2099-06-01T09:30 ",
            "FREQ=HOURLY",
            "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=0;BYMINUTE=59",
        ] {
            let normalized = validate_product_rrule(rrule)
                .unwrap_or_else(|e| panic!("{rrule} should pass: {e}"));
            assert_eq!(normalized, rrule.trim().to_ascii_uppercase());
        }
    }

    #[test]
    fn rrule_subset_rejects_cron_and_minute_granular() {
        for rrule in [
            "FREQ=CRON;EXPR=*/5 * * * *",
            "FREQ=MINUTELY;INTERVAL=5",
            "FREQ=DAILY;BYHOUR=8;BYMINUTE=30",
            "FREQ=HOURLY;INTERVAL=0",
            "FREQ=WEEKLY;BYHOUR=8;BYMINUTE=30",
            "FREQ=WEEKLY;BYDAY=XX;BYHOUR=8;BYMINUTE=30",
            "FREQ=WEEKLY;BYDAY=MO;BYHOUR=24;BYMINUTE=30",
            "FREQ=WEEKLY;BYDAY=MO;BYHOUR=8;BYMINUTE=60",
            // Round-30 REQUIRED-1: empty BYDAY tokens (trailing/double
            // comma — a classic LLM artifact) are rejected here like the
            // server does, instead of being filtered into an accepted
            // shape the domain then poisons after a fully-answered create.
            "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU,;BYHOUR=8;BYMINUTE=30",
            "FREQ=WEEKLY;BYDAY=MO,,TU;BYHOUR=8;BYMINUTE=30",
            "FREQ=WEEKLY;BYDAY=MO,;BYHOUR=8;BYMINUTE=30",
            "FREQ=WEEKLY;BYDAY=,;BYHOUR=8;BYMINUTE=30",
            "FREQ=ONCE;AT=2020-01-01T09:30",
            "FREQ=ONCE;AT=2099-06-01T09:30Z",
            "FREQ=ONCE;AT=2099-06-01T09:30+08:00",
            "FREQ=ONCE;AT=2099-06-01T09:30:30",
            "FREQ=ONCE;AT=2099-02-30T09:30",
            "FREQ=MONTHLY;BYDAY=1MO",
            "nonsense",
            "FREQ=HOURLY;WILDCARD=1",
            // Duplicate keys are ambiguous across the language boundary.
            "FREQ=ONCE;FREQ=HOURLY;AT=2099-06-01T09:30",
            // Oversize rrule is capped like every other field.
            &format!("FREQ=HOURLY;INTERVAL=2;{}", "X".repeat(MAX_RRULE_CHARS)),
        ] {
            assert!(
                validate_product_rrule(rrule).is_err(),
                "{rrule} should be rejected"
            );
        }
        // The positive arm of the same gate: the domain accepts the forms
        // that survive both layers (a here-filtered empty token used to
        // break this parity end to end).
        let surviving = spool_record_json(&[(
            "rrule",
            serde_json::json!("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30"),
        )]);
        let record: SpooledCreationRequest = serde_json::from_str(&surviving).unwrap();
        assert!(
            record.validate().is_ok(),
            "the surviving BYDAY form applies"
        );
    }

    /// Round-30 MAJOR-4: the pending-file ceiling's tail quarantine writes
    /// a failure marker for every record (a waiting caller whose record
    /// lands in the tail must receive a terminal outcome, not time out
    /// into a "pending" that is terminally dead) via the shared
    /// collision-suffix quarantine. The fixture uses a failing creator so
    /// the first MAX records stay queued (attempt-1 failures) and only the
    /// tail exercises the ceiling; removing write_failure_marker or
    /// reverting to the plain rename turns this red.
    #[tokio::test]
    async fn pending_ceiling_quarantines_the_tail_with_failure_markers() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        for i in 0..(MAX_PENDING_FILES + 8) {
            std::fs::write(spool.join(format!("f{i:04}.json")), spool_record_json(&[])).unwrap();
        }
        let mut retries = RetryState::default();
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            retries.ceiling_warn_logged,
            "an over-ceiling pass keeps the streak latched"
        );
        for i in (MAX_PENDING_FILES)..(MAX_PENDING_FILES + 8) {
            let name = format!("f{i:04}.json");
            assert!(
                failed_dir().join(&name).exists(),
                "the sorted tail beyond the ceiling quarantines: {name}"
            );
            assert!(!spool.join(&name).exists());
            let marker: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(done_dir().join(&name)).expect("failure marker"),
            )
            .unwrap();
            assert_eq!(marker["ok"], false, "{marker}");
            assert!(
                marker["error"]
                    .as_str()
                    .unwrap()
                    .contains("pending-file ceiling"),
                "the marker names the ceiling: {marker}"
            );
            assert_eq!(
                retries.attempts.get(&name).copied(),
                None,
                "the quarantined tail buys no retry budget"
            );
        }
        // The first MAX records: attempt-1 failures stay queued for the
        // budget (the ceiling is on pending files, not on deliveries).
        assert_eq!(
            retries.attempts.len(),
            MAX_PENDING_FILES,
            "the head of the list keeps its budget"
        );
    }

    /// Round-30 S7: the app's dead-tool deny (`automation`/`send_later`
    /// pushed onto the app session's disallow list in `lib.rs`, so the
    /// model sees one working path instead of a dead foundation pair) had
    /// no pin — deleting both pushes kept every test green. The production
    /// span is sliced the round-6 MAJOR-3 way: from the dead-tool comment
    /// to the first `#[cfg(test)]`, so the test module's own literals
    /// cannot self-satisfy an include_str-based pin.
    #[test]
    fn lib_rs_denies_the_dead_foundation_automation_tools() {
        let source = include_str!("../../lib.rs");
        // lib.rs scatters `#[cfg(test)]` modules from early on, so the
        // production-half bound is wrong here; the span runs from the
        // dead-tool comment to the next statement block, which contains
        // the pushes and no test module.
        let span = source
            .split("Base automation tools are dead in app sessions")
            .nth(1)
            .expect("the dead-tool deny site present")
            .split("match EnginePool::new_with_dependencies")
            .next()
            .expect("the deny closure is followed by the engine pool build");
        // Round-34 M4 (related): raw-source matching self-satisfies on
        // COMMENTED-OUT pushes — strip line-comment tails before matching
        // so commenting a deny line cannot satisfy the pin.
        let live = span
            .split('\n')
            .map(|line| line.split("//").next().unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n");
        for tool in ["automation", "send_later"] {
            let push = format!("tools.push(\"{tool}\".to_string());");
            assert!(
                live.contains(&push),
                "lib.rs must deny the dead foundation tool '{tool}'"
            );
        }
    }

    #[tokio::test]
    async fn creates_task_writes_marker_audits_and_notifies() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        // The audit append is open-and-append (no mkdir) in production too: a
        // live session's workspace already exists. Mirror that here.
        std::fs::create_dir_all(
            crate::platform::paths::sessions_root()
                .join("reqsrc01")
                .join("workspace"),
        )
        .unwrap();
        std::fs::write(spool.join("abc.json"), spool_record_json(&[])).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(calls.clone()),
            &mut retries,
        )
        .await;

        // The task landed in the very same store the panel reads.
        let records = state.automations.lock().await.list_automations().unwrap();
        assert_eq!(records.len(), 1, "exactly one task created");
        assert_eq!(records[0].name, "早报任务");
        assert_eq!(
            records[0].rrule,
            "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30"
        );
        assert!(
            records[0].next_run_at.is_some(),
            "active tasks get a scheduled slot"
        );
        // Result marker for the MCP server's synchronous wait.
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("abc.json")).expect("done marker"),
        )
        .unwrap();
        assert_eq!(marker["ok"], true);
        assert_eq!(marker["kind"], "create");
        assert_eq!(marker["task_id"], records[0].id);
        assert_eq!(marker["task_name"], "早报任务");
        // Round-24 minor 8: the digest binding pinned on a PRODUCTION-
        // written marker — every prior digest pin fed hand-written
        // markers, so a joint rename of the payload key and the reader
        // would ship green and surface only at runtime as keyed retries
        // degrading to "pending" forever.
        let spooled: SpooledCreationRequest =
            serde_json::from_str(&spool_record_json(&[])).unwrap();
        assert_eq!(
            marker["request_digest"],
            spool_request_digest(&spooled),
            "the ok marker must carry the canonical request digest"
        );
        // Spool file consumed, panel notified.
        assert!(!spool.join("abc.json").exists());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Audit trail in the requesting session's execution root (D3).
        let audit_path = crate::platform::paths::sessions_root()
            .join("reqsrc01")
            .join("workspace")
            .join("workflow_audit.jsonl");
        let audit = std::fs::read_to_string(audit_path).expect("audit record");
        let line: serde_json::Value = serde_json::from_str(audit.lines().next().unwrap()).unwrap();
        assert_eq!(line["kind"], "scheduled_task_create");
        assert_eq!(line["detail"]["tool"], SCHEDULED_TASK_CREATE_TOOL);
        assert_eq!(line["detail"]["task_id"], records[0].id);
        assert_eq!(line["detail"]["outcome"], "create");
        // Round-19 MAJOR-2: the honesty stamp rides the SUCCESS line too —
        // the sender is a model-supplied claim on every audit surface, not
        // only on failures.
        assert_eq!(line["detail"]["claimed_from_session_verified"], false);
    }

    #[tokio::test]
    async fn model_id_binds_and_paused_lands_paused() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            spool.join("bound.json"),
            spool_record_json(&[
                ("model_id", serde_json::json!("saved-model-7")),
                ("paused", serde_json::json!(true)),
            ]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        let records = state.automations.lock().await.list_automations().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].status,
            deepseek_tui::automation_manager::AutomationStatus::Paused,
            "paused:true creates a paused task (A7)"
        );
        assert!(
            records[0].next_run_at.is_none(),
            "paused tasks do not schedule"
        );
        let model = records[0].model.clone().expect("model");
        let binding = state
            .model_bindings
            .model_id_for(&records[0].id, &model)
            .expect("model binding persisted (A6)");
        assert_eq!(binding, "saved-model-7");
    }

    #[tokio::test]
    async fn tampered_spool_is_quarantined_without_creating() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        // Oversize prompt (B9) — beyond the cap the server enforces.
        std::fs::write(
            spool.join("big.json"),
            spool_record_json(&[(
                "prompt",
                serde_json::json!("x".repeat(MAX_PROMPT_CHARS + 1)),
            )]),
        )
        .unwrap();
        // Invalid rrule smuggled past the server (B9).
        std::fs::write(
            spool.join("cron.json"),
            spool_record_json(&[("rrule", serde_json::json!("FREQ=CRON;EXPR=*/5 * * * *"))]),
        )
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(calls.clone()),
            &mut retries,
        )
        .await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "tampered records never reach creation"
        );
        assert!(
            failed_dir().join("big.json").exists() && failed_dir().join("cron.json").exists(),
            "both are quarantined"
        );
        let records = state.automations.lock().await.list_automations().unwrap();
        assert!(
            records.is_empty(),
            "no task may be created from tampered spool"
        );
    }

    #[tokio::test]
    async fn done_marker_suppresses_recreation_across_restarts() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("abc.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        let sessions = state.sessions.clone();
        process_pending_spool(
            &StateCreator(&state),
            &sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert_eq!(
            state
                .automations
                .lock()
                .await
                .list_automations()
                .unwrap()
                .len(),
            1
        );
        // Restart replay: the same spool file reappears (e.g. crash between
        // delivery and file removal) but the marker exists → no second task.
        std::fs::write(spool.join("abc.json"), spool_record_json(&[])).unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert_eq!(
            state
                .automations
                .lock()
                .await
                .list_automations()
                .unwrap()
                .len(),
            1,
            "the done marker suppresses recreation"
        );
        assert!(
            !spool.join("abc.json").exists(),
            "stale spool file is consumed"
        );
    }

    #[tokio::test]
    async fn persistent_failure_retries_then_quarantines_with_failure_marker() {
        let _home = TempHome::new();
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            spool.join("stuck.json"),
            spool_record_json(&[("from_session", serde_json::json!("reqsrc01"))]),
        )
        .unwrap();
        std::fs::create_dir_all(
            crate::platform::paths::sessions_root()
                .join("reqsrc01")
                .join("workspace"),
        )
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut retries = RetryState::default();
        let sessions = crate::features::sessions::SessionStore::boot_for_process_startup()
            .expect("sessions store");
        for poll in 1..=MAX_CREATE_ATTEMPTS {
            process_pending_spool(
                &FailingCreator,
                &sessions,
                &CountingNotifier(calls.clone()),
                &mut retries,
            )
            .await;
            // Round-11 MAJOR-7: the mid-poll shape is pinned, not just the
            // terminal one — quarantining on the FIRST attempt (the
            // at-most-once mutation of the threshold) turned this green
            // before. Polls 1..N-1 must leave the record queued in the
            // spool with the budget reflected; only poll N quarantines.
            if poll < MAX_CREATE_ATTEMPTS {
                assert!(
                    spool.join("stuck.json").exists(),
                    "poll {poll}: a transient failure must stay queued (at-least-once)"
                );
                assert!(
                    !failed_dir().join("stuck.json").exists(),
                    "poll {poll}: nothing quarantines before the budget is spent"
                );
                assert_eq!(
                    retries.attempts.get("stuck.json").copied(),
                    Some(poll),
                    "poll {poll}: the attempt budget counts the failure"
                );
            }
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(retries.attempts.len(), 0, "terminal path clears state");
        assert!(failed_dir().join("stuck.json").exists(), "quarantined");
        assert!(!spool.join("stuck.json").exists());
        // Round-22 M4: the exhaustion arm's per-session failure line is
        // pinned (only the poison arm had a line pin before).
        let audit_path = crate::platform::paths::sessions_root()
            .join("reqsrc01")
            .join("workspace")
            .join("workflow_audit.jsonl");
        let audit = std::fs::read_to_string(audit_path).expect("session failure line");
        let line: serde_json::Value = serde_json::from_str(audit.lines().next().unwrap()).unwrap();
        assert_eq!(line["kind"], "scheduled_task_failed");
        assert_eq!(line["detail"]["outcome"], "failed");
        // The waiting MCP call receives the failure through the marker.
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("stuck.json")).expect("failure marker"),
        )
        .unwrap();
        assert_eq!(marker["ok"], false);
        assert!(
            marker["error"]
                .as_str()
                .unwrap()
                .contains("simulated persistent failure")
        );
        // Round-19 MAJOR-2: the apply-failed SHADOW line is pinned — the
        // body's "every applied and apply-failed request" claim rides on
        // it, and deleting the Err-arm shadow write shipped green.
        use crate::features::scheduled::tasks::scheduled_automation_root;
        let shadow = std::fs::read_to_string(
            scheduled_automation_root()
                .join("audit")
                .join("workflow_audit.jsonl"),
        )
        .expect("the shadow trail records the failures");
        let apply_failed = shadow
            .lines()
            .filter(|line| {
                let value: serde_json::Value = serde_json::from_str(line).unwrap();
                value["detail"]["outcome"] == "apply-failed"
            })
            .count();
        assert!(
            apply_failed >= 1,
            "the apply-failed disposition must reach the shadow trail: {shadow}"
        );
    }

    #[tokio::test]
    async fn applied_request_retries_only_receipt_when_terminal_dirs_are_blocked() {
        for remove_record in [false, true] {
            let _home = TempHome::new();
            let state = watcher_state().await;
            std::fs::create_dir_all(spool_root()).unwrap();
            std::fs::write(done_dir(), b"blocked").unwrap();
            std::fs::write(failed_dir(), b"blocked").unwrap();
            std::fs::write(spool_root().join("stuck.json"), spool_record_json(&[])).unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let mut retries = RetryState::default();
            for _ in 0..8 {
                process_pending_spool(
                    &StateCreator(&state),
                    &state.sessions,
                    &CountingNotifier(calls.clone()),
                    &mut retries,
                )
                .await;
            }
            assert_eq!(
                state
                    .automations
                    .lock()
                    .await
                    .list_automations()
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "applied changes notify even with no receipt"
            );
            assert!(retries.receipts.contains_key("stuck.json"));
            assert!(
                retries.attempts.is_empty(),
                "receipt failures do not spend another apply budget"
            );
            if remove_record {
                std::fs::remove_file(spool_root().join("stuck.json")).unwrap();
            }
            std::fs::remove_file(done_dir()).unwrap();
            std::fs::remove_file(failed_dir()).unwrap();
            process_pending_spool(
                &StateCreator(&state),
                &state.sessions,
                &CountingNotifier(calls),
                &mut retries,
            )
            .await;
            assert!(done_dir().join("stuck.json").is_file());
            assert!(!spool_root().join("stuck.json").exists());
            assert!(retries.receipts.is_empty());
            assert_eq!(
                state
                    .automations
                    .lock()
                    .await
                    .list_automations()
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    struct PanickingCreator;

    impl TaskCreator for PanickingCreator {
        async fn create(
            &self,
            _input: CreateScheduledTaskInput,
        ) -> std::result::Result<ScheduledTaskDto, String> {
            panic!("simulated domain panic");
        }

        async fn update(
            &self,
            _task_id: &str,
            _input: UpdateScheduledTaskInput,
        ) -> std::result::Result<ScheduledTaskDto, String> {
            panic!("simulated domain panic");
        }

        async fn delete(&self, _task_id: &str) -> std::result::Result<ScheduledTaskDto, String> {
            panic!("simulated domain panic");
        }
    }

    #[tokio::test]
    async fn panicking_record_burns_budget_and_does_not_starve_later_records() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        // "aaa" sorts BEFORE "zzz": the pre-round-21 whole-poll catch died
        // at "aaa" every pass, so "zzz" was never processed.
        std::fs::write(spool.join("aaa.json"), spool_record_json(&[])).unwrap();
        std::fs::write(spool.join("zzz.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        for _ in 0..MAX_CREATE_ATTEMPTS {
            process_pending_spool(
                &PanickingCreator,
                &state.sessions,
                &CountingNotifier(Arc::new(AtomicUsize::new(0))),
                &mut retries,
            )
            .await;
        }
        // BOTH records exhausted and quarantined: "zzz" reaching failed/
        // is the starvation proof — the pre-round-21 whole-poll catch died
        // at "aaa" every pass, so "zzz" was never processed at all. The
        // budgets cleared because both quarantines succeeded (the failed/
        // directory is writable in this fixture).
        assert!(failed_dir().join("aaa.json").exists(), "aaa quarantined");
        assert!(
            failed_dir().join("zzz.json").exists(),
            "zzz quarantined — not starved"
        );
        assert!(!spool.join("aaa.json").exists());
        assert!(!spool.join("zzz.json").exists());
        assert!(
            retries.attempts.is_empty(),
            "terminal paths clear the budgets"
        );
    }

    /// Round-26 M3: the contract's "every failed attempt leaves a shadow
    /// line; only a pure poison is silent" used to be false for panicking
    /// attempts — catch_unwind converted the panic to Processed::Retry in
    /// the POLL loop, past process_spool_file's Err arm where the
    /// apply-failed shadow lives. The panic arm now re-reads the record
    /// and writes the shadow itself; deleting that re-read + write turns
    /// this red.
    #[tokio::test]
    async fn panicking_attempt_leaves_a_shadow_audit_line() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("aaa.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &PanickingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        let shadow = crate::features::scheduled::tasks::scheduled_automation_root()
            .join("audit")
            .join("workflow_audit.jsonl");
        let lines = std::fs::read_to_string(&shadow)
            .expect("the panicking attempt must create the shadow audit");
        let line = lines
            .lines()
            .find(|line| line.contains("apply-panicked"))
            .expect("the panicking attempt must leave an apply-panicked shadow line");
        let entry: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            entry["detail"]["spool_stem"], "aaa",
            "the shadow keys the spool stem, not the record's self-description"
        );
        assert_eq!(
            entry["detail"]["claimed_from_session_verified"], false,
            "the claimed sender stays explicitly unverified"
        );
    }

    // Round-22 M1-pin: POSIX-only (read-only directory chmod blocks the
    // unlink deterministically; Windows has no equivalent one-liner).
    #[cfg(unix)]
    #[tokio::test]
    async fn private_directory_permission_repair_removes_without_duplicate_apply() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::create_dir_all(done_dir()).unwrap();
        std::fs::write(spool.join("stuck.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        // Poll 1: a transient apply failure — the budget entry exists.
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert_eq!(retries.attempts.get("stuck.json").copied(), Some(1));
        // Block the unlink: the spool directory goes read-only.
        let mut perms = std::fs::metadata(&spool).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt as _;
        perms.set_mode(0o555);
        std::fs::set_permissions(&spool, perms).unwrap();
        // Poll 2: the apply succeeds, the marker lands in `.done/`, and
        // the unlink fails.
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            done_dir().join("stuck.json").exists(),
            "the marker lands despite the blocked unlink"
        );
        assert!(
            !spool.join("stuck.json").exists(),
            "the private-directory primitive restores owner permissions before removal"
        );
        assert_eq!(
            retries.attempts.get("stuck.json").copied(),
            None,
            "successful removal clears the old transient budget"
        );
        // Restore the directory so the marker suppresses the next poll:
        // no re-creation, and the parked-in-place record is deduped.
        let mut perms = std::fs::metadata(&spool).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&spool, perms).unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert_eq!(
            state
                .automations
                .lock()
                .await
                .list_automations()
                .unwrap()
                .len(),
            1,
            "the marker suppresses re-creation while the removal retries"
        );
        // The restored poll completes the lifecycle: the queued record is
        // consumed and the budget clears — the PUT only ever bounded the
        // removal-failure window.
        assert!(!spool.join("stuck.json").exists(), "the record is consumed");
        assert_eq!(
            retries.attempts.get("stuck.json").copied(),
            None,
            "a successful removal clears the budget"
        );
    }

    // Round-17 MAJOR-4: the Poison arm's half — the budget clears only on
    // quarantine success. An entry driven via one retry, then the record
    // swapped to poison while `failed/` is blocked: the budget stays PUT
    // and the name parks. Clearing before the rename (the round-15 shape)
    // re-poisoned on a fresh budget every poll.
    #[cfg(unix)]
    #[tokio::test]
    async fn poison_arm_keeps_the_budget_when_quarantine_fails() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("failed"), b"blocked\n").unwrap();
        std::fs::write(spool.join("swap.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        // Poll 1: a transient apply failure — the budget entry exists.
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert_eq!(retries.attempts.get("swap.json").copied(), Some(1));
        // Swap the record to poison (schema drift: CRON smuggled past the
        // server — the watcher's mirror validation rejects it).
        std::fs::write(
            spool.join("swap.json"),
            r#"{"schema_version":1,"id":"swap","kind":"create","name":"x","prompt":"p","rrule":"FREQ=CRON;EXPR=* * * * *"}"#,
        )
        .unwrap();
        // Poll 2: poison → quarantine fails (failed/ blocked).
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            retries.parked.contains_key("swap.json"),
            "a failed poison quarantine parks the name"
        );
        assert_eq!(
            retries.attempts.get("swap.json").copied(),
            Some(1),
            "the Poison arm clears only on quarantine success"
        );
        assert!(spool.join("swap.json").exists(), "the record stays queued");
        assert_eq!(
            state
                .automations
                .lock()
                .await
                .list_automations()
                .unwrap()
                .len(),
            0,
            "a poisoned record never applies"
        );
    }

    /// Counts create() calls and delegates to the real domain pipeline —
    /// the un-park gate's pin must observe the DOMAIN re-apply, not just
    /// the final list.
    struct CountingStateCreator<'a>(&'a ScheduledTaskState, Arc<AtomicUsize>);

    impl TaskCreator for CountingStateCreator<'_> {
        async fn create(
            &self,
            input: CreateScheduledTaskInput,
        ) -> std::result::Result<ScheduledTaskDto, String> {
            self.1.fetch_add(1, Ordering::SeqCst);
            self.0.create_task(input).await
        }

        async fn update(
            &self,
            task_id: &str,
            input: UpdateScheduledTaskInput,
        ) -> std::result::Result<ScheduledTaskDto, String> {
            self.0.update_task(task_id.to_string(), input).await
        }

        async fn delete(&self, task_id: &str) -> std::result::Result<ScheduledTaskDto, String> {
            self.0.delete_task(task_id.to_string()).await
        }
    }

    /// #680 (R2's reproduced-red case): a poison-parked record whose caller
    /// re-spools a DIFFERENT payload — the server unlinks the stale
    /// ok:false marker, the fresh mtime un-parks, and the parked episode's
    /// digest identity must REBIND: a fresh audit line + fresh marker, not
    /// the stale entry's silence.
    #[tokio::test]
    async fn poison_park_corrected_respool_opens_a_fresh_episode() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("failed"), b"blocked\n").unwrap();
        let exec_root = crate::platform::paths::sessions_root()
            .join("reqsrc01")
            .join("workspace");
        std::fs::create_dir_all(&exec_root).unwrap();
        let audit_lines = || {
            std::fs::read_to_string(exec_root.join("workflow_audit.jsonl"))
                .map(|content| content.lines().count())
                .unwrap_or(0)
        };
        let poison_a =
            spool_record_json(&[("rrule", serde_json::json!("FREQ=CRON;EXPR=* * * * *"))]);
        std::fs::write(spool.join("swap.json"), &poison_a).unwrap();
        let marker_path = done_dir().join("swap.json");
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(retries.parked.contains_key("swap.json"), "poll 1 parks");
        assert_eq!(audit_lines(), 1, "poll 1 audits the episode");
        std::fs::read_to_string(&marker_path).expect("poll 1 writes the marker");
        let audited_before = retries
            .stuck_episode_audited
            .get("swap.json")
            .cloned()
            .expect("poll 1 stores the poison's digest identity");
        assert!(
            !audited_before.is_empty(),
            "a parseable poison stores a REAL digest identity"
        );
        // Corrected (different) payload, still poison; the server unlinks
        // the stale marker on re-spool and the rewrite bumps the mtime.
        // Deterministic timing (main's own un-park test's pattern): pin the
        // parked timestamp to a controlled past instant and stamp the
        // rewrite 1s after it — a fast runner's mtime granularity must not
        // swallow the bump (the natural write raced the park clock on CI).
        let parked_at = std::time::SystemTime::now() - std::time::Duration::from_secs(2);
        *retries.parked.get_mut("swap.json").expect("parked") = parked_at;
        let corrected =
            spool_record_json(&[("rrule", serde_json::json!("FREQ=CRON;EXPR=0 5 * * *"))]);
        std::fs::write(spool.join("swap.json"), &corrected).unwrap();
        std::fs::File::options()
            .write(true)
            .open(spool.join("swap.json"))
            .unwrap()
            .set_modified(parked_at + std::time::Duration::from_secs(1))
            .unwrap();
        std::fs::remove_file(&marker_path).unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert_eq!(
            audit_lines(),
            2,
            "the corrected payload rebinds: a fresh episode appends its own audit line"
        );
        let rebound: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&marker_path).expect("the fresh episode's marker"),
        )
        .unwrap();
        assert_eq!(rebound["ok"], false);
        let corrected_request =
            serde_json::from_slice::<SpooledCreationRequest>(corrected.as_bytes()).unwrap();
        assert_eq!(
            retries.stuck_episode_audited.get("swap.json"),
            Some(&spool_request_digest(&corrected_request)),
            "the episode identity rebinds onto the corrected payload's digest"
        );
        assert!(
            retries.parked.contains_key("swap.json"),
            "the fresh episode parks again (failed/ still blocked)"
        );
    }

    /// #680: a poison-parked record whose caller re-spools the SAME bytes
    /// — the un-park gate keeps it parked (no second audit line), and the
    /// marker the server unlinked is RESTORED for the waiting call.
    #[tokio::test]
    async fn poison_park_same_bytes_bump_stays_parked_and_marker_is_restored() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("failed"), b"blocked\n").unwrap();
        let exec_root = crate::platform::paths::sessions_root()
            .join("reqsrc01")
            .join("workspace");
        std::fs::create_dir_all(&exec_root).unwrap();
        let audit_lines = || {
            std::fs::read_to_string(exec_root.join("workflow_audit.jsonl"))
                .map(|content| content.lines().count())
                .unwrap_or(0)
        };
        let poison_record =
            spool_record_json(&[("rrule", serde_json::json!("FREQ=CRON;EXPR=* * * * *"))]);
        std::fs::write(spool.join("swap.json"), &poison_record).unwrap();
        let marker_path = done_dir().join("swap.json");
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(retries.parked.contains_key("swap.json"), "poll 1 parks");
        assert_eq!(audit_lines(), 1);
        // Same bytes re-spooled (fresh mtime); the server unlinks the stale
        // not-ok marker. Deterministic timing: see the corrected-respool
        // test — a natural write raced the park clock on CI.
        let parked_at = std::time::SystemTime::now() - std::time::Duration::from_secs(2);
        *retries.parked.get_mut("swap.json").expect("parked") = parked_at;
        std::fs::write(spool.join("swap.json"), &poison_record).unwrap();
        std::fs::File::options()
            .write(true)
            .open(spool.join("swap.json"))
            .unwrap()
            .set_modified(parked_at + std::time::Duration::from_secs(1))
            .unwrap();
        std::fs::remove_file(&marker_path).unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            retries.parked.contains_key("swap.json"),
            "the same-bytes bump does NOT un-park (the gate, not the clock)"
        );
        assert_eq!(
            audit_lines(),
            1,
            "the same-bytes re-poison stays deduped in the audit trail"
        );
        let restored: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&marker_path)
                .expect("the unlinked marker is restored for the waiting call"),
        )
        .unwrap();
        assert_eq!(restored["ok"], false);
        assert!(
            restored["error"]
                .as_str()
                .unwrap()
                .contains("could not be re-applied"),
            "the gate-path restore states the re-spool truth: {:?}",
            restored["error"]
        );
        // A quiet pass changes nothing.
        let restored_text = std::fs::read_to_string(&marker_path).unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert_eq!(audit_lines(), 1);
        assert_eq!(
            std::fs::read_to_string(&marker_path).unwrap(),
            restored_text,
            "the restored marker is not rewritten per pass"
        );
        // Round-10 R-2: the restore also fires when the SAME bump's move
        // SUCCEEDS — the record died unapplied either way, and the waiting
        // call gets its terminal answer.
        std::fs::remove_file(spool.join("failed")).unwrap();
        std::fs::write(spool.join("swap.json"), &poison_record).unwrap();
        std::fs::File::options()
            .write(true)
            .open(spool.join("swap.json"))
            .unwrap()
            .set_modified(parked_at + std::time::Duration::from_secs(2))
            .unwrap();
        std::fs::remove_file(&marker_path).unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            !spool.join("swap.json").exists() && failed_dir().join("swap.json").exists(),
            "the bump's move lands once failed/ works"
        );
        let restored_after_move: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&marker_path)
                .expect("the restore fires on the moved path too"),
        )
        .unwrap();
        assert_eq!(restored_after_move["ok"], false);
        assert_eq!(
            audit_lines(),
            1,
            "the moved same-bytes episode stays deduped in the audit trail"
        );
        assert!(retries.stuck_episode_audited.is_empty());
    }

    /// #680 round-12 minor 8: the bumped gate-path restore consumes the
    /// bump — the park timestamp is refreshed, so one historical bump
    /// cannot keep the probe + restore attempt alive on every pass.
    #[tokio::test]
    async fn bumped_restore_consumes_the_bump() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("failed"), b"blocked\n").unwrap();
        std::fs::write(spool.join("swap.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        for _ in 0..MAX_CREATE_ATTEMPTS {
            process_pending_spool(
                &FailingCreator,
                &state.sessions,
                &CountingNotifier(Arc::new(AtomicUsize::new(0))),
                &mut retries,
            )
            .await;
        }
        assert!(retries.parked.contains_key("swap.json"));
        // Controlled clock: pin the park to a past instant so the bump's
        // stamp is neither future nor swallowed by fs granularity.
        let parked_before = std::time::SystemTime::now() - std::time::Duration::from_secs(2);
        *retries.parked.get_mut("swap.json").expect("parked") = parked_before;
        // Same-bytes re-spool: the gate keeps the record parked and the
        // restore runs — and consumes the bump.
        std::fs::write(spool.join("swap.json"), spool_record_json(&[])).unwrap();
        std::fs::File::options()
            .write(true)
            .open(spool.join("swap.json"))
            .unwrap()
            .set_modified(parked_before + std::time::Duration::from_secs(1))
            .unwrap();
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        let parked_after = *retries.parked.get("swap.json").expect("still parked");
        assert!(
            parked_after > parked_before,
            "the restore consumes the bump: the park timestamp is refreshed"
        );
        // A quiet pass after the refresh does not re-attempt the restore:
        // the marker is untouched.
        let marker_path = done_dir().join("swap.json");
        let before = std::fs::read_to_string(&marker_path).expect("restored marker");
        let before_mtime = std::fs::metadata(&marker_path).unwrap().modified().unwrap();
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            std::fs::metadata(&marker_path).unwrap().modified().unwrap() == before_mtime,
            "the refreshed park stops the per-pass restore churn"
        );
        let _ = before;
    }

    /// #680 round-12 minor 11: a CORRUPT marker is repaired like a missing
    /// one — the server's poll cannot read it, so leaving it in place
    /// answers every same-key retry with "pending" forever.
    #[tokio::test]
    async fn corrupt_marker_repaired_by_the_gate_path() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("failed"), b"blocked\n").unwrap();
        std::fs::write(spool.join("stuck.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        for _ in 0..MAX_CREATE_ATTEMPTS {
            process_pending_spool(
                &FailingCreator,
                &state.sessions,
                &CountingNotifier(Arc::new(AtomicUsize::new(0))),
                &mut retries,
            )
            .await;
        }
        assert!(retries.parked.contains_key("stuck.json"));
        let marker_path = done_dir().join("stuck.json");
        std::fs::read_to_string(&marker_path).expect("poll 3 writes the marker");
        // Controlled clock (see bumped_restore_consumes_the_bump).
        let parked_at = std::time::SystemTime::now() - std::time::Duration::from_secs(2);
        *retries.parked.get_mut("stuck.json").expect("parked") = parked_at;
        // Corrupt the marker (hostile or partial write), then re-spool the
        // same bytes: the bump routes through the gate path and the repair
        // replaces the unreadable marker with a readable terminal answer.
        std::fs::write(&marker_path, b"not json{").unwrap();
        std::fs::write(spool.join("stuck.json"), spool_record_json(&[])).unwrap();
        std::fs::File::options()
            .write(true)
            .open(spool.join("stuck.json"))
            .unwrap()
            .set_modified(parked_at + std::time::Duration::from_secs(1))
            .unwrap();
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        let repaired: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&marker_path)
                .expect("the corrupt marker is repaired for the waiting call"),
        )
        .unwrap();
        assert_eq!(repaired["ok"], false);
        assert!(
            repaired["error"]
                .as_str()
                .unwrap()
                .contains("could not be re-applied"),
            "the repair states the re-spool truth: {:?}",
            repaired["error"]
        );
    }

    /// #680: the parked episode's identity (and what budget there was)
    /// clears when the terminal move finally succeeds — a lingering
    /// identity would silence a fresh same-name episode.
    #[tokio::test]
    async fn poison_park_episode_clears_when_the_quarantine_finally_succeeds() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("failed"), b"blocked\n").unwrap();
        let exec_root = crate::platform::paths::sessions_root()
            .join("reqsrc01")
            .join("workspace");
        std::fs::create_dir_all(&exec_root).unwrap();
        let audit_lines = || {
            std::fs::read_to_string(exec_root.join("workflow_audit.jsonl"))
                .map(|content| content.lines().count())
                .unwrap_or(0)
        };
        std::fs::write(
            spool.join("swap.json"),
            spool_record_json(&[("rrule", serde_json::json!("FREQ=CRON;EXPR=* * * * *"))]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(retries.parked.contains_key("swap.json"), "poll 1 parks");
        // Round-12 minor 7: the poisoning pass's marker write lost to IO
        // (simulated by removing the marker) — the quiet move below must
        // still leave the waiting call answered.
        std::fs::remove_file(done_dir().join("swap.json")).unwrap();
        // failed/ unlocks; no re-spool — the parked record's terminal move
        // is retried by the maintenance path itself.
        std::fs::remove_file(spool.join("failed")).unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            !spool.join("swap.json").exists() && failed_dir().join("swap.json").exists(),
            "the parked record's move is retried and lands"
        );
        assert_eq!(
            audit_lines(),
            1,
            "the maintenance move does not re-audit the episode"
        );
        assert!(retries.attempts.is_empty(), "the move clears the budget");
        assert!(retries.parked.is_empty(), "the move clears the park");
        assert!(
            retries.stuck_episode_audited.is_empty(),
            "the move clears the episode entry"
        );
        // The minor-7 restore: the moved record's missing marker is
        // delivered by the maintenance path.
        let restored: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("swap.json"))
                .expect("the moved record's missing marker is restored"),
        )
        .unwrap();
        assert_eq!(restored["ok"], false);
    }

    /// #680: the retry-exhausted park feeds the same gate — a same-bytes
    /// mtime bumper buys ZERO re-applies forever (the storm the gate
    /// exists to stop), while a corrected payload rebinds and applies.
    #[tokio::test]
    async fn retry_park_same_bytes_bump_never_reapplies_corrected_payload_rebinds() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(failed_dir(), b"blocked\n").unwrap();
        std::fs::write(spool.join("stuck.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        for _ in 0..MAX_CREATE_ATTEMPTS {
            process_pending_spool(
                &FailingCreator,
                &state.sessions,
                &CountingNotifier(Arc::new(AtomicUsize::new(0))),
                &mut retries,
            )
            .await;
        }
        assert!(
            retries.parked.contains_key("stuck.json"),
            "the exhausted record parks (quarantine blocked)"
        );
        assert!(
            !retries
                .stuck_episode_audited
                .get("stuck.json")
                .map(String::is_empty)
                .unwrap_or(true),
            "the park stores the attempted record's digest identity"
        );
        // Same bytes re-spooled: the gate must not grant a re-apply.
        // Deterministic timing: see the corrected-respool test — a natural
        // write raced the park clock on CI.
        let parked_at = std::time::SystemTime::now() - std::time::Duration::from_secs(2);
        *retries.parked.get_mut("stuck.json").expect("parked") = parked_at;
        std::fs::write(spool.join("stuck.json"), spool_record_json(&[])).unwrap();
        std::fs::File::options()
            .write(true)
            .open(spool.join("stuck.json"))
            .unwrap()
            .set_modified(parked_at + std::time::Duration::from_secs(1))
            .unwrap();
        let applies = Arc::new(AtomicUsize::new(0));
        process_pending_spool(
            &CountingStateCreator(&state, applies.clone()),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert_eq!(
            applies.load(Ordering::SeqCst),
            0,
            "a same-bytes mtime bump buys exactly zero re-applies"
        );
        assert!(retries.parked.contains_key("stuck.json"));
        // A corrected payload is a different digest: it rebinds and applies.
        // (Current mtime: the round-12 minor-8 refresh re-based the parked
        // timestamp, so a stamp from the old epoch would be quiet.)
        std::fs::write(
            spool.join("stuck.json"),
            spool_record_json(&[
                ("idempotency_key", serde_json::json!("k-corrected")),
                ("name", serde_json::json!("更正后的名字")),
            ]),
        )
        .unwrap();
        std::fs::File::options()
            .write(true)
            .open(spool.join("stuck.json"))
            .unwrap()
            .set_modified(std::time::SystemTime::now())
            .unwrap();
        process_pending_spool(
            &CountingStateCreator(&state, applies.clone()),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert_eq!(
            applies.load(Ordering::SeqCst),
            1,
            "the corrected payload rebinds and applies"
        );
        assert!(
            !spool.join("stuck.json").exists(),
            "the corrected record is consumed"
        );
    }

    /// #680: a CORRUPT tail record's marker names the real cause, not the
    /// ceiling label.
    #[tokio::test]
    async fn corrupt_ceiling_tail_names_the_real_cause() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        for i in 0..MAX_PENDING_FILES {
            std::fs::write(spool.join(format!("f{i:04}.json")), spool_record_json(&[])).unwrap();
        }
        std::fs::write(spool.join("f9999.json"), b"not json{").unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(failed_dir().join("f9999.json").exists());
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("f9999.json")).expect("failure marker"),
        )
        .unwrap();
        assert!(
            marker["error"].as_str().unwrap().contains("unparseable"),
            "the marker names the real cause: {marker}"
        );
    }

    /// #680: an oversize tail record's marker names the real cause.
    #[tokio::test]
    async fn oversize_ceiling_tail_names_the_real_cause() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        for i in 0..MAX_PENDING_FILES {
            std::fs::write(spool.join(format!("f{i:04}.json")), spool_record_json(&[])).unwrap();
        }
        let mut oversize = spool_record_json(&[]);
        oversize.insert_str(
            oversize.len() - 1,
            &format!(",\"pad\":\"{}\"", "x".repeat(MAX_SPOOL_FILE_BYTES as usize)),
        );
        std::fs::write(spool.join("f9999.json"), oversize).unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(failed_dir().join("f9999.json").exists());
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("f9999.json")).expect("failure marker"),
        )
        .unwrap();
        let error = marker["error"].as_str().unwrap();
        assert!(
            error.contains("unreadable") && error.contains("oversize"),
            "the marker names the real cause: {error}"
        );
        assert!(
            !error.contains("pending-file ceiling"),
            "the ceiling label must not mask the real cause: {error}"
        );
    }

    /// #680: a ceiling-tail record whose digest-bound ok receipt already
    /// landed writes NO failure marker (the receipt answers it), and the
    /// disposition still leaves an audit line.
    #[tokio::test]
    async fn ceiling_tail_with_landed_receipt_writes_no_failure_marker() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::create_dir_all(done_dir()).unwrap();
        for i in 0..MAX_PENDING_FILES {
            std::fs::write(spool.join(format!("f{i:04}.json")), spool_record_json(&[])).unwrap();
        }
        let tail_request =
            serde_json::from_slice::<SpooledCreationRequest>(spool_record_json(&[]).as_bytes())
                .unwrap();
        let digest = spool_request_digest(&tail_request);
        std::fs::write(
            done_dir().join("f9999.json"),
            serde_json::json!({"ok": true, "task_id": "already-there", "request_digest": digest})
                .to_string(),
        )
        .unwrap();
        std::fs::write(spool.join("f9999.json"), spool_record_json(&[])).unwrap();
        let exec_root = crate::platform::paths::sessions_root()
            .join("reqsrc01")
            .join("workspace");
        std::fs::create_dir_all(&exec_root).unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            failed_dir().join("f9999.json").exists(),
            "the tail is still quarantined (ceiling is terminal either way)"
        );
        let receipt: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("f9999.json")).expect("receipt intact"),
        )
        .unwrap();
        assert_eq!(
            receipt["ok"], true,
            "the landed receipt is not overwritten by a failure marker"
        );
        let audit = std::fs::read_to_string(exec_root.join("workflow_audit.jsonl"))
            .expect("the tail disposition is audited");
        assert!(
            audit.contains("recorded ok receipt"),
            "the receipt-answered tail's audit names the suppression: {audit}"
        );
    }

    /// #680 (the messaging sibling's round-4 C5 guard, ported): non-UTF-8
    /// spool names are quarantined ON SIGHT, before the batch count — they
    /// can never be processed or keyed, and counting them let planted
    /// squatters starve legitimately named records.
    /// #680 round-11 M-1 + m-D: the ghost filter is `symlink_metadata`-based,
    /// not `exists`-based. A VANISHED name is skipped (no junk episode opens
    /// for it), while a DANGLING SYMLINK at a record name has metadata, stays
    /// in the batch, and reaches the Poison arm for base's terminal answer —
    /// an `exists` filter would follow the link, drop it on every pass, and
    /// silently starve the waiting caller (ships this red).
    #[cfg(unix)]
    #[tokio::test]
    async fn vanished_ghost_skipped_and_dangling_link_answered_terminally() {
        use std::os::unix::fs::symlink;
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        // The vanished ghost: a tracked name whose file is gone this pass.
        let mut retries = RetryState::default();
        retries.attempts.insert("ghost.json".to_string(), 1);
        retries
            .stuck_episode_audited
            .insert("ghost.json".to_string(), "stale-digest".to_string());
        // The dangling link: metadata exists, the target does not — base
        // terminally refused it at the gated open.
        symlink(
            spool.join("nonexistent-target"),
            spool.join("dangling.json"),
        )
        .unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            !retries.stuck_episode_audited.contains_key("ghost.json")
                && !retries.attempts.contains_key("ghost.json"),
            "the vanished name is skipped and its stale entries retained away"
        );
        // The platform rename refuses symlinks (O_NOFOLLOW open), so the
        // terminal answer is the Poison arm's ok:false MARKER with the name
        // parked — not a successful move (an `exists` filter never reaches
        // this arm at all and ships this red).
        assert!(
            retries.parked.contains_key("dangling.json"),
            "the dangling link reaches the Poison arm and parks"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("dangling.json"))
                .expect("the waiting call gets a terminal answer"),
        )
        .unwrap();
        assert_eq!(marker["ok"], false);
        assert!(
            spool.join("dangling.json").symlink_metadata().is_ok(),
            "the link stays queued under its park (the platform rename refuses it)"
        );
        // Round-12 MAJOR-2: pass 2 — the symlink_metadata retains keep the
        // park and the episode identity alive for the dangling link (an
        // exists-based retain follows the link, dissolves both EVERY pass,
        // and the Poison arm rewrites the marker at 1 Hz forever).
        let marker_path = done_dir().join("dangling.json");
        let before = std::fs::read_to_string(&marker_path).unwrap();
        let before_mtime = std::fs::metadata(&marker_path).unwrap().modified().unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            retries.parked.contains_key("dangling.json"),
            "pass 2 keeps the park (the retain does not follow the link)"
        );
        assert!(
            std::fs::metadata(&marker_path).unwrap().modified().unwrap() == before_mtime,
            "pass 2 does not rewrite the marker — once per episode holds for a dangling link"
        );
        assert_eq!(std::fs::read_to_string(&marker_path).unwrap(), before);
    }

    /// #680 round-11 m-C: the Poison/Retry arms' ensure-presence repair
    /// branch — a pass whose snapshot diverged (a digest-preserving
    /// reformat) inserted the identity but skipped the marker write; the
    /// NEXT pass's deduped arm must repair the missing marker without
    /// re-auditing.
    #[tokio::test]
    async fn deduped_episode_repairs_a_marker_the_diverged_pass_skipped() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        let exec_root = crate::platform::paths::sessions_root()
            .join("reqsrc01")
            .join("workspace");
        std::fs::create_dir_all(&exec_root).unwrap();
        let audit_lines = || {
            std::fs::read_to_string(exec_root.join("workflow_audit.jsonl"))
                .map(|content| content.lines().count())
                .unwrap_or(0)
        };
        struct ReformattingFailureCreator {
            spool_path: std::path::PathBuf,
            applies: Arc<AtomicUsize>,
        }
        impl TaskCreator for ReformattingFailureCreator {
            async fn create(
                &self,
                _input: CreateScheduledTaskInput,
            ) -> std::result::Result<ScheduledTaskDto, String> {
                // Digest-preserving reformat on the EXHAUSTION apply only
                // (3rd call): pretty-printing changes the bytes — the loop's
                // snapshot diverges — while the parsed record, and its
                // payload digest, is intact. Later applies leave the bytes
                // stable so the repair pass's snapshot matches.
                if self.applies.fetch_add(1, Ordering::SeqCst) == MAX_CREATE_ATTEMPTS as usize - 1 {
                    let value: serde_json::Value =
                        serde_json::from_str(&std::fs::read_to_string(&self.spool_path).unwrap())
                            .unwrap();
                    std::fs::write(
                        &self.spool_path,
                        serde_json::to_string_pretty(&value).unwrap(),
                    )
                    .unwrap();
                }
                Err("simulated persistent failure".to_string())
            }
            async fn update(
                &self,
                _task_id: &str,
                _input: UpdateScheduledTaskInput,
            ) -> std::result::Result<ScheduledTaskDto, String> {
                Err("simulated persistent failure".to_string())
            }
            async fn delete(
                &self,
                _task_id: &str,
            ) -> std::result::Result<ScheduledTaskDto, String> {
                Err("simulated persistent failure".to_string())
            }
        }
        let creator = ReformattingFailureCreator {
            spool_path: spool.join("swap.json"),
            applies: Arc::new(AtomicUsize::new(0)),
        };
        std::fs::write(spool.join("swap.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        for _ in 0..MAX_CREATE_ATTEMPTS {
            process_pending_spool(
                &creator,
                &state.sessions,
                &CountingNotifier(Arc::new(AtomicUsize::new(0))),
                &mut retries,
            )
            .await;
        }
        assert_eq!(audit_lines(), 1, "one exhausted-episode audit line");
        assert!(
            !done_dir().join("swap.json").exists(),
            "the diverged pass inserted the identity but skipped the marker"
        );
        process_pending_spool(
            &creator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        let repaired: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("swap.json"))
                .expect("the deduped arm repairs the missing marker"),
        )
        .unwrap();
        assert_eq!(repaired["ok"], false);
        assert_eq!(
            audit_lines(),
            1,
            "the repair does not re-audit the deduped episode"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn non_utf8_squatters_do_not_starve_legitimate_records() {
        use std::os::unix::ffi::OsStringExt;
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        let mut squatters = Vec::new();
        for i in 0..MAX_PENDING_FILES {
            // A trailing 0xFF byte is never valid UTF-8; the .json suffix
            // passes the extension filter.
            let mut name_bytes = format!("sq{i:03}").into_bytes();
            name_bytes.push(0xFF);
            name_bytes.extend_from_slice(b".json");
            let name = std::ffi::OsString::from_vec(name_bytes);
            squatters.push(spool.join(&name));
            std::fs::write(spool.join(&name), b"{}\n").unwrap();
        }
        // Sorts after every squatter: without the on-sight quarantine it
        // falls past the batch window into the quarantined tail (or beyond
        // the enumeration window entirely).
        std::fs::write(spool.join("zzz.json"), spool_record_json(&[])).unwrap();
        let notify_count = Arc::new(AtomicUsize::new(0));
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(notify_count.clone()),
            &mut retries,
        )
        .await;
        assert_eq!(
            notify_count.load(Ordering::SeqCst),
            1,
            "the legitimate record applies — squatters did not consume its slot"
        );
        assert!(
            !spool.join("zzz.json").exists(),
            "the legitimate record is consumed, not tail-quarantined"
        );
        for squatter in &squatters {
            assert!(
                !squatter.exists() && failed_dir().join(squatter.file_name().unwrap()).exists(),
                "the non-UTF-8 squatter is quarantined on sight: {squatter:?}"
            );
        }
        assert!(
            retries.attempts.is_empty() && retries.stuck_episode_audited.is_empty(),
            "no keyed state leaks from the unnameable records"
        );
    }

    /// #680: the Poison arm's OWN post-move clear (quarantine succeeds on
    /// the same pass the episode opens) — a lingering identity would
    /// silence a fresh same-name episode. The maintenance-path clear has
    /// its own pin; this is the arm's.
    #[tokio::test]
    async fn poison_arm_clears_the_episode_entry_when_its_quarantine_succeeds() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        let exec_root = crate::platform::paths::sessions_root()
            .join("reqsrc01")
            .join("workspace");
        std::fs::create_dir_all(&exec_root).unwrap();
        let audit_lines = || {
            std::fs::read_to_string(exec_root.join("workflow_audit.jsonl"))
                .map(|content| content.lines().count())
                .unwrap_or(0)
        };
        std::fs::write(
            spool.join("swap.json"),
            spool_record_json(&[("rrule", serde_json::json!("FREQ=CRON;EXPR=* * * * *"))]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        // failed/ works: the poison arm quarantines on the same pass.
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            !spool.join("swap.json").exists() && failed_dir().join("swap.json").exists(),
            "the poison quarantines immediately"
        );
        assert_eq!(audit_lines(), 1, "exactly one episode audit line");
        assert!(
            retries.attempts.is_empty()
                && retries.parked.is_empty()
                && retries.stuck_episode_audited.is_empty(),
            "the arm's own move clears the budget, park, and episode state"
        );
        assert!(
            !retries.stuck_episode_audited.contains_key("swap.json"),
            "no episode identity outlives the move"
        );
    }

    /// #680: the retry-exhausted arm's OWN post-move clear — the review's
    /// "the only terminal arm without the post-move episode clear" finding,
    /// closed for every arm.
    #[tokio::test]
    async fn retry_arm_clears_the_episode_entry_when_its_quarantine_succeeds() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        let exec_root = crate::platform::paths::sessions_root()
            .join("reqsrc01")
            .join("workspace");
        std::fs::create_dir_all(&exec_root).unwrap();
        std::fs::write(spool.join("stuck.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        retries
            .attempts
            .insert("stuck.json".to_string(), MAX_CREATE_ATTEMPTS);
        // failed/ works: the exhausted record's terminal move succeeds on
        // the same pass the episode opens.
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            !spool.join("stuck.json").exists() && failed_dir().join("stuck.json").exists(),
            "the exhausted record quarantines immediately"
        );
        assert!(
            retries.attempts.is_empty()
                && retries.parked.is_empty()
                && retries.stuck_episode_audited.is_empty(),
            "the arm's own move clears the budget, park, and episode state"
        );
    }

    #[tokio::test]
    async fn id_field_is_never_trusted_for_watcher_paths() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        // Hostile id: traversal claims must not escape the .done namespace —
        // the marker is keyed by the file stem.
        std::fs::write(
            spool.join("hostile.json"),
            spool_record_json(&[("id", serde_json::json!("../../../../../../tmp/pinvou-evil"))]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            done_dir().join("hostile.json").exists(),
            "marker keyed by file stem"
        );
        assert!(
            !Path::new("/tmp/pinvou-evil.json").exists(),
            "no file may be created outside the done namespace"
        );
        let records = state.automations.lock().await.list_automations().unwrap();
        assert_eq!(records.len(), 1, "the request itself is still honored");
    }

    #[tokio::test]
    async fn scheduled_message_request_lands_kind_and_target() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        // A real ordinary session file so the domain's existence probe passes.
        let sessions_dir = crate::platform::paths::sessions_root();
        std::fs::create_dir_all(sessions_dir.join("reqsrc01").join("workspace")).unwrap();
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::write(
            sessions_dir.join("target001.json"),
            r#"{"schema_version":1,"metadata":{"id":"target001","title":"目标","created_at":"2026-09-29T00:00:00Z","updated_at":"2026-09-29T00:00:00Z","message_count":0,"total_tokens":0,"model":"test-model","workspace":"/tmp/target-workspace"},"messages":[],"system_prompt":null}"#,
        )
        .unwrap();
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            spool.join("msg.json"),
            spool_record_json(&[
                ("target_session", serde_json::json!("target001")),
                ("rrule", serde_json::json!("FREQ=ONCE;AT=2099-06-01T09:30")),
            ]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;

        let records = state.automations.lock().await.list_automations().unwrap();
        assert_eq!(records.len(), 1, "the scheduled-message task is created");
        assert_eq!(
            state.task_kinds.kind_for(&records[0].id).as_deref(),
            Some("session_message")
        );
        assert_eq!(
            state
                .task_kinds
                .target_session_for(&records[0].id)
                .as_deref(),
            Some("target001"),
            "the delivery target lands in the sidecar"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("msg.json")).expect("marker"),
        )
        .unwrap();
        assert_eq!(marker["kind"], "create");
        assert_eq!(marker["task_id"], records[0].id);
        // The audit line names the delivery target: it redirects where every
        // future prompt of this task fires.
        let audit_path = crate::platform::paths::sessions_root()
            .join("reqsrc01")
            .join("workspace")
            .join("workflow_audit.jsonl");
        let audit = std::fs::read_to_string(audit_path).expect("audit record");
        let line: serde_json::Value = serde_json::from_str(audit.lines().next().unwrap()).unwrap();
        assert_eq!(line["kind"], "scheduled_task_create");
        assert_eq!(line["detail"]["target_session"], "target001");
    }

    /// Round-4 M5b: the target re-check runs for EVERY kind — an update-kind
    /// record with a hostile target poisons too (the arm a create-only
    /// mutation would leave unguarded).
    #[tokio::test]
    async fn update_kind_hostile_target_is_quarantined() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            spool.join("upd-hostile.json"),
            spool_record_json(&[
                ("kind", serde_json::json!("update")),
                ("task_id", serde_json::json!("t-1")),
                ("target_session", serde_json::json!("sched-run9")),
            ]),
        )
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(calls.clone()),
            &mut retries,
        )
        .await;
        assert!(
            failed_dir().join("upd-hostile.json").exists(),
            "an update-kind hostile target must poison, not apply"
        );
        assert!(!spool.join("upd-hostile.json").exists());
    }

    /// Review round-15 N2: the typo-target update poisons at attempt 1
    /// through the LIVE drain path — "target_session not found" is a
    /// permanent-domain marker, so the budget stays untouched, the record
    /// leaves the spool, and the waiting MCP call gets its failure marker.
    #[tokio::test]
    async fn missing_target_update_poisons_at_attempt_1() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let sessions_dir = crate::platform::paths::sessions_root();
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::write(
            sessions_dir.join("target001.json"),
            r#"{"schema_version":1,"metadata":{"id":"target001","title":"目标","created_at":"2026-10-15T00:00:00Z","updated_at":"2026-10-15T00:00:00Z","message_count":0,"total_tokens":0,"model":"test-model","workspace":"/tmp/target-workspace"},"messages":[],"system_prompt":null}"#,
        )
        .unwrap();
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        // Land a real session-message task first (a chat task would die on
        // the kind guard before the target probe).
        std::fs::write(
            spool.join("mk.json"),
            spool_record_json(&[
                ("target_session", serde_json::json!("target001")),
                ("rrule", serde_json::json!("FREQ=ONCE;AT=2099-06-01T09:30")),
            ]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        let created = state
            .automations
            .lock()
            .await
            .list_automations()
            .unwrap()
            .pop()
            .expect("session-message task created");

        // The typo-target retarget: existence is probed at the domain, the
        // "not found" classification poisons on the first drain pass.
        std::fs::write(
            spool.join("typo.json"),
            spool_record_json(&[
                ("kind", serde_json::json!("update")),
                ("prompt", serde_json::Value::Null),
                ("rrule", serde_json::Value::Null),
                ("task_id", serde_json::json!(created.id)),
                ("target_session", serde_json::json!("no-such-target")),
            ]),
        )
        .unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            failed_dir().join("typo.json").exists(),
            "a missing target must quarantine on attempt 1"
        );
        assert!(!spool.join("typo.json").exists());
        assert_eq!(
            retries.attempts.get("typo.json").copied(),
            None,
            "attempt-1 poison must not buy retry budget"
        );
        assert_eq!(
            state.task_kinds.target_session_for(&created.id).as_deref(),
            Some("target001"),
            "the failed retarget leaves the old target in place"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("typo.json")).expect("failure marker"),
        )
        .unwrap();
        assert_eq!(marker["ok"], false);
        assert!(
            marker["error"].as_str().unwrap().contains("not found"),
            "{marker}"
        );
    }

    /// Review round-15 N2: an update/delete of an UNKNOWN task used to be
    /// retry-classified (the wrapped read miss matched no marker) and burned
    /// the whole 3-attempt budget on a permanent miss; the "No such file"
    /// marker poisons it at attempt 1.
    #[tokio::test]
    async fn unknown_task_update_poisons_at_attempt_1() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            spool.join("ghost.json"),
            spool_record_json(&[
                ("kind", serde_json::json!("update")),
                ("prompt", serde_json::Value::Null),
                ("rrule", serde_json::Value::Null),
                ("task_id", serde_json::json!("nosuch99")),
                ("name", serde_json::json!("改名")),
            ]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            failed_dir().join("ghost.json").exists(),
            "an unknown-task update must quarantine on attempt 1"
        );
        assert!(!spool.join("ghost.json").exists());
        assert_eq!(
            retries.attempts.get("ghost.json").copied(),
            None,
            "attempt-1 poison must not buy retry budget"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("ghost.json")).expect("failure marker"),
        )
        .unwrap();
        assert_eq!(marker["ok"], false);
        assert!(
            marker["error"]
                .as_str()
                .unwrap()
                .contains("scheduled task not found: nosuch99"),
            "{marker}"
        );
    }

    #[tokio::test]
    async fn unknown_task_delete_is_ensure_absent_success() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            spool.join("ghost.json"),
            spool_record_json(&[
                ("kind", serde_json::json!("delete")),
                ("task_id", serde_json::json!("nosuch99")),
                ("name", serde_json::Value::Null),
                ("prompt", serde_json::Value::Null),
                ("rrule", serde_json::Value::Null),
                ("paused", serde_json::Value::Null),
            ]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(!failed_dir().join("ghost.json").exists());
        assert!(!spool.join("ghost.json").exists());
        assert!(!retries.attempts.contains_key("ghost.json"));
        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(done_dir().join("ghost.json")).unwrap()).unwrap();
        assert_eq!(
            marker["ok"], true,
            "delete retry converges to absence: {marker}"
        );
        assert_eq!(marker["task_id"], "nosuch99");
    }

    #[tokio::test]
    async fn tampered_message_target_is_quarantined() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        // An unattended target smuggled past the server is the recursion
        // direction — quarantined without creating anything. Round-6
        // minor 7 (honest scope): this pins the SYSTEM invariant (nothing
        // created, every shape quarantined) — it does NOT attribute the
        // quarantine to the watcher's own checks, because the domain
        // re-check poisons the same records; the watcher-only coverage is
        // pinned by the unit tests on the individual check fns.
        std::fs::write(
            spool.join("selfwake.json"),
            spool_record_json(&[("target_session", serde_json::json!("sched-run1"))]),
        )
        .unwrap();
        std::fs::write(
            spool.join("selfwake-upper.json"),
            spool_record_json(&[("target_session", serde_json::json!("AUX-side1"))]),
        )
        .unwrap();
        std::fs::write(
            spool.join("selfwake-eval.json"),
            spool_record_json(&[("target_session", serde_json::json!("EVAL_b1"))]),
        )
        .unwrap();
        std::fs::write(
            spool.join("padded.json"),
            spool_record_json(&[("target_session", serde_json::json!("target 001"))]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(failed_dir().join("selfwake.json").exists());
        assert!(
            state
                .automations
                .lock()
                .await
                .list_automations()
                .unwrap()
                .is_empty(),
            "no task may be created from a hostile target"
        );
    }

    #[tokio::test]
    async fn target_only_update_spool_retargets_the_task() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let sessions_dir = crate::platform::paths::sessions_root();
        std::fs::create_dir_all(&sessions_dir).unwrap();
        for id in ["target001", "target002"] {
            let snapshot = format!(
                r#"{{"schema_version":1,"metadata":{{"id":"{id}","title":"目标","created_at":"2026-09-29T00:00:00Z","updated_at":"2026-09-29T00:00:00Z","message_count":0,"total_tokens":0,"model":"test-model","workspace":"/tmp/target-workspace"}},"messages":[],"system_prompt":null}}"#
            );
            std::fs::write(sessions_dir.join(format!("{id}.json")), snapshot).unwrap();
        }
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            spool.join("msg.json"),
            spool_record_json(&[
                ("target_session", serde_json::json!("target001")),
                ("rrule", serde_json::json!("FREQ=ONCE;AT=2099-06-01T09:30")),
            ]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        let created = state
            .automations
            .lock()
            .await
            .list_automations()
            .unwrap()
            .pop()
            .expect("session-message task created");
        assert_eq!(
            state.task_kinds.target_session_for(&created.id).as_deref(),
            Some("target001")
        );

        // The typical retarget call: an update whose ONLY changed field is
        // target_session. Before the Update-arm fix this record died as
        // poison with no marker while the caller was told "pending".
        std::fs::write(
            spool.join("retarget.json"),
            spool_record_json(&[
                ("kind", serde_json::json!("update")),
                ("task_id", serde_json::json!(created.id)),
                ("name", serde_json::Value::Null),
                ("prompt", serde_json::Value::Null),
                ("rrule", serde_json::Value::Null),
                ("paused", serde_json::Value::Null),
                ("target_session", serde_json::json!("target002")),
            ]),
        )
        .unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert_eq!(
            state.task_kinds.target_session_for(&created.id).as_deref(),
            Some("target002"),
            "the target-only update retargeted the task"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("retarget.json")).expect("marker"),
        )
        .unwrap();
        assert_eq!(marker["ok"], true);
        assert!(!spool.join("retarget.json").exists());
    }

    #[tokio::test]
    async fn success_marker_suppresses_a_replayed_request() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::create_dir_all(done_dir()).unwrap();
        // A replay after a completed create: the spool file re-lands (the
        // server rewrites it), but the success marker wins — no second
        // task. Round-7 M1b (rebase onto #628's digest gate): the plant
        // must be digest-BOUND to the record — the gate refuses bare
        // {"ok":true} markers now.
        let digest = spool_request_digest(
            &serde_json::from_str::<SpooledCreationRequest>(&spool_record_json(&[])).unwrap(),
        );
        std::fs::write(
            done_dir().join("replay.json"),
            serde_json::json!({"ok": true, "task_id": "already-there", "request_digest": digest})
                .to_string(),
        )
        .unwrap();
        std::fs::write(spool.join("replay.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            state
                .automations
                .lock()
                .await
                .list_automations()
                .unwrap()
                .is_empty(),
            "the success marker suppresses the replay (C3)"
        );
        assert!(!spool.join("replay.json").exists(), "the replay is dropped");
    }

    #[test]
    fn sender_validation_rejects_isolated_and_malformed_ids() {
        let mut request = valid_record();
        for sender in ["sched-run1", "SCHED-run1"] {
            request.from_session = Some(sender.to_string());
            request.idempotency_key = Some("interactive-retry".to_string());
            assert!(
                request.validate().is_ok(),
                "interactive scheduled origin: {sender}"
            );
        }
        request.idempotency_key = None;
        // Round-15 REQUIRED-1: the eval_ leg folds case — the uppercase
        // variant used to pass while the server rejected it.
        request.from_session = Some("EVAL_b1".to_string());
        assert!(
            request.validate().is_err(),
            "uppercase EVAL_ senders are rejected (case-folded)"
        );
        request.from_session = Some("AUX-side1".to_string());
        assert!(request.validate().is_err(), "AUX- variants are rejected");
        // Headless `agentic_` senders join the isolated set (both the
        // lowercase and the mixed-case form pin the case folding — the
        // scheduled family's sender red arm for the fourth family).
        request.from_session = Some("agentic_4242_0".to_string());
        assert!(request.validate().is_err(), "agentic_ senders are rejected");
        request.from_session = Some("AGENTIC_x".to_string());
        assert!(
            request.validate().is_err(),
            "uppercase AGENTIC_ senders are rejected (case-folded)"
        );
        request.from_session = Some("../escape".to_string());
        assert!(request.validate().is_err(), "traversal ids are rejected");
        request.from_session = None;
        assert!(
            request.validate().is_ok(),
            "absent from_session stays valid (no audit trail)"
        );
    }

    #[test]
    fn update_request_validation_requires_target_and_a_field() {
        let mut request = valid_record();
        request.kind = SpoolRequestKind::Update;
        request.task_id = None;
        assert!(request.validate().is_err(), "update requires a target id");
        request.task_id = Some("../escape".to_string());
        assert!(
            request.validate().is_err(),
            "traversal target ids are rejected"
        );
        request.task_id = Some("task-1".to_string());
        request.name = None;
        request.prompt = None;
        request.rrule = None;
        assert!(
            request.validate().is_ok(),
            "paused-only update is a valid no-op field set"
        );
        request.paused = None;
        request.target_session = Some("target001".to_string());
        assert!(
            request.validate().is_ok(),
            "a target-only retarget is a valid update (the typical call)"
        );
        request.target_session = None;
        assert!(
            request.validate().is_err(),
            "update with no field to change is rejected"
        );
        request.rrule = Some("FREQ=CRON;EXPR=*/5 * * * *".to_string());
        assert!(
            request.validate().is_err(),
            "update rrule obeys the product subset"
        );
    }

    #[test]
    fn delete_request_validation_rejects_extra_fields() {
        let mut request = valid_record();
        request.kind = SpoolRequestKind::Delete;
        request.task_id = Some("task-1".to_string());
        request.name = None;
        request.prompt = None;
        request.rrule = None;
        request.paused = None;
        assert!(request.validate().is_ok(), "a bare delete is valid");
        request.name = Some("x".to_string());
        assert!(
            request.validate().is_err(),
            "delete with extra fields is rejected"
        );
        request.name = None;
        request.paused = Some(false);
        assert!(
            request.validate().is_err(),
            "delete rejects a stray paused even when false"
        );
    }

    #[tokio::test]
    async fn update_request_changes_the_existing_task() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let created = state
            .create_task(CreateScheduledTaskInput {
                name: "早报任务".to_string(),
                prompt: "汇总今天的新闻".to_string(),
                rrule: "FREQ=WEEKLY;BYDAY=MO;BYHOUR=8;BYMINUTE=30".to_string(),
                cwds: Vec::new(),
                model: None,
                model_id: None,
                target_session: None,
                kind: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                paused: Some(false),
            })
            .await
            .expect("seed task");
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            spool.join("upd.json"),
            spool_record_json(&[
                ("kind", serde_json::json!("update")),
                ("task_id", serde_json::json!(created.id)),
                ("name", serde_json::json!("晚报任务")),
                ("paused", serde_json::json!(true)),
                ("prompt", serde_json::Value::Null),
                ("rrule", serde_json::Value::Null),
            ]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        let updated = state
            .automations
            .lock()
            .await
            .get_automation(&created.id)
            .expect("task still exists");
        assert_eq!(updated.name, "晚报任务");
        assert_eq!(
            updated.rrule, "FREQ=WEEKLY;BYDAY=MO;BYHOUR=8;BYMINUTE=30",
            "safe metadata update preserves the existing schedule"
        );
        assert!(
            matches!(
                updated.status,
                deepseek_tui::automation_manager::AutomationStatus::Paused
            ),
            "paused:true update pauses the task"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("upd.json")).expect("marker"),
        )
        .unwrap();
        assert_eq!(marker["kind"], "update");
        assert_eq!(marker["task_id"], created.id);
    }

    #[tokio::test]
    async fn delete_and_crash_before_receipt_replays_are_successful() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let created = state
            .create_task(CreateScheduledTaskInput {
                name: "早报任务".to_string(),
                prompt: "汇总今天的新闻".to_string(),
                rrule: "FREQ=WEEKLY;BYDAY=MO;BYHOUR=8;BYMINUTE=30".to_string(),
                cwds: Vec::new(),
                model: None,
                model_id: None,
                target_session: None,
                kind: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                paused: Some(true),
            })
            .await
            .expect("seed task");
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            spool.join("del.json"),
            spool_record_json(&[
                ("kind", serde_json::json!("delete")),
                ("task_id", serde_json::json!(created.id)),
                ("name", serde_json::Value::Null),
                ("prompt", serde_json::Value::Null),
                ("rrule", serde_json::Value::Null),
                ("paused", serde_json::Value::Null),
            ]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            state
                .automations
                .lock()
                .await
                .get_automation(&created.id)
                .is_err()
        );
        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(done_dir().join("del.json")).unwrap()).unwrap();
        assert_eq!(marker["ok"], true);
        assert_eq!(marker["task_id"], created.id);
        // Simulate a crash after deletion but before receipt publication. A
        // replay and a different delete key must both ensure absence successfully.
        std::fs::remove_file(done_dir().join("del.json")).unwrap();
        for key in ["del", "different-key"] {
            std::fs::write(
                spool.join(format!("{key}.json")),
                spool_record_json(&[
                    ("kind", serde_json::json!("delete")),
                    ("task_id", serde_json::json!(created.id)),
                    ("name", serde_json::Value::Null),
                    ("prompt", serde_json::Value::Null),
                    ("rrule", serde_json::Value::Null),
                    ("paused", serde_json::Value::Null),
                ]),
            )
            .unwrap();
            process_pending_spool(
                &StateCreator(&state),
                &state.sessions,
                &CountingNotifier(Arc::new(AtomicUsize::new(0))),
                &mut retries,
            )
            .await;
            let replay: serde_json::Value = serde_json::from_slice(
                &std::fs::read(done_dir().join(format!("{key}.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(replay["ok"], true, "{key}: {replay}");
            assert_eq!(replay["task_id"], created.id);
            assert!(!spool.join(format!("{key}.json")).exists());
            assert!(!failed_dir().join(format!("{key}.json")).exists());
        }
        let replay = state.delete_task(created.id.clone()).await.unwrap();
        assert_eq!(replay.id, created.id);
        assert_eq!(replay.status, "deleted");
    }

    #[tokio::test]
    async fn stale_once_record_over_success_marker_suppresses_without_poisoning() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::create_dir_all(done_dir()).unwrap();
        // A record whose schedule was valid when applied but is now past.
        let record =
            spool_record_json(&[("rrule", serde_json::json!("FREQ=ONCE;AT=2020-01-01T00:00"))]);
        let digest =
            spool_request_digest(&serde_json::from_str::<SpooledCreationRequest>(&record).unwrap());
        std::fs::write(
            done_dir().join("stale.json"),
            serde_json::json!({
                "ok": true, "kind": "create", "task_id": "t-1",
                "task_name": "早报任务", "request_digest": digest,
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(spool.join("stale.json"), &record).unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            state
                .automations
                .lock()
                .await
                .list_automations()
                .unwrap()
                .is_empty(),
            "no second task: the recorded success suppresses the replay"
        );
        assert!(
            !failed_dir().join("stale.json").exists(),
            "the stale record must NOT be quarantined (the probe precedes validate)"
        );
        let marker: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(done_dir().join("stale.json")).unwrap())
                .unwrap();
        assert_eq!(
            marker["ok"],
            serde_json::json!(true),
            "the success receipt survives — the poison arm never ran"
        );
    }

    /// Round-8 M3-4: the per-poll feature-switch drain gate — switch off
    /// hides the tools AND pauses application from already-spooled records
    /// (the messaging sibling's parity shape).
    #[tokio::test]
    async fn switch_off_pauses_drain_and_leaves_spool_untouched() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("s.json"), spool_record_json(&[])).unwrap();
        crate::features::marketplace::builtin::set_feature_enabled(
            "scheduled-task-automation",
            false,
        )
        .expect("disable scheduled-task-automation");
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            state
                .automations
                .lock()
                .await
                .list_automations()
                .unwrap()
                .is_empty(),
            "switch off must apply nothing"
        );
        assert!(
            spool.join("s.json").exists(),
            "the pending record stays for re-enable"
        );
        crate::features::marketplace::builtin::set_feature_enabled(
            "scheduled-task-automation",
            true,
        )
        .expect("re-enable scheduled-task-automation");
    }

    /// Round-12 MAJOR-4: the apply-time replacement — the creator writes a
    /// DIVERGENT keyed retry over the spool file (the server's os.replace
    /// landing during the awaited apply) before delegating to the real
    /// domain. The Done arm must NOT unlink the untried newest body:
    /// deleting the still_applied digest check ships this test red.
    struct DivergentApplyCreator(pub ScheduledTaskState);

    impl TaskCreator for DivergentApplyCreator {
        async fn create(
            &self,
            input: CreateScheduledTaskInput,
        ) -> std::result::Result<ScheduledTaskDto, String> {
            std::fs::write(
                spool_root().join("div.json"),
                spool_record_json(&[
                    ("idempotency_key", serde_json::json!("k-div")),
                    ("name", serde_json::json!("更正后的名字")),
                ]),
            )
            .unwrap();
            self.0.create_task(input).await
        }
        async fn update(
            &self,
            task_id: &str,
            input: UpdateScheduledTaskInput,
        ) -> std::result::Result<ScheduledTaskDto, String> {
            self.0.update_task(task_id.to_string(), input).await
        }
        async fn delete(&self, task_id: &str) -> std::result::Result<ScheduledTaskDto, String> {
            self.0.delete_task(task_id.to_string()).await
        }
    }

    #[tokio::test]
    async fn apply_time_replacement_is_not_unlinked() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            spool.join("div.json"),
            spool_record_json(&[("idempotency_key", serde_json::json!("k-div"))]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &DivergentApplyCreator(state.clone()),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert_eq!(
            state
                .automations
                .lock()
                .await
                .list_automations()
                .unwrap()
                .len(),
            1,
            "the old body applied exactly once"
        );
        assert!(
            spool.join("div.json").exists(),
            "the untried divergent newest body survives for the next poll"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("div.json")).expect("receipt"),
        )
        .unwrap();
        assert_eq!(marker["ok"], true, "the applied record's receipt stands");
    }

    /// Round-12 MAJOR-2: a symlink to /dev/zero at the spool path is
    /// REFUSED by the gated read (symlink_metadata never follows it, so
    /// is_file() is false) — the ungated read would follow the link and
    /// read unbounded, growing a Vec until OOM. If the gate regresses,
    /// this poll balloons or blocks and the test times out.
    #[cfg(unix)]
    #[tokio::test]
    async fn devzero_symlink_at_the_spool_path_is_refused() {
        use std::time::Duration;
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::os::unix::fs::symlink("/dev/zero", spool.join("p.json")).unwrap();
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            process_spool_file(
                spool.join("p.json").as_path(),
                &StateCreator(&state),
                &state.sessions,
                &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            ),
        )
        .await
        .expect("the gated read must refuse the FIFO without blocking");
        assert!(
            matches!(outcome, Processed::Poison(_)),
            "a FIFO record poisons instead of wedging the drain"
        );
    }

    /// Round-9 M5: the receipt guard — apply a record (receipt lands),
    /// corrupt the spool bytes to invalid JSON, re-drain: the poison arm
    /// must NOT overwrite the ok:true receipt (the last-resort idempotency
    /// defense for post-apply corruption; the round-8 guard was
    /// mutation-proven unpinned).
    /// Round-9 M5 (#628): the receipt guard — apply a record (receipt
    /// lands), corrupt the spool bytes to invalid JSON, re-drain: the
    /// poison arm must NOT overwrite the ok:true receipt.
    #[tokio::test]
    async fn poisoned_record_never_overwrites_a_landed_ok_receipt() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("receipt.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        let marker = done_dir().join("receipt.json");
        let landed = std::fs::read_to_string(&marker).expect("the receipt landed");
        assert!(
            serde_json::from_str::<serde_json::Value>(&landed).unwrap()["ok"]
                == serde_json::json!(true)
        );
        // Corrupt the spool bytes AFTER the successful apply — the exact
        // class probe-before-validate cannot cover.
        std::fs::write(spool.join("receipt.json"), b"not json{").unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        let after = std::fs::read_to_string(&marker).expect("the receipt still exists");
        assert!(
            serde_json::from_str::<serde_json::Value>(&after).unwrap()["ok"]
                == serde_json::json!(true),
            "the poison arm must not overwrite the ok:true receipt"
        );
        assert!(
            failed_dir().join("receipt.json").exists(),
            "the corrupted record is quarantined"
        );
    }

    #[test]
    fn result_marker_suppression_follows_the_ok_field() {
        let _home = TempHome::new();
        let dir = done_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("marker.json");
        let good = serde_json::json!({
            "ok": true,
            "task_id": "t",
            "request_digest": "d1",
        })
        .to_string();
        std::fs::write(&marker, &good).unwrap();
        assert!(result_marker_suppresses(&marker, "d1"));
        // A FORGED ok:true — wrong digest, or no digest binding at all —
        // must NOT suppress: the marker channel is untrusted input.
        assert!(!result_marker_suppresses(&marker, "d2"));
        std::fs::write(
            &marker,
            serde_json::json!({"ok": true, "task_id": "t"}).to_string(),
        )
        .unwrap();
        assert!(!result_marker_suppresses(&marker, "d1"));
        // Failure does NOT suppress: the retry re-applies (one terminal
        // failure must not poison the key forever).
        std::fs::write(
            &marker,
            serde_json::json!({"ok": false, "error": "e"}).to_string(),
        )
        .unwrap();
        assert!(!result_marker_suppresses(&marker, "d1"));
        // Round-7 MAJOR 1b: unparseable markers NO LONGER suppress — a
        // corrupt marker must not silently swallow a queued request; the
        // watcher logs and lets the retry re-apply (the server's poll
        // times out to pending and the model sees the truth eventually).
        std::fs::write(&marker, b"not json{").unwrap();
        assert!(!result_marker_suppresses(&marker, "d1"));
        // Round-21 minor 1: the suppression-matrix cells for non-literal
        // ok values — a marker with a MATCHING digest but ok absent,
        // ok:"true" (string), or ok:1 must NOT suppress (the receipt
        // probe binds `== Some(true)`; the old truthiness shape split the
        // outcome between watcher and server).
        for bad_ok in [
            serde_json::json!({"task_id": "t", "request_digest": "d1"}),
            serde_json::json!({"ok": "true", "task_id": "t", "request_digest": "d1"}),
            serde_json::json!({"ok": 1, "task_id": "t", "request_digest": "d1"}),
        ] {
            std::fs::write(&marker, bad_ok.to_string()).unwrap();
            assert!(
                !result_marker_suppresses(&marker, "d1"),
                "non-literal ok must not suppress: {bad_ok}"
            );
        }
        // Round-8 M3-2: the OVERSIZE arm — valid JSON over the 64 KiB cap
        // with the right digest still refuses (only the cap can refuse
        // this payload; the round-7 test never covered oversize).
        let oversize = format!(
            "{{\"ok\": true, \"task_id\": \"t\", \"request_digest\": \"d1\", \"pad\": \"{}\"}}",
            "x".repeat(66 * 1024),
        );
        std::fs::write(&marker, oversize).unwrap();
        assert!(
            !result_marker_suppresses(&marker, "d1"),
            "an oversize marker must be refused before read/parsing"
        );
        assert!(!result_marker_suppresses(&dir.join("missing.json"), "d1"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn failure_marker_lets_a_retry_reapply() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        // A stale failure marker from a previous attempt must not swallow the
        // retry: the operation re-applies and overwrites the marker.
        std::fs::create_dir_all(done_dir()).unwrap();
        std::fs::write(
            done_dir().join("retry.json"),
            serde_json::json!({"ok": false, "error": "store was read-only"}).to_string(),
        )
        .unwrap();
        std::fs::write(spool.join("retry.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        let records = state.automations.lock().await.list_automations().unwrap();
        assert_eq!(
            records.len(),
            1,
            "the retry re-applied despite the failure marker"
        );
        let marker: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(done_dir().join("retry.json")).unwrap())
                .unwrap();
        assert_eq!(
            marker["ok"], true,
            "the success overwrote the failure marker"
        );
    }

    #[tokio::test]
    async fn poison_writes_failure_marker_and_audits() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::create_dir_all(
            crate::platform::paths::sessions_root()
                .join("reqsrc01")
                .join("workspace"),
        )
        .unwrap();
        std::fs::write(
            spool.join("bad.json"),
            spool_record_json(&[(
                "prompt",
                serde_json::json!("x".repeat(MAX_PROMPT_CHARS + 1)),
            )]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        // The waiting MCP call receives a terminal failure instead of
        // "pending" (the poison arm writes the marker too).
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("bad.json")).expect("failure marker"),
        )
        .unwrap();
        assert_eq!(marker["ok"], false);
        assert!(
            marker["error"]
                .as_str()
                .unwrap()
                .contains("character limit"),
            "the validation reason reaches the caller"
        );
        // The failure is audited into the requesting session's execution root.
        let audit_path = crate::platform::paths::sessions_root()
            .join("reqsrc01")
            .join("workspace")
            .join("workflow_audit.jsonl");
        let audit = std::fs::read_to_string(audit_path).expect("audit record");
        let line: serde_json::Value = serde_json::from_str(audit.lines().next().unwrap()).unwrap();
        assert_eq!(line["kind"], "scheduled_task_failed");
        assert_eq!(line["detail"]["outcome"], "failed");
        assert!(
            !line["detail"]["error"].as_str().unwrap().contains("/home/"),
            "failure markers never leak absolute host paths"
        );
        // Round-13 MAJOR-2: the failure line is STAMPED as unverified
        // provenance (planted evidence must be distinguishable).
        assert_eq!(
            line["detail"]["claimed_from_session_verified"], false,
            "the failure line carries the unverified stamp"
        );
    }

    /// Round-13 MAJOR-2: a planted record claiming an ISOLATED sender
    /// poisons, but its failure disposition writes NO per-session line
    /// (the sender gate mirrors the success path; the automation-store
    /// shadow trail alone covers such records).
    #[tokio::test]
    async fn isolated_sender_failure_writes_no_session_line() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::create_dir_all(
            crate::platform::paths::sessions_root()
                .join("aux-side9")
                .join("workspace"),
        )
        .unwrap();
        std::fs::write(
            spool.join("bad.json"),
            spool_record_json(&[
                (
                    "prompt",
                    serde_json::json!("x".repeat(MAX_PROMPT_CHARS + 1)),
                ),
                ("from_session", serde_json::json!("aux-side9")),
            ]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            failed_dir().join("bad.json").exists(),
            "the isolated-sender record still poisons"
        );
        let content = std::fs::read_to_string(
            crate::platform::paths::sessions_root()
                .join("aux-side9")
                .join("workspace")
                .join("workflow_audit.jsonl"),
        )
        .unwrap_or_default();
        assert!(
            !content.contains("scheduled_task_failed"),
            "no failure line may land in an isolated sender's session: {content}"
        );
        // The caller-visible trace for a validate-poison record is the
        // failure marker (the poison arm carries no shadow line — the
        // contract's honest carve-out); what matters here is that the
        // per-session audit channel stays closed to planted isolated
        // senders.
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("bad.json")).expect("failure marker"),
        )
        .unwrap();
        assert_eq!(marker["ok"], false);
    }

    /// Round-26 M2: the round-24 M4 bound→ledger audit routing shipped
    /// unpinned — mutating `audit_root_for` to always return the
    /// execution root kept the whole module green, so the flagship
    /// "planted evidence must not land in the user's project tree" rule
    /// had no test. The messaging sibling owns the rule's template
    /// (`bound_session_audits_route_to_the_ledger_root`); this is the
    /// scheduled-side port, driven through the real `audit_request`
    /// append path with the production binding seam (the injected
    /// execution-root resolver).
    #[test]
    fn bound_session_audits_route_to_the_ledger_root() {
        let _home = TempHome::new();
        let store = crate::features::sessions::SessionStore::boot_for_process_startup()
            .expect("sessions store");
        let project = std::env::temp_dir().join(format!(
            "pinvou3-scheduled-bound-project-{}",
            crate::platform::paths::tests::unique_suffix()
        ));
        // Pre-clean (TempHome's pattern): one red run must not leave a
        // stale dir that fails every subsequent suite run.
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(&project).unwrap();
        let bound_id = "reqsrc01".to_string();
        let bound_project = project.clone();
        store.set_execution_root_resolver(std::sync::Arc::new(move |id: &str| {
            (id == bound_id).then(|| bound_project.clone())
        }));
        // The private ledger dir exists for a created session; create it
        // for this synthetic one.
        let ledger = crate::platform::paths::session_workspace_dir("reqsrc01");
        std::fs::create_dir_all(&ledger).unwrap();
        let request: SpooledCreationRequest =
            serde_json::from_str(&spool_record_json(&[])).unwrap();
        audit_request(&store, &request, "t-bound", "早报任务");
        assert!(
            ledger.join("workflow_audit.jsonl").exists(),
            "the audit line lands in the private ledger root"
        );
        assert!(
            !project.join("workflow_audit.jsonl").exists(),
            "the user's bound project directory stays clean"
        );
        let _ = std::fs::remove_dir_all(&project);
    }

    /// Round-26 M2: the anchored Done-arm unlink guard's decision
    /// function, pinned at its seam. The guard itself (the
    /// `same_file_identity` compare at the consume site) fires only in
    /// the microscopic compare-then-unlink TOCTOU window documented at
    /// the site — not sequentially drivable in a test — so the pin
    /// covers what the window's resolution means: a same-name replace
    /// (identical bytes, fresh mtime) must NOT count as the applied
    /// file, and the untouched file must.
    #[test]
    fn applied_file_identity_refuses_a_replaced_namesake() {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-sched-identity-{}",
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abc.json");
        std::fs::write(&path, b"payload").unwrap();
        let applied = std::fs::symlink_metadata(&path).unwrap();
        // The applied file untouched: the guard consumes it.
        assert!(same_file_identity(&applied, &applied));
        // A same-name replace with IDENTICAL bytes (the replay shape the
        // window targets): fresh mtime → not the applied file.
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&path, b"payload").unwrap();
        let replaced = std::fs::symlink_metadata(&path).unwrap();
        assert!(
            !same_file_identity(&replaced, &applied),
            "a replaced namesake must not pass the applied-identity guard"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn isolated_senders_keyless_ids_and_stray_delete_fields_are_rejected() {
        for sender in ["aux-side1", "AUX-side1", "eval_b1", "EVAL_b1"] {
            let record: SpooledCreationRequest = serde_json::from_str(&spool_record_json(&[(
                "from_session",
                serde_json::json!(sender),
            )]))
            .unwrap();
            assert!(record.validate().is_err(), "{sender} must be rejected");
        }
        // A key without a sender would degrade the namespace to global.
        let record: SpooledCreationRequest = serde_json::from_str(&spool_record_json(&[
            ("idempotency_key", serde_json::json!("anon-key")),
            ("from_session", serde_json::Value::Null),
        ]))
        .unwrap();
        assert!(record.validate().is_err(), "key requires from_session");
        // delete takes no extra fields, paused included.
        let record: SpooledCreationRequest = serde_json::from_str(&spool_record_json(&[
            ("kind", serde_json::json!("delete")),
            (
                "task_id",
                serde_json::json!("0f0e0d0c-0000-0000-0000-000000000000"),
            ),
            ("paused", serde_json::json!(true)),
            ("name", serde_json::Value::Null),
            ("prompt", serde_json::Value::Null),
            ("rrule", serde_json::Value::Null),
        ]))
        .unwrap();
        assert!(record.validate().is_err(), "delete rejects a stray paused");
    }

    /// Round-36: the starvation check must NOT fire on a healthy spool
    /// root (only the app's own `.done`/`failed` directories present) —
    /// the round-36 reviewer probe showed the unfiltered check warning
    /// forever on every healthy deployment. A foreign non-record entry
    /// must fire it.
    #[test]
    fn spool_starvation_excludes_app_owned_directories() {
        let owned = [
            std::ffi::OsString::from(".done"),
            std::ffi::OsString::from("failed"),
        ];
        let healthy = [
            std::ffi::OsString::from(".done"),
            std::ffi::OsString::from("failed"),
        ];
        assert!(
            !spool_starved_by_non_records(&healthy, &owned, true),
            "a healthy spool root must not warn"
        );
        let starved = [
            std::ffi::OsString::from(".done"),
            std::ffi::OsString::from("failed"),
            std::ffi::OsString::from("junk.txt"),
        ];
        assert!(
            spool_starved_by_non_records(&starved, &owned, true),
            "a foreign non-record entry must trip the starvation warn"
        );
        assert!(
            !spool_starved_by_non_records(&healthy, &owned, false),
            "non-spool directories are never starvation-checked"
        );
    }

    #[test]
    fn prune_stale_state_sweeps_old_markers_and_stray_tmp() {
        let _home = TempHome::new();
        let done = done_dir();
        let failed = failed_dir();
        std::fs::create_dir_all(&done).unwrap();
        std::fs::create_dir_all(&failed).unwrap();
        std::fs::create_dir_all(spool_root()).unwrap();
        let write = |path: &Path| {
            std::fs::write(path, b"{}").unwrap();
            let file = std::fs::File::options().write(true).open(path).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(
                std::time::SystemTime::now() - STATE_RETENTION - std::time::Duration::from_secs(60),
            ))
            .unwrap();
        };
        write(&done.join("old.json"));
        write(&failed.join("old.json"));
        write(&spool_root().join("stray.tmp"));
        std::fs::write(done.join("fresh.json"), b"{}").unwrap();
        std::fs::write(spool_root().join("pending.json"), b"{}").unwrap();
        prune_stale_state_at(std::time::SystemTime::now());
        assert!(!done.join("old.json").exists(), "stale marker pruned");
        assert!(!failed.join("old.json").exists(), "stale evidence pruned");
        assert!(!spool_root().join("stray.tmp").exists(), "stray tmp swept");
        assert!(done.join("fresh.json").exists(), "fresh marker kept");
        assert!(
            spool_root().join("pending.json").exists(),
            "a pending record is never pruned"
        );
    }

    /// Review R4-M3: the shadow audit under the automation store root fires
    /// for EVERY applied request — a record that omits from_session (the
    /// exact case where the session-workspace audit silently vanishes)
    /// still leaves a trace the requester cannot skip.
    #[tokio::test]
    async fn shadow_audit_covers_requests_without_from_session() {
        use crate::features::scheduled::tasks::scheduled_automation_root;
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        // No from_session: audit_request would early-return; the shadow
        // audit must still land under <home>/automations/audit.
        std::fs::write(
            spool.join("nosender.json"),
            spool_record_json(&[
                ("kind", serde_json::json!("create")),
                ("name", serde_json::json!("无发送者任务")),
                ("prompt", serde_json::json!("测试提示词")),
                // The default record's known-good weekly form (the product's
                // rrule subset rejects several RFC-valid forms; this test
                // pins the apply path, not rrule validation).
                (
                    "rrule",
                    serde_json::json!("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=0"),
                ),
                ("from_session", serde_json::Value::Null),
                ("from_title", serde_json::Value::Null),
            ]),
        )
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(calls.clone()),
            &mut retries,
        )
        .await;
        let audit_dir = scheduled_automation_root().join("audit");
        let audit_path = audit_dir.join("workflow_audit.jsonl");
        assert!(audit_path.exists(), "shadow audit file must exist");
        let content = std::fs::read_to_string(&audit_path).unwrap();
        assert!(
            content.contains("\"scheduled_task_request\""),
            "the shadow audit line carries the request kind: {content}"
        );
        assert!(
            content.contains("\"claimed_from_session\":null"),
            "the omitted sender is recorded as explicitly unverified"
        );
        // Round-19 MAJOR-2: the unverified stamp rides the SHADOW line as
        // well (the body's audit paragraph describes it there).
        assert!(
            content.contains("\"claimed_from_session_verified\":false"),
            "the shadow line carries the unverified stamp"
        );
        assert!(
            !spool.join("nosender.json").exists(),
            "the request itself was applied and consumed"
        );
    }

    // Round-18 MAJOR-1: a planted FIFO at the marker tmp path must be
    // REFUSED, not written. O_RDWR|O_NONBLOCK alone succeeds on a FIFO
    // (it supplies its own reader) — the round-17 shape — so the payload
    // vanished into the pipe buffer and the publish rename installed the
    // FIFO as the marker: silent receipt loss plus a duplicate-apply
    // window. The gated tmp open fails the open instead; the rename never
    // runs and the FIFO is left untouched for a deterministic re-refusal.
    #[cfg(unix)]
    #[tokio::test]
    async fn planted_fifo_at_the_marker_tmp_path_is_refused() {
        let _home = TempHome::new();
        std::fs::create_dir_all(done_dir()).unwrap();
        let marker = done_dir().join("victim.json");
        let tmp = marker.with_extension("json.tmp");
        let fifo_path = std::ffi::CString::new(tmp.as_os_str().to_str().unwrap()).unwrap();
        assert_eq!(
            unsafe { libc::mkfifo(fifo_path.as_ptr(), libc::S_IFIFO | 0o644) },
            0,
            "fixture: the FIFO must be created"
        );
        let result = write_done_marker(&marker, &serde_json::json!({"ok": true, "task_id": "t-1"}));
        result.expect("an unrelated planted FIFO cannot occupy the exclusive temp name");
        assert!(
            std::fs::symlink_metadata(&marker).unwrap().is_file(),
            "publish only the new regular file"
        );
        // Still a FIFO (not truncated into a regular file, not consumed):
        // the re-poll refuses again deterministically.
        let meta = std::fs::metadata(&tmp).unwrap();
        use std::os::unix::fs::FileTypeExt as _;
        assert!(meta.file_type().is_fifo(), "the FIFO must be untouched");
    }

    // Round-18 MAJOR-1 (truncation half): the gated tmp open truncates —
    // a leftover LONGER tmp (crash between write and rename, publish
    // failure, prune-window stray) plus a shorter payload must not
    // publish `payload + stale tail` (an unparseable torn marker that
    // degrades every replay to pending).
    #[tokio::test]
    async fn marker_tmp_write_truncates_a_leftover_longer_tmp() {
        let _home = TempHome::new();
        std::fs::create_dir_all(done_dir()).unwrap();
        let marker = done_dir().join("victim.json");
        let tmp = marker.with_extension("json.tmp");
        std::fs::write(&tmp, "x".repeat(4096)).unwrap();
        let payload = serde_json::json!({"ok": true, "task_id": "t-1"});
        write_done_marker(&marker, &payload).expect("the publish must succeed");
        let published = std::fs::read_to_string(&marker).unwrap();
        assert_eq!(
            published,
            payload.to_string(),
            "the marker must be exactly the payload (no stale tail)"
        );
    }

    // Round-21 M2 (reader-present variant): a FIFO at the tmp path WITH a
    // reader held open — the shape that motivated round-18 MAJOR-1's
    // silent-loss half (a write-only open succeeds when a reader exists).
    // The gate refuses at fstat; the payload must not vanish into the
    // pipe buffer and the rename must not publish the FIFO.
    #[cfg(unix)]
    #[tokio::test]
    async fn fifo_with_reader_at_the_marker_tmp_path_is_refused() {
        let _home = TempHome::new();
        std::fs::create_dir_all(done_dir()).unwrap();
        let marker = done_dir().join("victim.json");
        let tmp = marker.with_extension("json.tmp");
        let fifo_path = std::ffi::CString::new(tmp.as_os_str().to_str().unwrap()).unwrap();
        assert_eq!(
            unsafe { libc::mkfifo(fifo_path.as_ptr(), libc::S_IFIFO | 0o644) },
            0,
            "fixture: the FIFO must be created"
        );
        // A reader holding the FIFO: a gated write-only open would
        // otherwise succeed into the pipe buffer (round-18's silent-loss
        // shape). The reader drops after 300 ms; the gate fails the open
        // immediately either way.
        use std::os::unix::fs::OpenOptionsExt as _;
        let _reader = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&tmp)
            .unwrap();
        let result = write_done_marker(&marker, &serde_json::json!({"ok": true}));
        result.expect("an unrelated planted FIFO cannot occupy the exclusive temp name");
        assert!(
            std::fs::symlink_metadata(&marker).unwrap().is_file(),
            "publish only the new regular file"
        );
    }

    #[tokio::test]
    async fn vanished_records_release_retry_and_park_state() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        std::fs::create_dir_all(spool_root()).unwrap();
        let mut retries = RetryState::default();
        retries.attempts.insert("gone.json".into(), 3);
        retries
            .parked
            .insert("gone.json".into(), std::time::SystemTime::now());
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(retries.attempts.is_empty());
        assert!(retries.parked.is_empty());
        std::fs::remove_dir_all(spool_root()).unwrap();
        retries.attempts.insert("gone.json".into(), 3);
        retries
            .parked
            .insert("gone.json".into(), std::time::SystemTime::now());
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(retries.attempts.is_empty());
        assert!(retries.parked.is_empty());
    }

    #[test]
    fn retention_preserves_receipts_for_stuck_spool_records() {
        let _home = TempHome::new();
        std::fs::create_dir_all(done_dir()).unwrap();
        std::fs::create_dir_all(spool_root()).unwrap();
        let marker = done_dir().join("stuck.json");
        std::fs::write(&marker, b"{\"ok\":true}").unwrap();
        std::fs::write(spool_root().join("stuck.json"), b"{}").unwrap();
        let later =
            std::time::SystemTime::now() + STATE_RETENTION + std::time::Duration::from_secs(60);
        prune_stale_state_at(later);
        assert!(marker.exists(), "a stuck record must remain deduplicated");
        std::fs::remove_file(spool_root().join("stuck.json")).unwrap();
        prune_stale_state_at(later);
        assert!(
            !marker.exists(),
            "retention resumes when the spool name disappears"
        );
    }

    #[test]
    fn quarantine_keeps_colliding_evidence() {
        let _home = TempHome::new();
        std::fs::create_dir_all(spool_root()).unwrap();
        std::fs::create_dir_all(failed_dir()).unwrap();
        let source = spool_root().join("collision.json");
        let previous = failed_dir().join("collision.json");
        std::fs::write(&source, b"new").unwrap();
        std::fs::write(&previous, b"old").unwrap();
        assert!(quarantine(&source).is_ok());
        assert_eq!(std::fs::read(previous).unwrap(), b"old");
        assert_eq!(
            std::fs::read(failed_dir().join("collision.1.json")).unwrap(),
            b"new"
        );
        assert!(!source.exists());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn non_utf8_spool_names_are_quarantined() {
        use std::os::unix::ffi::OsStringExt as _;
        let _home = TempHome::new();
        let state = watcher_state().await;
        std::fs::create_dir_all(spool_root()).unwrap();
        let name = std::ffi::OsString::from_vec(b"bad-\xff.json".to_vec());
        std::fs::write(spool_root().join(&name), b"invalid").unwrap();
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut RetryState::default(),
        )
        .await;
        assert!(!spool_root().join(&name).exists());
        assert_eq!(std::fs::read(failed_dir().join(name)).unwrap(), b"invalid");
    }

    #[tokio::test]
    async fn a_drain_pass_coalesces_refresh_and_pauses_on_bad_settings() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        std::fs::create_dir_all(spool_root()).unwrap();
        for name in ["one.json", "two.json"] {
            std::fs::write(spool_root().join(name), spool_record_json(&[])).unwrap();
        }
        let count = Arc::new(AtomicUsize::new(0));
        let notifier = CountingNotifier(count.clone());
        let mut retries = RetryState::default();
        std::fs::write(crate::platform::paths::settings_path(), b"{").unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &notifier,
            &mut retries,
        )
        .await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(retries.feature_gate_error_logged);
        assert!(spool_root().join("one.json").exists());
        std::fs::write(crate::platform::paths::settings_path(), b"{}").unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &notifier,
            &mut retries,
        )
        .await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "two applies need one panel refresh"
        );
        assert!(!retries.feature_gate_error_logged);
    }

    #[tokio::test]
    async fn future_mtime_cannot_repeatedly_unpark_a_request() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        std::fs::create_dir_all(spool_root()).unwrap();
        let path = spool_root().join("future.json");
        std::fs::write(&path, spool_record_json(&[])).unwrap();
        let now = std::time::SystemTime::now();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(now + Duration::from_secs(3600))
            .unwrap();
        let mut retries = RetryState::default();
        retries
            .attempts
            .insert("future.json".into(), MAX_CREATE_ATTEMPTS);
        retries
            .parked
            .insert("future.json".into(), now - Duration::from_secs(3));
        let notifier = CountingNotifier(Arc::new(AtomicUsize::new(0)));
        for _ in 0..2 {
            process_pending_spool(&FailingCreator, &state.sessions, &notifier, &mut retries).await;
            assert!(
                path.exists(),
                "future timestamps must not buy another apply attempt"
            );
            assert_eq!(
                retries.attempts.get("future.json"),
                Some(&MAX_CREATE_ATTEMPTS)
            );
        }
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(now - Duration::from_secs(1))
            .unwrap();
        process_pending_spool(&FailingCreator, &state.sessions, &notifier, &mut retries).await;
        assert!(
            !path.exists(),
            "a real replacement in the elapsed window un-parks and exhausts"
        );
        assert!(failed_dir().join("future.json").exists());
    }

    #[tokio::test]
    async fn keyed_replacement_keeps_the_remaining_retry_budget() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        std::fs::create_dir_all(spool_root()).unwrap();
        let path = spool_root().join("replacement.json");
        std::fs::write(&path, spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        let notifier = CountingNotifier(Arc::new(AtomicUsize::new(0)));
        for _ in 0..2 {
            process_pending_spool(&FailingCreator, &state.sessions, &notifier, &mut retries).await;
        }
        assert_eq!(retries.attempts.get("replacement.json"), Some(&2));
        std::fs::write(
            &path,
            spool_record_json(&[("prompt", serde_json::json!("replacement body"))]),
        )
        .unwrap();
        process_pending_spool(&FailingCreator, &state.sessions, &notifier, &mut retries).await;
        assert!(
            !path.exists(),
            "a changed payload inherits only the one remaining attempt"
        );
        let evidence = std::fs::read_to_string(failed_dir().join("replacement.json")).unwrap();
        assert!(evidence.contains("replacement body"));
    }

    #[test]
    fn shadow_audit_rotates_on_disk_and_keeps_one_previous_segment() {
        let _home = TempHome::new();
        let dir = crate::features::scheduled::tasks::scheduled_automation_root().join("audit");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("workflow_audit.jsonl");
        let previous = dir.join("workflow_audit.jsonl.old");
        let request: SpooledCreationRequest =
            serde_json::from_str(&spool_record_json(&[])).unwrap();
        assert_eq!(SHADOW_AUDIT_ROTATE_BYTES, 2 * 1024 * 1024);
        for (stem, size) in [
            ("first", 2 * 1024 * 1024 + 1),
            ("second", 2 * 1024 * 1024 + 2),
        ] {
            std::fs::File::create(&path).unwrap().set_len(size).unwrap();
            // No in-memory rotation state: a new process observes the same
            // on-disk threshold and replaces the single previous segment.
            audit_request_shadow(&request, stem, "task", "name", "applied");
            assert_eq!(std::fs::metadata(&previous).unwrap().len(), size);
            let current = std::fs::read_to_string(&path).unwrap();
            assert_eq!(current.lines().count(), 1);
            let row: serde_json::Value = serde_json::from_str(&current).unwrap();
            assert_eq!(row["detail"]["spool_stem"], stem);
            assert!(current.len() < 4096);
        }
        std::fs::remove_file(&previous).unwrap();
        std::fs::create_dir(&previous).unwrap();
        let size = 2 * 1024 * 1024 + 3;
        std::fs::File::create(&path).unwrap().set_len(size).unwrap();
        audit_request_shadow(&request, "blocked", "task", "name", "applied");
        assert!(
            std::fs::metadata(&path).unwrap().len() > size,
            "failed rotation preserves old evidence and still appends the new outcome"
        );
    }
    #[tokio::test]
    async fn real_stdio_server_drains_through_real_rust_domain() {
        use std::io::{BufRead as _, Write as _};
        let _home = TempHome::new();
        let state = watcher_state().await;
        let server = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../resources/mcp-servers/app-automations/server.py");
        let python = if cfg!(windows) { "python" } else { "python3" };
        let mut child = std::process::Command::new(python)
            .arg(server)
            .arg("--task-requests-dir")
            .arg(spool_root().parent().unwrap())
            .arg("--automations-dir")
            .arg(super::super::scheduled_automation_root())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("Python runtime required for the shipped MCP package");
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let request = serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{
            "name":"create_scheduled_task", "arguments":{"name":"real seam", "prompt":"fixture prompt", "rrule":"FREQ=HOURLY", "paused":null, "from_session":"reqsrc01", "idempotency_key":"real-seam"}
        }});
        writeln!(stdin, "{request}").unwrap();
        let mut retries = RetryState::default();
        for _ in 0..100 {
            process_pending_spool(
                &StateCreator(&state),
                &state.sessions,
                &CountingNotifier(Arc::new(AtomicUsize::new(0))),
                &mut retries,
            )
            .await;
            if !state
                .automations
                .lock()
                .await
                .list_automations()
                .unwrap()
                .is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        let payload: serde_json::Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(payload["ok"], true, "{reply}");
        let records = state.automations.lock().await.list_automations().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(payload["taskId"], records[0].id);
        assert_eq!(records[0].prompt, "fixture prompt");
        assert!(
            matches!(
                records[0].status,
                deepseek_tui::automation_manager::AutomationStatus::Active
            ),
            "paused:null defaults to active through the real seam"
        );
        drop(stdin);
        assert!(child.wait().unwrap().success());
    }

    #[tokio::test]
    async fn null_paused_default_applies_active_and_normalized_digest_replays() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        std::fs::create_dir_all(spool_root()).unwrap();
        let path = spool_root().join("default.json");
        std::fs::write(
            &path,
            spool_record_json(&[("paused", serde_json::Value::Null)]),
        )
        .unwrap();
        let mut retries = RetryState::default();
        let notifier = CountingNotifier(Arc::new(AtomicUsize::new(0)));
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &notifier,
            &mut retries,
        )
        .await;
        let records = state.automations.lock().await.list_automations().unwrap();
        assert!(matches!(
            records[0].status,
            deepseek_tui::automation_manager::AutomationStatus::Active
        ));
        let mut request = valid_record();
        let digest = spool_request_digest(&request);
        request.name = request.name.map(|value| format!("  {value}  "));
        request.prompt = request.prompt.map(|value| format!(" {value} "));
        request.rrule = request
            .rrule
            .map(|value| format!(" {} ", value.to_lowercase()));
        assert_eq!(
            spool_request_digest(&request),
            digest,
            "digest binds applied normalized text"
        );
    }

    #[tokio::test]
    async fn large_spool_bounds_applies_and_tail_moves_per_pass() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        std::fs::create_dir_all(spool_root()).unwrap();
        for i in 0..1000 {
            std::fs::write(
                spool_root().join(format!("{i:04}.json")),
                spool_record_json(&[]),
            )
            .unwrap();
        }
        let mut retries = RetryState::default();
        process_pending_spool(
            &FailingCreator,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert_eq!(retries.attempts.len(), MAX_PENDING_FILES);
        let quarantines = std::fs::read_dir(failed_dir()).unwrap().count();
        assert_eq!(quarantines, MAX_TAIL_QUARANTINES);
        assert!(std::fs::read_dir(spool_root()).unwrap().count() > 900);
    }

    #[tokio::test]
    async fn retry_exhaustion_keeps_a_replacement_payload() {
        struct ReplacingFailure;
        impl TaskCreator for ReplacingFailure {
            async fn create(
                &self,
                _: CreateScheduledTaskInput,
            ) -> std::result::Result<ScheduledTaskDto, String> {
                std::fs::write(
                    spool_root().join("retry.json"),
                    spool_record_json(&[("name", serde_json::json!("corrected replacement"))]),
                )
                .unwrap();
                Err("old apply failed".to_string())
            }
            async fn update(
                &self,
                _: &str,
                _: UpdateScheduledTaskInput,
            ) -> std::result::Result<ScheduledTaskDto, String> {
                unreachable!()
            }
            async fn delete(&self, _: &str) -> std::result::Result<ScheduledTaskDto, String> {
                unreachable!()
            }
        }
        let _home = TempHome::new();
        let state = watcher_state().await;
        std::fs::create_dir_all(spool_root()).unwrap();
        let path = spool_root().join("retry.json");
        std::fs::write(&path, spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        retries
            .attempts
            .insert("retry.json".to_string(), MAX_CREATE_ATTEMPTS - 1);
        process_pending_spool(
            &ReplacingFailure,
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        let newest = std::fs::read_to_string(&path).unwrap();
        assert!(newest.contains("corrected replacement"));
        assert!(!failed_dir().join("retry.json").exists());
        assert!(
            !done_dir().join("retry.json").exists(),
            "old failure does not answer the replacement"
        );
    }

    #[test]
    fn quarantine_retention_starts_at_move_and_done_temps_expire() {
        let _home = TempHome::new();
        prepare_request_directories().unwrap();
        let path = spool_root().join("aged.json");
        std::fs::write(&path, b"evidence").unwrap();
        let now = std::time::SystemTime::now();
        let old = now - STATE_RETENTION - Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        quarantine(&path).unwrap();
        let held = failed_dir().join(".untimed-fixture");
        std::fs::write(&held, b"manual retention").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&held)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        let tmp = done_dir().join("stale.json.tmp");
        std::fs::write(&tmp, b"crash debris").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&tmp)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        prune_stale_state_at(now + Duration::from_secs(60));
        assert!(
            std::fs::read_dir(failed_dir())
                .unwrap()
                .flatten()
                .any(|entry| entry.path().is_file()
                    && std::fs::read(entry.path()).is_ok_and(|bytes| bytes == b"evidence")),
            "old pending mtime cannot discard fresh or explicitly held evidence"
        );
        assert!(!tmp.exists());
        assert!(
            held.exists(),
            "unstamped evidence requires manual cleanup rather than premature deletion"
        );
    }

    #[test]
    fn audit_file_obstruction_is_preserved_and_new_evidence_appends() {
        let _home = TempHome::new();
        std::fs::create_dir_all(super::super::scheduled_automation_root()).unwrap();
        let path = super::super::scheduled_automation_root().join("audit");
        std::fs::write(&path, b"preserve obstruction").unwrap();
        prepare_request_directories().unwrap();
        audit_request_shadow(&valid_record(), "recovered", "task", "name", "applied");
        assert!(
            std::fs::read_to_string(path.join("workflow_audit.jsonl"))
                .unwrap()
                .contains("recovered")
        );
        let backups: Vec<_> = std::fs::read_dir(super::super::scheduled_automation_root())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("audit-obstruction-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read(backups[0].path()).unwrap(),
            b"preserve obstruction"
        );
    }

    #[cfg(unix)]
    #[test]
    fn linked_spool_terminal_and_audit_directories_never_mutate_outside() {
        use std::os::unix::fs::symlink;
        let _home = TempHome::new();
        let outside = tempfile::tempdir().unwrap();
        let evidence = outside.path().join("old.json");
        std::fs::write(&evidence, b"outside").unwrap();
        let old = std::time::SystemTime::now() - STATE_RETENTION - Duration::from_secs(120);
        std::fs::File::options()
            .write(true)
            .open(&evidence)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        for target in [
            spool_root(),
            done_dir(),
            failed_dir(),
            super::super::scheduled_automation_root().join("audit"),
        ] {
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            if target.is_dir() {
                std::fs::remove_dir(&target).unwrap();
            }
            symlink(outside.path(), &target).unwrap();
            assert!(private_directory(&target).is_err());
            prune_stale_state();
            assert_eq!(std::fs::read(&evidence).unwrap(), b"outside");
            if target == done_dir() {
                assert!(
                    write_done_marker(
                        &target.join("injected.json"),
                        &serde_json::json!({"ok":true})
                    )
                    .is_err()
                );
            }
            if target.file_name().unwrap() == "audit" {
                audit_request_shadow(&valid_record(), "bad-link", "task", "name", "applied");
            }
            assert!(!outside.path().join("injected.json").exists());
            assert!(!outside.path().join("workflow_audit.jsonl").exists());
            std::fs::remove_file(&target).unwrap();
        }
    }
    #[test]
    fn poisoned_snapshot_does_not_match_a_corrected_record() {
        let _home = TempHome::new();
        std::fs::create_dir_all(spool_root()).unwrap();
        let path = spool_root().join("poison.json");
        std::fs::write(&path, b"invalid json").unwrap();
        let snapshot = RequestSnapshot::capture(&path).unwrap();
        assert!(snapshot.still_matches(&path));
        std::fs::write(&path, spool_record_json(&[])).unwrap();
        assert!(
            !snapshot.still_matches(&path),
            "terminal poison guard must preserve the corrected record"
        );
    }

    /// Round-10 R-1: the tail arm consults the same divergence predicate
    /// before its move — a snapshot of the probed bytes must NOT match a
    /// record replaced afterwards, so the replacement is left for its own
    /// fresh pass instead of being moved and answered by the old bytes'
    /// marker. (The interleaving window itself is not injectable from a
    /// black-box drain test; this pins the predicate the guard and the
    /// pinned Poison/Retry guards share.)
    #[test]
    fn tail_snapshot_divergence_leaves_the_replacement_for_the_next_pass() {
        let _home = TempHome::new();
        std::fs::create_dir_all(spool_root()).unwrap();
        let path = spool_root().join("tail.json");
        std::fs::write(&path, spool_record_json(&[])).unwrap();
        let snapshot = RequestSnapshot::capture(&path).expect("capture the probed bytes");
        assert!(snapshot.still_matches(&path));
        std::fs::write(
            &path,
            spool_record_json(&[("name", serde_json::json!("更正后的记录"))]),
        )
        .unwrap();
        assert!(
            !snapshot.still_matches(&path),
            "the tail guard must leave a replacement for its own fresh pass"
        );
    }
    #[tokio::test]
    async fn workspace_failure_rolls_back_the_inert_created_record() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let root = crate::platform::paths::scheduled_tasks_root();
        if root.is_dir() {
            std::fs::remove_dir(&root).unwrap();
        }
        std::fs::write(&root, b"block workspace root").unwrap();
        let error = state
            .create_task(build_create_input(&valid_record()))
            .await
            .unwrap_err();
        assert!(error.contains("workspace"), "{error}");
        assert!(
            state
                .automations
                .lock()
                .await
                .list_automations()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn selecting_a_model_id_for_a_null_model_is_not_a_noop() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let task = state
            .create_task(build_create_input(&valid_record()))
            .await
            .unwrap();
        {
            let manager = state.automations.lock().await;
            let mut record = manager.get_automation(&task.id).unwrap();
            record.model = None;
            manager.save_automation(&record).unwrap();
        }
        let mut request = valid_record();
        request.name = None;
        request.prompt = None;
        request.rrule = None;
        request.paused = None;
        request.model_id = Some("selected-model-id".to_string());
        let updated = state
            .update_task(task.id.clone(), build_update_input(&request))
            .await
            .unwrap();
        assert_eq!(
            updated.model.as_deref(),
            Some(state.fallback_model.as_str())
        );
        assert_eq!(updated.model_id.as_deref(), Some("selected-model-id"));
    }
    #[test]
    fn changing_listing_cannot_displace_tracked_requests_or_grow_state() {
        let _home = TempHome::new();
        // Real files: #680's ghost filter drops batch entries whose file
        // vanished this pass, so the fixture must exist on disk.
        let root = spool_root();
        std::fs::create_dir_all(&root).unwrap();
        let mut state = RetryState::default();
        for i in 0..MAX_PENDING_FILES {
            let name = format!("zzz-{i:04}.json");
            std::fs::write(root.join(&name), b"{}\n").unwrap();
            state.attempts.insert(name, 1);
        }
        let listing = (0..(MAX_PENDING_FILES + MAX_TAIL_QUARANTINES + 2))
            .map(|i| {
                let path = root.join(format!("aaa-new-{i:04}.json"));
                std::fs::write(&path, b"{}\n").unwrap();
                path
            })
            .collect();
        let batch = pending_batch(&root, &state, listing);
        assert_eq!(batch.len(), MAX_PENDING_FILES + MAX_TAIL_QUARANTINES);
        assert!(
            batch[..MAX_PENDING_FILES].iter().all(|path| state
                .attempts
                .contains_key(path.file_name().unwrap().to_str().unwrap())),
            "a changed listing must drain or park existing identities before admitting new ones"
        );
        assert!(batch[MAX_PENDING_FILES..].iter().all(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("aaa-new-")
        }));
    }
    #[tokio::test]
    async fn parked_debris_preserves_fresh_work_and_recovers_after_storage_repair() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        std::fs::create_dir_all(spool_root()).unwrap();
        std::fs::write(failed_dir(), b"blocked evidence directory").unwrap();
        let mut retries = RetryState::default();
        for i in 0..MAX_PENDING_FILES {
            let name = format!("parked-{i:04}.json");
            std::fs::write(spool_root().join(&name), b"poison").unwrap();
            retries
                .parked
                .insert(name.clone(), std::time::SystemTime::now());
            retries.attempts.insert(name, MAX_CREATE_ATTEMPTS);
        }
        let fresh = spool_root().join("fresh.json");
        std::fs::write(&fresh, spool_record_json(&[])).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let notifier = CountingNotifier(count.clone());
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &notifier,
            &mut retries,
        )
        .await;
        assert!(
            fresh.exists(),
            "IO backpressure must leave fresh work queued"
        );
        assert!(
            !done_dir().join("fresh.json").exists(),
            "no false ceiling receipt"
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert_eq!(retries.parked.len(), MAX_PENDING_FILES);
        std::fs::remove_file(failed_dir()).unwrap();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &notifier,
            &mut retries,
        )
        .await;
        assert!(retries.parked.is_empty());
        assert!(retries.attempts.is_empty());
        assert!(!fresh.exists());
        let receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(done_dir().join("fresh.json")).unwrap()).unwrap();
        assert_eq!(receipt["ok"], true);
        assert_eq!(count.load(Ordering::SeqCst), 1, "only fresh work applies");
        assert_eq!(
            state
                .automations
                .lock()
                .await
                .list_automations()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn parked_and_receipt_names_never_displace_fresh_apply_slots() {
        let _home = TempHome::new();
        // Real files: #680's ghost filter drops batch entries whose file
        // vanished this pass.
        let root = spool_root();
        std::fs::create_dir_all(&root).unwrap();
        let mut retries = RetryState::default();
        for i in 0..MAX_PENDING_FILES {
            retries
                .parked
                .insert(format!("parked-{i}.json"), std::time::SystemTime::now());
        }
        retries.receipts.insert(
            "receipt.json".into(),
            ("digest".into(), serde_json::json!({})),
        );
        for name in ["parked-0.json", "receipt.json", "fresh.json"] {
            std::fs::write(root.join(name), b"{}\n").unwrap();
        }
        let batch = pending_batch(
            &root,
            &retries,
            vec![
                root.join("parked-0.json"),
                root.join("receipt.json"),
                root.join("fresh.json"),
            ],
        );
        assert_eq!(batch, vec![root.join("fresh.json")]);
    }

    #[test]
    fn explicit_blank_target_is_rejected_for_every_request_kind() {
        for kind in ["create", "update", "delete"] {
            let request: SpooledCreationRequest = serde_json::from_str(&spool_record_json(&[
                ("kind", serde_json::json!(kind)),
                ("task_id", serde_json::json!("task-1")),
                ("target_session", serde_json::json!("  ")),
            ]))
            .unwrap();
            let mut request = request;
            if kind == "create" {
                request.task_id = None;
            }
            if kind == "delete" {
                request.name = None;
                request.prompt = None;
                request.rrule = None;
                request.paused = None;
            }
            let error = request.validate().unwrap_err().to_string();
            assert!(error.contains("target_session is blank"), "{kind}: {error}");
        }
    }

    #[test]
    fn missing_error_poisoning_is_scoped_to_domain_prechecks() {
        for error in [
            "scheduled task not found: task-1",
            "target_session not found: target001",
        ] {
            assert!(is_permanent_domain_error(error));
        }
        for error in [
            "Failed to delete scheduled task 'task-1': archive: No such file or directory",
            "Failed to delete scheduled task 'task-1': archive: file not found",
            "Failed to delete scheduled task 'task-1': archive: The system cannot find the file specified",
        ] {
            assert!(!is_permanent_domain_error(error));
        }
    }
}
