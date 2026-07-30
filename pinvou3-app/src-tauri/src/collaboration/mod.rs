use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use rand::distr::Alphanumeric;
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const PROTOCOL_VERSION: u8 = 1;
const MAX_TASK_CONTEXT_BYTES: usize = 2 * 1024 * 1024;
const MAX_INLINE_ARTIFACT_BYTES: u64 = 256 * 1024;

#[derive(Clone)]
pub struct CollaborationManager {
    app: AppHandle,
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    config: CollaborationConfig,
    persisted: CollaborationPersistedConfig,
    tx: Option<mpsc::UnboundedSender<Value>>,
    connected: bool,
    peers: Vec<PeerInfo>,
    incoming_tasks: Vec<CollaborationTask>,
    outgoing_tasks: Vec<CollaborationTask>,
    local_tasks: Vec<CollaborationTask>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationPersistedConfig {
    pub identity: Option<CollaborationIdentity>,
    pub project: Option<CollaborationProject>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationIdentity {
    pub peer_id: String,
    pub name: String,
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub description: String,
    pub device_token: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationProject {
    pub project_id: String,
    pub project_name: String,
    pub project_token: String,
    pub relay_ws_url: String,
    pub public_url: String,
    pub members: Vec<CollaborationProjectMember>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationProjectMember {
    pub member_id: String,
    pub name: String,
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub description: String,
    pub role: String,
    pub member_token: String,
    pub status: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationConfigState {
    pub identity_registered: bool,
    pub project_configured: bool,
    pub identity: Option<CollaborationIdentityPublic>,
    pub project: Option<CollaborationProjectPublic>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationIdentityPublic {
    pub peer_id: String,
    pub name: String,
    pub capabilities: Vec<String>,
    pub description: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationProjectPublic {
    pub project_id: String,
    pub project_name: String,
    pub relay_ws_url: String,
    pub public_url: String,
    pub members: Vec<CollaborationProjectMemberPublic>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationProjectMemberPublic {
    pub member_id: String,
    pub name: String,
    pub capabilities: Vec<String>,
    pub description: String,
    pub role: String,
    pub status: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationConfig {
    pub public_url: String,
    pub relay_ws_url: String,
    pub project_id: String,
    pub peer_id: String,
    pub display_name: String,
    #[serde(skip_serializing)]
    pub project_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationStatus {
    pub enabled: bool,
    pub connected: bool,
    pub reason: Option<String>,
    pub config: CollaborationPublicConfig,
    pub config_state: CollaborationConfigState,
    pub peers: Vec<PeerInfo>,
    pub incoming_tasks: Vec<CollaborationTask>,
    pub outgoing_tasks: Vec<CollaborationTask>,
    pub local_tasks: Vec<CollaborationTask>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationPublicConfig {
    pub public_url: String,
    pub relay_ws_url: String,
    pub project_id: String,
    pub peer_id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerInfo {
    #[serde(alias = "peer_id")]
    pub peer_id: String,
    #[serde(alias = "project_id")]
    pub project_id: String,
    #[serde(alias = "display_name")]
    pub display_name: String,
    #[serde(alias = "device_name")]
    pub device_name: String,
    pub status: String,
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(alias = "last_seen_at")]
    pub last_seen_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationTask {
    pub task_id: String,
    pub title: String,
    pub instruction: String,
    pub status: String,
    pub from_peer_id: String,
    pub from_display_name: String,
    pub to_peer_id: String,
    pub to_display_name: String,
    pub project_id: String,
    pub source_session_id: Option<String>,
    pub context_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_context: Option<TaskContextSummary>,
    pub risk_level: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskContextSummary {
    pub share_mode: String,
    pub message_count: usize,
    pub artifact_count: usize,
    pub inline_artifact_count: usize,
    pub byte_size: usize,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskRequest {
    pub to_peer_id: Option<String>,
    pub to_display_name: Option<String>,
    pub title: Option<String>,
    pub instruction: String,
    #[serde(alias = "source_session_id")]
    pub source_session_id: Option<String>,
    pub context_summary: Option<String>,
    #[serde(default)]
    pub task_context: Option<Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLocalTaskRequest {
    pub title: Option<String>,
    pub instruction: String,
    #[serde(alias = "source_session_id")]
    pub source_session_id: Option<String>,
    pub context_summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLocalTaskRequest {
    pub task_id: String,
    pub title: Option<String>,
    pub instruction: Option<String>,
    pub status: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartCollaborationRequest {
    pub name: String,
    pub collaboration_code: Option<String>,
    pub relay_ws_url: Option<String>,
    pub public_url: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterIdentityRequest {
    pub name: String,
    pub capabilities: Vec<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectRequest {
    pub project_name: String,
    pub relay_ws_url: Option<String>,
    pub public_url: Option<String>,
    pub members: Vec<ProjectMemberInput>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMemberInput {
    pub name: String,
    pub capabilities: Vec<String>,
    pub description: Option<String>,
    pub role: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinProjectRequest {
    pub invite_json: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationInvitePayload {
    pub relay_ws_url: String,
    pub public_url: String,
    pub project_id: String,
    pub project_name: String,
    pub project_token: String,
    pub member_id: String,
    pub member_name: String,
    pub member_token: String,
    pub capabilities: Vec<String>,
    pub description: Option<String>,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn short_token(len: usize) -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn env_or_default(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn config_path() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("collaboration")
        .join("config.json")
}

fn local_tasks_path() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("collaboration")
        .join("local_tasks.json")
}

fn task_context_path(task_id: &str) -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("collaboration")
        .join("tasks")
        .join(task_id)
        .join("context.json")
}

fn write_task_context(task_id: &str, context: &Value) -> Result<(), String> {
    let path = task_context_path(task_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(context).map_err(|error| error.to_string())?;
    std::fs::write(path, text).map_err(|error| error.to_string())
}

fn read_task_context(task_id: &str) -> Result<Value, String> {
    let path = task_context_path(task_id);
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

fn text_artifact_kind(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "md" | "markdown"
            | "txt"
            | "json"
            | "csv"
            | "html"
            | "htm"
            | "css"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "rs"
            | "toml"
            | "yaml"
            | "yml"
            | "xml"
            | "sql"
            | "log"
    )
}

fn sensitive_artifact_path(path: &Path) -> bool {
    let value = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    value.contains("/.git/")
        || value.contains("/node_modules/")
        || value.contains("/target/")
        || value.contains("/.venv/")
        || value.contains("/secrets/")
        || value.contains("/secret/")
        || value.contains("/tokens/")
        || value.contains("/credentials/")
        || value.ends_with("/.env")
        || value.contains("/.env.")
        || value.ends_with(".pem")
        || value.ends_with(".key")
}

fn basename(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string()
}

fn path_tail(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let parts = normalized.split('/').rev().take(4).collect::<Vec<_>>();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

fn build_task_context(
    store: &crate::features::sessions::SessionStore,
    session_id: &str,
) -> Result<(Value, TaskContextSummary), String> {
    let saved = store
        .load(session_id)
        .map_err(|error| format!("load task context session({session_id}): {error:?}"))?;
    let messages = serde_json::to_value(&saved.messages).map_err(|error| error.to_string())?;
    let mut artifact_values = Vec::new();
    let mut artifact_contents = Vec::new();
    let mut warnings = Vec::new();

    for artifact in &saved.artifacts {
        let path = &artifact.storage_path;
        let byte_size = std::fs::metadata(path)
            .map(|meta| meta.len())
            .unwrap_or(artifact.byte_size);
        let safe_for_inline = text_artifact_kind(path)
            && byte_size <= MAX_INLINE_ARTIFACT_BYTES
            && !sensitive_artifact_path(path);
        let inline_status = if safe_for_inline {
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    artifact_contents.push(json!({
                        "artifact_id": artifact.id,
                        "basename": basename(path),
                        "byte_size": byte_size,
                        "content": content,
                    }));
                    "inlined"
                }
                Err(error) => {
                    warnings.push(format!("产物 {} 读取失败：{}", basename(path), error));
                    "read_failed"
                }
            }
        } else if sensitive_artifact_path(path) {
            warnings.push(format!(
                "产物 {} 命中敏感路径规则，未内联内容",
                basename(path)
            ));
            "metadata_only_sensitive"
        } else if byte_size > MAX_INLINE_ARTIFACT_BYTES {
            "metadata_only_too_large"
        } else {
            "metadata_only_binary"
        };
        artifact_values.push(json!({
            "id": artifact.id,
            "basename": basename(path),
            "path_tail": path_tail(path),
            "kind": format!("{:?}", artifact.kind),
            "byte_size": byte_size,
            "created_at": artifact.created_at,
            "inline_status": inline_status,
        }));
    }

    let mut context = json!({
        "share_mode": "full_task_session",
        "session": {
            "id": saved.metadata.id,
            "title": saved.metadata.title,
            "message_count": saved.metadata.message_count,
            "updated_at": saved.metadata.updated_at,
        },
        "messages": messages,
        "artifacts": artifact_values,
        "artifact_contents": artifact_contents,
        "warnings": warnings,
    });

    let byte_size = serde_json::to_vec(&context)
        .map_err(|error| error.to_string())?
        .len();
    if byte_size > MAX_TASK_CONTEXT_BYTES {
        return Err(format!(
            "任务上下文过大：{} KB，当前限制 {} KB",
            byte_size / 1024,
            MAX_TASK_CONTEXT_BYTES / 1024
        ));
    }
    let artifact_count = context
        .get("artifacts")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let inline_artifact_count = context
        .get("artifact_contents")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let warnings = context
        .get("warnings")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let summary = TaskContextSummary {
        share_mode: "full_task_session".into(),
        message_count: saved.messages.len(),
        artifact_count,
        inline_artifact_count,
        byte_size,
        truncated: false,
        warnings,
    };
    context["summary"] = serde_json::to_value(&summary).map_err(|error| error.to_string())?;
    Ok((context, summary))
}

fn summarize_task_context(context: &Value) -> TaskContextSummary {
    if let Some(summary) = context.get("summary") {
        if let Ok(value) = serde_json::from_value::<TaskContextSummary>(summary.clone()) {
            return value;
        }
    }
    let byte_size = serde_json::to_vec(context)
        .map(|bytes| bytes.len())
        .unwrap_or(0);
    TaskContextSummary {
        share_mode: context
            .get("share_mode")
            .and_then(Value::as_str)
            .unwrap_or("full_task_session")
            .to_string(),
        message_count: context
            .pointer("/session/message_count")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| {
                context
                    .get("messages")
                    .and_then(Value::as_array)
                    .map(|values| values.len() as u64)
                    .unwrap_or(0)
            }) as usize,
        artifact_count: context
            .get("artifacts")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        inline_artifact_count: context
            .get("artifact_contents")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        byte_size,
        truncated: false,
        warnings: context
            .get("warnings")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    }
}

fn normalize_capabilities(values: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() || out.iter().any(|existing| existing == trimmed) {
            continue;
        }
        out.push(trimmed.chars().take(40).collect());
        if out.len() >= 12 {
            break;
        }
    }
    out
}

fn normalize_description(value: Option<&str>, name: &str, capabilities: &[String]) -> String {
    let provided = value.map(str::trim).filter(|value| !value.is_empty());
    if let Some(description) = provided {
        return description.chars().take(220).collect();
    }
    if capabilities.is_empty() {
        return format!("适合处理需要 {name} 协作确认、补充信息或推进落地的任务。");
    }
    format!(
        "适合处理{}相关任务；需要{}时优先找{}。",
        capabilities.join("、"),
        capabilities.join("、"),
        name
    )
    .chars()
    .take(220)
    .collect()
}

fn read_persisted_config() -> CollaborationPersistedConfig {
    let path = config_path();
    let Ok(text) = std::fs::read_to_string(path) else {
        return CollaborationPersistedConfig::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn write_persisted_config(config: &CollaborationPersistedConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    std::fs::write(path, text).map_err(|error| error.to_string())
}

fn read_local_tasks() -> Vec<CollaborationTask> {
    let path = local_tasks_path();
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn write_local_tasks(tasks: &[CollaborationTask]) -> Result<(), String> {
    let path = local_tasks_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(tasks).map_err(|error| error.to_string())?;
    std::fs::write(path, text).map_err(|error| error.to_string())
}

impl CollaborationConfig {
    fn from_persisted(persisted: &CollaborationPersistedConfig) -> Option<Self> {
        let identity = persisted.identity.as_ref()?;
        let project = persisted.project.as_ref()?;
        Some(Self {
            public_url: project.public_url.clone(),
            relay_ws_url: project.relay_ws_url.clone(),
            project_id: project.project_id.clone(),
            peer_id: identity.peer_id.clone(),
            display_name: identity.name.clone(),
            project_token: project.project_token.clone(),
        })
    }

    fn from_persisted_or_env(persisted: &CollaborationPersistedConfig) -> Self {
        Self::from_persisted(persisted).unwrap_or_else(Self::from_env)
    }

    fn from_env() -> Self {
        Self {
            public_url: env_or_default(
                "PINVOU_COLLAB_PUBLIC_URL",
                "https://pinvou.com/pinvou3/collaboration-test",
            ),
            relay_ws_url: env_or_default("PINVOU_COLLAB_RELAY_WS_URL", ""),
            project_id: env_or_default("PINVOU_COLLAB_PROJECT_ID", "pinvou"),
            peer_id: env_or_default("PINVOU_COLLAB_PEER_ID", ""),
            display_name: env_or_default("PINVOU_COLLAB_DISPLAY_NAME", ""),
            project_token: env_or_default("PINVOU_COLLAB_PROJECT_TOKEN", ""),
        }
    }

    fn enabled_reason(&self) -> Option<String> {
        if self.relay_ws_url.is_empty() {
            return Some("PINVOU_COLLAB_RELAY_WS_URL 未配置".into());
        }
        if self.project_token.is_empty() {
            return Some("PINVOU_COLLAB_PROJECT_TOKEN 未配置".into());
        }
        if self.peer_id.is_empty() {
            return Some("PINVOU_COLLAB_PEER_ID 未配置".into());
        }
        None
    }

    fn public(&self) -> CollaborationPublicConfig {
        CollaborationPublicConfig {
            public_url: self.public_url.clone(),
            relay_ws_url: self.relay_ws_url.clone(),
            project_id: self.project_id.clone(),
            peer_id: self.peer_id.clone(),
            display_name: if self.display_name.is_empty() {
                self.peer_id.clone()
            } else {
                self.display_name.clone()
            },
        }
    }
}

impl CollaborationPersistedConfig {
    fn public_state(&self) -> CollaborationConfigState {
        CollaborationConfigState {
            identity_registered: self.identity.is_some(),
            project_configured: self.project.is_some(),
            identity: self
                .identity
                .as_ref()
                .map(|identity| CollaborationIdentityPublic {
                    peer_id: identity.peer_id.clone(),
                    name: identity.name.clone(),
                    capabilities: identity.capabilities.clone(),
                    description: identity.description.clone(),
                    created_at: identity.created_at.clone(),
                }),
            project: self
                .project
                .as_ref()
                .map(|project| CollaborationProjectPublic {
                    project_id: project.project_id.clone(),
                    project_name: project.project_name.clone(),
                    relay_ws_url: project.relay_ws_url.clone(),
                    public_url: project.public_url.clone(),
                    members: project
                        .members
                        .iter()
                        .map(|member| CollaborationProjectMemberPublic {
                            member_id: member.member_id.clone(),
                            name: member.name.clone(),
                            capabilities: member.capabilities.clone(),
                            description: member.description.clone(),
                            role: member.role.clone(),
                            status: member.status.clone(),
                        })
                        .collect(),
                }),
        }
    }
}

impl CollaborationManager {
    pub fn new(app: AppHandle) -> Self {
        let persisted = read_persisted_config();
        let local_tasks = read_local_tasks();
        let config = CollaborationConfig::from_persisted_or_env(&persisted);
        Self {
            app,
            inner: Arc::new(Mutex::new(Inner {
                config,
                persisted,
                local_tasks,
                ..Inner::default()
            })),
        }
    }

    pub fn start_if_configured(&self) {
        let config = self.inner.lock().config.clone();
        if let Some(reason) = config.enabled_reason() {
            eprintln!("[pinvou3-app] collaboration disabled: {reason}");
            return;
        }
        self.spawn_client(config);
    }

    pub fn status(&self) -> CollaborationStatus {
        let inner = self.inner.lock();
        let reason = inner.config.enabled_reason();
        CollaborationStatus {
            enabled: reason.is_none(),
            connected: inner.connected,
            reason,
            config: inner.config.public(),
            config_state: inner.persisted.public_state(),
            peers: inner.peers.clone(),
            incoming_tasks: inner.incoming_tasks.clone(),
            outgoing_tasks: inner.outgoing_tasks.clone(),
            local_tasks: inner.local_tasks.clone(),
        }
    }

    pub fn config_state(&self) -> CollaborationConfigState {
        self.inner.lock().persisted.public_state()
    }

    pub fn start_collaboration(
        &self,
        request: StartCollaborationRequest,
    ) -> Result<CollaborationConfigState, String> {
        let name = request.name.trim();
        if name.is_empty() {
            return Err("名字不能为空".into());
        }
        let collaboration_code = request
            .collaboration_code
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("pinvou-task-mvp");
        let relay_ws_url = request
            .relay_ws_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                env_or_default(
                    "PINVOU_COLLAB_DEFAULT_RELAY_WS_URL",
                    "wss://pinvou.com/pinvou3/collaboration/ws",
                )
            });
        let public_url = request
            .public_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                env_or_default(
                    "PINVOU_COLLAB_DEFAULT_PUBLIC_URL",
                    "https://pinvou.com/pinvou3/collaboration",
                )
            });
        let project_token = env_or_default(
            "PINVOU_COLLAB_DEFAULT_PROJECT_TOKEN",
            "pinvou-task-mvp-token",
        );
        let mut inner = self.inner.lock();
        let created_at = now();
        let existing_identity = inner.persisted.identity.clone();
        let request_capabilities = normalize_capabilities(&request.capabilities);
        let capabilities = if request_capabilities.is_empty() {
            existing_identity
                .as_ref()
                .map(|identity| identity.capabilities.clone())
                .unwrap_or_default()
        } else {
            request_capabilities
        };
        let description = request
            .description
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                existing_identity
                    .as_ref()
                    .map(|identity| identity.description.clone())
                    .filter(|value| !value.trim().is_empty())
            })
            .unwrap_or_else(|| normalize_description(None, name, &capabilities));
        let identity = CollaborationIdentity {
            peer_id: existing_identity
                .as_ref()
                .map(|identity| identity.peer_id.clone())
                .unwrap_or_else(|| format!("peer_{}", short_token(18))),
            name: name.chars().take(40).collect(),
            capabilities: capabilities.clone(),
            description,
            device_token: existing_identity
                .as_ref()
                .map(|identity| identity.device_token.clone())
                .unwrap_or_else(|| format!("devtok_{}", short_token(32))),
            created_at: existing_identity
                .as_ref()
                .map(|identity| identity.created_at.clone())
                .unwrap_or(created_at),
        };
        inner.persisted.identity = Some(identity.clone());
        inner.persisted.project = Some(CollaborationProject {
            project_id: collaboration_code.chars().take(96).collect(),
            project_name: "Pinvou 一期协作".into(),
            project_token,
            relay_ws_url,
            public_url,
            members: vec![CollaborationProjectMember {
                member_id: "me".into(),
                name: identity.name.clone(),
                capabilities: identity.capabilities.clone(),
                description: identity.description.clone(),
                role: "member".into(),
                member_token: format!("memtok_{}", short_token(32)),
                status: "online".into(),
            }],
        });
        write_persisted_config(&inner.persisted)?;
        inner.config = CollaborationConfig::from_persisted_or_env(&inner.persisted);
        inner.peers.clear();
        let state = inner.persisted.public_state();
        self.emit_config_locked(&inner);
        self.emit_status_locked(&inner);
        drop(inner);
        self.start_if_configured();
        Ok(state)
    }

    pub fn register_identity(
        &self,
        request: RegisterIdentityRequest,
    ) -> Result<CollaborationConfigState, String> {
        let name = request.name.trim();
        if name.is_empty() {
            return Err("名字不能为空".into());
        }
        let mut inner = self.inner.lock();
        let created_at = now();
        let capabilities = normalize_capabilities(&request.capabilities);
        let identity = CollaborationIdentity {
            peer_id: inner
                .persisted
                .identity
                .as_ref()
                .map(|identity| identity.peer_id.clone())
                .unwrap_or_else(|| format!("peer_{}", short_token(18))),
            name: name.chars().take(40).collect(),
            capabilities: capabilities.clone(),
            description: normalize_description(request.description.as_deref(), name, &capabilities),
            device_token: inner
                .persisted
                .identity
                .as_ref()
                .map(|identity| identity.device_token.clone())
                .unwrap_or_else(|| format!("devtok_{}", short_token(32))),
            created_at: inner
                .persisted
                .identity
                .as_ref()
                .map(|identity| identity.created_at.clone())
                .unwrap_or(created_at),
        };
        inner.persisted.identity = Some(identity);
        write_persisted_config(&inner.persisted)?;
        inner.config = CollaborationConfig::from_persisted_or_env(&inner.persisted);
        let state = inner.persisted.public_state();
        self.emit_config_locked(&inner);
        self.emit_status_locked(&inner);
        Ok(state)
    }

    pub fn create_project(
        &self,
        request: CreateProjectRequest,
    ) -> Result<CollaborationConfigState, String> {
        let project_name = request.project_name.trim();
        if project_name.is_empty() {
            return Err("项目组名称不能为空".into());
        }
        let mut inner = self.inner.lock();
        let identity = inner
            .persisted
            .identity
            .clone()
            .ok_or_else(|| "请先创建你的 Pinvou 身份".to_string())?;
        let relay_ws_url = request
            .relay_ws_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("ws://127.0.0.1:8790/pinvou3/collaboration-test/ws")
            .to_string();
        let public_url = request
            .public_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("https://pinvou.com/pinvou3/collaboration-test")
            .to_string();
        let mut members = vec![CollaborationProjectMember {
            member_id: "me".into(),
            name: identity.name.clone(),
            capabilities: identity.capabilities.clone(),
            description: identity.description.clone(),
            role: "admin".into(),
            member_token: format!("memtok_{}", short_token(32)),
            status: "online".into(),
        }];
        for member in request.members {
            let name = member.name.trim();
            if name.is_empty() {
                continue;
            }
            let capabilities = normalize_capabilities(&member.capabilities);
            members.push(CollaborationProjectMember {
                member_id: format!("mem_{}", short_token(12)),
                name: name.chars().take(40).collect(),
                capabilities: capabilities.clone(),
                description: normalize_description(
                    member.description.as_deref(),
                    name,
                    &capabilities,
                ),
                role: member.role.unwrap_or_else(|| "member".into()),
                member_token: format!("memtok_{}", short_token(32)),
                status: "pending".into(),
            });
        }
        inner.persisted.project = Some(CollaborationProject {
            project_id: format!("prj_{}", short_token(18)),
            project_name: project_name.chars().take(80).collect(),
            project_token: format!("prjtok_{}", short_token(40)),
            relay_ws_url,
            public_url,
            members,
        });
        write_persisted_config(&inner.persisted)?;
        inner.config = CollaborationConfig::from_persisted_or_env(&inner.persisted);
        inner.peers.clear();
        let state = inner.persisted.public_state();
        self.emit_config_locked(&inner);
        self.emit_status_locked(&inner);
        drop(inner);
        self.start_if_configured();
        Ok(state)
    }

    pub fn join_project(
        &self,
        request: JoinProjectRequest,
    ) -> Result<CollaborationConfigState, String> {
        let invite: CollaborationInvitePayload =
            serde_json::from_str(&request.invite_json).map_err(|error| error.to_string())?;
        let mut inner = self.inner.lock();
        let existing_identity = inner.persisted.identity.clone();
        let identity = CollaborationIdentity {
            peer_id: existing_identity
                .as_ref()
                .map(|identity| identity.peer_id.clone())
                .unwrap_or_else(|| format!("peer_{}", short_token(18))),
            name: invite.member_name.clone(),
            capabilities: invite.capabilities.clone(),
            description: normalize_description(
                invite.description.as_deref(),
                &invite.member_name,
                &invite.capabilities,
            ),
            device_token: existing_identity
                .as_ref()
                .map(|identity| identity.device_token.clone())
                .unwrap_or_else(|| format!("devtok_{}", short_token(32))),
            created_at: existing_identity
                .map(|identity| identity.created_at)
                .unwrap_or_else(now),
        };
        inner.persisted.identity = Some(identity);
        let member_description = normalize_description(
            invite.description.as_deref(),
            &invite.member_name,
            &invite.capabilities,
        );
        inner.persisted.project = Some(CollaborationProject {
            project_id: invite.project_id,
            project_name: invite.project_name,
            project_token: invite.project_token,
            relay_ws_url: invite.relay_ws_url,
            public_url: invite.public_url,
            members: vec![CollaborationProjectMember {
                member_id: invite.member_id,
                name: invite.member_name,
                capabilities: invite.capabilities,
                description: member_description,
                role: "member".into(),
                member_token: invite.member_token,
                status: "online".into(),
            }],
        });
        write_persisted_config(&inner.persisted)?;
        inner.config = CollaborationConfig::from_persisted_or_env(&inner.persisted);
        inner.peers.clear();
        let state = inner.persisted.public_state();
        self.emit_config_locked(&inner);
        self.emit_status_locked(&inner);
        drop(inner);
        self.start_if_configured();
        Ok(state)
    }

    pub fn export_member_invite(
        &self,
        member_id: String,
    ) -> Result<CollaborationInvitePayload, String> {
        let inner = self.inner.lock();
        let project = inner
            .persisted
            .project
            .as_ref()
            .ok_or_else(|| "项目组未配置".to_string())?;
        let member = project
            .members
            .iter()
            .find(|member| member.member_id == member_id)
            .ok_or_else(|| "成员不存在".to_string())?;
        Ok(CollaborationInvitePayload {
            relay_ws_url: project.relay_ws_url.clone(),
            public_url: project.public_url.clone(),
            project_id: project.project_id.clone(),
            project_name: project.project_name.clone(),
            project_token: project.project_token.clone(),
            member_id: member.member_id.clone(),
            member_name: member.name.clone(),
            member_token: member.member_token.clone(),
            capabilities: member.capabilities.clone(),
            description: Some(member.description.clone()).filter(|value| !value.trim().is_empty()),
        })
    }

    pub fn create_task(&self, request: CreateTaskRequest) -> Result<CollaborationTask, String> {
        let instruction = request.instruction.trim();
        if instruction.is_empty() {
            return Err("任务内容不能为空".into());
        }
        let mut inner = self.inner.lock();
        if let Some(reason) = inner.config.enabled_reason() {
            return Err(reason);
        }
        let to_peer = self
            .resolve_peer_locked(
                &inner,
                request.to_peer_id.as_deref(),
                request.to_display_name.as_deref(),
            )
            .ok_or_else(|| "未找到已注册接收方".to_string())?;
        let created_at = now();
        let task_context = request.task_context;
        let task_context_summary = task_context.as_ref().map(summarize_task_context);
        let task = CollaborationTask {
            task_id: format!("pct_{}", short_token(18)),
            title: request
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .unwrap_or(instruction)
                .chars()
                .take(80)
                .collect(),
            instruction: instruction.to_string(),
            status: "waiting_delivery".into(),
            from_peer_id: inner.config.peer_id.clone(),
            from_display_name: inner.config.public().display_name,
            to_peer_id: to_peer.peer_id.clone(),
            to_display_name: to_peer.display_name.clone(),
            project_id: inner.config.project_id.clone(),
            source_session_id: request.source_session_id,
            context_summary: request.context_summary,
            task_context: task_context_summary,
            risk_level: "low".into(),
            created_at: created_at.clone(),
            updated_at: created_at,
        };
        let envelope = json!({
            "v": PROTOCOL_VERSION,
            "id": format!("msg_{}", short_token(18)),
            "type": "task_create",
            "from_peer_id": task.from_peer_id,
            "to_peer_id": task.to_peer_id,
            "project_id": task.project_id,
            "ts": now(),
            "payload": {
                "task_id": task.task_id,
                "title": task.title,
                "instruction": task.instruction,
                "context_summary": task.context_summary,
                "source_session_id": task.source_session_id,
                "task_context": task_context.clone(),
                "risk_level": task.risk_level,
                "from_display_name": task.from_display_name,
                "shared_scope": {
                    "policy": "project_default",
                    "messages": "full_task_session",
                    "code": "session_artifacts_metadata_or_text",
                    "logs": "included_when_in_session_messages",
                    "files": "text_artifacts_inlined_binary_metadata_only",
                    "secrets": "never"
                }
            }
        });
        self.send_locked(&inner, envelope)?;
        if let Some(context) = task_context.as_ref() {
            write_task_context(&task.task_id, context)?;
        }
        inner.outgoing_tasks.insert(0, task.clone());
        self.emit_status_locked(&inner);
        Ok(task)
    }

    pub fn accept_task(&self, task_id: String) -> Result<CollaborationTask, String> {
        self.update_incoming_and_send(task_id, "accepted", "task_accept")
    }

    pub fn reject_task(&self, task_id: String) -> Result<CollaborationTask, String> {
        self.update_incoming_and_send(task_id, "rejected", "task_reject")
    }

    pub fn create_local_task(
        &self,
        request: CreateLocalTaskRequest,
    ) -> Result<CollaborationTask, String> {
        let instruction = request.instruction.trim();
        if instruction.is_empty() {
            return Err("任务内容不能为空".into());
        }
        let mut inner = self.inner.lock();
        let created_at = now();
        let identity = inner.persisted.identity.as_ref();
        let peer_id = identity
            .map(|identity| identity.peer_id.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                if inner.config.peer_id.trim().is_empty() {
                    "self".into()
                } else {
                    inner.config.peer_id.clone()
                }
            });
        let display_name = identity
            .map(|identity| identity.name.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "我".into());
        let project_id = if inner.config.project_id.trim().is_empty() {
            "local".into()
        } else {
            inner.config.project_id.clone()
        };
        let task = CollaborationTask {
            task_id: format!("plt_{}", short_token(18)),
            title: request
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .unwrap_or(instruction)
                .chars()
                .take(80)
                .collect(),
            instruction: instruction.to_string(),
            status: "todo".into(),
            from_peer_id: peer_id.clone(),
            from_display_name: display_name.clone(),
            to_peer_id: peer_id,
            to_display_name: display_name,
            project_id,
            source_session_id: request.source_session_id,
            context_summary: request.context_summary,
            task_context: None,
            risk_level: "low".into(),
            created_at: created_at.clone(),
            updated_at: created_at,
        };
        inner.local_tasks.insert(0, task.clone());
        write_local_tasks(&inner.local_tasks)?;
        let _ = self.app.emit("collaboration:task_updated", task.clone());
        self.emit_status_locked(&inner);
        Ok(task)
    }

    pub fn list_local_tasks(&self) -> Vec<CollaborationTask> {
        self.inner.lock().local_tasks.clone()
    }

    pub fn update_local_task(
        &self,
        request: UpdateLocalTaskRequest,
    ) -> Result<CollaborationTask, String> {
        let mut inner = self.inner.lock();
        let index = inner
            .local_tasks
            .iter()
            .position(|task| task.task_id == request.task_id)
            .ok_or_else(|| "任务不存在".to_string())?;
        if let Some(title) = request
            .title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            inner.local_tasks[index].title = title.chars().take(80).collect();
        }
        if let Some(instruction) = request
            .instruction
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            inner.local_tasks[index].instruction = instruction.to_string();
        }
        if let Some(status) = request
            .status
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            inner.local_tasks[index].status = status.chars().take(40).collect();
        }
        inner.local_tasks[index].updated_at = now();
        let task = inner.local_tasks[index].clone();
        write_local_tasks(&inner.local_tasks)?;
        let _ = self.app.emit("collaboration:task_updated", task.clone());
        self.emit_status_locked(&inner);
        Ok(task)
    }

    pub fn complete_local_task(&self, task_id: String) -> Result<CollaborationTask, String> {
        self.update_local_task(UpdateLocalTaskRequest {
            task_id,
            title: None,
            instruction: None,
            status: Some("completed".into()),
        })
    }

    fn resolve_peer_locked(
        &self,
        inner: &Inner,
        peer_id: Option<&str>,
        display_name: Option<&str>,
    ) -> Option<PeerInfo> {
        if let Some(peer_id) = peer_id.map(str::trim).filter(|value| !value.is_empty()) {
            if let Some(peer) = inner.peers.iter().find(|peer| peer.peer_id == peer_id) {
                return Some(peer.clone());
            }
        }
        let display_name = display_name
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        inner
            .peers
            .iter()
            .find(|peer| peer.display_name == display_name)
            .cloned()
    }

    fn update_incoming_and_send(
        &self,
        task_id: String,
        status: &str,
        event_type: &str,
    ) -> Result<CollaborationTask, String> {
        let mut inner = self.inner.lock();
        let index = inner
            .incoming_tasks
            .iter()
            .position(|task| task.task_id == task_id)
            .ok_or_else(|| "任务不存在".to_string())?;
        let task = inner.incoming_tasks[index].clone();
        let envelope = json!({
            "v": PROTOCOL_VERSION,
            "id": format!("msg_{}", short_token(18)),
            "type": event_type,
            "from_peer_id": task.to_peer_id,
            "to_peer_id": task.from_peer_id,
            "project_id": task.project_id,
            "ts": now(),
            "payload": { "task_id": task.task_id, "status": status }
        });
        self.send_locked(&inner, envelope)?;
        inner.incoming_tasks[index].status = status.into();
        inner.incoming_tasks[index].updated_at = now();
        let task = inner.incoming_tasks[index].clone();
        self.emit_status_locked(&inner);
        Ok(task)
    }

    fn spawn_client(&self, config: CollaborationConfig) {
        let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
        {
            let mut inner = self.inner.lock();
            inner.tx = Some(tx.clone());
        }
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let ws_url = config.relay_ws_url.clone();
            let connect = tokio_tungstenite::connect_async(&ws_url).await;
            let (stream, _) = match connect {
                Ok(value) => value,
                Err(error) => {
                    manager.set_connected(false);
                    eprintln!("[pinvou3-app] collaboration relay connect failed: {error}");
                    return;
                }
            };
            manager.set_connected(true);
            let (mut write, mut read) = stream.split();
            let display_name = if config.display_name.is_empty() {
                config.peer_id.clone()
            } else {
                config.display_name.clone()
            };
            let (capabilities, description) = manager
                .inner
                .lock()
                .persisted
                .identity
                .as_ref()
                .map(|identity| {
                    let capabilities = if identity.capabilities.is_empty() {
                        vec!["协作任务".into()]
                    } else {
                        identity.capabilities.clone()
                    };
                    (capabilities, identity.description.clone())
                })
                .unwrap_or_else(|| (vec!["协作任务".into()], String::new()));
            let register = json!({
                "v": PROTOCOL_VERSION,
                "type": "peer_register",
                "peer_id": config.peer_id,
                "project_id": config.project_id,
                "project_token": config.project_token,
                "payload": {
                    "display_name": display_name,
                    "device_name": std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default(),
                    "capabilities": capabilities,
                    "description": description,
                    "app_version": env!("CARGO_PKG_VERSION")
                }
            });
            if write
                .send(Message::Text(register.to_string().into()))
                .await
                .is_err()
            {
                manager.set_connected(false);
                return;
            }
            let writer = tauri::async_runtime::spawn(async move {
                while let Some(value) = rx.recv().await {
                    if write
                        .send(Message::Text(value.to_string().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
            while let Some(message) = read.next().await {
                match message {
                    Ok(Message::Text(text)) => manager.handle_inbound_text(&text),
                    Ok(Message::Close(_)) => break,
                    Err(error) => {
                        eprintln!("[pinvou3-app] collaboration relay read failed: {error}");
                        break;
                    }
                    _ => {}
                }
            }
            writer.abort();
            manager.set_connected(false);
        });
    }

    fn set_connected(&self, connected: bool) {
        let mut inner = self.inner.lock();
        inner.connected = connected;
        if !connected {
            inner.tx = None;
            for peer in &mut inner.peers {
                peer.status = "offline".into();
            }
        }
        self.emit_status_locked(&inner);
    }

    fn handle_inbound_text(&self, text: &str) {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event_type {
            "peer_registered" | "peer_list" => self.handle_peer_list(value),
            "peer_status_changed" => self.handle_peer_status_changed(value),
            "task_create" => self.handle_task_create(value),
            "task_ack" => self.handle_task_status(value, "delivered"),
            "task_accept" => self.handle_task_status(value, "accepted"),
            "task_reject" => self.handle_task_status(value, "rejected"),
            "task_delivery_failed" => self.handle_task_status(value, "delivery_failed"),
            _ => {}
        }
    }

    fn handle_peer_list(&self, value: Value) {
        let peers = value
            .pointer("/payload/peers")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| serde_json::from_value::<PeerInfo>(item.clone()).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut inner = self.inner.lock();
        inner.peers = peers
            .into_iter()
            .filter(|peer| peer.peer_id != inner.config.peer_id)
            .collect();
        self.emit_status_locked(&inner);
    }

    fn handle_peer_status_changed(&self, value: Value) {
        let Some(payload) = value.get("payload") else {
            return;
        };
        let Ok(peer) = serde_json::from_value::<PeerInfo>(payload.clone()) else {
            return;
        };
        let mut inner = self.inner.lock();
        if peer.peer_id == inner.config.peer_id {
            return;
        }
        if let Some(existing) = inner
            .peers
            .iter_mut()
            .find(|existing| existing.peer_id == peer.peer_id)
        {
            *existing = peer;
        } else {
            inner.peers.push(peer);
        }
        self.emit_status_locked(&inner);
    }

    fn handle_task_create(&self, value: Value) {
        let payload = value.get("payload").cloned().unwrap_or_else(|| json!({}));
        let created_at = now();
        let from_peer_id = value
            .get("from_peer_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let to_peer_id = value
            .get("to_peer_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let project_id = value
            .get("project_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let title = payload
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("协作任务")
            .to_string();
        let task_context = payload.get("task_context").cloned();
        let task_context_summary = task_context.as_ref().map(summarize_task_context);
        let task = CollaborationTask {
            task_id: payload
                .get("task_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("pct_{}", short_token(18))),
            title: title.clone(),
            instruction: payload
                .get("instruction")
                .and_then(Value::as_str)
                .unwrap_or(&title)
                .to_string(),
            status: "needs_me".into(),
            from_display_name: payload
                .get("from_display_name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| self.display_name_for_peer(&from_peer_id)),
            from_peer_id,
            to_display_name: self.inner.lock().config.public().display_name,
            to_peer_id,
            project_id,
            source_session_id: payload
                .get("source_session_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            context_summary: payload
                .get("context_summary")
                .and_then(Value::as_str)
                .map(str::to_string),
            task_context: task_context_summary,
            risk_level: payload
                .get("risk_level")
                .and_then(Value::as_str)
                .unwrap_or("low")
                .to_string(),
            created_at: created_at.clone(),
            updated_at: created_at,
        };
        let mut inner = self.inner.lock();
        if inner
            .incoming_tasks
            .iter()
            .any(|existing| existing.task_id == task.task_id)
        {
            return;
        }
        if let Some(context) = task_context.as_ref() {
            if let Err(error) = write_task_context(&task.task_id, context) {
                eprintln!(
                    "[collaboration] failed to persist task context for {}: {}",
                    task.task_id, error
                );
            }
        }
        inner.incoming_tasks.insert(0, task.clone());
        let ack = json!({
            "v": PROTOCOL_VERSION,
            "id": format!("msg_{}", short_token(18)),
            "type": "task_ack",
            "from_peer_id": task.to_peer_id,
            "to_peer_id": task.from_peer_id,
            "project_id": task.project_id,
            "ts": now(),
            "payload": { "task_id": task.task_id, "status": "delivered" }
        });
        let _ = self.send_locked(&inner, ack);
        if self
            .app
            .try_state::<crate::platform::notifications::NotificationState>()
            .map(|state| {
                state.should_notify(format!("collaboration_task_received:{}", task.task_id))
            })
            .unwrap_or(true)
        {
            crate::platform::notifications::notify_collaboration_task_received(
                &self.app,
                &task.from_display_name,
                &task.title,
            );
        }
        let _ = self.app.emit("collaboration:incoming_task", task.clone());
        self.emit_status_locked(&inner);
    }

    fn handle_task_status(&self, value: Value, status: &str) {
        let task_id = value
            .pointer("/payload/task_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if task_id.is_empty() {
            return;
        }
        let mut inner = self.inner.lock();
        if let Some(task) = inner
            .outgoing_tasks
            .iter_mut()
            .find(|task| task.task_id == task_id)
        {
            task.status = status.into();
            task.updated_at = now();
            let _ = self.app.emit("collaboration:task_updated", task.clone());
        }
        self.emit_status_locked(&inner);
    }

    fn display_name_for_peer(&self, peer_id: &str) -> String {
        let inner = self.inner.lock();
        inner
            .peers
            .iter()
            .find(|peer| peer.peer_id == peer_id)
            .map(|peer| peer.display_name.clone())
            .unwrap_or_else(|| peer_id.to_string())
    }

    fn send_locked(&self, inner: &Inner, value: Value) -> Result<(), String> {
        let Some(tx) = &inner.tx else {
            return Err("协作网络未连接".into());
        };
        tx.send(value).map_err(|_| "协作网络已断开".to_string())
    }

    fn emit_status_locked(&self, inner: &Inner) {
        let status = CollaborationStatus {
            enabled: inner.config.enabled_reason().is_none(),
            connected: inner.connected,
            reason: inner.config.enabled_reason(),
            config: inner.config.public(),
            config_state: inner.persisted.public_state(),
            peers: inner.peers.clone(),
            incoming_tasks: inner.incoming_tasks.clone(),
            outgoing_tasks: inner.outgoing_tasks.clone(),
            local_tasks: inner.local_tasks.clone(),
        };
        let _ = self.app.emit("collaboration:status", status);
    }

    fn emit_config_locked(&self, inner: &Inner) {
        let _ = self
            .app
            .emit("collaboration:config", inner.persisted.public_state());
    }
}

#[tauri::command]
pub fn collaboration_status(
    manager: State<'_, CollaborationManager>,
) -> Result<CollaborationStatus, String> {
    Ok(manager.status())
}

#[tauri::command]
pub fn collaboration_get_config(
    manager: State<'_, CollaborationManager>,
) -> Result<CollaborationConfigState, String> {
    Ok(manager.config_state())
}

#[tauri::command]
pub fn collaboration_start(
    request: StartCollaborationRequest,
    manager: State<'_, CollaborationManager>,
) -> Result<CollaborationConfigState, String> {
    manager.start_collaboration(request)
}

#[tauri::command]
pub fn collaboration_register_identity(
    request: RegisterIdentityRequest,
    manager: State<'_, CollaborationManager>,
) -> Result<CollaborationConfigState, String> {
    manager.register_identity(request)
}

#[tauri::command]
pub fn collaboration_create_project(
    request: CreateProjectRequest,
    manager: State<'_, CollaborationManager>,
) -> Result<CollaborationConfigState, String> {
    manager.create_project(request)
}

#[tauri::command]
pub fn collaboration_join_project(
    request: JoinProjectRequest,
    manager: State<'_, CollaborationManager>,
) -> Result<CollaborationConfigState, String> {
    manager.join_project(request)
}

#[tauri::command]
pub fn collaboration_export_member_invite(
    member_id: String,
    manager: State<'_, CollaborationManager>,
) -> Result<CollaborationInvitePayload, String> {
    manager.export_member_invite(member_id)
}

#[tauri::command]
pub fn collaboration_create_task(
    mut request: CreateTaskRequest,
    manager: State<'_, CollaborationManager>,
    store: State<'_, crate::features::sessions::SessionStore>,
) -> Result<CollaborationTask, String> {
    let session_id = request
        .source_session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "发送协作任务需要当前会话上下文".to_string())?;
    let (task_context, _) = build_task_context(&store, session_id)?;
    request.task_context = Some(task_context);
    manager.create_task(request)
}

#[tauri::command]
pub fn collaboration_get_task_context(task_id: String) -> Result<Value, String> {
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return Err("缺少任务 ID".into());
    }
    read_task_context(task_id)
}

#[tauri::command]
pub fn collaboration_create_local_task(
    request: CreateLocalTaskRequest,
    manager: State<'_, CollaborationManager>,
) -> Result<CollaborationTask, String> {
    manager.create_local_task(request)
}

#[tauri::command]
pub fn collaboration_list_local_tasks(
    manager: State<'_, CollaborationManager>,
) -> Result<Vec<CollaborationTask>, String> {
    Ok(manager.list_local_tasks())
}

#[tauri::command]
pub fn collaboration_update_local_task(
    request: UpdateLocalTaskRequest,
    manager: State<'_, CollaborationManager>,
) -> Result<CollaborationTask, String> {
    manager.update_local_task(request)
}

#[tauri::command]
pub fn collaboration_complete_local_task(
    task_id: String,
    manager: State<'_, CollaborationManager>,
) -> Result<CollaborationTask, String> {
    manager.complete_local_task(task_id)
}

#[tauri::command]
pub fn collaboration_accept_task(
    task_id: String,
    manager: State<'_, CollaborationManager>,
) -> Result<CollaborationTask, String> {
    manager.accept_task(task_id)
}

#[tauri::command]
pub fn collaboration_reject_task(
    task_id: String,
    manager: State<'_, CollaborationManager>,
) -> Result<CollaborationTask, String> {
    manager.reject_task(task_id)
}
