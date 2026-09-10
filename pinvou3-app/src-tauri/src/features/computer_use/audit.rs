//! 审计日志：append-only JSONL，每次工具调用执行前写 begin 记录、完成后写
//! end 记录（同一 call_id 关联，纯追加绝不原地改）。
//!
//! 隐私契约：键入文本只记**长度 + salt || text 的 HMAC-SHA256**，永不记明文
//! （可能是密码）；逐记录 16 字节 CSPRNG 盐使同文本的 MAC 互不相同，批量
//! 「同长度同 MAC」相关性不存在；截图记 SHA-256 + 文件路径；target 字段按
//! 平台审计约定截到 ≤600 字节。
//!
//! HMAC 密钥获取链（进程级缓存）：OS 钥匙环（codewhale-secrets，缺则随机
//! 生成并写入）→ 钥匙环不可用时回退文件密钥 `<数据根>/keys/computer-use-
//! audit-hmac.key`（0700 目录 + 尽力 0600 文件；首次使用时若旧布局
//! `computer-use/audit-hmac.key` 存在则原子 rename 迁移）→ 两者都失败则
//! **只记长度**。密钥与日志分离是底线：只拿到单条 jsonl 的人不能对键入内容
//! 做离线字典恢复（评审发现：无盐 SHA-256 + 明文长度对密码这类小键空间形同
//! 明文）。

use std::ffi::OsStr;
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
/// 旧版（v1）密钥文件名：与日志同目录（审计目录内）。
const AUDIT_HMAC_KEY_FILE: &str = "audit-hmac.key";
/// 现行（v2）密钥文件名：`<数据根>/keys/` 内，与日志分目录。
const AUDIT_HMAC_KEY_V2_FILE: &str = "computer-use-audit-hmac.key";
const AUDIT_HMAC_KEY_BYTES: usize = 32;
/// 键入文本 MAC 的逐记录随机盐长度（CSPRNG，见 `with_typed_text`）。
const TYPED_TEXT_SALT_BYTES: usize = 16;

/// target / 元素标签字段的字节上限（平台审计字段统一约定）。
pub const AUDIT_TARGET_MAX_BYTES: usize = 600;

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(&sha2::Sha256::digest(bytes))
}

/// `<pinvou3 data dir>/computer-use/`。
pub fn audit_dir() -> PathBuf {
    paths::pinvou3_home().join("computer-use")
}

/// `<pinvou3 data dir>/keys/`——审计 HMAC 密钥文件的家（0700 私有目录，
/// 与日志分目录：拿到日志目录不等于拿到密钥）。
pub fn audit_keys_dir() -> PathBuf {
    paths::pinvou3_home().join("keys")
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
    /// 键入文本 MAC 的逐记录随机盐（hex）。与 `text_hmac_sha256` 同时出现；
    /// 密钥不可用时 MAC 缺省、盐保留（下次密钥恢复也无法重放旧记录的 MAC）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
    /// `salt || text` 的 HMAC-SHA256（密钥见模块文档）；密钥不可用时缺省
    /// （仅长度）。逐记录随机盐使同文本的 MAC 互不相同。
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
            salt: None,
            text_hmac_sha256: None,
            screenshot_sha256: None,
            screenshot_path: None,
            consent: consent.into(),
            result: None,
            error: None,
            duration_ms: None,
        }
    }

    /// 记录键入文本：只存长度、逐记录随机盐与 `salt || text` 的 HMAC（密钥
    /// 不可用时仅长度+盐）。盐使同文本的 MAC 互不相同——「同长度同 MAC」
    /// 的相关性与同文本的跨记录可关联性一并消除（评审发现）。
    pub fn with_typed_text(&mut self, text: &str) -> &mut Self {
        self.text_len = Some(text.chars().count());
        let salt: [u8; TYPED_TEXT_SALT_BYTES] = rand::random();
        self.salt = Some(hex_lower(&salt));
        if let Some(key) = audit_mac_key() {
            self.text_hmac_sha256 = typed_text_mac(key, &salt, text);
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
        // 审计记录（即使已脱敏）属私有数据治理范畴：走平台层的私有追加文件
        // 助手（unix 上 0600 创建、无 umask 暴露窗口；评审发现：此前经普通
        // OpenOptions 按 0644 落盘，纵深不足）。
        let mut file = crate::platform::filesystem::open_private_append_file(&self.path)?;
        file.write_all(line.as_bytes())
    }
}

fn decode_hex_exact(stored: &str, expected_len: usize) -> Option<Vec<u8>> {
    let stored = stored.trim();
    if stored.len() != expected_len * 2 || !stored.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..expected_len)
        .map(|i| u8::from_str_radix(&stored[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

fn decode_hex_32(stored: &str) -> Option<Vec<u8>> {
    decode_hex_exact(stored, AUDIT_HMAC_KEY_BYTES)
}

/// 键入文本的 MAC：`HMAC(key, salt || text)`。纯函数，便于向量测试。
fn typed_text_mac(key: &[u8], salt: &[u8], text: &str) -> Option<String> {
    let mut input = Vec::with_capacity(salt.len() + text.len());
    input.extend_from_slice(salt);
    input.extend_from_slice(text.as_bytes());
    hmac_sha256_hex(key, &input)
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
        // 钥匙环不可用：回退文件密钥（经 platform 私有文件基座，0700 目录 +
        // 私有 ACL/权限，OS 差异留在 platform 层）。现行位置是
        // `<数据根>/keys/`（与日志分目录）；首次使用时若旧布局（审计目录内
        // 的 audit-hmac.key）存在则原子 rename 迁移，新密钥直接写新位置。
        // keys/ 目录不可用时退回旧布局位置读写（保持 v1 保障不丢）。
        let legacy = crate::platform::filesystem::open_private_file_directory(&audit_dir()).ok();
        match crate::platform::filesystem::open_private_file_directory(&audit_keys_dir()) {
            Ok(keys_dir) => load_or_migrate_key_file(
                &keys_dir,
                OsStr::new(AUDIT_HMAC_KEY_V2_FILE),
                legacy
                    .as_ref()
                    .map(|directory| (directory, OsStr::new(AUDIT_HMAC_KEY_FILE))),
            ),
            Err(_) => legacy.and_then(|directory| {
                load_or_migrate_key_file(&directory, OsStr::new(AUDIT_HMAC_KEY_FILE), None)
            }),
        }
    })
    .as_ref()
}

/// 文件密钥回退链（`audit_mac_key` 的可测内核）：
/// 1. 主位置已有合法密钥 → 直接使用；
/// 2. 旧位置存在密钥 → 原子 rename 迁到主位置后使用（两代路径兼容）；
/// 3. 都没有 → 生成新密钥并**只写主位置**。
///
/// 任一步失败返回 None（调用方降级为只记长度）。
fn load_or_migrate_key_file(
    primary: &crate::platform::filesystem::PrivateFileDirectory,
    primary_name: &OsStr,
    legacy: Option<(&crate::platform::filesystem::PrivateFileDirectory, &OsStr)>,
) -> Option<Vec<u8>> {
    use crate::platform::filesystem::MovePlainFileOutcome;

    if let Some(key) = read_audit_key_file_named(primary, primary_name) {
        return Some(key);
    }
    if let Some((legacy_dir, legacy_name)) = legacy {
        // 原子 rename：进程崩溃也不会出现「两处都在/两处都不在」的中间态。
        match legacy_dir.move_plain_file_to(legacy_name, primary, primary_name) {
            Ok(MovePlainFileOutcome::Moved | MovePlainFileOutcome::AlreadyMoved) => {
                if let Some(key) = read_audit_key_file_named(primary, primary_name) {
                    return Some(key);
                }
            }
            // 旧位置无密钥（Missing）或迁移失败：继续生成新密钥。
            Ok(MovePlainFileOutcome::Missing) | Err(_) => {}
        }
    }
    let key: [u8; AUDIT_HMAC_KEY_BYTES] = rand::random();
    primary
        .atomic_write_private_file(primary_name, hex_lower(&key).as_bytes())
        .ok()?;
    Some(key.to_vec())
}

fn read_audit_key_file_named(
    directory: &crate::platform::filesystem::PrivateFileDirectory,
    name: &OsStr,
) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut file = directory.open_plain_file(name).ok()??;
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
        // 与改写 PINVOU3_HOME 的测试互斥：钥匙环不可用时密钥回退路径按
        // PINVOU3_HOME 解析，不能在 env 被并发改写时取密钥。
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
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
        // 盐字段存在（16 字节 = 32 hex）。
        let salt_field = first["salt"].as_str().unwrap_or_default();
        assert_eq!(salt_field.len(), TYPED_TEXT_SALT_BYTES * 2, "salt hex");
        // HMAC 字段存在且不等于无盐 SHA-256（密钥参与运算的证据），并与
        // 记录自身的盐一致（MAC 输入 = salt || text 的证据）。
        let hmac_field = first["text_hmac_sha256"].as_str().unwrap_or_default();
        assert_eq!(hmac_field.len(), 64, "hmac hex: {hmac_field}");
        assert_ne!(hmac_field, sha256_hex(secret.as_bytes()));
        let salt_bytes = decode_hex_exact(salt_field, TYPED_TEXT_SALT_BYTES).unwrap_or_default();
        assert_eq!(
            Some(hmac_field),
            audit_mac_key()
                .and_then(|key| typed_text_mac(key, &salt_bytes, secret))
                .as_deref()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// 评审修复回归：逐记录随机盐——同文本两条记录的 MAC 互不相同，且各与
    /// 自身盐一致；盐变更后旧 MAC 无法复算。
    #[test]
    fn typed_text_mac_is_salted_per_record() {
        // 与改写 PINVOU3_HOME 的测试互斥（理由见 begin_then_end 测试）。
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut a = AuditRecord::begin("c", "s", "type", "input", "input");
        a.with_typed_text("hunter2");
        let mut b = AuditRecord::begin("c", "s", "type", "input", "input");
        b.with_typed_text("hunter2");
        let Some(key) = audit_mac_key() else {
            panic!("test environment must provide an audit mac key");
        };
        let mac = |record: &AuditRecord| {
            let salt = decode_hex_exact(
                record.salt.as_deref().unwrap_or_default(),
                TYPED_TEXT_SALT_BYTES,
            )
            .expect("salt hex");
            typed_text_mac(key, &salt, "hunter2").expect("mac")
        };
        assert_ne!(mac(&a), mac(&b), "random per-record salt must differ");
        assert_eq!(mac(&a), a.text_hmac_sha256.clone().unwrap_or_default());
        // 换一个盐，MAC 必须变化（盐确实参与运算）。
        assert_ne!(
            typed_text_mac(key, &[0u8; TYPED_TEXT_SALT_BYTES], "hunter2"),
            typed_text_mac(key, &[1u8; TYPED_TEXT_SALT_BYTES], "hunter2")
        );
    }

    #[test]
    fn hmac_helper_matches_rfc4231_vector() {
        // RFC 4231 test case 2: key="Jefe", data="what do ya want for nothing?".
        assert_eq!(
            hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?").as_deref(),
            Some("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
        );
    }

    /// 加盐后的 MAC 仍是标准 HMAC：空盐时退化为 RFC 4231 原向量。
    #[test]
    fn typed_text_mac_matches_rfc4231_vector_with_empty_salt() {
        assert_eq!(
            typed_text_mac(b"Jefe", b"", "what do ya want for nothing?").as_deref(),
            Some("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
        );
        // 非空盐 = 数据前缀拼接（salt || text 语义的直接证据）。
        let expected = hmac_sha256_hex(b"Jefe", b"salwhat do ya want for nothing?");
        assert_eq!(
            typed_text_mac(b"Jefe", b"sal", "what do ya want for nothing?"),
            expected
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

    /// 密钥迁移测试基建：隔离的 legacy（审计）目录 + keys 目录。
    struct KeyDirs {
        base: PathBuf,
        legacy: crate::platform::filesystem::PrivateFileDirectory,
        keys: crate::platform::filesystem::PrivateFileDirectory,
    }

    impl Drop for KeyDirs {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    fn key_dirs() -> KeyDirs {
        let base = std::env::temp_dir().join(format!(
            "pinvou3-cu-keys-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let legacy =
            crate::platform::filesystem::open_private_file_directory(&base.join("computer-use"))
                .expect("legacy audit dir");
        let keys = crate::platform::filesystem::open_private_file_directory(&base.join("keys"))
            .expect("keys dir");
        KeyDirs { base, legacy, keys }
    }

    /// 评审修复回归：旧布局（审计目录内 audit-hmac.key）首次使用时原子
    /// rename 迁移到 `<数据根>/keys/computer-use-audit-hmac.key`，旧文件消失。
    #[test]
    fn legacy_key_file_migrates_atomically_to_keys_directory() {
        let dirs = key_dirs();
        let stored = "ab".repeat(AUDIT_HMAC_KEY_BYTES);
        dirs.legacy
            .atomic_write_private_file(OsStr::new(AUDIT_HMAC_KEY_FILE), stored.as_bytes())
            .expect("write legacy key");

        let key = load_or_migrate_key_file(
            &dirs.keys,
            OsStr::new(AUDIT_HMAC_KEY_V2_FILE),
            Some((&dirs.legacy, OsStr::new(AUDIT_HMAC_KEY_FILE))),
        )
        .expect("migrated key");

        assert_eq!(key, decode_hex_32(&stored).expect("valid key hex"));
        // 新位置内容一致，旧位置条目消失（真迁移，不是复制）。
        assert_eq!(
            read_audit_key_file_named(&dirs.keys, OsStr::new(AUDIT_HMAC_KEY_V2_FILE)),
            Some(key)
        );
        let legacy_leftover = dirs
            .legacy
            .open_plain_file(OsStr::new(AUDIT_HMAC_KEY_FILE))
            .expect("probe legacy dir");
        assert!(legacy_leftover.is_none(), "legacy key file must be gone");
    }

    /// 两代路径兼容：新位置已有合法密钥时直接使用，旧位置原样保留。
    #[test]
    fn existing_v2_key_wins_and_legacy_file_is_untouched() {
        let dirs = key_dirs();
        let v2 = "cd".repeat(AUDIT_HMAC_KEY_BYTES);
        let legacy = "ab".repeat(AUDIT_HMAC_KEY_BYTES);
        dirs.keys
            .atomic_write_private_file(OsStr::new(AUDIT_HMAC_KEY_V2_FILE), v2.as_bytes())
            .expect("write v2 key");
        dirs.legacy
            .atomic_write_private_file(OsStr::new(AUDIT_HMAC_KEY_FILE), legacy.as_bytes())
            .expect("write legacy key");

        let key = load_or_migrate_key_file(
            &dirs.keys,
            OsStr::new(AUDIT_HMAC_KEY_V2_FILE),
            Some((&dirs.legacy, OsStr::new(AUDIT_HMAC_KEY_FILE))),
        )
        .expect("v2 key");

        assert_eq!(key, decode_hex_32(&v2).expect("valid v2 hex"));
        assert_eq!(
            read_audit_key_file_named(&dirs.legacy, OsStr::new(AUDIT_HMAC_KEY_FILE)),
            Some(decode_hex_32(&legacy).expect("valid legacy hex"))
        );
    }

    /// 两代都无密钥：生成新密钥且只写新位置。
    #[test]
    fn fresh_key_is_written_to_the_new_location_only() {
        let dirs = key_dirs();
        let key = load_or_migrate_key_file(
            &dirs.keys,
            OsStr::new(AUDIT_HMAC_KEY_V2_FILE),
            Some((&dirs.legacy, OsStr::new(AUDIT_HMAC_KEY_FILE))),
        )
        .expect("fresh key");

        assert_eq!(key.len(), AUDIT_HMAC_KEY_BYTES);
        assert_eq!(
            read_audit_key_file_named(&dirs.keys, OsStr::new(AUDIT_HMAC_KEY_V2_FILE)),
            Some(key)
        );
        let legacy_absent = dirs
            .legacy
            .open_plain_file(OsStr::new(AUDIT_HMAC_KEY_FILE))
            .expect("probe legacy dir");
        assert!(legacy_absent.is_none());
    }

    /// 旧位置内容损坏（非 64 hex）：不迁移毒数据，生成新密钥写新位置。
    #[test]
    fn corrupt_legacy_key_is_ignored_and_replaced_by_fresh_key() {
        let dirs = key_dirs();
        dirs.legacy
            .atomic_write_private_file(OsStr::new(AUDIT_HMAC_KEY_FILE), b"not-a-key")
            .expect("write corrupt legacy key");
        let key = load_or_migrate_key_file(
            &dirs.keys,
            OsStr::new(AUDIT_HMAC_KEY_V2_FILE),
            Some((&dirs.legacy, OsStr::new(AUDIT_HMAC_KEY_FILE))),
        )
        .expect("fresh key");
        assert_eq!(key.len(), AUDIT_HMAC_KEY_BYTES);
        // 损坏的旧文件被迁移消费，新位置是可用的新密钥（恢复而非卡死）。
        let legacy_leftover = dirs
            .legacy
            .open_plain_file(OsStr::new(AUDIT_HMAC_KEY_FILE))
            .expect("probe legacy dir");
        assert!(
            legacy_leftover.is_none(),
            "corrupt legacy file must be consumed"
        );
        assert_eq!(
            read_audit_key_file_named(&dirs.keys, OsStr::new(AUDIT_HMAC_KEY_V2_FILE)),
            Some(key)
        );
    }
}
