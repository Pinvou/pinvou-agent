use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

use crate::platform::process::HiddenCommand;

const LIBREOFFICE_PACKAGE: &str = "libreoffice";
const LIBREOFFICE_WINGET_ID: &str = "TheDocumentFoundation.LibreOffice";
const INSTALL_CANCELLED_MARKER: &str = "__PINVOU_INSTALL_CANCELLED__";
const WINGET_MISSING_MARKER: &str = "__PINVOU_WINGET_MISSING__";
const INSTALL_ERROR_PREFIX: &str = "__PINVOU_INSTALL_ERROR_B64__";

fn libreoffice_install_script() -> String {
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
$args = @('install','--id','__LIBREOFFICE_WINGET_ID__','--exact','--source','winget','--accept-source-agreements','--accept-package-agreements','--silent');
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
        .replace("__LIBREOFFICE_WINGET_ID__", LIBREOFFICE_WINGET_ID)
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

fn install_failure_message(code: i32, stdout: &[u8], stderr: &[u8]) -> String {
    if marker_present(stdout, stderr, INSTALL_CANCELLED_MARKER) || code == 1223 {
        return "已取消 LibreOffice 安装。".to_string();
    }
    if marker_present(stdout, stderr, WINGET_MISSING_MARKER) || code == 127 {
        return "未找到 winget。请安装 App Installer，或手动安装 LibreOffice。".to_string();
    }
    if let Some(detail) =
        encoded_install_error(stdout, stderr).or_else(|| compact_utf8_detail(stdout, stderr))
    {
        return format!("LibreOffice 安装失败 (exit {code}): {detail}");
    }
    format!("LibreOffice 安装失败 (exit {code})。请检查 winget 是否可用，或手动安装 LibreOffice。")
}

pub fn install_dependencies(
    packages: Vec<String>,
    progress: Option<&(dyn Fn(&str, usize, usize, Option<&str>) + Sync)>,
) -> Result<(), String> {
    if packages.is_empty() {
        return Err("没有需要安装的依赖".into());
    }
    for package in &packages {
        if !package.eq_ignore_ascii_case(LIBREOFFICE_PACKAGE) {
            return Err(format!(
                "Windows 当前仅支持一键安装 LibreOffice，无法安装: {package}"
            ));
        }
    }

    if crate::platform::os::command_exists("soffice")
        || crate::platform::os::command_exists("libreoffice")
    {
        return Ok(());
    }

    // winget 安装由 UAC 弹窗驱动,无逐行输出可流式;执行前发一次粗粒度进度,
    // 让前端不至于全程只有静态「安装中…」。保持既有行为不变。
    if let Some(report) = progress {
        report(LIBREOFFICE_PACKAGE, 1, 1, None);
    }

    let script = libreoffice_install_script();
    let output = HiddenCommand::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
        .map_err(|e| format!("启动 LibreOffice 安装器失败: {e}"))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        return Err(install_failure_message(
            code,
            &output.stdout,
            &output.stderr,
        ));
    }

    if crate::platform::os::command_exists("soffice")
        || crate::platform::os::command_exists("libreoffice")
    {
        Ok(())
    } else {
        Err(
            "LibreOffice 安装器已结束，但未找到 soffice.exe；请重新打开应用或手动确认 LibreOffice 已安装。"
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_script_uses_ascii_markers_and_catches_uac_cancellation() {
        let script = libreoffice_install_script();
        assert!(script.is_ascii());
        assert!(script.contains(INSTALL_CANCELLED_MARKER));
        assert!(script.contains(WINGET_MISSING_MARKER));
        assert!(script.contains("NativeErrorCode"));
        assert!(script.contains(LIBREOFFICE_WINGET_ID));
    }

    #[test]
    fn cancellation_marker_returns_clean_localized_message() {
        let message = install_failure_message(1, INSTALL_CANCELLED_MARKER.as_bytes(), b"");
        assert_eq!(message, "已取消 LibreOffice 安装。");
        assert!(!message.contains('\u{fffd}'));
    }

    #[test]
    fn base64_error_round_trips_utf8_without_error_record_noise() {
        let detail = "无法启动提升权限的安装进程";
        let encoded = BASE64_STANDARD.encode(detail.as_bytes());
        let stdout = format!("{INSTALL_ERROR_PREFIX}{encoded}");
        let message = install_failure_message(1, stdout.as_bytes(), b"");
        assert_eq!(
            message,
            "LibreOffice 安装失败 (exit 1): 无法启动提升权限的安装进程"
        );
    }

    #[test]
    fn invalid_system_code_page_output_is_not_rendered_as_mojibake() {
        let message = install_failure_message(1, b"", &[0xD3, 0xC3, 0xBB, 0xA7]);
        assert_eq!(
            message,
            "LibreOffice 安装失败 (exit 1)。请检查 winget 是否可用，或手动安装 LibreOffice。"
        );
        assert!(!message.contains('\u{fffd}'));
    }
}
