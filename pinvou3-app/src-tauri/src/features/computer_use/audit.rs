//! Audit log: append-only JSONL, one record per tool call (pure append, never rewritten in
//! place).
//!
//! This is a PLAIN informational local log — no HMAC, no keyring, no salt,
//! no key files, no verify step (nobody ships crypto in a local audit trail;
//! mainstream products write plain logs). Privacy is enforced by REDACTION
//! at the call site: tool.rs never writes typed text (Type logs "typed N
//! characters"; typing-form key chords log "pressed 1 key"), targets are
//! parameter summaries only, and screenshots log SHA-256 + path.
//!
//! Records land in `<pinvou3 data dir>/computer-use/` via the platform
//! private-file base: 0700 directory + 0600 file (O_APPEND create, no umask
//! exposure window), one `write_all` per line (line atomicity comes from
//! O_APPEND plus that single write), and `sync_data` per record so a crash
//! cannot LOSE an already-written record — fsync does not make the line
//! indivisible: a partial write mid-line (e.g. ENOSPC) can still tear the
//! last JSONL line. Append failures are fail-open: the log is
//! informational, so the caller warns and continues — an audit write error
//! never blocks an action.
//!
//! Known limitation: one file per session grows unboundedly over a long
//! session; rotation/retention cleanup is not implemented yet (policy TBD).

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::Digest as _;

use crate::platform::encoding::hex_lower;
use crate::platform::paths;
use crate::platform::strings::truncate_utf8;

/// Byte cap for the target / element-label fields (the unified platform audit field
/// convention).
pub const AUDIT_TARGET_MAX_BYTES: usize = 600;
/// Byte cap for the free-form `error` field. Every current source is
/// fixed copy or a stable code, but the cap keeps the redaction contract
/// intact if a future error source starts embedding variable data (same
/// bound as the target field).
pub const AUDIT_ERROR_MAX_BYTES: usize = 600;

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(&sha2::Sha256::digest(bytes))
}

/// `<pinvou3 data dir>/computer-use/`.
pub fn audit_dir() -> PathBuf {
    paths::pinvou3_home().join("computer-use")
}

fn sanitize_session_id(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Length-cap so a pathological session id cannot push the audit filename
    // past filesystem limits and silently fail every append (fail-open, but
    // the trail would be gone).
    const MAX_SESSION_ID_CHARS: usize = 64;
    if sanitized.is_empty() {
        "unknown".to_string()
    } else if sanitized.chars().count() > MAX_SESSION_ID_CHARS {
        sanitized.chars().take(MAX_SESSION_ID_CHARS).collect()
    } else {
        sanitized
    }
}

/// `audit-<sanitized>-<hash8>.jsonl`. Sanitization is lossy (`a/b` and `a_b`
/// both become `a_b`), so a hash of the raw session id (first 8 bytes of the
/// SHA-256, hex) is part of the name to keep colliding ids in distinct files.
pub(crate) fn audit_file_name(session_id: &str) -> String {
    let hash = &sha256_hex(session_id.as_bytes())[..16];
    format!("audit-{}-{hash}.jsonl", sanitize_session_id(session_id))
}

/// One audit record: a purely informational snapshot of one tool call.
#[derive(Debug, Clone, Serialize)]
pub struct AuditRecord {
    pub timestamp: String,
    pub session_id: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// The consent tier/result actually applied, e.g. "observe" / "input:session-grant" /
    /// "input:session-grant+t3-confirmed" / "rejected:grant-required".
    pub consent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_path: Option<String>,
}

impl AuditRecord {
    pub fn new(
        session_id: impl Into<String>,
        action: impl Into<String>,
        consent: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: chrono::Utc::now().to_rfc3339(),
            session_id: session_id.into(),
            action: action.into(),
            target: None,
            consent: consent.into(),
            result: None,
            error: None,
            duration_ms: None,
            screenshot_sha256: None,
            screenshot_path: None,
        }
    }

    /// Record the target (coordinate summary or element label), truncated per the audit
    /// convention. The caller is responsible for redaction: typed text never enters this
    /// field (only lengths are recorded, see the module docs).
    pub fn with_target(&mut self, target: &str) -> &mut Self {
        self.target = Some(truncate_utf8(target, AUDIT_TARGET_MAX_BYTES).to_string());
        self
    }

    /// Screenshots record only the SHA-256 + file path, never the pixels.
    pub fn with_screenshot(&mut self, png: &[u8], path: &Path) -> &mut Self {
        self.screenshot_sha256 = Some(sha256_hex(png));
        self.screenshot_path = Some(path.to_string_lossy().into_owned());
        self
    }

    pub fn finish(mut self, result: &str, error: Option<String>, duration_ms: u64) -> Self {
        self.result = Some(result.to_string());
        self.error = error.map(|error| truncate_utf8(&error, AUDIT_ERROR_MAX_BYTES).to_string());
        self.duration_ms = Some(duration_ms);
        self
    }
}

/// Per-session audit log (append-only).
pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    /// `<pinvou3 data dir>/computer-use/audit-<session_id>-<hash8>.jsonl`
    /// (see [`audit_file_name`]), with the directory created under private permissions.
    pub fn for_session(session_id: &str) -> io::Result<Self> {
        let dir = audit_dir();
        // Private-permission directory helper: create/verify 0700 (Windows-equivalent ACL).
        crate::platform::filesystem::open_private_file_directory(&dir)?;
        Ok(Self {
            path: dir.join(audit_file_name(session_id)),
        })
    }

    /// Test-only: an arbitrary path (no private directory created).
    #[cfg(test)]
    pub(crate) fn at_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Test-only path accessor (production code never needs the raw path).
    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Append one record. Serialization cannot fail (pure string/number fields); IO failures
    /// propagate up (the caller is fail-open: eprintln then continue).
    pub fn append(&self, record: &AuditRecord) -> io::Result<()> {
        let mut line = serde_json::to_string(record)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        line.push('\n');
        // Audit records (even when sanitized) fall under private-data governance: go through
        // the platform layer's private append-file helper (0600 creation on unix, no umask
        // exposure window).
        let mut file = crate::platform::filesystem::open_private_append_file(&self.path)?;
        file.write_all(line.as_bytes())?;
        // fsync each record: a crash must not LOSE an already-written
        // record. Line atomicity comes from O_APPEND plus the single
        // write_all above, not from fsync — a partial write mid-line
        // (e.g. ENOSPC) can still tear the last JSONL line. One fsync per
        // record is acceptable at audit's per-tool-call frequency.
        file.sync_data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_log() -> (PathBuf, AuditLog) {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-cu-audit-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        fs::create_dir_all(&dir).map(|_| ()).unwrap_or(());
        let path = dir.join("audit-test.jsonl");
        (dir, AuditLog::at_path(path))
    }

    #[test]
    fn record_round_trips_through_jsonl_with_all_fields() {
        let (dir, log) = temp_log();
        let mut record = AuditRecord::new("s1", "left_click", "input:session-grant");
        record.with_target("left click x1 at Some((5, 6))");
        let record = record.finish("ok", None, 42);
        log.append(&record).unwrap_or(());

        let raw = fs::read_to_string(log.path()).unwrap_or_default();
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 1, "one record per call: {raw}");
        let parsed: serde_json::Value =
            serde_json::from_str(lines[0]).unwrap_or(serde_json::Value::Null);
        assert_eq!(parsed["session_id"], "s1");
        assert_eq!(parsed["action"], "left_click");
        assert_eq!(parsed["target"], "left click x1 at Some((5, 6))");
        assert_eq!(parsed["consent"], "input:session-grant");
        assert_eq!(parsed["result"], "ok");
        assert_eq!(parsed["duration_ms"], 42);
        assert!(parsed["timestamp"].is_string());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Redaction is the privacy contract: the log stores exactly what the
    /// caller passes — here redacted length-only targets — and no crypto
    /// fields exist at all.
    #[test]
    fn redacted_targets_round_trip_and_no_crypto_fields_exist() {
        let (dir, log) = temp_log();
        let mut typed = AuditRecord::new("s1", "type", "input:session-grant+t3-confirmed");
        typed.with_target("typed 13 characters");
        let typed = typed.finish("ok", None, 7);
        let mut chord = AuditRecord::new("s1", "key", "input:session-grant");
        chord.with_target("pressed 1 key");
        let chord = chord.finish("ok", None, 3);
        log.append(&typed).unwrap_or(());
        log.append(&chord).unwrap_or(());

        let raw = fs::read_to_string(log.path()).unwrap_or_default();
        assert!(raw.contains("\"typed 13 characters\""), "{raw}");
        assert!(raw.contains("\"pressed 1 key\""), "{raw}");
        for absent in ["text_hmac", "salt", "hmac", "keyring"] {
            assert!(!raw.to_lowercase().contains(absent), "{absent} in {raw}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn error_and_screenshot_fields_serialize() {
        let (dir, log) = temp_log();
        let png = b"\x89PNG-fake-bytes";
        let mut record = AuditRecord::new("s1", "screenshot", "observe");
        record.with_screenshot(png, Path::new("/tmp/shot.png"));
        let record = record.finish("error", Some("t3-confirmation-required".to_string()), 9);
        log.append(&record).unwrap_or(());

        let raw = fs::read_to_string(log.path()).unwrap_or_default();
        assert!(raw.contains(&sha256_hex(png)), "{raw}");
        assert!(raw.contains("/tmp/shot.png"), "{raw}");
        assert!(raw.contains("t3-confirmation-required"), "{raw}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Append failures surface as Err (the caller warns via eprintln and
    /// continues — fail-open for every action class); a path inside a plain
    /// file cannot be created.
    #[test]
    fn append_to_an_unwritable_path_returns_err_without_panicking() {
        let blocker = std::env::temp_dir().join(format!(
            "pinvou3-cu-audit-blocker-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        fs::write(&blocker, b"not a directory").expect("write blocker file");
        let log = AuditLog::at_path(blocker.join("audit.jsonl"));
        let record = AuditRecord::new("s1", "type", "input").finish("ok", None, 1);
        assert!(log.append(&record).is_err());
        let _ = fs::remove_file(&blocker);
    }

    /// Private-data governance: for_session creates a 0700 directory and the
    /// appended file is 0600. Mode assertions go through the platform
    /// adapter's cfg(test) helpers so the target cfg stays in the adapter
    /// layer (architecture guard rule: no target cfg outside adapters).
    /// Windows has no POSIX mode bits, so the helpers pass through trivially
    /// while the private-directory layout is still exercised.
    #[test]
    fn for_session_creates_private_dir_and_0600_file() {
        // Mutually exclusive with tests that rewrite PINVOU3_HOME.
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var_os("PINVOU3_HOME");
        let home = std::env::temp_dir().join(format!(
            "pinvou3-cu-audit-home-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        // SAFETY: holding ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };
        let log = AuditLog::for_session("s-private").expect("audit log for session");
        let record = AuditRecord::new("s-private", "screenshot", "observe").finish("ok", None, 1);
        log.append(&record).expect("append");
        crate::platform::filesystem::assert_private_file_mode(log.path());
        crate::platform::filesystem::assert_private_dir_mode(&audit_dir());
        // SAFETY: holding ENV_LOCK.
        unsafe {
            match previous {
                Some(value) => std::env::set_var("PINVOU3_HOME", value),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn target_truncates_to_audit_byte_contract() {
        let long = "删".repeat(500); // 1500 bytes
        let mut record = AuditRecord::new("s", "left_click", "input");
        record.with_target(&long);
        let target = record.target.clone().unwrap_or_default();
        assert!(target.len() <= AUDIT_TARGET_MAX_BYTES);
        // CJK is never cut mid-character (the truncate_utf8 contract).
        assert!(target.is_char_boundary(target.len()));
    }

    #[test]
    fn session_id_is_sanitized_for_filename() {
        assert_eq!(sanitize_session_id("abc-123_def"), "abc-123_def");
        assert_eq!(sanitize_session_id("../evil/x"), "___evil_x");
        assert_eq!(sanitize_session_id(""), "unknown");
    }

    /// Sanitization is lossy (`a/b` and `a_b` both become `a_b`), so the raw
    /// session id hash in the file name is what keeps colliding ids apart.
    #[test]
    fn colliding_sanitized_session_ids_get_distinct_file_names() {
        assert_eq!(sanitize_session_id("a/b"), sanitize_session_id("a_b"));
        let slash = audit_file_name("a/b");
        let underscore = audit_file_name("a_b");
        assert_ne!(slash, underscore);
        // Static messages: the file name derives from sanitize_session_id,
        // and formatting that taint into assert output trips CodeQL
        // rust/cleartext-logging (the repo-wide assert-message hygiene rule).
        assert!(
            slash.starts_with("audit-a_b-"),
            "the sanitized id must survive in the file name prefix"
        );
        // First 8 bytes of the SHA-256, hex-encoded (16 chars).
        let stem = slash
            .trim_start_matches("audit-a_b-")
            .trim_end_matches(".jsonl");
        assert_eq!(
            stem.len(),
            16,
            "the hash stem is the first 8 sha-256 bytes, hex-encoded"
        );
        assert!(
            stem.chars().all(|c| c.is_ascii_hexdigit()),
            "the hash stem must be pure hex"
        );
    }
}
