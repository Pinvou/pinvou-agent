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
const MAX_STRING_CHARS: usize = 2_048;
const MAX_ARRAY_ITEMS: usize = 64;
const MAX_DEPTH: usize = 8;
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
    let entry = json!({
        "schema_version": SCHEMA_VERSION,
        "recorded_at": now(),
        "process_run_id": process_run_id(),
        "pid": std::process::id(),
        "source": "rust",
        "event": clean_identifier(event),
        "details": sanitize_backend_details(details, 0),
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
                    "command_rejected",
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

fn sanitize_value(value: Value, depth: usize) -> Value {
    if depth >= MAX_DEPTH {
        return Value::String("[DEPTH_LIMIT]".into());
    }
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .take(MAX_ARRAY_ITEMS)
                .map(|(key, value)| {
                    let sanitized = if is_sensitive_key(&key) {
                        Value::String("[REDACTED]".into())
                    } else {
                        sanitize_value(value, depth + 1)
                    };
                    (clean_key(&key), sanitized)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .take(MAX_ARRAY_ITEMS)
                .map(|value| sanitize_value(value, depth + 1))
                .collect(),
        ),
        Value::String(value) => Value::String(clean_string(&value)),
        other => other,
    }
}

fn sanitize_backend_details(value: Value, depth: usize) -> Value {
    if depth >= MAX_DEPTH {
        return Value::String("[DEPTH_LIMIT]".into());
    }
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .take(MAX_ARRAY_ITEMS)
                .map(|(key, value)| {
                    let sanitized = if is_sensitive_key(&key) {
                        Value::String("[REDACTED]".into())
                    } else if is_backend_identifier_key(&key) {
                        value
                            .as_str()
                            .and_then(identifier_fingerprint)
                            .map(Value::String)
                            .unwrap_or_else(|| Value::String("[INVALID_IDENTIFIER]".into()))
                    } else {
                        sanitize_backend_details(value, depth + 1)
                    };
                    (clean_key(&key), sanitized)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .take(MAX_ARRAY_ITEMS)
                .map(|value| sanitize_backend_details(value, depth + 1))
                .collect(),
        ),
        Value::String(value) => Value::String(clean_string(&value)),
        other => other,
    }
}

fn is_backend_identifier_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().replace('-', "_").as_str(),
        "session_id"
            | "active_session_id"
            | "download_id"
            | "trace_id"
            | "event_id"
            | "client_run_id"
            | "endpoint_id"
    )
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace('-', "_");
    matches!(
        normalized.as_str(),
        "access_token"
            | "api_key"
            | "authorization"
            | "body"
            | "content"
            | "cookie"
            | "credential"
            | "credentials"
            | "cwd"
            | "directory"
            | "error"
            | "file_path"
            | "id_token"
            | "local_path"
            | "message"
            | "output"
            | "password"
            | "path"
            | "prompt"
            | "refresh_token"
            | "request"
            | "response"
            | "secret"
            | "text"
            | "token"
            | "user_input"
    ) || [
        "_access_token",
        "_api_key",
        "_authorization",
        "_body",
        "_content",
        "_cookie",
        "_credential",
        "_credentials",
        "_directory",
        "_error",
        "_file_path",
        "_id_token",
        "_local_path",
        "_message",
        "_output",
        "_password",
        "_path",
        "_prompt",
        "_refresh_token",
        "_request",
        "_response",
        "_secret",
        "_user_input",
    ]
    .iter()
    .any(|suffix| normalized.ends_with(suffix))
}

fn clean_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':' | '.')
        })
        .take(160)
        .collect()
}

fn clean_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':')
        })
        .take(120)
        .collect()
}

fn clean_string(value: &str) -> String {
    let cookie_safe = redact_cookie_header_lines(value);
    let normalized: String = cookie_safe
        .chars()
        .map(|character| {
            if matches!(character, '\r' | '\n' | '\t') {
                ' '
            } else {
                character
            }
        })
        .take(MAX_STRING_CHARS)
        .collect();
    redact_inline_secrets(&normalized)
}

fn redact_cookie_header_lines(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            let position = lower.find("set-cookie:").or_else(|| lower.find("cookie:"));
            match position {
                Some(position) => format!("{}Cookie: [REDACTED]", &line[..position]),
                None => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_inline_secrets(value: &str) -> String {
    let mut output = Vec::new();
    let mut redact_next = false;
    for part in value.split_whitespace() {
        if redact_next {
            output.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }
        if part.eq_ignore_ascii_case("bearer") || part.eq_ignore_ascii_case("basic") {
            output.push(part.to_string());
            redact_next = true;
            continue;
        }
        let lower = part.to_ascii_lowercase();
        if lower == "cookie:" || lower == "set-cookie:" {
            output.push("Cookie:".to_string());
            redact_next = true;
        } else if [
            "access_token=",
            "api_key=",
            "authorization=",
            "cookie=",
            "token=",
            "secret=",
            "password=",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
        {
            let separator = part.find('=').unwrap_or(part.len());
            output.push(format!("{}=[REDACTED]", &part[..separator]));
        } else if looks_like_local_path(part) {
            output.push("[LOCAL_PATH]".to_string());
        } else {
            output.push(part.to_string());
        }
    }
    output.join(" ")
}

fn looks_like_local_path(value: &str) -> bool {
    let value = value.trim_matches(|character: char| {
        matches!(character, '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';')
    });
    let bytes = value.as_bytes();
    let lower = value.to_ascii_lowercase();
    lower.starts_with("file://")
        || lower.starts_with("\\\\")
        || lower.starts_with("~/")
        || lower.starts_with("~\\")
        || [
            "/home/",
            "/users/",
            "/tmp/",
            "/private/var/",
            "/var/folders/",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'))
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::{
        append_entries_to_path, clean_string, identifier_fingerprint, is_sensitive_key,
        normalize_frontend_entry, sanitize_backend_details, sanitize_value, MAX_LOG_BYTES,
    };
    use serde_json::json;
    use std::io::Write as _;

    #[test]
    fn frontend_diagnostics_redact_user_content_and_credentials() {
        let sanitized = sanitize_value(
            json!({
                "session_id": "chat-safe",
                "message_count": 12,
                "message": "private user text",
                "access_token": "private-token",
                "details": { "content": "private model output", "revision": "sha-safe" },
            }),
            0,
        );
        assert_eq!(sanitized["session_id"], "chat-safe");
        assert_eq!(sanitized["message_count"], 12);
        assert_eq!(sanitized["message"], "[REDACTED]");
        assert_eq!(sanitized["access_token"], "[REDACTED]");
        assert_eq!(sanitized["details"]["content"], "[REDACTED]");
        assert_eq!(sanitized["details"]["revision"], "sha-safe");
    }

    #[test]
    fn sensitive_key_matching_does_not_hide_safe_counts() {
        assert!(is_sensitive_key("password"));
        assert!(is_sensitive_key("relay_access_token"));
        assert!(is_sensitive_key("x-api-key"));
        assert!(is_sensitive_key("provider_error"));
        assert!(is_sensitive_key("working_directory"));
        assert!(!is_sensitive_key("message_count"));
        assert!(!is_sensitive_key("input_token_count"));
        assert!(!is_sensitive_key("error_category"));
    }

    #[test]
    fn free_form_errors_redact_inline_credentials() {
        let cleaned = clean_string(
            "relay failed Basic cHJpdmF0ZQ== Bearer private-token Cookie: sid=private\n\
             https://x.test/?access_token=private api_key=private C:\\Users\\alice\\secret.txt \
             /home/alice/private.json",
        );
        assert!(!cleaned.contains("cHJpdmF0ZQ"));
        assert!(!cleaned.contains("private-token"));
        assert!(!cleaned.contains("sid=private"));
        assert!(!cleaned.contains("access_token=private"));
        assert!(!cleaned.contains("api_key=private"));
        assert!(!cleaned.contains("alice"));
        assert!(cleaned.contains("Basic [REDACTED]"));
        assert!(cleaned.contains("Bearer [REDACTED]"));
        assert!(cleaned.contains("access_token=[REDACTED]"));
        assert!(cleaned.contains("[LOCAL_PATH]"));
    }

    #[test]
    fn arbitrary_errors_prompts_responses_and_paths_are_fully_redacted() {
        let sanitized = sanitize_value(
            json!({
                "error": "private provider response",
                "cancel_error": "private cancellation response",
                "api_key": "sk-private",
                "refresh_token": "refresh-private",
                "id_token": "id-private",
                "credential": "credential-private",
                "user_input": "private input",
                "request_prompt": "private prompt",
                "response_body": "private response",
                "local_path": "C:\\Users\\alice\\session.json",
                "error_category": "snapshot_load_failed",
                "error_present": true,
            }),
            0,
        );
        for key in [
            "error",
            "cancel_error",
            "api_key",
            "refresh_token",
            "id_token",
            "credential",
            "user_input",
            "request_prompt",
            "response_body",
            "local_path",
        ] {
            assert_eq!(sanitized[key], "[REDACTED]", "{key} must be redacted");
        }
        assert_eq!(sanitized["error_category"], "snapshot_load_failed");
        assert_eq!(sanitized["error_present"], true);
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
        let backend = sanitize_backend_details(
            json!({
                "session_id": "chat-join-safe",
                "active_session_id": "chat-active-safe",
                "download_id": "download_web_join_safe",
                "trace_id": "trace-join-safe",
                "event_id": "event-join-safe",
                "client_run_id": "client-run-safe",
                "endpoint_id": "endpoint-join-safe",
                "transcript_revision": "a".repeat(64),
                "message_count": 4,
                "terminal_status": "Completed",
                "nested": { "session_id": "chat-join-safe" },
            }),
            0,
        );
        let frontend = normalize_frontend_entry(
            json!({
                "event": "reconcile_started",
                "connection": { "endpoint_id": "endpoint-join-safe" },
                "details": {
                    "session_id": "chat-join-safe",
                    "active_session_id": "chat-active-safe",
                    "download_id": "download_web_join_safe",
                    "trace_id": "trace-join-safe",
                },
            }),
            "2026-08-18T12:00:00.000Z",
        )
        .unwrap();

        for key in ["session_id", "active_session_id", "download_id", "trace_id"] {
            assert_eq!(backend[key], frontend["details"][key], "join key {key}");
        }
        assert_eq!(
            backend["endpoint_id"],
            frontend["connection"]["endpoint_id"]
        );
        for key in ["event_id", "client_run_id"] {
            let raw = if key == "event_id" {
                "event-join-safe"
            } else {
                "client-run-safe"
            };
            assert_eq!(backend[key], identifier_fingerprint(raw).unwrap());
        }
        assert_eq!(backend["nested"]["session_id"], backend["session_id"]);
        assert_eq!(backend["transcript_revision"], "a".repeat(64));
        assert_eq!(backend["message_count"], 4);
        assert_eq!(backend["terminal_status"], "Completed");
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
