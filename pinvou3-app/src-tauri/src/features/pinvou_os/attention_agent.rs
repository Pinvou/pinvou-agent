use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::model::{CapabilityContract, Interruptibility, ResourceClass, ResourcePressure};

pub const ATTENTION_AGENT_ID: &str = "agent:attention";
pub const ATTENTION_ALLOCATE_CAPABILITY_ID: &str = "attention.allocate";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttentionAgentState {
    Idle,
    Allocating,
    Constrained,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttentionDisposition {
    Run,
    Throttle,
    Pause,
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttentionGoal {
    pub mission_id: String,
    pub run_id: String,
    pub priority: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_at_ms: Option<i64>,
    pub user_visible: bool,
    pub user_blocking: bool,
    pub currently_running: bool,
    pub resource_class: ResourceClass,
    pub interruptibility: Interruptibility,
    pub estimated_remaining_ms: u64,
    pub minimum_slice_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttentionAllocationInput {
    pub now_ms: i64,
    pub resource_pressure: ResourcePressure,
    pub max_concurrent: usize,
    pub total_work_budget_ms: u64,
    #[serde(default)]
    pub goals: Vec<AttentionGoal>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalAttentionAllocation {
    pub mission_id: String,
    pub run_id: String,
    pub rank: u32,
    pub score: i64,
    pub disposition: AttentionDisposition,
    /// 0..=10000 的 basis points，避免浮点误差进入调度协议。
    pub attention_share_bps: u16,
    pub work_budget_ms: u64,
    /// Agent 收到中断请求后，允许到达安全边界所需的最大时间预算。
    pub interrupt_budget_ms: u64,
    pub reason_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttentionPlan {
    pub agent_id: String,
    pub state: AttentionAgentState,
    pub epoch: u64,
    pub resource_pressure: ResourcePressure,
    pub effective_concurrency: usize,
    pub allocations: Vec<GoalAttentionAllocation>,
    pub preempted_run_ids: Vec<String>,
    pub limitation_codes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionAllocationError {
    message: String,
}

impl AttentionAllocationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for AttentionAllocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for AttentionAllocationError {}

#[derive(Debug, Clone)]
pub struct AttentionAgent {
    state: AttentionAgentState,
    epoch: u64,
    last_plan: Option<AttentionPlan>,
}

impl Default for AttentionAgent {
    fn default() -> Self {
        Self {
            state: AttentionAgentState::Idle,
            epoch: 0,
            last_plan: None,
        }
    }
}

impl AttentionAgent {
    pub fn state(&self) -> AttentionAgentState {
        self.state
    }

    pub fn last_plan(&self) -> Option<&AttentionPlan> {
        self.last_plan.as_ref()
    }

    /// 对并发 Run 做确定性排序和预算切片。热/临界压力会直接减少并发与重任务，
    /// 不等待 LLM 自行决定降载；Atomic 段只允许节流到安全检查点。
    pub fn allocate(
        &mut self,
        input: AttentionAllocationInput,
    ) -> Result<AttentionPlan, AttentionAllocationError> {
        if let Err(error) = validate_request(&input) {
            self.state = AttentionAgentState::Failed;
            return Err(error);
        }

        self.epoch = self.epoch.saturating_add(1);
        if input.goals.is_empty() {
            self.state = AttentionAgentState::Idle;
            let plan = AttentionPlan {
                agent_id: ATTENTION_AGENT_ID.to_string(),
                state: self.state,
                epoch: self.epoch,
                resource_pressure: input.resource_pressure,
                effective_concurrency: 0,
                allocations: Vec::new(),
                preempted_run_ids: Vec::new(),
                limitation_codes: Vec::new(),
            };
            self.last_plan = Some(plan.clone());
            return Ok(plan);
        }

        let mut ranked = input
            .goals
            .iter()
            .map(|goal| (goal, score_goal(goal, input.now_ms)))
            .collect::<Vec<_>>();
        ranked.sort_by(|(left_goal, left_score), (right_goal, right_score)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_goal.run_id.cmp(&right_goal.run_id))
        });

        let pressure_capacity = match input.resource_pressure {
            ResourcePressure::Normal => input.max_concurrent,
            ResourcePressure::Warm => input.max_concurrent.saturating_mul(3).div_ceil(4).max(1),
            ResourcePressure::Hot => 1,
            ResourcePressure::Critical => 0,
        };
        let forced_atomic = ranked
            .iter()
            .filter(|(goal, _)| {
                goal.currently_running && goal.interruptibility == Interruptibility::Atomic
            })
            .map(|(goal, _)| goal.run_id.as_str())
            .collect::<BTreeSet<_>>();

        let mut selected = forced_atomic.clone();
        if input.resource_pressure != ResourcePressure::Critical {
            for (goal, _) in &ranked {
                if selected.len() >= pressure_capacity.max(forced_atomic.len()) {
                    break;
                }
                if input.resource_pressure == ResourcePressure::Hot
                    && goal.resource_class == ResourceClass::Heavy
                    && !forced_atomic.contains(goal.run_id.as_str())
                {
                    continue;
                }
                selected.insert(goal.run_id.as_str());
            }
        }

        let selected_weights = ranked
            .iter()
            .filter(|(goal, _)| selected.contains(goal.run_id.as_str()))
            .map(|(goal, score)| (goal.run_id.as_str(), (*score).max(1) as u64))
            .collect::<Vec<_>>();
        let total_weight = selected_weights
            .iter()
            .map(|(_, weight)| *weight)
            .sum::<u64>();
        let selected_count = selected_weights.len();
        let mut share_by_run = BTreeMap::new();
        let mut allocated_share = 0_u16;
        let mut allocated_budget = 0_u64;
        for (index, (run_id, weight)) in selected_weights.iter().enumerate() {
            let is_last = index + 1 == selected_count;
            let share = if is_last {
                10_000_u16.saturating_sub(allocated_share)
            } else {
                ((*weight).saturating_mul(10_000) / total_weight) as u16
            };
            let budget = if is_last {
                input.total_work_budget_ms.saturating_sub(allocated_budget)
            } else {
                input.total_work_budget_ms.saturating_mul(*weight) / total_weight
            };
            allocated_share = allocated_share.saturating_add(share);
            allocated_budget = allocated_budget.saturating_add(budget);
            share_by_run.insert(*run_id, (share, budget));
        }

        let mut limitation_codes = Vec::new();
        if input.resource_pressure >= ResourcePressure::Warm {
            limitation_codes.push("resource_pressure_reduced_attention".to_string());
        }
        if forced_atomic.len() > pressure_capacity {
            limitation_codes.push("atomic_sections_defer_full_preemption".to_string());
        }

        let mut allocations = Vec::with_capacity(ranked.len());
        let mut preempted_run_ids = Vec::new();
        for (rank, (goal, score)) in ranked.into_iter().enumerate() {
            let is_selected = selected.contains(goal.run_id.as_str());
            let forced = forced_atomic.contains(goal.run_id.as_str());
            let disposition = match input.resource_pressure {
                ResourcePressure::Critical if forced => AttentionDisposition::Throttle,
                ResourcePressure::Critical => AttentionDisposition::Stop,
                ResourcePressure::Hot if is_selected => AttentionDisposition::Throttle,
                ResourcePressure::Warm
                    if is_selected && goal.resource_class == ResourceClass::Heavy =>
                {
                    AttentionDisposition::Throttle
                }
                ResourcePressure::Normal | ResourcePressure::Warm if is_selected => {
                    AttentionDisposition::Run
                }
                ResourcePressure::Normal | ResourcePressure::Warm | ResourcePressure::Hot => {
                    AttentionDisposition::Pause
                }
            };
            let (attention_share_bps, work_budget_ms) = share_by_run
                .get(goal.run_id.as_str())
                .copied()
                .unwrap_or((0, 0));
            let interrupt_budget_ms = match goal.interruptibility {
                Interruptibility::Immediate => 0,
                Interruptibility::Checkpoint => goal.minimum_slice_ms.min(work_budget_ms),
                Interruptibility::Atomic => goal.estimated_remaining_ms,
            };
            let mut reason_codes = Vec::new();
            if forced {
                reason_codes.push("atomic_section_requires_checkpoint".to_string());
            }
            if goal.user_blocking {
                reason_codes.push("user_blocking_goal".to_string());
            }
            if input.resource_pressure == ResourcePressure::Hot
                && goal.resource_class == ResourceClass::Heavy
                && !forced
            {
                reason_codes.push("heavy_work_shed_under_hot_pressure".to_string());
            }
            if matches!(
                disposition,
                AttentionDisposition::Pause | AttentionDisposition::Stop
            ) {
                reason_codes.push("outside_current_attention_capacity".to_string());
                if goal.currently_running {
                    preempted_run_ids.push(goal.run_id.clone());
                }
            }
            allocations.push(GoalAttentionAllocation {
                mission_id: goal.mission_id.clone(),
                run_id: goal.run_id.clone(),
                rank: u32::try_from(rank + 1).unwrap_or(u32::MAX),
                score,
                disposition,
                attention_share_bps,
                work_budget_ms,
                interrupt_budget_ms,
                reason_codes,
            });
        }

        preempted_run_ids.sort();
        self.state = if input.resource_pressure == ResourcePressure::Normal {
            AttentionAgentState::Allocating
        } else {
            AttentionAgentState::Constrained
        };
        let plan = AttentionPlan {
            agent_id: ATTENTION_AGENT_ID.to_string(),
            state: self.state,
            epoch: self.epoch,
            resource_pressure: input.resource_pressure,
            effective_concurrency: selected.len(),
            allocations,
            preempted_run_ids,
            limitation_codes,
        };
        self.last_plan = Some(plan.clone());
        Ok(plan)
    }
}

pub fn attention_allocate_contract() -> CapabilityContract {
    CapabilityContract {
        capability_id: ATTENTION_ALLOCATE_CAPABILITY_ID.to_string(),
        version: 1,
        summary: "根据目标优先级、时限、中断边界和资源压力分配并行注意力预算".to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["nowMs", "resourcePressure", "maxConcurrent", "totalWorkBudgetMs", "goals"],
            "properties": {
                "resourcePressure": { "enum": ["normal", "warm", "hot", "critical"] },
                "maxConcurrent": { "type": "integer", "minimum": 1 },
                "totalWorkBudgetMs": { "type": "integer", "minimum": 0 },
                "goals": { "type": "array" }
            }
        }),
        output_schema: json!({
            "type": "object",
            "required": ["agentId", "state", "epoch", "effectiveConcurrency", "allocations"],
            "properties": {
                "state": { "enum": ["idle", "allocating", "constrained", "failed"] },
                "effectiveConcurrency": { "type": "integer", "minimum": 0 },
                "allocations": { "type": "array" },
                "preemptedRunIds": { "type": "array", "items": { "type": "string" } }
            }
        }),
        preconditions: Vec::new(),
        permissions: Vec::new(),
        side_effects: vec!["mission_scheduling_constraints".to_string()],
        resource_class: ResourceClass::Light,
        interruptibility: Interruptibility::Immediate,
        idempotent: false,
    }
}

fn validate_request(input: &AttentionAllocationInput) -> Result<(), AttentionAllocationError> {
    if input.now_ms < 0 {
        return Err(AttentionAllocationError::new(
            "attention allocation timestamp must be non-negative",
        ));
    }
    if input.max_concurrent == 0 {
        return Err(AttentionAllocationError::new(
            "attention max_concurrent must be positive",
        ));
    }
    let mut run_ids = BTreeSet::new();
    for goal in &input.goals {
        if goal.mission_id.trim().is_empty() || goal.run_id.trim().is_empty() {
            return Err(AttentionAllocationError::new(
                "attention goal mission_id and run_id must not be empty",
            ));
        }
        if !run_ids.insert(goal.run_id.as_str()) {
            return Err(AttentionAllocationError::new(format!(
                "duplicate attention run id {}",
                goal.run_id
            )));
        }
    }
    Ok(())
}

fn score_goal(goal: &AttentionGoal, now_ms: i64) -> i64 {
    let mut score = i64::from(goal.priority) * 1_000;
    if goal.user_blocking {
        score += 20_000;
    }
    if goal.user_visible {
        score += 5_000;
    }
    if goal.currently_running {
        // 小幅稳定性滞回，避免同分任务在采样边界反复抖动。
        score += 250;
    }
    if let Some(deadline_at_ms) = goal.deadline_at_ms {
        let remaining = deadline_at_ms.saturating_sub(now_ms);
        score += if remaining <= 0 {
            40_000
        } else if remaining <= 60_000 {
            30_000
        } else if remaining <= 300_000 {
            15_000
        } else {
            0
        };
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(run_id: &str, priority: u8, resource_class: ResourceClass) -> AttentionGoal {
        AttentionGoal {
            mission_id: format!("mission-{run_id}"),
            run_id: run_id.to_string(),
            priority,
            deadline_at_ms: None,
            user_visible: false,
            user_blocking: false,
            currently_running: true,
            resource_class,
            interruptibility: Interruptibility::Checkpoint,
            estimated_remaining_ms: 1_000,
            minimum_slice_ms: 100,
        }
    }

    #[test]
    fn user_blocking_goal_wins_and_budget_is_conserved() {
        let mut agent = AttentionAgent::default();
        let mut user_goal = goal("user", 50, ResourceClass::Moderate);
        user_goal.user_blocking = true;
        let plan = agent
            .allocate(AttentionAllocationInput {
                now_ms: 100,
                resource_pressure: ResourcePressure::Normal,
                max_concurrent: 2,
                total_work_budget_ms: 1_000,
                goals: vec![goal("background", 60, ResourceClass::Light), user_goal],
            })
            .expect("attention request should be valid");

        assert_eq!(plan.allocations[0].run_id, "user");
        assert_eq!(
            plan.allocations
                .iter()
                .map(|allocation| u32::from(allocation.attention_share_bps))
                .sum::<u32>(),
            10_000
        );
        assert_eq!(
            plan.allocations
                .iter()
                .map(|allocation| allocation.work_budget_ms)
                .sum::<u64>(),
            1_000
        );
    }

    #[test]
    fn hot_pressure_sheds_heavy_work_and_preempts_it() {
        let mut agent = AttentionAgent::default();
        let plan = agent
            .allocate(AttentionAllocationInput {
                now_ms: 100,
                resource_pressure: ResourcePressure::Hot,
                max_concurrent: 4,
                total_work_budget_ms: 500,
                goals: vec![
                    goal("heavy", 100, ResourceClass::Heavy),
                    goal("light", 20, ResourceClass::Light),
                ],
            })
            .expect("attention request should be valid");

        let heavy = plan
            .allocations
            .iter()
            .find(|allocation| allocation.run_id == "heavy")
            .expect("heavy allocation should exist");
        let light = plan
            .allocations
            .iter()
            .find(|allocation| allocation.run_id == "light")
            .expect("light allocation should exist");
        assert_eq!(heavy.disposition, AttentionDisposition::Pause);
        assert_eq!(light.disposition, AttentionDisposition::Throttle);
        assert_eq!(plan.effective_concurrency, 1);
        assert_eq!(plan.preempted_run_ids, vec!["heavy"]);
    }

    #[test]
    fn critical_pressure_stops_interruptible_work_but_respects_atomic_boundary() {
        let mut agent = AttentionAgent::default();
        let mut atomic = goal("atomic", 10, ResourceClass::Heavy);
        atomic.interruptibility = Interruptibility::Atomic;
        atomic.estimated_remaining_ms = 42;
        let plan = agent
            .allocate(AttentionAllocationInput {
                now_ms: 100,
                resource_pressure: ResourcePressure::Critical,
                max_concurrent: 2,
                total_work_budget_ms: 100,
                goals: vec![goal("normal", 100, ResourceClass::Light), atomic],
            })
            .expect("attention request should be valid");

        let atomic = plan
            .allocations
            .iter()
            .find(|allocation| allocation.run_id == "atomic")
            .expect("atomic allocation should exist");
        let normal = plan
            .allocations
            .iter()
            .find(|allocation| allocation.run_id == "normal")
            .expect("normal allocation should exist");
        assert_eq!(atomic.disposition, AttentionDisposition::Throttle);
        assert_eq!(atomic.interrupt_budget_ms, 42);
        assert_eq!(normal.disposition, AttentionDisposition::Stop);
    }
}
