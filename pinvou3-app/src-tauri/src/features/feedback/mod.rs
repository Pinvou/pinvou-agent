use std::fmt;

use serde::{Deserialize, Serialize};

const MAX_TITLE_CHARS: usize = 120;
const MAX_DESCRIPTION_CHARS: usize = 5000;
const COMMUNITY_ISSUES_URL: &str = "https://github.com/Pinvou/pinvou-agent/issues";

/// 回执状态。`Submitted` 必须保留：前端提交回执显式判
/// `receipt.status === 'submitted'`（SettingsView 提交流程）。
/// 原 `FailedRetryable` 变体已删——后端从不下发它，前端也从不匹配回执的
/// 该值（其本地 UI 状态字符串 'failed_retryable' 与本枚举无关）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackStatus {
    Submitted,
    FailedValidation,
}

/// 提交请求。社区版不上传：`type`/`error_summary`/`attachments`/
/// `privacy_notice_version` 等字段此前解析后即被丢弃（存根从不消费），
/// 已从结构体删除；前端多发的同名 JSON 键由 serde 忽略，线上行为不变。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackSubmitRequest {
    #[serde(default)]
    pub title: Option<String>,
    pub description: String,
    pub entry_point: String,
}

/// 提交回执。`feedback_id`/`submitted_at`/`retryable` 前端从不读取，已删；
/// `status` 与 `message` 前端消费，保留。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackReceipt {
    pub status: FeedbackStatus,
    pub message: String,
}

#[derive(Debug)]
pub enum FeedbackError {
    Validation(String),
}

impl fmt::Display for FeedbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FeedbackError::Validation(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for FeedbackError {}

pub async fn submit_feedback(
    request: FeedbackSubmitRequest,
) -> Result<FeedbackReceipt, FeedbackError> {
    validate_feedback_request(&request)?;
    Ok(FeedbackReceipt {
        status: FeedbackStatus::FailedValidation,
        message: format!(
            "社区版不会上传反馈、日志或附件。请前往 {COMMUNITY_ISSUES_URL} 提交 Issue。"
        ),
    })
}

fn validate_feedback_request(request: &FeedbackSubmitRequest) -> Result<(), FeedbackError> {
    let description_len = request.description.trim().chars().count();
    if description_len == 0 {
        return Err(FeedbackError::Validation("请填写反馈说明。".to_string()));
    }
    if description_len > MAX_DESCRIPTION_CHARS {
        return Err(FeedbackError::Validation(format!(
            "反馈说明最多 {MAX_DESCRIPTION_CHARS} 个字符。"
        )));
    }
    if request
        .title
        .as_ref()
        .is_some_and(|title| title.chars().count() > MAX_TITLE_CHARS)
    {
        return Err(FeedbackError::Validation(format!(
            "反馈标题最多 {MAX_TITLE_CHARS} 个字符。"
        )));
    }
    if !matches!(request.entry_point.as_str(), "settings" | "error_banner") {
        return Err(FeedbackError::Validation("反馈入口来源无效。".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> FeedbackSubmitRequest {
        FeedbackSubmitRequest {
            title: Some("问题".to_string()),
            description: "复现步骤".to_string(),
            entry_point: "settings".to_string(),
        }
    }

    #[tokio::test]
    async fn community_feedback_never_reports_submission() {
        let receipt = submit_feedback(request()).await.unwrap();
        assert_eq!(receipt.status, FeedbackStatus::FailedValidation);
        assert!(receipt.message.contains(COMMUNITY_ISSUES_URL));
    }
}
