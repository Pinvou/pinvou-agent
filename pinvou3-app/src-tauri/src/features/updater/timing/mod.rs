//! Pure arithmetic facade. The caller supplies owner-proven time projections;
//! this feature supplies no clock collector, durable ledger or execution rights.

pub(crate) fn assess_original_deadline(
    original_expiry: pinvou_upgrade_timing::Millis,
    projection: pinvou_upgrade_timing::ClockProjection,
) -> pinvou_upgrade_timing::DeadlineAssessment {
    pinvou_upgrade_timing::Deadline::at(original_expiry).assess(projection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinvou_upgrade_timing::{
        BudgetDuration, ClockProjection, DeadlineAssessment, Millis, PreparationBudget,
        PreparationKind, TimingError, UnknownReason,
    };

    #[test]
    fn feature_facade_distinguishes_reached_unproven_and_confirmed_fault() {
        let deadline = Millis::from_u64(100).unwrap();
        assert_eq!(
            assess_original_deadline(deadline, ClockProjection::Known(deadline)),
            DeadlineAssessment::Reached
        );
        assert_eq!(
            assess_original_deadline(deadline, ClockProjection::Unproven),
            DeadlineAssessment::Unknown(UnknownReason::ClockUnproven)
        );
        assert_eq!(
            assess_original_deadline(deadline, ClockProjection::ConfirmedFault),
            DeadlineAssessment::Unknown(UnknownReason::ComponentFault)
        );
    }

    #[test]
    fn application_graph_keeps_original_preparation_budget_and_session_cutoff() {
        let started = Millis::from_u64(100).unwrap();
        let budget = BudgetDuration::from_millis(Millis::from_u64(100).unwrap()).unwrap();
        let mut original = PreparationBudget::start(
            PreparationKind::Backup,
            started,
            budget,
            Millis::from_u64(150).unwrap(),
        )
        .unwrap();
        assert_eq!(
            original.assess(ClockProjection::Known(Millis::from_u64(149).unwrap())),
            DeadlineAssessment::Before { remaining_ms: 1 }
        );
        let mut transferred = original.clone();
        assert_eq!(
            transferred.tighten_session_expiry(Millis::from_u64(250).unwrap()),
            Err(TimingError::DeadlineExtension)
        );
        assert_eq!(
            transferred.assess(ClockProjection::Known(Millis::from_u64(150).unwrap())),
            DeadlineAssessment::Reached
        );
        assert_eq!(transferred.first_started_at(), started);
    }
}
