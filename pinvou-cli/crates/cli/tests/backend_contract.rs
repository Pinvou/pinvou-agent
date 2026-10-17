//! Contract tests that pin the CLI's hand-copied parity with the app/backend
//! sources it mirrors (`pinvou3-app/src-tauri`), at the source level.
//!
//! The app crate is a separate workspace member this test binary cannot link
//! against, so parity is pinned the way `cli_contract.rs` pins in-crate
//! boundaries: `include_str!` the source (repo-root-relative, same depth as
//! the `../../../../docs` pin there) and parse the mirrored function bodies
//! out of both sides. A parse that comes up empty panics the test, so a
//! rename or a reformat that breaks the extraction fails loudly instead of
//! silently pinning nothing.

use std::collections::BTreeSet;

/// The `'\u{` prefix every explicit sanitizer entry is spelled with in the
/// two remaining copies; the parser below is tight to this literal form on
/// purpose.
const CHAR_LITERAL: &str = "'\\u{";

/// Extracts the body of the sanitizer named by its full signature text (the
/// full `fn name(params) -> bool {` spelling, so bare doc-comment mentions
/// of the name cannot match first) and parses every explicit `'\u{...}'`
/// literal and `'\u{...}'..='\u{...}'` range in it into a code-point set.
///
/// Deliberately tight to the spelling all three copies use today: each
/// function also folds in `char::is_control()`, which carries no
/// differential (all three include it), so only the literal alternation is
/// compared. Any new entry in a different textual form, a rename, or a
/// reformat that breaks the body extraction makes the parse panic — the
/// test fails, which is the pin working.
fn unsafe_char_set(source: &str, signature: &str, owner: &str) -> BTreeSet<char> {
    let body = source
        .split_once(signature)
        .and_then(|(_, tail)| tail.split_once("\n}"))
        .map(|(body, _)| body)
        .unwrap_or_else(|| {
            panic!("{owner}: sanitizer `{signature}` must remain present and parseable")
        });
    let mut set = BTreeSet::new();
    let mut rest = body;
    while let Some(position) = rest.find(CHAR_LITERAL) {
        rest = &rest[position..];
        let (start, after) = parse_char_literal(rest, owner);
        if let Some(after_arrow) = after.trim_start().strip_prefix("..=") {
            let (end, after_end) = parse_char_literal(after_arrow.trim_start(), owner);
            let span = (start as u32)..=(end as u32);
            set.extend(span.filter_map(char::from_u32));
            rest = after_end;
        } else {
            set.insert(start);
            rest = after;
        }
    }
    set
}

/// Parses one `'\u{HEX}'` literal at the start of `text`, returning the
/// character and the text after the closing quote.
fn parse_char_literal<'a>(text: &'a str, owner: &str) -> (char, &'a str) {
    let tail = text
        .strip_prefix(CHAR_LITERAL)
        .unwrap_or_else(|| panic!("{owner}: expected a char literal, got {text:?}"));
    let close = tail
        .find('}')
        .unwrap_or_else(|| panic!("{owner}: unterminated char literal {text:?}"));
    let code = u32::from_str_radix(&tail[..close], 16)
        .unwrap_or_else(|error| panic!("{owner}: bad hex code point {text:?}: {error}"));
    let after = tail[close + 1..]
        .strip_prefix('\'')
        .unwrap_or_else(|| panic!("{owner}: char literal missing closing quote: {text:?}"));
    (
        char::from_u32(code).unwrap_or_else(|| panic!("{owner}: not a scalar value: U+{code:04X}")),
        after,
    )
}

/// Renders a set as a stable, greppable U+ list for the failure messages.
fn code_points(chars: impl IntoIterator<Item = char>) -> String {
    chars
        .into_iter()
        .map(|ch| format!("U+{:04X}", ch as u32))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The display-hygiene set behind the app's stored display names, extracted
/// once. Repo-root-relative:
/// four `..` from this file's directory reach the repository root (same depth
/// as cli_contract.rs's `../../../../docs`).
fn app_display_unsafe_set() -> BTreeSet<char> {
    unsafe_char_set(
        include_str!("../../../../pinvou3-app/src-tauri/src/features/marketplace/store.rs"),
        "fn is_display_unsafe_char(c: char) -> bool {",
        "app store.rs",
    )
}

/// The CLI's import/display lane no longer hand-mirrors the app's
/// `features/marketplace/store.rs::is_display_unsafe_char`: it imports the
/// shared (exported) predicate, so set equality is compiler-guaranteed and a
/// GUI-side range change moves the CLI's stored-name hygiene with it. This
/// pin keeps that wiring honest: plugins.rs must consume the shared
/// `is_display_unsafe_char` import and must NOT re-declare a local copy (a
/// re-added local fn would break the build via the unused import, and this
/// assertion names the regression directly). The set-level drift protection
/// this test used to provide for the replica is obsolete by construction.
#[test]
fn plugins_display_sanitizer_is_the_shared_app_fn() {
    let plugins = include_str!("../src/plugins.rs");
    assert!(
        plugins.contains("is_display_unsafe_char"),
        "plugins.rs must import the shared display-hygiene predicate"
    );
    assert!(
        !plugins.contains("fn is_display_unsafe_char"),
        "plugins.rs must not re-declare a local is_display_unsafe_char mirror"
    );
    // The app-side set stays the anchor for the support.rs superset pin.
    assert!(
        !app_display_unsafe_set().is_empty(),
        "the app set must parse to a non-empty pin (extraction broken?)"
    );
}

/// `support.rs::is_row_unsafe_char` (whose comment admits it was copied from
/// the app set) is a DELIBERATE SUPERSET: the terminal-row threat model adds
/// the invisible formatting characters the GUI's card/composer surface does
/// not need to reject (LRM/RLM, word joiners, variation selectors, tag
/// characters, ...). The app entries must all be present — a GUI-added
/// character must never slip through the CLI's human rows — and the
/// superset must stay strict, so dropping the CLI's own extras fails too.
#[test]
fn row_sanitizer_set_is_a_strict_superset_of_the_app_set() {
    let app = app_display_unsafe_set();
    let rows = unsafe_char_set(
        include_str!("../src/support.rs"),
        "fn is_row_unsafe_char(ch: char) -> bool {",
        "support.rs",
    );
    assert!(
        !app.is_empty(),
        "the app set must parse to a non-empty pin (extraction broken?)"
    );
    let missing: Vec<_> = app.difference(&rows).collect();
    assert!(
        missing.is_empty(),
        "support.rs row-hygiene set lost app entries: {}",
        code_points(missing.into_iter().copied()),
    );
    assert_ne!(
        rows, app,
        "the row set must stay a STRICT superset: its terminal-only extras \
         (e.g. LRM/RLM, variation selectors) are the deliberate divergence"
    );
}
