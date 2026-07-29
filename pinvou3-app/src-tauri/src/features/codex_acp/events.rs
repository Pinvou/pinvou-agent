use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionNotification, SessionUpdate, ToolCall, ToolCallStatus,
    ToolCallUpdate,
};
use anyhow::{bail, Context, Result};
use chrono::Utc;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use super::attachments::CodexDisplayAttachment;

const EVENT_VERSION: u32 = 1;
const TIMELINE_FILE: &str = "acp-timeline.jsonl";
const STATE_FILE: &str = "acp-state.json";

/// Codex ACP 页面唯一消费的事件合同。
///
/// 该合同刻意不复用 `chat:*`：ACP 的 reasoning、tool update、permission、
/// plan、mode 和 config 都保留原始协议字段，工作会话 UI 无需理解这些语义。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpEventEnvelope {
    pub version: u32,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub seq: u64,
    pub timestamp: String,
    pub event: AcpEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub data: Value,
}

#[derive(Clone)]
pub struct EventBridge {
    app: AppHandle,
    pinvou_session_id: String,
    seq: Arc<AtomicU64>,
    turn_serial: Arc<AtomicU64>,
    current_turn: Arc<RwLock<Option<String>>>,
    tools: Arc<Mutex<HashMap<String, ToolCall>>>,
}

impl EventBridge {
    pub fn new(app: AppHandle, pinvou_session_id: String) -> Self {
        let seq = load_timeline(&pinvou_session_id)
            .ok()
            .and_then(|events| events.last().map(|event| event.seq))
            .unwrap_or(0);
        Self {
            app,
            pinvou_session_id,
            seq: Arc::new(AtomicU64::new(seq)),
            turn_serial: Arc::new(AtomicU64::new(0)),
            current_turn: Arc::new(RwLock::new(None)),
            tools: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn pinvou_session_id(&self) -> &str {
        &self.pinvou_session_id
    }

    pub fn begin_turn(&self, content: &str, attachments: &[CodexDisplayAttachment]) -> String {
        let serial = self.turn_serial.fetch_add(1, Ordering::AcqRel) + 1;
        let turn_id = format!("turn-{}-{serial}", Utc::now().timestamp_millis());
        *self.current_turn.write() = Some(turn_id.clone());
        self.emit_with_turn(
            Some(turn_id.clone()),
            "user_message",
            json!({
                "content": [{ "type": "text", "text": content }],
                "attachments": attachments,
            }),
        );
        self.emit_with_turn(
            Some(turn_id.clone()),
            "turn_started",
            json!({ "status": "running" }),
        );
        turn_id
    }

    pub fn finish_turn(&self, turn_id: &str, status: &str, error: Option<&str>) {
        self.emit_with_turn(
            Some(turn_id.to_string()),
            "turn_completed",
            json!({ "status": status, "error": error }),
        );
        let mut current = self.current_turn.write();
        if current.as_deref() == Some(turn_id) {
            *current = None;
        }
    }

    pub fn handle(&self, notification: SessionNotification) {
        let meta = serde_json::to_value(notification.meta).unwrap_or(Value::Null);
        match notification.update {
            SessionUpdate::UserMessageChunk(chunk) => {
                // 设计 §13:timeline 不存图片/音频 Base64。agent 回显的这类块只会是
                // 我们发送内容的副本(附件 metadata 已由 user_message 事件记录),
                // 直接丢弃——不落盘也不推前端(前端对非文本块本就不渲染)。
                if !chunk_contains_binary(&chunk) {
                    self.emit_protocol("user_message_chunk", chunk, meta)
                }
            }
            SessionUpdate::AgentMessageChunk(chunk) => {
                self.emit_protocol("agent_message_chunk", chunk, meta)
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                self.emit_protocol("agent_thought_chunk", chunk, meta)
            }
            SessionUpdate::ToolCall(call) => self.tool_call(call, meta),
            SessionUpdate::ToolCallUpdate(update) => self.tool_update(update, meta),
            SessionUpdate::Plan(plan) => self.emit_protocol("plan", plan, meta),
            SessionUpdate::AvailableCommandsUpdate(commands) => {
                self.emit_protocol("available_commands", commands, meta)
            }
            SessionUpdate::CurrentModeUpdate(mode) => {
                self.emit_protocol("current_mode", mode, meta)
            }
            SessionUpdate::ConfigOptionUpdate(options) => {
                self.emit_protocol("config_options", options, meta)
            }
            SessionUpdate::SessionInfoUpdate(info) => {
                self.emit_protocol("session_info", info, meta)
            }
            SessionUpdate::UsageUpdate(usage) => self.emit_protocol("usage", usage, meta),
            _ => {}
        }
    }

    fn emit_protocol<T: Serialize>(&self, event_type: &str, value: T, notification_meta: Value) {
        let data = json!({
            "update": serde_json::to_value(value).unwrap_or(Value::Null),
            "notificationMeta": notification_meta,
        });
        self.emit(event_type, data);
    }

    fn tool_call(&self, call: ToolCall, notification_meta: Value) {
        let id = call.tool_call_id.to_string();
        let input = call.raw_input.clone().unwrap_or_else(|| json!({}));
        crate::features::memory::record_turn_tool_start(
            &self.pinvou_session_id,
            &call.title,
            &input,
        );
        let terminal = is_terminal(call.status);
        self.tools.lock().insert(id, call.clone());
        self.emit_protocol("tool_call", call.clone(), notification_meta);
        if terminal {
            self.record_tool_complete(&call);
        }
    }

    fn tool_update(&self, update: ToolCallUpdate, notification_meta: Value) {
        let id = update.tool_call_id.to_string();
        let mut completed = None;
        {
            let mut tools = self.tools.lock();
            if let Some(call) = tools.get_mut(&id) {
                call.update(update.fields.clone());
                if is_terminal(call.status) {
                    completed = Some(call.clone());
                }
            } else if let Ok(call) = ToolCall::try_from(update.clone()) {
                crate::features::memory::record_turn_tool_start(
                    &self.pinvou_session_id,
                    &call.title,
                    &call.raw_input.clone().unwrap_or_else(|| json!({})),
                );
                if is_terminal(call.status) {
                    completed = Some(call.clone());
                }
                tools.insert(id.clone(), call);
            }
        }
        self.emit_protocol("tool_call_update", update, notification_meta);
        if let Some(call) = completed {
            self.record_tool_complete(&call);
            self.tools.lock().remove(&id);
        }
    }

    fn record_tool_complete(&self, call: &ToolCall) {
        crate::features::memory::record_turn_tool_complete(
            &self.pinvou_session_id,
            &call.title,
            matches!(call.status, ToolCallStatus::Completed),
        );
    }

    pub fn emit(&self, event_type: &str, data: Value) -> AcpEventEnvelope {
        self.emit_with_turn(self.current_turn.read().clone(), event_type, data)
    }

    fn emit_with_turn(
        &self,
        turn_id: Option<String>,
        event_type: &str,
        data: Value,
    ) -> AcpEventEnvelope {
        let envelope = AcpEventEnvelope {
            version: EVENT_VERSION,
            session_id: self.pinvou_session_id.clone(),
            turn_id,
            seq: self.seq.fetch_add(1, Ordering::AcqRel) + 1,
            timestamp: Utc::now().to_rfc3339(),
            event: AcpEvent {
                event_type: event_type.to_string(),
                data,
            },
        };
        if let Err(error) = append_timeline(&envelope) {
            eprintln!(
                "[pinvou3-app] append Codex ACP timeline failed for {}: {error:#}",
                self.pinvou_session_id
            );
        }
        let last_status = match event_type {
            "turn_started" => Some("running"),
            "turn_completed" => envelope.event.data["status"].as_str(),
            "permission_requested" => Some("waiting_permission"),
            "permission_resolved" => Some("running"),
            "elicitation_requested" => Some("waiting_input"),
            "elicitation_resolved" => Some("running"),
            "cancel_requested" => Some("cancelling"),
            "runtime_error" => Some("error"),
            _ => None,
        };
        if let Some(status) = last_status {
            if let Err(error) = patch_acp_state(
                &self.pinvou_session_id,
                json!({ "lastStatus": status, "lastSeq": envelope.seq }),
            ) {
                eprintln!(
                    "[pinvou3-app] update Codex ACP state failed for {}: {error:#}",
                    self.pinvou_session_id
                );
            }
        }
        let _ = self.app.emit("acp:event", &envelope);
        envelope
    }
}

fn is_terminal(status: ToolCallStatus) -> bool {
    matches!(status, ToolCallStatus::Completed | ToolCallStatus::Failed)
}

/// ContentChunk 是否携带 Base64 二进制负载(Image/Audio 的 `data` 字段)。
/// 这类块进入 timeline 会违反「不持久化图片 Base64」约束(设计 §13)。
fn chunk_contains_binary(chunk: &ContentChunk) -> bool {
    matches!(
        chunk.content,
        ContentBlock::Image(_) | ContentBlock::Audio(_)
    )
}

fn timeline_path(session_id: &str) -> Result<PathBuf> {
    session_file_path(session_id, TIMELINE_FILE)
}

fn state_path(session_id: &str) -> Result<PathBuf> {
    session_file_path(session_id, STATE_FILE)
}

fn session_file_path(session_id: &str, filename: &str) -> Result<PathBuf> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        bail!("非法 Codex ACP session id");
    }
    Ok(crate::platform::paths::sessions_root()
        .join(session_id)
        .join(filename))
}

pub fn persist_acp_state(session_id: &str, mut state: Value) -> Result<()> {
    let path = state_path(session_id)?;
    if let Some(object) = state.as_object_mut() {
        object.insert("version".into(), json!(1));
        object.insert("pinvouSessionId".into(), json!(session_id));
        object.insert("updatedAt".into(), json!(Utc::now().to_rfc3339()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(&state)?)?;
    fs::rename(&temporary, &path)?;
    Ok(())
}

pub fn patch_acp_state(session_id: &str, patch: Value) -> Result<()> {
    let path = state_path(session_id)?;
    let mut state = if path.exists() {
        serde_json::from_slice::<Value>(&fs::read(&path)?).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    let state_object = state
        .as_object_mut()
        .context("Codex ACP state 根节点不是对象")?;
    if let Some(patch_object) = patch.as_object() {
        for (key, value) in patch_object {
            state_object.insert(key.clone(), value.clone());
        }
    }
    persist_acp_state(session_id, state)
}

fn append_timeline(event: &AcpEventEnvelope) -> Result<()> {
    let path = timeline_path(&event.session_id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建 ACP timeline 目录 {} 失败", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("打开 ACP timeline {} 失败", path.display()))?;
    serde_json::to_writer(&mut file, event)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

pub fn load_timeline(session_id: &str) -> Result<Vec<AcpEventEnvelope>> {
    let path = timeline_path(session_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&path)
        .with_context(|| format!("读取 ACP timeline {} 失败", path.display()))?;
    let mut events = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line.with_context(|| format!("读取 ACP timeline 第 {} 行失败", index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&line) {
            Ok(event) => events.push(event),
            Err(error) => eprintln!(
                "[pinvou3-app] skip malformed ACP timeline line {} for {}: {error}",
                index + 1,
                session_id
            ),
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{AudioContent, ImageContent, TextContent};

    #[test]
    fn timeline_rejects_path_traversal() {
        assert!(timeline_path("../escape").is_err());
        assert!(timeline_path("valid-session_1").is_ok());
        assert!(state_path("../escape").is_err());
    }

    #[test]
    fn envelope_keeps_version_and_event_type() {
        let value = serde_json::to_value(AcpEventEnvelope {
            version: EVENT_VERSION,
            session_id: "session-1".into(),
            turn_id: Some("turn-1".into()),
            seq: 7,
            timestamp: "2026-01-01T00:00:00Z".into(),
            event: AcpEvent {
                event_type: "tool_call".into(),
                data: json!({ "rawInput": { "path": "README.md" } }),
            },
        })
        .unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["sessionId"], "session-1");
        assert_eq!(value["event"]["type"], "tool_call");
    }

    #[test]
    fn user_chunk_filter_drops_binary_blocks_before_timeline() {
        // 图片/音频块含 Base64 data,必须被拦截;文本与资源链接照常放行。
        let image = ContentChunk::new(ContentBlock::Image(ImageContent::new(
            "aGVsbG8td29ybGQ=".repeat(64),
            "image/png",
        )));
        assert!(chunk_contains_binary(&image));
        let audio = ContentChunk::new(ContentBlock::Audio(AudioContent::new(
            "aGVsbG8td29ybGQ=",
            "audio/wav",
        )));
        assert!(chunk_contains_binary(&audio));
        let text = ContentChunk::new(ContentBlock::Text(TextContent::new("看图")));
        assert!(!chunk_contains_binary(&text));
        // 反向佐证:图片块序列化后确实带 data,不过滤就会写进 acp-timeline.jsonl。
        let leaked = serde_json::to_string(&image).unwrap();
        assert!(leaked.contains("aGVsbG8td29ybGQ="));
    }
}
