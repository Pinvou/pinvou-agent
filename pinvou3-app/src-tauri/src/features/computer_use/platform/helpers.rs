//! Pure helpers shared by the per-OS backends (hoisted in the ninth review
//! round: the per-platform copies had drifted — Linux truncated names in a
//! different order, macOS scroll did not clamp, and drag interpolation was
//! inlined twice). No OS handles here: everything is a plain function so it
//! stays unit-testable on every target.

use super::super::guard::{T3_MATCH_WINDOW_CHARS, fold_for_matching, matches_t3_denylist_folded};
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

/// Sanitizer for accessible text that is shown to the user or the model:
/// quotes are straightened, every control character (newlines, tabs, C0/C1)
/// folds to a space, bidi and zero-width formatting characters are dropped,
/// and the result is truncated and trimmed. Mapping before truncation
/// guarantees a single clean line; the Linux copy used to truncate first,
/// which let a cut point preserve a line break.
///
/// The formatting characters are dropped rather than kept because this text
/// ends up in the consent dialog's "target element" line: a right-to-left
/// override (U+202E) in an attacker-controlled `aria-label` renders the label
/// the user is being asked to trust in reverse, and zero-width characters let
/// two different controls display identically.
pub(crate) fn sanitize_name(name: &str, max_chars: usize) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !is_invisible_formatting(*c))
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

/// Zero-width and bidi formatting characters: invisible to the user, but they
/// change how the surrounding text renders. `char::is_control` covers only
/// category Cc and lets every one of these through.
fn is_invisible_formatting(c: char) -> bool {
    matches!(c,
        '\u{00AD}'
        | '\u{061C}'
        | '\u{180E}'
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}'
        | '\u{FEFF}')
}

/// Runs the T3 denylist over the **whole** raw accessible name, in bounded
/// memory.
///
/// [`sanitize_name`] truncates to a short display line, so the denylist must
/// not be matched against it: a label an attacker controls (an `aria-label`,
/// a window title) can pad past that window so a consequential term never
/// reaches the matcher. The previous screening copy raised that window to
/// 1024 characters and shaped anything longer as head…tail, which left the
/// evasion intact for the price of ~512 characters of padding on each side of
/// the term.
///
/// This walks the raw text in fixed chunks instead, folding each chunk with
/// [`fold_for_matching`] and carrying [`T3_MATCH_WINDOW_CHARS`] - 1 folded
/// characters across the boundary so a term straddling two chunks is still
/// found. Length therefore no longer buys an evasion, while peak memory stays
/// proportional to the chunk rather than to the label.
pub(crate) fn screening_hit(raw: &str) -> bool {
    /// Raw characters folded per pass: large enough that the per-chunk
    /// overhead is irrelevant, small enough that a pathological
    /// multi-megabyte accessible name is never copied wholesale.
    const CHUNK_CHARS: usize = 4096;
    let carry = T3_MATCH_WINDOW_CHARS.saturating_sub(1);
    let mut folded = String::new();
    let mut chars = raw.chars();
    loop {
        let chunk: String = chars.by_ref().take(CHUNK_CHARS).collect();
        if chunk.is_empty() {
            return false;
        }
        folded.push_str(&fold_for_matching(&chunk));
        if matches_t3_denylist_folded(&folded) {
            return true;
        }
        let count = folded.chars().count();
        if count > carry {
            folded = folded.chars().skip(count - carry).collect();
        }
    }
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

/// The UTF-16 unit budget of one `CGEventKeyboardSetUnicodeString` call.
///
/// The API stores at most 20 UTF-16 units. enigo chunks the text by `char`
/// before calling it, which is only equivalent while every character is in the
/// BMP: 20 emoji are 40 UTF-16 units, so the OS keeps the first 20 and drops
/// the rest — and the split can land inside a surrogate pair. `type` returned
/// `Ok` regardless, telling the agent it had typed text that was silently cut
/// in half.
#[cfg(target_os = "macos")]
pub(crate) const MACOS_UNICODE_STRING_UTF16_UNITS: usize = 20;

/// Splits `text` so every chunk fits in `max_units` UTF-16 units, cutting only
/// on character boundaries.
///
/// Because a chunk of at most `max_units` UTF-16 units also has at most
/// `max_units` characters, a consumer that re-chunks by `char` at the same
/// bound (as enigo does) emits each chunk whole.
#[cfg(target_os = "macos")]
pub(crate) fn utf16_chunks(text: &str, max_units: usize) -> Vec<&str> {
    debug_assert!(max_units >= 2, "a single character can be two UTF-16 units");
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut units = 0usize;
    for (index, character) in text.char_indices() {
        let width = character.len_utf16();
        if units + width > max_units && index > start {
            chunks.push(&text[start..index]);
            start = index;
            units = 0;
        }
        units += width;
    }
    if start < text.len() {
        chunks.push(&text[start..]);
    }
    chunks
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

    #[cfg(target_os = "macos")]
    #[test]
    fn utf16_chunks_bound_the_unicode_string_budget() {
        // BMP text: identical to character chunking.
        assert_eq!(utf16_chunks("abcd", 2), vec!["ab", "cd"]);
        assert!(utf16_chunks("", 20).is_empty());
        // Non-BMP: each emoji is two UTF-16 units, so a 20-unit budget takes
        // ten of them per chunk — the case that used to lose half the text.
        let emoji = "\u{1F600}".repeat(25);
        let chunks = utf16_chunks(&emoji, MACOS_UNICODE_STRING_UTF16_UNITS);
        assert_eq!(chunks.len(), 3);
        for chunk in &chunks {
            let units: usize = chunk.chars().map(char::len_utf16).sum();
            assert!(
                units <= MACOS_UNICODE_STRING_UTF16_UNITS,
                "chunk over budget: {units}"
            );
            // A chunk within the UTF-16 budget is also within the character
            // budget, so a consumer re-chunking by char emits it whole.
            assert!(chunk.chars().count() <= MACOS_UNICODE_STRING_UTF16_UNITS);
        }
        assert_eq!(chunks.concat(), emoji, "no character may be dropped");
        // Mixed content still splits only on character boundaries.
        let mixed = "ab\u{1F600}cd\u{1F600}";
        assert_eq!(utf16_chunks(mixed, 4).concat(), mixed);
    }

    #[test]
    fn sanitize_name_drops_bidi_and_zero_width_characters() {
        // These survive `char::is_control` (category Cc only) and reach the
        // consent dialog's target line, where a right-to-left override
        // reverses the very label the user is being asked to trust.
        assert_eq!(
            sanitize_name("Cancel \u{202E}drawhtiw", 80),
            "Cancel drawhtiw"
        );
        assert_eq!(sanitize_name("De\u{200B}lete", 80), "Delete");
        assert_eq!(sanitize_name("D\u{00AD}elete", 80), "Delete");
        // Ordinary text and the existing control folding are unchanged.
        assert_eq!(sanitize_name("a\0b\tc", 80), "a b c");
        assert_eq!(sanitize_name("say \"hi\"", 80), "say 'hi'");
    }

    #[test]
    fn screening_hit_decides_short_names() {
        assert!(screening_hit("Pay now"));
        assert!(screening_hit("  Buy\nnow "));
        assert!(!screening_hit("Open settings"));
        assert!(!screening_hit(""));
    }

    #[test]
    fn screening_hit_survives_display_truncation_padding() {
        // The attack shape: an attacker-controlled label pads past the
        // 80-char display window so a consequential term never reaches the
        // matcher ("AAAA…A Pay now").
        let raw = format!("{} Pay now", "A".repeat(200));
        let display = sanitize_name(&raw, 80);
        assert_eq!(display.chars().count(), 80);
        assert!(
            !display.contains("Pay now"),
            "display must truncate: {display}"
        );
        assert!(screening_hit(&raw), "the raw label must still be screened");
    }

    #[test]
    fn screening_hit_ignores_label_length() {
        // The evasion the previous head…tail screening copy left open: with
        // ~512 characters of padding on each side, the term fell into the
        // elided middle and the label screened Clear. Length must buy nothing.
        for pad in [600usize, 4000, 20_000] {
            let raw = format!("{}Delete{}", "x".repeat(pad), "y".repeat(pad));
            assert!(screening_hit(&raw), "padding {pad} must not hide the term");
        }
    }

    #[test]
    fn screening_hit_finds_terms_across_chunk_boundaries() {
        // The streaming matcher folds 4096 raw characters at a time; a term
        // straddling that boundary is only found because the carry keeps the
        // tail of the previous chunk.
        for offset in 0.."Delete".len() {
            let raw = format!("{}Delete", "x".repeat(4096 - offset));
            assert!(screening_hit(&raw), "term split at offset {offset}");
        }
    }

    #[test]
    fn screening_hit_sees_through_invisible_and_confusable_characters() {
        // Every one of these rendered identically to the user and screened
        // Clear under the previous plain `to_lowercase().contains()` match.
        assert!(screening_hit("De\u{200B}lete"), "zero-width space");
        assert!(screening_hit("D\u{00AD}elete"), "soft hyphen");
        assert!(screening_hit("\u{202E}Delete"), "bidi override");
        assert!(screening_hit("Ｄｅｌｅｔｅ"), "fullwidth");
        assert!(screening_hit("Pаy now"), "Cyrillic a");
        assert!(screening_hit("支 付"), "space-split CJK");
        // The inverse hole: folding a C0 character to a space used to split a
        // term that the matcher would otherwise have found.
        assert!(screening_hit("D\u{0001}elete"), "C0 inside the term");
    }
}
