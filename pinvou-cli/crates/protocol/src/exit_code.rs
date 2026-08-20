#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum StableExitCode {
    Success = 0,
    Internal = 1,
    Usage = 2,
    ControllerUnavailable = 3,
    BlockedAuth = 4,
    RuntimeFailed = 5,
    Cancelled = 6,
    ResourceExhausted = 7,
    DataCorruption = 8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitCause {
    Internal,
    Usage,
    ControllerUnavailable,
    BlockedAuth,
    RuntimeFailed,
    Cancelled,
    ResourceExhausted,
    DataCorruption,
    Unmapped,
}

impl StableExitCode {
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    pub fn from_causal_chain(causes: impl IntoIterator<Item = ExitCause>) -> Self {
        causes
            .into_iter()
            .next()
            .map(Self::from_cause)
            .unwrap_or(Self::Internal)
    }

    const fn from_cause(cause: ExitCause) -> Self {
        match cause {
            ExitCause::Internal | ExitCause::Unmapped => Self::Internal,
            ExitCause::Usage => Self::Usage,
            ExitCause::ControllerUnavailable => Self::ControllerUnavailable,
            ExitCause::BlockedAuth => Self::BlockedAuth,
            ExitCause::RuntimeFailed => Self::RuntimeFailed,
            ExitCause::Cancelled => Self::Cancelled,
            ExitCause::ResourceExhausted => Self::ResourceExhausted,
            ExitCause::DataCorruption => Self::DataCorruption,
        }
    }
}
