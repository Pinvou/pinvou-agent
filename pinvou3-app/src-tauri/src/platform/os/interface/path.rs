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
        assert_eq!(
            filesystem_path_identity_key("A.md") == filesystem_path_identity_key("a.md"),
            path_component_eq(OsStr::new("A"), "a"),
        );
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
    fn user_home_dir_returns_some_path() {
        assert!(!user_home_dir().as_os_str().is_empty());
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
}
