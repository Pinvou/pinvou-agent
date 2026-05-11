//! ContractValidator —— mode 硬规则的代码侧校验。
//!
//! 三类校验：
//! 1. `validate_tool_call`：LLM 试图调用工具时即时校验（白名单 / 禁用 / 选项数）
//! 2. `validate_response`：流式结束后校验响应文本（NoOpenQuestion 等）
//! 3. `validate_stage_completion`：流式结束后校验「应当调过的工具是否真调了」（RequiresToolCall）

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
    /// 工具调用即时校验：是否在白名单 / 是否被 ForbidTool / 选项数是否合法。
    pub fn validate_tool_call(
        contract: &MilestoneContract,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> ValidationResult {
        let mut result = ValidationResult::ok();

        // 黑白名单
        let allowed_explicit = !contract.allowed_tools.is_empty();
        let in_allowed = contract.allowed_tools.iter().any(|t| t == tool_name);
        let in_forbidden = contract.forbidden_tools.iter().any(|t| t == tool_name);
        let global_no_tool = contract
            .output_requirements
            .contains(&OutputRequirement::NoToolCall);

        if global_no_tool || in_forbidden || (allowed_explicit && !in_allowed) {
            result.push_issue(format!("当前阶段不允许调用工具 {tool_name}"));
        }

        // ForbidTool（mode 内置的禁用工具）
        for req in &contract.output_requirements {
            if let OutputRequirement::ForbidTool(forbidden) = req {
                if forbidden == tool_name {
                    result.push_issue(format!(
                        "本阶段（{:?}）禁止使用工具 {tool_name}",
                        contract.mode
                    ));
                }
            }
        }

        // request_user_input 选项数校验
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

    /// 响应文本校验（流式结束后）。当前仅 NoOpenQuestion，用「末尾是否问号」判定。
    pub fn validate_response(contract: &MilestoneContract, text: &str) -> ValidationResult {
        let mut result = ValidationResult::ok();

        for req in &contract.output_requirements {
            if matches!(req, OutputRequirement::NoOpenQuestion) && ends_with_question(text) {
                result.push_issue("响应末尾不能是开放式问题（不能以问号结尾）");
            }
        }

        result
    }

    /// 阶段完成校验：检查 RequiresToolCall 是否满足。
    ///
    /// `invoked_tools` 是本阶段流式过程中 LLM 实际调用过的工具名（重复也只算一次）。
    pub fn validate_stage_completion(
        contract: &MilestoneContract,
        text: &str,
        invoked_tools: &[String],
    ) -> ValidationResult {
        let mut result = Self::validate_response(contract, text);

        for req in &contract.output_requirements {
            if let OutputRequirement::RequiresToolCall(required) = req {
                if !invoked_tools.iter().any(|t| t == required) {
                    result.push_issue(format!(
                        "本阶段必须调用 `{required}` 工具，但未观察到调用"
                    ));
                }
            }
        }

        result
    }
}

/// 末尾是否以问号结尾（忽略尾部空白和换行）。覆盖中英问号。
fn ends_with_question(text: &str) -> bool {
    let trimmed = text.trim_end();
    matches!(trimmed.chars().last(), Some('?') | Some('？'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{MilestoneContract, MilestoneMode, OutputRequirement};

    fn contract_with(
        mode: MilestoneMode,
        reqs: Vec<OutputRequirement>,
        allowed_tools: Vec<String>,
    ) -> MilestoneContract {
        MilestoneContract {
            mode,
            output_requirements: reqs,
            allowed_tools,
            ..Default::default()
        }
    }

    // === validate_tool_call ===

    #[test]
    fn produce_options_rejects_single_choice_option() {
        let contract = contract_with(
            MilestoneMode::ProduceOptions,
            vec![OutputRequirement::MinOptions(2)],
            vec!["request_user_input".into()],
        );
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
        let contract = contract_with(
            MilestoneMode::ProduceOptions,
            vec![OutputRequirement::MinOptions(2)],
            vec!["request_user_input".into()],
        );
        let cases = [
            serde_json::json!({}),
            serde_json::json!({ "questions": [] }),
            serde_json::json!({ "questions": "bad" }),
            serde_json::json!({ "questions": [{ "id": "x" }] }),
            serde_json::json!({ "questions": [{ "id": "x", "options": "bad" }] }),
        ];
        for args in cases {
            let result =
                ContractValidator::validate_tool_call(&contract, "request_user_input", &args);
            assert!(!result.ok);
            assert!(result.issues.iter().any(|i| i.contains("至少 2 个选项")));
        }
    }

    #[test]
    fn forbid_tool_rejects_request_user_input_in_final_output() {
        let contract = contract_with(
            MilestoneMode::FinalOutput,
            vec![OutputRequirement::ForbidTool("request_user_input".into())],
            vec![],
        );
        let result = ContractValidator::validate_tool_call(
            &contract,
            "request_user_input",
            &serde_json::json!({}),
        );
        assert!(!result.ok);
        assert!(result.issues.iter().any(|i| i.contains("禁止使用工具")));
    }

    #[test]
    fn no_tool_call_blocks_any_tool() {
        let contract = contract_with(
            MilestoneMode::FinalOutput,
            vec![OutputRequirement::NoToolCall],
            vec![],
        );
        let result = ContractValidator::validate_tool_call(
            &contract,
            "anything",
            &serde_json::json!({}),
        );
        assert!(!result.ok);
        assert!(result.issues.iter().any(|i| i.contains("不允许调用工具")));
    }

    // === validate_response (NoOpenQuestion) ===

    #[test]
    fn no_open_question_blocks_response_ending_with_question_mark() {
        let contract = contract_with(
            MilestoneMode::Freeform,
            vec![OutputRequirement::NoOpenQuestion],
            vec![],
        );
        for case in [
            "这是细化方案。您还想补充什么需求？",
            "已经写完了。需要再调整吗？\n",
            "需要再问一句?",
        ] {
            let result = ContractValidator::validate_response(&contract, case);
            assert!(!result.ok, "case 应该失败: {case}");
            assert!(result.issues.iter().any(|i| i.contains("开放")));
        }
    }

    #[test]
    fn no_open_question_passes_when_response_ends_with_period() {
        let contract = contract_with(
            MilestoneMode::Freeform,
            vec![OutputRequirement::NoOpenQuestion],
            vec![],
        );
        let result =
            ContractValidator::validate_response(&contract, "这是细化方案。已经完整。");
        assert!(result.ok);
    }

    #[test]
    fn no_open_question_ignores_middle_question_marks() {
        // 现在只看末尾。中间有问号但末尾是句号 → 通过。
        // 这是 trade-off：宁可放过中间，也不误伤。
        let contract = contract_with(
            MilestoneMode::Freeform,
            vec![OutputRequirement::NoOpenQuestion],
            vec![],
        );
        let result = ContractValidator::validate_response(
            &contract,
            "这有三种方案可选。方案A是什么？答：徒步。方案B是什么？答：拍照。",
        );
        assert!(result.ok);
    }

    // === validate_stage_completion (RequiresToolCall) ===

    #[test]
    fn requires_tool_call_passes_when_tool_was_invoked() {
        let contract = contract_with(
            MilestoneMode::Collect,
            vec![OutputRequirement::RequiresToolCall("request_user_input".into())],
            vec!["request_user_input".into()],
        );
        let result = ContractValidator::validate_stage_completion(
            &contract,
            "已收集偏好。",
            &["request_user_input".to_string()],
        );
        assert!(result.ok);
    }

    #[test]
    fn requires_tool_call_fails_when_tool_not_invoked() {
        let contract = contract_with(
            MilestoneMode::Collect,
            vec![OutputRequirement::RequiresToolCall("request_user_input".into())],
            vec!["request_user_input".into()],
        );
        let result = ContractValidator::validate_stage_completion(
            &contract,
            "我直接用文字问：你想去几天？",
            &[], // 没调任何工具
        );
        assert!(!result.ok);
        // 既触发 RequiresToolCall 也触发 NoOpenQuestion（如果挂了）
        assert!(result
            .issues
            .iter()
            .any(|i| i.contains("必须调用 `request_user_input`")));
    }
}
