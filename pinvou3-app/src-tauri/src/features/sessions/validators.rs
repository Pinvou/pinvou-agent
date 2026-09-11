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

/// 普通 chat 会话用户选择的工作目录校验:非空、绝对路径、盘上必须存在且是
/// 目录。canonicalize 后返回(消除 `.`/符号链接/尾部分隔符),调用方直接使用
/// 返回值绑定与展示。
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
    // Windows canonicalize 会返回 \?\ verbatim 前缀;agent 与前端对 cwd 做
    // 字符串/前缀比较时会误判,统一归一成常规盘符路径(非 Windows 恒等)——
    // 与 validate_codex_project_workspace 同一条不变量(评审 #445 P1-1)。
    Ok(crate::platform::os::platform_compat_path(
        &canonical.to_string_lossy(),
    ))
}

/// 钥匙串快照(§6)的入参校验:每个附加根必须绝对路径(硬拒,与 cwd 同闸);
/// 不存在/非目录只记软警告——参照 rebind 的既有模式,目标目录可能被移走
/// 后重建,快照如实记录用户当时的选择,引擎侧对不存在根的写豁免自然落空。
/// canonicalize 成功的用 canonical 形态(消 symlink/verbatim 前缀,与
/// validate_user_workspace_path 同不变量),失败保留词法原值。
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
                eprintln!("[sessions] workspace root not an existing directory (kept as-is): {entry}");
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
