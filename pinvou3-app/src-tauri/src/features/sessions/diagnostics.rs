//! Concentrated, privacy-safe diagnostics for cross-client transcript authority.
//!
//! Every layer involved in the `chat:transcript_committed` → `chat:done` →
//! `load_session` reconciliation writes to one bounded JSONL file. The records
//! deliberately contain identifiers, revisions, counts, timings, and state
//! transitions, but never conversation text, model output, credentials, or
//! attachment contents.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Map, Value};

const SCHEMA_VERSION: u8 = 1;
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;
const MAX_BATCH_ENTRIES: usize = 64;
const MAX_ENTRY_BYTES: usize = 24 * 1024;
const MAX_STRING_CHARS: usize = 2_048;
const MAX_ARRAY_ITEMS: usize = 64;
const MAX_DEPTH: usize = 8;

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
        "details": sanitize_value(details, 0),
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
        .map(|entry| {
            let mut object = match sanitize_value(entry, 0) {
                Value::Object(object) => object,
                _ => Map::new(),
            };
            object.insert("schema_version".into(), Value::from(SCHEMA_VERSION));
            object.insert("received_at".into(), Value::String(received_at.clone()));
            object.insert(
                "process_run_id".into(),
                Value::String(process_run_id().to_string()),
            );
            object.insert("pid".into(), Value::from(std::process::id()));
            Value::Object(object)
        })
        .collect::<Vec<_>>();
    append_entries(&normalized)
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
    rotate_if_oversized(path)?;

    let mut file = crate::platform::filesystem::open_private_append_file(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
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
        file.write_all(&encoded)
            .map_err(|error| format!("append {}: {error}", path.display()))?;
    }
    file.flush()
        .map_err(|error| format!("flush {}: {error}", path.display()))
}

fn rotate_if_oversized(path: &Path) -> Result<(), String> {
    let size = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect {}: {error}", path.display())),
    };
    if size <= MAX_LOG_BYTES {
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

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "access_token"
            | "authorization"
            | "body"
            | "content"
            | "cookie"
            | "message"
            | "output"
            | "password"
            | "prompt"
            | "secret"
            | "text"
            | "token"
    ) || normalized.ends_with("_secret")
        || normalized.ends_with("_password")
        || normalized.ends_with("_access_token")
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
    let normalized: String = value
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

fn redact_inline_secrets(value: &str) -> String {
    let mut output = Vec::new();
    let mut redact_next = false;
    for part in value.split_whitespace() {
        if redact_next {
            output.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }
        if part.eq_ignore_ascii_case("bearer") {
            output.push(part.to_string());
            redact_next = true;
            continue;
        }
        let lower = part.to_ascii_lowercase();
        if ["access_token=", "token=", "secret=", "password="]
            .iter()
            .any(|marker| lower.contains(marker))
        {
            let separator = part.find('=').unwrap_or(part.len());
            output.push(format!("{}=[REDACTED]", &part[..separator]));
        } else {
            output.push(part.to_string());
        }
    }
    output.join(" ")
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::{clean_string, is_sensitive_key, sanitize_value};
    use serde_json::json;

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
        assert!(!is_sensitive_key("message_count"));
        assert!(!is_sensitive_key("input_token_count"));
    }

    #[test]
    fn free_form_errors_redact_inline_credentials() {
        let cleaned = clean_string(
            "relay failed Authorization: Bearer private-token https://x.test/?access_token=private",
        );
        assert!(!cleaned.contains("private-token"));
        assert!(!cleaned.contains("access_token=private"));
        assert!(cleaned.contains("Bearer [REDACTED]"));
        assert!(cleaned.contains("access_token=[REDACTED]"));
    }
}
