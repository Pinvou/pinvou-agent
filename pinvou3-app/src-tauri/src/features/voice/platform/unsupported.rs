use std::path::PathBuf;

use super::super::voice_asr::AsrModelSpec;

pub fn engine_binary_name() -> &'static str {
    "sense-voice-main"
}

pub fn bundled_engine_intact(
    _path: &std::path::Path,
    _bundled_dir: Option<&std::path::Path>,
) -> bool {
    true
}

pub fn asr_tool_path() -> PathBuf {
    PathBuf::from("paddlespeech")
}

pub fn default_asr_model_name() -> &'static str {
    "sensevoice-q8"
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
    crate::platform::paths::pinvou3_home()
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

/// 不支持的平台无原生识别后端，恒返回 `None`（调用方回退 CLI）。
pub fn recognize_native(
    _wav_path: &std::path::Path,
    _locale_tag: &str,
) -> Option<Result<String, String>> {
    None
}

pub fn recognize_audio_bytes(
    _audio_bytes: &[u8],
    _locale_tag: &str,
    _context: &str,
    _timeout: std::time::Duration,
) -> Option<Result<String, String>> {
    None
}

pub fn prewarm_audio_backend(_timeout: std::time::Duration) -> Option<Result<bool, String>> {
    None
}

/// 原生识别后端的来源标签（用于前端展示/日志区分）。
pub fn native_recognition_source() -> &'static str {
    "unsupported"
}

pub async fn reset_microphone_permission(_window: tauri::WebviewWindow) -> Result<bool, String> {
    Ok(false)
}
