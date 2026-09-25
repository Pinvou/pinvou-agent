use super::prelude::*;
// Native save dialog support for `export_session`; the other session
// commands do not interact with the dialog plugin.
use std::path::PathBuf;
use tauri_plugin_dialog::DialogExt;

#[derive(Debug, Clone, Serialize)]
pub struct SessionListItem {
    #[serde(flatten)]
    pub metadata: SessionMetadata,
    pub pinned: bool,
    pub pinned_at: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub title_attachment_names: Vec<String>,
    /// User workspace binding for plain sessions (#445; None = unbound). The
    /// project layer uses it to pull bound work sessions into project grouping
    /// (grouping follows binding, the same signal as the safety posture).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_binding: Option<String>,
}

/// Web boundary projection for SessionListItem: degrade the host absolute
/// workspace-binding path to its last component, mirroring the metadata
/// redaction, so the WebUI never receives host directory structure. The
/// projects slice is desktop-only and the frontend gates the matching
/// grouping branch on the same desktop capability, so on web the field is an
/// inert leaf name rather than a grouping key (review #464 round-6 finding 8b:
/// an earlier comment claimed the field "carries no function on web", which
/// stopped being true once the bound-session grouping filter became
/// path-based — the capability gate is what keeps leaf-name collisions from
/// collapsing distinct directories; the projection stays because the field is
/// still serialized and must not leak host paths).
pub(crate) fn redact_session_list_item_for_web(item: &mut SessionListItem) {
    if let Some(binding) = &item.workspace_binding {
        item.workspace_binding = Some(super::codex::redact_workspace_path_for_web(binding));
    }
}

/// Whole-list web projection applied by `web_access_list_sessions`: metadata
/// redaction plus `workspace_binding` degradation in one place, so the web
/// entry point cannot ship half the projection (review #464 round-4 — the
/// call site delegates here, which is what the test pins).
pub(crate) fn project_session_list_for_web(items: &mut [SessionListItem]) {
    for item in items.iter_mut() {
        item.metadata = super::codex::redact_session_metadata_for_web(item.metadata.clone());
        redact_session_list_item_for_web(item);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HiddenSessionListItem {
    #[serde(flatten)]
    pub metadata: SessionMetadata,
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
    // No raw session ids in these chains (round-28 N7): the rejections are
    // console.warn-ed by the panel and cross the relay to browser consoles
    // on the web lane; the action name identifies the failing step.
    match store
        .session_kind(id)
        .map_err(|error| format!("{action}: {error:#}"))?
    {
        SessionKind::Chat => Ok(()),
        SessionKind::ScheduledRun => Err(format!(
            "{action}: scheduled-run sessions are managed from Scheduled"
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
            // On a read-cache miss the sidecar is re-read; the list is ≤50
            // entries. Bound sessions stay resident once backfilled, while
            // unbound sessions get no negative caching — each refresh still
            // costs on the order of ≤2 syscalls × 50 (acceptable; review #464
            // nit: the comment must not exaggerate this as "resident after a
            // single N-read pass").
            let workspace_binding = store
                .session_workspace_binding(&metadata.id)
                .map(|path| path.display().to_string());
            SessionListItem {
                pinned: store.is_pinned(&metadata.id),
                pinned_at: store.pinned_at(&metadata.id),
                title_attachment_names,
                workspace_binding,
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
        // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
        // serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
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
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
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
    metas.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
    Ok(metas
        .into_iter()
        .map(|metadata| {
            let title_attachment_names = session_title_attachment_names(&store, &metadata);
            HiddenSessionListItem {
                archived_at: store.hidden_at(&metadata.id),
                title_attachment_names,
                metadata,
            }
        })
        .collect())
}

/// 新建空 session 并设为 active。返回创建的 SessionMetadata。
/// 引擎层的 session 状态切换由 chat() 下次发消息时自然处理（暂不发 SyncSession）。
/// When `workspace` is Some, metadata.workspace uses that directory (for
/// display); None keeps the status quo (pool.bridge.workspace, the home
/// directory).
pub(super) fn create_session_record(
    set_active: bool,
    store: &SessionStore,
    pool: &EnginePool,
    workspace: Option<PathBuf>,
) -> Result<SessionMetadata, String> {
    let (model, model_id) = pool.default_model_for_new_session();
    let workspace = workspace.unwrap_or_else(|| pool.bridge.workspace.clone());
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
    workspace_path: Option<String>,
    app: AppHandle,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<SessionMetadata, String> {
    let workspace = workspace_path
        .as_deref()
        .map(crate::features::sessions::validate_user_workspace_path)
        .transpose()
        .map_err(|e| format!("create_session: invalid workspace_path: {e:#}"))?;
    let metadata =
        create_session_record(set_active.unwrap_or(true), &store, &pool, workspace.clone())?;
    if let Some(workspace) = workspace {
        // A failed binding persist must not leave behind a session that "looked
        // created but falls back to the private execution root after restart":
        // roll back by deleting the just-created empty session (in the rollback
        // style of create_new).
        if let Err(error) = store.bind_session_workspace(&metadata.id, workspace) {
            let rollback = store.delete(&metadata.id);
            return Err(match rollback {
                Ok(()) => format!("create_session: bind workspace: {error:#}"),
                Err(rollback_error) => format!(
                    "create_session: bind workspace: {error:#}; rollback Session {}: {rollback_error:#}",
                    metadata.id
                ),
            });
        }
    }
    emit_session_event(&app, "session:list_changed", &metadata.id, "created");
    // 多 session 并发:不预热 engine(lazy)。新建的空 session 没有历史,首条 chat
    // 时 EnginePool.get_or_spawn 会为它 spawn 一个带专属 workspace 的 engine。
    Ok(metadata)
}

/// Queries a plain chat session's user working-directory binding (None when
/// unbound). The frontend uses this to apply the code lane's safety posture to
/// bound sessions (Plan on first use / one-shot YOLO confirm) and to show a
/// bound-directory indicator.
#[tauri::command]
pub async fn get_session_workspace_binding(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Option<String>, String> {
    Ok(store
        .session_workspace_binding(&session_id)
        .map(|path| path.display().to_string()))
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
                    "error_category": "session_load_failed",
                    "error_present": true,
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
                    "error_category": "revision_compute_failed",
                    "error_present": true,
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

/// Testable body of the `delete_session` Chat branch: evict the ACP side
/// first, then cascade-delete the aux session (gated delete -> forget ->
/// `session:deleted` event), and finally delete the main session and clean up
/// the Agent mapping.
///
/// The command body depends on `AppHandle` / `EnginePool` / `AcpPool` (Tauri
/// State, not constructible in unit tests), so the ordering and cascade
/// decisions are extracted here with side effects injected via closures — the
/// same technique as `EnginePool::cancel` extracting `cancel_turn_with_gates`.
/// Tests drive **this function** with fakes that record call order, so
/// reordering any step below turns the ordering assertions red, instead of
/// asserting against a copy of the command body (PR #433 review S2(a): the
/// original "ordering test" merely replayed the command's order with the
/// test's own callbacks; the real cascade body was never exercised).
///
/// Ordering constraint: the aux must be deleted strictly before the main
/// session. `SessionStore::delete`'s on-disk cascade only removes the record,
/// which would leave a still-running aux engine as a handle-less orphan; and
/// deleting the main session first would strip the aux teardown of its
/// session context.
async fn delete_chat_session_cascade<Ev, EvFut, De, DeFut, F, Em, R>(
    store: &SessionStore,
    session_id: &str,
    mut evict_acp: Ev,
    mut delete_session: De,
    mut forget_session: F,
    mut emit_deleted: Em,
    mut remove_acp_agent: R,
) -> Result<(), String>
where
    Ev: FnMut(&str) -> EvFut,
    EvFut: Future<Output = ()>,
    De: FnMut(&str) -> DeFut,
    DeFut: Future<Output = anyhow::Result<()>>,
    F: FnMut(&str),
    Em: FnMut(&str),
    R: FnMut(&str) -> Result<(), String>,
{
    evict_acp(session_id).await;
    // The aux session cascade must go through the gated delete first (turn
    // gate + engine reclaim + late sweep, the same path as
    // discard_aux_session): store.delete's cascade only removes the on-disk
    // records and would leave a still-running aux engine as a handle-less
    // orphan.
    if let Some(aux_id) = store.aux_session_id(session_id) {
        delete_session(&aux_id).await.map_err(|error| {
            // No raw aux id in this chain (round-29 S1, same stance as the
            // round-28 N7 fix): `delete_session` is on the web allowlist, so
            // the rejection crosses the relay to browser consoles; the step
            // name identifies the failing stage.
            format!("delete_session({session_id}): cascade delete aux session: {error:#}")
        })?;
        forget_session(&aux_id);
        emit_deleted(&aux_id);
    }
    let result = delete_session(session_id)
        .await
        .map_err(|error| format!("delete_session({session_id}): {error:#}"));
    if result.is_ok() {
        remove_acp_agent(session_id)?;
    }
    result
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
            let emit_deleted = |session_id: &str| {
                let payload = serde_json::json!({ "id": session_id });
                let _ = app.emit("session:deleted", payload.clone());
                crate::features::remote_control::forward_app_event(
                    &app,
                    "session:deleted",
                    payload,
                );
            };
            delete_chat_session_cascade(
                &store,
                &id,
                |session_id| {
                    let acp_pool = acp_pool.inner().clone();
                    let session_id = session_id.to_string();
                    async move { acp_pool.evict(&session_id).await }
                },
                |session_id| {
                    let pool = pool.inner().clone();
                    let session_id = session_id.to_string();
                    async move { pool.delete_chat_session(&session_id).await }
                },
                |session_id| pool.forget_session(session_id),
                emit_deleted,
                |session_id| {
                    acp_pool.agents().remove(session_id).map_err(|error| {
                        format!("failed to clean up the Agent session mapping: {error:#}")
                    })
                },
            )
            .await
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
        // Clearing keys of process-level per-session maps (timing/
        // pending_user_input/memory/monitor) is done uniformly by the
        // SessionPurgedHook fired on delete paths (registered at the lib.rs
        // composition root; Chat goes through store.delete, ScheduledRun
        // through purge_session_side_maps).
        let payload = serde_json::json!({ "id": &id });
        let _ = app.emit("session:deleted", payload.clone());
        crate::features::remote_control::forward_app_event(&app, "session:deleted", payload);
    }
    result
}

#[cfg(test)]
// The fixture holds the process-wide ENV_LOCK (std Mutex) across the awaits of
// the cascade under test to keep PINVOU3_HOME stable; cargo test runs test
// threads in parallel, and the lock is only contended by other env-writing
// tests, so it cannot deadlock.
#[allow(clippy::await_holding_lock)]
mod delete_session_cascade_tests {
    use super::delete_chat_session_cascade;
    use crate::features::sessions::SessionStore;
    use std::sync::{Arc, Mutex};

    /// Isolated `PINVOU3_HOME` with a main chat session plus its aux session,
    /// i.e. the store state `delete_session` sees when a task owns a side chat.
    struct AuxFixture {
        store: SessionStore,
        root: std::path::PathBuf,
        main_id: String,
        aux_id: String,
        previous_home: Option<std::ffi::OsString>,
        _env_lock: std::sync::MutexGuard<'static, ()>,
    }

    impl AuxFixture {
        fn boot(label: &str) -> Self {
            let env_lock = crate::platform::paths::tests::ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let root = std::env::temp_dir().join(format!(
                "pinvou3-delete-cascade-{label}-{}",
                std::process::id()
            ));
            let previous_home = std::env::var_os("PINVOU3_HOME");
            let _ = std::fs::remove_dir_all(&root);
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            unsafe { std::env::set_var("PINVOU3_HOME", &root) };
            let store = SessionStore::boot_with_scheduled_root(root.join("scheduled"))
                .expect("session store");
            let main_id = store
                .create_new("model".to_string(), None, root.join("workspace"))
                .expect("main chat session")
                .metadata
                .id;
            let aux_id = store
                .get_or_create_aux_session(&main_id)
                .expect("aux session")
                .id;
            Self {
                store,
                root,
                main_id,
                aux_id,
                previous_home,
                _env_lock: env_lock,
            }
        }
    }

    impl Drop for AuxFixture {
        fn drop(&mut self) {
            match self.previous_home.take() {
                // SAFETY: the fixture holds platform::paths::tests::ENV_LOCK.
                Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
                // SAFETY: the fixture holds platform::paths::tests::ENV_LOCK;
                // env writes are serialized in-process.
                None => unsafe { std::env::remove_var("PINVOU3_HOME") },
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// PR #433 review round-10 (S2(a)): the Chat branch of `delete_session` is
    /// driven through the extracted body, so this test fails if the body is
    /// reordered — the earlier "cascade order" test replayed the command's
    /// order with its own callbacks and would have stayed green.
    #[tokio::test]
    async fn chat_delete_cascades_aux_before_main_and_emits_aux_event() {
        let fixture = AuxFixture::boot("order");
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let (evict, delete, forget, emit, remove_agent) = (
            Arc::clone(&log),
            Arc::clone(&log),
            Arc::clone(&log),
            Arc::clone(&log),
            Arc::clone(&log),
        );

        delete_chat_session_cascade(
            &fixture.store,
            &fixture.main_id,
            move |session_id| {
                evict.lock().unwrap().push(format!("evict:{session_id}"));
                async {}
            },
            move |session_id| {
                delete.lock().unwrap().push(format!("delete:{session_id}"));
                async { Ok(()) }
            },
            move |session_id| {
                forget.lock().unwrap().push(format!("forget:{session_id}"));
            },
            move |session_id| emit.lock().unwrap().push(format!("emit:{session_id}")),
            move |session_id| {
                remove_agent
                    .lock()
                    .unwrap()
                    .push(format!("remove-agent:{session_id}"));
                Ok(())
            },
        )
        .await
        .expect("chat delete cascade");

        assert_eq!(
            *log.lock().unwrap(),
            vec![
                format!("evict:{}", fixture.main_id),
                format!("delete:{}", fixture.aux_id),
                format!("forget:{}", fixture.aux_id),
                format!("emit:{}", fixture.aux_id),
                format!("delete:{}", fixture.main_id),
                format!("remove-agent:{}", fixture.main_id),
            ],
            "the aux session's delete/forget/event must happen strictly before the main session \
             delete (deleting the main session first would leave the aux engine as a handle-less \
             orphan), and the ACP eviction must happen first"
        );
    }

    /// A failing aux delete must abort the cascade: the main session is left
    /// untouched and no aux `session:deleted` event is emitted (the command
    /// returns the error and skips its post-success block).
    #[tokio::test]
    async fn chat_delete_aborts_before_main_when_aux_delete_fails() {
        let fixture = AuxFixture::boot("aux-failure");
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let (evict, delete, forget, remove_agent) = (
            Arc::clone(&log),
            Arc::clone(&log),
            Arc::clone(&log),
            Arc::clone(&log),
        );

        let error = delete_chat_session_cascade(
            &fixture.store,
            &fixture.main_id,
            move |session_id| {
                evict.lock().unwrap().push(format!("evict:{session_id}"));
                async {}
            },
            move |session_id| {
                delete.lock().unwrap().push(format!("delete:{session_id}"));
                let session_id = session_id.to_string();
                async move {
                    if crate::features::sessions::is_aux_session_id(&session_id) {
                        anyhow::bail!("aux engine still running");
                    }
                    Ok(())
                }
            },
            move |session_id| {
                forget.lock().unwrap().push(format!("forget:{session_id}"));
            },
            // No session id in the message: `panic!` is a CodeQL
            // cleartext-logging sink, and the assertion is about the event
            // being emitted at all, not about which id it carried.
            |_session_id| {
                panic!("no session:deleted event may be emitted when the aux delete fails")
            },
            move |session_id| {
                remove_agent
                    .lock()
                    .unwrap()
                    .push(format!("remove-agent:{session_id}"));
                Ok(())
            },
        )
        .await
        .expect_err("aux delete failure must abort the cascade");

        assert!(
            error.contains("cascade delete aux session"),
            "the error must close with the aux cascade context, got: {error}"
        );
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                format!("evict:{}", fixture.main_id),
                format!("delete:{}", fixture.aux_id),
            ],
            "the main session must not be touched after the aux delete fails"
        );
    }
}

/// Payload returned by `export_session`: the save path (the location the
/// user confirmed in the native dialog) plus an archive size summary that
/// the frontend uses to render the success message.
#[derive(Debug, Clone, Serialize)]
pub struct ExportedSessionArchive {
    pub path: String,
    pub session_id: String,
    pub member_count: usize,
    pub includes_artifacts: bool,
    pub total_member_bytes: u64,
    pub compressed_bytes: u64,
}

/// Sanitize the archive default file name: keep only the base file name,
/// strip any existing extension, and reject control and path-sensitive
/// characters; abnormal input falls back to
/// `pinvou-session-<first 8 chars of id>.tar.xz` (the fallback stem keeps
/// only `[A-Za-z0-9_-]`, matching the upstream valid character set of
/// session ids). The shared guard lives in `prelude::safe_export_stem`; the
/// per-caller part here is the multi-segment `.tar.xz` extension handling and
/// the session-id-derived fallback stem.
fn normalized_archive_name(default_name: &str, session_id: &str) -> String {
    const EXTENSION: &str = "tar.xz";
    let fallback_stem = format!(
        "pinvou-session-{}",
        session_id
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
            .take(8)
            .collect::<String>()
    );
    safe_export_stem(
        default_name,
        EXTENSION,
        &format!("{fallback_stem}.{EXTENSION}"),
    )
}

/// One-click full session log export: open the native save dialog and pack
/// the session's full-fidelity record (system prompt, all turns, tool calls
/// and results) together with artifacts into a `.tar.xz` archive. Packing
/// reuses the base `deepseek_tui::session_export` and runs inside
/// spawn_blocking so the main thread is not blocked. Returns `Ok(None)` when
/// the user cancels.
///
/// Accepts any persisted session id under read-only semantics (same as
/// `load_session`, without calling `ensure_chat_session`): scheduled run
/// sessions can be exported too; external ACP sessions have no local
/// persisted record, and `store.export_archive` naturally reports "not
/// found". The menu entry only appears on chat sessions.
#[tauri::command]
pub async fn export_session(
    app: AppHandle,
    id: String,
    default_name: String,
    include_artifacts: Option<bool>,
    store: State<'_, SessionStore>,
) -> Result<Option<ExportedSessionArchive>, String> {
    let filename = normalized_archive_name(&default_name, &id);
    let Some(picked) = app
        .dialog()
        .file()
        .set_file_name(&filename)
        .add_filter("Session archive", &["tar.xz"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = picked
        .into_path()
        .map_err(|error| format!("resolve_export_path_failed: {error}"))?;
    let store = store.inner().clone();
    let include_artifacts = include_artifacts.unwrap_or(true);
    let summary =
        tokio::task::spawn_blocking(move || store.export_archive(&id, &path, include_artifacts))
            .await
            .map_err(|error| format!("session_export_task_failed: {error}"))?
            .map_err(|error| format!("session_export_failed: {error:#}"))?;
    Ok(Some(ExportedSessionArchive {
        path: summary.output.display().to_string(),
        member_count: summary.members.len(),
        includes_artifacts: summary.includes_artifacts,
        total_member_bytes: summary.total_member_bytes(),
        compressed_bytes: summary.compressed_bytes(),
        session_id: summary.session_id,
    }))
}

/// 重命名 session 标题。普通会话与定时运行会话共用 Session 元数据。
#[tauri::command]
pub async fn rename_session(
    id: String,
    title: String,
    app: AppHandle,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    if crate::features::sessions::is_aux_session_id(&id) {
        return Err(format!(
            "rename_session({id}): auxiliary conversations are managed through their main session"
        ));
    }
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
    // Aux ids would land in the pinned table as ghost entries: aux sessions
    // are invisible in every list, so the entry could never be cleared from
    // the UI.
    if crate::features::sessions::is_aux_session_id(&id) {
        return Err(format!(
            "set_session_pinned({id}): auxiliary conversations are managed through their main session"
        ));
    }
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
    // Same ghost-entry argument as set_session_pinned.
    if crate::features::sessions::is_aux_session_id(&id) {
        return Err(format!(
            "set_session_archived({id}): auxiliary conversations are managed through their main session"
        ));
    }
    // 先 load 一次确认 session 存在,避免收起表残留无效 id。
    store
        .load(&id)
        .map_err(|e| format!("set_session_archived({id}): {e:#}"))?;
    store.set_hidden(&id, archived);
    let action = if archived { "archived" } else { "restored" };
    emit_session_event(&app, "session:list_changed", &id, action);
    Ok(())
}

// ===================== Auxiliary conversation (aux session) =====================

/// Minimal projection of an aux session binding. This command is on the Web
/// RPC allowlist, so it must not return the full `SessionMetadata`: the
/// inherited host `workspace` path would cross the Web/Relay boundary to the
/// browser (the same redaction invariant behind
/// `redact_session_metadata_for_web` and the `web_access_*` projections).
/// Both bridges consume only `metadata.id`.
#[derive(Debug, Clone, Serialize)]
pub struct AuxSessionBinding {
    pub id: String,
}

/// Get (creating if absent) the auxiliary conversation of a main session. Aux
/// sessions are persisted with an `aux-` prefix and stay out of the ordinary
/// session list, so creation does **not** emit `session:list_changed`; the
/// frontend auxiliary conversation panel opens directly from the returned id.
#[tauri::command]
pub async fn get_or_create_aux_session(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<AuxSessionBinding, String> {
    // Every error path folds into the stable web_session code (round-30 B7):
    // the command is web-allowlisted and the native chains embed host paths
    // (sessions-root EACCES/EIO), which must not cross the relay to browser
    // consoles; the detail stays in the desktop log via web_session_result.
    super::remote_control::web_session_result(
        super::remote_control::WebSessionOperation::GetOrCreateAuxSession,
        get_or_create_aux_session_inner(session_id, store).await,
    )
}

async fn get_or_create_aux_session_inner(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<AuxSessionBinding, String> {
    // Auxiliary conversations may only hang off ordinary chat sessions:
    // scheduled sessions go through their own delete path
    // (delete_scheduled_run has no aux cascade), so attaching one would leak
    // an orphan aux session.
    ensure_chat_session(&store, &session_id, "get_or_create_aux_session")?;
    store
        .load(&session_id)
        .map_err(|e| format!("get_or_create_aux_session: main session not found: {e:#}"))?;
    store
        .get_or_create_aux_session(&session_id)
        .map(|metadata| AuxSessionBinding { id: metadata.id })
        .map_err(|e| format!("get_or_create_aux_session: {e:#}"))
}

/// Discard a main session's auxiliary conversation: reclaim the engine,
/// delete the aux session, and clear the mapping. Repeated calls are
/// idempotent (no mapping counts as already discarded).
#[tauri::command]
pub async fn discard_aux_session(
    session_id: String,
    app: AppHandle,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    // Same folding as get_or_create_aux_session (round-30 B7): the native
    // chain (e.g. "remove stale session dir /Users/<name>/...") must not
    // cross the relay.
    super::remote_control::web_session_result(
        super::remote_control::WebSessionOperation::DiscardAuxSession,
        discard_aux_session_inner(session_id, app, store, pool).await,
    )
}

async fn discard_aux_session_inner(
    session_id: String,
    app: AppHandle,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    // The derived-id forward query (round-30 B8): None only when the aux
    // record is genuinely absent (NotFound-only probe), so an absent aux is
    // the idempotent already-discarded case, and a transient stat fault reads
    // as "present" and flows into the gated delete, which surfaces the real
    // error instead of treating "unknown" as "nothing to discard".
    let Some(aux_id) = store.aux_session_id(&session_id) else {
        return Ok(());
    };
    // Same path as delete_session's Chat branch: delete_chat_session reclaims
    // the engine inside the turn gate and calls store.delete.
    pool.delete_chat_session(&aux_id)
        .await
        .map_err(|error| format!("discard_aux_session: {error:#}"))?;
    pool.forget_session(&aux_id);
    let payload = serde_json::json!({ "id": &aux_id });
    let _ = app.emit("session:deleted", payload.clone());
    crate::features::remote_control::forward_app_event(&app, "session:deleted", payload);
    Ok(())
}

/// 落盘 session 的产物 paths 列表。前端跟踪 File.write / File.edit 调用后调用,
/// 在 TurnComplete 时落盘。重启/切换 session 后,
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

/// Shared skeleton for the two position-keyed sidecar normalizers
/// (`normalize_pinvou_scene_events` / `normalize_steered_messages`): array
/// check, 10000-entry cap, per-entry `pos` validation, BTreeMap dedupe +
/// ascending sort, array rebuild. `extract_payload` validates/extracts the
/// entry payload and `singular`/`plural` thread the command-specific error
/// message prefixes.
fn normalize_position_keyed_entries(
    events: serde_json::Value,
    plural: &str,
    singular: &str,
    payload_key: &str,
    extract_payload: impl Fn(&serde_json::Value) -> Result<String, String>,
) -> Result<serde_json::Value, String> {
    let entries = events
        .as_array()
        .ok_or_else(|| format!("{plural} 必须是数组"))?;
    if entries.len() > 10_000 {
        return Err(format!("{plural} 超过 10000 条上限"));
    }
    let mut normalized = std::collections::BTreeMap::<u64, String>::new();
    for entry in entries {
        let pos = entry
            .get("pos")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| format!("{singular} 缺少有效 pos"))?;
        normalized.insert(pos, extract_payload(entry)?);
    }
    Ok(serde_json::Value::Array(
        normalized
            .into_iter()
            .map(|(pos, payload)| serde_json::json!({ "pos": pos, payload_key: payload }))
            .collect(),
    ))
}

fn normalize_pinvou_scene_events(events: serde_json::Value) -> Result<serde_json::Value, String> {
    normalize_position_keyed_entries(
        events,
        "pinvou scene events",
        "pinvou scene event",
        "scene",
        |entry| {
            match entry.get("scene").and_then(serde_json::Value::as_str) {
                Some(scene @ ("work:document-writing" | "work:personal-workbench")) => {
                    Ok(scene.to_string())
                }
                // design:poster / design:data-visualization / design:ppt are
                // historical persisted scene names from the merged design lane
                // and must stay accepted.
                Some(scene @ ("design:poster" | "design:data-visualization" | "design:ppt")) => {
                    Ok(scene.to_string())
                }
                _ => Err("pinvou scene event 包含无效 scene".to_string()),
            }
        },
    )
}

/// Unified session-sidecar persistence (shared by scene / steered / persona events / Pinvou reviews): create the
/// parent directory + serialize + `write_atomic`. Sidecars are session-persisted data; the atomic write
/// prevents an interrupted process from leaving half a JSON (error wording unified per sidecar, not per use).
pub(super) fn write_session_sidecar(
    path: &std::path::Path,
    value: &serde_json::Value,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create session sidecar directory: {error}"))?;
    }
    let payload = serde_json::to_vec(value)
        .map_err(|error| format!("failed to serialize session sidecar: {error}"))?;
    deepseek_tui::utils::write_atomic(path, &payload)
        .map_err(|error| format!("failed to write session sidecar: {error:#}"))
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
    write_session_sidecar(&path, &normalized)
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

fn normalize_steered_messages(events: serde_json::Value) -> Result<serde_json::Value, String> {
    normalize_position_keyed_entries(
        events,
        "steered messages",
        "steered message",
        "text",
        |entry| {
            let text = entry
                .get("text")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "steered message 缺少有效 text".to_string())?;
            if text.len() > 64_000 {
                return Err("steered message text 超过 64000 字符上限".to_string());
            }
            Ok(text.to_string())
        },
    )
}

/// 保存 mid-turn steer 消息的位置标记。steer 落盘与普通 admission 对齐、不含
/// `<turn_meta>` 块，重载投影靠该 sidecar 恢复"非 turn admission"标记；独立于
/// messages 持久化，桌面端与 WebUI 共享。
#[tauri::command]
pub async fn save_session_steered_messages(
    session_id: String,
    events: serde_json::Value,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    ensure_chat_session(&store, &session_id, "save_session_steered_messages")?;
    let normalized = normalize_steered_messages(events)?;
    let path = crate::platform::paths::session_steered_messages(&session_id);
    write_session_sidecar(&path, &normalized)
}

/// 读取 mid-turn steer 消息的位置标记。旧版本或损坏 sidecar 按空数组处理。
#[tauri::command]
pub async fn get_session_steered_messages(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<serde_json::Value, String> {
    ensure_chat_session(&store, &session_id, "get_session_steered_messages")?;
    let path = crate::platform::paths::session_steered_messages(&session_id);
    let Ok(payload) = std::fs::read(&path) else {
        return Ok(serde_json::json!([]));
    };
    let Ok(events) = serde_json::from_slice::<serde_json::Value>(&payload) else {
        return Ok(serde_json::json!([]));
    };
    Ok(normalize_steered_messages(events).unwrap_or_else(|_| serde_json::json!([])))
}

#[cfg(test)]
mod steered_message_tests {
    use super::normalize_steered_messages;

    #[test]
    fn steered_messages_are_validated_deduplicated_and_sorted() {
        let normalized = normalize_steered_messages(serde_json::json!([
            { "pos": 9, "text": "second steer" },
            { "pos": 4, "text": "first steer" },
            { "pos": 9, "text": "second steer edited" }
        ]))
        .expect("valid steered messages");
        assert_eq!(
            normalized,
            serde_json::json!([
                { "pos": 4, "text": "first steer" },
                { "pos": 9, "text": "second steer edited" }
            ])
        );
    }

    #[test]
    fn steered_messages_reject_invalid_entries() {
        assert!(
            normalize_steered_messages(serde_json::json!([
                { "pos": -1, "text": "x" }
            ]))
            .is_err()
        );
        assert!(normalize_steered_messages(serde_json::json!([{ "pos": 1 }])).is_err());
        assert!(normalize_steered_messages(serde_json::json!({ "pos": 1 })).is_err());
    }
}

#[cfg(test)]
mod pinvou_scene_event_tests {
    use super::normalize_pinvou_scene_events;

    #[test]
    fn scene_events_are_validated_deduplicated_and_sorted() {
        let normalized = normalize_pinvou_scene_events(serde_json::json!([
            { "pos": 7, "scene": "design:poster" },
            { "pos": 2, "scene": "work:document-writing" },
            { "pos": 7, "scene": "design:data-visualization" },
            { "pos": 5, "scene": "design:ppt" }
        ]))
        .expect("valid scene events");
        assert_eq!(
            normalized,
            serde_json::json!([
                { "pos": 2, "scene": "work:document-writing" },
                { "pos": 5, "scene": "design:ppt" },
                { "pos": 7, "scene": "design:data-visualization" }
            ])
        );
    }

    #[test]
    fn scene_events_reject_unknown_scenes_and_invalid_positions() {
        assert!(
            normalize_pinvou_scene_events(serde_json::json!([
                { "pos": 0, "scene": "design:webpage" }
            ]))
            .is_err()
        );
        assert!(
            normalize_pinvou_scene_events(serde_json::json!([
                { "pos": -1, "scene": "design:poster" }
            ]))
            .is_err()
        );
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

    #[test]
    fn legacy_design_scene_names_stay_valid() {
        // The design lane has been merged into the work lane, but the scene
        // marker strings design:poster / design:data-visualization /
        // design:ppt are historical persisted data and must stay accepted by
        // the whitelist.
        for scene in ["design:poster", "design:data-visualization", "design:ppt"] {
            let normalized = normalize_pinvou_scene_events(serde_json::json!([
                { "pos": 1, "scene": scene }
            ]))
            .expect("legacy design scene must stay valid");
            assert_eq!(
                normalized,
                serde_json::json!([{ "pos": 1, "scene": scene }])
            );
        }
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
        // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
        // serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
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
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod session_archive_name_tests {
    use super::*;

    #[test]
    fn archive_name_strips_extension_and_keeps_multi_segment_tar_xz() {
        assert_eq!(
            normalized_archive_name("会话 demo.tar.xz", "abcd1234-0000"),
            "会话 demo.tar.xz"
        );
        // Same semantics as the assistant export: keep the original stem and
        // always append .tar.xz.
        assert_eq!(
            normalized_archive_name("chat.txt", "abcd1234-0000"),
            "chat.txt.tar.xz"
        );
    }

    #[test]
    fn archive_name_cannot_escape_or_inject() {
        assert_eq!(
            normalized_archive_name("../evil/attack.tar.xz", "abcd1234-0000"),
            "attack.tar.xz"
        );
        assert_eq!(
            normalized_archive_name("bad:name?.tar.xz", "abcd1234-0000"),
            "pinvou-session-abcd1234.tar.xz"
        );
    }

    #[test]
    fn archive_name_falls_back_when_empty_or_overlong() {
        assert_eq!(
            normalized_archive_name("   ", "abcd1234-0000"),
            "pinvou-session-abcd1234.tar.xz"
        );
        let long = "x".repeat(200);
        assert_eq!(
            normalized_archive_name(&long, "abcd1234-0000"),
            "pinvou-session-abcd1234.tar.xz"
        );
    }
}

#[cfg(test)]
mod web_projection_tests {
    use super::*;

    /// The web session list must never carry the host absolute binding path:
    /// `workspace_binding` degrades to its last component, mirroring the
    /// metadata redaction and the codex list-item projection.
    #[test]
    fn redact_session_list_item_for_web_degrades_workspace_binding() {
        let metadata: SessionMetadata = serde_json::from_value(serde_json::json!({
            "id": "session-web-binding",
            "title": "Web binding projection",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "message_count": 0,
            "total_tokens": 0,
            "model": "test-model",
            "workspace": "/tmp/workspace"
        }))
        .expect("metadata");
        let mut item = SessionListItem {
            pinned: false,
            pinned_at: None,
            title_attachment_names: Vec::new(),
            workspace_binding: Some("/Users/host/Documents/secret-project".to_string()),
            metadata,
        };

        redact_session_list_item_for_web(&mut item);

        assert_eq!(
            item.workspace_binding.as_deref(),
            Some("secret-project"),
            "workspace_binding 过 Web 边界必须降级为末级目录名"
        );

        // Unbound sessions (None) are unaffected; the Windows form likewise
        // keeps only the final segment.
        item.workspace_binding = None;
        redact_session_list_item_for_web(&mut item);
        assert_eq!(item.workspace_binding, None);
        item.workspace_binding = Some(r#"C:\Users\host\proj"#.to_string());
        redact_session_list_item_for_web(&mut item);
        assert_eq!(item.workspace_binding.as_deref(), Some("proj"));
    }

    /// `web_access_list_sessions` delegates to `project_session_list_for_web`;
    /// this pins the whole-list contract (metadata + workspace_binding both
    /// projected) so deleting the one-line application at the call site cannot
    /// stay green (review #464 round-4 minor 3).
    #[test]
    fn project_session_list_for_web_projects_metadata_and_binding() {
        let metadata: SessionMetadata = serde_json::from_value(serde_json::json!({
            "id": "session-web-list",
            "title": "Web list projection",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "message_count": 0,
            "total_tokens": 0,
            "model": "test-model",
            "workspace": "/Users/host/Documents/secret-project"
        }))
        .expect("metadata");
        let mut items = vec![SessionListItem {
            pinned: false,
            pinned_at: None,
            title_attachment_names: Vec::new(),
            workspace_binding: Some("/Users/host/Documents/secret-project".to_string()),
            metadata,
        }];

        project_session_list_for_web(&mut items);

        let json = serde_json::to_value(&items[0]).expect("serialize projected item");
        assert_eq!(json["workspace"], "secret-project");
        assert_eq!(json["workspace_binding"], "secret-project");
        assert!(
            !json.to_string().contains("/Users/host"),
            "no host path component may cross the web boundary"
        );
    }
}
