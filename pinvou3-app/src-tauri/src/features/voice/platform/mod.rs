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

/// 就绪探测与执行路径（`asr_tool_path`：env 优先）必须同判定：设置了覆盖命令时，
/// 就绪 = 该命令本身可执行——执行会原样 spawn 它，打包运行时再完好也不能替它报
/// 就绪；未设置时才由各平台判定打包运行时。否则会出现"面板报就绪、转写仍 spawn
/// 失败"（env 失效却落到 bundled）或反向的假阴性。
pub(crate) fn asr_ready_decision(
    env_command: Option<&str>,
    command_exists: impl Fn(&str) -> bool,
    bundled_ready: bool,
) -> bool {
    match env_command {
        Some(command) => command_exists(command),
        None => bundled_ready,
    }
}

/// 各平台 `asr_tool_exists` 的共用骨架：取 env 覆盖命令的判定，未设置时交给
/// 平台的 `bundled_ready`。
pub(crate) fn asr_tool_exists_with_env(bundled_ready: impl FnOnce() -> bool) -> bool {
    let env_command = asr_tool_path_from_env().map(|path| path.to_string_lossy().into_owned());
    asr_ready_decision(
        env_command.as_deref(),
        |command| crate::platform::os::command_exists(command),
        bundled_ready(),
    )
}

#[cfg(test)]
mod asr_ready_tests {
    use super::asr_ready_decision;

    #[test]
    fn env_override_missing_never_falls_back_to_bundled() {
        // 覆盖命令失效时，即使打包运行时完好也必须报未就绪（执行会原样
        // spawn 失效路径并失败）。这是 Windows 探测回归的判定级 pin。
        assert!(!asr_ready_decision(Some("missing-asr"), |_| false, true));
    }

    #[test]
    fn env_override_present_is_ready_even_without_bundled() {
        assert!(asr_ready_decision(Some("asr"), |_| true, false));
        assert!(!asr_ready_decision(Some("asr"), |_| false, true));
    }

    #[test]
    fn no_env_uses_bundled_verdict() {
        assert!(asr_ready_decision(None, |_| true, true));
        assert!(!asr_ready_decision(None, |_| true, false));
    }
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
