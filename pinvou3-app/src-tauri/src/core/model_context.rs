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

/// operator-owned 端点（本地 vLLM、自定义 OpenAI 兼容 / custom；判定见
/// `SavedModel::is_operator_owned_endpoint`）的输出上限分档声明——宿主作为
/// 部署者的代理，按窗口分档代为声明 route 输出事实（替换底座对未编目模型
/// 的 8192 fail-close 猜测）。
///
/// 单一事实源：`bridge::route_limits_for_model` 的声明臂与监控页 live 探测
/// 测试都从这里取值，禁止再内联同公式（曾经的内联副本在引入当轮就漏掉了
/// 500K 档发生漂移）。
#[must_use]
pub fn operator_owned_output_declaration(window: Option<u32>) -> Option<u32> {
    let declared = match window {
        Some(window) if window >= 500_000 => 131_072,
        Some(window) if window >= 250_000 => 65_536,
        Some(window) => (window / 4).min(32_768),
        // 无窗口事实：按底座 128K 默认窗口的 1/4 兜底（非底座自身数值：
        // 底座模型级兜底 64000、路由级 fail-close ≤8192；min(64000, 32768)
        // 后恰好生效 32768）。
        None => 32_768,
    };
    // 窗口过小时 window/4 装不下一个有意义的输出预算（<4K）：保持不声明
    // （fail-closed），不发 Some(<4K) 的 route 事实。
    (declared >= 4_096).then_some(declared)
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

    #[test]
    fn operator_owned_output_declaration_tiers_and_fail_closed_floor() {
        // 分档边界（与 bridge 分档测试同表，此处钉纯函数本身）。
        assert_eq!(
            operator_owned_output_declaration(Some(1_048_576)),
            Some(131_072)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(500_000)),
            Some(131_072)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(499_999)),
            Some(65_536)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(250_000)),
            Some(65_536)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(249_999)),
            Some(32_768)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(262_144)),
            Some(65_536)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(131_072)),
            Some(32_768)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(65_536)),
            Some(16_384)
        );
        assert_eq!(operator_owned_output_declaration(Some(16_384)), Some(4_096));
        assert_eq!(operator_owned_output_declaration(Some(16_383)), None);
        assert_eq!(operator_owned_output_declaration(Some(4_096)), None);
        // 无窗口事实 → 128K 默认窗口的 1/4 兜底。
        assert_eq!(operator_owned_output_declaration(None), Some(32_768));
    }
}
