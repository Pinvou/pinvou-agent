
/// pinvou3 工具开关(全局持久):设置当前被关掉的连接器(connector_ids = 市场工具 id)。
/// 落盘 → 推算成模型可见工具全名广播给所有在跑引擎 → 隐藏这些工具。空 = 全开。
/// 持久:用户关一次,所有新对话/新窗口都继承,直到手动开回。
#[tauri::command]
pub async fn set_disabled_connectors(
    connector_ids: Vec<String>,
    app: AppHandle,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    crate::features::marketplace::apply_disabled_connectors(connector_ids).await?;
    pool.refresh_disallowed_tools().await;
    let _ = app.emit("remote_control:tools_changed", ());
    if let Some(manager) = app.try_state::<crate::remote_control::RemoteControlManager>() {
        if let Some(session_id) = manager.current_session_id() {
            manager.broadcast_to_mobile(&session_id, "tools_changed", serde_json::json!({}));
        }
    }
    Ok(())
}

/// pinvou3 工具开关:读全局被禁用的连接器 id 列表(前端启动时加载,初始化开关状态)。
#[tauri::command]
pub async fn get_disabled_connectors() -> Result<Vec<String>, String> {
    Ok(crate::features::marketplace::load_disabled_connectors())
}
use super::prelude::*;
