use super::prelude::*;
use anyhow::{Context, Result as AnyResult};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

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
        "edit" | "voice_edit" | "draft_edit" => "edit",
        _ => "dictation",
    }
}

fn voice_postprocess_prompt(mode: &str) -> &'static str {
    if mode == "edit" {
        r#"你是 Pinvou 的语音编辑器。你的唯一职责是根据用户的语音修改指令，改写“当前输入框已有文本”。

强规则：
1. 不回答问题，不执行任务，只输出修改后的完整输入框文本。
2. ASR 文本是修改指令，不是要追加到正文里的内容，除非用户明确说“加上/追加/补充”。
3. 必须保留原文中未被修改指令涉及的信息。
4. 不新增用户没说过的事实、时间、数量、条件、工具或结论。
5. 用户要求改成要点、列表或几条时，可以重排为 Markdown 列表。
6. 用户要求删除某条时，只删除明确指定的内容。
7. 用户要求替换实体时，只做对应替换。
8. 如果修改指令为空、纯噪声或无法理解，输出原文。
9. 只输出最终文本，不解释，不包裹代码块。

示例：
当前输入框已有文本：
帮我整理会议纪要，提取风险和待办，明天发给团队。

ASR 文本：
把它改成三条要点。

最终文本：
- 整理会议纪要。
- 提取风险和待办。
- 明天发给团队。"#
    } else if mode == "task" {
        r#"你是 Pinvou 的语音任务纠错器。你的唯一职责是把 ASR 文本纠正为用户原本想交给 Agent 执行的任务。

强规则：
1. 不回答问题，不执行任务。
2. 不新增用户没说过的目标、工具、格式、数量、时间、条件。
3. 必须保留任务槽位：动作、对象、时间、数量、格式、限制条件、输出形态。
4. 不把输出形态改掉：用户说图表就保留图表，不要改成表格；用户说输入框就不要发送。
5. 正常查询、比较、搜索、整理、生成、做、把、帮我等句子都必须保留原请求，不能输出空字符串。
6. 禁止截断句子；如果不确定，只做最小纠错并保留原句结构。
7. 英文实体、模型名、产品名和 API 名称要尽量标准化：GPT-5、Claude Sonnet、DeepSeek V3、REST API、PDF、Pinvou。
8. 只有整句去掉标点后只剩“嗯/啊/呃/额/那个/就是/文”等口头禅或噪声占位，才输出空字符串。
9. 优先纠正上下文中明显 ASR 错词：
   - 行情/价格查询里的“进价/惊吓”通常应修为“金价”
   - 数据分析可视化里的“图标”通常应修为“图表”
   - “屁屁提/PPTT”通常应修为“PPT”
   - “销售暑假”通常应修为“销售数据”
   - “截止事件”通常应修为“截止时间”
   - “负责任”通常应修为“负责人”
   - “风险电”通常应修为“风险点”
   - “表哥”通常应修为“表格”
   - “四零一/talken/过期处里”通常应修为“401/token/过期处理”
   - “批地爱福/pDF”通常应修为“PDF”
   - “g p t five/GP杠5”通常应修为“GPT-5”，“closonic/克劳德 sonnet”通常应修为“Claude Sonnet”
   - “deeps V3/deep seek v three”通常应修为“DeepSeek V3”
   - 搜索“爱新闻/AI新闻”通常应修为“AI 新闻”
10. 对明显口语断裂做最小顺句，例如“有长方形，的需要联网下的图片”应整理为“是长方形，需要联网下载图片”。
11. 去掉口头禅、重复词和误识别语气词。
12. 只输出最终任务文本，不解释，不使用 Markdown。

示例：
ASR 文本：查一下今日进价并生成数据分析图标。
最终文本：查一下今日金价并生成数据分析图表。
ASR 文本：比较GP杠5mini和deeps V3的调用成本。
最终文本：比较 GPT-5 mini 和 DeepSeek V3 的调用成本。
ASR 文本：嗯，做一张海报，这个海报有长方形，的需要联网下的图片。用于公司的下午茶需要有一些文字的内容。
最终文本：做一张用于公司下午茶的长方形海报，需要联网下载图片，并包含文案内容。
ASR 文本：文。
最终文本："#
    } else {
        r#"你是 Pinvou 的语音听写整理器。你的唯一职责是把 ASR 文本纠正并整理为用户原本想输入到文本框里的内容。

强规则：
1. 不回答问题，不执行任务。
2. 不新增用户没说过的目标、工具、格式、数量、时间、条件。
3. 先去掉口头禅、重复词和误识别语气词，再修明显 ASR 错词。
4. 只有极短、单一、无需拆解的自然句，才输出一条自然句。
5. 除极短自然句外，默认整理成结构化 Markdown 列表。
6. 内容包含目标、用途、功能、字段、截止时间、进度、多个事项、多个条件、步骤、约束或明显需求表达时，必须整理成 Markdown 列表。
7. 整理成列表时必须保留动作、对象、时间、地点、数量、格式、限制条件和输出形态。
8. 正常查询、比较、搜索、整理、生成、做、把、帮我等句子都必须保留原请求，不能输出空字符串。
9. 只有整句去掉标点后只剩“嗯/啊/呃/额/那个/就是/文”等口头禅或噪声占位，才输出空字符串。
10. 日期、时间、地点按用户原话保留；即使看起来不合理，也不能擅自修正或删除。
11. 优先纠正上下文中明显 ASR 错词：
   - 行情/价格查询里的“进价/惊吓”通常应修为“金价”
   - 数据分析可视化里的“图标”通常应修为“图表”
   - “屁屁提/PPTT”通常应修为“PPT”
   - “销售暑假”通常应修为“销售数据”
   - “截止事件”通常应修为“截止时间”
   - “负责任”通常应修为“负责人”
   - “风险电”通常应修为“风险点”
   - “g p t five”通常应修为“GPT-5”，“克劳德 sonnet”通常应修为“Claude Sonnet”
   - 搜索“爱新闻”通常应修为“AI 新闻”
12. 只输出最终文本，不解释。

示例：
ASR 文本：今天天气怎么样？
最终文本：今天天气怎么样？
ASR 文本：嗯。
最终文本：
ASR 文本：搜索一下今天的爱新闻，按重要性排序。
最终文本：搜索一下今天的 AI 新闻，按重要性排序。
ASR 文本：制作一个个人工作台，用于企业录入工作事项进度，包括截止时间。
最终文本：
- 制作一个个人工作台。
- 用途：用于企业录入工作事项进度。
- 需要包含截止时间。
ASR 文本：一张用于公司年会的海报，时间是下午3点，12月36日需要联网下载一张图片，然后这个图片要尽量的好看呃，突出员工协作。这个海报是长方形的，上面需要有一点点文字，然后是红色背景。
最终文本：
- 制作一张用于公司年会的长方形海报。
- 时间：12月36日下午3点。
- 需要联网下载一张图片。
- 图片尽量好看，并突出员工协作。
- 海报需要红色背景。
- 海报上需要包含少量文字。"#
    }
}

fn voice_postprocess_timeout(mode: &str, raw_text: &str) -> Duration {
    match normalize_voice_postprocess_mode(mode) {
        "task" => return Duration::from_millis(8000),
        "edit" => return Duration::from_millis(12000),
        _ => {}
    }
    let compact_len = raw_text
        .chars()
        .filter(|ch| {
            !ch.is_whitespace() && !"。！？!?，,、；;：:\"'“”‘’（）()【】[]….-—".contains(*ch)
        })
        .count();
    if compact_len <= 18 {
        Duration::from_millis(3000)
    } else {
        Duration::from_millis(5000)
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

fn voice_postprocess_retry_prompt(mode: &str) -> &'static str {
    if mode == "edit" {
        "你是语音编辑器。当前输入框已有文本是正文，ASR 文本是修改指令，不是要追加的正文。必须根据 ASR 指令修改正文，并只输出修改后的完整正文。除非 ASR 是纯口头禅、纯噪声或完全无法理解，否则禁止原样输出当前输入框已有文本。禁止解释，禁止空输出。"
    } else if mode == "task" {
        "你是语音任务纠错器。把 ASR 纠正为用户要交给 Agent 执行的任务。保留所有时间、地点、格式和限制条件。禁止新增事实。必须只输出最终任务文本，禁止空输出；除非 ASR 是纯口头禅或纯噪声。"
    } else {
        "你是语音听写整理器。把 ASR 纠正并整理为用户想输入的文本。只有极短、单一、无需拆解的自然句才输出自然句；除此以外默认整理成结构化 Markdown 列表。内容包含目标、用途、功能、字段、截止时间、进度、多个事项、多个条件、步骤、约束或明显需求表达时，必须结构化。保留所有时间、地点、格式和限制条件。禁止新增事实。必须只输出最终文本，禁止空输出；除非 ASR 是纯口头禅或纯噪声。"
    }
}

fn voice_postprocess_max_tokens(mode: &str, retry: bool) -> u32 {
    let base = match mode {
        "edit" => 768,
        "task" => 384,
        _ => 384,
    };
    if retry {
        base + 128
    } else {
        base
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

fn voice_postprocess_changed(mode: &str, text: &str, draft_text: Option<&str>) -> bool {
    if mode != "edit" {
        return true;
    }
    let draft = draft_text.unwrap_or("").trim();
    if draft.is_empty() {
        return true;
    }
    text.trim() != draft
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceReasoningDialect {
    None,
    ThinkingDisabled,
    QwenEnableThinking,
    VllmChatTemplate,
    Minimax,
}

fn apply_voice_reasoning_controls(
    body: &mut Value,
    preset: crate::platform::prefs::ModelPreset,
    provider: &str,
    base_url: &str,
    model: &str,
) {
    match voice_reasoning_dialect(preset, provider, base_url, model) {
        VoiceReasoningDialect::ThinkingDisabled => {
            body["thinking"] = json!({ "type": "disabled" });
        }
        VoiceReasoningDialect::QwenEnableThinking => {
            body["enable_thinking"] = json!(false);
        }
        VoiceReasoningDialect::VllmChatTemplate => {
            body["chat_template_kwargs"] = json!({ "enable_thinking": false });
        }
        VoiceReasoningDialect::Minimax => {
            body["thinking"] = json!({ "type": "disabled" });
            body["reasoning_split"] = json!(true);
        }
        VoiceReasoningDialect::None => {}
    }
}

fn voice_reasoning_dialect(
    preset: crate::platform::prefs::ModelPreset,
    provider: &str,
    base_url: &str,
    model: &str,
) -> VoiceReasoningDialect {
    if provider == "vllm" || preset == crate::platform::prefs::ModelPreset::LocalVllm {
        return VoiceReasoningDialect::VllmChatTemplate;
    }
    if provider == "deepseek" || preset == crate::platform::prefs::ModelPreset::Deepseek {
        return VoiceReasoningDialect::ThinkingDisabled;
    }

    match preset {
        crate::platform::prefs::ModelPreset::Kimi => {
            if voice_kimi_supports_disabled_thinking(model) {
                VoiceReasoningDialect::ThinkingDisabled
            } else {
                VoiceReasoningDialect::None
            }
        }
        crate::platform::prefs::ModelPreset::Qwen => VoiceReasoningDialect::QwenEnableThinking,
        crate::platform::prefs::ModelPreset::Doubao
        | crate::platform::prefs::ModelPreset::Glm
        | crate::platform::prefs::ModelPreset::Mimo => VoiceReasoningDialect::ThinkingDisabled,
        crate::platform::prefs::ModelPreset::Minimax => VoiceReasoningDialect::Minimax,
        crate::platform::prefs::ModelPreset::OpenaiCompatible
        | crate::platform::prefs::ModelPreset::LocalVllm
        | crate::platform::prefs::ModelPreset::Deepseek
        | crate::platform::prefs::ModelPreset::Openai
        | crate::platform::prefs::ModelPreset::Anthropic
        | crate::platform::prefs::ModelPreset::Gemini
        | crate::platform::prefs::ModelPreset::Xai => {
            voice_reasoning_dialect_from_base_url(base_url, model)
        }
    }
}

#[allow(clippy::if_same_then_else)]
fn voice_reasoning_dialect_from_base_url(base_url: &str, model: &str) -> VoiceReasoningDialect {
    let normalized = base_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches("/chat/completions")
        .trim_end_matches("/v1")
        .to_ascii_lowercase();

    if normalized.contains("api.deepseek.com") || normalized.contains("api.deepseeki.com") {
        VoiceReasoningDialect::ThinkingDisabled
    } else if normalized.contains("dashscope.aliyuncs.com") {
        VoiceReasoningDialect::QwenEnableThinking
    } else if normalized.contains("moonshot.cn") || normalized.contains("moonshot.ai") {
        if voice_kimi_supports_disabled_thinking(model) {
            VoiceReasoningDialect::ThinkingDisabled
        } else {
            VoiceReasoningDialect::None
        }
    } else if normalized.contains("volces.com")
        || normalized.contains("volcengine")
        || normalized.contains("byteplus.com")
    {
        VoiceReasoningDialect::ThinkingDisabled
    } else if normalized.contains("minimax.chat") || normalized.contains("minimaxi.com") {
        VoiceReasoningDialect::Minimax
    } else if normalized.contains("bigmodel.cn") || normalized.contains("z.ai") {
        VoiceReasoningDialect::ThinkingDisabled
    } else if normalized.contains("xiaomimimo.com") {
        VoiceReasoningDialect::ThinkingDisabled
    } else {
        VoiceReasoningDialect::None
    }
}

fn voice_kimi_supports_disabled_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    (model.contains("kimi-k2.5") || model.contains("kimi-k2.6"))
        && !model.contains("thinking")
        && !model.contains("k2.7")
}

fn voice_chat_message_text(value: &Value) -> String {
    let Some(message) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
    else {
        return String::new();
    };
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        return content.to_string();
    }
    if let Some(parts) = message.get("content").and_then(Value::as_array) {
        return parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .or_else(|| part.get("content"))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
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
    retry: bool,
    attempt: u8,
) -> AnyResult<String> {
    let timeout = voice_postprocess_timeout(mode, raw_text);
    let client = Client::builder()
        .timeout(timeout)
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
    log::info!(
        target: "pinvou.voice",
        "[voice_postprocess] request attempt={} mode={} provider={} model={} timeout_ms={} draft_present={}",
        attempt,
        mode,
        bridge.provider(),
        model_name,
        timeout.as_millis(),
        draft_text.map(|text| !text.trim().is_empty()).unwrap_or(false)
    );
    let system = if retry {
        voice_postprocess_retry_prompt(mode)
    } else {
        voice_postprocess_prompt(mode)
    };
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
            voice_postprocess_max_tokens(mode, retry),
        )
        .await?;
        return Ok(sanitize_voice_postprocess_output(&content));
    }

    let mut body = json!({
        "model": model_name,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "temperature": 0,
        "max_tokens": voice_postprocess_max_tokens(mode, retry),
        "stream": false
    });
    apply_voice_reasoning_controls(
        &mut body,
        preset,
        &bridge.provider(),
        &base_url,
        &model_name,
    );
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
    let content = voice_chat_message_text(&value);
    Ok(sanitize_voice_postprocess_output(&content))
}

#[tauri::command]
pub async fn postprocess_voice_text(
    request: VoicePostprocessRequest,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<VoicePostprocessResponse, String> {
    let started_at = Instant::now();
    let raw_text = request.text.trim();
    let mode = normalize_voice_postprocess_mode(&request.mode);
    let draft_len = request
        .draft_text
        .as_deref()
        .map(|text| text.trim().chars().count())
        .unwrap_or(0);
    if raw_text.is_empty() {
        log::info!(
            target: "pinvou.voice",
            "[voice_postprocess] skipped_empty mode={} elapsed_ms={}",
            mode,
            started_at.elapsed().as_millis()
        );
        return Ok(VoicePostprocessResponse {
            text: String::new(),
            mode: mode.to_string(),
            source: "empty".to_string(),
        });
    }
    let bridge = match voice_postprocess_bridge(request.session_id.as_deref(), &pool, &store).await
    {
        Ok(bridge) => bridge,
        Err(error) => {
            log::warn!(
                target: "pinvou.voice",
                "[voice_postprocess] prepare_failed mode={} raw_len={} draft_len={} elapsed_ms={} error={:#}",
                mode,
                raw_text.chars().count(),
                draft_len,
                started_at.elapsed().as_millis(),
                error
            );
            return Err(format!("prepare voice postprocess model: {error:#}"));
        }
    };
    let text = match call_voice_postprocess_model(
        &bridge,
        mode,
        raw_text,
        request.draft_text.as_deref(),
        false,
        1,
    )
    .await
    {
        Ok(text) => text,
        Err(error) => {
            log::warn!(
                target: "pinvou.voice",
                "[voice_postprocess] failed mode={} raw_len={} draft_len={} elapsed_ms={} error={:#}",
                mode,
                raw_text.chars().count(),
                draft_len,
                started_at.elapsed().as_millis(),
                error
            );
            return Err(format!("voice postprocess failed: {error:#}"));
        }
    };
    let text = if text.trim().is_empty()
        || !voice_postprocess_changed(mode, &text, request.draft_text.as_deref())
    {
        let retry_reason = if text.trim().is_empty() {
            "retry_empty_output"
        } else {
            "retry_unchanged_output"
        };
        log::warn!(
            target: "pinvou.voice",
            "[voice_postprocess] {} mode={} raw_len={} draft_len={} elapsed_ms={}",
            retry_reason,
            mode,
            raw_text.chars().count(),
            draft_len,
            started_at.elapsed().as_millis()
        );
        match call_voice_postprocess_model(
            &bridge,
            mode,
            raw_text,
            request.draft_text.as_deref(),
            true,
            2,
        )
        .await
        {
            Ok(text) if !text.trim().is_empty() => text,
            Ok(_) => {
                log::warn!(
                    target: "pinvou.voice",
                    "[voice_postprocess] empty_output mode={} raw_len={} draft_len={} elapsed_ms={}",
                    mode,
                    raw_text.chars().count(),
                    draft_len,
                    started_at.elapsed().as_millis()
                );
                return Err("voice postprocess failed: model returned empty output".to_string());
            }
            Err(error) => {
                log::warn!(
                    target: "pinvou.voice",
                    "[voice_postprocess] failed mode={} raw_len={} draft_len={} elapsed_ms={} error={:#}",
                    mode,
                    raw_text.chars().count(),
                    draft_len,
                    started_at.elapsed().as_millis(),
                    error
                );
                return Err(format!("voice postprocess failed: {error:#}"));
            }
        }
    } else {
        text
    };
    let changed = voice_postprocess_changed(mode, &text, request.draft_text.as_deref());
    log::info!(
        target: "pinvou.voice",
        "[voice_postprocess] completed mode={} raw_len={} draft_len={} output_len={} changed={} elapsed_ms={}",
        mode,
        raw_text.chars().count(),
        draft_len,
        text.chars().count(),
        changed,
        started_at.elapsed().as_millis()
    );
    Ok(VoicePostprocessResponse {
        text,
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
