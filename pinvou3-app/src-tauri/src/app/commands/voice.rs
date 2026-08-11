use super::prelude::*;
use anyhow::{Context, Result as AnyResult};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug, Deserialize)]
pub struct VoiceTranscriptionRequest {
    /// WAV bytes captured by the WebView.
    pub audio_bytes: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub struct VoiceTranscriptionResponse {
    pub text: String,
    pub source: String,
}

#[derive(Debug, Deserialize)]
pub struct VoicePostprocessRequest {
    pub text: String,
    pub mode: String,
    pub session_id: Option<String>,
    pub draft_text: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VoicePostprocessResponse {
    pub text: String,
    pub mode: String,
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

fn local_asr_command_name() -> String {
    crate::features::voice::asr_tool_path()
        .to_string_lossy()
        .into_owned()
}

fn local_asr_model_name() -> String {
    std::env::var("PINVOU3_ASR_MODEL")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_MODEL"))
        .unwrap_or_else(|_| "sensevoice-q8".to_string())
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
        source: "local_cli".to_string(),
    })
}

/// Transcribe a short one-shot voice capture from the desktop WebView using
/// local SenseVoice/FunASR ASR.
#[tauri::command]
pub async fn transcribe_voice_audio(
    request: VoiceTranscriptionRequest,
) -> Result<VoiceTranscriptionResponse, VoiceCommandError> {
    if request.audio_bytes.len() < 44 {
        return Err(VoiceCommandError::new(
            "recording_failed",
            "recording",
            "Recorded audio is empty or invalid.",
        ));
    }

    let wav_path = voice_temp_wav_path();
    let audio_bytes = request.audio_bytes;
    let asr_output = tokio::task::spawn_blocking(move || {
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
            // 识别语言跟随 UI 语言偏好（首次启动时由系统 locale 决定）：避免 UI 与
            // 识别模型语言错配，例如中文 UI 却把中文音频当英文解析。
            let locale_tag = crate::platform::prefs::UserPrefs::load()
                .language
                .speech_recognition_locale();
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

fn normalize_voice_postprocess_mode(mode: &str) -> &'static str {
    match mode.trim() {
        "task" | "rewrite" | "task_rewrite" => "task",
        _ => "dictation",
    }
}

fn voice_postprocess_prompt(mode: &str) -> &'static str {
    if mode == "task" {
        r#"你是 Pinvou 的语音 ASR 纠错器，只负责把 ASR 文本纠正为用户原本想说的话。

强规则：
1. 不回答问题，不执行任务。
2. 不新增用户没说过的目标、工具、格式、数量、时间、条件。
3. 不把输出形态改掉：用户说图表就保留图表，不要改成表格；用户说输入框就不要发送。
4. 正常查询、比较、搜索、整理、生成、做、把、帮我等句子都必须保留原请求，不能输出空字符串。
5. 只有整句去掉标点后只剩“嗯/啊/呃/额/那个/就是”等口头禅，才输出空字符串。
6. 优先纠正上下文中明显 ASR 错词：
   - 行情/价格查询里的“进价/惊吓”通常应修为“金价”
   - 数据分析可视化里的“图标”通常应修为“图表”
   - “屁屁提/PPTT”通常应修为“PPT”
   - “销售暑假”通常应修为“销售数据”
   - “截止事件”通常应修为“截止时间”
   - “负责任”通常应修为“负责人”
   - “风险电”通常应修为“风险点”
   - “g p t five”通常应修为“GPT-5”，“克劳德 sonnet”通常应修为“Claude Sonnet”
   - 搜索“爱新闻”通常应修为“AI 新闻”
7. 去掉口头禅、重复词和误识别语气词。
8. 只输出最终文本，不解释，不使用 Markdown。

示例：
ASR 文本：今天天气怎么样？
最终文本：今天天气怎么样？
ASR 文本：嗯。
最终文本：
ASR 文本：搜索一下今天的爱新闻，按重要性排序。
最终文本：搜索一下今天的 AI 新闻，按重要性排序。"#
    } else {
        r#"你是 Pinvou 的语音 ASR 纠错器，只负责把 ASR 文本纠正为用户原本想说的话。

强规则：
1. 不回答问题，不执行任务。
2. 不新增用户没说过的目标、工具、格式、数量、时间、条件。
3. 不把输出形态改掉：用户说图表就保留图表，不要改成表格；用户说输入框就不要发送。
4. 正常查询、比较、搜索、整理、生成、做、把、帮我等句子都必须保留原请求，不能输出空字符串。
5. 只有整句去掉标点后只剩“嗯/啊/呃/额/那个/就是”等口头禅，才输出空字符串。
6. 优先纠正上下文中明显 ASR 错词：
   - 行情/价格查询里的“进价/惊吓”通常应修为“金价”
   - 数据分析可视化里的“图标”通常应修为“图表”
   - “屁屁提/PPTT”通常应修为“PPT”
   - “销售暑假”通常应修为“销售数据”
   - “截止事件”通常应修为“截止时间”
   - “负责任”通常应修为“负责人”
   - “风险电”通常应修为“风险点”
   - “g p t five”通常应修为“GPT-5”，“克劳德 sonnet”通常应修为“Claude Sonnet”
   - 搜索“爱新闻”通常应修为“AI 新闻”
7. 去掉口头禅、重复词和误识别语气词。
8. 只输出最终文本，不解释，不使用 Markdown。

示例：
ASR 文本：今天天气怎么样？
最终文本：今天天气怎么样？
ASR 文本：嗯。
最终文本：
ASR 文本：搜索一下今天的爱新闻，按重要性排序。
最终文本：搜索一下今天的 AI 新闻，按重要性排序。"#
    }
}

fn voice_postprocess_timeout(mode: &str, raw_text: &str) -> Duration {
    if normalize_voice_postprocess_mode(mode) == "task" {
        return Duration::from_millis(2500);
    }
    let compact_len = raw_text
        .chars()
        .filter(|ch| {
            !ch.is_whitespace() && !"。！？!?，,、；;：:\"'“”‘’（）()【】[]….-—".contains(*ch)
        })
        .count();
    if compact_len <= 18 {
        Duration::from_millis(1500)
    } else {
        Duration::from_millis(2000)
    }
}

fn voice_postprocess_user_content(raw_text: &str, draft_text: Option<&str>) -> String {
    let draft = draft_text.unwrap_or("").trim();
    if draft.is_empty() {
        format!("ASR 文本：\n{}", raw_text.trim())
    } else {
        format!(
            "当前输入框已有文本：\n{}\n\nASR 文本：\n{}",
            draft,
            raw_text.trim()
        )
    }
}

fn sanitize_voice_postprocess_output(text: &str) -> String {
    text.trim()
        .trim_matches('\u{feff}')
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

async fn voice_postprocess_bridge(
    session_id: Option<&str>,
    pool: &EnginePool,
    store: &SessionStore,
) -> AnyResult<crate::features::assistant::platform::bridge::Pinvou3Bridge> {
    if let Some(sid) = session_id.filter(|sid| !sid.trim().is_empty()) {
        return pool
            .fresh_bridge_for(sid)
            .await
            .context("prepare session model for voice postprocess");
    }
    if let Some(sid) = store.active_id() {
        return pool
            .fresh_bridge_for(&sid)
            .await
            .context("prepare active session model for voice postprocess");
    }
    let mut bridge = pool.bridge.clone();
    bridge.prefs = UserPrefs::load();
    bridge.session_model = bridge.prefs.active_model().cloned();
    Ok(bridge)
}

async fn call_voice_postprocess_model(
    bridge: &crate::features::assistant::platform::bridge::Pinvou3Bridge,
    mode: &str,
    raw_text: &str,
    draft_text: Option<&str>,
) -> AnyResult<String> {
    let client = Client::builder()
        .timeout(voice_postprocess_timeout(mode, raw_text))
        .build()
        .context("build voice postprocess client")?;
    let base_url = bridge.base_url();
    let model_name = if bridge.provider() == "vllm" {
        crate::features::monitor::probe_vllm_model_info(&base_url)
            .await
            .0
            .unwrap_or_else(|| bridge.model())
    } else {
        bridge.model()
    };
    let system = voice_postprocess_prompt(mode);
    let user = voice_postprocess_user_content(raw_text, draft_text);
    let preset = bridge
        .effective_model_owned()
        .map(|model| model.preset)
        .unwrap_or_else(|| bridge.prefs.advanced.model_preset.unwrap_or_default());

    if preset == crate::platform::prefs::ModelPreset::Anthropic {
        let content = crate::core::model_endpoint::post_anthropic_messages(
            &client,
            &base_url,
            &bridge.api_key(),
            &model_name,
            system,
            &user,
            128,
        )
        .await?;
        return Ok(sanitize_voice_postprocess_output(&content));
    }

    let body = json!({
        "model": model_name,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "temperature": 0,
        "max_tokens": 128,
        "stream": false
    });
    let resp = client
        .post(format!(
            "{}/chat/completions",
            base_url.trim_end_matches('/')
        ))
        .bearer_auth(bridge.api_key())
        .json(&body)
        .send()
        .await
        .context("post voice postprocess chat/completions")?
        .error_for_status()
        .context("voice postprocess chat/completions status")?;
    let value: Value = resp
        .json()
        .await
        .context("parse voice postprocess response json")?;
    let content = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(sanitize_voice_postprocess_output(content))
}

#[tauri::command]
pub async fn postprocess_voice_text(
    request: VoicePostprocessRequest,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<VoicePostprocessResponse, String> {
    let raw_text = request.text.trim();
    if raw_text.is_empty() {
        return Ok(VoicePostprocessResponse {
            text: String::new(),
            mode: normalize_voice_postprocess_mode(&request.mode).to_string(),
            source: "empty".to_string(),
        });
    }
    let mode = normalize_voice_postprocess_mode(&request.mode);
    let bridge = voice_postprocess_bridge(request.session_id.as_deref(), &pool, &store)
        .await
        .map_err(|error| format!("prepare voice postprocess model: {error:#}"))?;
    let text = call_voice_postprocess_model(&bridge, mode, raw_text, request.draft_text.as_deref())
        .await
        .map_err(|error| format!("voice postprocess failed: {error:#}"))?;
    Ok(VoicePostprocessResponse {
        text: if text.trim().is_empty() {
            raw_text.to_string()
        } else {
            text
        },
        mode: mode.to_string(),
        source: "llm".to_string(),
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
