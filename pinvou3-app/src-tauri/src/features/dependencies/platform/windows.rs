use super::windows_install_text::{install_failure_message, libreoffice_install_script};
use crate::platform::process::HiddenCommand;

const LIBREOFFICE_PACKAGE: &str = "libreoffice";

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
