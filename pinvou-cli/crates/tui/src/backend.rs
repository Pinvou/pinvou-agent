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
    fn runtime_list(&self) -> Result<RuntimeList, BackendError>;
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
    fn resolve_approval(&self, approval_id: String, accepted: bool) -> Result<(), BackendError>;
    fn resolve_input(&self, input_id: String, value: String) -> Result<(), BackendError>;
    fn interrupt(&self, turn_id: String) -> Result<(), BackendError>;
    fn switch_runtime(&self, runtime: String) -> Result<RuntimeStatus, BackendError>;
}
