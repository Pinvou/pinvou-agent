use crate::{
    BudgetDuration, CONSUME_WAIT_MS, ClockProjection, Deadline, DeadlineAssessment,
    EXECUTION_LIMIT_MS, Millis, TimingError, UnknownReason, VALIDATION_LIMIT_MS,
};

#[derive(Clone)]
struct BudgetWindow {
    first_started_at: Millis,
    duration: BudgetDuration,
    original_deadline: Deadline,
    last_known_at: Millis,
}

impl BudgetWindow {
    fn start(first_started_at: Millis, duration: BudgetDuration) -> Result<Self, TimingError> {
        Ok(Self {
            first_started_at,
            duration,
            original_deadline: Deadline::after(first_started_at, duration)?,
            last_known_at: first_started_at,
        })
    }

    fn observe(&mut self, projection: ClockProjection, deadline: Deadline) -> DeadlineAssessment {
        if let ClockProjection::Known(now) = projection {
            if now < self.last_known_at {
                return DeadlineAssessment::Unknown(UnknownReason::ContinuityUnproven);
            }
            self.last_known_at = now;
        }
        deadline.assess(projection)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationKind {
    CoordinationAndFreeze,
    Backup,
}

/// Captures this preparation's session cutoff. Session authentication/identity,
/// scope ownership and protected persistence remain the caller's responsibilities.
#[derive(Clone)]
pub struct PreparationBudget {
    kind: PreparationKind,
    window: BudgetWindow,
    session_deadline: Deadline,
}

impl PreparationBudget {
    pub fn start(
        kind: PreparationKind,
        first_started_at: Millis,
        qualified_duration: BudgetDuration,
        original_session_expiry: Millis,
    ) -> Result<Self, TimingError> {
        if original_session_expiry <= first_started_at {
            return Err(TimingError::SessionNotActiveAtStart);
        }
        Ok(Self {
            kind,
            window: BudgetWindow::start(first_started_at, qualified_duration)?,
            session_deadline: Deadline::at(original_session_expiry),
        })
    }

    /// A current, tighter session restriction may reduce this window. A new
    /// later session cannot extend or revive an existing preparation record.
    pub fn tighten_session_expiry(&mut self, expiry: Millis) -> Result<(), TimingError> {
        if expiry > self.session_deadline.expiry() {
            return Err(TimingError::DeadlineExtension);
        }
        self.session_deadline = Deadline::at(expiry);
        Ok(())
    }

    pub fn assess(&mut self, projection: ClockProjection) -> DeadlineAssessment {
        let effective = self.window.original_deadline.min(self.session_deadline);
        self.window.observe(projection, effective)
    }

    pub fn kind(&self) -> PreparationKind {
        self.kind
    }
    pub fn first_started_at(&self) -> Millis {
        self.window.first_started_at
    }
    pub fn duration(&self) -> BudgetDuration {
        self.window.duration
    }
    pub fn original_deadline(&self) -> Deadline {
        self.window.original_deadline
    }
    pub fn effective_deadline(&self) -> Deadline {
        self.window.original_deadline.min(self.session_deadline)
    }
    pub fn session_deadline(&self) -> Deadline {
        self.session_deadline
    }
    pub fn last_known_at(&self) -> Millis {
        self.window.last_known_at
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionPurpose {
    Preinstall,
    Install,
    Activate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageKind {
    FullArtifactValidation,
    Execution(ExecutionPurpose),
}

#[derive(Clone)]
pub struct StageBudget {
    kind: StageKind,
    window: BudgetWindow,
}

impl StageBudget {
    /// The caller supplies the actual qualified nonzero budget. Upper limits
    /// are policy bounds, never a substitute for target certification.
    pub fn start(
        kind: StageKind,
        first_started_at: Millis,
        qualified_duration: BudgetDuration,
    ) -> Result<Self, TimingError> {
        let cap = match kind {
            StageKind::FullArtifactValidation => VALIDATION_LIMIT_MS,
            StageKind::Execution(_) => EXECUTION_LIMIT_MS,
        };
        if qualified_duration.as_millis().as_u64() > cap {
            return Err(TimingError::BudgetAboveLimit);
        }
        Ok(Self {
            kind,
            window: BudgetWindow::start(first_started_at, qualified_duration)?,
        })
    }

    pub fn assess(&mut self, projection: ClockProjection) -> DeadlineAssessment {
        self.window
            .observe(projection, self.window.original_deadline)
    }
    pub fn kind(&self) -> StageKind {
        self.kind
    }
    pub fn first_started_at(&self) -> Millis {
        self.window.first_started_at
    }
    pub fn duration(&self) -> BudgetDuration {
        self.window.duration
    }
    pub fn original_deadline(&self) -> Deadline {
        self.window.original_deadline
    }
    pub fn last_known_at(&self) -> Millis {
        self.window.last_known_at
    }
}

/// The original wait begins at the first consume transmission, including an
/// unknown ACK. A status query/retry cannot create a later origin on this object.
#[derive(Clone)]
pub struct ConsumeRecoveryBudget {
    window: BudgetWindow,
}
impl ConsumeRecoveryBudget {
    pub fn from_first_consume_send(first_sent_at: Millis) -> Result<Self, TimingError> {
        let duration = BudgetDuration::from_millis(Millis::from_u64(CONSUME_WAIT_MS)?)?;
        Ok(Self {
            window: BudgetWindow::start(first_sent_at, duration)?,
        })
    }
    pub fn assess(&mut self, projection: ClockProjection) -> DeadlineAssessment {
        self.window
            .observe(projection, self.window.original_deadline)
    }
    pub fn first_sent_at(&self) -> Millis {
        self.window.first_started_at
    }
    pub fn original_deadline(&self) -> Deadline {
        self.window.original_deadline
    }
    pub fn last_known_at(&self) -> Millis {
        self.window.last_known_at
    }
}
