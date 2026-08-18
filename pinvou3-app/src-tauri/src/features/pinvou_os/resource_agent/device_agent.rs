use std::collections::BTreeSet;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{ClaimCandidate, ObservationHealth, DEFAULT_MAX_SAMPLE_AGE_MS, MAX_FUTURE_SKEW_MS};

pub const DEVICE_AGENT_ID: &str = "agent:device";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    AudioInput,
    AudioOutput,
    Camera,
    Display,
    Network,
    Storage,
    Accelerator,
    Battery,
    Sensor,
    Other,
}

/// Device Provider 的单设备观测。properties 只能承载非敏感、可序列化元数据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSnapshot {
    pub device_id: String,
    pub display_name: String,
    pub kind: DeviceKind,
    pub present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operational: Option<bool>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default = "empty_object")]
    pub properties: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInventoryObservation {
    pub sampled_at_ms: i64,
    #[serde(default)]
    pub devices: Vec<DeviceSnapshot>,
    /// Provider 级错误码；不得写入任意命令输出、路径或凭据。
    #[serde(default)]
    pub source_error_codes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceHealth {
    Available,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceFactSet {
    pub device_id: String,
    pub health: DeviceHealth,
    pub claims: Vec<ClaimCandidate>,
}

/// `device.inspect` 一次原子调用的完整输出。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAgentAssessment {
    pub assessed_at_ms: i64,
    pub health: ObservationHealth,
    pub devices: Vec<DeviceFactSet>,
    pub claims: Vec<ClaimCandidate>,
}

pub fn assess_device_inventory(
    observation: &DeviceInventoryObservation,
    assessed_at_ms: i64,
) -> Result<DeviceAgentAssessment> {
    validate_inventory(observation, assessed_at_ms)?;

    let stale =
        assessed_at_ms.saturating_sub(observation.sampled_at_ms) > DEFAULT_MAX_SAMPLE_AGE_MS;
    let health = if stale {
        ObservationHealth::Stale
    } else if observation.devices.is_empty() && !observation.source_error_codes.is_empty() {
        ObservationHealth::Unavailable
    } else if observation.source_error_codes.is_empty() {
        ObservationHealth::Healthy
    } else {
        ObservationHealth::Degraded
    };

    let observation_confidence: f32 = match health {
        ObservationHealth::Healthy => 1.0,
        ObservationHealth::Degraded => 0.75,
        ObservationHealth::Stale => 0.4,
        ObservationHealth::Unavailable => 0.0,
    };
    let mut devices = observation
        .devices
        .iter()
        .map(|device| {
            device_fact_set(
                device,
                observation.sampled_at_ms,
                health,
                observation_confidence,
            )
        })
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.device_id.cmp(&right.device_id));

    let available = devices
        .iter()
        .filter(|device| device.health == DeviceHealth::Available)
        .count();
    let degraded = devices
        .iter()
        .filter(|device| device.health == DeviceHealth::Degraded)
        .count();
    let unavailable = devices
        .iter()
        .filter(|device| device.health == DeviceHealth::Unavailable)
        .count();
    let claims = vec![
        ClaimCandidate {
            subject: DEVICE_AGENT_ID.to_string(),
            predicate: "observation_health".to_string(),
            value: json!({
                "status": health,
                "sourceErrorCodes": normalized_strings(&observation.source_error_codes),
            }),
            confidence: 1.0,
            observed_at_ms: observation.sampled_at_ms,
            asserted_by_actor_id: DEVICE_AGENT_ID.to_string(),
        },
        ClaimCandidate {
            subject: "device.inventory".to_string(),
            predicate: "health_summary".to_string(),
            value: json!({
                "total": devices.len(),
                "available": available,
                "degraded": degraded,
                "unavailable": unavailable,
            }),
            confidence: observation_confidence,
            observed_at_ms: observation.sampled_at_ms,
            asserted_by_actor_id: DEVICE_AGENT_ID.to_string(),
        },
    ];

    Ok(DeviceAgentAssessment {
        assessed_at_ms,
        health,
        devices,
        claims,
    })
}

fn device_fact_set(
    device: &DeviceSnapshot,
    observed_at_ms: i64,
    observation_health: ObservationHealth,
    confidence_cap: f32,
) -> DeviceFactSet {
    let health = if !device.present {
        DeviceHealth::Unavailable
    } else if device.operational == Some(true) && observation_health == ObservationHealth::Healthy {
        DeviceHealth::Available
    } else {
        DeviceHealth::Degraded
    };
    let subject = format!("device:{}", device.device_id);
    let base_confidence: f32 = if device.present && device.operational.is_none() {
        0.75
    } else {
        1.0
    };
    let confidence = base_confidence.min(confidence_cap);
    let claims = vec![
        ClaimCandidate {
            subject: subject.clone(),
            predicate: "presence".to_string(),
            value: json!({
                "present": device.present,
                "kind": device.kind,
                "displayName": device.display_name,
            }),
            confidence: confidence_cap,
            observed_at_ms,
            asserted_by_actor_id: DEVICE_AGENT_ID.to_string(),
        },
        ClaimCandidate {
            subject: subject.clone(),
            predicate: "operational_state".to_string(),
            value: json!({ "health": health, "operational": device.operational }),
            confidence,
            observed_at_ms,
            asserted_by_actor_id: DEVICE_AGENT_ID.to_string(),
        },
        ClaimCandidate {
            subject,
            predicate: "capabilities".to_string(),
            value: json!({
                "capabilityIds": normalized_strings(&device.capabilities),
                "properties": device.properties,
            }),
            confidence,
            observed_at_ms,
            asserted_by_actor_id: DEVICE_AGENT_ID.to_string(),
        },
    ];

    DeviceFactSet {
        device_id: device.device_id.clone(),
        health,
        claims,
    }
}

fn validate_inventory(observation: &DeviceInventoryObservation, assessed_at_ms: i64) -> Result<()> {
    if observation.sampled_at_ms <= 0 {
        bail!("device inventory timestamp must be positive");
    }
    if observation.sampled_at_ms > assessed_at_ms.saturating_add(MAX_FUTURE_SKEW_MS) {
        bail!("device inventory timestamp is too far in the future");
    }
    let mut device_ids = BTreeSet::new();
    for device in &observation.devices {
        validate_identifier(&device.device_id, "device id")?;
        validate_text(&device.display_name, "device display name")?;
        if !device_ids.insert(device.device_id.as_str()) {
            bail!("duplicate device id {}", device.device_id);
        }
        if !device.properties.is_object() {
            bail!("device properties must be a JSON object");
        }
        if contains_sensitive_property(&device.properties) {
            bail!("device properties must not contain credentials or secret material");
        }
        if !device.present && device.operational == Some(true) {
            bail!("an absent device cannot be operational");
        }
        for capability in &device.capabilities {
            validate_identifier(capability, "device capability id")?;
        }
    }
    for code in &observation.source_error_codes {
        validate_identifier(code, "device source error code")?;
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        bail!("{label} must not be empty");
    }
    if value.chars().count() > 128 {
        bail!("{label} is too long");
    }
    if !value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | ':' | '_' | '-')
    }) {
        bail!("{label} contains unsupported characters");
    }
    Ok(())
}

fn validate_text(value: &str, label: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        bail!("{label} must not be empty");
    }
    if value.chars().count() > 256 {
        bail!("{label} is too long");
    }
    Ok(())
}

fn normalized_strings(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn contains_sensitive_property(value: &Value) -> bool {
    const FORBIDDEN: &[&str] = &[
        "password",
        "passwd",
        "secret",
        "token",
        "apikey",
        "credential",
        "privatekey",
    ];
    match value {
        Value::Object(properties) => properties.iter().any(|(key, value)| {
            let normalized = key
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            FORBIDDEN
                .iter()
                .any(|forbidden| normalized.contains(forbidden))
                || contains_sensitive_property(value)
        }),
        Value::Array(values) => values.iter().any(contains_sensitive_property),
        _ => false,
    }
}

fn empty_object() -> Value {
    json!({})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn microphone() -> DeviceSnapshot {
        DeviceSnapshot {
            device_id: "audio-input:default".to_string(),
            display_name: "Built-in microphone".to_string(),
            kind: DeviceKind::AudioInput,
            present: true,
            operational: Some(true),
            capabilities: vec![
                "audio.capture".to_string(),
                "audio.capture".to_string(),
                "audio.level.observe".to_string(),
            ],
            properties: json!({ "isDefault": true }),
        }
    }

    #[test]
    fn inventory_becomes_sorted_deduplicated_device_facts() {
        let result = assess_device_inventory(
            &DeviceInventoryObservation {
                sampled_at_ms: 100_000,
                devices: vec![microphone()],
                source_error_codes: Vec::new(),
            },
            100_100,
        )
        .unwrap();

        assert_eq!(result.health, ObservationHealth::Healthy);
        assert_eq!(result.devices[0].health, DeviceHealth::Available);
        assert_eq!(result.devices[0].claims.len(), 3);
        assert_eq!(
            result.devices[0].claims[2].value["capabilityIds"],
            json!(["audio.capture", "audio.level.observe"])
        );
        assert_eq!(result.claims[1].value["available"], 1);
    }

    #[test]
    fn absent_and_unknown_operational_state_do_not_claim_availability() {
        let mut absent = microphone();
        absent.device_id = "camera:front".to_string();
        absent.kind = DeviceKind::Camera;
        absent.present = false;
        absent.operational = None;

        let mut unknown = microphone();
        unknown.device_id = "display:internal".to_string();
        unknown.kind = DeviceKind::Display;
        unknown.operational = None;

        let result = assess_device_inventory(
            &DeviceInventoryObservation {
                sampled_at_ms: 200_000,
                devices: vec![unknown, absent],
                source_error_codes: Vec::new(),
            },
            200_100,
        )
        .unwrap();
        assert_eq!(result.devices[0].device_id, "camera:front");
        assert_eq!(result.devices[0].health, DeviceHealth::Unavailable);
        assert_eq!(result.devices[1].health, DeviceHealth::Degraded);
    }

    #[test]
    fn provider_errors_and_staleness_are_visible_agent_health() {
        let degraded = assess_device_inventory(
            &DeviceInventoryObservation {
                sampled_at_ms: 300_000,
                devices: vec![microphone()],
                source_error_codes: vec!["udev.partial".to_string()],
            },
            300_100,
        )
        .unwrap();
        assert_eq!(degraded.health, ObservationHealth::Degraded);
        assert_eq!(
            degraded.claims[0].value["sourceErrorCodes"],
            json!(["udev.partial"])
        );

        let unavailable = assess_device_inventory(
            &DeviceInventoryObservation {
                sampled_at_ms: 300_000,
                devices: Vec::new(),
                source_error_codes: vec!["provider.unavailable".to_string()],
            },
            300_100,
        )
        .unwrap();
        assert_eq!(unavailable.health, ObservationHealth::Unavailable);
        assert_eq!(unavailable.claims[1].confidence, 0.0);

        let stale = assess_device_inventory(
            &DeviceInventoryObservation {
                sampled_at_ms: 300_000,
                devices: vec![microphone()],
                source_error_codes: Vec::new(),
            },
            316_000,
        )
        .unwrap();
        assert_eq!(stale.health, ObservationHealth::Stale);
        assert_eq!(stale.devices[0].health, DeviceHealth::Degraded);
        assert_eq!(stale.devices[0].claims[0].confidence, 0.4);
    }

    #[test]
    fn malformed_or_duplicate_device_identity_is_rejected() {
        let mut malformed = microphone();
        malformed.device_id = "../../microphone".to_string();
        let bad = DeviceInventoryObservation {
            sampled_at_ms: 400_000,
            devices: vec![malformed],
            source_error_codes: Vec::new(),
        };
        assert!(assess_device_inventory(&bad, 400_100).is_err());

        let duplicate = DeviceInventoryObservation {
            sampled_at_ms: 400_000,
            devices: vec![microphone(), microphone()],
            source_error_codes: Vec::new(),
        };
        assert!(assess_device_inventory(&duplicate, 400_100).is_err());

        let mut contradictory = microphone();
        contradictory.present = false;
        let contradictory = DeviceInventoryObservation {
            sampled_at_ms: 400_000,
            devices: vec![contradictory],
            source_error_codes: Vec::new(),
        };
        assert!(assess_device_inventory(&contradictory, 400_100).is_err());

        let mut secret = microphone();
        secret.properties = json!({ "nested": { "apiKey": "must-not-enter-world-state" } });
        let secret = DeviceInventoryObservation {
            sampled_at_ms: 400_000,
            devices: vec![secret],
            source_error_codes: Vec::new(),
        };
        assert!(assess_device_inventory(&secret, 400_100).is_err());
    }
}
