use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn user_home_dir() -> PathBuf {
    super::super::platform::user_home_dir()
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    super::super::platform::platform_compat_path(value)
}

pub fn validate_upload_location(canon: &Path) -> Result<(), String> {
    super::super::platform::validate_upload_location(canon)
}

pub fn path_component_eq(component: &OsStr, expected: &str) -> bool {
    super::super::platform::path_component_eq(component, expected)
}

pub fn path_component_prefix(component: &OsStr, prefix: &str) -> bool {
    super::super::platform::path_component_prefix(component, prefix)
}

pub fn path_component_suffix(component: &OsStr, suffix: &str) -> bool {
    super::super::platform::path_component_suffix(component, suffix)
}

/// Stable identity key for paths already stored by the application. The platform adapter
/// applies only equivalences guaranteed by that OS (for example Windows separators and case).
pub fn filesystem_path_identity_key(path: &str) -> String {
    super::super::platform::filesystem_path_identity_key(path)
}

/// Component-aware "same as, or nested under" for two folded identity keys:
/// a bare `starts_with` would count `/a/bc` as nested in `/a/b`, so the
/// boundary has to be a separator. The POSIX root nests every absolute path.
///
/// Single source of truth for the folded-prefix predicate (review #463
/// round-8 elegance): project-root validation, the codex/session rebind
/// suffix matcher and the command-layer nesting rejection all need exactly
/// this rule, and three hand-rolled copies had already drifted apart. Keys —
/// not paths — go in, so callers keep their own component-cut arithmetic.
pub fn path_identity_is_same_or_nested(key: &str, base: &str) -> bool {
    // A trailing separator is noise on both sides (`/a/b` and `/a/b/` are the
    // same directory); normalising it here also keeps the POSIX-root case
    // below from being shadowed by an empty-trim mismatch.
    let key = key.strip_suffix('/').unwrap_or(key);
    let base = base.strip_suffix('/').unwrap_or(base);
    if key == base {
        return true;
    }
    if base.is_empty() {
        return key.starts_with('/');
    }
    key.starts_with(base) && key[base.len()..].starts_with('/')
}

/// Volume spelling fold shared by folding platforms (macOS default APFS
/// volumes / Windows NTFS). Lexical credential judgments must match the
/// volume's real folding semantics: in Unicode simple case folding, the
/// non-ASCII code points that fold to ASCII are U+017F (ſ→s) and U+212A
/// (K→k); NTFS resolves via the `$UpCase` table of Unicode uppercase
/// mappings and additionally folds U+0131 (ı) to `I` — case-insensitive
/// volumes resolve `id_rſa` and `ıd_rsa` to `id_rsa` per their own fold
/// tables, which a pure-ASCII fold would miss. std has no case-fold API, so
/// explicitly replacing these three code points and lowercasing covers every
/// folded spelling of the "blocklist constants are pure ASCII" scenario
/// without new dependencies. Non-ASCII spellings are rare inputs: the body
/// allocates only on the slow path; see the two predicates below for the hot
/// path. Note that folding ı is an over-fold on macOS (the APFS simple fold
/// table does not fold ı; that code point comes from NTFS `$UpCase`) — the
/// blocklist only gains matches, the direction is fail-closed, acceptable.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn volume_fold_spelling(text: &str) -> String {
    text.replace('\u{017F}', "s")
        .replace('\u{212A}', "k")
        .replace('\u{0131}', "i")
        .to_lowercase()
}

/// Fold implementation behind `path_component_eq`: zero-allocation fast
/// path when both sides are ASCII (the lexical judgment runs on the
/// workspace list/search/change hot paths), otherwise folds both strings
/// and compares.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn volume_fold_eq(component: &OsStr, expected: &str) -> bool {
    let text = component.to_string_lossy();
    if text.is_ascii() && expected.is_ascii() {
        return text.eq_ignore_ascii_case(expected);
    }
    volume_fold_spelling(&text) == volume_fold_spelling(expected)
}

/// Fold implementation behind `path_component_prefix`, fast path as above
/// (ASCII-insensitive compare on a prefix-length slice, no new string
/// built).
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn volume_fold_prefix(component: &OsStr, prefix: &str) -> bool {
    let text = component.to_string_lossy();
    if text.is_ascii() && prefix.is_ascii() {
        return text
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix));
    }
    volume_fold_spelling(&text).starts_with(&volume_fold_spelling(prefix))
}

/// Fold implementation behind `path_component_suffix`, fast path as above
/// (ASCII-insensitive compare on a suffix-length slice, no new string
/// built). With both sides ASCII every byte index is a char boundary and
/// the slice cannot panic; the `get` Option guards defensively against
/// non-ASCII slipping in.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn volume_fold_suffix(component: &OsStr, suffix: &str) -> bool {
    let text = component.to_string_lossy();
    if text.is_ascii() && suffix.is_ascii() {
        return text.len() >= suffix.len()
            && text
                .get(text.len() - suffix.len()..)
                .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix));
    }
    volume_fold_spelling(&text).ends_with(&volume_fold_spelling(suffix))
}

/// Component-sequence `starts_with`: the system-area prefix blocklist
/// (`/etc/ssh` etc.) is compared component by component through
/// [`path_component_eq`], following the platform fold semantics — a
/// case-insensitive volume resolves `/Etc/Sudoers` to the real file, where a
/// byte `starts_with` would let the uppercase variant bypass the system
/// area. Component boundaries are also more precise than the byte version:
/// `/etc/shadowed` no longer hits `/etc/shadow`. Exhausting the prefix is a
/// hit (nesting semantics); exhausting the compared path first is a miss;
/// an absolute prefix against a relative path necessarily mismatches on the
/// first component (RootDir vs an ordinary name), naturally fail-closed.
/// Entries whose last component carries `*` (list discipline for open
/// families like rotated logs that cannot be enumerated one by one) switch
/// to component-prefix matching via [`path_component_prefix`] at that
/// component position; exact entity entries stay exact — if `shadow` were
/// matched as a prefix wholesale, innocent same-named files like `shadowed`
/// would be blocked again (Round-9 review MAJOR). Note: this implementation
/// handles `*` per component position, so a `*` in a middle component also
/// matches by prefix (over-blocking direction, fail-closed); the list
/// discipline (only the last component may use `*`) is constrained by the
/// load-side discipline pin (`wildcard_entries_keep_list_discipline`), not
/// this function's runtime responsibility (Round-15 review: the doc
/// previously promised last-component-only, inconsistent with the
/// implementation).
pub fn path_starts_with_folded(path: &Path, prefix: &str) -> bool {
    let mut path_components = path.components();
    let mut prefix_components = Path::new(prefix).components();
    loop {
        match (prefix_components.next(), path_components.next()) {
            (None, _) => return true,
            (Some(expected), Some(actual)) => {
                let expected = expected.as_os_str().to_string_lossy();
                match expected.strip_suffix('*') {
                    Some(base) => {
                        if !path_component_prefix(actual.as_os_str(), base) {
                            return false;
                        }
                    }
                    None => {
                        if !path_component_eq(actual.as_os_str(), expected.as_ref()) {
                            return false;
                        }
                    }
                }
            }
            (Some(_), None) => return false,
        }
    }
}

/// The subdirectory content of `path` relative to `base`: containment uses
/// [`path_identity_is_same_or_nested`] on the folded identity keys, and the
/// suffix is cut by `base`'s component count so a nested path keeps its
/// original casing. `None` = not under `base`; an empty suffix = the path IS
/// `base`. An empty base or a `..`-bearing argument is also `None`: the
/// former would return the full absolute path as the "suffix" (and
/// `to.join(suffix)` with an absolute suffix REPLACES `to`), the latter would
/// cut an escaping suffix. Single source of truth for the three rebind
/// lanes' suffix cut (round-8 review should-fix 9): codex/ACP index records,
/// plain-chat binding sidecars and project roots all translate
/// `from`-prefixed paths the same way.
pub fn path_relative_suffix_under(path: &Path, base: &Path) -> Option<PathBuf> {
    // Sharp edges (review #464 follow-up, unreachable from today's callers):
    // an empty base passes the string gate (the empty key nests every
    // absolute POSIX/UNC key; a Windows drive-letter key does not start with
    // `/`) and would return the full absolute path as the "suffix" — and
    // `to.join(suffix)` with an absolute suffix REPLACES `to` downstream. A
    // `..`-bearing argument passes the string gate too, but the component cut
    // then yields an escaping suffix — the same blanket rule the
    // scheduled-workspace gate applies (round-26 minor 4). A `..`-bearing
    // BASE needs no arm of its own (round-13): the string gate only passes
    // when the path textually starts with the base, which then makes the
    // path itself `..`-bearing and `escapes(path)` refuses it first.
    if base.as_os_str().is_empty() {
        return None;
    }
    // Cheap string gate first (round-14): the common non-nested miss pays
    // no components walk; a `..` that passes it is refused right after.
    let path_key = filesystem_path_identity_key(&path.to_string_lossy());
    let base_key = filesystem_path_identity_key(&base.to_string_lossy());
    if !path_identity_is_same_or_nested(
        path_key.trim_end_matches('/'),
        base_key.trim_end_matches('/'),
    ) {
        return None;
    }
    let escapes = |candidate: &Path| {
        candidate
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    };
    if escapes(path) {
        return None;
    }
    Some(path.components().skip(base.components().count()).collect())
}

pub fn python_command() -> String {
    super::super::platform::python_command()
}

/// Null device path for external tools (git empty config etc.).
pub fn null_device() -> &'static str {
    super::super::platform::null_device()
}

pub fn configure_onnxruntime_dylib() -> Result<(), String> {
    super::super::platform::configure_onnxruntime_dylib()
}

pub fn obsidian_config_path() -> Option<PathBuf> {
    super::super::platform::obsidian_config_path()
}

/// Convert a filesystem path to the native form accepted by desktop applications.
pub fn external_application_path(path: &Path) -> PathBuf {
    platform_compat_path(&path.to_string_lossy())
}

/// Build a standards-compliant file URL after normalising platform-only path prefixes.
pub fn file_url_from_path(path: &Path) -> Result<tauri::Url, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join(path)
    };
    let native = external_application_path(&absolute);
    tauri::Url::from_file_path(&native).map_err(|_| format!("convert file url: {}", path.display()))
}

pub fn connector_cli_command(cli_bin: &str, program: &str) -> Command {
    super::super::platform::connector_cli_command(cli_bin, program)
}

pub fn apply_user_npm_prefix(cmd: &mut Command) {
    super::super::platform::apply_user_npm_prefix(cmd);
}

pub fn kill_pid_tree(pid: u32) {
    super::super::platform::kill_pid_tree(pid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_path_api_returns_pathbuf() {
        let p = platform_compat_path("/tmp/pinvou3-os-test");
        assert!(!p.as_os_str().is_empty());
    }

    #[test]
    fn path_identity_case_behavior_matches_component_comparison() {
        // On macOS the identity key stays case-sensitive on purpose: APFS's
        // default volume folds case, and canonicalize preserves the caller's
        // spelling there, so two casings of one directory must fail the grant
        // identity match closed rather than alias. Component comparison folds
        // there (credential blocklist must survive `.AWS`-style spellings),
        // so the two predicates deliberately diverge on darwin. Note the
        // canonicalize spelling is a platform detail no check may rely on:
        // every component compare goes through the folded predicate.
        if cfg!(target_os = "macos") {
            assert_ne!(
                filesystem_path_identity_key("A.md"),
                filesystem_path_identity_key("a.md"),
            );
            assert!(path_component_eq(OsStr::new("A"), "a"));
        } else {
            assert_eq!(
                filesystem_path_identity_key("A.md") == filesystem_path_identity_key("a.md"),
                path_component_eq(OsStr::new("A"), "a"),
            );
        }
        if std::path::MAIN_SEPARATOR == '\\' {
            assert_eq!(
                filesystem_path_identity_key(r"folder\file.md"),
                filesystem_path_identity_key("folder/file.md"),
            );
        } else {
            assert_ne!(
                filesystem_path_identity_key(r"folder\file.md"),
                filesystem_path_identity_key("folder/file.md"),
            );
        }
    }

    #[test]
    fn blocked_list_entries_fold_onto_themselves_for_every_entry() {
        // Round-12 review: the fold-semantics pin over the blocklist's full
        // entry set. Case-insensitive volumes (APFS/NTFS) resolve uppercase
        // spellings to the same on-disk file as the original spelling, so
        // the adapter-chosen folded compares (posix/windows
        // path_component_eq/prefix/suffix) must judge every entry's
        // uppercase variant a hit. The adapters' own selection branches run
        // on the macOS/Windows CI legs; this pins the fold primitive's
        // semantics across platforms for every list entry — missing any
        // entry on the folded path (or degrading the fold primitive to a
        // byte compare) turns red on every platform. Pure string logic, no
        // filesystem access.
        let entries: Vec<&str> = crate::platform::path_policy::BLOCKED_COMPONENTS
            .iter()
            .chain(crate::platform::path_policy::BLOCKED_COMPONENT_PREFIXES.iter())
            .chain(crate::platform::path_policy::BLOCKED_COMPONENT_SUFFIXES.iter())
            .copied()
            .collect();
        assert!(entries.len() >= 30, "黑名单条目数不应莫名缩水");
        for entry in &entries {
            let upper = entry.to_uppercase();
            assert!(
                volume_fold_eq(OsStr::new(upper.as_str()), entry),
                "折叠比较必须把大写变体 {upper} 判为命中 {entry}"
            );
        }
        for suffix in crate::platform::path_policy::BLOCKED_COMPONENT_SUFFIXES {
            assert!(
                volume_fold_suffix(OsStr::new("PRODUCTION.ENV"), suffix),
                "折叠后缀必须命中 PRODUCTION.ENV"
            );
            assert!(
                !volume_fold_suffix(OsStr::new("dotenv.md"), suffix),
                "折叠后缀不得误伤非 .env 文件"
            );
        }
    }

    #[test]
    fn path_component_prefix_semantics_follow_platform() {
        // The `.env.` component-prefix blocklist routes through the same
        // platform-aware compare as path_component_eq: folded where the
        // default volume folds case, byte-exact where it does not.
        assert!(path_component_prefix(
            OsStr::new(".env.production"),
            ".env."
        ));
        assert!(!path_component_prefix(OsStr::new(".environment"), ".env."));
        if cfg!(target_os = "macos") || std::path::MAIN_SEPARATOR == '\\' {
            assert!(path_component_prefix(
                OsStr::new(".ENV.PRODUCTION"),
                ".env."
            ));
        } else {
            assert!(!path_component_prefix(
                OsStr::new(".ENV.PRODUCTION"),
                ".env."
            ));
        }
    }

    #[test]
    fn volume_fold_matches_unicode_case_folding_onto_ascii() {
        // Platform-independent pure function, identical semantics on every
        // platform: in Unicode simple case folding only ſ (U+017F) and
        // KELVIN SIGN (U+212A) fold to ASCII, while NTFS's $UpCase
        // uppercase table additionally folds ı (U+0131) to `I`;
        // case-insensitive volumes (APFS/NTFS) resolve `id_rſa` and
        // `ıd_rsa` to `id_rsa` per their own fold tables, so the folding
        // platforms' lexical judgment must recognize these spellings or the
        // credential blocklist is bypassed by non-ASCII spellings.
        // `std::str::to_lowercase` performs none of these folds (ſ/ı are
        // already lowercase), so the explicit replacement carries the
        // semantics and must not be "simplified" away.
        assert!(volume_fold_eq(OsStr::new("id_r\u{017F}a"), "id_rsa"));
        assert!(volume_fold_eq(OsStr::new("id_R\u{017F}A"), "id_rsa"));
        assert!(volume_fold_eq(OsStr::new(".\u{017F}sh"), ".ssh"));
        assert!(volume_fold_eq(OsStr::new(".\u{212A}ube"), ".kube"));
        assert!(volume_fold_eq(OsStr::new("\u{0131}d_rsa"), "id_rsa"));
        assert!(volume_fold_eq(OsStr::new("\u{0131}D_RSA"), "id_rsa"));
        assert!(volume_fold_eq(OsStr::new(".AWS"), ".aws"));
        assert!(!volume_fold_eq(OsStr::new("id_rsa2"), "id_rsa"));
        assert!(
            !volume_fold_eq(OsStr::new("caf\u{e9}"), "cafe"),
            "non-ASCII spellings that fold to non-ASCII must not match ASCII names"
        );
        assert!(volume_fold_prefix(OsStr::new(".env.local"), ".env."));
        assert!(volume_fold_prefix(OsStr::new(".ENV.\u{212A}ey"), ".env."));
        assert!(!volume_fold_prefix(OsStr::new(".environment"), ".env."));
        assert!(!volume_fold_prefix(OsStr::new("notes.md"), ".env."));
    }

    #[test]
    fn volume_fold_spelling_expansion_is_pinned_directly() {
        // Round-7 review: the subtlest line of the fold table is the explicit
        // expansion of ı (U+0131) — `to_lowercase` returns it unchanged while
        // `.replace` expands it to `i`; İ (U+0130) must stay **un**folded:
        // its to_lowercase expands to `i` + U+0307 combining mark, which can
        // never equal or prefix-hit an ASCII name (fail-closed over-fold
        // direction). Previously this behavior was only exercised indirectly
        // through eq/prefix, with no direct assertion.
        assert_eq!(volume_fold_spelling("id_r\u{017F}a"), "id_rsa");
        assert_eq!(volume_fold_spelling("\u{0131}d_rsa"), "id_rsa");
        assert_eq!(
            volume_fold_spelling("\u{0130}"),
            "i\u{0307}",
            "İ must expand to i + combining dot, never a bare ASCII i"
        );
        assert_ne!(volume_fold_spelling("\u{0130}"), "i");
        assert_ne!(
            volume_fold_spelling("\u{0130}d_rsa"),
            "id_rsa",
            "the İ expansion must not fold onto the ASCII credential name"
        );
        assert_eq!(volume_fold_spelling(".AWS"), ".aws");
    }

    #[test]
    fn path_starts_with_folded_follows_component_folding_and_boundaries() {
        // The prefix compares component-wise through path_component_eq's
        // platform semantics: folding platforms recognize `/Etc/Sudoers`,
        // byte-exact platforms (Linux) do not; component boundaries keep
        // `/etc/shadowed` from hitting `/etc/shadow` (a byte starts_with
        // would false-positive).
        let folded = cfg!(target_os = "macos") || std::path::MAIN_SEPARATOR == '\\';
        assert_eq!(
            path_starts_with_folded(Path::new("/Etc/Sudoers"), "/etc/sudoers"),
            folded
        );
        assert!(path_starts_with_folded(
            Path::new("/etc/ssh/host.key"),
            "/etc/ssh/"
        ));
        assert!(path_starts_with_folded(
            Path::new("/etc/sudoers"),
            "/etc/sudoers"
        ));
        assert!(!path_starts_with_folded(
            Path::new("/etc/shadowed"),
            "/etc/shadow"
        ));
        assert!(!path_starts_with_folded(Path::new("/etc"), "/etc/sudoers"));
        assert!(!path_starts_with_folded(
            Path::new("etc/sudoers"),
            "/etc/sudoers"
        ));
        assert!(path_starts_with_folded(Path::new("/etc/"), "/etc/"));
    }

    #[test]
    fn path_identity_nesting_requires_a_component_boundary() {
        assert!(path_identity_is_same_or_nested("/a/b", "/a/b"));
        assert!(path_identity_is_same_or_nested("/a/b/c", "/a/b"));
        assert!(
            !path_identity_is_same_or_nested("/a/bc", "/a/b"),
            "sibling prefix must not count as nested"
        );
        assert!(!path_identity_is_same_or_nested("/a", "/a/b"));
        assert!(
            path_identity_is_same_or_nested("/a/b", "/a/b/"),
            "a trailing separator on the base does not change the answer"
        );
        assert!(path_identity_is_same_or_nested("/", "/"));
        assert!(
            path_identity_is_same_or_nested("/a", "/"),
            "the POSIX root nests every absolute path"
        );
    }

    #[test]
    fn path_relative_suffix_under_cuts_by_component_count() {
        let base = std::path::Path::new("/work/alpha");
        assert_eq!(
            path_relative_suffix_under(std::path::Path::new("/work/alpha/sub"), base),
            Some(std::path::PathBuf::from("sub"))
        );
        assert_eq!(
            path_relative_suffix_under(base, base),
            Some(std::path::PathBuf::new()),
            "the path IS the base: empty suffix"
        );
        assert_eq!(
            path_relative_suffix_under(std::path::Path::new("/work/alpha-beta"), base),
            None,
            "sibling prefix must not count as under"
        );
    }

    #[test]
    fn path_relative_suffix_under_refuses_parent_segments() {
        // Round-26 minor 4: a `..`-bearing stored binding passes the textual
        // nest test but must not translate — its suffix would carry the
        // parent segments into the destination and lexically escape it.
        let base = std::path::Path::new("/work/alpha");
        assert_eq!(
            path_relative_suffix_under(std::path::Path::new("/work/alpha/../../escape"), base),
            None,
            "parent segments must refuse the translation"
        );
        assert_eq!(
            path_relative_suffix_under(std::path::Path::new("/work/../work/alpha/sub"), base),
            None,
            "any parent segment refuses — the same blanket rule the scheduled-workspace gate applies"
        );
    }

    #[test]
    fn path_relative_suffix_under_refuses_empty_base_and_parent_components() {
        // review #464 follow-up: an empty base passes the string gate (the
        // empty key nests every absolute POSIX/UNC key; a Windows
        // drive-letter key does not start with `/`) and would return the
        // full absolute path as the "suffix" — and `to.join(suffix)` with an
        // absolute suffix REPLACES `to` downstream. A `..`-bearing argument
        // passes the string gate too, but the component cut yields an
        // escaping suffix. Both must be None.
        let base = std::path::Path::new("/work/alpha");
        assert_eq!(
            path_relative_suffix_under(
                std::path::Path::new("/work/alpha/sub"),
                std::path::Path::new("")
            ),
            None,
            "empty base"
        );
        assert_eq!(
            path_relative_suffix_under(std::path::Path::new("/work/alpha/../beta"), base),
            None,
            "a `..`-bearing path would cut an escaping suffix"
        );
        // A `..`-bearing base is covered without its own arm: the string
        // gate only passes when the path textually starts with it, which
        // makes the path `..`-bearing — refused above.
    }

    #[test]
    fn windows_identity_key_keeps_the_normalization_sensitive_verbatim_guard_pinned() {
        // windows_path.rs is cfg-gated out of non-Windows builds, so a
        // revert of the plain/verbatim namespace guard in the Windows
        // `filesystem_path_identity_key` would stay green on every other
        // CI leg (the behavioral regression next to the key only runs on
        // the Windows leg). This source pin holds the guard open
        // everywhere: for a Win32-normalization-sensitive path (any
        // component with a trailing `.`/space — plain `folder.\report.txt`
        // resolves to `folder\report.txt` while the verbatim spelling
        // keeps the literal `folder.` directory) the key must treat the
        // two namespaces differently: the PLAIN spelling is keyed by its
        // normalized form (components stripped, matching what Win32
        // resolves), and only the VERBATIM spelling keeps the unstripped
        // components under a `\\?/` namespace marker. Marking both sides
        // alike would re-collapse the spellings onto one key. Exact-text
        // pin per the repo's production-source pin convention.
        let src = include_str!("../windows/windows_path.rs");
        let key_start = src
            .find("pub fn filesystem_path_identity_key")
            .expect("the Windows identity key entry point must exist");
        let body = src[key_start..]
            .split("\npub fn ")
            .next()
            .expect("non-empty source slice");
        assert!(
            body.contains("has_win32_normalization_sensitive_component(path)"),
            "the identity key must branch on the Win32-normalization-sensitivity guard"
        );
        assert!(
            body.contains("if verbatim {"),
            "the key must distinguish the verbatim spelling before folding sensitive paths"
        );
        assert!(
            body.contains(r"\\?/{folded}"),
            "sensitive verbatim paths must keep the `\\?/` namespace marker in the key"
        );
        assert!(
            body.contains(r#"trim_end_matches(['.', ' '])"#),
            "sensitive plain paths must be keyed by the normalized form Win32 resolves to"
        );
        let guard = src
            .find("fn has_win32_normalization_sensitive_component")
            .expect("the sensitivity guard helper must exist");
        assert!(
            src[guard..].contains("ends_with('.')"),
            "the guard must flag trailing-dot components (Win32 normalization rewrites them)"
        );
        assert!(
            src[guard..].contains("ends_with(' ')"),
            "the guard must flag trailing-space components (Win32 normalization rewrites them)"
        );
    }
}
