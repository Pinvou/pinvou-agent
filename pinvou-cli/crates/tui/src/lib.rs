pub mod action;
pub mod app;
pub mod backend;
pub mod commands;
pub mod model;
pub mod terminal;
pub mod update;
pub mod view;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pinvou_protocol::{RuntimeEventEnvelope, RuntimeEventKind, StableExitCode};
    use serde_json::{Value, json};

    use crate::{
        action::{Action, ApprovalDecision, Effect},
        backend::{Backend, BackendError, BackendErrorKind, RuntimeList, RuntimeStatus},
        commands::{CommandError, SlashCommand, parse},
        model::{Interaction, Model, OperationToken, Overlay, TranscriptEntry, TurnState},
        update::update,
    };

    fn runtime(id: &str) -> RuntimeStatus {
        RuntimeStatus::new(id, id, true)
    }

    fn event(kind: RuntimeEventKind, payload: Value, seq: u64) -> RuntimeEventEnvelope {
        event_for(kind, payload, seq, Some("turn-1"))
    }

    fn event_for(
        kind: RuntimeEventKind,
        payload: Value,
        seq: u64,
        turn_id: Option<&str>,
    ) -> RuntimeEventEnvelope {
        let control = matches!(
            kind,
            RuntimeEventKind::TurnStarted
                | RuntimeEventKind::TurnEnded
                | RuntimeEventKind::ApprovalRequested
                | RuntimeEventKind::ApprovalResolved
                | RuntimeEventKind::InputRequested
                | RuntimeEventKind::InputResolved
                | RuntimeEventKind::ErrorRaised
                | RuntimeEventKind::StreamAborted
        );
        RuntimeEventEnvelope::from_value(json!({
            "protocol_version": 1,
            "schema_version": 1,
            "node_id": "node-local",
            "logical_session_id": "session-1",
            "attachment_id": "attachment-1",
            "work_id": null,
            "collaborative_run_id": null,
            "stream_id": if control { "control" } else { "main" },
            "turn_id": turn_id,
            "seq": seq,
            "source_span": null,
            "timestamp": "2026-08-24T00:00:00Z",
            "rate_class": if control { "R0" } else { "R1" },
            "kind": kind,
            "payload": payload
        }))
        .expect("valid test runtime event")
    }

    #[test]
    fn streaming_turn_approval_and_completion_are_projected_deterministically() {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));

        let start_effects = update(&mut model, Action::Submit("hello".into()));
        assert!(matches!(
            start_effects.as_slice(),
            [Effect::StartTurn { prompt, .. }] if prompt == "hello"
        ));
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-1"}),
                1,
            ),
        );
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::TextDelta,
                json!({"role": "assistant", "content": "hel"}),
                2,
            ),
        );
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::TextDelta,
                json!({"role": "assistant", "content": "lo"}),
                3,
            ),
        );
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::ApprovalRequested,
                json!({
                    "approval_id": "approval-1",
                    "tool": "shell",
                    "summary": "run cargo test",
                    "options": ["allow", "deny"]
                }),
                4,
            ),
        );
        assert!(matches!(
            model.interaction,
            Interaction::ApprovalPending(ref request) if request.approval_id == "approval-1"
        ));
        let approval_effects = update(
            &mut model,
            Action::ApprovalChosen(ApprovalDecision::AllowOnce),
        );
        assert!(matches!(
            approval_effects.as_slice(),
            [Effect::ResolveApproval {
                approval_id,
                turn_id,
                accepted: true,
                ..
            }] if approval_id == "approval-1" && turn_id == "turn-1"
        ));
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::TurnEnded,
                json!({"end_reason": "completed"}),
                5,
            ),
        );

        assert_eq!(model.transcript.assistant_text(), "hello");
        assert_eq!(model.turn, TurnState::Idle);
    }

    #[test]
    fn input_tool_error_and_runtime_actions_have_explicit_state_and_effects() {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(&mut model, Action::Submit("inspect".into()));
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-1"}),
                1,
            ),
        );
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::ToolCallStarted,
                json!({"tool_id": "tool-1", "name": "read_file"}),
                2,
            ),
        );
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::ToolCallOutputDelta,
                json!({"tool_id": "tool-1", "chunk": "done"}),
                3,
            ),
        );
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::InputRequested,
                json!({"input_id": "input-1", "prompt": "Continue?"}),
                4,
            ),
        );
        assert!(matches!(model.interaction, Interaction::InputPending(_)));
        let input_effects = update(&mut model, Action::InputSubmitted("yes".into()));
        assert!(matches!(
            input_effects.as_slice(),
            [Effect::ResolveInput {
                input_id,
                turn_id,
                value,
                ..
            }] if input_id == "input-1" && turn_id == "turn-1" && value == "yes"
        ));
        assert!(model.transcript.entries().iter().any(|entry| matches!(
            entry,
            TranscriptEntry::Tool { output, .. } if output == "done"
        )));

        let interrupt = update(&mut model, Action::Interrupt);
        assert!(matches!(
            interrupt.as_slice(),
            [Effect::Interrupt { turn_id, operation_token }]
                if turn_id == "turn-1"
                    && model.pending_interrupt.as_ref().map(|pending| pending.operation_token)
                        == Some(*operation_token)
        ));
        assert!(update(&mut model, Action::RuntimeSwitch("claude".into())).is_empty());
        assert!(model.pending_runtime_switch.is_none());

        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::ErrorRaised,
                json!({
                    "code": "runtime_failed",
                    "message": "runtime stopped",
                    "fatal": true,
                    "source": "runtime"
                }),
                5,
            ),
        );
        assert_eq!(model.status_message.as_deref(), Some("runtime stopped"));
        assert_eq!(model.turn, TurnState::Idle);
        assert_eq!(model.interaction, Interaction::None);
    }

    #[test]
    fn interrupt_is_single_flight_retryable_and_cleared_by_terminal_events() {
        let mut model = active_model();
        let first = update(&mut model, Action::Interrupt);
        let first_token = match first.as_slice() {
            [
                Effect::Interrupt {
                    turn_id,
                    operation_token,
                },
            ] if turn_id == "turn-1" => *operation_token,
            other => panic!("unexpected interrupt effects: {other:?}"),
        };
        assert!(update(&mut model, Action::Interrupt).is_empty());

        update(
            &mut model,
            Action::InterruptCompleted {
                turn_id: "turn-1".into(),
                operation_token: first_token,
                result: Err(BackendError::new(
                    BackendErrorKind::ControllerUnavailable,
                    "cancel failed",
                )),
            },
        );
        assert!(model.pending_interrupt.is_none());
        let retry = update(&mut model, Action::Interrupt);
        let retry_token = match retry.as_slice() {
            [
                Effect::Interrupt {
                    operation_token, ..
                },
            ] => *operation_token,
            other => panic!("unexpected retry effects: {other:?}"),
        };
        assert_ne!(retry_token, first_token);

        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::TurnEnded,
                json!({"end_reason": "interrupted"}),
                9,
            ),
        );
        assert!(model.pending_interrupt.is_none());
        assert_eq!(model.turn, TurnState::Idle);
    }

    #[test]
    fn slash_commands_drive_overlays_and_exit_without_advertising_future_commands() {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));

        update(&mut model, Action::Submit("/help".into()));
        let Overlay::Help { ref commands } = model.overlay else {
            panic!("help overlay expected");
        };
        assert_eq!(commands, &["/help", "/runtime", "/exit", "/quit"]);
        assert!(
            !commands
                .iter()
                .any(|command| matches!(*command, "/resume" | "/model" | "/permissions"))
        );

        update(&mut model, Action::Submit("/runtime".into()));
        assert_eq!(model.overlay, Overlay::RuntimeList);
        update(&mut model, Action::Submit("/quit".into()));
        assert!(model.should_quit);

        assert_eq!(parse("plain text"), Ok(None));
        assert_eq!(parse(" /exit "), Ok(Some(SlashCommand::Exit)));
        assert_eq!(parse("/model"), Err(CommandError::Unknown("/model".into())));
    }

    #[test]
    fn backend_is_an_object_safe_tui_owned_port() {
        struct StubBackend;

        impl Backend for StubBackend {
            fn workspace(&self) -> Result<PathBuf, BackendError> {
                Ok(PathBuf::from("workspace"))
            }

            fn runtime_list(&self) -> Result<RuntimeList, BackendError> {
                Ok(RuntimeList::new(
                    Some("codex".into()),
                    vec![runtime("codex")],
                ))
            }

            fn stream_turn(
                &self,
                _operation_token: u64,
                _prompt: String,
                _emit: Box<dyn FnMut(RuntimeEventEnvelope) -> Result<(), BackendError> + Send>,
            ) -> Result<(), BackendError> {
                Ok(())
            }

            fn detach_stream(&self, _operation_token: u64) -> Result<(), BackendError> {
                Ok(())
            }

            fn detach_controls(&self) -> Result<(), BackendError> {
                Ok(())
            }

            fn resolve_approval(
                &self,
                _approval_id: String,
                _accepted: bool,
            ) -> Result<(), BackendError> {
                Ok(())
            }

            fn resolve_input(&self, _input_id: String, _value: String) -> Result<(), BackendError> {
                Ok(())
            }

            fn interrupt(&self, _turn_id: String) -> Result<(), BackendError> {
                Ok(())
            }

            fn switch_runtime(&self, runtime: String) -> Result<RuntimeStatus, BackendError> {
                Ok(RuntimeStatus::new(runtime.clone(), runtime, true))
            }
        }

        let backend: Box<dyn Backend> = Box::new(StubBackend);
        assert_eq!(backend.workspace().unwrap(), PathBuf::from("workspace"));
        assert_eq!(backend.runtime_list().unwrap().runtimes.len(), 1);
        assert!(
            backend
                .stream_turn(1, "hello".into(), Box::new(|_| Ok(())))
                .is_ok()
        );
    }

    #[test]
    fn submit_is_rejected_while_starting_streaming_or_waiting_for_control() {
        let mut starting = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(&mut starting, Action::Submit("first".into()));
        assert!(update(&mut starting, Action::Submit("second".into())).is_empty());
        assert_eq!(starting.transcript.entries().len(), 1);
        assert!(
            starting
                .status_message
                .as_deref()
                .unwrap()
                .contains("active")
        );

        dispatch_current_runtime(
            &mut starting,
            event(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-1"}),
                1,
            ),
        );
        assert!(update(&mut starting, Action::Submit("third".into())).is_empty());
        assert_eq!(starting.transcript.entries().len(), 1);

        request_approval(&mut starting);
        let approval = starting.interaction.clone();
        assert!(update(&mut starting, Action::Submit("fourth".into())).is_empty());
        assert_eq!(starting.interaction, approval);
        assert_eq!(starting.transcript.entries().len(), 1);

        let mut input = active_model();
        dispatch_current_runtime(
            &mut input,
            event(
                RuntimeEventKind::InputRequested,
                json!({"input_id": "input-1", "prompt": "Continue?"}),
                2,
            ),
        );
        let interaction = input.interaction.clone();
        assert!(update(&mut input, Action::Submit("second".into())).is_empty());
        assert_eq!(input.interaction, interaction);
        assert_eq!(input.transcript.entries().len(), 1);
    }

    #[test]
    fn events_from_old_or_unknown_turns_cannot_pollute_the_active_turn() {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(&mut model, Action::Submit("new turn".into()));

        dispatch_current_runtime(
            &mut model,
            event_for(
                RuntimeEventKind::TextDelta,
                json!({"role": "assistant", "content": "too early"}),
                1,
                Some("turn-1"),
            ),
        );
        assert_eq!(model.transcript.assistant_text(), "");
        assert!(
            model
                .diagnostic_message
                .as_deref()
                .unwrap()
                .contains("turn")
        );

        dispatch_current_runtime(
            &mut model,
            event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-1"}),
                2,
                Some("turn-1"),
            ),
        );
        assert!(matches!(
            model.turn,
            TurnState::Streaming { ref turn_id, .. } if turn_id == "turn-1"
        ));

        let operation_token = active_turn_token(&model);
        for foreign in [
            event_for(
                RuntimeEventKind::TextDelta,
                json!({"role": "assistant", "content": "stale"}),
                3,
                Some("old-turn"),
            ),
            event_for(
                RuntimeEventKind::ApprovalRequested,
                json!({
                    "approval_id": "old-approval",
                    "tool": "shell",
                    "summary": "stale",
                    "options": ["allow", "deny"]
                }),
                4,
                Some("old-turn"),
            ),
            event_for(
                RuntimeEventKind::InputRequested,
                json!({"input_id": "old-input", "prompt": "stale"}),
                5,
                Some("old-turn"),
            ),
            event_for(
                RuntimeEventKind::ToolCallStarted,
                json!({"tool_id": "old-tool", "name": "shell"}),
                6,
                Some("old-turn"),
            ),
            event_for(
                RuntimeEventKind::ErrorRaised,
                json!({
                    "code": "old_error",
                    "message": "stale failure",
                    "fatal": true,
                    "source": "runtime"
                }),
                7,
                Some("old-turn"),
            ),
            event_for(
                RuntimeEventKind::TurnEnded,
                json!({"end_reason": "completed"}),
                8,
                Some("old-turn"),
            ),
        ] {
            dispatch_runtime_with_token(&mut model, operation_token, foreign);
        }
        assert_eq!(model.transcript.assistant_text(), "");
        assert_eq!(model.interaction, Interaction::None);
        assert_eq!(model.transcript.entries().len(), 1);
        assert!(matches!(
            model.turn,
            TurnState::Streaming { ref turn_id, .. } if turn_id == "turn-1"
        ));

        dispatch_current_runtime(
            &mut model,
            event_for(
                RuntimeEventKind::TextDelta,
                json!({"role": "assistant", "content": "current"}),
                9,
                Some("turn-1"),
            ),
        );
        assert_eq!(model.transcript.assistant_text(), "current");
    }

    #[test]
    fn turn_started_is_bound_once_and_late_terminal_events_are_ignored() {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        dispatch_runtime_with_token(
            &mut model,
            OperationToken::new(999),
            event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "orphan"}),
                1,
                Some("orphan-turn"),
            ),
        );
        assert_eq!(model.turn, TurnState::Idle);
        assert!(
            model
                .diagnostic_message
                .as_deref()
                .unwrap()
                .contains("token")
        );

        update(&mut model, Action::Submit("hello".into()));
        dispatch_current_runtime(
            &mut model,
            event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "missing-id"}),
                2,
                None,
            ),
        );
        assert!(matches!(model.turn, TurnState::Starting { .. }));
        dispatch_current_runtime(
            &mut model,
            event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-1"}),
                3,
                Some("turn-1"),
            ),
        );
        dispatch_current_runtime(
            &mut model,
            event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "duplicate"}),
                4,
                Some("turn-2"),
            ),
        );
        assert!(matches!(
            model.turn,
            TurnState::Streaming { ref turn_id, .. } if turn_id == "turn-1"
        ));
        dispatch_current_runtime(
            &mut model,
            event_for(
                RuntimeEventKind::TurnEnded,
                json!({"end_reason": "completed"}),
                5,
                Some("turn-1"),
            ),
        );
        assert_eq!(model.turn, TurnState::Idle);

        update(&mut model, Action::Submit("next".into()));
        dispatch_current_runtime(
            &mut model,
            event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-2"}),
                6,
                Some("turn-2"),
            ),
        );
        dispatch_current_runtime(
            &mut model,
            event_for(
                RuntimeEventKind::TurnEnded,
                json!({"end_reason": "completed"}),
                7,
                Some("turn-1"),
            ),
        );
        assert!(matches!(
            model.turn,
            TurnState::Streaming { ref turn_id, .. } if turn_id == "turn-2"
        ));
    }

    #[test]
    fn approval_resolution_is_two_phase_retryable_and_race_safe() {
        let mut model = active_model();
        request_approval(&mut model);

        let effect = update(
            &mut model,
            Action::ApprovalChosen(ApprovalDecision::AllowOnce),
        );
        let (approval_turn, approval_token) = match effect.as_slice() {
            [
                Effect::ResolveApproval {
                    turn_id,
                    operation_token,
                    ..
                },
            ] => (turn_id.clone(), *operation_token),
            other => panic!("unexpected approval effects: {other:?}"),
        };
        assert!(matches!(
            model.interaction,
            Interaction::ApprovalResolving { ref request, .. }
                if request.approval_id == "approval-1" && request.summary == "run command"
        ));
        assert!(
            update(
                &mut model,
                Action::ApprovalChosen(ApprovalDecision::AllowOnce)
            )
            .is_empty()
        );

        let error = BackendError::new(
            BackendErrorKind::ControllerUnavailable,
            "controller connection closed",
        )
        .with_exit_code(StableExitCode::ControllerUnavailable);
        update(
            &mut model,
            Action::ApprovalResolutionCompleted {
                turn_id: approval_turn,
                approval_id: "approval-1".into(),
                operation_token: approval_token,
                result: Err(error.clone()),
            },
        );
        assert!(matches!(model.interaction, Interaction::ApprovalPending(_)));
        assert_eq!(model.last_backend_error.as_ref(), Some(&error));

        let retry_effect = update(
            &mut model,
            Action::ApprovalChosen(ApprovalDecision::AllowOnce),
        );
        let (retry_turn, retry_token) = match retry_effect.as_slice() {
            [
                Effect::ResolveApproval {
                    turn_id,
                    operation_token,
                    ..
                },
            ] => (turn_id.clone(), *operation_token),
            other => panic!("unexpected retry effects: {other:?}"),
        };
        assert_ne!(retry_token, approval_token);
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::ApprovalResolved,
                json!({"approval_id": "approval-1", "outcome": "approved"}),
                3,
            ),
        );
        assert_eq!(model.interaction, Interaction::None);
        update(
            &mut model,
            Action::ApprovalResolutionCompleted {
                turn_id: retry_turn,
                approval_id: "approval-1".into(),
                operation_token: retry_token,
                result: Err(error),
            },
        );
        assert_eq!(model.interaction, Interaction::None);
    }

    #[test]
    fn input_resolution_preserves_request_on_failure_and_clears_on_success() {
        let mut model = active_model();
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::InputRequested,
                json!({"input_id": "input-1", "prompt": "Continue?"}),
                2,
            ),
        );
        let first_effect = update(&mut model, Action::InputSubmitted("yes".into()));
        let (input_turn, first_token) = match first_effect.as_slice() {
            [
                Effect::ResolveInput {
                    turn_id,
                    operation_token,
                    ..
                },
            ] => (turn_id.clone(), *operation_token),
            other => panic!("unexpected input effects: {other:?}"),
        };
        assert!(matches!(
            model.interaction,
            Interaction::InputResolving { ref request, ref value }
                if request.input_id == "input-1" && value == "yes"
        ));
        assert!(update(&mut model, Action::InputSubmitted("again".into())).is_empty());
        assert!(
            model
                .status_message
                .as_deref()
                .unwrap()
                .contains("progress")
        );

        let error = BackendError::new(BackendErrorKind::Operation, "write failed");
        update(
            &mut model,
            Action::InputResolutionCompleted {
                turn_id: input_turn,
                input_id: "input-1".into(),
                operation_token: first_token,
                result: Err(error),
            },
        );
        assert!(matches!(model.interaction, Interaction::InputPending(_)));

        let retry_effect = update(&mut model, Action::InputSubmitted("yes".into()));
        let (retry_turn, retry_token) = match retry_effect.as_slice() {
            [
                Effect::ResolveInput {
                    turn_id,
                    operation_token,
                    ..
                },
            ] => (turn_id.clone(), *operation_token),
            other => panic!("unexpected input retry effects: {other:?}"),
        };
        assert_ne!(retry_token, first_token);
        update(
            &mut model,
            Action::InputResolutionCompleted {
                turn_id: retry_turn,
                input_id: "input-1".into(),
                operation_token: retry_token,
                result: Ok(()),
            },
        );
        assert_eq!(model.interaction, Interaction::None);
    }

    #[test]
    fn backend_error_class_and_stable_exit_code_survive_action_projection() {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        let error = BackendError::new(BackendErrorKind::AuthBlocked, "sign in required")
            .with_exit_code(StableExitCode::BlockedAuth);
        let effects = update(&mut model, Action::RuntimeSwitch("claude".into()));
        let operation_token = match effects.as_slice() {
            [
                Effect::SwitchRuntime {
                    operation_token, ..
                },
            ] => *operation_token,
            other => panic!("unexpected runtime effects: {other:?}"),
        };

        update(
            &mut model,
            Action::RuntimeSwitched {
                operation_token,
                result: Err(error.clone()),
            },
        );

        assert_eq!(model.status_message.as_deref(), Some("sign in required"));
        assert_eq!(model.last_backend_error.as_ref(), Some(&error));
        assert!(model.pending_runtime_switch.is_none());
        assert_eq!(error.kind(), BackendErrorKind::AuthBlocked);
        assert_eq!(error.exit_code(), Some(StableExitCode::BlockedAuth));
    }

    #[test]
    fn runtime_switch_is_idle_only_single_flight_and_token_correlated() {
        let mut active = active_model();
        update(&mut active, Action::Submit("/runtime".into()));
        let overlay = active.overlay.clone();
        assert!(update(&mut active, Action::RuntimeSwitch("claude".into())).is_empty());
        assert_eq!(active.overlay, overlay);
        assert!(active.pending_runtime_switch.is_none());

        let mut starting = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(&mut starting, Action::Submit("hello".into()));
        update(&mut starting, Action::Submit("/runtime".into()));
        let overlay = starting.overlay.clone();
        assert!(update(&mut starting, Action::RuntimeSwitch("claude".into())).is_empty());
        assert_eq!(starting.overlay, overlay);

        let mut approval = active_model();
        request_approval(&mut approval);
        update(&mut approval, Action::Submit("/runtime".into()));
        let interaction = approval.interaction.clone();
        let overlay = approval.overlay.clone();
        assert!(update(&mut approval, Action::RuntimeSwitch("claude".into())).is_empty());
        assert_eq!(approval.interaction, interaction);
        assert_eq!(approval.overlay, overlay);

        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(&mut model, Action::Submit("/runtime".into()));
        let effects = update(&mut model, Action::RuntimeSwitch("claude".into()));
        let operation_token = match effects.as_slice() {
            [
                Effect::SwitchRuntime {
                    runtime,
                    operation_token,
                },
            ] if runtime == "claude" => *operation_token,
            other => panic!("unexpected runtime switch effects: {other:?}"),
        };
        let pending = model.pending_runtime_switch.clone();
        assert!(pending.is_some());
        assert_eq!(model.overlay, Overlay::RuntimeList);
        assert!(update(&mut model, Action::Submit("wait".into())).is_empty());
        assert_eq!(model.pending_runtime_switch, pending);
        assert!(model.transcript.entries().is_empty());

        assert!(update(&mut model, Action::RuntimeSwitch("kimi".into())).is_empty());
        assert_eq!(model.pending_runtime_switch, pending);

        update(
            &mut model,
            Action::RuntimeSwitched {
                operation_token: OperationToken::new(operation_token.as_u64() + 100),
                result: Ok(runtime("kimi")),
            },
        );
        assert_eq!(model.runtime.id, "codex");
        assert_eq!(model.pending_runtime_switch, pending);

        update(
            &mut model,
            Action::RuntimeSwitched {
                operation_token,
                result: Ok(runtime("claude")),
            },
        );
        assert_eq!(model.runtime.id, "claude");
        assert!(model.pending_runtime_switch.is_none());
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn old_control_completion_cannot_mutate_reused_id_in_a_new_turn() {
        let mut model = active_model();
        request_approval(&mut model);
        let old_effects = update(
            &mut model,
            Action::ApprovalChosen(ApprovalDecision::AllowOnce),
        );
        let (old_turn, old_token) = match old_effects.as_slice() {
            [
                Effect::ResolveApproval {
                    turn_id,
                    operation_token,
                    ..
                },
            ] => (turn_id.clone(), *operation_token),
            other => panic!("unexpected old control effects: {other:?}"),
        };
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::TurnEnded,
                json!({"end_reason": "completed"}),
                3,
            ),
        );

        update(&mut model, Action::Submit("second".into()));
        dispatch_current_runtime(
            &mut model,
            event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-2"}),
                4,
                Some("turn-2"),
            ),
        );
        dispatch_current_runtime(
            &mut model,
            event_for(
                RuntimeEventKind::ApprovalRequested,
                json!({
                    "approval_id": "approval-1",
                    "tool": "shell",
                    "summary": "same id, new turn",
                    "options": ["allow", "deny"]
                }),
                5,
                Some("turn-2"),
            ),
        );
        let new_effects = update(
            &mut model,
            Action::ApprovalChosen(ApprovalDecision::AllowOnce),
        );
        let new_token = match new_effects.as_slice() {
            [
                Effect::ResolveApproval {
                    turn_id,
                    operation_token,
                    ..
                },
            ] if turn_id == "turn-2" => *operation_token,
            other => panic!("unexpected new control effects: {other:?}"),
        };
        assert_ne!(old_token, new_token);

        update(
            &mut model,
            Action::ApprovalResolutionCompleted {
                turn_id: old_turn,
                approval_id: "approval-1".into(),
                operation_token: old_token,
                result: Err(BackendError::new(
                    BackendErrorKind::Operation,
                    "stale failure",
                )),
            },
        );
        assert!(matches!(
            model.interaction,
            Interaction::ApprovalResolving { ref request, .. }
                if request.turn_id == "turn-2" && request.operation_token == new_token
        ));
    }

    #[test]
    fn ignored_runtime_diagnostic_does_not_hide_actionable_backend_error() {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        let effects = update(&mut model, Action::RuntimeSwitch("claude".into()));
        let token = match effects.as_slice() {
            [
                Effect::SwitchRuntime {
                    operation_token, ..
                },
            ] => *operation_token,
            other => panic!("unexpected runtime effects: {other:?}"),
        };
        let error = BackendError::new(BackendErrorKind::AuthBlocked, "sign in required")
            .with_exit_code(StableExitCode::BlockedAuth);
        update(
            &mut model,
            Action::RuntimeSwitched {
                operation_token: token,
                result: Err(error.clone()),
            },
        );

        dispatch_runtime_with_token(
            &mut model,
            OperationToken::new(999),
            event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "orphan"}),
                1,
                Some("old-turn"),
            ),
        );

        assert_eq!(model.status_message.as_deref(), Some("sign in required"));
        assert_eq!(model.last_backend_error.as_ref(), Some(&error));
        assert!(
            model
                .diagnostic_message
                .as_deref()
                .unwrap()
                .contains("ignored")
        );
    }

    #[test]
    fn stream_abort_clears_the_active_control_operation() {
        let mut model = active_model();
        request_approval(&mut model);
        update(
            &mut model,
            Action::ApprovalChosen(ApprovalDecision::AllowOnce),
        );
        assert!(matches!(
            model.interaction,
            Interaction::ApprovalResolving { .. }
        ));

        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::StreamAborted,
                json!({"reason": "transport closed"}),
                3,
            ),
        );

        assert_eq!(model.turn, TurnState::Idle);
        assert_eq!(model.interaction, Interaction::None);
        assert!(model.pending_runtime_switch.is_none());
    }

    #[test]
    fn mismatched_runtime_switch_result_is_a_protocol_error_and_never_activates() {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(&mut model, Action::Submit("/runtime".into()));
        let effects = update(&mut model, Action::RuntimeSwitch("claude".into()));
        let token = match effects.as_slice() {
            [
                Effect::SwitchRuntime {
                    operation_token, ..
                },
            ] => *operation_token,
            other => panic!("unexpected runtime effects: {other:?}"),
        };

        update(
            &mut model,
            Action::RuntimeSwitched {
                operation_token: token,
                result: Ok(runtime("kimi")),
            },
        );

        assert_eq!(model.runtime.id, "codex");
        assert!(model.pending_runtime_switch.is_none());
        assert_eq!(model.overlay, Overlay::RuntimeList);
        assert_eq!(
            model.last_backend_error.as_ref().map(BackendError::kind),
            Some(BackendErrorKind::Protocol)
        );
        assert!(model.status_message.as_deref().unwrap().contains("claude"));
        let diagnostic = model.diagnostic_message.as_deref().unwrap();
        assert!(diagnostic.contains("claude") && diagnostic.contains("kimi"));
    }

    #[test]
    fn stream_completion_failure_recovers_starting_and_streaming_turns() {
        let mut starting = Model::new(PathBuf::from("workspace"), runtime("codex"));
        let effects = update(&mut starting, Action::Submit("hello".into()));
        let starting_token = match effects.as_slice() {
            [
                Effect::StartTurn {
                    operation_token, ..
                },
            ] => *operation_token,
            other => panic!("unexpected start effects: {other:?}"),
        };
        let pre_start_error = BackendError::new(
            BackendErrorKind::ControllerUnavailable,
            "controller unavailable",
        );
        update(
            &mut starting,
            Action::TurnStreamCompleted {
                operation_token: starting_token,
                result: Err(pre_start_error.clone()),
            },
        );
        assert_eq!(starting.turn, TurnState::Idle);
        assert_eq!(starting.interaction, Interaction::None);
        assert_eq!(starting.last_backend_error.as_ref(), Some(&pre_start_error));

        let mut streaming = active_model();
        request_approval(&mut streaming);
        let streaming_token = active_turn_token(&streaming);
        let stream_error = BackendError::new(BackendErrorKind::Operation, "stream closed");
        update(
            &mut streaming,
            Action::TurnStreamCompleted {
                operation_token: streaming_token,
                result: Err(stream_error.clone()),
            },
        );
        assert_eq!(streaming.turn, TurnState::Idle);
        assert_eq!(streaming.interaction, Interaction::None);
        assert_eq!(streaming.last_backend_error.as_ref(), Some(&stream_error));
    }

    #[test]
    fn successful_stream_without_terminal_event_is_a_protocol_error() {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        let effects = update(&mut model, Action::Submit("hello".into()));
        let token = match effects.as_slice() {
            [
                Effect::StartTurn {
                    operation_token, ..
                },
            ] => *operation_token,
            other => panic!("unexpected start effects: {other:?}"),
        };

        update(
            &mut model,
            Action::TurnStreamCompleted {
                operation_token: token,
                result: Ok(()),
            },
        );

        assert_eq!(model.turn, TurnState::Idle);
        assert_eq!(
            model.last_backend_error.as_ref().map(BackendError::kind),
            Some(BackendErrorKind::Protocol)
        );
        assert!(
            model
                .status_message
                .as_deref()
                .unwrap()
                .contains("turn.ended")
        );

        let mut streaming = active_model();
        let token = active_turn_token(&streaming);
        update(
            &mut streaming,
            Action::TurnStreamCompleted {
                operation_token: token,
                result: Ok(()),
            },
        );
        assert_eq!(streaming.turn, TurnState::Idle);
        assert_eq!(
            streaming
                .last_backend_error
                .as_ref()
                .map(BackendError::kind),
            Some(BackendErrorKind::Protocol)
        );
    }

    #[test]
    fn terminal_event_then_stream_success_is_normal_but_old_completion_is_isolated() {
        let mut model = active_model();
        let completed_token = active_turn_token(&model);
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::TurnEnded,
                json!({"end_reason": "completed"}),
                2,
            ),
        );
        update(
            &mut model,
            Action::TurnStreamCompleted {
                operation_token: completed_token,
                result: Ok(()),
            },
        );
        assert_eq!(model.turn, TurnState::Idle);
        assert!(model.last_backend_error.is_none());

        let effects = update(&mut model, Action::Submit("next".into()));
        let new_token = match effects.as_slice() {
            [
                Effect::StartTurn {
                    operation_token, ..
                },
            ] => *operation_token,
            other => panic!("unexpected next-turn effects: {other:?}"),
        };
        assert_ne!(completed_token, new_token);
        update(
            &mut model,
            Action::TurnStreamCompleted {
                operation_token: completed_token,
                result: Err(BackendError::new(
                    BackendErrorKind::Operation,
                    "late old failure",
                )),
            },
        );
        assert!(matches!(
            model.turn,
            TurnState::Starting { operation_token } if operation_token == new_token
        ));
        assert!(model.last_backend_error.is_none());
        assert!(
            model
                .diagnostic_message
                .as_deref()
                .unwrap()
                .contains("stale")
        );
    }

    #[test]
    fn runtime_events_are_correlated_by_turn_operation_token_before_projection() {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        let effects = update(&mut model, Action::Submit("hello".into()));
        let current_token = start_turn_token(&effects);
        let stale_token = OperationToken::new(current_token.as_u64() + 100);

        dispatch_runtime_with_token(
            &mut model,
            stale_token,
            event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "stale"}),
                1,
                Some("same-runtime-turn-id"),
            ),
        );
        assert!(matches!(
            model.turn,
            TurnState::Starting { operation_token } if operation_token == current_token
        ));
        assert!(
            model
                .diagnostic_message
                .as_deref()
                .unwrap()
                .contains("token")
        );

        dispatch_runtime_with_token(
            &mut model,
            current_token,
            event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "current"}),
                2,
                Some("same-runtime-turn-id"),
            ),
        );
        dispatch_runtime_with_token(
            &mut model,
            stale_token,
            event_for(
                RuntimeEventKind::TextDelta,
                json!({"role": "assistant", "content": "stale"}),
                3,
                Some("same-runtime-turn-id"),
            ),
        );
        dispatch_runtime_with_token(
            &mut model,
            stale_token,
            event_for(
                RuntimeEventKind::TurnEnded,
                json!({"end_reason": "completed"}),
                4,
                Some("same-runtime-turn-id"),
            ),
        );
        assert_eq!(model.transcript.assistant_text(), "");
        assert!(matches!(
            model.turn,
            TurnState::Streaming { operation_token, .. } if operation_token == current_token
        ));
    }

    #[test]
    fn terminal_status_survives_stream_failure_and_late_runtime_events() {
        let mut fatal = active_model();
        let fatal_token = active_turn_token(&fatal);
        dispatch_runtime_with_token(
            &mut fatal,
            fatal_token,
            event(
                RuntimeEventKind::ErrorRaised,
                json!({
                    "code": "fatal",
                    "message": "runtime stopped",
                    "fatal": true,
                    "source": "runtime"
                }),
                2,
            ),
        );
        update(
            &mut fatal,
            Action::TurnStreamCompleted {
                operation_token: fatal_token,
                result: Err(BackendError::new(
                    BackendErrorKind::Operation,
                    "transport followed fatal",
                )),
            },
        );
        assert_eq!(fatal.status_message.as_deref(), Some("runtime stopped"));
        assert!(
            fatal
                .diagnostic_message
                .as_deref()
                .unwrap()
                .contains("terminal")
        );

        let mut aborted = active_model();
        let aborted_token = active_turn_token(&aborted);
        dispatch_runtime_with_token(
            &mut aborted,
            aborted_token,
            event(
                RuntimeEventKind::StreamAborted,
                json!({"reason": "transport closed"}),
                2,
            ),
        );
        dispatch_runtime_with_token(
            &mut aborted,
            aborted_token,
            event(
                RuntimeEventKind::TextDelta,
                json!({"role": "assistant", "content": "late"}),
                3,
            ),
        );
        assert_eq!(aborted.status_message.as_deref(), Some("transport closed"));
        assert_eq!(aborted.transcript.assistant_text(), "");
        assert!(
            aborted
                .diagnostic_message
                .as_deref()
                .unwrap()
                .contains("token")
        );
    }

    #[test]
    fn abnormal_stream_end_is_not_recorded_as_a_runtime_terminal() {
        let mut model = active_model();
        let token = active_turn_token(&model);
        let error = BackendError::new(BackendErrorKind::Operation, "stream failed");
        update(
            &mut model,
            Action::TurnStreamCompleted {
                operation_token: token,
                result: Err(error.clone()),
            },
        );
        assert_eq!(model.last_terminal_turn_token, None);
        assert_eq!(model.status_message.as_deref(), Some("stream failed"));

        update(
            &mut model,
            Action::TurnStreamCompleted {
                operation_token: token,
                result: Ok(()),
            },
        );
        assert_eq!(model.last_backend_error.as_ref(), Some(&error));
        assert_eq!(model.status_message.as_deref(), Some("stream failed"));
        assert!(
            model
                .diagnostic_message
                .as_deref()
                .unwrap()
                .contains("stale")
        );
    }

    #[test]
    fn overlays_are_idle_only_and_never_cover_actionable_interactions() {
        let mut starting = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(&mut starting, Action::Submit("hello".into()));
        assert!(update(&mut starting, Action::Submit("/runtime".into())).is_empty());
        assert_eq!(starting.overlay, Overlay::None);
        assert!(
            starting
                .status_message
                .as_deref()
                .unwrap()
                .contains("active")
        );
        assert!(update(&mut starting, Action::Submit("/help".into())).is_empty());
        assert_eq!(starting.overlay, Overlay::None);

        let mut approval = active_model();
        request_approval(&mut approval);
        assert!(matches!(
            approval.interaction,
            Interaction::ApprovalPending(_)
        ));
        assert!(update(&mut approval, Action::Submit("/runtime".into())).is_empty());
        assert_eq!(approval.overlay, Overlay::None);

        let mut stale_approval_overlay = active_model();
        stale_approval_overlay.overlay = Overlay::RuntimeList;
        request_approval(&mut stale_approval_overlay);
        assert_eq!(stale_approval_overlay.overlay, Overlay::None);

        let mut stale_input_overlay = active_model();
        stale_input_overlay.overlay = Overlay::Help {
            commands: vec!["/help"],
        };
        dispatch_current_runtime(
            &mut stale_input_overlay,
            event(
                RuntimeEventKind::InputRequested,
                json!({"input_id": "input-1", "prompt": "Continue?"}),
                2,
            ),
        );
        assert_eq!(stale_input_overlay.overlay, Overlay::None);
        assert!(matches!(
            stale_input_overlay.interaction,
            Interaction::InputPending(_)
        ));
    }

    #[test]
    fn runtime_selector_consumes_submit_while_help_yields_to_a_prompt() {
        let mut selector = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(&mut selector, Action::Submit("/runtime".into()));
        let effects = update(&mut selector, Action::Submit("do not send".into()));
        assert!(effects.is_empty());
        assert!(selector.transcript.entries().is_empty());
        assert_eq!(selector.overlay, Overlay::RuntimeList);
        assert!(
            selector
                .status_message
                .as_deref()
                .unwrap()
                .contains("runtime")
        );

        let mut help = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(&mut help, Action::Submit("/help".into()));
        let effects = update(&mut help, Action::Submit("send me".into()));
        assert!(matches!(
            effects.as_slice(),
            [Effect::StartTurn { prompt, .. }] if prompt == "send me"
        ));
        assert_eq!(help.overlay, Overlay::None);
    }

    #[test]
    fn turn_start_dismisses_a_stale_non_actionable_overlay() {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(&mut model, Action::Submit("hello".into()));
        model.overlay = Overlay::Help {
            commands: vec!["/help"],
        };
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-1"}),
                1,
            ),
        );
        assert_eq!(model.overlay, Overlay::None);
    }

    fn active_model() -> Model {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(&mut model, Action::Submit("hello".into()));
        dispatch_current_runtime(
            &mut model,
            event(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-1"}),
                1,
            ),
        );
        model
    }

    fn active_turn_token(model: &Model) -> OperationToken {
        match model.turn {
            TurnState::Streaming {
                operation_token, ..
            } => operation_token,
            ref other => panic!("expected streaming turn, got {other:?}"),
        }
    }

    fn start_turn_token(effects: &[Effect]) -> OperationToken {
        match effects {
            [
                Effect::StartTurn {
                    operation_token, ..
                },
            ] => *operation_token,
            other => panic!("expected one start effect, got {other:?}"),
        }
    }

    fn dispatch_runtime_with_token(
        model: &mut Model,
        operation_token: OperationToken,
        event: RuntimeEventEnvelope,
    ) {
        update(
            model,
            Action::Runtime {
                operation_token,
                event,
            },
        );
    }

    fn dispatch_current_runtime(model: &mut Model, event: RuntimeEventEnvelope) {
        let operation_token = match model.turn {
            TurnState::Starting { operation_token }
            | TurnState::Streaming {
                operation_token, ..
            } => operation_token,
            TurnState::Idle => panic!("runtime event requires an active turn operation"),
        };
        dispatch_runtime_with_token(model, operation_token, event);
    }

    fn request_approval(model: &mut Model) {
        dispatch_current_runtime(
            model,
            event(
                RuntimeEventKind::ApprovalRequested,
                json!({
                    "approval_id": "approval-1",
                    "tool": "shell",
                    "summary": "run command",
                    "options": ["allow", "deny"]
                }),
                2,
            ),
        );
    }
}
