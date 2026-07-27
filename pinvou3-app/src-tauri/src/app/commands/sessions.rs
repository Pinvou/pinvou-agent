#[derive(Debug, Clone, Serialize)]
pub struct SessionListItem {
    #[serde(flatten)]
    pub metadata: SessionMetadata,
    pub pinned: bool,
    pub pinned_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HiddenSessionListItem {
    #[serde(flatten)]
    pub metadata: SessionMetadata,
    pub hidden_at: Option<String>,
    #[serde(rename = "archived_at")]
    pub archived_at: Option<String>,
}

/// 仅普通 chat 会话可用的命令守卫（transcript/产物由前端覆盖持久化的路径）。
/// 重命名/置顶/归档/删除等元数据操作按 SessionKind 分发，不走这个守卫。
pub(super) fn ensure_chat_session(
    store: &SessionStore,
    id: &str,
    action: &str,
) -> Result<(), String> {
    match store
        .session_kind(id)
        .map_err(|error| format!("{action}({id}): {error:?}"))?
    {
        SessionKind::Chat => Ok(()),
        SessionKind::ScheduledRun => Err(format!(
            "{action}({id}): scheduled-run sessions are managed from Scheduled"
        )),
    }
}

pub(super) fn emit_session_event(app: &AppHandle, event: &str, id: &str, action: &str) {
    let payload = serde_json::json!({
        "id": id,
        "action": action,
    });
    let _ = app.emit(event, payload.clone());
    crate::features::remote_control::forward_app_event(app, event, payload);
}

/// 清当前会话历史。
///
/// **当前 MVP 限制**：仅返回 Ok 让前端清显示；后端 EngineHandle 仍持
/// 累积的消息历史，下次 chat 时 LLM 仍能看到之前的对话。真清需要重启
/// app（spawn 全新 Engine）。
///
/// 实装路径（Phase C）：发 `Op::Shutdown` 给 engine + 在 Tauri State 上
/// 替换 AppEngine 为新 spawn 出来的实例。
#[tauri::command]
pub async fn clear_session() -> Result<(), String> {
    eprintln!("[pinvou3-app] clear_session: frontend cleared, backend session unchanged (MVP)");
    Ok(())
}
// ===================== 阶段 C: 多对话历史 =====================

/// 列出所有 session 元数据，按 updated_at 倒序。前端历史面板渲染用。
/// 返回 SessionMetadata 数组（id/title/时间/token/model/workspace 等字段）。
/// [2026-06-04 白浪:chat 与工作流彻底分开] 过滤工作流宿主 session(绑定带 project_dir
/// 即是,bindings 开机回灌持久化)——它们仅作 SubAgent 运行时,不进 chat 侧栏。
#[tauri::command]
pub async fn list_sessions(
    store: State<'_, SessionStore>,
    acp_pool: State<'_, crate::features::codex_acp::AcpPool>,
) -> Result<Vec<SessionListItem>, String> {
    let mut metas = store.list().map_err(|e| format!("list_sessions: {e:?}"))?;
    metas.retain(|m| {
        matches!(store.session_kind(&m.id), Ok(SessionKind::Chat))
            && !acp_pool.is_codex(&m.id)
            && !store.is_hidden(&m.id)
            && store
                .active_skill(&m.id)
                .is_none_or(|b| b.project_dir.is_none())
    });
    Ok(metas
        .into_iter()
        .map(|metadata| SessionListItem {
            pinned: store.is_pinned(&metadata.id),
            pinned_at: store.pinned_at(&metadata.id),
            metadata,
        })
        .collect())
}

/// 列出已从左侧任务列表收起的 session（含收起的定时运行会话）。前端设置页渲染用。
#[tauri::command]
pub async fn list_archived_sessions(
    store: State<'_, SessionStore>,
) -> Result<Vec<HiddenSessionListItem>, String> {
    let mut metas = store
        .list()
        .map_err(|e| format!("list_archived_sessions: {e:?}"))?;
    metas.extend(
        store
            .list_scheduled()
            .map_err(|e| format!("list_archived_sessions: {e:?}"))?,
    );
    metas.retain(|m| {
        store.is_hidden(&m.id)
            && store
                .active_skill(&m.id)
                .is_none_or(|b| b.project_dir.is_none())
    });
    metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(metas
        .into_iter()
        .map(|metadata| {
            let hidden_at = store.hidden_at(&metadata.id);
            HiddenSessionListItem {
                archived_at: hidden_at.clone(),
                hidden_at,
                metadata,
            }
        })
        .collect())
}

/// 新建空 session 并设为 active。返回创建的 SessionMetadata。
/// 引擎层的 session 状态切换由 chat() 下次发消息时自然处理（暂不发 SyncSession）。
#[tauri::command]
pub async fn create_session(
    set_active: Option<bool>,
    app: AppHandle,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<SessionMetadata, String> {
    let (model, model_id) = pool.default_model_for_new_session();
    let workspace = pool.bridge.workspace.clone();
    let session = store
        .create_new(model, model_id, workspace)
        .map_err(|e| format!("create_session: {e:?}"))?;
    if set_active.unwrap_or(true) {
        store.set_active(Some(session.metadata.id.clone()));
    }
    emit_session_event(
        &app,
        "session:list_changed",
        &session.metadata.id,
        "created",
    );
    // 多 session 并发:不预热 engine(lazy)。新建的空 session 没有历史,首条 chat
    // 时 EnginePool.get_or_spawn 会为它 spawn 一个带专属 workspace 的 engine。
    Ok(session.metadata)
}

/// 加载指定 session 的完整对话（含 messages）。
/// 前端切换历史时调用 → 用返回的 messages 重渲染对话区。
#[tauri::command]
pub async fn load_session(
    id: String,
    set_active: Option<bool>,
    store: State<'_, SessionStore>,
) -> Result<SavedSession, String> {
    let session = store
        .load(&id)
        .map_err(|e| format!("load_session({id}): {e:?}"))?;
    if set_active.unwrap_or(true) {
        store.set_active(Some(id.clone()));
    }
    // 多 session 并发:切换不再 SyncSession 替换全局引擎(那是旧单引擎模型)。该 session
    // 有自己独立的 engine(已起则持有自己的上下文、还在跑就继续跑;未起则下次 chat 时
    // lazy spawn 并注水这里返回的 messages)。本命令只切 active 指针 + 返回 messages 给前端渲染。
    Ok(session)
}

/// 删除 session（含 artifacts 目录）。按 SessionKind 分发：定时运行会话联动
/// 删除该次 Session、Run 与底座 Task（任务定义、共享工作间和其他运行保留）。
#[tauri::command]
pub async fn delete_session(
    id: String,
    app: AppHandle,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    acp_pool: State<'_, crate::features::codex_acp::AcpPool>,
) -> Result<(), String> {
    let result = match store
        .session_kind(&id)
        .map_err(|e| format!("delete_session({id}): {e:?}"))?
    {
        SessionKind::Chat => {
            acp_pool.evict(&id).await;
            // 先回收该 session 的 engine(cancel 在跑的 turn + shutdown + abort forwarder),
            // 再删盘上数据,避免僵尸 engine 继续往已删 session 写产物。
            pool.evict(&id).await;
            let result = store
                .delete(&id)
                .map_err(|e| format!("delete_session({id}): {e:?}"));
            if result.is_ok() {
                pool.forget_session(&id);
                acp_pool
                    .agents()
                    .remove(&id)
                    .map_err(|error| format!("清理 Agent 会话映射失败: {error:#}"))?;
            }
            result
        }
        SessionKind::ScheduledRun => {
            let scheduled = app
                .try_state::<crate::features::scheduled::tasks::ScheduledTaskState>()
                .ok_or_else(|| "Scheduled task runtime is unavailable".to_string())?;
            scheduled.delete_run_for_session(&id).await
        }
    };
    if result.is_ok() {
        pool.forget_session(&id);
        let payload = serde_json::json!({ "id": &id });
        let _ = app.emit("session:deleted", payload.clone());
        crate::features::remote_control::forward_app_event(&app, "session:deleted", payload);
    }
    result
}

/// 重命名 session 标题。普通会话与定时运行会话共用 Session 元数据。
#[tauri::command]
pub async fn rename_session(
    id: String,
    title: String,
    app: AppHandle,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    store
        .set_title(&id, title)
        .map_err(|e| format!("rename_session({id}): {e:?}"))?;
    emit_session_event(&app, "session:list_changed", &id, "renamed");
    Ok(())
}

/// 设置历史对话置顶状态。普通会话与定时运行会话共用置顶表。
#[tauri::command]
pub async fn set_session_pinned(
    id: String,
    pinned: bool,
    app: AppHandle,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    // 先 load 一次确认 session 存在,避免置顶表残留无效 id。
    store
        .load(&id)
        .map_err(|e| format!("set_session_pinned({id}): {e:?}"))?;
    store.set_pinned(&id, pinned);
    let action = if pinned { "pinned" } else { "unpinned" };
    emit_session_event(&app, "session:list_changed", &id, action);
    Ok(())
}

/// 设置 session 是否从左侧任务列表收起。普通会话与定时运行会话共用收起表。
#[tauri::command]
pub async fn set_session_archived(
    id: String,
    archived: bool,
    app: AppHandle,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    // 先 load 一次确认 session 存在,避免收起表残留无效 id。
    store
        .load(&id)
        .map_err(|e| format!("set_session_archived({id}): {e:?}"))?;
    store.set_hidden(&id, archived);
    let action = if archived { "archived" } else { "restored" };
    emit_session_event(&app, "session:list_changed", &id, action);
    Ok(())
}

/// 取当前 active session id（前端启动时高亮历史面板用）。
#[tauri::command]
pub async fn get_active_session(store: State<'_, SessionStore>) -> Result<Option<String>, String> {
    Ok(store.active_id())
}

/// 落盘普通 chat session 的 messages 数组。前端是普通 chat 的 source of truth；
/// scheduled-run transcript 由 Engine `SessionUpdated` 独占持久化，拒绝 UI 覆盖。
#[tauri::command]
pub async fn save_session_messages(
    id: String,
    messages: Vec<Message>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    ensure_chat_session(&store, &id, "save_session_messages")?;
    store
        .update_messages(&id, messages)
        .map_err(|e| format!("save_session_messages({id}): {e:?}"))
}

/// 落盘 session 的产物 paths 列表。前端跟踪 write_file / append_file 调用后调用,
/// 跟 save_session_messages 一起落 (TurnComplete 时)。重启/切换 session 后,
/// 从 SavedSession.artifacts 重建前端产物列表。
#[tauri::command]
pub async fn save_session_artifacts(
    id: String,
    paths: Vec<String>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    ensure_chat_session(&store, &id, "save_session_artifacts")?;
    store
        .update_artifacts(&id, paths)
        .map_err(|e| format!("save_session_artifacts({id}): {e:?}"))
}

/// 扫描 session workspace 目录,返回实际存在的产物文件绝对路径(过滤隐藏/临时文件)。
/// 前端切换 session 时用它对账 —— 让产物面板以**磁盘真相**为准,不受跟踪遗漏 /
/// app 中途重启(内存跟踪丢失)影响。过滤规则与 file_watcher::should_skip 对齐。
#[tauri::command]
pub async fn list_workspace_files(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Vec<String>, String> {
    list_workspace_files_for_session(&session_id, &store)
}

pub(super) fn list_workspace_files_for_session(
    session_id: &str,
    store: &SessionStore,
) -> Result<Vec<String>, String> {
    let execution_workspace = store
        .execution_workspace(session_id)
        .map_err(|error| format!("resolve execution workspace for {session_id}: {error:#}"))?;
    let mut out = Vec::new();
    for dir in [
        execution_workspace,
        crate::platform::paths::session_artifacts_dir(session_id),
    ] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if name.is_empty()
                    || name.starts_with('.')
                    || name.starts_with("~$")
                    || name.ends_with('~')
                    || name.ends_with(".swp")
                    || name.ends_with(".swo")
                    || name.ends_with(".tmp")
                    || name.ends_with(".bak")
                {
                    continue;
                }
                out.push(path.to_string_lossy().to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}
use super::prelude::*;
