//! Cross-session messaging delivery (docs/builtin-toolset-contract.md §5 L1 / §6).
//!
//! The session-reader MCP server validates a `send_message_to_session` call and
//! spools it to `<pinvou3 home>/messaging/spool/<name>.json` (see server.py's
//! `send_message_to_session` — the spool record schema is the contract between
//! the two sides; the file name is the idempotency identity: sha256 of
//! "<from_session>|<idempotency_key>" when a key is given, a random uuid
//! otherwise). This module is the app-side consumer:
//!
//! - a poll watcher picks spool files up, re-validates them (server-side
//!   defense in depth — the spool directory is user-writable), and delivers
//!   through the engine pool;
//! - delivery semantics: target mid-turn → [`EnginePool::steer`] into the
//!   current turn; target idle / engine not live → a new turn dispatched
//!   immediately (the scheduled-task wake precedent);
//! - every delivery writes an audit record into both sessions' workspaces
//!   (`assistant::audit`, contract §5 L1 requirement);
//! - a delivered idempotency-keyed message leaves a marker under
//!   `spool/.done/<file-stem>` (the stem is the sender-scoped key hash), so a
//!   retried tool call cannot deliver twice across watcher restarts;
//! - poison files (schema drift, hostile content, oversize) are quarantined
//!   under `spool/failed/` immediately; *transient* delivery failures (rewind
//!   gates, engine spawn errors) retry with backoff and only quarantine after
//!   [`MAX_DELIVERY_ATTEMPTS`].
//!
//! Known delivery guarantees (documented, accepted for v1):
//! - **At-least-once, not exactly-once**: a crash between delivery and the
//!   `.done` marker write replays the message on next boot.
//! - **Steer loss window**: an accepted steer can still be dropped by the
//!   foundation when the target turn is cancelled or the engine evicted
//!   (`chat:steer_dropped`); the messaging path treats steer-Ok as final and
//!   does not yet correlate that event. Sender identity is model-supplied and
//!   unauthenticated — the execpolicy approval prompt is the authorization
//!   boundary (contract §5 L1).
//!
//! The delivered text carries a machine-readable sender header block (the
//! session-mention block pattern mirrored on receive); `features/chat`
//! renders it as a sender card and all three auto-title paths strip it.
//! Dependency direction: `messaging → assistant` only (acyclic; assistant
//! never imports messaging).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::sessions::validators::{is_aux_session_id, is_sched_session_id};

/// Full model-visible tool name — single-sourced in
/// `assistant::platform::bridge` (which turns it into the execpolicy Ask
/// rule); re-exported here for the audit records. Dependency direction:
/// messaging -> assistant only.
pub use crate::features::assistant::platform::bridge::MESSAGING_SEND_TOOL as MESSAGING_TOOL_FULL_NAME;

/// Same bound as the MCP server's MAX_MESSAGE_TEXT_CHARS — re-checked here
/// because the spool directory is user-writable.
const MAX_MESSAGE_TEXT_CHARS: usize = 32 * 1024;
/// Same bound as the MCP server's MAX_SESSION_ID_LEN.
const MAX_SESSION_ID_LEN: usize = 128;
/// Sender/target title cap (mirrors the receiver parser's 200-char title cap;
/// an oversized title would overflow the 64 KB strip-line bound and render
/// raw).
const MAX_TITLE_CHARS: usize = 200;
/// Spool file size cap: a legitimate record is bounded by the 32k text cap
/// plus small metadata; anything bigger is hostile and is quarantined before
/// being read into memory.
const MAX_SPOOL_FILE_BYTES: u64 = 64 * 1024;
/// Transient delivery failures retry with backoff; quarantine only after this
/// many attempts (each attempt waits `[MIN_BACKOFF, MAX_BACKOFF]` since the
/// previous one, so worst case is minutes, not the poll interval).
const MAX_DELIVERY_ATTEMPTS: u32 = 10;
const MIN_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Upper bound for one delivery attempt: a wedged engine's steer channel can
/// park `reserve_owned().await` forever, which must not stall the single
/// global watcher (other sessions' messages queue behind it).
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);
/// Watch poll interval: inter-session messages are rare; latency budget is
/// conversational, not real-time.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// `.done` markers and quarantined files older than this are pruned at
/// watcher start (best-effort bounds on unbounded directories).
const RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);

/// One spooled message. Field names mirror server.py's `_spool_payload`
/// exactly (snake_case JSON on disk); unknown fields are skipped on read
/// (contract §4.4 drift defense). The `id` field is informational only —
/// the watcher keys its state on the directory-listed file name, never on
/// this field (a user-writable spool must not control watcher paths).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SpooledMessage {
    pub schema_version: u32,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub from_session: Option<String>,
    #[serde(default)]
    pub from_title: Option<String>,
    pub to_session: String,
    #[serde(default)]
    pub to_title: Option<String>,
    pub text: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// Header line of the delivered block. Mirrored verbatim by
/// `features/chat/session-message-block.js` — change both together.
pub const MESSAGE_BLOCK_HEADER: &str = "## Message from another session";

/// Build the delivered user-turn text: sender header block + body. The block
/// layout mirrors the session-mention contract: header line, one JSON line,
/// blank line, then the body (which may itself be multi-line).
pub fn build_session_message_block(
    from_session: Option<&str>,
    from_title: Option<&str>,
    body: &str,
) -> String {
    let sender = serde_json::json!({
        "sessionId": from_session,
        "title": from_title,
    });
    format!("{}\n{}\n\n{}", MESSAGE_BLOCK_HEADER, sender, body)
}

/// Charset + length + isolation validation for a spool participant (same
/// rules as the MCP server; the Rust side re-checks because the spool
/// directory is user-writable). Isolated prefixes are rejected as sender or
/// target: sched- sessions are subsystem-owned (Scheduled Tasks), aux-
/// sessions are cross-session isolated by design, eval_ is benchmark-private.
fn check_participant_id(session_id: Option<&String>, label: &str) -> Result<()> {
    let Some(id) = session_id else {
        return Ok(());
    };
    if id.is_empty()
        || id.len() > MAX_SESSION_ID_LEN
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("invalid {label} session id");
    }
    if is_sched_session_id(id)
        || is_aux_session_id(id)
        || id.to_ascii_lowercase().starts_with("eval_")
    {
        bail!("{label} session {id} is isolated and cannot take cross-session messages");
    }
    Ok(())
}

fn check_title(title: &Option<String>, label: &str) -> Result<()> {
    if let Some(title) = title {
        if title.chars().count() > MAX_TITLE_CHARS {
            bail!("{label} title exceeds {MAX_TITLE_CHARS} characters");
        }
    }
    Ok(())
}

impl SpooledMessage {
    /// Server-side re-validation of a spool record (contract §4.4: errors are
    /// explicit; §5: the L1 write re-checks everything it was told).
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            bail!("unsupported spool schema_version {}", self.schema_version);
        }
        if self.to_session.is_empty() {
            bail!("missing to_session");
        }
        check_participant_id(Some(&self.to_session), "target")?;
        check_participant_id(self.from_session.as_ref(), "sender")?;
        if let (Some(from), Some(to)) = (self.from_session.as_ref(), Some(&self.to_session)) {
            if from.eq_ignore_ascii_case(to) {
                bail!("sending to the session itself is not supported");
            }
        }
        check_title(&self.from_title, "sender")?;
        check_title(&self.to_title, "target")?;
        let text = self.text.trim();
        if text.is_empty() {
            bail!("message text is empty");
        }
        if text.chars().count() > MAX_MESSAGE_TEXT_CHARS {
            bail!(
                "message text exceeds the {} character limit",
                MAX_MESSAGE_TEXT_CHARS
            );
        }
        Ok(())
    }

    /// The exact text delivered into the target session (steer content /
    /// new-turn user message).
    pub fn delivered_text(&self) -> String {
        build_session_message_block(
            self.from_session.as_deref(),
            self.from_title.as_deref(),
            self.text.trim(),
        )
    }
}

fn spool_root() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("messaging")
        .join("spool")
}

fn failed_dir() -> PathBuf {
    spool_root().join("failed")
}

fn done_dir() -> PathBuf {
    spool_root().join(".done")
}

/// Delivery outcome (audit + log).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// Target was mid-turn: injected at the next step boundary.
    Steered,
    /// Target was idle or not loaded: a new turn was dispatched.
    Dispatched,
}

impl DeliveryOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            DeliveryOutcome::Steered => "steered",
            DeliveryOutcome::Dispatched => "dispatched",
        }
    }
}

/// Delivery port: the real impl drives the engine pool; tests inject a stub
/// (native async fn in trait, used only through generic static dispatch).
trait SpoolDelivery {
    async fn deliver(&self, message: &SpooledMessage) -> Result<DeliveryOutcome>;
}

struct PoolDelivery<'a>(&'a EnginePool);

impl SpoolDelivery for PoolDelivery<'_> {
    async fn deliver(&self, message: &SpooledMessage) -> Result<DeliveryOutcome> {
        deliver_spooled_message(self.0, message).await
    }
}

/// Deliver one validated message through the engine pool. `steer` errors are
/// the normal idle signal (no live engine / no active turn accepting), so the
/// fallback dispatches a fresh turn — the two paths together implement the
/// contract §6 queue/steer decision (busy → steer, idle → wake). Both paths
/// are bounded by [`DELIVERY_TIMEOUT`] so a wedged engine cannot stall the
/// watcher; a timeout surfaces as a transient error and is retried.
pub async fn deliver_spooled_message(
    pool: &EnginePool,
    message: &SpooledMessage,
) -> Result<DeliveryOutcome> {
    let text = message.delivered_text();
    let steered = tokio::time::timeout(
        DELIVERY_TIMEOUT,
        pool.steer(&message.to_session, text.clone()),
    )
    .await;
    match steered {
        Ok(Ok(_)) => Ok(DeliveryOutcome::Steered),
        Ok(Err(steer_error)) => {
            // The dispatch path re-checks deletion/lifecycle gates itself
            // (send_reserved_user_message loads the store under the turn
            // lock); a genuinely gone session fails loudly here.
            let dispatched = tokio::time::timeout(
                DELIVERY_TIMEOUT,
                pool.deliver_messaging_turn(&message.to_session, text),
            )
            .await;
            match dispatched {
                Ok(Ok(())) => Ok(DeliveryOutcome::Dispatched),
                Ok(Err(error)) => Err(error)
                    .with_context(|| format!("dispatch after steer failure ({steer_error:#})")),
                Err(_) => bail!("dispatch timed out after {DELIVERY_TIMEOUT:?} (engine wedged?)"),
            }
        }
        Err(_) => bail!("steer timed out after {DELIVERY_TIMEOUT:?} (engine wedged?)"),
    }
}

/// Append the L1 audit records (both sessions' workspaces; failures inside
/// `audit::append` only log, never panic).
fn audit_delivery(
    store: &crate::features::sessions::SessionStore,
    message: &SpooledMessage,
    outcome: DeliveryOutcome,
) {
    let detail = serde_json::json!({
        "tool": MESSAGING_TOOL_FULL_NAME,
        "to_session": message.to_session,
        "from_session": message.from_session,
        "outcome": outcome.as_str(),
        "chars": message.text.trim().chars().count(),
    });
    for sid in [
        message.from_session.as_deref(),
        Some(message.to_session.as_str()),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(roots) = store.session_roots(sid) {
            crate::features::assistant::audit::append(
                &roots.execution,
                "session_message",
                "app",
                detail.clone(),
            );
        }
    }
}

/// Terminal disposition of one spool file after processing. Parse/validate/
/// oversize problems are permanent (retrying cannot succeed); delivery
/// failures are treated as transient (rewind gates, engine spawn errors) and
/// retried with backoff.
enum Processed {
    /// Delivered (or already-delivered skip); the file can be removed.
    Done,
    /// Permanent rejection: quarantine now.
    Poison(anyhow::Error),
    /// Transient delivery failure: retry with backoff.
    Retry(anyhow::Error),
}

/// Process one spool file: read → validate → deliver. The spool identity is
/// the directory-listed file name (the server names idempotent retries by
/// their sender-scoped key hash) — the JSON `id` field is never trusted for
/// watcher paths.
async fn process_spool_file<D: SpoolDelivery>(
    path: &Path,
    delivery: &D,
    store: &crate::features::sessions::SessionStore,
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
    let message: SpooledMessage = match serde_json::from_slice(&bytes) {
        Ok(message) => message,
        Err(error) => return poisoned(anyhow::Error::new(error).context("parse spool file")),
    };
    if let Err(error) = message.validate() {
        return poisoned(error);
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let done_marker = done_dir().join(format!("{stem}.json"));
    if done_marker.exists() {
        return Processed::Done;
    }
    match delivery.deliver(&message).await {
        Ok(outcome) => {
            audit_delivery(store, &message, outcome);
            if message.idempotency_key.is_some() {
                let _ = std::fs::create_dir_all(done_dir());
                let _ = std::fs::write(&done_marker, b"");
            }
            Processed::Done
        }
        Err(error) => Processed::Retry(error),
    }
}

/// Retry bookkeeping per file: attempt count + last attempt time (for
/// backoff). Entries are removed on every terminal path.
#[derive(Default)]
struct RetryState {
    attempts: HashMap<String, (u32, Instant)>,
}

impl RetryState {
    /// Whether a transiently-failing file is due for another attempt
    /// (linear backoff from [`MIN_BACKOFF`], capped at [`MAX_BACKOFF`]).
    fn due(&self, name: &str) -> bool {
        match self.attempts.get(name) {
            Some((count, last)) => {
                let backoff = (*count as u64 * MIN_BACKOFF.as_secs())
                    .clamp(MIN_BACKOFF.as_secs(), MAX_BACKOFF.as_secs());
                last.elapsed().as_secs() >= backoff
            }
            None => true,
        }
    }

    fn record(&mut self, name: &str) -> u32 {
        let entry = self
            .attempts
            .entry(name.to_string())
            .or_insert((0, Instant::now()));
        entry.0 += 1;
        entry.1 = Instant::now();
        entry.0
    }

    fn clear(&mut self, name: &str) {
        self.attempts.remove(name);
    }
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
            "[messaging] quarantine rename failed for {:?}: {error}",
            path
        );
    }
}

/// Watch loop body: process every pending spool file (oldest first — file
/// names are sortable hex), quarantining poison files immediately and
/// transient failures after [`MAX_DELIVERY_ATTEMPTS`] spread-out attempts.
async fn process_pending_spool<D: SpoolDelivery>(
    delivery: &D,
    store: &crate::features::sessions::SessionStore,
    retries: &mut RetryState,
) {
    let root = spool_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return; // no spool directory yet = nothing was ever sent
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
        if !retries.due(&name) {
            continue;
        }
        match process_spool_file(&path, delivery, store).await {
            Processed::Done => {
                retries.clear(&name);
                let _ = std::fs::remove_file(&path);
            }
            Processed::Poison(error) => {
                retries.clear(&name);
                log::warn!("[messaging] quarantining {name}: {error:#}");
                quarantine(&path);
            }
            Processed::Retry(error) => {
                let count = retries.record(&name);
                if count >= MAX_DELIVERY_ATTEMPTS {
                    log::warn!(
                        "[messaging] quarantining {name} after {count} delivery attempts: {error:#}"
                    );
                    retries.clear(&name);
                    quarantine(&path);
                } else {
                    log::warn!(
                        "[messaging] delivery attempt {count} for {name} failed (will retry): {error:#}"
                    );
                }
            }
        }
    }
}

/// Best-effort prune of `.done` markers and quarantined files older than
/// [`RETENTION`] (called once at watcher start).
fn prune_stale_state() {
    for dir in [done_dir(), failed_dir()] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            let age = meta
                .modified()
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .unwrap_or(Duration::ZERO);
            if age > RETENTION {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// Spawn the delivery watcher: processes the boot backlog first, then polls
/// until the process exits (the app lifetime is the watcher lifetime —
/// started once from `lib.rs` setup). Uses `tauri::async_runtime::spawn`
/// (not `tokio::spawn`): the setup hook runs outside any raw tokio context.
pub fn spawn_delivery_watcher(
    pool: EnginePool,
    store: crate::features::sessions::SessionStore,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        prune_stale_state();
        let mut retries = RetryState::default();
        loop {
            process_pending_spool(&PoolDelivery(&pool), &store, &mut retries).await;
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
}

#[cfg(test)]
mod spool_pipeline_tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

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
                "pinvou3-messaging-pipeline-{}-{}",
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

    fn record_json(id: &str, key: Option<&str>) -> String {
        format!(
            r#"{{"schema_version":1,"id":"{id}","from_session":"src0001","from_title":"源","to_session":"tgt0001","to_title":"目标","text":"正文","created_at":"2026-09-28T00:00:00Z","idempotency_key":{}}}"#,
            key.map(|k| format!(r#""{k}""#))
                .unwrap_or_else(|| "null".into())
        )
    }

    struct FakeDelivery {
        outcome: Result<DeliveryOutcome, String>,
        calls: Rc<RefCell<usize>>,
    }

    impl SpoolDelivery for FakeDelivery {
        async fn deliver(&self, _message: &SpooledMessage) -> Result<DeliveryOutcome> {
            *self.calls.borrow_mut() += 1;
            match &self.outcome {
                Ok(outcome) => Ok(*outcome),
                Err(error) => bail!("{error}"),
            }
        }
    }

    fn sessions_store() -> crate::features::sessions::SessionStore {
        crate::features::sessions::SessionStore::boot_for_process_startup().expect("store")
    }

    #[tokio::test]
    async fn delivers_removes_and_writes_done_marker() {
        let _home = TempHome::new();
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("abc.json"), record_json("abc", Some("k1"))).unwrap();
        let fake = FakeDelivery {
            outcome: Ok(DeliveryOutcome::Steered),
            calls: Rc::new(RefCell::new(0)),
        };
        let mut retries = RetryState::default();
        let sessions = sessions_store();
        process_pending_spool(&fake, &sessions, &mut retries).await;
        assert!(
            !spool.join("abc.json").exists(),
            "delivered file is removed"
        );
        assert_eq!(*fake.calls.borrow(), 1);
        assert!(done_dir().join("abc.json").exists(), "done marker written");

        // Same idempotency identity returns under a new poll: the marker wins
        // and the file is dropped WITHOUT a second delivery.
        std::fs::write(spool.join("abc.json"), record_json("abc", Some("k1"))).unwrap();
        process_pending_spool(&fake, &sessions, &mut retries).await;
        assert_eq!(*fake.calls.borrow(), 1, "done marker suppresses redelivery");
        assert!(!spool.join("abc.json").exists());
    }

    #[tokio::test]
    async fn poison_file_is_quarantined_immediately() {
        let _home = TempHome::new();
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("bad.json"), b"not json").unwrap();
        let fake = FakeDelivery {
            outcome: Ok(DeliveryOutcome::Steered),
            calls: Rc::new(RefCell::new(0)),
        };
        let mut retries = RetryState::default();
        process_pending_spool(&fake, &sessions_store(), &mut retries).await;
        assert_eq!(*fake.calls.borrow(), 0, "poison never reaches delivery");
        assert!(failed_dir().join("bad.json").exists());
        assert!(!spool.join("bad.json").exists());
    }

    #[tokio::test]
    async fn oversize_file_is_quarantined_before_read() {
        let _home = TempHome::new();
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("big.json"), vec![b'x'; 65 * 1024]).unwrap();
        let fake = FakeDelivery {
            outcome: Ok(DeliveryOutcome::Steered),
            calls: Rc::new(RefCell::new(0)),
        };
        let mut retries = RetryState::default();
        process_pending_spool(&fake, &sessions_store(), &mut retries).await;
        assert_eq!(*fake.calls.borrow(), 0);
        assert!(failed_dir().join("big.json").exists());
    }

    #[tokio::test]
    async fn transient_failures_retry_then_quarantine_after_max_attempts() {
        let _home = TempHome::new();
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("slow.json"), record_json("slow", None)).unwrap();
        let fake = FakeDelivery {
            outcome: Err("rewind gate".into()),
            calls: Rc::new(RefCell::new(0)),
        };
        let mut retries = RetryState::default();
        let sessions = sessions_store();
        // Pre-seed attempts so the backoff window has elapsed: this poll is
        // the 10th (MAX_DELIVERY_ATTEMPTS) and must quarantine.
        retries.attempts.insert(
            "slow.json".into(),
            (
                MAX_DELIVERY_ATTEMPTS - 1,
                Instant::now() - Duration::from_secs(60),
            ),
        );
        process_pending_spool(&fake, &sessions, &mut retries).await;
        assert_eq!(*fake.calls.borrow(), 1);
        assert!(
            failed_dir().join("slow.json").exists(),
            "quarantined after max attempts"
        );
        assert!(!spool.join("slow.json").exists());
        assert!(retries.attempts.is_empty(), "terminal path clears state");
    }

    #[tokio::test]
    async fn backoff_skips_recent_failures() {
        let _home = TempHome::new();
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(spool.join("r.json"), record_json("r", None)).unwrap();
        let fake = FakeDelivery {
            outcome: Err("transient".into()),
            calls: Rc::new(RefCell::new(0)),
        };
        let mut retries = RetryState::default();
        retries
            .attempts
            .insert("r.json".into(), (3, Instant::now()));
        process_pending_spool(&fake, &sessions_store(), &mut retries).await;
        assert_eq!(
            *fake.calls.borrow(),
            0,
            "backoff must skip the recent failure"
        );
        assert!(
            spool.join("r.json").exists(),
            "file stays for the next poll"
        );
    }

    #[tokio::test]
    async fn id_field_is_never_trusted_for_watcher_paths() {
        let _home = TempHome::new();
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        // Hostile id: traversal claims must not escape the .done namespace
        // nor probe arbitrary files — the marker is keyed by the file stem.
        let record = record_json("../../../../../../tmp/pinvou-evil", Some("k2"));
        std::fs::write(spool.join("hostile.json"), record).unwrap();
        let fake = FakeDelivery {
            outcome: Ok(DeliveryOutcome::Dispatched),
            calls: Rc::new(RefCell::new(0)),
        };
        let mut retries = RetryState::default();
        process_pending_spool(&fake, &sessions_store(), &mut retries).await;
        assert_eq!(*fake.calls.borrow(), 1, "the message itself still delivers");
        assert!(
            done_dir().join("hostile.json").exists(),
            "marker keyed by file stem"
        );
        assert!(
            !Path::new("/tmp/pinvou-evil.json").exists(),
            "no file may be created outside the done namespace"
        );
    }
}
