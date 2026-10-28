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

/// 折叠平台（macOS 默认 APFS 卷 / Windows NTFS）共用的卷拼写折叠。词法凭据
/// 判定必须与卷的真实折叠语义一致：Unicode 简单大小写折叠中，仅有的两个
/// 折叠为 ASCII 的非 ASCII 码点是 U+017F（ſ→s）与 U+212A（K→k）——大小写
/// 不敏感卷按同一折叠表把 `id_rſa` 解析到 `id_rsa`，纯 ASCII 折叠会漏过。
/// std 没有 case-fold API，显式替换这两个码点后做 Unicode 小写即可覆盖
/// 「黑名单常量均为纯 ASCII」场景的全部折叠拼写，不引入新依赖。非 ASCII
/// 拼写是罕见输入：函数体本身只在慢路径分配，热路径见下面两个谓语。
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn volume_fold_spelling(text: &str) -> String {
    text.replace('\u{017F}', "s")
        .replace('\u{212A}', "k")
        .to_lowercase()
}

/// `path_component_eq` 的折叠实现：双方皆 ASCII 时零分配快速路径（词法
/// 判定跑在 workspace 列表/搜索/变更热道上），否则整串折叠后比较。
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn volume_fold_eq(component: &OsStr, expected: &str) -> bool {
    let text = component.to_string_lossy();
    if text.is_ascii() && expected.is_ascii() {
        return text.eq_ignore_ascii_case(expected);
    }
    volume_fold_spelling(&text) == volume_fold_spelling(expected)
}

/// `path_component_prefix` 的折叠实现，快速路径同上（前缀长度切片上做
/// ASCII 无关比较，不构造新串）。
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

/// 组件序列版 `starts_with`：系统区前缀黑名单（`/etc/ssh` 等）按组件对
/// 组件经 [`path_component_eq`] 比较，走平台折叠语义——大小写不敏感卷会把
/// `/Etc/Sudoers` 解析到真文件，字节 `starts_with` 会让大写变体绕过系统区。
/// 组件边界同时比字节版更精确：`/etc/shadowed` 不再命中 `/etc/shadow`。
/// 前缀用尽即命中（嵌套语义）；被比较路径先耗尽分量则未命中；绝对前缀与
/// 相对路径在首个分量（RootDir 对普通名）上必然失配，天然 fail-closed。
pub fn path_starts_with_folded(path: &Path, prefix: &str) -> bool {
    let mut path_components = path.components();
    let mut prefix_components = Path::new(prefix).components();
    loop {
        match (prefix_components.next(), path_components.next()) {
            (None, _) => return true,
            (Some(expected), Some(actual)) => {
                if !path_component_eq(
                    actual.as_os_str(),
                    expected.as_os_str().to_string_lossy().as_ref(),
                ) {
                    return false;
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
        // 平台无关的纯函数,全平台同语义:Unicode 简单大小写折叠里仅
        // ſ(U+017F)与 KELVIN SIGN(U+212A)折叠为 ASCII,大小写不敏感卷
        // (APFS/NTFS)按同一折叠表把 `id_rſa` 解析到 `id_rsa`,折叠平台的
        // 词法判定必须识别这些拼写,否则凭据黑名单被非 ASCII 拼写绕过。
        // `std::str::to_lowercase` 不做这个折叠(ſ 已是小写),所以显式替换
        // 是承载语义的,不得「简化」掉。
        assert!(volume_fold_eq(OsStr::new("id_r\u{017F}a"), "id_rsa"));
        assert!(volume_fold_eq(OsStr::new("id_R\u{017F}A"), "id_rsa"));
        assert!(volume_fold_eq(OsStr::new(".\u{017F}sh"), ".ssh"));
        assert!(volume_fold_eq(OsStr::new(".\u{212A}ube"), ".kube"));
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
    fn path_starts_with_folded_follows_component_folding_and_boundaries() {
        // 前缀按分量比较走 path_component_eq 的平台语义:折叠平台识别
        // `/Etc/Sudoers`,字节精确平台(Linux)不识别;组件边界让
        // `/etc/shadowed` 不命中 `/etc/shadow`(字节 starts_with 会误报)。
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
}
