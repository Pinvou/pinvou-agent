//! `deps` family: system dependency health check and one-click install,
//! mirroring `pinvou3-app/src-tauri/src/app/commands/dependencies.rs`.
//!
//! - `deps check` calls the exact GUI probe
//!   `features::files::file_ingest::check_dependencies` (sync, pure
//!   detection — no host, no network): one row per file-parsing capability
//!   with its installed flag and the platform package names.
//! - `deps install` calls the exact GUI installer
//!   `features::dependencies::install_dependencies` (Linux: package
//!   whitelist + `pkexec apt-get install`; macOS: Homebrew; Windows: bundled
//!   repair). The GUI gates this behind its settings dialog; the CLI gates
//!   it behind `--yes` (exit 2 without it). Requires root authorization
//!   through the OS policy agent at run time.

use crate::support::{render, require_yes, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::files::file_ingest;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DepsCommand {
    Check,
    Install { packages: Vec<String>, yes: bool },
}

const USAGE: &str = "usage: pinvou deps <check|install <NAME...> [--yes]>";

pub fn parse(values: &[String]) -> Result<DepsCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    match subcommand.as_str() {
        "check" => {
            if values.len() > 2 {
                return Err(CliError::usage("deps check accepts no options"));
            }
            Ok(DepsCommand::Check)
        }
        "install" => {
            let mut packages = Vec::new();
            let mut yes = false;
            for token in &values[2..] {
                match token.as_str() {
                    "--yes" => {
                        if yes {
                            return Err(CliError::usage("duplicate deps option --yes"));
                        }
                        yes = true;
                    }
                    other => {
                        if other.starts_with("--") {
                            return Err(CliError::usage(format!(
                                "unsupported deps option: {other}"
                            )));
                        }
                        packages.push(other.to_owned());
                    }
                }
            }
            if packages.is_empty() {
                return Err(CliError::usage(
                    "deps install requires at least one package name",
                ));
            }
            Ok(DepsCommand::Install { packages, yes })
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

pub fn execute(command: DepsCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    match command {
        DepsCommand::Check => check(output),
        DepsCommand::Install { packages, yes } => install(&packages, yes, output),
    }
}

fn check(output: OutputMode) -> Result<CliOutcome, CliError> {
    let items = file_ingest::check_dependencies();
    let value = serde_json::json!({ "items": items });
    let human = items
        .iter()
        .map(|item| {
            let packages = if item.apt.is_empty() {
                item.hint.clone().unwrap_or_else(|| "-".to_owned())
            } else {
                item.apt.clone()
            };
            format!(
                "{}\t{}\t{}",
                item.key,
                if item.installed {
                    "installed"
                } else {
                    "missing"
                },
                packages,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(success(render(output, human, &value)))
}

fn install(packages: &[String], yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    pinvou3_lib::features::dependencies::install_dependencies(packages.to_vec(), None).map_err(
        |error| {
            CliError::failed(format!(
                "deps install failed: {}",
                translate_deps_error(&error)
            ))
        },
    )?;
    let value = serde_json::json!({ "installed": packages });
    Ok(success(render(
        output,
        format!("Installed: {}", packages.join(", ")),
        &value,
    )))
}

/// The per-platform dependency installers surface Chinese GUI copy; the CLI
/// is an English tool, so the known messages are translated at this boundary
/// and anything unrecognized passes through unchanged rather than being
/// dropped.
fn translate_deps_error(message: &str) -> String {
    for (needle, english) in [
        ("用户取消授权", "authorization was cancelled by the user"),
        (
            "未授权或 pkexec 不可用",
            "not authorized, or pkexec is unavailable",
        ),
        ("没有需要安装的依赖", "nothing to install"),
        ("非法包名", "package is not in the dependency allowlist"),
        (
            "未检测到 Homebrew",
            "Homebrew was not found; one-click dependency installation requires Homebrew (https://brew.sh)",
        ),
        (
            "brew 启动失败",
            "brew failed to start (confirm Homebrew is installed: https://brew.sh)",
        ),
        (
            "当前系统不支持一键安装依赖",
            "one-click dependency installation is not supported on this platform; install the missing tools manually",
        ),
    ] {
        if message.contains(needle) {
            return english.to_owned();
        }
    }
    for (prefix, english) in [
        ("pkexec 启动失败: ", "pkexec failed to start: "),
        ("安装失败 (exit ", "install failed (exit "),
        (
            "启动 LibreOffice 安装器失败: ",
            "failed to start the LibreOffice installer: ",
        ),
        (
            "Windows 当前仅支持一键安装 LibreOffice，无法安装: ",
            "Windows only supports one-click LibreOffice installation; cannot install: ",
        ),
    ] {
        if let Some(rest) = message.strip_prefix(prefix) {
            return format!("{english}{rest}");
        }
    }
    if message.contains("soffice.exe") && message.contains("安装器已结束") {
        return "the LibreOffice installer finished, but soffice.exe was not found; \
                reopen the app or verify LibreOffice is installed"
            .to_owned();
    }
    message.to_owned()
}
