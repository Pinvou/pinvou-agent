use super::prelude::*;
use anyhow::{Context, Result as AnyResult};
use base64::Engine as _;
use reqwest::Client;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize)]
pub struct VoiceTranscriptionRequest {
    /// Standard padded-base64-encoded WAV produced by the WebView recorder.
    /// Replaces the old `audio_bytes: Vec<u8>` JSON number array — 60s of
    /// 16kHz/16-bit mono is about 1.92MB, and per-element deserialization of
    /// that means 1.92M JSON values and ~25MB peak memory; base64 is just one
    /// string decode.
    pub audio_base64: String,
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
    /// Raw ASR transcript (before rule-based correction). When present and
    /// different from `text`, it is sent along in the prompt so the model can
    /// undo deterministic rule mis-corrections against the original (e.g.
    /// "表哥" → "表格").
    pub raw_text: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VoicePostprocessResponse {
    pub text: String,
    pub mode: String,
    pub source: String,
    /// The model output was truncated by max_tokens (finish_reason=length);
    /// the frontend falls back on this and skips write-back. The serde default
    /// keeps older caller constructions compatible.
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct VoiceCommandError {
    pub category: String,
    pub stage: String,
    /// Stable machine-decidable error code: the frontend maps it to trilingual
    /// copy. The text in `message` is kept only as the raw log/diagnostic
    /// string and is no longer passed through to en/ja user interfaces.
    pub code: String,
    pub message: String,
}

#[tauri::command]
pub(crate) fn set_voice_shortcut_enabled(enabled: bool) -> Result<(), String> {
    // Persist first, then flip the in-memory hook (same order as
    // save_model/set_active_model): if the settings write fails, the native
    // hook must not disagree with what settings.json still says, or the
    // next launch would silently revert the user's choice. The frontend
    // localStorage is only a mirror; the authoritative state lives on the
    // Rust side (replayed into the AtomicBool at startup by lib.rs), so
    // repeated invokes from multi-window mounts are idempotent and clearing
    // WebView storage cannot lose the setting.
    crate::platform::prefs::UserPrefs::update_transaction(|prefs| {
        prefs.voice_shortcut_enabled = enabled;
        Ok(())
    })?;
    crate::features::voice_shortcut::set_enabled(enabled);
    Ok(())
}

/// Cross-window recording mutual exclusion: the frontend syncs its own window
/// label when recording starts/ends/fails, and the native shortcut hook uses
/// it to pick the trigger target window (see
/// voice_shortcut::set_recording_label). A window may only register its own
/// label, so a broken or compromised renderer cannot pin an arbitrary window
/// as the recording window and hijack the global Alt gesture.
#[tauri::command]
pub(crate) fn set_voice_shortcut_recording(
    window: tauri::WebviewWindow,
    label: Option<String>,
) -> Result<(), String> {
    if let Some(label) = &label {
        if label != window.label() {
            return Err("voice shortcut recording label must match the calling window".to_string());
        }
    }
    crate::features::voice_shortcut::set_recording_label(label);
    Ok(())
}

impl VoiceCommandError {
    pub(crate) fn new(code: &str, category: &str, stage: &str, message: impl Into<String>) -> Self {
        Self {
            category: category.to_string(),
            stage: stage.to_string(),
            code: code.to_string(),
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

/// Temporary WAV file: `NamedTempFile` generates an unpredictable file name
/// with 0600 permissions on Unix (the old hand-built pid+millisecond name was
/// predictable and world-readable at 0644); the file is deleted on drop.
struct VoiceTempWav {
    file: tempfile::NamedTempFile,
}

impl VoiceTempWav {
    fn create() -> std::io::Result<Self> {
        let file = tempfile::Builder::new()
            .prefix("pinvou3-voice-")
            .suffix(".wav")
            .tempfile()?;
        Ok(Self { file })
    }

    fn path(&self) -> &std::path::Path {
        self.file.path()
    }
}

struct LocalAsrOutput {
    text: String,
    /// Recognition backend source (system_speech /
    /// pinvou-webview-sensevoice-local / local_cli), passed through to
    /// VoiceTranscriptionResponse.source so the frontend/triage can tell them
    /// apart.
    source: String,
}

pub(super) fn parse_local_asr_text(stdout: &str, stderr: &str) -> Option<String> {
    crate::features::voice::parse_asr_transcript(stdout, stderr)
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
        let (code, message) = if e.kind() == std::io::ErrorKind::NotFound {
            (
                "asr_engine_missing",
                crate::features::voice::asr_missing_message().to_string(),
            )
        } else {
            // The io error detail goes to the log; the user-visible copy stays
            // generic, consistent with the macOS lane's copy style.
            log::warn!(
                target: "pinvou.voice",
                "[voice_transcribe] local ASR spawn failed: {e}"
            );
            (
                "asr_engine_start_failed",
                "Local speech recognition engine failed to start; check the recognition component installation and retry".to_string(),
            )
        };
        VoiceCommandError::new(code, "recognition_failed", "transcribing", message)
    })?;

    // stdout/stderr must be drained concurrently with the wait polling: if the
    // pipes are not read while polling, model-loading logs fill the OS pipe
    // buffer (~64KB) and the child blocks forever on write, leaving the
    // polling side to wait for the timeout and misreport. The two drain
    // threads keep the child writable at all times; on the timeout path the
    // kill closes the pipes, the threads exit right away, and join cannot
    // hang.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_drain = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut pipe) = stdout_pipe {
            let _ = pipe.read_to_string(&mut buf);
        }
        buf
    });
    let stderr_drain = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut pipe) = stderr_pipe {
            let _ = pipe.read_to_string(&mut buf);
        }
        buf
    });

    let started = std::time::Instant::now();
    let wait_result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    break Err(VoiceCommandError::new(
                        "asr_timeout",
                        "recognition_failed",
                        "transcribing",
                        format!(
                            "Local speech recognition timed out ({} s); confirm the recognition model and runtime are available",
                            timeout.as_secs()
                        ),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                log::warn!(
                    target: "pinvou.voice",
                    "[voice_transcribe] local ASR wait failed: {e}"
                );
                break Err(VoiceCommandError::new(
                    "asr_runtime_error",
                    "recognition_failed",
                    "transcribing",
                    "Local speech recognition failed unexpectedly; please retry",
                ));
            }
        }
    };
    // All three paths wrap up the same way: on timeout/wait failure too, kill
    // + wait before joining; calling kill/wait on an already-exited child is a
    // harmless no-op.
    let _ = child.kill();
    let _ = child.wait();
    let stdout = stdout_drain.join().unwrap_or_default();
    let stderr = stderr_drain.join().unwrap_or_default();
    let status = match wait_result {
        Ok(status) => status,
        Err(error) => return Err(error),
    };

    if !status.success() {
        let exit_code = status.code();
        // Privacy: stdout/stderr may contain recognized speech fragments, so
        // the log records only lengths and the exit code; the user-visible
        // copy stays generic, consistent with the macOS lane's copy style.
        log::warn!(
            target: "pinvou.voice",
            "[voice_transcribe] local ASR failed exit={:?} stdout_len={} stderr_len={}",
            exit_code,
            stdout.len(),
            stderr.len()
        );
        // Exit code 6 = the CLI's "no speech recognized" convention (the
        // frontend's emptyResultLike check also classifies by exit 6); return
        // the empty_result category at the source so the frontend no longer
        // has to guess from English error text.
        if exit_code == Some(6) {
            return Err(VoiceCommandError::new(
                "asr_no_speech",
                "empty_result",
                "transcribing",
                "No speech content recognized; move closer to the microphone and retry",
            ));
        }
        return Err(VoiceCommandError::new(
            "asr_cli_failed",
            "recognition_failed",
            "transcribing",
            "Local speech recognition failed; please retry",
        ));
    }

    let text = parse_local_asr_text(&stdout, &stderr).ok_or_else(|| {
        // As above: the failure log carries no raw process output, only lengths.
        log::warn!(
            target: "pinvou.voice",
            "[voice_transcribe] local ASR returned no usable text: stdout_len={} stderr_len={}",
            stdout.len(),
            stderr.len()
        );
        VoiceCommandError::new(
            "asr_parse_failed",
            "empty_result",
            "transcribing",
            "Local speech recognition returned no usable text; please retry",
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
    // Length pre-check before decoding: a normal 60s recording is ~1.9MiB of
    // WAV (≈2.6MB base64), so 4MiB already leaves ample headroom; without a
    // cap a misbehaving renderer could create memory pressure via unbounded
    // base64 decoding.
    const MAX_TRANSCRIBE_AUDIO_BYTES: usize = 4 * 1024 * 1024;
    let max_base64_chars = (MAX_TRANSCRIBE_AUDIO_BYTES / 3 + 1) * 4;
    if request.audio_base64.len() > max_base64_chars {
        return Err(VoiceCommandError::new(
            "recording_too_long",
            "recording_failed",
            "recording",
            "Voice recording too long; shorten it and retry",
        ));
    }
    let audio_bytes = base64::engine::general_purpose::STANDARD
        .decode(request.audio_base64.trim())
        .map_err(|error| {
            log::warn!(
                target: "pinvou.voice",
                "[voice_transcribe] audio base64 decode failed: {error}"
            );
            VoiceCommandError::new(
                "audio_invalid",
                "recording_failed",
                "recording",
                "Invalid voice audio data; please record again",
            )
        })?;
    transcribe_voice_audio_bytes(audio_bytes).await
}

/// Decoded WAV bytes go through the unified recognition path. Remote control
/// (`web_access_transcribe_voice_audio`) brings its own validation and
/// decoding and reuses this function directly.
pub(crate) async fn transcribe_voice_audio_bytes(
    audio_bytes: Vec<u8>,
) -> Result<VoiceTranscriptionResponse, VoiceCommandError> {
    if audio_bytes.len() < 44 {
        return Err(VoiceCommandError::new(
            "audio_empty",
            "recording_failed",
            "recording",
            "Recording is empty or corrupted; please record again",
        ));
    }

    let asr_output = tokio::task::spawn_blocking(move || {
        let wav_file = VoiceTempWav::create().map_err(|e| {
            log::warn!(
                target: "pinvou.voice",
                "[voice_transcribe] create temp wav failed: {e}"
            );
            VoiceCommandError::new(
                "temp_file_unavailable",
                "recording_failed",
                "recording",
                "Could not create temporary audio file; please retry",
            )
        })?;
        std::fs::write(wav_file.path(), &audio_bytes).map_err(|e| {
            log::warn!(
                target: "pinvou.voice",
                "[voice_transcribe] write temp wav failed: {e}"
            );
            VoiceCommandError::new(
                "temp_file_write_failed",
                "recording_failed",
                "recording",
                "Failed to write temporary audio file; please retry",
            )
        })?;
        // Recognition path branching (platform-neutral):
        //   macOS         → system Speech framework (no model download, no
        //                    ffmpeg, usable on first run)
        //   Linux/Windows → bundled SenseVoice (otherwise fall back to the CLI)
        // Platform selection is encapsulated in
        // `features::voice::recognize_native` (a platform/ adapter); this site
        // only dispatches on the return value and contains no cfg(target_os).
        let result = {
            // The recognition language follows the UI language preference
            // (decided by the system locale on first launch): this avoids a
            // UI/model language mismatch, e.g. a Chinese UI parsing Chinese
            // audio as English.
            let locale_tag = crate::platform::prefs::UserPrefs::load()
                .language
                .speech_recognition_locale();
            let native = crate::features::voice::recognize_native(wav_file.path(), locale_tag);
            match native {
                Some(Ok(text)) => Ok(LocalAsrOutput {
                    text,
                    source: crate::features::voice::native_recognition_source().to_string(),
                }),
                Some(Err(e)) => {
                    if has_explicit_asr_cli_fallback() {
                        run_local_asr_cli(wav_file.path())
                    } else {
                        Err(VoiceCommandError::new(
                            "asr_engine_error",
                            "recognition_failed",
                            "transcribing",
                            e,
                        ))
                    }
                }
                None => run_local_asr_cli(wav_file.path()),
            }
        };
        result
    })
    .await
    .map_err(|e| {
        log::warn!(
            target: "pinvou.voice",
            "[voice_transcribe] recognition task join failed: {e}"
        );
        VoiceCommandError::new(
            "asr_join_failed",
            "recognition_failed",
            "transcribing",
            "Local speech recognition task failed unexpectedly; please retry",
        )
    })??;

    Ok(VoiceTranscriptionResponse {
        text: asr_output.text,
        source: asr_output.source,
    })
}

fn normalize_voice_postprocess_mode(mode: &str) -> &'static str {
    match mode.trim() {
        // "rewrite" is a legacy alias from an earlier pipeline; the frontend's
        // normalizeVoiceMode already folds it into the dictation fallback. Keep
        // both sides consistent so the Rust side does not escalate the same
        // request to task.
        "task" | "task_rewrite" => "task",
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
10. 输入用 <<<…>>> 定界符分段：DRAFT_TEXT 是输入框正文，ASR_TEXT 是规则纠错后的修改指令，ASR_RAW（如有）是原始识别；纠错可能有误，可参考原始识别恢复被误纠的内容。

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
13. 输入用 <<<…>>> 定界符分段：ASR_TEXT 是规则纠错后文本，ASR_RAW（如有）是原始识别；纠错可能有误，可参考原始识别恢复被误纠的实体。

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
13. 输入用 <<<…>>> 定界符分段：ASR_TEXT 是规则纠错后文本，ASR_RAW（如有）是原始识别；纠错可能有误，可参考原始识别恢复被误纠的实体。

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

/// Assemble the user message. Sections are isolated with <<<…>>> delimiters:
/// the draft/ASR body itself may contain markers like "ASR 文本：", and bare
/// concatenation would let the model mistake the draft for an instruction
/// (section cross-talk). `asr_raw_text` (the raw recognition) is sent along
/// when it differs from the corrected text: rule-based correction can be
/// wrong, and the model can restore mis-corrected entities against the
/// original.
fn voice_postprocess_user_content(
    corrected_text: &str,
    asr_raw_text: Option<&str>,
    draft_text: Option<&str>,
) -> String {
    let mut sections = Vec::new();
    let draft = draft_text.unwrap_or("").trim();
    if !draft.is_empty() {
        sections.push(format!(
            "当前输入框已有文本：\n<<<DRAFT_TEXT>>>\n{draft}\n<<<END>>>"
        ));
    }
    let asr_raw = asr_raw_text.unwrap_or("").trim();
    if !asr_raw.is_empty() && asr_raw != corrected_text.trim() {
        sections.push(format!(
            "原始 ASR 识别（第一段为原始识别，第二段为规则纠错后文本；纠错可能有误，可参考原始识别恢复）：\n<<<ASR_RAW>>>\n{asr_raw}\n<<<END>>>"
        ));
    }
    sections.push(format!(
        "ASR 文本（规则纠错后）：\n<<<ASR_TEXT>>>\n{}\n<<<END>>>",
        corrected_text.trim()
    ));
    // Pin the output language back to the source language: the prompts and
    // examples are written in Chinese, so without this an en/ja user's
    // dictation would come back translated into Chinese and be written into
    // the input box.
    sections.push(
        "无论系统提示中的规则如何，输出必须使用 ASR 文本（及已有文本）原本的语言，不要翻译成其他语言。"
            .to_string(),
    );
    sections.join("\n\n")
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
    // The budget is sized for the worst-case output of typical input (short
    // sentences up to a few hundred characters): edit must fit a full draft
    // rewrite, dictation/task must fit the restructured list. It does not
    // cover the full expansion of the worst-case CJK output under the
    // 4000-character input cap — oversized inputs instead take the safe
    // degradation chain of "reject → retry (+512) → fall back to the
    // rule-corrected text" via finish_reason=length, so a half-finished
    // rewrite is never silently written back, but smart restructuring does not
    // apply to oversized input; how much input to cover is a product
    // trade-off.
    let base = match mode {
        "edit" => 2048,
        "task" => 768,
        _ => 768,
    };
    if retry { base + 512 } else { base }
}

/// Input truncation cap (characters) before the LLM: neither the ASR text nor
/// the input-box draft should be passed through unbounded.
const VOICE_POSTPROCESS_MAX_INPUT_CHARS: usize = 4000;

fn truncate_voice_postprocess_input(text: &str) -> String {
    text.chars()
        .take(VOICE_POSTPROCESS_MAX_INPUT_CHARS)
        .collect()
}

fn sanitize_voice_postprocess_output(text: &str) -> String {
    let without_thinking = strip_leading_thinking_block(text);
    // Only strip ONE wrapping pair per quote kind: models occasionally wrap
    // the whole answer in quotes, but repeated stripping would eat legitimate
    // leading/trailing quote characters of the actual content (dictated
    // quotations, quoted code like `'await'`, …) and, in edit mode, would
    // also corrupt the unchanged comparison against the draft.
    let cleaned = strip_wrapping_quote(
        &strip_wrapping_quote(without_thinking.trim().trim_matches('\u{feff}').trim(), '"'),
        '\'',
    )
    .trim();
    strip_voice_markdown_fence(cleaned).trim().to_string()
}

/// Strip a leading `<think>…</think>` reasoning block. The dialect controls
/// only cover recognized vendors (qwen/deepseek etc.); other thinking models
/// behind custom OpenAI-compatible gateways still emit reasoning text, so this
/// is the output-side fallback that keeps thinking text out of the input box.
/// Only a leading block is handled: an unclosed leading block (truncation) is
/// treated as empty output and dropped by the empty-output contract; a bare
/// `<think>` in the middle of the body is literal content and left untouched.
fn strip_leading_thinking_block(text: &str) -> &str {
    // BOM (U+FEFF) is not White_Space, so trim_start does not strip it; if it
    // is not stripped here too, "BOM + <think>…" fails the strip_prefix match
    // and the whole block leaks (the BOM is only stripped later in sanitize,
    // by which point the think block has already passed this gate). Any
    // interleaving of whitespace and BOM is tolerated.
    let trimmed = text.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let Some(rest) = trimmed.strip_prefix("<think>") else {
        return text;
    };
    match rest.find("</think>") {
        Some(end) => rest[end + "</think>".len()..]
            .trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}'),
        None => "",
    }
}

/// Frontend-facing error summary: a reqwest error's Display carries the full
/// URL (possibly an intranet address or embedded credentials), and passing it
/// through rawMessage would land it in frontend diagnostics persisted to
/// localStorage. Keep only the error class and HTTP status here; the full
/// error chain goes to the local log only.
fn summarize_voice_postprocess_error(error: &anyhow::Error) -> String {
    for cause in error.chain() {
        let Some(request_error) = cause.downcast_ref::<reqwest::Error>() else {
            continue;
        };
        if let Some(status) = request_error.status() {
            return format!("model endpoint http {status}");
        }
        if request_error.is_timeout() {
            return "model endpoint timeout".to_string();
        }
        if request_error.is_connect() {
            return "model endpoint connect failed".to_string();
        }
        return "model endpoint request failed".to_string();
    }
    // Non-HTTP errors (missing model/credential configuration etc.): the
    // outermost context is already a short human-written sentence and carries
    // no URL, so it is safe to pass through.
    error.to_string()
}

fn strip_wrapping_quote(text: &str, quote: char) -> &str {
    // A lone quote character is ambiguous output; keep it rather than
    // emitting an empty string. A bare quote pair ("", ''), i.e. the model
    // wrapping an empty answer, still unwraps to empty.
    if text.chars().count() == 1 {
        return text;
    }
    let Some(inner) = text.strip_prefix(quote) else {
        return text;
    };
    let Some(stripped) = inner.strip_suffix(quote) else {
        return text;
    };
    stripped
}

/// Strip one outer fully-wrapping ``` fence (```lang\n…\n```): models
/// occasionally wrap the entire output in a code block, and writing it back
/// verbatim would carry the fences into the input box. Only one fully-wrapping
/// layer is stripped; legitimate Markdown lists inside are preserved.
fn strip_voice_markdown_fence(text: &str) -> &str {
    let Some(inner) = text.strip_prefix("```") else {
        return text;
    };
    // The first line is an optional language tag; no newline means this is not
    // a fully-wrapping fence.
    let Some(newline) = inner.find('\n') else {
        return text;
    };
    let body = &inner[newline + 1..];
    match body.trim_end().strip_suffix("```") {
        Some(stripped) => stripped.trim(),
        None => text,
    }
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

impl From<crate::core::reasoning_dialect::ReasoningDialect> for VoiceReasoningDialect {
    fn from(d: crate::core::reasoning_dialect::ReasoningDialect) -> Self {
        use crate::core::reasoning_dialect::ReasoningDialect as D;
        match d {
            D::None => VoiceReasoningDialect::None,
            D::ThinkingDisabled => VoiceReasoningDialect::ThinkingDisabled,
            D::QwenEnableThinking => VoiceReasoningDialect::QwenEnableThinking,
            D::Minimax => VoiceReasoningDialect::Minimax,
        }
    }
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
            if crate::core::reasoning_dialect::kimi_supports_disabled_thinking(model) {
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
            // Mirror the memory-review lane: when URL sniffing cannot identify
            // the vendor, fall back to model-name matching so custom
            // OpenAI-compatible endpoints fronting qwen/deepseek models still
            // disable thinking output — the voice output sanitizer only strips
            // a single leading `<think>` block, so reasoning leaked after the
            // first block or mid-text would still land verbatim in the
            // user's input box.
            let d =
                crate::core::reasoning_dialect::reasoning_dialect_from_base_url(base_url, model);
            if matches!(d, crate::core::reasoning_dialect::ReasoningDialect::None) {
                let lower = model.to_ascii_lowercase();
                if lower.contains("qwen") {
                    return VoiceReasoningDialect::QwenEnableThinking;
                }
                if lower.contains("deepseek") {
                    return VoiceReasoningDialect::ThinkingDisabled;
                }
            }
            d.into()
        }
    }
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

/// Returns (sanitized text, whether truncated by max_tokens). Truncation
/// detection: OpenAI-compatible endpoints report
/// choices[0].finish_reason == "length"; the Anthropic endpoint's stop_reason
/// is not surfaced (post_anthropic_messages returns text only), so it stays
/// false for now.
async fn call_voice_postprocess_model(
    bridge: &crate::features::assistant::platform::bridge::Pinvou3Bridge,
    mode: &str,
    raw_text: &str,
    asr_raw_text: Option<&str>,
    draft_text: Option<&str>,
    retry: bool,
    attempt: u8,
    model_name: &str,
    timeout: Duration,
) -> AnyResult<(String, bool)> {
    let client = Client::builder()
        .timeout(timeout)
        .build()
        .context("build voice postprocess client")?;
    let base_url = bridge.base_url();
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
    let user = voice_postprocess_user_content(raw_text, asr_raw_text, draft_text);
    let preset = bridge
        .effective_model_owned()
        .map(|model| model.preset)
        .unwrap_or_else(|| bridge.prefs.advanced.model_preset.unwrap_or_default());

    if preset == crate::platform::prefs::ModelPreset::Anthropic {
        let content = crate::core::model_endpoint::post_anthropic_messages(
            &client,
            &base_url,
            &bridge.api_key(),
            model_name,
            system,
            &user,
            voice_postprocess_max_tokens(mode, retry),
        )
        .await?;
        return Ok((sanitize_voice_postprocess_output(&content), false));
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
    apply_voice_reasoning_controls(&mut body, preset, &bridge.provider(), &base_url, model_name);
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
    let truncated = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str)
        == Some("length");
    Ok((sanitize_voice_postprocess_output(&content), truncated))
}

/// At most one in-flight postprocess request at a time: this command calls a
/// paid model endpoint with the user's key, and without a concurrency gate a
/// broken or compromised renderer could burn quota with high-frequency calls.
/// The frontend is serial anyway, so the normal path never hits the gate (the
/// conflicting caller falls back to the rule-corrected text).
static VOICE_POSTPROCESS_INFLIGHT: AtomicBool = AtomicBool::new(false);

struct VoicePostprocessSlot;

impl Drop for VoicePostprocessSlot {
    fn drop(&mut self) {
        VOICE_POSTPROCESS_INFLIGHT.store(false, Ordering::Relaxed);
    }
}

#[tauri::command]
pub async fn postprocess_voice_text(
    request: VoicePostprocessRequest,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<VoicePostprocessResponse, String> {
    let started_at = Instant::now();
    if VOICE_POSTPROCESS_INFLIGHT
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        log::warn!(
            target: "pinvou.voice",
            "[voice_postprocess] busy_conflict elapsed_ms={}",
            started_at.elapsed().as_millis()
        );
        return Err("voice postprocess busy".to_string());
    }
    let _slot = VoicePostprocessSlot;
    // Truncate before the model: neither the ASR text nor the input-box draft
    // should reach the LLM unbounded.
    let raw_text = truncate_voice_postprocess_input(request.text.trim());
    let mode = normalize_voice_postprocess_mode(&request.mode);
    let asr_raw_text = request
        .raw_text
        .as_deref()
        .map(|text| truncate_voice_postprocess_input(text.trim()));
    let draft_text = request
        .draft_text
        .as_deref()
        .map(|text| truncate_voice_postprocess_input(text.trim()));
    let draft_len = draft_text
        .as_deref()
        .map(|text| text.chars().count())
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
            truncated: false,
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
            return Err(format!(
                "prepare voice postprocess model: {}",
                summarize_voice_postprocess_error(&error)
            ));
        }
    };
    // vllm's /v1/models probe carries its own 3s timeout; both attempts share
    // this one probe result, so the retry does not pay another 3s.
    let model_name = if bridge.provider() == "vllm" {
        // The served-name probe uses an inference-same-origin key:
        // authenticated vLLM 401s on /v1/models.
        crate::features::monitor::probe_vllm_model_info(
            &bridge.base_url(),
            Some(bridge.api_key().as_str()),
        )
        .await
        .0
        .unwrap_or_else(|| bridge.model())
    } else {
        bridge.model()
    };
    // The frontend times the whole invoke against the same
    // voice_postprocess_timeout budget; both Rust attempts share that budget,
    // each taking only the remaining headroom, so the total latency does not
    // double from each attempt spending the full budget.
    let budget = voice_postprocess_timeout(mode, &raw_text);
    if started_at.elapsed() >= budget {
        // Symmetric with the retry guard below: bridge preparation and the
        // vllm model probe can consume the whole budget, and handing a zero
        // timeout to the first request would guarantee an instant timeout
        // failure.
        log::warn!(
            target: "pinvou.voice",
            "[voice_postprocess] budget_exhausted_before_first_request mode={} elapsed_ms={}",
            mode,
            started_at.elapsed().as_millis()
        );
        return Err(
            "voice postprocess failed: timeout budget exhausted before first request".to_string(),
        );
    }
    let (text, truncated) = match call_voice_postprocess_model(
        &bridge,
        mode,
        &raw_text,
        asr_raw_text.as_deref(),
        draft_text.as_deref(),
        false,
        1,
        &model_name,
        budget.saturating_sub(started_at.elapsed()),
    )
    .await
    {
        Ok(output) => output,
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
            return Err(format!(
                "voice postprocess failed: {}",
                summarize_voice_postprocess_error(&error)
            ));
        }
    };
    let needs_retry = text.trim().is_empty()
        || truncated
        || !voice_postprocess_changed(mode, &text, draft_text.as_deref());
    let (text, truncated) = if needs_retry {
        let retry_reason = if text.trim().is_empty() {
            "retry_empty_output"
        } else if truncated {
            "retry_truncated_output"
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
        let remaining = budget.saturating_sub(started_at.elapsed());
        if remaining.is_zero() {
            log::warn!(
                target: "pinvou.voice",
                "[voice_postprocess] retry_budget_exhausted mode={} elapsed_ms={}",
                mode,
                started_at.elapsed().as_millis()
            );
            return Err(
                "voice postprocess failed: timeout budget exhausted before retry".to_string(),
            );
        }
        match call_voice_postprocess_model(
            &bridge,
            mode,
            &raw_text,
            asr_raw_text.as_deref(),
            draft_text.as_deref(),
            true,
            2,
            &model_name,
            remaining,
        )
        .await
        {
            Ok((text, truncated)) if !text.trim().is_empty() => (text, truncated),
            Ok(_) => {
                log::warn!(
                    target: "pinvou.voice",
                    "[voice_postprocess] empty_output mode={} raw_len={} draft_len={} elapsed_ms={}",
                    mode,
                    raw_text.chars().count(),
                    draft_len,
                    started_at.elapsed().as_millis()
                );
                // The prompts define an empty output as the correct answer for
                // pure filler/noise input, so this is a success, not an error:
                // the frontend validator accepts an empty candidate exactly
                // when the raw/corrected text is filler-only (and discards it),
                // falling back to the rule-corrected text otherwise. Returning
                // an Err here would make the frontend write the filler back
                // into the input box.
                (String::new(), false)
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
                return Err(format!(
                    "voice postprocess failed: {}",
                    summarize_voice_postprocess_error(&error)
                ));
            }
        }
    } else {
        (text, truncated)
    };
    let changed = voice_postprocess_changed(mode, &text, draft_text.as_deref());
    log::info!(
        target: "pinvou.voice",
        "[voice_postprocess] completed mode={} raw_len={} draft_len={} output_len={} changed={} truncated={} elapsed_ms={}",
        mode,
        raw_text.chars().count(),
        draft_len,
        text.chars().count(),
        changed,
        truncated,
        started_at.elapsed().as_millis()
    );
    Ok(VoicePostprocessResponse {
        text,
        mode: mode.to_string(),
        source: "llm".to_string(),
        truncated,
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
mod voice_postprocess_tests {
    use super::*;

    #[test]
    fn sanitize_strips_one_wrapping_quote_pair_only() {
        assert_eq!(
            sanitize_voice_postprocess_output("\"你好世界\""),
            "你好世界"
        );
        assert_eq!(sanitize_voice_postprocess_output("'你好'"), "你好");
        // Content quotes must survive: dictated quotations and quoted code.
        assert_eq!(
            sanitize_voice_postprocess_output("'await' 关键字"),
            "'await' 关键字"
        );
        assert_eq!(
            sanitize_voice_postprocess_output("以引号结尾\""),
            "以引号结尾\""
        );
        // A lone quote is ambiguous output; keep it rather than emitting empty.
        assert_eq!(sanitize_voice_postprocess_output("\""), "\"");
        assert_eq!(sanitize_voice_postprocess_output("'"), "'");
        // A bare quote pair wraps an empty answer and still unwraps to empty.
        assert_eq!(sanitize_voice_postprocess_output("\"\""), "");
        assert_eq!(sanitize_voice_postprocess_output("''"), "");
        assert_eq!(sanitize_voice_postprocess_output("\u{feff}文本"), "文本");
    }

    #[test]
    fn sanitize_strips_one_wrapping_markdown_fence() {
        assert_eq!(
            sanitize_voice_postprocess_output("```text\n内容\n```"),
            "内容"
        );
        assert_eq!(sanitize_voice_postprocess_output("普通文本"), "普通文本");
    }

    #[test]
    fn sanitize_strips_leading_thinking_block_as_output_side_fallback() {
        // Paired leading think block: reasoning text must not reach the input box.
        assert_eq!(
            sanitize_voice_postprocess_output("<think>推理过程……</think>结构化结果"),
            "结构化结果"
        );
        // Whitespace before the block is tolerated.
        assert_eq!(
            sanitize_voice_postprocess_output("\n  <think>r</think>答案"),
            "答案"
        );
        // A BOM before the block must not defeat the strip: U+FEFF is not
        // White_Space, and the sanitize pipeline strips BOM only after this
        // step, so the block would otherwise leak verbatim into the input box.
        assert_eq!(
            sanitize_voice_postprocess_output("\u{feff}<think>r</think>答案"),
            "答案"
        );
        // Whitespace and BOM interleaved before the block.
        assert_eq!(
            sanitize_voice_postprocess_output(" \u{feff}\n<think>r</think>答案"),
            "答案"
        );
        // BOM right after the closing tag is also trimmed.
        assert_eq!(
            sanitize_voice_postprocess_output("<think>r</think>\u{feff}答案"),
            "答案"
        );
        // Unclosed leading block (truncation) falls through the empty-output contract.
        assert_eq!(sanitize_voice_postprocess_output("<think>被截断的推理"), "");
        // No think block: byte-identical passthrough.
        assert_eq!(sanitize_voice_postprocess_output("普通文本"), "普通文本");
        // A bare <think> in the middle is literal content, not a block.
        assert_eq!(
            sanitize_voice_postprocess_output("先说<think>再写"),
            "先说<think>再写"
        );
    }

    #[test]
    fn voice_reasoning_dialect_falls_back_to_model_name() {
        use crate::platform::prefs::ModelPreset;
        assert_eq!(
            voice_reasoning_dialect(
                ModelPreset::OpenaiCompatible,
                "openai",
                "https://example.com/v1",
                "qwen2.5-72b-instruct"
            ),
            VoiceReasoningDialect::QwenEnableThinking
        );
        assert_eq!(
            voice_reasoning_dialect(
                ModelPreset::OpenaiCompatible,
                "openai",
                "https://example.com/v1",
                "deepseek-chat"
            ),
            VoiceReasoningDialect::ThinkingDisabled
        );
        // Unknown URL and unknown model name: no dialect, as before.
        assert_eq!(
            voice_reasoning_dialect(
                ModelPreset::OpenaiCompatible,
                "openai",
                "https://example.com/v1",
                "meta-llama-3"
            ),
            VoiceReasoningDialect::None
        );
    }
}
