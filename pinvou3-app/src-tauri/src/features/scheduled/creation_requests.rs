// architecture-guard: allow-target-cfg -- the hostile-input tests (devzero-symlink spool entry, symlink at the marker tmp path) need std::os::unix::fs::symlink (unix-only) to plant non-regular files; the attribute form is the only compiling gate for those imports.
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
//!   duplicate; update/delete replay idempotently). This is the same
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
//! - every success appends a kind-specific audit record into the requesting
//!   session's execution root (contract §5 L1; model-supplied `from_session`
//!   is the same unauthenticated provenance as messaging's — the Ask rules
//!   and the audit trail are the trust boundary) and emits the panel refresh
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
use crate::features::sessions::validators::{is_aux_session_id, is_sched_session_id};

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
/// Round-11 sibling parity: the pending-file ceiling — the sorted tail
/// beyond this many queued records is quarantined with failure markers
/// (hostile growth bounded; each excess record answers its recorded error
/// on the next call instead of fresh pending).
const MAX_PENDING_FILES: usize = 256;
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
    // The from_session is audit provenance only, but isolated sessions must
    // never appear as the requester: sched- sessions are unattended by
    // design, eval_ is benchmark-private, and aux- marks auxiliary
    // side-chats (the MCP server rejects all three for every request kind;
    // re-checked here because the spool directory is user-writable —
    // defense in depth, mirrors messaging).
    if is_sched_session_id(id)
        || is_aux_session_id(id)
        || id.to_ascii_lowercase().starts_with("eval_")
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
    /// told). Mirrors the MCP server's per-kind validation exactly.
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
        let Ok(number) = value.parse::<u32>() else {
            bail!("invalid rrule: {key} must be an integer, got '{value}'");
        };
        if number < minimum || number > maximum {
            bail!("invalid rrule: {key} must be between {minimum} and {maximum}");
        }
        Ok(number)
    };
    let parse_byday = |value: &str| -> Result<()> {
        let tokens: Vec<&str> = value
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect();
        if tokens.is_empty() {
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
    /// Permanent rejection: quarantine now.
    Poison(anyhow::Error),
    /// Transient creation failure: retry.
    Retry(anyhow::Error),
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
        ]
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(label, _)| label)
        .collect();
        detail["changed"] = serde_json::json!(changed);
    }
    if let Ok(roots) = sessions.session_roots(from) {
        crate::features::assistant::audit::append(&roots.execution, kind, "app", detail);
    }
}

/// Shadow audit (review R4-M3): an append-only trace under the automation
/// store root, written for EVERY applied request regardless of
/// `from_session`. The session-workspace audit above early-returns when the
/// model-supplied `from_session` is omitted — and an unattended session can
/// omit it precisely to skip the claimed-origin trace — so this
/// watcher-side line is the floor the requester cannot skip: the watcher
/// sees the spool file no matter what the record claims about its sender.
/// The spool file NAME (not the record fields) keys the identity column,
/// and the claimed sender is recorded as explicitly unverified.
fn audit_request_shadow(
    request: &SpooledCreationRequest,
    spool_stem: &str,
    task_id: &str,
    task_name: &str,
    outcome: &str,
) {
    let dir = crate::features::scheduled::tasks::scheduled_automation_root().join("audit");
    // audit::append is best-effort and does not create the parent; the
    // shadow trail must survive a fresh install's first write.
    let _ = std::fs::create_dir_all(&dir);
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
        "spool_stem": spool_stem,
        "claimed_from_session": request.from_session,
        "claimed_from_session_verified": false,
    });
    crate::features::assistant::audit::append(&dir, "scheduled_task_request", "app", detail);
}

/// Best-effort failure audit: a poison or retry-exhausted record with a
/// usable `from_session` leaves a trace in that session's execution root —
/// the audit trail must not be success-only. Runs before quarantine because
/// it re-reads the spool file.
fn audit_failure(
    sessions: &crate::features::sessions::SessionStore,
    path: &Path,
    error: &anyhow::Error,
) {
    // Round-11 minor 7: the evidence re-read is gated+bounded too.
    let Some(bytes) = read_regular_bounded(path, MAX_SPOOL_FILE_BYTES as usize) else {
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
    crate::features::assistant::audit::append(
        &roots.execution,
        "scheduled_task_failed",
        "app",
        detail,
    );
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
    let _ = std::fs::remove_file(&done_marker);
    let applied = match request.kind {
        SpoolRequestKind::Create => creator.create(build_create_input(&request)).await,
        SpoolRequestKind::Update => {
            let task_id = request.task_id.as_deref().unwrap_or_default().to_string();
            creator.update(&task_id, build_update_input(&request)).await
        }
        SpoolRequestKind::Delete => {
            let task_id = request.task_id.as_deref().unwrap_or_default().to_string();
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
            if let Err(error) =
                write_done_marker(&done_marker, &payload).context("write result marker")
            {
                // A missing marker means the MCP server's short wait times out
                // into "pending" and a retried call could re-apply: surface
                // this as a retryable failure instead of reporting success.
                return Processed::Retry(error);
            }
            audit_request(sessions, &request, &dto.id, &dto.name);
            notifier.notify();
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
            Processed::Retry(anyhow::anyhow!(error))
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
        ("model_id", serde_json::json!(&request.model_id)),
        ("name", serde_json::json!(&request.name)),
        ("paused", serde_json::json!(normalized_paused)),
        ("prompt", serde_json::json!(&request.prompt)),
        ("rrule", serde_json::json!(&request.rrule)),
        ("task_id", serde_json::json!(&request.task_id)),
    ]);
    use sha2::Digest;
    let serialized = serde_json::to_string(&canonical).unwrap_or_default();
    let digest = sha2::Sha256::digest(serialized.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Round-11 MAJOR-2/3: stat + regular-file gate BEFORE any read, then a
/// capped read. `symlink_metadata` never follows the final component, so a
/// planted FIFO is refused by `is_file()` instead of opened blocking (the
/// single drain task would wedge forever), a symlink to /dev/zero cannot
/// spin an infinite read, and a sparse oversize file hits the cap instead
/// of being slurped whole at 1 Hz. `None` = missing/not a regular file/
/// oversize/unreadable — callers treat that as "no usable bytes".
fn read_regular_bounded(path: &Path, cap: usize) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > cap as u64 {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    file.take(cap as u64 + 1).read_to_end(&mut buf).ok()?;
    if buf.len() > cap {
        return None;
    }
    Some(buf)
}

fn write_done_marker(path: &Path, payload: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create done dir {}", parent.display()))?;
    }
    // Atomic tmp+rename: the server polls this file, so it must never
    // observe a torn write. Round-11 sibling parity: the tmp path is
    // model-computable (sha256(from|kind|task_id|key)) — a planted FIFO
    // would block fs::write forever and wedge the single watcher task.
    // Refuse non-regular tmp files before writing.
    let tmp = path.with_extension("json.tmp");
    if tmp
        .symlink_metadata()
        .map(|meta| !meta.is_file())
        .unwrap_or(false)
    {
        anyhow::bail!(
            "result marker tmp path is not a regular file: {}",
            tmp.display()
        );
    }
    std::fs::write(&tmp, payload.to_string())
        .with_context(|| format!("write result marker {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("publish result marker {}", path.display()))
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
            if value.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
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

fn quarantine(path: &Path) {
    let _ = std::fs::create_dir_all(failed_dir());
    let mut target = failed_dir().join(path.file_name().unwrap_or_default());
    if target.exists() {
        // Never overwrite earlier evidence: suffix a counter until free.
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        for counter in 1..u32::MAX {
            let candidate = failed_dir().join(format!("{stem}.{counter}{ext}"));
            if !candidate.exists() {
                target = candidate;
                break;
            }
        }
    }
    if let Err(error) = std::fs::rename(path, &target) {
        // Terminal-path failure: the poison file stays in the spool and will
        // be re-processed (and re-logged) every poll — say so loudly instead
        // of failing silently forever.
        log::warn!(
            "[scheduled-creation] quarantine rename failed for {:?}: {error}",
            path
        );
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

/// Age-based retention for terminal state (see [`STATE_RETENTION`]): prune
/// stale markers in `.done/` and evidence in `failed/`, and sweep stray
/// `*.tmp` files left by a crash between mkstemp and rename on the server
/// side (nothing else ever removes them).
fn prune_stale_state() {
    prune_stale_state_at(std::time::SystemTime::now());
}

/// The clock is injected so tests can age files without sleeping.
fn prune_stale_state_at(now: std::time::SystemTime) {
    let prune_dir = |dir: &Path| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= STATE_RETENTION);
            if stale {
                // Best effort: a busy file is retried on the next sweep.
                let _ = std::fs::remove_file(&path);
            }
        }
    };
    prune_dir(&done_dir());
    prune_dir(&failed_dir());
    if let Ok(entries) = std::fs::read_dir(spool_root()) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_stray_tmp = path.extension().is_some_and(|ext| ext == "tmp");
            if !is_stray_tmp {
                continue;
            }
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= STATE_RETENTION);
            if stale {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// Retry bookkeeping per file: attempt count. Entries are removed on every
/// terminal path. `spool_dir_error_logged` keeps the unreadable-directory
/// warning to one line per streak instead of 1 Hz spam.
#[derive(Default)]
struct RetryState {
    attempts: HashMap<String, u32>,
    spool_dir_error_logged: bool,
    /// Round-1 M4 (#680 review), generalized round-3 M4/M5: stuck records
    /// whose episode already appended its audit line + wrote its failure
    /// marker — a stuck record used to append one jsonl line and rewrite
    /// the marker EVERY poll (86,400/day on an append-only jsonl with no
    /// rotation; the read/parse/digest probe stays per-poll, m-C — only
    /// the append and the write are deduped). The value is the audited record's digest for gate-arm
    /// episodes (identity: a same-name corrected re-spool with a DIFFERENT
    /// digest starts a fresh episode instead of being gated on the old
    /// budget — round-3 M5), and empty for Poison/ceiling-tail episodes
    /// (no identity to rebind). Entries clear when the record finally
    /// moves, and the poll's vanished-name retain drops entries whose file
    /// was deleted externally (round-3 m2 — the messaging vanished-name
    /// guard's shape).
    stuck_episode_audited: std::collections::HashMap<String, String>,
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
    // Round-6 minor 6: the switch pauses the drain too (messaging parity) —
    // `scheduled-task-automation` off hides the tools AND stops already-
    // spooled requests from applying; the queue resumes on re-enable.
    if crate::features::marketplace::builtin::feature_disabled_tool_names()
        .iter()
        .any(|tool| tool == SCHEDULED_TASK_CREATE_TOOL)
    {
        return;
    }
    let root = spool_root();
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => {
            retries.spool_dir_error_logged = false;
            entries
        }
        // No spool directory yet = nothing was ever requested.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        // Anything else (permissions, ...) is a real anomaly: warn once per
        // streak instead of silently spinning at 1 Hz forever.
        Err(error) => {
            if !retries.spool_dir_error_logged {
                log::warn!("[scheduled-creation] cannot read the spool directory: {error}");
                retries.spool_dir_error_logged = true;
            }
            return;
        }
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    // Round-3 m2 / round-4 R1: the retain set is built from the PRE-SPLIT
    // listing — the ceiling tail names were listed too, and building it
    // after split_off dropped their just-inserted episode entries every
    // poll, reinstating the per-poll audit/marker spam the dedupe exists
    // to stop (and re-arming a budget-exhausted record that drifts into
    // the tail). Entries for externally-vanished files (deleted by the
    // same user mid-episode) are what this retain exists to drop — a
    // keyed re-spool would otherwise inherit the stale counts/identity.
    let listed: std::collections::HashSet<String> = files
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .collect();
    retries.attempts.retain(|name, _| listed.contains(name));
    retries
        .stuck_episode_audited
        .retain(|name, _| listed.contains(name));
    // Round-3 m-D: a hostile 10k-file burst reserves O(N) buckets that
    // HashMap::retain never shrinks — release them when everything is
    // empty again.
    if retries.attempts.is_empty() && retries.stuck_episode_audited.is_empty() {
        retries.attempts.shrink_to_fit();
        retries.stuck_episode_audited.clear();
    }

    // Round-11 sibling parity: the ceiling quarantines the sorted tail.
    // The record read is BOUNDED (the tail is exactly the hostile zone,
    // never slurped whole), and a digest-bound readable ok:true receipt is
    // never overwritten. Round-1 M5 (#680 review): the marker names the
    // REAL cause — a corrupt/oversize tail record used to be mislabeled
    // "ceiling exceeded", hiding the actual fault from the waiting call.
    if files.len() > MAX_PENDING_FILES {
        for path in files.split_off(MAX_PENDING_FILES) {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<unnamed>")
                .to_string();
            let marker_reason = match read_regular_bounded(&path, MAX_SPOOL_FILE_BYTES as usize) {
                Some(bytes) => match serde_json::from_slice::<SpooledCreationRequest>(&bytes) {
                    // Only a digest-bound recorded success suppresses the
                    // failure marker (an ok receipt already answers it).
                    Ok(request) => {
                        let stem = path.file_stem().and_then(|s| s.to_str());
                        let suppressed = stem
                            .map(|stem| {
                                let marker = done_dir().join(format!("{stem}.json"));
                                result_marker_suppresses(&marker, &spool_request_digest(&request))
                            })
                            .unwrap_or(false);
                        if suppressed {
                            None
                        } else {
                            Some(anyhow::anyhow!(
                                "pending-file ceiling {MAX_PENDING_FILES} exceeded"
                            ))
                        }
                    }
                    Err(error) => Some(
                        anyhow::Error::new(error)
                            .context("pending-ceiling tail record is unparseable"),
                    ),
                },
                None => Some(anyhow::anyhow!(
                    "pending-ceiling tail record is unreadable or oversize"
                )),
            };
            // Round-3 M4: the audit APPEND and the marker REWRITE fire
            // once per stuck episode (a squatted failed/ used to append a
            // jsonl line and rewrite the marker for every tail record
            // EVERY poll — the read/parse/digest probe itself stays
            // per-poll, m-C); the quarantine retry stays per-poll so the
            // record still self-heals when failed/ unlocks. Round-3 M3:
            // the disposition is AUDITED like every other terminal arm
            // (the tail can hold legitimate-looking records — the trail
            // must show where they went and why).
            if let Some(reason) = marker_reason.as_ref() {
                if retries
                    .stuck_episode_audited
                    .insert(name.clone(), String::new())
                    .is_none()
                {
                    audit_failure(sessions, &path, &reason);
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        write_failure_marker(stem, &reason);
                    }
                }
            }
            log::warn!(
                "[scheduled-creation] quarantining {name}: {}",
                marker_reason
                    .as_ref()
                    .map(|e| format!("{e:#}"))
                    .unwrap_or_else(|| "recorded ok receipt".to_string())
            );
            quarantine(&path);
            if !path.exists() {
                retries.attempts.remove(&name);
                retries.stuck_episode_audited.remove(&name);
            }
        }
    }
    for path in files {
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        // Round-11: the retry budget GATES the apply — the exhausted state
        // (marker write + quarantine rename both persistently failing) must
        // stop writing tasks; unconditional clears would re-arm the
        // compound failure every poll (~3 domain writes per poll, forever).
        // The constraint that makes the gate SAFE (round-3 M5): the gate is
        // about the RECORD we audited, not the file name — the server
        // replaces same-name keyed re-spools, so a corrected payload at the
        // same name must start a FRESH episode (identity = payload digest),
        // or the gate would silently drop a healthy corrected request and
        // quarantine it untried when failed/ unlocks.
        if retries
            .attempts
            .get(&name)
            .map(|count| *count >= MAX_CREATE_ATTEMPTS)
            .unwrap_or(false)
        {
            let current_digest = read_regular_bounded(&path, MAX_SPOOL_FILE_BYTES as usize)
                .and_then(|bytes| serde_json::from_slice::<SpooledCreationRequest>(&bytes).ok())
                .map(|request| spool_request_digest(&request));
            let audited = retries.stuck_episode_audited.get(&name).cloned();
            if let (Some(current), Some(audited)) = (current_digest.clone(), audited) {
                // An empty identity marks a Poison episode (its record was
                // unparseable) — a record that PARSES now necessarily
                // differs from the one audited, so it rebinds too
                // (round-4 m-A).
                if audited.is_empty() || audited != current {
                    // A corrected same-name re-spool: fresh episode.
                    log::warn!(
                        "[scheduled-creation] {name} was replaced with a corrected payload; resetting the exhausted budget for the new body"
                    );
                    retries.attempts.remove(&name);
                    retries.stuck_episode_audited.remove(&name);
                    continue;
                }
            }
            log::warn!(
                "[scheduled-creation] retry budget exhausted for {name} (marker and quarantine both failing); not re-applying"
            );
            let exhausted_error = anyhow::anyhow!(
                "scheduled task request could not be finalized on the app side (the result marker and the quarantine move both keep failing); check the Scheduled Tasks panel before retrying"
            );
            // Round-1 M4: the audit line and the marker rewrite fire ONCE
            // per stuck episode, not every poll; the quarantine retry below
            // stays per-poll so the record still self-heals when failed/
            // unlocks.
            if retries
                .stuck_episode_audited
                .insert(name.clone(), current_digest.unwrap_or_else(|| name.clone()))
                .is_none()
            {
                audit_failure(sessions, &path, &exhausted_error);
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| name.clone());
                write_failure_marker(&stem, &exhausted_error);
            }
            quarantine(&path);
            if !path.exists() {
                retries.attempts.remove(&name);
                retries.stuck_episode_audited.remove(&name);
            }
            continue;
        }
        match process_spool_file(path.as_path(), creator, sessions, notifier).await {
            Processed::Done(applied_digest) => {
                // Round-1 R2: the clear is UNCONDITIONAL, and that is safe
                // HERE because receipt suppression upstream dedups the Done
                // loop (a persistently failing removal re-runs Done without
                // re-applying); keeping the entry instead let a same-name
                // keyed re-spool inherit a stale count and quarantine a
                // fresh record after one failure.
                retries.attempts.remove(&name);
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
                let still_applied = read_regular_bounded(&path, MAX_SPOOL_FILE_BYTES as usize)
                    .and_then(|bytes| serde_json::from_slice::<SpooledCreationRequest>(&bytes).ok())
                    .map(|request| spool_request_digest(&request) == applied_digest)
                    .unwrap_or(false);
                if !still_applied {
                    log::warn!(
                        "[scheduled-creation] {name} was replaced while its request was being applied; leaving the newest payload for the next poll"
                    );
                    continue;
                }
                if let Err(error) = std::fs::remove_file(&path) {
                    // The marker exists, so the operation stays deduped, but
                    // a stuck file would loop Done/remove every poll — say
                    // so instead of failing silently. No budget concern: the
                    // entry was already cleared above (receipt suppression
                    // dedups this loop; the gate arm is what stops applies).
                    log::warn!(
                        "[scheduled-creation] processed {name} but could not remove it: {error}"
                    );
                }
            }
            Processed::Poison(error) => {
                log::warn!("[scheduled-creation] quarantining {name}: {error:#}");
                // Round-3 M4: the audit line and the failure marker fire ONCE
                // per stuck episode — a squatted failed/ used to re-append
                // the jsonl line and rewrite the marker every poll
                // (~86,400/day). The quarantine retry stays per-poll so the
                // record still self-heals when failed/ unlocks.
                let first_of_episode = retries
                    .stuck_episode_audited
                    .insert(name.clone(), String::new())
                    .is_none();
                if first_of_episode {
                    audit_failure(sessions, &path, &error);
                }
                quarantine(&path);
                // Round-1 R2: the clear sits AFTER the move — before it,
                // path.exists() was always true and the clear never ran (a
                // leaked entry let a same-name keyed re-spool inherit a
                // stale count and quarantine a fresh record after one
                // failure). Round-4 m-A: the EPISODE entry clears here too
                // — a lingering empty-identity entry would silence the
                // audit/marker of a fresh same-name poison episode, and its
                // empty identity would block the gate arm's rebind for a
                // corrected re-spool.
                if !path.exists() {
                    retries.attempts.remove(&name);
                    retries.stuck_episode_audited.remove(&name);
                }
                if first_of_episode {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&name)
                        .to_string();
                    write_failure_marker(&stem, &error);
                }
            }
            Processed::Retry(error) => {
                let count = {
                    let attempt = retries.attempts.entry(name.clone()).or_insert(0);
                    *attempt += 1;
                    *attempt
                };
                if count >= MAX_CREATE_ATTEMPTS {
                    log::warn!(
                        "[scheduled-creation] quarantining {name} after {count} create attempts: {error:#}"
                    );
                    audit_failure(sessions, &path, &error);
                    quarantine(&path);
                    // Round-11 sibling parity: the budget clears only when
                    // the quarantine rename actually moved the file — the
                    // unconditional clear re-armed the compound failure
                    // (failing rename + marker-write loop) every poll.
                    if !path.exists() {
                        retries.attempts.remove(&name);
                    }
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&name)
                        .to_string();
                    write_failure_marker(&stem, &error);
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
                    prune_stale_state();
                    last_prune = tokio::time::Instant::now();
                }
                // Round-6 minor 4: the messaging sibling's panic isolation —
                // one poisoned record must not kill the drain for the app
                // lifetime (every later tool call would burn its 5s window
                // on a dead loop). The state refs are bound outside the
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
            "ee13de67c7837ccbb71afbf4c376e6f58bf80509bf7e79744c60c9ed6cd639fe"
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
            "3e5a7833ac3bd752cf7cc5dd72b0ff4347a65c4155bbe2b69c0fb61673d9bf53"
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
        std::fs::write(spool.join("stuck.json"), spool_record_json(&[])).unwrap();
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

    #[test]
    fn sender_validation_rejects_isolated_and_malformed_ids() {
        let mut request = valid_record();
        request.from_session = Some("sched-run1".to_string());
        assert!(request.validate().is_err(), "sched- senders are rejected");
        // Round-12 M5 pin: the eval_ rejection folds case — the uppercase
        // variant used to pass the watcher gate while the server rejected
        // it, a silent watcher/server divergence on the sender gate.
        request.from_session = Some("EVAL_b1".to_string());
        assert!(
            request.validate().is_err(),
            "uppercase EVAL_ senders are rejected (case-folded)"
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
                (
                    "rrule",
                    serde_json::json!("freq=weekly;byday=fr;byhour=20;byminute=0"),
                ),
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
            updated.rrule, "FREQ=WEEKLY;BYDAY=FR;BYHOUR=20;BYMINUTE=0",
            "update normalization lands uppercase"
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
    async fn delete_request_archives_and_removes_the_task() {
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
                .is_err(),
            "the task is gone from the active store"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("del.json")).expect("marker"),
        )
        .unwrap();
        assert_eq!(marker["kind"], "delete");
        assert_eq!(marker["task_id"], created.id);
    }

    /// Round-8 M3-1: the R7 probe-order fix finally has its pin — a
    /// leftover FREQ=ONCE record whose AT has since PASSED (validate would
    /// fail → the poison arm whose failure-marker write would overwrite
    /// the ok:true receipt) suppresses on its recorded success instead:
    /// Done, no quarantine, marker intact, no create.
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

    /// Round-11 sibling parity: the budget-stickiness pin — with failed/
    /// squatted by a regular file, an exhausted budget must gate the apply
    /// across polls (the unconditional clears re-armed the compound
    /// failure: ~3 domain writes per poll, forever).
    #[tokio::test]
    async fn exhausted_budget_gates_across_polls_when_rename_fails() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("stuck.json"), spool_record_json(&[])).unwrap();
        // Squat the quarantine directory with a regular file: every rename
        // into failed/ fails, so the record stays.
        std::fs::write(failed_dir(), b"not a directory").unwrap();
        let mut retries = RetryState::default();
        retries
            .attempts
            .insert("stuck.json".to_string(), MAX_CREATE_ATTEMPTS);
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
            "first poll: the exhausted budget gates the apply"
        );
        assert!(
            spool.join("stuck.json").exists(),
            "the rename failed; the record stays"
        );
        assert_eq!(
            retries.attempts.get("stuck.json").copied(),
            Some(MAX_CREATE_ATTEMPTS),
            "the budget was not recycled by the failing rename"
        );
        // Round-3 M6a: the audit line + failure marker are ONCE per stuck
        // episode — the jsonl line count must not grow across polls (the
        // one-shot insert made unconditional = 86,400 lines/day shipped
        // green before this pin). The workspace dir is created FIRST so
        // the audit appends actually land.
        let exec_root = state
            .sessions
            .session_roots("reqsrc01")
            .expect("roots")
            .execution;
        std::fs::create_dir_all(&exec_root).unwrap();
        let audit_lines = || {
            std::fs::read_to_string(exec_root.join("workflow_audit.jsonl"))
                .map(|c| c.lines().count())
                .unwrap_or(0)
        };
        let _ = audit_lines();
        let audit_lines = || {
            std::fs::read_to_string(exec_root.join("workflow_audit.jsonl"))
                .map(|c| c.lines().count())
                .unwrap_or(0)
        };
        let marker_before =
            std::fs::read_to_string(done_dir().join("stuck.json")).unwrap_or_default();
        let first_count = audit_lines();
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
            "second poll: still gated — the budget does not re-buy"
        );
        assert_eq!(
            audit_lines(),
            first_count,
            "the stuck episode does not re-append an audit line per poll"
        );
        assert_eq!(
            std::fs::read_to_string(done_dir().join("stuck.json")).unwrap_or_default(),
            marker_before,
            "the failure marker is not rewritten per poll"
        );
        assert!(
            retries
                .stuck_episode_audited
                .get("stuck.json")
                .map(|digest| !digest.is_empty())
                .unwrap_or(false),
            "the episode stays audited (with the record's digest identity) while stuck"
        );
        // Round-4 R2: the digest-identity REBIND is the fix's headline — a
        // corrected same-name re-spool must start a fresh episode and
        // APPLY, not be gated on the old record's exhausted budget
        // (deleting the rebind block ships this red).
        std::fs::write(
            spool.join("stuck.json"),
            spool_record_json(&[
                ("idempotency_key", serde_json::json!("k-stuck")),
                ("name", serde_json::json!("更正后的名字")),
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
            retries.attempts.is_empty(),
            "the corrected body resets the exhausted budget"
        );
        assert!(
            !failed_dir().join("stuck.json").exists(),
            "the corrected body is not quarantined untried when failed/ unlocks"
        );
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
            "the corrected body applies after the rebind"
        );
        assert!(!spool.join("stuck.json").exists(), "then is consumed");
        assert!(
            done_dir().join("stuck.json").exists(),
            "its receipt is written"
        );
    }

    /// Round-11 sibling parity: the pending-file ceiling — the sorted tail
    /// beyond 256 is quarantined with failure markers and applies nothing.
    #[tokio::test]
    async fn pending_ceiling_quarantines_tail_without_applying() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        for i in 0..=(MAX_PENDING_FILES as u32) {
            let name = format!("c{i:05}.json");
            std::fs::write(spool.join(&name), spool_record_json(&[])).unwrap();
        }
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
            .len();
        assert_eq!(
            created, MAX_PENDING_FILES,
            "exactly the ceiling applies; the tail never reaches the domain"
        );
        assert!(
            failed_dir()
                .join(format!("c{:05}.json", MAX_PENDING_FILES))
                .exists(),
            "the sorted tail is the quarantined excess"
        );
        // Round-3 M6b: the tail's failure marker is the waiting caller's
        // answer — a mutant that suppresses it (silently starving the
        // poll) shipped green before this read.
        let tail_marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join(format!("c{:05}.json", MAX_PENDING_FILES)))
                .expect("failure marker for the ceiling tail"),
        )
        .unwrap();
        assert_eq!(tail_marker["ok"], false);
        assert!(
            tail_marker["error"].as_str().unwrap().contains(&format!(
                "pending-file ceiling {MAX_PENDING_FILES} exceeded"
            )),
            "the marker names the ceiling: {}",
            tail_marker["error"]
        );
    }

    /// Round-3 M6b: a CORRUPT tail record's marker names the real cause
    /// (unparseable), not the ceiling — the mislabel hid the actual fault
    /// from the waiting call.
    #[tokio::test]
    async fn corrupt_ceiling_tail_gets_the_real_reason() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        for i in 0..MAX_PENDING_FILES {
            let name = format!("c{i:05}.json");
            std::fs::write(spool.join(&name), spool_record_json(&[])).unwrap();
        }
        std::fs::write(spool.join("c99999.json"), b"not json{").unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(failed_dir().join("c99999.json").exists());
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("c99999.json")).expect("failure marker"),
        )
        .unwrap();
        assert!(
            marker["error"].as_str().unwrap().contains("unparseable"),
            "the marker names the real cause: {}",
            marker["error"]
        );
    }

    /// Round-4 R1: a stuck CEILING-TAIL record's audit/marker fire ONCE per
    /// episode — the pre-split `listed` set must include tail names, or the
    /// retain drops their episode entries every poll and reinstates the
    /// per-poll audit spam (probe-proven [1,2,3] before the ordering fix).
    #[tokio::test]
    async fn stuck_ceiling_tail_audit_is_once_per_episode() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::create_dir_all(done_dir()).unwrap();
        // Parseable but invalid (CRON rrule): validate() poisons them AND
        // audit_failure can attribute them (unparseable records have the
        // documented no-audit carve-out, so they cannot pin this).
        for i in 0..=MAX_PENDING_FILES {
            let name = format!("c{i:05}.json");
            std::fs::write(
                spool.join(&name),
                spool_record_json(&[("rrule", serde_json::json!("FREQ=CRON;*"))]),
            )
            .unwrap();
        }
        // Squat failed/: every quarantine rename fails, so every record
        // re-runs its terminal arm every poll.
        std::fs::write(failed_dir(), b"not a directory").unwrap();
        let exec_root = state
            .sessions
            .session_roots("reqsrc01")
            .expect("roots")
            .execution;
        std::fs::create_dir_all(&exec_root).unwrap();
        let audit_lines = || {
            std::fs::read_to_string(exec_root.join("workflow_audit.jsonl"))
                .map(|c| c.lines().count())
                .unwrap_or(0)
        };
        let mut retries = RetryState::default();
        let mut counts = Vec::new();
        for _ in 0..3 {
            process_pending_spool(
                &StateCreator(&state),
                &state.sessions,
                &CountingNotifier(Arc::new(AtomicUsize::new(0))),
                &mut retries,
            )
            .await;
            counts.push(audit_lines());
        }
        assert_eq!(
            counts[0], counts[1],
            "poll 2 must not re-append tail audit lines: {counts:?}"
        );
        assert_eq!(
            counts[1], counts[2],
            "poll 3 must not re-append tail audit lines: {counts:?}"
        );
        assert!(
            counts[0] > 0,
            "fixture guard: every record audited exactly once on poll 1"
        );
    }

    /// Round-1 M6 (#680 review): the tmp refusal is BEHAVIORAL — a symlink
    /// planted at the model-computable tmp path is refused, not followed
    /// (an inverted is_file() previously passed CI silently; a FIFO there
    /// would block the write forever).
    #[cfg(unix)]
    #[test]
    fn marker_tmp_path_refuses_non_regular_files() {
        let _home = TempHome::new();
        std::fs::create_dir_all(done_dir()).unwrap();
        let marker = done_dir().join("t.json");
        std::os::unix::fs::symlink("/dev/zero", done_dir().join("t.json.tmp")).unwrap();
        assert!(
            write_done_marker(&marker, &serde_json::json!({"ok": true})).is_err(),
            "a symlinked tmp path is refused, not followed"
        );
        assert!(
            !marker.exists(),
            "no marker is published through the symlink"
        );
    }

    /// Round-1 M6 + M5 (#680 review): an oversize sorted-tail record is
    /// quarantined and its failure marker names the REAL cause (oversize),
    /// not the ceiling label.
    #[tokio::test]
    async fn oversize_ceiling_tail_gets_the_real_reason() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        for i in 0..MAX_PENDING_FILES {
            let name = format!("c{i:05}.json");
            std::fs::write(spool.join(&name), spool_record_json(&[])).unwrap();
        }
        // The sorted-tail record: valid shape but over the byte cap.
        let mut oversize = spool_record_json(&[]);
        oversize.insert_str(
            oversize.len() - 1,
            &format!(",\"pad\":\"{}\"", "x".repeat(MAX_SPOOL_FILE_BYTES as usize)),
        );
        std::fs::write(spool.join("c99999.json"), oversize).unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            failed_dir().join("c99999.json").exists(),
            "the oversize tail is quarantined"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("c99999.json")).expect("failure marker"),
        )
        .unwrap();
        assert_eq!(marker["ok"], false);
        let error = marker["error"].as_str().unwrap();
        assert!(
            error.contains("unreadable or oversize"),
            "the marker names the real cause: {error}"
        );
        assert!(
            !error.contains(&format!(
                "pending-file ceiling {MAX_PENDING_FILES} exceeded"
            )),
            "the ceiling label must not mask the real cause: {error}"
        );
    }

    /// Round-1 M6 (#680 review): a ceiling-tail record whose digest-bound
    /// ok receipt already landed writes NO failure marker — the receipt
    /// answers it (the const doc's "answers its recorded error" applies
    /// only to records without one).
    #[tokio::test]
    async fn ceiling_tail_with_landed_receipt_writes_no_failure_marker() {
        let _home = TempHome::new();
        let state = watcher_state().await;
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::create_dir_all(done_dir()).unwrap();
        for i in 0..MAX_PENDING_FILES {
            let name = format!("c{i:05}.json");
            std::fs::write(spool.join(&name), spool_record_json(&[])).unwrap();
        }
        let tail_request =
            serde_json::from_slice::<SpooledCreationRequest>(spool_record_json(&[]).as_bytes())
                .unwrap();
        let digest = spool_request_digest(&tail_request);
        std::fs::write(
            done_dir().join("c99999.json"),
            serde_json::json!({"ok": true, "task_id": "already-there", "request_digest": digest})
                .to_string(),
        )
        .unwrap();
        std::fs::write(spool.join("c99999.json"), spool_record_json(&[])).unwrap();
        let mut retries = RetryState::default();
        process_pending_spool(
            &StateCreator(&state),
            &state.sessions,
            &CountingNotifier(Arc::new(AtomicUsize::new(0))),
            &mut retries,
        )
        .await;
        assert!(
            failed_dir().join("c99999.json").exists(),
            "the tail is still quarantined (ceiling is terminal either way)"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("c99999.json")).expect("receipt intact"),
        )
        .unwrap();
        assert_eq!(
            marker["ok"], true,
            "the landed receipt is not overwritten by a failure marker"
        );
        // Round-4 m-B: what the suppression branch buys is audit accuracy —
        // a receipt-answered tail must NOT gain a "ceiling exceeded"
        // failure audit line (the marker half is independently protected by
        // write_failure_marker's ok-receipt guard; this line is not).
        let audit_root = crate::platform::paths::sessions_root()
            .join("reqsrc01")
            .join("workspace")
            .join("workflow_audit.jsonl");
        assert!(
            !audit_root.exists()
                || !std::fs::read_to_string(&audit_root)
                    .unwrap_or_default()
                    .contains("pending-file ceiling"),
            "a receipt-answered tail must not be audited as a ceiling failure"
        );
    }

    /// Round-9 M5 (#628): the receipt guard — apply a record (receipt
    /// lands), corrupt the spool bytes to invalid JSON (post-apply
    /// corruption — the exact class probe-before-validate CANNOT cover),
    /// re-drain: the poison arm must NOT overwrite the ok:true receipt
    /// (the last-resort idempotency defense).
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
        let dir = std::env::temp_dir().join(format!("pinvou-marker-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("m.json");
        // Success with a matching digest suppresses the replay (C3).
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
        // Round-4 m10 (the assert round-3 claimed but had not shipped):
        // the poison path clears the attempt budget.
        assert!(retries.attempts.is_empty(), "poison clears the budget");
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
                .join("sched-run9")
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
                ("from_session", serde_json::json!("sched-run9")),
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
                .join("sched-run9")
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

    #[test]
    fn isolated_senders_keyless_ids_and_stray_delete_fields_are_rejected() {
        for sender in ["sched-run1", "SCHED-run1", "aux-side1", "eval_b1"] {
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
        assert!(
            !spool.join("nosender.json").exists(),
            "the request itself was applied and consumed"
        );
    }
}
