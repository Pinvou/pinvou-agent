use std::fmt;

use serde::{Deserialize, Serialize};

const MAX_TITLE_CHARS: usize = 120;
const MAX_DESCRIPTION_CHARS: usize = 5000;
const COMMUNITY_ISSUES_URL: &str = "https://github.com/Pinvou/pinvou-agent/issues";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackType {
    Issue,
    Suggestion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackStatus {
    Submitted,
    FailedRetryable,
    FailedValidation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackSubmitRequest {
    #[serde(rename = "type")]
    pub feedback_type: FeedbackType,
    #[serde(default)]
    pub title: Option<String>,
    pub description: String,
    pub entry_point: String,
    #[serde(default)]
    pub error_summary: Option<String>,
    #[serde(default)]
    pub attachments: Vec<FeedbackAttachmentRequest>,
    pub privacy_notice_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackAttachmentRequest {
    pub path: String,
    pub name: String,
    pub media_type: String,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackReceipt {
    pub feedback_id: String,
    pub status: FeedbackStatus,
    pub submitted_at: Option<String>,
    pub message: String,
    pub retryable: bool,
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
        feedback_id: String::new(),
        status: FeedbackStatus::FailedValidation,
        submitted_at: None,
        message: format!(
            "社区版不会上传反馈、日志或附件。请前往 {COMMUNITY_ISSUES_URL} 提交 Issue。"
        ),
        retryable: false,
    })
}

pub fn validate_feedback_request(request: &FeedbackSubmitRequest) -> Result<(), FeedbackError> {
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
            feedback_type: FeedbackType::Issue,
            title: Some("问题".to_string()),
            description: "复现步骤".to_string(),
            entry_point: "settings".to_string(),
            error_summary: None,
            attachments: vec![],
            privacy_notice_version: "community-v1".to_string(),
        }
    }

    #[tokio::test]
    async fn community_feedback_never_reports_submission() {
        let receipt = submit_feedback(request()).await.unwrap();
        assert_eq!(receipt.status, FeedbackStatus::FailedValidation);
        assert!(!receipt.retryable);
        assert!(receipt.message.contains(COMMUNITY_ISSUES_URL));
    }
}
