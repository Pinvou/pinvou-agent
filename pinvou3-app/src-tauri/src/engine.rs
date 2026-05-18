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

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use deepseek_tui::core::engine::{spawn_engine, EngineHandle};
use deepseek_tui::core::events::Event;
use deepseek_tui::core::ops::Op;
use deepseek_tui::models::{Message, SystemPrompt};
use deepseek_tui::tools::user_input::UserInputResponse;
use deepseek_tui::tui::app::AppMode;
use parking_lot::Mutex;
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::bridge::mode_state::{PlanPhase, SerializableMode};
use crate::bridge::sessions::SessionStore;
use crate::bridge::Pinvou3Bridge;

/// Tauri State 持有。前端通过 `invoke('chat', ...)` 间接调到这里。
#[derive(Clone)]
pub struct AppEngine {
    pub handle: EngineHandle,
    pub bridge: Pinvou3Bridge,
}

impl AppEngine {
    /// 启动序列：boot bridge → build configs → spawn engine → 启 event forwarder。
    /// 必须在 Tauri `setup()` 的异步上下文里调。
    pub async fn spawn(app: AppHandle, store: SessionStore) -> Result<Self> {
        let bridge = Pinvou3Bridge::boot()?;
        let engine_config = bridge.build_engine_config();
        let dt_config = bridge.build_dt_config();

        eprintln!(
            "[pinvou3-app] spawn_engine model={} workspace={} skills_dir={} instructions={}",
            engine_config.model,
            engine_config.workspace.display(),
            engine_config.skills_dir.display(),
            format_instructions(&engine_config.instructions),
        );

        let handle = spawn_engine(engine_config, &dt_config);
        spawn_event_forwarder(app, handle.clone(), store, bridge.clone());

        Ok(Self { handle, bridge })
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
        phase: PlanPhase,
    ) -> Result<()> {
        let op = self.bridge.build_send_message_op(content, mode, phase);
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
    /// workspace + 重拼 system_prompt (把 PINVOU3_WORKSPACE 占位符换成
    /// 该 session 的独立 workspace 目录)。
    ///
    /// 实施动机:
    /// - 不切 engine.messages → 上下文跨 session 串台
    /// - 不切 workspace → AI 默认产物目录全局共享,多 session 写同名文件冲突
    /// - 不重拼 system_prompt → AI 看到的 PINVOU3_WORKSPACE 路径跟实际 workspace 不一致
    pub async fn sync_session(&self, session_id: String, messages: Vec<Message>) -> Result<()> {
        let workspace = self.bridge.session_workspace(&session_id);
        // 重写 disk 上的 instructions.md 为 session-specific 路径。
        // engine 的 rehydrate 会从 disk 重读覆盖 session.system_prompt,
        // 所以必须改 disk 才能让 AI 看到正确的 PINVOU3_WORKSPACE。
        if let Err(e) = self.bridge.rewrite_instructions_for_session(&session_id) {
            eprintln!("[sync_session] rewrite instructions failed: {e}");
        }
        let prompt_text = self.bridge.build_session_system_prompt(&session_id);
        self.handle
            .send(Op::SyncSession {
                session_id: Some(session_id),
                messages,
                system_prompt: Some(SystemPrompt::Text(prompt_text)),
                system_prompt_override: false,
                model: self.bridge.model(),
                workspace,
            })
            .await?;
        Ok(())
    }
}

fn format_instructions(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "none".to_string()
    } else {
        paths
            .iter()
            .map(|p| p.display().to_string())
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
    /// 本 turn 是否调过任何会真正改变世界的工具(write_file / edit_file /
    /// exec_shell / code_execution)。M2 自驱判据:Executing 态 turn end +
    /// write_tool_used==false → 是"假执行",触发 auto-continue。
    write_tool_used: bool,
    /// 本 turn assistant text 累积字符数。M3 判据:Planning 态 + 没调 plan 工具 +
    /// text > 阈值 + 含关键词 → 弹文本兜底卡片。
    assistant_text_len: usize,
}

/// 判断 plan_snapshot 是否还有"未完成"的步骤(status != "completed")。
/// snapshot 结构:{ explanation, items: [{step, status}] }。
fn plan_has_pending(snapshot: &serde_json::Value) -> bool {
    snapshot
        .get("items")
        .and_then(|v| v.as_array())
        .map(|items| {
            items.iter().any(|item| {
                item.get("status")
                    .and_then(|s| s.as_str())
                    .map(|s| s != "completed")
                    .unwrap_or(true)
            })
        })
        .unwrap_or(false)
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
fn spawn_event_forwarder(
    app: AppHandle,
    handle: EngineHandle,
    store: SessionStore,
    bridge: Pinvou3Bridge,
) {
    let approve_handle = handle.clone();
    let plan_tracker: Arc<Mutex<TurnPlanTracker>> =
        Arc::new(Mutex::new(TurnPlanTracker::default()));
    tauri::async_runtime::spawn(async move {
        let mut rx = handle.rx_event.write().await;
        while let Some(event) = rx.recv().await {
            match event {
                Event::MessageDelta { content, .. } => {
                    plan_tracker.lock().assistant_text_len += content.chars().count();
                    let _ = app.emit("chat:delta", json!({ "text": content }));
                }
                Event::ThinkingDelta { .. } => {
                    // Qwen3 已用 reasoning_effort=off 关 thinking，丢这段
                }
                Event::ToolCallStarted { id, name, input } => {
                    let _ = app.emit(
                        "chat:tool_start",
                        json!({ "id": id, "name": name, "args": input }),
                    );
                }
                Event::ToolCallComplete { id, name, result } => {
                    let (output, success) = match result {
                        Ok(r) => (r.content, true),
                        Err(e) => (format!("{e:?}"), false),
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
                        let active_id_for_emit = store.active_id();
                        drop(tracker);
                        let _ = app.emit(
                            "chat:plan_snapshot",
                            json!({
                                "session_id": active_id_for_emit,
                                "plan_snapshot": plan_emit,
                                "todos_snapshot": todos_emit,
                            }),
                        );
                    }
                    // M2: 跟踪本 turn 是否调过会真正产出的工具
                    if success
                        && matches!(
                            name.as_str(),
                            "write_file" | "edit_file" | "exec_shell" | "code_execution"
                        )
                    {
                        plan_tracker.lock().write_tool_used = true;
                    }
                    let _ = app.emit(
                        "chat:tool_end",
                        json!({ "id": id, "name": name, "output": output, "success": success }),
                    );
                }
                Event::UserInputRequired { id, request } => {
                    // 底座 emit 这个事件后会 block 在 await_user_input，等 submit_user_input
                    // 或 cancel_user_input。前端渲染选择气泡 → 用户点选 →
                    // invoke('submit_user_input', toolCallId, answers) 解锁。
                    let _ = app.emit(
                        "chat:user_input_required",
                        json!({
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
                Event::TurnComplete {
                    usage,
                    status,
                    error,
                } => {
                    // 单独发 usage 给前端 token 进度条
                    let _ = app.emit(
                        "chat:usage",
                        json!({
                            "input_tokens": usage.input_tokens,
                            "output_tokens": usage.output_tokens,
                        }),
                    );
                    // turn end:取出 tracker 快照,然后重置(下个 turn 重新累积)
                    let mut tracker = plan_tracker.lock();
                    let plan_used = tracker.plan_tool_used;
                    let write_used = tracker.write_tool_used;
                    let text_len = tracker.assistant_text_len;
                    let plan_snapshot = tracker.last_plan_snapshot.take();
                    let todos_snapshot = tracker.last_todos_snapshot.take();
                    *tracker = TurnPlanTracker::default();
                    drop(tracker);

                    if let Some(active_id) = store.active_id() {
                        let state = store.mode_state(&active_id);

                        // ── plan_ready 触发(Plan + 调过 plan 类工具) ──
                        // 覆盖两个场景:
                        //   1. Planning: 初次出方案 (phase: Planning → Ready)
                        //   2. Ready (修订): 用户在 Ready 态发新消息让 AI 重出方案 (phase 保持 Ready,
                        //      set_plan_phase(Ready) 幂等). 修法 D: revise 不切回 Planning,
                        //      所以触发条件不能绑死 Planning,否则新卡片不弹.
                        if plan_used
                            && state.mode == SerializableMode::Plan
                            && matches!(state.plan_phase, PlanPhase::Planning | PlanPhase::Ready)
                        {
                            store.set_plan_phase(&active_id, PlanPhase::Ready);
                            let _ = app.emit(
                                "chat:plan_ready",
                                json!({
                                    "session_id": active_id.clone(),
                                    "plan_snapshot": plan_snapshot.clone(),
                                    "todos_snapshot": todos_snapshot.clone(),
                                }),
                            );
                        }

                        // ── M2: Executing 自驱(turn 内只标 in_progress 没真产出) ──
                        // 条件:mode=Yolo + phase=Executing + 本 turn 没调 write 类工具 +
                        // plan 仍有未完成步骤 + auto_continue 未到上限。
                        // 满足 → 自动 send "继续" 用户消息(注:不重置 turn,作为新 user message)
                        let plan_pending = plan_snapshot
                            .as_ref()
                            .map(plan_has_pending)
                            .unwrap_or(false);
                        if state.mode == SerializableMode::Yolo
                            && state.plan_phase == PlanPhase::Executing
                            && !write_used
                            && plan_pending
                        {
                            let n = store.bump_auto_continue(&active_id);
                            if n <= 3 {
                                eprintln!(
                                    "[pinvou3-app] M2 auto-continue #{} (Executing 假执行)",
                                    n
                                );
                                let bridge_clone = bridge.clone();
                                let handle_clone = approve_handle.clone();
                                tauri::async_runtime::spawn(async move {
                                    let op = bridge_clone.build_send_message_op(
                                        "继续执行下一步,记得用 write_file/edit_file/exec_shell \
                                         实际产出文件,不要只调 update_plan 标记进度。"
                                            .to_string(),
                                        deepseek_tui::tui::app::AppMode::Yolo,
                                        PlanPhase::Executing,
                                    );
                                    if let Err(e) = handle_clone.send(op).await {
                                        eprintln!("[M2 auto-continue] send failed: {e:?}");
                                    }
                                });
                            } else {
                                // 超上限 → 弹兜底事件让前端展示卡顿提示
                                let _ = app.emit(
                                    "chat:execution_stuck",
                                    json!({ "session_id": active_id.clone(), "auto_continue_tried": n }),
                                );
                            }
                        }

                        // ── M3: Planning 文本兜底(没调 plan 工具但 text 长 + 含关键词) ──
                        if !plan_used
                            && state.mode == SerializableMode::Plan
                            && state.plan_phase == PlanPhase::Planning
                            && text_len > 300
                        {
                            // 关键词在 text 累积里检测——但 text 已 stream 到前端,
                            // 后端没保留完整 string。改用 text_len 阈值 + 让前端做关键词判断。
                            // 这里只 emit 事件,前端从 messages 数组拿最后一条 assistant text 检关键词。
                            let _ = app.emit(
                                "chat:plan_text_fallback",
                                json!({
                                    "session_id": active_id.clone(),
                                    "text_len": text_len,
                                }),
                            );
                        }
                    }
                    let _ = app.emit(
                        "chat:done",
                        json!({ "status": format!("{status:?}"), "error": error }),
                    );
                }
                Event::CompactionStarted { message, auto, .. } => {
                    let _ = app.emit(
                        "chat:compaction",
                        json!({ "phase": "start", "auto": auto, "message": message }),
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
                        json!({ "phase": "fail", "auto": auto, "message": message }),
                    );
                }
                Event::Error { envelope, .. } => {
                    let _ = app.emit(
                        "chat:done",
                        json!({ "status": "error", "error": envelope.message }),
                    );
                }
                _ => {}
            }
        }
        eprintln!("[pinvou3-app] event forwarder stopped (engine shut down?)");
    });
}

/// 让 main.rs 编译时知道这个模块（供 docs/CI 用）。
pub fn _force_link() -> Arc<()> {
    Arc::new(())
}
