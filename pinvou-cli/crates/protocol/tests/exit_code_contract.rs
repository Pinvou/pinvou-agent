use pinvou_protocol::{ExitCause, StableExitCode};

#[test]
fn exit_codes_are_stable_from_zero_through_eight() {
    assert_eq!(StableExitCode::Success.as_i32(), 0);
    assert_eq!(StableExitCode::Internal.as_i32(), 1);
    assert_eq!(StableExitCode::Usage.as_i32(), 2);
    assert_eq!(StableExitCode::ControllerUnavailable.as_i32(), 3);
    assert_eq!(StableExitCode::BlockedAuth.as_i32(), 4);
    assert_eq!(StableExitCode::RuntimeFailed.as_i32(), 5);
    assert_eq!(StableExitCode::Cancelled.as_i32(), 6);
    assert_eq!(StableExitCode::ResourceExhausted.as_i32(), 7);
    assert_eq!(StableExitCode::DataCorruption.as_i32(), 8);
}

#[test]
fn the_earliest_cause_wins_and_unmapped_errors_fall_back_to_one() {
    assert_eq!(
        StableExitCode::from_causal_chain([
            ExitCause::ControllerUnavailable,
            ExitCause::BlockedAuth
        ]),
        StableExitCode::ControllerUnavailable
    );
    assert_eq!(
        StableExitCode::from_causal_chain([ExitCause::BlockedAuth, ExitCause::RuntimeFailed]),
        StableExitCode::BlockedAuth
    );
    assert_eq!(
        StableExitCode::from_causal_chain([
            ExitCause::BlockedAuth,
            ExitCause::ControllerUnavailable
        ]),
        StableExitCode::BlockedAuth
    );
    assert_eq!(
        StableExitCode::from_causal_chain([ExitCause::Internal, ExitCause::ControllerUnavailable]),
        StableExitCode::Internal
    );
    assert_eq!(
        StableExitCode::from_causal_chain([ExitCause::Unmapped]),
        StableExitCode::Internal
    );
    assert_eq!(
        StableExitCode::from_causal_chain([]),
        StableExitCode::Internal
    );
}
