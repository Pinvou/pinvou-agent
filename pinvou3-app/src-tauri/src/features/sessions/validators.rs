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
/// `pub` for the pinvou-cli `projects` family: the rebind orphan probe
/// validates an id before joining it into a sessions-root path (the same
/// fail-closed rule the GUI path applies), through the `features::sessions`
/// re-export. The GUI itself keeps calling the crate-private path.
pub fn validate_session_id(id: &str) -> Result<()> {
    validate_id_charset(id, "session")
}

/// The URL-safe charset check shared by every id kind in this store: non-empty
/// and `[A-Za-z0-9_-]` only (ids resolve to filenames, so no separators).
fn validate_id_charset(id: &str, kind: &str) -> Result<()> {
    if id.trim().is_empty()
        || !id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        bail!("Invalid {kind} id '{id}'");
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
    // `get(..4)` instead of slicing: byte slicing panics when the index falls
    // inside a multibyte UTF-8 char, and these guards run on client-supplied
    // ids before any charset validation (e.g. the delete_session cascade
    // resolves the aux mapping before the session manager's validate).
    id.get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("aux-"))
}

/// Single-sourced negation for the "global side-effect channel" gates
/// (memory review, task-completion notification): aux turns must never
/// trigger them (round-15 MAJOR-4, round-26 minor M3). One helper keeps the
/// exclusion rule from drifting apart across its call sites (round-32
/// review minor 10).
pub(crate) fn aux_side_effect_exclusion(id: &str) -> bool {
    !is_aux_session_id(id)
}

/// Case-insensitive `sched-` prefix test — same alias-defeating argument as
/// [`is_aux_session_id`]. Applied at the aux creation guard, the
/// list/retention filters, the sched- send gates, and
/// `SessionStore::is_scheduled_session`'s prefix leg (round-34 minor 4: a
/// `SCHED-` file alias on a case-insensitive filesystem loads the real
/// record but must not slip past the send gates' profile re-validation;
/// round-36 minor 2: without the case-insensitive leg, such an alias would
/// skip every gate built on this predicate, the delete refusal among
/// them). The sched-side REGISTRY lookups keep their pre-existing
/// exact-match checks — the `contains_key` inside `is_scheduled_session`,
/// plus `purge_all_scheduled_side_maps` and `list_scheduled` — since
/// scheduled profiles are only ever written through the validating API, so
/// their registry keys cannot carry case variants; the alias-defeating
/// prefix policy lives here, not in the registries.
pub(crate) fn is_sched_session_id(id: &str) -> bool {
    // See is_aux_session_id for the boundary-safe `get(..6)`.
    id.get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("sched-"))
}

pub(crate) fn validate_scheduled_task_id(id: &str) -> Result<()> {
    validate_id_charset(id, "scheduled task")
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

#[cfg(test)]
mod tests {
    use super::{validate_scheduled_task_id, validate_session_id};

    /// Pins the two public error messages byte-for-byte: the shared charset
    /// helper interpolates the id kind, and callers surface these strings.
    #[test]
    fn id_charset_rejects_bad_ids_with_kind_verbatim_messages() {
        assert_eq!(
            validate_session_id("../escape").unwrap_err().to_string(),
            "Invalid session id '../escape'"
        );
        assert_eq!(
            validate_scheduled_task_id("has space")
                .unwrap_err()
                .to_string(),
            "Invalid scheduled task id 'has space'"
        );
    }

    #[test]
    fn id_charset_accepts_url_safe_and_rejects_blank_ids() {
        assert!(validate_session_id("Ab_09-z").is_ok());
        assert!(validate_scheduled_task_id("task_1").is_ok());
        assert!(validate_session_id("   ").is_err());
        assert!(validate_scheduled_task_id("").is_err());
    }
}
