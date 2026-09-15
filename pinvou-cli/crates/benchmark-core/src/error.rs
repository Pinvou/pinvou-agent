use thiserror::Error;

#[derive(Debug, Error)]
pub enum BenchmarkError {
    #[error("benchmark contract violation: {0}")]
    Contract(String),
    #[error("benchmark I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

impl BenchmarkError {
    pub(crate) fn coded(code: impl Into<String>) -> Self {
        Self::Contract(code.into())
    }

    pub fn code(&self) -> &str {
        match self {
            Self::Contract(code) => code,
            Self::Io(_) => "io_failed",
        }
    }
}

pub type Result<T> = std::result::Result<T, BenchmarkError>;
