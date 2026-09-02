//! 思考不可关闭（always-thinking）模型的知识表与运行时归一。
//!
//! 本地路由默认关思考（防 SSE timeout / 首包抢占），但有一类模型思考永开、
//! 关不掉也无档位可控，或只允许部分档位——把 "off" 或越界档位发过去只会被
//! 服务端忽略/报错。本表按模型名识别这类模型，供
//! `features::assistant::platform::bridge::request_reasoning_effort` 归一。
//!
//! 设计约定：框架明确回报可关性时以框架为准，本表只是兜底；目前各部署框架
//! （vLLM / SGLang / Ollama 等）均不回报思考可关性，运行时只能靠模型名匹配。

/// 思考不可关闭模型的可控形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlwaysThinkingSpec {
    /// 思考永开但档位可控：只允许列出的档位（首项为越界/缺省时的归一目标，
    /// 即该模型允许的最低档）。
    Tiers(&'static [&'static str]),
    /// 思考永开且无任何可控档位：运行时不发任何思考参数，让模型原生思考。
    NoControl,
}

/// 按模型 id 查知识表。匹配口径：id trim 后小写化，并把 `_`/空白字符的连续
/// 段折叠为单个 `-`，再做子串匹配（覆盖 `vendor/Model_Name`、tag、量化后缀
/// 等常见书写形态）。与前端 `model-catalog.js alwaysThinkingSpecForModel`
/// 的归一口径（`trim().toLowerCase().replaceAll(/[\s_]+/g, '-')`）对齐。
pub fn always_thinking_spec(model_id: &str) -> Option<AlwaysThinkingSpec> {
    let trimmed = model_id.trim();
    let mut normalized = String::with_capacity(trimmed.len());
    let mut pending_sep = false;
    for ch in trimmed.chars() {
        if ch == '_' || ch.is_whitespace() {
            pending_sep = true;
        } else {
            // 折叠分隔符连续段为单个 '-'；前导分隔符（trim 后仅剩 `_`）不产生
            // 前导 '-'，对子串匹配无影响（表内条目均不以 '-' 开头）。
            if pending_sep && !normalized.is_empty() {
                normalized.push('-');
            }
            pending_sep = false;
            normalized.push(ch.to_ascii_lowercase());
        }
    }
    let id = normalized.as_str();
    // Kimi K3：官方仅 low/high/max 档位且思考永开；底座 vllm wire 会把 max
    // 钳到 high，故只暴露 low/high。
    if id.contains("kimi-k3") {
        return Some(AlwaysThinkingSpec::Tiers(&["low", "high"]));
    }
    // GLM-5.3：同 Kimi K3——官方仅 low/high/max 且思考永开，max 被底座钳到
    // high，只暴露 low/high。
    if id.contains("glm-5.3") {
        return Some(AlwaysThinkingSpec::Tiers(&["low", "high"]));
    }
    // GLM-4.7：同上（low/high/max 且思考永开，max 钳到 high）。
    if id.contains("glm-4.7") {
        return Some(AlwaysThinkingSpec::Tiers(&["low", "high"]));
    }
    // GPT-OSS：官方仅 low/medium/high 档位，思考永开（无 off）。
    if id.contains("gpt-oss") {
        return Some(AlwaysThinkingSpec::Tiers(&["low", "medium", "high"]));
    }
    // Kimi K2 Thinking / K2.5 Thinking / K2.7：思考不可关且无档位可控。
    if id.contains("kimi-k2-thinking")
        || id.contains("kimi-k2.5-thinking")
        || id.contains("kimi-k2.7")
    {
        return Some(AlwaysThinkingSpec::NoControl);
    }
    // DeepSeek-R1 系列（含 distill）：思考不可关且无档位可控。
    if id.contains("deepseek-r1") {
        return Some(AlwaysThinkingSpec::NoControl);
    }
    // Qwen3 Thinking 变体：思考不可关且无档位可控。普通 qwen3（如
    // qwen3-32b）可控可关，不含 "thinking" 不命中。
    if id.contains("qwen3") && id.contains("thinking") {
        return Some(AlwaysThinkingSpec::NoControl);
    }
    // MiniMax-M2：思考不可关且无档位可控。
    if id.contains("minimax-m2") {
        return Some(AlwaysThinkingSpec::NoControl);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 档位可控条目：精确档位表与归一目标（首项）。
    #[test]
    fn tiers_entries_match_expected_tables() {
        for model in ["kimi-k3", "moonshotai/Kimi-K3-Instruct"] {
            assert_eq!(
                always_thinking_spec(model),
                Some(AlwaysThinkingSpec::Tiers(&["low", "high"])),
                "{model}"
            );
        }
        for model in ["glm-5.3", "glm-4.7"] {
            assert_eq!(
                always_thinking_spec(model),
                Some(AlwaysThinkingSpec::Tiers(&["low", "high"])),
                "{model}"
            );
        }
        assert_eq!(
            always_thinking_spec("gpt-oss-120b"),
            Some(AlwaysThinkingSpec::Tiers(&["low", "medium", "high"]))
        );
    }

    /// NoControl 条目：思考不可关且无档位可控。
    #[test]
    fn no_control_entries_match() {
        for model in [
            "kimi-k2-thinking",
            "kimi-k2.5-thinking",
            "kimi-k2.7",
            "deepseek-r1",
            "deepseek-r1-distill-qwen-32b",
            "minimax-m2",
        ] {
            assert_eq!(
                always_thinking_spec(model),
                Some(AlwaysThinkingSpec::NoControl),
                "{model}"
            );
        }
    }

    /// 匹配口径：大小写不敏感，`_`/空格归一为 `-` 后子串匹配。
    #[test]
    fn matching_normalizes_case_underscores_and_spaces() {
        assert_eq!(
            always_thinking_spec("Kimi_K3"),
            Some(AlwaysThinkingSpec::Tiers(&["low", "high"]))
        );
        assert_eq!(
            always_thinking_spec("DeepSeek R1 0528"),
            Some(AlwaysThinkingSpec::NoControl)
        );
        assert_eq!(
            always_thinking_spec("MINIMAX_M2"),
            Some(AlwaysThinkingSpec::NoControl)
        );
    }

    /// 归一与前端 JS 口径对齐：trim + 连续 `_`/空白段折叠为单个 `-`。
    #[test]
    fn matching_trims_and_collapses_separator_runs() {
        assert_eq!(
            always_thinking_spec("  Kimi  K3  "),
            Some(AlwaysThinkingSpec::Tiers(&["low", "high"]))
        );
        assert_eq!(
            always_thinking_spec("deepseek__r1"),
            Some(AlwaysThinkingSpec::NoControl)
        );
        assert_eq!(
            always_thinking_spec("Qwen3-235B-A22B_Thinking"),
            Some(AlwaysThinkingSpec::NoControl)
        );
        assert_eq!(always_thinking_spec("   "), None);
    }

    /// qwen3 只有 Thinking 变体命中：普通 qwen3（qwen3-32b、qwen3:8b）可控
    /// 可关，不命中；qwen3-thinking 命中 NoControl。
    #[test]
    fn qwen3_matches_only_thinking_variants() {
        assert_eq!(always_thinking_spec("qwen3-32b"), None);
        assert_eq!(always_thinking_spec("qwen3:8b"), None);
        assert_eq!(always_thinking_spec("qwen3-coder-480b"), None);
        assert_eq!(
            always_thinking_spec("qwen3-thinking-2507"),
            Some(AlwaysThinkingSpec::NoControl)
        );
    }

    /// 未知/可控模型不命中。
    #[test]
    fn unrelated_models_do_not_match() {
        assert_eq!(always_thinking_spec("llama3.2:3b"), None);
        assert_eq!(always_thinking_spec("glm-5.2"), None);
        assert_eq!(always_thinking_spec("kimi-k2"), None);
        assert_eq!(always_thinking_spec(""), None);
    }
}
