use pinvou_upgrade_timing::{
    ClockProjection, FinishedHealthCheck, HealthBudget, HealthCheckHandle, HealthCheckStart,
    HealthClosureAssessment, HealthCompletionAssessment, HealthLimit, HealthWaitAssessment, Millis,
    TimingError, UnknownReason,
};

fn ms(value: u64) -> Millis {
    Millis::from_u64(value).unwrap()
}
fn known(value: u64) -> ClockProjection {
    ClockProjection::Known(ms(value))
}
fn begin(budget: &mut HealthBudget, at: u64) -> HealthCheckHandle {
    match budget.begin_check(known(at)).unwrap() {
        HealthCheckStart::Started(handle) => handle,
        result => panic!("unexpected start: {result:?}"),
    }
}

#[test]
fn total_window_includes_launch_and_waiting_before_checks() {
    let mut budget = HealthBudget::from_first_launch(ms(100)).unwrap();
    assert_eq!(
        budget.assess_waiting(known(100)),
        HealthWaitAssessment::Waiting {
            remaining_total_ms: 300_000,
            remaining_check_ms: None,
        }
    );
    assert_eq!(
        budget.assess_waiting(known(300_099)),
        HealthWaitAssessment::Waiting {
            remaining_total_ms: 1,
            remaining_check_ms: None,
        }
    );
    assert_eq!(
        budget.assess_waiting(known(300_100)),
        HealthWaitAssessment::Exhausted(HealthLimit::Total)
    );
    assert_eq!(
        budget.begin_check(known(300_100)).unwrap(),
        HealthCheckStart::Exhausted(HealthLimit::Total)
    );
    assert_eq!(
        budget.begin_check(known(300_101)).unwrap(),
        HealthCheckStart::Exhausted(HealthLimit::Total)
    );
    assert_eq!(budget.checks_started(), 0);
    assert_eq!(budget.active_check_started_at(), None);
}

#[test]
fn completion_is_inclusive_but_waiting_at_the_same_check_boundary_exhausts() {
    for elapsed in [119_999, 120_000, 120_001] {
        let mut budget = HealthBudget::from_first_launch(ms(100)).unwrap();
        let check = begin(&mut budget, 110);
        let result = budget.complete_check(check, known(110 + elapsed)).unwrap();
        let expected = if elapsed <= 120_000 {
            HealthCompletionAssessment::WithinBounds {
                attempt_number: 1,
                total_elapsed_ms: 10 + elapsed,
                check_elapsed_ms: elapsed,
            }
        } else {
            HealthCompletionAssessment::Exhausted(HealthLimit::Check)
        };
        assert_eq!(result, expected);
        assert_eq!(
            budget.finished_check(check),
            Some(FinishedHealthCheck::Completed(expected))
        );
        assert_eq!(budget.active_check_started_at(), None);
    }
    let mut waiting = HealthBudget::from_first_launch(ms(100)).unwrap();
    let check = begin(&mut waiting, 110);
    assert_eq!(
        waiting.assess_waiting(known(120_109)),
        HealthWaitAssessment::Waiting {
            remaining_total_ms: 179_991,
            remaining_check_ms: Some(1),
        }
    );
    assert_eq!(
        waiting.assess_waiting(known(120_110)),
        HealthWaitAssessment::Exhausted(HealthLimit::Check)
    );
    assert_eq!(
        waiting.complete_check(check, known(120_110)).unwrap(),
        HealthCompletionAssessment::Exhausted(HealthLimit::Check)
    );
}

#[test]
fn second_check_is_cut_short_by_the_original_total_boundary() {
    for finish in [300_099, 300_100, 300_101] {
        let mut budget = HealthBudget::from_first_launch(ms(100)).unwrap();
        let first = begin(&mut budget, 110);
        budget.close_stopped_check(first, known(200)).unwrap();
        let second = begin(&mut budget, 250_100);
        assert_eq!(
            budget.assess_waiting(known(250_100)),
            HealthWaitAssessment::Waiting {
                remaining_total_ms: 50_000,
                remaining_check_ms: Some(50_000),
            }
        );
        let expected = if finish <= 300_100 {
            HealthCompletionAssessment::WithinBounds {
                attempt_number: 2,
                total_elapsed_ms: finish - 100,
                check_elapsed_ms: finish - 250_100,
            }
        } else {
            HealthCompletionAssessment::Exhausted(HealthLimit::Total)
        };
        assert_eq!(
            budget.complete_check(second, known(finish)).unwrap(),
            expected
        );
        assert_eq!(budget.first_launch_at(), ms(100));
        assert_eq!(budget.original_total_deadline().expiry(), ms(300_100));
    }
    let mut waiting = HealthBudget::from_first_launch(ms(100)).unwrap();
    let first = begin(&mut waiting, 110);
    waiting.close_stopped_check(first, known(200)).unwrap();
    let second = begin(&mut waiting, 250_100);
    assert_eq!(
        waiting.assess_waiting(known(300_100)),
        HealthWaitAssessment::Exhausted(HealthLimit::Total)
    );
    assert_eq!(
        waiting.complete_check(second, known(300_100)).unwrap(),
        HealthCompletionAssessment::Exhausted(HealthLimit::Total)
    );
}

#[test]
fn overlap_old_callbacks_duplicates_and_a_third_check_cannot_change_state() {
    let mut budget = HealthBudget::from_first_launch(ms(0)).unwrap();
    let first = begin(&mut budget, 10);
    assert_eq!(
        budget.begin_check(known(20)),
        Err(TimingError::HealthCheckAlreadyActive)
    );
    budget.close_stopped_check(first, known(30)).unwrap();
    assert_eq!(
        budget.complete_check(first, known(35)),
        Err(TimingError::HealthCheckNotActive)
    );
    let second = begin(&mut budget, 40);
    assert_eq!(second.attempt_number(), 2);
    assert_eq!(second.started_at(), ms(40));
    assert_eq!(
        budget.complete_check(first, known(50)),
        Err(TimingError::HealthCheckMismatch)
    );
    assert_eq!(
        budget.close_stopped_check(first, known(50)),
        Err(TimingError::HealthCheckMismatch)
    );
    assert_eq!(budget.last_known_at(), ms(40));
    assert_eq!(budget.active_check_started_at(), Some(ms(40)));
    budget.complete_check(second, known(60)).unwrap();
    assert_eq!(
        budget.begin_check(known(70)),
        Err(TimingError::HealthAttemptsExhausted)
    );
    assert_eq!(budget.last_known_at(), ms(60));
    let budget = budget.clone();
    assert_eq!(budget.checks_started(), 2);
    assert_eq!(budget.last_known_at(), ms(60));
    assert_eq!(budget.first_launch_at(), ms(0));
    assert!(matches!(
        budget.finished_check(first),
        Some(FinishedHealthCheck::Stopped {
            observed_exhaustion: None
        })
    ));
    assert!(matches!(
        budget.finished_check(second),
        Some(FinishedHealthCheck::Completed(
            HealthCompletionAssessment::WithinBounds {
                attempt_number: 2,
                ..
            }
        ))
    ));
    let mut other = HealthBudget::from_first_launch(ms(1)).unwrap();
    let foreign = begin(&mut other, 40);
    assert_eq!(budget.finished_check(foreign), None);
}

#[test]
fn unknown_fault_and_regression_do_not_end_checks_or_consume_attempts() {
    let mut budget = HealthBudget::from_first_launch(ms(100)).unwrap();
    for (projection, reason) in [
        (ClockProjection::Unproven, UnknownReason::ClockUnproven),
        (
            ClockProjection::ConfirmedFault,
            UnknownReason::ComponentFault,
        ),
        (known(99), UnknownReason::ContinuityUnproven),
    ] {
        assert_eq!(
            budget.begin_check(projection).unwrap(),
            HealthCheckStart::Unknown(reason)
        );
        assert_eq!(budget.checks_started(), 0);
        assert_eq!(budget.last_known_at(), ms(100));
    }
    let check = begin(&mut budget, 110);
    budget.assess_waiting(known(120));
    for (projection, reason) in [
        (ClockProjection::Unproven, UnknownReason::ClockUnproven),
        (
            ClockProjection::ConfirmedFault,
            UnknownReason::ComponentFault,
        ),
        (known(119), UnknownReason::ContinuityUnproven),
    ] {
        assert_eq!(
            budget.assess_waiting(projection),
            HealthWaitAssessment::Unknown(reason)
        );
        assert_eq!(
            budget.complete_check(check, projection).unwrap(),
            HealthCompletionAssessment::Unknown(reason)
        );
        assert_eq!(
            budget.close_stopped_check(check, projection).unwrap(),
            HealthClosureAssessment::Unknown(reason)
        );
        assert_eq!(budget.active_check_started_at(), Some(ms(110)));
        assert_eq!(budget.finished_check(check), None);
        assert_eq!(budget.last_known_at(), ms(120));
    }
}

#[test]
fn first_timeout_is_preserved_during_the_only_permitted_retry() {
    let mut budget = HealthBudget::from_first_launch(ms(100)).unwrap();
    let first = begin(&mut budget, 100);
    assert_eq!(
        budget.assess_waiting(known(120_100)),
        HealthWaitAssessment::Exhausted(HealthLimit::Check)
    );
    let mut transferred = budget.clone();
    assert_eq!(transferred.active_check_started_at(), Some(ms(100)));
    assert_eq!(transferred.last_known_at(), ms(120_100));
    assert_eq!(
        transferred
            .close_stopped_check(first, known(120_100))
            .unwrap(),
        HealthClosureAssessment::Closed {
            observed_exhaustion: Some(HealthLimit::Check)
        }
    );
    let second = begin(&mut transferred, 130_100);
    assert_eq!(
        transferred.complete_check(first, known(140_100)),
        Err(TimingError::HealthCheckMismatch)
    );
    assert!(matches!(
        transferred.complete_check(second, known(150_100)).unwrap(),
        HealthCompletionAssessment::WithinBounds {
            attempt_number: 2,
            total_elapsed_ms: 150_000,
            check_elapsed_ms: 20_000
        }
    ));
    assert_eq!(
        transferred.finished_check(first),
        Some(FinishedHealthCheck::Stopped {
            observed_exhaustion: Some(HealthLimit::Check)
        })
    );
    assert_eq!(transferred.original_total_deadline().expiry(), ms(300_100));
    assert_eq!(transferred.first_launch_at(), ms(100));
    assert_eq!(
        transferred.begin_check(known(150_100)),
        Err(TimingError::HealthAttemptsExhausted)
    );
}

#[test]
fn simultaneous_total_and_check_exhaustion_prioritizes_total_and_blocks_retry() {
    let mut budget = HealthBudget::from_first_launch(ms(100)).unwrap();
    let check = begin(&mut budget, 180_100);
    assert_eq!(
        budget.assess_waiting(known(300_100)),
        HealthWaitAssessment::Exhausted(HealthLimit::Total)
    );
    assert_eq!(
        budget.close_stopped_check(check, known(300_100)).unwrap(),
        HealthClosureAssessment::Closed {
            observed_exhaustion: Some(HealthLimit::Total)
        }
    );
    let mut transferred = budget.clone();
    assert_eq!(
        transferred.begin_check(known(300_100)).unwrap(),
        HealthCheckStart::Exhausted(HealthLimit::Total)
    );
    assert_eq!(
        transferred.begin_check(known(300_099)).unwrap(),
        HealthCheckStart::Unknown(UnknownReason::ContinuityUnproven)
    );
    assert_eq!(transferred.checks_started(), 1);
    assert_eq!(transferred.first_launch_at(), ms(100));
}

#[test]
fn safe_max_total_deadline_allows_a_last_millisecond_check_without_false_overflow() {
    const MAX: u64 = 9_007_199_254_740_991;
    let mut budget = HealthBudget::from_first_launch(ms(MAX - 300_000)).unwrap();
    assert_eq!(budget.original_total_deadline().expiry(), ms(MAX));
    let check = begin(&mut budget, MAX - 1);
    assert_eq!(
        budget.assess_waiting(known(MAX - 1)),
        HealthWaitAssessment::Waiting {
            remaining_total_ms: 1,
            remaining_check_ms: Some(1),
        }
    );
    assert_eq!(
        budget.complete_check(check, known(MAX)).unwrap(),
        HealthCompletionAssessment::WithinBounds {
            attempt_number: 1,
            total_elapsed_ms: 300_000,
            check_elapsed_ms: 1,
        }
    );
    assert!(matches!(
        HealthBudget::from_first_launch(ms(MAX - 299_999)),
        Err(TimingError::ArithmeticOverflow)
    ));
}

#[test]
fn unknown_fault_and_regression_cannot_clear_a_latched_equal_boundary_timeout() {
    for (started_at, boundary, limit) in [
        (100, 120_100, HealthLimit::Check),
        (180_100, 300_100, HealthLimit::Total),
    ] {
        let mut budget = HealthBudget::from_first_launch(ms(100)).unwrap();
        let check = begin(&mut budget, started_at);
        assert_eq!(
            budget.assess_waiting(known(boundary)),
            HealthWaitAssessment::Exhausted(limit)
        );
        for (projection, reason) in [
            (ClockProjection::Unproven, UnknownReason::ClockUnproven),
            (
                ClockProjection::ConfirmedFault,
                UnknownReason::ComponentFault,
            ),
            (known(boundary - 1), UnknownReason::ContinuityUnproven),
        ] {
            assert_eq!(
                budget.assess_waiting(projection),
                HealthWaitAssessment::Unknown(reason)
            );
            assert_eq!(
                budget.complete_check(check, projection).unwrap(),
                HealthCompletionAssessment::Unknown(reason)
            );
            assert_eq!(
                budget.close_stopped_check(check, projection).unwrap(),
                HealthClosureAssessment::Unknown(reason)
            );
            assert_eq!(budget.active_check_started_at(), Some(ms(started_at)));
            assert_eq!(budget.last_known_at(), ms(boundary));
            assert_eq!(budget.finished_check(check), None);
        }
        let mut transferred = budget.clone();
        assert_eq!(
            transferred.complete_check(check, known(boundary)).unwrap(),
            HealthCompletionAssessment::Exhausted(limit)
        );
        assert_eq!(
            transferred.finished_check(check),
            Some(FinishedHealthCheck::Completed(
                HealthCompletionAssessment::Exhausted(limit)
            ))
        );
        assert_eq!(
            transferred.complete_check(check, known(boundary)),
            Err(TimingError::HealthCheckNotActive)
        );
    }
}
