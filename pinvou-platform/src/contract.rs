use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningMode {
    StaticOnly,
    DynamicWithStaticFallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanningConfig {
    #[serde(default = "default_planning_mode")]
    pub mode: PlanningMode,
    #[serde(default)]
    pub confirm_dynamic_plan: bool,
    #[serde(default = "default_max_plan_retries")]
    pub max_plan_retries: u8,
}

impl Default for PlanningConfig {
    fn default() -> Self {
        Self {
            mode: default_planning_mode(),
            confirm_dynamic_plan: false,
            max_plan_retries: default_max_plan_retries(),
        }
    }
}

fn default_planning_mode() -> PlanningMode {
    PlanningMode::DynamicWithStaticFallback
}

fn default_max_plan_retries() -> u8 {
    2
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneMode {
    Collect,
    ProduceOptions,
    RefineSelectedOption,
    FinalOutput,
    Freeform,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdvancePolicy {
    OnChoice,
    OnValidOutput,
    ManualContinue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputRequirement {
    MinOptions(u8),
    MaxOptions(u8),
    MustContainTable,
    MustContainSchedule,
    MustContainRiskSection,
    NoOpenQuestion,
    NoToolCall,
}

impl Serialize for OutputRequirement {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_config_value())
    }
}

impl OutputRequirement {
    fn as_config_value(&self) -> String {
        match self {
            OutputRequirement::MinOptions(value) => format!("min_options:{value}"),
            OutputRequirement::MaxOptions(value) => format!("max_options:{value}"),
            OutputRequirement::MustContainTable => "must_contain_table".to_string(),
            OutputRequirement::MustContainSchedule => "must_contain_schedule".to_string(),
            OutputRequirement::MustContainRiskSection => "must_contain_risk_section".to_string(),
            OutputRequirement::NoOpenQuestion => "no_open_question".to_string(),
            OutputRequirement::NoToolCall => "no_tool_call".to_string(),
        }
    }
}

impl<'de> Deserialize<'de> for OutputRequirement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        parse_output_requirement(&raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MilestoneContract {
    #[serde(default = "default_milestone_mode")]
    pub mode: MilestoneMode,
    #[serde(default = "default_question_budget")]
    pub question_budget: u8,
    #[serde(default)]
    pub required_context: Vec<String>,
    #[serde(default)]
    pub produced_context: Vec<String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub forbidden_tools: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_output_requirements")]
    pub output_requirements: Vec<OutputRequirement>,
    #[serde(default = "default_advance_policy")]
    pub advance_policy: AdvancePolicy,
}

impl Default for MilestoneContract {
    fn default() -> Self {
        Self {
            mode: default_milestone_mode(),
            question_budget: default_question_budget(),
            required_context: Vec::new(),
            produced_context: Vec::new(),
            allowed_tools: Vec::new(),
            forbidden_tools: Vec::new(),
            output_requirements: Vec::new(),
            advance_policy: default_advance_policy(),
        }
    }
}

fn default_milestone_mode() -> MilestoneMode {
    MilestoneMode::Collect
}

fn default_question_budget() -> u8 {
    1
}

fn default_advance_policy() -> AdvancePolicy {
    AdvancePolicy::ManualContinue
}

/// 根据 mode 返回内置默认 contract（新设计：mode → 规则映射在代码里硬编码）。
///
/// 注意：返回的 `allowed_tools` 留空。新设计中 `allowed_tools` 由 LLM 拆解时挑选，
/// 但会被 mode 兼容性规则（`mode_tool_compatibility`）约束。
pub fn contract_for_mode(mode: MilestoneMode) -> MilestoneContract {
    match mode {
        MilestoneMode::Collect => MilestoneContract {
            mode,
            question_budget: 1,
            allowed_tools: Vec::new(),
            output_requirements: Vec::new(),
            advance_policy: AdvancePolicy::OnChoice,
            ..MilestoneContract::default()
        },
        MilestoneMode::ProduceOptions => MilestoneContract {
            mode,
            question_budget: 1,
            allowed_tools: Vec::new(),
            output_requirements: vec![
                OutputRequirement::MinOptions(2),
                OutputRequirement::MaxOptions(3),
            ],
            advance_policy: AdvancePolicy::OnChoice,
            ..MilestoneContract::default()
        },
        MilestoneMode::RefineSelectedOption => MilestoneContract {
            mode,
            question_budget: 0,
            allowed_tools: Vec::new(),
            output_requirements: vec![OutputRequirement::NoOpenQuestion],
            advance_policy: AdvancePolicy::OnValidOutput,
            ..MilestoneContract::default()
        },
        MilestoneMode::Freeform => MilestoneContract {
            mode,
            question_budget: 1,
            allowed_tools: Vec::new(),
            output_requirements: vec![OutputRequirement::NoOpenQuestion],
            advance_policy: AdvancePolicy::OnValidOutput,
            ..MilestoneContract::default()
        },
        MilestoneMode::FinalOutput => MilestoneContract {
            mode,
            question_budget: 0,
            allowed_tools: Vec::new(),
            output_requirements: vec![OutputRequirement::NoOpenQuestion],
            advance_policy: AdvancePolicy::OnValidOutput,
            ..MilestoneContract::default()
        },
    }
}

/// mode 与工具的硬兼容性检查。
///
/// 返回 None 表示兼容；Some(reason) 表示拒绝。
///
/// 规则：
/// - `final_output` 禁止 `request_user_input`（最终输出阶段不能再问用户）
/// - 其他 mode 当前无硬限制
pub fn mode_tool_compatibility(mode: MilestoneMode, tool: &str) -> Option<String> {
    if matches!(mode, MilestoneMode::FinalOutput) && tool == "request_user_input" {
        return Some(format!(
            "final_output 阶段禁止使用 {tool}（最终输出不能再向用户提问）"
        ));
    }
    None
}

/// 全局工具池：所有可用工具的白名单。
pub const GLOBAL_TOOL_POOL: &[&str] = &[
    "request_user_input",
    "file_read",
    "file_write",
    "web_search",
    "python_exec",
];

/// 工具是否在全局白名单内
pub fn is_tool_in_global_pool(tool: &str) -> bool {
    GLOBAL_TOOL_POOL.contains(&tool)
}

pub fn default_contract_for_label(label: &str) -> MilestoneContract {
    if label.contains("方案") || label.contains("对比") {
        MilestoneContract {
            mode: MilestoneMode::ProduceOptions,
            question_budget: 1,
            allowed_tools: vec!["request_user_input".to_string()],
            output_requirements: vec![
                OutputRequirement::MinOptions(2),
                OutputRequirement::MaxOptions(3),
                OutputRequirement::MustContainTable,
                OutputRequirement::NoOpenQuestion,
            ],
            advance_policy: AdvancePolicy::OnChoice,
            ..MilestoneContract::default()
        }
    } else if label.contains("细化") {
        MilestoneContract {
            mode: MilestoneMode::RefineSelectedOption,
            question_budget: 0,
            output_requirements: vec![
                OutputRequirement::MustContainSchedule,
                OutputRequirement::MustContainRiskSection,
                OutputRequirement::NoOpenQuestion,
            ],
            advance_policy: AdvancePolicy::OnValidOutput,
            ..MilestoneContract::default()
        }
    } else if label.contains("输出") || label.contains("定稿") {
        MilestoneContract {
            mode: MilestoneMode::FinalOutput,
            question_budget: 0,
            output_requirements: vec![OutputRequirement::NoToolCall],
            advance_policy: AdvancePolicy::OnValidOutput,
            ..MilestoneContract::default()
        }
    } else {
        MilestoneContract {
            mode: MilestoneMode::Collect,
            question_budget: 1,
            allowed_tools: vec!["request_user_input".to_string()],
            advance_policy: AdvancePolicy::OnChoice,
            ..MilestoneContract::default()
        }
    }
}

pub fn parse_output_requirement(raw: &str) -> Result<OutputRequirement, String> {
    match raw {
        "must_contain_table" => Ok(OutputRequirement::MustContainTable),
        "must_contain_schedule" => Ok(OutputRequirement::MustContainSchedule),
        "must_contain_risk_section" => Ok(OutputRequirement::MustContainRiskSection),
        "no_open_question" => Ok(OutputRequirement::NoOpenQuestion),
        "no_tool_call" => Ok(OutputRequirement::NoToolCall),
        _ if raw.starts_with("min_options:") => {
            parse_u8_suffix(raw, "min_options:").map(OutputRequirement::MinOptions)
        }
        _ if raw.starts_with("max_options:") => {
            parse_u8_suffix(raw, "max_options:").map(OutputRequirement::MaxOptions)
        }
        _ => Err(format!("unknown output requirement: {raw}")),
    }
}

fn parse_u8_suffix(raw: &str, prefix: &str) -> Result<u8, String> {
    raw.strip_prefix(prefix)
        .unwrap_or_default()
        .parse::<u8>()
        .map_err(|err| format!("invalid output requirement {raw}: {err}"))
}

pub fn deserialize_output_requirements<'de, D>(
    deserializer: D,
) -> Result<Vec<OutputRequirement>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<String>::deserialize(deserializer)?;
    values
        .iter()
        .map(|raw| parse_output_requirement(raw).map_err(serde::de::Error::custom))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_for_mode_collect_has_question_budget_one_and_on_choice() {
        let c = contract_for_mode(MilestoneMode::Collect);
        assert_eq!(c.question_budget, 1);
        assert_eq!(c.advance_policy, AdvancePolicy::OnChoice);
        assert!(c.output_requirements.is_empty());
    }

    #[test]
    fn contract_for_mode_produce_options_requires_2_to_3_options() {
        let c = contract_for_mode(MilestoneMode::ProduceOptions);
        assert!(c.output_requirements.contains(&OutputRequirement::MinOptions(2)));
        assert!(c.output_requirements.contains(&OutputRequirement::MaxOptions(3)));
        assert_eq!(c.advance_policy, AdvancePolicy::OnChoice);
    }

    #[test]
    fn contract_for_mode_final_output_no_budget_no_open_question() {
        let c = contract_for_mode(MilestoneMode::FinalOutput);
        assert_eq!(c.question_budget, 0);
        assert!(c.output_requirements.contains(&OutputRequirement::NoOpenQuestion));
        assert_eq!(c.advance_policy, AdvancePolicy::OnValidOutput);
    }

    #[test]
    fn mode_tool_compatibility_blocks_request_input_in_final_output() {
        let res = mode_tool_compatibility(MilestoneMode::FinalOutput, "request_user_input");
        assert!(res.is_some());
        assert!(res.unwrap().contains("final_output"));
    }

    #[test]
    fn mode_tool_compatibility_allows_request_input_in_collect() {
        let res = mode_tool_compatibility(MilestoneMode::Collect, "request_user_input");
        assert!(res.is_none());
    }

    #[test]
    fn mode_tool_compatibility_allows_file_write_in_final_output() {
        let res = mode_tool_compatibility(MilestoneMode::FinalOutput, "file_write");
        assert!(res.is_none());
    }

    #[test]
    fn global_tool_pool_contains_expected_tools() {
        assert!(is_tool_in_global_pool("request_user_input"));
        assert!(is_tool_in_global_pool("file_write"));
        assert!(is_tool_in_global_pool("python_exec"));
        assert!(!is_tool_in_global_pool("rm_rf"));
        assert!(!is_tool_in_global_pool(""));
    }
}
