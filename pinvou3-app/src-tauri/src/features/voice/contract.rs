//! Byte contract shared verbatim with `pinvou-cli`'s `voice` family.
//!
//! `pinvou-cli/crates/cli/src/voice.rs` used to hand-copy this module's items
//! and demand a same-PR bump on every drift (its "DRIFT GUARD" / "duplicated
//! here verbatim" comments; findings CL4-1 / RT27 F27-2). The items here are
//! the single source now: the CLI imports them from
//! `pinvou3_lib::features::voice::contract`, so a prompt, budget, model spec,
//! or the download band cannot change on one surface without the other seeing
//! it at compile time. The GUI keeps calling them unchanged through the `use`
//! re-imports in `app/commands/voice.rs` — no call site moved.
//!
//! Scope is exactly the blocks the CLI previously mirrored byte-identically.
//! Deliberately NOT exposed here — that divergence is the disclosed CLI
//! contract, not drift: engine/model discovery (`engine_path` / `model_path`
//! / `model_available`: the CLI has no AppHandle-injected bundled dir and no
//! Windows MSI view), ffmpeg probe budgets (10 s GUI probe vs the CLI's 15 s
//! supervised probe), external ASR command resolution (the CLI falls through
//! to the next env candidate), the native/external transcribe lanes, and the
//! CLI's deterministic subset of the reasoning controls.
//!
//! One verdict-level divergence the CLI inherited with this module (round-3
//! review): on targets outside linux/macOS/windows the re-exported platform
//! adapters answer the app's stub verdicts — engine name `sense-voice-main`
//! and a placeholder model spec (size 0, empty sha/urls), so `model_available`
//! can never hold and `asr-install` would refuse rather than download. The
//! CLI's old hand-rolled else-branch answered `pinvou-asr` with the real q4
//! spec instead; every supported desktop target behaves identically to it.

use std::time::Duration;

pub use super::platform::engine_binary_name;
pub use super::voice_asr::{AsrModelSpec, asr_dir, asr_download_max_bytes, current_model_spec};
pub use crate::core::reasoning_dialect::kimi_supports_disabled_thinking;

pub(crate) fn normalize_voice_postprocess_mode(mode: &str) -> &'static str {
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

pub fn voice_postprocess_prompt(mode: &str) -> &'static str {
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

pub fn voice_postprocess_timeout(mode: &str, raw_text: &str) -> Duration {
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
pub fn voice_postprocess_user_content(
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

pub fn voice_postprocess_retry_prompt(mode: &str) -> &'static str {
    if mode == "edit" {
        "你是语音编辑器。当前输入框已有文本是正文，ASR 文本是修改指令，不是要追加的正文。必须根据 ASR 指令修改正文，并只输出修改后的完整正文。除非 ASR 是纯口头禅、纯噪声或完全无法理解，否则禁止原样输出当前输入框已有文本。禁止解释，禁止空输出。"
    } else if mode == "task" {
        "你是语音任务纠错器。把 ASR 纠正为用户要交给 Agent 执行的任务。保留所有时间、地点、格式和限制条件。禁止新增事实。必须只输出最终任务文本，禁止空输出；除非 ASR 是纯口头禅或纯噪声。"
    } else {
        "你是语音听写整理器。把 ASR 纠正并整理为用户想输入的文本。只有极短、单一、无需拆解的自然句才输出自然句；除此以外默认整理成结构化 Markdown 列表。内容包含目标、用途、功能、字段、截止时间、进度、多个事项、多个条件、步骤、约束或明显需求表达时，必须结构化。保留所有时间、地点、格式和限制条件。禁止新增事实。必须只输出最终文本，禁止空输出；除非 ASR 是纯口头禅或纯噪声。"
    }
}

pub fn voice_postprocess_max_tokens(mode: &str, retry: bool) -> u32 {
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
pub const VOICE_POSTPROCESS_MAX_INPUT_CHARS: usize = 4000;

pub fn truncate_voice_postprocess_input(text: &str) -> String {
    text.chars()
        .take(VOICE_POSTPROCESS_MAX_INPUT_CHARS)
        .collect()
}

pub fn sanitize_voice_postprocess_output(text: &str) -> String {
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
pub fn strip_leading_thinking_block(text: &str) -> &str {
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

pub fn strip_wrapping_quote(text: &str, quote: char) -> &str {
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
pub fn strip_voice_markdown_fence(text: &str) -> &str {
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
