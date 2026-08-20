use super::model::{
    AgentKind, AgentManifest, AgentState, AppCgroupResourceObservation, DirectiveAction, HostWork,
    HostWorkAction, HostWorkDesiredState, HostWorkObservedState, Interruptibility,
    ResourceObservation, ResourcePressure,
};

pub const RESOURCE_GOVERNOR_POLICY_REVISION: &str = "resource-governor:v1";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResourceGovernorPolicy {
    pub warm_temperature_c: f64,
    pub hot_temperature_c: f64,
    pub critical_temperature_c: f64,
    pub warm_memory_pct: f64,
    pub hot_memory_pct: f64,
    pub critical_memory_pct: f64,
    pub warm_cpu_pct: f64,
    pub hot_priority_floor: u8,
}

impl Default for ResourceGovernorPolicy {
    fn default() -> Self {
        Self {
            warm_temperature_c: 80.0,
            hot_temperature_c: 88.0,
            critical_temperature_c: 95.0,
            warm_memory_pct: 85.0,
            hot_memory_pct: 92.0,
            critical_memory_pct: 97.0,
            warm_cpu_pct: 95.0,
            hot_priority_floor: 90,
        }
    }
}

pub fn classify_pressure(
    observation: &ResourceObservation,
    policy: ResourceGovernorPolicy,
) -> ResourcePressure {
    evaluate_pressure(observation, None, false, policy).pressure
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AppCgroupPressureEvaluation {
    pub active: bool,
    pub baseline_only: bool,
    /// 同一压力等级下仍值得重新治理的一次新事实边沿。持续 above-high、缺失、
    /// stale、counter reset 与实例切换都不会反复推进 epoch。
    pub governance_edge: bool,
    pub memory_events_high_delta: Option<u64>,
    pub memory_events_oom_delta: Option<u64>,
    pub memory_events_oom_kill_delta: Option<u64>,
    /// 这张可信样本自身即可证明 cgroup 正处于 Critical，而不是只沿用 sticky hold。
    pub fresh_critical_evidence: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResourcePressureEvaluation {
    pub pressure: ResourcePressure,
    pub reason_codes: Vec<&'static str>,
    pub app_cgroup: AppCgroupPressureEvaluation,
    /// 当前样本自身的整机 Critical 阈值或可信 cgroup Critical 事实。仅用于一次有界的
    /// Rejected 治理补偿；missing/stale/reset-below-high 与 sticky hold 都不会伪造它。
    pub fresh_critical_evidence: bool,
}

/// 把整机探针和固定 Pinvou app cgroup 的可信绝对计数合并成一次确定性判断。
///
/// cgroup counter 只与最后一条已经落账的同实例观测比较。第一次观测、实例切换或
/// counter 回退只建立 baseline；已有 Critical 则继续保持，直到后续同实例样本明确
/// 证明三个事件计数均无新增且 `memory.current < memory.high`。
pub(super) fn evaluate_pressure(
    observation: &ResourceObservation,
    previous_app_cgroup: Option<&AppCgroupResourceObservation>,
    previous_app_cgroup_critical: bool,
    policy: ResourceGovernorPolicy,
) -> ResourcePressureEvaluation {
    let (system_pressure, mut reason_codes) = classify_system_pressure(observation, policy);
    let (app_cgroup, mut cgroup_reasons) = evaluate_app_cgroup_pressure(
        observation.app_cgroup.as_ref(),
        previous_app_cgroup,
        previous_app_cgroup_critical,
    );
    reason_codes.append(&mut cgroup_reasons);
    let pressure = if app_cgroup.active {
        ResourcePressure::Critical
    } else {
        system_pressure
    };
    let fresh_critical_evidence =
        system_pressure == ResourcePressure::Critical || app_cgroup.fresh_critical_evidence;
    if pressure == ResourcePressure::Normal && reason_codes.is_empty() {
        reason_codes.push("within_policy_limits");
    }
    ResourcePressureEvaluation {
        pressure,
        reason_codes,
        app_cgroup,
        fresh_critical_evidence,
    }
}

fn classify_system_pressure(
    observation: &ResourceObservation,
    policy: ResourceGovernorPolicy,
) -> (ResourcePressure, Vec<&'static str>) {
    let temperature = observation.temperature_c.unwrap_or_default();
    let memory = observation.memory_used_pct.unwrap_or_default();
    let cpu = observation.cpu_usage_pct.unwrap_or_default();

    if temperature >= policy.critical_temperature_c || memory >= policy.critical_memory_pct {
        let mut reasons = Vec::new();
        if temperature >= policy.critical_temperature_c {
            reasons.push("temperature_critical");
        }
        if memory >= policy.critical_memory_pct {
            reasons.push("memory_critical");
        }
        (ResourcePressure::Critical, reasons)
    } else if temperature >= policy.hot_temperature_c || memory >= policy.hot_memory_pct {
        let mut reasons = Vec::new();
        if temperature >= policy.hot_temperature_c {
            reasons.push("temperature_hot");
        }
        if memory >= policy.hot_memory_pct {
            reasons.push("memory_hot");
        }
        (ResourcePressure::Hot, reasons)
    } else if temperature >= policy.warm_temperature_c
        || memory >= policy.warm_memory_pct
        || cpu >= policy.warm_cpu_pct
    {
        let mut reasons = Vec::new();
        if temperature >= policy.warm_temperature_c {
            reasons.push("temperature_warm");
        }
        if memory >= policy.warm_memory_pct {
            reasons.push("memory_warm");
        }
        if cpu >= policy.warm_cpu_pct {
            reasons.push("cpu_saturated");
        }
        (ResourcePressure::Warm, reasons)
    } else {
        (ResourcePressure::Normal, Vec::new())
    }
}

fn evaluate_app_cgroup_pressure(
    current: Option<&AppCgroupResourceObservation>,
    previous: Option<&AppCgroupResourceObservation>,
    was_active: bool,
) -> (AppCgroupPressureEvaluation, Vec<&'static str>) {
    let Some(current) = current else {
        return baseline_evaluation(None, was_active, false, "app_cgroup_telemetry_missing_hold");
    };
    let Some(previous) = previous else {
        return baseline_evaluation(
            Some(current),
            was_active,
            true,
            "app_cgroup_baseline_pending",
        );
    };
    if current.instance_generation != previous.instance_generation
        || current.observed_at_ms <= previous.observed_at_ms
    {
        return baseline_evaluation(
            Some(current),
            was_active,
            false,
            "app_cgroup_instance_baseline_pending",
        );
    }

    let high_delta = monotonic_delta(current.memory_events_high, previous.memory_events_high);
    let oom_delta = monotonic_delta(current.memory_events_oom, previous.memory_events_oom);
    let oom_kill_delta = monotonic_delta(
        current.memory_events_oom_kill,
        previous.memory_events_oom_kill,
    );
    // 任一 cumulative counter 回退都意味着内核/实例侧 baseline 发生变化。该样本
    // 不能既重建 baseline 又宣告恢复，否则已有 Critical 会被一次 reset 伪造解除。
    if counter_regressed(current.memory_events_high, previous.memory_events_high)
        || counter_regressed(current.memory_events_oom, previous.memory_events_oom)
        || counter_regressed(
            current.memory_events_oom_kill,
            previous.memory_events_oom_kill,
        )
    {
        return baseline_evaluation(
            Some(current),
            was_active,
            false,
            "app_cgroup_counter_baseline_reset",
        );
    }

    let current_at_or_above_high = matches!(
        (current.memory_current_bytes, current.memory_high_bytes),
        (Some(memory_current), Some(memory_high)) if memory_high > 0 && memory_current >= memory_high
    );
    let mut reasons = Vec::new();
    if high_delta.is_some_and(|delta| delta > 0) {
        reasons.push("app_cgroup_memory_high_event");
    }
    if oom_delta.is_some_and(|delta| delta > 0) {
        reasons.push("app_cgroup_memory_oom_event");
    }
    if oom_kill_delta.is_some_and(|delta| delta > 0) {
        reasons.push("app_cgroup_memory_oom_kill_event");
    }
    if current_at_or_above_high {
        reasons.push("app_cgroup_memory_current_at_or_above_high");
    }
    let triggered = !reasons.is_empty();
    let previous_below_high = matches!(
        (previous.memory_current_bytes, previous.memory_high_bytes),
        (Some(memory_current), Some(memory_high)) if memory_high > 0 && memory_current < memory_high
    );
    let governance_edge = high_delta.is_some_and(|delta| delta > 0)
        || oom_delta.is_some_and(|delta| delta > 0)
        || oom_kill_delta.is_some_and(|delta| delta > 0)
        || (previous_below_high && current_at_or_above_high);
    let counters_explicitly_unchanged = matches!(high_delta, Some(0))
        && matches!(oom_delta, Some(0))
        && matches!(oom_kill_delta, Some(0));
    let current_explicitly_below_high = matches!(
        (current.memory_current_bytes, current.memory_high_bytes),
        (Some(memory_current), Some(memory_high)) if memory_high > 0 && memory_current < memory_high
    );
    let active = triggered
        || (was_active && !(counters_explicitly_unchanged && current_explicitly_below_high));
    if active && reasons.is_empty() {
        reasons.push("app_cgroup_critical_hold");
    }
    (
        AppCgroupPressureEvaluation {
            active,
            baseline_only: false,
            governance_edge,
            memory_events_high_delta: high_delta,
            memory_events_oom_delta: oom_delta,
            memory_events_oom_kill_delta: oom_kill_delta,
            fresh_critical_evidence: high_delta.is_some_and(|delta| delta > 0)
                || oom_delta.is_some_and(|delta| delta > 0)
                || oom_kill_delta.is_some_and(|delta| delta > 0)
                || current_at_or_above_high,
        },
        reasons,
    )
}

fn baseline_evaluation(
    current: Option<&AppCgroupResourceObservation>,
    was_active: bool,
    first_trusted_baseline: bool,
    hold_reason: &'static str,
) -> (AppCgroupPressureEvaluation, Vec<&'static str>) {
    // cumulative memory.events 需要 baseline；memory.current/high 是同一可信 Status
    // 中的瞬时事实，不应为了等待第二次计数样本而漏掉已经越线的 cgroup。
    let current_at_or_above_high = current.is_some_and(|current| {
        matches!(
            (current.memory_current_bytes, current.memory_high_bytes),
            (Some(memory_current), Some(memory_high))
                if memory_high > 0 && memory_current >= memory_high
        )
    });
    let active = was_active || current_at_or_above_high;
    let reasons = if current_at_or_above_high {
        vec!["app_cgroup_memory_current_at_or_above_high"]
    } else if was_active {
        vec![hold_reason]
    } else {
        Vec::new()
    };
    (
        AppCgroupPressureEvaluation {
            active,
            baseline_only: true,
            governance_edge: first_trusted_baseline && current_at_or_above_high && !was_active,
            memory_events_high_delta: None,
            memory_events_oom_delta: None,
            memory_events_oom_kill_delta: None,
            fresh_critical_evidence: current_at_or_above_high,
        },
        reasons,
    )
}

fn monotonic_delta(current: Option<u64>, previous: Option<u64>) -> Option<u64> {
    current
        .zip(previous)
        .and_then(|(current, previous)| current.checked_sub(previous))
}

fn counter_regressed(current: Option<u64>, previous: Option<u64>) -> bool {
    current
        .zip(previous)
        .is_some_and(|(current, previous)| current < previous)
}

pub(super) fn directive_for_agent(
    agent: &AgentManifest,
    _previous: ResourcePressure,
    current: ResourcePressure,
    policy: ResourceGovernorPolicy,
) -> Option<(DirectiveAction, bool)> {
    if agent.kind != AgentKind::Mission {
        return None;
    }

    match current {
        ResourcePressure::Critical => {
            if matches!(
                agent.desired_state,
                AgentState::Stopped | AgentState::Failed
            ) {
                None
            } else {
                Some((DirectiveAction::Stop, true))
            }
        }
        ResourcePressure::Hot => {
            if agent.priority >= policy.hot_priority_floor
                || agent.interruptibility == Interruptibility::Atomic
                || matches!(
                    agent.desired_state,
                    AgentState::Paused | AgentState::Stopped | AgentState::Failed
                )
            {
                None
            } else {
                Some((DirectiveAction::Pause, false))
            }
        }
        ResourcePressure::Normal => {
            if agent.desired_state == AgentState::Paused {
                Some((DirectiveAction::Resume, false))
            } else {
                None
            }
        }
        ResourcePressure::Warm => None,
    }
}

/// HostWork 与 Mission Agent 是两条独立治理路径。这里只产出确定性候选，不调用
/// Adapter；Runtime 先把候选持久化为 Pending，执行器异步领取。
pub(super) fn directive_for_host_work(
    work: &HostWork,
    current: ResourcePressure,
    policy: ResourceGovernorPolicy,
) -> Option<(HostWorkAction, bool)> {
    if !work.governable {
        return None;
    }

    match current {
        ResourcePressure::Critical => {
            if work.essential
                || !work.supported_actions.contains(&HostWorkAction::Stop)
                || work.desired_state == HostWorkDesiredState::Stopped
            {
                None
            } else {
                Some((HostWorkAction::Stop, true))
            }
        }
        ResourcePressure::Hot => {
            if work.essential
                || work.priority >= policy.hot_priority_floor
                || work.interruptibility == Interruptibility::Atomic
                || !work.supported_actions.contains(&HostWorkAction::Pause)
                || !work.supported_actions.contains(&HostWorkAction::Resume)
                || work.desired_state != HostWorkDesiredState::Running
                || work.observed_state != HostWorkObservedState::Running
            {
                None
            } else {
                Some((HostWorkAction::Pause, false))
            }
        }
        ResourcePressure::Normal => {
            if work.governor_pause_directive_id.is_some()
                && work.supported_actions.contains(&HostWorkAction::Resume)
                && work.desired_state == HostWorkDesiredState::Paused
                && work.observed_state == HostWorkObservedState::Paused
            {
                Some((HostWorkAction::Resume, false))
            } else {
                None
            }
        }
        ResourcePressure::Warm => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(
        temp: Option<f64>,
        memory: Option<f64>,
        cpu: Option<f64>,
    ) -> ResourceObservation {
        ResourceObservation {
            sampled_at_ms: 1,
            cpu_usage_pct: cpu,
            memory_used_pct: memory,
            gpu_usage_pct: None,
            temperature_c: temp,
            power_w: None,
            app_cgroup: None,
        }
    }

    fn app_cgroup(
        observed_at_ms: i64,
        current: u64,
        high: u64,
        events_high: u64,
        events_oom: u64,
        events_oom_kill: u64,
    ) -> AppCgroupResourceObservation {
        AppCgroupResourceObservation {
            observed_at_ms,
            instance_generation: "0123456789abcdef0123456789abcdef".to_string(),
            memory_current_bytes: Some(current),
            memory_high_bytes: Some(high),
            memory_max_bytes: Some(high.saturating_mul(2)),
            memory_events_high: Some(events_high),
            memory_events_oom: Some(events_oom),
            memory_events_oom_kill: Some(events_oom_kill),
            memory_pressure_full_avg10: Some(0.25),
        }
    }

    #[test]
    fn pressure_uses_deterministic_hard_thresholds() {
        let policy = ResourceGovernorPolicy::default();
        assert_eq!(
            classify_pressure(&observation(Some(79.0), Some(70.0), Some(20.0)), policy),
            ResourcePressure::Normal
        );
        assert_eq!(
            classify_pressure(&observation(Some(80.0), None, None), policy),
            ResourcePressure::Warm
        );
        assert_eq!(
            classify_pressure(&observation(Some(88.0), None, None), policy),
            ResourcePressure::Hot
        );
        assert_eq!(
            classify_pressure(&observation(None, Some(97.0), None), policy),
            ResourcePressure::Critical
        );
    }

    #[test]
    fn first_cgroup_counter_sample_only_builds_baseline_below_high() {
        let mut sample = observation(None, Some(20.0), Some(10.0));
        sample.app_cgroup = Some(app_cgroup(1, 3_000, 4_000, 8, 2, 1));

        let result = evaluate_pressure(&sample, None, false, ResourceGovernorPolicy::default());

        assert_eq!(result.pressure, ResourcePressure::Normal);
        assert!(result.app_cgroup.baseline_only);
        assert!(!result.app_cgroup.active);
        assert_eq!(result.app_cgroup.memory_events_high_delta, None);
    }

    #[test]
    fn trusted_current_at_high_is_immediately_critical_even_on_baseline() {
        let mut sample = observation(None, Some(20.0), Some(10.0));
        sample.app_cgroup = Some(app_cgroup(1, 4_000, 4_000, 8, 2, 1));

        let result = evaluate_pressure(&sample, None, false, ResourceGovernorPolicy::default());

        assert_eq!(result.pressure, ResourcePressure::Critical);
        assert!(result.app_cgroup.active);
        assert!(result
            .reason_codes
            .contains(&"app_cgroup_memory_current_at_or_above_high"));
    }

    #[test]
    fn cumulative_high_and_oom_kill_deltas_trigger_critical_at_low_system_usage() {
        let baseline = app_cgroup(1, 2_000, 4_000, 8, 2, 1);
        let mut sample = observation(None, Some(20.0), Some(10.0));
        sample.app_cgroup = Some(app_cgroup(2, 2_100, 4_000, 9, 2, 2));

        let result = evaluate_pressure(
            &sample,
            Some(&baseline),
            false,
            ResourceGovernorPolicy::default(),
        );

        assert_eq!(result.pressure, ResourcePressure::Critical);
        assert_eq!(result.app_cgroup.memory_events_high_delta, Some(1));
        assert_eq!(result.app_cgroup.memory_events_oom_kill_delta, Some(1));
        assert!(result
            .reason_codes
            .contains(&"app_cgroup_memory_high_event"));
        assert!(result
            .reason_codes
            .contains(&"app_cgroup_memory_oom_kill_event"));

        let mut oom_only = observation(None, Some(20.0), Some(10.0));
        oom_only.app_cgroup = Some(app_cgroup(2, 2_100, 4_000, 8, 3, 1));
        let oom_result = evaluate_pressure(
            &oom_only,
            Some(&baseline),
            false,
            ResourceGovernorPolicy::default(),
        );
        assert_eq!(oom_result.pressure, ResourcePressure::Critical);
        assert_eq!(oom_result.app_cgroup.memory_events_oom_delta, Some(1));
        assert!(oom_result
            .reason_codes
            .contains(&"app_cgroup_memory_oom_event"));
    }

    #[test]
    fn cgroup_critical_only_relaxes_on_new_same_instance_explicit_relief() {
        let baseline = app_cgroup(1, 2_000, 4_000, 9, 2, 2);
        let missing = observation(None, Some(20.0), Some(10.0));
        assert!(
            evaluate_pressure(
                &missing,
                Some(&baseline),
                true,
                ResourceGovernorPolicy::default(),
            )
            .app_cgroup
            .active
        );

        let mut new_instance = observation(None, Some(20.0), Some(10.0));
        let mut reset = app_cgroup(2, 1_000, 4_000, 0, 0, 0);
        reset.instance_generation = "1123456789abcdef0123456789abcdef".to_string();
        new_instance.app_cgroup = Some(reset);
        assert!(
            evaluate_pressure(
                &new_instance,
                Some(&baseline),
                true,
                ResourceGovernorPolicy::default(),
            )
            .app_cgroup
            .active
        );

        let mut relief = observation(None, Some(20.0), Some(10.0));
        relief.app_cgroup = Some(app_cgroup(2, 1_500, 4_000, 9, 2, 2));
        let relieved = evaluate_pressure(
            &relief,
            Some(&baseline),
            true,
            ResourceGovernorPolicy::default(),
        );
        assert_eq!(relieved.pressure, ResourcePressure::Normal);
        assert!(!relieved.app_cgroup.active);
        assert_eq!(relieved.app_cgroup.memory_events_high_delta, Some(0));
    }
}
