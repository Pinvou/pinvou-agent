use pinvou_upgrade_timing::{
    BudgetDuration, ClockProjection, ConsumeRecoveryBudget, Deadline, DeadlineAssessment,
    ExecutionPurpose, Millis, PreparationBudget, PreparationKind, StageBudget, StageKind,
    TimingError, UnknownReason,
};

const MAX: u64 = 9_007_199_254_740_991;
fn ms(value: u64) -> Millis {
    Millis::from_u64(value).unwrap()
}
fn duration(value: u64) -> BudgetDuration {
    BudgetDuration::from_millis(ms(value)).unwrap()
}
fn known(value: u64) -> ClockProjection {
    ClockProjection::Known(ms(value))
}

#[test]
fn finite_safe_integer_millis_and_positive_budget_are_distinct() {
    for value in [0, 1, MAX] {
        assert_eq!(
            Millis::from_protocol_number(value as f64).unwrap(),
            ms(value)
        );
    }
    assert_eq!(Millis::from_protocol_number(-0.0).unwrap(), ms(0));
    for value in [
        -1.0,
        0.5,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        (MAX + 1) as f64,
    ] {
        assert_eq!(
            Millis::from_protocol_number(value),
            Err(TimingError::InvalidMillis)
        );
    }
    for value in [MAX + 1, u64::MAX] {
        assert_eq!(Millis::from_u64(value), Err(TimingError::InvalidMillis));
    }
    assert_eq!(
        BudgetDuration::from_millis(ms(0)),
        Err(TimingError::ZeroBudget)
    );
    assert_eq!(
        BudgetDuration::from_millis(Millis::from_protocol_number(-0.0).unwrap()),
        Err(TimingError::ZeroBudget)
    );
    assert_eq!(duration(1).as_millis(), ms(1));
    assert_eq!(duration(MAX).as_millis(), ms(MAX));
    assert_eq!(
        TimingError::InvalidMillis.to_string(),
        "TIME_MILLIS_INVALID"
    );
}

#[test]
fn deadline_before_equal_after_and_unknown_fault_do_not_share_a_zero_remaining() {
    let deadline = Deadline::at(ms(100));
    assert_eq!(
        deadline.assess(known(99)),
        DeadlineAssessment::Before { remaining_ms: 1 }
    );
    assert_eq!(deadline.assess(known(100)), DeadlineAssessment::Reached);
    assert_eq!(deadline.assess(known(101)), DeadlineAssessment::Reached);
    assert_eq!(
        Deadline::at(ms(0)).assess(known(0)),
        DeadlineAssessment::Reached
    );
    assert_eq!(
        deadline.assess(ClockProjection::default()),
        DeadlineAssessment::Unknown(UnknownReason::ClockUnproven)
    );
    assert_eq!(
        deadline.assess(ClockProjection::ConfirmedFault),
        DeadlineAssessment::Unknown(UnknownReason::ComponentFault)
    );
    assert_eq!(deadline.expiry(), ms(100));
}

#[test]
fn original_deadline_addition_accepts_exact_safe_max_and_refuses_overflow() {
    assert_eq!(
        Deadline::after(ms(0), duration(MAX)).unwrap().expiry(),
        ms(MAX)
    );
    assert_eq!(
        Deadline::after(ms(MAX - 1), duration(1)).unwrap().expiry(),
        ms(MAX)
    );
    assert_eq!(
        Deadline::after(ms(MAX), duration(1)),
        Err(TimingError::ArithmeticOverflow)
    );
    assert_eq!(
        Deadline::after(ms(MAX - 1), duration(2)),
        Err(TimingError::ArithmeticOverflow)
    );
}

#[test]
fn preparations_have_independent_starts_and_take_the_earlier_original_cutoff() {
    let mut coordination = PreparationBudget::start(
        PreparationKind::CoordinationAndFreeze,
        ms(100),
        duration(50),
        ms(200),
    )
    .unwrap();
    assert_eq!(coordination.effective_deadline().expiry(), ms(150));
    assert_eq!(
        coordination.assess(known(130)),
        DeadlineAssessment::Before { remaining_ms: 20 }
    );
    let mut backup =
        PreparationBudget::start(PreparationKind::Backup, ms(130), duration(40), ms(160)).unwrap();
    assert_eq!(backup.effective_deadline().expiry(), ms(160));
    assert_eq!(
        backup.assess(known(130)),
        DeadlineAssessment::Before { remaining_ms: 30 }
    );
    assert_eq!(coordination.first_started_at(), ms(100));
    assert_eq!(backup.first_started_at(), ms(130));
    assert_ne!(coordination.kind(), backup.kind());
    assert_eq!(coordination.original_deadline().expiry(), ms(150));
    assert_eq!(backup.original_deadline().expiry(), ms(170));
    for session_expiry in [99, 100] {
        assert!(matches!(
            PreparationBudget::start(
                PreparationKind::Backup,
                ms(100),
                duration(1),
                ms(session_expiry)
            ),
            Err(TimingError::SessionNotActiveAtStart)
        ));
    }
    assert!(matches!(
        PreparationBudget::start(PreparationKind::Backup, ms(MAX - 1), duration(2), ms(MAX)),
        Err(TimingError::ArithmeticOverflow)
    ));
}

#[test]
fn a_new_session_cannot_extend_or_revive_an_existing_preparation() {
    let mut budget = PreparationBudget::start(
        PreparationKind::CoordinationAndFreeze,
        ms(100),
        duration(100),
        ms(250),
    )
    .unwrap();
    budget.tighten_session_expiry(ms(180)).unwrap();
    assert_eq!(budget.effective_deadline().expiry(), ms(180));
    assert_eq!(budget.original_deadline().expiry(), ms(200));
    for later in [190, 250, 400] {
        assert_eq!(
            budget.tighten_session_expiry(ms(later)),
            Err(TimingError::DeadlineExtension)
        );
        assert_eq!(budget.effective_deadline().expiry(), ms(180));
    }
    budget.tighten_session_expiry(ms(180)).unwrap();
    assert_eq!(
        budget.assess(known(179)),
        DeadlineAssessment::Before { remaining_ms: 1 }
    );
    assert_eq!(budget.assess(known(180)), DeadlineAssessment::Reached);
    let mut transferred = budget.clone();
    assert_eq!(transferred.first_started_at(), ms(100));
    assert_eq!(transferred.duration(), duration(100));
    assert_eq!(
        transferred.assess(known(179)),
        DeadlineAssessment::Unknown(UnknownReason::ContinuityUnproven)
    );
    assert_eq!(transferred.assess(known(181)), DeadlineAssessment::Reached);
    transferred.tighten_session_expiry(ms(90)).unwrap();
    assert_eq!(transferred.assess(known(182)), DeadlineAssessment::Reached);
}

#[test]
fn actual_validation_and_three_execution_budgets_obey_their_own_caps() {
    for (kind, cap) in [
        (StageKind::FullArtifactValidation, 1_800_000),
        (
            StageKind::Execution(ExecutionPurpose::Preinstall),
            7_200_000,
        ),
        (StageKind::Execution(ExecutionPurpose::Install), 7_200_000),
        (StageKind::Execution(ExecutionPurpose::Activate), 7_200_000),
    ] {
        for amount in [1, cap] {
            let mut budget = StageBudget::start(kind, ms(100), duration(amount)).unwrap();
            assert_eq!(budget.kind(), kind);
            assert_eq!(budget.duration(), duration(amount));
            assert_eq!(
                budget.assess(known(100 + amount - 1)),
                DeadlineAssessment::Before { remaining_ms: 1 }
            );
            assert_eq!(
                budget.assess(known(100 + amount)),
                DeadlineAssessment::Reached
            );
            let mut transferred = budget.clone();
            assert_eq!(transferred.first_started_at(), ms(100));
            assert_eq!(transferred.original_deadline().expiry(), ms(100 + amount));
            assert_eq!(
                transferred.assess(known(101 + amount)),
                DeadlineAssessment::Reached
            );
        }
        assert!(matches!(
            StageBudget::start(kind, ms(100), duration(cap + 1)),
            Err(TimingError::BudgetAboveLimit)
        ));
        assert!(matches!(
            StageBudget::start(kind, ms(MAX), duration(1)),
            Err(TimingError::ArithmeticOverflow)
        ));
    }
}

#[test]
fn unproven_fault_and_regression_preserve_the_original_anchor_and_watermark() {
    let mut budget =
        StageBudget::start(StageKind::FullArtifactValidation, ms(100), duration(100)).unwrap();
    assert_eq!(
        budget.assess(known(99)),
        DeadlineAssessment::Unknown(UnknownReason::ContinuityUnproven)
    );
    assert_eq!(budget.last_known_at(), ms(100));
    assert_eq!(
        budget.assess(known(110)),
        DeadlineAssessment::Before { remaining_ms: 90 }
    );
    assert_eq!(
        budget.assess(ClockProjection::Unproven),
        DeadlineAssessment::Unknown(UnknownReason::ClockUnproven)
    );
    assert_eq!(
        budget.assess(ClockProjection::ConfirmedFault),
        DeadlineAssessment::Unknown(UnknownReason::ComponentFault)
    );
    assert_eq!(budget.last_known_at(), ms(110));
    assert_eq!(
        budget.assess(known(109)),
        DeadlineAssessment::Unknown(UnknownReason::ContinuityUnproven)
    );
    assert_eq!(budget.last_known_at(), ms(110));
    assert_eq!(budget.original_deadline().expiry(), ms(200));
    assert_eq!(budget.assess(known(200)), DeadlineAssessment::Reached);
    let mut restored_projection = budget.clone();
    assert_eq!(
        restored_projection.assess(ClockProjection::Unproven),
        DeadlineAssessment::Unknown(UnknownReason::ClockUnproven)
    );
    assert_eq!(
        restored_projection.assess(ClockProjection::ConfirmedFault),
        DeadlineAssessment::Unknown(UnknownReason::ComponentFault)
    );
    assert_eq!(
        restored_projection.assess(known(150)),
        DeadlineAssessment::Unknown(UnknownReason::ContinuityUnproven)
    );
    assert_eq!(
        restored_projection.assess(known(201)),
        DeadlineAssessment::Reached
    );
    assert_eq!(restored_projection.first_started_at(), ms(100));
    assert_eq!(restored_projection.original_deadline().expiry(), ms(200));
}

#[test]
fn unknown_consume_wait_is_five_minutes_from_first_send_without_refresh() {
    let mut budget = ConsumeRecoveryBudget::from_first_consume_send(ms(100)).unwrap();
    assert_eq!(
        budget.assess(known(100)),
        DeadlineAssessment::Before {
            remaining_ms: 300_000
        }
    );
    assert_eq!(
        budget.assess(known(300_099)),
        DeadlineAssessment::Before { remaining_ms: 1 }
    );
    assert_eq!(budget.assess(known(300_100)), DeadlineAssessment::Reached);
    let mut transferred = budget.clone();
    assert_eq!(transferred.first_sent_at(), ms(100));
    assert_eq!(transferred.original_deadline().expiry(), ms(300_100));
    assert_eq!(
        transferred.assess(known(300_099)),
        DeadlineAssessment::Unknown(UnknownReason::ContinuityUnproven)
    );
    assert_eq!(
        transferred.assess(known(300_101)),
        DeadlineAssessment::Reached
    );
    assert_eq!(transferred.last_known_at(), ms(300_101));
    assert_eq!(
        ConsumeRecoveryBudget::from_first_consume_send(ms(MAX - 300_000))
            .unwrap()
            .original_deadline()
            .expiry(),
        ms(MAX)
    );
    assert!(matches!(
        ConsumeRecoveryBudget::from_first_consume_send(ms(MAX - 299_999)),
        Err(TimingError::ArithmeticOverflow)
    ));
}
