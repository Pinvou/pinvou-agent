//! StepBuilder — 阶段 prompt 构造器。
//!
//! 当前主路径只用 `build_contract_prompt`：把 ContractRuntime 给出的
//! system_requirements + allowed_tools + 已知 context 拼成最终 system prompt。
//! 旧的 `build` / `build_decomposition` / `build_review_prompt` / `ban_list`
//! 已随 legacy 模块（app.rs / response_checker / reviewer）一起删除。

use std::collections::HashMap;

/// 构造好的 prompt 输出
#[derive(Debug, Clone)]
pub struct StepPrompt {
    pub system: String,
    pub append_user_message: bool,
}

pub struct StepBuilder;

impl StepBuilder {
    /// 用 ContractRuntime 输出的 ContractPrompt 渲染最终 system prompt。
    ///
    /// 结构：
    /// ```text
    /// ## 应用角色（如有）
    /// ...agent 的 markdown body...
    ///
    /// ## 当前契约要求
    /// - <每条 system_requirement>
    ///
    /// ## 已知信息
    /// - <key>: <value>
    ///
    /// ## 工具限制 / 可用工具
    ///
    /// ## 用户消息
    /// ```
    pub fn build_contract_prompt(
        prompt: &crate::contract_runtime::ContractPrompt,
        context: &HashMap<String, String>,
        agent_prompt: Option<&str>,
    ) -> StepPrompt {
        let mut parts = Vec::new();
        if let Some(agent_prompt) = agent_prompt {
            parts.push("## 应用角色".to_string());
            parts.push(agent_prompt.to_string());
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

        StepPrompt {
            system: parts.join("\n"),
            append_user_message: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_contract_prompt_includes_contract_rules() {
        let prompt = crate::contract_runtime::ContractPrompt {
            milestone_id: "options".into(),
            user_message: "继续".into(),
            allowed_tools: vec!["request_user_input".into()],
            system_requirements: vec!["必须给出 2-3 个可选方案".into(), "不能只问偏好".into()],
        };

        let rendered = StepBuilder::build_contract_prompt(
            &prompt,
            &Default::default(),
            Some("计划制定 agent body"),
        );

        assert!(rendered.system.contains("必须给出 2-3 个可选方案"));
        assert!(rendered.system.contains("request_user_input"));
        assert!(rendered.system.contains("计划制定 agent body"));
    }

    #[test]
    fn build_contract_prompt_without_tools_says_no_tool_call() {
        let prompt = crate::contract_runtime::ContractPrompt {
            milestone_id: "final".into(),
            user_message: "".into(),
            allowed_tools: vec![],
            system_requirements: vec!["输出最终 markdown".into()],
        };
        let rendered = StepBuilder::build_contract_prompt(&prompt, &Default::default(), None);
        assert!(rendered.system.contains("不要调用工具"));
    }

    #[test]
    fn build_contract_prompt_injects_context() {
        let mut ctx = std::collections::HashMap::new();
        ctx.insert("doc_type".into(), "周报".into());
        let prompt = crate::contract_runtime::ContractPrompt {
            milestone_id: "draft".into(),
            user_message: "".into(),
            allowed_tools: vec![],
            system_requirements: vec!["生成草稿".into()],
        };
        let rendered = StepBuilder::build_contract_prompt(&prompt, &ctx, None);
        assert!(rendered.system.contains("doc_type"));
        assert!(rendered.system.contains("周报"));
    }
}
