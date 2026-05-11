//! CombinedPlanner — 一次 LLM 调用同时输出 agent + milestones。
//!
//! 取代旧的 `DynamicPlanner`（仅拆解 milestone）+ 隐式 App 选择。
//!
//! ## 输入
//! - 用户消息
//! - `AgentRegistry`（用于注入 agent 列表 + 校验 agent_id）
//!
//! ## 输出
//! ```json
//! {
//!   "agent": "doc_generation",
//!   "milestones": [
//!     {"label": "...", "mode": "collect", "tools": ["request_user_input"],
//!      "prompt_hint": "...", "required_context": [], "produced_context": []}
//!   ]
//! }
//! ```
//! 若 `agent == "qa"`，则 `milestones` 必须是空数组。
//!
//! ## 校验
//! 只做结构性校验，不判断内容合理性：
//! - `agent` ∈ AgentRegistry 注册项
//! - 若非 qa：
//!   - milestones 数量 ∈ [2, 8]
//!   - 每个 mode 是合法枚举
//!   - 最后一个 mode == FinalOutput
//!   - 每个 tool ∈ GLOBAL_TOOL_POOL
//!   - mode-tool 兼容性通过
//!   - label 非空且不重复
//!
//! 校验失败时调用方可以使用 `fallback_plan()`。

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashSet;

use crate::agent_registry::AgentRegistry;
use crate::contract::{MilestoneMode, is_tool_in_global_pool, mode_tool_compatibility};

/// LLM 拆解的完整结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombinedPlan {
    pub agent_id: String,
    /// 若 agent_id == "qa"，此处为空
    pub milestones: Vec<PlannedMilestone>,
}

impl CombinedPlan {
    pub fn is_qa(&self) -> bool {
        self.agent_id == "qa" && self.milestones.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedMilestone {
    pub label: String,
    pub mode: MilestoneMode,
    pub tools: Vec<String>,
    pub prompt_hint: Option<String>,
    pub required_context: Vec<String>,
    pub produced_context: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PlanDto {
    agent: String,
    #[serde(default)]
    milestones: Vec<PlannedMilestoneDto>,
}

#[derive(Debug, Deserialize)]
struct PlannedMilestoneDto {
    label: String,
    mode: MilestoneMode,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    prompt_hint: Option<String>,
    #[serde(default)]
    required_context: Vec<String>,
    #[serde(default)]
    produced_context: Vec<String>,
}

pub struct CombinedPlanner;

impl CombinedPlanner {
    /// 构造发给 LLM 的拆解 prompt。
    ///
    /// `available_tools` 必须是当前 harness **实际可执行**的工具列表，
    /// 否则 LLM 会被诱导调用根本不存在的工具，输出形似 `[web_search: ...]`
    /// 的纯文本伪工具调用。
    pub fn build_prompt(
        user_message: &str,
        agents: &AgentRegistry,
        available_tools: &[String],
    ) -> String {
        let agent_list = agents.render_for_planner();
        let tools_list = if available_tools.is_empty() {
            "- （当前无可用工具，所有阶段只能用文本输出）".to_string()
        } else {
            available_tools
                .iter()
                .map(|t| format!("- {t}"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        format!(
            "用户输入: \"{user_message}\"

可用 agents:
{agent_list}

可用 mode（每个 milestone 必须选一个）:
- collect: 收集用户决策性信息
- produce_options: 给 2-3 个方案让用户选
- refine_selected_option: 细化已选方案
- freeform: 自由产出（写作 / 分析 / 计算）
- final_output: 最终交付物（必须是最后一个 milestone）

可用工具池（每个 milestone 从中选 0-N 个）:
{tools_list}

请输出 JSON:
{{
  \"agent\": \"<agent_id>\",
  \"milestones\": [
    {{
      \"label\": \"...\",
      \"mode\": \"...\",
      \"tools\": [\"...\"],
      \"prompt_hint\": \"...\",
      \"required_context\": [],
      \"produced_context\": []
    }}
  ]
}}

约束:
- 如果 agent=qa，milestones 必须为空数组 []
- 否则 milestones 数量 2-8 个
- 最后一个 milestone 必须 mode=final_output
- tools 必须从工具池中选；mode=final_output 时不能含 request_user_input
- 如果用户首条消息已提供关键信息，对应的 collect 阶段可以省略
- 只输出 JSON，不要任何其他文本"
        )
    }

    /// 解析 LLM 输出并做结构性校验。
    ///
    /// `available_tools` 是当前 harness 实际可执行的工具集；LLM 选了池外的工具
    /// （比如训练数据里见过但当前没注册的）会直接拒绝，避免下游伪工具调用。
    pub fn parse_plan(
        text: &str,
        agents: &AgentRegistry,
        available_tools: &[String],
    ) -> Result<CombinedPlan> {
        let json = extract_json_object(text).context("response has no JSON object")?;
        let dto: PlanDto = serde_json::from_str(json).context("failed to parse plan JSON")?;
        validate_dto(dto, agents, available_tools)
    }

    /// 校验失败时使用的兜底计划
    pub fn fallback_plan() -> CombinedPlan {
        CombinedPlan {
            agent_id: "generic".to_string(),
            milestones: vec![
                PlannedMilestone {
                    label: "明确需求".to_string(),
                    mode: MilestoneMode::Collect,
                    tools: vec!["request_user_input".to_string()],
                    prompt_hint: Some("确认用户具体要做什么".to_string()),
                    required_context: vec![],
                    produced_context: vec![],
                },
                PlannedMilestone {
                    label: "完成任务".to_string(),
                    mode: MilestoneMode::Freeform,
                    tools: vec![],
                    prompt_hint: Some("基于已知需求完成任务".to_string()),
                    required_context: vec![],
                    produced_context: vec![],
                },
                PlannedMilestone {
                    label: "输出结果".to_string(),
                    mode: MilestoneMode::FinalOutput,
                    tools: vec![],
                    prompt_hint: Some("输出最终交付物".to_string()),
                    required_context: vec![],
                    produced_context: vec![],
                },
            ],
        }
    }
}

fn validate_dto(
    dto: PlanDto,
    agents: &AgentRegistry,
    available_tools: &[String],
) -> Result<CombinedPlan> {
    // 1. agent 必须已注册
    if !agents.contains(&dto.agent) {
        bail!("agent '{}' not in registry", dto.agent);
    }

    // 2. qa 必须 milestones 为空
    if dto.agent == "qa" {
        if !dto.milestones.is_empty() {
            bail!("qa agent must have empty milestones, got {}", dto.milestones.len());
        }
        return Ok(CombinedPlan {
            agent_id: dto.agent,
            milestones: vec![],
        });
    }

    // 3. 非 qa 的 milestone 数量限制
    if dto.milestones.is_empty() {
        bail!("non-qa agent must have milestones, got 0");
    }
    if dto.milestones.len() < 2 || dto.milestones.len() > 8 {
        bail!("milestones count must be 2-8, got {}", dto.milestones.len());
    }

    // 4. label 非空且不重复
    let mut seen_labels = HashSet::new();
    for m in &dto.milestones {
        if m.label.trim().is_empty() {
            bail!("milestone label is empty");
        }
        if !seen_labels.insert(m.label.clone()) {
            bail!("duplicate milestone label: {}", m.label);
        }
    }

    // 5. 最后一个必须是 final_output
    let last_mode = &dto.milestones.last().unwrap().mode;
    if !matches!(last_mode, MilestoneMode::FinalOutput) {
        bail!("last milestone must be final_output, got {:?}", last_mode);
    }

    // 6. 中间不能有 final_output（仅末尾允许）
    for m in &dto.milestones[..dto.milestones.len() - 1] {
        if matches!(m.mode, MilestoneMode::FinalOutput) {
            bail!(
                "final_output may only appear as the last milestone, found '{}'",
                m.label
            );
        }
    }

    // 7. tools 校验
    let mut milestones = Vec::with_capacity(dto.milestones.len());
    for m in dto.milestones {
        for t in &m.tools {
            if !is_tool_in_global_pool(t) {
                bail!("tool '{}' not in global pool", t);
            }
            if !available_tools.iter().any(|x| x == t) {
                bail!(
                    "tool '{}' not available on current harness (advertised={:?})",
                    t, available_tools
                );
            }
            if let Some(reason) = mode_tool_compatibility(m.mode.clone(), t) {
                bail!("tool incompat: {reason}");
            }
        }
        milestones.push(PlannedMilestone {
            label: m.label,
            mode: m.mode,
            tools: m.tools,
            prompt_hint: m.prompt_hint,
            required_context: m.required_context,
            produced_context: m.produced_context,
        });
    }

    Ok(CombinedPlan {
        agent_id: dto.agent,
        milestones,
    })
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    Some(&text[start..=end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_registry::AgentDefinition;

    fn test_tool_pool() -> Vec<String> {
        vec![
            "request_user_input".into(),
            "file_read".into(),
            "file_write".into(),
            "web_search".into(),
            "python_exec".into(),
        ]
    }

    fn registry_with_basic_agents() -> AgentRegistry {
        let mut reg = AgentRegistry::default();
        for (id, name) in [
            ("qa", "Q&A"),
            ("doc_generation", "Docs"),
            ("data_analysis", "Data"),
            ("planning", "Plans"),
            ("generic", "Generic"),
        ] {
            reg.register(AgentDefinition {
                id: id.to_string(),
                name: name.to_string(),
                description: format!("{name} agent"),
                emoji: None,
                body: String::new(),
            });
        }
        reg
    }

    #[test]
    fn build_prompt_includes_agents_and_tools() {
        let reg = registry_with_basic_agents();
        let prompt = CombinedPlanner::build_prompt("帮我写周报", &reg, &test_tool_pool());
        assert!(prompt.contains("帮我写周报"));
        assert!(prompt.contains("- qa:"));
        assert!(prompt.contains("- doc_generation:"));
        assert!(prompt.contains("request_user_input"));
        assert!(prompt.contains("collect:"));
        assert!(prompt.contains("final_output:"));
    }

    #[test]
    fn parse_qa_plan_with_empty_milestones() {
        let reg = registry_with_basic_agents();
        let json = r#"{"agent": "qa", "milestones": []}"#;
        let plan = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap();
        assert!(plan.is_qa());
        assert_eq!(plan.agent_id, "qa");
    }

    #[test]
    fn parse_qa_with_milestones_fails() {
        let reg = registry_with_basic_agents();
        let json = r#"{"agent": "qa", "milestones": [{"label": "x", "mode": "freeform"}]}"#;
        assert!(CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).is_err());
    }

    #[test]
    fn parse_unknown_agent_fails() {
        let reg = registry_with_basic_agents();
        let json = r#"{"agent": "nonexistent", "milestones": []}"#;
        assert!(CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).is_err());
    }

    #[test]
    fn parse_valid_doc_generation_plan() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "确认结构", "mode": "produce_options", "tools": ["request_user_input"]},
                {"label": "生成草稿", "mode": "freeform", "tools": []},
                {"label": "定稿", "mode": "final_output", "tools": ["file_write"]}
            ]
        }"#;
        let plan = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap();
        assert_eq!(plan.agent_id, "doc_generation");
        assert_eq!(plan.milestones.len(), 3);
        assert_eq!(plan.milestones[0].mode, MilestoneMode::ProduceOptions);
        assert_eq!(plan.milestones[2].mode, MilestoneMode::FinalOutput);
    }

    #[test]
    fn parse_rejects_last_not_final_output() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "a", "mode": "collect", "tools": []},
                {"label": "b", "mode": "freeform", "tools": []}
            ]
        }"#;
        let err = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap_err();
        assert!(err.to_string().contains("final_output"));
    }

    #[test]
    fn parse_rejects_final_output_in_middle() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "a", "mode": "final_output", "tools": []},
                {"label": "b", "mode": "final_output", "tools": []}
            ]
        }"#;
        assert!(CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).is_err());
    }

    #[test]
    fn parse_rejects_too_few_milestones() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "a", "mode": "final_output", "tools": []}
            ]
        }"#;
        assert!(CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).is_err());
    }

    #[test]
    fn parse_rejects_too_many_milestones() {
        let _reg = registry_with_basic_agents();
        let mut milestones: Vec<String> = Vec::new();
        for i in 0..8 {
            milestones.push(format!(
                r#"{{"label": "m{i}", "mode": "freeform", "tools": []}}"#
            ));
        }
        milestones.push(r#"{"label": "end", "mode": "final_output", "tools": []}"#.into());
        let json = format!(
            r#"{{"agent": "doc_generation", "milestones": [{}]}}"#,
            milestones.join(",")
        );
        assert!(CombinedPlanner::parse_plan(&json, &registry_with_basic_agents(), &test_tool_pool()).is_err());
    }

    #[test]
    fn parse_rejects_tool_outside_global_pool() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "a", "mode": "freeform", "tools": ["rm_rf"]},
                {"label": "b", "mode": "final_output", "tools": []}
            ]
        }"#;
        let err = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap_err();
        assert!(err.to_string().contains("rm_rf"));
    }

    #[test]
    fn parse_rejects_request_input_in_final_output() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "a", "mode": "freeform", "tools": []},
                {"label": "b", "mode": "final_output", "tools": ["request_user_input"]}
            ]
        }"#;
        let err = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap_err();
        assert!(err.to_string().contains("final_output"));
    }

    #[test]
    fn parse_rejects_duplicate_labels() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "X", "mode": "freeform", "tools": []},
                {"label": "X", "mode": "final_output", "tools": []}
            ]
        }"#;
        let err = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn parse_rejects_empty_label() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "  ", "mode": "freeform", "tools": []},
                {"label": "end", "mode": "final_output", "tools": []}
            ]
        }"#;
        assert!(CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).is_err());
    }

    #[test]
    fn parse_handles_json_with_surrounding_text() {
        let reg = registry_with_basic_agents();
        let text = "Here is the plan:\n{\"agent\": \"qa\", \"milestones\": []}\n\nDone.";
        let plan = CombinedPlanner::parse_plan(text, &reg, &test_tool_pool()).unwrap();
        assert!(plan.is_qa());
    }

    #[test]
    fn fallback_plan_is_generic_and_passes_validation() {
        let reg = registry_with_basic_agents();
        let plan = CombinedPlanner::fallback_plan();
        assert_eq!(plan.agent_id, "generic");
        assert_eq!(plan.milestones.len(), 3);
        assert_eq!(
            plan.milestones.last().unwrap().mode,
            MilestoneMode::FinalOutput
        );
        // 也能通过同样的校验逻辑（自洽性）
        let json = serde_json::json!({
            "agent": plan.agent_id,
            "milestones": plan.milestones.iter().map(|m| serde_json::json!({
                "label": m.label,
                "mode": serde_json::to_value(&m.mode).unwrap(),
                "tools": m.tools,
            })).collect::<Vec<_>>()
        });
        assert!(CombinedPlanner::parse_plan(&json.to_string(), &reg, &test_tool_pool()).is_ok());
    }
}
