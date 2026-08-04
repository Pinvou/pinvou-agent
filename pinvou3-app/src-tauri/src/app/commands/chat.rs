use super::attachments::{
    build_message_with_attachments_in_dir, validate_staged_attachment_basename,
};
use super::knowledge::build_kb_agentic_guide;
use super::prelude::*;
use crate::features::assistant::engine::TurnReservation;
use crate::features::assistant::engine_pool::user_display_message;

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
    acp_pool: State<'_, crate::features::codex_acp::AcpPool>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<(), String> {
    let trimmed = message.trim();
    if trimmed.is_empty() && attachments.as_ref().is_none_or(|a| a.is_empty()) {
        return Err("empty message".to_string());
    }
    // 多 session 并发:消息显式路由到指定 session(前端传 session_id);兼容旧前端时
    // 回退到全局 active_id。每条消息按各自 session 取 mode/phase/skill,送到对应 engine。
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    if acp_pool.is_acp(&sid) {
        return Err("ACP 代码会话必须通过独立代码页面发送".to_string());
    }
    let reservation = pool
        .reserve_turn(&sid)
        .map_err(|error| format!("reserve chat turn: {error:#}"))?;
    chat_with_reservation(
        message,
        attachments,
        sid,
        restrict_tools,
        reservation,
        &pool,
        &store,
        &app,
    )
    .await
}

/// Complete a turn after its session was atomically reserved. Web access uses
/// this seam so attachment-handle resolution cannot race another submission.
pub(crate) async fn chat_with_reservation(
    message: String,
    attachments: Option<Vec<crate::features::files::file_ingest::IngestResult>>,
    sid: String,
    restrict_tools: Option<bool>,
    reservation: TurnReservation,
    pool: &EnginePool,
    store: &SessionStore,
    app: &AppHandle,
) -> Result<(), String> {
    if message.trim().is_empty() && attachments.as_ref().is_none_or(|a| a.is_empty()) {
        return Err("empty message".to_string());
    }
    // 默认标题会话用首条消息自动命名（原生代码会话与 ACP 同一语义）。
    // 命名失败不阻断发送——标题是展示层信息，正文投递才是关键路径。
    let title_source = if message.trim().is_empty() {
        attachments
            .as_deref()
            .unwrap_or_default()
            .first()
            .map(|attachment| attachment.basename.clone())
            .unwrap_or_default()
    } else {
        message.trim().to_string()
    };
    if let Err(error) = super::sessions::apply_default_session_title(store, &sid, &title_source) {
        log::warn!("[pinvou3][chat] auto title failed for {sid}: {error}");
    }
    let chat_started_at = std::time::Instant::now();
    let message_len = message.len();
    let attachment_count = attachments.as_ref().map_or(0, Vec::len);
    log::info!(
        "[pinvou3][chat] request start sid={} message_len={} attachments={}",
        sid,
        message_len,
        attachment_count
    );
    let roots = store
        .session_roots(&sid)
        .map_err(|error| format!("resolve session roots for {sid}: {error:#}"))?;
    // 附件引用与落盘都走账本根（scheduled 会话 = 其 automation workspace，其余
    // 会话 = 会话私有目录），与引擎执行根解耦。
    let ledger_root = roots.ledger.clone();
    let attachment_record =
        prepare_conversation_attachment_record(attachments.as_deref().unwrap_or_default(), || {
            store
                .load(&sid)
                .map(|session| session.messages.len())
                .map_err(|error| format!("load Session {sid} for attachment references: {error:#}"))
        })?;
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
    let display_content =
        display_chat_message(&message, attachments.as_deref().unwrap_or_default());
    let raw_message = message.clone();
    // 原生代码会话绑项目目录时，落盘根（会话私有目录）与引擎 cwd（项目目录）
    // 不同根，附件引用必须绝对路径；其余会话两根一致，维持相对路径引用。
    // 两个根由 SessionStore::session_roots 统一解析（与引擎执行根同一来源）。
    let reference_absolute = roots.ledger != roots.execution;
    let mut full = build_message_with_attachments_in_dir(
        message,
        attachments.unwrap_or_default(),
        &ledger_root,
        &attachment_dir,
        reference_absolute,
    );
    // 工作流 Phase 可视化:用户在工作流页"启用"卡片 = start_skill_session
    // 新建一个绑定了 skill 的 session。该 session 第一条 chat 消息时,
    // 把 skill body + phase 规则 prepend 一次,后续 turn 靠 LLM session 上下文保持。
    //
    // 另外 — 实测 Qwen3.6 在长上下文里对 system prompt 顶端的 phase marker
    // MANDATORY 段遵循率衰减(legacy-ppt-workflow 跑到 p5+ 后频繁不出 `<phase id=".."/>`
    // marker),每个 user turn 都重申一遍约束,把信号搬到距 LLM 最近的位置。
    let pending_injections = store.take_pending_turn_injections(&sid);
    if let Some(injected) = pending_injections.skill_instruction() {
        full = format!("{injected}\n\n---\n\n{full}");
    }
    // [phase marker 下线] 原 active_skill 非工作流分支每 turn 注入 `<phase id=.../>`
    // marker reminder,但消费链(底座抽取 → chat:phase_changed → 前端 chips)已整体拆除,
    // marker 产出后无人消费。已删。active_skill 的 pending_instruction(skill body)仍走上面。

    // Side B 卡片池: 加持后首条消息一次性 prepend 完整人设 body(agency-agents-zh)。
    // 之后每 turn 只靠 equip_anchor 轻锚点维持身份(EnginePool 注入),不再重灌 body。
    if let Some(body) = pending_injections.persona_body() {
        full = format!("{body}\n\n---\n\n{full}");
    }
    let memory_enabled = crate::features::memory::memory_enabled();
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
    // 调 kb_search 工具(engine 已注入)检索、严格基于结果作答、无依据就说不知道;需要命中
    // 附近上下文时用 kb_open_source,不直接打开二进制原文件。不再自动注入片段(注入式已
    // 废弃)。collection_name 是单行查询,直接调即可(非大查询不必 spawn)。
    //
    // 关键防线:kb_search 的可见性是 engine config.disallowed_tools 控制的,而知识库模型/
    // 索引状态可能在 engine spawn 后才变化。挂集 turn 先刷新 live engine 的工具门控;
    // 若 kb_search 当前仍不可用,不要注入“必须调用 kb_search”的提示,避免模型把提示/sudo
    // 状态当普通文本复述给用户。
    let mounted_collection_ids = store.mounted_collection_ids(&sid);
    if !mounted_collection_ids.is_empty() {
        let disallowed = pool.compute_disallowed_tools();
        let kb_search_hidden = disallowed
            .iter()
            .any(|t| t.eq_ignore_ascii_case("kb_search"));
        pool.set_disallowed_all(disallowed).await;
        if !kb_search_hidden {
            let collection_names = app
                .try_state::<KnowledgeService>()
                .map(|kb| {
                    mounted_collection_ids
                        .iter()
                        .filter_map(|collection_id| {
                            kb.l1().collection_name(*collection_id).ok().flatten()
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            full = format!(
                "{}\n\n---\n\n{full}",
                build_kb_agentic_guide(&collection_names)
            );
        }
    }
    // 取该 session 的 mode 状态（mode + 多智能体开关）。
    let mode_state = store.mode_state(&sid);
    // 多智能体模式（ADR-0006）：开着就每轮把主动委派提醒拼在用户消息前。
    // 每轮重申而非首轮一次性教学——长上下文里开头信号的遵循率会衰减
    // （skill phase marker 同款教训），关掉开关即停止注入。
    if mode_state.multi_agent {
        full = format!(
            "{}\n\n---\n\n{full}",
            super::multiagent::delegation_reminder()
        );
    }
    let mode = mode_state.mode;
    let send_started_at = std::time::Instant::now();
    crate::features::assistant::timing::start_turn(&sid);
    log::info!(
        "[pinvou3][chat] engine send start sid={} mode={:?} content_len={}",
        sid,
        mode,
        full.len()
    );
    match pool
        .send_reserved_user_message(
            &sid,
            full,
            user_display_message(display_content.clone()),
            mode.to_app_mode(),
            restrict_tools.unwrap_or(false),
            reservation,
        )
        .await
    {
        Ok(()) => {
            pending_injections.commit();
            if let Some((message_index, attachment_references)) = attachment_record {
                if let Err(error) =
                    crate::features::files::attachment_upload::record_conversation_attachments(
                        &ledger_root,
                        &sid,
                        message_index,
                        &display_content,
                        attachment_references,
                    )
                {
                    log::warn!(
                        "[pinvou3][chat] persist attachment references failed sid={} error={}",
                        sid,
                        error
                    );
                }
            }
            if memory_enabled {
                crate::features::memory::record_turn_user(&sid, &raw_message);
            }
            log::info!(
                "[pinvou3][chat] engine send ok sid={} send_elapsed_ms={} total_elapsed_ms={}",
                sid,
                send_started_at.elapsed().as_millis(),
                chat_started_at.elapsed().as_millis()
            );
            Ok(())
        }
        Err(e) => {
            crate::features::assistant::timing::finish_turn(
                &sid,
                "send_error",
                Some(&format!("{e:?}")),
            );
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

fn prepare_conversation_attachment_record(
    attachments: &[crate::features::files::file_ingest::IngestResult],
    load_message_index: impl FnOnce() -> Result<usize, String>,
) -> Result<
    Option<(
        usize,
        Vec<crate::features::files::attachment_upload::ConversationAttachmentReference>,
    )>,
    String,
> {
    if attachments.is_empty() {
        return Ok(None);
    }
    let message_index = load_message_index()?;
    let references = attachments
        .iter()
        .map(|attachment| {
            crate::features::files::attachment_upload::ConversationAttachmentReference {
                basename: attachment.basename.clone(),
                path: attachment.path.clone(),
            }
        })
        .collect();
    Ok(Some((message_index, references)))
}

fn display_chat_message(
    message: &str,
    attachments: &[crate::features::files::file_ingest::IngestResult],
) -> String {
    if attachments.is_empty() {
        return message.to_string();
    }
    let names = attachments
        .iter()
        .map(|attachment| attachment.basename.as_str())
        .collect::<Vec<_>>();
    // Persist a JSON array after the human-readable marker. Unlike the legacy
    // `name · name` format, this preserves every legal filename exactly.
    let names = serde_json::to_string(&names).expect("attachment filenames serialize");
    if message.trim().is_empty() {
        format!("📎 {names}")
    } else {
        format!("{message}\n\n📎 {names}")
    }
}

#[cfg(test)]
mod attachment_record_tests {
    use super::{display_chat_message, prepare_conversation_attachment_record};
    use crate::features::files::file_ingest::IngestResult;

    #[test]
    fn messages_without_attachments_do_not_load_the_session_index() {
        let record = prepare_conversation_attachment_record(&[], || {
            panic!("message index must stay lazy without attachments")
        })
        .unwrap();
        assert!(record.is_none());
    }

    #[test]
    fn attachment_display_protocol_preserves_delimiter_inside_filename() {
        let attachments = vec![IngestResult {
            kind: "xlsx".into(),
            basename: "预算 · 最终.xlsx".into(),
            path: "/tmp/预算 · 最终.xlsx".into(),
            markdown: None,
            token_estimate: 0,
            byte_size: 1,
            warning: None,
        }];
        assert_eq!(
            display_chat_message("请分析", &attachments),
            "请分析\n\n📎 [\"预算 · 最终.xlsx\"]"
        );
    }
}
