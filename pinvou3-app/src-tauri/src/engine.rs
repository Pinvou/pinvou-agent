//! pinvou3-app 与 DeepSeek-TUI Engine 的桥接层。
//!
//! 职责：
//!  1. 通过 [`bridge::Pinvou3Bridge`] 把 `~/.pinvou3/settings.json` 翻译成
//!     [`EngineConfig`] / [`DtConfig`]，然后 `spawn_engine`，存到 Tauri State
//!  2. 后台 task 持续读 `EngineHandle::rx_event`，转译成 Tauri 事件
//!     （`chat:delta` / `chat:tool_start` / `chat:tool_end` / `chat:done`）
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
use serde_json::json;
use tauri::{AppHandle, Emitter};

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
    pub async fn spawn(app: AppHandle) -> Result<Self> {
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
        spawn_event_forwarder(app, handle.clone());

        Ok(Self { handle, bridge })
    }

    /// 发用户消息给 Engine。Engine 内部自管 session，多轮自然累积。
    pub async fn send_user_message(&self, content: String) -> Result<()> {
        let op = self.bridge.build_send_message_op(content);
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
        self.handle
            .send(Op::EditLastTurn { new_message })
            .await?;
        Ok(())
    }

    /// 手动触发上下文压缩（用户点 token 进度条 → 立即压缩）。
    /// 自动压缩由上游 CompactionConfig.enabled 控制（pinvou3 走默认 = on）。
    pub async fn compact_now(&self) -> Result<()> {
        self.handle.send(Op::CompactContext).await?;
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
    pub async fn sync_session(
        &self,
        session_id: String,
        messages: Vec<Message>,
    ) -> Result<()> {
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

/// 后台 task：持续读 rx_event 转 Tauri emit。
///
/// 关键点：监听 `Event::ApprovalRequired` 并主动 `approve_tool_call`——
/// 上游 `Op::SendMessage.auto_approve` 不旁路 `await_tool_approval`
/// （turn_loop.rs:1117 只看 ToolSpec.approval_requirement，不看
/// session.auto_approve），需要 frontend 端主动发 ApprovalDecision::Approved
/// 才能解锁工具执行。
fn spawn_event_forwarder(app: AppHandle, handle: EngineHandle) {
    let approve_handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let mut rx = handle.rx_event.write().await;
        while let Some(event) = rx.recv().await {
            match event {
                Event::MessageDelta { content, .. } => {
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
                    let _ = app.emit(
                        "chat:tool_end",
                        json!({ "id": id, "name": name, "output": output, "success": success }),
                    );
                }
                Event::ApprovalRequired { id, tool_name, .. } => {
                    // pinvou3 yolo 助手：主动 approve（上游 bug 旁路，见上方注释）
                    eprintln!(
                        "[pinvou3-app] auto-approving tool {} id={}",
                        tool_name, id
                    );
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
                Event::TurnComplete { usage, status, error } => {
                    // 单独发 usage 给前端 token 进度条
                    let _ = app.emit(
                        "chat:usage",
                        json!({
                            "input_tokens": usage.input_tokens,
                            "output_tokens": usage.output_tokens,
                        }),
                    );
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
                    message, auto, messages_before, messages_after, ..
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
