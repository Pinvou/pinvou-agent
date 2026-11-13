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
    /// Swarm-regime scope (scheduled sessions excluded) so the toggle hides
    /// where the engine would never honor the lifted caps.
    multi_agent_available: bool,
    /// The execution mode an `accept_plan` on this state would resume into
    /// (`claim_pending_plan`'s restore: pre_plan_mode, fallback Auto —
    /// including the rebooted-Plan fallback). Review zhuowp 5780927743
    /// third: the web acceptPlan face previously gated only on the CURRENT
    /// mode (`mode === "plan"` passed) and a card entered from Auto then
    /// executed under Auto, a posture the web face cannot serve (no
    /// approval-card decision surface). The gate now reads this projection
    /// and refuses such accepts with the same face as its Auto rejection.
    post_accept_mode: SerializableMode,
}

#[tauri::command]
pub async fn get_mode_state(
    session_id: String,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<SessionModeStateView, String> {
    let state = store.mode_state(&session_id);
    Ok(SessionModeStateView {
        post_accept_mode: state.post_accept_mode(),
        state,
        multi_agent_available: pool.swarm_mode_available(&session_id),
    })
}

/// code 会话权限模式的全局偏好：新建 code 会话的默认 mode（`last_mode`，
/// None = first use → Plan; an explicit Auto persists via the `last_mode_auto` companion flag and folds
/// back into `last_mode` here, see `code_permission_prefs_view`) plus the yolo one-time confirmation
/// flag. The frontend code page fetches it at startup / when entering the draft.
#[tauri::command]
pub async fn get_code_permission_prefs(
    store: State<'_, SessionStore>,
) -> Result<crate::platform::prefs::CodePermissionPrefs, String> {
    Ok(store.code_permission_prefs_view())
}

/// 用户在 code 页确认卡【确认】切 yolo：全局记住，之后任何会话 Plan↔yolo
/// 切换不再弹卡。确认是 UI 层语义（与 VS Code 同款），后端不在
/// `exit_plan_to_yolo` 强制门控。
#[tauri::command]
pub async fn confirm_code_yolo(
    store: State<'_, SessionStore>,
) -> Result<crate::platform::prefs::CodePermissionPrefs, String> {
    // Round-54 audit A5: the settings transaction holds the cross-process
    // settings.json.lock across keychain I/O (a CLI `models` command parked
    // at a keychain prompt can hold it for minutes) — join it on a blocking
    // worker instead of parking this runtime worker, like the settings
    // family.
    let store = store.inner().clone();
    super::settings::join_blocking("confirm_code_yolo", move || store.confirm_code_yolo()).await
}

/// The per-lane (work/code) global default modes. Fetched at startup / on
/// entering the draft state to drive the draft-state chip; None = the lane
/// was never explicitly chosen (defaults code→plan, work→Auto for new
/// unbound plain sessions — see `resolved_default_mode`).
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
    // Round-54 audit A5: same blocking-worker join as confirm_code_yolo —
    // the draft-state switch persists through the settings transaction.
    let store = store.inner().clone();
    super::settings::join_blocking("set_mode_default", move || {
        // Persistence failure propagates (review JensenChen28 5780647695):
        // returning Ok(stale defaults) made both bridges file the old
        // default while drawing the draft chip as the target — the switch
        // was never durable and a session materialized later under the OLD
        // default. The frontend catch keeps the authoritative state and
        // surfaces the error instead of drawing the target.
        store.set_mode_default(lane, mode)?;
        Ok(store.mode_defaults())
    })
    .await
}

// ===================== 卡片池: 专家面具 =====================

/// 用户在 composer chip 选 Plan：设 mode=Plan。
/// 下一条 chat 消息带 mode=Plan 发送，底座自动切只读工具集 + ReadOnly sandbox。
/// Funneled through the same path as `set_session_mode`: after the write, both go through the same
/// "posture hot-sync + chat:mode_changed" publication sequence (`publish_session_mode_change`), so mode
/// chips on all surfaces converge immediately. The target session first passes the existence gate
/// (`ensure_mode_target_session`) — preventing IPC direct calls from minting a mode entry + sidecar for an arbitrary id.
#[tauri::command]
pub async fn set_plan_mode_next(
    session_id: String,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    app: AppHandle,
) -> Result<SessionModeState, String> {
    ensure_mode_target_session("set_plan_mode_next", &session_id, &store)?;
    let store = store.inner().clone();
    // The mode write RMWs the sidecar files (fsync-heavy) under a std Mutex
    // — join it on a blocking worker instead of parking this runtime
    // worker, like accept_plan's commit and the settings family.
    let sid = session_id.clone();
    let worker_store = store.clone();
    super::settings::join_blocking("set_plan_mode_next", move || {
        worker_store
            .set_mode(&sid, SerializableMode::Plan)
            .map_err(|error| format!("set_plan_mode_next({sid}): {error:#}"))
    })
    .await?;
    publish_session_mode_change(&session_id, &store, &pool, &app).await
}

/// The user picks Yolo in the composer chip (falling back from Plan): the mode switches to Yolo (effective
/// immediately; the one-time confirmation gate lives in the frontend UI). Funneled through the same path as
/// `set_session_mode`: after the write, both go through the same "posture hot-sync + chat:mode_changed"
/// publication sequence, so mode chips converge immediately. Chat history is naturally preserved — the AI in
/// YOLO can see the previously discussed context. The target session first passes the existence gate (same as `set_plan_mode_next`).
#[tauri::command]
pub async fn exit_plan_to_yolo(
    session_id: String,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    app: AppHandle,
) -> Result<SessionModeState, String> {
    ensure_mode_target_session("exit_plan_to_yolo", &session_id, &store)?;
    let store = store.inner().clone();
    // Same blocking-worker join as set_plan_mode_next: the sidecar RMW is
    // file I/O under a std Mutex and need not park this runtime worker.
    let sid = session_id.clone();
    let worker_store = store.clone();
    super::settings::join_blocking("exit_plan_to_yolo", move || {
        worker_store
            .set_mode(&sid, SerializableMode::Yolo)
            .map_err(|error| format!("exit_plan_to_yolo({sid}): {error:#}"))
    })
    .await?;
    publish_session_mode_change(&session_id, &store, &pool, &app).await
}

// ===================== 多智能体模式开关（ADR-0006） =====================

/// 模型列表下方的会话级开关。开启：装配专家名册，并让下一次发送按多智能体
/// 资源边界重建引擎；关闭：让下一次发送恢复普通对话的底座资源配置。切换时
/// 回收空闲旧引擎，避免旧 hook / 深度 / 并发配置泄漏到新模式；正在生成时拒绝
/// 切换。工具面不随开关变化——与主线完全一致：`workflow` 保持可用（蜂群契约
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
    // Aux = scoped Q&A engine (ADR-0006 round-32 minor 20, upgraded by
    // ADR-0024 to at-most-one-scoped-read-tool): explicit rejection, same
    // stance as the sibling metadata commands.
    if crate::features::sessions::is_aux_session_id(&session_id) {
        return Err(
            "set_multi_agent_mode: auxiliary conversations do not take multi-agent mode".into(),
        );
    }
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
    app: AppHandle,
) -> Result<SessionModeState, String> {
    let mut reservation = pool
        .reserve_turn(&session_id)
        .map_err(|error| format!("reserve accept_plan turn: {error:#}"))?;
    // 与 chat.rs 同款的 turn 快照前奏：accept_plan 发送真实用户消息（
    // is_user_turn_prompt 计数口径一致），切 YOLO 执行恰是最高风险的一轮——
    // 缺快照会让其编辑只能连同前一轮一起回退（评审 M6）。失败/超预算如实记
    // 日志不阻断 turn（设计 §5 降级语义）；发送失败按 id 作废「未成活」快照。
    // Computed BEFORE the snapshot so the checkpoint label quotes this turn's
    // actual display content; the literal below is only the no-display-message
    // fallback. That fallback is fixed zh persisted as ledger fidelity data
    // (historical fidelity with the pre-cleanup surface) and printed verbatim
    // by the CLI — localizing it is a consumer-side decision, not a label-side
    // one (PR #602).
    let display_content = display_message
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| "✅ 就这么干".to_string());
    let mut created_snapshot_id: Option<String> = None;
    let mut checkpoint_ledger_root = None;
    if store.is_code_session(&session_id) {
        let roots = store
            .session_roots(&session_id)
            .map_err(|error| format!("解析会话根失败: {error:#}"))?;
        checkpoint_ledger_root = Some(roots.ledger.clone());
        created_snapshot_id = super::checkpoints::create_turn_checkpoint(
            store.inner(),
            &session_id,
            roots.ledger,
            roots.execution,
            display_content.clone(),
            "accept_plan",
        )
        .await;
    }
    let plan_claim = match store.claim_pending_plan(&session_id, &plan_id) {
        Ok(claim) => claim,
        Err(error) => {
            super::checkpoints::drop_unsent_turn_checkpoint(
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
        super::checkpoints::drop_unsent_turn_checkpoint(
            checkpoint_ledger_root.clone(),
            created_snapshot_id.clone(),
            &session_id,
            "accept_plan",
        )
        .await;
        return Err(format!("prepare accept_plan admission: {error:#}"));
    }
    // 候选专家只从计划正文（plan_markdown）匹配，不加 accept 前缀——与快照同源；
    // 实际发送内容仍是包装后的 accept 指令。蜂群契约在 spawn 级 instructions，
    // 这里不再改写消息内容。
    let prepared_delegation = super::multiagent::prepare_delegation_turn(
        pool.inner(),
        &session_id,
        accepted_mode_state.multi_agent,
        accept_plan_instruction(&plan_markdown),
        super::multiagent::MatchSource(&plan_markdown),
    );
    crate::features::assistant::timing::start_turn(&session_id);
    // Tri-mode safety (S-1): execution after plan approval runs in the mode
    // the session was in before entering Plan (fallback Auto) — the claim
    // already flipped the store; carry its posture on the wire.
    let accepted_mode = accepted_mode_state.mode;
    let approval =
        crate::features::assistant::platform::bridge::TurnApprovalPosture::from_serialized_mode(
            accepted_mode,
        );
    if let Err(error) = pool
        .send_reserved_user_message(
            &session_id,
            prepared_delegation.content,
            user_display_message(display_content),
            accepted_mode.to_app_mode(),
            approval,
            false,
            prepared_delegation.expert_snapshot,
            prepared_delegation.expert_candidates,
            reservation,
        )
        .await
    {
        crate::features::assistant::timing::finish_turn(
            &session_id,
            "send_error",
            Some(&format!("{error:#}")),
        );
        // 发送失败：作废「未成活」快照（与 chat.rs 同款，按 id 精确删除）。
        super::checkpoints::drop_unsent_turn_checkpoint(
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
    // Round-54 audit A5, round-1 correction: commit runs
    // finish_pending_plan_claim plus the accepted-mode persist — a
    // `_session_mode_states.json` sidecar RMW under a std Mutex, NOT the
    // settings transaction (the audit's "plan accept" label pointed here,
    // but this file's settings-transaction callers are confirm_code_yolo
    // and set_mode_default above). Still joined on a blocking worker: the
    // sidecar RMW is file I/O and need not park this runtime worker.
    // Log-only on a failed join: the turn is already submitted, and
    // persist_accepted_yolo_mode's own write-failure arm has the same
    // tolerance.
    let joined = tauri::async_runtime::spawn_blocking(move || plan_claim.commit()).await;
    if let Err(error) = joined {
        eprintln!("[sessions] accept_plan commit join failed: {error}");
    }
    // Tri-mode convergence: the claim flipped the store to the accepted mode
    // (pre-Plan posture, fallback Auto) — publish the SAME posture sync +
    // `chat:mode_changed` sequence as the mode-chip commands so every client
    // converges on the claimed mode (this is the point where
    // `set_plan_mode_next`'s pending Plan mode becomes the session mode).
    // The turn is already submitted, so a sync failure is log-only (same
    // tolerance as the commit join above), never a command error.
    if let Err(error) = publish_session_mode_change(&session_id, &store, &pool, &app).await {
        eprintln!("[sessions] accept_plan posture publish failed: {error}");
    }
    Ok(accepted_mode_state)
}

/// 超级权限开关：当前用户能否跑 sudo 免密。
/// 源真相 = `/etc/sudoers.d/pinvou3` 是否存在；前端启动时调一次同步 UI 状态。
#[tauri::command]
pub async fn get_super_permission_status() -> Result<bool, String> {
    Ok(crate::platform::super_permission::is_enabled())
}

/// 切换超级权限。开启时 pkexec 弹系统密码框写 sudoers，关闭时 pkexec 删文件。
/// After toggling, rebuild and hot-refresh the execpolicy ruleset of every
/// running engine (sudo hard-deny is added/removed with the toggle); the
/// session's sudo guidance state is injected live by the per-turn reminder
/// (the static prompt renders only at spawn and is not hot-refreshed).
/// Returns the real effective state (unchanged when pkexec fails/is cancelled).
///
/// Holds the process-wide [`crate::platform::super_permission::TOGGLE_LOCK`]
/// for the whole sequence: pkexec write/remove → `refresh_permission_rulesets`
/// rebuild + broadcast (the rebuild re-reads the sudo state from disk inside
/// `safety_deny_rules`) → the effective-state read-back for the return
/// value. All of it runs serialized as one unit, so concurrent toggles no
/// longer interleave and the sudo hard-deny ruleset is never rebuilt from a
/// snapshot older than the toggle itself (the toggle-vs-toggle
/// stale-snapshot window registered in safety_deny_rules is closed;
/// the connector/marketplace-vs-toggle refresh windows remain registered
/// there as transient residues — this lock does not cover them).
/// Toggling is a low-frequency user action, so holding the lock across the
/// slow pkexec call is acceptable; the lock is held only here, the guard is
/// scoped to the function, and an early pkexec error return releases it
/// automatically without blocking other Tauri commands. The pkexec wait
/// itself runs on the blocking pool (it blocks on the polkit dialog for as
/// long as the user takes), so it does not park an async worker thread.
#[tauri::command]
pub async fn set_super_permission(
    enabled: bool,
    pool: State<'_, EnginePool>,
) -> Result<bool, String> {
    let _toggle_guard = crate::platform::super_permission::TOGGLE_LOCK.lock().await;
    let toggle_result = if enabled {
        tokio::task::spawn_blocking(crate::platform::super_permission::enable)
    } else {
        tokio::task::spawn_blocking(crate::platform::super_permission::disable)
    };
    toggle_result
        .await
        .map_err(|e| format!("super-permission toggle task failed: {e}"))??;
    // refresh_all_instructions is a deliberate no-op today (engine_pool.rs):
    // the static prompt renders once at engine spawn and the sudo guidance
    // state is not part of the static segment — it is injected live by the
    // per-turn reminder (super_permission::turn_reminder), so the toggle takes
    // effect on the next turn. Keep this call as a hook point: if instructions
    // hot-refresh is ever restored, the toggle sequence is the right trigger.
    pool.refresh_all_instructions().await;
    // sudo hard-deny rules are added/removed with the toggle state (deny sudo
    // while off / allow while on): recompute and hot-refresh the execpolicy
    // ruleset of every running engine so it applies from the next turn. Same
    // channel as the connector/skill toggles.
    pool.refresh_permission_rulesets()
        .await
        .map_err(|error| format!("refresh_permission_rulesets: {error:#}"))?;
    Ok(crate::platform::super_permission::is_enabled())
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

// ===================== Pinvou v4 召唤式检阅 =====================

/// Boss 主动召唤 Pinvou 检阅当前 session 的工作（设计 `docs/品悟v4-召唤式评审设计.md` §1）。
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

// ===================== Interactive approval surface (tri-mode safety) =====================

/// The frontend's approval-card decision callback: allow (this once only) / allow_for_session (same-family
/// operations in this session are silently allowed, grouping_key scope) / deny (this session remembers the
/// denial for this exact operation, exact approval_key scope). Validate the decision first (registry removal is
/// irreversible; an invalid value must Err as-is); a valid decision first removes the pending registration (the
/// card is settled and the engine-side approval id dies with the call, so no residue may remain), and the
/// grant/denial caches are written after the engine call succeeds. Once the decision settles, broadcast
/// `chat:approval_resolved` (M5 convergence surface): the decision may come from a second window or a remote
/// surface, so same-id cards on other surfaces must settle in place; the frontend applies it only to pending cards (idempotent), the originator's own card updates optimistically (the event echo never overwrites a decided card), and the payload carries no tool input (sensitive-surface discipline).
#[tauri::command]
pub async fn decide_approval(
    session_id: String,
    approval_id: String,
    decision: String,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<(), String> {
    use crate::features::assistant::pending_approvals;
    // Validate the decision first: registry removal is irreversible, and an invalid decision discovered only
    // after removal would leave the card gone with no engine-side responder (stranded until the turn ends).
    // Validating first Errs as-is with no side effects.
    if !matches!(decision.as_str(), "allow" | "allow_for_session" | "deny") {
        return Err(format!(
            "decide_approval: invalid decision \"{decision}\" (expected allow | allow_for_session | deny)"
        ));
    }
    // Then remove the registration: this retrieves the approval_key / grouping_key for the cache writes; if the
    // engine-side call fails (approval already timed out / turn already interrupted), the removed registration
    // goes back as-is — the card stays pending, this surface can retry, and clicks on other surfaces can still
    // settle it; skipping the rollback would leave a permanently dead "approval not found" card (the only
    // exception to irreversible registry removal is an engine-call failure).
    // The success path does not roll back: the card is consumed, and residue would revive a settled card.
    //
    // The restore is guarded by a double fence (review P2: a restore is allowed only within the same still-live
    // approval lifecycle) — between the removal and the await, a turn-terminal sweep / session purge /
    // concurrent decision / engine rebuild can happen; an unconditional restore would resurrect requests from a
    // closed lifecycle, and a fresh remount snapshot would still expose them: frontend tombstones cannot reach a
    // backend resurrect:
    // (a) Registry fence = the generation snapshot returned by remove_fenced; the restore goes through
    //     restore_if_generation_is — the comparison and the reinsertion share one registry critical section
    //     (review P2: the two-phase read-generation()-then-record() locking had a window where a terminal sweep
    //     could advance the generation after the comparison and before the reinsertion, so a stale restore
    //     resurrected dead entries); any registry mutation (terminal sweep/purge/concurrent decision/new card)
    //     cancels the restore. The conservative direction is right — a missed restore costs one re-trigger, a wrong restore costs reviving a dead card whose engine-side approval no longer exists.
    // (b) Engine incarnation fence = re-read engine_incarnation() before restoring (an advisory precheck; the
    //     lifecycle boundary is still adjudicated by (a)'s registry critical section): the incarnation snapshotted
    //     at removal must still be a live entry — a new engine after eviction/rebuild never answers the old approval id, and restoring would only mint a zombie card that errors on click.
    let Some((pending, registry_generation)) =
        pending_approvals::remove_fenced(&session_id, &approval_id)
    else {
        return Err(format!(
            "decide_approval: approval not found: {approval_id}"
        ));
    };
    let Some((engine, incarnation)) = pool.handle_with_incarnation(&session_id).await else {
        // Engine absent: no incarnation to compare against; only the registry fence guards.
        pending_approvals::restore_if_generation_is(&session_id, pending, registry_generation);
        return Err("session_engine_not_running".to_string());
    };
    let result = if decision == "deny" {
        engine.handle.deny_tool_call(approval_id.clone()).await
    } else {
        // "allow" | "allow_for_session" (validated above).
        engine.handle.approve_tool_call(approval_id.clone()).await
    };
    let outcome = match result {
        Ok(()) => Ok(()),
        Err(error) => {
            // Engine Errs come in two classes (the ack/settle contract; classified by
            // approval_outcome_is_unknown) — only a determinate refusal restores the registration:
            // (a) Determinate refusal (stale id / failed durable commit / never consumed by the engine / channel
            //     closed): the decision definitely did not take effect and the card is still a live pending
            //     request. The engine was present at send time: the incarnation precheck + registry fence (see
            //     above; the reinsertion's comparison and insert complete inside restore_if_generation_is's single
            //     critical section), so this surface can retry and clicks on other surfaces can still settle.
            // (b) Unknown outcome (ack + settle dual-window timeout): the decision may still commit and execute
            //     late on the engine side, and a late commit has no host reconciliation event and can no longer
            //     establish the promised session-level grant/denial memory — restoring active decision controls
            //     would promise that a possibly consumed decision can be redone. Same conservative direction as
            //     the registry fence: no registration restore (remount does not revive the card; the click's
            //     terminal state lives only in the transcript), and the error text honestly carries the engine's
            //     unknown wording for the surfaces to render. Caches and the convergence broadcast are already
            //     written/emitted only after success, so nothing new here.
            //     But registry removal only covers remount: mounted cards (chat lane / code
            //     native lane) each hold active decision controls, and without a synchronous notice the UI
            //     still invites one more click on a possibly consumed decision (every subsequent click
            //     can only fail, review P2). The unknown arm goes through the resolved broadcast channel carrying
            //     decision "unknown" (a resolved-shaped event, payload still from the
            //     single constructor): both mounted faces render the card as an undecidable unknown
            //     (resolved stays false, and a late authoritative resolution event can still reconcile in place); the
            //     determinate-refusal arm does not broadcast (the card is still a live pending request, this surface can retry, other surfaces' clicks can still settle — behavior unchanged).
            if !pending_approvals::approval_outcome_is_unknown(&error) {
                let engine_still_live =
                    pool.engine_incarnation(&session_id).await == Some(incarnation);
                if engine_still_live {
                    // Round-20 (review minor): a `false` restore means the
                    // fence cancelled the re-arming — a registry mutation
                    // raced this await (turn-terminal sweep, session purge,
                    // a concurrent decision) and the entry is gone
                    // (fail-closed, correct). But nothing told the mounted
                    // controls: their next click can only fail with
                    // "approval not found", exactly the condition the
                    // incarnation arm below broadcasts for. Emit the same
                    // resolved-shaped "unknown" so every mounted face
                    // renders undecidable; `resolved` stays false so a late
                    // authoritative resolution still reconciles in place.
                    if !pending_approvals::restore_if_generation_is(
                        &session_id,
                        pending,
                        registry_generation,
                    ) {
                        let payload = pending_approvals::approval_resolved_payload(
                            &session_id,
                            &approval_id,
                            "unknown",
                        );
                        let _ = app.emit("chat:approval_resolved", payload.clone());
                        crate::features::remote_control::forward_app_event(
                            &app,
                            "chat:approval_resolved",
                            payload,
                        );
                    }
                } else {
                    // Incarnation moved on while the decision was in flight:
                    // the registry entry cannot be restored (a rebuilt engine
                    // never answers the old approval id) and every mounted
                    // control's next click can only fail with "approval not
                    // found". Tell the mounted faces through the same
                    // resolved-shaped broadcast channel with decision
                    // "unknown" (payload from the single constructor):
                    // rendered as undecidable instead of inviting a click
                    // that cannot succeed; `resolved` stays false so a late
                    // authoritative resolution can still reconcile in place
                    // (round-18 review, detached-worker eviction residue).
                    let payload = pending_approvals::approval_resolved_payload(
                        &session_id,
                        &approval_id,
                        "unknown",
                    );
                    let _ = app.emit("chat:approval_resolved", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:approval_resolved",
                        payload,
                    );
                }
            } else {
                let payload = pending_approvals::approval_resolved_payload(
                    &session_id,
                    &approval_id,
                    "unknown",
                );
                let _ = app.emit("chat:approval_resolved", payload.clone());
                crate::features::remote_control::forward_app_event(
                    &app,
                    "chat:approval_resolved",
                    payload,
                );
            }
            return Err(format!("decide_approval({decision}): {error:#}"));
        }
    };
    // The grant/denial caches are written only **after** the engine call succeeds: if the decision did not take
    // effect engine-side (approval consumed / turn interrupted), the cache must not let a click that never
    // landed allow later calls. Session-liveness gate (review 5812242993): the session may have been deleted
    // concurrently during the engine round trip — once the purge hook (which clears workspace_generations
    // entries) has landed, a never-rebound session (origin_generation = None) passes the None==Null comparison
    // and the late cache write lands in a dead session with no one left to clear it; if an agentic id is reused
    // in-process and happens to hit a same-family key, that becomes a cross-lifecycle allow. Before writing,
    // verify the durable record still exists (NotFound = definitely deleted) and skip only when deletion is
    // certain; ambiguous states (deletion in flight with the record present / transient) keep the existing behavior.
    // Known residual gap (round-19 review minor 6, direction stated honestly): after a delete-then-**recreate with the same id**,
    // if the recreated record is persisted before this probe, the probe sees the new life's record and the old
    // click's grant silently carries into the new lifecycle via the None==Null comparison (not the "one extra
    // re-approval" an earlier comment claimed — the purge already cleared the generation entry and nothing clears this cache afterwards). Both writes share the same gate: the denial cache is directionally safe, but leaving memory in a dead session is the same kind of leak.
    let session_live = !store.durable_session_record_is_absent(&session_id);
    if outcome.is_ok() && decision == "allow_for_session" && session_live {
        // The grant is recorded on the lossy grouping_key: covers later flag variants (v0.8.37 semantics).
        // It carries the registered card's origin workspace generation: an old success arm resumed after a rebind
        // advanced the generation is refused here in-lock on mismatch, never writing the old directory's decision into new-generation memory (review 5764675531).
        pending_approvals::remember_grant(
            &session_id,
            &pending.approval_grouping_key,
            pending.request_workspace_generation,
        );
    }
    if outcome.is_ok() && decision == "deny" && session_live {
        // The denial is recorded on the exact approval_key: changed arguments require asking again (#1617 semantics).
        // Generation validation matches remember_grant.
        pending_approvals::remember_denial(
            &session_id,
            &pending.approval_key,
            pending.request_workspace_generation,
        );
    }
    // The convergence broadcast happens after the engine call succeeds (same order as the cache writes): on
    // failure nothing is broadcast — other surfaces' cards stay pending and the originator gets the error; the removed registration went back as-is, so retries can still settle.
    if outcome.is_ok() {
        let payload =
            pending_approvals::approval_resolved_payload(&session_id, &approval_id, &decision);
        let _ = app.emit("chat:approval_resolved", payload.clone());
        crate::features::remote_control::forward_app_event(&app, "chat:approval_resolved", payload);
    }
    outcome
}

/// The session's currently pending approval cards. The code page's session lane is destroyed on component
/// unmount and `chat:approval_required` is not re-sent; call this command on remount to restore the cards
/// (the same restore idea as get_pending_user_inputs).
#[tauri::command]
pub async fn get_pending_approvals(
    session_id: String,
) -> Vec<crate::features::assistant::pending_approvals::PendingApproval> {
    crate::features::assistant::pending_approvals::list(&session_id)
}

/// Unified tri-mode switch: writes the session mode (same persistence as `set_plan_mode_next`),
/// optionally flipping ask-on-guardrail at the same time; then runs the unified publication sequence
/// (`publish_session_mode_change`): when the session engine is live it hot-syncs the base safety posture
/// (`Op::ChangeMode`), otherwise the next send assembles the op from the new mode; and broadcasts `chat:mode_changed`.
/// Returns the refreshed full mode state. Plan's pending ticket is cleared by `set_mode_in_memory`'s existing
/// semantics and is not re-handled here. The target session first passes the existence gate
/// (`ensure_mode_target_session`, same id-shape + existence validation as `set_multi_agent_mode`; a plain
/// session is a valid target, no aux restriction) — preventing IPC direct calls from minting a mode entry + sidecar for an arbitrary id.
#[tauri::command]
pub async fn set_session_mode(
    session_id: String,
    mode: SerializableMode,
    ask_on_guardrail: Option<bool>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    app: AppHandle,
) -> Result<SessionModeState, String> {
    ensure_mode_target_session("set_session_mode", &session_id, &store)?;
    let store = store.inner().clone();
    // The mode + ask writes each RMW the sidecar files (fsync-heavy) under
    // a std Mutex — join them on a blocking worker instead of parking this
    // runtime worker, like accept_plan's commit and the settings family.
    let sid = session_id.clone();
    // The map_err here is `Ok` by construction: `set_mode` is the lenient
    // lane (persist failures are logged inside the store, never propagated —
    // the real failure surface of this command is the ask half below). The
    // error mapping exists so the shape matches the ask half and a future
    // strict lane cannot silently drop its error (round-20 review minor).
    super::settings::join_blocking("set_session_mode", {
        let store = store.clone();
        let sid = sid.clone();
        move || {
            store
                .set_mode(&sid, mode)
                .map_err(|error| format!("set_session_mode({sid}): {error:#}"))
        }
    })
    .await?;
    // An ask-half failure must not swallow the mode half: the mode is already persisted and effective, and the
    // publication sequence still runs (the broadcast carries the authoritative state so other surfaces/windows
    // converge to the effective mode); the ask error is logged loudly and returned — on a half-applied failure the frontend
    // converges the chip from the broadcast instead of rolling back an actually persisted mode.
    let mut ask_error = None;
    if let Some(enabled) = ask_on_guardrail {
        let store = store.clone();
        let value = sid.clone();
        if let Err(error) = super::settings::join_blocking("set_session_mode:ask", move || {
            store
                .set_ask_on_guardrail(&value, enabled)
                .map_err(|error| format!("ask_on_guardrail: {error:#}"))
        })
        .await
        {
            // stderr is persisted with the startup logs: the message carries no session id (this PR's own
            // cleartext-logging discipline; see the matching comment in `mode_state.rs`); the sid
            // goes only into the Err returned to the caller, which is the caller's own display surface.
            eprintln!(
                "[commands] set_session_mode: {error} (mode already persisted; broadcasting \
                 authoritative state)"
            );
            ask_error = Some(format!("set_session_mode({sid}): {error}"));
        }
    }
    let state = publish_session_mode_change(&session_id, &store, &pool, &app).await?;
    if let Some(error) = ask_error {
        return Err(error);
    }
    Ok(state)
}

/// Toggles the session's ask-on-guardrail on its own (whether a guardrail hit raises a card or hard-denies in
/// the Yolo full-auto tier). Sessions without a mode-state entry follow the global default (the store does not
/// materialize an entry, avoiding pinning the mode). The target session first passes the existence gate (same as `set_session_mode`):
/// the switch is only meaningful for real sessions, preventing IPC direct calls from writing a sidecar for an arbitrary id.
#[tauri::command]
pub async fn set_ask_on_guardrail(
    session_id: String,
    enabled: bool,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    app: AppHandle,
) -> Result<SessionModeState, String> {
    ensure_mode_target_session("set_ask_on_guardrail", &session_id, &store)?;
    let store = store.inner().clone();
    // Same blocking-worker join as set_session_mode: the sidecar RMW is
    // file I/O under a std Mutex and need not park this runtime worker.
    let sid = session_id.clone();
    let worker_store = store.clone();
    super::settings::join_blocking("set_ask_on_guardrail", move || {
        worker_store
            .set_ask_on_guardrail(&sid, enabled)
            .map_err(|error| format!("set_ask_on_guardrail({sid}): {error:#}"))
    })
    .await?;
    publish_session_mode_change(&session_id, &store, &pool, &app).await
}

/// Target-session gate shared by the mode write commands: id-shape validation (keeping malformed ids from
/// escaping the session boundary) + the session actually exists (preventing IPC direct calls from minting a
/// mode-state entry / `_ask_on_guardrail.json` sidecar for a nonexistent id). Mirrors the first two gates of `set_multi_agent_mode`
/// but **without** the aux restriction — a plain session is a valid mode target. Both checks must run before
/// any store/sidecar action: a failed check Errs as-is and leaves no persistence trace.
pub(super) fn ensure_mode_target_session(
    command: &str,
    session_id: &str,
    store: &SessionStore,
) -> Result<(), String> {
    crate::features::sessions::validate_session_id(session_id)
        .map_err(|error| format!("{command}: {error:#}"))?;
    // TOCTOU residual, stated honestly (round-20 review minor): a session
    // deleted concurrently between this existence gate and the caller's
    // sidecar write mints an orphan mode entry + `_ask_on_guardrail.json`
    // override. The window is bounded and the direction is conservative —
    // the boot ghost sweeps and the retention/delete purge remove orphans,
    // and an orphan entry degrades to the resolved default, never to a more
    // permissive posture. Closing it would need a re-check under the
    // sidecar lock at every write site; the mirror `set_multi_agent_mode`
    // gate carries the same shape.
    store
        .load(session_id)
        .map_err(|error| format!("{command}({session_id}): 会话不存在: {error:#}"))?;
    Ok(())
}

/// The unified publication sequence of the session mode write path (shared by `set_session_mode` /
/// `set_ask_on_guardrail` / `set_plan_mode_next` / `exit_plan_to_yolo` / `accept_plan`): when the session
/// engine is live, hot-sync the new safety posture into the base (`Op::ChangeMode` + ruleset refresh);
/// otherwise skip (the next send after the lazy spawn assembles the op from the store's mode);
/// then broadcast `chat:mode_changed` (desktop emit + remote forwarding) so mode chips on all surfaces
/// converge immediately. Returns the refreshed full mode state.
///
/// A failed live sync now FAILS the publication (zhuowp round-4 P2): the previous log-and-succeed shape
/// let the UI report the new mode while the live engine kept the previous ruleset, and ordinary sends
/// carry no ruleset replacement, so nothing repaired the discrepancy. The store's mode write has already
/// landed (the command writes first, then publishes), so the error surfaces as a visible failure against
/// an already-persisted switch: the user can retry the toggle, and the retry re-runs the sync against the
/// persisted mode and heals the engine. The broadcast fires only after the sync succeeded, so no surface
/// is told the new mode while the engine still holds the old ruleset.
async fn publish_session_mode_change(
    session_id: &str,
    store: &SessionStore,
    pool: &EnginePool,
    app: &AppHandle,
) -> Result<SessionModeState, String> {
    sync_engine_safety_posture(session_id, pool).await?;
    let state = store.mode_state(session_id);
    emit_mode_changed(app, session_id, state.mode, state.ask_on_guardrail);
    Ok(state)
}

/// When the session engine is live, hot-syncs the new safety posture into the base (`Op::ChangeMode`); when
/// absent, skips — the next send after the lazy spawn assembles the op from the store's mode.
async fn sync_engine_safety_posture(session_id: &str, pool: &EnginePool) -> Result<(), String> {
    if pool.handle_for(session_id).await.is_some() {
        pool.sync_session_safety_posture(session_id)
            .await
            .map_err(|error| format!("sync_session_safety_posture({session_id}): {error:#}"))?;
    }
    Ok(())
}

/// Dual-leg broadcast of `chat:mode_changed` (desktop emit + remote forwarding); the payload's mode is the
/// snake_case serialized value and ask_on_guardrail is the session-resolved value (no-entry sessions = global default).
fn emit_mode_changed(
    app: &AppHandle,
    session_id: &str,
    mode: SerializableMode,
    ask_on_guardrail: bool,
) {
    let payload = mode_changed_payload(session_id, mode, ask_on_guardrail);
    let _ = app.emit("chat:mode_changed", payload.clone());
    crate::features::remote_control::forward_app_event(app, "chat:mode_changed", payload);
}

/// The `chat:mode_changed` payload (single source for both broadcast legs):
/// `mode` serializes snake_case (`yolo`/`auto`/`plan`), `ask_on_guardrail`
/// is the session-resolved value. Factored out so the wire contract is unit-
/// pinnable without an AppHandle.
pub(super) fn mode_changed_payload(
    session_id: &str,
    mode: SerializableMode,
    ask_on_guardrail: bool,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "mode": mode,
        "ask_on_guardrail": ask_on_guardrail,
    })
}

#[cfg(test)]
mod get_mode_state_view_wire_tests {
    //! Round-13 review (M-5): the `post_accept_mode` IPC seam had no test
    //! that bites — the projection was store-tested and the web gate test
    //! MOCKED `get_mode_state`, so deleting the wire field (or renaming it
    //! via a serde attribute) kept the whole suite green while the web
    //! acceptPlan gate silently lost its input (`undefined !== "auto"` never
    //! fires). This pin joins the two halves: the serialized VIEW must carry
    //! the exact snake_case wire field, alongside the flattened state keys,
    //! with the claim-mirroring values.

    use super::*;

    fn view_for(state: SessionModeState) -> serde_json::Value {
        let multi_agent_available = false;
        serde_json::to_value(&SessionModeStateView {
            post_accept_mode: state.post_accept_mode(),
            state,
            multi_agent_available,
        })
        .expect("the view must serialize")
    }

    #[test]
    fn post_accept_mode_is_on_the_wire_with_exact_keys() {
        // Plan entered from nothing (rebooted-Plan fallback): the gate must
        // see `post_accept_mode === "auto"` even though `mode === "plan"`.
        let mut state = SessionModeState::default();
        state.mode = SerializableMode::Plan;
        state.pre_plan_mode = None;
        let value = view_for(state);
        assert_eq!(
            value.get("post_accept_mode"),
            Some(&serde_json::json!("auto")),
            "the web acceptPlan gate reads `post_accept_mode` off this exact snake_case wire \
             field; a rename or omission silently voids the gate"
        );
        assert_eq!(
            value.get("mode"),
            Some(&serde_json::json!("plan")),
            "the current mode stays `plan` — the gate needs BOTH fields to distinguish the \
             accept-resume posture"
        );

        // Plan entered from Yolo resumes Yolo.
        let mut state = SessionModeState::default();
        state.mode = SerializableMode::Plan;
        state.pre_plan_mode = Some(SerializableMode::Yolo);
        assert_eq!(
            view_for(state).get("post_accept_mode"),
            Some(&serde_json::json!("yolo"))
        );

        // A non-Plan state projects itself (the gate's second leg).
        let mut state = SessionModeState::default();
        state.mode = SerializableMode::Auto;
        assert_eq!(
            view_for(state).get("post_accept_mode"),
            Some(&serde_json::json!("auto"))
        );
    }

    #[test]
    fn view_keys_are_the_flattened_state_plus_the_two_extras() {
        let mut state = SessionModeState::default();
        state.mode = SerializableMode::Plan;
        let state_keys = serde_json::to_value(&state)
            .expect("state serializes")
            .as_object()
            .expect("state is an object")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let view_keys = view_for(state)
            .as_object()
            .expect("view is an object")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let mut expected = state_keys;
        expected.insert("multi_agent_available".to_string());
        expected.insert("post_accept_mode".to_string());
        assert_eq!(
            view_keys, expected,
            "the view's wire keys must be exactly the flattened state keys plus \
             `multi_agent_available` and `post_accept_mode` (snake_case) — a serde rename or \
             an un-flattened shape breaks the web/desktop readers"
        );
    }
}
