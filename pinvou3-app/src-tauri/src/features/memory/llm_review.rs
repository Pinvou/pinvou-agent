//! LLM 后台记忆复盘：触发判别、提示词、chat/completions 调用、响应清洗与
//! 自动落库，以及纯启发式的 turn 候选发现（不走 LLM 的兜底）。
//!
//! 抽离自 `mod.rs`。`review_turn_candidates_with_llm` 是 pub 入口；诊断日志、
//! reasoning dialect 控制、JSON 解析与候选清洗等 helper 集中在本模块内。

use std::collections::BTreeMap;
use std::fs;
use std::io::{self as stdio, Write as IoWrite};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration as StdDuration;

use anyhow::{Context, Result};
use chrono::Utc;
use parking_lot::Mutex;
use reqwest::Client;
use serde_json::{Value, json};

use crate::platform::paths;
use crate::platform::prefs::ModelPreset;

use super::io;
use super::types::{
    LlmMemoryItem, LlmMemoryReview, MemoryReviewModel, MemoryReviewOutcome, MemorySuggestion,
    MemoryWriteEvent, ProfilePatch, SanitizedMemoryDecision, TurnMemoryCapture,
    clean_profile_memory_content, normalize_preference_topic, normalize_timed_memory_topic,
    normalize_work_context_topic,
};
use super::util::{
    clean_id, clean_text, looks_completed_work_status, looks_recent_work_status, looks_sensitive,
    looks_sensitive_or_task_like, looks_task_like,
};

const LLM_REVIEW_TIMEOUT: StdDuration = StdDuration::from_secs(75);

/// auto_write / auto_update 置信度门槛：`_RELAXED` 后缀为用户明确要求记住
/// （explicit_remember）时的放宽值。代码判定与提示词都必须从这一组常量取值，
/// 避免提示词与实现漂移；清洗下限 0.70 与敏感过滤不在此列，保持不变。
pub(super) const PROFILE_AUTO_WRITE_THRESHOLD: f32 = 0.92;
pub(super) const PROFILE_AUTO_WRITE_THRESHOLD_RELAXED: f32 = 0.85;
pub(super) const TIMED_AUTO_THRESHOLD: f32 = 0.86;
pub(super) const TIMED_AUTO_THRESHOLD_RELAXED: f32 = 0.80;
pub(super) const WORK_CONTEXT_AUTO_THRESHOLD: f32 = 0.94;
pub(super) const WORK_CONTEXT_AUTO_THRESHOLD_RELAXED: f32 = 0.90;

/// 按轮复盘的系统提示词模板。基线与放宽的置信度门槛都从同一组常量渲染
/// （[`llm_review_prompt`] / [`explicit_signal_prompt`]）：若提示词仍教旧门槛
/// （例如放宽后基线段漏改），守规模型会在放宽带内输出 pending_confirm，
/// 代码侧的调整等于失效。花括号哨兵走 replace 而非 format!，避免转义 JSON 示例。
pub(super) const LLM_REVIEW_PROMPT_TEMPLATE: &str = r#"你是 pinvou 的后台记忆整理器。你只做一件事：复盘刚刚这一轮对话，并对照已有记忆，输出是否需要保存、更新或跳过记忆。不要回答用户问题，不要解释你的判断。

你必须只输出 JSON，不要解释。格式：
{
  "items": [
    {
      "action": "skip | pending_confirm | auto_write | auto_update",
      "kind": "profile | preference | work_context | current_focus | recent_activity",
      "topic": "call_name | assistant_alias | answer_style | workflow_preference | document_preference | role_domain | project_context | task_pattern | tooling_context | output_expectation | current_work | completed_work",
      "content": "整理后的完整记忆内容",
      "confidence": 0.0,
      "ttl_days": null,
      "reason": "一句话说明"
    }
  ]
}

你会收到：
- trigger：触发原因，可能是 explicit_user_signal 或 delivery_complete。
- delivery_complete_hint：如果为 true，表示本轮可能完成了交付。你需要评估是否值得形成 recent_activity，但不要为了有交付就硬写记忆。
- current_user_message：本轮用户消息。
- assistant_response：本轮助手回复。
- delivery_summary：本轮交付物相关工具摘要，例如写入文件、修改文件、展示产物。只用于理解交付结果，不要保存工具过程。
- current_memory：当前注入给主模型的记忆摘要，仅作参考；结构化字段优先。
- existing_profile：已生效的用户资料。
- existing_preferences：已生效的长期偏好。
- existing_work_context：已生效的用户工作背景。
- active_current_focus：未过期的当前关注。
- active_recent_activity：未过期的近期动态。
- pending_memory：待用户确认的候选记忆。
- never_memory：用户不希望再提示的记忆。

记忆类别：
- profile：稳定、低敏的用户资料，例如用户希望被如何称呼、用户如何称呼助手。
- preference：长期使用习惯，例如回答风格、工作方式、文档偏好。
- work_context：用户长期工作背景，例如长期角色、领域、项目、任务类型、工具流、交付物期待。它描述用户，不描述 pinvou 的运行环境。
- current_focus：用户最近正在推进、后续短期内可能继续聊的事项，会过期。
- recent_activity：用户最近刚完成的交付、修复、报告、文档或调研，会过期。

topic 规则：
- profile 只使用 call_name 或 assistant_alias。
- preference 只使用 answer_style、workflow_preference、document_preference。
- work_context 只使用 role_domain、project_context、task_pattern、tooling_context、output_expectation。
- current_focus 使用 current_work。
- recent_activity 使用 completed_work。

判断原则：
1. 只记录以后仍然有用的信息。一次性问答、普通闲聊、临时情绪、问题本身、模型猜测都不要记。
2. 记忆必须以用户为中心。不要把 pinvou 当前模型、临时路径、调试状态、工具日志、文件原文当作用户记忆。
3. 不记录密码、手机号、证件号、token、API key、地址等敏感信息。
4. content 必须是清洗后的事实摘要，不要照抄整句，不要包含“请记住/以后你要”等命令口吻。
5. 同一主题已有记忆或 pending_memory 已覆盖时输出 skip。
6. 新信息修正或补充已有记忆时输出 auto_update 或 pending_confirm，并在 content 中给出合并后的完整版本，不要新增重复条目。
7. never_memory 中已有的内容不要再输出。
8. 不确定是否长期稳定时，优先 pending_confirm；不确定是否值得记时，输出 skip。

动作选择：
- skip：没有值得保存的信息，或已有记忆已经覆盖。
- pending_confirm：信息可能有用，但属于长期偏好、工作背景、敏感边界或判断不够确定，需要用户确认。
- auto_write：低敏、高置信、未来明显有用，且不会打扰用户确认也能安全保存。
- auto_update：低敏、高置信，且是对已有同主题记忆的合并或修正。

ttl_days 规则：
- profile / preference / work_context 使用 null。
- current_focus 默认使用 21。
- recent_activity 默认使用 14。

自动写入边界：
- profile 只有在用户非常明确表达，且 confidence >= {{PROFILE_AUTO_GATE}} 时才允许 auto_write。
- preference 默认 pending_confirm。
- work_context 默认 pending_confirm；只有用户明确要求记住、内容低敏且 confidence >= {{WORK_CONTEXT_AUTO_GATE}} 时，才允许 auto_write 或 auto_update。
- current_focus / recent_activity 内容清楚、低敏且 confidence >= {{TIMED_AUTO_GATE}} 时，默认使用 auto_write 或 auto_update；只有不确定、较敏感或用户可能不希望记录时才使用 pending_confirm。

近期记忆质量：
- current_focus 要写“用户正在推进什么，以及为什么后续还可能有用”。
- recent_activity 要写“完成了什么、交付物或结果是什么、后续继续该主题时有什么线索”。
- 不要只写“完成了某某某”，也不要记录普通工具过程。
- delivery_complete_hint=true 时要认真评估 recent_activity；如果交付结果清楚、低敏、对未来有用，优先 auto_write，不要仅因为它是近期动态就要求用户确认。

如果没有值得记的内容，输出 {"items":[]}。
"#;

/// 渲染后的复盘系统提示词：基线门槛从常量取值（见 [`LLM_REVIEW_PROMPT_TEMPLATE`]）。
pub(super) fn llm_review_prompt() -> String {
    LLM_REVIEW_PROMPT_TEMPLATE
        .replace(
            "{{PROFILE_AUTO_GATE}}",
            &PROFILE_AUTO_WRITE_THRESHOLD.to_string(),
        )
        .replace(
            "{{WORK_CONTEXT_AUTO_GATE}}",
            &WORK_CONTEXT_AUTO_THRESHOLD.to_string(),
        )
        .replace("{{TIMED_AUTO_GATE}}", &TIMED_AUTO_THRESHOLD.to_string())
}

/// trigger 为 explicit_user_signal 时追加到系统提示的硬约束：用户明确要求记住
/// 的内容不允许被 skip 掉，敏感边界保持不变；同时复述放宽后的置信度门槛。
/// 门槛放宽以 explicit_remember 为准（见 [`apply_llm_memory_review`]），提示词
/// 必须从同一组常量取值：若提示词仍教基线门槛，守规模型会在放宽区间输出
/// pending_confirm，代码侧的放宽等于失效。
pub(super) fn explicit_signal_prompt() -> String {
    format!(
        "\n\n本轮 trigger 为 explicit_user_signal：用户明确要求记住或表达了长期偏好。\
         用户明确要求记住的内容必须落在输出里（auto_write / auto_update / pending_confirm 之一），\
         不要输出 skip；仍不得记录敏感信息。\
         本轮置信度门槛已放宽：profile confidence >= {PROFILE_AUTO_WRITE_THRESHOLD_RELAXED} 即可 auto_write；\
         work_context confidence >= {WORK_CONTEXT_AUTO_THRESHOLD_RELAXED} 即可 auto_write / auto_update；\
         current_focus / recent_activity confidence >= {TIMED_AUTO_THRESHOLD_RELAXED}。"
    )
}

/// Output-language directive for the memory review prompt, appended to the
/// system prompt according to the UI locale.
///
/// Equivalent of the review-side `output_language_directive`
/// (features/review/mod.rs): the prompt body stays Chinese (tuned for
/// convergence; translating it line by line would introduce behavioral drift)
/// and only the **natural-language field values** (`content` / `reason`) switch
/// to the target language — JSON keys and `kind` / `topic` / `action` enum
/// values stay ASCII. zh-Hans and unknown locales → None (no-op, prompt
/// unchanged).
///
/// Reachability: `enforce_memory_locale_policy` (platform/prefs/mod.rs) forces
/// `memory_enabled` back to false for non-zh-Hans UI on every load/save, so in
/// the normal flow en/ja users never run memory review. This directive is
/// defense-in-depth mirroring the review-side precedent, not a fix for a
/// reachable failure — the zh-Hans branch is the live per-review path (it
/// hard-forces Chinese `content` even in English conversations, the same
/// measured drift the review side hit); the en/ja branches only cover the
/// narrow window where the locale was switched to Chinese with "restart later",
/// memory was re-enabled, and a pre-switch engine snapshot still carries the
/// old locale.
pub(super) fn memory_output_language_directive(locale_tag: &str) -> Option<String> {
    // zh-Hans: force `content` to Simplified Chinese even when the conversation
    // is in English — the Chinese prompt body alone is not hard enough, and the
    // values drift to English in English contexts (same issue measured on the
    // review side).
    if locale_tag == "zh-Hans" {
        return Some(
            "\n\n## 输出语言(强制)\n\
             JSON 里所有自然语言字段值(content / reason)必须用简体中文,即使本轮\
             对话是英文/日文也别跟着写。JSON 的 key、action / kind / topic 枚举值\
             保持原样 ASCII。"
                .to_string(),
        );
    }
    let lang = match locale_tag {
        "en" => "English",
        "ja" => "Japanese (日本語)",
        _ => return None, // unknown locale → keep the prompt as-is (Chinese)
    };
    Some(format!(
        "\n\n## Output Language (HARD override)\n\
         Write EVERY natural-language value in your JSON output in {lang}: `content` \
         and `reason`. This OVERRIDES any wording above that asks for Chinese. Keep \
         all JSON keys and enum values (`action`, `kind`, `topic`) exactly as \
         specified — those stay ASCII/English."
    ))
}

fn memory_review_log_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 写入不含对话原文的记忆复盘诊断信息。日志达到 2 MiB 后从空文件重新开始，
/// 避免后台复盘长期运行无限占用磁盘。
pub(crate) fn append_memory_review_diagnostic(session_id: &str, stage: &str, detail: Value) {
    let _guard = memory_review_log_lock().lock();
    let path = paths::memory_review_log();
    if let Err(err) = append_memory_review_diagnostic_to(&path, session_id, stage, detail) {
        eprintln!(
            "[pinvou3-app] append memory review diagnostic failed ({}): {err}",
            path.display()
        );
    }
}

pub(super) fn append_memory_review_diagnostic_to(
    path: &Path,
    session_id: &str,
    stage: &str,
    detail: Value,
) -> stdio::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::metadata(path)
        .map(|metadata| metadata.len() >= super::types::MEMORY_REVIEW_LOG_MAX_BYTES)
        .unwrap_or(false)
    {
        fs::remove_file(path)?;
    }
    let mut line = json!({
        "ts": Utc::now().to_rfc3339(),
        "session_id": clean_id(session_id),
        "stage": clean_text(stage, 48),
        "detail": detail,
    })
    .to_string();
    line.push('\n');
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

pub(super) fn memory_review_error_stage(error: &anyhow::Error) -> &'static str {
    let message = format!("{error:#}").to_ascii_lowercase();
    if message.contains("parse memory review") || message.contains("memory review response json") {
        "parse_failed"
    } else if message.contains("chat/completions")
        || message.contains("memory review client")
        || message.contains("error sending request")
    {
        "request_failed"
    } else {
        "apply_failed"
    }
}

pub async fn review_turn_candidates_with_llm(
    bridge: &(impl MemoryReviewModel + ?Sized),
    capture: &TurnMemoryCapture,
    session_id: &str,
) -> Result<MemoryReviewOutcome> {
    let user = clean_text(&capture.user, 4000);
    let assistant = clean_text(&capture.assistant, 4000);
    let delivery_summary = capture
        .tool_summaries
        .iter()
        .map(|s| clean_text(s, 600))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let explicit_signal = has_memory_review_signal(&user);
    // 写后果（门槛放宽 + 禁止 skip 硬约束）由窄口径的明确记录请求驱动；
    // 宽集只决定是否发起复盘（见 has_explicit_remember_signal 文档）。
    let explicit_remember = has_explicit_remember_signal(&user);
    let delivery_complete =
        capture.delivery_complete || assistant_suggests_delivery_complete(&user, &assistant);
    let skip_reason = if user.is_empty() {
        Some("empty_user_message")
    } else if looks_sensitive(&user) {
        Some("sensitive_content")
    } else if !explicit_signal && !delivery_complete {
        Some("no_review_signal")
    } else {
        None
    };
    if let Some(reason) = skip_reason {
        append_memory_review_diagnostic(
            session_id,
            "skipped",
            json!({
                "reason": reason,
                "user_chars": user.chars().count(),
                "assistant_chars": assistant.chars().count(),
                "tool_summary_count": delivery_summary.len(),
            }),
        );
        return Ok(MemoryReviewOutcome::default());
    }

    let trigger = if delivery_complete {
        "delivery_complete"
    } else {
        "explicit_user_signal"
    };
    append_memory_review_diagnostic(
        session_id,
        "triggered",
        json!({
            "trigger": trigger,
            "explicit_signal": explicit_signal,
            "explicit_remember": explicit_remember,
            "provider": bridge.memory_provider(),
            "model": bridge.memory_model(),
            "user_chars": user.chars().count(),
            "assistant_chars": assistant.chars().count(),
            "tool_summary_count": delivery_summary.len(),
        }),
    );
    let review = match request_llm_memory_review(
        bridge,
        &user,
        &assistant,
        trigger,
        explicit_remember,
        &delivery_summary,
    )
    .await
    {
        Ok(review) => review,
        Err(error) => {
            append_memory_review_diagnostic(
                session_id,
                memory_review_error_stage(&error),
                json!({ "error": clean_text(&format!("{error:#}"), 500) }),
            );
            return Err(error);
        }
    };
    let received_items = review.items.len();
    let mut action_counts = BTreeMap::<String, usize>::new();
    for item in &review.items {
        *action_counts
            .entry(clean_text(&item.action, 24))
            .or_default() += 1;
    }
    let outcome = match apply_llm_memory_review(review, explicit_remember) {
        Ok(outcome) => outcome,
        Err(error) => {
            append_memory_review_diagnostic(
                session_id,
                memory_review_error_stage(&error),
                json!({ "error": clean_text(&format!("{error:#}"), 500) }),
            );
            return Err(error);
        }
    };
    append_memory_review_diagnostic(
        session_id,
        "completed",
        json!({
            "received_items": received_items,
            "model_action_counts": action_counts,
            "auto_event_count": outcome.events.len(),
            "pending_candidate_count": outcome.pending.len(),
            "result": if outcome.pending.is_empty() && outcome.events.is_empty() {
                "no_memory_change"
            } else if outcome.pending.is_empty() {
                "auto_written"
            } else {
                "candidate_created"
            },
        }),
    );
    Ok(outcome)
}

pub(super) fn has_memory_review_signal(user: &str) -> bool {
    // CJK 词素无大小写之分，直接在原文匹配；ASCII 短语在 to_lowercase 后匹配，
    // 覆盖 "Remember" / "REMEMBER" 等大小写变体。
    let lower = user.to_lowercase();
    [
        "记住",
        "以后",
        "之后",
        "叫我",
        "称呼我",
        "我叫你",
        "你的名字",
        "我喜欢",
        "我不喜欢",
        "我偏好",
        "我的习惯",
        "记一下",
        "帮我记",
        "记录一下",
        "记着",
        "记好",
        "记牢",
        "默认",
        "优先",
        "尽量",
        "别太",
        "不要太",
        "长期",
        "经常",
        "负责",
        "参与",
        "最近在",
        "最近",
        "这周",
        "这周在",
        "本周",
        "本周在",
        "目前在",
        "主要在",
        "正在",
        "最近主要",
        "后面还",
        "继续",
    ]
    .iter()
    .any(|needle| user.contains(needle))
        || ["keep in mind", "don't forget", "do not forget"]
            .iter()
            .any(|needle| lower.contains(needle))
        || contains_imperative_remember(&lower)
}

/// 窄口径的“用户明确要求记住”判定：只有记录请求短语本身命中时，才追加
/// “禁止 skip”硬约束并放宽 auto_write 置信度门槛。[`has_memory_review_signal`]
/// 的宽集（最近 / 正在 / 继续 / 优先 等状态词）仍负责“是否发起复盘”——它们
/// 只说明本轮可能有值得记的内容，不构成明确的记录请求，不承担放宽的写后果。
pub(super) fn has_explicit_remember_signal(user: &str) -> bool {
    let lower = user.to_lowercase();
    [
        "记住",
        "记一下",
        "帮我记",
        "记录一下",
        "记着",
        "记好",
        "记牢",
    ]
    .iter()
    .any(|needle| user.contains(needle))
        || ["keep in mind", "don't forget", "do not forget"]
            .iter()
            .any(|needle| lower.contains(needle))
        || contains_imperative_remember(&lower)
}

/// "remember" 仅在祈使位置匹配（句首、标点之后或 "please" 之后的词首）：
/// explicit_user_signal 在本模块携带 auto_write 门槛放宽的实际写后果，而
/// "Do you remember...?" / "I don't remember ..." 这类陈述、疑问句中的
/// remember 并不是记录请求，宁窄勿宽。
fn contains_imperative_remember(lower: &str) -> bool {
    const NEEDLE: &str = "remember";
    let mut from = 0;
    while let Some(pos) = lower[from..].find(NEEDLE) {
        let start = from + pos;
        let after = &lower[start + NEEDLE.len()..];
        let word_end = !after.starts_with(|c: char| c.is_ascii_alphabetic());
        let before = lower[..start].trim_end();
        let lead = before.is_empty()
            || before.ends_with(['.', ',', '!', '?', ';', ':'])
            || before.ends_with("please");
        if word_end && lead {
            return true;
        }
        from = start + NEEDLE.len();
    }
    false
}

pub(super) fn assistant_suggests_delivery_complete(user: &str, assistant: &str) -> bool {
    let user = user.trim();
    let assistant = assistant.trim();
    if user.is_empty() || assistant.is_empty() {
        return false;
    }
    if assistant.chars().count() < 8 || looks_sensitive(assistant) {
        return false;
    }
    let negative = [
        "无法完成",
        "不能完成",
        "没完成",
        "未完成",
        "还没完成",
        "没有完成",
        "无法生成",
        "不能生成",
        "没生成",
        "无法修复",
        "不能修复",
    ];
    if negative.iter().any(|needle| assistant.contains(needle)) {
        return false;
    }
    [
        "已完成",
        "已经完成",
        "完成了",
        "已生成",
        "已经生成",
        "生成了",
        "写好了",
        "整理好了",
        "已整理",
        "已经整理",
        "已实现",
        "已经实现",
        "实现了",
        "已修复",
        "已经修复",
        "修复了",
        "已交付",
        "已经交付",
        "交付了",
        "已更新",
        "已经更新",
        "更新了",
    ]
    .iter()
    .any(|needle| assistant.contains(needle))
}

async fn request_llm_memory_review(
    bridge: &(impl MemoryReviewModel + ?Sized),
    user: &str,
    assistant: &str,
    trigger: &str,
    explicit_remember: bool,
    delivery_summary: &[String],
) -> Result<LlmMemoryReview> {
    let client = Client::builder()
        .timeout(LLM_REVIEW_TIMEOUT)
        .build()
        .context("build memory review client")?;
    let base_url = bridge.memory_base_url();
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let provider = bridge.memory_provider();
    let preset = bridge.memory_model_preset();
    let model_name = if provider == "vllm" {
        // The served-name probe uses an inference-same-origin key:
        // authenticated vLLM 401s on /v1/models.
        crate::features::monitor::probe_vllm_model_info(
            &base_url,
            Some(bridge.memory_api_key().as_str()),
        )
        .await
        .0
        .unwrap_or_else(|| bridge.memory_model())
    } else {
        bridge.memory_model()
    };
    let current_memory = super::render::render_memory_block()
        .map(|(block, _)| block)
        .unwrap_or_default();
    let existing_profile = io::load_profile().unwrap_or_default();
    let existing_preferences = io::load_preferences().unwrap_or_default();
    let existing_work_context = io::load_work_context().unwrap_or_default();
    let focus_items = io::load_current_focus().unwrap_or_default();
    let active_current_focus = io::active_timed_memory(&focus_items, Utc::now())
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let activity_items = io::load_recent_activity().unwrap_or_default();
    let active_recent_activity = io::active_timed_memory(&activity_items, Utc::now())
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let pending = io::load_pending_memory()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.status == super::types::PENDING_STATUS_PENDING)
        .collect::<Vec<_>>();
    let never = io::load_never_memory().unwrap_or_default();
    let user_content = json!({
        "trigger": trigger,
        "delivery_complete_hint": trigger == "delivery_complete",
        "current_user_message": user,
        "assistant_response": assistant,
        "delivery_summary": delivery_summary,
        "current_memory": current_memory,
        "existing_profile": existing_profile,
        "existing_preferences": existing_preferences,
        "existing_work_context": existing_work_context,
        "active_current_focus": active_current_focus,
        "active_recent_activity": active_recent_activity,
        "pending_memory": pending,
        "never_memory": never,
    })
    .to_string();
    // Append the output-language directive per locale, mirroring the review-side
    // output_language_directive precedent (defense-in-depth: memory is disabled
    // for non-Chinese UIs by enforce_memory_locale_policy; see the
    // memory_output_language_directive docs for reachability).
    let mut prompt = llm_review_prompt();
    // 用 explicit_remember（而非 trigger）驱动硬约束：门槛放宽以 explicit_remember
    // 为准，两者必须同开同关，否则会出现“门槛已放宽但 prompt 未禁止 skip”的
    // 不一致回合（如 explicit_signal 与 delivery_complete 并存时）。
    if explicit_remember {
        prompt.push_str(&explicit_signal_prompt());
    }
    if let Some(suffix) = memory_output_language_directive(&bridge.memory_locale_tag()) {
        prompt.push_str(&suffix);
    }
    // Anthropic 官方端点是 Messages 协议（x-api-key 鉴权，system 独立字段，
    // 无 response_format），走原生直连；其余 preset 仍走 OpenAI chat/completions。
    if preset == ModelPreset::Anthropic {
        let content = crate::core::model_endpoint::post_anthropic_messages(
            &client,
            &base_url,
            &bridge.memory_api_key(),
            &model_name,
            &prompt,
            &user_content,
            900,
        )
        .await?;
        return parse_llm_memory_review(&content);
    }
    let mut body = json!({
        "model": model_name,
        "messages": [
            { "role": "system", "content": prompt },
            { "role": "user", "content": user_content }
        ],
        "temperature": 0,
        "max_tokens": 900,
        "stream": false,
        "response_format": { "type": "json_object" }
    });
    apply_memory_review_reasoning_controls(&mut body, preset, &provider, &base_url, &model_name);
    let resp = client
        .post(url)
        .bearer_auth(bridge.memory_api_key())
        .json(&body)
        .send()
        .await
        .context("post memory review chat/completions")?
        .error_for_status()
        .context("memory review chat/completions status")?;
    let value: Value = resp
        .json()
        .await
        .context("parse memory review response json")?;
    let content = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    parse_llm_memory_review(content)
}

/// 应用已解析的复盘结果。`explicit_remember` 表示本轮命中了用户明确的“记住”
/// 类记录请求（窄口径 [`has_explicit_remember_signal`]）：auto_write /
/// auto_update 的置信度门槛按模块顶部常量放宽（profile / timed / work_context），
/// 清洗下限 0.70 与敏感过滤不变。
pub(super) fn apply_llm_memory_review(
    review: LlmMemoryReview,
    explicit_remember: bool,
) -> Result<MemoryReviewOutcome> {
    let timed_auto_threshold = if explicit_remember {
        TIMED_AUTO_THRESHOLD_RELAXED
    } else {
        TIMED_AUTO_THRESHOLD
    };
    let work_context_auto_threshold = if explicit_remember {
        WORK_CONTEXT_AUTO_THRESHOLD_RELAXED
    } else {
        WORK_CONTEXT_AUTO_THRESHOLD
    };
    let mut outcome = MemoryReviewOutcome::default();
    for raw in review.items {
        let Some(decision) = sanitize_llm_memory_item(raw, explicit_remember) else {
            continue;
        };
        let suggestion = decision.suggestion;
        if decision.action == "auto_write" && suggestion.kind == "profile" {
            if let Some(event) = auto_write_profile_suggestion(&suggestion)? {
                outcome.events.push(event);
            }
            continue;
        }
        if matches!(
            suggestion.kind.as_str(),
            "current_focus" | "recent_activity"
        ) && matches!(decision.action.as_str(), "auto_write" | "auto_update")
            && decision.confidence >= timed_auto_threshold
        {
            match io::upsert_timed_memory_locked(
                &suggestion.kind,
                &suggestion.topic,
                &suggestion.content,
                &suggestion.source,
                decision.ttl_days,
                decision.confidence,
            ) {
                Ok(item) => outcome.events.push(MemoryWriteEvent {
                    kind: item.kind,
                    action: "remembered".to_string(),
                    id: item.id,
                    text: item.text,
                }),
                Err(err) if err.kind() == stdio::ErrorKind::InvalidInput => {}
                Err(err) => return Err(err).context("auto write timed memory"),
            }
            continue;
        }
        if suggestion.kind == "work_context"
            && matches!(decision.action.as_str(), "auto_write" | "auto_update")
            && decision.confidence >= work_context_auto_threshold
        {
            match io::upsert_work_context_locked(&suggestion, decision.confidence) {
                Ok(item) => outcome.events.push(MemoryWriteEvent {
                    kind: item.kind,
                    action: "remembered".to_string(),
                    id: item.id,
                    text: item.text,
                }),
                Err(err) if err.kind() == stdio::ErrorKind::InvalidInput => {}
                Err(err) => return Err(err).context("auto write work context"),
            }
            continue;
        }
        match io::enqueue_memory_candidate(suggestion) {
            Ok(item) => outcome.pending.push(item),
            Err(err) if err.kind() == stdio::ErrorKind::InvalidInput => {}
            Err(err) => return Err(err).context("enqueue llm memory candidate"),
        }
    }
    Ok(outcome)
}

pub(super) fn sanitize_llm_memory_item(
    raw: LlmMemoryItem,
    explicit_remember: bool,
) -> Option<SanitizedMemoryDecision> {
    let action = clean_text(&raw.action, 24);
    if action == "skip" || raw.confidence < 0.70 {
        return None;
    }
    if !matches!(
        action.as_str(),
        "pending_confirm" | "auto_write" | "auto_update"
    ) {
        return None;
    }
    let raw_kind = clean_text(&raw.kind, 24);
    let mut kind = match raw_kind.as_str() {
        "profile" => "profile".to_string(),
        "work_context" => "work_context".to_string(),
        "current_focus" => "current_focus".to_string(),
        "recent_activity" => "recent_activity".to_string(),
        "recent_work" => {
            if looks_completed_work_status(&raw.content) {
                "recent_activity".to_string()
            } else {
                "current_focus".to_string()
            }
        }
        _ => "preference".to_string(),
    };
    let mut topic = clean_text(&raw.topic, 40);
    let mut content = super::util::clean_candidate_sentence(&raw.content, 180);
    let _reason = clean_text(&raw.reason, 120);
    if content.is_empty() || looks_sensitive(&content) {
        return None;
    }
    if raw_kind == "recent_work" {
        topic = if kind == "recent_activity" {
            "completed_work".to_string()
        } else {
            "current_work".to_string()
        };
    }
    if topic == "call_name" || topic == "assistant_alias" {
        kind = "profile".to_string();
    }
    if kind == "current_focus" && topic == "completed_work" {
        kind = "recent_activity".to_string();
    }
    if kind == "recent_activity" && topic == "current_work" {
        kind = "current_focus".to_string();
    }

    if kind == "profile" {
        topic = match topic.as_str() {
            "assistant_alias" => "assistant_alias".to_string(),
            "call_name" => "call_name".to_string(),
            _ => return None,
        };
        content = super::util::clean_memory_label(&clean_profile_memory_content(&content, &topic))?;
        // 用户明确要求记住时称呼类记忆门槛放宽（常量单一来源，见模块顶部）。
        let profile_auto_write_threshold = if explicit_remember {
            PROFILE_AUTO_WRITE_THRESHOLD_RELAXED
        } else {
            PROFILE_AUTO_WRITE_THRESHOLD
        };
        if action == "auto_write" && raw.confidence < profile_auto_write_threshold {
            return None;
        }
    } else if matches!(kind.as_str(), "current_focus" | "recent_activity") {
        if looks_task_like(&content) && !looks_recent_work_status(&content) {
            return None;
        }
        topic = normalize_timed_memory_topic(&kind, &topic);
    } else if kind == "work_context" {
        if content.chars().count() < 8 {
            return None;
        }
        topic = normalize_work_context_topic(&topic);
    } else {
        kind = "preference".to_string();
        if looks_sensitive_or_task_like(&content) || content.chars().count() < 6 {
            return None;
        }
        topic = normalize_preference_topic(&topic);
    }

    Some(SanitizedMemoryDecision {
        action,
        suggestion: MemorySuggestion {
            kind,
            topic,
            content,
            source: "llm_review".to_string(),
        },
        confidence: raw.confidence,
        ttl_days: raw.ttl_days,
    })
}

fn auto_write_profile_suggestion(
    suggestion: &MemorySuggestion,
) -> Result<Option<MemoryWriteEvent>> {
    let mut patch = ProfilePatch::default();
    let current = io::load_profile().context("load profile for auto memory write")?;
    let (id, text) = match suggestion.topic.as_str() {
        "call_name" if suggestion.content != current.identity.call_name => {
            patch.call_name = Some(suggestion.content.clone());
            (
                "profile.call_name".to_string(),
                format!("称呼：{}", suggestion.content),
            )
        }
        "assistant_alias" if suggestion.content != current.identity.assistant_alias => {
            patch.assistant_alias = Some(suggestion.content.clone());
            (
                "profile.assistant_alias".to_string(),
                format!("助手昵称：{}", suggestion.content),
            )
        }
        _ => return Ok(None),
    };
    io::update_profile(patch).context("auto write profile memory")?;
    Ok(Some(MemoryWriteEvent {
        kind: "profile".to_string(),
        action: "remembered".to_string(),
        id,
        text,
    }))
}

pub(super) fn parse_llm_memory_review(content: &str) -> Result<LlmMemoryReview> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(LlmMemoryReview::default());
    }
    match serde_json::from_str::<LlmMemoryReview>(trimmed) {
        Ok(review) => Ok(review),
        Err(first_err) => {
            let Some(json_text) = extract_json_object(trimmed) else {
                return Err(first_err).context("parse memory review json");
            };
            serde_json::from_str::<LlmMemoryReview>(json_text)
                .context("parse extracted memory review json")
        }
    }
}

fn extract_json_object(value: &str) -> Option<&str> {
    let start = value.find('{')?;
    let end = value.rfind('}')?;
    (start <= end).then(|| &value[start..=end])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryReviewReasoningDialect {
    None,
    ThinkingDisabled,
    QwenEnableThinking,
    VllmChatTemplate,
    Minimax,
}

impl From<crate::core::reasoning_dialect::ReasoningDialect> for MemoryReviewReasoningDialect {
    fn from(d: crate::core::reasoning_dialect::ReasoningDialect) -> Self {
        use crate::core::reasoning_dialect::ReasoningDialect as D;
        match d {
            D::None => MemoryReviewReasoningDialect::None,
            D::ThinkingDisabled => MemoryReviewReasoningDialect::ThinkingDisabled,
            D::QwenEnableThinking => MemoryReviewReasoningDialect::QwenEnableThinking,
            D::Minimax => MemoryReviewReasoningDialect::Minimax,
        }
    }
}

pub(super) fn apply_memory_review_reasoning_controls(
    body: &mut Value,
    preset: ModelPreset,
    provider: &str,
    base_url: &str,
    model: &str,
) {
    match memory_review_reasoning_dialect(preset, provider, base_url, model) {
        MemoryReviewReasoningDialect::ThinkingDisabled => {
            body["thinking"] = json!({ "type": "disabled" });
        }
        MemoryReviewReasoningDialect::QwenEnableThinking => {
            body["enable_thinking"] = json!(false);
        }
        MemoryReviewReasoningDialect::VllmChatTemplate => {
            body["chat_template_kwargs"] = json!({ "enable_thinking": false });
        }
        MemoryReviewReasoningDialect::Minimax => {
            body["thinking"] = json!({ "type": "disabled" });
            body["reasoning_split"] = json!(true);
        }
        MemoryReviewReasoningDialect::None => {}
    }
}

fn memory_review_reasoning_dialect(
    preset: ModelPreset,
    provider: &str,
    base_url: &str,
    model: &str,
) -> MemoryReviewReasoningDialect {
    use crate::core::reasoning_dialect::{
        kimi_supports_disabled_thinking, reasoning_dialect_from_base_url,
    };
    if provider == "vllm" || preset == ModelPreset::LocalVllm {
        return MemoryReviewReasoningDialect::VllmChatTemplate;
    }
    if provider == "deepseek" || preset == ModelPreset::Deepseek {
        return MemoryReviewReasoningDialect::ThinkingDisabled;
    }
    match preset {
        ModelPreset::Qwen => MemoryReviewReasoningDialect::QwenEnableThinking,
        ModelPreset::Doubao | ModelPreset::Glm | ModelPreset::Mimo => {
            MemoryReviewReasoningDialect::ThinkingDisabled
        }
        ModelPreset::Minimax => MemoryReviewReasoningDialect::Minimax,
        ModelPreset::Kimi => {
            // Wave 3 统一：使用共享的 kimi_supports_disabled_thinking（与 review 一致）。
            // 原 memory 用 model.contains("k2.6")||model.contains("kimi-k2") 门控更宽，
            // 统一后 kimi-k2.5 也被正确识别，k2.7/thinking 变体被正确排除。
            if kimi_supports_disabled_thinking(model) {
                MemoryReviewReasoningDialect::ThinkingDisabled
            } else {
                MemoryReviewReasoningDialect::None
            }
        }
        ModelPreset::OpenaiCompatible
        | ModelPreset::LocalVllm
        | ModelPreset::Deepseek
        | ModelPreset::Openai
        | ModelPreset::Anthropic
        | ModelPreset::Gemini
        | ModelPreset::Xai => {
            // 先取共享的 URL sniff 结果;若 URL 无法识别厂商,回退到 model 名匹配
            // (保留原 memory 的 model.contains 回退,覆盖自定义 OpenAI 兼容端点)。
            let d = reasoning_dialect_from_base_url(base_url, model);
            if matches!(d, crate::core::reasoning_dialect::ReasoningDialect::None) {
                let lower = model.to_ascii_lowercase();
                if lower.contains("qwen") {
                    return MemoryReviewReasoningDialect::QwenEnableThinking;
                }
                if lower.contains("deepseek") {
                    return MemoryReviewReasoningDialect::ThinkingDisabled;
                }
            }
            d.into()
        }
    }
}

pub(super) fn discover_turn_suggestions(user: &str) -> Vec<MemorySuggestion> {
    let text = clean_text(user, 500);
    if text.is_empty() || looks_sensitive(&text) {
        return Vec::new();
    }

    let clauses: Vec<String> = text
        .split(['。', '！', '？', '；', ';', '\n'])
        .map(|s| clean_text(s, 160))
        .filter(|s| !s.is_empty())
        .collect();

    let mut out = Vec::new();
    for clause in clauses {
        if let Some(content) = preference_candidate_text(&clause) {
            out.push(MemorySuggestion {
                kind: "preference".to_string(),
                topic: "output_preference".to_string(),
                content,
                source: "auto_review".to_string(),
            });
            continue;
        }
        if let Some(content) = recent_work_candidate_text(&clause) {
            out.push(MemorySuggestion {
                kind: "recent_work".to_string(),
                topic: "current_work".to_string(),
                content,
                source: "auto_review".to_string(),
            });
        }
    }
    out
}

fn preference_candidate_text(clause: &str) -> Option<String> {
    let has_trigger = [
        "我喜欢",
        "我不喜欢",
        "我偏好",
        "我的习惯",
        "以后回答",
        "以后默认",
        "以后都",
        "每次都",
        "尽量",
        "不要太",
        "别太",
        "默认用",
        "优先用",
    ]
    .iter()
    .any(|needle| clause.contains(needle));
    if !has_trigger || looks_sensitive_or_task_like(clause) {
        return None;
    }
    let content = super::util::clean_candidate_sentence(clause, 80);
    if content.chars().count() < 6 {
        return None;
    }
    Some(content)
}

fn recent_work_candidate_text(clause: &str) -> Option<String> {
    let has_time_trigger = [
        "最近在",
        "这周在",
        "本周在",
        "目前在",
        "正在",
        "继续",
        "上次那个",
        "最近主要",
    ]
    .iter()
    .any(|needle| clause.contains(needle));
    let has_work_noun = [
        "项目", "材料", "报告", "PPT", "方案", "文档", "工作", "任务", "周报", "汇报",
    ]
    .iter()
    .any(|needle| clause.contains(needle));
    if !has_time_trigger || !has_work_noun || looks_sensitive(clause) {
        return None;
    }
    let content = super::util::clean_candidate_sentence(clause, 60);
    if content.chars().count() < 6 {
        return None;
    }
    Some(content)
}
