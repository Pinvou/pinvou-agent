use crate::process::HiddenCommand;

const LIBREOFFICE_PACKAGE: &str = "libreoffice";
const LIBREOFFICE_WINGET_ID: &str = "TheDocumentFoundation.LibreOffice";

pub fn install_dependencies(packages: Vec<String>) -> Result<(), String> {
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

    if super::windows_system::command_exists("soffice")
        || super::windows_system::command_exists("libreoffice")
    {
        return Ok(());
    }

    let script = format!(
        "$ErrorActionPreference = 'Stop'; \
         $winget = (Get-Command winget.exe -ErrorAction SilentlyContinue).Source; \
         if ([string]::IsNullOrWhiteSpace($winget)) {{ \
           Write-Error '未找到 winget。请安装 App Installer，或从 https://www.libreoffice.org/download/download-libreoffice/ 手动安装 LibreOffice。'; \
           exit 127 \
         }} \
         $args = @('install','--id','{LIBREOFFICE_WINGET_ID}','--exact','--source','winget','--accept-source-agreements','--accept-package-agreements','--silent'); \
         $p = Start-Process -FilePath $winget -ArgumentList $args -Verb RunAs -Wait -PassThru; \
         exit $p.ExitCode"
    );
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
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        let code = output.status.code().unwrap_or(-1);
        return Err(if detail.is_empty() {
            format!("LibreOffice 安装失败或已取消 (exit {code})")
        } else {
            format!("LibreOffice 安装失败或已取消 (exit {code}): {detail}")
        });
    }

    if super::windows_system::command_exists("soffice")
        || super::windows_system::command_exists("libreoffice")
    {
        Ok(())
    } else {
        Err(
            "LibreOffice 安装器已结束，但未找到 soffice.exe；请重新打开应用或手动确认 LibreOffice 已安装。"
                .into(),
        )
    }
}
