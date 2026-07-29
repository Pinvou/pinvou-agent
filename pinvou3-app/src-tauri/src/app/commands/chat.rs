use super::attachments::{
    attachment_display_text, build_message_with_attachments_in_dir,
    prepare_native_user_message_in_dir, validate_staged_attachment_basename,
};
use super::knowledge::build_kb_agentic_guide;
use super::prelude::*;
use crate::features::assistant::engine::TurnReservation;
use crate::features::assistant::engine_pool::user_display_message;
use crate::features::assistant::image_capability::{EffectiveImageCapability, ImageInputMode};

/// 图片输入不受支持的稳定错误码(设计 §9.2 Unsupported):前端按码匹配,
/// 码后为用户可操作指引。改码字符串必须同步前端匹配处
/// (platform/tauri/bridge/chat.js、platform/web/bridge.js)。
pub(crate) const IMAGE_INPUT_UNSUPPORTED_ERROR: &str = "image_input_unsupported: \
     当前模型不支持图片。请切换到支持图片的模型,或在模型设置中配置视觉模型。";

/// 能力未知(Unknown)时的拒绝文案:与"确认不支持"区分,给出显式确认的出口
/// (设计 §6.3:Unknown 不冒充支持,但允许用户手工开启)。错误码前缀相同。
pub(crate) const IMAGE_INPUT_UNKNOWN_ERROR: &str = "image_input_unsupported: \
     当前模型的图片输入能力未知。如果它支持图片,请在模型设置中将图片输入能力\
     设为“支持图片”后重试;也可以切换到支持图片的模型,或配置视觉模型。";

/// 接收用户消息并转发给 Engine。
/// 立即返回，LLM 流式输出通过 Tauri Event 异步推给前端。
///
/// `attachments` 是前端已经过 `ingest_file` 处理后的 IngestResult 数组。
/// 非图片附件拼到 message 末尾，格式：
/// ```text
/// ---
/// 用户附上了以下文件：
///
/// ### {basename} ({kind}, ~{tokens} tokens)
/// {markdown 或 警告}
/// ---
/// ```
/// 图片附件按当前模型能力路由(设计 §9.2):Native 走结构化 LocalImage 块,
/// VisionToolFallback 维持上方文本拼接 + image_analyze 硬规则,
/// Unsupported 发送前拒绝(稳定错误码 `image_input_unsupported`)。
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
    if acp_pool.is_codex(&sid) {
        return Err("Codex ACP 会话必须通过独立 Codex 页面发送".to_string());
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
    let chat_started_at = std::time::Instant::now();
    let message_len = message.len();
    let attachment_count = attachments.as_ref().map_or(0, Vec::len);
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
    let attachments = attachments.unwrap_or_default();
    // 普通会话图片输入路由(设计 §9.2,阶段 D):仅当消息含图片附件时按当前有效
    // 模型算 Native/VisionToolFallback/Unsupported;纯文本与非图片附件行为零变化。
    // scheduled 会话不路由,保持 image_analyze 工具兜底现状(其模型由任务 profile
    // 固定,且无人值守场景无法在发送前向用户展示切换模型提示)。
    let has_images = attachments.iter().any(|a| a.kind == "image");
    // capability 仅在路由时需要一并取出:Unsupported 拒绝时据此区分
    // "确认不支持"与"能力未知",给不同用户指引(Unknown 提供显式确认出口)。
    let (image_mode, capability) = if has_images && !is_scheduled {
        let bridge = pool
            .fresh_bridge_for(&sid)
            .await
            .map_err(|error| format!("resolve image input route for {sid}: {error:#}"))?;
        (bridge.image_input_mode(), bridge.effective_image_capability())
    } else if has_images {
        (
            ImageInputMode::VisionToolFallback,
            EffectiveImageCapability::Unknown,
        )
    } else {
        (ImageInputMode::Native, EffectiveImageCapability::Unknown)
    };
    if has_images && image_mode == ImageInputMode::Unsupported {
        // 不创建 Engine turn:early return 时 reservation 随 Drop 自动释放
        // (TurnReservation::drop → on_reservation_failed 归还 turn slot),
        // 稳定错误码经 invoke Err 回传前端匹配展示。
        log::info!("[pinvou3][chat] image input rejected sid={} (capability={capability:?})", sid);
        return Err(match capability {
            EffectiveImageCapability::Unknown => IMAGE_INPUT_UNKNOWN_ERROR.to_string(),
            _ => IMAGE_INPUT_UNSUPPORTED_ERROR.to_string(),
        });
    }
    let display_content = attachment_display_text(&message, &attachments);
    let raw_message = message.clone();
    // Native: 图片成 LocalImage 引用块,不注入"看不到图"硬规则;其余路径维持
    // 现有文本拼接(Fallback 含 image_analyze 硬规则提示)。两分支互斥,各自
    // consume message/attachments。
    let (prepared, mut full) = if has_images && image_mode == ImageInputMode::Native {
        let prepared = prepare_native_user_message_in_dir(
            message,
            attachments,
            &execution_workspace,
            &attachment_dir,
        )?;
        let segment = prepared.text_segment().to_string();
        (Some(prepared), segment)
    } else {
        (
            None,
            build_message_with_attachments_in_dir(
                message,
                attachments,
                &execution_workspace,
                &attachment_dir,
            ),
        )
    };
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
    // Native: skill/persona/知识库引导 prepend 全部完成后,用最终文本刷新
    // Text block(图片块保持不动),组装结构化 input。bridge 之后还会把
    // `<system-reminder>` 并进该 Text block 与 Op.content(两者保持一致,
    // 供 transcript sanitize 匹配)。
    let input = prepared.map(|mut prepared| {
        prepared.replace_text_segment(full.clone());
        deepseek_tui::core::ops::UserMessageInput {
            blocks: prepared.input_blocks,
        }
    });
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
            user_display_message(display_content),
            mode.to_app_mode(),
            restrict_tools.unwrap_or(false),
            reservation,
            input,
        )
        .await
    {
        Ok(()) => {
            pending_injections.commit();
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
