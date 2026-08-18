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
const QWEN3_ASR_MODEL_ID: &str = "qwen3-asr-0.6b-int8-openvino";
const QWEN3_ASR_DIRNAME: &str = "qwen3-asr-openvino";
const QWEN3_ASR_LAUNCHER: &str = "qwen3-asr-openvino";

fn explicit_asr_tool_path() -> Option<PathBuf> {
    [
        "PINVOU3_ASR_CMD",
        "PINVOU3_DEEPSPEECH2_CMD",
        "PADDLESPEECH_BIN",
    ]
    .into_iter()
    .find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|path| path.trim().to_string())
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    })
}

fn qwen3_asr_root() -> PathBuf {
    std::env::var("PINVOU3_QWEN3_ASR_ROOT")
        .ok()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| voice_asr::asr_dir().join(QWEN3_ASR_DIRNAME))
}

fn qwen3_asr_launcher_path() -> PathBuf {
    qwen3_asr_root().join(QWEN3_ASR_LAUNCHER)
}

fn qwen3_asr_model_dir() -> PathBuf {
    std::env::var("PINVOU3_QWEN3_ASR_MODEL_DIR")
        .ok()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| qwen3_asr_root().join("model"))
}

fn qwen3_asr_cache_dir() -> PathBuf {
    std::env::var("PINVOU3_QWEN3_ASR_CACHE_DIR")
        .ok()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| qwen3_asr_root().join("cache"))
}

fn path_looks_like_qwen3_asr(path: &std::path::Path) -> bool {
    path.file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .map(|name| name.contains("qwen3") && name.contains("asr"))
        .unwrap_or(false)
}

fn qwen3_asr_selected() -> bool {
    if let Some(path) = explicit_asr_tool_path() {
        return path_looks_like_qwen3_asr(&path)
            || std::env::var("PINVOU3_ASR_BACKEND_KIND")
                .map(|kind| kind.eq_ignore_ascii_case("qwen3-openvino"))
                .unwrap_or(false);
    }
    qwen3_asr_launcher_path().is_file()
}

fn qwen3_asr_model_ready() -> bool {
    let model_dir = qwen3_asr_model_dir();
    [
        "config.json",
        "preprocessor_config.json",
        "openvino_encoder_model.xml",
        "openvino_encoder_model.bin",
        "openvino_decoder_model.xml",
        "openvino_decoder_model.bin",
        "openvino_tokenizer.xml",
        "openvino_tokenizer.bin",
        "openvino_detokenizer.xml",
        "openvino_detokenizer.bin",
    ]
    .into_iter()
    .all(|name| model_dir.join(name).is_file())
}

pub fn asr_tool_path() -> PathBuf {
    if let Some(path) = explicit_asr_tool_path() {
        return path;
    }
    let qwen3 = qwen3_asr_launcher_path();
    if qwen3.is_file() {
        return qwen3;
    }
    PathBuf::from("pinvou-asr")
}

pub fn default_asr_model_name() -> &'static str {
    if qwen3_asr_selected() {
        QWEN3_ASR_MODEL_ID
    } else {
        "sensevoice-q8"
    }
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
    if qwen3_asr_selected() {
        qwen3_asr_model_ready()
    } else {
        voice_asr::model_available()
    }
}

pub fn asr_tool_exists() -> bool {
    if qwen3_asr_selected() {
        return qwen3_asr_launcher_path().is_file()
            || explicit_asr_tool_path().is_some_and(|path| {
                path.is_file() || crate::platform::os::command_exists(&path.to_string_lossy())
            });
    }
    let configured = asr_tool_path();
    if configured.is_file() || crate::platform::os::command_exists(&configured.to_string_lossy()) {
        return true;
    }
    voice_asr::engine_path().is_file()
}

pub fn asr_bundled_runtime_status() -> Option<bool> {
    qwen3_asr_selected().then(asr_tool_exists)
}

pub fn asr_dependency_installable() -> bool {
    !qwen3_asr_selected()
}

pub fn asr_install_unavailable_message() -> &'static str {
    "Qwen3-ASR OpenVINO GPU 运行时或 INT8 模型缺失，请运行 Linux ASR 部署脚本。"
}

pub async fn install_asr_runtime(app: tauri::AppHandle) -> Result<(), String> {
    if qwen3_asr_selected() {
        if asr_tool_exists() && qwen3_asr_model_ready() {
            return Ok(());
        }
        return Err(asr_install_unavailable_message().to_string());
    }
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
    "部署 Qwen3-ASR 0.6B INT8 OpenVINO GPU 运行时"
}

pub fn asr_missing_message() -> &'static str {
    "本地语音识别需要 Qwen3-ASR OpenVINO GPU 常驻服务；请检查用户服务、INT8 模型和 Intel GPU 运行时。"
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

/// Send PCM16 WAV bytes directly to the resident OpenVINO GPU service. This
/// avoids temporary files and per-request Python process startup.
pub fn recognize_audio_bytes(
    audio_bytes: &[u8],
    locale_tag: &str,
    context: &str,
    timeout: std::time::Duration,
) -> Option<Result<String, String>> {
    if !qwen3_asr_selected() {
        return None;
    }
    let language = std::env::var("PINVOU3_ASR_LANG")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_LANG"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| locale_tag.to_string());
    Some(crate::features::voice::qwen3_resident::transcribe(
        &qwen3_asr_cache_dir(),
        audio_bytes,
        &language,
        context,
        timeout,
    ))
}

pub fn prewarm_audio_backend(timeout: std::time::Duration) -> Option<Result<bool, String>> {
    if !qwen3_asr_selected() {
        return None;
    }
    Some(crate::features::voice::qwen3_resident::prewarm(
        &qwen3_asr_cache_dir(),
        timeout,
    ))
}

/// 原生识别后端的来源标签（用于前端展示/日志区分）。
pub fn native_recognition_source() -> &'static str {
    if qwen3_asr_selected() {
        "pinvou-webview-qwen3-asr-openvino-gpu"
    } else {
        "pinvou-webview-sensevoice-local"
    }
}

pub async fn reset_microphone_permission(_window: tauri::WebviewWindow) -> Result<bool, String> {
    Ok(false)
}
