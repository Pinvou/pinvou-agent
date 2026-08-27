use std::path::PathBuf;

use pinvou_protocol::{RuntimeEventEnvelope, StableExitCode};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeStatus {
    pub id: String,
    pub display_name: String,
    pub available: bool,
    pub capability_summary: Option<String>,
}

impl RuntimeStatus {
    pub fn new(id: impl Into<String>, display_name: impl Into<String>, available: bool) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            available,
            capability_summary: None,
        }
    }

    pub fn with_capability_summary(mut self, summary: impl Into<String>) -> Self {
        self.capability_summary = Some(summary.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeList {
    pub active_runtime: Option<String>,
    pub runtimes: Vec<RuntimeStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionCandidate {
    pub id: String,
    pub title: String,
    pub last_active_at: String,
    pub runtime_id: String,
    pub model_id: Option<String>,
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionList {
    pub sessions: Vec<SessionCandidate>,
}

#[derive(Debug)]
pub struct ResumeResult {
    pub session_id: String,
    pub runtime_id: String,
    pub model_id: Option<String>,
    pub permission_profile: PermissionMode,
    pub attachment_epoch: u64,
    pub cursor: u64,
    pub events: Vec<RuntimeEventEnvelope>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCandidate {
    pub id: String,
    pub display_name: String,
    pub is_default: bool,
    pub available: bool,
    pub provider_id: Option<String>,
    pub provider_display_name: Option<String>,
    pub configured: bool,
    pub requires_api_key: bool,
    pub supported_reasoning_levels: Vec<String>,
    pub default_reasoning_level: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelList {
    pub runtime_id: String,
    pub current_model: Option<String>,
    pub current_reasoning_level: Option<String>,
    pub models: Vec<ModelCandidate>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionMode {
    Request,
    Assisted,
    FullAccess,
}

impl PermissionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Assisted => "assisted",
            Self::FullAccess => "full_access",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionControlStrength {
    Enforced,
    Partial,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionStatus {
    pub current_profile: PermissionMode,
    pub supported_profiles: Vec<PermissionMode>,
    pub control_strength: PermissionControlStrength,
    pub native_mode: Option<String>,
    pub sandbox: Option<String>,
    pub residual_guards: Vec<String>,
    pub evidence_version: String,
}

impl RuntimeList {
    pub fn new(active_runtime: Option<String>, runtimes: Vec<RuntimeStatus>) -> Self {
        Self {
            active_runtime,
            runtimes,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendErrorKind {
    ControllerUnavailable,
    AuthBlocked,
    Cancelled,
    Protocol,
    Operation,
    WorkerPanic,
    Timeout,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{safe_message}")]
pub struct BackendError {
    kind: BackendErrorKind,
    exit_code: Option<StableExitCode>,
    safe_message: String,
}

impl BackendError {
    pub fn new(kind: BackendErrorKind, safe_message: impl Into<String>) -> Self {
        Self {
            kind,
            exit_code: None,
            safe_message: safe_message.into(),
        }
    }

    pub fn with_exit_code(mut self, exit_code: StableExitCode) -> Self {
        self.exit_code = Some(exit_code);
        self
    }

    pub const fn kind(&self) -> BackendErrorKind {
        self.kind
    }

    pub const fn exit_code(&self) -> Option<StableExitCode> {
        self.exit_code
    }

    pub fn safe_message(&self) -> &str {
        &self.safe_message
    }
}

pub type EventEmitter = Box<dyn FnMut(RuntimeEventEnvelope) -> Result<(), BackendError> + Send>;

/// Narrow TUI-owned boundary implemented by a Controller adapter.
pub trait Backend: Send + Sync + 'static {
    fn workspace(&self) -> Result<PathBuf, BackendError>;
    /// Announces a stream lease synchronously before its OS worker is spawned.
    fn begin_stream(&self, _operation_token: u64) -> Result<(), BackendError> {
        Ok(())
    }
    /// Announces a control lease synchronously before its OS worker is spawned.
    fn begin_control(&self, _operation_token: u64) -> Result<(), BackendError> {
        Ok(())
    }
    fn runtime_list(&self, operation_token: u64) -> Result<RuntimeList, BackendError>;
    fn session_list(
        &self,
        _operation_token: u64,
        _query: Option<String>,
    ) -> Result<SessionList, BackendError> {
        Err(unsupported_backend_operation("session listing"))
    }
    fn resume_session(
        &self,
        _operation_token: u64,
        _session_id: String,
    ) -> Result<ResumeResult, BackendError> {
        Err(unsupported_backend_operation("session resume"))
    }
    fn model_list(&self, _operation_token: u64) -> Result<ModelList, BackendError> {
        Err(unsupported_backend_operation("model listing"))
    }
    fn switch_model(
        &self,
        _operation_token: u64,
        _model_id: String,
        _reasoning_level: Option<String>,
    ) -> Result<(), BackendError> {
        Err(unsupported_backend_operation("model switching"))
    }
    fn save_model_credential(
        &self,
        _operation_token: u64,
        _model_id: String,
        _api_key: String,
    ) -> Result<(), BackendError> {
        Err(unsupported_backend_operation("model credential storage"))
    }
    fn permissions(&self, _operation_token: u64) -> Result<PermissionStatus, BackendError> {
        Err(unsupported_backend_operation("permission inspection"))
    }
    fn switch_permissions(
        &self,
        _operation_token: u64,
        _profile: PermissionMode,
        _full_access_confirmed: bool,
    ) -> Result<(), BackendError> {
        Err(unsupported_backend_operation("permission switching"))
    }
    /// Streams one turn. `operation_token` identifies the local subscription, not a runtime turn.
    fn stream_turn(
        &self,
        operation_token: u64,
        prompt: String,
        emit: EventEmitter,
    ) -> Result<(), BackendError>;
    /// Detaches the local stream subscription without interrupting the remote runtime turn.
    ///
    /// Implementations must be idempotent and promptly unblock the matching `stream_turn`
    /// call, normally by closing its local IPC subscription or socket. This is lifecycle cleanup;
    /// it must never be translated into a runtime interrupt request.
    fn detach_stream(&self, operation_token: u64) -> Result<(), BackendError>;
    /// Closes and wakes local in-flight Controller requests without issuing any runtime action.
    ///
    /// Implementations must be idempotent and prompt. A remote decision that was already sent is
    /// not rolled back; this only releases local request sockets/subscriptions during TUI exit.
    fn detach_controls(&self) -> Result<(), BackendError>;
    fn resolve_approval(
        &self,
        operation_token: u64,
        approval_id: String,
        accepted: bool,
    ) -> Result<(), BackendError>;
    fn resolve_input(
        &self,
        operation_token: u64,
        input_id: String,
        value: String,
    ) -> Result<(), BackendError>;
    fn interrupt(&self, operation_token: u64, turn_id: String) -> Result<(), BackendError>;
    fn switch_runtime(
        &self,
        operation_token: u64,
        runtime: String,
    ) -> Result<RuntimeStatus, BackendError>;
}

fn unsupported_backend_operation(operation: &str) -> BackendError {
    BackendError::new(
        BackendErrorKind::Operation,
        format!("{operation} is unavailable for this backend"),
    )
}
