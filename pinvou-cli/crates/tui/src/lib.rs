pub mod action;
pub mod backend;
pub mod commands;
pub mod model;
pub mod update;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pinvou_protocol::{RuntimeEventEnvelope, RuntimeEventKind};
    use serde_json::{Value, json};

    use crate::{
        action::{Action, ApprovalDecision, Effect},
        backend::{Backend, BackendError, RuntimeList, RuntimeStatus},
        commands::{CommandError, SlashCommand, parse},
        model::{Interaction, Model, Overlay, TranscriptEntry, TurnState},
        update::update,
    };

    fn runtime(id: &str) -> RuntimeStatus {
        RuntimeStatus::new(id, id, true)
    }

    fn event(kind: RuntimeEventKind, payload: Value, seq: u64) -> RuntimeEventEnvelope {
        let control = matches!(
            kind,
            RuntimeEventKind::TurnStarted
                | RuntimeEventKind::TurnEnded
                | RuntimeEventKind::ApprovalRequested
                | RuntimeEventKind::InputRequested
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
            "turn_id": "turn-1",
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
            Interaction::Approval { ref approval_id, .. } if approval_id == "approval-1"
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
        assert!(matches!(model.interaction, Interaction::Input { .. }));
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
}
