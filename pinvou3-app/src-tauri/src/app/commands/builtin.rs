// ---------------------------------------------------------------------------
// 内置插件功能开关（内置工具集长期契约 §3.3）
// ---------------------------------------------------------------------------

use super::prelude::*;

/// 功能注册表：内置插件声明的可开关功能（feature id、来源插件、关联工具、
/// enabled 状态）。本期只建机制：开关 UI 随各功能落地时接入其设置页（契约 §3.3：
/// 开关以功能为单位出现在功能自己的设置项，不出现在插件中心板块）。
#[tauri::command]
pub fn list_builtin_features()
-> Result<Vec<crate::features::marketplace::builtin::BuiltinFeature>, String> {
    Ok(crate::features::marketplace::builtin::feature_registry())
}

/// 开关一个内置功能：写 UserPrefs.disabled_builtin_features + 状态文件
/// `~/.pinvou3/marketplace/builtin_features.json`（MCP server 读取面），随后与
/// 连接器开关同等收尾——热刷 disallowed 工具白名单（被关功能按并集语义摘除的
/// 工具即时对模型不可见）并广播，返回落盘后的最新注册表。
#[tauri::command]
pub async fn set_builtin_feature_enabled(
    feature_id: String,
    enabled: bool,
    app: AppHandle,
    pool: State<'_, EnginePool>,
) -> Result<Vec<crate::features::marketplace::builtin::BuiltinFeature>, String> {
    let registry =
        crate::features::marketplace::builtin::set_feature_enabled(&feature_id, enabled)?;
    super::connectors::refresh_tools_and_broadcast(&app, pool.inner()).await;
    Ok(registry)
}
