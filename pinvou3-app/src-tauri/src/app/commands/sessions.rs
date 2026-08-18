#[derive(Debug, Clone, Serialize)]
pub struct SessionListItem {
    #[serde(flatten)]
    pub metadata: SessionMetadata,
    pub pinned: bool,
    pub pinned_at: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub title_attachment_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HiddenSessionListItem {
    #[serde(flatten)]
    pub metadata: SessionMetadata,
    pub hidden_at: Option<String>,
    #[serde(rename = "archived_at")]
    pub archived_at: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub title_attachment_names: Vec<String>,
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
        .map_err(|error| format!("{action}({id}): {error:#}"))?
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

fn title_contains_attachment_marker(title: &str) -> bool {
    title.starts_with("📎 ") || title.contains("\n\n📎 ")
}

fn attachment_names_from_display_message(text: &str) -> Vec<String> {
    let payload = if let Some(names) = text.strip_prefix("📎 ") {
        (!names.contains('\n')).then_some(names)
    } else {
        text.rsplit_once("\n\n📎 ")
            .and_then(|(_, names)| (!names.contains('\n')).then_some(names))
    };
    let Some(payload) = payload else {
        return Vec::new();
    };
    if payload.trim_start().starts_with('[') {
        if let Ok(names) = serde_json::from_str::<Vec<String>>(payload) {
            return names.into_iter().filter(|name| !name.is_empty()).collect();
        }
    }
    // Compatibility for transcripts written before the JSON attachment marker.
    payload
        .split(" · ")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

fn session_title_attachment_names(store: &SessionStore, metadata: &SessionMetadata) -> Vec<String> {
    if !title_contains_attachment_marker(&metadata.title) {
        return Vec::new();
    }
    let allow_legacy_unscoped = matches!(
        store.session_kind(&metadata.id),
        Ok(crate::features::sessions::SessionKind::Chat)
    );
    let indexed_names = store
        .ledger_root(&metadata.id)
        .ok()
        .and_then(|workspace| {
            crate::features::files::attachment_upload::conversation_attachment_names_for_display_prefix(
                &workspace,
                &metadata.id,
                &metadata.title,
                allow_legacy_unscoped,
            ).ok()
        })
        .unwrap_or_default();
    if !indexed_names.is_empty() {
        return indexed_names;
    }
    let Ok(session) = store.load(&metadata.id) else {
        return Vec::new();
    };
    session
        .messages
        .iter()
        .find(|message| message.role == "user")
        .and_then(|message| {
            message.content.iter().find_map(|block| match block {
                deepseek_tui::models::ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
        })
        .map(attachment_names_from_display_message)
        .unwrap_or_default()
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
/// 代码会话(ACP 与品悟原生)同样不进 chat 侧栏,由 list_codex_acp_sessions 单独提供。
#[tauri::command]
pub async fn list_sessions(
    store: State<'_, SessionStore>,
    acp_pool: State<'_, crate::features::codex_acp::AcpPool>,
) -> Result<Vec<SessionListItem>, String> {
    let mut metas = store.list().map_err(|e| format!("list_sessions: {e:#}"))?;
    metas.retain(|m| {
        matches!(store.session_kind(&m.id), Ok(SessionKind::Chat))
            && !acp_pool.is_acp_metadata(m)
            // 原生代码会话（code_session 绑定）归属代码列表，不进 chat 侧栏
            && !acp_pool.agents().is_code_session(&m.id)
            && !store.is_hidden(&m.id)
    });
    Ok(metas
        .into_iter()
        .map(|metadata| {
            let title_attachment_names = session_title_attachment_names(&store, &metadata);
            SessionListItem {
                pinned: store.is_pinned(&metadata.id),
                pinned_at: store.pinned_at(&metadata.id),
                title_attachment_names,
                metadata,
            }
        })
        .collect())
}

/// 标题仍为默认值「新对话」时，用首条消息（或附件名兜底）派生会话标题（前 28 字符）。
///
/// ACP（codex_acp_prompt）与原生（chat）两条发送链路统一经此自动命名；
/// 用户已重命名过的会话不会被覆盖。`title_source` 为空时不动作。
pub(crate) fn apply_default_session_title(
    store: &SessionStore,
    session_id: &str,
    title_source: &str,
) -> Result<(), String> {
    let title_source = title_source.trim();
    if title_source.is_empty() {
        return Ok(());
    }
    let session = store
        .load(session_id)
        .map_err(|error| format!("读取会话 {session_id} 失败: {error:#}"))?;
    if session.metadata.title != "新对话" {
        return Ok(());
    }
    let title = title_source.chars().take(28).collect::<String>();
    store
        .set_title(session_id, title)
        .map_err(|error| format!("更新会话标题失败: {error:#}"))
}

#[cfg(test)]
mod session_title_attachment_tests {
    use super::{attachment_names_from_display_message, title_contains_attachment_marker};

    #[test]
    fn parses_attachment_names_from_supported_display_messages() {
        assert_eq!(
            attachment_names_from_display_message("看一下\n\n📎 [\"决策基线.md\",\"数据.xlsx\"]"),
            vec!["决策基线.md", "数据.xlsx"]
        );
        assert_eq!(
            attachment_names_from_display_message("📎 [\"报告.pdf\"]"),
            vec!["报告.pdf"]
        );
        assert_eq!(
            attachment_names_from_display_message("📎 [\"预算 · 最终.xlsx\"]"),
            vec!["预算 · 最终.xlsx"]
        );
        assert_eq!(
            attachment_names_from_display_message("📎 旧报告.pdf · 旧数据.xlsx"),
            vec!["旧报告.pdf", "旧数据.xlsx"]
        );
        assert_eq!(
            attachment_names_from_display_message("📎 [草稿].md"),
            vec!["[草稿].md"]
        );
    }

    #[test]
    fn ignores_inline_or_non_terminal_paperclip_text() {
        assert!(attachment_names_from_display_message("正文提到 📎 符号").is_empty());
        assert!(attachment_names_from_display_message("正文\n\n📎 文件.md\n尾部").is_empty());
        assert!(!title_contains_attachment_marker("正文提到 📎 符号"));
        assert!(title_contains_attachment_marker("正文\n\n📎 文件"));
    }
}

#[cfg(test)]
mod default_session_title_tests {
    use super::apply_default_session_title;

    #[test]
    fn default_title_is_derived_once_and_truncated() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root =
            std::env::temp_dir().join(format!("pinvou3-default-title-test-{}", std::process::id()));
        let previous = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("PINVOU3_HOME", &root);
        let store = crate::features::sessions::SessionStore::boot_with_scheduled_root(
            root.join("scheduled"),
        )
        .expect("session store");

        let session = store
            .create_new("model".to_string(), None, root.clone())
            .expect("create session");
        let id = session.metadata.id.clone();
        assert_eq!(session.metadata.title, "新对话");

        // 默认标题：用首条消息命名（去首尾空白）
        apply_default_session_title(&store, &id, "  帮我review这段代码  ").expect("auto title");
        assert_eq!(
            store.load(&id).expect("reload").metadata.title,
            "帮我review这段代码"
        );

        // 已命名后不再被后续消息覆盖
        apply_default_session_title(&store, &id, "第二条消息不应覆盖").expect("second call");
        assert_eq!(
            store.load(&id).expect("reload").metadata.title,
            "帮我review这段代码"
        );

        // 空来源不动作；超长来源截断到 28 字符
        let session2 = store
            .create_new("model".to_string(), None, root.clone())
            .expect("create session 2");
        let id2 = session2.metadata.id.clone();
        apply_default_session_title(&store, &id2, "   ").expect("empty source is a no-op");
        assert_eq!(store.load(&id2).expect("reload").metadata.title, "新对话");
        apply_default_session_title(
            &store,
            &id2,
            "这是一个非常非常长的首条消息用来验证标题派生时会按字符数截断而不是全部保留",
        )
        .expect("auto title 2");
        assert_eq!(
            store
                .load(&id2)
                .expect("reload")
                .metadata
                .title
                .chars()
                .count(),
            28
        );

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// 列出已从左侧任务列表收起的 session（含收起的定时运行会话）。前端设置页渲染用。
#[tauri::command]
pub async fn list_archived_sessions(
    store: State<'_, SessionStore>,
) -> Result<Vec<HiddenSessionListItem>, String> {
    let mut metas = store
        .list()
        .map_err(|e| format!("list_archived_sessions: {e:#}"))?;
    metas.extend(
        store
            .list_scheduled()
            .map_err(|e| format!("list_archived_sessions: {e:#}"))?,
    );
    metas.retain(|m| store.is_hidden(&m.id));
    metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(metas
        .into_iter()
        .map(|metadata| {
            let hidden_at = store.hidden_at(&metadata.id);
            let title_attachment_names = session_title_attachment_names(&store, &metadata);
            HiddenSessionListItem {
                archived_at: hidden_at.clone(),
                hidden_at,
                title_attachment_names,
                metadata,
            }
        })
        .collect())
}

/// 新建空 session 并设为 active。返回创建的 SessionMetadata。
/// 引擎层的 session 状态切换由 chat() 下次发消息时自然处理（暂不发 SyncSession）。
pub(super) fn create_session_record(
    set_active: bool,
    store: &SessionStore,
    pool: &EnginePool,
) -> Result<SessionMetadata, String> {
    let (model, model_id) = pool.default_model_for_new_session();
    let workspace = pool.bridge.workspace.clone();
    let session = store
        .create_new(model, model_id, workspace)
        .map_err(|e| format!("create_session: {e:#}"))?;
    if set_active {
        store.set_active(Some(session.metadata.id.clone()));
    }
    Ok(session.metadata)
}

#[tauri::command]
pub async fn create_session(
    set_active: Option<bool>,
    app: AppHandle,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<SessionMetadata, String> {
    let metadata = create_session_record(set_active.unwrap_or(true), &store, &pool)?;
    emit_session_event(&app, "session:list_changed", &metadata.id, "created");
    // 多 session 并发:不预热 engine(lazy)。新建的空 session 没有历史,首条 chat
    // 时 EnginePool.get_or_spawn 会为它 spawn 一个带专属 workspace 的 engine。
    Ok(metadata)
}

/// Desktop `load_session` response: same shape as `SavedSession` plus the
/// authoritative transcript revision, mirroring the Web download path
/// (`WebSavedSession`). The frontend reconciles remote turns by this
/// committed revision instead of presentation-derived message equality.
#[derive(Debug, Serialize)]
pub struct DesktopSavedSession {
    #[serde(flatten)]
    session: SavedSession,
    transcript_revision: String,
}

/// 加载指定 session 的完整对话（含 messages）。
/// 前端切换历史时调用 → 用返回的 messages 重渲染对话区。
#[tauri::command]
pub async fn load_session(
    id: String,
    set_active: Option<bool>,
    store: State<'_, SessionStore>,
) -> Result<DesktopSavedSession, String> {
    let started = std::time::Instant::now();
    let session = match store.load(&id) {
        Ok(session) => session,
        Err(error) => {
            crate::features::sessions::diagnostics::record_backend(
                "desktop_load_session_failed",
                serde_json::json!({
                    "session_id": id,
                    "set_active": set_active.unwrap_or(true),
                    "elapsed_ms": started.elapsed().as_millis(),
                    "error": format!("{error:#}"),
                }),
            );
            return Err(format!("load_session({id}): {error:#}"));
        }
    };
    if set_active.unwrap_or(true) {
        store.set_active(Some(id.clone()));
    }
    // 多 session 并发:切换不再 SyncSession 替换全局引擎(那是旧单引擎模型)。该 session
    // 有自己独立的 engine(已起则持有自己的上下文、还在跑就继续跑;未起则下次 chat 时
    // lazy spawn 并注水这里返回的 messages)。本命令只切 active 指针 + 返回 messages 给前端渲染。
    let revision = match crate::features::sessions::transcript_revision(&session.messages) {
        Ok(revision) => revision,
        Err(error) => {
            crate::features::sessions::diagnostics::record_backend(
                "desktop_load_session_revision_failed",
                serde_json::json!({
                    "session_id": id,
                    "message_count": session.messages.len(),
                    "elapsed_ms": started.elapsed().as_millis(),
                    "error": format!("{error:#}"),
                }),
            );
            return Err(format!("load_session({id}) revision: {error:#}"));
        }
    };
    crate::features::sessions::diagnostics::record_backend(
        "desktop_load_session_succeeded",
        serde_json::json!({
            "session_id": id,
            "set_active": set_active.unwrap_or(true),
            "transcript_revision": revision,
            "message_count": session.messages.len(),
            "elapsed_ms": started.elapsed().as_millis(),
        }),
    );
    Ok(DesktopSavedSession {
        session,
        transcript_revision: revision,
    })
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
        .map_err(|e| format!("delete_session({id}): {e:#}"))?
    {
        SessionKind::Chat => {
            acp_pool.evict(&id).await;
            let result = pool
                .delete_chat_session(&id)
                .await
                .map_err(|error| format!("delete_session({id}): {error:#}"));
            if result.is_ok() {
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
        .map_err(|e| format!("rename_session({id}): {e:#}"))?;
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
        .map_err(|e| format!("set_session_pinned({id}): {e:#}"))?;
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
        .map_err(|e| format!("set_session_archived({id}): {e:#}"))?;
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
        .map_err(|e| format!("save_session_messages({id}): {e:#}"))
}

/// 落盘 session 的产物 paths 列表。前端跟踪 File.write / File.edit 调用后调用,
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
        .map_err(|e| format!("save_session_artifacts({id}): {e:#}"))
}

fn normalize_pinvou_scene_events(events: serde_json::Value) -> Result<serde_json::Value, String> {
    let entries = events
        .as_array()
        .ok_or_else(|| "pinvou scene events 必须是数组".to_string())?;
    if entries.len() > 10_000 {
        return Err("pinvou scene events 超过 10000 条上限".to_string());
    }
    let mut normalized = std::collections::BTreeMap::<u64, &'static str>::new();
    for entry in entries {
        let pos = entry
            .get("pos")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| "pinvou scene event 缺少有效 pos".to_string())?;
        let scene = match entry.get("scene").and_then(serde_json::Value::as_str) {
            Some("work:document-writing") => "work:document-writing",
            Some("work:personal-workbench") => "work:personal-workbench",
            Some("design:poster") => "design:poster",
            Some("design:data-visualization") => "design:data-visualization",
            _ => return Err("pinvou scene event 包含无效 scene".to_string()),
        };
        normalized.insert(pos, scene);
    }
    Ok(serde_json::Value::Array(
        normalized
            .into_iter()
            .map(|(pos, scene)| serde_json::json!({ "pos": pos, "scene": scene }))
            .collect(),
    ))
}

/// 保存用户消息专业场景标签。sidecar 独立于 messages，但属于 session 持久数据，
/// 因此通过后端共享给桌面端和 WebUI，而不是只留在某个宿主的 localStorage。
#[tauri::command]
pub async fn save_session_pinvou_scene_events(
    session_id: String,
    events: serde_json::Value,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    ensure_chat_session(&store, &session_id, "save_session_pinvou_scene_events")?;
    let normalized = normalize_pinvou_scene_events(events)?;
    let path = crate::platform::paths::session_pinvou_scene_events(&session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 scene sidecar 目录失败: {error}"))?;
    }
    let payload = serde_json::to_vec(&normalized)
        .map_err(|error| format!("序列化 scene sidecar 失败: {error}"))?;
    deepseek_tui::utils::write_atomic(&path, &payload)
        .map_err(|error| format!("写 scene sidecar 失败: {error:#}"))
}

/// 读取用户消息专业场景标签。旧版本或损坏 sidecar 按空数组处理，不影响会话正文。
#[tauri::command]
pub async fn get_session_pinvou_scene_events(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<serde_json::Value, String> {
    ensure_chat_session(&store, &session_id, "get_session_pinvou_scene_events")?;
    let path = crate::platform::paths::session_pinvou_scene_events(&session_id);
    let Ok(payload) = std::fs::read(&path) else {
        return Ok(serde_json::json!([]));
    };
    let Ok(events) = serde_json::from_slice::<serde_json::Value>(&payload) else {
        return Ok(serde_json::json!([]));
    };
    Ok(normalize_pinvou_scene_events(events).unwrap_or_else(|_| serde_json::json!([])))
}

#[cfg(test)]
mod pinvou_scene_event_tests {
    use super::normalize_pinvou_scene_events;

    #[test]
    fn scene_events_are_validated_deduplicated_and_sorted() {
        let normalized = normalize_pinvou_scene_events(serde_json::json!([
            { "pos": 7, "scene": "design:poster" },
            { "pos": 2, "scene": "work:document-writing" },
            { "pos": 7, "scene": "design:data-visualization" }
        ]))
        .expect("valid scene events");
        assert_eq!(
            normalized,
            serde_json::json!([
                { "pos": 2, "scene": "work:document-writing" },
                { "pos": 7, "scene": "design:data-visualization" }
            ])
        );
    }

    #[test]
    fn scene_events_reject_unknown_scenes_and_invalid_positions() {
        assert!(normalize_pinvou_scene_events(serde_json::json!([
            { "pos": 0, "scene": "design:ppt" }
        ]))
        .is_err());
        assert!(normalize_pinvou_scene_events(serde_json::json!([
            { "pos": -1, "scene": "design:poster" }
        ]))
        .is_err());
    }

    #[test]
    fn scene_events_accept_personal_workbench_scene() {
        // 个人工作台场景标签必须被后端接受并持久化，
        // 否则 sidecar 重载后该消息的工作台标签会丢失。
        let normalized = normalize_pinvou_scene_events(serde_json::json!([
            { "pos": 3, "scene": "work:personal-workbench" }
        ]))
        .expect("personal-workbench scene must be accepted");
        assert_eq!(
            normalized,
            serde_json::json!([
                { "pos": 3, "scene": "work:personal-workbench" }
            ])
        );
    }
}

/// 扫描 session workspace 目录,返回实际存在的产物文件绝对路径(过滤隐藏/临时文件)。
/// 前端切换 session 时用它对账 —— 让产物面板以**磁盘真相**为准,不受跟踪遗漏 /
/// app 中途重启(内存跟踪丢失)影响。过滤规则与 file_watcher::should_skip 对齐。
#[tauri::command]
pub async fn list_workspace_files(
    session_id: String,
    store: State<'_, SessionStore>,
    acp_pool: State<'_, crate::features::codex_acp::AcpPool>,
) -> Result<Vec<String>, String> {
    // 语义守卫:代码会话(品悟原生)没有“产物面板”概念——其文件浏览与变更 diff 走
    // 代码模式的基线工作区面板,这里跳过顶层扫描,避免把工作区代码文件误当产物。
    if acp_pool.agents().is_code_session(&session_id) {
        return Ok(Vec::new());
    }
    list_workspace_files_for_session(&session_id, &store)
}

pub(super) fn list_workspace_files_for_session(
    session_id: &str,
    store: &SessionStore,
) -> Result<Vec<String>, String> {
    let ledger_root = store
        .ledger_root(session_id)
        .map_err(|error| format!("resolve ledger root for {session_id}: {error:#}"))?;
    let mut out = Vec::new();
    for dir in [
        ledger_root,
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

#[cfg(test)]
mod desktop_saved_session_contract_tests {
    use super::*;
    use crate::features::sessions::transcript_revision;

    /// Desktop `load_session` must expose `transcript_revision` at the top
    /// level (snake_case, flatten with the rest of `SavedSession`), otherwise
    /// the bridge reconcile falls into the every-turn-misreport path.
    #[test]
    fn desktop_load_session_response_contract() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!(
            "pinvou3-desktop-session-contract-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &root);
        let store = crate::features::sessions::SessionStore::boot_with_scheduled_root(
            root.join("scheduled"),
        )
        .expect("session store");
        let session = store
            .create_new("model".to_string(), None, root.clone())
            .expect("create session");
        let revision = transcript_revision(&session.messages).expect("transcript revision");

        let response = DesktopSavedSession {
            session,
            transcript_revision: revision.clone(),
        };
        let value = serde_json::to_value(&response).expect("serialize DesktopSavedSession");

        // 顶层必须带 snake_case 的 transcript_revision(WebSavedSession 同契约)。
        assert_eq!(
            value.get("transcript_revision").and_then(|v| v.as_str()),
            Some(revision.as_str()),
            "load_session 响应必须携带 transcript_revision 字段"
        );
        // SavedSession 其余字段通过 flatten 保留在顶层,不得嵌套丢失。
        assert!(
            value.get("messages").is_some(),
            "flatten 后 messages 必须仍在顶层"
        );
        assert!(
            value.get("metadata").is_some(),
            "flatten 后 metadata 必须仍在顶层"
        );
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
