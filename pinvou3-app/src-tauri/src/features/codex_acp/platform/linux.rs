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
        .join("linux")
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
    match architecture {
        "x86_64" => Ok(ManagedCodexArtifact {
            urls: &[
                "https://registry.npmjs.org/@openai/codex/-/codex-0.144.6-linux-x64.tgz",
                "https://registry.npmmirror.com/@openai/codex/-/codex-0.144.6-linux-x64.tgz",
            ],
            integrity: "sha512-4E7EnzCg0OnBxCyYnwJ+qnZwWHYe0YScr5ucKWbngE9u4+0XrpWELqq2Kn9jl5GZK8MDjU7PrJwFIwusHOHjuw==",
            vendor_triple: "x86_64-unknown-linux-musl",
        }),
        "aarch64" => Ok(ManagedCodexArtifact {
            urls: &[
                "https://registry.npmjs.org/@openai/codex/-/codex-0.144.6-linux-arm64.tgz",
                "https://registry.npmmirror.com/@openai/codex/-/codex-0.144.6-linux-arm64.tgz",
            ],
            integrity: "sha512-PGiLXMN+2IQRkf7tOLi64dMInjU1pRLbz0Rwfj/yt2Y97SZQqAjFQoi2wmswmqtqMDnfwCPTC1DRXVQkvU6T6Q==",
            vendor_triple: "aarch64-unknown-linux-musl",
        }),
        _ => bail!("当前托管 Codex 下载不支持平台: linux-{architecture}"),
    }
}

pub(super) fn should_retry_file_lock(_error: &io::Error) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_artifacts_are_available_for_supported_architectures() {
        for architecture in ["x86_64", "aarch64"] {
            let artifact =
                managed_artifact(architecture).expect("resolve supported Linux Codex artifact");
            assert!(!artifact.vendor_triple.is_empty());
            assert!(artifact.urls[0].starts_with("https://"));
            assert!(artifact.integrity.starts_with("sha512-"));
        }
    }

    #[test]
    fn file_lock_errors_are_not_retried() {
        assert!(!should_retry_file_lock(&io::Error::from_raw_os_error(13)));
    }
}
