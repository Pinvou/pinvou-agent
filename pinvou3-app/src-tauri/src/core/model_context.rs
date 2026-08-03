//! pinvou3 运行状态与 Engine 路由共用的模型上下文窗口解析。
//!
//! 已由 CodeWhale 维护的模型事实优先复用底座；这里只补充 pinvou3 设置页已经提供、
//! 但当前底座尚未覆盖的云端模型。所有消费者必须走这一入口，避免页面显示窗口与
//! `active_route_limits` / 压缩阈值使用不同口径。

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
        // Kimi K3 官方标称 100 万 token（platform.kimi.com/docs/models）。
        ("kimi-k3", 1_048_576),
        // Coding Plan 裸 k3 同为 1M；低档服务端限 256K 时由显式后缀优先覆盖。
        ("k3", 1_048_576),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supplemental_cloud_models_resolve_to_verified_windows() {
        for (model, expected) in [
            ("kimi-k3", 1_048_576),
            ("k3", 1_048_576),
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
}
