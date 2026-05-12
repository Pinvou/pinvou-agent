//! Tauri 命令实现。前端通过 `invoke('chat', { message })` 调到这里。
//!
//! 只暴露 Week 1 必须的 `chat` 命令。Engine 事件通过 `engine::spawn_event_forwarder`
//! 异步推到前端，本命令立即返回（不等回复完成）。

use tauri::State;

use crate::engine::AppEngine;

/// 接收用户消息，转发给 Engine。
/// 不等 Engine 处理完成 —— LLM 流式输出会通过 Tauri Event 异步推给前端。
#[tauri::command]
pub async fn chat(
    message: String,
    engine: State<'_, AppEngine>,
) -> Result<(), String> {
    if message.trim().is_empty() {
        return Err("empty message".to_string());
    }
    engine
        .inner()
        .send_user_message(message)
        .await
        .map_err(|e| format!("send_user_message failed: {e:?}"))
}
