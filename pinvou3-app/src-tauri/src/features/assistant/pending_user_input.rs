//! 进程内 pending `request_user_input` 登记（按 session）。
//!
//! 底座 engine 的 await_user_input 状态只活在 engine 进程里，`chat:user_input_required`
//! 事件发一次不重发。前端代码页（CodexAcpView）的会话 lane 随组件卸载销毁后，
//! 挂起的确认卡无从还原。本模块在事件发射点登记、工具收口或 turn 终态时清除，
//! 供 `get_pending_user_inputs` 命令查询，让前端 remount 后恢复确认卡。
//! 进程重启后 engine lazy spawn，挂起的 input 随旧 engine 消失，map 随进程消亡，
//! 无需持久化。

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PendingUserInput {
    pub id: String,
    pub questions: serde_json::Value,
}

#[derive(Debug, Default)]
struct PendingUserInputs {
    by_session: HashMap<String, Vec<PendingUserInput>>,
}

static PENDING: Mutex<Option<PendingUserInputs>> = Mutex::new(None);

fn with_store<T>(f: impl FnOnce(&mut PendingUserInputs) -> T) -> T {
    let mut guard = PENDING.lock().unwrap_or_else(|poison| poison.into_inner());
    f(guard.get_or_insert_with(PendingUserInputs::default))
}

/// 发射 `chat:user_input_required` 时登记（同 id 去重，重放不重复出卡）。
pub fn record(session_id: &str, id: &str, questions: serde_json::Value) {
    with_store(|store| {
        let entries = store.by_session.entry(session_id.to_string()).or_default();
        if entries.iter().any(|entry| entry.id == id) {
            return;
        }
        entries.push(PendingUserInput {
            id: id.to_string(),
            questions,
        });
    });
}

/// 工具收口（submit/cancel 后底座发 request_user_input 的 tool_end）时清除。
pub fn clear(session_id: &str, tool_call_id: &str) {
    with_store(|store| {
        if let Some(entries) = store.by_session.get_mut(session_id) {
            entries.retain(|entry| entry.id != tool_call_id);
            if entries.is_empty() {
                store.by_session.remove(session_id);
            }
        }
    });
}

/// turn 终态兜底清空该 session 的全部挂起（取消/中断时可能没有 tool_end）。
pub fn clear_session(session_id: &str) {
    with_store(|store| {
        store.by_session.remove(session_id);
    });
}

pub fn list(session_id: &str) -> Vec<PendingUserInput> {
    with_store(|store| {
        store
            .by_session
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_clear_and_terminal_cleanup() {
        let session = "pending-input-unit-test";
        clear_session(session);
        record(session, "call-1", serde_json::json!([{ "id": "q1" }]));
        record(session, "call-1", serde_json::json!([{ "id": "q1" }]));
        record(session, "call-2", serde_json::json!([{ "id": "q2" }]));
        assert_eq!(list(session).len(), 2, "同 id 登记去重");

        clear(session, "call-1");
        let remaining = list(session);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "call-2");

        clear_session(session);
        assert!(list(session).is_empty());
    }
}
