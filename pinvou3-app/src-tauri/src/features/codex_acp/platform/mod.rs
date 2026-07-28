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

pub(super) fn should_retry_file_lock(error: &io::Error) -> bool {
    current::should_retry_file_lock(error)
}
