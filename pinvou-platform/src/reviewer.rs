//! **LEGACY**：LLM 自审拆解结果（语义检查）。
//!
//! 新设计的 `combined_planner` 只做结构性校验（mode 枚举、tool 池、数量），
//! 不需要 LLM 二次审阅。本模块仅在 `engine::decompose_and_execute` 中使用，
//! 而该方法已不在 web 主路径上。P1 删除。
//!
//! LLMReviewer — 调用 LLM 对拆解结果做语义审阅。
//!
//! JSON 解析失败 → 正则降级 → 再失败默认 ok=true 放行。

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::harness::AgentHarness;
use super::step_builder::StepBuilder;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    pub ok: bool,
    pub issues: Vec<Issue>,
    pub overall: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub step: Option<u32>,
    pub problem: String,
    pub suggestion: String,
}

pub struct LLMReviewer;

impl LLMReviewer {
    pub async fn review(
        harness: &dyn AgentHarness,
        decomposition: &str,
        user_request: &str,
        available_tools: &[String],
    ) -> Result<ReviewResult> {
        let prompt = StepBuilder::build_review_prompt(decomposition, user_request, available_tools);

        let response = harness
            .chat(super::harness::ChatRequest {
                user_message: prompt,
                platform_system_prompt: Some(
                    "你是一个任务审阅员。只输出 JSON，不要输出其他内容。".into(),
                ),
                context: Default::default(),
                tools: vec![],
                model: None,
                session_id: None,
                previous_messages: vec![],
            })
            .await?;

        Ok(Self::parse_review_response(&response))
    }

    fn parse_review_response(text: &str) -> ReviewResult {
        // Layer 1: Standard JSON (possibly in ```json block)
        let json_text = if let Some(start) = text.find("```json") {
            let inner = &text[start + 7..];
            if let Some(end) = inner.find("```") {
                inner[..end].trim().to_string()
            } else {
                inner.trim().to_string()
            }
        } else if let Some(start) = text.find('{') {
            if let Some(end) = text.rfind('}') {
                text[start..=end].to_string()
            } else {
                text.to_string()
            }
        } else {
            text.to_string()
        };

        if let Ok(result) = serde_json::from_str::<ReviewResult>(&json_text) {
            return result;
        }

        // Layer 2: Regex fallback
        Self::fallback_parse(text)
    }

    fn fallback_parse(text: &str) -> ReviewResult {
        let ok = !text.contains("\"ok\": false")
            && !text.contains("\"ok\":false")
            && !text.contains("ok: false")
            && !text.contains("ok:false");

        let issues = Self::extract_issues_fallback(text);

        let re_overall = Regex::new(r#"overall"[:\s]*"([^"]+)""#).unwrap();
        let overall = re_overall
            .captures(text)
            .map(|c| c[1].to_string())
            .unwrap_or_else(|| {
                if ok {
                    "审阅通过（解析降级）".to_string()
                } else {
                    "审阅未通过（解析降级），请人工确认".to_string()
                }
            });

        ReviewResult {
            ok,
            issues,
            overall,
        }
    }

    fn extract_issues_fallback(text: &str) -> Vec<Issue> {
        let mut issues = Vec::new();

        let re_block = Regex::new(
            r#""step"[:\s]*(\d+)[^}]*"problem"[:\s]*"([^"]+)"[^}]*"suggestion"[:\s]*"([^"]+)""#,
        )
        .unwrap();

        for caps in re_block.captures_iter(text) {
            issues.push(Issue {
                step: caps.get(1).and_then(|m| m.as_str().parse().ok()),
                problem: caps
                    .get(2)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                suggestion: caps
                    .get(3)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
            });
        }

        if issues.is_empty() {
            let re_loose = Regex::new(r"步骤\s*(\d+)[:：]\s*(\S+)\s*[-–—]\s*(\S+)").unwrap();
            for caps in re_loose.captures_iter(text) {
                issues.push(Issue {
                    step: caps.get(1).and_then(|m| m.as_str().parse().ok()),
                    problem: caps
                        .get(2)
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default(),
                    suggestion: caps
                        .get(3)
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default(),
                });
            }
        }

        issues
    }

    pub fn format_feedback(result: &ReviewResult) -> String {
        let mut feedback = String::from("审阅意见：\n");
        for issue in &result.issues {
            if let Some(step) = issue.step {
                feedback.push_str(&format!(
                    "- 步骤 {step}: {}, 建议: {}\n",
                    issue.problem, issue.suggestion
                ));
            } else {
                feedback.push_str(&format!(
                    "- {}, 建议: {}\n",
                    issue.problem, issue.suggestion
                ));
            }
        }
        feedback.push_str(&format!(
            "\n总体评价: {}\n请根据以上反馈修改拆解。",
            result.overall
        ));
        feedback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_json() {
        let json = r#"{"ok":false,"issues":[{"step":2,"problem":"太笼统","suggestion":"改为具体步骤"}],"overall":"需要修改"}"#;
        let result = LLMReviewer::parse_review_response(json);
        assert!(!result.ok);
        assert_eq!(result.issues.len(), 1);
        assert_eq!(result.issues[0].step, Some(2));
    }

    #[test]
    fn test_parse_json_in_code_block() {
        let text = "```json\n{\"ok\":true,\"issues\":[],\"overall\":\"很好\"}\n```";
        let result = LLMReviewer::parse_review_response(text);
        assert!(result.ok);
        assert!(result.issues.is_empty());
    }

    #[test]
    fn test_parse_malformed_json_fallback() {
        let text = "ok: false\noverall: 需要改进";
        let result = LLMReviewer::parse_review_response(text);
        assert!(!result.ok);
    }

    #[test]
    fn test_parse_ungrammatical_default_ok() {
        let text = "看起来还不错，拆解得挺好的。";
        let result = LLMReviewer::parse_review_response(text);
        assert!(result.ok);
        assert!(result.issues.is_empty());
    }

    #[test]
    fn test_format_feedback() {
        let result = ReviewResult {
            ok: false,
            issues: vec![Issue {
                step: Some(1),
                problem: "太笼统".into(),
                suggestion: "改为具体步骤".into(),
            }],
            overall: "需要修改".into(),
        };
        let feedback = LLMReviewer::format_feedback(&result);
        assert!(feedback.contains("步骤 1"));
        assert!(feedback.contains("太笼统"));
        assert!(feedback.contains("改为具体步骤"));
        assert!(feedback.contains("需要修改"));
    }

    #[test]
    fn test_format_feedback_all_ok() {
        let result = ReviewResult {
            ok: true,
            issues: vec![],
            overall: "拆解很好".into(),
        };
        let feedback = LLMReviewer::format_feedback(&result);
        assert!(feedback.contains("拆解很好"));
    }
}
