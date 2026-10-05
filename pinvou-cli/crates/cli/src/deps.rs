//! `deps` family: system dependency health check and one-click install,
//! mirroring `pinvou3-app/src-tauri/src/app/commands/dependencies.rs`.
//!
//! - `deps check` calls the exact GUI probe
//!   `features::files::file_ingest::check_dependencies` (sync, pure
//!   detection — no host, no network): one row per file-parsing capability
//!   with its installed flag and the platform package names. On Windows the
//!   GUI's settings dialog shows a variant list (the `app/commands`
//!   windows adapter drops `voice_asr` and adds the two model-download
//!   rows); the CLI always renders the shared lib list shown on the other
//!   platforms.
//! - `deps install` calls the exact GUI installer
//!   `features::dependencies::install_dependencies` (Linux: package
//!   whitelist + `pkexec apt-get install`; macOS: Homebrew; Windows: bundled
//!   repair). The GUI gates this behind its settings dialog; the CLI gates
//!   it behind `--yes` (exit 2 without it). Requires root authorization
//!   through the OS policy agent at run time. The installer's progress hook —
//!   the one the GUI turns into `deps:install_progress` events — is wired to
//!   stderr `note!` lines so a multi-minute `brew`/`pkexec` run is not
//!   indistinguishable from a hang; stdout stays a single JSON line.

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

/// Progress sink `install_dependencies` calls for each adapter line:
/// `(package, index, total, line)`. Named because the bare trait-object type is
/// unreadable at the call site and appears in both the closure's annotation and
/// the adapter's own signature.
type InstallProgress = dyn Fn(&str, usize, usize, Option<&str>) + Sync;

fn check(output: OutputMode) -> Result<CliOutcome, CliError> {
    let mut items = file_ingest::check_dependencies();
    // The GUI localizes dependency `hint` strings through its i18n table;
    // the CLI has no i18n layer, so known keys are translated at this
    // boundary (same technique as `translate_deps_error`) and unknown hints
    // pass through unchanged rather than being dropped.
    for item in &mut items {
        if let Some(hint) = item.hint.as_deref() {
            item.hint = Some(translate_deps_hint(hint));
        }
    }
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
    // The installer's second argument is the progress hook the GUI wires to
    // `app.emit("deps:install_progress", …)`. Passing `None` made the whole
    // install silent: on macOS the Homebrew adapter streams every brew
    // stdout/stderr line through this hook, and a `libreoffice` cask can run
    // for tens of minutes; on Linux `pkexec` can block indefinitely waiting on
    // a polkit agent that is not running. With no output at all the two are
    // indistinguishable from a hang, so the hook is wired to stderr here.
    //
    // stderr, not stdout: `--output json` must stay a single serde_json line,
    // and progress is not part of the result (same split as every other lane
    // that narrates with `note!`).
    let progress = |package: &str, current: usize, total: usize, detail: Option<&str>| {
        match detail {
            // Vendor lines are third-party process output the CLI did not
            // compose. `brew`/`apt-get` echo URLs and occasionally tokens
            // embedded in them, so they get the same heuristic scrub every
            // other lane applies to external command output before showing it
            // (see `code.rs`'s vendor-CLI transcript handling).
            Some(line) => note!(
                "[deps] ({current}/{total}) {package}: {}",
                pinvou3_lib::platform::credential_store::redact_secret(line)
            ),
            None => note!("[deps] ({current}/{total}) installing {package}"),
        }
    };
    // Spelled out rather than inferred: the adapter takes a trait object with a
    // `Sync` bound (macOS drains stdout and stderr from two scoped threads that
    // both call it), and the annotation makes the unsize coercion explicit
    // instead of leaving it to expected-type inference. The closure captures
    // nothing, so `Sync` holds trivially.
    let progress: &InstallProgress = &progress;
    pinvou3_lib::features::dependencies::install_dependencies(packages.to_vec(), Some(progress))
        .map_err(|error| {
            // The adapter error embeds raw vendor stderr (brew/apt tails);
            // the progress lane scrubs every line it streams, so the failure
            // that summarizes those same lines gets the identical scrub —
            // otherwise a token-shaped string would be `[REDACTED]` in the
            // progress but printed verbatim here.
            CliError::failed(format!(
                "deps install failed: {}",
                pinvou3_lib::platform::credential_store::redact_secret(&translate_deps_error(
                    &error
                ))
            ))
        })?;
    // Not `installed`: this is the caller's argv, and the adapters do not
    // report back which packages the package manager actually placed on disk
    // (Homebrew installs per package and joins the failures; apt installs the
    // batch in one `pkexec` call). Naming the field `requested` keeps it from
    // being read as a verified outcome, and `deps check` is the lane that
    // answers "what is installed now".
    let value = serde_json::json!({ "requested": packages });
    // Round-37 review: the packages are raw argv tokens, so the human cell
    // gets the same row sanitizer as every other argv-derived cell.
    let requested = crate::support::collapse_control_characters(&packages.join(", "));
    Ok(success(render(
        output,
        format!(
            "Requested: {requested}\nThe installer reported success; run `pinvou deps check` to \
             confirm which capabilities are now available."
        ),
        &value,
    )))
}

/// Maps the dependency hints the GUI localizes through its i18n table; the
/// CLI is an English tool, so the known keys get English copy here and
/// anything unrecognized passes through unchanged rather than being dropped.
fn translate_deps_hint(hint: &str) -> String {
    match hint {
        "email_manual" => "install the Perl Email::Outlook::Message module manually".to_owned(),
        other => other.to_owned(),
    }
}

/// The per-platform dependency installers surface Chinese GUI copy; the CLI
/// is an English tool, so the known messages are translated at this boundary
/// and anything unrecognized passes through unchanged rather than being
/// dropped.
pub(crate) fn translate_deps_error(message: &str) -> String {
    for (needle, english) in [
        ("用户取消授权", "authorization was cancelled by the user"),
        (
            "未授权或 pkexec 不可用",
            "not authorized, or pkexec is unavailable",
        ),
        ("没有需要安装的依赖", "nothing to install"),
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
    // The allowlist refusal names the offending package after "："/":" in
    // the upstream message; dropping the tail would leave a user running
    // `pinvou deps install a b c --yes` unable to tell which package was
    // refused, so it is carried over.
    if message.contains("非法包名") {
        let tail = message
            .split_once("：")
            .or_else(|| message.split_once(':'))
            .map(|(_, rest)| rest.trim())
            .unwrap_or_default();
        return if tail.is_empty() {
            "package is not in the dependency allowlist".to_owned()
        } else {
            format!("package is not in the dependency allowlist: {tail}")
        };
    }
    // The winget adapter's three failure shapes (windows_install_text.rs):
    // a UAC/winget cancellation, a missing winget, and a failed install
    // carrying a compacted vendor detail. All three reach an English-only
    // terminal here, so each gets English copy; the 300-char vendor detail
    // tail is third-party output and survives as-is.
    if message.contains("已取消 LibreOffice 安装") {
        return "the LibreOffice installation was cancelled".to_owned();
    }
    if message.contains("未找到 winget") {
        return "winget was not found; one-click dependency installation on Windows requires \
                the App Installer package (winget), or install LibreOffice manually"
            .to_owned();
    }
    // The no-detail winget failure carries a Chinese tail AFTER the exit
    // code, so it must be matched whole before the generic `安装失败`
    // block below (which would preserve that tail verbatim).
    if message.contains("请检查 winget") {
        return "LibreOffice install failed; check that winget is available, or install \
                LibreOffice manually"
            .to_owned();
    }
    // `安装失败` appears in three adapter shapes: Linux bare
    // (`安装失败 (exit …)`), macOS package-prefixed (`ffmpeg 安装失败
    // (exit …)`), and Windows (`LibreOffice 安装失败 (exit …)`). The
    // package name is the user's argv and must survive, so this entry is
    // contains-based rather than a prefix strip.
    if let Some((head, rest)) = message.split_once("安装失败 (exit ") {
        let package = head.trim();
        return if package.is_empty() {
            format!("install failed (exit {rest}")
        } else {
            format!("{package} install failed (exit {rest}")
        };
    }
    for (prefix, english) in [
        ("pkexec 启动失败: ", "pkexec failed to start: "),
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

#[cfg(test)]
mod tests {
    use super::translate_deps_error;

    /// The allowlist refusal must keep the offending package name the
    /// upstream message carries (`非法包名（不在依赖白名单内）: {package}`):
    /// with `pinvou deps install a b c --yes` a bare "not in the allowlist"
    /// cannot say WHICH package was refused, and the user's next step
    /// depends on it.
    #[test]
    fn allowlist_refusal_keeps_the_offending_package_name() {
        let translated = translate_deps_error("非法包名（不在依赖白名单内）: cowsay");
        assert!(
            translated.starts_with("package is not in the dependency allowlist"),
            "{translated}"
        );
        assert!(translated.contains("cowsay"), "{translated}");
        // A message with no parseable tail still translates to the bare
        // refusal instead of passing the CJK text through.
        assert_eq!(
            translate_deps_error("非法包名（不在依赖白名单内）"),
            "package is not in the dependency allowlist"
        );
    }

    /// The install-failure shapes differ per adapter (Linux bare, macOS
    /// package-prefixed, Windows LibreOffice-prefixed with and without a
    /// vendor detail); every one must reach the terminal in English with
    /// the package name and exit code intact — a macOS `brew` failure used
    /// to pass through untranslated because only the bare Linux shape was
    /// matched, and the winget copy was not translated at all.
    #[test]
    fn install_failure_shapes_translate_per_adapter_with_the_package_kept() {
        assert_eq!(
            translate_deps_error("安装失败 (exit 100): apt-get returned 100"),
            "install failed (exit 100): apt-get returned 100"
        );
        assert_eq!(
            translate_deps_error("ffmpeg 安装失败 (exit 1): Error: ffmpeg@7 dependency failure"),
            "ffmpeg install failed (exit 1): Error: ffmpeg@7 dependency failure"
        );
        assert_eq!(
            translate_deps_error(
                "LibreOffice 安装失败 (exit 1): winget: Installation failed with exit code 1"
            ),
            "LibreOffice install failed (exit 1): winget: Installation failed with exit code 1"
        );
        assert_eq!(
            translate_deps_error(
                "LibreOffice 安装失败 (exit 1)。请检查 winget 是否可用，或手动安装 LibreOffice。"
            ),
            "LibreOffice install failed; check that winget is available, or install LibreOffice manually"
        );
        assert_eq!(
            translate_deps_error("已取消 LibreOffice 安装。"),
            "the LibreOffice installation was cancelled"
        );
        assert_eq!(
            translate_deps_error("未找到 winget。请安装 App Installer，或手动安装 LibreOffice。"),
            "winget was not found; one-click dependency installation on Windows requires the App Installer package (winget), or install LibreOffice manually"
        );
    }
}
