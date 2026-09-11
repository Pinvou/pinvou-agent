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
    // 底座 catalog 与 known 表均已按 1M 收录 claude-opus-5（此注释曾误称底座未收录）。
    // 条目保留为显式锚点：底座口径未来回退时仍以官方 1M 为准
    // （Claude 5 系除 haiku 外均为 1M，platform.claude.com models overview）。
    ("claude-opus-5", 1_000_000),
    // 底座上下文启发式对 deepseek 系只有含 "v4" 子串的名字给 1M，deepseek-flash
    // 无 "v4" 会落 128K legacy 启发式（实测返回 Some(128_000) 且先行短路
    // PINVOU_KNOWN）；V4.1-Flash 官方口径 1M 上下文
    // （api-docs.deepseek.com/quick_start/pricing，2026-09-11 核对）。
    // 故必须放本覆盖表而非 PINVOU_KNOWN。
    ("deepseek-flash", 1_000_000),
    // 底座 known 表仍把 grok-4.20-0309-* 记为 2M（crates/tui/src/models.rs 旧行），
    // docs.x.ai 模型详情页 2026-09-11 复核为 1M。known 表命中发生在 prefs
    // context_window_fallback 之前，故修底座数值必须放本覆盖表（目录 desc 已按 1M 标注）。
    ("grok-4.20-0309-reasoning", 1_000_000),
    ("grok-4.20-0309-non-reasoning", 1_000_000),
    // 底座 known 表只精确收录 claude-fable-5，claude-fable-5-1 未命中会落
    // 「未知名 claude 通配 200K」兜底；官方口径 fable-5-1 为 1M
    // （platform.claude.com models overview，2026-09-11）。
    // model_name_matches 同时容忍未来 -日期/快照后缀。
    ("claude-fable-5-1", 1_000_000),
    // 底座链（bundled catalog/known 表/启发式/claude 通配）均无 gpt-6 系行，
    // resolved 返回 None：engine 侧落 128K，而监控页还有 Openai 预设兜底
    // 1.05M，两侧分叉。目录行官方口径 1,050,000
    // （developers.openai.com/api/docs/models，2026-09-11）。
    // 注意：该模型工具调用仅限 Responses 协议，故不是 openai 预设默认
    // （见 prefs::model），但目录行仍可选，engine 侧必须与监控页同源。
    ("gpt-6-astra", 1_050_000),
    // Gemini 预设默认模型；底座 known 表 gemini 行止于 3.7，3.8 无行时
    // engine 落 128K 与监控页 Gemini 兜底 1M 分叉。官方输入上限 1,048,576
    // （ai.google.dev gemini-3.8-flash 页，2026-09-11）。
    ("gemini-3.8-flash", 1_048_576),
    // 目录 desc 标 256K；底座 known 表只有裸 "grok-build"→512K（精确全等
    // 命不中 -0.1 wire id），engine 解析 None 落 128K。官方 256K
    // （docs.x.ai grok-build-0.1 页，2026-09-11）。
    ("grok-build-0.1", 256_000),
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
        // 底座已按 1M 收录 opus-5，覆盖条目作为显式锚点先行生效并锁定口径。
        assert_eq!(resolved_context_window("claude-opus-5"), Some(1_000_000));
        // 底座把无 "v4" 的 deepseek 名按 legacy 128K 兜底；V4.1-Flash 官方 1M
        // 必须由覆盖表先行修正（见 PINVOU_OVERRIDES 注释）。
        assert_eq!(resolved_context_window("deepseek-flash"), Some(1_000_000));
        // 底座 known 表仍记 grok-4.20-0309-* 为 2M（落后于 docs.x.ai 2026-09 口径），
        // 覆盖表须先行修正；fable-5-1 底座未精确收录，否则落 claude 通配 200K。
        assert_eq!(
            resolved_context_window("grok-4.20-0309-reasoning"),
            Some(1_000_000)
        );
        assert_eq!(
            resolved_context_window("grok-4.20-0309-non-reasoning"),
            Some(1_000_000)
        );
        assert_eq!(resolved_context_window("claude-fable-5-1"), Some(1_000_000));
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
