//! Upgrade deadline/budget arithmetic over owner-provided time projections.
//! These types prove no clock provenance, durable storage or execution authority.

mod budget;
mod deadline;
mod millis;

pub use budget::{
    ConsumeRecoveryBudget, ExecutionPurpose, PreparationBudget, PreparationKind, StageBudget,
    StageKind,
};
pub use deadline::{ClockProjection, Deadline, DeadlineAssessment, UnknownReason};
pub use millis::{BudgetDuration, Millis, TimingError};

const MAX_SAFE_MILLIS: u64 = 9_007_199_254_740_991;
const VALIDATION_LIMIT_MS: u64 = 30 * 60 * 1000;
const EXECUTION_LIMIT_MS: u64 = 2 * 60 * 60 * 1000;
const CONSUME_WAIT_MS: u64 = 5 * 60 * 1000;
