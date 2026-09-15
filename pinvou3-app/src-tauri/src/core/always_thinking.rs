//! Knowledge table and runtime normalization for always-thinking models
//! (thinking cannot be disabled).
//!
//! Local routing defaults to thinking off (to guard against SSE timeouts /
//! first-chunk preemption), but one class of models is always-thinking —
//! thinking cannot be disabled and there is no controllable effort tier, or
//! only some tiers are allowed. Sending "off" or an out-of-range tier would
//! only be ignored/rejected by the server. This table identifies such models
//! by name for `features::assistant::platform::bridge::request_reasoning_effort`
//! to normalize.
//!
//! Design convention: when a framework explicitly reports whether thinking can
//! be disabled, the framework wins; this table is only a fallback. Currently
//! none of the deployment frameworks (vLLM / SGLang / Ollama, etc.) report
//! thinking toggleability, so at runtime we can only match by model name.

/// Controllable shape of always-thinking models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlwaysThinkingSpec {
    /// Always-thinking but tiers are controllable: only the listed tiers are
    /// allowed (the first entry is the normalization target for out-of-range
    /// or missing tiers, i.e. the lowest tier the model allows).
    Tiers(&'static [&'static str]),
    /// Always-thinking with no controllable tier at all: the runtime sends no
    /// thinking parameter and lets the model think natively.
    NoControl,
}

/// Look up the knowledge table by model id. Matching rules: trim the id,
/// lowercase it, collapse runs of `_`/whitespace into a single `-`, then do a
/// substring match (covers common spellings such as `vendor/Model_Name`, tags,
/// and quantization suffixes). Aligned with the frontend normalization rules of
/// `model-catalog.js alwaysThinkingSpecForModel`
/// (`trim().toLowerCase().replaceAll(/[\s_]+/g, '-')`).
///
/// Accepted risk: substring matching also covers future names (e.g. a future
/// `kimi-k3.5` matches the `kimi-k3` entry); entries are re-reviewed as new
/// models ship.
pub fn always_thinking_spec(model_id: &str) -> Option<AlwaysThinkingSpec> {
    let trimmed = model_id.trim();
    let mut normalized = String::with_capacity(trimmed.len());
    let mut pending_sep = false;
    for ch in trimmed.chars() {
        if ch == '_' || ch.is_whitespace() {
            pending_sep = true;
        } else {
            // Collapse separator runs into a single '-'; a leading separator
            // (only `_` left after trim) produces no leading '-', which does
            // not affect substring matching (no table entry starts with '-').
            if pending_sep && !normalized.is_empty() {
                normalized.push('-');
            }
            pending_sep = false;
            normalized.push(ch.to_ascii_lowercase());
        }
    }
    let id = normalized.as_str();
    // Kimi K3: officially only low/high/max tiers and always-thinking; the
    // engine's vllm wire clamps max to high, so only low/high are exposed.
    if id.contains("kimi-k3") {
        return Some(AlwaysThinkingSpec::Tiers(&["low", "high"]));
    }
    // GLM-5.3: same as Kimi K3 — officially only low/high/max and
    // always-thinking, max is clamped to high by the engine, so only low/high
    // are exposed. Scope note: this entry applies to local routes only; the
    // cloud exact route (z.ai first-party) deliberately keeps its own
    // ['off', 'high', 'max'] tiers from the hosted API contract.
    if id.contains("glm-5.3") {
        return Some(AlwaysThinkingSpec::Tiers(&["low", "high"]));
    }
    // GLM-4.7: same as above (low/high/max and always-thinking, max clamped to
    // high).
    if id.contains("glm-4.7") {
        return Some(AlwaysThinkingSpec::Tiers(&["low", "high"]));
    }
    // GPT-OSS: officially only low/medium/high tiers, always-thinking (no off).
    if id.contains("gpt-oss") {
        return Some(AlwaysThinkingSpec::Tiers(&["low", "medium", "high"]));
    }
    // Kimi K2 Thinking / K2.5 Thinking / K2.7: thinking cannot be disabled and
    // no tiers are controllable.
    if id.contains("kimi-k2-thinking")
        || id.contains("kimi-k2.5-thinking")
        || id.contains("kimi-k2.7")
    {
        return Some(AlwaysThinkingSpec::NoControl);
    }
    // DeepSeek-R1 family (including distill): thinking cannot be disabled and
    // no tiers are controllable.
    if id.contains("deepseek-r1") {
        return Some(AlwaysThinkingSpec::NoControl);
    }
    // Qwen3 Thinking variants: thinking cannot be disabled and no tiers are
    // controllable. Plain qwen3 (e.g. qwen3-32b) is controllable and can be
    // disabled; it does not contain "thinking", so it does not match.
    if id.contains("qwen3") && id.contains("thinking") {
        return Some(AlwaysThinkingSpec::NoControl);
    }
    // MiniMax-M2: thinking cannot be disabled and no tiers are controllable.
    if id.contains("minimax-m2") {
        return Some(AlwaysThinkingSpec::NoControl);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tier-controllable entries: exact tier tables and the normalization
    /// target (first entry).
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

    /// NoControl entries: thinking cannot be disabled and no tiers are
    /// controllable.
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

    /// Matching rules: case-insensitive; `_`/spaces are normalized to `-`
    /// before substring matching.
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

    /// Normalization aligned with the frontend JS rules: trim + collapse runs
    /// of `_`/whitespace into a single `-`.
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

    /// Only qwen3 Thinking variants match: plain qwen3 (qwen3-32b, qwen3:8b) is
    /// controllable and can be disabled, so it does not match; qwen3-thinking
    /// matches NoControl.
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

    /// Unknown/controllable models do not match.
    #[test]
    fn unrelated_models_do_not_match() {
        assert_eq!(always_thinking_spec("llama3.2:3b"), None);
        assert_eq!(always_thinking_spec("glm-5.2"), None);
        assert_eq!(always_thinking_spec("kimi-k2"), None);
        assert_eq!(always_thinking_spec(""), None);
    }
}
