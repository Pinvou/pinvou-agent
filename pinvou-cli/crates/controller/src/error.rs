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
    #[error("controller local I/O failed")]
    Io(#[from] std::io::Error),
    #[error("controller request is unsupported")]
    UnsupportedRequest,
    #[error("controller IPC message is invalid")]
    InvalidMessage,
    #[error("controller command usage is invalid")]
    Usage,
}

impl ControllerError {
    pub const fn exit_code(&self) -> StableExitCode {
        match self {
            Self::AlreadyRunning | Self::ProtocolMismatch => StableExitCode::ControllerUnavailable,
            Self::UnsupportedRequest | Self::InvalidMessage => StableExitCode::RuntimeFailed,
            Self::Usage => StableExitCode::Usage,
            Self::UnsupportedPlatform | Self::PathUnavailable | Self::Io(_) => {
                StableExitCode::Internal
            }
        }
    }
}
