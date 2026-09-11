//! pinvou3 运行状态与 Engine 路由共用的模型上下文窗口解析。
//!
//! 已由 CodeWhale 维护的模型事实优先复用底座；这里只补充 pinvou3 设置页已经提供、
//! 但当前底座尚未覆盖的云端模型。窗口事实（按名解析）与窗口优先级（声明 vs
//! 探测 vs 推断）都以这里为唯一入口，避免页面显示窗口与 `active_route_limits` /
//! 压缩阈值 / 监控分母使用不同口径。

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

/// 上下文窗口的统一优先级：宿主 `bridge::route_limits_for_model`（决定推理与
/// 压缩阈值）与监控展示（`model_probe`）必须共用本函数，禁止各自另写一份
/// match。优先级为：用户在模型表单显式声明的窗口优先，且与实测探测值取小
/// （探测值是部署实地事实）；声明缺席时用探测值，再缺席才用调用方解析好的
/// 推断兜底。第二个返回值标记「推断兜底被真正采用」，供监控页标注
/// `context_window_inferred` 诊断。
///
/// 「探测值是否可信/是否参与收紧」由调用方决定：宿主只对可实地内省的本地
/// vLLM 探测（云端恒为 `probed = None`），监控侧见 `model_probe` 的门控。
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

    /// 窗口优先级表（单一事实源）：声明+探测取小、声明优先于缺席探测、
    /// 无声明时探测优先于推断、仅推断被真正采用时置标记。
    #[test]
    fn context_window_precedence_is_single_sourced() {
        // 声明 + 探测 → 取小（本地部署实地收紧；under-declare 同理取声明）。
        assert_eq!(
            resolve_context_window(Some(1_048_576), Some(131_072), None),
            (Some(131_072), false)
        );
        assert_eq!(
            resolve_context_window(Some(32_768), Some(131_072), None),
            (Some(32_768), false)
        );
        // 声明 + 无探测 → 声明原样生效（云端从不被宿主探测；本地探测失败同此）。
        assert_eq!(
            resolve_context_window(Some(1_048_576), None, Some(131_072)),
            (Some(1_048_576), false)
        );
        // 无声明 → 探测优先；探测缺席才用推断，且仅此时标记推断被采用。
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
