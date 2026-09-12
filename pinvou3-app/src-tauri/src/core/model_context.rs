//! Model context-window resolution shared by pinvou3 runtime state and Engine
//! routing.
//!
//! Model facts maintained by CodeWhale reuse the base catalog first; this module
//! only supplements cloud models that the pinvou3 settings page already provides
//! but the base does not cover yet. Both the window facts (resolved by name) and
//! the window precedence (declared vs probed vs inferred) have their single entry
//! point here, so the page display never uses a different scale than
//! `active_route_limits` / compaction thresholds / the monitor denominator.

/// 精确匹配模型名，并容忍 `-` 分隔的日期、快照或服务档位后缀。
fn model_name_matches(lower: &str, name: &str) -> bool {
    lower == name
        || lower
            .strip_prefix(name)
            .is_some_and(|rest| rest.starts_with('-'))
}

/// CodeWhale 已统一解析 `Nk` 后缀；这里只补它尚未覆盖的 `1m` 写法。
fn explicit_one_million_hint(lower: &str) -> Option<u32> {
    lower.contains("1m").then_some(1_048_576)
}

/// pinvou3 对底座的定向覆盖：底座已收录但数值落后于官方口径的模型。
/// 优先于底座 catalog 生效；上游修复后应移除对应条目。
const PINVOU_OVERRIDES: &[(&str, u32)] = &[
    // 底座对未知名 claude 一律兜底 200K，且未收录 claude-opus-5；
    // 官方口径 Claude 5 系（除 haiku 外）均为 1M 上下文。
    ("claude-opus-5", 1_000_000),
];

/// 解析 pinvou3 已知模型的上下文窗口。
///
/// 顺序为：显式 `1m` → pinvou3 定向覆盖 → CodeWhale 模型 catalog/`Nk` 启发式 → pinvou3 补充表。
#[must_use]
pub fn resolved_context_window(model: &str) -> Option<u32> {
    let lower = model.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    if let Some(window) = explicit_one_million_hint(&lower) {
        return Some(window);
    }
    if let Some((_, window)) = PINVOU_OVERRIDES
        .iter()
        .find(|(name, _)| model_name_matches(&lower, name))
    {
        return Some(*window);
    }
    if let Some(window) = deepseek_tui::models::context_window_for_model(&lower) {
        return Some(window);
    }

    const PINVOU_KNOWN: &[(&str, u32)] = &[
        // Kimi K3 直连平台模型官方标称 100 万 token（platform.kimi.com/docs/models）。
        ("kimi-k3", 1_048_576),
        // Coding Plan 裸 k3 的窗口取决于套餐；底座以 256K 安全值收录，1M 套餐应显式配置。
        // kimi-for-coding 系属 K2.7 Code，官方 256K。
        ("kimi-for-coding-highspeed", 262_144),
        ("kimi-k2.7-code-highspeed", 262_144),
        // 阿里云官方文档给 qwen3.7-plus/max/flash 1M 上下文。
        ("qwen3.7-plus", 1_000_000),
        ("qwen3.7-max", 1_000_000),
        ("qwen3.7-flash", 1_000_000),
        // qwen3.8-max 已 GA：官方口径上下文 1M（983,616 为思考模式最大输入，非上下文窗口），
        // `-preview` 后缀由 model_name_matches 容忍。
        ("qwen3.8-max", 1_000_000),
        // 底座当前只覆盖带 `qwen/` 前缀的 qwen3.6-flash。
        ("qwen3.6-flash", 1_000_000),
        // 2026-07 火山引擎公告：doubao-seed-evolving 升为 1M 上下文。
        ("doubao-seed-evolving", 1_048_576),
        // 智谱官方标称 GLM-4.7 为 200K；沿用设置页二进制 K 展示口径。
        ("glm-4.7", 204_800),
    ];
    PINVOU_KNOWN
        .iter()
        .find(|(name, _)| model_name_matches(&lower, name))
        .map(|(_, window)| *window)
}

/// The unified context-window precedence: the host `bridge::route_limits_for_model`
/// (which decides inference and compaction thresholds) and the monitor display
/// (`model_probe`) must both call this function; writing a second match elsewhere
/// is not allowed. Precedence: the window the user explicitly declares in the
/// model form wins and is min-clamped against the probed value (the probe is
/// deployment ground truth); without a declaration the probed value applies, and
/// only when that is absent too does the caller-resolved inferred fallback apply.
/// The second return value flags "the inferred fallback was actually adopted",
/// for the monitor page to label the `context_window_inferred` diagnostic.
///
/// "Whether the probed value is trustworthy / participates in clamping" is the
/// caller's decision: the host only probes locally introspectable vLLM (cloud
/// is always `probed = None`); the monitor's gate lives in `model_probe`.
#[must_use]
pub fn resolve_context_window(
    configured: Option<u32>,
    probed: Option<u32>,
    inferred: Option<u32>,
) -> (Option<u32>, bool) {
    match (configured, probed) {
        (Some(configured), Some(probed)) => (Some(configured.min(probed)), false),
        (Some(configured), None) => (Some(configured), false),
        (None, probed) => (probed.or(inferred), probed.is_none() && inferred.is_some()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supplemental_cloud_models_resolve_to_verified_windows() {
        for (model, expected) in [
            ("kimi-k3", 1_048_576),
            ("k3", 262_144),
            ("qwen3.7-plus", 1_000_000),
            ("qwen3.7-max", 1_000_000),
            ("qwen3.7-flash", 1_000_000),
            ("qwen3.6-flash", 1_000_000),
            ("qwen3.8-max", 1_000_000),
            ("qwen3.8-max-preview", 1_000_000),
            ("doubao-seed-evolving", 1_048_576),
            ("glm-4.7", 204_800),
        ] {
            assert_eq!(resolved_context_window(model), Some(expected), "{model}");
        }
    }

    #[test]
    fn pinvou_overrides_take_precedence_over_codewhale_catalog() {
        // 底座 claude 通配兜底 200K 落后于官方口径（opus-5 为 1M），覆盖须先生效。
        assert_eq!(resolved_context_window("claude-opus-5"), Some(1_000_000));
        // 底座已收录且口径正确的模型不受影响。
        assert_eq!(resolved_context_window("claude-haiku-4-5"), Some(200_000));
        assert_eq!(resolved_context_window("claude-sonnet-5"), Some(1_000_000));
    }

    #[test]
    fn explicit_window_wins_and_codewhale_remains_the_base_catalog() {
        assert_eq!(resolved_context_window("kimi-k3-256k"), Some(256_000));
        assert_eq!(resolved_context_window("kimi-k3-1m"), Some(1_048_576));
        assert_eq!(
            resolved_context_window("gpt-5.6-sol"),
            deepseek_tui::models::context_window_for_model("gpt-5.6-sol")
        );
        assert_eq!(resolved_context_window("unknown-cloud-model"), None);
    }

    /// Window precedence table (single source of truth): declaration+probe →
    /// min, declaration beats an absent probe, probe beats inference without a
    /// declaration, and the flag is set only when inference is truly adopted.
    #[test]
    fn context_window_precedence_is_single_sourced() {
        // Declaration + probe → min (ground-truth clamping for local deployments;
        // under-declaring likewise keeps the declaration).
        assert_eq!(
            resolve_context_window(Some(1_048_576), Some(131_072), None),
            (Some(131_072), false)
        );
        assert_eq!(
            resolve_context_window(Some(32_768), Some(131_072), None),
            (Some(32_768), false)
        );
        // Declaration + no probe → the declaration applies as-is (the host never
        // probes cloud; a failed local probe lands here too).
        assert_eq!(
            resolve_context_window(Some(1_048_576), None, Some(131_072)),
            (Some(1_048_576), false)
        );
        // No declaration → probe first; only when the probe is absent does
        // inference apply, and the adopted flag is set only in that case.
        assert_eq!(
            resolve_context_window(None, Some(262_144), Some(131_072)),
            (Some(262_144), false)
        );
        assert_eq!(
            resolve_context_window(None, None, Some(1_000_000)),
            (Some(1_000_000), true)
        );
        assert_eq!(resolve_context_window(None, None, None), (None, false));
    }
}
