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
//!   - milestones 数量 ∈ [2, 12]
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
use crate::contract::{MilestoneMode, OutputRequirement, contract_for_mode};

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
- final_output: 最终交付物（必须出现，要么是最后一个，要么倒数第二个）
- review: 产物审核，让用户决定满意/微调/重做（**可选**：建议在 final_output 之后加一个，适合 planning / doc_generation 类任务，让用户拿到产物后能微调）

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
- 否则 milestones 数量 2-12 个
- 必须有且仅有一个 mode=final_output
- 最后一个 milestone 必须是 final_output 或 review；如果最后是 review，其前一个必须是 final_output
- tools 必须从工具池中选；mode=final_output 时不能含 request_user_input
- 如果用户首条消息已提供关键信息，对应的 collect 阶段可以省略

**严格的 mode → tools 对应（违反会被拒绝）**：
- mode=collect          → tools **必须含** `request_user_input`
- mode=produce_options  → tools **必须含** `request_user_input`（用于展示 2-3 选项给用户选）
- mode=refine_selected_option → tools 可以为空，或含 file_read / web_search
- mode=freeform         → tools 可以为空（纯产出），或含搜索/读文件类工具
- mode=final_output     → tools 可含 file_write 等导出工具，**不能含** request_user_input
- mode=review           → tools **必须含** `request_user_input`（给用户做选择题：满意/微调/重做）

口诀：要让用户做选择就用 collect 或 produce_options，并把 request_user_input 放进它的 tools 里。
绝对不要在 produce_options 里写 `\"tools\": []` —— 这会让用户看到一堆文本问题而不是选择卡片。

只输出 JSON，不要任何其他文本"
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
    if dto.milestones.len() < 2 || dto.milestones.len() > 12 {
        bail!("milestones count must be 2-12, got {}", dto.milestones.len());
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

    // 5. 最后一个必须是 final_output 或 review
    let last_idx = dto.milestones.len() - 1;
    let last_mode = &dto.milestones[last_idx].mode;
    match last_mode {
        MilestoneMode::FinalOutput => { /* ok */ }
        MilestoneMode::Review => {
            // review 作为最后一个时，其前一个必须是 final_output
            if last_idx == 0 {
                bail!("review milestone cannot be the only milestone");
            }
            let prev_mode = &dto.milestones[last_idx - 1].mode;
            if !matches!(prev_mode, MilestoneMode::FinalOutput) {
                bail!(
                    "review milestone must follow final_output, found {:?} before review",
                    prev_mode
                );
            }
        }
        MilestoneMode::PatchOutput => bail!(
            "patch_output cannot appear in initial plan (it is dynamically inserted by review tweak)"
        ),
        other => bail!(
            "last milestone must be final_output or review, got {:?}",
            other
        ),
    }

    // 6. 中间不能有 final_output / review / patch_output
    // - final_output 只能末尾或紧邻 review 之前
    // - review 只能末尾
    // - patch_output 完全不能在初始拆解中出现（只能由 review tweak 动态触发插入）
    for m in &dto.milestones[..last_idx] {
        match m.mode {
            MilestoneMode::FinalOutput => {
                // 例外：如果最后一个是 review，倒数第二个 final_output 是合法的
                let is_pre_review_final = matches!(last_mode, MilestoneMode::Review)
                    && std::ptr::eq(m, &dto.milestones[last_idx - 1]);
                if !is_pre_review_final {
                    bail!(
                        "final_output may only appear as the last milestone (or right before review), found '{}'",
                        m.label
                    );
                }
            }
            MilestoneMode::Review => {
                bail!(
                    "review may only appear as the last milestone, found '{}'",
                    m.label
                );
            }
            MilestoneMode::PatchOutput => {
                bail!(
                    "patch_output cannot appear in initial plan (dynamically inserted by review tweak), found '{}'",
                    m.label
                );
            }
            _ => {}
        }
    }

    // 7. tools 校验 —— 三层：
    //   a) available_tools 是权威来源（每个 tool 必须在 harness 实际注册的池里）
    //   b) ForbidTool：mode 禁用的工具不能出现
    //   c) RequiresToolCall：mode 必需的工具必须出现（防 LLM 拆解时漏填导致下游退化文本）
    let mut milestones = Vec::with_capacity(dto.milestones.len());
    for m in dto.milestones {
        let mode_contract = contract_for_mode(m.mode.clone());

        // a + b：逐 tool 检查白名单与禁用
        for t in &m.tools {
            if !available_tools.iter().any(|x| x == t) {
                bail!(
                    "tool '{}' not available on current harness (advertised={:?})",
                    t, available_tools
                );
            }
            for req in &mode_contract.output_requirements {
                if let OutputRequirement::ForbidTool(forbidden) = req {
                    if forbidden == t {
                        bail!(
                            "tool '{}' forbidden in mode {:?}（contract::ForbidTool）",
                            t, m.mode
                        );
                    }
                }
            }
        }

        // c：检查必需工具都已声明。
        // 这是关键的「自洽校验」：mode 的 RequiresToolCall 规则要求某工具，
        // 那 milestone.tools 里必须有它。否则下游 filter_tools_for_contract
        // 会过滤掉所有工具，LLM 看到空 tools 后退化文本路径。
        for req in &mode_contract.output_requirements {
            if let OutputRequirement::RequiresToolCall(required) = req {
                if !m.tools.iter().any(|t| t == required) {
                    bail!(
                        "milestone '{}' (mode={:?}) 必须在 tools 中声明 `{}`（mode 的 RequiresToolCall 硬规则）",
                        m.label, m.mode, required
                    );
                }
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
        // 与 GLOBAL_TOOL_POOL 对齐 + 保留 file_write 兼容旧测试用例
        vec![
            "request_user_input".into(),
            "read_file".into(),
            "write_file".into(),
            "web_search".into(),
            "exec_shell".into(),
            "file_write".into(),
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
    fn parse_accepts_review_as_last_after_final_output() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "选方案", "mode": "produce_options", "tools": ["request_user_input"]},
                {"label": "定稿", "mode": "final_output", "tools": ["write_file"]},
                {"label": "审核", "mode": "review", "tools": ["request_user_input"]}
            ]
        }"#;
        let plan = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap();
        assert_eq!(plan.milestones[2].mode, MilestoneMode::Review);
    }

    #[test]
    fn parse_rejects_review_not_after_final_output() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "草稿", "mode": "freeform", "tools": []},
                {"label": "审核", "mode": "review", "tools": ["request_user_input"]}
            ]
        }"#;
        let err = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap_err();
        assert!(
            err.to_string().contains("review")
                && err.to_string().contains("final_output"),
            "应提示 review 必须跟在 final_output 后面: {err}"
        );
    }

    #[test]
    fn parse_rejects_review_in_middle() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "审核1", "mode": "review", "tools": ["request_user_input"]},
                {"label": "定稿", "mode": "final_output", "tools": []}
            ]
        }"#;
        let err = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap_err();
        assert!(
            err.to_string().contains("review may only appear as the last"),
            "review 在中间应拒绝: {err}"
        );
    }

    #[test]
    fn parse_rejects_patch_output_in_initial_plan() {
        // patch_output 是 review tweak 动态插入的，不应在初始拆解里
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "草稿", "mode": "freeform", "tools": []},
                {"label": "修订", "mode": "patch_output", "tools": ["edit_file"]},
                {"label": "定稿", "mode": "final_output", "tools": ["write_file"]}
            ]
        }"#;
        let err = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap_err();
        assert!(
            err.to_string().contains("patch_output"),
            "应拒绝初始拆解里出现 patch_output: {err}"
        );
    }

    #[test]
    fn parse_rejects_patch_output_as_last_milestone() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "定稿", "mode": "final_output", "tools": ["write_file"]},
                {"label": "修订", "mode": "patch_output", "tools": ["edit_file"]}
            ]
        }"#;
        let err = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap_err();
        assert!(err.to_string().contains("patch_output"));
    }

    #[test]
    fn parse_rejects_review_without_request_user_input() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "定稿", "mode": "final_output", "tools": ["write_file"]},
                {"label": "审核", "mode": "review", "tools": []}
            ]
        }"#;
        let err = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap_err();
        assert!(
            err.to_string().contains("request_user_input")
                || err.to_string().contains("RequiresToolCall"),
            "review 没声明 request_user_input 应拒绝: {err}"
        );
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
        // 12 freeform + 1 final = 13 总数，超过 12 上限
        for i in 0..12 {
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
    fn parse_accepts_10_milestones() {
        // 验证新上限：10 步（含 final_output）应该通过
        let reg = registry_with_basic_agents();
        let mut milestones: Vec<String> = Vec::new();
        for i in 0..9 {
            milestones.push(format!(
                r#"{{"label": "m{i}", "mode": "freeform", "tools": []}}"#
            ));
        }
        milestones.push(r#"{"label": "end", "mode": "final_output", "tools": []}"#.into());
        let json = format!(
            r#"{{"agent": "doc_generation", "milestones": [{}]}}"#,
            milestones.join(",")
        );
        let plan = CombinedPlanner::parse_plan(&json, &reg, &test_tool_pool())
            .expect("10 milestones should be within cap 12");
        assert_eq!(plan.milestones.len(), 10);
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
    fn parse_rejects_produce_options_without_request_user_input_in_tools() {
        // 关键 bug 修复测试：LLM 拆解时给 produce_options 写 tools=[]
        // 应该被拒绝（mode 的 RequiresToolCall 要求）
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "选结构", "mode": "produce_options", "tools": []},
                {"label": "定稿", "mode": "final_output", "tools": []}
            ]
        }"#;
        let err = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("RequiresToolCall") || msg.contains("request_user_input"),
            "expected RequiresToolCall error, got: {msg}"
        );
    }

    #[test]
    fn parse_rejects_collect_without_request_user_input_in_tools() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "了解需求", "mode": "collect", "tools": []},
                {"label": "定稿", "mode": "final_output", "tools": []}
            ]
        }"#;
        let err = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap_err();
        assert!(err.to_string().contains("request_user_input"));
    }

    #[test]
    fn parse_accepts_produce_options_with_required_tool() {
        let reg = registry_with_basic_agents();
        let json = r#"{
            "agent": "doc_generation",
            "milestones": [
                {"label": "选结构", "mode": "produce_options", "tools": ["request_user_input"]},
                {"label": "定稿", "mode": "final_output", "tools": []}
            ]
        }"#;
        let plan = CombinedPlanner::parse_plan(json, &reg, &test_tool_pool()).unwrap();
        assert_eq!(plan.milestones.len(), 2);
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
        // 错误信息含 ForbidTool 或 mode 名（错误措辞从 mode_tool_compatibility 改为 ForbidTool）
        assert!(
            err.to_string().to_lowercase().contains("forbidden")
                || err.to_string().contains("FinalOutput"),
            "expected forbidden tool error, got: {err}"
        );
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
