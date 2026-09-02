use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use agent_client_protocol::schema::v1::{
    SessionNotification, SessionUpdate, ToolCall, ToolCallStatus, ToolCallUpdate,
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use parking_lot::{Condvar, Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter};

use super::attachments::CodexDisplayAttachment;

const EVENT_VERSION: u32 = 1;
const TIMELINE_FILE: &str = "acp-timeline.jsonl";
const STATE_FILE: &str = "acp-state.json";
/// Leaves headroom for the remote event/RPC envelope below the 2 MiB Relay cap.
const MAX_WEB_ACP_EVENT_BYTES: usize = 1_750_000;
const HOST_PATH_REDACTION: &str = "[host path omitted]";
/// Upper bound for waiting on a missing predecessor sequence before ordered
/// Web delivery skips the gap and keeps the session pipeline responsive.
const WEB_DELIVERY_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Build the ACP timeline payload that may cross the Web remote-control
/// boundary. The desktop timeline and native event remain lossless; WebUI
/// receives the same user-visible event shape without adapter metadata,
/// credentials, environment snapshots, or hidden diagnostic fields.
fn project_acp_event_for_web(event: &AcpEventEnvelope) -> AcpEventEnvelope {
    let mut projected = event.clone();
    projected.event.data =
        project_acp_event_data_for_web(&projected.event.event_type, projected.event.data);
    projected
}

/// Project an event into a transport-safe, size-bounded representation. The
/// event identity and ordering fields are always retained so one large tool
/// result cannot make the rest of a Web timeline unreachable.
fn project_acp_event_for_web_bounded(
    event: &AcpEventEnvelope,
    max_bytes: usize,
) -> AcpEventEnvelope {
    let projected = project_acp_event_for_web(event);
    let original_bytes = serialized_len(&projected);
    if original_bytes <= max_bytes {
        return projected;
    }

    for (max_string_bytes, max_collection_items) in [
        (128 * 1024, 256),
        (32 * 1024, 128),
        (8 * 1024, 64),
        (2 * 1024, 32),
    ] {
        let mut candidate = projected.clone();
        candidate.event.data =
            truncate_web_value(candidate.event.data, max_string_bytes, max_collection_items);
        mark_web_projection_truncated(&mut candidate.event.data, original_bytes);
        if serialized_len(&candidate) <= max_bytes {
            return candidate;
        }
    }

    let mut fallback = projected;
    fallback.event.data = minimal_truncated_event_data(&fallback.event.data, original_bytes);
    if serialized_len(&fallback) > max_bytes {
        fallback.event.data = json!({
            "webProjection": {
                "truncated": true,
                "originalBytes": original_bytes,
            }
        });
    }
    fallback
}

pub(crate) fn project_acp_value_for_web(value: Value) -> Value {
    sanitize_web_value(value)
}

pub fn project_acp_permission_request_for_web(value: Value) -> Value {
    let Value::Object(mut values) = value else {
        return Value::Object(serde_json::Map::new());
    };
    let mut projected = serde_json::Map::new();
    if let Some(tool_call) = values.remove("toolCall") {
        projected.insert("toolCall".into(), project_tool_update_for_web(tool_call));
    }
    if let Some(Value::Array(options)) = values.remove("options") {
        projected.insert(
            "options".into(),
            Value::Array(
                options
                    .into_iter()
                    .map(|option| project_allowed_fields(option, &["optionId", "name", "kind"]))
                    .collect(),
            ),
        );
    }
    Value::Object(projected)
}

pub fn project_acp_elicitation_request_for_web(value: Value) -> Value {
    let Value::Object(mut values) = value else {
        return Value::Object(serde_json::Map::new());
    };
    let mut projected = serde_json::Map::new();
    for key in ["mode", "message"] {
        if let Some(value) = values.remove(key) {
            projected.insert(key.into(), sanitize_web_value(value));
        }
    }
    if let Some(schema) = values.remove("requestedSchema") {
        projected.insert("requestedSchema".into(), sanitize_web_schema(schema));
    }
    Value::Object(projected)
}

fn project_acp_event_data_for_web(event_type: &str, value: Value) -> Value {
    match event_type {
        "user_message" => project_allowed_fields(value, &["content", "attachments"]),
        "user_message_chunk" | "agent_message_chunk" | "agent_thought_chunk" => {
            project_protocol_update(value, &["content", "_meta"])
        }
        "tool_call" | "tool_call_update" => project_protocol_tool_update(value),
        "plan" => project_protocol_update(value, &["entries"]),
        "available_commands" => project_protocol_update(value, &["availableCommands"]),
        "current_mode" => project_protocol_update(value, &["currentModeId"]),
        "config_options" => project_protocol_update(value, &["configOptions"]),
        "session_info" => project_protocol_update(value, &["title", "updatedAt"]),
        "usage" => project_protocol_update(value, &["used", "size"]),
        "permission_requested" => project_permission_event(value),
        "elicitation_requested" => project_elicitation_event(value),
        "permission_resolved" => {
            project_allowed_fields(value, &["toolCallId", "optionId", "outcome"])
        }
        "elicitation_resolved" => {
            project_allowed_fields(value, &["elicitationId", "action", "reason"])
        }
        "turn_started" | "turn_completed" | "cancel_requested" | "runtime_error" => {
            project_allowed_fields(value, &["status", "error", "message", "recoveryReason"])
        }
        "config_change_requested"
        | "config_change_applied"
        | "config_change_failed"
        | "config_persistence_failed" => {
            project_allowed_fields(value, &["configId", "valueId", "message"])
        }
        // runtime_ready is a signal; the Web client fetches the authoritative
        // session info separately and does not need adapter capabilities here.
        "runtime_ready" => Value::Object(serde_json::Map::new()),
        // Unknown future events stay ordered but do not automatically acquire
        // permission to expose an adapter-defined payload across the Relay.
        _ => json!({ "webProjection": { "omitted": true } }),
    }
}

fn project_protocol_update(value: Value, allowed_update_fields: &[&str]) -> Value {
    let Value::Object(mut values) = value else {
        return Value::Object(serde_json::Map::new());
    };
    let Some(update) = values.remove("update") else {
        return Value::Object(serde_json::Map::new());
    };
    json!({ "update": project_allowed_fields(update, allowed_update_fields) })
}

fn project_protocol_tool_update(value: Value) -> Value {
    let Value::Object(mut values) = value else {
        return Value::Object(serde_json::Map::new());
    };
    let Some(update) = values.remove("update") else {
        return Value::Object(serde_json::Map::new());
    };
    json!({ "update": project_tool_update_for_web(update) })
}

fn project_tool_update_for_web(value: Value) -> Value {
    project_allowed_fields(
        value,
        &[
            "toolCallId",
            "title",
            "kind",
            "status",
            "content",
            "locations",
            "rawInput",
            "rawOutput",
            "inputTokens",
            "_meta",
        ],
    )
}

fn project_permission_event(value: Value) -> Value {
    project_request_event(value, "toolCallId", project_acp_permission_request_for_web)
}

fn project_elicitation_event(value: Value) -> Value {
    project_request_event(
        value,
        "elicitationId",
        project_acp_elicitation_request_for_web,
    )
}

fn project_request_event(
    value: Value,
    id_field: &str,
    project_request: fn(Value) -> Value,
) -> Value {
    let Value::Object(mut values) = value else {
        return Value::Object(serde_json::Map::new());
    };
    let mut projected = serde_json::Map::new();
    if let Some(value) = values.remove(id_field) {
        projected.insert(id_field.into(), sanitize_web_value(value));
    }
    if let Some(value) = values.remove("request") {
        projected.insert("request".into(), project_request(value));
    }
    Value::Object(projected)
}

fn project_allowed_fields(value: Value, allowed: &[&str]) -> Value {
    let Value::Object(mut values) = value else {
        return Value::Object(serde_json::Map::new());
    };
    let mut projected = serde_json::Map::new();
    for key in allowed {
        let Some(value) = values.remove(*key) else {
            continue;
        };
        let value = if *key == "_meta" {
            project_web_ui_metadata(&value).unwrap_or(Value::Null)
        } else {
            sanitize_web_value(value)
        };
        if !value.is_null() {
            projected.insert((*key).to_string(), value);
        }
    }
    Value::Object(projected)
}

fn sanitize_web_value(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(sanitize_web_value).collect()),
        Value::Object(values) => {
            let mut projected = serde_json::Map::new();
            for (key, value) in values {
                if web_redacted_key(&key) || absolute_host_path_fragment_start(&key).is_some() {
                    continue;
                }
                if key.eq_ignore_ascii_case("error") {
                    if let Value::String(message) = value {
                        projected.insert(key, Value::String(sanitize_web_string(&message)));
                    }
                    continue;
                }
                // Only the ACP elicitation schema may preserve arbitrary
                // property names (for example a field literally named
                // `password`). It is projected through a schema whitelist so
                // defaults/examples cannot smuggle credential values.
                if key == "requestedSchema" {
                    projected.insert(key, sanitize_web_schema(value));
                    continue;
                }
                if key == "_meta" {
                    if let Some(metadata) = project_web_ui_metadata(&value) {
                        projected.insert(key, metadata);
                    }
                    continue;
                }
                if web_path_key(&key) {
                    projected.insert(key, sanitize_web_path_value(value));
                    continue;
                }
                projected.insert(key, sanitize_web_value(value));
            }
            Value::Object(projected)
        }
        Value::String(message) => Value::String(sanitize_web_string(&message)),
        scalar => scalar,
    }
}

fn sanitize_web_string(message: &str) -> String {
    if let Some(path_start) = absolute_host_path_fragment_start(message) {
        let visible_prefix =
            crate::platform::credential_store::redact_secret(&message[..path_start]);
        return format!("{visible_prefix}{HOST_PATH_REDACTION}");
    }
    crate::platform::credential_store::redact_secret(message)
}

fn web_path_key(key: &str) -> bool {
    ["cwd", "path", "filePath", "workspacePath"]
        .iter()
        .any(|candidate| key.eq_ignore_ascii_case(candidate))
}

fn sanitize_web_path_value(value: Value) -> Value {
    match value {
        Value::String(path) => {
            let display = if absolute_host_path(&path) {
                path.trim_end_matches(['/', '\\'])
                    .rsplit(['/', '\\'])
                    .find(|component| !component.is_empty())
                    .unwrap_or("workspace")
            } else {
                &path
            };
            Value::String(sanitize_web_string(display))
        }
        other => sanitize_web_value(other),
    }
}

fn absolute_host_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    std::path::Path::new(path).is_absolute()
        // ACP timelines can be reopened on a different OS. Windows path
        // parsing does not classify a single-leading-slash POSIX path as
        // absolute, so retain only its basename instead of treating it as a
        // relative value and redacting the whole field later.
        || path.starts_with('/')
        || path.starts_with("\\\\")
        || path.starts_with("//")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

/// Detect absolute host paths embedded in otherwise free-form adapter text.
/// Explicit path fields retain a basename through `sanitize_web_path_value`;
/// free-form strings fail closed because path boundaries with quoting and
/// spaces cannot be reconstructed safely from arbitrary shell output.
fn absolute_host_path_fragment_start(message: &str) -> Option<usize> {
    let bytes = message.as_bytes();
    let starts_at_boundary = |index: usize| {
        index == 0
            || bytes[index - 1].is_ascii_whitespace()
            || matches!(
                bytes[index - 1],
                b'\''
                    | b'"'
                    | b'('
                    | b'['
                    | b'{'
                    | b'='
                    | b':'
                    | b','
                    | b';'
                    | b'<'
                    | b'>'
                    | b'|'
                    | b'&'
            )
    };
    for index in 0..bytes.len() {
        if index + 5 < bytes.len()
            && bytes[index..index + 5].eq_ignore_ascii_case(b"file:")
            && matches!(bytes[index + 5], b'/' | b'\\')
        {
            return Some(index);
        }
        if index + 2 < bytes.len()
            && starts_at_boundary(index)
            && bytes[index].is_ascii_alphabetic()
            && bytes[index + 1] == b':'
            && matches!(bytes[index + 2], b'/' | b'\\')
        {
            return Some(index);
        }
        if index + 1 < bytes.len()
            && starts_at_boundary(index)
            && bytes[index] == b'~'
            && matches!(bytes[index + 1], b'/' | b'\\')
        {
            return Some(index);
        }
        if index + 1 < bytes.len()
            && starts_at_boundary(index)
            && bytes[index] == b'\\'
            && bytes[index + 1] == b'\\'
        {
            return Some(index);
        }
        if bytes[index] != b'/' {
            continue;
        }
        if index + 1 < bytes.len() && bytes[index + 1] == b'/' {
            // `scheme://...` is a URL, while a leading or delimited `//...`
            // is a UNC-style host path.
            if index == 0 || bytes[index - 1] != b':' {
                return Some(index);
            }
            continue;
        }
        if starts_at_boundary(index) {
            return Some(index);
        }
    }
    None
}

fn sanitize_web_schema(value: Value) -> Value {
    let Value::Object(values) = value else {
        return Value::Object(serde_json::Map::new());
    };
    let mut projected = serde_json::Map::new();
    for (key, value) in values {
        match key.as_str() {
            "properties" => {
                let Value::Object(properties) = value else {
                    continue;
                };
                let properties = properties
                    .into_iter()
                    .filter_map(|(name, definition)| {
                        (absolute_host_path_fragment_start(&name).is_none()
                            && definition.is_object())
                        .then(|| (name, sanitize_web_schema(definition)))
                    })
                    .collect();
                projected.insert(key, Value::Object(properties));
            }
            "oneOf" | "anyOf" | "allOf" => {
                let Value::Array(options) = value else {
                    continue;
                };
                projected.insert(
                    key,
                    Value::Array(options.into_iter().map(sanitize_web_schema).collect()),
                );
            }
            "items" => {
                projected.insert(key, sanitize_web_schema(value));
            }
            "_meta" => {
                if let Some(metadata) = project_web_ui_metadata(&value) {
                    projected.insert(key, metadata);
                }
            }
            // These are the JSON Schema fields consumed by the shared ACP
            // elicitation UI. `default`, `examples`, extension metadata, and
            // all unknown fields intentionally stay on the desktop.
            "type" | "title" | "description" | "format" | "required" | "enum" | "const"
            | "minLength" | "maxLength" | "minimum" | "maximum" | "minItems" | "maxItems"
            | "pattern" => {
                projected.insert(key, sanitize_web_value(value));
            }
            _ => {}
        }
    }
    Value::Object(projected)
}

fn project_web_ui_metadata(value: &Value) -> Option<Value> {
    let codex = value.get("codex")?.as_object()?;
    let mut visible = serde_json::Map::new();
    for key in [
        "phase",
        "isOtherAnswer",
        "questionId",
        "isOther",
        "isSecret",
    ] {
        let Some(value) = codex.get(key) else {
            continue;
        };
        let valid = match key {
            "phase" | "questionId" => value.is_string(),
            _ => value.is_boolean(),
        };
        if valid {
            visible.insert(key.to_string(), value.clone());
        }
    }
    if visible.is_empty() {
        None
    } else {
        Some(json!({ "codex": visible }))
    }
}

fn web_redacted_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect::<String>();
    if matches!(
        normalized.as_str(),
        "notificationmeta"
            | "diagnostic"
            | "diagnostics"
            | "debug"
            | "stack"
            | "backtrace"
            | "env"
            | "environment"
            | "environmentvariables"
            | "hiddenreasoning"
            | "reasoningsummaryraw"
            | "accesstoken"
            | "refreshtoken"
            | "authtoken"
            | "bearertoken"
            | "apikey"
            | "authorization"
            | "headers"
            | "httpheaders"
            | "requestheaders"
            | "responseheaders"
            | "cookie"
            | "cookies"
            | "credential"
            | "credentials"
            | "password"
            | "passphrase"
            | "privatekey"
            | "clientsecret"
            | "secret"
            | "token"
    ) {
        return true;
    }

    let safe_token_count = matches!(
        normalized.as_str(),
        "inputtokens"
            | "outputtokens"
            | "totaltokens"
            | "prompttokens"
            | "completiontokens"
            | "cachedtokens"
            | "tokencount"
    );
    [
        "apikey",
        "accesstoken",
        "refreshtoken",
        "authtoken",
        "bearertoken",
        "idtoken",
        "sessiontoken",
        "oauthtoken",
        "authorization",
        "clientsecret",
        "privatekey",
        "secretaccesskey",
        "accesskeyid",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
        || (normalized.contains("token") && !safe_token_count)
        || normalized.starts_with("password")
        || normalized.ends_with("password")
        || normalized.starts_with("passphrase")
        || normalized.ends_with("passphrase")
        || normalized.starts_with("secret")
        || normalized.ends_with("secret")
        || normalized.ends_with("credential")
        || normalized.ends_with("credentials")
        || normalized.ends_with("headers")
        || normalized.ends_with("cookies")
        || normalized.ends_with("environment")
        || normalized.ends_with("environmentvariables")
}

fn serialized_len(event: &AcpEventEnvelope) -> usize {
    serde_json::to_vec(event).map_or(usize::MAX, |value| value.len())
}

fn truncate_web_value(value: Value, max_string_bytes: usize, max_items: usize) -> Value {
    match value {
        Value::String(value) => Value::String(truncate_utf8(&value, max_string_bytes)),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .take(max_items)
                .map(|value| truncate_web_value(value, max_string_bytes, max_items))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .take(max_items)
                .map(|(key, value)| (key, truncate_web_value(value, max_string_bytes, max_items)))
                .collect(),
        ),
        scalar => scalar,
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let suffix = "\n… [Web output truncated]";
    let mut end = max_bytes.saturating_sub(suffix.len()).min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &value[..end], suffix)
}

fn mark_web_projection_truncated(data: &mut Value, original_bytes: usize) {
    let marker = json!({
        "truncated": true,
        "originalBytes": original_bytes,
    });
    match data {
        Value::Object(values) => {
            values.insert("webProjection".into(), marker);
        }
        _ => {
            *data = json!({ "value": data.take(), "webProjection": marker });
        }
    }
}

fn minimal_truncated_event_data(data: &Value, original_bytes: usize) -> Value {
    let source = data.get("update").unwrap_or(data);
    let mut essential = serde_json::Map::new();
    if let Some(values) = source.as_object() {
        for key in ["toolCallId", "elicitationId", "title", "kind", "status"] {
            if let Some(value) = values.get(key) {
                essential.insert(key.into(), truncate_web_value(value.clone(), 1024, 16));
            }
        }
    }
    let marker = json!({
        "truncated": true,
        "originalBytes": original_bytes,
    });
    if data.get("update").is_some() {
        json!({ "update": essential, "webProjection": marker })
    } else {
        essential.insert("webProjection".into(), marker);
        Value::Object(essential)
    }
}

#[derive(Clone)]
pub struct EventBridge {
    app: AppHandle,
    pinvou_session_id: String,
    seq: Arc<AtomicU64>,
    turn_serial: Arc<AtomicU64>,
    current_turn: Arc<RwLock<Option<String>>>,
    event_order: Arc<Mutex<()>>,
    web_delivery: OrderedWebDelivery,
    tools: Arc<Mutex<HashMap<String, ToolCall>>>,
}

#[derive(Clone)]
struct OrderedWebDelivery {
    last_seq: Arc<Mutex<u64>>,
    ready: Arc<Condvar>,
    wait_timeout: Arc<Duration>,
}

impl OrderedWebDelivery {
    fn new(last_seq: u64) -> Self {
        Self::with_wait_timeout(last_seq, WEB_DELIVERY_WAIT_TIMEOUT)
    }

    fn with_wait_timeout(last_seq: u64, wait_timeout: Duration) -> Self {
        Self {
            last_seq: Arc::new(Mutex::new(last_seq)),
            ready: Arc::new(Condvar::new()),
            wait_timeout: Arc::new(wait_timeout),
        }
    }

    fn deliver(&self, seq: u64, delivery: impl FnOnce()) {
        let mut last_seq = self.last_seq.lock();
        let deadline = Instant::now() + *self.wait_timeout;
        while seq > last_seq.saturating_add(1) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                // A producer that dies between sequence allocation and
                // delivery must not block the session pipeline forever.
                // The skipped envelope leaves a seq hole in the live stream;
                // the Web client's envelope-seq gap detection heals it with a
                // debounced authoritative timeline resync, and the next
                // reconnect or session reopen refetches it as well.
                eprintln!(
                    "[acp] Web event delivery for sequence {seq} timed out waiting for sequence {}; skipping the gap",
                    last_seq.saturating_add(1)
                );
                break;
            }
            self.ready.wait_for(&mut last_seq, remaining);
        }
        if seq <= *last_seq {
            return;
        }
        delivery();
        *last_seq = seq;
        self.ready.notify_all();
    }
}

impl EventBridge {
    pub fn new(app: AppHandle, pinvou_session_id: String) -> Self {
        let last_seq = load_timeline(&pinvou_session_id)
            .ok()
            .and_then(|events| events.last().map(|event| event.seq))
            .unwrap_or(0);
        Self {
            app,
            pinvou_session_id,
            seq: Arc::new(AtomicU64::new(last_seq)),
            turn_serial: Arc::new(AtomicU64::new(0)),
            current_turn: Arc::new(RwLock::new(None)),
            event_order: Arc::new(Mutex::new(())),
            web_delivery: OrderedWebDelivery::new(last_seq),
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

    /// 把 timeline 中只开始、未结束的旧回合收口为已中断。
    ///
    /// ACP prompt future 和当前 turn 只存在于宿主进程内；应用被直接关闭后，Agent
    /// 会话虽然可以恢复，但旧 prompt 已无法重新挂接。继续把这种回合展示为 running
    /// 会让前端永久停在“处理中”，而恢复后的 session/cancel 也没有旧 turn 可取消。
    ///
    /// 本方法只处理当前 timeline 已存在的孤儿回合。正常的同进程活跃回合仍由
    /// `prompt()` 返回后调用 `finish_turn()` 收口。
    pub fn interrupt_orphaned_turns(&self, reason: &str) -> usize {
        let events = match load_timeline(&self.pinvou_session_id) {
            Ok(events) => events,
            Err(error) => {
                eprintln!(
                    "[pinvou3-app] inspect orphaned ACP turns failed for {}: {error:#}",
                    self.pinvou_session_id
                );
                return 0;
            }
        };
        let orphaned = orphaned_turn_ids(&events);
        for turn_id in &orphaned {
            self.emit_with_turn(
                Some(turn_id.clone()),
                "turn_completed",
                json!({
                    "status": "Interrupted",
                    "error": null,
                    "recoveryReason": reason,
                }),
            );
        }
        orphaned.len()
    }

    pub fn handle(&self, notification: SessionNotification) {
        let meta = serde_json::to_value(notification.meta).unwrap_or(Value::Null);
        match notification.update {
            SessionUpdate::UserMessageChunk(chunk) => {
                self.emit_protocol("user_message_chunk", chunk, meta)
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
        self.tools.lock().insert(id.clone(), call.clone());
        self.emit_protocol("tool_call", call.clone(), notification_meta);
        if terminal {
            self.record_tool_complete(&call);
            // When the first notification is already terminal (no later
            // update), the tool_update removal path never runs; leaving the
            // entry here would pin its raw_input/raw_output until bridge
            // teardown.
            self.tools.lock().remove(&id);
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
            &call.raw_input.clone().unwrap_or_else(|| json!({})),
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
        // While the remote endpoint is active the projection must be built
        // even if the browser is momentarily disconnected: the journal
        // replays disconnect-window events on reconnect. Without an endpoint
        // at all, both projection cost and journal are skipped.
        let web_transport_active =
            crate::platform::app_events::has_active_app_event_transport(&self.app);
        let envelope = {
            // Sequence allocation, persistence, and native publication remain
            // one ordered unit. Potentially large Web projection and Relay
            // serialization happen after this lock is released.
            let _order = self.event_order.lock();
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
            // The native desktop event remains lossless and in the exact same
            // order as its durable timeline entry.
            let _ = self.app.emit("acp:event", &envelope);
            envelope
        };

        let web_payload = web_transport_active.then(|| {
            serde_json::to_value(project_acp_event_for_web_bounded(
                &envelope,
                MAX_WEB_ACP_EVENT_BYTES,
            ))
        });
        self.web_delivery
            .deliver(envelope.seq, || match web_payload {
                Some(Ok(payload)) => {
                    crate::platform::app_events::forward_app_event(&self.app, "acp:event", payload)
                }
                Some(Err(error)) => {
                    eprintln!("[acp] serialize Web event projection failed: {error}")
                }
                None => {}
            });
        envelope
    }
}

fn is_terminal(status: ToolCallStatus) -> bool {
    matches!(status, ToolCallStatus::Completed | ToolCallStatus::Failed)
}

fn orphaned_turn_ids(events: &[AcpEventEnvelope]) -> Vec<String> {
    let mut open = HashMap::<String, u64>::new();
    let mut ordered = events.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|event| event.seq);
    for envelope in ordered {
        let Some(turn_id) = envelope.turn_id.as_ref() else {
            continue;
        };
        match envelope.event.event_type.as_str() {
            "turn_started" => {
                open.entry(turn_id.clone()).or_insert(envelope.seq);
            }
            "turn_completed" => {
                open.remove(turn_id);
            }
            _ => {}
        }
    }
    let mut orphaned = open.into_iter().collect::<Vec<_>>();
    orphaned.sort_by_key(|(_, started_seq)| *started_seq);
    orphaned.into_iter().map(|(turn_id, _)| turn_id).collect()
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

#[derive(Debug)]
pub(crate) struct WebAcpTimelineSlice {
    pub(crate) events: Vec<AcpEventEnvelope>,
    pub(crate) next_cursor: Option<u64>,
    pub(crate) has_more: bool,
}

/// Read one Web page directly from the append-only JSONL timeline. `cursor`
/// is the byte position returned by the previous page; older clients may omit
/// it and continue using `after_seq`, at the cost of scanning from the start.
pub(crate) fn load_web_timeline_page(
    session_id: &str,
    after_seq: u64,
    cursor: Option<u64>,
    limit: usize,
    max_page_bytes: usize,
    max_event_bytes: usize,
) -> Result<WebAcpTimelineSlice> {
    let path = timeline_path(session_id)?;
    load_web_timeline_page_from_path(
        &path,
        session_id,
        after_seq,
        cursor,
        limit,
        max_page_bytes,
        max_event_bytes,
    )
}

#[allow(clippy::too_many_arguments)]
fn load_web_timeline_page_from_path(
    path: &Path,
    timeline_label: &str,
    after_seq: u64,
    cursor: Option<u64>,
    limit: usize,
    max_page_bytes: usize,
    max_event_bytes: usize,
) -> Result<WebAcpTimelineSlice> {
    if !path.exists() {
        return Ok(WebAcpTimelineSlice {
            events: Vec::new(),
            next_cursor: cursor,
            has_more: false,
        });
    }

    let mut file = fs::File::open(path)
        .with_context(|| format!("读取 ACP timeline {} 失败", path.display()))?;
    let file_len = file.metadata()?.len();
    let start = cursor.unwrap_or(0);
    if start > file_len {
        bail!("ACP timeline cursor 已失效");
    }
    if start > 0 {
        file.seek(SeekFrom::Start(start - 1))?;
        let mut boundary = [0_u8; 1];
        file.read_exact(&mut boundary)?;
        if boundary[0] != b'\n' {
            bail!("ACP timeline cursor 未对齐事件边界");
        }
    }
    file.seek(SeekFrom::Start(start))?;

    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut page_bytes = 0usize;
    let mut resume_cursor = start;
    let mut has_more = false;
    let mut line_number = 0usize;
    loop {
        let line_start = reader.stream_position()?;
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            break;
        }
        line_number += 1;
        let line_end = reader.stream_position()?;
        // append_timeline writes JSON and the newline separately. A concurrent
        // reader can briefly observe the final JSON fragment; never advance a
        // durable cursor past that incomplete event.
        if !line.ends_with('\n') {
            break;
        }
        if line.trim().is_empty() {
            resume_cursor = line_end;
            continue;
        }
        let event = match serde_json::from_str::<AcpEventEnvelope>(&line) {
            Ok(event) => event,
            Err(error) => {
                eprintln!(
                    "[pinvou3-app] skip malformed ACP timeline page line {} for {}: {error}",
                    line_number, timeline_label
                );
                resume_cursor = line_end;
                continue;
            }
        };
        if event.seq <= after_seq {
            resume_cursor = line_end;
            continue;
        }

        let event = project_acp_event_for_web_bounded(&event, max_event_bytes);
        let event_bytes = serde_json::to_vec(&event)?.len();
        if events.len() >= limit
            || (!events.is_empty() && page_bytes.saturating_add(event_bytes) > max_page_bytes)
        {
            // Do not consume this line: the returned cursor points immediately
            // after the last included/skipped line, so the next page rereads it.
            debug_assert!(line_start >= resume_cursor);
            has_more = true;
            break;
        }
        page_bytes = page_bytes.saturating_add(event_bytes);
        events.push(event);
        resume_cursor = line_end;
    }

    Ok(WebAcpTimelineSlice {
        next_cursor: Some(resume_cursor),
        events,
        has_more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    static NEXT_TIMELINE_ID: AtomicU64 = AtomicU64::new(1);

    struct TempTimeline(PathBuf);

    impl TempTimeline {
        fn create(events: &[AcpEventEnvelope]) -> Self {
            let id = NEXT_TIMELINE_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pinvou3-acp-timeline-{}-{id}.jsonl",
                std::process::id()
            ));
            let mut file = fs::File::create(&path).expect("create temporary ACP timeline");
            for event in events {
                serde_json::to_writer(&mut file, event).expect("serialize ACP timeline event");
                writeln!(file).expect("terminate ACP timeline line");
            }
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn append(&self, bytes: &[u8]) {
            OpenOptions::new()
                .append(true)
                .open(&self.0)
                .expect("open temporary ACP timeline")
                .write_all(bytes)
                .expect("append temporary ACP timeline");
        }
    }

    impl Drop for TempTimeline {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn event(seq: u64, turn_id: Option<&str>, event_type: &str) -> AcpEventEnvelope {
        AcpEventEnvelope {
            version: EVENT_VERSION,
            session_id: "session-1".into(),
            turn_id: turn_id.map(str::to_string),
            seq,
            timestamp: format!("2026-01-01T00:00:{seq:02}Z"),
            event: AcpEvent {
                event_type: event_type.into(),
                data: json!({}),
            },
        }
    }

    fn message_event(seq: u64, text: &str) -> AcpEventEnvelope {
        let mut event = event(seq, Some("turn-1"), "agent_message_chunk");
        event.event.data = json!({
            "update": {
                "content": {
                    "type": "text",
                    "text": text,
                }
            }
        });
        event
    }

    #[test]
    fn web_projection_keeps_visible_tool_data_and_removes_private_metadata() {
        let event = AcpEventEnvelope {
            version: 1,
            session_id: "session-1".into(),
            turn_id: Some("turn-1".into()),
            seq: 7,
            timestamp: "2026-08-03T00:00:00Z".into(),
            event: AcpEvent {
                event_type: "tool_call".into(),
                data: json!({
                    "notificationMeta": { "adapter": "private" },
                    "update": {
                        "toolCallId": "tool-1",
                        "title": "Read file",
                        "rawInput": {
                            "path": "README.md",
                            "cwd": "/home/alice/secret-project",
                            "filePath": "C:\\Users\\alice\\secret-project\\src\\main.rs",
                            "environment": { "HOME": "/private/home" },
                            "apiKey": "must-not-cross-web",
                            "access_token": "must-not-cross-web",
                            "OPENAI_API_KEY": "must-not-cross-web",
                            "x-api-key": "must-not-cross-web",
                            "id_token": "must-not-cross-web",
                            "custom_token_value": "must-not-cross-web",
                            "tokenCount": 7,
                            "aws_secret_access_key": "must-not-cross-web",
                            "request-headers": { "authorization": "must-not-cross-web" }
                        },
                        "locations": [
                            { "path": "/home/alice/secret-project/src/lib.rs", "line": 7 }
                        ],
                        "rawOutput": {
                            "text": "visible\n  output",
                            "message": "request used sk-web-secret-1234567890"
                        },
                        "_meta": {
                            "codex": { "phase": "analysis", "internal": "hidden" },
                            "adapter": { "trace": "hidden" }
                        },
                        "inputTokens": 42
                    }
                }),
            },
        };

        let projected = serde_json::to_value(project_acp_event_for_web(&event)).unwrap();
        assert!(projected["event"]["data"].get("notificationMeta").is_none());
        assert_eq!(
            projected["event"]["data"]["update"]["rawInput"]["path"],
            "README.md"
        );
        assert_eq!(
            projected["event"]["data"]["update"]["rawInput"]["cwd"],
            "secret-project"
        );
        assert_eq!(
            projected["event"]["data"]["update"]["rawInput"]["filePath"],
            "main.rs"
        );
        assert_eq!(
            projected["event"]["data"]["update"]["locations"][0]["path"],
            "lib.rs"
        );
        assert!(
            projected["event"]["data"]["update"]["rawInput"]
                .get("environment")
                .is_none()
        );
        assert!(
            projected["event"]["data"]["update"]["rawInput"]
                .get("apiKey")
                .is_none()
        );
        assert!(
            projected["event"]["data"]["update"]["rawInput"]
                .get("access_token")
                .is_none()
        );
        assert!(
            projected["event"]["data"]["update"]["rawInput"]
                .get("request-headers")
                .is_none()
        );
        for key in [
            "OPENAI_API_KEY",
            "x-api-key",
            "id_token",
            "custom_token_value",
            "aws_secret_access_key",
        ] {
            assert!(
                projected["event"]["data"]["update"]["rawInput"]
                    .get(key)
                    .is_none()
            );
        }
        assert_eq!(
            projected["event"]["data"]["update"]["rawOutput"]["text"],
            "visible\n  output"
        );
        assert_eq!(
            projected["event"]["data"]["update"]["rawInput"]["tokenCount"],
            7
        );
        assert_eq!(
            projected["event"]["data"]["update"]["rawOutput"]["message"],
            "request used [REDACTED]"
        );
        assert_eq!(
            projected["event"]["data"]["update"]["_meta"],
            json!({ "codex": { "phase": "analysis" } })
        );
        assert_eq!(projected["event"]["data"]["update"]["inputTokens"], 42);
    }

    #[test]
    fn web_projection_redacts_absolute_paths_inside_arbitrary_nested_strings() {
        let mut event = event(8, Some("turn-1"), "tool_call_update");
        event.event.data = json!({
            "update": {
                "toolCallId": "tool-1",
                "rawInput": {
                    "command": "type C:\\Users\\alice\\private\\secrets.txt",
                    "arguments": [
                        { "text": "cat /home/alice/private/config.toml" },
                        { "message": "read file:///Users/alice/private/key.txt" },
                        { "text": "tail ~/.pinvou3/sessions/private.jsonl" }
                    ],
                    "C:\\Users\\alice\\private\\as-a-key": "must-not-cross-web",
                    "relativeHint": "docs/guide.md",
                    "publicUrl": "https://example.test/docs/guide"
                },
                "rawOutput": [{
                    "message": "opened \\\\fileserver\\private\\report.txt"
                }]
            }
        });

        let projected = serde_json::to_value(project_acp_event_for_web(&event)).unwrap();
        let update = &projected["event"]["data"]["update"];
        assert_eq!(
            update["rawInput"]["command"],
            format!("type {HOST_PATH_REDACTION}")
        );
        assert_eq!(
            update["rawInput"]["arguments"][0]["text"],
            format!("cat {HOST_PATH_REDACTION}")
        );
        assert_eq!(
            update["rawInput"]["arguments"][1]["message"],
            format!("read {HOST_PATH_REDACTION}")
        );
        assert_eq!(
            update["rawInput"]["arguments"][2]["text"],
            format!("tail {HOST_PATH_REDACTION}")
        );
        assert_eq!(
            update["rawOutput"][0]["message"],
            format!("opened {HOST_PATH_REDACTION}")
        );
        assert_eq!(update["rawInput"]["relativeHint"], "docs/guide.md");
        assert_eq!(
            update["rawInput"]["publicUrl"],
            "https://example.test/docs/guide"
        );

        let wire = serde_json::to_string(&projected).unwrap();
        for private_fragment in [
            "C:\\\\Users",
            "/home/alice",
            "file:///Users",
            "~/.pinvou3",
            "fileserver",
        ] {
            assert!(
                !wire.contains(private_fragment),
                "private path fragment crossed Web projection: {private_fragment}"
            );
        }
    }

    #[test]
    fn ordered_web_delivery_preserves_sequence_under_concurrency() {
        let delivery = OrderedWebDelivery::new(0);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut threads = Vec::new();
        for seq in [2_u64, 1_u64] {
            let delivery = delivery.clone();
            let seen = Arc::clone(&seen);
            let barrier = Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                delivery.deliver(seq, || seen.lock().push(seq));
            }));
        }
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }

        assert_eq!(*seen.lock(), vec![1, 2]);
    }

    #[test]
    fn ordered_web_delivery_skips_gap_after_bounded_wait() {
        let delivery = OrderedWebDelivery::with_wait_timeout(0, Duration::from_millis(20));
        let seen = Arc::new(Mutex::new(Vec::new()));

        // Sequence 1 never arrives (its producer died before delivering).
        delivery.deliver(2, || seen.lock().push(2));

        assert_eq!(*seen.lock(), vec![2], "delivery must skip the dead gap");

        // A late predecessor must not double-deliver or regress the cursor.
        delivery.deliver(1, || seen.lock().push(1));
        assert_eq!(*seen.lock(), vec![2]);

        // Later sequences continue in order without re-blocking.
        delivery.deliver(3, || seen.lock().push(3));
        assert_eq!(*seen.lock(), vec![2, 3]);
    }

    #[test]
    fn ordered_web_delivery_timeout_does_not_lose_arriving_predecessor() {
        let delivery = OrderedWebDelivery::with_wait_timeout(0, Duration::from_secs(30));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let later = {
            let delivery = delivery.clone();
            let seen = Arc::clone(&seen);
            std::thread::spawn(move || delivery.deliver(2, || seen.lock().push(2)))
        };
        std::thread::sleep(Duration::from_millis(50));
        delivery.deliver(1, || seen.lock().push(1));
        later.join().unwrap();

        // The predecessor arrived inside the wait window, so strict order holds.
        assert_eq!(*seen.lock(), vec![1, 2]);
    }

    #[test]
    fn web_projection_preserves_elicitation_form_metadata_and_property_names() {
        let projected = project_acp_value_for_web(json!({
            "request": {
                "requestedSchema": {
                    "properties": {
                        "password": {
                            "type": "string",
                            "default": "must-not-cross-web",
                            "examples": ["must-not-cross-web"],
                            "_meta": {
                                "codex": {
                                    "isSecret": true,
                                    "isOther": false,
                                    "questionId": "credentials"
                                },
                                "adapter": { "trace": "hidden" }
                            }
                        }
                    }
                }
            }
        }));

        let password = &projected["request"]["requestedSchema"]["properties"]["password"];
        assert_eq!(password["type"], "string");
        assert!(password.get("default").is_none());
        assert!(password.get("examples").is_none());
        assert_eq!(
            password["_meta"],
            json!({
                "codex": {
                    "isSecret": true,
                    "isOther": false,
                    "questionId": "credentials"
                }
            })
        );
    }

    #[test]
    fn web_projection_does_not_treat_arbitrary_properties_as_a_schema() {
        let projected = project_acp_value_for_web(json!({
            "toolOutput": {
                "properties": {
                    "accessToken": "must-not-cross-web",
                    "visible": "kept"
                }
            }
        }));
        let properties = &projected["toolOutput"]["properties"];
        assert!(properties.get("accessToken").is_none());
        assert_eq!(properties["visible"], "kept");
    }

    #[test]
    fn web_projection_redacts_secret_like_error_text() {
        let projected = project_acp_value_for_web(json!({
            "status": "failed",
            "error": "request failed with synthetic-redaction-value-1234567890"
        }));
        let error = projected["error"].as_str().unwrap();
        assert!(!error.contains("synthetic-redaction-value"));
        assert!(error.contains("[REDACTED]"));
    }

    #[test]
    fn web_session_info_projection_preserves_shape_and_redacts_adapter_strings() {
        let projected = project_acp_value_for_web(json!({
            "session_id": "session-1",
            "current_model_id": "model-1",
            "models": [{
                "id": "model-1",
                "name": "loaded from C:\\Users\\alice\\.codex\\models.json",
                "description": "cache /home/alice/.cache/acp/model.json"
            }],
            "modes": {
                "currentModeId": "agent",
                "availableModes": [{
                    "id": "agent",
                    "name": "Agent",
                    "description": "reads \\\\server\\private\\workspace"
                }]
            },
            "config_options": [{
                "id": "provider",
                "name": "Provider",
                "description": "file:///Users/alice/.config/provider.json",
                "currentValue": "safe"
            }],
            "provider": "synthetic-redaction-value-1234567890",
            "pending_permissions": [],
            "pending_elicitations": []
        }));

        assert_eq!(projected["session_id"], "session-1");
        assert_eq!(projected["models"][0]["id"], "model-1");
        assert_eq!(projected["modes"]["currentModeId"], "agent");
        assert!(projected["config_options"].is_array());
        let wire = serde_json::to_string(&projected).unwrap();
        for private_fragment in [
            "C:\\\\Users",
            "/home/alice",
            "file:///Users",
            "server",
            "synthetic-redaction-value",
        ] {
            assert!(
                !wire.contains(private_fragment),
                "private adapter string crossed SessionInfo projection: {private_fragment}"
            );
        }
    }

    #[test]
    fn web_projection_fails_closed_for_unknown_or_malformed_event_payloads() {
        let mut malformed = event(1, Some("turn-1"), "user_message");
        malformed.event.data = json!([{"content": "must-not-cross-web"}]);
        assert_eq!(
            project_acp_event_for_web(&malformed).event.data,
            json!({}),
            "known event types must still satisfy their expected object schema"
        );

        let mut future = event(2, Some("turn-1"), "adapter_private_future_event");
        future.event.data = json!({"apiKey": "must-not-cross-web", "visible": "also-private"});
        assert_eq!(
            project_acp_event_for_web(&future).event.data,
            json!({"webProjection": {"omitted": true}}),
            "new adapter events require an explicit Web projection before exposing data"
        );
    }

    #[test]
    fn web_timeline_reader_resumes_from_a_byte_cursor() {
        let events: Vec<_> = (1..=5).map(|seq| message_event(seq, "chunk")).collect();
        let timeline = TempTimeline::create(&events);

        let first = load_web_timeline_page_from_path(
            timeline.path(),
            "test",
            0,
            None,
            2,
            1024 * 1024,
            1024 * 1024,
        )
        .expect("load first ACP timeline page");
        assert_eq!(
            first
                .events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(first.has_more);
        let first_cursor = first.next_cursor.expect("first page cursor");
        assert!(first_cursor > 0);

        let second = load_web_timeline_page_from_path(
            timeline.path(),
            "test",
            2,
            Some(first_cursor),
            2,
            1024 * 1024,
            1024 * 1024,
        )
        .expect("resume ACP timeline page");
        assert_eq!(
            second
                .events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert!(second.has_more);
        assert!(second.next_cursor.expect("second page cursor") > first_cursor);

        assert!(
            load_web_timeline_page_from_path(
                timeline.path(),
                "test",
                0,
                Some(1),
                2,
                1024 * 1024,
                1024 * 1024,
            )
            .is_err()
        );
    }

    #[test]
    fn web_timeline_reader_truncates_large_events_without_blocking_later_events() {
        let timeline = TempTimeline::create(&[
            message_event(1, &"x".repeat(64 * 1024)),
            message_event(2, "after-large-event"),
        ]);

        let first =
            load_web_timeline_page_from_path(timeline.path(), "test", 0, None, 1, 4096, 4096)
                .expect("load bounded ACP timeline event");
        assert_eq!(first.events.len(), 1);
        assert_eq!(first.events[0].seq, 1);
        assert_eq!(
            first.events[0].event.data["webProjection"]["truncated"],
            true
        );
        assert!(serde_json::to_vec(&first.events[0]).unwrap().len() <= 4096);
        assert!(first.has_more);

        let second = load_web_timeline_page_from_path(
            timeline.path(),
            "test",
            1,
            first.next_cursor,
            1,
            4096,
            4096,
        )
        .expect("load event after bounded ACP timeline event");
        assert_eq!(second.events.len(), 1);
        assert_eq!(second.events[0].seq, 2);
        assert!(!second.has_more);
    }

    #[test]
    fn web_timeline_reader_does_not_consume_a_concurrent_partial_append() {
        let timeline = TempTimeline::create(&[message_event(1, "complete")]);
        let serialized = serde_json::to_vec(&message_event(2, "concurrent")).unwrap();
        let split = serialized.len() / 2;
        timeline.append(&serialized[..split]);

        let first = load_web_timeline_page_from_path(
            timeline.path(),
            "test",
            0,
            None,
            10,
            1024 * 1024,
            1024 * 1024,
        )
        .expect("load timeline with a partial append");
        assert_eq!(first.events.len(), 1);
        assert_eq!(first.events[0].seq, 1);
        assert!(!first.has_more);

        timeline.append(&serialized[split..]);
        timeline.append(b"\n");
        let second = load_web_timeline_page_from_path(
            timeline.path(),
            "test",
            1,
            first.next_cursor,
            10,
            1024 * 1024,
            1024 * 1024,
        )
        .expect("resume after the concurrent append completes");
        assert_eq!(second.events.len(), 1);
        assert_eq!(second.events[0].seq, 2);
    }

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
    fn orphaned_turns_are_detected_in_start_order_and_recovery_is_idempotent() {
        let mut events = vec![
            event(1, Some("turn-completed"), "turn_started"),
            event(2, Some("turn-orphan-1"), "turn_started"),
            event(3, Some("turn-completed"), "turn_completed"),
            event(4, None, "cancel_requested"),
            event(5, Some("turn-orphan-2"), "turn_started"),
        ];
        assert_eq!(
            orphaned_turn_ids(&events),
            vec!["turn-orphan-1", "turn-orphan-2"],
            "global cancel events must not accidentally close a persisted turn"
        );

        events.push(event(6, Some("turn-orphan-1"), "turn_completed"));
        events.push(event(7, Some("turn-orphan-2"), "turn_completed"));
        assert!(
            orphaned_turn_ids(&events).is_empty(),
            "re-running recovery after terminal events must be a no-op"
        );
    }
}
