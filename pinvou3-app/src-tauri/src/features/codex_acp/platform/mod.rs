use std::io;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::process::Command;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
use linux as current;
#[cfg(target_os = "macos")]
use macos as current;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
use unsupported as current;
#[cfg(target_os = "windows")]
use windows as current;

pub(super) struct ManagedCodexArtifact {
    pub(super) urls: &'static [&'static str],
    pub(super) integrity: &'static str,
    pub(super) vendor_triple: &'static str,
}

pub(super) fn development_bridge_root(manifest_dir: &Path) -> PathBuf {
    current::development_bridge_root(manifest_dir)
}

pub(super) fn node_executable_name() -> &'static str {
    current::NODE_EXECUTABLE_NAME
}

pub(super) fn system_codex_name() -> &'static str {
    current::SYSTEM_CODEX_NAME
}

pub(super) fn managed_adapter_name() -> &'static str {
    current::MANAGED_ADAPTER_NAME
}

pub(super) fn bundled_adapter_name() -> &'static str {
    current::BUNDLED_ADAPTER_NAME
}

pub(super) fn managed_codex_executable_name() -> &'static str {
    current::MANAGED_CODEX_EXECUTABLE_NAME
}

pub(super) fn adapter_needs_node(adapter: &Path) -> bool {
    current::adapter_needs_node(adapter)
}

pub(super) fn adapter_command(adapter: &Path, node: Option<&Path>) -> Result<Command> {
    current::adapter_command(adapter, node)
}

pub(super) fn codex_login_command(codex: &Path) -> Command {
    current::codex_login_command(codex)
}

pub(super) fn managed_artifact(architecture: &str) -> Result<ManagedCodexArtifact> {
    current::managed_artifact(architecture)
}

/// 当前平台的 Codex 安装方式（status.install_method 契约字段）：
/// "managed_download"（linux/windows）/ "homebrew"（macOS）/ "manual"（其他）。
pub(super) fn install_method() -> &'static str {
    current::INSTALL_METHOD
}

#[cfg(target_os = "macos")]
pub(super) fn brew_bin() -> &'static str {
    current::brew_bin()
}

/// Homebrew 仅 macOS 使用；其他平台给保守默认值，仅保证编译通过。
#[cfg(not(target_os = "macos"))]
pub(super) fn brew_bin() -> &'static str {
    "brew"
}

#[cfg(target_os = "macos")]
pub(super) fn brew_available() -> bool {
    current::brew_available()
}

/// 仅 macOS 探测 Homebrew；其他平台恒 false。
#[cfg(not(target_os = "macos"))]
pub(super) fn brew_available() -> bool {
    false
}

pub(super) fn should_retry_file_lock(error: &io::Error) -> bool {
    current::should_retry_file_lock(error)
}

/// 测试辅助：当前平台是否类 Unix（假 codex 脚本依赖可执行位）。
#[cfg(test)]
pub(super) fn unix_like() -> bool {
    cfg!(unix)
}

/// 测试辅助：为测试脚本加可执行位；非 Unix 平台为空操作。
#[cfg(test)]
pub(super) fn make_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
