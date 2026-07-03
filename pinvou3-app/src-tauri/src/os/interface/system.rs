use std::ffi::OsStr;

use std::path::PathBuf;

pub fn open_target(target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    super::super::platform::open_target(target, label)
}

pub fn command_exists(command: &str) -> bool {
    super::super::platform::command_exists(command)
}

pub fn pandoc_tool_path() -> PathBuf {
    super::super::platform::pandoc_tool_path()
}

pub fn libreoffice_tool_path() -> PathBuf {
    super::super::platform::libreoffice_tool_path()
}

pub fn ocr_tool_path() -> PathBuf {
    super::super::platform::ocr_tool_path()
}

pub fn ocr_tessdata_dir() -> Option<PathBuf> {
    super::super::platform::ocr_tessdata_dir()
}

pub fn asr_tool_path() -> PathBuf {
    super::super::platform::asr_tool_path()
}

pub fn asr_model_filename() -> &'static str {
    super::super::platform::asr_model_filename()
}

pub fn archive_tool_path() -> PathBuf {
    super::super::platform::archive_tool_path()
}

pub fn pandoc_tool_exists() -> bool {
    super::super::platform::pandoc_tool_exists()
}

pub fn ocr_tool_exists() -> bool {
    super::super::platform::ocr_tool_exists()
}

pub fn asr_tool_exists() -> bool {
    super::super::platform::asr_tool_exists()
}

pub fn asr_bundled_runtime_status() -> Option<bool> {
    super::super::platform::asr_bundled_runtime_status()
}

pub fn asr_dependency_installable() -> bool {
    super::super::platform::asr_dependency_installable()
}

pub fn asr_install_unavailable_message() -> &'static str {
    super::super::platform::asr_install_unavailable_message()
}

pub async fn install_asr_runtime(app: tauri::AppHandle) -> Result<(), String> {
    super::super::platform::install_asr_runtime(app).await
}

pub fn archive_tool_exists() -> bool {
    super::super::platform::archive_tool_exists()
}

pub fn msg_native_supported() -> bool {
    super::super::platform::msg_native_supported()
}

pub fn msg_converter_required() -> bool {
    super::super::platform::msg_converter_required()
}

pub fn email_tool_exists() -> bool {
    super::super::platform::email_tool_exists()
}

pub fn show_pandoc_dependency_check() -> bool {
    super::super::platform::show_pandoc_dependency_check()
}

pub fn show_ocr_dependency_check() -> bool {
    super::super::platform::show_ocr_dependency_check()
}

pub fn show_archive_dependency_check() -> bool {
    super::super::platform::show_archive_dependency_check()
}

pub fn pandoc_dependency_packages() -> &'static str {
    super::super::platform::pandoc_dependency_packages()
}

pub fn asr_dependency_packages() -> &'static str {
    super::super::platform::asr_dependency_packages()
}

pub fn archive_dependency_packages() -> &'static str {
    super::super::platform::archive_dependency_packages()
}

pub fn email_dependency_packages() -> &'static str {
    super::super::platform::email_dependency_packages()
}

pub fn pandoc_missing_message() -> &'static str {
    super::super::platform::pandoc_missing_message()
}

pub fn asr_missing_message() -> &'static str {
    super::super::platform::asr_missing_message()
}

pub fn pdf_tool_path(command: &str) -> PathBuf {
    super::super::platform::pdf_tool_path(command)
}

pub fn pdf_tool_exists(command: &str) -> bool {
    super::super::platform::pdf_tool_exists(command)
}

pub fn show_pdf_dependency_check() -> bool {
    super::super::platform::show_pdf_dependency_check()
}

pub fn pdf_dependency_packages() -> &'static str {
    super::super::platform::pdf_dependency_packages()
}

pub fn ocr_dependency_packages() -> &'static str {
    super::super::platform::ocr_dependency_packages()
}

pub fn pdf_text_missing_message() -> &'static str {
    super::super::platform::pdf_text_missing_message()
}

pub fn pdf_render_missing_message() -> &'static str {
    super::super::platform::pdf_render_missing_message()
}

pub fn pdf_ocr_missing_message() -> &'static str {
    super::super::platform::pdf_ocr_missing_message()
}

pub fn presentation_pdf_missing_message() -> &'static str {
    super::super::platform::presentation_pdf_missing_message()
}

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    super::super::platform::nvidia_smi_candidates()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_smi_candidates_starts_with_generic_command() {
        let candidates = nvidia_smi_candidates();
        if !candidates.is_empty() {
            assert_eq!(candidates.first().copied(), Some("nvidia-smi"));
        }
    }

    #[test]
    fn pdf_tool_path_returns_non_empty_program() {
        assert!(!pdf_tool_path("pdftotext").as_os_str().is_empty());
    }

    #[test]
    fn pandoc_tool_path_returns_non_empty_program() {
        assert!(!pandoc_tool_path().as_os_str().is_empty());
    }

    #[test]
    fn libreoffice_tool_path_returns_non_empty_program() {
        assert!(!libreoffice_tool_path().as_os_str().is_empty());
    }

    #[test]
    fn ocr_tool_path_returns_non_empty_program() {
        assert!(!ocr_tool_path().as_os_str().is_empty());
    }

    #[test]
    fn asr_tool_path_returns_non_empty_program() {
        assert!(!asr_tool_path().as_os_str().is_empty());
    }

    #[test]
    fn archive_tool_path_returns_non_empty_program() {
        assert!(!archive_tool_path().as_os_str().is_empty());
    }
}
