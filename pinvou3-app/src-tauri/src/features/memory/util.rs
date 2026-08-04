//! Low-level shared primitives for the memory feature: text sanitization,
//! stable id hashing, time parsing, and atomic JSONL writes.
//!
//! 抽离自 `mod.rs`——这些 helper 被实体存储（io）、LLM 审查（llm_review）与
//! 渲染（render）多处复用，集中放这里避免循环依赖。纯函数，行为与原实现一致。

use std::fs;
use std::io;
use std::path::Path;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Topic 生命周期锁：迁移 journal / 恢复提升期间独占，读侧借它等待活跃写入者
/// 落盘完成，避免读到中间态。拆分前定义在 `mod.rs` 的 `write_lock` 旁，
/// 拆分后随原子写盘/恢复契约一起落在 util。
pub(super) fn file_lifecycle_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub(super) fn clean_scalar(value: &str) -> String {
    clean_text(value, 200)
}

pub(super) fn clean_text(value: &str, max_chars: usize) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .chars()
        .take(max_chars)
        .collect()
}

pub(super) fn clean_id(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .take(80)
        .collect()
}

pub(super) fn stable_id_from_text(value: &str) -> String {
    stable_id_with_prefix("rw", value)
}

pub(super) fn stable_id_with_prefix(prefix: &str, value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{}_{hash:016x}", clean_id(prefix))
}

pub(super) fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

pub(super) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(invalid_data)?;
    write_text_atomic(path, &(text + "\n"))
}

pub(super) fn write_json_atomic_unlocked<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(invalid_data)?;
    write_text_atomic_unlocked(path, &(text + "\n"))
}

pub(super) fn write_text_atomic(path: &Path, text: &str) -> io::Result<()> {
    let _lifecycle = file_lifecycle_lock().lock();
    write_text_atomic_unlocked(path, text)
}

pub(super) fn write_text_atomic_unlocked(path: &Path, text: &str) -> io::Result<()> {
    write_text_atomic_unlocked_with(
        path,
        text,
        crate::platform::filesystem::replace_file_atomically,
    )
}

pub(super) fn write_text_atomic_unlocked_with<F>(path: &Path, text: &str, replace: F) -> io::Result<()>
where
    F: FnOnce(&Path, &Path, &Path) -> crate::platform::filesystem::ReplaceResult,
{
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = path.with_extension(format!("tmp-{}-{timestamp}-{sequence}", std::process::id()));
    let backup = path.with_extension("bak");
    let stage_result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        Ok(())
    })();
    if let Err(error) = stage_result {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    match replace(&tmp, path, &backup) {
        Ok(crate::platform::filesystem::ReplaceState::Committed) => Ok(()),
        Ok(state) => Err(io::Error::other(format!(
            "unexpected successful replacement state: {state:?}"
        ))),
        Err(error) => {
            if error.state() == crate::platform::filesystem::ReplaceState::RolledBack {
                let _ = fs::remove_file(&tmp);
                let _ = fs::remove_file(&backup);
            } else if error.state() == crate::platform::filesystem::ReplaceState::RecoveryRequired
                && path.exists()
            {
                // A target that still exists (e.g. a directory occupying its
                // path) is a permanent failure: recovery can never promote a
                // candidate over it, so the staged tmp/backup are garbage. A
                // truly missing target keeps its candidates for
                // read_text_recovering_unlocked_with — matching the artifact
                // write path.
                let _ = fs::remove_file(&tmp);
                let _ = fs::remove_file(&backup);
            }
            Err(error.into_io_error())
        }
    }
}

pub(super) fn json_lines_are_valid<T: for<'de> Deserialize<'de>>(raw: &str) -> bool {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .all(|line| serde_json::from_str::<T>(line).is_ok())
}

pub(super) fn read_text_recovering(path: &Path, validate: impl Fn(&str) -> bool) -> io::Result<String> {
    let _lifecycle = file_lifecycle_lock().lock();
    read_text_recovering_unlocked(path, &validate)
}

pub(super) fn read_text_recovering_unlocked(
    path: &Path,
    validate: &dyn Fn(&str) -> bool,
) -> io::Result<String> {
    read_text_recovering_unlocked_with(path, validate, &promote_recovery_candidate)
}

/// Windows maps `ERROR_SHARING_VIOLATION` (32) / `ERROR_LOCK_VIOLATION` (33) to
/// `ErrorKind::Uncategorized` rather than `PermissionDenied`, so a file held by
/// an antivirus scan or another process without read-sharing fails
/// `fs::read_to_string` this way. Treat those codes as transient so recovery
/// never promotes a stale backup over a still-valid authoritative file.
#[cfg(windows)]
pub(super) fn is_transient_windows_lock(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(32) | Some(33))
}

#[cfg(not(windows))]
pub(super) fn is_transient_windows_lock(_error: &io::Error) -> bool {
    false
}

pub(super) fn read_text_recovering_unlocked_with(
    path: &Path,
    validate: &dyn Fn(&str) -> bool,
    promote: &dyn Fn(&Path, &Path, &[u8]) -> io::Result<()>,
) -> io::Result<String> {
    let current_error = match fs::read_to_string(path) {
        Ok(raw) if validate(&raw) => return Ok(raw),
        Ok(_) => io::Error::new(
            io::ErrorKind::InvalidData,
            "authoritative memory file is invalid",
        ),
        Err(error) => {
            // A transient read failure on an existing authoritative file (e.g.
            // an antivirus scan lock on Windows) must not trigger recovery:
            // promoting a stale backup would overwrite the still-valid target.
            // Deterministic errors (e.g. invalid UTF-8 → InvalidData) still
            // fall through to recovery so a corrupted authoritative file can
            // self-heal from a valid backup candidate.
            let transient = matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied
                    | io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::Interrupted
            ) || is_transient_windows_lock(&error);
            if transient && path.is_file() {
                return Err(error);
            }
            error
        }
    };
    let Some(parent) = path.parent() else {
        return Err(current_error);
    };
    let file_stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("profile");
    let mut candidates = Vec::new();
    let backup = path.with_extension("bak");
    if backup.is_file() {
        candidates.push((true, backup));
    }
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let candidate = entry.path();
            let Some(name) = candidate.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let is_regular_file = fs::symlink_metadata(&candidate)
                .is_ok_and(|metadata| metadata.file_type().is_file());
            if is_regular_file && name.starts_with(&format!("{file_stem}.tmp-")) {
                candidates.push((false, candidate));
            }
        }
    }
    // A ReplaceFileW 1177 backup is the old authority under its documented
    // alternate name. Prefer it over a replacement candidate; otherwise use
    // the newest valid completed temporary file.
    candidates.sort_by_key(|(is_backup, candidate)| {
        (
            *is_backup,
            fs::metadata(candidate)
                .and_then(|metadata| metadata.modified())
                .ok(),
        )
    });
    candidates.reverse();
    for (_, candidate) in candidates {
        let Ok(raw) = fs::read_to_string(&candidate) else {
            continue;
        };
        if !validate(&raw) {
            continue;
        }
        promote(&candidate, path, raw.as_bytes())?;
        let restored = fs::read_to_string(path)?;
        if validate(&restored) {
            cleanup_recovery_candidates(path);
            return Ok(restored);
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "promoted memory recovery candidate is invalid",
        ));
    }
    Err(current_error)
}

pub(super) fn promote_recovery_candidate(candidate: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};

    static RECOVERY_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let sequence = RECOVERY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let replacement = target.with_extension(format!("recover-{}-{sequence}", std::process::id()));
    let backup = target.with_extension("recover-bak");
    let stage_result = (|| -> io::Result<()> {
        use std::io::Write as _;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&replacement)?;
        output.write_all(bytes)?;
        output.sync_all()?;
        drop(output);
        Ok(())
    })();
    if let Err(error) = stage_result {
        let _ = fs::remove_file(replacement);
        return Err(error);
    }
    match crate::platform::filesystem::replace_file_atomically(&replacement, target, &backup) {
        Ok(crate::platform::filesystem::ReplaceState::Committed) => {
            let _ = fs::remove_file(candidate);
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Ok(state) => Err(io::Error::other(format!(
            "unexpected successful replacement state: {state:?}"
        ))),
        Err(error) => {
            if error.state() == crate::platform::filesystem::ReplaceState::RolledBack {
                let _ = fs::remove_file(replacement);
                let _ = fs::remove_file(backup);
            } else if error.state() == crate::platform::filesystem::ReplaceState::RecoveryRequired
                && target.exists()
            {
                // A permanently blocked target (e.g. a directory occupying its
                // path) can never accept a promoted candidate: drop the staged
                // recover-* files instead of leaking one pair per failed read.
                // A truly missing target keeps them for a later attempt.
                let _ = fs::remove_file(replacement);
                let _ = fs::remove_file(backup);
            }
            Err(error.into_io_error())
        }
    }
}

fn cleanup_recovery_candidates(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let candidate = entry.path();
            let name = candidate
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if candidate == path.with_extension("bak")
                || name.starts_with(&format!("{stem}.tmp-"))
                || name.starts_with(&format!("{stem}.recover-"))
            {
                let _ = fs::remove_file(candidate);
            }
        }
    }
}

pub(super) fn recover_directory_json_files<T: for<'de> Deserialize<'de>>(dir: &Path) -> io::Result<()> {
    let _lifecycle = file_lifecycle_lock().lock();
    recover_directory_json_files_unlocked::<T>(dir)
}

pub(super) fn recover_directory_json_files_unlocked<T: for<'de> Deserialize<'de>>(
    dir: &Path,
) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut targets = std::collections::BTreeSet::new();
    for entry in entries {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let stem = if let Some(stem) = name.strip_suffix(".bak") {
            Some(stem)
        } else {
            name.split_once(".tmp-").map(|(stem, _)| stem)
        };
        if let Some(stem) = stem.filter(|stem| !stem.is_empty()) {
            targets.insert(dir.join(format!("{stem}.json")));
        }
    }
    for target in targets {
        if target.is_file() {
            continue;
        }
        match read_text_recovering_unlocked(&target, &|raw| serde_json::from_str::<T>(raw).is_ok())
        {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(super) fn invalid_data(err: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

pub(super) fn clean_candidate_sentence(value: &str, max_chars: usize) -> String {
    let cleaned = value
        .trim()
        .trim_start_matches("请记住")
        .trim_start_matches("记住")
        .trim_start_matches("你要记住")
        .trim_matches(|c: char| {
            c.is_ascii_punctuation()
                || matches!(
                    c,
                    '“' | '”' | '‘' | '’' | '《' | '》' | '「' | '」' | '：' | ':'
                )
        });
    clean_text(cleaned, max_chars)
}

pub(super) fn push_if_present(out: &mut Vec<String>, value: &str) {
    if !value.is_empty() {
        out.push(value.to_string());
    }
}

/// 记忆文本敏感性与一次性任务判别器。供 io（落库前过滤）与 llm_review（候选清洗）
/// 共用——纯文本分类，行为与原 `mod.rs` 实现逐字一致。
pub(super) fn clean_memory_label(value: &str) -> Option<String> {
    let cleaned = value
        .trim()
        .trim_matches(|c: char| {
            c.is_ascii_punctuation()
                || matches!(
                    c,
                    '“' | '”' | '‘' | '’' | '《' | '》' | '「' | '」' | '：' | ':'
                )
        })
        .trim_end_matches(['吧', '呗', '哦', '哈', '啦', '呀'])
        .trim();
    let cleaned = clean_text(cleaned, 24);
    if cleaned.is_empty() || cleaned.chars().count() > 12 {
        return None;
    }
    if invalid_memory_label(&cleaned) || looks_sensitive_or_task_like(&cleaned) {
        return None;
    }
    Some(cleaned)
}

pub(super) fn invalid_memory_label(value: &str) -> bool {
    let value = clean_text(value, 24);
    if value.is_empty() {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    matches!(
        value.as_str(),
        "谁" | "什么" | "啥" | "哪位" | "哪个" | "哪里" | "哪儿" | "为什么" | "怎么"
    ) || value.contains('？')
        || value.contains('?')
        || value.ends_with('吗')
        || value.ends_with('呢')
        || lower == "who"
        || lower == "what"
        || lower == "which"
        || lower == "why"
        || lower == "how"
}

pub(super) fn looks_sensitive_or_task_like(value: &str) -> bool {
    looks_sensitive(value) || looks_task_like(value)
}

pub(super) fn looks_sensitive(value: &str) -> bool {
    let value = clean_text(value, 500);
    let lower = value.to_ascii_lowercase();
    let digit_count = value.chars().filter(|c| c.is_ascii_digit()).count();
    if digit_count >= 11 {
        return true;
    }
    if [
        "身份证",
        "手机号",
        "手机号码",
        "电话号码",
        "联系电话",
        "密码",
        "口令",
        "密钥",
        "私钥",
        "api_key",
        "apikey",
        "api key",
        "secret",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return true;
    }
    if lower.contains("token")
        && (lower.contains('=')
            || lower.contains(':')
            || value.contains('是')
            || value.contains('为'))
    {
        return true;
    }
    looks_like_url(&lower)
        || looks_like_email(&value)
        || looks_like_filesystem_path(&value)
        || looks_like_credential_assignment(&value)
}

pub(super) fn looks_like_url(lower: &str) -> bool {
    lower.contains("http://") || lower.contains("https://")
}

pub(super) fn looks_like_email(value: &str) -> bool {
    value
        .split(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '，' | '。' | '；' | ';' | ',' | '<' | '>' | '"' | '\'' | '(' | ')' | '[' | ']'
                )
        })
        .any(|token| {
            let Some((left, right)) = token.split_once('@') else {
                return false;
            };
            !left.is_empty() && right.contains('.') && right.len() >= 3
        })
}

pub(super) fn looks_like_filesystem_path(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let trimmed = lower.trim();
    let unix_roots = ["/home/", "/tmp/", "/users/", "/var/", "/etc/", "/opt/"];
    if unix_roots
        .iter()
        .any(|root| trimmed.starts_with(root) || lower.contains(root))
    {
        return true;
    }
    trimmed.starts_with("~/")
        || lower.contains("c:\\")
        || lower.contains("c:/")
        || lower.contains("\\users\\")
        || lower.contains("\\appdata\\")
}

pub(super) fn looks_like_credential_assignment(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let has_assignment =
        lower.contains('=') || lower.contains(':') || value.contains('是') || value.contains('为');
    has_assignment
        && [
            "token", "api_key", "apikey", "api key", "secret", "password", "passwd", "密钥",
            "私钥", "口令", "密码",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
}

pub(super) fn looks_task_like(value: &str) -> bool {
    let text = clean_text(value, 160);
    if text.is_empty() || looks_like_stable_instruction(&text) {
        return false;
    }
    let text = text.trim_start();
    if ["帮我", "请帮我", "麻烦", "麻烦你"]
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        return contains_one_off_action(text);
    }
    starts_with_one_off_action(text)
}

pub(super) fn looks_like_stable_instruction(value: &str) -> bool {
    [
        "以后",
        "默认",
        "每次",
        "总是",
        "回答时",
        "回复时",
        "生成报告时",
        "写报告时",
        "做文档时",
        "尽量",
        "不要",
        "别太",
        "优先",
        "习惯",
        "偏好",
        "风格",
        "先给",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

pub(super) fn starts_with_one_off_action(value: &str) -> bool {
    [
        "写", "查", "生成", "总结", "翻译", "安装", "打开", "搜索", "创建", "修复", "做", "整理",
        "规划",
    ]
    .iter()
    .any(|prefix| value.starts_with(prefix))
}

pub(super) fn contains_one_off_action(value: &str) -> bool {
    [
        "写", "查", "生成", "总结", "翻译", "安装", "打开", "搜索", "创建", "修复", "做", "整理",
        "规划",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

pub(super) fn looks_recent_work_status(value: &str) -> bool {
    looks_ongoing_work_status(value) || looks_completed_work_status(value)
}

pub(super) fn looks_ongoing_work_status(value: &str) -> bool {
    ["正在", "最近", "本周", "这周", "目前", "推进", "处理中"]
        .iter()
        .any(|needle| value.contains(needle))
}

pub(super) fn looks_completed_work_status(value: &str) -> bool {
    [
        "刚完成",
        "已完成",
        "完成了",
        "已生成",
        "生成了",
        "已实现",
        "实现了",
        "已修复",
        "修复了",
        "已交付",
        "交付了",
        "写完",
        "整理完",
        "已整理",
        "整理好了",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}
