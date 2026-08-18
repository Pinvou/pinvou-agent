use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    classify_pressure, PinvouOsRuntime, ResourceGovernorPolicy, ResourceObservation,
    ResourcePressure, RESOURCE_AGENT_ID,
};

mod device_agent;

#[allow(unused_imports)]
pub use device_agent::{
    assess_device_inventory, DeviceAgentAssessment, DeviceHealth, DeviceInventoryObservation,
    DeviceKind, DeviceSnapshot, DEVICE_AGENT_ID,
};

pub type ResourceSampler = Arc<dyn Fn() -> ResourceObservation + Send + Sync + 'static>;

/// 所有观测型原子 Agent 共享的最小健康语义。
///
/// 它描述的是 Agent 此次观测链路是否可信，不是被观测对象本身是否健康。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObservationHealth {
    Healthy,
    Degraded,
    Stale,
    Unavailable,
}

/// 尚未写入事件账本的标准化 Claim。
///
/// Runtime 在接受它时负责分配 claim/event id，并把 `observed_at_ms` 对应到证据事件；
/// 原子 Agent 不自行伪造账本身份。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClaimCandidate {
    pub subject: String,
    pub predicate: String,
    pub value: Value,
    pub confidence: f32,
    pub observed_at_ms: i64,
    pub asserted_by_actor_id: String,
}

/// Resource Agent 只提出资源治理意图，最终目标选择和动作签发仍由 Governor 完成。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceMitigationAction {
    DeferHeavyWork,
    PauseInterruptibleWork,
    StopNonEssentialWork,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDirectiveCandidate {
    pub action: ResourceMitigationAction,
    pub reason_codes: Vec<String>,
    pub hard: bool,
}

/// `resource.observe` 一次原子调用的完整输出。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceAgentAssessment {
    pub assessed_at_ms: i64,
    pub health: ObservationHealth,
    pub pressure: ResourcePressure,
    pub claims: Vec<ClaimCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub governor_candidate: Option<ResourceDirectiveCandidate>,
}

const DEFAULT_MAX_SAMPLE_AGE_MS: i64 = 15_000;
const MAX_FUTURE_SKEW_MS: i64 = 5_000;

/// 把一次资源采样变成确定性的事实与治理候选，不调用模型、不执行控制动作。
pub fn assess_resource_observation(
    observation: &ResourceObservation,
    assessed_at_ms: i64,
    policy: ResourceGovernorPolicy,
) -> Result<ResourceAgentAssessment> {
    validate_observation(observation, assessed_at_ms)?;

    let health = observation_health(observation, assessed_at_ms, DEFAULT_MAX_SAMPLE_AGE_MS);
    let pressure = classify_pressure(observation, policy);
    let reason_codes = pressure_reason_codes(observation, policy, pressure);
    let confidence = match health {
        ObservationHealth::Healthy => 1.0,
        ObservationHealth::Degraded => 0.75,
        ObservationHealth::Stale => 0.4,
        ObservationHealth::Unavailable => 0.0,
    };
    let claims = vec![
        ClaimCandidate {
            subject: "agent:resource".to_string(),
            predicate: "observation_health".to_string(),
            value: json!({ "status": health }),
            confidence: 1.0,
            observed_at_ms: observation.sampled_at_ms,
            asserted_by_actor_id: RESOURCE_AGENT_ID.to_string(),
        },
        ClaimCandidate {
            subject: "device.resources".to_string(),
            predicate: "pressure_level".to_string(),
            value: json!({
                "level": pressure,
                "reasonCodes": reason_codes,
                "cpuUsagePct": observation.cpu_usage_pct,
                "memoryUsedPct": observation.memory_used_pct,
                "gpuUsagePct": observation.gpu_usage_pct,
                "temperatureC": observation.temperature_c,
                "powerW": observation.power_w,
            }),
            confidence,
            observed_at_ms: observation.sampled_at_ms,
            asserted_by_actor_id: RESOURCE_AGENT_ID.to_string(),
        },
    ];

    let governor_candidate = match pressure {
        ResourcePressure::Normal => None,
        ResourcePressure::Warm => Some(ResourceDirectiveCandidate {
            action: ResourceMitigationAction::DeferHeavyWork,
            reason_codes,
            hard: false,
        }),
        ResourcePressure::Hot => Some(ResourceDirectiveCandidate {
            action: ResourceMitigationAction::PauseInterruptibleWork,
            reason_codes,
            hard: false,
        }),
        ResourcePressure::Critical => Some(ResourceDirectiveCandidate {
            action: ResourceMitigationAction::StopNonEssentialWork,
            reason_codes,
            hard: true,
        }),
    };

    Ok(ResourceAgentAssessment {
        assessed_at_ms,
        health,
        pressure,
        claims,
        governor_candidate,
    })
}

fn observation_health(
    observation: &ResourceObservation,
    assessed_at_ms: i64,
    max_sample_age_ms: i64,
) -> ObservationHealth {
    if assessed_at_ms.saturating_sub(observation.sampled_at_ms) > max_sample_age_ms {
        return ObservationHealth::Stale;
    }
    let available = [
        observation.cpu_usage_pct,
        observation.memory_used_pct,
        observation.gpu_usage_pct,
        observation.temperature_c,
        observation.power_w,
    ]
    .into_iter()
    .flatten()
    .count();
    if available == 0 {
        ObservationHealth::Unavailable
    } else if observation.memory_used_pct.is_some() && observation.temperature_c.is_some() {
        // 内存与温度覆盖了当前 Governor 的两条 critical 路径；GPU/功耗是可选遥测。
        ObservationHealth::Healthy
    } else {
        ObservationHealth::Degraded
    }
}

fn pressure_reason_codes(
    observation: &ResourceObservation,
    policy: ResourceGovernorPolicy,
    pressure: ResourcePressure,
) -> Vec<String> {
    let mut reasons = Vec::new();
    match pressure {
        ResourcePressure::Critical => {
            push_if_at_least(
                &mut reasons,
                observation.temperature_c,
                policy.critical_temperature_c,
                "temperature_critical",
            );
            push_if_at_least(
                &mut reasons,
                observation.memory_used_pct,
                policy.critical_memory_pct,
                "memory_critical",
            );
        }
        ResourcePressure::Hot => {
            push_if_at_least(
                &mut reasons,
                observation.temperature_c,
                policy.hot_temperature_c,
                "temperature_hot",
            );
            push_if_at_least(
                &mut reasons,
                observation.memory_used_pct,
                policy.hot_memory_pct,
                "memory_hot",
            );
        }
        ResourcePressure::Warm => {
            push_if_at_least(
                &mut reasons,
                observation.temperature_c,
                policy.warm_temperature_c,
                "temperature_warm",
            );
            push_if_at_least(
                &mut reasons,
                observation.memory_used_pct,
                policy.warm_memory_pct,
                "memory_warm",
            );
            push_if_at_least(
                &mut reasons,
                observation.cpu_usage_pct,
                policy.warm_cpu_pct,
                "cpu_saturated",
            );
        }
        ResourcePressure::Normal => reasons.push("within_policy_limits".to_string()),
    }
    reasons
}

fn push_if_at_least(reasons: &mut Vec<String>, value: Option<f64>, threshold: f64, reason: &str) {
    if value.is_some_and(|value| value >= threshold) {
        reasons.push(reason.to_string());
    }
}

fn validate_observation(observation: &ResourceObservation, assessed_at_ms: i64) -> Result<()> {
    if observation.sampled_at_ms <= 0 {
        bail!("resource sample timestamp must be positive");
    }
    if observation.sampled_at_ms > assessed_at_ms.saturating_add(MAX_FUTURE_SKEW_MS) {
        bail!("resource sample timestamp is too far in the future");
    }
    for (label, value) in [
        ("cpu_usage_pct", observation.cpu_usage_pct),
        ("memory_used_pct", observation.memory_used_pct),
        ("gpu_usage_pct", observation.gpu_usage_pct),
    ] {
        if value.is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value)) {
            bail!("{label} must be between 0 and 100");
        }
    }
    if observation
        .temperature_c
        .is_some_and(|value| !value.is_finite() || !(-50.0..=150.0).contains(&value))
    {
        bail!("temperature_c is outside the supported range");
    }
    if observation
        .power_w
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        bail!("power_w must be non-negative");
    }
    Ok(())
}

/// 启动常驻 Resource Agent。
///
/// 采样持续发生；为避免稳定状态每 5 秒永久膨胀账本，只在压力等级变化时立即写入，
/// 或至少每 30 秒写一条心跳证据。该节流不经过模型，也不改变 Governor 的硬阈值。
pub fn spawn_resource_agent(
    runtime: PinvouOsRuntime,
    sampler: ResourceSampler,
    cadence: Duration,
) -> tauri::async_runtime::JoinHandle<()> {
    // Tauri 的 `setup` 回调运行在桌面事件循环线程，不保证当前线程已经进入
    // Tokio reactor。必须通过 Tauri 持有的全局 async runtime 启动常驻 Agent；
    // 直接 `tokio::spawn` 会让冷启动在窗口创建后立即 panic。
    tauri::async_runtime::spawn(async move {
        let cadence = cadence.max(Duration::from_secs(1));
        let mut ticker = tokio::time::interval(cadence);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_persisted = None::<Instant>;
        loop {
            ticker.tick().await;
            let sampler = sampler.clone();
            let observation = match tokio::task::spawn_blocking(move || sampler()).await {
                Ok(observation) => observation,
                Err(error) => {
                    log::warn!("PinvouOS Resource Agent sampler failed: {error}");
                    continue;
                }
            };
            let assessment = match assess_resource_observation(
                &observation,
                chrono::Utc::now().timestamp_millis(),
                ResourceGovernorPolicy::default(),
            ) {
                Ok(assessment) => assessment,
                Err(error) => {
                    log::warn!("PinvouOS Resource Agent rejected invalid sample: {error:#}");
                    continue;
                }
            };
            if matches!(
                assessment.health,
                ObservationHealth::Stale | ObservationHealth::Unavailable
            ) {
                log::warn!(
                    "PinvouOS Resource Agent observation health is {:?}",
                    assessment.health
                );
            }
            let pressure = assessment.pressure;
            let pressure_changed = pressure != runtime.snapshot().resources.pressure;
            let heartbeat_due =
                last_persisted.is_none_or(|instant| instant.elapsed() >= Duration::from_secs(30));
            if pressure_changed || heartbeat_due {
                match runtime.observe_resources(observation) {
                    Ok(_) => last_persisted = Some(Instant::now()),
                    Err(error) => {
                        log::warn!("PinvouOS Resource Agent observation failed: {error:#}")
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(sampled_at_ms: i64) -> ResourceObservation {
        ResourceObservation {
            sampled_at_ms,
            cpu_usage_pct: Some(25.0),
            memory_used_pct: Some(50.0),
            gpu_usage_pct: Some(15.0),
            temperature_c: Some(55.0),
            power_w: Some(18.0),
        }
    }

    #[test]
    fn healthy_sample_emits_normal_claim_without_control_candidate() {
        let result = assess_resource_observation(
            &observation(100_000),
            100_100,
            ResourceGovernorPolicy::default(),
        )
        .unwrap();

        assert_eq!(result.health, ObservationHealth::Healthy);
        assert_eq!(result.pressure, ResourcePressure::Normal);
        assert!(result.governor_candidate.is_none());
        assert_eq!(result.claims.len(), 2);
        assert_eq!(result.claims[1].subject, "device.resources");
        assert_eq!(result.claims[1].predicate, "pressure_level");
        assert_eq!(result.claims[1].value["level"], "normal");
        assert_eq!(
            result.claims[1].value["reasonCodes"],
            json!(["within_policy_limits"])
        );
    }

    #[test]
    fn heat_and_memory_pressure_are_explicit_governor_inputs() {
        let mut sample = observation(200_000);
        sample.temperature_c = Some(90.0);
        sample.memory_used_pct = Some(94.0);
        let hot = assess_resource_observation(&sample, 200_100, ResourceGovernorPolicy::default())
            .unwrap();
        let candidate = hot.governor_candidate.unwrap();
        assert_eq!(hot.pressure, ResourcePressure::Hot);
        assert_eq!(
            candidate.action,
            ResourceMitigationAction::PauseInterruptibleWork
        );
        assert!(!candidate.hard);
        assert_eq!(
            candidate.reason_codes,
            vec!["temperature_hot", "memory_hot"]
        );

        sample.memory_used_pct = Some(98.0);
        let critical =
            assess_resource_observation(&sample, 200_200, ResourceGovernorPolicy::default())
                .unwrap();
        let candidate = critical.governor_candidate.unwrap();
        assert_eq!(critical.pressure, ResourcePressure::Critical);
        assert_eq!(
            candidate.action,
            ResourceMitigationAction::StopNonEssentialWork
        );
        assert!(candidate.hard);
        assert!(candidate
            .reason_codes
            .contains(&"memory_critical".to_string()));
    }

    #[test]
    fn missing_or_stale_telemetry_downgrades_observation_health() {
        let missing = ResourceObservation {
            sampled_at_ms: 300_000,
            cpu_usage_pct: None,
            memory_used_pct: None,
            gpu_usage_pct: None,
            temperature_c: None,
            power_w: None,
        };
        let unavailable =
            assess_resource_observation(&missing, 300_100, ResourceGovernorPolicy::default())
                .unwrap();
        assert_eq!(unavailable.health, ObservationHealth::Unavailable);
        assert_eq!(unavailable.claims[1].confidence, 0.0);

        let stale = assess_resource_observation(
            &observation(300_000),
            316_000,
            ResourceGovernorPolicy::default(),
        )
        .unwrap();
        assert_eq!(stale.health, ObservationHealth::Stale);
        assert_eq!(stale.claims[1].confidence, 0.4);
    }

    #[test]
    fn invalid_metrics_are_rejected_before_claims_are_formed() {
        let mut invalid = observation(400_000);
        invalid.memory_used_pct = Some(101.0);
        assert!(
            assess_resource_observation(&invalid, 400_100, ResourceGovernorPolicy::default())
                .is_err()
        );
    }
}
