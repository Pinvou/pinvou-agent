use std::path::{Path, PathBuf};

use crate::platform::process::HiddenTokioCommand;
use anyhow::{Context, Result};
use tokio::process::Command;

pub(super) const NODE_EXECUTABLE_NAME: &str = "node.exe";
pub(super) const SYSTEM_CODEX_NAME: &str = "codex.cmd";
pub(super) const MANAGED_ADAPTER_NAME: &str = "codex-acp.cmd";

pub(super) fn development_bridge_root(manifest_dir: &Path) -> PathBuf {
    manifest_dir
        .join("target")
        .join("windows-runtime")
        .join("codex-bridge")
}

pub(super) fn bridge_node_relative_path() -> PathBuf {
    PathBuf::from("node").join("bin").join(NODE_EXECUTABLE_NAME)
}

pub(super) fn codex_official_install_path() -> PathBuf {
    // OpenAI install.ps1 默认使用
    // %LOCALAPPDATA%\Programs\OpenAI\Codex\bin\codex.exe；用户可在安装脚本中
    // 通过 CODEX_INSTALL_DIR 改写，但 Pinvou 未设置该变量，因此按默认路径探测。
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            crate::platform::os::user_home_dir()
                .join("AppData")
                .join("Local")
        })
        .join("Programs")
        .join("OpenAI")
        .join("Codex")
        .join("bin")
        .join("codex.exe")
}

pub(super) fn adapter_needs_node(_adapter: &Path) -> bool {
    true
}

pub(super) fn adapter_command(adapter: &Path, node: Option<&Path>) -> Result<Command> {
    let adapter = crate::platform::os::external_application_path(adapter);
    if adapter.extension().and_then(|value| value.to_str()) == Some("js") {
        let node = node.context("Codex ACP Bridge 缺少可用 Node")?;
        let mut command =
            HiddenTokioCommand::new(crate::platform::os::external_application_path(node));
        command.arg(adapter);
        Ok(command)
    } else if adapter.extension().and_then(|value| value.to_str()) == Some("cmd") {
        let mut command = HiddenTokioCommand::new("cmd");
        command.args(["/D", "/S", "/C"]).arg(adapter);
        Ok(command)
    } else {
        Ok(HiddenTokioCommand::new(adapter))
    }
}

pub(super) fn codex_login_command(codex: &Path) -> Command {
    let codex = crate::platform::os::external_application_path(codex);
    if codex.extension().and_then(|value| value.to_str()) == Some("cmd") {
        let mut command = HiddenTokioCommand::new("cmd");
        command.args(["/D", "/S", "/C"]).arg(codex).arg("login");
        command
    } else {
        let mut command = HiddenTokioCommand::new(codex);
        command.arg("login");
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_javascript_adapter_uses_native_windows_paths() {
        let command = adapter_command(
            Path::new(r"\\?\C:\Program Files\pinvou3\runtime\codex-bridge\index.js"),
            Some(Path::new(
                r"\\?\C:\Program Files\pinvou3\runtime\node\node.exe",
            )),
        )
        .expect("build installed JavaScript adapter command");

        assert_eq!(
            command.as_std().get_program(),
            r"C:\Program Files\pinvou3\runtime\node\node.exe"
        );
        assert_eq!(
            command
                .as_std()
                .get_args()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec![r"C:\Program Files\pinvou3\runtime\codex-bridge\index.js"]
        );
    }

    #[test]
    fn command_shim_uses_windows_command_interpreter() {
        let command = adapter_command(Path::new(r"C:\runtime\codex-acp.cmd"), None)
            .expect("build Windows command-shim adapter command");
        assert_eq!(command.as_std().get_program(), "cmd");
        assert_eq!(
            command
                .as_std()
                .get_args()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["/D", "/S", "/C", r"C:\runtime\codex-acp.cmd"]
        );
    }
}
