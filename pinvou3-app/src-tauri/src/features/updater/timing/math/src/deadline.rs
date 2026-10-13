use crate::{BudgetDuration, MAX_SAFE_MILLIS, Millis, TimingError};

/// Source trust/continuity must already be established by the protected owner.
/// No implementation here reads ordinary system dates or supplies a provider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClockProjection {
    Known(Millis),
    #[default]
    Unproven,
    ConfirmedFault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownReason {
    ClockUnproven,
    ComponentFault,
    ContinuityUnproven,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadlineAssessment {
    Before { remaining_ms: u64 },
    Reached,
    // Unknown carries neither a guessed remaining value nor a cleanup decision.
    Unknown(UnknownReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Deadline(Millis);

impl Deadline {
    pub fn at(original_expiry: Millis) -> Self {
        Self(original_expiry)
    }

    pub fn after(origin: Millis, duration: BudgetDuration) -> Result<Self, TimingError> {
        let expiry = origin
            .as_u64()
            .checked_add(duration.as_millis().as_u64())
            .filter(|expiry| *expiry <= MAX_SAFE_MILLIS)
            .ok_or(TimingError::ArithmeticOverflow)?;
        Ok(Self(Millis::from_u64(expiry)?))
    }

    pub fn expiry(self) -> Millis {
        self.0
    }

    /// Stateless comparison requires caller-proven source continuity. Budget
    /// records additionally protect their own observed high-water value.
    pub fn assess(self, projection: ClockProjection) -> DeadlineAssessment {
        match projection {
            ClockProjection::Known(now) if now >= self.0 => DeadlineAssessment::Reached,
            ClockProjection::Known(now) => DeadlineAssessment::Before {
                remaining_ms: self.0.as_u64() - now.as_u64(),
            },
            ClockProjection::Unproven => DeadlineAssessment::Unknown(UnknownReason::ClockUnproven),
            ClockProjection::ConfirmedFault => {
                DeadlineAssessment::Unknown(UnknownReason::ComponentFault)
            }
        }
    }
}
