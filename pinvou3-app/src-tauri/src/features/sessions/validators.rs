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

/// Case-insensitive `aux-` prefix test. The aux zero-tools gates
/// (`turn_restrict_tools`, the spawn-config backstop) and the aux-of-aux /
/// sidecar guards decide "is this an aux session" from the client-supplied id
/// string, but `validate_session_id` allows uppercase and ids resolve to
/// files without case canonicalization — on case-insensitive filesystems
/// (NTFS/APFS) an `AUX-<suffix>` alias would load the real aux record while
/// every case-sensitive prefix test misses it, running a full-tool turn over
/// the aux session. Every is-aux decision must go through this helper so the
/// gates hold regardless of filesystem case semantics.
pub(crate) fn is_aux_session_id(id: &str) -> bool {
    id.len() >= 4 && id[..4].eq_ignore_ascii_case("aux-")
}

/// Case-insensitive `sched-` prefix test — same alias-defeating argument as
/// [`is_aux_session_id`], applied wherever a sched- identity decides a guard.
pub(crate) fn is_sched_session_id(id: &str) -> bool {
    id.len() >= 6 && id[..6].eq_ignore_ascii_case("sched-")
}

/// Log-safe rendering of a session id: keeps only the kind prefix, hashes the
/// remainder. Diagnostics stay greppable by session kind and stable per
/// session without writing any slice of the identifier itself to logs —
/// partial echoes still trip the cleartext-logging scanner, so nothing of the
/// raw id may survive into the output.
pub(crate) fn mask_session_id(id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let (prefix, rest) = id.split_once('-').map_or(("", id), |(head, tail)| {
        if matches!(head, "aux" | "sched" | "eval") {
            (head, tail)
        } else {
            ("", id)
        }
    });
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rest.hash(&mut hasher);
    let digest = hasher.finish();
    if prefix.is_empty() {
        format!("…{digest:08x}")
    } else {
        format!("{prefix}-…{digest:08x}")
    }
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
