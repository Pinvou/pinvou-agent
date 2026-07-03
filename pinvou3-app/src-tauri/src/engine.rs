//! pinvou3-app 与 DeepSeek-TUI Engine 的桥接层。
//!
//! 职责：
//!  1. 通过 [`bridge::Pinvou3Bridge`] 把 `~/.pinvou3/settings.json` 翻译成
//!     [`EngineConfig`] / [`DtConfig`]，然后 `spawn_engine`，存到 Tauri State
//!  2. 后台 task 持续读 `EngineHandle::rx_event`，转译成 Tauri 事件
//!     （`chat:delta` / `chat:tool_start` / `chat:tool_end` / `chat:done`
//!      / `chat:plan_ready`）
//!  3. 暴露 `send_user_message()` 给 [`commands::chat`] 调用
//!
//! 所有配置决策（model / paths / locale / allow_shell ...）都在 bridge 里，
//! 这一层只做 "boot engine + 转发事件"。Engine 自管 session 状态，多轮对话
//! 在同一个 EngineHandle 内自然累积。

use std::sync::Arc;

use anyhow::Result;
use deepseek_tui::core::engine::{spawn_engine, EngineHandle};
use deepseek_tui::core::events::Event;
use deepseek_tui::core::ops::Op;
use deepseek_tui::models::Message;
use deepseek_tui::tools::user_input::UserInputResponse;
use deepseek_tui::tui::app::AppMode;
use parking_lot::Mutex;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::bridge::mode_state::SerializableMode;
use crate::bridge::sessions::SessionStore;
use crate::bridge::Pinvou3Bridge;

/// 单个 session 的 engine wrapper(handle + 该 session 绑定的 bridge)。
///
/// 多引擎并发模型下,[`EnginePool`](crate::engine_pool::EnginePool) 为每个 session
/// 持有一个 `AppEngine`(经 [`spawn_for_session`](Self::spawn_for_session) 创建);
/// L1 headless harness 经 [`spawn_headless`](Self::spawn_headless) 单独用一个。
/// Clone 廉价(EngineHandle 内部 Arc)。
#[derive(Clone)]
pub struct AppEngine {
    pub handle: EngineHandle,
    pub bridge: Pinvou3Bridge,
}

impl AppEngine {
    /// 为指定 session spawn 一个**独立** engine:绑定该 session 专属的 workspace +
    /// instructions(spawn 时由 [`build_engine_config_for_session`] 固化进 config,
    /// 不再靠 `Op::SyncSession` 动态切),并启一个带 `session_id` 的 event forwarder。
    /// 返回 `(engine, forwarder_handle)`,[`EnginePool`] 回收 session 时 abort forwarder。
    ///
    /// 调用方(EnginePool)负责复用同一份已 boot 的 `bridge`,避免每个 session 重 boot
    /// (boot 会写盘 / 设 env)。
    ///
    /// [`build_engine_config_for_session`]: crate::bridge::Pinvou3Bridge::build_engine_config_for_session
    /// [`EnginePool`]: crate::engine_pool::EnginePool
    pub async fn spawn_for_session(
        app: AppHandle,
        store: SessionStore,
        bridge: Pinvou3Bridge,
        session_id: &str,
    ) -> Result<(Self, tauri::async_runtime::JoinHandle<()>)> {
        // C 方案(P-no-disk): instructions 走 Inline,不再写 disk(远端)。
        // 工作流会话不再施加监工白名单(对话型监工已废弃);SubAgent 角色的工具
        // 由 agent_registry.json 各自约束,与此处无关。
        let mut engine_config = bridge.build_engine_config_for_session(session_id);
        // Agentic RAG:给该 session 的 engine 注入 kb_search 工具(持 session_id,execute 时
        // 查该会话挂载的知识集)。工具常驻所有会话,挂没挂集由其运行时判断。
        engine_config.extra_tools.0.push(std::sync::Arc::new(
            crate::knowledge::KbSearchTool::new(app.clone(), session_id.to_string()),
        ));
        // 工具门控:连接器开关禁用 +(知识库为空时)隐藏 kb_search。compute 返回**完整**列表
        // (已含连接器禁用),直接覆盖 build_engine_config 设的「连接器-only」初值,让新会话天生正确
        // ——空知识库就看不到 kb_search,不会宣称能本地检索。
        let disallowed = crate::commands::compute_disallowed_tools(&app);
        engine_config.disallowed_tools = if disallowed.is_empty() { None } else { Some(disallowed) };
        let dt_config = bridge.build_dt_config();

        eprintln!(
            "[pinvou3-app] spawn_engine session={} model={} workspace={} instructions={}",
            session_id,
            engine_config.model,
            engine_config.workspace.display(),
            format_instructions(&engine_config.instructions),
        );

        let handle = spawn_engine(engine_config, &dt_config);
        let forwarder = spawn_event_forwarder(
            app,
            handle.clone(),
            store,
            bridge.clone(),
            session_id.to_string(),
        );

        Ok((Self { handle, bridge }, forwarder))
    }

    /// 测试入口(L1 harness 用):用预先 boot 好的 bridge spawn 一个 engine,
    /// **不启 Tauri event forwarder** (不需要 AppHandle / SessionStore),
    /// 调用方自己消费 `engine.handle.rx_event` 拿到 ToolCallStarted /
    /// ToolCallComplete / TurnComplete 做断言。
    ///
    /// 不复用 [`spawn`] 是因为它强依赖 Tauri AppHandle (`spawn_event_forwarder`
    /// 里 `app.emit(...)`),测试场景没有 webview/event 系统跑不起来。
    #[allow(dead_code)] // L1 runner 接入前临时 unused
    pub async fn spawn_headless(bridge: Pinvou3Bridge) -> Result<Self> {
        let engine_config = bridge.build_engine_config();
        let dt_config = bridge.build_dt_config();
        let handle = spawn_engine(engine_config, &dt_config);
        Ok(Self { handle, bridge })
    }

    /// 发用户消息给 Engine。Engine 内部自管 session，多轮自然累积。
    ///
    /// `mode` + `phase` 由 commands::chat 从 SessionStore 取当前 session 的
    /// mode_state，注入 Op::SendMessage。底座按 mode 自动切工具白名单 + sandbox。
    /// M1 弱模型加固:bridge 按 phase 在 user content 前 prepend `<system-reminder>`。
    pub async fn send_user_message(
        &self,
        content: String,
        mode: AppMode,
        persona_reminder: Option<String>,
        restrict_tools: bool,
    ) -> Result<()> {
        let op =
            self.bridge
                .build_send_message_op(content, mode, persona_reminder, restrict_tools);
        self.handle.send(op).await?;
        Ok(())
    }

    /// 取消当前正在生成的回复（点⏹️停止按钮）。
    /// 同步触发 cancel_token，engine turn loop 会立即跳出并发 TurnComplete 事件。
    pub fn cancel_current(&self) {
        self.handle.cancel();
    }

    /// 编辑/重发最后一轮 user 消息（点 ✏️ 编辑或 🔄 重发按钮）。
    /// 上游 [`Op::EditLastTurn`] 行为：砍掉 session 末尾最近的 user 消息及之后
    /// 所有消息，然后用 `new_message` 当成新 user 消息重新发送。
    pub async fn edit_last_turn(&self, new_message: String) -> Result<()> {
        self.handle.send(Op::EditLastTurn { new_message }).await?;
        Ok(())
    }

    /// 手动触发上下文压缩（用户点 token 进度条 → 立即压缩）。
    /// 自动压缩由上游 CompactionConfig.enabled 控制（pinvou3 走默认 = on）。
    pub async fn compact_now(&self) -> Result<()> {
        self.handle.send(Op::CompactContext).await?;
        Ok(())
    }

    /// 提交 request_user_input 工具的用户选择（前端选择气泡点击后调用）。
    /// 底座 `EngineHandle::submit_user_input` 把答案放回 rx_user_input channel,
    /// engine 的 await_user_input loop 收到后把 UserInputResponse 转成 ToolResult。
    pub async fn submit_user_input(
        &self,
        tool_call_id: String,
        response: UserInputResponse,
    ) -> Result<()> {
        self.handle
            .submit_user_input(tool_call_id, response)
            .await?;
        Ok(())
    }

    /// 取消 request_user_input(前端 ✕ 按钮或对话切换时调用)。
    pub async fn cancel_user_input(&self, tool_call_id: String) -> Result<()> {
        self.handle.cancel_user_input(tool_call_id).await?;
        Ok(())
    }

    /// 切换 engine 内部 session 状态：替换 messages + 切到 session-specific
    /// workspace。
    ///
    /// C 方案(P-no-disk): 不再传 `system_prompt` — `EngineConfig.instructions`
    /// 是内存 inline,底座 refresh_system_prompt 自动从中重拼 + 完整替换
    /// `{{PINVOU3_WORKSPACE}}` 占位符。原先 sync 时重写 disk + 传 SystemPrompt::Text
    /// 都是 disk-API-限制的副作用,现在彻底走掉。
    pub async fn sync_session(&self, session_id: String, messages: Vec<Message>) -> Result<()> {
        let workspace = self.bridge.session_workspace(&session_id);
        self.handle
            .send(Op::SyncSession {
                session_id: Some(session_id),
                messages,
                system_prompt: None,
                system_prompt_override: false,
                model: self.bridge.model(),
                workspace,
            })
            .await?;
        Ok(())
    }
}

fn format_instructions(sources: &[deepseek_tui::prompts::InstructionSource]) -> String {
    use deepseek_tui::prompts::InstructionSource;
    if sources.is_empty() {
        "none".to_string()
    } else {
        sources
            .iter()
            .map(|s| match s {
                InstructionSource::File(p) => p.display().to_string(),
                InstructionSource::Inline { name, .. } => format!("inline:{name}"),
            })
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Per-turn 状态：跟踪本 turn 是否调过 plan 类工具 + 最后一次 snapshot。
/// 底座两层 plan 结构：
///   - `update_plan`         → strategy 层（`plan_snapshot`）
///   - `checklist_write` / `todo_write` → leaf task 层（`todos_snapshot`）
/// 任一调过 + plan_phase=Planning → TurnComplete 时 emit `chat:plan_ready`。
/// 参考底座 `tui/ui.rs:1072-1085` 的 `plan_tool_used_in_turn` 判据 +
/// `prompts/modes/plan.md` "Use update_plan ... and checklist_write ..." 双工具引导。
#[derive(Default)]
struct TurnPlanTracker {
    plan_tool_used: bool,
    /// `update_plan` 最近一次结果 JSON：`{ explanation, items: [{step, status}] }`
    /// 上游 `UpdatePlanTool::execute` 返回 "Plan updated: ...\n<json>"，截 \n 后 parse。
    last_plan_snapshot: Option<serde_json::Value>,
    /// `todo_write` / `checklist_write` 最近一次结果 JSON：
    /// `{ items: [{id, content, status}], completion_pct, in_progress_id }`
    /// 上游 `TodoWriteTool::execute` 返回 "Todo list updated (...)\n<json>"。
    last_todos_snapshot: Option<serde_json::Value>,
}

/// [edict-obs] per-role token 账本：role_id → (input 累计, output 累计, 调用次数)。
/// 每收到一条 MailboxMessage::TokenUsage 调 add，返回该 role 最新累计快照。
#[derive(Default)]
struct TokenLedger {
    by_role: std::collections::HashMap<String, (u64, u64, u32)>,
}

impl TokenLedger {
    /// 累加一次调用，返回 (input_total, output_total, calls)。
    fn add(&mut self, role: &str, input: u64, output: u64) -> (u64, u64, u32) {
        let e = self.by_role.entry(role.to_string()).or_insert((0, 0, 0));
        e.0 += input;
        e.1 += output;
        e.2 += 1;
        *e
    }
}

/// [per_page] 把某 fan-out 节点的逐页状态(queued/running/done/retrying)推给前端，
/// 让工作流界面把该节点展开成 N 个 SubAgent chip 实时显示并发。
pub(crate) fn emit_fanout(app: &AppHandle, session_id: &str, base_role: &str) {
    let pages = crate::harness::fanout_snapshot(session_id, base_role);
    let _ = app.emit(
        "workflow:fanout",
        json!({
            "session_id": session_id,
            "base_role": base_role,
            "pages": pages,
        }),
    );
}

/// [pinvou3-fork] 执行一个 [`HarnessAction`](crate::harness::HarnessAction)：emit
/// 前端事件，派发真 SubAgent（SpawnAgent → `Op::SpawnSubAgent`）
/// 或等待/收尾（WaitForHuman/AllDone/Blocked）。由 `TurnComplete`（首轮 step_fresh）
/// 和 `AgentComplete`（SubAgent 完成后推进）两条路径共用。返回 `true` = harness
/// 推进了（调用方据此 emit `workflow:full_state` 快照）。
pub(crate) async fn apply_harness_action(
    action: crate::harness::HarnessAction,
    app: &AppHandle,
    bridge: &Pinvou3Bridge,
    handle: &EngineHandle,
    active_id: &str,
) -> bool {
    use crate::harness::HarnessAction as HA;
    let ws = bridge.session_workspace(active_id);
    match action {
        HA::SpawnAgent {
            role_id,
            role_name,
            prompt,
            allowed_tools,
            max_steps,
            output_schema,
            expects_file_output,
        } => {
            eprintln!(
                "[harness] Step C spawn → {role_name} ({role_id}) tools={allowed_tools:?} max_steps={max_steps:?} structured={}",
                output_schema.is_some()
            );
            crate::audit::append(&ws, "dispatch", &role_id, json!({ "role_name": &role_name }));
            let _ = app.emit(
                "workflow:agent_state_changed",
                json!({
                    "session_id": active_id, "role_id": role_id,
                    "role_name": role_name, "status": "running",
                }),
            );
            let op = Op::SpawnSubAgent {
                prompt,
                role_id,
                allowed_tools,
                max_steps,
                output_schema,
                expects_file_output,
            };
            if let Err(e) = handle.send(op).await {
                eprintln!("[harness] spawn subagent failed: {e:?}");
            }
            true
        }
        // [per_page] 纵向 fan-out：有界并发派发。底座在 running>=max 时硬拒绝(不排队)，
        // 故 Router 运行时自己排队：先派 K 个(per_page_concurrency)，其余留全局队列，由
        // AgentComplete 每页完成补派一个 → 在飞稳定=K。join 计数在 State(record_page_done)；
        // N 实例全到时 AgentComplete handler 对【单一逻辑节点】base_role 验收一次。
        HA::SpawnAgentBatch { base_role, role_name, tasks } => {
            let total = tasks.len();
            let k = crate::harness::per_page_concurrency();
            eprintln!("[harness] Step C fan-out → {role_name} ({base_role}) {total} 页, 在飞并发={k}, 其余排队");
            crate::audit::append(&ws, "dispatch_batch", &base_role, json!({ "role_name": &role_name, "pages": total, "concurrency": k }));
            let _ = app.emit(
                "workflow:agent_state_changed",
                json!({
                    "session_id": active_id, "role_id": &base_role,
                    "role_name": role_name, "status": "running",
                }),
            );
            let first = crate::harness::batch_seed_and_take(active_id, &base_role, tasks, k);
            for t in first {
                let op = Op::SpawnSubAgent {
                    prompt: t.prompt,
                    role_id: t.agent_role, // "slide_writer#p01" → 回到 AgentComplete.role
                    allowed_tools: t.allowed_tools,
                    max_steps: t.max_steps,
                    output_schema: t.output_schema,
                    expects_file_output: t.expects_file_output,
                };
                if let Err(e) = handle.send(op).await {
                    eprintln!("[harness] fan-out spawn failed: {e:?}");
                }
            }
            emit_fanout(app, active_id, &base_role); // 初始 fan-out 状态 → 前端
            true
        }
        HA::WaitForHuman {
            role_id,
            role_name,
            description,
        } => {
            eprintln!("[harness] waiting for human → {role_name} ({role_id})");
            crate::audit::append(&ws, "human_gate", &role_id, json!({ "role_name": &role_name, "description": crate::audit::clip(&description) }));
            let _ = app.emit(
                "workflow:gate_approval",
                json!({
                    "session_id": active_id, "role_id": role_id,
                    "role_name": role_name, "gate_description": description,
                }),
            );
            true
        }
        HA::AllDone => {
            eprintln!("[harness] workflow complete");
            // [edict-obs] 定位最终成品(deck 播放器入口),带进完成事件让前端弹"成品卡"。
            // 找不到(非 deck 类工作流/产物缺失)→ artifact=null,前端只标完成不弹卡。
            let artifact: Option<String> = crate::harness::read_full_agent_state(&ws)
                .and_then(|st| {
                    st.get("project_dir")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .map(|p| std::path::Path::new(&p).join("HTML_Deck").join("index.html"))
                .filter(|p| p.exists())
                .map(|p| p.display().to_string());
            crate::audit::append(&ws, "complete", "", json!({ "artifact": artifact }));
            let _ = app.emit(
                "workflow:complete",
                json!({ "session_id": active_id, "artifact": artifact }),
            );
            true
        }
        HA::Blocked { message } => {
            eprintln!("[harness] blocked: {message}");
            crate::audit::append(&ws, "blocked", "", json!({ "message": crate::audit::clip(&message) }));
            let warmup_report = serde_json::from_str::<serde_json::Value>(&message).ok();
            let _ = app.emit(
                "workflow:blocked",
                json!({
                    "session_id": active_id, "message": message, "warmup_report": warmup_report,
                }),
            );
            true
        }
        HA::Error(e) => {
            eprintln!("[harness] error: {e}");
            false
        }
        HA::NotApplicable => false,
    }
}

/// 后台 task：持续读 rx_event 转 Tauri emit。
///
/// 关键点：监听 `Event::ApprovalRequired` 并主动 `approve_tool_call`——
/// 上游 `Op::SendMessage.auto_approve` 不旁路 `await_tool_approval`
/// （turn_loop.rs:1117 只看 ToolSpec.approval_requirement，不看
/// session.auto_approve），需要 frontend 端主动发 ApprovalDecision::Approved
/// 才能解锁工具执行。
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
fn spawn_event_forwarder(
    app: AppHandle,
    handle: EngineHandle,
    store: SessionStore,
    bridge: Pinvou3Bridge,
    session_id: String,
) -> tauri::async_runtime::JoinHandle<()> {
    let approve_handle = handle.clone();
    let plan_tracker: Arc<Mutex<TurnPlanTracker>> =
        Arc::new(Mutex::new(TurnPlanTracker::default()));
    tauri::async_runtime::spawn(async move {
        // [B2] 完成账本:agent_id -> 决策 role。dedup——同一 agent_id 重复
        // AgentComplete 时跳过推进(防双推进)。角色重派(gate 失败/回滚)得新
        // agent_id,不会被误跳。
        let mut seen_completions: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        // [edict-obs] agent_id→role_id 关联(由 fork 的 AgentSpawned 第二发喂,
        // 见下方 AgentSpawned 臂注释)+ per-role token 账本。
        // 有意不清理:存活期=本 session forwarder,单 run 最多几十条目(含 fan-out 重试),
        // 跟 seen_completions 同模式 —— 别加"AgentComplete 时清理",会破坏 dedup 语义。
        let mut agent_roles: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut token_ledger = TokenLedger::default();
        // app 侧自测推理指标累加器(TTFT/生成速度/累计tokens/KV)。try_state:headless
        // harness / 测试可能没 manage MonitorState,拿不到就整块跳过,不 panic。
        let self_metrics = app
            .try_state::<crate::monitor::MonitorState>()
            .map(|s| s.self_metrics());
        let mut rx = handle.rx_event.write().await;
        while let Some(event) = rx.recv().await {
            match event {
                Event::TurnStarted { .. } => {
                    // 本轮起始打点(TTFT 起点)。底座已发此事件,原先落 `_` 被忽略。
                    if let Some(m) = &self_metrics {
                        m.on_turn_started(&session_id);
                    }
                }
                Event::MessageDelta { content, .. } => {
                    if let Some(m) = &self_metrics {
                        m.on_first_delta(&session_id); // 首个才记 TTFT,幂等
                    }
                    let _ = app.emit(
                        "chat:delta",
                        json!({ "session_id": session_id, "text": content }),
                    );
                }
                Event::ThinkingDelta { .. } => {
                    // Qwen3 已用 reasoning_effort=off 关 thinking，丢这段
                }
                Event::ToolCallStarted { id, name, input } => {
                    if let Some(m) = &self_metrics {
                        m.on_tool(&session_id); // 本轮有工具 → 收尾跳过 TTFT/TPS(D2)
                    }
                    let _ = app.emit(
                        "chat:tool_start",
                        json!({ "session_id": session_id, "id": id, "name": name, "args": input }),
                    );
                }
                Event::ToolCallComplete { id, name, result } => {
                    // 携带 metadata 让前端识别 careful hook 拦截 (safety_level=="dangerous")
                    let (output, success, metadata) = match result {
                        Ok(r) => (r.content, true, r.metadata),
                        Err(e) => (format!("{e:?}"), false, None),
                    };
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
                        let _ = app.emit(
                            "chat:plan_snapshot",
                            json!({
                                "session_id": session_id,
                                "plan_snapshot": plan_emit,
                                "todos_snapshot": todos_emit,
                            }),
                        );
                    }
                    let _ = app.emit(
                        "chat:tool_end",
                        json!({
                            "session_id": session_id,
                            "id": id,
                            "name": name,
                            "output": output,
                            "success": success,
                            "metadata": metadata,
                        }),
                    );
                }
                Event::UserInputRequired { id, request } => {
                    // 底座 emit 这个事件后会 block 在 await_user_input，等 submit_user_input
                    // 或 cancel_user_input。前端渲染选择气泡 → 用户点选 →
                    // invoke('submit_user_input', toolCallId, answers) 解锁。
                    let _ = app.emit(
                        "chat:user_input_required",
                        json!({
                            "session_id": session_id,
                            "id": id,
                            "questions": request.questions,
                        }),
                    );
                }
                Event::ApprovalRequired { id, tool_name, .. } => {
                    // pinvou3 yolo 助手：主动 approve（上游 bug 旁路，见上方注释）
                    eprintln!("[pinvou3-app] auto-approving tool {} id={}", tool_name, id);
                    let h = approve_handle.clone();
                    let id_clone = id.clone();
                    tokio::spawn(async move {
                        if let Err(e) = h.approve_tool_call(id_clone).await {
                            eprintln!("[pinvou3-app] approve_tool_call failed: {e:?}");
                        }
                    });
                    // 不重复 emit chat:tool_start —— 上游 ToolCallStarted（带完整 input）
                    // 已先于 ApprovalRequired fire，前端已收到正确的 args。
                    // 之前在此 emit 会用 args=null 覆盖前端 toolMeta，导致产物路径丢失。
                }
                // [edict-obs] 同一 agent_id 会收到两发 AgentSpawned:第一发来自
                // subagent manager 内部(prompt=任务文本,subagent/mod.rs:1518),
                // 第二发来自 fork 的 Op::SpawnSubAgent 成功臂(prompt=role_id)。
                // FIFO 保证第二发后到 → HashMap::insert last-write-wins,最终值
                // 必为 role_id。这是有意设计,别"去重优化"。
                // v0.8.65 上游给 AgentSpawned 加 parent_run_id/spawn_depth(谱系遥测);
                // pinvou3 forwarder 只用 id→role(prompt)关联,新字段 `..` 忽略。
                Event::AgentSpawned { id, prompt, .. } => {
                    agent_roles.insert(id, prompt);
                }
                // [edict-obs] SubAgent 每步进展(底座自动发,不靠 prompt 纪律)→ 前端看板。
                Event::AgentProgress { id, status, .. } => {
                    // 早期事件可能赶在第二发 AgentSpawned(role_id)之前到 —— 兜底用
                    // agent_id 而不是空串,跟 TokenUsage 臂的 fallback 语义一致。
                    let role = agent_roles.get(&id).cloned().unwrap_or_else(|| id.clone());
                    let _ = app.emit(
                        "workflow:agent_progress",
                        json!({
                            "session_id": session_id,
                            "agent_id": id,
                            "role_id": role,
                            "status": status,
                        }),
                    );
                }
                // [edict-obs] mailbox 信封:工具调用→progress;TokenUsage→账本+审计;
                // Completed/Failed→审计。审计写盘是同步 IO,但单行 append 微秒级,
                // 不值得为它 spawn_blocking(forwarder 本身不在 LLM 关键路径上)。
                Event::SubAgentMailbox { message, .. } => {
                    use deepseek_tui::tools::subagent::MailboxMessage as MM;
                    let ws = bridge.session_workspace(&session_id);
                    match message {
                        MM::TokenUsage { agent_id, model, usage } => {
                            let role = agent_roles
                                .get(&agent_id)
                                .cloned()
                                .unwrap_or_else(|| agent_id.clone());
                            let (input_total, output_total, calls) = token_ledger.add(
                                &role,
                                usage.input_tokens as u64,
                                usage.output_tokens as u64,
                            );
                            let _ = app.emit(
                                "workflow:token_usage",
                                json!({
                                    "session_id": session_id,
                                    "role_id": role,
                                    "agent_id": agent_id,
                                    "model": model,
                                    "input_tokens_total": input_total,
                                    "output_tokens_total": output_total,
                                    "calls": calls,
                                }),
                            );
                            crate::audit::append(&ws, "token", &role, json!({
                                "agent_id": agent_id,
                                "input": usage.input_tokens,
                                "output": usage.output_tokens,
                            }));
                        }
                        MM::ToolCallStarted { agent_id, tool_name, step } => {
                            // 兜底语义跟 AgentProgress 臂一致(agent_id 而非空串)
                            let role = agent_roles
                                .get(&agent_id)
                                .cloned()
                                .unwrap_or_else(|| agent_id.clone());
                            let _ = app.emit(
                                "workflow:agent_progress",
                                json!({
                                    "session_id": session_id,
                                    "agent_id": agent_id,
                                    "role_id": role,
                                    "status": format!("🔧 {tool_name} (step {step})"),
                                }),
                            );
                        }
                        MM::Completed { agent_id, summary } => {
                            let role = agent_roles.get(&agent_id).cloned().unwrap_or_default();
                            crate::audit::append(&ws, "agent_done", &role, json!({
                                "agent_id": agent_id,
                                "summary": crate::audit::clip(&summary),
                            }));
                        }
                        MM::Failed { agent_id, error } => {
                            let role = agent_roles.get(&agent_id).cloned().unwrap_or_default();
                            crate::audit::append(&ws, "agent_failed", &role, json!({
                                "agent_id": agent_id,
                                "error": crate::audit::clip(&error),
                            }));
                        }
                        _ => {}
                    }
                }
                // [pinvou3-fork] 真 SubAgent(Step C)完成 → Step D Gate。executing 期间
                // 主 session 不发 TurnComplete(它空闲,subagent 在 subagent_manager 跑),
                // 所以单角色周期的"完成"信号在这里、不在 TurnComplete。
                Event::AgentComplete { id, result, role: envelope_role, failed } => {
                    eprintln!(
                        "[harness] subagent {id} complete ({} chars summary, failed={failed})",
                        result.len()
                    );
                    let _ = app.emit(
                        "workflow:agent_complete",
                        json!({ "session_id": session_id, "agent_id": id }),
                    );
                    let state = store.mode_state(&session_id);
                    if state.active_skill.is_some() {
                        // [C2] 完成→节点关联完全用信封 role（SDAN Result.from）。
                        // harness_phase 已删,无 phase 兜底——正常 workflow subagent
                        // 派发必带 role(SubAgentAssignment.role)。
                        let decision_role: Option<String> = envelope_role.clone();
                        if decision_role.is_none() {
                            eprintln!("[harness] ⚠️AgentComplete 无 role(非 workflow subagent?) id={id},不推进");
                        }
                        // dedup:同一 agent_id 重复完成 → 跳过,防双推进。
                        // 角色重派(gate 失败/回滚)会得新 agent_id,不会被误跳。
                        let is_dup = seen_completions
                            .insert(id.clone(), decision_role.clone().unwrap_or_default())
                            .is_some();
                        if is_dup {
                            eprintln!("[harness] 重复 AgentComplete id={id},跳过");
                        } else if let Some(role) = decision_role {
                            // [per_page] role 形如 "slide_writer#p01" = fan-out 成员：
                            // 记一页 join 计数，未齐则等其余页（不推进）；齐了才对【单一
                            // 逻辑节点】base_role 验收一次。普通角色 base = role 本身。
                            let base_for_step: Option<String> =
                                if let Some((base, page_str)) = role.split_once('#') {
                                    let page: u32 =
                                        page_str.trim_start_matches('p').parse().unwrap_or(0);
                                    let ws = bridge.session_workspace(&session_id);
                                    // [per_page] 先校验该页【真写成】：SSE 超时/放弃的 agent 也
                                    // emit AgentComplete，但只留空壳骨架。空壳不计 done，自动重派
                                    // 该页(带上限)，挡住空壳混入 batch → 避免 gate pagenum_mismatch
                                    // 误判回滚的死循环。
                                    let outs = crate::harness::batch_outputs_for(&session_id, base, page);
                                    let real =
                                        crate::harness::page_output_is_real(&ws, base, page, &outs);
                                    let mut count_done = real;
                                    if real {
                                        eprintln!("[harness] per_page {role} 真写成 ✓");
                                        crate::harness::fanout_mark(&session_id, base, page, "done");
                                    } else {
                                        let n = crate::harness::page_retry_inc(&session_id, base, page);
                                        let maxr = crate::harness::max_page_retry();
                                        if n <= maxr {
                                            // 空壳 → 重派该页(占用刚释放的在飞名额，不补排队页)。
                                            let ws_r = ws.clone();
                                            let base_r = base.to_string();
                                            let respawn = tokio::task::spawn_blocking(move || {
                                                crate::harness::respawn_page(&ws_r, &base_r, page)
                                            })
                                            .await
                                            .unwrap_or(None);
                                            if let Some(t) = respawn {
                                                let rr = t.agent_role.clone();
                                                let op = Op::SpawnSubAgent {
                                                    prompt: t.prompt,
                                                    role_id: t.agent_role,
                                                    allowed_tools: t.allowed_tools,
                                                    max_steps: t.max_steps,
                                                    output_schema: t.output_schema,
                                                    expects_file_output: t.expects_file_output,
                                                };
                                                if let Err(e) = approve_handle.send(op).await {
                                                    eprintln!("[harness] per_page {role} 空壳→重派 {rr} 失败: {e:?}");
                                                } else {
                                                    eprintln!("[harness] per_page {role} 空壳→重派(第{n}/{maxr}次) {rr}");
                                                    crate::harness::fanout_mark(&session_id, base, page, "retrying");
                                                }
                                            } else {
                                                eprintln!("[harness] per_page {role} 空壳但 respawn 取不到任务 → 兜底计 done");
                                                count_done = true;
                                                crate::harness::fanout_mark(&session_id, base, page, "done");
                                            }
                                        } else {
                                            eprintln!("[harness] per_page {role} 空壳且重试耗尽({maxr}) → 兜底计 done(留给 gate/人工)");
                                            count_done = true;
                                            crate::harness::fanout_mark(&session_id, base, page, "done");
                                        }
                                    }
                                    // 每页完成/重试后把最新 fan-out 状态推给前端工作流界面。
                                    emit_fanout(&app, &session_id, base);
                                    if count_done {
                                        let ws_d = ws.clone();
                                        let base_d = base.to_string();
                                        let complete = tokio::task::spawn_blocking(move || {
                                            crate::harness::record_page_done(&ws_d, &base_d, page)
                                        })
                                        .await
                                        .unwrap_or(true);
                                        eprintln!(
                                            "[harness] per_page {role} done; batch_complete={complete}"
                                        );
                                        if complete {
                                            crate::harness::batch_clear(&session_id, base);
                                            Some(base.to_string())
                                        } else {
                                            // 未齐 → 补派下一排队页，维持在飞并发=K；不推进。
                                            if let Some(t) =
                                                crate::harness::batch_pop_next(&session_id, base)
                                            {
                                                let next_role = t.agent_role.clone();
                                                let op = Op::SpawnSubAgent {
                                                    prompt: t.prompt,
                                                    role_id: t.agent_role,
                                                    allowed_tools: t.allowed_tools,
                                                    max_steps: t.max_steps,
                                                    output_schema: t.output_schema,
                                                    expects_file_output: t.expects_file_output,
                                                };
                                                if let Err(e) = approve_handle.send(op).await {
                                                    eprintln!("[harness] per_page 补派 {next_role} 失败: {e:?}");
                                                } else {
                                                    eprintln!("[harness] per_page 补派下一页 {next_role}");
                                                }
                                            }
                                            None
                                        }
                                    } else {
                                        // 重派路径：等该页重试结果，不推进、不补排队页。
                                        None
                                    }
                                } else {
                                    Some(role.clone())
                                };

                            if let Some(base_role) = base_for_step {
                                let ws = bridge.session_workspace(&session_id);
                                // [2026-06-06] SubAgent 执行失败(failed=true,单角色)绝不走 gate：
                                // gate 只验产物存在+非空，会拿【上一轮陈旧产物】把失败洗成 PASS
                                // (实锤:web_search 不可用→PM 0步即死→旧 brief 过关)。改走
                                // agent_failed:--fail 计次→重派带失败原因，耗尽→Blocked。
                                // per_page 成员(role 带 #)不走这里:空壳检测已按页处理。
                                let failed_single = failed && !role.contains('#');
                                let err_text = result.clone();
                                let action = tokio::task::spawn_blocking(move || {
                                    if failed_single {
                                        crate::harness::agent_failed(&ws, &base_role, &err_text)
                                    } else {
                                        crate::harness::step_after_role(&ws, &base_role)
                                    }
                                })
                                .await
                                .unwrap_or_else(|_| {
                                    crate::harness::HarnessAction::Error("spawn_blocking panicked".into())
                                });
                                let handled = apply_harness_action(
                                    action,
                                    &app,
                                    &bridge,
                                    &approve_handle,
                                    &session_id,
                                )
                                .await;
                                if handled {
                                    let ws = bridge.session_workspace(&session_id);
                                    let app_clone = app.clone();
                                    let sid_clone = session_id.clone();
                                    tokio::task::spawn_blocking(move || {
                                        if let Some(mut st) = crate::harness::read_full_agent_state(&ws) {
                                            if let Some(obj) = st.as_object_mut() {
                                                obj.insert("session_id".into(), json!(sid_clone));
                                            }
                                            let _ = app_clone.emit("workflow:full_state", st);
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
                Event::TurnComplete {
                    usage,
                    status,
                    error,
                    // v0.8.49 上游新增 tool_catalog / base_url(调试/审计用),pinvou3 不消费
                    ..
                } => {
                    // 单独发 usage 给前端 token 进度条
                    let _ = app.emit(
                        "chat:usage",
                        json!({
                            "session_id": session_id,
                            "input_tokens": usage.input_tokens,
                            "output_tokens": usage.output_tokens,
                        }),
                    );
                    // app 侧自测:用精确 usage 收尾本轮(过滤 output==0 的非 LLM/错误/取消轮,
                    // 见 Usage::default() 内联 shell 轮 / zero_usage 错误路径)。KV 白捡 D3。
                    if let Some(m) = &self_metrics {
                        if usage.output_tokens > 0 {
                            m.on_turn_complete(
                                &session_id,
                                usage.input_tokens,
                                usage.output_tokens,
                                usage.prompt_cache_hit_tokens,
                                usage.prompt_cache_miss_tokens,
                            );
                        }
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

                    {
                        // 多引擎:mode 判据基于本 forwarder 的 session_id,不读全局 active。
                        let active_id = session_id.clone();
                        let state = store.mode_state(&active_id);

                        // ── plan_ready 触发(Plan + 调过 plan 类工具) ──
                        // 底座式:Plan 模式调过 update_plan = 出方案 → 弹决策卡。不再设 phase
                        // (已砍 PlanPhase),mode 留 Plan 直到用户 accept/discard 切 Yolo。
                        // 修订(还在 Plan 时再调 update_plan)天然幂等:再弹一张新卡。
                        if plan_used && state.mode == SerializableMode::Plan {
                            let _ = app.emit(
                                "chat:plan_ready",
                                json!({
                                    "session_id": active_id.clone(),
                                    "plan_snapshot": plan_snapshot.clone(),
                                    "todos_snapshot": todos_snapshot.clone(),
                                }),
                            );
                        }

                        // ── H1: Harness Loop (skill session 图执行器) ──
                        // 本 session 绑定了 skill 且项目目录有 workflow_progress.json →
                        // harness 图执行器驱动下一步。workspace 用**本 session 的**目录
                        // (多 session 并发:每个工作流的项目落在各自 session workspace)。
                        let harness_workspace = bridge.session_workspace(&session_id);
                        let harness_handled = if state.active_skill.is_some() {
                            // [C2] harness_phase 已删。工作流由 kick(命令)+ AgentComplete
                            // 驱动;主 session 在工作流期间空闲,此处 TurnComplete 一般不参与
                            // 推进。仍兜底走 step_fresh:由 scheduler 据 State 决策——有角色在
                            // 跑则返回 role_running→NotApplicable(防重复派发),否则派下一个。
                            let ws = harness_workspace.clone();
                            let action = tokio::task::spawn_blocking(move || {
                                crate::harness::step_fresh(&ws)
                            })
                            .await
                            .unwrap_or(crate::harness::HarnessAction::Error(
                                "spawn_blocking panicked".into(),
                            ));

                            apply_harness_action(action, &app, &bridge, &approve_handle, &active_id)
                                .await
                        } else {
                            false
                        };

                        // ── H1b: harness 推进了 → 推送全量 agent 状态快照给前端 ──
                        if harness_handled {
                            let ws = harness_workspace.clone();
                            let app_clone = app.clone();
                            let sid_clone = active_id.clone();
                            tokio::task::spawn_blocking(move || {
                                if let Some(mut state) = crate::harness::read_full_agent_state(&ws) {
                                    if let Some(obj) = state.as_object_mut() {
                                        obj.insert("session_id".into(), json!(sid_clone));
                                    }
                                    let _ = app_clone.emit("workflow:full_state", state);
                                }
                            });
                        }

                        // ── [回归底座式] M2 自驱 + M3 文本兜底已彻底砍掉 ──
                        // M2:执行不自动续跑,回底座由用户驱动。M3(Plan 写了方案没调
                        // update_plan 的救援)放弃不做:底座 update_plan→plan_ready→方案卡
                        // 这条链已可靠,漏的少数"光说不出卡"由 composer chip 手切 + plan_stuck
                        // 卡兜底,不值得用噪音判据再造一层。
                    }
                    let _ = app.emit(
                        "chat:done",
                        json!({ "session_id": session_id, "status": format!("{status:?}"), "error": error }),
                    );
                }
                Event::CompactionStarted { message, auto, .. } => {
                    let _ = app.emit(
                        "chat:compaction",
                        json!({ "session_id": session_id, "phase": "start", "auto": auto, "message": message }),
                    );
                }
                Event::CompactionCompleted {
                    message,
                    auto,
                    messages_before,
                    messages_after,
                    ..
                } => {
                    let _ = app.emit(
                        "chat:compaction",
                        json!({
                            "session_id": session_id,
                            "phase": "done",
                            "auto": auto,
                            "message": message,
                            "messages_before": messages_before,
                            "messages_after": messages_after,
                        }),
                    );
                }
                Event::CompactionFailed { message, auto, .. } => {
                    let _ = app.emit(
                        "chat:compaction",
                        json!({ "session_id": session_id, "phase": "fail", "auto": auto, "message": message }),
                    );
                }
                Event::Error { envelope, .. } => {
                    // 可恢复错误(如 SSE idle timeout、瞬态工具失败)turn 不会结束——
                    // 引擎会 retry / 继续跑后续步骤。绝不能发 chat:done,否则前端
                    // setBusy(false) 把"思考中"指示器掐掉,而引擎还在干活,看着像卡死
                    // (且会误触发 flush/closeBubble/plan_phase 收尾)。只飘个 advisory。
                    // 仅 recoverable==false(致命)才是真结束 → chat:done。
                    if envelope.recoverable {
                        let _ = app.emit(
                            "chat:transient_error",
                            json!({ "session_id": session_id, "error": envelope.message }),
                        );
                    } else {
                        let _ = app.emit(
                            "chat:done",
                            json!({ "session_id": session_id, "status": "error", "error": envelope.message }),
                        );
                        // [C2] harness_phase 已删。工作流绑定(active_skill)的 session 遇
                        // 致命错误(SubAgent 派发失败 / 内部 fatal)→ 可能收不到 AgentComplete
                        // = 死锁。兜底:emit blocked 通知前端,让用户看到中断可重开(宁可多
                        // 通知一次,不可无声卡死等永不到来的事件)。
                        let was_active = store.mode_state(&session_id).active_skill.is_some();
                        if was_active {
                            let _ = app.emit(
                                "workflow:blocked",
                                json!({
                                    "session_id": session_id,
                                    "message": "工作流执行中断（致命错误，可能是 SubAgent 派发失败或内部错误），请检查后重新开始。",
                                }),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        eprintln!(
            "[pinvou3-app] event forwarder stopped for session {session_id} (engine shut down?)"
        );
    })
}

/// 让 main.rs 编译时知道这个模块（供 docs/CI 用）。
pub fn _force_link() -> Arc<()> {
    Arc::new(())
}

#[cfg(test)]
mod token_ledger_tests {
    use super::TokenLedger;

    #[test]
    fn accumulates_per_role() {
        let mut l = TokenLedger::default();
        assert_eq!(l.add("pm", 100, 20), (100, 20, 1));
        assert_eq!(l.add("pm", 50, 10), (150, 30, 2));
        assert_eq!(l.add("writer", 7, 3), (7, 3, 1));
        assert_eq!(l.add("pm", 0, 0), (150, 30, 3));
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::bridge::mode_state::SerializableMode;
    use crate::monitor::SelfMetrics;

    /// 真机集成(#[ignore]):打真 vLLM 跑一轮,drain rx_event 时**照 forwarder 四臂
    /// 原样喂 SelfMetrics**,证明真实事件流(TurnStarted→MessageDelta→TurnComplete+真
    /// usage)累加出合理指标 + 事件顺序符合预期(TurnStarted 在首 MessageDelta 前)。
    ///
    ///   DEEPSEEK_ALLOW_INSECURE_HTTP=1 DEEPSEEK_FORCE_HTTP1=1 \
    ///   cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml \
    ///     --lib engine::live_tests::self_metrics_populates_from_real_turn \
    ///     -- --ignored --nocapture
    #[ignore]
    #[tokio::test]
    async fn self_metrics_populates_from_real_turn() {
        std::env::set_var("DEEPSEEK_ALLOW_INSECURE_HTTP", "1");
        std::env::set_var("DEEPSEEK_FORCE_HTTP1", "1");
        std::env::set_var("PINVOU3_SKIP_WARMUP", "1");

        let bridge = Pinvou3Bridge::boot().expect("boot bridge");
        let engine = AppEngine::spawn_headless(bridge).await.expect("spawn engine");

        let m = SelfMetrics::default();
        let sid = "live-test";
        let prompts = ["用一句话介绍你自己。", "再用一句话讲个冷笑话。"];

        // 跑两轮:首轮 = 冷/warmup(A 跳过 TTFT/TPS),二轮 = 暖(记)。
        engine
            .send_user_message(prompts[0].to_string(), SerializableMode::Yolo.to_app_mode(), None, false)
            .await
            .expect("send_user_message #1");

        let mut rx = engine.handle.rx_event.write().await;
        let mut turns_done = 0usize;
        let mut seq: Vec<String> = Vec::new();
        let mut tool_in_turn2 = false;
        loop {
            let ev = tokio::time::timeout(std::time::Duration::from_secs(90), rx.recv())
                .await
                .expect("timeout waiting for event");
            let Some(ev) = ev else { break };
            let turn = turns_done + 1;
            match ev {
                Event::TurnStarted { .. } => {
                    seq.push(format!("t{turn}:TurnStarted"));
                    m.on_turn_started(sid);
                }
                Event::MessageDelta { .. } => {
                    m.on_first_delta(sid);
                }
                Event::ToolCallStarted { .. } => {
                    seq.push(format!("t{turn}:ToolCallStarted"));
                    m.on_tool(sid);
                    if turns_done == 1 {
                        tool_in_turn2 = true;
                    }
                }
                Event::TurnComplete { usage, .. } => {
                    seq.push(format!("t{turn}:TurnComplete(out={})", usage.output_tokens));
                    if usage.output_tokens > 0 {
                        m.on_turn_complete(
                            sid,
                            usage.input_tokens,
                            usage.output_tokens,
                            usage.prompt_cache_hit_tokens,
                            usage.prompt_cache_miss_tokens,
                        );
                    }
                    turns_done += 1;
                    if turns_done == 1 {
                        engine
                            .send_user_message(prompts[1].to_string(), SerializableMode::Yolo.to_app_mode(), None, false)
                            .await
                            .expect("send_user_message #2");
                    } else {
                        break;
                    }
                }
                _ => {}
            }
        }

        let s = m.snapshot();
        eprintln!("[live] event seq: {seq:?}");
        eprintln!(
            "[live] snapshot: ttft_count={} ttft_sum_s={:.4} tps_tokens={} tps_time_s={:.4} gen={} prompt={} cache_hit={} cache_miss={}",
            s.ttft_count, s.ttft_sum_s, s.tps_tokens, s.tps_time_s,
            s.gen_tokens_total, s.prompt_tokens_total, s.cache_hit_tokens, s.cache_miss_tokens
        );
        if s.ttft_count > 0 {
            eprintln!(
                "[live] → 稳态 TTFT={:.3}s  TPS={:.1} tok/s (已排除首轮冷启)",
                s.ttft_sum_s / s.ttft_count as f64,
                if s.tps_time_s > 0.0 { s.tps_tokens as f64 / s.tps_time_s } else { 0.0 }
            );
        }

        assert_eq!(turns_done, 2, "未跑满两轮 seq={seq:?}");
        assert!(s.gen_tokens_total > 0, "无 output token 累加(usage 空?) seq={seq:?}");
        // 二轮纯文本(无工具)才断言:首轮已被 A 跳过,TTFT 应只来自二轮。
        if !tool_in_turn2 {
            assert_eq!(s.ttft_count, 1, "二轮应恰好记 1 次 TTFT(首轮跳过) seq={seq:?}");
            assert!(s.tps_time_s > 0.0, "TPS 时长未记 seq={seq:?}");
        }
    }
}
