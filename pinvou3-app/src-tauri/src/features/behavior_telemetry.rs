use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Manager;
use tokio::sync::{mpsc, Mutex};

use crate::platform::{filesystem, paths};

const STATE_VERSION: u8 = 1;
const EVENT_QUEUE_CAPACITY: usize = 512;
const EVENT_FLUSH_MAX: usize = 50;
const EVENT_FLUSH_DEBOUNCE: Duration = Duration::from_millis(1500);

#[derive(Clone)]
pub struct BehaviorTelemetry {
    client: reqwest::Client,
    runtime: Arc<RuntimeConfig>,
    state: Arc<Mutex<Option<PersistedState>>>,
    credential_gate: Arc<Mutex<()>>,
    event_tx: Option<mpsc::Sender<BehaviorEvent>>,
}

#[derive(Debug)]
struct RuntimeConfig {
    enabled: bool,
    telemetry_base_url: String,
    behavior_events_url: String,
    enrollment_token: Option<String>,
    app_version: String,
    platform: String,
    arch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedState {
    version: u8,
    telemetry_base_url: String,
    behavior_events_url: String,
    installation_id: String,
    registration_secret: String,
    device_id: Option<String>,
    device_token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BehaviorEvent {
    pub event_id: String,
    pub event_name: &'static str,
    pub occurred_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_l1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_l2: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
}

impl BehaviorEvent {
    pub fn new(event_name: &'static str) -> Self {
        Self {
            event_id: event_id(),
            event_name,
            occurred_at: now_ms(),
            app_version: None,
            platform: None,
            session_id: None,
            turn_id: None,
            input_type: None,
            status: None,
            stage: None,
            tool_key: None,
            tool_name: None,
            tool_type: None,
            success: None,
            scene_l1: None,
            scene_l2: None,
            provider_key: None,
            provider_name: None,
            model_id: None,
            model_name: None,
        }
    }

    pub fn session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = nonempty(session_id.into());
        self
    }

    pub fn turn(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = nonempty(turn_id.into());
        self
    }

    pub fn input_type(mut self, input_type: impl Into<String>) -> Self {
        self.input_type = nonempty(input_type.into());
        self
    }

    pub fn status(mut self, status: impl Into<String>) -> Self {
        self.status = nonempty(status.into());
        self
    }

    pub fn stage(mut self, stage: impl Into<String>) -> Self {
        self.stage = nonempty(stage.into());
        self
    }

    pub fn tool(
        mut self,
        key: impl Into<String>,
        name: impl Into<String>,
        tool_type: impl Into<String>,
    ) -> Self {
        self.tool_key = nonempty(key.into());
        self.tool_name = nonempty(name.into());
        self.tool_type = nonempty(tool_type.into());
        self
    }

    pub fn success(mut self, success: bool) -> Self {
        self.success = Some(success);
        self
    }

    pub fn scene(mut self, l1: impl Into<String>, l2: impl Into<String>) -> Self {
        self.scene_l1 = nonempty(l1.into());
        self.scene_l2 = nonempty(l2.into());
        self
    }

    pub fn model(
        mut self,
        provider_key: impl Into<String>,
        provider_name: impl Into<String>,
        model_id: impl Into<String>,
        model_name: impl Into<String>,
    ) -> Self {
        self.provider_key = nonempty(provider_key.into());
        self.provider_name = nonempty(provider_name.into());
        self.model_id = nonempty(model_id.into());
        self.model_name = nonempty(model_name.into());
        self
    }
}

impl BehaviorTelemetry {
    pub fn new() -> Self {
        let runtime = Arc::new(RuntimeConfig::from_env());
        let event_tx = runtime.enabled.then(|| {
            let (tx, rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
            tauri::async_runtime::spawn(Self::run_worker(rx, Self::worker_clone(runtime.clone())));
            tx
        });
        let telemetry = Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(12))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            runtime,
            state: Arc::new(Mutex::new(load_state().ok().flatten())),
            credential_gate: Arc::new(Mutex::new(())),
            event_tx,
        };
        if telemetry.runtime.enabled && telemetry.event_tx.is_none() {
            log::debug!("[pinvou3][behavior] telemetry disabled: queue initialization failed");
        }
        telemetry
    }

    pub fn track(&self, event: BehaviorEvent) {
        let Some(tx) = &self.event_tx else {
            return;
        };
        if let Err(error) = tx.try_send(event) {
            log::debug!("[pinvou3][behavior] telemetry queue skipped event: {error}");
        }
    }

    fn worker_clone(runtime: Arc<RuntimeConfig>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(12))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            runtime,
            state: Arc::new(Mutex::new(load_state().ok().flatten())),
            credential_gate: Arc::new(Mutex::new(())),
            event_tx: None,
        }
    }

    async fn run_worker(mut rx: mpsc::Receiver<BehaviorEvent>, telemetry: Self) {
        let mut batch = Vec::with_capacity(EVENT_FLUSH_MAX);
        while let Some(event) = rx.recv().await {
            batch.push(event);
            while batch.len() < EVENT_FLUSH_MAX {
                match tokio::time::timeout(EVENT_FLUSH_DEBOUNCE, rx.recv()).await {
                    Ok(Some(event)) => batch.push(event),
                    Ok(None) | Err(_) => break,
                }
            }
            let events = std::mem::take(&mut batch);
            if let Err(error) = telemetry.send_events(events).await {
                log::debug!("[pinvou3][behavior] telemetry skipped: {error:#}");
            }
        }
    }

    async fn send_events(&self, events: Vec<BehaviorEvent>) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let credentials = self.ensure_credentials().await?;
        let events: Vec<Value> = events
            .into_iter()
            .filter_map(|event| {
                let mut value = serde_json::to_value(event).ok()?;
                if let Value::Object(ref mut object) = value {
                    object
                        .entry("app_version".to_string())
                        .or_insert_with(|| Value::String(self.runtime.app_version.clone()));
                    object
                        .entry("platform".to_string())
                        .or_insert_with(|| Value::String(self.runtime.platform.clone()));
                }
                Some(value)
            })
            .collect();
        let body = json!({
            "device_id": credentials.device_id,
            "events": events,
        });
        let response = self
            .client
            .post(&credentials.behavior_events_url)
            .bearer_auth(&credentials.device_token)
            .json(&body)
            .send()
            .await
            .context("send behavior events")?;
        if !response.status().is_success() {
            anyhow::bail!("behavior events endpoint returned {}", response.status());
        }
        Ok(())
    }

    async fn ensure_credentials(&self) -> Result<Credentials> {
        if let Some(credentials) = self.current_credentials().await {
            return Ok(credentials);
        }
        let _credential_guard = self.credential_gate.lock().await;
        if let Some(credentials) = self.current_credentials().await {
            return Ok(credentials);
        }
        let enrollment_token = self
            .runtime
            .enrollment_token
            .as_deref()
            .context("PINVOU_TELEMETRY_ENROLLMENT_TOKEN is not configured")?;
        let mut state = {
            let mut guard = self.state.lock().await;
            let state = guard.get_or_insert_with(|| PersistedState::fresh(&self.runtime));
            state.clone()
        };
        if state.telemetry_base_url != self.runtime.telemetry_base_url
            || state.behavior_events_url != self.runtime.behavior_events_url
        {
            state = PersistedState::fresh(&self.runtime);
        }
        let body = json!({
            "enrollment_token": enrollment_token,
            "hardware_claim": format!("pinvou3-installation:{}", state.installation_id),
            "registration_secret": state.registration_secret,
            "hardware_source": "installation_id",
            "identity_quality": "installation_only",
            "app_version": self.runtime.app_version,
            "platform": self.runtime.platform,
            "arch": self.runtime.arch,
        });
        let response = self
            .client
            .post(format!(
                "{}/v1/register",
                state.telemetry_base_url.trim_end_matches('/')
            ))
            .json(&body)
            .send()
            .await
            .context("register behavior telemetry device")?;
        if !response.status().is_success() {
            anyhow::bail!("telemetry registration returned {}", response.status());
        }
        let registered: RegistrationResponse = response
            .json()
            .await
            .context("parse telemetry registration response")?;
        state.device_id = Some(registered.device_id);
        state.device_token = Some(registered.device_token);
        persist_state(&state)?;
        let mut guard = self.state.lock().await;
        *guard = Some(state);
        self.current_credentials()
            .await
            .context("telemetry credentials missing after registration")
    }

    async fn current_credentials(&self) -> Option<Credentials> {
        let state = self.state.lock().await.clone()?;
        if state.telemetry_base_url != self.runtime.telemetry_base_url
            || state.behavior_events_url != self.runtime.behavior_events_url
        {
            return None;
        }
        let device_id = state.device_id?;
        let device_token = state.device_token?;
        Some(Credentials {
            device_id,
            device_token,
            behavior_events_url: state.behavior_events_url,
        })
    }
}

#[derive(Debug, Deserialize)]
struct RegistrationResponse {
    device_id: String,
    device_token: String,
}

#[derive(Debug)]
struct Credentials {
    device_id: String,
    device_token: String,
    behavior_events_url: String,
}

impl RuntimeConfig {
    fn from_env() -> Self {
        let requested_enabled = std::env::var("PINVOU_BEHAVIOR_TELEMETRY_ENABLED")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            });
        let telemetry_base_url = std::env::var("PINVOU_TELEMETRY_BASE_URL")
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let behavior_events_url = std::env::var("PINVOU_BEHAVIOR_EVENTS_URL").unwrap_or_default();
        let enrollment_token = std::env::var("PINVOU_TELEMETRY_ENROLLMENT_TOKEN")
            .ok()
            .filter(|value| value.len() >= 24);
        let enabled =
            requested_enabled && !telemetry_base_url.is_empty() && !behavior_events_url.is_empty();
        Self {
            enabled,
            telemetry_base_url,
            behavior_events_url,
            enrollment_token,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            platform: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        }
    }
}

impl PersistedState {
    fn fresh(runtime: &RuntimeConfig) -> Self {
        Self {
            version: STATE_VERSION,
            telemetry_base_url: runtime.telemetry_base_url.clone(),
            behavior_events_url: runtime.behavior_events_url.clone(),
            installation_id: token("inst_", 18),
            registration_secret: token("rs_", 32),
            device_id: None,
            device_token: None,
        }
    }
}

fn load_state() -> Result<Option<PersistedState>> {
    let path = state_path();
    if !path.is_file() {
        return Ok(None);
    }
    let data = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let state: PersistedState =
        serde_json::from_slice(&data).with_context(|| format!("parse {}", path.display()))?;
    if state.version != STATE_VERSION {
        return Ok(None);
    }
    Ok(Some(state))
}

fn persist_state(state: &PersistedState) -> Result<()> {
    let path = state_path();
    let parent = path
        .parent()
        .with_context(|| format!("invalid behavior telemetry path: {}", path.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let data = serde_json::to_vec_pretty(state).context("serialize behavior telemetry state")?;
    write_private_file(&path, &data).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn write_private_file(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut file = filesystem::create_secret_file(path)?;
    file.write_all(data)?;
    file.sync_all()
}

fn state_path() -> PathBuf {
    paths::pinvou3_home().join("behavior-telemetry.json")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn event_id() -> String {
    token("beh_", 18)
}

fn token(prefix: &str, bytes: usize) -> String {
    let mut raw = vec![0_u8; bytes];
    use rand::RngCore as _;
    rand::rng().fill_bytes(&mut raw);
    format!(
        "{}{}",
        prefix,
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
    )
}

fn nonempty(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.chars().take(128).collect())
}

pub fn track(app: &tauri::AppHandle, event: BehaviorEvent) {
    if let Some(telemetry) = app.try_state::<BehaviorTelemetry>() {
        telemetry.track(event);
    }
}

pub fn track_tool_call(
    app: &tauri::AppHandle,
    session_id: &str,
    turn_id: Option<&str>,
    tool_name: &str,
    success: bool,
) {
    let mut event = BehaviorEvent::new("tool_call_completed")
        .session(session_id)
        .tool(tool_name, tool_name, classify_tool(tool_name))
        .success(success);
    if let Some(turn_id) = turn_id {
        event = event.turn(turn_id);
    }
    track(app, event);
}

pub fn track_model_used(
    app: &tauri::AppHandle,
    session_id: &str,
    turn_id: &str,
    bridge: &crate::features::assistant::platform::bridge::Pinvou3Bridge,
) {
    let Some(model) = bridge.effective_model_owned() else {
        return;
    };
    let provider_key = model
        .vendor
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| model.preset.as_str())
        .to_ascii_lowercase();
    let provider_name = provider_key.clone();
    let model_id = model.model.trim();
    if model_id.is_empty() {
        return;
    }
    track(
        app,
        BehaviorEvent::new("model_used")
            .session(session_id)
            .turn(turn_id)
            .model(provider_key, provider_name, model_id, model_id),
    );
}

pub fn classify_tool(tool_name: &str) -> &'static str {
    if tool_name.starts_with("mcp_") || tool_name.contains("-mcp") {
        "mcp"
    } else if matches!(tool_name, "Bash" | "exec_shell" | "task_shell_start") {
        "cli"
    } else if matches!(tool_name, "load_skill" | "tool_search") {
        "skill"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behavior_event_serializes_sparse_fields() {
        let event = BehaviorEvent::new("tool_call_completed")
            .session("s1")
            .turn("t1")
            .tool(
                "mcp_weather_get_weather",
                "mcp_weather_get_weather",
                classify_tool("mcp_weather_get_weather"),
            )
            .success(false);
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["event_name"], "tool_call_completed");
        assert_eq!(value["session_id"], "s1");
        assert_eq!(value["turn_id"], "t1");
        assert_eq!(value["tool_type"], "mcp");
        assert_eq!(value["success"], false);
        assert!(value.get("app_version").is_none());
        assert!(value.get("scene_l1").is_none());
    }

    #[test]
    fn generated_registration_secret_meets_server_minimum() {
        let runtime = RuntimeConfig {
            enabled: true,
            telemetry_base_url: "https://example.test/pinvou3/telemetry".to_string(),
            behavior_events_url: "https://example.test/pinvou3/stats/api/behavior/events"
                .to_string(),
            enrollment_token: None,
            app_version: "0.0.0".to_string(),
            platform: "test".to_string(),
            arch: "test".to_string(),
        };
        let state = PersistedState::fresh(&runtime);
        assert!(state.registration_secret.len() >= 32);
        assert!(state.installation_id.starts_with("inst_"));
    }
}
