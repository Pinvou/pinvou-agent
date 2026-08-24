use std::path::PathBuf;

use pinvou_protocol::RuntimeEventEnvelope;
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeStatus {
    pub id: String,
    pub display_name: String,
    pub available: bool,
}

impl RuntimeStatus {
    pub fn new(id: impl Into<String>, display_name: impl Into<String>, available: bool) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            available,
        }
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

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BackendError {
    #[error("backend is unavailable: {0}")]
    Unavailable(String),
    #[error("backend operation failed: {0}")]
    Operation(String),
    #[error("runtime protocol error: {0}")]
    Protocol(String),
}

pub type EventEmitter = Box<dyn FnMut(RuntimeEventEnvelope) -> Result<(), BackendError> + Send>;

/// Narrow TUI-owned boundary implemented by a Controller adapter.
pub trait Backend: Send + Sync + 'static {
    fn workspace(&self) -> Result<PathBuf, BackendError>;
    fn runtime_list(&self) -> Result<RuntimeList, BackendError>;
    fn stream_turn(&self, prompt: String, emit: EventEmitter) -> Result<(), BackendError>;
    fn resolve_approval(&self, approval_id: String, accepted: bool) -> Result<(), BackendError>;
    fn resolve_input(&self, input_id: String, value: String) -> Result<(), BackendError>;
    fn interrupt(&self, turn_id: String) -> Result<(), BackendError>;
    fn switch_runtime(&self, runtime: String) -> Result<RuntimeStatus, BackendError>;
}
