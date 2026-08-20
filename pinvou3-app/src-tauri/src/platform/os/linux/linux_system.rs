use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use super::linux_path;

pub fn open_target(target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(target.as_ref())
        .spawn()
        .map_err(|e| format!("系统打开失败({label}): {e}"))?;
    Ok(())
}

pub fn reveal_target(target: &Path) -> Result<(), String> {
    let target = super::linux_path::platform_compat_path(&target.to_string_lossy());
    let parent = target
        .parent()
        .ok_or_else(|| format!("no parent dir for {}", target.display()))?;

    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() {
        if let Ok(url) = tauri::Url::from_directory_path(&target) {
            let items = format!("array:string:{url}");
            if Command::new("dbus-send")
                .args([
                    "--session",
                    "--dest=org.freedesktop.FileManager1",
                    "--type=method_call",
                    "/org/freedesktop/FileManager1",
                    "org.freedesktop.FileManager1.ShowItems",
                    &items,
                    "string:",
                ])
                .output()
                .is_ok_and(|output| output.status.success())
            {
                return Ok(());
            }
        }
    }
    open_target(parent, "文件所在目录")
}

pub fn command_exists(command: &str) -> bool {
    Command::new("which")
        .arg(command)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn pandoc_tool_path() -> std::path::PathBuf {
    linux_path::pandoc_tool_path()
}

pub fn libreoffice_tool_path() -> std::path::PathBuf {
    std::path::PathBuf::from("soffice")
}

pub fn ocr_tool_path() -> std::path::PathBuf {
    std::path::PathBuf::from("tesseract")
}

pub fn ocr_tessdata_dir() -> Option<std::path::PathBuf> {
    None
}

pub fn archive_tool_path() -> std::path::PathBuf {
    std::path::PathBuf::from("7z")
}

pub fn pandoc_tool_exists() -> bool {
    command_exists("pandoc")
}

pub fn ocr_tool_exists() -> bool {
    command_exists("tesseract")
}

pub fn archive_tool_exists() -> bool {
    command_exists("7z")
}

pub fn msg_native_supported() -> bool {
    false
}

pub fn msg_converter_required() -> bool {
    true
}

pub fn email_tool_exists() -> bool {
    command_exists("python3") && command_exists("msgconvert")
}

pub fn show_pandoc_dependency_check() -> bool {
    true
}

pub fn show_ocr_dependency_check() -> bool {
    true
}

pub fn show_archive_dependency_check() -> bool {
    true
}

pub fn pandoc_dependency_packages() -> &'static str {
    "pandoc"
}

pub fn archive_dependency_packages() -> &'static str {
    "p7zip-full"
}

pub fn pandoc_missing_message() -> &'static str {
    "文档解析需要 pandoc，请运行: sudo apt install pandoc"
}

pub fn libreoffice_missing_message() -> &'static str {
    "Office 文档预览需要 LibreOffice，请运行: sudo apt install libreoffice"
}

pub fn email_dependency_packages() -> &'static str {
    "python3 libemail-outlook-message-perl"
}

/// Linux 上邮件依赖可通过 apt 一键安装(libemail-outlook-message-perl),无需手动指引。
pub fn email_manual_hint() -> Option<&'static str> {
    None
}

pub fn pdf_tool_path(command: &str) -> std::path::PathBuf {
    linux_path::pdf_tool_path(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_keeps_pandoc_dependency_check_visible() {
        assert!(show_pandoc_dependency_check());
        assert_eq!(pandoc_dependency_packages(), "pandoc");
        assert!(pandoc_missing_message().contains("sudo apt install pandoc"));
    }

    #[test]
    fn linux_keeps_ocr_dependency_check_visible() {
        assert!(show_ocr_dependency_check());
        assert_eq!(
            ocr_dependency_packages(),
            "tesseract-ocr tesseract-ocr-chi-sim poppler-utils"
        );
    }

    #[test]
    fn linux_keeps_email_msgconvert_dependency_visible() {
        assert!(!msg_native_supported());
        assert!(msg_converter_required());
        assert_eq!(
            email_dependency_packages(),
            "python3 libemail-outlook-message-perl"
        );
    }

    #[test]
    fn linux_keeps_archive_dependency_check_visible() {
        assert!(show_archive_dependency_check());
        assert_eq!(archive_dependency_packages(), "p7zip-full");
        assert_eq!(archive_tool_path(), std::path::PathBuf::from("7z"));
    }
}

pub fn pdf_tool_exists(command: &str) -> bool {
    command_exists(command)
}

pub fn show_pdf_dependency_check() -> bool {
    true
}

pub fn pdf_dependency_packages() -> &'static str {
    "poppler-utils"
}

pub fn ocr_dependency_packages() -> &'static str {
    "tesseract-ocr tesseract-ocr-chi-sim poppler-utils"
}

pub fn pdf_text_missing_message() -> &'static str {
    "PDF 解析需要 pdftotext，请运行: sudo apt install poppler-utils"
}

pub fn pdf_render_missing_message() -> &'static str {
    "PDF 预览需要 poppler-utils: sudo apt install poppler-utils"
}

pub fn pdf_ocr_missing_message() -> &'static str {
    "PDF 无文字层（疑似扫描件），OCR 兜底需要 poppler-utils + tesseract: sudo apt install poppler-utils tesseract-ocr tesseract-ocr-chi-sim"
}

pub fn presentation_pdf_missing_message() -> &'static str {
    "演示文稿解析需要 LibreOffice + poppler-utils: sudo apt install libreoffice poppler-utils"
}

pub fn system_default_open_supported(_path: &Path) -> bool {
    false
}

pub fn libreoffice_open_fallback_needed(_path: &Path) -> bool {
    false
}

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    vec![
        "nvidia-smi",
        "/usr/bin/nvidia-smi",
        "/usr/local/bin/nvidia-smi",
    ]
}

/// GPU 分级（本地引擎设备自动选择）：Linux 首版恒按无 GPU（CPU 推理），
/// Vulkan 可用性探测留给后续迭代。
pub fn gpu_class() -> crate::platform::os::GpuClass {
    crate::platform::os::GpuClass::None
}

/// 物理核数（llama-server `-t` 用）：按 /sys CPU 拓扑
/// (physical_package_id, core_id) 去重；拓扑缺失时回落逻辑核数。
pub fn physical_core_count() -> usize {
    let mut cores = std::collections::HashSet::new();
    if let Ok(entries) = std::fs::read_dir("/sys/devices/system/cpu") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // 只要 cpuN 目录，跳过 cpufreq/cpuidle 等。
            if name.len() <= 3
                || !name.starts_with("cpu")
                || !name[3..].chars().all(|c| c.is_ascii_digit())
            {
                continue;
            }
            let topo = entry.path().join("topology");
            let pkg = std::fs::read_to_string(topo.join("physical_package_id")).ok();
            let core = std::fs::read_to_string(topo.join("core_id")).ok();
            if let (Some(pkg), Some(core)) = (pkg, core) {
                cores.insert((pkg.trim().to_string(), core.trim().to_string()));
            }
        }
    }
    if cores.is_empty() {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    } else {
        cores.len()
    }
}
