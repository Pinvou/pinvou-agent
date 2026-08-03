use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

use crate::platform::process::HiddenCommand;

const INSTALL_CANCELLED_MARKER: &str = "__PINVOU_INSTALL_CANCELLED__";
const WINGET_MISSING_MARKER: &str = "__PINVOU_WINGET_MISSING__";
const INSTALL_ERROR_PREFIX: &str = "__PINVOU_INSTALL_ERROR_B64__";

/// Windows 一键安装统一走 winget 提权静默安装：依赖体检的包名 → winget 包。
struct WingetPackage {
    /// 依赖体检 `apt` 字段里的包名（各平台同名）。
    package: &'static str,
    winget_id: &'static str,
    /// 报错文案里的人话名。
    display: &'static str,
    /// 装好判定（新装的包改的是系统 PATH，本进程环境看不见时提示重启应用）。
    installed: fn() -> bool,
}

fn libreoffice_installed() -> bool {
    crate::platform::os::command_exists("soffice")
        || crate::platform::os::command_exists("libreoffice")
}

fn git_installed() -> bool {
    crate::platform::os::command_exists("git")
}

const WINGET_PACKAGES: &[WingetPackage] = &[
    WingetPackage {
        package: "libreoffice",
        winget_id: "TheDocumentFoundation.LibreOffice",
        display: "LibreOffice",
        installed: libreoffice_installed,
    },
    // 多智能体的并行隔离（git worktree）依赖 git；Windows 是唯一常缺 git 的平台。
    WingetPackage {
        package: "git",
        winget_id: "Git.Git",
        display: "Git",
        installed: git_installed,
    },
];

fn winget_install_script(winget_id: &str) -> String {
    // Keep the script itself ASCII-only. Windows PowerShell 5.1 writes redirected
    // ErrorRecord text using the active system code page, which cannot safely be
    // decoded as UTF-8. Known outcomes use ASCII markers; unexpected localized
    // exception messages are UTF-8 encoded and transported as Base64.
    r#"$ErrorActionPreference = 'Stop';
$winget = (Get-Command winget.exe -ErrorAction SilentlyContinue).Source;
if ([string]::IsNullOrWhiteSpace($winget)) {
  [Console]::Out.Write('__PINVOU_WINGET_MISSING__');
  exit 127
}
$args = @('install','--id','__WINGET_ID__','--exact','--source','winget','--accept-source-agreements','--accept-package-agreements','--silent');
try {
  $p = Start-Process -FilePath $winget -ArgumentList $args -Verb RunAs -Wait -PassThru
} catch {
  $nativeCode = 0;
  if ($null -ne $_.Exception.InnerException -and $null -ne $_.Exception.InnerException.NativeErrorCode) {
    $nativeCode = $_.Exception.InnerException.NativeErrorCode
  }
  if ($nativeCode -eq 1223) {
    [Console]::Out.Write('__PINVOU_INSTALL_CANCELLED__');
    exit 1223
  }
  $bytes = [System.Text.Encoding]::UTF8.GetBytes($_.Exception.Message);
  [Console]::Out.Write('__PINVOU_INSTALL_ERROR_B64__' + [Convert]::ToBase64String($bytes));
  exit 1
}
exit $p.ExitCode"#
        .replace("__WINGET_ID__", winget_id)
}

fn marker_present(stdout: &[u8], stderr: &[u8], marker: &str) -> bool {
    [stdout, stderr]
        .into_iter()
        .filter_map(|bytes| std::str::from_utf8(bytes).ok())
        .any(|text| text.contains(marker))
}

fn encoded_install_error(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    [stdout, stderr]
        .into_iter()
        .filter_map(|bytes| std::str::from_utf8(bytes).ok())
        .find_map(|text| {
            let payload = text.split_once(INSTALL_ERROR_PREFIX)?.1;
            let payload = payload.lines().next().unwrap_or(payload).trim();
            let decoded = BASE64_STANDARD.decode(payload).ok()?;
            String::from_utf8(decoded).ok()
        })
}

fn compact_utf8_detail(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    [stderr, stdout]
        .into_iter()
        .filter_map(|bytes| std::str::from_utf8(bytes).ok())
        .map(str::trim)
        .find(|text| !text.is_empty())
        .map(|text| {
            let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
            compact.chars().take(300).collect()
        })
}

fn install_failure_message(display: &str, code: i32, stdout: &[u8], stderr: &[u8]) -> String {
    if marker_present(stdout, stderr, INSTALL_CANCELLED_MARKER) || code == 1223 {
        return format!("已取消 {display} 安装。");
    }
    if marker_present(stdout, stderr, WINGET_MISSING_MARKER) || code == 127 {
        return format!("未找到 winget。请安装 App Installer，或手动安装 {display}。");
    }
    if let Some(detail) =
        encoded_install_error(stdout, stderr).or_else(|| compact_utf8_detail(stdout, stderr))
    {
        return format!("{display} 安装失败 (exit {code}): {detail}");
    }
    format!("{display} 安装失败 (exit {code})。请检查 winget 是否可用，或手动安装 {display}。")
}

pub fn install_dependencies(packages: Vec<String>) -> Result<(), String> {
    if packages.is_empty() {
        return Err("没有需要安装的依赖".into());
    }
    let mut todo = Vec::new();
    for package in &packages {
        let Some(entry) = WINGET_PACKAGES
            .iter()
            .find(|candidate| candidate.package.eq_ignore_ascii_case(package))
        else {
            let supported = WINGET_PACKAGES
                .iter()
                .map(|candidate| candidate.display)
                .collect::<Vec<_>>()
                .join("、");
            return Err(format!(
                "Windows 当前仅支持一键安装 {supported}，无法安装: {package}"
            ));
        };
        todo.push(entry);
    }

    for entry in todo {
        if (entry.installed)() {
            continue;
        }
        let script = winget_install_script(entry.winget_id);
        let output = HiddenCommand::new("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .output()
            .map_err(|e| format!("启动 {} 安装器失败: {e}", entry.display))?;

        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            return Err(install_failure_message(
                entry.display,
                code,
                &output.stdout,
                &output.stderr,
            ));
        }

        if !(entry.installed)() {
            return Err(format!(
                "{} 已安装完成，但新的 PATH 对正在运行的应用不可见——重启应用后生效。",
                entry.display
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_script_uses_ascii_markers_and_catches_uac_cancellation() {
        for entry in WINGET_PACKAGES {
            let script = winget_install_script(entry.winget_id);
            assert!(script.is_ascii());
            assert!(script.contains(INSTALL_CANCELLED_MARKER));
            assert!(script.contains(WINGET_MISSING_MARKER));
            assert!(script.contains("NativeErrorCode"));
            assert!(script.contains(entry.winget_id));
        }
    }

    /// git 必须可一键安装：多智能体的并行隔离依赖它，而 Windows 最常缺。
    #[test]
    fn git_is_one_click_installable() {
        assert!(WINGET_PACKAGES
            .iter()
            .any(|entry| entry.package == "git" && entry.winget_id == "Git.Git"));
    }

    #[test]
    fn cancellation_marker_returns_clean_localized_message() {
        let message =
            install_failure_message("LibreOffice", 1, INSTALL_CANCELLED_MARKER.as_bytes(), b"");
        assert_eq!(message, "已取消 LibreOffice 安装。");
        assert!(!message.contains('\u{fffd}'));
    }

    #[test]
    fn base64_error_round_trips_utf8_without_error_record_noise() {
        let detail = "无法启动提升权限的安装进程";
        let encoded = BASE64_STANDARD.encode(detail.as_bytes());
        let stdout = format!("{INSTALL_ERROR_PREFIX}{encoded}");
        let message = install_failure_message("LibreOffice", 1, stdout.as_bytes(), b"");
        assert_eq!(
            message,
            "LibreOffice 安装失败 (exit 1): 无法启动提升权限的安装进程"
        );
    }

    #[test]
    fn invalid_system_code_page_output_is_not_rendered_as_mojibake() {
        let message = install_failure_message("LibreOffice", 1, b"", &[0xD3, 0xC3, 0xBB, 0xA7]);
        assert_eq!(
            message,
            "LibreOffice 安装失败 (exit 1)。请检查 winget 是否可用，或手动安装 LibreOffice。"
        );
        assert!(!message.contains('\u{fffd}'));
    }
}
