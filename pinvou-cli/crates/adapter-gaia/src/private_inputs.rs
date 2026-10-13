use std::fmt;
use std::sync::Arc;

use agent_backend_api::{
    AgentBackendError, AttachmentHandle, PrivateInputHandle, PrivateInputResolver,
    ResolvedAttachmentSource, ResolvedPrivateInput, SecretText,
};
use async_trait::async_trait;

use crate::GaiaDataset;

const FINAL_ANSWER_INSTRUCTION: &str =
    "\n\nReturn only the final answer using exactly this format:\nFINAL ANSWER: <answer>";

const PRIVATE_INPUT_UNKNOWN: &str = "gaia_private_input_unknown";
const ATTACHMENT_HANDLE_UNKNOWN: &str = "gaia_attachment_handle_unknown";
const ATTACHMENT_UNSAFE: &str = "gaia_attachment_unsafe";

pub struct GaiaPrivateInputs {
    dataset: Arc<GaiaDataset>,
}

impl GaiaPrivateInputs {
    pub fn new(dataset: Arc<GaiaDataset>) -> Self {
        Self { dataset }
    }

    pub fn resolve_handle(
        &self,
        handle: &PrivateInputHandle,
    ) -> Result<ResolvedPrivateInput, AgentBackendError> {
        let task_id = parse_handle(handle.expose_to_backend(), "prompt", PRIVATE_INPUT_UNKNOWN)?;
        let row = self
            .dataset
            .rows()
            .iter()
            .find(|row| row.task_id() == task_id)
            .ok_or_else(|| fixed_error(PRIVATE_INPUT_UNKNOWN))?;
        let attachments = row
            .attachment()
            .map(|_| AttachmentHandle::new(format!("gaia:{task_id}:attachment")))
            .into_iter()
            .collect();
        Ok(ResolvedPrivateInput::new(
            SecretText::new(format!(
                "{}{}",
                row.question().expose_to_backend(),
                FINAL_ANSWER_INSTRUCTION
            )),
            attachments,
        ))
    }

    pub fn resolve_attachment_handle(
        &self,
        handle: &AttachmentHandle,
    ) -> Result<ResolvedAttachmentSource, AgentBackendError> {
        let task_id = parse_handle(
            handle.expose_to_backend(),
            "attachment",
            ATTACHMENT_HANDLE_UNKNOWN,
        )?;
        let attachment = self
            .dataset
            .rows()
            .iter()
            .find(|row| row.task_id() == task_id)
            .and_then(|row| row.attachment())
            .ok_or_else(|| fixed_error(ATTACHMENT_HANDLE_UNKNOWN))?;
        let suggested_name = attachment
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| fixed_error(ATTACHMENT_UNSAFE))?;

        let verified_file = attachment
            .reopen_verified()
            .map_err(|_| fixed_error(ATTACHMENT_UNSAFE))?;
        let source = ResolvedAttachmentSource::from_verified_file(
            attachment.path(),
            suggested_name,
            verified_file,
        )
        .map_err(|_| fixed_error(ATTACHMENT_UNSAFE))?;
        attachment
            .verify_immutable_source(&source)
            .map_err(|_| fixed_error(ATTACHMENT_UNSAFE))?;
        Ok(source)
    }
}

impl fmt::Debug for GaiaPrivateInputs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GaiaPrivateInputs([redacted])")
    }
}

#[async_trait]
impl PrivateInputResolver for GaiaPrivateInputs {
    async fn resolve(
        &self,
        handle: &PrivateInputHandle,
    ) -> Result<ResolvedPrivateInput, AgentBackendError> {
        self.resolve_handle(handle)
    }

    async fn resolve_attachment(
        &self,
        handle: &AttachmentHandle,
    ) -> Result<ResolvedAttachmentSource, AgentBackendError> {
        self.resolve_attachment_handle(handle)
    }
}

fn parse_handle<'a>(
    handle: &'a str,
    kind: &str,
    error_code: &'static str,
) -> Result<&'a str, AgentBackendError> {
    let task_id = handle
        .strip_prefix("gaia:")
        .and_then(|value| value.strip_suffix(&format!(":{kind}")))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| fixed_error(error_code))?;
    Ok(task_id)
}

fn fixed_error(code: &'static str) -> AgentBackendError {
    AgentBackendError::Operation(code.to_owned())
}
