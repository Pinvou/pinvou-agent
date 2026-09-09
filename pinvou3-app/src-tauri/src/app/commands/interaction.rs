use super::prelude::*;
use crate::features::assistant::engine::TurnAdmissionMetadata;
use crate::features::assistant::engine_pool::user_display_message;

/// 手动触发上下文压缩。用户点 token 进度条 → 立即压缩当前对话历史。
/// 触发后 engine 会发 CompactionStarted / Completed / Failed 事件，
/// 通过 chat:compaction 系列 event 通知前端。
#[tauri::command]
pub async fn compact_now(
    session_id: Option<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let sid = require_active_sid(session_id, &store)?;
    pool.compact_now(&sid)
        .await
        .map_err(|e| format!("compact_now: {e:#}"))
}

// ===================== 阶段 D: Plan / YOLO 双模式 =====================

/// 查询当前 session 的 mode 状态（前端启动 / 切换 session 时拉一次）。
#[derive(Serialize)]
pub struct SessionModeStateView {
    #[serde(flatten)]
    state: SessionModeState,
    /// Whether the product-level multi-agent mode is available for this session.
    /// This is resolved by SessionPolicy instead of inferred by the frontend.
    multi_agent_available: bool,
}

#[tauri::command]
pub async fn get_mode_state(
    session_id: String,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<SessionModeStateView, String> {
    Ok(SessionModeStateView {
        state: store.mode_state(&session_id),
        multi_agent_available: pool.multi_agent_mode_available(&session_id),
    })
}

/// code 会话权限模式的全局偏好：新建 code 会话的默认 mode（`last_mode`，
/// None = 首次使用 → Plan）与 yolo 一次性确认标志。前端 code 页启动/进草稿时拉取。
#[tauri::command]
pub async fn get_code_permission_prefs(
    store: State<'_, SessionStore>,
) -> Result<crate::platform::prefs::CodePermissionPrefs, String> {
    Ok(store.code_permission_prefs())
}

/// 用户在 code 页确认卡【确认】切 yolo：全局记住，之后任何会话 Plan↔yolo
/// 切换不再弹卡。确认是 UI 层语义（与 VS Code 同款），后端不在
/// `exit_plan_to_yolo` 强制门控。
#[tauri::command]
pub async fn confirm_code_yolo(
    store: State<'_, SessionStore>,
) -> Result<crate::platform::prefs::CodePermissionPrefs, String> {
    store.confirm_code_yolo()
}

/// The per-lane (work/code) global default modes. Fetched at startup / on
/// entering the draft state to drive the draft-state chip; None = the lane
/// was never explicitly chosen (defaults code→plan, work→yolo).
#[tauri::command]
pub async fn get_mode_defaults(
    store: State<'_, SessionStore>,
) -> Result<crate::core::mode_state::ModeDefaultsView, String> {
    Ok(store.mode_defaults())
}

/// Explicit draft-state mode switch: writes the matching lane's global
/// default (new sessions follow it as their default). Switches inside
/// already-materialized sessions do not go through here
/// (`set_plan_mode_next`/`exit_plan_to_yolo` write only per-session
/// records) — two-lane semantics: drafts write the global, sessions write
/// their own record.
#[tauri::command]
pub async fn set_mode_default(
    lane: String,
    mode: SerializableMode,
    store: State<'_, SessionStore>,
) -> Result<crate::core::mode_state::ModeDefaultsView, String> {
    let lane = crate::core::mode_state::ModeLane::parse(&lane)?;
    store.set_mode_default(lane, mode);
    Ok(store.mode_defaults())
}

// ===================== 卡片池: 专家面具 =====================

/// 用户在 composer chip 选 Plan：设 mode=Plan。
/// 下一条 chat 消息带 mode=Plan 发送，底座自动切只读工具集 + ReadOnly sandbox。
#[tauri::command]
pub async fn set_plan_mode_next(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<SessionModeState, String> {
    store
        .set_mode(&session_id, SerializableMode::Plan)
        .map_err(|error| format!("set_plan_mode_next({session_id}): {error:#}"))?;
    Ok(store.mode_state(&session_id))
}

/// 用户在 composer chip 选 Yolo（从 Plan 退回）：mode 切 Yolo。
/// 对话历史天然保留，AI 在 YOLO 下能看到之前讨论的 context。
#[tauri::command]
pub async fn exit_plan_to_yolo(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<SessionModeState, String> {
    store
        .set_mode(&session_id, SerializableMode::Yolo)
        .map_err(|error| format!("exit_plan_to_yolo({session_id}): {error:#}"))?;
    Ok(store.mode_state(&session_id))
}

// ===================== 多智能体模式开关（ADR-0006） =====================

/// 模型列表下方的会话级开关。开启：装配专家名册，并让下一次发送按多智能体
/// 资源边界重建引擎；关闭：让下一次发送恢复普通对话的底座资源配置。切换时
/// 回收空闲旧引擎，避免旧 hook / 深度 / 并发配置泄漏到新模式；正在生成时拒绝
/// 切换。工具面不随开关变化——与主线完全一致：`workflow` 保持可用（委派提醒
/// 不教学不推荐），裸 `agent` 本就对所有会话可用。
#[tauri::command]
pub async fn set_multi_agent_mode(
    session_id: String,
    enabled: bool,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<SessionModeState, String> {
    // 持久化开关并回收旧引擎之前先过两道门：id 形状校验（避免非法 id
    // 逃逸会话边界）+ 会话确实存在（防 IPC 直调给不存在的 id 造孤儿状态）。
    crate::features::sessions::validate_session_id(&session_id)
        .map_err(|error| format!("set_multi_agent_mode: {error:#}"))?;
    store
        .load(&session_id)
        .map_err(|error| format!("set_multi_agent_mode({session_id}): 会话不存在: {error:#}"))?;
    // EngineConfig 的深度、并发、准入上限不能完整热切；同时 SendMessage 会覆盖
    // engine 级 hook。内存名册装配、状态持久化与旧引擎回收必须和发送共用同一个
    // lifecycle + turn gate，避免切换/发送竞态。关闭也必须回收，否则普通对话
    // 会继续背着多智能体限制。生成中 reserve 会直接拒绝，不会打断当前回复。
    pool.reconfigure_multi_agent_mode(&session_id, enabled)
        .await
        .map_err(|error| format!("set_multi_agent_mode({session_id}): {error:#}"))?;
    Ok(store.mode_state(&session_id))
}

/// `accept_plan` 切 Yolo 后注入的执行指令文本。抽成函数供单测钉契约:
/// 必须裹住方案全文 + 带明确"立即执行"信号,否则切了 Yolo 但 AI 收到空指令不知道干嘛。
pub(super) fn accept_plan_instruction(plan_markdown: &str) -> String {
    format!("用户已批准方案,立即开始执行。方案:\n\n{plan_markdown}")
}

/// 「未成活」快照作废（发送前任何早退共用，按 id 精确删除；残留快照会抢占
/// 重试时同号 turn 的 first-wins 对齐锚——评审 M5）。
async fn drop_unsent_turn_checkpoint(
    ledger: Option<std::path::PathBuf>,
    snapshot_id: Option<String>,
    session_id: &str,
    caller: &str,
) {
    if let (Some(ledger), Some(snapshot_id)) = (ledger, snapshot_id) {
        let sid = session_id.to_string();
        let caller = caller.to_string();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) =
                crate::features::code_checkpoints::drop_checkpoint(&ledger, &snapshot_id)
            {
                log::warn!(
                    "[pinvou3][{caller}] drop unsent-turn checkpoint failed sid={sid}: {error:#}"
                );
            }
        })
        .await;
    }
}

/// 用户点 plan_card [✅ 就这么干]：接受 plan，切 YOLO 执行(对齐底座 accept-yolo)。
/// 流程：
///   1. 设 mode=Yolo
///   2. 用 plan_markdown 作为指令前缀发一条 user message 触发执行(底座共享 PlanState 仍在)
/// 前端在调用前应在消息流追加 user 气泡显示「✅ 就这么干」让用户感知。
#[tauri::command]
pub async fn accept_plan(
    session_id: String,
    plan_id: String,
    plan_markdown: String,
    display_message: Option<String>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<SessionModeState, String> {
    let mut reservation = pool
        .reserve_turn(&session_id)
        .map_err(|error| format!("reserve accept_plan turn: {error:#}"))?;
    // 与 chat.rs 同款的 turn 快照前奏：accept_plan 发送真实用户消息（
    // is_user_turn_prompt 计数口径一致），切 YOLO 执行恰是最高风险的一轮——
    // 缺快照会让其编辑只能连同前一轮一起回退（评审 M6）。失败/超预算如实记
    // 日志不阻断 turn（设计 §5 降级语义）；发送失败按 id 作废「未成活」快照。
    let mut created_snapshot_id: Option<String> = None;
    let mut checkpoint_ledger_root = None;
    if store.is_code_session(&session_id) {
        let roots = store
            .session_roots(&session_id)
            .map_err(|error| format!("解析会话根失败: {error:#}"))?;
        checkpoint_ledger_root = Some(roots.ledger.clone());
        let store_count = store.inner().clone();
        let sid_count = session_id.clone();
        let checkpoint_ledger = roots.ledger.clone();
        let checkpoint_execution = roots.execution.clone();
        let label = "✅ 就这么干".to_string();
        let snapshot = tauri::async_runtime::spawn_blocking(move || {
            if !crate::features::code_checkpoints::execution_root_within_snapshot_budget(
                &checkpoint_execution,
                &checkpoint_ledger,
            ) {
                return Ok(None);
            }
            let turn_number = store_count
                .load(&sid_count)
                .map(|session| {
                    crate::features::code_checkpoints::count_user_turns(&session.messages) + 1
                })
                .ok();
            crate::features::code_checkpoints::create_checkpoint(
                &checkpoint_ledger,
                &checkpoint_execution,
                turn_number,
                crate::features::code_checkpoints::CheckpointKind::Turn,
                &label,
            )
            .map(Some)
        })
        .await;
        created_snapshot_id = match snapshot {
            Ok(Ok(Some(meta))) => Some(meta.id),
            Ok(Ok(None)) => {
                log::info!(
                    "[pinvou3][accept_plan] checkpoint skipped sid={session_id}: execution root over snapshot size budget or not fully readable (see earlier checkpoint estimate log)"
                );
                None
            }
            Ok(Err(error)) => {
                // git alias 冲突：与 chat.rs 同款 error 级显式上报（评审 M1）。
                if format!("{error:#}").contains("alias") {
                    log::error!(
                        "[pinvou3][accept_plan] checkpoint failed (git alias conflict, session has no rewind entries) sid={session_id}: {error:#}"
                    );
                } else {
                    log::warn!(
                        "[pinvou3][accept_plan] checkpoint failed sid={session_id}: {error:#}"
                    );
                }
                None
            }
            Err(error) => {
                log::warn!(
                    "[pinvou3][accept_plan] checkpoint task failed sid={session_id}: {error}"
                );
                None
            }
        };
    }
    let plan_claim = match store.claim_pending_plan(&session_id, &plan_id) {
        Ok(claim) => claim,
        Err(error) => {
            drop_unsent_turn_checkpoint(
                checkpoint_ledger_root.clone(),
                created_snapshot_id.clone(),
                &session_id,
                "accept_plan",
            )
            .await;
            return Err(format!("accept_plan({session_id}): {error:#}"));
        }
    };
    let accepted_mode_state = plan_claim.accepted_state().clone();
    if let Err(error) = reservation.set_admission_metadata(TurnAdmissionMetadata::accept_plan(
        plan_id.clone(),
        accepted_mode_state.clone(),
    )) {
        drop_unsent_turn_checkpoint(
            checkpoint_ledger_root.clone(),
            created_snapshot_id.clone(),
            &session_id,
            "accept_plan",
        )
        .await;
        return Err(format!("prepare accept_plan admission: {error:#}"));
    }
    let prepared_delegation = super::multiagent::prepare_delegation_turn(
        pool.inner(),
        &session_id,
        accepted_mode_state.multi_agent,
        &plan_markdown,
        accept_plan_instruction(&plan_markdown),
    );
    let display_content = display_message
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| "✅ 就这么干".to_string());
    if let Err(error) = pool
        .send_reserved_user_message(
            &session_id,
            prepared_delegation.content,
            user_display_message(display_content),
            SerializableMode::Yolo.to_app_mode(),
            false,
            prepared_delegation.expert_snapshot,
            reservation,
        )
        .await
    {
        // 发送失败：作废「未成活」快照（与 chat.rs 同款，按 id 精确删除）。
        drop_unsent_turn_checkpoint(
            checkpoint_ledger_root,
            created_snapshot_id,
            &session_id,
            "accept_plan",
        )
        .await;
        let rollback = plan_claim.rollback();
        return Err(match rollback {
            Ok(()) => format!("accept_plan send_user_message: {error:#}"),
            Err(rollback_error) => format!(
                "accept_plan send_user_message: {error:#}; restore plan claim failed: {rollback_error:#}"
            ),
        });
    }
    plan_claim.commit();
    Ok(accepted_mode_state)
}

/// 超级权限开关：当前用户能否跑 sudo 免密。
/// 源真相 = `/etc/sudoers.d/pinvou3` 是否存在；前端启动时调一次同步 UI 状态。
#[tauri::command]
pub async fn get_super_permission_status() -> Result<bool, String> {
    Ok(crate::platform::super_permission::is_enabled())
}

/// 切换超级权限。开启时 pkexec 弹系统密码框写 sudoers，关闭时 pkexec 删文件。
/// 切换后同步当前 session 让新 system prompt 立即生效（注入/抹掉 sudo 引导段）。
/// 返回真实生效状态（pkexec 失败/取消时不会变）。
#[tauri::command]
pub async fn set_super_permission(
    enabled: bool,
    pool: State<'_, EnginePool>,
) -> Result<bool, String> {
    if enabled {
        crate::platform::super_permission::enable()?;
    } else {
        crate::platform::super_permission::disable()?;
    }
    // 多 session 并发:重写所有已起 engine 的 session 专属 instructions(含新 sudo 引导块),
    // engine 下个 turn rehydrate 时从 disk 重读 → 「下次 turn 生效」。低频操作,不为即时
    // 生效去 SyncSession 打断在跑的 turn。未起的 session 首次 spawn 时自然带上新引导。
    pool.refresh_all_instructions().await;
    // sudo hard-deny rules are added/removed with the toggle state (deny sudo
    // while off / allow while on): recompute and hot-refresh the execpolicy
    // ruleset of every running engine so it applies from the next turn. Same
    // channel as the connector/skill toggles.
    pool.refresh_permission_rulesets().await;
    Ok(crate::platform::super_permission::is_enabled())
}

/// 读 pinvou3 内置 skill 的 body(去掉 frontmatter)。
/// 用途:前端 autoTriggerPinvouReview 把完整 SKILL.md 内容塞进 user message,
/// 不依赖本地 Qwen3.6 主动 read_file —— 弱模型不会主动用 progressive disclosure。
/// 设计依据:docs/Pinvou-品悟设计.md §10.5 (即将补)
#[tauri::command]
pub async fn read_skill_body(name: String) -> Result<String, String> {
    use crate::platform::paths;
    let safe_name: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if safe_name != name || safe_name.is_empty() {
        return Err(format!("invalid skill name: {name}"));
    }
    // 市场技能按包聚合（bundles/<pkg>/skills/）优先，旧扁平布局回退由
    // find_skill_dir 内置（迁移过渡容错，下个版本删除回退）。
    let path = crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
        .find_skill_dir(&safe_name)
        .unwrap_or_else(|| paths::bundle_skills_dir().join(&safe_name))
        .join("SKILL.md");
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("read SKILL.md ({}): {e}", path.display()))?;
    // 剥 frontmatter ---\n...\n---\n
    let body = if let Some(rest) = content.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            rest[end + 5..].trim_start().to_string()
        } else if let Some(end) = rest.find("\n---") {
            rest[end + 4..].trim_start().to_string()
        } else {
            content
        }
    } else {
        content
    };
    Ok(body)
}

// 修法 D 删除了 revise_plan 命令.
// 用户点 [✏️ 改改] 时前端走 CodeWhale 底座做法:不切 phase, 仅 input 预填"修订方案:"前缀.
// phase 保持 Ready, 下一条 chat 触发的 Ready reminder 已包含"用户发新消息=隐式修订"语义.

/// 用户点 plan_card [🚪 算了]：放弃这个方案,但**留在当前模式**(Plan 不踢回 Yolo)。
/// "算了"= 这个方案不要了,不等于退出规划态;要换模式用户自己点 chip。
/// 与 accept_plan(切 Yolo 执行) / exit_plan_to_yolo(切 Yolo 直接干) 区别:discard 只关卡片、不动 mode。
#[tauri::command]
pub async fn discard_plan(
    session_id: String,
    plan_id: String,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<SessionModeState, String> {
    // 不动 mode——放弃方案 ≠ 退出 Plan;仅回传当前状态供前端刷新卡片。
    let mode_state = store
        .discard_pending_plan(&session_id, &plan_id)
        .map_err(|error| format!("discard_plan({session_id}): {error:#}"))?;
    let payload = serde_json::json!({
        "session_id": session_id,
        "plan_id": plan_id,
        "action": "discard_plan",
        "mode_state": mode_state,
    });
    let _ = app.emit("chat:plan_resolved", payload.clone());
    crate::features::remote_control::forward_app_event(&app, "chat:plan_resolved", payload);
    Ok(mode_state)
}

// ===================== request_user_input 工具气泡 =====================

/// 前端选择气泡点击后调用：把用户选择回传给 engine,解锁 await_user_input。
/// answers 数组里每项 { id, label, value } 对应底座 `UserInputAnswer`。
#[tauri::command]
pub async fn submit_user_input(
    tool_call_id: String,
    answers: Vec<UserInputAnswer>,
    session_id: Option<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let sid = require_active_sid(session_id, &store)?;
    let response = UserInputResponse { answers };
    pool.submit_user_input(&sid, tool_call_id, response)
        .await
        .map_err(|e| format!("submit_user_input: {e:#}"))
}

/// 前端 ✕ 按钮 / 切换 session 时调用：取消 request_user_input。
/// engine 把工具结果置为 "User input cancelled" error,LLM 收到后会继续 turn。
#[tauri::command]
pub async fn cancel_user_input(
    tool_call_id: String,
    session_id: Option<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let sid = require_active_sid(session_id, &store)?;
    pool.cancel_user_input(&sid, tool_call_id)
        .await
        .map_err(|e| format!("cancel_user_input: {e:#}"))
}

/// 会话当前的挂起输入请求与 turn 状态。
///
/// 代码页（CodexAcpView）的会话 lane 随组件卸载销毁，`chat:user_input_required`
/// 事件不重发；remount 加载会话时调本命令还原确认卡并恢复 busy 展示。
#[derive(serde::Serialize)]
pub struct PendingUserInputState {
    pub busy: bool,
    pub pending: Vec<crate::features::assistant::pending_user_input::PendingUserInput>,
}

#[tauri::command]
pub async fn get_pending_user_inputs(
    session_id: String,
    pool: State<'_, EnginePool>,
) -> Result<PendingUserInputState, String> {
    Ok(PendingUserInputState {
        busy: pool.is_turn_active(&session_id),
        pending: crate::features::assistant::pending_user_input::list(&session_id),
    })
}

#[tauri::command]
pub async fn restart_engine(
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    // 多 session 并发:重启 = evict 当前 active session 的 engine(取消在跑 turn +
    // Shutdown + abort forwarder),下次 chat 时 EnginePool 重新 spawn 干净的并从磁盘
    // rehydrate 历史。也是 engine-busy 卡死时的恢复路径。
    if let Some(sid) = store.active_id() {
        pool.evict(&sid).await;
    }
    Ok(())
}

// ===================== Pinvou v4 召唤式检阅 =====================

/// Boss 主动召唤 Pinvou 检阅当前 session 的工作（设计 `docs/品悟v4-常驻检阅助手设计.md`）。
/// 取该 session 全部 messages → 投影/全喂 → 单次独立 LLM 审查 → 返回 personas/issues。
/// 纯召唤、不替 Boss 决策；自动触发已彻底移除。
#[tauri::command]
pub async fn summon_pinvou(
    session_id: Option<String>,
    focus: Option<String>,
    mode: Option<String>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<crate::features::review::PinvouReview, String> {
    let sid = require_active_sid(session_id, &store)?;
    let session = store
        .load(&sid)
        .map_err(|e| format!("summon_pinvou load({sid}): {e:#}"))?;
    let bridge = pool
        .fresh_bridge_for(&sid)
        .await
        .map_err(|e| format!("summon_pinvou prepare bridge({sid}): {e:#}"))?;
    let workspace = store
        .ledger_root(&sid)
        .map_err(|error| format!("resolve ledger root for {sid}: {error:#}"))?;
    crate::features::review::summon(
        &bridge,
        &session.messages,
        &workspace,
        &sid,
        focus.as_deref(),
        mode.as_deref(),
    )
    .await
    .map_err(|e| format!("summon_pinvou: {e:#}"))
}
