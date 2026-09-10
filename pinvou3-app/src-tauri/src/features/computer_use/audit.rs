//! 审计日志：append-only JSONL，每次工具调用执行前写 begin 记录、完成后写
//! end 记录（同一 call_id 关联，纯追加绝不原地改）。
//!
//! 隐私契约：键入文本只记**长度 + HMAC-SHA256**，永不记明文（可能是密码）；
//! 截图记 SHA-256 + 文件路径；target 字段按平台审计约定截到 ≤600 字节。
//!
//! HMAC 密钥获取链（进程级缓存）：OS 钥匙环（codewhale-secrets，缺则随机
//! 生成并写入）→ 钥匙环不可用时回退审计目录内的 `audit-hmac.key`（0700
//! 目录 + 尽力 0600 文件）→ 两者都失败则**只记长度**。密钥与日志分离是
//! 底线：只拿到单条 jsonl 的人不能对键入内容做离线字典恢复（评审发现：
//! 无盐 SHA-256 + 明文长度对密码这类小键空间形同明文）。

use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use hmac::{KeyInit, Mac};
use serde::Serialize;
use sha2::Digest as _;
type HmacSha256 = hmac::Hmac<sha2::Sha256>;

use crate::platform::encoding::hex_lower;
use crate::platform::paths;
use crate::platform::strings::truncate_utf8;

const AUDIT_HMAC_SECRET_NAME: &str = "pinvou3-computer-use-audit-hmac";
const AUDIT_HMAC_KEY_FILE: &str = "audit-hmac.key";
const AUDIT_HMAC_KEY_BYTES: usize = 32;

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

/// 一条审计记录。`phase` 为 "begin"（执行前）或 "end"（完成后）。
#[derive(Debug, Clone, Serialize)]
pub struct AuditRecord {
    pub call_id: String,
    pub phase: String,
    pub timestamp: String,
    pub session_id: String,
    pub action: String,
    pub class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_len: Option<usize>,
    /// 键入文本的 HMAC-SHA256（密钥见模块文档）；密钥不可用时缺省（仅长度）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_hmac_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_path: Option<String>,
    /// 实际施加的同意层级/结果，如 "observe" / "input:session-grant" /
    /// "input:confirmed:<id>" / "rejected:grant-required"。
    pub consent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl AuditRecord {
    pub fn begin(
        call_id: impl Into<String>,
        session_id: impl Into<String>,
        action: impl Into<String>,
        class: impl Into<String>,
        consent: impl Into<String>,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            phase: "begin".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            session_id: session_id.into(),
            action: action.into(),
            class: class.into(),
            target: None,
            text_len: None,
            text_hmac_sha256: None,
            screenshot_sha256: None,
            screenshot_path: None,
            consent: consent.into(),
            result: None,
            error: None,
            duration_ms: None,
        }
    }

    /// 记录键入文本：只存长度与 HMAC（密钥不可用时仅长度）。
    pub fn with_typed_text(&mut self, text: &str) -> &mut Self {
        self.text_len = Some(text.chars().count());
        if let Some(digest) = audit_mac_key().and_then(|key| hmac_sha256_hex(key, text.as_bytes()))
        {
            self.text_hmac_sha256 = Some(digest);
        }
        self
    }

    /// 记录目标（坐标摘要或元素标签），按审计约定截断。
    pub fn with_target(&mut self, target: &str) -> &mut Self {
        self.target = Some(truncate_utf8(target, AUDIT_TARGET_MAX_BYTES).to_string());
        self
    }

    pub fn with_screenshot(&mut self, png: &[u8], path: &Path) -> &mut Self {
        self.screenshot_sha256 = Some(sha256_hex(png));
        self.screenshot_path = Some(path.to_string_lossy().into_owned());
        self
    }

    pub fn finish(mut self, result: &str, error: Option<String>, duration_ms: u64) -> Self {
        self.phase = "end".to_string();
        self.timestamp = chrono::Utc::now().to_rfc3339();
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

    /// 追加一条记录。序列化失败不可能（纯字符串/数字字段），IO 失败向上抛。
    pub fn append(&self, record: &AuditRecord) -> io::Result<()> {
        let mut line = serde_json::to_string(record)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())
    }
}

fn decode_hex_32(stored: &str) -> Option<Vec<u8>> {
    let stored = stored.trim();
    if stored.len() != AUDIT_HMAC_KEY_BYTES * 2 || !stored.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..AUDIT_HMAC_KEY_BYTES)
        .map(|i| u8::from_str_radix(&stored[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

/// 审计 HMAC 密钥（进程级缓存）。获取链见模块文档。
fn audit_mac_key() -> Option<&'static Vec<u8>> {
    static KEY: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    KEY.get_or_init(|| {
        let secrets = codewhale_secrets::Secrets::auto_detect();
        if let Ok(Some(stored)) = secrets.get(AUDIT_HMAC_SECRET_NAME) {
            if let Some(key) = decode_hex_32(&stored) {
                return Some(key);
            }
        }
        let key: [u8; AUDIT_HMAC_KEY_BYTES] = rand::random();
        let hex = hex_lower(&key);
        if secrets.set(AUDIT_HMAC_SECRET_NAME, &hex).is_ok() {
            return Some(key.to_vec());
        }
        // 钥匙环不可用：回退审计目录内的密钥文件（经 platform 私有文件
        // 基座写入，0700 目录 + 私有 ACL/权限，OS 差异留在 platform 层）。
        if let Ok(directory) =
            crate::platform::filesystem::open_private_file_directory(&audit_dir())
        {
            if let Some(key) = read_audit_key_file(&directory, AUDIT_HMAC_KEY_FILE) {
                return Some(key);
            }
            if directory
                .atomic_write_private_file(OsStr::new(AUDIT_HMAC_KEY_FILE), hex.as_bytes())
                .is_ok()
            {
                return Some(key.to_vec());
            }
        }
        None
    })
    .as_ref()
}

fn read_audit_key_file(
    directory: &crate::platform::filesystem::PrivateFileDirectory,
    name: &str,
) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut file = directory.open_plain_file(OsStr::new(name)).ok()??;
    let mut stored = String::new();
    file.read_to_string(&mut stored).ok()?;
    decode_hex_32(&stored)
}

fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> Option<String> {
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(key).ok()?;
    mac.update(data);
    Some(hex_lower(&mac.finalize().into_bytes()))
}

pub fn new_call_id() -> String {
    format!("cu-call-{:016x}", rand::random::<u64>())
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
    fn begin_then_end_records_share_call_id_and_never_hold_plaintext() {
        let (dir, log) = temp_log();
        let secret = "hunter2-密码";
        let mut begin =
            AuditRecord::begin("cu-call-1", "s1", "type", "input", "input:session-grant");
        begin.with_typed_text(secret).with_target("password field");
        log.append(&begin).unwrap_or(());
        let end = begin.clone().finish("ok", None, 12);
        log.append(&end).unwrap_or(());

        let raw = fs::read_to_string(log.path()).unwrap_or_default();
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value =
            serde_json::from_str(lines[0]).unwrap_or(serde_json::Value::Null);
        let second: serde_json::Value =
            serde_json::from_str(lines[1]).unwrap_or(serde_json::Value::Null);
        assert_eq!(first["phase"], "begin");
        assert_eq!(second["phase"], "end");
        assert_eq!(first["call_id"], second["call_id"]);
        assert_eq!(first["text_len"], secret.chars().count() as u64);
        assert_eq!(second["result"], "ok");
        assert_eq!(second["duration_ms"], 12);
        // 明文绝不进日志（中英文两边都查）。
        assert!(!raw.contains("hunter2"));
        assert!(!raw.contains("密码"));
        // HMAC 字段存在且不等于无盐 SHA-256（密钥参与运算的证据）。
        let hmac_field = first["text_hmac_sha256"].as_str().unwrap_or_default();
        assert_eq!(hmac_field.len(), 64, "hmac hex: {hmac_field}");
        assert_ne!(hmac_field, sha256_hex(secret.as_bytes()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hmac_helper_matches_rfc4231_vector() {
        // RFC 4231 test case 2: key="Jefe", data="what do ya want for nothing?".
        assert_eq!(
            hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?").as_deref(),
            Some("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
        );
    }

    #[test]
    fn target_truncates_to_audit_byte_contract() {
        let long = "删".repeat(500); // 1500 字节
        let mut record = AuditRecord::begin("c", "s", "left_click", "input", "input");
        record.with_target(&long);
        let target = record.target.clone().unwrap_or_default();
        assert!(target.len() <= AUDIT_TARGET_MAX_BYTES);
        // 中文不被切成半字符（truncate_utf8 契约）。
        assert!(target.is_char_boundary(target.len()));
    }

    #[test]
    fn screenshot_records_hash_and_path_not_pixels() {
        let (dir, log) = temp_log();
        let png = b"\x89PNG-fake-bytes";
        let mut record = AuditRecord::begin("c", "s", "screenshot", "observe", "observe");
        record.with_screenshot(png, Path::new("/tmp/shot.png"));
        log.append(&record).unwrap_or(());
        let raw = fs::read_to_string(log.path()).unwrap_or_default();
        assert!(raw.contains(&sha256_hex(png)));
        assert!(raw.contains("/tmp/shot.png"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_id_is_sanitized_for_filename() {
        assert_eq!(sanitize_session_id("abc-123_def"), "abc-123_def");
        assert_eq!(sanitize_session_id("../evil/x"), "___evil_x");
        assert_eq!(sanitize_session_id(""), "unknown");
    }
}
