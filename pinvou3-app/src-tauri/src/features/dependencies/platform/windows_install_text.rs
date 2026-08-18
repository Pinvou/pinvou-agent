// Windows 安装脚本与失败文案的平台无关纯逻辑。脚本只生成 ASCII/UTF-8
// 文本、不触碰任何 Windows API,因此以 `#[cfg(any(target_os = "windows", test))]`
// 接线(见 mod.rs),让单元测试可在全平台 cargo test 下编译执行。

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

const LIBREOFFICE_WINGET_ID: &str = "TheDocumentFoundation.LibreOffice";
const INSTALL_CANCELLED_MARKER: &str = "__PINVOU_INSTALL_CANCELLED__";
const WINGET_MISSING_MARKER: &str = "__PINVOU_WINGET_MISSING__";
const INSTALL_ERROR_PREFIX: &str = "__PINVOU_INSTALL_ERROR_B64__";

pub(super) fn libreoffice_install_script() -> String {
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

pub(super) fn install_failure_message(code: i32, stdout: &[u8], stderr: &[u8]) -> String {
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
