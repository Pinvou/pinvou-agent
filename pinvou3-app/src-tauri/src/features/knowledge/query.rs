//! 自然语言 → 结构化查询（零模型，纯规则）。见 docs §4.3：
//! 「上周的保险 pdf」「大于10mb的视频」「最近7天 png」这类先走规则解析成
//! `SearchQuery`（exts / mtime_after / size 过滤 + 残余文本），不路由进 35B。
//!
//! 全程在小写副本上处理：trigram/LIKE 都大小写不敏感，残余文本用小写无碍。

use super::store::SearchQuery;

const MIN_KEYWORDS: &[&str] = &["大于", "超过", "至少", ">", "≥"];
const MAX_KEYWORDS: &[&str] = &["小于", "不到", "最多", "<", "≤"];

/// 类型关键词 → 扩展名集合。
const TYPE_KEYWORDS: &[(&str, &[&str])] = &[
    ("截图", &["png", "jpg", "jpeg"]),
    (
        "图片",
        &["jpg", "jpeg", "png", "gif", "webp", "bmp", "heic", "svg"],
    ),
    ("照片", &["jpg", "jpeg", "png", "heic"]),
    ("image", &["jpg", "jpeg", "png", "gif", "webp", "bmp"]),
    ("视频", &["mp4", "mov", "mkv", "avi", "webm", "flv"]),
    ("video", &["mp4", "mov", "mkv", "avi", "webm"]),
    ("音频", &["mp3", "wav", "flac", "m4a", "aac", "ogg"]),
    ("音乐", &["mp3", "flac", "wav", "m4a"]),
    ("audio", &["mp3", "wav", "flac", "m4a"]),
    ("word", &["docx", "doc"]),
    ("excel", &["xlsx", "xls", "csv"]),
    ("表格", &["xlsx", "xls", "csv"]),
    ("演示文稿", &["pptx", "ppt"]),
    ("幻灯", &["pptx", "ppt"]),
    ("ppt", &["pptx", "ppt"]),
    ("演示", &["pptx", "ppt"]),
];

/// 裸扩展名词（需词边界，避免 "md" 命中 "command"）。
const COMMON_EXTS: &[&str] = &[
    "pdf", "docx", "doc", "xlsx", "xls", "pptx", "ppt", "txt", "md", "csv", "json", "png", "jpg",
    "jpeg", "gif", "mp4", "mov", "mp3", "zip", "rar",
];

/// 把自然语言解析成结构化查询。识别不到的部分原样留在 `text` 里走 FTS。
pub fn parse(input: &str) -> SearchQuery {
    let mut q = SearchQuery::default();
    let mut work = input.to_lowercase();

    if let Some((v, m)) = detect_size(&work, MIN_KEYWORDS) {
        q.min_size = Some(v);
        work = work.replacen(&m, " ", 1);
    }
    if let Some((v, m)) = detect_size(&work, MAX_KEYWORDS) {
        q.max_size = Some(v);
        work = work.replacen(&m, " ", 1);
    }
    if let Some((after, m)) = detect_time(&work) {
        q.mtime_after = Some(after);
        work = work.replacen(&m, " ", 1);
    }

    for (kw, exts) in TYPE_KEYWORDS {
        if work.contains(kw) {
            q.exts.extend(exts.iter().map(|e| e.to_string()));
            work = work.replace(kw, " ");
        }
    }
    for e in COMMON_EXTS {
        let removed = remove_word(&work, e);
        if removed != work {
            q.exts.push((*e).to_string());
            work = removed;
        }
    }
    // 去重保序
    let mut seen = std::collections::HashSet::new();
    q.exts.retain(|e| seen.insert(e.clone()));

    // 残余连接词清理
    for c in ["的", "最近", "过去", "所有"] {
        work = work.replace(c, " ");
    }
    let text = work.split_whitespace().collect::<Vec<_>>().join(" ");
    q.text = if text.is_empty() { None } else { Some(text) };
    q
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn detect_size(work: &str, keywords: &[&str]) -> Option<(u64, String)> {
    for kw in keywords {
        if let Some(pos) = work.find(kw) {
            if let Some((val, end)) = parse_num_unit(work, pos + kw.len()) {
                return Some((val, work[pos..end].to_string()));
            }
        }
    }
    None
}

/// 解析 `[空格]数字[空格]单位`，返回 (字节数, 结束 byte 索引)。**必须有单位**，否则
/// 返回 None（避免把无关数字当大小过滤）。
fn parse_num_unit(s: &str, start: usize) -> Option<(u64, usize)> {
    let b = s.as_bytes();
    let mut i = start;
    while i < b.len() && b[i] == b' ' {
        i += 1;
    }
    let num_start = i;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
        i += 1;
    }
    if i == num_start {
        return None;
    }
    let num: f64 = s[num_start..i].parse().ok()?;
    while i < b.len() && b[i] == b' ' {
        i += 1;
    }
    let unit_start = i;
    while i < b.len() && b[i].is_ascii_alphabetic() {
        i += 1;
    }
    let mut unit = s[unit_start..i].to_string();
    if unit.is_empty() && s[i..].starts_with('兆') {
        unit = "mb".into();
        i += '兆'.len_utf8();
    }
    let mult: f64 = match unit.as_str() {
        "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "g" | "gb" => 1024.0 * 1024.0 * 1024.0,
        _ => return None, // 无/未知单位 → 不当 size 过滤
    };
    Some(((num * mult) as u64, i))
}

const FIXED_TIME: &[(&str, i64)] = &[
    ("前天", 3),
    ("昨天", 2),
    ("今天", 1),
    ("上个月", 30),
    ("最近一个月", 30),
    ("近一个月", 30),
    ("上月", 30),
    ("这个月", 30),
    ("本月", 30),
    ("最近一周", 7),
    ("近一周", 7),
    ("上周", 7),
    ("本周", 7),
    ("这周", 7),
    ("一星期", 7),
    ("今年", 365),
    ("本年", 365),
    ("yesterday", 2),
    ("today", 1),
    ("last week", 7),
    ("last month", 30),
];

fn detect_time(work: &str) -> Option<(i64, String)> {
    for (p, days) in FIXED_TIME {
        if let Some(pos) = work.find(p) {
            return Some((now() - days * 86400, work[pos..pos + p.len()].to_string()));
        }
    }
    // 数字 + 天/日/days
    let b = work.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let num: i64 = work[start..i].parse().ok()?;
            let mut j = i;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            for (suffix, slen) in [
                ("天", '天'.len_utf8()),
                ("日", '日'.len_utf8()),
                ("days", 4),
                ("day", 3),
            ] {
                if work[j..].starts_with(suffix) {
                    return Some((now() - num * 86400, work[start..j + slen].to_string()));
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

/// 删除 `w` 的首个**词边界**出现（两侧非字母数字）。无则原样返回。
fn remove_word(s: &str, w: &str) -> String {
    for (i, _) in s.match_indices(w) {
        let before_ok = i == 0 || !s.as_bytes()[i - 1].is_ascii_alphanumeric();
        let after = i + w.len();
        let after_ok = after >= s.len() || !s.as_bytes()[after].is_ascii_alphanumeric();
        if before_ok && after_ok {
            let mut out = String::with_capacity(s.len());
            out.push_str(&s[..i]);
            out.push(' ');
            out.push_str(&s[after..]);
            return out;
        }
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_and_time_and_residual() {
        let q = parse("上周的保险 pdf");
        assert_eq!(q.exts, vec!["pdf"]);
        assert!(q.mtime_after.is_some());
        assert_eq!(q.text.as_deref(), Some("保险"));
    }

    #[test]
    fn size_and_type() {
        let q = parse("大于10mb的视频");
        assert_eq!(q.min_size, Some(10 * 1024 * 1024));
        assert!(q.exts.contains(&"mp4".to_string()));
        assert_eq!(q.text, None);
    }

    #[test]
    fn numeric_days_and_bare_ext() {
        let q = parse("最近7天 png");
        assert!(q.mtime_after.is_some());
        assert_eq!(q.exts, vec!["png"]);
        assert_eq!(q.text, None);
    }

    #[test]
    fn small_size_with_k_unit() {
        let q = parse("小于500k 的 md");
        assert_eq!(q.max_size, Some(500 * 1024));
        assert_eq!(q.exts, vec!["md"]);
    }

    #[test]
    fn plain_text_untouched() {
        let q = parse("季度报告");
        assert_eq!(q.text.as_deref(), Some("季度报告"));
        assert!(q.exts.is_empty());
        assert!(q.mtime_after.is_none());
        assert!(q.min_size.is_none());
    }

    #[test]
    fn bare_ext_not_matched_inside_word() {
        // "md" 不应命中 "command" 里的子串
        let q = parse("command 手册");
        assert!(q.exts.is_empty());
        assert!(q.text.as_deref().unwrap().contains("command"));
    }
}
