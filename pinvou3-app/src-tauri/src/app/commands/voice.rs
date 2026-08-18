use super::prelude::*;
use base64::Engine as _;

const MAX_VOICE_WAV_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct VoiceTranscriptionRequest {
    /// Legacy transport retained for compatibility with older callers.
    #[serde(default)]
    pub audio_bytes: Vec<u8>,
    /// Compact desktop transport for up to 60 seconds of PCM16 WAV audio.
    #[serde(default)]
    pub audio_base64: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VoiceTranscriptionResponse {
    pub text: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct VoiceCommandError {
    pub category: String,
    pub stage: String,
    pub message: String,
}

impl VoiceCommandError {
    pub(crate) fn new(category: &str, stage: &str, message: impl Into<String>) -> Self {
        Self {
            category: category.to_string(),
            stage: stage.to_string(),
            message: message.into(),
        }
    }
}

impl VoiceTranscriptionRequest {
    pub(super) fn into_audio_bytes(self) -> Result<Vec<u8>, VoiceCommandError> {
        let has_legacy_bytes = !self.audio_bytes.is_empty();
        let has_base64 = self
            .audio_base64
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
        if has_legacy_bytes && has_base64 {
            return Err(VoiceCommandError::new(
                "recording_failed",
                "recording",
                "Voice request must use exactly one audio transport.",
            ));
        }
        let audio_bytes = if has_base64 {
            base64::engine::general_purpose::STANDARD
                .decode(self.audio_base64.as_deref().unwrap_or_default())
                .map_err(|error| {
                    VoiceCommandError::new(
                        "recording_failed",
                        "recording",
                        format!("Voice WAV base64 is invalid: {error}"),
                    )
                })?
        } else {
            self.audio_bytes
        };
        if audio_bytes.len() > MAX_VOICE_WAV_BYTES {
            return Err(VoiceCommandError::new(
                "recording_failed",
                "recording",
                format!("Recorded voice WAV exceeds {MAX_VOICE_WAV_BYTES} bytes."),
            ));
        }
        Ok(audio_bytes)
    }
}

fn local_asr_command_name() -> String {
    crate::features::voice::asr_tool_path()
        .to_string_lossy()
        .into_owned()
}

fn local_asr_model_name() -> String {
    std::env::var("PINVOU3_ASR_MODEL")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_MODEL"))
        .unwrap_or_else(|_| crate::features::voice::default_asr_model_name().to_string())
}

fn local_asr_source(model: &str) -> String {
    std::env::var("PINVOU3_ASR_SOURCE")
        .ok()
        .map(|source| source.trim().to_string())
        .filter(|source| !source.is_empty())
        .unwrap_or_else(|| {
            if model.to_ascii_lowercase().contains("qwen3-asr") {
                "pinvou-webview-qwen3-asr-openvino-gpu".to_string()
            } else {
                "local_cli".to_string()
            }
        })
}

fn local_asr_language() -> String {
    std::env::var("PINVOU3_ASR_LANG")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_LANG"))
        .unwrap_or_else(|_| "zh".to_string())
}

fn local_asr_timeout() -> std::time::Duration {
    let secs = std::env::var("PINVOU3_ASR_TIMEOUT_SECS")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_TIMEOUT_SECS"))
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(60);
    std::time::Duration::from_secs(secs)
}

pub(super) fn has_nonempty_asr_cli_config(values: &[Option<&str>]) -> bool {
    values
        .iter()
        .flatten()
        .any(|value| !value.trim().is_empty())
}

fn has_explicit_asr_cli_fallback() -> bool {
    let values = [
        std::env::var("PINVOU3_ASR_CMD").ok(),
        std::env::var("PINVOU3_DEEPSPEECH2_CMD").ok(),
        std::env::var("PADDLESPEECH_BIN").ok(),
    ];
    has_nonempty_asr_cli_config(&[
        values[0].as_deref(),
        values[1].as_deref(),
        values[2].as_deref(),
    ])
}

/// Ask the resident ASR service to refresh an idle GPU pipeline while the user
/// is recording. Non-resident backends report that no prewarm was performed.
#[tauri::command]
pub async fn prewarm_voice_asr() -> Result<bool, String> {
    tokio::task::spawn_blocking(|| {
        crate::features::voice::prewarm_audio_backend(std::time::Duration::from_secs(10))
            .unwrap_or(Ok(false))
    })
    .await
    .map_err(|error| format!("Voice ASR prewarm task failed: {error}"))?
}

pub(super) fn apply_local_asr_model_env(
    command: &mut std::process::Command,
    model_path: Option<std::path::PathBuf>,
) {
    if let Some(path) = model_path.filter(|path| path.is_file()) {
        command.env("PINVOU3_SENSEVOICE_MODEL", path);
    }
}

fn voice_temp_wav_path() -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    std::env::temp_dir().join(format!("pinvou3-voice-{}-{stamp}.wav", std::process::id()))
}

struct LocalAsrOutput {
    text: String,
    /// 识别后端来源（system_speech / pinvou-webview-sensevoice-local / local_cli），
    /// 透传到 VoiceTranscriptionResponse.source 供前端/排查区分。
    source: String,
}

fn compact_process_output(stdout: &str, stderr: &str) -> String {
    let joined = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if joined.chars().count() <= 2000 {
        return joined;
    }
    let tail = joined
        .chars()
        .rev()
        .take(2000)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}

pub(super) fn parse_local_asr_text(stdout: &str, stderr: &str) -> Option<String> {
    let combined = format!("{stdout}\n{stderr}");
    let result_prefixes = [
        "result:",
        "asr result:",
        "recognition result:",
        "text:",
        "output:",
    ];

    for line in combined.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        for prefix in result_prefixes {
            if lower.starts_with(prefix) {
                let text = line[prefix.len()..].trim();
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
        if (line.starts_with("['") && line.ends_with("']"))
            || (line.starts_with("[\"") && line.ends_with("\"]"))
        {
            let text = line
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim_matches('\'')
                .trim_matches('"')
                .trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
        if lower.contains("error")
            || lower.contains("warning")
            || lower.contains("paddlespeech")
            || lower.contains("sensevoice")
            || lower.contains("funasr")
            || lower.contains("gguf")
            || lower.contains("python")
            || lower.contains("download")
            || lower.starts_with('[')
        {
            continue;
        }
        if line
            .chars()
            .any(|ch| ch.is_alphabetic() || ('\u{4e00}'..='\u{9fff}').contains(&ch))
        {
            return Some(line.to_string());
        }
    }
    None
}

fn run_local_asr_cli(wav_path: &std::path::Path) -> Result<LocalAsrOutput, VoiceCommandError> {
    use std::io::Read;
    use std::process::Stdio;

    let executable = local_asr_command_name();
    let model = local_asr_model_name();
    let language = local_asr_language();
    let timeout = local_asr_timeout();

    let mut command = std::process::Command::new(&executable);
    apply_local_asr_model_env(
        &mut command,
        Some(crate::features::voice::voice_asr::model_path()),
    );
    command
        .arg("asr")
        .arg("--model")
        .arg(&model)
        .arg("--lang")
        .arg(&language)
        .arg("--input")
        .arg(wav_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::platform::process::hide_std_console(&mut command);

    let mut child = command.spawn().map_err(|e| {
        let message = if e.kind() == std::io::ErrorKind::NotFound {
            crate::features::voice::asr_missing_message().to_string()
        } else {
            format!("Failed to start local SenseVoice/FunASR ASR: {e}")
        };
        VoiceCommandError::new("recognition_failed", "transcribing", message)
    })?;

    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(VoiceCommandError::new(
                        "recognition_failed",
                        "transcribing",
                        format!(
                            "Local SenseVoice/FunASR ASR timed out after {} seconds. Check that the downloaded q8 model and local runtime are available.",
                            timeout.as_secs()
                        ),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(VoiceCommandError::new(
                    "recognition_failed",
                    "transcribing",
                    format!("Failed while waiting for local SenseVoice/FunASR ASR: {e}"),
                ));
            }
        }
    };

    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }

    if !status.success() {
        return Err(VoiceCommandError::new(
            "recognition_failed",
            "transcribing",
            format!(
                "Local SenseVoice/FunASR ASR failed (exit {}): {}",
                status
                    .code()
                    .map_or_else(|| "unknown".to_string(), |code| code.to_string()),
                compact_process_output(&stdout, &stderr)
            ),
        ));
    }

    let text = parse_local_asr_text(&stdout, &stderr).ok_or_else(|| {
        VoiceCommandError::new(
            "empty_result",
            "transcribing",
            format!(
                "Local SenseVoice/FunASR ASR returned no usable text: {}",
                compact_process_output(&stdout, &stderr)
            ),
        )
    })?;

    Ok(LocalAsrOutput {
        text,
        source: local_asr_source(&model),
    })
}

/// Transcribe a short one-shot voice capture from the desktop WebView using
/// local SenseVoice/FunASR ASR.
#[tauri::command]
pub async fn transcribe_voice_audio(
    request: VoiceTranscriptionRequest,
    context_agent: State<'_, crate::features::pinvou_os::AsrContextAgent>,
) -> Result<VoiceTranscriptionResponse, VoiceCommandError> {
    transcribe_voice_audio_with_context(request, context_agent.current_context()).await
}

pub(crate) async fn transcribe_voice_audio_with_context(
    request: VoiceTranscriptionRequest,
    asr_context: String,
) -> Result<VoiceTranscriptionResponse, VoiceCommandError> {
    let audio_bytes = request.into_audio_bytes()?;
    if audio_bytes.len() < 44 {
        return Err(VoiceCommandError::new(
            "recording_failed",
            "recording",
            "Recorded audio is empty or invalid.",
        ));
    }

    let asr_output = tokio::task::spawn_blocking(move || {
        // System locales such as en-US can make Chinese speech produce
        // meaningless letters, so keep the app's explicit speech locale.
        let locale_tag = crate::platform::prefs::UserPrefs::load()
            .language
            .speech_recognition_locale();

        // Qwen3-ASR v3 sends WAV bytes plus the precompiled context snapshot
        // directly to the loopback-only resident
        // service. A configured CLI remains available as a rolling-upgrade
        // fallback when the resident endpoint is unavailable.
        match crate::features::voice::recognize_audio_bytes(
            &audio_bytes,
            locale_tag,
            &asr_context,
            local_asr_timeout(),
        ) {
            Some(Ok(text)) => {
                return Ok(LocalAsrOutput {
                    text,
                    source: crate::features::voice::native_recognition_source().to_string(),
                });
            }
            Some(Err(error)) if !has_explicit_asr_cli_fallback() => {
                return Err(VoiceCommandError::new(
                    "recognition_failed",
                    "transcribing",
                    error,
                ));
            }
            Some(Err(error)) => {
                log::warn!("direct resident ASR unavailable; using CLI fallback: {error}");
            }
            None => {}
        }

        let wav_path = voice_temp_wav_path();
        std::fs::write(&wav_path, &audio_bytes).map_err(|e| {
            VoiceCommandError::new(
                "recording_failed",
                "recording",
                format!("Failed to write temporary voice audio: {e}"),
            )
        })?;
        // 识别路径分支（平台中立）：
        //   macOS  → 系统 Speech 框架（免模型下载、免 ffmpeg、首次即用）
        //   Linux/Windows → 内置 SenseVoice（否则回退 CLI）
        // 平台选择封装在 `features::voice::recognize_native`（platform/ 适配器），
        // 此处只按返回值分发，不出现 cfg(target_os)。
        let result = {
            let native = crate::features::voice::recognize_native(&wav_path, locale_tag);
            match native {
                Some(Ok(text)) => Ok(LocalAsrOutput {
                    text,
                    source: crate::features::voice::native_recognition_source().to_string(),
                }),
                Some(Err(e)) => {
                    if has_explicit_asr_cli_fallback() {
                        run_local_asr_cli(&wav_path)
                    } else {
                        Err(VoiceCommandError::new(
                            "recognition_failed",
                            "transcribing",
                            e,
                        ))
                    }
                }
                None => run_local_asr_cli(&wav_path),
            }
        };
        let _ = std::fs::remove_file(&wav_path);
        result
    })
    .await
    .map_err(|e| {
        VoiceCommandError::new(
            "recognition_failed",
            "transcribing",
            format!("Local SenseVoice/FunASR ASR task failed: {e}"),
        )
    })??;

    Ok(VoiceTranscriptionResponse {
        text: asr_output.text,
        source: asr_output.source,
    })
}

use crate::features::voice::{
    microphone_permission as microphone_domain, voice_asr as voice_asr_domain,
};
use voice_asr_domain::*;

async_command_passthrough!(microphone_domain, reset_microphone_permission(window: tauri::WebviewWindow) -> Result<bool, String>);
async_command_passthrough!(voice_asr_domain, voice_asr_status() -> VoiceAsrStatus);
async_command_passthrough!(voice_asr_domain, install_voice_asr(app: AppHandle) -> Result<VoiceAsrStatus, String>);
sync_command_passthrough!(voice_asr_domain, cancel_voice_asr());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_compact_base64_voice_transport() {
        let request = VoiceTranscriptionRequest {
            audio_bytes: Vec::new(),
            audio_base64: Some("UklGRg==".to_string()),
        };
        assert_eq!(request.into_audio_bytes().expect("base64 WAV"), b"RIFF");
    }

    #[test]
    fn rejects_ambiguous_voice_transport() {
        let request = VoiceTranscriptionRequest {
            audio_bytes: vec![1],
            audio_base64: Some("Ag==".to_string()),
        };
        let error = request
            .into_audio_bytes()
            .expect_err("exactly one transport is required");
        assert_eq!(error.category, "recording_failed");
    }

    #[test]
    fn rejects_voice_payload_over_two_mib() {
        let request = VoiceTranscriptionRequest {
            audio_bytes: vec![0; MAX_VOICE_WAV_BYTES + 1],
            audio_base64: None,
        };
        assert!(request.into_audio_bytes().is_err());
    }
}
