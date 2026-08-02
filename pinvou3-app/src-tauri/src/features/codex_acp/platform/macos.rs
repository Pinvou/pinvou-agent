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
/// macOS 与 Linux/Windows 一样走托管下载；系统 Homebrew cask 版本过旧时
/// 的 `brew upgrade` 特例由 features/codex_acp/mod.rs 处理。
pub(super) const INSTALL_METHOD: &str = "managed_download";

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
    match architecture {
        "aarch64" => Ok(ManagedCodexArtifact {
            urls: &[
                "https://registry.npmjs.org/@openai/codex/-/codex-0.144.6-darwin-arm64.tgz",
                "https://registry.npmmirror.com/@openai/codex/-/codex-0.144.6-darwin-arm64.tgz",
            ],
            integrity: "sha512-6zgvh70MzBNSeT17HEhSOrmmGGZGAKzSC7x6JAq+edkJkdPYA9P0I1tG7aJ49GlBkBxuC+MKBH1qm6+2Cghcww==",
            vendor_triple: "aarch64-apple-darwin",
        }),
        "x86_64" => Ok(ManagedCodexArtifact {
            urls: &[
                "https://registry.npmjs.org/@openai/codex/-/codex-0.144.6-darwin-x64.tgz",
                "https://registry.npmmirror.com/@openai/codex/-/codex-0.144.6-darwin-x64.tgz",
            ],
            integrity: "sha512-THRyPG0zSU6M8NQAge1LHEHsJDnoH4BpKsfJHB/qe3Fm+Wf6zqAmWJFlOKzBm27m0K2Hq3za4Ac2I5p5i4yp/A==",
            vendor_triple: "x86_64-apple-darwin",
        }),
        _ => bail!("当前托管 Codex 下载不支持平台: macos-{architecture}"),
    }
}

pub(super) fn should_retry_file_lock(_error: &io::Error) -> bool {
    false
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_artifacts_are_available_for_supported_architectures() {
        for (architecture, triple) in [
            ("aarch64", "aarch64-apple-darwin"),
            ("x86_64", "x86_64-apple-darwin"),
        ] {
            let artifact =
                managed_artifact(architecture).expect("resolve supported macOS Codex artifact");
            assert_eq!(artifact.vendor_triple, triple);
            assert!(artifact.urls[0].starts_with("https://"));
            assert!(artifact.integrity.starts_with("sha512-"));
        }
        assert!(managed_artifact("riscv64").is_err());
    }

    #[test]
    fn file_lock_errors_are_not_retried() {
        assert!(!should_retry_file_lock(&io::Error::from_raw_os_error(13)));
    }
}
