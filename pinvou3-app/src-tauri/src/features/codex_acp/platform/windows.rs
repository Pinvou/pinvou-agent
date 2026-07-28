use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tokio::process::Command;

use super::ManagedCodexArtifact;

pub(super) const NODE_EXECUTABLE_NAME: &str = "node.exe";
pub(super) const SYSTEM_CODEX_NAME: &str = "codex.cmd";
pub(super) const MANAGED_ADAPTER_NAME: &str = "codex-acp.cmd";
pub(super) const BUNDLED_ADAPTER_NAME: &str = "codex-acp.exe";
pub(super) const MANAGED_CODEX_EXECUTABLE_NAME: &str = "codex.exe";

pub(super) fn development_bridge_root(manifest_dir: &Path) -> PathBuf {
    manifest_dir
        .join("target")
        .join("windows-runtime")
        .join("codex-bridge")
}

pub(super) fn adapter_needs_node(_adapter: &Path) -> bool {
    true
}

pub(super) fn adapter_command(adapter: &Path, node: Option<&Path>) -> Result<Command> {
    let adapter = crate::platform::os::external_application_path(adapter);
    if adapter.extension().and_then(|value| value.to_str()) == Some("js") {
        let node = node.context("Codex ACP Bridge 缺少可用 Node")?;
        let mut command = Command::new(crate::platform::os::external_application_path(node));
        command.arg(adapter);
        Ok(command)
    } else if adapter.extension().and_then(|value| value.to_str()) == Some("cmd") {
        let mut command = Command::new("cmd");
        command.args(["/D", "/S", "/C"]).arg(adapter);
        Ok(command)
    } else {
        Ok(Command::new(adapter))
    }
}

pub(super) fn codex_login_command(codex: &Path) -> Command {
    let codex = crate::platform::os::external_application_path(codex);
    if codex.extension().and_then(|value| value.to_str()) == Some("cmd") {
        let mut command = Command::new("cmd");
        command.args(["/D", "/S", "/C"]).arg(codex).arg("login");
        command
    } else {
        let mut command = Command::new(codex);
        command.arg("login");
        command
    }
}

pub(super) fn managed_artifact(architecture: &str) -> Result<ManagedCodexArtifact> {
    match architecture {
        "x86_64" => Ok(ManagedCodexArtifact {
            urls: &[
                "https://registry.npmjs.org/@openai/codex/-/codex-0.144.6-win32-x64.tgz",
                "https://registry.npmmirror.com/@openai/codex/-/codex-0.144.6-win32-x64.tgz",
            ],
            integrity: "sha512-dN39VnjEthKz5io1RNWwZDtErdSn07nW3pGUgvlA6DMxgm/nuGaIAZO/sG/Hgxq/x5j9HteAENfrFgVkpZ0lFg==",
            vendor_triple: "x86_64-pc-windows-msvc",
        }),
        _ => bail!("当前托管 Codex 下载不支持平台: windows-{architecture}"),
    }
}

pub(super) fn should_retry_file_lock(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::PermissionDenied || matches!(error.raw_os_error(), Some(5 | 32))
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

    #[test]
    fn x64_managed_artifact_is_available() {
        let artifact = managed_artifact("x86_64").expect("resolve Windows x64 Codex artifact");
        assert_eq!(artifact.vendor_triple, "x86_64-pc-windows-msvc");
        assert!(artifact.urls[0].starts_with("https://"));
        assert!(artifact.integrity.starts_with("sha512-"));
    }

    #[test]
    fn file_lock_errors_are_retryable() {
        assert!(should_retry_file_lock(&io::Error::from_raw_os_error(5)));
        assert!(should_retry_file_lock(&io::Error::from_raw_os_error(32)));
        assert!(!should_retry_file_lock(&io::Error::from_raw_os_error(2)));
    }
}
