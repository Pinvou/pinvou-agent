use pinvou_protocol::StableExitCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ControllerError {
    #[error("controller instance is already running")]
    AlreadyRunning,
    #[error("controller IPC protocol version mismatch")]
    ProtocolMismatch,
    #[error("controller platform is unsupported")]
    UnsupportedPlatform,
    #[error("controller local path is unavailable")]
    PathUnavailable,
    #[error("controller local I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("controller session storage failed: {0}")]
    Storage(#[from] crate::SessionStoreError),
    #[error("controller request is unsupported")]
    UnsupportedRequest,
    #[error("controller IPC message is invalid")]
    InvalidMessage,
    #[error("controller command usage is invalid")]
    Usage,
    #[error("local node exhausted its bounded restart budget")]
    NodeRestartExhausted,
    #[error("{context}: {source}")]
    IoContext {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },
}

impl ControllerError {
    pub const fn exit_code(&self) -> StableExitCode {
        match self {
            Self::AlreadyRunning | Self::ProtocolMismatch => StableExitCode::ControllerUnavailable,
            Self::UnsupportedRequest | Self::InvalidMessage => StableExitCode::RuntimeFailed,
            Self::Usage => StableExitCode::Usage,
            Self::NodeRestartExhausted => StableExitCode::RuntimeFailed,
            Self::UnsupportedPlatform | Self::PathUnavailable | Self::Io(_) | Self::Storage(_) => {
                StableExitCode::Internal
            }
            Self::IoContext { .. } => StableExitCode::Internal,
        }
    }
}

pub(crate) fn io_context(context: &'static str) -> impl FnOnce(std::io::Error) -> ControllerError {
    move |source| ControllerError::IoContext { context, source }
}
