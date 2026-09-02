//! Best-effort startup timeline shared by the Rust backend and WebView frontend.
//!
//! Windows release builds do not have a console, so startup diagnostics must be
//! persisted.  Runs are appended to one bounded file and contain no credentials
//! or user content.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Deserialize;

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
static STARTED_AT: OnceLock<Instant> = OnceLock::new();
static LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static RUN_ID: OnceLock<String> = OnceLock::new();

struct RotationInfo {
    previous_bytes: u64,
    rotated: bool,
    error: Option<String>,
}

fn elapsed() -> Duration {
    STARTED_AT.get_or_init(Instant::now).elapsed()
}

fn clean_field(value: &str) -> String {
    crate::platform::credential_store::redact_secret(value)
        .chars()
        .map(|c| {
            if matches!(c, '\r' | '\n' | '\t') {
                ' '
            } else {
                c
            }
        })
        .take(500)
        .collect()
}

fn write_line(source: &str, stage: &str, elapsed_ms: f64, detail: Option<&str>) {
    let source = clean_field(source);
    let stage = clean_field(stage);
    let detail = detail.map(clean_field).unwrap_or_default();
    let line = format!(
        "{} +{:>10.3}ms [run={}] [{}] {}{}\n",
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        elapsed_ms,
        RUN_ID.get().map(String::as_str).unwrap_or("unknown"),
        source,
        stage,
        if detail.is_empty() {
            String::new()
        } else {
            format!(" | {detail}")
        }
    );
    eprint!("[startup] {line}");
    if let Some(lock) = LOG_FILE.get() {
        if let Ok(mut file) = lock.lock() {
            if let Some(file) = file.as_mut() {
                let _ = file.write_all(line.as_bytes());
                let _ = file.flush();
            }
        }
    }
}

fn rotate_if_oversized(path: &Path) -> RotationInfo {
    let previous_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if previous_bytes <= MAX_LOG_BYTES {
        return RotationInfo {
            previous_bytes,
            rotated: false,
            error: None,
        };
    }
    match std::fs::remove_file(path) {
        Ok(()) => RotationInfo {
            previous_bytes,
            rotated: true,
            error: None,
        },
        Err(error) => RotationInfo {
            previous_bytes,
            rotated: false,
            error: Some(error.to_string()),
        },
    }
}

pub fn init() {
    STARTED_AT.get_or_init(Instant::now);
    let started_utc = Utc::now();
    let _ = RUN_ID.set(format!(
        "{}-{}",
        started_utc.format("%Y%m%dT%H%M%S%.3fZ"),
        std::process::id()
    ));
    let path = crate::platform::paths::pinvou3_home()
        .join("logs")
        .join("startup.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let rotation = rotate_if_oversized(&path);
    let file = path.parent().and_then(|parent| {
        std::fs::create_dir_all(parent).ok()?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
    });
    let _ = LOG_FILE.set(Mutex::new(file));
    mark_with_detail(
        "rust",
        "process:start",
        &format!(
            "pid={} previous_log_bytes={} log_rotated={} rotation_error={}",
            std::process::id(),
            rotation.previous_bytes,
            rotation.rotated,
            rotation.error.as_deref().unwrap_or("none"),
        ),
    );
    mark_with_detail(
        "rust",
        "log:ready",
        &format!(
            "path={} max_bytes={MAX_LOG_BYTES} mode=append",
            path.display()
        ),
    );
}

pub fn mark(stage: &str) {
    write_line("rust", stage, elapsed().as_secs_f64() * 1000.0, None);
}

pub fn mark_with_detail(source: &str, stage: &str, detail: &str) {
    write_line(
        source,
        stage,
        elapsed().as_secs_f64() * 1000.0,
        Some(detail),
    );
}

#[derive(Debug, Deserialize)]
pub struct FrontendStartupEntry {
    stage: String,
    #[serde(default)]
    detail: String,
    since_navigation_ms: f64,
}

/// Receive batched WebView performance marks.  The frontend supplies offsets
/// from `performance.timeOrigin`; keeping that clock separate from the Rust
/// process clock makes WebView/Babel stalls immediately visible.
pub fn report_frontend_startup(entries: Vec<FrontendStartupEntry>) {
    for entry in entries {
        write_line(
            "webview",
            &entry.stage,
            entry.since_navigation_ms.max(0.0),
            (!entry.detail.is_empty()).then_some(entry.detail.as_str()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_LOG_BYTES, clean_field, rotate_if_oversized};

    #[test]
    fn startup_log_fields_are_single_line_and_bounded() {
        let dirty = format!("a\nb\tc\r{}", "x".repeat(600));
        let clean = clean_field(&dirty);
        assert!(!clean.contains(['\n', '\r', '\t']));
        assert_eq!(clean.chars().count(), 500);
    }

    #[test]
    fn startup_log_limit_is_exactly_two_mib() {
        assert_eq!(MAX_LOG_BYTES, 2_097_152);
    }

    #[test]
    fn oversized_startup_log_is_deleted_before_open() {
        let root =
            std::env::temp_dir().join(format!("pinvou3-startup-rotate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let log = root.join("startup.log");
        let file = std::fs::File::create(&log).unwrap();
        file.set_len(MAX_LOG_BYTES + 1).unwrap();

        let result = rotate_if_oversized(&log);
        assert_eq!(result.previous_bytes, MAX_LOG_BYTES + 1);
        assert!(result.rotated);
        assert!(!log.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
