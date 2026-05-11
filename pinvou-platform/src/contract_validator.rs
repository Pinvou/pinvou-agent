use crate::contract::{MilestoneContract, OutputRequirement};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationResult {
    pub ok: bool,
    pub issues: Vec<String>,
}

impl ValidationResult {
    pub fn ok() -> Self {
        Self {
            ok: true,
            issues: vec![],
        }
    }

    pub fn fail(issue: impl Into<String>) -> Self {
        Self {
            ok: false,
            issues: vec![issue.into()],
        }
    }

    pub fn push_issue(&mut self, issue: impl Into<String>) {
        self.ok = false;
        self.issues.push(issue.into());
    }
}

pub struct ContractValidator;

impl ContractValidator {
    pub fn validate_tool_call(
        contract: &MilestoneContract,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> ValidationResult {
        let mut result = ValidationResult::ok();

        if contract
            .output_requirements
            .contains(&OutputRequirement::NoToolCall)
            || contract.forbidden_tools.iter().any(|t| t == tool_name)
            || (!contract.allowed_tools.is_empty()
                && !contract.allowed_tools.iter().any(|t| t == tool_name))
        {
            result.push_issue(format!("当前阶段不允许调用工具 {tool_name}"));
        }

        if tool_name == "request_user_input" {
            let option_counts: Vec<usize> = arguments
                .get("questions")
                .and_then(|v| v.as_array())
                .map(|questions| {
                    if questions.is_empty() {
                        vec![0]
                    } else {
                        questions
                            .iter()
                            .map(|q| {
                                q.get("options")
                                    .and_then(|v| v.as_array())
                                    .map(|a| a.len())
                                    .unwrap_or(0)
                            })
                            .collect()
                    }
                })
                .unwrap_or_else(|| vec![0]);

            for req in &contract.output_requirements {
                match req {
                    OutputRequirement::MinOptions(min) => {
                        if option_counts.iter().any(|count| *count < *min as usize) {
                            result.push_issue(format!("request_user_input 至少 {} 个选项", min));
                        }
                    }
                    OutputRequirement::MaxOptions(max) => {
                        if option_counts.iter().any(|count| *count > *max as usize) {
                            result.push_issue(format!("request_user_input 最多 {} 个选项", max));
                        }
                    }
                    _ => {}
                }
            }
        }

        result
    }

    pub fn validate_response(contract: &MilestoneContract, text: &str) -> ValidationResult {
        let mut result = ValidationResult::ok();

        for req in &contract.output_requirements {
            match req {
                OutputRequirement::MustContainTable => {
                    if !text.lines().any(|line| line.contains('|')) {
                        result.push_issue("响应必须包含对比表格");
                    }
                }
                OutputRequirement::MustContainSchedule => {
                    if !text.contains("时间")
                        && !text.contains("上午")
                        && !text.contains("下午")
                        && !text.contains(':')
                    {
                        result.push_issue("响应必须包含时间表");
                    }
                }
                OutputRequirement::MustContainRiskSection => {
                    if !text.contains("风险") && !text.contains("预案") {
                        result.push_issue("响应必须包含风险或预案");
                    }
                }
                OutputRequirement::NoOpenQuestion => {
                    let tail: String = text
                        .chars()
                        .rev()
                        .take(120)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    if tail.contains("还想")
                        || tail.contains("需要补充")
                        || tail.contains("你想")
                        || tail.contains("您想")
                    {
                        result.push_issue("响应不能以开放式问题继续收集需求");
                    }
                }
                _ => {}
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{MilestoneContract, MilestoneMode, OutputRequirement};

    #[test]
    fn produce_options_rejects_single_choice_option() {
        let contract = MilestoneContract {
            mode: MilestoneMode::ProduceOptions,
            output_requirements: vec![OutputRequirement::MinOptions(2)],
            allowed_tools: vec!["request_user_input".into()],
            ..Default::default()
        };
        let args = serde_json::json!({
            "questions": [{
                "header": "方案",
                "id": "selected_option",
                "question": "请选择方案",
                "options": [{"label": "湖畔水库", "description": "只看水库"}]
            }]
        });
        let result = ContractValidator::validate_tool_call(&contract, "request_user_input", &args);
        assert!(!result.ok);
        assert!(result.issues.iter().any(|i| i.contains("至少 2 个选项")));
    }

    #[test]
    fn produce_options_rejects_invalid_question_shapes_for_min_options() {
        let contract = MilestoneContract {
            mode: MilestoneMode::ProduceOptions,
            output_requirements: vec![OutputRequirement::MinOptions(2)],
            allowed_tools: vec!["request_user_input".into()],
            ..Default::default()
        };
        let cases = [
            ("missing questions", serde_json::json!({})),
            ("empty questions", serde_json::json!({ "questions": [] })),
            (
                "malformed questions",
                serde_json::json!({ "questions": "bad" }),
            ),
            (
                "missing options",
                serde_json::json!({ "questions": [{ "id": "target" }] }),
            ),
            (
                "malformed options",
                serde_json::json!({ "questions": [{ "id": "target", "options": "bad" }] }),
            ),
            (
                "too few options",
                serde_json::json!({
                    "questions": [{
                        "id": "target",
                        "options": [{"label": "A", "description": "Only option"}]
                    }]
                }),
            ),
        ];

        for (case_name, args) in cases {
            let result =
                ContractValidator::validate_tool_call(&contract, "request_user_input", &args);
            assert!(!result.ok, "{case_name} should fail MinOptions validation");
            assert!(
                result.issues.iter().any(|i| i.contains("至少 2 个选项")),
                "{case_name} should report MinOptions issue, got {:?}",
                result.issues
            );
        }
    }

    #[test]
    fn final_output_rejects_tool_call() {
        let contract = MilestoneContract {
            mode: MilestoneMode::FinalOutput,
            output_requirements: vec![OutputRequirement::NoToolCall],
            ..Default::default()
        };
        let result = ContractValidator::validate_tool_call(
            &contract,
            "request_user_input",
            &serde_json::json!({}),
        );
        assert!(!result.ok);
        assert!(result.issues.iter().any(|i| i.contains("不允许调用工具")));
    }

    #[test]
    fn refine_rejects_open_question_response() {
        let contract = MilestoneContract {
            mode: MilestoneMode::RefineSelectedOption,
            output_requirements: vec![OutputRequirement::NoOpenQuestion],
            ..Default::default()
        };
        let result =
            ContractValidator::validate_response(&contract, "这是细化方案。您还想补充什么需求？");
        assert!(!result.ok);
        assert!(result.issues.iter().any(|i| i.contains("开放式问题")));
    }
}
