use super::attachments::{
    build_message_with_attachments_in_dir, validate_staged_attachment_basename,
};
use super::knowledge::build_kb_agentic_guide;
use super::prelude::*;

/// 接收用户消息并转发给 Engine。
/// 立即返回，LLM 流式输出通过 Tauri Event 异步推给前端。
///
/// `attachments` 是前端已经过 `ingest_file` 处理后的 IngestResult 数组。
/// 本函数把它们拼到 message 末尾，格式：
/// ```text
/// ---
/// 用户附上了以下文件：
///
/// ### {basename} ({kind}, ~{tokens} tokens)
/// {markdown 或 警告}
/// ---
/// ```
#[tauri::command]
pub async fn chat(
    message: String,
    attachments: Option<Vec<crate::features::files::file_ingest::IngestResult>>,
    session_id: Option<String>,
    restrict_tools: Option<bool>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<(), String> {
    let chat_started_at = std::time::Instant::now();
    let message_len = message.len();
    let attachment_count = attachments.as_ref().map_or(0, Vec::len);
    let trimmed = message.trim();
    if trimmed.is_empty() && attachments.as_ref().map_or(true, |a| a.is_empty()) {
        return Err("empty message".to_string());
    }
    // 多 session 并发:消息显式路由到指定 session(前端传 session_id);兼容旧前端时
    // 回退到全局 active_id。每条消息按各自 session 取 mode/phase/skill,送到对应 engine。
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    log::info!(
        "[pinvou3][chat] request start sid={} message_len={} attachments={}",
        sid,
        message_len,
        attachment_count
    );
    let execution_workspace = store
        .execution_workspace(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let is_scheduled = store.scheduled_profile(&sid).is_some();
    if is_scheduled {
        for attachment in attachments.as_deref().unwrap_or_default() {
            validate_staged_attachment_basename(&attachment.basename).map_err(|error| {
                format!(
                    "invalid scheduled attachment basename {:?}: {error}",
                    attachment.basename
                )
            })?;
        }
    }
    let attachment_dir = if is_scheduled {
        // The scheduled engine runs in the configured project workspace, so
        // staged paths must remain relative to that same root. Isolate every
        // run under an app-named hidden directory to avoid cross-run clashes.
        format!(".pinvou3/scheduled-attachments/{sid}/attachments")
    } else {
        "attachments".to_string()
    };
    let raw_message = message.clone();
    crate::features::assistant::timing::start_turn(&sid);
    let mut full = build_message_with_attachments_in_dir(
        message,
        attachments.unwrap_or_default(),
        &execution_workspace,
        &attachment_dir,
    );
    // 工作流 Phase 可视化:用户在工作流页"启用"卡片 = start_skill_session
    // 新建一个绑定了 skill 的 session。该 session 第一条 chat 消息时,
    // 把 skill body + phase 规则 prepend 一次,后续 turn 靠 LLM session 上下文保持。
    //
    // 另外 — 实测 Qwen3.6 在长上下文里对 system prompt 顶端的 phase marker
    // MANDATORY 段遵循率衰减(h3c-ppt 跑到 p5+ 后频繁不出 `<phase id=".."/>`
    // marker),每个 user turn 都重申一遍约束,把信号搬到距 LLM 最近的位置。
    if let Some(injected) = store.take_pending_skill_instruction(&sid) {
        full = format!("{injected}\n\n---\n\n{full}");
    }
    // [phase marker 下线] 原 active_skill 非工作流分支每 turn 注入 `<phase id=.../>`
    // marker reminder,但消费链(底座抽取 → chat:phase_changed → 前端 chips)已整体拆除,
    // marker 产出后无人消费。已删。active_skill 的 pending_instruction(skill body)仍走上面。

    // Side B 卡片池: 加持后首条消息一次性 prepend 完整人设 body(agency-agents-zh)。
    // 之后每 turn 只靠 equip_anchor 轻锚点维持身份(EnginePool 注入),不再重灌 body。
    if let Some(body) = store.take_pending_persona_body(&sid) {
        full = format!("{body}\n\n---\n\n{full}");
    }
    let memory_enabled = crate::features::memory::memory_enabled();
    if memory_enabled {
        crate::features::memory::record_turn_user(&sid, &raw_message);
    }
    match crate::features::memory::runtime_snapshot(&sid) {
        Ok(snapshot) => {
            let _ = app.emit(
                "chat:memory",
                serde_json::json!({
                    "session_id": sid.clone(),
                    "items": snapshot.items,
                    "runtime_path": snapshot.runtime_path,
                }),
            );
        }
        Err(err) => {
            eprintln!("[pinvou3-app] refresh memory runtime failed for session {sid}: {err}");
        }
    }
    // Agentic RAG:该 session 挂了知识集 → 每 turn prepend Self-RAG 自检引导,让模型自己
    // 调 kb_search 工具(engine 已注入)检索、严格基于结果作答、无依据就说不知道。不再自动
    // 注入片段(注入式已废弃)。collection_name 是单行查询,直接调即可(非大查询不必 spawn)。
    //
    // 关键防线:kb_search 的可见性是 engine config.disallowed_tools 控制的,而知识库模型/
    // 索引状态可能在 engine spawn 后才变化。挂集 turn 先刷新 live engine 的工具门控;
    // 若 kb_search 当前仍不可用,不要注入“必须调用 kb_search”的提示,避免模型把提示/sudo
    // 状态当普通文本复述给用户。
    if let Some(cid) = store.mounted_collection(&sid) {
        let disallowed = pool.compute_disallowed_tools();
        let kb_search_hidden = disallowed
            .iter()
            .any(|t| t.eq_ignore_ascii_case("kb_search"));
        pool.set_disallowed_all(disallowed).await;
        if !kb_search_hidden {
            let coll_name = app
                .try_state::<KnowledgeService>()
                .and_then(|kb| kb.l1().collection_name(cid).ok().flatten());
            full = format!(
                "{}\n\n---\n\n{full}",
                build_kb_agentic_guide(coll_name.as_deref())
            );
        }
    }
    // 取该 session 的 mode。
    let mode = store.mode_state(&sid).mode;
    if session_uses_builtin_llmapi(&store, &sid) {
        let provision_started_at = std::time::Instant::now();
        log::info!("[pinvou3][chat] builtin provisioning start sid={}", sid);
        let provision_result = tokio::time::timeout(
            std::time::Duration::from_secs(25),
            tokio::task::spawn_blocking(
                crate::features::llmapi_hub::provisioning::ensure_binding_for_current_user,
            ),
        )
        .await;
        match provision_result {
            Ok(Ok(Ok(response))) => {
                log::info!(
                    "[pinvou3][chat] builtin provisioning ok sid={} status={:?} elapsed_ms={}",
                    sid,
                    response.status,
                    provision_started_at.elapsed().as_millis()
                );
            }
            Ok(Ok(Err(err))) => {
                log::warn!(
                    "[pinvou3][chat] builtin provisioning failed sid={} code={:?} retryable={} elapsed_ms={} message={}",
                    sid,
                    err.code,
                    err.retryable,
                    provision_started_at.elapsed().as_millis(),
                    err.message
                );
                return Err(err.to_tauri_error());
            }
            Ok(Err(err)) => {
                log::error!(
                    "[pinvou3][chat] builtin provisioning task join failed sid={} elapsed_ms={} error={}",
                    sid,
                    provision_started_at.elapsed().as_millis(),
                    err
                );
                return Err(format!("内置模型开通任务失败: {err}"));
            }
            Err(_) => {
                log::warn!(
                    "[pinvou3][chat] builtin provisioning timeout sid={} elapsed_ms={}",
                    sid,
                    provision_started_at.elapsed().as_millis()
                );
                return Err("内置模型开通超时，请稍后重试".to_string());
            }
        }
    }
    let send_started_at = std::time::Instant::now();
    log::info!(
        "[pinvou3][chat] engine send start sid={} mode={:?} content_len={}",
        sid,
        mode,
        full.len()
    );
    match pool
        .send_user_message(
            &sid,
            full,
            mode.to_app_mode(),
            restrict_tools.unwrap_or(false),
        )
        .await
    {
        Ok(()) => {
            log::info!(
                "[pinvou3][chat] engine send ok sid={} send_elapsed_ms={} total_elapsed_ms={}",
                sid,
                send_started_at.elapsed().as_millis(),
                chat_started_at.elapsed().as_millis()
            );
            Ok(())
        }
        Err(e) => {
            crate::features::assistant::timing::finish_turn(&sid, "send_error", Some(&format!("{e:?}")));
            log::error!(
                "[pinvou3][chat] engine send failed sid={} send_elapsed_ms={} total_elapsed_ms={} error={:?}",
                sid,
                send_started_at.elapsed().as_millis(),
                chat_started_at.elapsed().as_millis(),
                e
            );
            Err(format!("send_user_message failed: {e:?}"))
        }
    }
}

fn session_uses_builtin_llmapi(store: &SessionStore, session_id: &str) -> bool {
    let prefs = UserPrefs::load();
    let model = store
        .session_model_id(session_id)
        .and_then(|model_id| prefs.model_by_id(&model_id))
        .or_else(|| prefs.active_model());
    model.is_some_and(|model| model.is_builtin_llmapi())
}
