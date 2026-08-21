use pinvou_protocol::StableExitCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NodeError {
    #[error("node instance is already running")]
    AlreadyRunning,
    #[error("node IPC protocol version or instance challenge mismatch")]
    ProtocolMismatch,
    #[error("node request is unsupported")]
    UnsupportedRequest,
    #[error("node message is invalid")]
    InvalidMessage,
    #[error("node local IPC is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("node local I/O failed")]
    Io(#[from] std::io::Error),
    #[error("node command usage is invalid")]
    Usage,
}

impl NodeError {
    pub const fn exit_code(&self) -> StableExitCode {
        match self {
            Self::ProtocolMismatch | Self::AlreadyRunning => StableExitCode::ControllerUnavailable,
            Self::UnsupportedRequest | Self::InvalidMessage => StableExitCode::RuntimeFailed,
            Self::Usage => StableExitCode::Usage,
            Self::UnsupportedPlatform | Self::Io(_) => StableExitCode::Internal,
        }
    }
}
