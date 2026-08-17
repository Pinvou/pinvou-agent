use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use agent_backend_api::{
    AgentBackendError, AttachmentHandle, PrivateInputHandle, PrivateInputResolver,
    ResolvedAttachmentSource, ResolvedPrivateInput,
};

use crate::GaiaDataset;

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
            row.question().clone(),
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

impl PrivateInputResolver for GaiaPrivateInputs {
    fn resolve<'life0, 'life1, 'async_trait>(
        &'life0 self,
        handle: &'life1 PrivateInputHandle,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ResolvedPrivateInput, AgentBackendError>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { self.resolve_handle(handle) })
    }

    fn resolve_attachment<'life0, 'life1, 'async_trait>(
        &'life0 self,
        handle: &'life1 AttachmentHandle,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ResolvedAttachmentSource, AgentBackendError>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { self.resolve_attachment_handle(handle) })
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
