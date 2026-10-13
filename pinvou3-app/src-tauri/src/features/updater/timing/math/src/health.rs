use crate::{ClockProjection, Deadline, Millis, TimingError, UnknownReason};

const TOTAL_MS: u64 = 300_000;
const CHECK_MS: u64 = 120_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthLimit {
    Total,
    Check,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthCheckStart {
    Started(HealthCheckHandle),
    Exhausted(HealthLimit),
    Unknown(UnknownReason),
}

/// Not an authorization token. The owner binds the whole budget to its scope
/// and original transaction; this handle distinguishes callbacks within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HealthCheckHandle {
    attempt_number: u8,
    first_launch_at: Millis,
    started_at: Millis,
}

impl HealthCheckHandle {
    pub fn attempt_number(self) -> u8 {
        self.attempt_number
    }
    pub fn started_at(self) -> Millis {
        self.started_at
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthClosureAssessment {
    Closed {
        observed_exhaustion: Option<HealthLimit>,
    },
    Unknown(UnknownReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinishedHealthCheck {
    Completed(HealthCompletionAssessment),
    Stopped {
        observed_exhaustion: Option<HealthLimit>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthWaitAssessment {
    Waiting {
        remaining_total_ms: u64,
        remaining_check_ms: Option<u64>,
    },
    Exhausted(HealthLimit),
    Unknown(UnknownReason),
}

/// WithinBounds establishes only timing, not health or transaction success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthCompletionAssessment {
    WithinBounds {
        attempt_number: u8,
        total_elapsed_ms: u64,
        check_elapsed_ms: u64,
    },
    Exhausted(HealthLimit),
    Unknown(UnknownReason),
}

#[derive(Clone)]
struct ActiveCheck {
    handle: HealthCheckHandle,
    exhausted: bool,
}

/// The protected owner binds these observations to the actual new-version
/// launch/check and proves safe stop before any retry. No process is started here.
#[derive(Clone)]
pub struct HealthBudget {
    first_launch_at: Millis,
    total_deadline: Deadline,
    last_known_at: Millis,
    checks_started: u8,
    active_check: Option<ActiveCheck>,
    finished_checks: [Option<(HealthCheckHandle, FinishedHealthCheck)>; 2],
    total_exhausted: bool,
}

impl HealthBudget {
    pub fn from_first_launch(first_launch_at: Millis) -> Result<Self, TimingError> {
        let total_deadline = Deadline::after(
            first_launch_at,
            crate::BudgetDuration::from_millis(Millis::from_u64(TOTAL_MS)?)?,
        )?;
        Ok(Self {
            first_launch_at,
            total_deadline,
            last_known_at: first_launch_at,
            checks_started: 0,
            active_check: None,
            finished_checks: [None, None],
            total_exhausted: false,
        })
    }

    fn observe(&mut self, projection: ClockProjection) -> Result<Millis, UnknownReason> {
        match projection {
            ClockProjection::Known(now) if now < self.last_known_at => {
                Err(UnknownReason::ContinuityUnproven)
            }
            ClockProjection::Known(now) => {
                self.last_known_at = now;
                Ok(now)
            }
            ClockProjection::Unproven => Err(UnknownReason::ClockUnproven),
            ClockProjection::ConfirmedFault => Err(UnknownReason::ComponentFault),
        }
    }

    pub fn begin_check(
        &mut self,
        projection: ClockProjection,
    ) -> Result<HealthCheckStart, TimingError> {
        if self.active_check.is_some() {
            return Err(TimingError::HealthCheckAlreadyActive);
        }
        if self.checks_started >= 2 {
            return Err(TimingError::HealthAttemptsExhausted);
        }
        let now = match self.observe(projection) {
            Ok(now) => now,
            Err(reason) => return Ok(HealthCheckStart::Unknown(reason)),
        };
        if self.total_exhausted || now >= self.total_deadline.expiry() {
            self.total_exhausted = true;
            return Ok(HealthCheckStart::Exhausted(HealthLimit::Total));
        }
        self.checks_started += 1;
        let handle = HealthCheckHandle {
            attempt_number: self.checks_started,
            first_launch_at: self.first_launch_at,
            started_at: now,
        };
        self.active_check = Some(ActiveCheck {
            handle,
            exhausted: false,
        });
        Ok(HealthCheckStart::Started(handle))
    }

    pub fn assess_waiting(&mut self, projection: ClockProjection) -> HealthWaitAssessment {
        let now = match self.observe(projection) {
            Ok(now) => now,
            Err(reason) => return HealthWaitAssessment::Unknown(reason),
        };
        if self.total_exhausted || now >= self.total_deadline.expiry() {
            self.total_exhausted = true;
            return HealthWaitAssessment::Exhausted(HealthLimit::Total);
        }
        let remaining_check_ms = if let Some(check) = &mut self.active_check {
            let elapsed = now.as_u64() - check.handle.started_at.as_u64();
            if check.exhausted || elapsed >= CHECK_MS {
                check.exhausted = true;
                return HealthWaitAssessment::Exhausted(HealthLimit::Check);
            }
            Some(CHECK_MS - elapsed)
        } else {
            None
        };
        HealthWaitAssessment::Waiting {
            remaining_total_ms: self.total_deadline.expiry().as_u64() - now.as_u64(),
            remaining_check_ms: remaining_check_ms.map(|remaining| {
                remaining.min(self.total_deadline.expiry().as_u64() - now.as_u64())
            }),
        }
    }

    /// Call with an immediate owner-confirmed finished-check observation,
    /// independently of its result. Historical completion timestamps are not
    /// accepted through this API; saved finished records are queried separately.
    /// Unknown keeps the check active. Known completion closes it. At equality,
    /// completion can satisfy the inclusive bound only if exhaustion was not
    /// already observed; a late callback cannot undo an earlier timeout.
    pub fn complete_check(
        &mut self,
        handle: HealthCheckHandle,
        projection: ClockProjection,
    ) -> Result<HealthCompletionAssessment, TimingError> {
        self.require_active(handle)?;
        let now = match self.observe(projection) {
            Ok(now) => now,
            Err(reason) => return Ok(HealthCompletionAssessment::Unknown(reason)),
        };
        let check = self
            .active_check
            .take()
            .ok_or(TimingError::HealthCheckNotActive)?;
        let assessment = if self.total_exhausted || now > self.total_deadline.expiry() {
            self.total_exhausted = true;
            HealthCompletionAssessment::Exhausted(HealthLimit::Total)
        } else {
            let check_elapsed_ms = now.as_u64() - check.handle.started_at.as_u64();
            if check.exhausted || check_elapsed_ms > CHECK_MS {
                HealthCompletionAssessment::Exhausted(HealthLimit::Check)
            } else {
                HealthCompletionAssessment::WithinBounds {
                    attempt_number: handle.attempt_number,
                    total_elapsed_ms: now.as_u64() - self.first_launch_at.as_u64(),
                    check_elapsed_ms,
                }
            }
        };
        self.remember(handle, FinishedHealthCheck::Completed(assessment));
        Ok(assessment)
    }

    /// Closes a check only after the owner has independently proved safe stop.
    /// It produces no timing-eligible completion and grants no restart authority.
    pub fn close_stopped_check(
        &mut self,
        handle: HealthCheckHandle,
        projection: ClockProjection,
    ) -> Result<HealthClosureAssessment, TimingError> {
        self.require_active(handle)?;
        let observed_exhaustion = match self.assess_waiting(projection) {
            HealthWaitAssessment::Unknown(reason) => {
                return Ok(HealthClosureAssessment::Unknown(reason));
            }
            HealthWaitAssessment::Exhausted(limit) => Some(limit),
            HealthWaitAssessment::Waiting { .. } => None,
        };
        self.active_check = None;
        self.remember(
            handle,
            FinishedHealthCheck::Stopped {
                observed_exhaustion,
            },
        );
        Ok(HealthClosureAssessment::Closed {
            observed_exhaustion,
        })
    }

    fn require_active(&self, handle: HealthCheckHandle) -> Result<(), TimingError> {
        match &self.active_check {
            None => Err(TimingError::HealthCheckNotActive),
            Some(check) if check.handle != handle => Err(TimingError::HealthCheckMismatch),
            Some(_) => Ok(()),
        }
    }

    fn remember(&mut self, handle: HealthCheckHandle, result: FinishedHealthCheck) {
        self.finished_checks[usize::from(handle.attempt_number - 1)] = Some((handle, result));
    }

    pub fn finished_check(&self, handle: HealthCheckHandle) -> Option<FinishedHealthCheck> {
        self.finished_checks
            .iter()
            .flatten()
            .find(|(stored, _)| *stored == handle)
            .map(|(_, result)| *result)
    }

    pub fn first_launch_at(&self) -> Millis {
        self.first_launch_at
    }
    pub fn original_total_deadline(&self) -> Deadline {
        self.total_deadline
    }
    pub fn last_known_at(&self) -> Millis {
        self.last_known_at
    }
    pub fn checks_started(&self) -> u8 {
        self.checks_started
    }
    pub fn active_check_started_at(&self) -> Option<Millis> {
        self.active_check
            .as_ref()
            .map(|check| check.handle.started_at)
    }
}
