use super::super::models::{LlmApiError, LlmApiErrorCode, LlmApiIdentity};

pub(crate) fn resolve_current_identity() -> Result<LlmApiIdentity, LlmApiError> {
    Err(LlmApiError::new(
        LlmApiErrorCode::UnsupportedPlatform,
        "LLM API Hub is currently only available on Windows",
        false,
    ))
}
