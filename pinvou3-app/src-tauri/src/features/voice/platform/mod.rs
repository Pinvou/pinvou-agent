#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "macos")]
mod voice_asr_speech;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub use unsupported::*;
#[cfg(target_os = "windows")]
pub use windows::*;

/// 环境变量指向的 ASR 工具路径：`PINVOU3_ASR_CMD` / `PINVOU3_DEEPSPEECH2_CMD` /
/// `PADDLESPEECH_BIN`，取首个非空配置。三平台的探测循环唯一实现在此；命中后
/// 的兜底策略（打包引擎路径 / PATH 名）留在各自平台的 `asr_tool_path`。
pub(crate) fn asr_tool_path_from_env() -> Option<std::path::PathBuf> {
    for name in [
        "PINVOU3_ASR_CMD",
        "PINVOU3_DEEPSPEECH2_CMD",
        "PADDLESPEECH_BIN",
    ] {
        if let Ok(path) = std::env::var(name) {
            if !path.trim().is_empty() {
                return Some(std::path::PathBuf::from(path));
            }
        }
    }
    None
}

// linux/macOS 共用的 SenseVoice GGUF（q4_k）模型契约。windows 打包不同量化
// 模型（sensevoice-small-q8），unsupported 平台为占位实现——两者的
// `asr_model_spec` 有意不同，保留在各自模块（经下方 glob re-export 暴露）。
#[cfg(any(target_os = "linux", target_os = "macos"))]
const ASR_MODEL_URL: &str = "https://www.modelscope.cn/models/lovemefan/SenseVoiceGGUF/resolve/master/sense-voice-small-q4_k.gguf";
#[cfg(any(target_os = "linux", target_os = "macos"))]
const ASR_MODEL_MIRROR_URL: &str =
    "https://huggingface.co/lovemefan/sense-voice-gguf/resolve/main/sense-voice-small-q4_k.gguf";
#[cfg(any(target_os = "linux", target_os = "macos"))]
const ASR_MODEL_SIZE: u64 = 182_278_688;
#[cfg(any(target_os = "linux", target_os = "macos"))]
const ASR_MODEL_SHA256: &str = "c8e7bf77acd860c5b83d2106da44aa7b985026ef4e7dbf5236c7f0f4001d9e9b";

/// linux/macOS 的当前模型规格（windows/unsupported 平台各有自己的实现）。
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn asr_model_spec() -> super::voice_asr::AsrModelSpec {
    super::voice_asr::AsrModelSpec {
        id: "sensevoice-q4-k",
        filename: "sense-voice-small-q4_k.gguf",
        expected_size: ASR_MODEL_SIZE,
        sha256: ASR_MODEL_SHA256,
        primary_url: ASR_MODEL_URL,
        mirror_url: ASR_MODEL_MIRROR_URL,
    }
}
