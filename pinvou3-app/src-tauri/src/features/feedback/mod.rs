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

/// Submission request. The community edition uploads nothing, so the stub
/// never reads `type`, `attachments` or `privacy_notice_version`; they are
/// kept because the headless CLI records the request it submitted in its
/// local receipt. The GUI's `error_summary` and per-attachment `mime` keys
/// are always null and are ignored by serde.
///
/// Attachment entries are metadata only: nothing in this crate ever opens
/// `path`. A consumer that stages or uploads the bytes must apply the
/// credential-path policy (`platform::path_policy`) itself —
/// [`validate_feedback_request`] deliberately does not, because where the
/// bytes may come from is a property of the destination, not of the
/// request shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackSubmitRequest {
    #[serde(rename = "type")]
    pub feedback_type: FeedbackType,
    #[serde(default)]
    pub title: Option<String>,
    pub description: String,
    pub entry_point: String,
    #[serde(default)]
    pub attachments: Vec<FeedbackAttachmentRequest>,
    /// Optional on deserialize so a payload without it still reaches
    /// validation instead of failing to parse.
    #[serde(default)]
    pub privacy_notice_version: String,
}

/// One attachment's metadata. `path` is never opened by this crate; see the
/// path-policy note on [`FeedbackSubmitRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackAttachmentRequest {
    pub path: String,
    pub name: String,
    pub media_type: String,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

/// Submission receipt. `status` and `message` are consumed by the frontend;
/// `retryable` is part of the receipt the headless CLI writes to disk and is
/// always `false` here because nothing is ever queued for retry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackReceipt {
    pub status: FeedbackStatus,
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
        status: FeedbackStatus::FailedValidation,
        message: format!(
            "社区版不会上传反馈、日志或附件。请前往 {COMMUNITY_ISSUES_URL} 提交 Issue。"
        ),
        retryable: false,
    })
}

/// Validate the request shape the GUI and the CLI share. Deliberately says
/// nothing about `attachments`: their count, size and path policy belong to
/// the consumer that actually stages the bytes (see
/// [`FeedbackSubmitRequest`]).
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

    #[test]
    fn gui_payload_deserializes_and_validates() {
        // The exact shape SettingsView sends, including the keys this struct
        // does not model (`error_summary`, `mime`).
        let payload = serde_json::json!({
            "type": "suggestion",
            "title": null,
            "description": "复现步骤",
            "entry_point": "settings",
            "error_summary": null,
            "attachments": [{
                "path": "/tmp/a.png",
                "name": "a.png",
                "media_type": "image",
                "mime": null,
                "size_bytes": null,
            }],
            "privacy_notice_version": "2026-06-24",
        });
        let request: FeedbackSubmitRequest =
            serde_json::from_value(payload).expect("the GUI payload must deserialize");
        assert_eq!(request.feedback_type, FeedbackType::Suggestion);
        assert_eq!(request.attachments.len(), 1);
        assert!(validate_feedback_request(&request).is_ok());
    }

    #[test]
    fn payload_without_privacy_notice_version_still_validates() {
        let payload = serde_json::json!({
            "type": "issue",
            "description": "复现步骤",
            "entry_point": "settings",
        });
        let request: FeedbackSubmitRequest = serde_json::from_value(payload)
            .expect("a payload without privacy_notice_version must deserialize");
        assert_eq!(request.privacy_notice_version, "");
        assert!(request.attachments.is_empty());
        assert!(validate_feedback_request(&request).is_ok());
    }

    #[test]
    fn validate_feedback_request_rejects_out_of_bounds_payloads() {
        // These rules are a cross-crate contract: the CLI translates its
        // output by `contains`-matching these exact anchors, so pin every
        // rejection path together with the anchor the CLI depends on.
        let mut req = request();
        req.description = "  \n".to_string();
        let err = validate_feedback_request(&req).expect_err("whitespace-only description");
        assert!(err.to_string().contains("请填写反馈说明"));

        let mut req = request();
        req.description = "字".repeat(MAX_DESCRIPTION_CHARS + 1);
        let err = validate_feedback_request(&req).expect_err("description beyond the cap");
        assert!(err.to_string().contains("反馈说明最多"));

        let mut req = request();
        req.title = Some("t".repeat(MAX_TITLE_CHARS + 1));
        let err = validate_feedback_request(&req).expect_err("title beyond the cap");
        assert!(err.to_string().contains("反馈标题最多"));

        let mut req = request();
        req.entry_point = "other".to_string();
        let err = validate_feedback_request(&req).expect_err("unknown entry_point");
        assert!(err.to_string().contains("反馈入口来源无效"));
    }
}
