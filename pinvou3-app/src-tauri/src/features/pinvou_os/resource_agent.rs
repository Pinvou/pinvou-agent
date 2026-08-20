use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::governor::{
    directive_for_host_work, evaluate_pressure, ResourcePressureEvaluation,
    RESOURCE_GOVERNOR_POLICY_REVISION,
};
use super::{
    HostWorkDirectiveStatus, PinvouOsRuntime, ResourceGovernorPolicy, ResourceObservation,
    ResourcePressure, ResourceState, RuntimeSnapshot, RESOURCE_AGENT_ID,
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
    assess_resource_observation_with_state(
        observation,
        assessed_at_ms,
        &ResourceState::default(),
        policy,
    )
}

fn assess_resource_observation_with_state(
    observation: &ResourceObservation,
    assessed_at_ms: i64,
    previous: &ResourceState,
    policy: ResourceGovernorPolicy,
) -> Result<ResourceAgentAssessment> {
    validate_observation(observation, assessed_at_ms)?;

    let health = observation_health(observation, assessed_at_ms, DEFAULT_MAX_SAMPLE_AGE_MS);
    let evaluation = evaluate_pressure(
        observation,
        previous.last_app_cgroup_observation.as_ref(),
        previous.app_cgroup_critical,
        policy,
    );
    let pressure = evaluation.pressure;
    let reason_codes = evaluation
        .reason_codes
        .iter()
        .map(|reason| (*reason).to_string())
        .collect::<Vec<_>>();
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
                "appCgroup": app_cgroup_claim_value(observation, &evaluation),
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

pub(super) fn app_cgroup_claim_value(
    observation: &ResourceObservation,
    evaluation: &ResourcePressureEvaluation,
) -> Value {
    let Some(cgroup) = observation.app_cgroup.as_ref() else {
        return Value::Null;
    };
    json!({
        "observedAtMs": cgroup.observed_at_ms,
        "instanceGeneration": cgroup.instance_generation,
        "memoryCurrentBytes": cgroup.memory_current_bytes,
        "memoryHighBytes": cgroup.memory_high_bytes,
        "memoryMaxBytes": cgroup.memory_max_bytes,
        "memoryEvents": {
            "high": cgroup.memory_events_high,
            "oom": cgroup.memory_events_oom,
            "oomKill": cgroup.memory_events_oom_kill,
        },
        "memoryEventDeltas": {
            "high": evaluation.app_cgroup.memory_events_high_delta,
            "oom": evaluation.app_cgroup.memory_events_oom_delta,
            "oomKill": evaluation.app_cgroup.memory_events_oom_kill_delta,
        },
        "memoryPressureFullAvg10": cgroup.memory_pressure_full_avg10,
        "baselineOnly": evaluation.app_cgroup.baseline_only,
        "governanceEdge": evaluation.app_cgroup.governance_edge,
        "critical": evaluation.app_cgroup.active,
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
    if available == 0 && observation.app_cgroup.is_none() {
        ObservationHealth::Unavailable
    } else if observation.memory_used_pct.is_some() && observation.temperature_c.is_some() {
        // 内存与温度覆盖了当前 Governor 的两条 critical 路径；GPU/功耗是可选遥测。
        ObservationHealth::Healthy
    } else {
        ObservationHealth::Degraded
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
    if let Some(cgroup) = observation.app_cgroup.as_ref() {
        validate_app_cgroup_observation(cgroup, observation.sampled_at_ms)?;
    }
    Ok(())
}

pub(super) fn validate_app_cgroup_observation(
    cgroup: &super::AppCgroupResourceObservation,
    sampled_at_ms: i64,
) -> Result<()> {
    if cgroup.observed_at_ms <= 0
        || cgroup.observed_at_ms > sampled_at_ms.saturating_add(MAX_FUTURE_SKEW_MS)
        || sampled_at_ms.saturating_sub(cgroup.observed_at_ms) > DEFAULT_MAX_SAMPLE_AGE_MS
    {
        bail!("app cgroup observation timestamp is stale or invalid");
    }
    if cgroup.instance_generation.len() != 32
        || cgroup.instance_generation == "00000000000000000000000000000000"
        || !cgroup
            .instance_generation
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    {
        bail!("app cgroup instance generation is invalid");
    }
    if cgroup
        .memory_pressure_full_avg10
        .is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value))
    {
        bail!("app cgroup memory pressure avg10 must be between 0 and 100");
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
            let before = runtime.snapshot();
            let assessment = match assess_resource_observation_with_state(
                &observation,
                chrono::Utc::now().timestamp_millis(),
                &before.resources,
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
            let pressure_changed = pressure != before.resources.pressure;
            let cgroup_governance_due = app_cgroup_governance_due(&observation, &before.resources);
            let evaluation = evaluate_pressure(
                &observation,
                before.resources.last_app_cgroup_observation.as_ref(),
                before.resources.app_cgroup_critical,
                ResourceGovernorPolicy::default(),
            );
            let advancing_sample = before
                .resources
                .last_observation
                .as_ref()
                .is_none_or(|previous| observation.sampled_at_ms > previous.sampled_at_ms);
            let retry_evidence_due = !matches!(
                assessment.health,
                ObservationHealth::Stale | ObservationHealth::Unavailable
            ) && advancing_sample
                && fresh_governor_retry_due(&before, pressure, evaluation.fresh_critical_evidence);
            let heartbeat_due =
                last_persisted.is_none_or(|instant| instant.elapsed() >= Duration::from_secs(30));
            if pressure_changed || cgroup_governance_due || retry_evidence_due || heartbeat_due {
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

fn fresh_governor_retry_due(
    snapshot: &RuntimeSnapshot,
    pressure: ResourcePressure,
    fresh_critical_evidence: bool,
) -> bool {
    if !fresh_critical_evidence {
        return false;
    }
    snapshot.host_works.values().any(|work| {
        let Some((action, _hard)) =
            directive_for_host_work(work, pressure, ResourceGovernorPolicy::default())
        else {
            return false;
        };
        let unresolved = snapshot.host_work_directives.values().any(|directive| {
            directive.work_id == work.work_id
                && directive.generation == work.generation
                && matches!(
                    directive.status,
                    HostWorkDirectiveStatus::Pending
                        | HostWorkDirectiveStatus::AwaitingReconciliation
                        | HostWorkDirectiveStatus::OutcomeUnknown
                )
        });
        if unresolved {
            return false;
        }
        snapshot
            .host_work_directives
            .values()
            .filter(|directive| {
                directive.work_id == work.work_id
                    && directive.generation == work.generation
                    && directive.action == action
                    && directive.resource_pressure_epoch == snapshot.resources.pressure_epoch
                    && directive.policy_revision == RESOURCE_GOVERNOR_POLICY_REVISION
                    && directive.status == HostWorkDirectiveStatus::Rejected
            })
            .count()
            == 1
    })
}

fn app_cgroup_governance_due(observation: &ResourceObservation, previous: &ResourceState) -> bool {
    let Some(current) = observation.app_cgroup.as_ref() else {
        return false;
    };
    let Some(baseline) = previous.last_app_cgroup_observation.as_ref() else {
        return true;
    };
    if current.observed_at_ms <= baseline.observed_at_ms {
        return false;
    }
    if current.instance_generation != baseline.instance_generation {
        return true;
    }
    let current_at_or_above_high = matches!(
        (current.memory_current_bytes, current.memory_high_bytes),
        (Some(memory_current), Some(memory_high)) if memory_high > 0 && memory_current >= memory_high
    );
    let current_explicitly_below_high = matches!(
        (current.memory_current_bytes, current.memory_high_bytes),
        (Some(memory_current), Some(memory_high)) if memory_high > 0 && memory_current < memory_high
    );
    let policy_or_counter_changed = current.memory_high_bytes != baseline.memory_high_bytes
        || current.memory_max_bytes != baseline.memory_max_bytes
        || current.memory_events_high != baseline.memory_events_high
        || current.memory_events_oom != baseline.memory_events_oom
        || current.memory_events_oom_kill != baseline.memory_events_oom_kill;
    policy_or_counter_changed
        || if previous.app_cgroup_critical {
            current_explicitly_below_high
        } else {
            current_at_or_above_high
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::pinvou_os::{
        HostWork, HostWorkAction, HostWorkDesiredState, HostWorkDirective, HostWorkDirectiveStatus,
        HostWorkKind, HostWorkObservedState, Interruptibility, ResourceClass,
    };

    fn observation(sampled_at_ms: i64) -> ResourceObservation {
        ResourceObservation {
            sampled_at_ms,
            cpu_usage_pct: Some(25.0),
            memory_used_pct: Some(50.0),
            gpu_usage_pct: Some(15.0),
            temperature_c: Some(55.0),
            power_w: Some(18.0),
            app_cgroup: None,
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
            app_cgroup: None,
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

    #[test]
    fn fresh_critical_evidence_for_one_rejected_action_forces_cadence_persistence() {
        let mut snapshot = RuntimeSnapshot::default();
        snapshot.resources.pressure = ResourcePressure::Critical;
        snapshot.resources.pressure_epoch = 7;
        let work = HostWork {
            work_id: "host-work-resource-retry".to_string(),
            generation: 3,
            owner: "host:test-resource-retry".to_string(),
            kind: HostWorkKind::ScheduledRun,
            resource_class: ResourceClass::Heavy,
            priority: 20,
            interruptibility: Interruptibility::Immediate,
            essential: false,
            governable: true,
            supported_actions: std::collections::BTreeSet::from([HostWorkAction::Stop]),
            desired_state: HostWorkDesiredState::Running,
            observed_state: HostWorkObservedState::Running,
            registered_at_ms: 1,
            last_observed_at_ms: 1,
            governor_pause_directive_id: None,
        };
        snapshot
            .host_works
            .insert(work.work_id.clone(), work.clone());
        let rejected = HostWorkDirective {
            directive_id: "host-directive-resource-retry-1".to_string(),
            work_id: work.work_id.clone(),
            generation: work.generation,
            action: HostWorkAction::Stop,
            reason: "critical resource pressure".to_string(),
            policy_revision: RESOURCE_GOVERNOR_POLICY_REVISION.to_string(),
            resource_pressure_epoch: 7,
            issued_event_sequence: Some(10),
            issued_at_ms: 10,
            status: HostWorkDirectiveStatus::Rejected,
            acknowledgement: None,
            acknowledged_at_ms: None,
            acknowledgement_detail: None,
            reconciliation: None,
            reconciled_observed_state: None,
            reconciled_at_ms: None,
            reconciliation_detail: None,
        };
        snapshot
            .host_work_directives
            .insert(rejected.directive_id.clone(), rejected.clone());

        assert!(!fresh_governor_retry_due(
            &snapshot,
            ResourcePressure::Critical,
            false
        ));
        assert!(fresh_governor_retry_due(
            &snapshot,
            ResourcePressure::Critical,
            true
        ));

        let mut second_rejected = rejected;
        second_rejected.directive_id = "host-directive-resource-retry-2".to_string();
        snapshot
            .host_work_directives
            .insert(second_rejected.directive_id.clone(), second_rejected);
        assert!(!fresh_governor_retry_due(
            &snapshot,
            ResourcePressure::Critical,
            true
        ));
    }
}
