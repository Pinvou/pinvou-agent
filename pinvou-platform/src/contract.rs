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
