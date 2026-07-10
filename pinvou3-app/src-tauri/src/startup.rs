//! Best-effort startup timeline shared by the Rust backend and WebView frontend.
//!
//! Windows release builds do not have a console, so startup diagnostics must be
//! persisted.  The file is truncated on every real process start and contains
//! no credentials or user content.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Deserialize;

static STARTED_AT: OnceLock<Instant> = OnceLock::new();
static LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();

fn elapsed() -> Duration {
    STARTED_AT.get_or_init(Instant::now).elapsed()
}

fn clean_field(value: &str) -> String {
    crate::credential_store::redact_secret(value)
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
        "{} +{:>10.3}ms [{}] {}{}\n",
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        elapsed_ms,
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

pub fn init() {
    STARTED_AT.get_or_init(Instant::now);
    let path = crate::bridge::paths::pinvou3_home()
        .join("logs")
        .join("startup.log");
    let file = path.parent().and_then(|parent| {
        std::fs::create_dir_all(parent).ok()?;
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .ok()
    });
    let _ = LOG_FILE.set(Mutex::new(file));
    mark_with_detail(
        "rust",
        "process:start",
        &format!("pid={}", std::process::id()),
    );
    mark_with_detail("rust", "log:ready", &format!("path={}", path.display()));
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
#[tauri::command]
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
    use super::clean_field;

    #[test]
    fn startup_log_fields_are_single_line_and_bounded() {
        let dirty = format!("a\nb\tc\r{}", "x".repeat(600));
        let clean = clean_field(&dirty);
        assert!(!clean.contains(['\n', '\r', '\t']));
        assert_eq!(clean.chars().count(), 500);
    }
}
