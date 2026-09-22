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

/// Wider screening copy of an accessible name for denylist matching.
/// [`sanitize_name`] truncates to a short display line; a label an attacker
/// controls (an `aria-label`, a window title) can pad past that window so a
/// consequential term never reaches the matcher ("AAAA…A Pay now" truncates
/// to "AAAA…A"). This copy control-folds the raw text like the display name
/// but keeps far more of it, shaped head…tail inside a bounded cap so the
/// memory cost stays bounded for large trees; the residual gap only opens
/// for names longer than the cap with the term buried past the tail window.
/// `None` when the display name already covers the whole folded text (the
/// common case), so screening can fall back to the display name.
pub(crate) fn screening_name(raw: &str, display: &str) -> Option<String> {
    const SCREENING_MAX_CHARS: usize = 1024;
    let folded: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let folded = folded.trim();
    if folded.chars().count() <= display.chars().count() {
        return None;
    }
    if folded.chars().count() <= SCREENING_MAX_CHARS {
        return Some(folded.to_string());
    }
    let head = SCREENING_MAX_CHARS / 2;
    let tail = SCREENING_MAX_CHARS - head - 1;
    let mut shaped: String = folded.chars().take(head).collect();
    shaped.push('…');
    shaped.extend(folded.chars().skip(folded.chars().count() - tail));
    Some(shaped)
}

/// Normalize CR/CRLF line breaks to `'\n'` for typed text, shared by the
/// per-OS backends (the per-platform copies had drifted: Windows normalized,
/// Linux folded, macOS injected the raw CR verbatim — where
/// CGEventKeyboardSetUnicodeString renders it as a line break, so CRLF text
/// double-broke/double-submitted). Borrowed when there is nothing to
/// normalize.
pub(crate) fn normalize_typed_newlines(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains('\r') {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
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

/// Appends the buffered text run to `runs`, chunked into [`TypeRun::Text`] pieces
/// (shared by the `'\n'`/`'\t'` arms and the trailing flush of [`split_type_runs`]).
fn push_text_runs(runs: &mut Vec<TypeRun>, buf: &str, chunk_chars: usize) {
    runs.extend(
        char_chunks(buf, chunk_chars)
            .into_iter()
            .map(|chunk| TypeRun::Text(chunk.to_string())),
    );
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
                    push_text_runs(&mut runs, &buf, chunk_chars);
                    buf.clear();
                }
                runs.push(TypeRun::Return);
            }
            '\t' => {
                if !buf.is_empty() {
                    push_text_runs(&mut runs, &buf, chunk_chars);
                    buf.clear()
                }
                runs.push(TypeRun::Tab);
            }
            _ => buf.push(ch),
        }
    }
    if !buf.is_empty() {
        push_text_runs(&mut runs, &buf, chunk_chars);
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
    fn normalize_typed_newlines_maps_cr_and_crlf() {
        // Borrowed when there is no CR.
        let plain = "abc";
        let borrowed = normalize_typed_newlines(plain);
        assert!(matches!(borrowed, std::borrow::Cow::Borrowed(_)));
        assert_eq!(borrowed.as_ref(), "abc");
        // CRLF collapses to one '\n' (not two).
        assert_eq!(normalize_typed_newlines("a\r\nb").as_ref(), "a\nb");
        // Lone CR maps too.
        assert_eq!(normalize_typed_newlines("a\rb").as_ref(), "a\nb");
        // Mixed forms all normalize.
        assert_eq!(
            normalize_typed_newlines("a\r\nb\rc\nd").as_ref(),
            "a\nb\nc\nd"
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

    #[test]
    fn screening_name_none_when_display_covers_all() {
        // The common case: the folded raw fits inside the display window —
        // no wider copy is needed.
        assert_eq!(screening_name("Pay now", "Pay now"), None);
        assert_eq!(screening_name("  Buy\nnow ", "Buy now"), None);
    }

    #[test]
    fn screening_name_survives_display_truncation_padding() {
        // The attack shape: an attacker-controlled label pads past the
        // 80-char display window so a consequential term never reaches the
        // matcher ("AAAA…A Pay now"). The wider copy must keep the tail (and
        // therefore the term) matchable, while staying bounded.
        let raw = format!("{} Pay now", "A".repeat(200));
        let display = sanitize_name(&raw, 80);
        assert_eq!(display.chars().count(), 80);
        assert!(
            !display.contains("Pay now"),
            "display must truncate: {display}"
        );
        let screening = screening_name(&raw, &display).expect("wider copy expected");
        assert!(
            screening.contains("Pay now"),
            "term must survive: {screening}"
        );
        // Under the cap the copy is the whole folded raw: nothing is lost.
        assert_eq!(screening, raw);
    }

    #[test]
    fn screening_name_shapes_oversized_names_head_tail() {
        // Beyond the cap the copy is shaped head…tail: bounded memory for
        // pathological labels, with both ends still matchable.
        let raw = format!("{}Pay now{}", "x".repeat(2000), "y".repeat(2000));
        let display = sanitize_name(&raw, 80);
        let screening = screening_name(&raw, &display).expect("wider copy expected");
        let count = screening.chars().count();
        assert!(count <= 1024, "bounded: {count}");
        assert!(screening.starts_with('x') && screening.ends_with('y'));
        assert!(screening.contains('…'), "elided middle marked: {screening}");
        // Control characters fold exactly like the display name.
        assert_eq!(screening_name("a\0b\tc", "a b c"), None);
    }
}
