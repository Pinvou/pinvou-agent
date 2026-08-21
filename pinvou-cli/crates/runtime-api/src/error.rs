use pinvou_protocol::StableExitCode;
use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum AdapterError {
    #[error("runtime operation '{operation}' is unsupported")]
    Unsupported { operation: &'static str },
    #[error("runtime adapter has not completed capability probing")]
    NotProbed,
    #[error("runtime handshake timed out")]
    HandshakeTimeout,
    #[error("runtime authentication is blocked")]
    BlockedAuth,
    #[error("runtime quota is exhausted")]
    QuotaExceeded,
    #[error("runtime protocol failed: {details}")]
    Protocol {
        code: Option<i64>,
        method: Option<String>,
        details: String,
    },
    #[error("runtime process exited: {details}")]
    ProcessExit {
        code: Option<i32>,
        signal: Option<i32>,
        unexpected_eof: bool,
        details: String,
    },
    #[error("runtime request is invalid: {details}")]
    InvalidRequest { details: String },
    #[error("runtime operation was cancelled")]
    Cancelled,
}

impl AdapterError {
    pub const fn unsupported(operation: &'static str) -> Self {
        Self::Unsupported { operation }
    }

    pub const fn exit_code(&self) -> StableExitCode {
        match self {
            Self::BlockedAuth => StableExitCode::BlockedAuth,
            Self::Cancelled | Self::HandshakeTimeout => StableExitCode::Cancelled,
            Self::QuotaExceeded => StableExitCode::RuntimeFailed,
            Self::Unsupported { .. }
            | Self::Protocol { .. }
            | Self::ProcessExit { .. }
            | Self::InvalidRequest { .. } => StableExitCode::RuntimeFailed,
            Self::NotProbed => StableExitCode::Internal,
        }
    }
}
