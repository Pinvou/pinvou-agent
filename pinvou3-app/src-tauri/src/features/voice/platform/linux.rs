use std::path::PathBuf;

use tauri::Emitter;

use super::super::voice_asr::{self, AsrModelSpec};

pub fn engine_binary_name() -> &'static str {
    "sense-voice-main"
}

pub fn bundled_engine_intact(
    _path: &std::path::Path,
    _bundled_dir: Option<&std::path::Path>,
) -> bool {
    true
}

const ASR_MODEL_URL: &str =
    "https://www.modelscope.cn/models/lovemefan/SenseVoiceGGUF/resolve/master/sense-voice-small-q4_k.gguf";
const ASR_MODEL_MIRROR_URL: &str =
    "https://huggingface.co/lovemefan/sense-voice-gguf/resolve/main/sense-voice-small-q4_k.gguf";
const ASR_MODEL_SIZE: u64 = 182_278_688;
const ASR_MODEL_SHA256: &str = "c8e7bf77acd860c5b83d2106da44aa7b985026ef4e7dbf5236c7f0f4001d9e9b";

pub fn asr_tool_path() -> PathBuf {
    for name in [
        "PINVOU3_ASR_CMD",
        "PINVOU3_DEEPSPEECH2_CMD",
        "PADDLESPEECH_BIN",
    ] {
        if let Ok(path) = std::env::var(name) {
            if !path.trim().is_empty() {
                return PathBuf::from(path);
            }
        }
    }
    PathBuf::from("pinvou-asr")
}

pub fn asr_model_spec() -> AsrModelSpec {
    AsrModelSpec {
        id: "sensevoice-q4-k",
        filename: "sense-voice-small-q4_k.gguf",
        expected_size: ASR_MODEL_SIZE,
        sha256: ASR_MODEL_SHA256,
        primary_url: ASR_MODEL_URL,
        mirror_url: ASR_MODEL_MIRROR_URL,
    }
}

pub fn asr_model_path() -> PathBuf {
    voice_asr::model_download_path()
}

pub fn asr_model_exists() -> bool {
    voice_asr::model_available()
}

pub fn asr_tool_exists() -> bool {
    let configured = asr_tool_path();
    if configured.is_file() || crate::platform::os::command_exists(&configured.to_string_lossy()) {
        return true;
    }
    voice_asr::engine_path().is_file()
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
    if !voice_asr::ffmpeg_available() {
        let _ = app.emit(
            "voice_asr:progress",
            serde_json::json!({ "stage": "ffmpeg", "downloaded": 0, "total": 0 }),
        );
        tokio::task::spawn_blocking(|| {
            // 该 spawn_blocking 闭包不持有 app 句柄,无法把 brew 式逐行进度透传给
            // 前端,故显式传 None:行为与新增 progress 回调前一致(静默安装 ffmpeg)。
            crate::features::dependencies::install_dependencies(vec!["ffmpeg".to_string()], None)
        })
        .await
        .map_err(|e| format!("ffmpeg install task failed: {e}"))??;
    }
    if !voice_asr::model_available() {
        voice_asr::download_current_model(&app).await?;
    }
    Ok(())
}

pub fn asr_dependency_packages() -> &'static str {
    "安装 pinvou ASR runtime，或设置 PINVOU3_ASR_CMD"
}

pub fn asr_missing_message() -> &'static str {
    "本地语音识别需要 SenseVoice/FunASR 运行时，请安装 pinvou ASR runtime，或通过 PINVOU3_ASR_CMD 指向 pinvou-asr。"
}

/// Linux 用内置 SenseVoice 引擎识别（区别于 macOS 的系统 Speech）。
///
/// 引擎/模型就绪时走内置 Rust 转码+识别；否则返回 `None`，由调用方回退 CLI。
pub fn recognize_native(
    wav_path: &std::path::Path,
    _locale_tag: &str,
) -> Option<Result<String, String>> {
    // Rust 内置路径只接受由 voice_asr 管理的引擎和模型。外部 CLI 即使存在，
    // 也必须返回 None 交给 run_local_asr_cli，不能误送进 Rust transcribe。
    if voice_asr::engine_path().is_file() && voice_asr::model_path().is_file() {
        Some(voice_asr::transcribe(wav_path))
    } else {
        None
    }
}

/// 原生识别后端的来源标签（用于前端展示/日志区分）。
pub fn native_recognition_source() -> &'static str {
    "pinvou-webview-sensevoice-local"
}

pub async fn reset_microphone_permission(_window: tauri::WebviewWindow) -> Result<bool, String> {
    // Linux webkit2gtk 的麦克风放行是 lib.rs 里无状态的 permission-request 处理器
    // （每次 getUserMedia 即时 allow），应用侧没有可重置的持久授权；返回 false
    // 让前端保留原始失败文案（此时失败多在系统层 pipewire/设备，重置无济于事）。
    Ok(false)
}
