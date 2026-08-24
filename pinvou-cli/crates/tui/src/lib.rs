pub mod action;
pub mod backend;
pub mod commands;
pub mod model;
pub mod update;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pinvou_protocol::{RuntimeEventEnvelope, RuntimeEventKind, StableExitCode};
    use serde_json::{Value, json};

    use crate::{
        action::{Action, ApprovalDecision, Effect},
        backend::{Backend, BackendError, BackendErrorKind, RuntimeList, RuntimeStatus},
        commands::{CommandError, SlashCommand, parse},
        model::{Interaction, Model, Overlay, TranscriptEntry, TurnState},
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

        assert_eq!(
            update(&mut model, Action::Submit("hello".into())),
            vec![Effect::StartTurn {
                prompt: "hello".into()
            }]
        );
        update(
            &mut model,
            Action::Runtime(event(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-1"}),
                1,
            )),
        );
        update(
            &mut model,
            Action::Runtime(event(
                RuntimeEventKind::TextDelta,
                json!({"role": "assistant", "content": "hel"}),
                2,
            )),
        );
        update(
            &mut model,
            Action::Runtime(event(
                RuntimeEventKind::TextDelta,
                json!({"role": "assistant", "content": "lo"}),
                3,
            )),
        );
        update(
            &mut model,
            Action::Runtime(event(
                RuntimeEventKind::ApprovalRequested,
                json!({
                    "approval_id": "approval-1",
                    "tool": "shell",
                    "summary": "run cargo test",
                    "options": ["allow", "deny"]
                }),
                4,
            )),
        );
        assert!(matches!(
            model.interaction,
            Interaction::ApprovalPending(ref request) if request.approval_id == "approval-1"
        ));
        assert_eq!(
            update(
                &mut model,
                Action::ApprovalChosen(ApprovalDecision::AllowOnce)
            ),
            vec![Effect::ResolveApproval {
                approval_id: "approval-1".into(),
                accepted: true,
            }]
        );
        update(
            &mut model,
            Action::Runtime(event(
                RuntimeEventKind::TurnEnded,
                json!({"end_reason": "completed"}),
                5,
            )),
        );

        assert_eq!(model.transcript.assistant_text(), "hello");
        assert_eq!(model.turn, TurnState::Idle);
    }

    #[test]
    fn input_tool_error_and_runtime_actions_have_explicit_state_and_effects() {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(&mut model, Action::Submit("inspect".into()));
        update(
            &mut model,
            Action::Runtime(event(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-1"}),
                1,
            )),
        );
        update(
            &mut model,
            Action::Runtime(event(
                RuntimeEventKind::ToolCallStarted,
                json!({"tool_id": "tool-1", "name": "read_file"}),
                2,
            )),
        );
        update(
            &mut model,
            Action::Runtime(event(
                RuntimeEventKind::ToolCallOutputDelta,
                json!({"tool_id": "tool-1", "chunk": "done"}),
                3,
            )),
        );
        update(
            &mut model,
            Action::Runtime(event(
                RuntimeEventKind::InputRequested,
                json!({"input_id": "input-1", "prompt": "Continue?"}),
                4,
            )),
        );
        assert!(matches!(model.interaction, Interaction::InputPending(_)));
        assert_eq!(
            update(&mut model, Action::InputSubmitted("yes".into())),
            vec![Effect::ResolveInput {
                input_id: "input-1".into(),
                value: "yes".into(),
            }]
        );
        assert!(model.transcript.entries().iter().any(|entry| matches!(
            entry,
            TranscriptEntry::Tool { output, .. } if output == "done"
        )));

        assert_eq!(
            update(&mut model, Action::Interrupt),
            vec![Effect::Interrupt {
                turn_id: "turn-1".into()
            }]
        );
        assert_eq!(
            update(&mut model, Action::RuntimeSwitch("claude".into())),
            vec![Effect::SwitchRuntime {
                runtime: "claude".into()
            }]
        );

        update(
            &mut model,
            Action::Runtime(event(
                RuntimeEventKind::ErrorRaised,
                json!({
                    "code": "runtime_failed",
                    "message": "runtime stopped",
                    "fatal": true,
                    "source": "runtime"
                }),
                5,
            )),
        );
        assert_eq!(model.status_message.as_deref(), Some("runtime stopped"));
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
                _prompt: String,
                _emit: Box<dyn FnMut(RuntimeEventEnvelope) -> Result<(), BackendError> + Send>,
            ) -> Result<(), BackendError> {
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
                .stream_turn("hello".into(), Box::new(|_| Ok(())))
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

        update(
            &mut starting,
            Action::Runtime(event(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-1"}),
                1,
            )),
        );
        assert!(update(&mut starting, Action::Submit("third".into())).is_empty());
        assert_eq!(starting.transcript.entries().len(), 1);

        request_approval(&mut starting);
        let approval = starting.interaction.clone();
        assert!(update(&mut starting, Action::Submit("fourth".into())).is_empty());
        assert_eq!(starting.interaction, approval);
        assert_eq!(starting.transcript.entries().len(), 1);

        let mut input = active_model();
        update(
            &mut input,
            Action::Runtime(event(
                RuntimeEventKind::InputRequested,
                json!({"input_id": "input-1", "prompt": "Continue?"}),
                2,
            )),
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

        update(
            &mut model,
            Action::Runtime(event_for(
                RuntimeEventKind::TextDelta,
                json!({"role": "assistant", "content": "too early"}),
                1,
                Some("turn-1"),
            )),
        );
        assert_eq!(model.transcript.assistant_text(), "");
        assert!(model.status_message.as_deref().unwrap().contains("turn"));

        update(
            &mut model,
            Action::Runtime(event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-1"}),
                2,
                Some("turn-1"),
            )),
        );
        assert_eq!(
            model.turn,
            TurnState::Streaming {
                turn_id: "turn-1".into()
            }
        );

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
            update(&mut model, Action::Runtime(foreign));
        }
        assert_eq!(model.transcript.assistant_text(), "");
        assert_eq!(model.interaction, Interaction::None);
        assert_eq!(model.transcript.entries().len(), 1);
        assert_eq!(
            model.turn,
            TurnState::Streaming {
                turn_id: "turn-1".into()
            }
        );

        update(
            &mut model,
            Action::Runtime(event_for(
                RuntimeEventKind::TextDelta,
                json!({"role": "assistant", "content": "current"}),
                9,
                Some("turn-1"),
            )),
        );
        assert_eq!(model.transcript.assistant_text(), "current");
    }

    #[test]
    fn turn_started_is_bound_once_and_late_terminal_events_are_ignored() {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(
            &mut model,
            Action::Runtime(event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "orphan"}),
                1,
                Some("orphan-turn"),
            )),
        );
        assert_eq!(model.turn, TurnState::Idle);
        assert!(model.status_message.as_deref().unwrap().contains("turn"));

        update(&mut model, Action::Submit("hello".into()));
        update(
            &mut model,
            Action::Runtime(event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "missing-id"}),
                2,
                None,
            )),
        );
        assert_eq!(model.turn, TurnState::Starting);
        update(
            &mut model,
            Action::Runtime(event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-1"}),
                3,
                Some("turn-1"),
            )),
        );
        update(
            &mut model,
            Action::Runtime(event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "duplicate"}),
                4,
                Some("turn-2"),
            )),
        );
        assert_eq!(
            model.turn,
            TurnState::Streaming {
                turn_id: "turn-1".into()
            }
        );
        update(
            &mut model,
            Action::Runtime(event_for(
                RuntimeEventKind::TurnEnded,
                json!({"end_reason": "completed"}),
                5,
                Some("turn-1"),
            )),
        );
        assert_eq!(model.turn, TurnState::Idle);

        update(&mut model, Action::Submit("next".into()));
        update(
            &mut model,
            Action::Runtime(event_for(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-2"}),
                6,
                Some("turn-2"),
            )),
        );
        update(
            &mut model,
            Action::Runtime(event_for(
                RuntimeEventKind::TurnEnded,
                json!({"end_reason": "completed"}),
                7,
                Some("turn-1"),
            )),
        );
        assert_eq!(
            model.turn,
            TurnState::Streaming {
                turn_id: "turn-2".into()
            }
        );
    }

    #[test]
    fn approval_resolution_is_two_phase_retryable_and_race_safe() {
        let mut model = active_model();
        request_approval(&mut model);

        let effect = update(
            &mut model,
            Action::ApprovalChosen(ApprovalDecision::AllowOnce),
        );
        assert_eq!(effect.len(), 1);
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
                approval_id: "approval-1".into(),
                result: Err(error.clone()),
            },
        );
        assert!(matches!(model.interaction, Interaction::ApprovalPending(_)));
        assert_eq!(model.last_backend_error.as_ref(), Some(&error));

        update(
            &mut model,
            Action::ApprovalChosen(ApprovalDecision::AllowOnce),
        );
        update(
            &mut model,
            Action::Runtime(event(
                RuntimeEventKind::ApprovalResolved,
                json!({"approval_id": "approval-1", "outcome": "approved"}),
                3,
            )),
        );
        assert_eq!(model.interaction, Interaction::None);
        update(
            &mut model,
            Action::ApprovalResolutionCompleted {
                approval_id: "approval-1".into(),
                result: Err(error),
            },
        );
        assert_eq!(model.interaction, Interaction::None);
    }

    #[test]
    fn input_resolution_preserves_request_on_failure_and_clears_on_success() {
        let mut model = active_model();
        update(
            &mut model,
            Action::Runtime(event(
                RuntimeEventKind::InputRequested,
                json!({"input_id": "input-1", "prompt": "Continue?"}),
                2,
            )),
        );
        update(&mut model, Action::InputSubmitted("yes".into()));
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
                input_id: "input-1".into(),
                result: Err(error),
            },
        );
        assert!(matches!(model.interaction, Interaction::InputPending(_)));

        update(&mut model, Action::InputSubmitted("yes".into()));
        update(
            &mut model,
            Action::InputResolutionCompleted {
                input_id: "input-1".into(),
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

        update(&mut model, Action::RuntimeSwitched(Err(error.clone())));

        assert_eq!(model.status_message.as_deref(), Some("sign in required"));
        assert_eq!(model.last_backend_error.as_ref(), Some(&error));
        assert_eq!(error.kind(), BackendErrorKind::AuthBlocked);
        assert_eq!(error.exit_code(), Some(StableExitCode::BlockedAuth));
    }

    fn active_model() -> Model {
        let mut model = Model::new(PathBuf::from("workspace"), runtime("codex"));
        update(&mut model, Action::Submit("hello".into()));
        update(
            &mut model,
            Action::Runtime(event(
                RuntimeEventKind::TurnStarted,
                json!({"user_input_ref": "prompt-1"}),
                1,
            )),
        );
        model
    }

    fn request_approval(model: &mut Model) {
        update(
            model,
            Action::Runtime(event(
                RuntimeEventKind::ApprovalRequested,
                json!({
                    "approval_id": "approval-1",
                    "tool": "shell",
                    "summary": "run command",
                    "options": ["allow", "deny"]
                }),
                2,
            )),
        );
    }
}
