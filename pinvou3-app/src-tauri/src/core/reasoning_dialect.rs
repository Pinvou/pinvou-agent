//! 共享的推理 dialect 检测。
//!
//! Wave 3 合并：review 与 memory 此前各维护一份 reasoning-dialect 检测，
//! 行为不等价（review 7 厂商簇 + 剥 `/v1`，memory 仅 2 簇 + model 回退）。
//! 用户决策：以 review 覆盖面为准修正 memory 缺口。
//!
//! 此模块提供：
//! - [`ReasoningDialect`]：统一的 dialect 枚举（两特性变体集相同）
//! - [`reasoning_dialect_from_base_url`]：URL 嗅探（7 厂商簇 + 剥 `/v1`）
//! - [`kimi_supports_disabled_thinking`]：Kimi 模型门控
//!
//! 各 feature 的 wrapper（`review_reasoning_dialect` / `memory_review_reasoning_dialect`）
//! 仍保留各自的 preset 分发逻辑，但 URL 回退统一委托此模块。

/// 推理 dialect 枚举。review 与 memory 变体集完全相同，统一于此。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningDialect {
    None,
    ThinkingDisabled,
    QwenEnableThinking,
    VllmChatTemplate,
    Minimax,
}

/// Kimi 模型是否支持 disabled thinking。
///
/// kimi-k2.5 / kimi-k2.6 支持；thinking 变体和 k2.7 不支持。
pub fn kimi_supports_disabled_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    (model.contains("kimi-k2.5") || model.contains("kimi-k2.6"))
        && !model.contains("thinking")
        && !model.contains("k2.7")
}

/// 从 base_url 嗅探 reasoning dialect（OpenAI-compatible 回退路径）。
///
/// 规范化：trim + 剥尾部 `/`、`/chat/completions`、`/v1`，再小写。
/// 覆盖 7 个厂商簇：deepseek、dashscope(qwen)、moonshot(kimi)、volces/doubao、
/// minimax、bigmodel/glm/z.ai、xiaomimimo(mimo)。
pub fn reasoning_dialect_from_base_url(base_url: &str, model: &str) -> ReasoningDialect {
    let normalized = base_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches("/chat/completions")
        .trim_end_matches("/v1")
        .to_ascii_lowercase();

    if normalized.contains("api.deepseek.com") || normalized.contains("api.deepseeki.com") {
        ReasoningDialect::ThinkingDisabled
    } else if normalized.contains("dashscope.aliyuncs.com") {
        ReasoningDialect::QwenEnableThinking
    } else if normalized.contains("moonshot.cn") || normalized.contains("moonshot.ai") {
        if kimi_supports_disabled_thinking(model) {
            ReasoningDialect::ThinkingDisabled
        } else {
            ReasoningDialect::None
        }
    } else if normalized.contains("volces.com")
        || normalized.contains("volcengine")
        || normalized.contains("byteplus.com")
    {
        ReasoningDialect::ThinkingDisabled
    } else if normalized.contains("minimax.chat") || normalized.contains("minimaxi.com") {
        ReasoningDialect::Minimax
    } else if normalized.contains("bigmodel.cn") || normalized.contains("z.ai") {
        ReasoningDialect::ThinkingDisabled
    } else if normalized.contains("xiaomimimo.com") {
        ReasoningDialect::ThinkingDisabled
    } else {
        ReasoningDialect::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_strips_v1_suffix() {
        assert_eq!(
            reasoning_dialect_from_base_url("https://api.deepseek.com/v1", ""),
            ReasoningDialect::ThinkingDisabled
        );
    }

    #[test]
    fn deepseeki_variant_detected() {
        assert_eq!(
            reasoning_dialect_from_base_url("https://api.deepseeki.com/chat/completions", ""),
            ReasoningDialect::ThinkingDisabled
        );
    }

    #[test]
    fn qwen_dashscope_detected() {
        assert_eq!(
            reasoning_dialect_from_base_url("https://dashscope.aliyuncs.com", ""),
            ReasoningDialect::QwenEnableThinking
        );
    }

    #[test]
    fn moonshot_kimi_k26_supports_disabled() {
        assert_eq!(
            reasoning_dialect_from_base_url("https://moonshot.cn", "kimi-k2.6"),
            ReasoningDialect::ThinkingDisabled
        );
    }

    #[test]
    fn moonshot_kimi_k27_does_not_support() {
        assert_eq!(
            reasoning_dialect_from_base_url("https://moonshot.cn", "kimi-k2.7"),
            ReasoningDialect::None
        );
    }

    #[test]
    fn doubao_volcengine_detected() {
        assert_eq!(
            reasoning_dialect_from_base_url("https://ark.volces.com/api/v3", ""),
            ReasoningDialect::ThinkingDisabled
        );
    }

    #[test]
    fn minimax_detected() {
        assert_eq!(
            reasoning_dialect_from_base_url("https://api.minimax.chat/v1", ""),
            ReasoningDialect::Minimax
        );
    }

    #[test]
    fn glm_bigmodel_detected() {
        assert_eq!(
            reasoning_dialect_from_base_url("https://open.bigmodel.cn", ""),
            ReasoningDialect::ThinkingDisabled
        );
    }

    #[test]
    fn mimo_xiaomimimo_detected() {
        assert_eq!(
            reasoning_dialect_from_base_url("https://api.xiaomimimo.com", ""),
            ReasoningDialect::ThinkingDisabled
        );
    }

    #[test]
    fn unknown_url_returns_none() {
        assert_eq!(
            reasoning_dialect_from_base_url("https://api.openai.com", ""),
            ReasoningDialect::None
        );
    }

    #[test]
    fn kimi_thinking_variant_excluded() {
        assert!(!kimi_supports_disabled_thinking("kimi-k2.6-thinking"));
    }
}
