use super::model::{
    AgentKind, AgentManifest, AgentState, DirectiveAction, Interruptibility, ResourceObservation,
    ResourcePressure,
};

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
    let temperature = observation.temperature_c.unwrap_or_default();
    let memory = observation.memory_used_pct.unwrap_or_default();
    let cpu = observation.cpu_usage_pct.unwrap_or_default();

    if temperature >= policy.critical_temperature_c || memory >= policy.critical_memory_pct {
        ResourcePressure::Critical
    } else if temperature >= policy.hot_temperature_c || memory >= policy.hot_memory_pct {
        ResourcePressure::Hot
    } else if temperature >= policy.warm_temperature_c
        || memory >= policy.warm_memory_pct
        || cpu >= policy.warm_cpu_pct
    {
        ResourcePressure::Warm
    } else {
        ResourcePressure::Normal
    }
}

pub(super) fn directive_for_agent(
    agent: &AgentManifest,
    previous: ResourcePressure,
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
        ResourcePressure::Normal if previous >= ResourcePressure::Hot => {
            if agent.desired_state == AgentState::Paused {
                Some((DirectiveAction::Resume, false))
            } else {
                None
            }
        }
        ResourcePressure::Normal | ResourcePressure::Warm => None,
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
}
