//! Session id, workspace, and path validators plus the small persistence
//! helpers shared across the session store submodules.
//!
//! These free functions are intentionally side-effect-free (validation) or
//! trivially derivable (path composition / system-prompt flattening) so that
//! every other submodule can depend on them without introducing cycles.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use deepseek_tui::models::SystemPrompt;
use deepseek_tui::session_manager::SessionManager;

/// 生成 URL-safe session id（短 8 字节 timestamp + nanos hash）。
/// 上游 `validated_session_path` 只允许 `[A-Za-z0-9_-]`，所以走 base32-like 字符集。
pub(crate) fn validate_session_id(id: &str) -> Result<()> {
    if id.trim().is_empty()
        || !id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        bail!("Invalid session id '{id}'");
    }
    Ok(())
}

pub(crate) fn validate_scheduled_session_id(id: &str) -> Result<()> {
    validate_session_id(id)?;
    if !id.starts_with("sched-") {
        bail!("Scheduled session id must start with 'sched-': {id}");
    }
    Ok(())
}

pub(crate) fn validate_scheduled_task_id(id: &str) -> Result<()> {
    if id.trim().is_empty()
        || !id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        bail!("Invalid scheduled task id '{id}'");
    }
    Ok(())
}

pub(crate) fn validate_scheduled_workspace_path(root: &Path, workspace: &Path) -> Result<()> {
    if !workspace.is_absolute() {
        bail!(
            "Scheduled profile workspace must be absolute: {}",
            workspace.display()
        );
    }
    if workspace
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!(
            "Scheduled profile workspace must not contain parent segments: {}",
            workspace.display()
        );
    }
    if !workspace.starts_with(root) {
        bail!(
            "Scheduled profile workspace must live under {}: {}",
            root.display(),
            workspace.display()
        );
    }
    if workspace.file_name().and_then(|name| name.to_str()) != Some("workspace") {
        bail!(
            "Scheduled profile workspace must end with 'workspace': {}",
            workspace.display()
        );
    }
    Ok(())
}

/// Validates the working directory a user selected for a plain chat session:
/// non-empty, absolute, must exist on disk and be a directory. Returns the
/// canonicalized path (removing `.`, symlinks, and trailing separators); callers
/// use the return value for both binding and display.
pub(crate) fn validate_user_workspace_path(raw: &str) -> Result<PathBuf> {
    if raw.trim().is_empty() {
        bail!("Workspace path must not be empty");
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        bail!("Workspace path must be absolute: {raw}");
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Workspace path does not exist: {raw}"))?;
    if !canonical.is_dir() {
        bail!("Workspace path must be a directory: {raw}");
    }
    // Windows canonicalize returns a \?\ verbatim prefix; agents and the
    // frontend misjudge cwd in string/prefix comparisons, so normalize to a
    // regular drive path (identity on non-Windows) — the same invariant as
    // validate_codex_project_workspace.
    Ok(crate::platform::os::platform_compat_path(
        &canonical.to_string_lossy(),
    ))
}

/// Input validation for the keychain snapshot (§6): every additional root
/// must be absolute (hard reject, same gate as cwd); nonexistent/non-
/// directory roots only log a soft warning — following rebind's established
/// pattern, the target directory may be moved away and recreated later, the
/// snapshot faithfully records the user's choice at the time, and the
/// engine-side write exemption for a nonexistent root naturally lapses.
/// Successfully canonicalized roots use the canonical form (resolving
/// symlink/verbatim prefixes, same invariant as
/// validate_user_workspace_path); failures keep the lexical original.
pub(crate) fn validate_workspace_roots(raw: Vec<String>) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::with_capacity(raw.len());
    for entry in raw {
        let path = PathBuf::from(&entry);
        if !path.is_absolute() {
            return Err(format!("workspace root must be absolute: {entry}"));
        }
        match path.canonicalize() {
            Ok(canonical) if canonical.is_dir() => {
                roots.push(crate::platform::os::platform_compat_path(
                    &canonical.to_string_lossy(),
                ));
            }
            _ => {
                eprintln!(
                    "[sessions] workspace root not an existing directory (kept as-is): {entry}"
                );
                roots.push(path);
            }
        }
    }
    Ok(roots)
}

pub(crate) fn persisted_system_prompt(system_prompt: Option<&SystemPrompt>) -> Option<String> {
    match system_prompt {
        Some(SystemPrompt::Text(text)) => Some(text.clone()),
        Some(SystemPrompt::Blocks(blocks)) => Some(
            blocks
                .iter()
                .map(|block| block.text.clone())
                .collect::<Vec<_>>()
                .join("\n\n---\n\n"),
        ),
        None => None,
    }
}

pub(crate) fn chat_session_file(manager: &SessionManager, id: &str) -> Result<PathBuf> {
    validate_session_id(id)?;
    Ok(manager.sessions_dir().join(format!("{id}.json")))
}

pub(crate) fn scheduled_session_file(manager: &SessionManager, id: &str) -> Result<PathBuf> {
    validate_scheduled_session_id(id)?;
    Ok(manager.sessions_dir().join(format!("{id}.json")))
}

pub(crate) fn generate_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    const ALPHA: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut n = nanos;
    let mut buf = String::with_capacity(13);
    for _ in 0..13 {
        buf.push(ALPHA[(n % 36) as usize] as char);
        n /= 36;
    }
    buf
}

#[cfg(test)]
mod workspace_roots_tests {
    //! `validate_workspace_roots` is the security-relevant validation of a
    //! new input surface (§6 keychain snapshot): relative paths are hard-
    //! rejected, nonexistent/non-directory roots are soft-kept, existing
    //! directories are canonicalized.
    use super::*;

    fn unique_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pinvou3-roots-validate-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn rejects_relative_roots() {
        let error = validate_workspace_roots(vec!["relative/x".to_string()])
            .expect_err("relative root must be rejected");
        assert!(error.contains("must be absolute"), "{error}");
        // Mixing in a legal root does not let it through: any relative path
        // rejects the whole batch.
        let abs = std::env::temp_dir().to_string_lossy().into_owned();
        assert!(validate_workspace_roots(vec![abs, "x".to_string()]).is_err());
    }

    #[test]
    fn empty_input_is_single_root_semantics() {
        assert_eq!(
            validate_workspace_roots(Vec::new()).unwrap(),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn existing_dirs_are_canonicalized() {
        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        // Trailing `.` and separator spellings are normalized by
        // canonicalize (symlink/verbatim prefix stripping is covered by the
        // platform helper; this locks the lexical normalization).
        let spelled = format!("{}/./", real.display());
        let roots = validate_workspace_roots(vec![spelled]).expect("valid");
        // The expected side goes through the same projection as production
        // (canonicalize + platform_compat_path), so Windows verbatim `\?\`
        // prefixes are stripped on both sides of the comparison.
        let expected = crate::platform::os::platform_compat_path(
            &real.canonicalize().unwrap().to_string_lossy(),
        );
        assert_eq!(roots, vec![expected]);
    }

    #[test]
    fn symlink_spelling_resolves_to_canonical_form() {
        // Same shape as macOS /var→/private/var: a symlink spelling is
        // stored in canonical form, consistent with the project layer's
        // stored-value identity keys. Directory symlinks on Windows need
        // privileges; unix/macOS cover this (no cfg: architecture-guard).
        if std::env::consts::OS == "windows" {
            return;
        }
        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = temp.path().join("link");
        let status = std::process::Command::new("ln")
            .arg("-s")
            .arg(&real)
            .arg(&link)
            .status()
            .expect("spawn ln");
        assert!(status.success());
        let roots = validate_workspace_roots(vec![link.to_string_lossy().into_owned()])
            .expect("symlink root kept");
        assert_eq!(roots, vec![real.canonicalize().unwrap()]);
    }

    #[test]
    fn missing_or_file_roots_are_soft_kept_lexically() {
        // Soft keep: a nonexistent additional root is kept at its lexical
        // value (the target may be recreated later).
        let missing = unique_dir("missing");
        let roots =
            validate_workspace_roots(vec![missing.to_string_lossy().into_owned()]).expect("kept");
        assert_eq!(roots, vec![missing.clone()]);

        // Existing but not a directory is equally soft-kept (lexical value,
        // no canonicalize).
        let temp = tempfile::tempdir().expect("tempdir");
        let file = temp.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();
        let roots =
            validate_workspace_roots(vec![file.to_string_lossy().into_owned()]).expect("kept");
        assert_eq!(roots, vec![file]);
        let _ = std::fs::remove_dir_all(&missing);
    }
}
