//! **LEGACY**：旧版动态拆解器，要求严格复用 app.toml 模板的 id 和 mode。
//!
//! 已被 [`crate::combined_planner::CombinedPlanner`] 替代。后者一次 LLM 调用
//! 同时输出 agent + milestones，不受静态模板约束。
//!
//! 仍保留是因为 `engine::ensure_plan_initialized` 在 legacy 路径上仍使用它
//! （当 AgentRegistry 未注入时）。P1 删除。

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashSet;

use crate::app::{AppConfig, Milestone};
use crate::contract::MilestoneMode;

#[deprecated(note = "use CombinedPlanner instead; this is P1 legacy")]
pub struct DynamicPlanner;

#[derive(Debug, Deserialize)]
struct DynamicPlanDto {
    milestones: Vec<DynamicMilestoneDto>,
}

#[derive(Debug, Deserialize)]
struct DynamicMilestoneDto {
    id: String,
    label: String,
    #[serde(default)]
    prompt_hint: Option<String>,
    #[serde(default)]
    contract_mode: Option<MilestoneMode>,
    #[serde(default)]
    required_context: Vec<String>,
    #[serde(default)]
    produced_context: Vec<String>,
}

#[allow(deprecated)]
impl DynamicPlanner {
    pub fn build_prompt(user_message: &str, app: &AppConfig) -> String {
        let templates = app
            .milestones
            .iter()
            .map(|m| {
                format!(
                    "- id={} label={} mode={}",
                    m.id,
                    m.label,
                    milestone_mode_name(&m.contract.mode)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "用户需求: {user_message}\n当前应用: {} -- {}\n\n基于以下静态模板生成动态里程碑：必须完整输出每个模板 id，顺序必须一致，不能新增、删除或重排 id；contract_mode 必须与模板 mode 完全一致。\n{templates}\n\n输出 JSON: {{\"milestones\":[{{\"id\":\"...\",\"label\":\"...\",\"prompt_hint\":\"...\",\"contract_mode\":\"collect|produce_options|refine_selected_option|final_output|freeform\",\"required_context\":[],\"produced_context\":[]}}]}}",
            app.name, app.description
        )
    }

    pub fn parse_plan(text: &str, app: &AppConfig) -> Result<Vec<Milestone>> {
        let json = extract_json_object(text).context("dynamic plan response has no JSON object")?;
        let dto: DynamicPlanDto =
            serde_json::from_str(json).context("failed to parse dynamic plan JSON")?;
        let mut output = Vec::new();
        let mut seen_ids = HashSet::new();
        for item in &dto.milestones {
            if !seen_ids.insert(item.id.clone()) {
                anyhow::bail!("duplicate milestone id {}", item.id);
            }
            if item.label.trim().is_empty() {
                anyhow::bail!("blank milestone label for {}", item.id);
            }
        }
        if dto.milestones.len() != app.milestones.len() {
            anyhow::bail!("dynamic plan must include all template milestones in order");
        }
        for (idx, item) in dto.milestones.into_iter().enumerate() {
            let template = app
                .milestones
                .get(idx)
                .with_context(|| format!("dynamic milestone references unknown index {idx}"))?;
            if item.id != template.id {
                anyhow::bail!("dynamic plan must include all template milestones in order");
            }
            let template = app
                .milestones
                .iter()
                .find(|m| m.id == item.id)
                .with_context(|| format!("dynamic milestone references unknown id {}", item.id))?;
            let mut milestone = template.clone();
            milestone.label = item.label;
            milestone.prompt_hint = item.prompt_hint.or(milestone.prompt_hint);
            if let Some(mode) = item.contract_mode {
                if mode != template.contract.mode {
                    anyhow::bail!("contract_mode mismatch for {}", item.id);
                }
            }
            if !item.required_context.is_empty() {
                milestone.contract.required_context = item.required_context;
            }
            if !item.produced_context.is_empty() {
                milestone.contract.produced_context = item.produced_context;
            }
            output.push(milestone);
        }
        if output.is_empty() {
            anyhow::bail!("dynamic plan has no milestones");
        }
        Ok(output)
    }
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    Some(&text[start..=end])
}

fn milestone_mode_name(mode: &MilestoneMode) -> &'static str {
    match mode {
        MilestoneMode::Collect => "collect",
        MilestoneMode::ProduceOptions => "produce_options",
        MilestoneMode::RefineSelectedOption => "refine_selected_option",
        MilestoneMode::FinalOutput => "final_output",
        MilestoneMode::Freeform => "freeform",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppConfig, Milestone};
    use crate::contract::{MilestoneContract, MilestoneMode};

    fn app_template() -> AppConfig {
        AppConfig {
            id: "计划敲定".into(),
            name: "计划敲定".into(),
            description: "测试".into(),
            icon: "*".into(),
            prompt_file: None,
            prompt: None,
            model_preference: "large".into(),
            tools: vec!["request_user_input".into()],
            milestones: vec![Milestone {
                id: "goal".into(),
                label: "明确目标".into(),
                prompt_hint: Some("确认目标".into()),
                icon: None,
                contract: MilestoneContract {
                    mode: MilestoneMode::Collect,
                    ..Default::default()
                },
                ..Default::default()
            }],
            planning: Default::default(),
            granularity: Some("fine".into()),
            confirm_at: None,
            ban_list: vec![],
            meta: Default::default(),
        }
    }

    fn app_template_with_two_milestones() -> AppConfig {
        let mut app = app_template();
        app.milestones.push(Milestone {
            id: "options".into(),
            label: "方案对比".into(),
            prompt_hint: Some("给出方案".into()),
            icon: None,
            contract: MilestoneContract {
                mode: MilestoneMode::ProduceOptions,
                ..Default::default()
            },
            ..Default::default()
        });
        app
    }

    #[test]
    fn parse_valid_dynamic_plan_specializes_static_template() {
        let json = r#"{
          "milestones": [
            {
              "id": "goal",
              "label": "明确水生水库徒步目标",
              "prompt_hint": "确认徒步强度、兴趣和时长",
              "contract_mode": "collect",
              "produced_context": ["interest", "duration", "intensity"]
            }
          ]
        }"#;
        let plan = DynamicPlanner::parse_plan(json, &app_template()).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].id, "goal");
        assert_eq!(plan[0].label, "明确水生水库徒步目标");
        assert_eq!(plan[0].contract.mode, MilestoneMode::Collect);
        assert_eq!(
            plan[0].contract.produced_context,
            vec!["interest", "duration", "intensity"]
        );
    }

    #[test]
    fn parse_dynamic_plan_rejects_duplicate_milestone_ids() {
        let json = r#"{
          "milestones": [
            {
              "id": "goal",
              "label": "明确目标一"
            },
            {
              "id": "goal",
              "label": "明确目标二"
            }
          ]
        }"#;

        let err = DynamicPlanner::parse_plan(json, &app_template()).unwrap_err();

        assert!(err.to_string().contains("duplicate milestone id goal"));
    }

    #[test]
    fn parse_dynamic_plan_rejects_blank_milestone_label() {
        let json = r#"{
          "milestones": [
            {
              "id": "goal",
              "label": "   "
            }
          ]
        }"#;

        let err = DynamicPlanner::parse_plan(json, &app_template()).unwrap_err();

        assert!(err.to_string().contains("blank milestone label for goal"));
    }

    #[test]
    fn parse_dynamic_plan_rejects_contract_mode_mismatch() {
        let json = r#"{
          "milestones": [
            {
              "id": "goal",
              "label": "明确目标",
              "contract_mode": "final_output"
            }
          ]
        }"#;

        let err = DynamicPlanner::parse_plan(json, &app_template()).unwrap_err();

        assert!(err.to_string().contains("contract_mode mismatch for goal"));
    }

    #[test]
    fn parse_dynamic_plan_rejects_missing_or_reordered_template_milestones() {
        let json = r#"{
          "milestones": [
            {
              "id": "options",
              "label": "方案对比",
              "contract_mode": "produce_options"
            }
          ]
        }"#;

        let err =
            DynamicPlanner::parse_plan(json, &app_template_with_two_milestones()).unwrap_err();

        assert!(
            err.to_string()
                .contains("must include all template milestones in order")
        );
    }

    #[test]
    fn build_prompt_renders_template_modes_as_snake_case() {
        let mut app = app_template();
        app.milestones[0].contract.mode = MilestoneMode::ProduceOptions;

        let prompt = DynamicPlanner::build_prompt("安排一次周末活动", &app);

        assert!(prompt.contains("mode=produce_options"));
        assert!(!prompt.contains("mode=ProduceOptions"));
    }

    #[test]
    fn invalid_dynamic_plan_falls_back_to_static_template() {
        let plan = DynamicPlanner::parse_plan("not json", &app_template())
            .unwrap_or_else(|_| app_template().milestones);
        assert_eq!(plan[0].label, "明确目标");
    }
}
