use std::ffi::OsStr;
use std::process::Command;

use super::linux_path;

pub fn open_target(target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(target.as_ref())
        .spawn()
        .map_err(|e| format!("系统打开失败({label}): {e}"))?;
    Ok(())
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

pub fn pandoc_tool_exists() -> bool {
    command_exists("pandoc")
}

pub fn show_pandoc_dependency_check() -> bool {
    true
}

pub fn pandoc_dependency_packages() -> &'static str {
    "pandoc"
}

pub fn pandoc_missing_message() -> &'static str {
    "文档解析需要 pandoc，请运行: sudo apt install pandoc"
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

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    vec![
        "nvidia-smi",
        "/usr/bin/nvidia-smi",
        "/usr/local/bin/nvidia-smi",
    ]
}
