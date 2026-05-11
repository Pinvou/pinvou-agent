//! StepBuilder — 小范围 prompt 构造器。
//!
//! 纯函数模块，无状态，不做 LLM 调用。

use super::app::{AppConfig, Milestone};
use std::collections::HashMap;

/// 构造好的 prompt 输出
#[derive(Debug, Clone)]
pub struct StepPrompt {
    pub system: String,
    pub append_user_message: bool,
}

pub struct StepBuilder;

impl StepBuilder {
    /// 为当前里程碑构造小范围执行 prompt
    pub fn build(
        milestone: &Milestone,
        context: &HashMap<String, String>,
        user_message: &str,
        app_config: &AppConfig,
    ) -> StepPrompt {
        let mut parts = Vec::new();

        parts.push("## 当前任务（只做这个）".to_string());
        let task_desc = milestone.prompt_hint.as_deref().unwrap_or(&milestone.label);
        parts.push(task_desc.to_string());

        if !context.is_empty() {
            parts.push("\n## 已知信息".to_string());
            for (k, v) in context {
                parts.push(format!("- **{k}**: {v}"));
            }
        }

        parts.push("\n## 禁止".to_string());
        for ban in Self::ban_list(app_config, &milestone.label) {
            parts.push(format!("- {ban}"));
        }

        // 预告下一阶段，让 LLM 知道还有后续
        let next_label = app_config
            .milestones
            .iter()
            .skip_while(|m| m.id != milestone.id)
            .nth(1)
            .map(|m| m.label.as_str());
        if let Some(next) = next_label {
            parts.push(format!("\n## 下一阶段预告\n当前阶段完成后，下一阶段是「{next}」。现在只做当前阶段，完成后输出 [OK] 让系统自动推进。"));
        }

        // 引导使用 request_user_input 工具，并提供文本降级方案
        parts.push("\n## 向用户收集信息的方式（重要）".to_string());
        parts.push("优先调用 `request_user_input` 工具让用户做选择题。".to_string());
        parts.push("如果你无法调用该工具，用以下文本格式直接在消息中给出选项：".to_string());
        parts.push("  [选项名] - 简短解释".to_string());
        parts.push(
            "不要问「你想怎么样」「请描述你的需求」这类开放式问题——用户不想打字。".to_string(),
        );
        parts.push(
            "例如：「[轻松休闲] - 以放松欣赏风景为主\\n[中等探索] - 适度运动与探索」".to_string(),
        );
        parts.push("".to_string());
        parts.push("**关键规则：每个阶段只能进行一轮提问。**".to_string());
        parts.push(
            "调用 request_user_input 一次，收到用户回答后，立刻总结信息并输出 [OK]。".to_string(),
        );
        parts.push(
            "绝对不要在收到回答后再发起第二轮提问——用户已经回答了，你应该推进而不是追问。"
                .to_string(),
        );
        parts.push("如果你觉得信息还不够，信息已经够了——输出 [OK] 让系统推进到下一阶段，在下一阶段再补充。".to_string());

        parts.push("\n## 输出末尾附加当前步骤状态：".to_string());
        parts.push("[OK] / [MORE] 还需要:{具体内容} / [BLOCKED] 原因:{具体原因}".to_string());

        let system = parts.join("\n");
        let user_section = format!("\n\n## 用户消息\n{user_message}");

        StepPrompt {
            system: if user_message.is_empty() {
                system
            } else {
                system + &user_section
            },
            append_user_message: false,
        }
    }

    pub fn build_contract_prompt(
        prompt: &crate::contract_runtime::ContractPrompt,
        context: &std::collections::HashMap<String, String>,
        app_prompt: Option<&str>,
    ) -> StepPrompt {
        let mut parts = Vec::new();
        if let Some(app_prompt) = app_prompt {
            parts.push("## 应用角色".to_string());
            parts.push(app_prompt.to_string());
        }
        parts.push("\n## 当前契约要求".to_string());
        for req in &prompt.system_requirements {
            parts.push(format!("- {req}"));
        }
        if !context.is_empty() {
            parts.push("\n## 已知信息".to_string());
            for (k, v) in context {
                parts.push(format!("- **{k}**: {v}"));
            }
        }
        if prompt.allowed_tools.is_empty() {
            parts.push("\n## 工具限制\n- 当前阶段不要调用工具。".to_string());
        } else {
            parts.push(format!(
                "\n## 可用工具\n- {}",
                prompt.allowed_tools.join("\n- ")
            ));
        }
        parts.push("\n## 用户消息".to_string());
        parts.push(prompt.user_message.clone());
        parts.push(
            "\n## 状态信号\n回复末尾附加 [OK] / [MORE] 还需要:{具体内容} / [BLOCKED] 原因:{具体原因}"
                .to_string(),
        );

        StepPrompt {
            system: parts.join("\n"),
            append_user_message: false,
        }
    }

    /// 构造任务拆解 prompt
    pub fn build_decomposition(
        user_request: &str,
        app_config: &AppConfig,
        available_tools: &[String],
        context_summary: &str,
    ) -> String {
        let tools_str = if available_tools.is_empty() {
            "（无额外工具）".to_string()
        } else {
            available_tools.join(", ")
        };

        format!(
            "用户想: \"{user_request}\"\n\
             当前应用: {app_name} -- {app_desc}\n\
             可用工具: {tools_str}\n\
             已知信息: {ctx}\n\
             \n\
             请把这个任务拆成多个小步骤。\n\
             \n\
             拆解规则:\n\
             1. 每步 = 一个可用 1-3 次工具调用完成的完整动作\n\
             2. 每步必须有明确的可验证产出物（文件、图表、文本段落、确认）\n\
             3. 每步的产出物是下一步的输入\n\
             4. 不限制步骤总数 -- 复杂任务可以多步，简单任务可以少步\n\
             5. 不能假设用户的工具能力\n\
             6. 需要用户决策的步骤不要写成\"询问用户...\"这种开放式文本题\n\
                而应该写成\"使用 request_user_input 工具让用户做选择题\"\n\
                提供 2-3 个具体选项，每个选项有 label 和 description\n\
             \n\
             禁止:\n\
             x 笼统步骤: \"分析数据\"、\"写文档\"、\"处理\"、\"做\"\n\
             x 无产出物的步骤\n\
             x 超过 5 次工具调用才能完成的粗粒度步骤\n\
             x TBD、TODO、placeholder\n\
             x 开放式问答步骤（用户需要打很多字）\n\
             \n\
             好例子:\n\
             \"读取 sales.csv，展示列名、行数、数据类型和缺失情况\"\n\
             \"按月汇总销售额，计算环比增长率，用表格展示\"\n\
             \"生成周报草稿(三段: 本周工作/问题/下周计划，约500字)\"\n\
             \"使用 request_user_input 让用户选择分析维度（销售趋势/地区对比/都看看）\"\n\
             \n\
             差例子:\n\
             x \"分析销售数据\" -- 太笼统\n\
             x \"打开文件\" -- 太细，不是完整动作\n\
             x \"询问用户想分析什么\" -- 应该是选择题不是问答题\n\
             \n\
             只输出步骤列表，每行: \"N. {{具体动词+具体对象+明确产出}}\"",
            user_request = user_request,
            app_name = app_config.name,
            app_desc = app_config.description,
            tools_str = tools_str,
            ctx = if context_summary.is_empty() {
                "暂无"
            } else {
                context_summary
            },
        )
    }

    /// 构造审阅 prompt
    pub fn build_review_prompt(
        decomposition: &str,
        user_request: &str,
        available_tools: &[String],
    ) -> String {
        let tools_str = if available_tools.is_empty() {
            "（无额外工具）".to_string()
        } else {
            available_tools.join(", ")
        };

        format!(
            "你是任务拆解审阅员。检查以下步骤拆解。\n\
             \n\
             拆解结果:\n\
             {decomposition}\n\
             \n\
             用户原始需求: {user_request}\n\
             可用工具: {tools_str}\n\
             \n\
             检查项:\n\
             1. 每步都具体吗？（有没有\"分析\"、\"处理\"、\"做\"这种空洞词？）\n\
             2. 每步的产出物明确吗？（做完能判断\"完成了\"吗？）\n\
             3. 步骤之间连续吗？（前一步输出是后一步输入吗？）\n\
             4. 整体覆盖用户需求吗？（有没有遗漏？）\n\
             \n\
             输出 JSON (只输出 JSON):\n\
             {{\n\
               \"ok\": true/false,\n\
               \"issues\": [\n\
                 {{\"step\": 2, \"problem\": \"太笼统\", \"suggestion\": \"改为...\"}}\n\
               ],\n\
               \"overall\": \"一句话总结\"\n\
             }}"
        )
    }

    /// 根据 app id、当前阶段和 app 自定义规则返回禁止清单
    pub fn ban_list(app_config: &AppConfig, phase: &str) -> Vec<String> {
        let mut bans: Vec<String> = vec![
            "严格按照当前步骤操作，不要提前完成后续步骤".into(),
            "不要自己编造不存在的数据".into(),
            "完成当前任务后必须附加自评信号 [OK]/[MORE]/[BLOCKED]".into(),
            "即使已掌握足够信息，也只完成当前这一步，输出 [OK] 交给系统推进".into(),
            "每阶段最多提问一轮。收到用户回答后立即输出 [OK]，严禁追问".into(),
        ];

        // 早期阶段：只问不写，不急于出方案
        let is_early_phase = phase.contains("需求")
            || phase.contains("确认")
            || phase.contains("收集")
            || phase.contains("目标")
            || phase.contains("约束");
        // 中间阶段：可以出方案但不要给最终完整交付物
        let is_mid_phase =
            phase.contains("方案") || phase.contains("对比") || phase.contains("细化");
        // 最后一步：可以给完整计划书
        let is_final_phase =
            phase.contains("输出") || phase.contains("计划书") || phase.contains("定稿");

        match app_config.id.as_str() {
            "文档生成" => {
                if is_early_phase {
                    bans.push("只问不写，不要提前生成内容".into());
                }
                if is_mid_phase {
                    bans.push("只做草稿/大纲，末尾请用户确认再继续".into());
                }
            }
            "数据分析" => {
                if phase.contains("探索") || phase.contains("查看") {
                    bans.push("不要跳过数据验证".into());
                }
            }
            "计划敲定" => {
                if is_early_phase {
                    bans.push("只确认用户需求和约束条件，不要开始写方案".into());
                    bans.push("这一步只做信息收集，输出 [OK] 让系统推进到方案对比阶段".into());
                }
                if is_mid_phase {
                    bans.push("只做当前阶段的分析/对比/细化，不要跳到完整计划书".into());
                    bans.push("方案对比阶段：给出选项让用户选，不要替用户决定".into());
                    bans.push("细化方案阶段：对选定的方案细化，不要开始写最终文档".into());
                }
                if is_final_phase {
                    bans.push("可以输出完整的最终计划书".into());
                }
            }
            _ => {}
        }

        if phase.contains("生成") || phase.contains("草稿") || phase.contains("撰写") {
            bans.push("输出完整内容，末尾询问用户'需要调整哪里？'".into());
        }
        if phase.contains("定稿") || phase.contains("保存") || phase.contains("提交") {
            bans.push("执行保存操作，不要重新生成内容".into());
        }

        // 追加 app.toml 自定义 ban 规则
        for ban in &app_config.ban_list {
            bans.push(ban.clone());
        }

        bans
    }
}

#[cfg(test)]
mod tests {
    use super::super::app::{AppConfig, Milestone};
    use super::*;

    fn test_milestone() -> Milestone {
        Milestone {
            id: "draft".into(),
            label: "生成草稿".into(),
            prompt_hint: Some(
                "根据已知信息生成周报草稿，三段式，500字以内。输出后问'需要调整哪里？'".into(),
            ),
            icon: None,
            ..Default::default()
        }
    }

    fn test_app() -> AppConfig {
        test_app_with_id("文档生成")
    }

    fn test_app_with_id(id: &str) -> AppConfig {
        AppConfig {
            id: id.into(),
            name: id.into(),
            description: "测试应用".into(),
            icon: "[..]".into(),
            prompt_file: None,
            prompt: None,
            model_preference: "medium".into(),
            tools: vec!["file_write".into()],
            milestones: vec![],
            granularity: None,
            confirm_at: None,
            ban_list: vec![],
            meta: Default::default(),
            ..Default::default()
        }
    }

    #[test]
    fn test_build_contains_scope_limit() {
        let prompt = StepBuilder::build(
            &test_milestone(),
            &Default::default(),
            "帮我写周报",
            &test_app(),
        );
        assert!(prompt.system.contains("只做这个"));
    }

    #[test]
    fn test_build_contains_ban_list() {
        let prompt = StepBuilder::build(
            &test_milestone(),
            &Default::default(),
            "帮我写周报",
            &test_app(),
        );
        assert!(prompt.system.contains("严格按照当前步骤操作"));
        assert!(prompt.system.contains("自评信号"));
    }

    #[test]
    fn test_build_contains_context() {
        let mut ctx = std::collections::HashMap::new();
        ctx.insert("doc_type".into(), "周报".into());
        ctx.insert("audience".into(), "内部".into());
        let prompt = StepBuilder::build(&test_milestone(), &ctx, "写周报", &test_app());
        assert!(prompt.system.contains("doc_type"));
        assert!(prompt.system.contains("周报"));
    }

    #[test]
    fn test_build_contains_user_message() {
        let prompt = StepBuilder::build(
            &test_milestone(),
            &Default::default(),
            "帮我写周报",
            &test_app(),
        );
        assert!(prompt.system.contains("帮我写周报"));
    }

    #[test]
    fn test_decomposition_prompt_structure() {
        let prompt = StepBuilder::build_decomposition(
            "帮我分析销售数据",
            &test_app(),
            &["file_read".into(), "shell".into()],
            "无",
        );
        assert!(prompt.contains("拆成多个小步骤"));
        assert!(prompt.contains("好例子"));
        assert!(prompt.contains("差例子"));
        assert!(prompt.contains("禁止"));
        assert!(prompt.contains("file_read"));
    }

    #[test]
    fn test_review_prompt_structure() {
        let prompt = StepBuilder::build_review_prompt(
            "1. 读取文件\n2. 分析数据",
            "分析销售数据",
            &["file_read".into()],
        );
        assert!(prompt.contains("审阅员"));
        assert!(prompt.contains("ok"));
        assert!(prompt.contains("issues"));
    }

    #[test]
    fn test_build_from_contract_prompt_includes_contract_rules() {
        let prompt = crate::contract_runtime::ContractPrompt {
            milestone_id: "options".into(),
            user_message: "继续".into(),
            allowed_tools: vec!["request_user_input".into()],
            system_requirements: vec!["必须给出 2-3 个可选方案".into(), "不能只问偏好".into()],
        };

        let rendered = StepBuilder::build_contract_prompt(
            &prompt,
            &Default::default(),
            Some("计划敲定 prompt"),
        );

        assert!(rendered.system.contains("必须给出 2-3 个可选方案"));
        assert!(rendered.system.contains("request_user_input"));
        assert!(rendered.system.contains("计划敲定 prompt"));
    }

    #[test]
    fn test_ban_list_doc_requirement_phase() {
        let bans = StepBuilder::ban_list(&test_app(), "明确需求");
        assert!(bans.iter().any(|b| b.contains("只问不写")));
        assert!(bans.iter().any(|b| b.contains("只问不写")));
    }

    #[test]
    fn test_ban_list_doc_generation_phase() {
        let bans = StepBuilder::ban_list(&test_app(), "生成草稿");
        assert!(bans.iter().any(|b| b.contains("需要调整哪里")));
    }

    #[test]
    fn test_ban_list_analysis_explore_phase() {
        let bans = StepBuilder::ban_list(&test_app_with_id("数据分析"), "探索数据");
        assert!(bans.iter().any(|b| b.contains("数据验证")));
    }

    #[test]
    fn test_ban_list_plan_options_phase() {
        let bans = StepBuilder::ban_list(&test_app_with_id("计划敲定"), "方案对比");
        assert!(bans.iter().any(|b| b.contains("给出选项让用户选")));
    }
}
