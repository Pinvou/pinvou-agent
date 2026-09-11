//! 审计日志：append-only JSONL，每次工具调用一条记录（纯追加绝不原地改）。
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
//! exposure window) + `sync_data` per record so a crash cannot tear the
//! last JSONL line. Append failures are fail-open: the log is
//! informational, so the caller warns and continues — an audit write error
//! never blocks an action.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::Digest as _;

use crate::platform::encoding::hex_lower;
use crate::platform::paths;
use crate::platform::strings::truncate_utf8;

/// target / 元素标签字段的字节上限（平台审计字段统一约定）。
pub const AUDIT_TARGET_MAX_BYTES: usize = 600;

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(&sha2::Sha256::digest(bytes))
}

/// `<pinvou3 data dir>/computer-use/`。
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
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

/// 一条审计记录：一次工具调用的纯信息性快照。
#[derive(Debug, Clone, Serialize)]
pub struct AuditRecord {
    pub timestamp: String,
    pub session_id: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// 实际施加的同意层级/结果，如 "observe" / "input:session-grant" /
    /// "input:session-grant+t3-confirmed" / "rejected:grant-required"。
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

    /// 记录目标（坐标摘要或元素标签），按审计约定截断。调用方负责脱敏：
    /// 键入文本永不进此字段（只记长度，见模块文档）。
    pub fn with_target(&mut self, target: &str) -> &mut Self {
        self.target = Some(truncate_utf8(target, AUDIT_TARGET_MAX_BYTES).to_string());
        self
    }

    /// 截图只记 SHA-256 + 文件路径，绝不记像素。
    pub fn with_screenshot(&mut self, png: &[u8], path: &Path) -> &mut Self {
        self.screenshot_sha256 = Some(sha256_hex(png));
        self.screenshot_path = Some(path.to_string_lossy().into_owned());
        self
    }

    pub fn finish(mut self, result: &str, error: Option<String>, duration_ms: u64) -> Self {
        self.result = Some(result.to_string());
        self.error = error;
        self.duration_ms = Some(duration_ms);
        self
    }
}

/// 单会话审计日志（append-only）。
pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    /// `<pinvou3 data dir>/computer-use/audit-<session_id>.jsonl`，
    /// 目录以私有权限创建。
    pub fn for_session(session_id: &str) -> io::Result<Self> {
        let dir = audit_dir();
        // 私有权限目录助手：创建/校验 0700（Windows 等价 ACL）。
        crate::platform::filesystem::open_private_file_directory(&dir)?;
        Ok(Self {
            path: dir.join(format!("audit-{}.jsonl", sanitize_session_id(session_id))),
        })
    }

    /// 测试用：指定任意路径（不建私有目录）。
    #[cfg(test)]
    pub(crate) fn at_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 追加一条记录。序列化失败不可能（纯字符串/数字字段），IO 失败向上抛
    /// （调用方 fail-open：eprintln 后继续）。
    pub fn append(&self, record: &AuditRecord) -> io::Result<()> {
        let mut line = serde_json::to_string(record)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        line.push('\n');
        // 审计记录（即使已脱敏）属私有数据治理范畴：走平台层的私有追加文件
        // 助手（unix 上 0600 创建、无 umask 暴露窗口；评审发现：此前经普通
        // OpenOptions 按 0644 落盘，纵深不足）。
        let mut file = crate::platform::filesystem::open_private_append_file(&self.path)?;
        file.write_all(line.as_bytes())?;
        // fsync each record: a crash must not tear the last JSONL line. One
        // fsync per record is acceptable at audit's per-tool-call frequency.
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
    /// appended file is 0600.
    #[test]
    fn for_session_creates_private_dir_and_0600_file() {
        // 与改写 PINVOU3_HOME 的测试互斥。
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
        use std::os::unix::fs::PermissionsExt;
        let file_mode = fs::metadata(log.path())
            .expect("audit file metadata")
            .permissions()
            .mode();
        assert_eq!(file_mode & 0o7777, 0o600, "audit file must be 0600");
        let dir_mode = fs::metadata(audit_dir())
            .expect("audit dir metadata")
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o7777, 0o700, "audit dir must be 0700");
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
        let long = "删".repeat(500); // 1500 字节
        let mut record = AuditRecord::new("s", "left_click", "input");
        record.with_target(&long);
        let target = record.target.clone().unwrap_or_default();
        assert!(target.len() <= AUDIT_TARGET_MAX_BYTES);
        // 中文不被切成半字符（truncate_utf8 契约）。
        assert!(target.is_char_boundary(target.len()));
    }

    #[test]
    fn session_id_is_sanitized_for_filename() {
        assert_eq!(sanitize_session_id("abc-123_def"), "abc-123_def");
        assert_eq!(sanitize_session_id("../evil/x"), "___evil_x");
        assert_eq!(sanitize_session_id(""), "unknown");
    }
}
