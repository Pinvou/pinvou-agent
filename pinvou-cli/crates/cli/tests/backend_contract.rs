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

/// The `'\u{` prefix every explicit sanitizer entry is spelled with in all
/// three copies; the parser below is tight to this literal form on purpose.
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
fn code_points(set: &BTreeSet<char>) -> String {
    set.iter()
        .map(|ch| format!("U+{:04X}", *ch as u32))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `plugins.rs::is_display_unsafe_char` documents itself as a mirror of the
/// app's `features/marketplace/store.rs::is_display_unsafe_char` (crate-
/// private there): the sets must be EQUAL, so a GUI-added invisible
/// character can never silently bypass the CLI's import/display rejection
/// (and vice versa — the CLI must not reject a name the GUI would store).
#[test]
fn plugins_display_sanitizer_set_equals_the_app_set() {
    // Repo-root-relative: four `..` from this file's directory reach the
    // repository root (same depth as cli_contract.rs's `../../../../docs`).
    let app = unsafe_char_set(
        include_str!("../../../../pinvou3-app/src-tauri/src/features/marketplace/store.rs"),
        "fn is_display_unsafe_char(c: char) -> bool {",
        "app store.rs",
    );
    let plugins = unsafe_char_set(
        include_str!("../src/plugins.rs"),
        "fn is_display_unsafe_char(c: char) -> bool {",
        "plugins.rs",
    );
    assert!(
        !app.is_empty(),
        "the app set must parse to a non-empty pin (extraction broken?)"
    );
    let missing: Vec<_> = app.difference(&plugins).collect();
    let extra: Vec<_> = plugins.difference(&app).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "plugins.rs display-hygiene set drifted from the app's set\n  app-only: {}\n  cli-only: {}",
        code_points(&missing.into_iter().cloned().collect()),
        code_points(&extra.into_iter().cloned().collect()),
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
    let app = unsafe_char_set(
        include_str!("../../../../pinvou3-app/src-tauri/src/features/marketplace/store.rs"),
        "fn is_display_unsafe_char(c: char) -> bool {",
        "app store.rs",
    );
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
        code_points(&missing.into_iter().cloned().collect()),
    );
    assert_ne!(
        rows, app,
        "the row set must stay a STRICT superset: its terminal-only extras \
         (e.g. LRM/RLM, variation selectors) are the deliberate divergence"
    );
}
