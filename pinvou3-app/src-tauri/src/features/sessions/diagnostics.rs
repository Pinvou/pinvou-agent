//! Concentrated, privacy-safe diagnostics for cross-client transcript authority.
//!
//! Every layer involved in the `chat:transcript_committed` → `chat:done` →
//! `load_session` reconciliation writes to one bounded JSONL file. The records
//! deliberately contain identifiers, revisions, counts, timings, and state
//! transitions, but never conversation text, model output, credentials, or
//! attachment contents.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

const SCHEMA_VERSION: u8 = 1;
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;
const MAX_BATCH_ENTRIES: usize = 64;
const MAX_ENTRY_BYTES: usize = 24 * 1024;
const FRONTEND_EVENTS: &[&str] = &[
    "authority_sync_notice_shown",
    "browser_network_offline",
    "browser_network_online",
    "chat_done_classified",
    "connection_state_changed",
    "diagnostics_initialized",
    "document_visibility_changed",
    "local_send_blocked_by_remote_sync",
    "local_turn_admission_failed",
    "local_turn_admitted",
    "local_turn_claimed",
    "reconcile_attempt_failed",
    "reconcile_attempt_rejected",
    "reconcile_deferred_busy",
    "reconcile_exhausted",
    "reconcile_joined_inflight",
    "reconcile_started",
    "reconcile_succeeded",
    "remote_sync_blocked_action",
    "remote_turn_marked",
    "session_download_capability_wait_failed",
    "session_download_cleanup",
    "transcript_committed_event_received",
];

fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn process_run_id() -> &'static str {
    static RUN_ID: OnceLock<String> = OnceLock::new();
    RUN_ID
        .get_or_init(|| {
            format!(
                "{}-{}",
                Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
                std::process::id()
            )
        })
        .as_str()
}

pub(crate) fn log_path() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("logs")
        .join("authority-sync.jsonl")
}

pub(crate) fn record_backend(event: &str, details: Value) {
    let Some(details) = normalize_backend_details(event, &details) else {
        // 事件名是编译期常量、须命中白名单才落盘，此处带名打印便于定位拼写漂移。
        eprintln!("[authority-sync] rejected unknown or malformed backend diagnostic: {event}");
        return;
    };
    let entry = json!({
        "schema_version": SCHEMA_VERSION,
        "recorded_at": now(),
        "process_run_id": process_run_id(),
        "pid": std::process::id(),
        "source": "rust",
        "event": event,
        "details": details,
    });
    if let Err(error) = append_entries(&[entry]) {
        eprintln!("[authority-sync] unable to append backend diagnostic: {error}");
    }
}

pub(crate) fn record_frontend_batch(entries: Vec<Value>) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    if entries.len() > MAX_BATCH_ENTRIES {
        return Err(format!(
            "authority-sync diagnostic batch exceeds {MAX_BATCH_ENTRIES} entries"
        ));
    }
    let received_at = now();
    let normalized = entries
        .into_iter()
        .filter_map(|entry| normalize_frontend_entry(entry, &received_at))
        .collect::<Vec<_>>();
    if normalized.is_empty() {
        return Ok(());
    }
    append_entries(&normalized)
}

fn normalize_frontend_entry(entry: Value, received_at: &str) -> Option<Value> {
    let object = entry.as_object()?;
    let event = object.get("event")?.as_str()?;
    if !FRONTEND_EVENTS.contains(&event) {
        return None;
    }
    Some(json!({
        "schema_version": SCHEMA_VERSION,
        "recorded_at": received_at,
        "record_id": next_frontend_record_id(),
        "process_run_id": process_run_id(),
        "pid": std::process::id(),
        "source": "frontend",
        "event": event,
        "connection": normalize_frontend_connection(object.get("connection")),
        "details": normalize_frontend_details(object.get("details")),
    }))
}

fn next_frontend_record_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-frontend-{}",
        process_run_id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1
    )
}

fn normalize_frontend_connection(value: Option<&Value>) -> Value {
    let Some(object) = value.and_then(Value::as_object) else {
        return Value::Object(Map::new());
    };
    let mut output = Map::new();
    for key in ["browser_online", "desktop_online"] {
        if let Some(value) = object.get(key).and_then(Value::as_bool) {
            output.insert(key.to_string(), Value::Bool(value));
        }
    }
    for (key, allowed) in [
        ("platform_kind", &["desktop", "unknown", "web"][..]),
        (
            "visibility",
            &["hidden", "prerender", "unknown", "visible"][..],
        ),
        (
            "connection_status",
            &[
                "connected",
                "connecting",
                "credentials_missing",
                "denied",
                "desktop_offline",
                "error",
                "idle",
                "incompatible_desktop",
                "local",
                "replaced",
                "revoked",
                "unknown",
            ][..],
        ),
    ] {
        if let Some(value) = allowed_enum(object.get(key), allowed) {
            output.insert(key.to_string(), Value::String(value));
        }
    }
    if let Some(value) = object
        .get("endpoint_id")
        .and_then(Value::as_str)
        .and_then(identifier_fingerprint)
    {
        output.insert("endpoint_id".into(), Value::String(value));
    }
    Value::Object(output)
}

fn normalize_frontend_details(value: Option<&Value>) -> Value {
    let Some(object) = value.and_then(Value::as_object) else {
        return Value::Object(Map::new());
    };
    let mut output = Map::new();
    for (key, value) in object {
        let normalized = match key.as_str() {
            "transport" => Some(normalize_frontend_details(Some(value))),
            "session_id" | "active_session_id" | "trace_id" | "download_id" => value
                .as_str()
                .and_then(identifier_fingerprint)
                .map(Value::String),
            "session_revision"
            | "committed_revision"
            | "expected_committed_revision"
            | "saved_revision"
            | "event_revision" => value
                .as_str()
                .and_then(normalize_revision)
                .map(Value::String),
            "message_count"
            | "chat_item_count"
            | "queued_count"
            | "expected_assistant_key_length"
            | "baseline_message_count"
            | "minimum_terminal_message_count"
            | "attempt"
            | "attempts"
            | "elapsed_ms"
            | "saved_message_count"
            | "chunk_count"
            | "bytes_received"
            | "declared_total_bytes"
            | "cleanup_requested_count"
            | "cleanup_failed_count"
            | "cleanup_succeeded_count"
            | "restored_queue_count" => normalize_nonnegative_number(value),
            "buffer_present"
            | "local_turn_owned"
            | "remote_turn_active"
            | "remote_terminal_seen"
            | "loaded_from_disk"
            | "buffer_busy"
            | "ui_busy"
            | "baseline_trusted"
            | "preserve_committed_revision"
            | "snapshot_present"
            | "completed_local_turn"
            | "requires_authority_reconcile"
            | "terminal_error_present"
            | "terminal_seen_before_event"
            | "concurrent_turn"
            | "error_present"
            | "cancellable_lease"
            | "cancel_requested"
            | "cancel_succeeded"
            | "desktop_online" => value.as_bool().map(Value::Bool),
            "saved_roles" => normalize_saved_roles(value),
            "cause" => value
                .as_str()
                .filter(|value| allowed_cause(value))
                .map(|value| Value::String(value.to_string())),
            "reason" => allowed_enum(
                Some(value),
                &[
                    "assistant_identity_missing",
                    "invalid_snapshot",
                    "load_session_error",
                    "message_count_short",
                    "revision_mismatch",
                ],
            )
            .map(Value::String),
            "error_category" => allowed_enum(
                Some(value),
                &[
                    "cancel_rpc_failed",
                    "capability_snapshot_timeout",
                    "capability_snapshot_unavailable",
                    "command_rejected",
                    "download_id_mismatch",
                    "session_turn_in_progress",
                    "snapshot_load_failed",
                ],
            )
            .map(Value::String),
            "operation" => allowed_enum(
                Some(value),
                &["accept_plan", "edit_last_turn", "send", "send_to_session"],
            )
            .map(Value::String),
            "notice" => allowed_enum(
                Some(value),
                &["desktop_done_sync_pending", "remote_done_unsynced"],
            )
            .map(Value::String),
            "terminal_status" => allowed_enum(
                Some(value),
                &[
                    "Cancelled",
                    "Canceled",
                    "Completed",
                    "Failed",
                    "Interrupted",
                    "cancelled",
                    "canceled",
                    "completed",
                    "failed",
                    "interrupted",
                ],
            )
            .map(Value::String),
            "transport_kind" => {
                allowed_enum(Some(value), &["desktop_invoke", "web_chunked_rpc"]).map(Value::String)
            }
            "status" => allowed_enum(
                Some(value),
                &[
                    "connected",
                    "connecting",
                    "desktop_offline",
                    "error",
                    "idle",
                    "local",
                    "unknown",
                ],
            )
            .map(Value::String),
            "visibility" => {
                allowed_enum(Some(value), &["hidden", "prerender", "unknown", "visible"])
                    .map(Value::String)
            }
            _ => None,
        };
        if let Some(normalized) = normalized {
            output.insert(key.clone(), normalized);
        }
    }
    Value::Object(output)
}

fn normalize_nonnegative_number(value: &Value) -> Option<Value> {
    if value.is_null() {
        return Some(Value::Null);
    }
    value
        .as_u64()
        .filter(|value| *value <= 1_000_000_000_000_000)
        .map(Value::from)
}

fn normalize_saved_roles(value: &Value) -> Option<Value> {
    let values = value.as_array()?;
    if values.len() > 12 {
        return None;
    }
    let mut roles = Vec::with_capacity(values.len());
    for value in values {
        let role = allowed_enum(
            Some(value),
            &["assistant", "invalid", "system", "tool", "user"],
        )?;
        roles.push(Value::String(role));
    }
    Some(Value::Array(roles))
}

fn allowed_enum(value: Option<&Value>, allowed: &[&str]) -> Option<String> {
    let value = value?.as_str()?;
    allowed.contains(&value).then(|| value.to_string())
}

fn allowed_cause(value: &str) -> bool {
    [
        "accept_plan_concurrent_turn",
        "chat_done_without_local_owner",
        "edit_last_turn_concurrent_turn",
        "local_send_concurrent_turn",
        "remote_user_message_event",
    ]
    .contains(&value)
        || value.strip_prefix("event:").is_some_and(|event| {
            [
                "chat:delta",
                "chat:reasoning_delta",
                "chat:reasoning_done",
                "chat:reasoning_start",
                "chat:tool_end",
                "chat:tool_start",
                "chat:transient_error",
                "chat:turn_started",
                "chat:user_input_required",
                "chat:user_message",
            ]
            .contains(&event)
        })
}

fn normalize_revision(value: &str) -> Option<String> {
    if value.is_empty() {
        return Some(String::new());
    }
    (value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

fn identifier_fingerprint(value: &str) -> Option<String> {
    if value.is_empty() {
        return Some(String::new());
    }
    if value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return None;
    }
    let digest = Sha256::digest(value.as_bytes());
    Some(format!(
        "id:{}",
        crate::platform::encoding::hex_lower(&digest[..12])
    ))
}

fn append_entries(entries: &[Value]) -> Result<(), String> {
    let _guard = write_lock()
        .lock()
        .map_err(|_| "authority-sync diagnostic lock is poisoned".to_string())?;
    let path = log_path();
    append_entries_to_path(&path, entries)
}

fn append_entries_to_path(path: &Path, entries: &[Value]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create diagnostics directory: {error}"))?;
    }
    for entry in entries {
        let mut encoded = serde_json::to_vec(entry)
            .map_err(|error| format!("serialize authority-sync diagnostic: {error}"))?;
        if encoded.len() > MAX_ENTRY_BYTES {
            encoded = serde_json::to_vec(&json!({
                "schema_version": SCHEMA_VERSION,
                "recorded_at": now(),
                "process_run_id": process_run_id(),
                "pid": std::process::id(),
                "source": "rust",
                "event": "diagnostic_entry_dropped",
                "details": {
                    "reason": "entry_too_large",
                    "encoded_bytes": encoded.len(),
                    "limit_bytes": MAX_ENTRY_BYTES,
                },
            }))
            .expect("bounded diagnostic fallback must serialize");
        }
        encoded.push(b'\n');
        rotate_before_write(path, encoded.len() as u64)?;
        let mut file = crate::platform::filesystem::open_private_append_file(path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        file.write_all(&encoded)
            .map_err(|error| format!("append {}: {error}", path.display()))?;
        file.flush()
            .map_err(|error| format!("flush {}: {error}", path.display()))?;
    }
    Ok(())
}

fn rotate_before_write(path: &Path, upcoming_bytes: u64) -> Result<(), String> {
    let size = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect {}: {error}", path.display())),
    };
    if size.saturating_add(upcoming_bytes) <= MAX_LOG_BYTES {
        return Ok(());
    }
    let previous = path.with_extension("jsonl.1");
    match std::fs::remove_file(&previous) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("remove {}: {error}", previous.display())),
    }
    std::fs::rename(path, &previous).map_err(|error| {
        format!(
            "rotate {} to {}: {error}",
            path.display(),
            previous.display()
        )
    })
}

fn normalize_backend_details(event: &str, value: &Value) -> Option<Value> {
    let allowed_fields: &[&str] = match event {
        "chat_done_emitting" => &[
            "session_id",
            "terminal_status",
            "terminal_error_present",
            "shell_cleanup_failed",
        ],
        "transcript_committed_emitting" => &[
            "session_id",
            "transcript_revision",
            "persistence_origin",
            "message_count",
        ],
        "web_session_download_cancelled" => &["session_id", "download_id", "total_bytes", "ready"],
        "web_session_chunk_served" => &[
            "session_id",
            "download_id",
            "offset",
            "chunk_bytes",
            "total_bytes",
            "eof",
            "transcript_revision",
            "elapsed_ms",
        ],
        "desktop_load_session_failed" => &[
            "session_id",
            "set_active",
            "elapsed_ms",
            "error_category",
            "error_present",
        ],
        "desktop_load_session_revision_failed" => &[
            "session_id",
            "message_count",
            "elapsed_ms",
            "error_category",
            "error_present",
        ],
        "desktop_load_session_succeeded" => &[
            "session_id",
            "set_active",
            "transcript_revision",
            "message_count",
            "elapsed_ms",
        ],
        _ => return None,
    };
    let object = value.as_object()?;
    let mut output = Map::new();
    for key in allowed_fields {
        let Some(value) = object.get(*key) else {
            continue;
        };
        let normalized = match *key {
            "session_id" | "download_id" => value
                .as_str()
                .and_then(identifier_fingerprint)
                .map(Value::String),
            "transcript_revision" => {
                if value.is_null() {
                    Some(Value::Null)
                } else {
                    value
                        .as_str()
                        .and_then(normalize_revision)
                        .map(Value::String)
                }
            }
            "message_count" | "total_bytes" | "offset" | "chunk_bytes" | "elapsed_ms" => {
                normalize_nonnegative_number(value)
            }
            "ready"
            | "eof"
            | "set_active"
            | "terminal_error_present"
            | "shell_cleanup_failed"
            | "error_present" => value.as_bool().map(Value::Bool),
            "terminal_status" => allowed_enum(
                Some(value),
                &[
                    "Cancelled",
                    "Canceled",
                    "Completed",
                    "Failed",
                    "Interrupted",
                ],
            )
            .map(Value::String),
            "persistence_origin" => allowed_enum(
                Some(value),
                &[
                    "engine_state_update",
                    "reclaimed_fallback",
                    "session_store",
                    "terminal_fallback",
                    "unknown",
                ],
            )
            .map(Value::String),
            "error_category" => allowed_enum(
                Some(value),
                &["revision_compute_failed", "session_load_failed"],
            )
            .map(Value::String),
            _ => None,
        };
        if let Some(normalized) = normalized {
            output.insert((*key).to_string(), normalized);
        }
    }
    Some(Value::Object(output))
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::{
        append_entries_to_path, identifier_fingerprint, normalize_backend_details,
        normalize_frontend_entry, MAX_LOG_BYTES,
    };
    use serde_json::json;
    use std::io::Write as _;

    #[test]
    fn backend_diagnostics_use_event_specific_allowlists() {
        let normalized = normalize_backend_details(
            "desktop_load_session_failed",
            &json!({
                "session_id": "chat-safe",
                "set_active": true,
                "elapsed_ms": 12,
                "error_category": "session_load_failed",
                "error_present": true,
                "relay_token": "private-token",
                "provider_text": "private provider response",
                "nested": { "prompt": "private prompt" },
            }),
        )
        .unwrap();
        assert_ne!(normalized["session_id"], "chat-safe");
        assert_eq!(normalized["set_active"], true);
        assert_eq!(normalized["elapsed_ms"], 12);
        assert_eq!(normalized["error_category"], "session_load_failed");
        assert_eq!(normalized["error_present"], true);
        for key in ["relay_token", "provider_text", "nested"] {
            assert!(normalized.get(key).is_none(), "{key} must be dropped");
        }
        assert!(normalize_backend_details("future_backend_event", &json!({})).is_none());
    }

    #[test]
    fn frontend_batch_normalization_rebuilds_the_server_envelope() {
        let normalized = normalize_frontend_entry(
            json!({
                "schema_version": 99,
                "occurred_at": "forged-time",
                "recorded_at": "forged-server-time",
                "record_id": "forged-id",
                "source": "rust",
                "event": "reconcile_attempt_failed",
                "event_id": "forged-event-id",
                "connection": {
                    "platform_kind": "web",
                    "desktop_online": true,
                    "endpoint_id": "endpoint-safe",
                    "refresh_token": "refresh-private",
                    "message": "private connection message",
                },
                "details": {
                    "session_id": "chat-safe",
                    "expected_committed_revision": "a".repeat(64),
                    "attempt": 2,
                    "error_category": "snapshot_load_failed",
                    "error_present": true,
                    "transport": {
                        "transport_kind": "web_chunked_rpc",
                        "chunk_count": 3,
                        "response_body": "private response",
                    },
                    "refresh_token": "refresh-private",
                    "id_token": "id-private",
                    "credential": "credential-private",
                    "user_input": "private prompt",
                    "unknown": "private unknown field",
                },
            }),
            "2026-08-18T12:00:00.000Z",
        )
        .unwrap();

        assert_eq!(normalized["schema_version"], 1);
        assert_eq!(normalized["recorded_at"], "2026-08-18T12:00:00.000Z");
        assert_eq!(normalized["source"], "frontend");
        assert_eq!(normalized["event"], "reconcile_attempt_failed");
        assert_ne!(normalized["record_id"], "forged-id");
        assert!(normalized.get("occurred_at").is_none());
        assert!(normalized.get("event_id").is_none());
        assert_eq!(normalized["connection"]["platform_kind"], "web");
        assert_ne!(normalized["connection"]["endpoint_id"], "endpoint-safe");
        assert!(normalized["connection"].get("refresh_token").is_none());
        assert!(normalized["connection"].get("message").is_none());
        assert_ne!(normalized["details"]["session_id"], "chat-safe");
        assert_eq!(
            normalized["details"]["expected_committed_revision"],
            "a".repeat(64)
        );
        assert_eq!(
            normalized["details"]["error_category"],
            "snapshot_load_failed"
        );
        assert_eq!(normalized["details"]["transport"]["chunk_count"], 3);
        for key in [
            "refresh_token",
            "id_token",
            "credential",
            "user_input",
            "unknown",
        ] {
            assert!(
                normalized["details"].get(key).is_none(),
                "{key} must be dropped"
            );
        }
        assert!(normalized["details"]["transport"]
            .get("response_body")
            .is_none());
    }

    #[test]
    fn frontend_batch_rejects_unknown_events_and_forged_categories() {
        assert!(normalize_frontend_entry(
            json!({ "event": "rust_backend_event", "details": {} }),
            "2026-08-18T12:00:00.000Z"
        )
        .is_none());
        let normalized = normalize_frontend_entry(
            json!({
                "event": "local_turn_admission_failed",
                "details": {
                    "error_category": "provider said private prompt",
                    "operation": "arbitrary operation",
                    "error_present": true,
                },
            }),
            "2026-08-18T12:00:00.000Z",
        )
        .unwrap();
        assert!(normalized["details"].get("error_category").is_none());
        assert!(normalized["details"].get("operation").is_none());
        assert_eq!(normalized["details"]["error_present"], true);
    }

    #[test]
    fn backend_identifiers_use_the_same_join_keys_as_frontend_entries() {
        let backend = normalize_backend_details(
            "web_session_chunk_served",
            &json!({
                "session_id": "chat-join-safe",
                "download_id": "download_web_join_safe",
                "transcript_revision": "a".repeat(64),
                "offset": 0,
                "chunk_bytes": 4,
                "total_bytes": 4,
                "eof": true,
                "elapsed_ms": 1,
            }),
        )
        .unwrap();
        let frontend = normalize_frontend_entry(
            json!({
                "event": "reconcile_started",
                "connection": { "endpoint_id": "endpoint-join-safe" },
                "details": {
                    "session_id": "chat-join-safe",
                    "download_id": "download_web_join_safe",
                },
            }),
            "2026-08-18T12:00:00.000Z",
        )
        .unwrap();

        for key in ["session_id", "download_id"] {
            assert_eq!(backend[key], frontend["details"][key], "join key {key}");
        }
        assert_eq!(
            backend["session_id"],
            identifier_fingerprint("chat-join-safe").unwrap()
        );
        assert_eq!(backend["transcript_revision"], "a".repeat(64));
        assert_eq!(backend["total_bytes"], 4);
        assert_eq!(backend["eof"], true);
        assert_ne!(backend["session_id"], "chat-join-safe");
    }

    #[test]
    fn rotation_accounts_for_the_next_entry_before_appending() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-authority-sync-rotation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("authority-sync.jsonl");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&vec![b'x'; (MAX_LOG_BYTES - 4) as usize])
            .unwrap();
        drop(file);

        append_entries_to_path(&path, &[json!({"event": "boundary"})]).unwrap();

        let previous = path.with_extension("jsonl.1");
        assert_eq!(
            std::fs::metadata(previous).unwrap().len(),
            MAX_LOG_BYTES - 4
        );
        assert!(std::fs::metadata(&path).unwrap().len() <= MAX_LOG_BYTES);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
