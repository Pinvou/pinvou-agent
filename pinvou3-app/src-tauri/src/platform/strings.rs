//! UTF-8 安全的字符串截断，供 audit（审计字段 ≤600 字节）与 harness
//! （LLM prompt 各段字节上限）等处复用。
//!
//! 直接 `&s[..max_bytes]` 在 `max_bytes` 落在多字节字符（中文）中间时会 panic——
//! 角色产出几乎全是中文，曾导致 `build_review_prompt` 在 `spawn_blocking` 里
//! panic → turn 崩溃 → engine busy flag 永不复位 → 卡死（P0 根因）。所有按字节
//! 切中文的地方都必须走这里。

/// 按 UTF-8 char 边界向下取整截断 `s` 到 ≤ `max_bytes` 字节，返回前缀切片。
///
/// 收敛原 `harness::truncate_on_char_boundary` 与 `audit::clip`（硬编码 600），
/// 两者实现完全一致（`while end > 0 && !s.is_char_boundary(end) { end -= 1 }`），
/// 故语义零变化。
pub(crate) fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_returns_empty() {
        assert_eq!(truncate_utf8("", 0), "");
        assert_eq!(truncate_utf8("", 10), "");
    }

    #[test]
    fn all_ascii_truncates_at_byte_count() {
        assert_eq!(truncate_utf8("hello world", 5), "hello");
        // 整串不超过上限时原样返回。
        assert_eq!(truncate_utf8("abc", 100), "abc");
    }

    #[test]
    fn multibyte_truncates_when_max_is_on_boundary() {
        // max_bytes 恰为 char 边界时不应回退。
        let s = "abcd压"; // "abcd"=4 字节, '压'=3 字节
        assert_eq!(truncate_utf8(s, 4), "abcd");
        assert_eq!(truncate_utf8(s, 7), "abcd压");
    }

    #[test]
    fn max_inside_multibyte_char_falls_back_to_boundary() {
        // max_bytes 落在 '压'（3 字节）中间(5,6) → 回退到 4。
        let s = "abcd压";
        assert_eq!(truncate_utf8(s, 5), "abcd");
        assert_eq!(truncate_utf8(s, 6), "abcd");
        assert!(truncate_utf8(s, 5).is_char_boundary(truncate_utf8(s, 5).len()));
    }

    #[test]
    fn never_panics_on_chinese_char_boundary() {
        // 复现 P0 根因:大纲是中文,直接 &s[..2000] 会切在多字节字符中间 panic。
        // 构造一个 byte 2000 恰好落在某个 3 字节汉字中间的串。
        let mut s = String::from("# 中国人口发展趋势分析 — 大纲\n");
        while s.len() < 2100 {
            s.push_str("总人口降至14.09亿，老龄化压力持续加大；");
        }
        assert!(s.len() > 2000);
        let out = truncate_utf8(&s, 2000);
        assert!(out.len() <= 2000);
        assert!(s.starts_with(out));
        // 返回值本身必须是合法 UTF-8(能再被切片/打印而不 panic)。
        let _ = format!("{out}...");
    }

    #[test]
    fn audit_clip_600_semantics_preserved() {
        // 原 audit::clip 的 600 硬编码契约：中文字段截到 ≤600 字节且不切半字符。
        let s = "中".repeat(300); // 900 字节
        let c = truncate_utf8(&s, 600);
        assert!(c.len() <= 600);
        assert!(c.chars().all(|ch| ch == '中'));
        assert_eq!(truncate_utf8("short", 600), "short");
    }
}
