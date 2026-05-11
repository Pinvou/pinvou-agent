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

/// 阶段输出约束 —— 由 Mode 自动挂载，违反时代码会拦截。
///
/// 注意：领域性的内容约束（"必须含成本/风险对比表"等）**不在这里**。
/// 那些是 agent 软建议，写在 `prompts/<id>.md` 正文里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputRequirement {
    /// `request_user_input` 的 questions[i].options 数量下限
    MinOptions(u8),
    /// `request_user_input` 的 questions[i].options 数量上限
    MaxOptions(u8),
    /// 响应末尾不能是开放式问句（以 ? 或 ？ 结尾）
    NoOpenQuestion,
    /// 本阶段完全禁止任何工具调用
    NoToolCall,
    /// 本阶段必须调用指定工具（如 collect 必须调 request_user_input）
    RequiresToolCall(String),
    /// 本阶段禁止调用指定工具（如 final_output 禁 request_user_input）
    ForbidTool(String),
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
            OutputRequirement::NoOpenQuestion => "no_open_question".to_string(),
            OutputRequirement::NoToolCall => "no_tool_call".to_string(),
            OutputRequirement::RequiresToolCall(name) => format!("requires_tool_call:{name}"),
            OutputRequirement::ForbidTool(name) => format!("forbid_tool:{name}"),
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

/// 根据 mode 返回内置默认 contract（mode → 硬规则映射，写在代码里）。
///
/// 返回的 `allowed_tools` 留空：由 LLM 拆解时选具体工具，
/// `output_requirements` 中的 `ForbidTool` 在校验阶段拦截违规。
pub fn contract_for_mode(mode: MilestoneMode) -> MilestoneContract {
    match mode {
        // 收信息：必须用 request_user_input 让用户做选择题；不能开放追问
        MilestoneMode::Collect => MilestoneContract {
            mode,
            question_budget: 1,
            allowed_tools: Vec::new(),
            output_requirements: vec![
                OutputRequirement::RequiresToolCall("request_user_input".to_string()),
                OutputRequirement::NoOpenQuestion,
            ],
            advance_policy: AdvancePolicy::OnChoice,
            ..MilestoneContract::default()
        },
        // 出方案：必须用 request_user_input 给 2-3 个选项；不能开放追问
        MilestoneMode::ProduceOptions => MilestoneContract {
            mode,
            question_budget: 1,
            allowed_tools: Vec::new(),
            output_requirements: vec![
                OutputRequirement::RequiresToolCall("request_user_input".to_string()),
                OutputRequirement::MinOptions(2),
                OutputRequirement::MaxOptions(3),
                OutputRequirement::NoOpenQuestion,
            ],
            advance_policy: AdvancePolicy::OnChoice,
            ..MilestoneContract::default()
        },
        // 细化已选方案：不能再问，基于现有上下文产出
        MilestoneMode::RefineSelectedOption => MilestoneContract {
            mode,
            question_budget: 0,
            allowed_tools: Vec::new(),
            output_requirements: vec![OutputRequirement::NoOpenQuestion],
            advance_policy: AdvancePolicy::OnValidOutput,
            ..MilestoneContract::default()
        },
        // 自由产出：纯产出阶段；要确认偏好就拆 collect
        MilestoneMode::Freeform => MilestoneContract {
            mode,
            question_budget: 0,
            allowed_tools: Vec::new(),
            output_requirements: vec![OutputRequirement::NoOpenQuestion],
            advance_policy: AdvancePolicy::OnValidOutput,
            ..MilestoneContract::default()
        },
        // 最终输出：禁问用户、禁开放问题
        MilestoneMode::FinalOutput => MilestoneContract {
            mode,
            question_budget: 0,
            allowed_tools: Vec::new(),
            output_requirements: vec![
                OutputRequirement::ForbidTool("request_user_input".to_string()),
                OutputRequirement::NoOpenQuestion,
            ],
            advance_policy: AdvancePolicy::OnValidOutput,
            ..MilestoneContract::default()
        },
    }
}

/// 全局工具池：所有可能用到的工具名（用作设计意图文档 / 拼写防呆参考）。
///
/// 运行时的权威工具列表来自 `AgentHarness::tools()`，由 `engine_factory` 注册
/// （含 DeepSeek-TUI 真实可执行的工具，如 `web_search`、`exec_shell` 等）。
/// `combined_planner` 不再用本常量做校验，而是接收 `available_tools: &[String]`
/// 参数对照实际 harness 注册情况。
pub const GLOBAL_TOOL_POOL: &[&str] = &[
    "request_user_input",
    "web_search",
    "fetch_url",
    "read_file",
    "write_file",
    "edit_file",
    "list_dir",
    "grep_files",
    "file_search",
    "exec_shell",
];

/// 工具是否在全局白名单内（用于拼写防呆，不是权威校验）。
pub fn is_tool_in_global_pool(tool: &str) -> bool {
    GLOBAL_TOOL_POOL.contains(&tool)
}

pub fn parse_output_requirement(raw: &str) -> Result<OutputRequirement, String> {
    match raw {
        "no_open_question" => Ok(OutputRequirement::NoOpenQuestion),
        "no_tool_call" => Ok(OutputRequirement::NoToolCall),
        _ if raw.starts_with("min_options:") => {
            parse_u8_suffix(raw, "min_options:").map(OutputRequirement::MinOptions)
        }
        _ if raw.starts_with("max_options:") => {
            parse_u8_suffix(raw, "max_options:").map(OutputRequirement::MaxOptions)
        }
        _ if raw.starts_with("requires_tool_call:") => Ok(OutputRequirement::RequiresToolCall(
            raw.trim_start_matches("requires_tool_call:").to_string(),
        )),
        _ if raw.starts_with("forbid_tool:") => Ok(OutputRequirement::ForbidTool(
            raw.trim_start_matches("forbid_tool:").to_string(),
        )),
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
    fn contract_for_mode_collect_requires_user_input_and_no_open_question() {
        let c = contract_for_mode(MilestoneMode::Collect);
        assert_eq!(c.question_budget, 1);
        assert_eq!(c.advance_policy, AdvancePolicy::OnChoice);
        assert!(c.output_requirements.contains(&OutputRequirement::RequiresToolCall(
            "request_user_input".to_string()
        )));
        assert!(c.output_requirements.contains(&OutputRequirement::NoOpenQuestion));
    }

    #[test]
    fn contract_for_mode_produce_options_full_constraints() {
        let c = contract_for_mode(MilestoneMode::ProduceOptions);
        assert!(c.output_requirements.contains(&OutputRequirement::MinOptions(2)));
        assert!(c.output_requirements.contains(&OutputRequirement::MaxOptions(3)));
        assert!(c.output_requirements.contains(&OutputRequirement::RequiresToolCall(
            "request_user_input".to_string()
        )));
        assert!(c.output_requirements.contains(&OutputRequirement::NoOpenQuestion));
        assert_eq!(c.advance_policy, AdvancePolicy::OnChoice);
    }

    #[test]
    fn contract_for_mode_final_output_forbids_user_input() {
        let c = contract_for_mode(MilestoneMode::FinalOutput);
        assert_eq!(c.question_budget, 0);
        assert!(c.output_requirements.contains(&OutputRequirement::ForbidTool(
            "request_user_input".to_string()
        )));
        assert!(c.output_requirements.contains(&OutputRequirement::NoOpenQuestion));
        assert_eq!(c.advance_policy, AdvancePolicy::OnValidOutput);
    }

    #[test]
    fn contract_for_mode_freeform_no_question_budget() {
        let c = contract_for_mode(MilestoneMode::Freeform);
        assert_eq!(c.question_budget, 0, "freeform 是纯产出阶段，不允许提问");
        assert_eq!(c.advance_policy, AdvancePolicy::OnValidOutput);
    }

    #[test]
    fn parse_output_requirement_handles_new_variants() {
        assert_eq!(
            parse_output_requirement("requires_tool_call:request_user_input").unwrap(),
            OutputRequirement::RequiresToolCall("request_user_input".to_string())
        );
        assert_eq!(
            parse_output_requirement("forbid_tool:exec_shell").unwrap(),
            OutputRequirement::ForbidTool("exec_shell".to_string())
        );
    }

    #[test]
    fn global_tool_pool_contains_expected_tools() {
        assert!(is_tool_in_global_pool("request_user_input"));
        assert!(is_tool_in_global_pool("write_file"));
        assert!(is_tool_in_global_pool("exec_shell"));
        assert!(!is_tool_in_global_pool("rm_rf"));
        assert!(!is_tool_in_global_pool(""));
    }
}
