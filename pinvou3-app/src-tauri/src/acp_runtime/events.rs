use std::collections::HashMap;
use std::sync::Arc;

use agent_client_protocol::schema::{
    ContentBlock, SessionNotification, SessionUpdate, ToolCall, ToolCallStatus, ToolCallUpdate,
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

#[derive(Clone)]
pub struct EventBridge {
    app: AppHandle,
    pinvou_session_id: String,
    tools: Arc<Mutex<HashMap<String, ToolCall>>>,
    latest_plan: Arc<Mutex<Option<Value>>>,
}

impl EventBridge {
    pub fn new(app: AppHandle, pinvou_session_id: String) -> Self {
        Self {
            app,
            pinvou_session_id,
            tools: Arc::new(Mutex::new(HashMap::new())),
            latest_plan: Arc::new(Mutex::new(None)),
        }
    }

    pub fn pinvou_session_id(&self) -> &str {
        &self.pinvou_session_id
    }

    pub fn handle(&self, notification: SessionNotification) {
        match notification.update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let ContentBlock::Text(text) = chunk.content {
                    self.emit("chat:delta", json!({ "text": text.text }));
                }
            }
            SessionUpdate::AgentThoughtChunk(_) => {
                // 当前 pinvou UI 只展示“思考中”状态，不落盘思维链。
            }
            SessionUpdate::ToolCall(call) => self.tool_start(call),
            SessionUpdate::ToolCallUpdate(update) => self.tool_update(update),
            SessionUpdate::Plan(plan) => {
                let items = plan
                    .entries
                    .into_iter()
                    .map(|entry| {
                        json!({
                            "step": entry.content,
                            "status": serde_json::to_value(entry.status).unwrap_or(Value::String("pending".into())),
                        })
                    })
                    .collect::<Vec<_>>();
                let snapshot = json!({ "items": items });
                *self.latest_plan.lock() = Some(snapshot.clone());
                self.emit("chat:plan_snapshot", json!({ "plan_snapshot": snapshot }));
            }
            SessionUpdate::CurrentModeUpdate(mode) => {
                self.emit(
                    "chat:acp_mode",
                    json!({ "mode_id": mode.current_mode_id.to_string() }),
                );
            }
            SessionUpdate::ConfigOptionUpdate(options) => {
                self.emit(
                    "chat:acp_config",
                    serde_json::to_value(options.config_options).unwrap_or(Value::Null),
                );
            }
            SessionUpdate::UsageUpdate(usage) => {
                self.emit(
                    "chat:usage",
                    json!({
                        "input_tokens": usage.used,
                        "max_tokens": usage.size,
                    }),
                );
            }
            _ => {}
        }
    }

    fn tool_start(&self, call: ToolCall) {
        let id = call.tool_call_id.to_string();
        let input = call.raw_input.clone().unwrap_or_else(|| json!({}));
        crate::memory::record_turn_tool_start(&self.pinvou_session_id, &call.title, &input);
        self.emit(
            "chat:tool_start",
            json!({
                "id": id,
                "name": call.title,
                "args": input,
            }),
        );
        let terminal = matches!(
            call.status,
            ToolCallStatus::Completed | ToolCallStatus::Failed
        );
        self.tools.lock().insert(id.clone(), call);
        if terminal {
            self.finish_tool(&id);
        }
    }

    fn tool_update(&self, update: ToolCallUpdate) {
        let id = update.tool_call_id.to_string();
        let mut tools = self.tools.lock();
        if let Some(call) = tools.get_mut(&id) {
            call.update(update.fields);
            let terminal = matches!(
                call.status,
                ToolCallStatus::Completed | ToolCallStatus::Failed
            );
            drop(tools);
            if terminal {
                self.finish_tool(&id);
            }
            return;
        }
        if let Ok(call) = ToolCall::try_from(update) {
            drop(tools);
            self.tool_start(call);
        }
    }

    fn finish_tool(&self, id: &str) {
        let Some(call) = self.tools.lock().remove(id) else {
            return;
        };
        let success = matches!(call.status, ToolCallStatus::Completed);
        crate::memory::record_turn_tool_complete(&self.pinvou_session_id, &call.title, success);
        self.emit(
            "chat:tool_end",
            json!({
                "id": id,
                "success": success,
                "output": call.raw_output.unwrap_or_else(|| {
                    serde_json::to_value(call.content).unwrap_or(Value::Null)
                }),
            }),
        );
    }

    pub fn emit(&self, event: &str, value: Value) {
        let mut payload = match value {
            Value::Object(map) => map,
            other => {
                let mut map = serde_json::Map::new();
                map.insert("value".into(), other);
                map
            }
        };
        payload.insert(
            "session_id".into(),
            Value::String(self.pinvou_session_id.clone()),
        );
        let payload = Value::Object(payload);
        let _ = self.app.emit(event, payload.clone());
        crate::remote_control::forward_app_event(&self.app, event, payload);
    }

    pub fn emit_plan_ready(&self) {
        if let Some(plan) = self.latest_plan.lock().clone() {
            self.emit("chat:plan_ready", json!({ "plan_snapshot": plan }));
        }
    }
}
