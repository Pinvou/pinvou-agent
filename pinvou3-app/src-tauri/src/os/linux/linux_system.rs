use std::ffi::OsStr;
use std::io::Write;
use std::process::Command;

use tauri::Emitter;

use super::linux_path;

const ASR_MODEL_URL: &str =
    "https://www.modelscope.cn/models/lovemefan/SenseVoiceGGUF/resolve/master/sense-voice-small-q4_k.gguf";
const ASR_MODEL_SIZE: u64 = 182_278_688;

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
        tokio::task::spawn_blocking(|| super::linux_dependency::install_dependencies(vec!["ffmpeg".to_string()]))
            .await
            .map_err(|e| format!("ffmpeg install task failed: {e}"))??;
    }

    if !crate::voice_asr::model_path().is_file() {
        download_asr_model(&app).await?;
    }

    Ok(())
}

async fn download_asr_model(app: &tauri::AppHandle) -> Result<(), String> {
    let dir = crate::voice_asr::asr_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 ASR 目录失败: {e}"))?;
    let dest = crate::voice_asr::model_path();
    if dest
        .metadata()
        .map(|m| m.len() == ASR_MODEL_SIZE)
        .unwrap_or(false)
    {
        return Ok(());
    }

    let url = std::env::var("PINVOU3_ASR_MODEL_URL").unwrap_or_else(|_| ASR_MODEL_URL.to_string());
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .user_agent("pinvou3-asr/1.0")
        .build()
        .map_err(|e| format!("构建 ASR 模型下载客户端失败: {e}"))?;
    let mut resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("连接 ASR 模型源失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("ASR 模型源响应异常: {e}"))?;

    let total = resp.content_length().unwrap_or(ASR_MODEL_SIZE);
    let tmp = dir.join(format!("{}.part", asr_model_filename()));
    let mut file = std::fs::File::create(&tmp).map_err(|e| format!("创建 ASR 模型文件失败: {e}"))?;
    let mut downloaded: u64 = 0;
    let mut last_emit: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("ASR 模型下载中断: {e}"))?
    {
        file.write_all(&chunk)
            .map_err(|e| format!("写入 ASR 模型失败: {e}"))?;
        downloaded += chunk.len() as u64;
        if downloaded - last_emit >= 1_048_576 || downloaded == total {
            last_emit = downloaded;
            let _ = app.emit(
                "voice_asr:progress",
                serde_json::json!({ "stage": "model", "downloaded": downloaded, "total": total }),
            );
        }
    }
    drop(file);
    std::fs::rename(&tmp, &dest).map_err(|e| format!("保存 ASR 模型失败: {e}"))?;
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

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    vec![
        "nvidia-smi",
        "/usr/bin/nvidia-smi",
        "/usr/local/bin/nvidia-smi",
    ]
}
