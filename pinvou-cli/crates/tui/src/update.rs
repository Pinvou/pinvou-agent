use pinvou_protocol::{RuntimeEventEnvelope, RuntimeEventKind};
use serde_json::Value;

use crate::{
    action::{Action, ApprovalDecision, Effect},
    commands::{AVAILABLE_COMMANDS, SlashCommand, parse},
    model::{Interaction, Model, Overlay, TurnState},
};

pub fn update(model: &mut Model, action: Action) -> Vec<Effect> {
    match action {
        Action::Submit(input) => submit(model, input),
        Action::Runtime(event) => {
            project_runtime_event(model, &event);
            Vec::new()
        }
        Action::ApprovalChosen(decision) => choose_approval(model, decision),
        Action::InputSubmitted(value) => submit_input(model, value),
        Action::Interrupt => interrupt(model),
        Action::RuntimeSwitch(runtime) => switch_runtime(model, runtime),
        Action::RuntimeSwitched(result) => {
            match result {
                Ok(runtime) => {
                    model.runtime = runtime;
                    model.status_message = None;
                }
                Err(error) => model.status_message = Some(error),
            }
            Vec::new()
        }
    }
}

fn submit(model: &mut Model, input: String) -> Vec<Effect> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    match parse(trimmed) {
        Ok(Some(SlashCommand::Help)) => {
            model.overlay = Overlay::Help {
                commands: AVAILABLE_COMMANDS.to_vec(),
            };
            Vec::new()
        }
        Ok(Some(SlashCommand::Runtime)) => {
            model.overlay = Overlay::RuntimeList;
            Vec::new()
        }
        Ok(Some(SlashCommand::Exit)) => {
            model.should_quit = true;
            Vec::new()
        }
        Ok(None) => {
            let prompt = trimmed.to_owned();
            model.transcript.push_user(prompt.clone());
            model.composer.input.clear();
            model.overlay = Overlay::None;
            model.interaction = Interaction::None;
            model.status_message = None;
            model.turn = TurnState::Starting;
            vec![Effect::StartTurn { prompt }]
        }
        Err(error) => {
            model.status_message = Some(error.to_string());
            Vec::new()
        }
    }
}

fn choose_approval(model: &mut Model, decision: ApprovalDecision) -> Vec<Effect> {
    let Interaction::Approval { approval_id, .. } = &model.interaction else {
        return Vec::new();
    };
    let effect = Effect::ResolveApproval {
        approval_id: approval_id.clone(),
        accepted: decision == ApprovalDecision::AllowOnce,
    };
    model.interaction = Interaction::None;
    vec![effect]
}

fn submit_input(model: &mut Model, value: String) -> Vec<Effect> {
    let Interaction::Input { input_id, .. } = &model.interaction else {
        return Vec::new();
    };
    let effect = Effect::ResolveInput {
        input_id: input_id.clone(),
        value,
    };
    model.interaction = Interaction::None;
    vec![effect]
}

fn interrupt(model: &mut Model) -> Vec<Effect> {
    let TurnState::Streaming {
        turn_id: Some(turn_id),
    } = &model.turn
    else {
        return Vec::new();
    };
    vec![Effect::Interrupt {
        turn_id: turn_id.clone(),
    }]
}

fn switch_runtime(model: &mut Model, runtime: String) -> Vec<Effect> {
    model.overlay = Overlay::None;
    vec![Effect::SwitchRuntime { runtime }]
}

fn project_runtime_event(model: &mut Model, event: &RuntimeEventEnvelope) {
    let payload = match serde_json::from_str::<Value>(event.payload().get()) {
        Ok(payload) => payload,
        Err(error) => {
            model.status_message = Some(format!("invalid runtime event payload: {error}"));
            return;
        }
    };

    match event.event_kind() {
        RuntimeEventKind::TurnStarted => {
            model.turn = TurnState::Streaming {
                turn_id: event.turn_id().map(str::to_owned),
            };
        }
        RuntimeEventKind::TextDelta => {
            if payload.get("role").and_then(Value::as_str) == Some("assistant") {
                if let Some(content) = payload.get("content").and_then(Value::as_str) {
                    model.transcript.append_assistant(content);
                }
            }
        }
        RuntimeEventKind::ApprovalRequested => {
            if let (Some(approval_id), Some(tool), Some(summary), Some(options)) = (
                string(&payload, "approval_id"),
                string(&payload, "tool"),
                string(&payload, "summary"),
                strings(&payload, "options"),
            ) {
                model.interaction = Interaction::Approval {
                    approval_id,
                    tool,
                    summary,
                    options,
                };
            }
        }
        RuntimeEventKind::InputRequested => {
            if let (Some(input_id), Some(prompt)) =
                (string(&payload, "input_id"), string(&payload, "prompt"))
            {
                model.interaction = Interaction::Input { input_id, prompt };
            }
        }
        RuntimeEventKind::ToolCallStarted => {
            if let (Some(tool_id), Some(name)) =
                (string(&payload, "tool_id"), string(&payload, "name"))
            {
                model.transcript.start_tool(tool_id, name);
            }
        }
        RuntimeEventKind::ToolCallOutputDelta => {
            if let (Some(tool_id), Some(chunk)) =
                (string(&payload, "tool_id"), string(&payload, "chunk"))
            {
                model.transcript.append_tool_output(&tool_id, &chunk);
            }
        }
        RuntimeEventKind::ToolCallCompleted => {
            if let Some(tool_id) = string(&payload, "tool_id") {
                let failed = payload
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                model.transcript.complete_tool(&tool_id, failed);
            }
        }
        RuntimeEventKind::TurnEnded => {
            model.turn = TurnState::Idle;
            model.interaction = Interaction::None;
        }
        RuntimeEventKind::ErrorRaised => {
            if let Some(message) = string(&payload, "message") {
                model.status_message = Some(message);
            }
            if payload.get("fatal").and_then(Value::as_bool) == Some(true) {
                model.turn = TurnState::Idle;
                model.interaction = Interaction::None;
            }
        }
        RuntimeEventKind::StreamAborted => {
            model.turn = TurnState::Idle;
            model.interaction = Interaction::None;
            model.status_message = string(&payload, "reason");
        }
        _ => {}
    }
}

fn string(payload: &Value, field: &str) -> Option<String> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn strings(payload: &Value, field: &str) -> Option<Vec<String>> {
    payload.get(field).and_then(Value::as_array).map(|values| {
        values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    })
}
