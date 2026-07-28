use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tokio::process::Command;

use super::ManagedCodexArtifact;

pub(super) const NODE_EXECUTABLE_NAME: &str = "node";
pub(super) const SYSTEM_CODEX_NAME: &str = "codex";
pub(super) const MANAGED_ADAPTER_NAME: &str = "codex-acp";
pub(super) const BUNDLED_ADAPTER_NAME: &str = "codex-acp";
pub(super) const MANAGED_CODEX_EXECUTABLE_NAME: &str = "codex";

pub(super) fn development_bridge_root(manifest_dir: &Path) -> PathBuf {
    manifest_dir
        .join("resources")
        .join("platforms")
        .join("macos")
        .join("codex-bridge")
}

pub(super) fn adapter_needs_node(adapter: &Path) -> bool {
    adapter.extension().and_then(|value| value.to_str()) == Some("js")
}

pub(super) fn adapter_command(adapter: &Path, node: Option<&Path>) -> Result<Command> {
    if adapter_needs_node(adapter) {
        let node = node.context("Codex ACP Bridge 缺少可用 Node")?;
        let mut command = Command::new(crate::platform::os::external_application_path(node));
        command.arg(crate::platform::os::external_application_path(adapter));
        Ok(command)
    } else {
        Ok(Command::new(
            crate::platform::os::external_application_path(adapter),
        ))
    }
}

pub(super) fn codex_login_command(codex: &Path) -> Command {
    let mut command = Command::new(crate::platform::os::external_application_path(codex));
    command.arg("login");
    command
}

pub(super) fn managed_artifact(architecture: &str) -> Result<ManagedCodexArtifact> {
    bail!("当前托管 Codex 下载不支持平台: macos-{architecture}")
}

pub(super) fn should_retry_file_lock(_error: &io::Error) -> bool {
    false
}
