use crate::MAX_SAFE_MILLIS;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingError {
    InvalidMillis,
    ZeroBudget,
    ArithmeticOverflow,
    BudgetAboveLimit,
    SessionNotActiveAtStart,
    DeadlineExtension,
}

impl fmt::Display for TimingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidMillis => "TIME_MILLIS_INVALID",
            Self::ZeroBudget => "TIME_BUDGET_ZERO",
            Self::ArithmeticOverflow => "TIME_ARITHMETIC_OVERFLOW",
            Self::BudgetAboveLimit => "TIME_BUDGET_ABOVE_LIMIT",
            Self::SessionNotActiveAtStart => "TIME_PREPARATION_SESSION_EXPIRED",
            Self::DeadlineExtension => "TIME_DEADLINE_EXTENSION_REFUSED",
        })
    }
}
impl std::error::Error for TimingError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Millis(u64);

impl Millis {
    pub fn from_u64(value: u64) -> Result<Self, TimingError> {
        if value > MAX_SAFE_MILLIS {
            return Err(TimingError::InvalidMillis);
        }
        Ok(Self(value))
    }

    /// The protocol uses finite binary64 safe integers. Checking numeric shape
    /// does not authenticate the source of a timestamp or elapsed-time record.
    pub fn from_protocol_number(value: f64) -> Result<Self, TimingError> {
        if !value.is_finite()
            || value < 0.0
            || value > MAX_SAFE_MILLIS as f64
            || value.fract() != 0.0
        {
            return Err(TimingError::InvalidMillis);
        }
        Self::from_u64(value as u64)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetDuration(Millis);

impl BudgetDuration {
    pub fn from_millis(value: Millis) -> Result<Self, TimingError> {
        if value.as_u64() == 0 {
            return Err(TimingError::ZeroBudget);
        }
        Ok(Self(value))
    }

    pub fn as_millis(self) -> Millis {
        self.0
    }
}
