
/// 计算当前应「对模型隐藏」的工具全名**完整列表**(小写)。
///
/// 因 `EnginePool::set_disallowed_all` 是**全量替换** `config.disallowed_tools`,任何调用方
/// 都必须传完整列表,不能传增量。组成 = 市场连接器开关禁用的工具名 +(知识库不可用时)`kb_search`。
/// 知识库"可用" = 有已入库内容 **且** embedding 模型已就绪(semantic_ready)。embedding 模型按需
/// 下载,没装时知识库走完全门控 → kb_search 进列表 → 模型目录里看不到 → AI 不再宣称能本地检索;
/// 库删光文件后同理。KnowledgeService state 取不到时保守隐藏(宁可少功能不误宣传)。
pub fn compute_disallowed_tools(app: &AppHandle) -> Vec<String> {
    let mut tools = crate::bridge::marketplace::disabled_tool_names();
    let kb_usable = app
        .try_state::<KnowledgeService>()
        .map(|s| s.has_indexed_content() && s.semantic_ready())
        .unwrap_or(false);
    if !kb_usable {
        tools.push("kb_search".to_string());
    }
    tools
}

pub async fn apply_disabled_connectors(
    app: Option<&AppHandle>,
    pool: &EnginePool,
    connector_ids: Vec<String>,
) -> Result<(), String> {
    let app_clone = app.cloned();
    let disallowed = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
        crate::bridge::marketplace::save_disabled_connectors(&connector_ids);
        crate::bridge::skill_marketplace::refresh_disabled_skills();
        Ok(match &app_clone {
            Some(app) => compute_disallowed_tools(app),
            None => crate::bridge::marketplace::disabled_tool_names(),
        })
    })
    .await
    .map_err(|error| format!("apply_disabled_connectors join: {error}"))??;
    pool.set_disallowed_all(disallowed).await;
    Ok(())
}

/// pinvou3 工具开关(全局持久):设置当前被关掉的连接器(connector_ids = 市场工具 id)。
/// 落盘 → 推算成模型可见工具全名广播给所有在跑引擎 → 隐藏这些工具。空 = 全开。
/// 持久:用户关一次,所有新对话/新窗口都继承,直到手动开回。
#[tauri::command]
pub async fn set_disabled_connectors(
    connector_ids: Vec<String>,
    app: AppHandle,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    apply_disabled_connectors(Some(&app), &pool, connector_ids).await?;
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
    Ok(crate::bridge::marketplace::load_disabled_connectors())
}
