//! Cross-session messaging delivery (docs/builtin-toolset-contract.md §5 L1 / §6).
//!
//! The session-reader MCP server validates a `send_message_to_session` call and
//! spools it to `<pinvou3 home>/messaging/spool/<name>.json` (see server.py's
//! `send_message_to_session` — the spool record schema is the contract between
//! the two sides). This module is the app-side consumer:
//!
//! - a poll watcher (same shape as the scheduled retention loop) picks spool
//!   files up, re-validates them (server-side defense in depth — the spool
//!   directory is user-writable), and delivers through the engine pool;
//! - delivery semantics: target mid-turn → [`EnginePool::steer`] into the
//!   current turn; target idle / engine not live → a new turn dispatched
//!   immediately (the scheduled-task wake precedent);
//! - every delivery writes an audit record into both sessions' workspaces
//!   (`assistant::audit`, contract §5 L1 requirement);
//! - a delivered idempotency key leaves a marker under `spool/.done/`, so a
//!   retried tool call (same key → same spool file name on the server side)
//!   cannot deliver twice across watcher restarts;
//! - poison files (schema drift, hostile content) are quarantined under
//!   `spool/failed/` instead of blocking the queue.
//!
//! The delivered text carries a machine-readable sender header block (the
//! session-mention block pattern mirrored on receive); `features/chat`
//! renders it as a sender card and all three auto-title paths strip it.
//! Dependency direction: `messaging → assistant` only (acyclic; assistant
//! never imports messaging).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
/// Delivery attempts per spool file before quarantine (`spool/failed/`).
const MAX_DELIVERY_ATTEMPTS: u32 = 3;
/// Watch poll interval: inter-session messages are rare; latency budget is
/// conversational, not real-time.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// One spooled message. Field names mirror server.py's `_spool_payload`
/// exactly (snake_case JSON on disk); unknown fields are skipped on read
/// (contract §4.4 drift defense).
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

/// Deliver one validated message through the engine pool. `steer` errors are
/// the normal idle signal (no live engine / no active turn accepting), so the
/// fallback dispatches a fresh turn — the two paths together implement the
/// contract §6 queue/steer decision (busy → steer, idle → wake).
pub async fn deliver_spooled_message(
    pool: &EnginePool,
    message: &SpooledMessage,
) -> Result<DeliveryOutcome> {
    let text = message.delivered_text();
    match pool.steer(&message.to_session, text.clone()).await {
        Ok(_) => Ok(DeliveryOutcome::Steered),
        Err(steer_error) => {
            // The dispatch path re-checks deletion/lifecycle gates itself
            // (send_reserved_user_message loads the store under the turn
            // lock); a genuinely gone session fails loudly here.
            pool.deliver_messaging_turn(&message.to_session, text)
                .await
                .with_context(|| format!("dispatch after steer failure ({steer_error:#})"))?;
            Ok(DeliveryOutcome::Dispatched)
        }
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

/// Process one spool file: read → validate → deliver → remove. Returns
/// `Ok(Some(outcome))` on success, `Ok(None)` when the file was skipped
/// (already-delivered idempotency marker), `Err` when the file must be
/// retried or quarantined by the caller.
async fn process_spool_file(
    path: &Path,
    pool: &EnginePool,
    store: &crate::features::sessions::SessionStore,
) -> Result<Option<DeliveryOutcome>> {
    let bytes = std::fs::read(path).context("read spool file")?;
    let message: SpooledMessage = serde_json::from_slice(&bytes).context("parse spool file")?;
    message.validate().context("validate spooled message")?;
    let spool_id = if message.id.is_empty() {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string()
    } else {
        message.id.clone()
    };
    let done_marker = done_dir().join(format!("{}.json", spool_id));
    if done_marker.exists() {
        return Ok(None);
    }
    let outcome = deliver_spooled_message(pool, &message).await?;
    audit_delivery(store, &message, outcome);
    if message.idempotency_key.is_some() {
        let _ = std::fs::create_dir_all(done_dir());
        let _ = std::fs::write(&done_marker, b"");
    }
    Ok(Some(outcome))
}

/// Watch loop body: process every pending spool file (oldest first — file
/// names are sortable hex), quarantine poison files, retry transient
/// failures up to [`MAX_DELIVERY_ATTEMPTS`].
async fn process_pending_spool(
    pool: &EnginePool,
    store: &crate::features::sessions::SessionStore,
    attempts: &mut HashMap<String, u32>,
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
        match process_spool_file(&path, pool, store).await {
            Ok(Some(outcome)) => {
                attempts.remove(&name);
                log::info!("[messaging] delivered {} ({outcome:?})", name);
                let _ = std::fs::remove_file(&path);
            }
            Ok(None) => {
                attempts.remove(&name);
                let _ = std::fs::remove_file(&path);
            }
            Err(error) => {
                let count = attempts.entry(name.clone()).or_insert(0);
                *count += 1;
                if *count >= MAX_DELIVERY_ATTEMPTS {
                    log::warn!("[messaging] quarantining {name} after {count} attempts: {error:#}");
                    let _ = std::fs::create_dir_all(failed_dir());
                    let _ = std::fs::rename(&path, failed_dir().join(&name));
                    attempts.remove(&name);
                } else {
                    log::warn!("[messaging] delivery attempt {count} for {name} failed: {error:#}");
                }
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
        let mut attempts: HashMap<String, u32> = HashMap::new();
        loop {
            process_pending_spool(&pool, &store, &mut attempts).await;
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SpooledMessage {
        serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "id": "abc123",
            "from_session": "src0001",
            "from_title": "源会话",
            "to_session": "tgt0001",
            "to_title": "目标会话",
            "text": "请帮我确认上次的结论",
            "created_at": "2026-09-28T00:00:00Z",
            "idempotency_key": "k1",
        }))
        .unwrap()
    }

    #[test]
    fn valid_message_passes_and_builds_block() {
        let message = sample();
        message.validate().unwrap();
        let text = message.delivered_text();
        assert!(text.starts_with(MESSAGE_BLOCK_HEADER));
        assert!(text.contains("{\"sessionId\":\"src0001\",\"title\":\"源会话\"}"));
        assert!(text.ends_with("请帮我确认上次的结论"));
        // Blank separator line between JSON line and body.
        assert!(text.contains("}\n\n请帮我确认上次的结论"));
    }

    #[test]
    fn schema_and_content_validation_rejects_drift() {
        let mut message = sample();
        message.schema_version = 2;
        assert!(message.validate().is_err());
        let mut message = sample();
        message.text = "   ".to_string();
        assert!(message.validate().is_err());
        let mut message = sample();
        message.text = "x".repeat(MAX_MESSAGE_TEXT_CHARS + 1);
        assert!(message.validate().is_err());
        let mut message = sample();
        message.to_session = String::new();
        assert!(message.validate().is_err());
    }

    #[test]
    fn isolated_and_self_targets_are_rejected() {
        for target in [
            "sched-run1",
            "SCHED-run1",
            "aux-side1",
            "AUX-x",
            "eval_bench1",
            "Eval_1",
        ] {
            let mut message = sample();
            message.to_session = target.to_string();
            assert!(message.validate().is_err(), "{target} must be rejected");
        }
        let mut message = sample();
        message.from_session = Some("TGT0001".to_string());
        assert!(
            message.validate().is_err(),
            "self-send must be rejected (case-insensitive)"
        );
    }

    #[test]
    fn unattributed_message_is_valid() {
        let mut message = sample();
        message.from_session = None;
        message.from_title = None;
        message.validate().unwrap();
        let text = message.delivered_text();
        assert!(text.contains("{\"sessionId\":null,\"title\":null}"));
    }

    #[test]
    fn participant_charset_is_enforced() {
        let mut message = sample();
        message.to_session = "../escape".to_string();
        assert!(message.validate().is_err());
        let mut message = sample();
        message.from_session = Some("x".repeat(MAX_SESSION_ID_LEN + 1).into());
        assert!(message.validate().is_err());
    }
}
