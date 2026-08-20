use std::ffi::OsStr;

use std::path::{Path, PathBuf};

pub fn open_target(target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    super::super::platform::open_target(target, label)
}

pub fn reveal_target(target: &Path) -> Result<(), String> {
    super::super::platform::reveal_target(target)
}

pub fn command_exists(command: &str) -> bool {
    super::super::platform::command_exists(command)
}

pub fn current_system_locale() -> Option<String> {
    super::super::platform::current_system_locale()
}

#[cfg(target_os = "windows")]
pub fn bios_serial_number() -> Result<String, String> {
    super::super::platform::bios_serial_number()
}

pub fn pandoc_tool_path() -> PathBuf {
    super::super::platform::pandoc_tool_path()
}

pub fn libreoffice_tool_path() -> PathBuf {
    super::super::platform::libreoffice_tool_path()
}

pub fn libreoffice_missing_message() -> &'static str {
    super::super::platform::libreoffice_missing_message()
}

pub fn ocr_tool_path() -> PathBuf {
    super::super::platform::ocr_tool_path()
}

pub fn ocr_tessdata_dir() -> Option<PathBuf> {
    super::super::platform::ocr_tessdata_dir()
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

pub fn archive_dependency_packages() -> &'static str {
    super::super::platform::archive_dependency_packages()
}

pub fn email_dependency_packages() -> &'static str {
    super::super::platform::email_dependency_packages()
}

pub fn email_manual_hint() -> Option<&'static str> {
    super::super::platform::email_manual_hint()
}

pub fn pandoc_missing_message() -> &'static str {
    super::super::platform::pandoc_missing_message()
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

pub fn system_default_open_supported(path: &Path) -> bool {
    super::super::platform::system_default_open_supported(path)
}

pub fn libreoffice_open_fallback_needed(path: &Path) -> bool {
    super::super::platform::libreoffice_open_fallback_needed(path)
}

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    super::super::platform::nvidia_smi_candidates()
}

/// GPU 能力分级（本地引擎设备自动选择与模型档位推荐共用口径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuClass {
    /// 独显（Windows：任一适配器专用显存 ≥5.6GB；macOS 恒此档）。
    Dedicated,
    /// 强核显白名单命中（Radeon 680M/780M/880M/890M、Iris Xe、Arc Graphics）。
    StrongIgpu,
    /// 无可用 GPU（枚举失败 / Vulkan 运行时缺失 / 其余核显）→ 走 CPU 推理。
    None,
}

/// 本地引擎 GPU 分级；GPU 判定以 Vulkan/Metal 运行时可用为前提。
pub fn gpu_class() -> GpuClass {
    super::super::platform::gpu_class()
}

/// 物理 CPU 核数（llama-server `-t` 用）；各平台读取失败时回落逻辑核数。
pub fn physical_core_count() -> usize {
    super::super::platform::physical_core_count()
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
    fn archive_tool_path_returns_non_empty_program() {
        assert!(!archive_tool_path().as_os_str().is_empty());
    }
}
