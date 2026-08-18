use std::time::{Duration, Instant};

use serde_json::json;

use crate::core::model_endpoint::models_probe_url;
use crate::platform::prefs::UserPrefs;

use super::{
    CapabilityContract, ConnectivityObservation, ConnectivityStatus, Interruptibility,
    PinvouOsRuntime, ResourceClass,
};

pub const CONNECTIVITY_AGENT_ID: &str = "agent:connectivity";

pub fn connectivity_observe_contract() -> CapabilityContract {
    CapabilityContract {
        capability_id: "network.observe".to_string(),
        version: 1,
        summary: "验证当前联网路径并提供分层网络诊断事实".to_string(),
        input_schema: json!({ "type": "object", "additionalProperties": false }),
        output_schema: json!({
            "type": "object",
            "required": ["checkedAtMs", "status"],
            "properties": {
                "checkedAtMs": { "type": "integer" },
                "status": { "enum": ["unknown", "online", "degraded", "offline"] },
                "latencyMs": { "type": ["integer", "null"] },
                "reasonCode": { "type": ["string", "null"] }
            }
        }),
        preconditions: Vec::new(),
        // This capability exposes the Agent's sanitized health projection. It
        // does not grant raw network access or execute a recovery action.
        permissions: Vec::new(),
        side_effects: Vec::new(),
        resource_class: ResourceClass::Light,
        interruptibility: Interruptibility::Immediate,
        idempotent: true,
    }
}

/// Connectivity uses an unauthenticated request deliberately: any HTTP
/// response, including 401/403, proves that DNS, routing and TLS reached the
/// configured model endpoint. Model authorization remains Inference Agent's
/// responsibility.
pub async fn observe_active_route_connectivity() -> ConnectivityObservation {
    let checked_at_ms = chrono::Utc::now().timestamp_millis();
    let prefs = UserPrefs::load();
    let Some(model) = prefs.active_model() else {
        return ConnectivityObservation {
            checked_at_ms,
            status: ConnectivityStatus::Unknown,
            latency_ms: None,
            reason_code: Some("model_route_missing".to_string()),
        };
    };
    if model.base_url.trim().is_empty() {
        return ConnectivityObservation {
            checked_at_ms,
            status: ConnectivityStatus::Unknown,
            latency_ms: None,
            reason_code: Some("model_endpoint_missing".to_string()),
        };
    }
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            return ConnectivityObservation {
                checked_at_ms,
                status: ConnectivityStatus::Unknown,
                latency_ms: None,
                reason_code: Some("probe_unavailable".to_string()),
            };
        }
    };
    let started = Instant::now();
    match client.get(models_probe_url(&model.base_url)).send().await {
        Ok(_) => ConnectivityObservation {
            checked_at_ms,
            status: ConnectivityStatus::Online,
            latency_ms: Some(elapsed_ms(started)),
            reason_code: None,
        },
        Err(error) => ConnectivityObservation {
            checked_at_ms,
            status: if error.is_builder() {
                ConnectivityStatus::Unknown
            } else {
                ConnectivityStatus::Offline
            },
            latency_ms: Some(elapsed_ms(started)),
            reason_code: Some(
                if error.is_timeout() {
                    "network_timeout"
                } else if error.is_connect() {
                    "network_connect_failed"
                } else if error.is_builder() {
                    "model_endpoint_invalid"
                } else {
                    "network_request_failed"
                }
                .to_string(),
            ),
        },
    }
}

pub fn spawn_connectivity_agent(runtime: PinvouOsRuntime, interval: Duration) {
    tauri::async_runtime::spawn(async move {
        let mut last_signature: Option<(ConnectivityStatus, Option<String>)> = None;
        let mut last_emitted_at: Option<Instant> = None;
        loop {
            let observation = observe_active_route_connectivity().await;
            let signature = (observation.status, observation.reason_code.clone());
            let should_emit = last_signature.as_ref() != Some(&signature)
                || last_emitted_at
                    .is_none_or(|emitted_at| emitted_at.elapsed() >= Duration::from_secs(60));
            if should_emit {
                if let Err(error) = runtime.observe_connectivity(observation) {
                    log::warn!("[pinvou-os][connectivity] observation rejected: {error:#}");
                } else {
                    last_signature = Some(signature);
                    last_emitted_at = Some(Instant::now());
                }
            }
            tokio::time::sleep(interval).await;
        }
    });
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
