use std::path::PathBuf;

use super::super::voice_asr::AsrModelSpec;

pub fn asr_tool_path() -> PathBuf {
    PathBuf::from("paddlespeech")
}

pub fn asr_model_spec() -> AsrModelSpec {
    AsrModelSpec {
        id: "unsupported",
        filename: "sense-voice-small-q4_k.gguf",
        expected_size: 0,
        sha256: "",
        primary_url: "",
        mirror_url: "",
    }
}

pub fn asr_model_path() -> PathBuf {
    crate::bridge::paths::pinvou3_home()
        .join("asr")
        .join(asr_model_spec().filename)
}

pub fn asr_model_exists() -> bool {
    false
}

pub fn asr_tool_exists() -> bool {
    false
}

pub fn asr_bundled_runtime_status() -> Option<bool> {
    None
}

pub fn asr_dependency_installable() -> bool {
    false
}

pub fn asr_install_unavailable_message() -> &'static str {
    "ASR runtime installation is not supported on this platform."
}

pub async fn install_asr_runtime(_app: tauri::AppHandle) -> Result<(), String> {
    Err(asr_install_unavailable_message().to_string())
}

pub fn asr_dependency_packages() -> &'static str {
    ""
}

pub fn asr_missing_message() -> &'static str {
    "当前平台暂不支持本地语音识别"
}

pub async fn reset_microphone_permission(_window: tauri::WebviewWindow) -> Result<bool, String> {
    Ok(false)
}
