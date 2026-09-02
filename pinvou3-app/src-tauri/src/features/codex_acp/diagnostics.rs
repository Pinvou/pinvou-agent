//! Persistent, credential-safe diagnostics for Codex ACP setup and login.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use chrono::Utc;

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_FIELD_CHARS: usize = 2_000;

pub(super) fn log_path() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("logs")
        .join("codex-acp.log")
}

pub(super) fn operation_id(kind: &str) -> String {
    format!(
        "{}-{}-{}",
        clean_field(kind),
        std::process::id(),
        Utc::now().timestamp_millis()
    )
}

pub(super) fn write(operation_id: &str, stage: &str, detail: impl AsRef<str>) {
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    rotate_if_oversized(&path);

    let line = format!(
        "{} [operation={}] [{}] {}\n",
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        clean_identifier(operation_id),
        clean_identifier(stage),
        clean_field(detail.as_ref())
    );
    eprint!("[codex-acp] {line}");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

fn rotate_if_oversized(path: &std::path::Path) {
    if std::fs::metadata(path)
        .map(|metadata| metadata.len() > MAX_LOG_BYTES)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(path);
    }
}

fn clean_field(value: &str) -> String {
    redact_bearer_tokens(value)
        .chars()
        .map(|character| {
            if matches!(character, '\r' | '\n' | '\t') {
                ' '
            } else {
                character
            }
        })
        .take(MAX_FIELD_CHARS)
        .collect()
}

fn clean_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':')
        })
        .take(160)
        .collect()
}

fn redact_bearer_tokens(value: &str) -> String {
    let mut output = Vec::new();
    let mut redact_next = false;
    for part in value.split_whitespace() {
        if redact_next {
            output.push("[REDACTED]");
            redact_next = false;
            continue;
        }
        output.push(part);
        if part
            .trim_matches(|character: char| matches!(character, '"' | '\'' | ',' | ';'))
            .eq_ignore_ascii_case("bearer")
        {
            redact_next = true;
        }
    }
    output.join(" ")
}

#[cfg(test)]
mod tests {
    use super::clean_field;

    #[test]
    fn diagnostic_fields_are_single_line_and_bounded() {
        let cleaned = clean_field(&format!("first\r\nsecond {}", "x".repeat(3_000)));
        assert!(!cleaned.contains('\n'));
        assert!(!cleaned.contains('\r'));
        assert!(cleaned.chars().count() <= 2_000);
    }

    #[test]
    fn diagnostic_fields_redact_bearer_tokens() {
        assert_eq!(clean_field("Bearer secret-value"), "Bearer [REDACTED]");
    }
}
