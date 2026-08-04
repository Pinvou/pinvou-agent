//! Low-level shared primitives for the memory feature: text sanitization,
//! stable id hashing, time parsing, and atomic JSONL writes.
//!
//! 抽离自 `mod.rs`——这些 helper 被实体存储（io）、LLM 审查（llm_review）与
//! 渲染（render）多处复用，集中放这里避免循环依赖。纯函数，行为与原实现一致。

use std::fs;
use std::io;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;

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

pub(super) fn write_text_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, text)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(err) if path.exists() => {
            let _ = fs::remove_file(path);
            fs::rename(tmp, path).map_err(|rename_err| {
                io::Error::new(
                    rename_err.kind(),
                    format!("replace after rename failed ({err}); {rename_err}"),
                )
            })?;
            Ok(())
        }
        Err(err) => Err(err),
    }
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
