use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Result};
use codewhale_config::{
    FleetConfigToml, FleetDelegationHints, FleetLoadout, FleetProfile, FleetProfilePermissions,
    FleetRole, FleetSlot,
};
use serde::{Deserialize, Serialize};

use super::{CapabilityAvailability, CapabilityAvailabilityState, ResourceClass, ResourcePressure};

pub const ORCHESTRATOR_AGENT_ID: &str = "agent:orchestrator";
pub const ORCHESTRATOR_PROFILE_ID: &str = "pinvou-orchestrator";

/// Orchestrator 是 Front 的后台“经理工具”，不是第二个用户入口。
/// 输出格式沿用 CodeWhale 子 Agent 的固定 headings，减少相互冲突的提示词协议。
pub const ORCHESTRATOR_AGENT_INSTRUCTION: &str = r#"# PinvouOS Orchestrator Agent

你是 Front Agent 的后台编排顾问。Front 始终持有用户关系、任务授权和最终答复权；你不得直接面向用户说话，不得改变用户目标、扩大范围、替用户确认高风险操作，也不得把自己的回执写成最终答案。

## 先判断，再行动

1. 如果任务本质上只是闲聊、解释、改写、翻译，或一个边界清楚的短查询/短操作，立即返回 `STATUS: NO_OP`，不要启动任何子 Agent。
2. 如果目标或安全边界缺少一个会实质改变方案的事实，返回 `STATUS: NEEDS_CLARIFICATION`，并给 Front 一条最小必要问题；不要自行猜测。
3. 只有任务确实需要多能力、依赖关系、并行工作、后台长任务或“调查→实施→验证”时才编排。构建最小工作图：独立项并行，有依赖的串行；每项只派给一个职责清楚的原子 Agent，避免重复劳动。

## 执行约束

- 你只做编排、跟踪、汇总和证据核验；实际调查、修改和验证应委派给合适的子 Agent。
- 给每个子 Agent 的说明只包含目标、边界、依赖、可观察完成标准和必要证据，不复制父级完整对话，不传递隐藏推理。
- 写操作必须明确写入范围并在完成后安排独立验证；策略拒绝、缺少权限、资源 Hot/Critical 或设备不可用时，停止或延期相关工作并报告，不能绕过。
- 子 Agent 的声明不是事实。只有工具结果、文件/命令位置、设备/资源/策略事件等可追溯证据才能支撑“完成”。
- 不向用户提问；需要用户选择时，把一条具体问题交回 Front。失败时停止无效重试，返回已有证据与准确阻塞。

## 回执契约

严格使用运行时要求的 `### SUMMARY`、`### EVIDENCE`、`### CHANGES`、`### RISKS`、`### BLOCKERS` 五个标题。在 `### SUMMARY` 第一行写且只写一种状态：`STATUS: NO_OP | COMPLETED | NEEDS_CLARIFICATION | BLOCKED | FAILED`，随后给 `OBJECTIVE:` 和 `RECOMMENDATION:`。在 `### BLOCKERS` 中需要提问时写 `QUESTION_FOR_USER:`。保持简洁，不输出思维链。

边界示例：解释 KV cache → `NO_OP`，零派工；调研一个方案、实现并验证 → 建最小工作图并派工；缺少目标设备且不同设备会改变操作 → `NEEDS_CLARIFICATION`，把一个问题交给 Front。"#;

/// 把 PinvouOS 保留的 Orchestrator profile 合并进已有 Fleet 配置。
/// 系统 profile 使用保留 id 并最终写入，防止个人/专家 profile 偶然覆盖核心角色。
#[must_use]
pub fn pinvou_os_fleet_config(base: Option<&FleetConfigToml>) -> FleetConfigToml {
    let mut config = base.cloned().unwrap_or_default();
    config.profiles.insert(
        ORCHESTRATOR_PROFILE_ID.to_string(),
        FleetProfile {
            slot: FleetSlot::Manager,
            role: FleetRole {
                name: "manager".to_string(),
                description: Some(
                    "PinvouOS 后台编排顾问；只向 Front 返回建议、证据与阻塞".to_string(),
                ),
                instructions: Some(ORCHESTRATOR_AGENT_INSTRUCTION.to_string()),
            },
            loadout: FleetLoadout::Inherit,
            permissions: FleetProfilePermissions::default(),
            delegation: FleetDelegationHints {
                max_spawn_depth: Some(2),
                max_concurrency: Some(4),
            },
            ..FleetProfile::default()
        },
    );
    config
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityNeed {
    pub capability_id: String,
    pub resource_class: ResourceClass,
    #[serde(default)]
    pub depends_on_capability_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MissionPlanningInput {
    pub objective: String,
    pub priority: u8,
    pub resource_pressure: ResourcePressure,
    pub needs: Vec<CapabilityNeed>,
    pub capability_reports: Vec<CapabilityAvailability>,
    #[serde(default)]
    pub evidence_event_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkDisposition {
    Ready,
    Deferred,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AtomicWorkItem {
    pub work_id: String,
    pub capability_id: String,
    pub resource_class: ResourceClass,
    pub disposition: WorkDisposition,
    pub candidate_agent_ids: Vec<String>,
    pub depends_on_work_ids: Vec<String>,
    pub reason_codes: Vec<String>,
}

/// Orchestrator Agent 的确定性第一版输出。真正的执行由能力 Agent 完成；
/// Orchestrator 只生成工作图，并把资源/能力事实造成的阻塞显式保留下来。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MissionWorkGraph {
    pub objective: String,
    pub priority: u8,
    pub work_items: Vec<AtomicWorkItem>,
    pub ready: bool,
    pub blocked: bool,
    pub reason_codes: Vec<String>,
    pub evidence_event_ids: Vec<String>,
}

pub fn build_mission_work_graph(input: &MissionPlanningInput) -> Result<MissionWorkGraph> {
    let objective = input.objective.trim();
    if objective.is_empty() {
        bail!("mission objective must not be empty");
    }
    if input.needs.is_empty() {
        bail!("mission must require at least one atomic capability");
    }

    let reports = index_reports(&input.capability_reports)?;
    let need_ids = validate_needs(&input.needs)?;
    let work_ids = need_ids
        .iter()
        .enumerate()
        .map(|(index, capability_id)| (capability_id.clone(), format!("work:{:03}", index + 1)))
        .collect::<BTreeMap<_, _>>();

    let mut work_items = Vec::with_capacity(input.needs.len());
    for need in &input.needs {
        let capability_id = need.capability_id.trim();
        let report = reports.get(capability_id);
        let (mut disposition, mut reason_codes, mut candidates) = match report {
            Some(report) if report.state == CapabilityAvailabilityState::Available => (
                WorkDisposition::Ready,
                report.reason_codes.clone(),
                report.candidate_agent_ids.clone(),
            ),
            Some(report) if report.state == CapabilityAvailabilityState::TemporarilyUnavailable => {
                (
                    WorkDisposition::Deferred,
                    report.reason_codes.clone(),
                    report.candidate_agent_ids.clone(),
                )
            }
            Some(report) => (
                WorkDisposition::Blocked,
                report.reason_codes.clone(),
                report.candidate_agent_ids.clone(),
            ),
            None => (
                WorkDisposition::Blocked,
                vec!["capability_report_missing".to_string()],
                Vec::new(),
            ),
        };

        if disposition == WorkDisposition::Ready
            && resource_governor_defers(need.resource_class, input.resource_pressure)
        {
            disposition = WorkDisposition::Deferred;
            reason_codes.push(
                match input.resource_pressure {
                    ResourcePressure::Warm => "heavy_work_deferred_by_resource_governor",
                    ResourcePressure::Hot => "work_deferred_while_device_hot",
                    ResourcePressure::Critical => "work_deferred_while_device_critical",
                    ResourcePressure::Normal => unreachable!(),
                }
                .to_string(),
            );
        }
        candidates.sort();
        candidates.dedup();
        reason_codes.sort();
        reason_codes.dedup();

        work_items.push(AtomicWorkItem {
            work_id: work_ids[capability_id].clone(),
            capability_id: capability_id.to_string(),
            resource_class: need.resource_class,
            disposition,
            candidate_agent_ids: candidates,
            depends_on_work_ids: need
                .depends_on_capability_ids
                .iter()
                .map(|dependency| work_ids[dependency.trim()].clone())
                .collect(),
            reason_codes,
        });
    }

    // 依赖尚未 Ready 时，下游不能被误报为可立即执行。固定点传播覆盖多级依赖。
    loop {
        let dispositions = work_items
            .iter()
            .map(|item| (item.work_id.clone(), item.disposition))
            .collect::<BTreeMap<_, _>>();
        let mut changed = false;
        for item in &mut work_items {
            if item.disposition != WorkDisposition::Ready {
                continue;
            }
            let dependency_states = item
                .depends_on_work_ids
                .iter()
                .filter_map(|work_id| dispositions.get(work_id))
                .copied()
                .collect::<Vec<_>>();
            if dependency_states
                .iter()
                .any(|state| *state == WorkDisposition::Blocked)
            {
                item.disposition = WorkDisposition::Blocked;
                item.reason_codes.push("dependency_blocked".to_string());
                changed = true;
            } else if dependency_states
                .iter()
                .any(|state| *state == WorkDisposition::Deferred)
            {
                item.disposition = WorkDisposition::Deferred;
                item.reason_codes.push("waiting_for_dependency".to_string());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let blocked = work_items
        .iter()
        .any(|item| item.disposition == WorkDisposition::Blocked);
    let ready = !blocked
        && work_items
            .iter()
            .all(|item| item.disposition == WorkDisposition::Ready);
    let mut reason_codes = BTreeSet::new();
    if blocked {
        reason_codes.insert("one_or_more_capabilities_blocked".to_string());
    }
    if work_items
        .iter()
        .any(|item| item.disposition == WorkDisposition::Deferred)
    {
        reason_codes.insert("one_or_more_capabilities_deferred".to_string());
    }
    if ready {
        reason_codes.insert("work_graph_ready".to_string());
    }

    Ok(MissionWorkGraph {
        objective: objective.to_string(),
        priority: input.priority,
        work_items,
        ready,
        blocked,
        reason_codes: reason_codes.into_iter().collect(),
        evidence_event_ids: normalized_identifiers(&input.evidence_event_ids)?,
    })
}

fn resource_governor_defers(class: ResourceClass, pressure: ResourcePressure) -> bool {
    match pressure {
        ResourcePressure::Normal => false,
        ResourcePressure::Warm => class == ResourceClass::Heavy,
        ResourcePressure::Hot => matches!(class, ResourceClass::Moderate | ResourceClass::Heavy),
        ResourcePressure::Critical => true,
    }
}

fn index_reports(
    reports: &[CapabilityAvailability],
) -> Result<BTreeMap<&str, &CapabilityAvailability>> {
    let mut indexed = BTreeMap::new();
    for report in reports {
        validate_identifier(&report.capability_id, "capability report id")?;
        if indexed
            .insert(report.capability_id.as_str(), report)
            .is_some()
        {
            bail!("duplicate capability report {}", report.capability_id);
        }
    }
    Ok(indexed)
}

fn validate_needs(needs: &[CapabilityNeed]) -> Result<Vec<String>> {
    let mut ids = Vec::with_capacity(needs.len());
    let mut unique = BTreeSet::new();
    for need in needs {
        validate_identifier(&need.capability_id, "capability need id")?;
        let id = need.capability_id.trim().to_string();
        if !unique.insert(id.clone()) {
            bail!("duplicate capability need {id}");
        }
        ids.push(id);
    }
    for need in needs {
        for dependency in &need.depends_on_capability_ids {
            validate_identifier(dependency, "capability dependency id")?;
            if dependency.trim() == need.capability_id.trim() {
                bail!("capability cannot depend on itself");
            }
            if !unique.contains(dependency.trim()) {
                bail!("unknown capability dependency {}", dependency.trim());
            }
        }
    }
    let dependencies = needs
        .iter()
        .map(|need| {
            (
                need.capability_id.trim(),
                need.depends_on_capability_ids
                    .iter()
                    .map(|dependency| dependency.trim())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for capability_id in dependencies.keys() {
        ensure_acyclic(capability_id, &dependencies, &mut visiting, &mut visited)?;
    }
    Ok(ids)
}

fn ensure_acyclic<'a>(
    capability_id: &'a str,
    dependencies: &BTreeMap<&'a str, Vec<&'a str>>,
    visiting: &mut BTreeSet<&'a str>,
    visited: &mut BTreeSet<&'a str>,
) -> Result<()> {
    if visited.contains(capability_id) {
        return Ok(());
    }
    if !visiting.insert(capability_id) {
        bail!("capability dependency cycle includes {capability_id}");
    }
    for dependency in &dependencies[capability_id] {
        ensure_acyclic(dependency, dependencies, visiting, visited)?;
    }
    visiting.remove(capability_id);
    visited.insert(capability_id);
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 128 {
        bail!("{label} must contain 1 to 128 characters");
    }
    if !value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | ':' | '_' | '-')
    }) {
        bail!("{label} contains unsupported characters");
    }
    Ok(())
}

fn normalized_identifiers(values: &[String]) -> Result<Vec<String>> {
    let mut normalized = values
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    for value in &normalized {
        validate_identifier(value, "evidence event id")?;
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_orchestrator_profile_is_bounded_and_preserves_other_profiles() {
        let mut base = FleetConfigToml::default();
        base.profiles
            .insert("other-agent".to_string(), FleetProfile::default());
        // 即使外部配置碰巧使用保留 id，系统定义仍必须最终获胜。
        base.profiles
            .insert(ORCHESTRATOR_PROFILE_ID.to_string(), FleetProfile::default());

        let merged = pinvou_os_fleet_config(Some(&base));
        let profile = &merged.profiles[ORCHESTRATOR_PROFILE_ID];

        assert!(merged.profiles.contains_key("other-agent"));
        assert_eq!(profile.slot, FleetSlot::Manager);
        assert_eq!(profile.role.name, "manager");
        assert_eq!(profile.delegation.max_spawn_depth, Some(2));
        assert_eq!(profile.delegation.max_concurrency, Some(4));
        assert!(!profile.permissions.allow_shell);
        assert!(profile.permissions.approval_required);
        let instructions = profile.role.instructions.as_deref().unwrap();
        assert!(instructions.contains("STATUS: NO_OP"));
        assert!(instructions.contains("Front 始终持有用户关系"));
        assert!(instructions.contains("QUESTION_FOR_USER:"));
    }

    fn report(capability_id: &str, state: CapabilityAvailabilityState) -> CapabilityAvailability {
        CapabilityAvailability {
            capability_id: capability_id.to_string(),
            state,
            candidate_agent_ids: vec!["agent:z".to_string(), "agent:a".to_string()],
            reason_codes: Vec::new(),
        }
    }

    fn need(capability_id: &str, resource_class: ResourceClass) -> CapabilityNeed {
        CapabilityNeed {
            capability_id: capability_id.to_string(),
            resource_class,
            depends_on_capability_ids: Vec::new(),
        }
    }

    #[test]
    fn available_atomic_capabilities_form_a_ready_graph() {
        let graph = build_mission_work_graph(&MissionPlanningInput {
            objective: "answer the user".to_string(),
            priority: 70,
            resource_pressure: ResourcePressure::Normal,
            needs: vec![
                need("memory.context", ResourceClass::Moderate),
                need("user.interact", ResourceClass::Light),
            ],
            capability_reports: vec![
                report("memory.context", CapabilityAvailabilityState::Available),
                report("user.interact", CapabilityAvailabilityState::Available),
            ],
            evidence_event_ids: vec!["event:2".to_string(), "event:1".to_string()],
        })
        .unwrap();

        assert!(graph.ready);
        assert!(!graph.blocked);
        assert_eq!(graph.work_items.len(), 2);
        assert_eq!(
            graph.work_items[0].candidate_agent_ids,
            vec!["agent:a", "agent:z"]
        );
        assert_eq!(graph.evidence_event_ids, vec!["event:1", "event:2"]);
    }

    #[test]
    fn resource_fact_defers_heavy_work_without_hiding_other_work() {
        let graph = build_mission_work_graph(&MissionPlanningInput {
            objective: "inspect and answer".to_string(),
            priority: 60,
            resource_pressure: ResourcePressure::Hot,
            needs: vec![
                need("surface.observe", ResourceClass::Heavy),
                need("user.interact", ResourceClass::Light),
            ],
            capability_reports: vec![
                report("surface.observe", CapabilityAvailabilityState::Available),
                report("user.interact", CapabilityAvailabilityState::Available),
            ],
            evidence_event_ids: Vec::new(),
        })
        .unwrap();

        assert!(!graph.ready);
        assert!(!graph.blocked);
        assert_eq!(graph.work_items[0].disposition, WorkDisposition::Deferred);
        assert_eq!(graph.work_items[1].disposition, WorkDisposition::Ready);
    }

    #[test]
    fn unsupported_capability_is_an_explicit_blocker() {
        let graph = build_mission_work_graph(&MissionPlanningInput {
            objective: "teleport".to_string(),
            priority: 50,
            resource_pressure: ResourcePressure::Normal,
            needs: vec![need("teleport.execute", ResourceClass::Heavy)],
            capability_reports: vec![report(
                "teleport.execute",
                CapabilityAvailabilityState::Unsupported,
            )],
            evidence_event_ids: Vec::new(),
        })
        .unwrap();
        assert!(graph.blocked);
        assert_eq!(graph.work_items[0].disposition, WorkDisposition::Blocked);
    }

    #[test]
    fn cyclic_capability_dependencies_are_rejected() {
        let mut first = need("first", ResourceClass::Light);
        first.depends_on_capability_ids = vec!["second".to_string()];
        let mut second = need("second", ResourceClass::Light);
        second.depends_on_capability_ids = vec!["first".to_string()];
        let result = build_mission_work_graph(&MissionPlanningInput {
            objective: "cycle".to_string(),
            priority: 50,
            resource_pressure: ResourcePressure::Normal,
            needs: vec![first, second],
            capability_reports: vec![
                report("first", CapabilityAvailabilityState::Available),
                report("second", CapabilityAvailabilityState::Available),
            ],
            evidence_event_ids: Vec::new(),
        });
        assert!(result.is_err());
    }

    #[test]
    fn blocked_dependency_blocks_its_downstream_work() {
        let first = need("first", ResourceClass::Light);
        let mut second = need("second", ResourceClass::Light);
        second.depends_on_capability_ids = vec!["first".to_string()];
        let graph = build_mission_work_graph(&MissionPlanningInput {
            objective: "dependency".to_string(),
            priority: 50,
            resource_pressure: ResourcePressure::Normal,
            needs: vec![first, second],
            capability_reports: vec![
                report("first", CapabilityAvailabilityState::Unsupported),
                report("second", CapabilityAvailabilityState::Available),
            ],
            evidence_event_ids: Vec::new(),
        })
        .unwrap();
        assert_eq!(graph.work_items[0].disposition, WorkDisposition::Blocked);
        assert_eq!(graph.work_items[1].disposition, WorkDisposition::Blocked);
        assert!(graph.work_items[1]
            .reason_codes
            .contains(&"dependency_blocked".to_string()));
    }
}
