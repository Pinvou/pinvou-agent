use anyhow::Result;

use crate::workflow::Milestone;
use crate::contract::{MilestoneMode, OutputRequirement};
use crate::workflow::ConversationState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceRequest {
    pub call_id: String,
    pub questions: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractPrompt {
    pub milestone_id: String,
    pub user_message: String,
    pub allowed_tools: Vec<String>,
    pub system_requirements: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnDirective {
    AskUser(ChoiceRequest),
    CallLlm(ContractPrompt),
    CompleteStep(String),
    Blocked(String),
}

pub struct ContractRuntime;

impl ContractRuntime {
    pub fn next_directive(
        milestone: &Milestone,
        state: &ConversationState,
        user_message: &str,
    ) -> Result<TurnDirective> {
        let mut requirements = vec![format!("当前阶段：{}", milestone.label)];
        let contract = &milestone.contract;

        match contract.mode {
            MilestoneMode::Collect => {
                if question_budget_reached(milestone, state) {
                    return Ok(TurnDirective::Blocked(question_budget_message(milestone)));
                }
                requirements.push(format!(
                    "最多调用 request_user_input {} 次；收到选择后立即总结并完成阶段。",
                    contract.question_budget
                ));
                push_request_user_input_quality_rules(&mut requirements);
            }
            MilestoneMode::ProduceOptions => {
                if question_budget_reached(milestone, state) {
                    return Ok(TurnDirective::Blocked(question_budget_message(milestone)));
                }
                requirements
                    .push("必须先给出 2-3 个可选方案，并包含成本、时间、风险、收益对比。".into());
                requirements.push(
                    "可以调用 request_user_input 让用户选择一个方案，但选项数量必须是 2-3 个。"
                        .into(),
                );
                push_request_user_input_quality_rules(&mut requirements);
            }
            MilestoneMode::RefineSelectedOption => {
                requirements.push(
                    "根据当前阶段目标和已知上下文完成本阶段产出，不能继续收集目标类信息。".into(),
                );
                push_stage_hint(&mut requirements, milestone);
                push_output_requirements(&mut requirements, &contract.output_requirements);
            }
            MilestoneMode::FinalOutput => {
                if contract
                    .output_requirements
                    .contains(&OutputRequirement::NoToolCall)
                    || contract.allowed_tools.is_empty()
                {
                    requirements.push("输出最终 markdown 文档，不要再提问，不要调用工具。".into());
                } else {
                    requirements.push(format!(
                        "输出最终 markdown 文档，不要再提问；如需完成最终输出、导出或保存，只能调用契约允许的工具：{}。",
                        contract.allowed_tools.join(", ")
                    ));
                }
            }
            MilestoneMode::Freeform => {
                requirements.push("按当前阶段目标完成任务。".into());
                push_stage_hint(&mut requirements, milestone);
                push_output_requirements(&mut requirements, &contract.output_requirements);
            }
            MilestoneMode::Review => {
                if question_budget_reached(milestone, state) {
                    return Ok(TurnDirective::Blocked(question_budget_message(milestone)));
                }
                requirements.push(
                    "这是产物审核阶段。基于上一阶段的最终产出，让用户决定是否接受或调整。".into(),
                );
                requirements.push(
                    "硬规则：必须调用 request_user_input 给用户做选择题，选项 2-4 个。".into(),
                );
                requirements.push(
                    "选项必须包含：① 一个「满意，结束」选项；② 1-2 个最可能的微调方向（预判用户可能想改什么，比如「调整时间安排」「换成自驾路线」「补充雨天预案」）；③ 一个「不满意，重新规划」选项。".into(),
                );
                requirements.push(
                    "硬规则：标「满意」的选项 label 必须以「满意」开头（如「满意，结束」「满意，按此输出」）；标「重做」的选项 label 必须以「重做」开头（如「重做，重新规划」）。这是状态机识别的依据。".into(),
                );
                push_request_user_input_quality_rules(&mut requirements);
                push_stage_hint(&mut requirements, milestone);
            }
        }

        Ok(TurnDirective::CallLlm(ContractPrompt {
            milestone_id: milestone.id.clone(),
            user_message: user_message.to_string(),
            allowed_tools: contract.allowed_tools.clone(),
            system_requirements: requirements,
        }))
    }
}

/// 注入 request_user_input 的选项质量规则。
/// 用在 Collect / ProduceOptions / (未来) Review 等所有发选择卡的 mode。
fn push_request_user_input_quality_rules(requirements: &mut Vec<String>) {
    requirements.push(
        "硬规则：调用 request_user_input 时，每个 option.description 必须 ≥ 30 字，说明这个选项与其他选项的关键差异、适用场景、取舍——不能是「一句话注释」级别。".into(),
    );
    requirements.push(
        "硬规则：description 字段支持 markdown，可以用粗体、列表、内联代码强调关键信息，让用户一眼看清差异。".into(),
    );
    requirements.push(
        "建议：在多选项场景，挑一个最契合用户偏好的选项标 `recommended: true`，并在 description 里 1-2 句说明为什么推荐——可以显著提高用户决策信心。".into(),
    );
}

fn push_stage_hint(requirements: &mut Vec<String>, milestone: &Milestone) {
    if let Some(hint) = milestone.prompt_hint.as_deref() {
        requirements.push(format!("阶段目标：{hint}"));
    }
}

fn push_output_requirements(
    requirements: &mut Vec<String>,
    output_requirements: &[OutputRequirement],
) {
    for req in output_requirements {
        match req {
            OutputRequirement::NoOpenQuestion => {
                requirements.push("响应末尾不要以问号结尾（不要开放追问）。".into());
            }
            OutputRequirement::RequiresToolCall(name) => {
                requirements.push(format!("本阶段必须调用 `{name}` 工具完成任务。"));
            }
            OutputRequirement::ForbidTool(name) => {
                requirements.push(format!("本阶段禁止调用 `{name}` 工具。"));
            }
            OutputRequirement::MinOptions(n) => {
                requirements.push(format!("选择题选项数量不少于 {n}。"));
            }
            OutputRequirement::MaxOptions(n) => {
                requirements.push(format!("选择题选项数量不超过 {n}。"));
            }
            OutputRequirement::NoToolCall => {
                requirements.push("本阶段禁止调用任何工具。".into());
            }
        }
    }
}

fn question_budget_reached(milestone: &Milestone, state: &ConversationState) -> bool {
    let budget = milestone.contract.question_budget;
    budget > 0 && state.question_count(&milestone.id) >= budget
}

fn question_budget_message(milestone: &Milestone) -> String {
    format!(
        "当前阶段「{}」已达到提问次数上限，请基于已有信息继续推进。",
        milestone.label
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::Milestone;
    use crate::contract::{AdvancePolicy, MilestoneContract, MilestoneMode, OutputRequirement};
    use crate::workflow::ConversationState;

    fn milestone(id: &str, mode: MilestoneMode) -> Milestone {
        Milestone {
            id: id.into(),
            label: id.into(),
            prompt_hint: Some(format!("hint {id}")),
            icon: None,
            contract: MilestoneContract {
                mode,
                question_budget: 1,
                allowed_tools: vec!["request_user_input".into()],
                advance_policy: AdvancePolicy::OnChoice,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn collect_with_budget_returns_llm_call_allowing_user_input() {
        let ms = milestone("goal", MilestoneMode::Collect);
        let cs = ConversationState::new("计划敲定".into(), vec![ms.clone()]);
        let directive = ContractRuntime::next_directive(&ms, &cs, "用户需求").unwrap();
        assert!(matches!(directive, TurnDirective::CallLlm(_)));
        let TurnDirective::CallLlm(prompt) = directive else {
            unreachable!()
        };
        assert!(prompt.allowed_tools.contains(&"request_user_input".into()));
        assert!(
            prompt
                .system_requirements
                .iter()
                .any(|r| r.contains("最多调用"))
        );
    }

    #[test]
    fn final_output_disallows_tools_and_requires_final_document() {
        let mut ms = milestone("output", MilestoneMode::FinalOutput);
        ms.contract.question_budget = 0;
        ms.contract.allowed_tools = vec![];
        ms.contract.output_requirements = vec![OutputRequirement::NoToolCall];
        let cs = ConversationState::new("计划敲定".into(), vec![ms.clone()]);
        let directive = ContractRuntime::next_directive(&ms, &cs, "继续").unwrap();
        let TurnDirective::CallLlm(prompt) = directive else {
            unreachable!()
        };
        assert!(prompt.allowed_tools.is_empty());
        assert!(
            prompt
                .system_requirements
                .iter()
                .any(|r| r.contains("最终"))
        );
    }

    #[test]
    fn final_output_with_allowed_tool_does_not_forbid_tools() {
        let mut ms = milestone("output", MilestoneMode::FinalOutput);
        ms.contract.question_budget = 0;
        ms.contract.allowed_tools = vec!["file_write".into()];
        ms.contract.output_requirements = vec![];
        let cs = ConversationState::new("文档生成".into(), vec![ms.clone()]);

        let directive = ContractRuntime::next_directive(&ms, &cs, "继续").unwrap();

        let TurnDirective::CallLlm(prompt) = directive else {
            unreachable!()
        };
        assert!(prompt.allowed_tools.contains(&"file_write".into()));
        assert!(
            prompt
                .system_requirements
                .iter()
                .any(|r| r.contains("file_write") || r.contains("允许"))
        );
        assert!(
            !prompt
                .system_requirements
                .iter()
                .any(|r| r.contains("不要调用工具"))
        );
    }

    #[test]
    fn refine_prompt_uses_stage_hint_without_plan_specific_requirements() {
        let mut ms = milestone("draft", MilestoneMode::RefineSelectedOption);
        ms.label = "生成草稿".into();
        ms.prompt_hint = Some("根据素材和需求生成初稿".into());
        ms.contract.question_budget = 0;
        ms.contract.allowed_tools = vec![];
        ms.contract.output_requirements = vec![OutputRequirement::NoOpenQuestion];
        let cs = ConversationState::new("文档生成".into(), vec![ms.clone()]);

        let directive = ContractRuntime::next_directive(&ms, &cs, "继续").unwrap();

        let TurnDirective::CallLlm(prompt) = directive else {
            unreachable!()
        };
        let rendered_requirements = prompt.system_requirements.join("\n");
        assert!(rendered_requirements.contains("根据素材和需求生成初稿"));
        assert!(!rendered_requirements.contains("已选方案"));
        assert!(!rendered_requirements.contains("时间表、资源分配和风险预案"));
    }

    #[test]
    fn freeform_prompt_includes_stage_hint_and_output_requirements() {
        let mut ms = milestone("analyze", MilestoneMode::Freeform);
        ms.label = "分析".into();
        ms.prompt_hint = Some("根据用户需求进行深入分析，输出表格和文字解读".into());
        ms.contract.question_budget = 0;
        ms.contract.allowed_tools = vec!["python".into(), "shell".into()];
        ms.contract.output_requirements = vec![OutputRequirement::NoOpenQuestion];
        let cs = ConversationState::new("数据分析".into(), vec![ms.clone()]);

        let directive = ContractRuntime::next_directive(&ms, &cs, "继续").unwrap();

        let TurnDirective::CallLlm(prompt) = directive else {
            unreachable!()
        };
        let rendered_requirements = prompt.system_requirements.join("\n");
        assert!(rendered_requirements.contains("深入分析"));
        assert!(rendered_requirements.contains("问号"));
        assert!(prompt.allowed_tools.contains(&"python".into()));
    }

    #[test]
    fn collect_over_question_budget_returns_blocked() {
        let ms = milestone("goal", MilestoneMode::Collect);
        let mut cs = ConversationState::new("计划敲定".into(), vec![ms.clone()]);
        cs.increment_question_count("goal");

        let directive = ContractRuntime::next_directive(&ms, &cs, "继续").unwrap();

        let TurnDirective::Blocked(message) = directive else {
            panic!("expected blocked directive")
        };
        assert!(message.contains("已达到提问次数上限"));
    }

    #[test]
    fn produce_options_over_question_budget_returns_blocked() {
        let ms = milestone("options", MilestoneMode::ProduceOptions);
        let mut cs = ConversationState::new("计划敲定".into(), vec![ms.clone()]);
        cs.increment_question_count("options");

        let directive = ContractRuntime::next_directive(&ms, &cs, "继续").unwrap();

        let TurnDirective::Blocked(message) = directive else {
            panic!("expected blocked directive")
        };
        assert!(message.contains("已达到提问次数上限"));
    }
}
