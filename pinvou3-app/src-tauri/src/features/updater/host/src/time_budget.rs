//! Internal budget-window DATA, never proof that a stage started, stopped or
//! succeeded. The native owner supplies actual UTC, protected policy and CAS.
use crate::{
    error::{Error, Result},
    json,
    trusted_time::Binding,
};
use serde::{Deserialize, Serialize};

const VALIDATION_MS: u64 = 30 * 60 * 1_000;
const EXECUTION_MS: u64 = 2 * 60 * 60 * 1_000;
const HEALTH_TOTAL_MS: u64 = 5 * 60 * 1_000;
const HEALTH_CHECK_MS: u64 = 120 * 1_000;
const CONSUME_WAIT_MS: u64 = 5 * 60 * 1_000;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TimeInterval {
    lower_ms: u64,
    upper_ms: u64,
}
impl TimeInterval {
    pub(crate) fn new(lower_ms: u64, upper_ms: u64) -> Result<Self> {
        let value = Self { lower_ms, upper_ms };
        value.validate()?;
        Ok(value)
    }
    fn validate(self) -> Result<()> {
        if self.lower_ms > self.upper_ms || self.upper_ms > json::SAFE_MAX {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
    pub(crate) fn lower_ms(self) -> u64 {
        self.lower_ms
    }
    pub(crate) fn upper_ms(self) -> u64 {
        self.upper_ms
    }
}

/// Exactly ten fixed kinds. There is no download or arbitrary stage ordinal.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WindowKind {
    PrepFreeze,
    Backup,
    Validation,
    Preinstall,
    Install,
    Activate,
    HealthTotal,
    HealthCheck1,
    HealthCheck2,
    ConsumeWait,
}
impl WindowKind {
    pub(crate) fn requires_qualified_duration(self) -> bool {
        matches!(
            self,
            Self::PrepFreeze
                | Self::Backup
                | Self::Validation
                | Self::Preinstall
                | Self::Install
                | Self::Activate
        )
    }
    pub(crate) fn requires_session_cutoff(self) -> bool {
        matches!(self, Self::PrepFreeze | Self::Backup)
    }
    pub(crate) fn is_health_check(self) -> bool {
        matches!(self, Self::HealthCheck1 | Self::HealthCheck2)
    }
    fn valid_original_duration(self, duration: u64) -> bool {
        duration > 0
            && duration <= json::SAFE_MAX
            && match self {
                Self::PrepFreeze | Self::Backup => true,
                Self::Validation => duration <= VALIDATION_MS,
                Self::Preinstall | Self::Install | Self::Activate => duration <= EXECUTION_MS,
                Self::HealthTotal => duration == HEALTH_TOTAL_MS,
                Self::HealthCheck1 | Self::HealthCheck2 => duration == HEALTH_CHECK_MS,
                Self::ConsumeWait => duration == CONSUME_WAIT_MS,
            }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WindowId {
    binding: Binding,
    operation_id: String,
    kind: WindowKind,
}
impl WindowId {
    pub(crate) fn new(binding: Binding, operation_id: String, kind: WindowKind) -> Result<Self> {
        let value = Self {
            binding,
            operation_id,
            kind,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<()> {
        self.binding.validate()?;
        if !json::identifier(&self.operation_id) {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
    pub(crate) fn binding(&self) -> &Binding {
        &self.binding
    }
    pub(crate) fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub(crate) fn kind(&self) -> WindowKind {
        self.kind
    }
    fn same_operation(&self, other: &Self) -> bool {
        self.binding == other.binding && self.operation_id == other.operation_id
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HealthLink {
    origin: TimeInterval,
    original_deadline_ms: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BudgetWindow {
    protocol_version: u8,
    id: WindowId,
    origin: TimeInterval,
    original_duration_ms: u64,
    duration_ms: u64,
    original_deadline_ms: u64,
    session_cutoff_ms: Option<u64>,
    health_total: Option<HealthLink>,
    health_total_cutoff_ms: Option<u64>,
    previous_check: Option<HealthLink>,
    last_observation: TimeInterval,
    spent_max_ms: u64,
    exhausted: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Assessment {
    Before { remaining_ms: u64 },
    Reached,
    Unknown,
}

fn plus(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .filter(|n| *n <= json::SAFE_MAX)
        .ok_or(Error::StateCorrupt)
}

impl BudgetWindow {
    /// Explicit qualified duration and original session limit, with no default.
    /// This records a budget intent, not T28/T32/T36 stage-start authority.
    pub(crate) fn begin_qualified(
        id: WindowId,
        origin: TimeInterval,
        duration_ms: u64,
        session_cutoff_ms: Option<u64>,
    ) -> Result<Self> {
        if !id.kind.requires_qualified_duration() {
            return Err(Error::StateCorrupt);
        }
        Self::begin(id, origin, duration_ms, session_cutoff_ms, None, None, None)
    }
    pub(crate) fn begin_health_total(id: WindowId, origin: TimeInterval) -> Result<Self> {
        if id.kind != WindowKind::HealthTotal {
            return Err(Error::StateCorrupt);
        }
        Self::begin(id, origin, HEALTH_TOTAL_MS, None, None, None, None)
    }
    /// Check two must retain check one's original identity/time. This is not a
    /// claim that check one or its real process has stopped; the stage owner
    /// must combine that separate actual fact in its own transaction.
    pub(crate) fn begin_health_check(
        id: WindowId,
        origin: TimeInterval,
        total: &Self,
        previous: Option<&Self>,
    ) -> Result<Self> {
        total.validate()?;
        origin.validate()?;
        if !id.kind.is_health_check()
            || total.id.kind != WindowKind::HealthTotal
            || !id.same_operation(&total.id)
            || origin.lower_ms < total.origin.lower_ms
            || origin.lower_ms < total.last_observation.lower_ms
            || origin.upper_ms >= total.effective_deadline_ms()?
            || total.exhausted
        {
            return Err(Error::StateCorrupt);
        }
        let previous = match (id.kind, previous) {
            (WindowKind::HealthCheck1, None) => None,
            (WindowKind::HealthCheck2, Some(prior)) => {
                prior.validate()?;
                if prior.id.kind != WindowKind::HealthCheck1
                    || !id.same_operation(&prior.id)
                    || prior.health_total
                        != Some(HealthLink {
                            origin: total.origin,
                            original_deadline_ms: total.original_deadline_ms,
                        })
                    || origin.lower_ms < prior.origin.lower_ms
                {
                    return Err(Error::StateCorrupt);
                }
                Some(HealthLink {
                    origin: prior.origin,
                    original_deadline_ms: prior.original_deadline_ms,
                })
            }
            _ => return Err(Error::StateCorrupt),
        };
        let mut value = Self::begin(
            id,
            origin,
            HEALTH_CHECK_MS,
            None,
            Some(HealthLink {
                origin: total.origin,
                original_deadline_ms: total.original_deadline_ms,
            }),
            Some(total.effective_deadline_ms()?),
            previous,
        )?;
        let upper_high_water =
            plus(total.origin.lower_ms, total.spent_max_ms)?.max(origin.upper_ms);
        if upper_high_water >= value.effective_deadline_ms()? {
            return Err(Error::StateCorrupt);
        }
        value.spent_max_ms = upper_high_water - origin.lower_ms;
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn begin_consume_wait(id: WindowId, origin: TimeInterval) -> Result<Self> {
        if id.kind != WindowKind::ConsumeWait {
            return Err(Error::StateCorrupt);
        }
        Self::begin(id, origin, CONSUME_WAIT_MS, None, None, None, None)
    }
    fn begin(
        id: WindowId,
        origin: TimeInterval,
        duration_ms: u64,
        session_cutoff_ms: Option<u64>,
        health_total: Option<HealthLink>,
        health_total_cutoff_ms: Option<u64>,
        previous_check: Option<HealthLink>,
    ) -> Result<Self> {
        id.validate()?;
        origin.validate()?;
        let original_deadline_ms = plus(origin.lower_ms, duration_ms)?;
        let value = Self {
            protocol_version: 1,
            id,
            origin,
            original_duration_ms: duration_ms,
            duration_ms,
            original_deadline_ms,
            session_cutoff_ms,
            health_total,
            health_total_cutoff_ms,
            previous_check,
            last_observation: origin,
            spent_max_ms: origin.upper_ms - origin.lower_ms,
            exhausted: false,
        };
        value.validate()?;
        if origin.upper_ms >= value.effective_deadline_ms()? {
            return Err(Error::StateCorrupt);
        }
        Ok(value)
    }

    pub(crate) fn id(&self) -> &WindowId {
        &self.id
    }
    pub(crate) fn origin(&self) -> TimeInterval {
        self.origin
    }
    pub(crate) fn original_duration_ms(&self) -> u64 {
        self.original_duration_ms
    }
    pub(crate) fn duration_ms(&self) -> u64 {
        self.duration_ms
    }
    pub(crate) fn original_deadline_ms(&self) -> u64 {
        self.original_deadline_ms
    }
    pub(crate) fn session_cutoff_ms(&self) -> Option<u64> {
        self.session_cutoff_ms
    }
    pub(crate) fn spent_max_ms(&self) -> u64 {
        self.spent_max_ms
    }
    pub(crate) fn exhausted(&self) -> bool {
        self.exhausted
    }
    pub(crate) fn last_observation(&self) -> TimeInterval {
        self.last_observation
    }

    pub(crate) fn effective_deadline_ms(&self) -> Result<u64> {
        let mut deadline = plus(self.origin.lower_ms, self.duration_ms)?;
        if let Some(session) = self.session_cutoff_ms {
            deadline = deadline.min(session);
        }
        if let Some(total) = self.health_total_cutoff_ms {
            deadline = deadline.min(total);
        }
        Ok(deadline)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        self.id.validate()?;
        self.origin.validate()?;
        self.last_observation.validate()?;
        if self.protocol_version != 1
            || !self
                .id
                .kind
                .valid_original_duration(self.original_duration_ms)
            || self.duration_ms == 0
            || self.duration_ms > self.original_duration_ms
            || self.original_deadline_ms != plus(self.origin.lower_ms, self.original_duration_ms)?
            || self.origin.upper_ms >= self.original_deadline_ms
            || self.session_cutoff_ms.is_some() != self.id.kind.requires_session_cutoff()
            || self.session_cutoff_ms.is_some_and(|n| n > json::SAFE_MAX)
            || self.last_observation.lower_ms < self.origin.lower_ms
            || self.spent_max_ms > json::SAFE_MAX
            || self.spent_max_ms < self.origin.upper_ms - self.origin.lower_ms
            || self.spent_max_ms < self.last_observation.upper_ms - self.origin.lower_ms
        {
            return Err(Error::StateCorrupt);
        }
        match (
            self.id.kind,
            self.health_total,
            self.health_total_cutoff_ms,
            self.previous_check,
        ) {
            (kind, Some(total), Some(cutoff), previous) if kind.is_health_check() => {
                total.origin.validate()?;
                if total.original_deadline_ms != plus(total.origin.lower_ms, HEALTH_TOTAL_MS)?
                    || total.origin.upper_ms >= total.original_deadline_ms
                    || self.origin.lower_ms < total.origin.lower_ms
                    || cutoff > total.original_deadline_ms
                    || cutoff > json::SAFE_MAX
                    || (kind == WindowKind::HealthCheck2) != previous.is_some()
                {
                    return Err(Error::StateCorrupt);
                }
                if let Some(previous) = previous {
                    previous.origin.validate()?;
                    if previous.origin.lower_ms < total.origin.lower_ms
                        || previous.origin.lower_ms > self.origin.lower_ms
                        || previous.original_deadline_ms
                            != plus(previous.origin.lower_ms, HEALTH_CHECK_MS)?
                    {
                        return Err(Error::StateCorrupt);
                    }
                }
            }
            (kind, None, None, None) if !kind.is_health_check() => {}
            _ => return Err(Error::StateCorrupt),
        }
        let reached = self.last_observation.lower_ms >= self.effective_deadline_ms()?;
        if self.exhausted != reached {
            return Err(Error::StateCorrupt);
        }
        plus(self.origin.lower_ms, self.spent_max_ms)?;
        Ok(())
    }

    /// Native persistence must compare against its actual retained predecessor,
    /// not merely parse a caller's self-consistent replacement record.
    pub(crate) fn check_successor(&self, next: &Self) -> Result<()> {
        self.validate()?;
        next.validate()?;
        if self.id != next.id
            || self.origin != next.origin
            || self.original_duration_ms != next.original_duration_ms
            || self.original_deadline_ms != next.original_deadline_ms
            || self.health_total != next.health_total
            || self.previous_check != next.previous_check
            || next.duration_ms > self.duration_ms
            || next.spent_max_ms < self.spent_max_ms
            || next.last_observation.lower_ms < self.last_observation.lower_ms
            || self.exhausted && !next.exhausted
        {
            return Err(Error::StateCorrupt);
        }
        fn tightened(before: Option<u64>, after: Option<u64>) -> bool {
            match (before, after) {
                (None, None) => true,
                (Some(a), Some(b)) => b <= a,
                _ => false,
            }
        }
        if !tightened(self.session_cutoff_ms, next.session_cutoff_ms)
            || !tightened(self.health_total_cutoff_ms, next.health_total_cutoff_ms)
        {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }

    fn accept(&mut self, mut next: Self) -> Result<()> {
        next.exhausted |= next.last_observation.lower_ms >= next.effective_deadline_ms()?;
        self.check_successor(&next)?;
        *self = next;
        Ok(())
    }
    pub(crate) fn tighten_duration(&mut self, duration_ms: u64) -> Result<()> {
        if duration_ms == 0 || duration_ms > self.duration_ms {
            return Err(Error::StateCorrupt);
        }
        let mut next = self.clone();
        next.duration_ms = duration_ms;
        self.accept(next)
    }
    pub(crate) fn tighten_session_cutoff(&mut self, cutoff_ms: u64) -> Result<()> {
        if cutoff_ms > json::SAFE_MAX || self.session_cutoff_ms.is_none_or(|old| cutoff_ms > old) {
            return Err(Error::StateCorrupt);
        }
        let mut next = self.clone();
        next.session_cutoff_ms = Some(cutoff_ms);
        self.accept(next)
    }
    /// For checks, the actual current total must be supplied and checked by the
    /// native read set. A historical cloned total is not native authority.
    pub(crate) fn assess(
        &mut self,
        current: Option<TimeInterval>,
        total: Option<&Self>,
    ) -> Result<Assessment> {
        self.validate()?;
        let mut next = self.clone();
        if self.id.kind.is_health_check() {
            let total = total.ok_or(Error::StateCorrupt)?;
            total.validate()?;
            if total.id.kind != WindowKind::HealthTotal
                || !self.id.same_operation(&total.id)
                || self.health_total
                    != Some(HealthLink {
                        origin: total.origin,
                        original_deadline_ms: total.original_deadline_ms,
                    })
            {
                return Err(Error::StateCorrupt);
            }
            next.health_total_cutoff_ms = Some(
                self.health_total_cutoff_ms
                    .ok_or(Error::StateCorrupt)?
                    .min(total.effective_deadline_ms()?),
            );
            let total_upper_high_water = plus(total.origin.lower_ms, total.spent_max_ms)?;
            if total_upper_high_water >= next.origin.lower_ms {
                next.spent_max_ms = next
                    .spent_max_ms
                    .max(total_upper_high_water - next.origin.lower_ms);
            }
            if total.exhausted {
                // Authenticated native callers load this real parent in the
                // same transaction. Its confirmed past reach cannot be undone
                // by an unavailable or narrower new observation of the child.
                next.last_observation = TimeInterval::new(
                    next.last_observation
                        .lower_ms
                        .max(total.last_observation.lower_ms),
                    next.last_observation
                        .upper_ms
                        .max(total.last_observation.upper_ms),
                )?;
                next.spent_max_ms = next
                    .spent_max_ms
                    .max(next.last_observation.upper_ms - next.origin.lower_ms);
            }
        } else if total.is_some() {
            return Err(Error::StateCorrupt);
        }
        if let Some(now) = current {
            now.validate()?;
            if now.lower_ms >= next.last_observation.lower_ms {
                next.spent_max_ms = next.spent_max_ms.max(now.upper_ms - next.origin.lower_ms);
                next.last_observation = now;
            } else {
                self.accept(next)?;
                return Ok(if self.exhausted {
                    Assessment::Reached
                } else {
                    Assessment::Unknown
                });
            }
        }
        self.accept(next)?;
        if self.exhausted {
            return Ok(Assessment::Reached);
        }
        let Some(now) = current else {
            return Ok(Assessment::Unknown);
        };
        let deadline = self.effective_deadline_ms()?;
        // A later, narrower anchor cannot refund an already observed worst-case
        // spend. The exact latest observation remains separate from this bound.
        let upper_high_water = plus(self.origin.lower_ms, self.spent_max_ms)?.max(now.upper_ms);
        if upper_high_water < deadline {
            Ok(Assessment::Before {
                remaining_ms: deadline - upper_high_water,
            })
        } else {
            Ok(Assessment::Unknown)
        }
    }
    /// Retain historical health-family timing knowledge from a related protected
    /// child. This is budget DATA, never a new live observation or health result.
    /// The native owner loads both actual records in the same transaction.
    pub(crate) fn retain_health_child(&mut self, child: &Self) -> Result<()> {
        self.validate()?;
        child.validate()?;
        if self.id.kind != WindowKind::HealthTotal
            || !child.id.kind.is_health_check()
            || !self.id.same_operation(&child.id)
            || child.health_total
                != Some(HealthLink {
                    origin: self.origin,
                    original_deadline_ms: self.original_deadline_ms,
                })
        {
            return Err(Error::StateCorrupt);
        }
        let mut next = self.clone();
        let child_upper = plus(child.origin.lower_ms, child.spent_max_ms)?;
        next.spent_max_ms = next.spent_max_ms.max(child_upper - self.origin.lower_ms);
        next.last_observation = TimeInterval::new(
            next.last_observation
                .lower_ms
                .max(child.last_observation.lower_ms),
            next.last_observation
                .upper_ms
                .max(child.last_observation.upper_ms),
        )?;
        self.accept(next)
    }

    pub(crate) fn record_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        json::encode(self, false)
    }
    pub(crate) fn parse(raw: &[u8]) -> Result<Self> {
        let value: Self = json::decode(raw, false)?;
        value.validate()?;
        if value.record_bytes()? != raw {
            return Err(Error::StateCorrupt);
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // All inputs below are detached model DATA, not SCM, native UTC, target
    // qualification, an actual stage, backup inventory or execution authority.
    fn binding() -> Binding {
        Binding {
            installation_scope_id: "installation-one".into(),
            product: "pinvou".into(),
            component: "agent".into(),
            target_key: "windows-x86_64-nsis-perMachine".into(),
            profile_kind: "production".into(),
            profile_id: "test-profile".into(),
            genesis_seed_sha256: "1".repeat(64),
            genesis_manifest_sha256: "2".repeat(64),
            original_install_operation_id: "original-install".into(),
        }
    }
    fn id(kind: WindowKind) -> WindowId {
        WindowId::new(binding(), "original-operation".into(), kind).unwrap()
    }
    fn at(lower: u64, upper: u64) -> TimeInterval {
        TimeInterval::new(lower, upper).unwrap()
    }
    fn qualified() -> BudgetWindow {
        BudgetWindow::begin_qualified(id(WindowKind::PrepFreeze), at(100, 102), 100, Some(1_000))
            .unwrap()
    }

    #[test]
    fn checked_intervals_and_explicit_durations_reject_zero_overflow_and_wrong_kinds() {
        assert!(TimeInterval::new(2, 1).is_err());
        assert!(TimeInterval::new(0, json::SAFE_MAX + 1).is_err());
        assert!(
            BudgetWindow::begin_qualified(id(WindowKind::PrepFreeze), at(0, 0), 0, Some(100))
                .is_err()
        );
        assert!(
            BudgetWindow::begin_qualified(
                id(WindowKind::Backup),
                at(json::SAFE_MAX, json::SAFE_MAX),
                1,
                Some(json::SAFE_MAX)
            )
            .is_err()
        );
        assert!(
            BudgetWindow::begin_qualified(
                id(WindowKind::Validation),
                at(0, 0),
                VALIDATION_MS + 1,
                None
            )
            .is_err()
        );
        for kind in [
            WindowKind::Preinstall,
            WindowKind::Install,
            WindowKind::Activate,
        ] {
            assert!(
                BudgetWindow::begin_qualified(id(kind), at(0, 0), EXECUTION_MS + 1, None).is_err()
            );
        }
        assert!(
            BudgetWindow::begin_qualified(id(WindowKind::ConsumeWait), at(0, 0), 1, None).is_err()
        );
        assert!(
            BudgetWindow::begin_qualified(id(WindowKind::Backup), at(100, 102), 100, Some(102))
                .is_err()
        );
        assert!(
            BudgetWindow::begin_qualified(id(WindowKind::PrepFreeze), at(0, 0), 100, None).is_err()
        );
        assert!(
            BudgetWindow::begin_qualified(id(WindowKind::Validation), at(0, 0), 100, Some(200))
                .is_err()
        );
        let maximum = BudgetWindow::begin_qualified(
            id(WindowKind::Backup),
            at(0, 0),
            json::SAFE_MAX,
            Some(json::SAFE_MAX),
        )
        .unwrap();
        assert_eq!(maximum.effective_deadline_ms().unwrap(), json::SAFE_MAX);
    }

    #[test]
    fn interval_boundary_unknown_and_sticky_exhaustion_never_refund_a_window() {
        let mut window = qualified();
        assert!(
            window.assess(Some(at(190, 199)), None).unwrap()
                == Assessment::Before { remaining_ms: 1 }
        );
        assert!(window.assess(Some(at(191, 200)), None).unwrap() == Assessment::Unknown);
        let spent = window.spent_max_ms();
        // A tighter later anchor's upper bound cannot refund the old upper=200.
        assert!(window.assess(Some(at(192, 195)), None).unwrap() == Assessment::Unknown);
        assert_eq!(window.spent_max_ms(), spent);
        let old = window.record_bytes().unwrap();
        assert!(window.assess(None, None).unwrap() == Assessment::Unknown);
        assert_eq!(window.record_bytes().unwrap(), old);
        assert!(window.assess(Some(at(191, 198)), None).unwrap() == Assessment::Unknown);
        assert_eq!(window.record_bytes().unwrap(), old);
        assert!(window.assess(Some(at(200, 200)), None).unwrap() == Assessment::Reached);
        assert!(window.exhausted());
        assert!(window.assess(None, None).unwrap() == Assessment::Reached);
        assert!(window.assess(Some(at(199, 199)), None).unwrap() == Assessment::Reached);
    }

    #[test]
    fn only_tightening_preserves_original_begin_and_session_cutoff() {
        let mut window = qualified();
        let original = window.clone();
        window.tighten_duration(80).unwrap();
        window.tighten_session_cutoff(170).unwrap();
        assert_eq!(window.original_duration_ms(), 100);
        assert_eq!(window.original_deadline_ms(), 200);
        assert!(window.origin() == original.origin());
        assert_eq!(window.effective_deadline_ms().unwrap(), 170);
        assert!(original.check_successor(&window).is_ok());
        assert!(
            window.assess(Some(at(160, 169)), None).unwrap()
                == Assessment::Before { remaining_ms: 1 }
        );
        let retained = window.record_bytes().unwrap();
        assert!(window.tighten_duration(81).is_err());
        assert!(window.tighten_session_cutoff(171).is_err());
        assert_eq!(window.record_bytes().unwrap(), retained);
        window.tighten_session_cutoff(160).unwrap();
        assert!(window.exhausted());
        assert!(window.assess(None, None).unwrap() == Assessment::Reached);
    }

    #[test]
    fn successor_rejects_new_identity_new_origin_and_rolled_back_spend() {
        let mut original = qualified();
        original.assess(Some(at(140, 150)), None).unwrap();
        let restarted = BudgetWindow::begin_qualified(
            id(WindowKind::PrepFreeze),
            at(151, 152),
            100,
            Some(1_000),
        )
        .unwrap();
        assert!(original.check_successor(&restarted).is_err());
        let mut changed = original.clone();
        changed.id.operation_id = "new-random-id".into();
        assert!(changed.validate().is_ok());
        assert!(original.check_successor(&changed).is_err());
        changed = original.clone();
        changed.id.binding.installation_scope_id = "other-scope".into();
        assert!(original.check_successor(&changed).is_err());
        let mut narrowed = original.clone();
        narrowed.assess(Some(at(141, 145)), None).unwrap();
        changed = narrowed.clone();
        changed.spent_max_ms = 45;
        assert!(changed.validate().is_ok());
        assert!(original.check_successor(&changed).is_err());
        assert!(original.check_successor(&narrowed).is_ok());
    }

    #[test]
    fn health_checks_bind_original_total_and_predecessor_without_stop_authority() {
        let mut total =
            BudgetWindow::begin_health_total(id(WindowKind::HealthTotal), at(1_000, 1_001))
                .unwrap();
        let mut first = BudgetWindow::begin_health_check(
            id(WindowKind::HealthCheck1),
            at(1_002, 1_003),
            &total,
            None,
        )
        .unwrap();
        assert!(
            BudgetWindow::begin_health_check(
                id(WindowKind::HealthCheck2),
                at(2_000, 2_001),
                &total,
                None
            )
            .is_err()
        );
        let second = BudgetWindow::begin_health_check(
            id(WindowKind::HealthCheck2),
            at(2_000, 2_001),
            &total,
            Some(&first),
        )
        .unwrap();
        assert_eq!(second.original_duration_ms(), HEALTH_CHECK_MS);
        let mut other = first.clone();
        other.id.operation_id = "other-operation".into();
        assert!(
            BudgetWindow::begin_health_check(
                id(WindowKind::HealthCheck2),
                at(2_000, 2_001),
                &total,
                Some(&other)
            )
            .is_err()
        );
        assert!(first.assess(Some(at(1_100, 1_101)), None).is_err());
        total.tighten_duration(2_000).unwrap();
        assert!(
            first.assess(Some(at(2_900, 2_999)), Some(&total)).unwrap()
                == Assessment::Before { remaining_ms: 1 }
        );
        assert_eq!(first.effective_deadline_ms().unwrap(), 3_000);
        assert!(first.assess(Some(at(2_999, 3_000)), Some(&total)).unwrap() == Assessment::Unknown);
        assert!(first.assess(Some(at(3_000, 3_000)), Some(&total)).unwrap() == Assessment::Reached);
        let mut different_total = total.clone();
        different_total.id.binding.installation_scope_id = "other-scope".into();
        assert!(first.assess(None, Some(&different_total)).is_err());
        let mut later_total =
            BudgetWindow::begin_health_total(id(WindowKind::HealthTotal), at(1_000, 1_001))
                .unwrap();
        let mut later_first = BudgetWindow::begin_health_check(
            id(WindowKind::HealthCheck1),
            at(1_002, 1_003),
            &later_total,
            None,
        )
        .unwrap();
        later_total.assess(Some(at(1_100, 121_002)), None).unwrap();
        assert!(
            later_first
                .assess(Some(at(1_101, 1_102)), Some(&later_total))
                .unwrap()
                == Assessment::Unknown
        );
        later_total
            .assess(Some(at(301_000, 301_000)), None)
            .unwrap();
        assert!(later_first.assess(None, Some(&later_total)).unwrap() == Assessment::Reached);
        assert!(later_first.exhausted());
        // These records prove neither that first actually ran nor that it was
        // safely stopped before second. Only the future native stage owner can.
    }

    #[test]
    fn intrinsic_windows_and_all_six_qualified_kinds_have_distinct_fixed_identities() {
        let kinds = [
            WindowKind::PrepFreeze,
            WindowKind::Backup,
            WindowKind::Validation,
            WindowKind::Preinstall,
            WindowKind::Install,
            WindowKind::Activate,
        ];
        let mut records = Vec::new();
        for kind in kinds {
            let cutoff = if kind.requires_session_cutoff() {
                Some(2_000)
            } else {
                None
            };
            let window =
                BudgetWindow::begin_qualified(id(kind), at(10, 11), 1_000, cutoff).unwrap();
            assert!(window.id().kind() == kind);
            assert_eq!(window.id().operation_id(), "original-operation");
            assert!(window.id().binding() == &binding());
            records.push(window);
        }
        for (index, window) in records.iter().enumerate() {
            for other in &records[index + 1..] {
                assert!(window.id() != other.id());
            }
        }
        let mut consume =
            BudgetWindow::begin_consume_wait(id(WindowKind::ConsumeWait), at(50, 51)).unwrap();
        assert_eq!(consume.original_deadline_ms(), 50 + CONSUME_WAIT_MS);
        assert!(!consume.id().kind().requires_session_cutoff());
        assert!(consume.tighten_session_cutoff(1_000).is_err());
        let replay =
            BudgetWindow::begin_consume_wait(id(WindowKind::ConsumeWait), at(60, 61)).unwrap();
        assert!(consume.check_successor(&replay).is_err());
    }

    #[test]
    fn canonical_closed_records_reject_unknown_missing_duplicate_and_tampered_fields() {
        let window = qualified();
        let raw = window.record_bytes().unwrap();
        assert!(BudgetWindow::parse(&raw).unwrap() == window);
        let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        for mode in 0..6 {
            let mut bad = value.clone();
            match mode {
                0 => {
                    bad["extra"] = true.into();
                }
                1 => {
                    bad.as_object_mut().unwrap().remove("healthTotal");
                }
                2 => {
                    bad["id"]["kind"] = "download".into();
                }
                3 => {
                    bad["origin"]["upperMs"] = (json::SAFE_MAX + 1).into();
                }
                4 => {
                    bad["originalDeadlineMs"] = 201.into();
                }
                _ => {
                    bad["sessionCutoffMs"] = serde_json::Value::Null;
                }
            }
            // Do not prevalidate the intentionally unsafe integer in the test
            // writer; the actual record parser must reject these input bytes.
            let bytes = serde_json::to_vec(&bad).unwrap();
            assert!(BudgetWindow::parse(&bytes).is_err());
        }
        let mut noncanonical = vec![b' '];
        noncanonical.extend_from_slice(&raw);
        assert!(BudgetWindow::parse(&noncanonical).is_err());
        let duplicate = String::from_utf8(raw)
            .unwrap()
            .replacen("{", "{\"protocolVersion\":1,", 1);
        assert!(BudgetWindow::parse(duplicate.as_bytes()).is_err());
        let mut third = serde_json::to_value(id(WindowKind::HealthCheck2)).unwrap();
        third["kind"] = "healthCheck3".into();
        assert!(serde_json::from_value::<WindowId>(third).is_err());
    }
    #[test]
    fn health_family_retains_child_upper_spend_before_total_or_second_check_reopens() {
        let mut total =
            BudgetWindow::begin_health_total(id(WindowKind::HealthTotal), at(0, 0)).unwrap();
        let mut first =
            BudgetWindow::begin_health_check(id(WindowKind::HealthCheck1), at(1, 1), &total, None)
                .unwrap();
        // Actual historical DATA may be broad. A narrower fresh anchor cannot
        // refund the same total merely by switching to the second check.
        assert!(
            first
                .assess(Some(at(2, HEALTH_TOTAL_MS)), Some(&total))
                .unwrap()
                == Assessment::Unknown
        );
        total.retain_health_child(&first).unwrap();
        assert_eq!(total.spent_max_ms(), HEALTH_TOTAL_MS);
        assert!(total.assess(Some(at(3, 4)), None).unwrap() == Assessment::Unknown);
        assert!(
            BudgetWindow::begin_health_check(
                id(WindowKind::HealthCheck2),
                at(3, 4),
                &total,
                Some(&first),
            )
            .is_err()
        );
        let mut unrelated = first.clone();
        unrelated.id.operation_id = "another-operation".into();
        assert!(total.retain_health_child(&unrelated).is_err());
        assert!(
            total
                .assess(Some(at(HEALTH_TOTAL_MS, HEALTH_TOTAL_MS)), None)
                .unwrap()
                == Assessment::Reached
        );
        total.retain_health_child(&first).unwrap();
        assert!(total.assess(None, None).unwrap() == Assessment::Reached);
    }
}
