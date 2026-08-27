//! 引擎事件转发器:消费 ACP 事件流并转为前端 chat 事件。
//!
//! 从 engine.rs 抽离的 `spawn_event_forwarder` —— 1342 行的事件处理闭包,
//! 负责把引擎 AgentEvent 逐条翻译为 Tauri emit + 会话状态更新。
//! 纯文件提取,不改任何并发/控制流/事件序列语义。通过 `use super::*`
//! 复用 engine.rs 的全部导入与类型。

use super::*;

fn behavior_task_status(status: TurnOutcomeStatus, error: Option<&str>) -> &'static str {
    match status {
        TurnOutcomeStatus::Completed if error.is_none() => "success",
        TurnOutcomeStatus::Interrupted => "interrupted",
        _ => "failed",
    }
}

/// 后台 task：持续读 rx_event 转 Tauri emit。
///
/// 关键点：监听 `Event::ApprovalRequired` 并主动 `approve_tool_call`。
/// 注意底座语义：`auto_approve = true` 时 `registered_tool_approval_required`
/// 会直接**旁路**可绕过的 Required 审批、不发事件（turn_loop.rs，非绕过名单仅
/// rlm_eval / start_mcp_server）。因此本臂对普通会话只在 auto_approve 关闭的
/// 场景（例如定时任务按 profile）收到事件；普通会话仍沿用既有批准策略。
///
/// Plan ready 触发：监听 `update_plan` 工具结果 + TurnComplete，
/// 若 active session mode=Plan + 本 turn 调过 update_plan + plan 非空 →
/// 设 phase=Ready + emit `chat:plan_ready` 含 plan snapshot。
///
/// **多引擎并发**:每个 session 一个独立 engine + 一个独立 forwarder,所以这里捕获
/// 的 `session_id` 唯一标识本 forwarder 服务的 session。**所有 emit 的 payload 都带
/// `session_id`**,前端按它把事件分流到对应 session 的缓冲;TurnComplete 里的 mode
/// 判据(plan_ready / M2 / M3)也全部基于本 `session_id`,不再读全局 `store.active_id()`
/// (并发下 active 会变,读全局会把判据算到错误 session 上)。返回 forwarder 的
/// `JoinHandle`,EnginePool 回收 session 时 `abort()` 它。
pub(crate) fn spawn_event_forwarder(
    app: AppHandle,
    handle: EngineHandle,
    store: SessionStore,
    bridge: Pinvou3Bridge,
    execution_workspace: PathBuf,
    session_id: String,
    turn_events: broadcast::Sender<EngineTurnSignal>,
    scheduled_profile: Option<ScheduledRunProfile>,
    scheduled_base_total_tokens: Option<u64>,
    scheduled_unattended: Arc<AtomicBool>,
    turn_lifecycle: Arc<TurnLifecycle>,
    shell_manager: SharedShellManager,
    turn_shell_tasks: TurnShellTaskRegistry,
) -> tauri::async_runtime::JoinHandle<()> {
    let approve_handle = handle.clone();
    let plan_tracker: Arc<Mutex<TurnPlanTracker>> =
        Arc::new(Mutex::new(TurnPlanTracker::default()));
    let shell_output = crate::features::assistant::shell_output::ShellOutputMonitor::spawn(
        app.clone(),
        &shell_manager,
        session_id.clone(),
    );
    tauri::async_runtime::spawn(async move {
        let mut tool_inputs: std::collections::HashMap<String, (String, serde_json::Value)> =
            std::collections::HashMap::new();
        let mut turn_tracker = TurnCompletionTracker::default();
        let mut scheduled_engine_total_tokens = 0_u64;
        let mut scheduled_persistence_error: Option<String> = None;
        let mut latest_chat_engine_state: Option<ChatEngineState> = None;
        let mut chat_persistence_error: Option<String> = None;
        let mut active_transcript_seen = false;
        // A typed preflight rejection completes the admitted lifecycle without
        // changing engine history. Preserve that distinction through
        // `chat:done` so the UI rolls back its optimistic edit.
        let mut active_operation_rejected = false;
        // app 侧自测推理指标累加器(TTFT/生成速度/累计tokens/KV)。try_state:headless
        // harness / 测试可能没 manage MonitorState,拿不到就整块跳过,不 panic。
        let self_metrics = app
            .try_state::<crate::features::monitor::MonitorState>()
            .map(|s| s.self_metrics());
        let mut current_turn_id: Option<String> = None;
        #[cfg(feature = "benchmark-hooks")]
        let mut first_delta_done: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut rx = handle.rx_event.write().await;
        while let Some(event) = rx.recv().await {
            match event {
                Event::TurnStarted { turn_id, .. } => {
                    // Publish admission from the authoritative engine event,
                    // before this serial forwarder can observe any delta or
                    // terminal event for the same turn. Reclaim uses the same
                    // emission gate, so it cannot overtake this pair.
                    let admitted =
                        turn_lifecycle.emit_started_admission(&app, &session_id, turn_id.clone());
                    // 消费 pending_cancel（无论 admitted 与否，防止跨轮泄漏）。
                    // reset_cancel_token() 在 TurnStarted 之前已执行，若 cancel
                    // 在此之前 arm 了标记，现在重新 cancel 命中的是本轮活跃 token。
                    // pending_cancel 携带 arming 时的 turn_epoch 与 steer 处置
                    // mode：只有匹配当前轮的 arm 才在此重放 cancel（跨轮的
                    // stale arm 被丢弃，#207），且重放必须按 arming 时的 mode
                    // 调 cancel_with_mode——无 mode 的 cancel() 硬编码
                    // StopDropInbox，会把 ⚡ 的 keepInbox 语义在重放路径丢失。
                    let epoch = turn_lifecycle.current_turn_generation().unwrap_or(0);
                    let pending_cancel = turn_lifecycle.take_pending_cancel(epoch);
                    if !admitted {
                        continue;
                    }
                    if let Some((_, mode)) = pending_cancel {
                        approve_handle
                            .cancel_with_mode(deepseek_tui::core::engine::CancelReason::User, mode);
                    }
                    active_transcript_seen = false;
                    active_operation_rejected = false;
                    chat_persistence_error = None;
                    current_turn_id = Some(turn_id.clone());
                    if let Err(error) = turn_shell_tasks.bind_or_prepare_turn(&turn_id).await {
                        log::error!(
                            "[pinvou3][chat] failed to bind shell task scope sid={} turn={}: {error:#}",
                            session_id,
                            turn_id
                        );
                    }
                    crate::features::behavior_telemetry::track(
                        &app,
                        crate::features::behavior_telemetry::BehaviorEvent::new("task_started")
                            .session(&session_id)
                            .turn(&turn_id),
                    );
                    crate::features::behavior_telemetry::track_model_used(
                        &app,
                        &session_id,
                        &turn_id,
                        &bridge,
                    );
                    let _ = turn_events.send(turn_tracker.on_started(turn_id));
                    // 本轮起始打点(TTFT 起点)。底座已发此事件,原先落 `_` 被忽略。
                    if let Some(m) = &self_metrics {
                        m.on_turn_started(&session_id);
                    }
                    #[cfg(feature = "benchmark-hooks")]
                    if crate::features::assistant::timing::eval_observation_enabled(&session_id) {
                        crate::features::assistant::timing::record_engine_turn_started(&session_id);
                    }
                }
                Event::MessageDelta { content, .. } => {
                    #[cfg(feature = "benchmark-hooks")]
                    if crate::features::assistant::timing::eval_observation_enabled(&session_id) {
                        crate::features::assistant::timing::record_first_message_delta(&session_id);
                        if let Some(turn_id) = &current_turn_id {
                            if first_delta_done.insert(turn_id.clone()) {
                                crate::features::assistant::timing::record_milestone(
                                    &session_id,
                                    "first_delta",
                                );
                            }
                        }
                    }
                    if let Some(m) = &self_metrics {
                        m.on_message_delta(&session_id, content.chars().count());
                    }
                    crate::features::memory::append_turn_assistant(&session_id, &content);
                    let payload = json!({ "session_id": session_id, "text": content });
                    let _ = app.emit("chat:delta", payload.clone());
                    crate::features::remote_control::forward_app_event(&app, "chat:delta", payload);
                }
                Event::ThinkingStarted { index } => {
                    let payload = json!({ "session_id": session_id, "index": index });
                    let _ = app.emit("chat:reasoning_start", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:reasoning_start",
                        payload,
                    );
                }
                Event::ThinkingDelta { index, content } => {
                    // 只转发底座已经识别为 ThinkingDelta 的独立思考内容。普通
                    // MessageDelta 不做启发式猜测，避免把最终回答误折叠成思考。
                    let payload =
                        json!({ "session_id": session_id, "index": index, "text": content });
                    let _ = app.emit("chat:reasoning_delta", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:reasoning_delta",
                        payload,
                    );
                }
                Event::ThinkingComplete { index } => {
                    let payload = json!({ "session_id": session_id, "index": index });
                    let _ = app.emit("chat:reasoning_done", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:reasoning_done",
                        payload,
                    );
                }
                Event::ToolCallStarted { id, name, input } => {
                    #[cfg(feature = "benchmark-hooks")]
                    if crate::features::assistant::timing::eval_observation_enabled(&session_id) {
                        crate::features::assistant::timing::record_tool_started(&session_id, &name);
                        crate::features::assistant::timing::record_milestone_meta(
                            &session_id,
                            "tool_call_started",
                            json!({ "tool_name": &name, "tool_id": &id }),
                        );
                    }
                    if let Some(m) = &self_metrics {
                        m.on_tool(&session_id); // 本轮有工具 → 收尾跳过 TTFT/TPS(D2)
                    }
                    crate::features::memory::record_turn_tool_start(&session_id, &name, &input);
                    shell_output.tool_started(&id, &name, &input);
                    tool_inputs.insert(id.clone(), (name.clone(), input.clone()));
                    let payload =
                        json!({ "session_id": session_id, "id": id, "name": name, "args": input });
                    let _ = app.emit("chat:tool_start", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:tool_start",
                        payload,
                    );
                }
                Event::ToolCallComplete { id, name, result } => {
                    #[cfg(feature = "benchmark-hooks")]
                    if crate::features::assistant::timing::eval_observation_enabled(&session_id) {
                        crate::features::assistant::timing::record_milestone_meta(
                            &session_id,
                            "tool_call_completed",
                            json!({ "tool_name": &name, "tool_id": &id }),
                        );
                    }
                    // 携带 metadata 让前端识别 careful hook 拦截 (safety_level=="dangerous")
                    let (output, success, metadata) = tool_call_result_parts(result);
                    #[cfg(feature = "benchmark-hooks")]
                    if crate::features::assistant::timing::eval_observation_enabled(&session_id) {
                        crate::features::assistant::timing::record_tool_completed(
                            &session_id,
                            &name,
                            success,
                        );
                    }
                    let background_task_id =
                        if matches!(name.as_str(), "exec_shell" | "task_shell_start" | "Bash")
                            && metadata
                                .as_ref()
                                .and_then(|value| value.get("status"))
                                .and_then(serde_json::Value::as_str)
                                == Some("Running")
                        {
                            metadata
                                .as_ref()
                                .and_then(|value| value.get("task_id"))
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        } else {
                            None
                        };
                    if let (Some(turn_id), Some(task_id)) =
                        (current_turn_id.as_deref(), background_task_id.as_deref())
                    {
                        if !turn_shell_tasks.register_task(turn_id, task_id) {
                            log::warn!(
                                "[pinvou3][chat] shell task has no root-turn scope sid={} turn={} task={}",
                                session_id,
                                turn_id,
                                task_id
                            );
                        }
                    }
                    shell_output.tool_completed(&id, background_task_id.as_deref());
                    let tracked_input = tool_inputs
                        .remove(&id)
                        .map(|(_, input)| input)
                        .unwrap_or(serde_json::Value::Null);
                    if success {
                        if let Some(profile) = scheduled_profile.as_ref() {
                            let artifact_store = store.clone();
                            let artifact_session_id = session_id.clone();
                            let artifact_workspace = profile.workspace.clone();
                            let artifact_name = name.clone();
                            let artifact_input = tracked_input.clone();
                            let artifact_output = output.clone();
                            match tokio::task::spawn_blocking(move || {
                                persist_successful_tool_artifact(
                                    &artifact_store,
                                    &artifact_session_id,
                                    &artifact_workspace,
                                    &artifact_name,
                                    &artifact_input,
                                    &artifact_output,
                                )
                            })
                            .await
                            {
                                Ok(Ok(_)) => {}
                                Ok(Err(error)) => eprintln!(
                                    "[pinvou3-app] scheduled artifact persistence skipped: {error:#}"
                                ),
                                Err(error) => eprintln!(
                                    "[pinvou3-app] scheduled artifact persistence task failed: {error}"
                                ),
                            }
                        }
                    }
                    crate::features::memory::record_turn_tool_complete(
                        &session_id,
                        &name,
                        &tracked_input,
                        success,
                    );
                    // Plan 类工具结果：标记 + 缓存 snapshot（两层）+ 实时 emit 给前端 chip 进度区
                    if success
                        && (name == "update_plan"
                            || name == "checklist_write"
                            || name == "todo_write")
                    {
                        let mut tracker = plan_tracker.lock();
                        tracker.plan_tool_used = true;
                        // 上游格式："Plan/Todo ... updated: ...\n{json}"——切第一个 '\n' 后是 json
                        if let Some(json_part) = output.find('\n').map(|i| &output[i + 1..]) {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_part) {
                                if name == "update_plan" {
                                    tracker.last_plan_snapshot = Some(v);
                                } else {
                                    tracker.last_todos_snapshot = Some(v);
                                }
                            }
                        }
                        // emit chat:plan_snapshot 实时更新 chip 进度——跟 plan_ready 解耦,
                        // 后者是状态转移信号(Planning→Ready 弹卡片),这个是数据更新信号(刷新 chip)。
                        // **只 emit 本次工具改的那个 snapshot**,另一个置 null——这样前端只更新对应
                        // 的时间戳,pickProgressItems 才能正确按时间挑最新的(否则两个 ts 总相等)。
                        let (plan_emit, todos_emit) = if name == "update_plan" {
                            (tracker.last_plan_snapshot.clone(), None)
                        } else {
                            (None, tracker.last_todos_snapshot.clone())
                        };
                        drop(tracker);
                        let payload = json!({
                            "session_id": session_id,
                            "plan_snapshot": plan_emit,
                            "todos_snapshot": todos_emit,
                        });
                        let _ = app.emit("chat:plan_snapshot", payload.clone());
                        crate::features::remote_control::forward_app_event(
                            &app,
                            "chat:plan_snapshot",
                            payload,
                        );
                    }
                    let payload = json!({
                        "session_id": session_id,
                        "id": id,
                        "name": name,
                        "output": output,
                        "success": success,
                        "metadata": metadata,
                    });
                    // request_user_input 收口：submit/cancel 已由底座转为工具结果。
                    if name == "request_user_input" {
                        crate::features::assistant::pending_user_input::clear(&session_id, &id);
                    }
                    let _ = app.emit("chat:tool_end", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:tool_end",
                        payload,
                    );
                }
                Event::UserInputRequired { id, request } => {
                    if scheduled_profile.is_some() && scheduled_unattended.load(Ordering::Acquire) {
                        // 定时任务没有前台操作者；不能把底座卡在等待输入的超时上。
                        let h = approve_handle.clone();
                        tokio::spawn(async move {
                            if let Err(error) = h.cancel_user_input(id).await {
                                eprintln!(
                                    "[pinvou3-app] cancel scheduled user input failed: {error:#}"
                                );
                            }
                        });
                    } else {
                        // 普通对话继续交给前端或远程控制端处理。登记 pending，
                        // 供前端 remount 后经 get_pending_user_inputs 恢复确认卡。
                        let payload = json!({
                            "session_id": session_id,
                            "id": id,
                            "questions": request.questions,
                        });
                        crate::features::assistant::pending_user_input::record(
                            &session_id,
                            &id,
                            payload["questions"].clone(),
                        );
                        let _ = app.emit("chat:user_input_required", payload.clone());
                        crate::features::remote_control::forward_app_event(
                            &app,
                            "chat:user_input_required",
                            payload,
                        );
                    }
                }
                Event::SessionUpdated {
                    session_id: event_session_id,
                    messages,
                    system_prompt,
                    model,
                    workspace,
                } => {
                    if event_session_id != session_id {
                        let mismatch = format!(
                            "Engine session id mismatch: expected {session_id}, got {event_session_id}"
                        );
                        if scheduled_profile.is_some() {
                            scheduled_persistence_error = Some(mismatch);
                        } else {
                            chat_persistence_error = Some(mismatch);
                        }
                        continue;
                    }
                    if let Some(profile) = scheduled_profile.as_ref() {
                        let state = ScheduledEngineState {
                            messages,
                            system_prompt,
                            model,
                            workspace,
                            mode: profile.execution_mode(),
                            token_accounting: ScheduledTokenAccounting::PreservePersisted,
                        };
                        let store_for_save = store.clone();
                        let session_for_save = session_id.clone();
                        match tokio::task::spawn_blocking(move || {
                            store_for_save.persist_scheduled_engine_state(&session_for_save, state)
                        })
                        .await
                        {
                            Ok(Ok(_)) => scheduled_persistence_error = None,
                            Ok(Err(error)) => {
                                scheduled_persistence_error = Some(format!("{error:#}"));
                            }
                            Err(error) => {
                                scheduled_persistence_error = Some(format!(
                                    "scheduled transcript persistence task failed: {error}"
                                ));
                            }
                        }
                    } else {
                        let (messages, matched_active_rule) =
                            turn_lifecycle.sanitize_messages(messages);
                        active_transcript_seen |= matched_active_rule;
                        let state = ChatEngineState {
                            messages,
                            system_prompt,
                            model,
                            workspace,
                        };
                        latest_chat_engine_state = Some(state.clone());
                        let store_for_save = store.clone();
                        let session_for_save = session_id.clone();
                        match tokio::task::spawn_blocking(move || {
                            store_for_save.persist_chat_engine_state(&session_for_save, state)
                        })
                        .await
                        {
                            Ok(Ok(saved)) => {
                                chat_persistence_error = None;
                                if let Ok(revision) = transcript_revision(&saved.messages) {
                                    emit_transcript_committed(
                                        &app,
                                        &session_id,
                                        revision,
                                        "engine_state_update",
                                        saved.messages.len(),
                                    );
                                }
                            }
                            Ok(Err(error)) => {
                                chat_persistence_error = Some(format!("{error:#}"));
                            }
                            Err(error) => {
                                chat_persistence_error = Some(format!(
                                    "chat transcript persistence task failed: {error}"
                                ));
                            }
                        }
                    }
                }
                Event::ApprovalRequired {
                    id,
                    tool_name,
                    approval_force_prompt,
                    ..
                } => {
                    // 多智能体会话与普通对话共用同一套审批语义（ADR-0006）：
                    // 只有无人值守的初始定时执行使用任务保存的批准策略；
                    // 其余会话按普通对话处理。
                    let should_approve = if scheduled_profile.is_some()
                        && scheduled_unattended.load(Ordering::Acquire)
                    {
                        scheduled_tool_should_auto_approve(
                            scheduled_profile.as_ref(),
                            approval_force_prompt,
                        )
                    } else {
                        true
                    };
                    eprintln!(
                        "[pinvou3-app] {} tool {} id={}",
                        if should_approve {
                            "auto-approving"
                        } else {
                            "denying"
                        },
                        tool_name,
                        id
                    );
                    let h = approve_handle.clone();
                    let id_clone = id.clone();
                    tokio::spawn(async move {
                        let result = if should_approve {
                            h.approve_tool_call(id_clone).await
                        } else {
                            h.deny_tool_call(id_clone).await
                        };
                        if let Err(e) = result {
                            eprintln!("[pinvou3-app] scheduled approval decision failed: {e:#}");
                        }
                    });
                    // 不重复 emit chat:tool_start —— 上游 ToolCallStarted（带完整 input）
                    // 已先于 ApprovalRequired fire，前端已收到正确的 args。
                    // 之前在此 emit 会用 args=null 覆盖前端 toolMeta，导致产物路径丢失。
                }
                // main 侧清理：CodeWhale 原生 workflow 事件由底座自行管理，
                // 应用只消费通用子智能体事件（AgentSpawned 不再登记 role）。
                Event::AgentSpawned { .. } => {}
                // mid-turn inject 投递确认（P0-A）：引擎把 steer 消息追加进
                // transcript 后发 SteerCommitted（带回入队时的 steer_id），前端
                // 据此把对应排队 chip 转气泡；引擎丢弃时才发 SteerDropped。前端
                // 不再靠 chat:transcript_committed + load_session 比对消息数来猜。
                Event::SteerCommitted { steer_id } => {
                    let payload = json!({
                        "session_id": session_id,
                        "steer_id": steer_id,
                    });
                    let _ = app.emit("chat:steer_committed", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:steer_committed",
                        payload,
                    );
                }
                Event::SteerDropped { steer_id } => {
                    let payload = json!({
                        "session_id": session_id,
                        "steer_id": steer_id,
                    });
                    let _ = app.emit("chat:steer_dropped", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:steer_dropped",
                        payload,
                    );
                }
                Event::WorkflowUi { .. } => {}
                Event::AgentProgress { id, status, .. } => {
                    let payload = json!({
                        "session_id": session_id,
                        "agent_id": id,
                        "status": status,
                    });
                    let _ = app.emit("multiagent:agent_progress", payload);
                }
                // mailbox 信封同时维护 Shell scope 与通用子智能体审计。
                Event::SubAgentMailbox { message, .. } => {
                    use deepseek_tui::tools::subagent::MailboxMessage as MM;
                    // 审计是应用账本：绑了项目目录的原生代码会话写会话私有目录，
                    // 不污染用户项目；其余会话账本根与执行根相同，行为不变。
                    let ws = bridge.audit_workspace(&session_id, &execution_workspace);
                    match message {
                        MM::Started { agent_id, .. } => {
                            if let Some(turn_id) = current_turn_id.as_deref() {
                                turn_shell_tasks.register_agent(turn_id, &agent_id);
                            }
                        }
                        MM::ChildSpawned {
                            parent_id,
                            child_id,
                        } => {
                            if !turn_shell_tasks.register_child_agent(&parent_id, &child_id) {
                                log::warn!(
                                    "[pinvou3][chat] child agent has no parent shell scope sid={} parent={} child={}",
                                    session_id,
                                    parent_id,
                                    child_id
                                );
                            }
                        }
                        MM::TokenUsage {
                            agent_id, usage, ..
                        } => {
                            crate::features::assistant::audit::append(
                                &ws,
                                "token",
                                &agent_id,
                                json!({
                                    "agent_id": agent_id,
                                    "input": usage.input_tokens,
                                    "output": usage.output_tokens,
                                }),
                            );
                        }
                        MM::ToolCallStarted {
                            agent_id,
                            tool_name,
                            step,
                        } => {
                            let payload = json!({
                                "session_id": session_id,
                                "agent_id": agent_id,
                                "status": format!("🔧 {tool_name} (step {step})"),
                            });
                            let _ = app.emit("multiagent:agent_progress", payload);
                        }
                        MM::Completed { agent_id, summary } => {
                            turn_shell_tasks.complete_agent(&agent_id);
                            crate::features::assistant::audit::append(
                                &ws,
                                "agent_done",
                                &agent_id,
                                json!({
                                    "agent_id": agent_id,
                                    "summary": crate::platform::strings::truncate_utf8(&summary, 600),
                                }),
                            );
                        }
                        MM::Failed { agent_id, error } => {
                            turn_shell_tasks.complete_agent(&agent_id);
                            crate::features::assistant::audit::append(
                                &ws,
                                "agent_failed",
                                &agent_id,
                                json!({
                                    "agent_id": agent_id,
                                    "error": crate::platform::strings::truncate_utf8(&error, 600),
                                }),
                            );
                        }
                        MM::Cancelled { agent_id } => {
                            turn_shell_tasks.complete_agent(&agent_id);
                        }
                        MM::Interrupted { agent_id, .. } => {
                            // CodeWhale 当前不保留可原位恢复的 live task；中断后只能基于
                            // checkpoint 重新派发，因此它对本轮 Shell scope 同样是终态。
                            turn_shell_tasks.complete_agent(&agent_id);
                        }
                        _ => {}
                    }
                }
                Event::AgentComplete { id, failed, .. } => {
                    let payload = json!({
                        "session_id": session_id,
                        "agent_id": id,
                        "failed": failed,
                    });
                    let _ = app.emit("multiagent:agent_complete", payload);
                }
                Event::TurnComplete {
                    usage,
                    status,
                    error,
                    tool_catalog,
                    base_url: _,
                } => {
                    #[cfg(not(feature = "benchmark-hooks"))]
                    let _ = &tool_catalog;
                    #[cfg(feature = "benchmark-hooks")]
                    let authorized_tool_catalog = crate::features::assistant::timing::eval_observation_enabled(&session_id)
                        .then(|| {
                            tool_catalog.as_ref().and_then(|catalog| {
                                serde_json::to_vec(catalog).ok().map(|catalog_json| {
                                    crate::features::assistant::timing::ToolCatalogSummary::from_serialized_catalog(
                                        catalog.len(),
                                        &catalog_json,
                                    )
                                })
                            })
                        })
                        .flatten();
                    let shell_turn_id = current_turn_id.clone();
                    #[cfg(feature = "benchmark-hooks")]
                    if let Some(turn_id) = shell_turn_id.as_ref() {
                        first_delta_done.remove(turn_id);
                    }
                    let shell_interrupted = status == TurnOutcomeStatus::Interrupted;
                    let operation_rejected = std::mem::take(&mut active_operation_rejected);
                    let mut shell_cleanup_failed = false;
                    let mut terminal_status = status;
                    let mut terminal_error = error;
                    if let Some(base_total_tokens) = scheduled_base_total_tokens {
                        scheduled_engine_total_tokens = scheduled_engine_total_tokens
                            .saturating_add(u64::from(usage.input_tokens))
                            .saturating_add(u64::from(usage.output_tokens));
                        let store_for_save = store.clone();
                        let session_for_save = session_id.clone();
                        match tokio::task::spawn_blocking(move || {
                            store_for_save.persist_scheduled_token_total(
                                &session_for_save,
                                base_total_tokens,
                                scheduled_engine_total_tokens,
                            )
                        })
                        .await
                        {
                            Ok(Ok(_)) => {}
                            Ok(Err(save_error)) => {
                                scheduled_persistence_error = Some(format!("{save_error:#}"));
                            }
                            Err(join_error) => {
                                scheduled_persistence_error = Some(format!(
                                    "scheduled token persistence task failed: {join_error}"
                                ));
                            }
                        }
                        if let Some(save_error) = scheduled_persistence_error.take() {
                            terminal_status = TurnOutcomeStatus::Failed;
                            terminal_error = Some(match terminal_error {
                                Some(engine_error) => format!(
                                    "{engine_error}; scheduled conversation persistence failed: {save_error}"
                                ),
                                None => format!(
                                    "Scheduled conversation persistence failed: {save_error}"
                                ),
                            });
                        }
                    } else {
                        let terminal_save = if operation_rejected {
                            // Rejection must win over both an engine snapshot
                            // and the optimistic admission fallback.
                            None
                        } else if active_transcript_seen {
                            latest_chat_engine_state.clone().map(|state| {
                                let store_for_save = store.clone();
                                let session_for_save = session_id.clone();
                                tokio::task::spawn_blocking(move || {
                                    store_for_save
                                        .persist_chat_engine_state(&session_for_save, state)
                                })
                            })
                        } else {
                            turn_lifecycle.active_transcript_fallback().map(|fallback| {
                                let store_for_save = store.clone();
                                let session_for_save = session_id.clone();
                                tokio::task::spawn_blocking(move || {
                                    store_for_save.persist_admitted_chat_display(
                                        &session_for_save,
                                        &fallback.baseline_revision,
                                        fallback.display_message,
                                        fallback.operation == TranscriptOperation::EditLast,
                                    )
                                })
                            })
                        };
                        if let Some(save_task) = terminal_save {
                            match save_task.await {
                                Ok(Ok(saved)) => {
                                    chat_persistence_error = None;
                                    if let Ok(revision) = transcript_revision(&saved.messages) {
                                        emit_transcript_committed(
                                            &app,
                                            &session_id,
                                            revision,
                                            "terminal_fallback",
                                            saved.messages.len(),
                                        );
                                    }
                                }
                                Ok(Err(save_error)) => {
                                    chat_persistence_error = Some(format!("{save_error:#}"));
                                }
                                Err(join_error) => {
                                    chat_persistence_error = Some(format!(
                                        "chat transcript persistence task failed: {join_error}"
                                    ));
                                }
                            }
                        }
                        if let Some(save_error) = chat_persistence_error.take() {
                            terminal_status = TurnOutcomeStatus::Failed;
                            terminal_error = Some(match terminal_error {
                                Some(engine_error) => format!(
                                    "{engine_error}; chat conversation persistence failed: {save_error}"
                                ),
                                None => format!(
                                    "Chat conversation persistence failed: {save_error}"
                                ),
                            });
                        }
                    }
                    // TurnComplete closes the snapshot ownership window. Edit
                    // preflight rejection has no TurnStarted, so retaining a
                    // prior turn's state here could otherwise make that later
                    // rejection repersist a stale snapshot.
                    active_transcript_seen = false;
                    latest_chat_engine_state = None;
                    // Keep the shell scope cancellable throughout persistence.
                    // Final cleanup runs before terminal admission is claimed;
                    // Engine reclaim can therefore still win an await race and
                    // close the same scope without leaving the UI permanently
                    // busy. The cleanup itself executes on blocking workers.
                    if let Some(turn_id) = shell_turn_id.as_deref() {
                        match turn_shell_tasks
                            .finalize_turn(turn_id, shell_interrupted)
                            .await
                        {
                            Ok(report) => {
                                if !report.killed.is_empty() {
                                    log::info!(
                                        "[pinvou3][chat] finalized {} interrupted shell task(s) sid={} turn={}",
                                        report.killed.len(),
                                        session_id,
                                        turn_id
                                    );
                                }
                                if let Some(cleanup_error) = report.failure_summary() {
                                    shell_cleanup_failed = true;
                                    log::error!(
                                        "[pinvou3][chat] shell cleanup remained incomplete sid={} turn={}: {}",
                                        session_id,
                                        turn_id,
                                        cleanup_error
                                    );
                                }
                            }
                            Err(error) => {
                                log::error!(
                                    "[pinvou3][chat] failed to finalize shell task scope sid={} turn={}: {error:#}",
                                    session_id,
                                    turn_id
                                );
                                shell_cleanup_failed = true;
                            }
                        }
                    }
                    // Persistence above can change the authoritative outcome and
                    // may await. Claim only after that result is known, but before
                    // every completion-only side effect below. If reclaim won
                    // during the await, this late TurnComplete is discarded.
                    let Some(_) = turn_lifecycle.claim_terminal_with_admission(&app, &session_id)
                    else {
                        continue;
                    };
                    // 单独发 usage 给前端 token 进度条
                    let payload = json!({
                        "session_id": session_id,
                        "input_tokens": usage.input_tokens,
                        "output_tokens": usage.output_tokens,
                        "context_window": bridge.usage_context_window(),
                    });
                    let _ = app.emit("chat:usage", payload.clone());
                    crate::features::remote_control::forward_app_event(&app, "chat:usage", payload);
                    // app 侧自测:用精确 usage 收尾本轮。部分远端模型会流式返回文本,
                    // 但最终 usage.output_tokens 为 0,因此不能用 output_tokens 门控收尾。
                    if let Some(m) = &self_metrics {
                        m.on_turn_complete(
                            &session_id,
                            usage.input_tokens,
                            usage.output_tokens,
                            usage.prompt_cache_hit_tokens,
                            usage.prompt_cache_miss_tokens,
                        );
                    }
                    // turn end:取出 tracker 快照,然后重置(下个 turn 重新累积)。
                    // 用独立 block 把 parking_lot guard 的生命周期限死在这里:下方
                    // H1 harness 段有 .await(spawn_blocking),guard 是 !Send,若跨
                    // await 会让整个 forwarder future 变 !Send(tauri::spawn 要求 Send)。
                    let (plan_used, plan_snapshot, todos_snapshot) = {
                        let mut tracker = plan_tracker.lock();
                        let plan_used = tracker.plan_tool_used;
                        let plan_snapshot = tracker.last_plan_snapshot.take();
                        let todos_snapshot = tracker.last_todos_snapshot.take();
                        *tracker = TurnPlanTracker::default();
                        (plan_used, plan_snapshot, todos_snapshot)
                    };

                    // 先完成权威时间线，再发 chat:done。前端终态事件会据此重新
                    // 读取整份时间线，补齐后台运行/页面恢复期间漏掉的本地事件。
                    // 收尾只走 observation 版一次:tool_calls/catalog 摘要随
                    // assistant_done 落盘;若先前再用 usage 版收尾会先 pop 掉
                    // 队首活跃轮,排队双轮场景错配下一轮的时间线。
                    let status_text = format!("{terminal_status:?}");
                    let timing_usage = Some(crate::features::assistant::timing::TurnUsage {
                        input_tokens: u64::from(usage.input_tokens),
                        output_tokens: u64::from(usage.output_tokens),
                        cache_hit_tokens: u64::from(usage.prompt_cache_hit_tokens.unwrap_or(0)),
                        cache_miss_tokens: u64::from(usage.prompt_cache_miss_tokens.unwrap_or(0)),
                        cache_write_tokens: u64::from(usage.prompt_cache_write_tokens.unwrap_or(0)),
                        reasoning_tokens: u64::from(usage.reasoning_tokens.unwrap_or(0)),
                        // 与 chat:usage 同一来源：代码页 hydration 回填用量 chip 分母。
                        context_window: u64::from(bridge.usage_context_window()),
                    });
                    #[cfg(feature = "benchmark-hooks")]
                    if crate::features::assistant::timing::eval_observation_enabled(&session_id) {
                        crate::features::assistant::timing::finish_turn_with_observation(
                            &session_id,
                            &status_text,
                            terminal_error.as_deref(),
                            timing_usage,
                            authorized_tool_catalog,
                        );
                    } else {
                        crate::features::assistant::timing::finish_turn_with_usage(
                            &session_id,
                            &status_text,
                            terminal_error.as_deref(),
                            timing_usage,
                        );
                    }
                    #[cfg(not(feature = "benchmark-hooks"))]
                    crate::features::assistant::timing::finish_turn_with_usage(
                        &session_id,
                        &status_text,
                        terminal_error.as_deref(),
                        timing_usage,
                    );

                    {
                        // 多引擎:mode 判据基于本 forwarder 的 session_id,不读全局 active。
                        let active_id = session_id.clone();
                        let state = store.mode_state(&active_id);

                        // ── plan_ready 触发(Plan + 调过 plan 类工具) ──
                        // 底座式:Plan 模式调过 update_plan = 出方案 → 弹决策卡。不再设 phase
                        // (已砍 PlanPhase),mode 留 Plan 直到用户 accept/discard 切 Yolo。
                        // 修订(还在 Plan 时再调 update_plan)天然幂等:再弹一张新卡。
                        if plan_used && state.mode == SerializableMode::Plan {
                            if let Some(plan_id) = current_turn_id.clone() {
                                if let Some(mode_state) =
                                    store.register_pending_plan(&active_id, plan_id.clone())
                                {
                                    let payload = json!({
                                        "session_id": active_id.clone(),
                                        "plan_id": plan_id,
                                        "plan_snapshot": plan_snapshot.clone(),
                                        "todos_snapshot": todos_snapshot.clone(),
                                        "mode_state": mode_state,
                                    });
                                    let _ = app.emit("chat:plan_ready", payload.clone());
                                    crate::features::remote_control::forward_app_event(
                                        &app,
                                        "chat:plan_ready",
                                        payload,
                                    );
                                }
                            }
                        }

                        // Publish the claimed
                        // terminal first so a concurrent Engine eviction cannot
                        // abort this forwarder and leave the frontend permanently
                        // busy. Reclaim now observes the lifecycle as terminal and
                        // cannot publish a conflicting Interrupted outcome.
                        //
                        // P0-B：finish_terminal_emission 必须先于 emit 执行，使
                        // chat:done 到达时 reserve 闸门已重开（契约：chat:done ⇒
                        // 槽位已释放），前端 interruptAndSend 不再需要固定 sleep。
                        // generation 在 finish 前读取（finish 后回到 None）。
                        // emit 保持在 harness await 之前：harness 段可能被引擎
                        // eviction 中断，终态必须先发出去，前端才不会永久 busy。
                        let generation = turn_lifecycle.current_turn_generation().unwrap_or(0);
                        turn_lifecycle.finish_terminal_emission();
                        emit_chat_terminal(
                            &app,
                            &session_id,
                            generation,
                            terminal_status,
                            terminal_error.clone(),
                            shell_cleanup_failed,
                            operation_rejected,
                        );

                        // ── [回归底座式] M2 自驱 + M3 文本兜底已彻底砍掉 ──
                        // M2:执行不自动续跑,回底座由用户驱动。M3(Plan 写了方案没调
                        // update_plan 的救援)放弃不做:底座 update_plan→plan_ready→方案卡
                        // 这条链已可靠,漏的少数"光说不出卡"由 composer chip 手切 + plan_stuck
                        // 卡兜底,不值得用噪音判据再造一层。
                    }
                    if crate::features::memory::memory_enabled() {
                        if let Some(capture) =
                            crate::features::memory::take_turn_capture(&session_id)
                        {
                            let app_clone = app.clone();
                            let bridge_clone = bridge.clone();
                            let sid_clone = session_id.clone();
                            tauri::async_runtime::spawn(async move {
                                match crate::features::memory::review_turn_candidates_with_llm(
                                    &bridge_clone,
                                    &capture,
                                    &sid_clone,
                                )
                                .await
                                {
                                    Ok(outcome) => {
                                        let mut events = outcome.events;
                                        events.extend(outcome.pending.into_iter().filter_map(
                                            |item| {
                                                (item.status == "pending_confirm").then(|| {
                                                    crate::features::memory::MemoryWriteEvent {
                                                        kind: item.kind,
                                                        action: "pending".to_string(),
                                                        id: item.id,
                                                        text: item.content,
                                                    }
                                                })
                                            },
                                        ));
                                        if !events.is_empty() {
                                            let _ = app_clone.emit(
                                                "chat:memory_write",
                                                json!({
                                                    "session_id": sid_clone,
                                                    "events": events,
                                                }),
                                            );
                                            match crate::features::memory::runtime_snapshot(&sid_clone) {
                                            Ok(snapshot) => {
                                                let _ = app_clone.emit(
                                                    "chat:memory",
                                                    json!({
                                                        "session_id": sid_clone,
                                                        "items": snapshot.items,
                                                        "runtime_path": snapshot.runtime_path,
                                                    }),
                                                );
                                            }
                                            Err(err) => eprintln!(
                                                "[pinvou3-app] refresh memory runtime after review failed for session {sid_clone}: {err}"
                                            ),
                                        }
                                        }
                                    }
                                    Err(err) => {
                                        eprintln!(
                                        "[pinvou3-app] memory llm review failed for session {sid_clone}: {err:#}"
                                    );
                                    }
                                }
                            });
                        }
                    }
                    if let Some(turn_id) = current_turn_id.as_deref() {
                        crate::features::behavior_telemetry::track(
                            &app,
                            crate::features::behavior_telemetry::BehaviorEvent::new(
                                "task_finished",
                            )
                            .session(&session_id)
                            .turn(turn_id)
                            .status(behavior_task_status(
                                terminal_status,
                                terminal_error.as_deref(),
                            )),
                        );
                    }
                    maybe_notify_task_completed(
                        &app,
                        &store,
                        &session_id,
                        current_turn_id.take(),
                        terminal_status,
                        terminal_error.as_deref(),
                    );
                    if let Some(signal) = turn_tracker.on_terminal(terminal_status, terminal_error)
                    {
                        let _ = turn_events.send(signal);
                    }
                }
                Event::CompactionStarted {
                    id, message, auto, ..
                } => {
                    let payload = json!({ "session_id": session_id, "phase": "start", "id": id, "auto": auto, "message": message });
                    let _ = app.emit("chat:compaction", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:compaction",
                        payload,
                    );
                }
                Event::CompactionCompleted {
                    id,
                    message,
                    auto,
                    messages_before,
                    messages_after,
                    post_input_tokens,
                    ..
                } => {
                    let payload = json!({
                        "session_id": session_id,
                        "phase": "done",
                        "id": id,
                        "auto": auto,
                        "message": message,
                        "messages_before": messages_before,
                        "messages_after": messages_after,
                        // Conservative post-compaction context estimate for refreshing the
                        // usage chip before the next authoritative chat:usage event.
                        "post_tokens": post_input_tokens,
                    });
                    let _ = app.emit("chat:compaction", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:compaction",
                        payload,
                    );
                    // Compaction does not call start_turn, and its TurnComplete carries zero
                    // usage. Persist this estimate separately so hydration does not restore
                    // the stale pre-compaction value.
                    if let Some(tokens) = post_input_tokens.filter(|tokens| *tokens > 0) {
                        crate::features::assistant::timing::record_context_snapshot(
                            &session_id,
                            tokens,
                            u64::from(bridge.usage_context_window()),
                        );
                    }
                }
                Event::CompactionFailed { message, auto, .. } => {
                    let payload = json!({ "session_id": session_id, "phase": "fail", "auto": auto, "message": message });
                    let _ = app.emit("chat:compaction", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:compaction",
                        payload,
                    );
                }
                Event::Error { envelope, .. } => {
                    // Edit preflight failures are non-terminal advisories until
                    // the paired TurnComplete, but their typed code tells the
                    // terminal path not to persist the optimistic fallback.
                    if !envelope.recoverable && envelope.code.starts_with("edit_last_turn_") {
                        active_operation_rejected = true;
                    }
                    // 可恢复错误(如 SSE idle timeout、瞬态工具失败)turn 不会结束——
                    // 引擎会 retry / 继续跑后续步骤。绝不能发 chat:done,否则前端
                    // setBusy(false) 把"思考中"指示器掐掉,而引擎还在干活,看着像卡死
                    // (且会误触发 flush/closeBubble/plan_phase 收尾)。只飘个 advisory。
                    // 仅 recoverable==false(致命)才是真结束 → chat:done。
                    if envelope.recoverable {
                        let payload =
                            json!({ "session_id": session_id, "error": envelope.message });
                        let _ = app.emit("chat:transient_error", payload.clone());
                        crate::features::remote_control::forward_app_event(
                            &app,
                            "chat:transient_error",
                            payload,
                        );
                    } else {
                        // 底座在致命 Error 后仍会发权威 TurnComplete(Failed)。这里只缓存
                        // 文本；完成、持久化和 chat:done 全部由 TurnComplete 单点处理。
                        turn_tracker.on_fatal_error(envelope.message);
                    }
                }
                _ => {}
            }
        }
        let stopped_error = "Engine event stream stopped before a terminal event".to_string();
        let mut shell_cleanup_failed = false;
        {
            // The stream can stop before `TurnStarted` bound the provisional
            // submission scope; close the active scope too, otherwise the next
            // `prepare_turn` keeps bailing on "scope already active".
            let finalize = match current_turn_id.as_deref() {
                Some(turn_id) => turn_shell_tasks.finalize_turn(turn_id, true).await,
                None => turn_shell_tasks.finalize_active_scope(true).await,
            };
            match finalize {
                Ok(report) => {
                    if let Some(error) = report.failure_summary() {
                        shell_cleanup_failed = true;
                        log::error!(
                            "[pinvou3][chat] shell cleanup remained incomplete after event stream stop sid={} turn={:?}: {}",
                            session_id,
                            current_turn_id,
                            error
                        );
                    }
                }
                Err(error) => {
                    shell_cleanup_failed = true;
                    log::error!(
                        "[pinvou3][chat] failed to close shell task scope after event stream stop sid={} turn={:?}: {error:#}",
                        session_id,
                        current_turn_id
                    );
                }
            }
        }
        // 事件流中断/停止不经过 TurnComplete：清掉本轮 self_metrics 打点与 memory
        // turn capture，否则 inflight / turn_capture_store 会按 session 永久驻留。
        if let Some(m) = &self_metrics {
            m.on_turn_aborted(&session_id);
        }
        crate::features::memory::discard_turn_capture(&session_id);
        match finish_reclaimed_lifecycle_turn(
            &turn_lifecycle,
            &app,
            &store,
            &session_id,
            TurnOutcomeStatus::Failed,
            Some(stopped_error.clone()),
            shell_cleanup_failed,
            active_operation_rejected,
        )
        .await
        {
            Some(EmittedTerminal {
                turn_id: Some(turn_id),
            }) => {
                crate::features::behavior_telemetry::track(
                    &app,
                    crate::features::behavior_telemetry::BehaviorEvent::new("task_finished")
                        .session(&session_id)
                        .turn(&turn_id)
                        .status("failed"),
                );
                let _ = turn_events.send(EngineTurnSignal::Terminal {
                    turn_id,
                    status: TurnOutcomeStatus::Failed,
                    error: Some(stopped_error),
                });
            }
            Some(EmittedTerminal { turn_id: None }) | None => {
                let _ = turn_events.send(EngineTurnSignal::ForwarderStopped {
                    error: stopped_error,
                });
            }
        }
        eprintln!(
            "[pinvou3-app] event forwarder stopped for session {session_id} (engine shut down?)"
        );
    })
}
