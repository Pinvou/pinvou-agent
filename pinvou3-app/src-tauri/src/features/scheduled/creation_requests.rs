//! Scheduled-task creation request watcher (docs/app-automations-定时任务创建工具-设计与验收.md;
//! docs/builtin-toolset-contract.md §5 L1 / §6).
//!
//! The app-automations MCP server validates a `create_scheduled_task` call and
//! spools it to `<pinvou3 home>/task-requests/spool/<name>.json` (see that
//! server.py's `create_scheduled_task` — the spool record schema is the
//! contract between the two sides; the file name is the idempotency identity:
//! the sender-scoped sha256 of "<from_session>|<idempotency_key>" when a key
//! is given, a random uuid otherwise). This module is the app-side consumer:
//!
//! - a poll watcher picks spool files up, re-validates them (server-side
//!   checks are not trusted — the spool directory is user-writable), and
//!   creates the task through the panel's own domain function
//!   ([`ScheduledTaskState::create_task`]: forced YOLO, per-task workspace,
//!   model sidecar, failure rollback — the exact pipeline the panel uses);
//! - a created request leaves a result marker `spool/.done/<file-stem>.json`
//!   (`{"ok":true,"task_id","task_name"}`) so the MCP server's short
//!   synchronous wait can return the task ids, and a retried tool call cannot
//!   create twice across watcher restarts;
//! - poison files (schema drift, hostile content, oversize) are quarantined
//!   under `spool/failed/` immediately; *transient* creation failures retry
//!   up to [`MAX_CREATE_ATTEMPTS`] times and then quarantine too, writing a
//!   `{"ok":false,"error"}` marker so a waiting caller receives the failure
//!   instead of hanging;
//! - every successful creation appends an audit record into the requesting
//!   session's execution root (contract §5 L1; model-supplied `from_session`
//!   is the same unauthenticated provenance as messaging's — the Ask rule and
//!   the audit trail are the trust boundary) and emits the panel refresh
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
//! private domain entry point without widening it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::{CreateScheduledTaskInput, ScheduledTaskDto, ScheduledTaskState};
use crate::features::assistant::platform::bridge::SCHEDULED_TASK_CREATE_TOOL;
use crate::features::sessions::validators::is_sched_session_id;

/// Same bounds as the MCP server's caps — re-checked here because the spool
/// directory is user-writable.
const MAX_NAME_CHARS: usize = 200;
const MAX_PROMPT_CHARS: usize = 32 * 1024;
const MAX_MODEL_ID_CHARS: usize = 200;
const MAX_IDEMPOTENCY_KEY_CHARS: usize = 128;
const MAX_SESSION_ID_LEN: usize = 128;
const MAX_TITLE_CHARS: usize = 200;
/// Spool file size cap: a legitimate record is bounded by the 32k-char prompt
/// (~96 KB as UTF-8 CJK) plus small metadata; anything bigger is hostile and
/// is quarantined before being read into memory.
const MAX_SPOOL_FILE_BYTES: u64 = 256 * 1024;
/// Transient creation failures retry on consecutive polls; quarantine only
/// after this many attempts (design C4: ≤3 次重试 → 隔离 + 失败标记).
const MAX_CREATE_ATTEMPTS: u32 = 3;
/// Watch poll interval: task creations are rare; the MCP server's synchronous
/// wait covers up to 5s, so 1s keeps the typical create inside one poll.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// rrule product subset (deliberately stricter than the domain parser, same
/// rules as the MCP server's `validate_rrule`): HOURLY/WEEKLY/ONCE only —
/// CRON and minute-granular frequencies are a product-level rejection, not a
/// parse error.
const WEEKDAY_TOKENS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

/// Guard for the creation-request watcher task: held in a
/// `Arc<SyncMutex<Option<_>>>` slot on [`ScheduledTaskState`] so the state can
/// derive `Clone` while the watcher keeps a single owner; Drop cancels the
/// token and aborts the task (EnginePool idle-reaper pattern).
pub(crate) struct CreationWatchGuard {
    cancel: CancellationToken,
    handle: tauri::async_runtime::JoinHandle<()>,
}

impl Drop for CreationWatchGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.handle.abort();
    }
}

/// One spooled creation request. Field names mirror server.py's
/// `_spool_payload` exactly (snake_case JSON on disk); unknown fields are
/// skipped on read (contract §4.4 drift defense). The `id` field is
/// informational only — the watcher keys its state on the directory-listed
/// file name, never on this field (a user-writable spool must not control
/// watcher paths).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct SpooledCreationRequest {
    pub schema_version: u32,
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub rrule: String,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub from_session: Option<String>,
    #[serde(default)]
    pub from_title: Option<String>,
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
    // The from_session is audit provenance only, but a sched- session must
    // never appear as the requester: scheduled-run sessions are unattended by
    // design and the Ask rule already denies the tool there — a spool record
    // claiming one is hostile (defense in depth, mirrors messaging).
    if is_sched_session_id(id) {
        bail!("from_session {id} is a scheduled-run session and cannot request task creation");
    }
    Ok(())
}

impl SpooledCreationRequest {
    /// Server-side re-validation of a spool record (contract §4.4: errors are
    /// explicit; §5: the L1 write re-checks everything it was told).
    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            bail!("unsupported spool schema_version {}", self.schema_version);
        }
        let name = self.name.trim();
        if name.is_empty() {
            bail!("task name is empty");
        }
        if name.chars().count() > MAX_NAME_CHARS {
            bail!("task name exceeds the {MAX_NAME_CHARS} character limit");
        }
        let prompt = self.prompt.trim();
        if prompt.is_empty() {
            bail!("task prompt is empty");
        }
        if prompt.chars().count() > MAX_PROMPT_CHARS {
            bail!("task prompt exceeds the {MAX_PROMPT_CHARS} character limit");
        }
        if let Some(model_id) = &self.model_id {
            let model_id = model_id.trim();
            if model_id.is_empty() {
                bail!("model_id is blank");
            }
            if model_id.chars().count() > MAX_MODEL_ID_CHARS {
                bail!("model_id exceeds the {MAX_MODEL_ID_CHARS} character limit");
            }
        }
        if let Some(key) = &self.idempotency_key {
            if key.trim().is_empty() {
                bail!("idempotency_key is blank");
            }
            if key.chars().count() > MAX_IDEMPOTENCY_KEY_CHARS {
                bail!("idempotency_key exceeds the {MAX_IDEMPOTENCY_KEY_CHARS} character limit");
            }
        }
        check_sender_session_id(self.from_session.as_ref())?;
        if let Some(title) = &self.from_title {
            if title.chars().count() > MAX_TITLE_CHARS {
                bail!("from_title exceeds the {MAX_TITLE_CHARS} character limit");
            }
        }
        validate_product_rrule(&self.rrule)?;
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
    let mut parts: Vec<(String, String)> = Vec::new();
    for raw in normalized.split(';') {
        let item = raw.trim();
        if item.is_empty() {
            continue;
        }
        let Some((key, value)) = item.split_once('=') else {
            bail!("invalid rrule segment '{item}'");
        };
        parts.push((key.trim().to_string(), value.trim().to_string()));
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

/// Creation port: the real impl drives the state's domain function; tests
/// inject a stub (native async fn in trait, used only through generic static
/// dispatch — same pattern as messaging's SpoolDelivery).
trait TaskCreator {
    async fn create(
        &self,
        input: CreateScheduledTaskInput,
    ) -> std::result::Result<ScheduledTaskDto, String>;
}

struct StateCreator<'a>(&'a ScheduledTaskState);

impl TaskCreator for StateCreator<'_> {
    async fn create(
        &self,
        input: CreateScheduledTaskInput,
    ) -> std::result::Result<ScheduledTaskDto, String> {
        self.0.create_task(input).await
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
    /// file can be removed.
    Done,
    /// Permanent rejection: quarantine now.
    Poison(anyhow::Error),
    /// Transient creation failure: retry.
    Retry(anyhow::Error),
}

fn build_create_input(request: &SpooledCreationRequest) -> CreateScheduledTaskInput {
    CreateScheduledTaskInput {
        name: request.name.trim().to_string(),
        prompt: request.prompt.trim().to_string(),
        rrule: request.rrule.trim().to_string(),
        cwds: Vec::new(),
        model: None,
        model_id: request
            .model_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        kind: None,
        mode: None,
        allow_shell: None,
        trust_mode: None,
        auto_approve: None,
        paused: Some(request.paused),
    }
}

/// Append the L1 audit record into the requesting session's execution root
/// (failures inside `audit::append` only log, never panic). Without a usable
/// `from_session` the task record itself is the audit trail (design D3).
fn audit_creation(
    sessions: &crate::features::sessions::SessionStore,
    request: &SpooledCreationRequest,
    task_id: &str,
    task_name: &str,
) {
    let Some(from) = request.from_session.as_deref() else {
        return;
    };
    let detail = serde_json::json!({
        "tool": SCHEDULED_TASK_CREATE_TOOL,
        "task_id": task_id,
        "task_name": task_name,
        "outcome": "created",
    });
    if let Ok(roots) = sessions.session_roots(from) {
        crate::features::assistant::audit::append(
            &roots.execution,
            "scheduled_task_create",
            "app",
            detail,
        );
    }
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
    let size = match std::fs::metadata(path).map(|meta| meta.len()) {
        Ok(size) => size,
        Err(error) => return poisoned(anyhow::Error::new(error).context("stat spool file")),
    };
    if size > MAX_SPOOL_FILE_BYTES {
        return poisoned(anyhow::anyhow!(
            "spool file exceeds the {} byte cap",
            MAX_SPOOL_FILE_BYTES
        ));
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return poisoned(anyhow::Error::new(error).context("read spool file")),
    };
    let request: SpooledCreationRequest = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(error) => return poisoned(anyhow::Error::new(error).context("parse spool file")),
    };
    if let Err(error) = request.validate() {
        return poisoned(error);
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let done_marker = done_dir().join(format!("{stem}.json"));
    if done_marker.exists() {
        // Idempotent retry after a completed creation: the marker wins and
        // the leftover file is dropped WITHOUT a second create (C3).
        return Processed::Done;
    }
    match creator.create(build_create_input(&request)).await {
        Ok(dto) => {
            let payload = serde_json::json!({
                "ok": true,
                "task_id": dto.id,
                "task_name": dto.name,
            });
            if let Err(error) =
                write_done_marker(&done_marker, &payload).context("write result marker")
            {
                // A missing marker means the MCP server's short wait times out
                // into "pending" and a retried call could re-create: surface
                // this as a retryable failure instead of reporting success.
                return Processed::Retry(error);
            }
            audit_creation(sessions, &request, &dto.id, &dto.name);
            notifier.notify();
            Processed::Done
        }
        Err(error) => Processed::Retry(anyhow::anyhow!(error)),
    }
}

fn write_done_marker(path: &Path, payload: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create done dir {}", parent.display()))?;
    }
    std::fs::write(path, payload.to_string())
        .with_context(|| format!("write result marker {}", path.display()))
}

fn quarantine(path: &Path) {
    let _ = std::fs::create_dir_all(failed_dir());
    if let Err(error) = std::fs::rename(
        path,
        failed_dir().join(path.file_name().unwrap_or_default()),
    ) {
        // Terminal-path failure: the poison file stays in the spool and will
        // be re-processed (and re-logged) every poll — say so loudly instead
        // of failing silently forever.
        log::warn!(
            "[scheduled-creation] quarantine rename failed for {:?}: {error}",
            path
        );
    }
}

/// Write a failure marker for an already-consumed spool file (the caller has
/// quarantined it): a still-waiting MCP call receives the failure instead of
/// hanging until its 5s timeout (design C4).
fn write_failure_marker(stem: &str, error: &anyhow::Error) {
    let payload = serde_json::json!({
        "ok": false,
        "error": format!("{error:#}"),
    });
    if let Err(marker_error) = write_done_marker(&done_dir().join(format!("{stem}.json")), &payload)
    {
        log::warn!("[scheduled-creation] failure marker write failed for {stem}: {marker_error:#}");
    }
}

/// Retry bookkeeping per file: attempt count. Entries are removed on every
/// terminal path.
#[derive(Default)]
struct RetryState {
    attempts: HashMap<String, u32>,
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
    let Ok(entries) = std::fs::read_dir(&root) else {
        return; // no spool directory yet = nothing was ever requested
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    for path in files {
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        match process_spool_file(path.as_path(), creator, sessions, notifier).await {
            Processed::Done => {
                retries.attempts.remove(&name);
                let _ = std::fs::remove_file(&path);
            }
            Processed::Poison(error) => {
                retries.attempts.remove(&name);
                log::warn!("[scheduled-creation] quarantining {name}: {error:#}");
                quarantine(&path);
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
                    retries.attempts.remove(&name);
                    quarantine(&path);
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
        let handle = tauri::async_runtime::spawn(async move {
            let mut retries = RetryState::default();
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(POLL_INTERVAL) => {
                        process_pending_spool(
                            &StateCreator(&state),
                            &state.sessions,
                            &EventNotifier(&app),
                            &mut retries,
                        )
                        .await;
                    }
                }
            }
        });
        let mut slot = self
            .creation_watch
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        *slot = Some(CreationWatchGuard { cancel, handle });
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
            "id": "abc123",
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
        assert_eq!(line["detail"]["outcome"], "created");
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
        for _ in 0..MAX_CREATE_ATTEMPTS {
            process_pending_spool(
                &FailingCreator,
                &sessions,
                &CountingNotifier(calls.clone()),
                &mut retries,
            )
            .await;
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
        request.from_session = Some("../escape".to_string());
        assert!(request.validate().is_err(), "traversal ids are rejected");
        request.from_session = None;
        assert!(
            request.validate().is_ok(),
            "absent from_session stays valid (no audit trail)"
        );
    }
}
