//! Pure helpers shared by the per-OS backends (hoisted in the ninth review
//! round: the per-platform copies had drifted — Linux truncated names in a
//! different order, macOS scroll did not clamp, and drag interpolation was
//! inlined twice). No OS handles here: everything is a plain function so it
//! stays unit-testable on every target.

use super::super::types::ScrollDirection;

/// Overflow guard on scroll clicks per call: enigo multiplies the click
/// count by `WHEEL_DELTA` (120) internally, so an unclamped count overflows
/// i32 (debug panic / release wrap-around). Matches the tool layer's
/// `MAX_SCROLL_AMOUNT` parse cap, which is the effective limit — this one
/// only bounds the backend math. Windows used to clamp here; macOS did not
/// clamp at all — both go through this now.
pub(crate) const MAX_SCROLL_CLICKS: u32 = 100;

/// Scroll direction → enigo (axis, signed clicks). enigo convention:
/// Vertical positive is down / negative up, Horizontal positive is right /
/// negative left.
pub(crate) fn map_scroll(direction: ScrollDirection, clicks: u32) -> (enigo::Axis, i32) {
    let clicks = i32::try_from(clicks.min(MAX_SCROLL_CLICKS)).unwrap_or(i32::MAX);
    match direction {
        ScrollDirection::Up => (enigo::Axis::Vertical, -clicks),
        ScrollDirection::Down => (enigo::Axis::Vertical, clicks),
        ScrollDirection::Left => (enigo::Axis::Horizontal, -clicks),
        ScrollDirection::Right => (enigo::Axis::Horizontal, clicks),
    }
}

/// Linear interpolation waypoints for a drag (includes the endpoint, excludes
/// the start) so the target receives enough motion events even when the
/// compositor coalesces absolute moves.
pub(crate) fn drag_waypoints(from: (i32, i32), to: (i32, i32), steps: usize) -> Vec<(i32, i32)> {
    let steps = steps.max(1);
    (1..=steps)
        .map(|i| {
            let t = i as f64 / steps as f64;
            let x = f64::from(from.0) + f64::from(to.0 - from.0) * t;
            let y = f64::from(from.1) + f64::from(to.1 - from.1) * t;
            (x.round() as i32, y.round() as i32)
        })
        .collect()
}

/// Element-name sanitizer for model-facing single-line tree text: quotes are
/// straightened, every control character (newlines, tabs, C0/C1) folds to a
/// space, the result is truncated and trimmed. Mapping before truncation
/// guarantees a single clean line; the Linux copy used to truncate first,
/// which let a cut point preserve a line break.
pub(crate) fn sanitize_name(name: &str, max_chars: usize) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() {
                ' '
            } else if c == '"' {
                '\''
            } else {
                c
            }
        })
        .take(max_chars)
        .collect();
    cleaned.trim().to_string()
}

/// The chunk granularity of type injection (in characters): single-event backends (Windows
/// SendInput batch, macOS CGEvent batch) have no between-event cancellation checkpoint, and
/// a whole-text `enigo.text()` can legitimately exceed the call budget under low-level
/// keyboard hooks (AV/anti-keylogger products process each event synchronously) — chunking
/// plus between-chunk cancellation checks shrink the upper bound of zombie injection after
/// a caller timeout from the whole text to one chunk (round-12 review M5). A 64-character
/// chunk is roughly 6s under the slowest reasonable 50ms/event hook, negligible against the
/// 700s budget.
pub(crate) const TYPE_CHUNK_CHARS: usize = 64;

/// Split the text into chunks of at most `chunk_chars` characters (by character, not by
/// byte). Empty input produces an empty vector (the caller's loop body never runs,
/// consistent with enigo's no-op on empty text).
/// One segment of a split type request (review finding, Windows: enigo's
/// `text()` queues BOTH a Return/Tab key click and the Unicode control
/// character for `'\n'`/`'\t'`, so multi-line text double-injected
/// newlines on targets that handle both). Newlines and tabs become real key
/// clicks; everything else stays Unicode text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypeRun {
    /// Unicode text (already chunked).
    Text(String),
    /// A real Return key click for `'\n'`.
    Return,
    /// A real Tab key click for `'\t'`.
    Tab,
}

/// Splits a type request into chunked text runs and explicit Return/Tab key
/// clicks. Chunking applies per text run, so a text with many newlines keeps
/// the between-run cancellation granularity.
pub(crate) fn split_type_runs(text: &str, chunk_chars: usize) -> Vec<TypeRun> {
    debug_assert!(chunk_chars > 0);
    let mut runs = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        match ch {
            '\n' => {
                if !buf.is_empty() {
                    runs.extend(
                        char_chunks(&buf, chunk_chars)
                            .into_iter()
                            .map(|chunk| TypeRun::Text(chunk.to_string())),
                    );
                    buf.clear();
                }
                runs.push(TypeRun::Return);
            }
            '\t' => {
                if !buf.is_empty() {
                    runs.extend(
                        char_chunks(&buf, chunk_chars)
                            .into_iter()
                            .map(|chunk| TypeRun::Text(chunk.to_string())),
                    );
                    buf.clear()
                }
                runs.push(TypeRun::Tab);
            }
            _ => buf.push(ch),
        }
    }
    if !buf.is_empty() {
        runs.extend(
            char_chunks(&buf, chunk_chars)
                .into_iter()
                .map(|chunk| TypeRun::Text(chunk.to_string())),
        );
    }
    runs
}

pub(crate) fn char_chunks(text: &str, chunk_chars: usize) -> Vec<&str> {
    debug_assert!(chunk_chars > 0);
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut count = 0usize;
    for (index, _) in text.char_indices() {
        if count == chunk_chars {
            chunks.push(&text[start..index]);
            start = index;
            count = 0;
        }
        count += 1;
    }
    if start < text.len() {
        chunks.push(&text[start..]);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_waypoints_interpolates_and_excludes_start() {
        let pts = drag_waypoints((0, 0), (100, 50), 4);
        assert_eq!(pts, vec![(25, 13), (50, 25), (75, 38), (100, 50)]);
        assert_eq!(drag_waypoints((7, 7), (9, 9), 0).len(), 1);
        assert_eq!(drag_waypoints((7, 7), (9, 9), 0)[0], (9, 9));
    }

    #[test]
    fn map_scroll_clamps_and_signs() {
        assert_eq!(
            map_scroll(ScrollDirection::Up, 3),
            (enigo::Axis::Vertical, -3)
        );
        assert_eq!(
            map_scroll(ScrollDirection::Right, 3),
            (enigo::Axis::Horizontal, 3)
        );
        assert_eq!(
            map_scroll(ScrollDirection::Down, 1_000),
            (enigo::Axis::Vertical, MAX_SCROLL_CLICKS as i32)
        );
    }

    #[test]
    fn sanitize_name_folds_controls_and_trims() {
        assert_eq!(sanitize_name("a\"b\nc\td\u{1}e ", 20), "a'b c d e");
        assert_eq!(sanitize_name("abcdef", 3), "abc");
        assert_eq!(sanitize_name("  \n x", 10), "x");
    }

    #[test]
    fn split_type_runs_extracts_real_keys_and_chunks_text() {
        use TypeRun::*;
        assert!(split_type_runs("", 4).is_empty());
        assert_eq!(split_type_runs("abc", 4), vec![Text("abc".into())]);
        // Multi-line: each newline becomes a Return click; text around it is
        // preserved (an empty trailing segment adds nothing, so "a\n" still
        // submits the "a" line).
        assert_eq!(
            split_type_runs("a\nb\n", 8),
            vec![Text("a".into()), Return, Text("b".into()), Return]
        );
        // Tabs split too.
        assert_eq!(
            split_type_runs("x\ty", 8),
            vec![Text("x".into()), Tab, Text("y".into())]
        );
        // Chunking applies per run, not across key clicks.
        assert_eq!(
            split_type_runs("abcd\nef", 2),
            vec![
                Text("ab".into()),
                Text("cd".into()),
                Return,
                Text("ef".into())
            ]
        );
        // Unicode content survives the split on char boundaries.
        assert_eq!(
            split_type_runs("中文\n测试", 2),
            vec![Text("中文".into()), Return, Text("测试".into())]
        );
    }

    #[test]
    fn char_chunks_splits_on_char_boundaries() {
        assert!(char_chunks("", 4).is_empty());
        assert_eq!(char_chunks("ab", 4), vec!["ab"]);
        assert_eq!(char_chunks("abcd", 2), vec!["ab", "cd"]);
        assert_eq!(char_chunks("abcde", 2), vec!["ab", "cd", "e"]);
        // Multi-byte characters must not be split: a 3-character chunk is equally safe for
        // 4-byte CJK.
        assert_eq!(char_chunks("中文测试", 3), vec!["中文测", "试"]);
    }
}
