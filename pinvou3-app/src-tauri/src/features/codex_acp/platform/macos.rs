use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::process::Command;

pub(super) const NODE_EXECUTABLE_NAME: &str = "node";
pub(super) const SYSTEM_CODEX_NAME: &str = "codex";
pub(super) const MANAGED_ADAPTER_NAME: &str = "codex-acp";

pub(super) fn development_bridge_root(manifest_dir: &Path) -> PathBuf {
    manifest_dir
        .join("resources")
        .join("platforms")
        .join("macos")
        .join("codex-bridge")
}

pub(super) fn bridge_node_relative_path() -> PathBuf {
    let target = match std::env::consts::ARCH {
        "aarch64" => "darwin-arm64",
        _ => "darwin-x64",
    };
    PathBuf::from("node")
        .join(target)
        .join("bin")
        .join(NODE_EXECUTABLE_NAME)
}

pub(super) fn codex_official_install_path() -> PathBuf {
    crate::platform::os::user_home_dir()
        .join(".local")
        .join("bin")
        .join(SYSTEM_CODEX_NAME)
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

/// 解析 brew 绝对路径（与 dependencies/platform/macos.rs 的 brew_bin 同策略）：
/// GUI 启动的 app 通常不继承 shell 的 PATH，先探测 Apple Silicon
/// (/opt/homebrew/bin/brew) 与 Intel (/usr/local/bin/brew) 两个标准位置，
/// 都没找到才回退 PATH 查找。
pub(super) fn brew_bin() -> &'static str {
    for candidate in ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"] {
        if Path::new(candidate).is_file() {
            return candidate;
        }
    }
    "brew"
}

/// brew 安装前缀（如 /opt/homebrew、/usr/local），由 brew_bin() 推导；
/// 标准路径未命中时回退 `brew --prefix` 查询，brew 不可用返回 None。
/// 供 install_source 判定时确认「正在使用的 CLI 路径」是否真由 brew 管理。
pub(super) fn brew_prefix() -> Option<PathBuf> {
    let bin = brew_bin();
    if bin != "brew" {
        return Path::new(bin)
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
    }
    crate::platform::process::HiddenCommand::new("brew")
        .arg("--prefix")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            let prefix = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (!prefix.is_empty()).then_some(PathBuf::from(prefix))
        })
}

/// 探测 Homebrew 是否可用。brew_bin() 返回非 "brew" 说明标准路径下找到了
/// brew，一定可用；回退到裸 "brew" 时走 which 检查 PATH（覆盖非标准安装位置）。
pub(super) fn brew_available() -> bool {
    if brew_bin() != "brew" {
        return true;
    }
    std::process::Command::new("/usr/bin/which")
        .arg("brew")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}
