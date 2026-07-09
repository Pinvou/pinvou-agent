use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use tauri::Emitter;

use super::linux_path;

const ASR_MODEL_URL: &str =
    "https://www.modelscope.cn/models/lovemefan/SenseVoiceGGUF/resolve/master/sense-voice-small-q4_k.gguf";
const ASR_MODEL_MIRROR_URL: &str =
    "https://huggingface.co/lovemefan/sense-voice-gguf/resolve/main/sense-voice-small-q4_k.gguf";
const ASR_MODEL_SIZE: u64 = 182_278_688;
const ASR_MODEL_SHA256: &str = "c8e7bf77acd860c5b83d2106da44aa7b985026ef4e7dbf5236c7f0f4001d9e9b";

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

pub fn libreoffice_tool_path() -> std::path::PathBuf {
    std::path::PathBuf::from("soffice")
}

pub fn ocr_tool_path() -> std::path::PathBuf {
    std::path::PathBuf::from("tesseract")
}

pub fn ocr_tessdata_dir() -> Option<std::path::PathBuf> {
    None
}

pub fn asr_tool_path() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("PINVOU3_ASR_CMD") {
        if !path.trim().is_empty() {
            return std::path::PathBuf::from(path);
        }
    }
    if let Ok(path) = std::env::var("PINVOU3_DEEPSPEECH2_CMD") {
        if !path.trim().is_empty() {
            return std::path::PathBuf::from(path);
        }
    }
    if let Ok(path) = std::env::var("PADDLESPEECH_BIN") {
        if !path.trim().is_empty() {
            return std::path::PathBuf::from(path);
        }
    }
    std::path::PathBuf::from("pinvou-asr")
}

pub fn asr_model_filename() -> &'static str {
    "sense-voice-small-q4_k.gguf"
}

pub fn asr_model_spec() -> crate::voice_asr::AsrModelSpec {
    crate::voice_asr::AsrModelSpec {
        id: "sensevoice-q4-k",
        filename: asr_model_filename(),
        expected_size: ASR_MODEL_SIZE,
        sha256: ASR_MODEL_SHA256,
        primary_url: ASR_MODEL_URL,
        mirror_url: ASR_MODEL_MIRROR_URL,
    }
}

pub fn asr_model_path() -> std::path::PathBuf {
    crate::voice_asr::model_download_path()
}

pub fn asr_model_exists() -> bool {
    crate::voice_asr::model_available()
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

pub fn asr_tool_exists() -> bool {
    if let Ok(path) = std::env::var("PINVOU3_ASR_CMD") {
        if !path.trim().is_empty() {
            return command_exists(&path);
        }
    }
    if let Ok(path) = std::env::var("PINVOU3_DEEPSPEECH2_CMD") {
        if !path.trim().is_empty() {
            return command_exists(&path);
        }
    }
    if let Ok(path) = std::env::var("PADDLESPEECH_BIN") {
        if !path.trim().is_empty() {
            return command_exists(&path);
        }
    }
    // Bundled SenseVoice runtime, installed as an app resource or into ~/.pinvou3/asr.
    if crate::voice_asr::engine_path().is_file() {
        return true;
    }
    command_exists("pinvou-asr")
}

pub fn asr_bundled_runtime_status() -> Option<bool> {
    None
}

pub fn asr_dependency_installable() -> bool {
    true
}

pub fn asr_install_unavailable_message() -> &'static str {
    "当前 Linux 环境可通过一键安装补全语音识别依赖。"
}

pub async fn install_asr_runtime(app: tauri::AppHandle) -> Result<(), String> {
    if !crate::voice_asr::ffmpeg_available() {
        let _ = app.emit(
            "voice_asr:progress",
            serde_json::json!({ "stage": "ffmpeg", "downloaded": 0, "total": 0 }),
        );
        tokio::task::spawn_blocking(|| {
            super::linux_dependency::install_dependencies(vec!["ffmpeg".to_string()])
        })
        .await
        .map_err(|e| format!("ffmpeg install task failed: {e}"))??;
    }

    if !crate::voice_asr::model_available() {
        crate::voice_asr::download_current_model(&app).await?;
    }

    Ok(())
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

pub fn asr_dependency_packages() -> &'static str {
    "安装 pinvou ASR runtime，或设置 PINVOU3_ASR_CMD"
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

pub fn asr_missing_message() -> &'static str {
    "本地语音识别需要 SenseVoice/FunASR 运行时，请安装 pinvou ASR runtime，或通过 PINVOU3_ASR_CMD 指向 pinvou-asr。"
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
