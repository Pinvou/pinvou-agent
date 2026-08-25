use pinvou_protocol::{RuntimeEventEnvelope, RuntimeEventKind};
use serde_json::Value;

use crate::{
    action::{Action, ApprovalDecision, Effect},
    backend::BackendError,
    commands::{AVAILABLE_COMMANDS, SlashCommand, parse},
    model::{
        ApprovalRequest, InputRequest, Interaction, Model, Overlay, PendingInterrupt,
        PendingPermissionSwitch, PendingRuntimeSwitch, PendingSelection, Transcript, TurnState,
    },
};

pub fn update(model: &mut Model, action: Action) -> Vec<Effect> {
    match action {
        Action::Submit(input) => submit(model, input),
        Action::Runtime {
            operation_token,
            event,
        } => {
            project_runtime_event(model, operation_token, &event);
            Vec::new()
        }
        Action::ApprovalChosen(decision) => choose_approval(model, decision),
        Action::ApprovalResolutionCompleted {
            turn_id,
            approval_id,
            operation_token,
            result,
        } => complete_approval(model, &turn_id, &approval_id, operation_token, result),
        Action::InputSubmitted(value) => submit_input(model, value),
        Action::InputResolutionCompleted {
            turn_id,
            input_id,
            operation_token,
            result,
        } => complete_input(model, &turn_id, &input_id, operation_token, result),
        Action::TurnStreamCompleted {
            operation_token,
            result,
        } => complete_turn_stream(model, operation_token, result),
        Action::Interrupt => interrupt(model),
        Action::InterruptCompleted {
            turn_id,
            operation_token,
            result,
        } => complete_interrupt(model, &turn_id, operation_token, result),
        Action::LoadRuntimeList => load_runtime_list(model),
        Action::RuntimeListLoaded {
            operation_token,
            result,
        } => complete_runtime_list(model, operation_token, result),
        Action::RuntimeSwitch(runtime) => switch_runtime(model, runtime),
        Action::RuntimeSwitched {
            operation_token,
            result,
        } => complete_runtime_switch(model, operation_token, result),
        Action::LoadSessionList { query } => load_session_list(model, query),
        Action::SessionListLoaded {
            operation_token,
            result,
        } => complete_session_list(model, operation_token, result),
        Action::ResumeSession(session_id) => resume_session(model, session_id),
        Action::SessionResumed {
            operation_token,
            result,
        } => complete_resume(model, operation_token, result),
        Action::LoadModelList => load_model_list(model),
        Action::ModelListLoaded {
            operation_token,
            result,
        } => complete_model_list(model, operation_token, result),
        Action::ModelSwitch(model_id) => switch_model(model, model_id),
        Action::ModelSwitched {
            operation_token,
            result,
        } => complete_model_switch(model, operation_token, result),
        Action::LoadPermissions => load_permissions(model),
        Action::PermissionsLoaded {
            operation_token,
            result,
        } => complete_permissions(model, operation_token, result),
        Action::PermissionSwitch {
            profile,
            full_access_confirmed,
        } => switch_permissions(model, profile, full_access_confirmed),
        Action::PermissionSwitched {
            operation_token,
            result,
        } => complete_permission_switch(model, operation_token, result),
    }
}

fn submit(model: &mut Model, input: String) -> Vec<Effect> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    match parse(trimmed) {
        Ok(Some(SlashCommand::Help)) => {
            open_overlay(
                model,
                Overlay::Help {
                    commands: AVAILABLE_COMMANDS.to_vec(),
                },
            );
            Vec::new()
        }
        Ok(Some(SlashCommand::Runtime)) => load_runtime_list(model),
        Ok(Some(SlashCommand::Resume)) => load_session_list(model, String::new()),
        Ok(Some(SlashCommand::Model)) => load_model_list(model),
        Ok(Some(SlashCommand::Permissions)) => load_permissions(model),
        Ok(Some(SlashCommand::Exit)) => {
            model.should_quit = true;
            Vec::new()
        }
        Ok(None)
            if matches!(
                model.overlay,
                Overlay::RuntimeList
                    | Overlay::ResumeList
                    | Overlay::ModelList
                    | Overlay::PermissionList
                    | Overlay::FullAccessConfirmation
            ) =>
        {
            model.status_message =
                Some("choose a runtime or close the runtime selector before submitting".into());
            Vec::new()
        }
        Ok(None) => submit_prompt(model, trimmed),
        Err(error) => {
            model.status_message = Some(error.to_string());
            Vec::new()
        }
    }
}

fn open_overlay(model: &mut Model, overlay: Overlay) {
    if model.turn != TurnState::Idle
        || model.interaction != Interaction::None
        || has_pending_selection(model)
    {
        model.status_message =
            Some("overlay cannot open during an active turn or interaction".into());
        return;
    }
    model.overlay = overlay;
    model.status_message = None;
    model.diagnostic_message = None;
}

fn submit_prompt(model: &mut Model, prompt: &str) -> Vec<Effect> {
    if matches!(model.connection, crate::model::ConnectionState::Failed(_)) {
        model.status_message = Some("backend must be reinitialized before continuing".into());
        return Vec::new();
    }
    if model.turn != TurnState::Idle
        || model.interaction != Interaction::None
        || has_pending_selection(model)
    {
        model.status_message = Some("an active turn or interaction must finish first".into());
        return Vec::new();
    }

    let prompt = prompt.to_owned();
    model.transcript.push_user(prompt.clone());
    model.composer.input.clear();
    model.overlay = Overlay::None;
    model.status_message = None;
    model.diagnostic_message = None;
    model.last_backend_error = None;
    model.pending_runtime_switch = None;
    let operation_token = model.allocate_operation_token();
    model.turn = TurnState::Starting { operation_token };
    vec![Effect::StartTurn {
        prompt,
        operation_token,
    }]
}

fn choose_approval(model: &mut Model, decision: ApprovalDecision) -> Vec<Effect> {
    let Interaction::ApprovalPending(request) = &model.interaction else {
        if matches!(model.interaction, Interaction::ApprovalResolving { .. }) {
            model.status_message = Some("approval resolution is already in progress".into());
        }
        return Vec::new();
    };

    let request = request.clone();
    let effect = Effect::ResolveApproval {
        turn_id: request.turn_id.clone(),
        approval_id: request.approval_id.clone(),
        operation_token: request.operation_token,
        accepted: decision == ApprovalDecision::AllowOnce,
    };
    model.interaction = Interaction::ApprovalResolving { request, decision };
    model.status_message = None;
    model.last_backend_error = None;
    vec![effect]
}

fn complete_approval(
    model: &mut Model,
    turn_id: &str,
    approval_id: &str,
    operation_token: crate::model::OperationToken,
    result: Result<(), BackendError>,
) -> Vec<Effect> {
    let Interaction::ApprovalResolving { request, .. } = &model.interaction else {
        return Vec::new();
    };
    if request.turn_id != turn_id
        || request.approval_id != approval_id
        || request.operation_token != operation_token
    {
        record_ignored(model, "ignored stale approval completion".into());
        return Vec::new();
    }

    match result {
        Ok(()) => {
            model.interaction = Interaction::None;
            model.status_message = None;
            model.last_backend_error = None;
        }
        Err(error) => {
            let mut request = request.clone();
            request.operation_token = model.allocate_operation_token();
            model.interaction = Interaction::ApprovalPending(request);
            record_backend_error(model, error);
        }
    }
    Vec::new()
}

fn submit_input(model: &mut Model, value: String) -> Vec<Effect> {
    let Interaction::InputPending(request) = &model.interaction else {
        if matches!(model.interaction, Interaction::InputResolving { .. }) {
            model.status_message = Some("input resolution is already in progress".into());
        }
        return Vec::new();
    };

    let request = request.clone();
    let effect = Effect::ResolveInput {
        turn_id: request.turn_id.clone(),
        input_id: request.input_id.clone(),
        operation_token: request.operation_token,
        value: value.clone(),
    };
    model.interaction = Interaction::InputResolving { request, value };
    model.status_message = None;
    model.last_backend_error = None;
    vec![effect]
}

fn complete_input(
    model: &mut Model,
    turn_id: &str,
    input_id: &str,
    operation_token: crate::model::OperationToken,
    result: Result<(), BackendError>,
) -> Vec<Effect> {
    let Interaction::InputResolving { request, .. } = &model.interaction else {
        return Vec::new();
    };
    if request.turn_id != turn_id
        || request.input_id != input_id
        || request.operation_token != operation_token
    {
        record_ignored(model, "ignored stale input completion".into());
        return Vec::new();
    }

    match result {
        Ok(()) => {
            model.interaction = Interaction::None;
            model.status_message = None;
            model.last_backend_error = None;
        }
        Err(error) => {
            let mut request = request.clone();
            request.operation_token = model.allocate_operation_token();
            model.interaction = Interaction::InputPending(request);
            record_backend_error(model, error);
        }
    }
    Vec::new()
}

fn complete_turn_stream(
    model: &mut Model,
    operation_token: crate::model::OperationToken,
    result: Result<(), BackendError>,
) -> Vec<Effect> {
    let active_token = match &model.turn {
        TurnState::Starting { operation_token }
        | TurnState::Streaming {
            operation_token, ..
        } => Some(*operation_token),
        TurnState::Idle => None,
    };

    if let Some(active_token) = active_token {
        if active_token != operation_token {
            record_ignored(model, "ignored stale turn stream completion".into());
            return Vec::new();
        }

        recover_turn_without_terminal(model);
        match result {
            Ok(()) => {
                let error = BackendError::new(
                    crate::backend::BackendErrorKind::Protocol,
                    "turn stream completed without turn.ended",
                )
                .with_exit_code(pinvou_protocol::StableExitCode::RuntimeFailed);
                record_backend_error(model, error);
            }
            Err(error) => record_backend_error(model, error),
        }
        return Vec::new();
    }

    if model.last_terminal_turn_token == Some(operation_token) {
        if result.is_err() {
            record_ignored(
                model,
                "ignored stream failure after runtime terminal".into(),
            );
        }
    } else {
        record_ignored(model, "ignored stale turn stream completion".into());
    }
    Vec::new()
}

fn interrupt(model: &mut Model) -> Vec<Effect> {
    let TurnState::Streaming { turn_id, .. } = &model.turn else {
        return Vec::new();
    };
    if model.pending_interrupt.is_some() {
        return Vec::new();
    }
    let turn_id = turn_id.clone();
    let operation_token = model.allocate_operation_token();
    model.pending_interrupt = Some(PendingInterrupt {
        turn_id: turn_id.clone(),
        operation_token,
    });
    vec![Effect::Interrupt {
        turn_id,
        operation_token,
    }]
}

fn complete_interrupt(
    model: &mut Model,
    turn_id: &str,
    operation_token: crate::model::OperationToken,
    result: Result<(), BackendError>,
) -> Vec<Effect> {
    let current = matches!(
        &model.pending_interrupt,
        Some(pending) if pending.turn_id == turn_id && pending.operation_token == operation_token
    );
    if !current {
        record_ignored(model, "ignored stale interrupt completion".into());
    } else {
        model.pending_interrupt = None;
        if let Err(error) = result {
            record_backend_error(model, error);
        }
    }
    Vec::new()
}

fn load_runtime_list(model: &mut Model) -> Vec<Effect> {
    if matches!(model.connection, crate::model::ConnectionState::Failed(_)) {
        model.status_message = Some("backend must be reinitialized before continuing".into());
        return Vec::new();
    }
    if model.turn != TurnState::Idle
        || model.interaction != Interaction::None
        || model.pending_runtime_switch.is_some()
    {
        model.status_message =
            Some("runtime selector cannot open during an active turn or interaction".into());
        return Vec::new();
    }
    if model.pending_runtime_list.is_some() {
        return Vec::new();
    }
    let operation_token = model.allocate_operation_token();
    model.overlay = Overlay::RuntimeList;
    model.pending_runtime_list = Some(operation_token);
    model.status_message = Some("loading runtimes...".into());
    vec![Effect::LoadRuntimeList { operation_token }]
}

fn complete_runtime_list(
    model: &mut Model,
    operation_token: crate::model::OperationToken,
    result: Result<crate::backend::RuntimeList, BackendError>,
) -> Vec<Effect> {
    if model.pending_runtime_list != Some(operation_token) {
        record_ignored(model, "ignored stale runtime list completion".into());
        return Vec::new();
    }
    model.pending_runtime_list = None;
    match result {
        Ok(list) => {
            model.runtime_candidates = list.runtimes;
            model.selected_runtime = model
                .runtime_candidates
                .iter()
                .position(|runtime| runtime.id == model.runtime.id)
                .unwrap_or(0);
            model.status_message = None;
            model.last_backend_error = None;
        }
        Err(error) => record_backend_error(model, error),
    }
    Vec::new()
}

fn switch_runtime(model: &mut Model, runtime: String) -> Vec<Effect> {
    if matches!(model.connection, crate::model::ConnectionState::Failed(_)) {
        model.status_message = Some("backend must be reinitialized before continuing".into());
        return Vec::new();
    }
    if model.turn != TurnState::Idle || model.interaction != Interaction::None {
        model.status_message = Some("runtime cannot switch during an active turn".into());
        return Vec::new();
    }
    if model.pending_runtime_switch.is_some() {
        model.status_message = Some("runtime switch is already in progress".into());
        return Vec::new();
    }

    let operation_token = model.allocate_operation_token();
    model.pending_runtime_switch = Some(PendingRuntimeSwitch {
        target: runtime.clone(),
        operation_token,
    });
    model.status_message = None;
    model.diagnostic_message = None;
    model.last_backend_error = None;
    vec![Effect::SwitchRuntime {
        runtime,
        operation_token,
    }]
}

fn complete_runtime_switch(
    model: &mut Model,
    operation_token: crate::model::OperationToken,
    result: Result<crate::backend::RuntimeStatus, BackendError>,
) -> Vec<Effect> {
    let Some(pending) = &model.pending_runtime_switch else {
        record_ignored(
            model,
            "ignored runtime switch completion without pending operation".into(),
        );
        return Vec::new();
    };
    if pending.operation_token != operation_token {
        record_ignored(model, "ignored stale runtime switch completion".into());
        return Vec::new();
    }

    let target = pending.target.clone();
    model.pending_runtime_switch = None;
    match result {
        Ok(runtime) if runtime.id == target => {
            model.runtime = runtime;
            model.overlay = Overlay::None;
            model.status_message = None;
            model.diagnostic_message = None;
            model.last_backend_error = None;
        }
        Ok(runtime) => {
            model.overlay = Overlay::RuntimeList;
            let diagnostic = format!(
                "runtime switch expected '{}' but backend returned '{}'",
                target, runtime.id
            );
            let error = BackendError::new(
                crate::backend::BackendErrorKind::Protocol,
                diagnostic.clone(),
            )
            .with_exit_code(pinvou_protocol::StableExitCode::RuntimeFailed);
            record_backend_error(model, error);
            model.diagnostic_message = Some(diagnostic);
        }
        Err(error) => record_backend_error(model, error),
    }
    Vec::new()
}

fn has_pending_selection(model: &Model) -> bool {
    model.pending_runtime_switch.is_some()
        || model.pending_resume.is_some()
        || model.pending_model_switch.is_some()
        || model.pending_permission_switch.is_some()
}

fn selector_available(model: &mut Model, name: &str) -> bool {
    if matches!(model.connection, crate::model::ConnectionState::Failed(_)) {
        model.status_message = Some("backend must be reinitialized before continuing".into());
        return false;
    }
    if model.turn != TurnState::Idle
        || model.interaction != Interaction::None
        || has_pending_selection(model)
    {
        model.status_message = Some(format!(
            "{name} cannot open during an active turn or interaction"
        ));
        return false;
    }
    true
}

fn load_session_list(model: &mut Model, query: String) -> Vec<Effect> {
    if !selector_available(model, "session selector") || model.pending_session_list.is_some() {
        return Vec::new();
    }
    let operation_token = model.allocate_operation_token();
    model.overlay = Overlay::ResumeList;
    model.session_query = query.clone();
    model.pending_session_list = Some(operation_token);
    model.status_message = Some("loading sessions...".into());
    vec![Effect::LoadSessionList {
        operation_token,
        query,
    }]
}

fn complete_session_list(
    model: &mut Model,
    operation_token: crate::model::OperationToken,
    result: Result<crate::backend::SessionList, BackendError>,
) -> Vec<Effect> {
    if model.pending_session_list != Some(operation_token) {
        record_ignored(model, "ignored stale session list completion".into());
        return Vec::new();
    }
    model.pending_session_list = None;
    match result {
        Ok(list) => {
            model.session_candidates = list.sessions;
            model.selected_session = 0;
            model.status_message = None;
            model.last_backend_error = None;
        }
        Err(error) => record_backend_error(model, error),
    }
    Vec::new()
}

fn resume_session(model: &mut Model, session_id: String) -> Vec<Effect> {
    if !selector_available(model, "session resume") {
        return Vec::new();
    }
    let operation_token = model.allocate_operation_token();
    model.pending_resume = Some(PendingSelection {
        target: session_id.clone(),
        operation_token,
    });
    model.status_message = Some("resuming session...".into());
    vec![Effect::ResumeSession {
        operation_token,
        session_id,
    }]
}

fn complete_resume(
    model: &mut Model,
    operation_token: crate::model::OperationToken,
    result: Result<crate::backend::ResumeResult, BackendError>,
) -> Vec<Effect> {
    let Some(pending) = &model.pending_resume else {
        record_ignored(
            model,
            "ignored resume completion without pending operation".into(),
        );
        return Vec::new();
    };
    if pending.operation_token != operation_token {
        record_ignored(model, "ignored stale resume completion".into());
        return Vec::new();
    }
    let target = pending.target.clone();
    model.pending_resume = None;
    match result {
        Ok(resumed) if resumed.session_id == target => {
            model.transcript = transcript_from_events(&resumed.events);
            model.active_session = Some(resumed.session_id);
            model.session_cursor = resumed.cursor;
            model.runtime = crate::backend::RuntimeStatus::new(
                resumed.runtime_id.clone(),
                resumed.runtime_id,
                true,
            );
            model.model_id = resumed.model_id;
            model.permission_profile = resumed.permission_profile;
            model.overlay = Overlay::None;
            model.status_message = None;
            model.last_backend_error = None;
        }
        Ok(_) => record_backend_error(
            model,
            BackendError::new(
                crate::backend::BackendErrorKind::Protocol,
                "session resume response does not match selection",
            ),
        ),
        Err(error) => record_backend_error(model, error),
    }
    Vec::new()
}

fn load_model_list(model: &mut Model) -> Vec<Effect> {
    if !selector_available(model, "model selector") || model.pending_model_list.is_some() {
        return Vec::new();
    }
    let operation_token = model.allocate_operation_token();
    model.overlay = Overlay::ModelList;
    model.pending_model_list = Some(operation_token);
    model.status_message = Some("loading models...".into());
    vec![Effect::LoadModelList { operation_token }]
}

fn complete_model_list(
    model: &mut Model,
    operation_token: crate::model::OperationToken,
    result: Result<crate::backend::ModelList, BackendError>,
) -> Vec<Effect> {
    if model.pending_model_list != Some(operation_token) {
        record_ignored(model, "ignored stale model list completion".into());
        return Vec::new();
    }
    model.pending_model_list = None;
    match result {
        Ok(list) => {
            model.model_id = list.current_model;
            model.model_candidates = list.models;
            model.selected_model = model
                .model_candidates
                .iter()
                .position(|candidate| Some(candidate.id.as_str()) == model.model_id.as_deref())
                .unwrap_or(0);
            model.status_message = None;
            model.last_backend_error = None;
        }
        Err(error) => record_backend_error(model, error),
    }
    Vec::new()
}

fn switch_model(model: &mut Model, model_id: String) -> Vec<Effect> {
    if !selector_available(model, "model switch") {
        return Vec::new();
    }
    let operation_token = model.allocate_operation_token();
    model.pending_model_switch = Some(PendingSelection {
        target: model_id.clone(),
        operation_token,
    });
    model.status_message = Some("switching model...".into());
    vec![Effect::SwitchModel {
        operation_token,
        model_id,
    }]
}

fn complete_model_switch(
    model: &mut Model,
    operation_token: crate::model::OperationToken,
    result: Result<(), BackendError>,
) -> Vec<Effect> {
    let Some(pending) = &model.pending_model_switch else {
        record_ignored(model, "ignored model switch completion".into());
        return Vec::new();
    };
    if pending.operation_token != operation_token {
        record_ignored(model, "ignored stale model switch completion".into());
        return Vec::new();
    }
    let target = pending.target.clone();
    model.pending_model_switch = None;
    match result {
        Ok(()) => {
            model.model_id = Some(target);
            model.overlay = Overlay::None;
            model.status_message = None;
            model.last_backend_error = None;
        }
        Err(error) => record_backend_error(model, error),
    }
    Vec::new()
}

fn load_permissions(model: &mut Model) -> Vec<Effect> {
    if !selector_available(model, "permission selector") || model.pending_permissions.is_some() {
        return Vec::new();
    }
    let operation_token = model.allocate_operation_token();
    model.overlay = Overlay::PermissionList;
    model.pending_permissions = Some(operation_token);
    model.status_message = Some("loading permissions...".into());
    vec![Effect::LoadPermissions { operation_token }]
}

fn complete_permissions(
    model: &mut Model,
    operation_token: crate::model::OperationToken,
    result: Result<crate::backend::PermissionStatus, BackendError>,
) -> Vec<Effect> {
    if model.pending_permissions != Some(operation_token) {
        record_ignored(model, "ignored stale permission completion".into());
        return Vec::new();
    }
    model.pending_permissions = None;
    match result {
        Ok(status) => {
            model.permission_profile = status.current_profile;
            model.selected_permission = status
                .supported_profiles
                .iter()
                .position(|profile| *profile == status.current_profile)
                .unwrap_or(0);
            model.permission_status = Some(status);
            model.status_message = None;
            model.last_backend_error = None;
        }
        Err(error) => record_backend_error(model, error),
    }
    Vec::new()
}

fn switch_permissions(
    model: &mut Model,
    profile: crate::backend::PermissionMode,
    full_access_confirmed: bool,
) -> Vec<Effect> {
    if profile == crate::backend::PermissionMode::FullAccess && !full_access_confirmed {
        model.overlay = Overlay::FullAccessConfirmation;
        model.status_message = Some("full access requires explicit confirmation".into());
        return Vec::new();
    }
    if !selector_available(model, "permission switch") {
        return Vec::new();
    }
    let operation_token = model.allocate_operation_token();
    model.pending_permission_switch = Some(PendingPermissionSwitch {
        target: profile,
        operation_token,
    });
    model.status_message = Some("switching permissions...".into());
    vec![Effect::SwitchPermissions {
        operation_token,
        profile,
        full_access_confirmed,
    }]
}

fn complete_permission_switch(
    model: &mut Model,
    operation_token: crate::model::OperationToken,
    result: Result<(), BackendError>,
) -> Vec<Effect> {
    let Some(pending) = &model.pending_permission_switch else {
        record_ignored(model, "ignored permission switch completion".into());
        return Vec::new();
    };
    if pending.operation_token != operation_token {
        record_ignored(model, "ignored stale permission switch completion".into());
        return Vec::new();
    }
    let target = pending.target;
    model.pending_permission_switch = None;
    match result {
        Ok(()) => {
            model.permission_profile = target;
            model.overlay = Overlay::None;
            model.status_message = None;
            model.last_backend_error = None;
        }
        Err(error) => record_backend_error(model, error),
    }
    Vec::new()
}

fn transcript_from_events(events: &[RuntimeEventEnvelope]) -> Transcript {
    let mut transcript = Transcript::default();
    for event in events {
        let Ok(payload) = serde_json::from_str::<Value>(event.payload().get()) else {
            continue;
        };
        match event.event_kind() {
            RuntimeEventKind::TextDelta
                if payload.get("role").and_then(Value::as_str) == Some("assistant") =>
            {
                if let Some(content) = payload.get("content").and_then(Value::as_str) {
                    transcript.append_assistant(content);
                }
            }
            RuntimeEventKind::ToolCallStarted => {
                if let (Some(tool_id), Some(name)) = (
                    payload.get("tool_id").and_then(Value::as_str),
                    payload.get("name").and_then(Value::as_str),
                ) {
                    transcript.start_tool(tool_id.to_owned(), name.to_owned());
                }
            }
            RuntimeEventKind::ToolCallOutputDelta => {
                if let (Some(tool_id), Some(chunk)) = (
                    payload.get("tool_id").and_then(Value::as_str),
                    payload.get("chunk").and_then(Value::as_str),
                ) {
                    transcript.append_tool_output(tool_id, chunk);
                }
            }
            RuntimeEventKind::ToolCallCompleted => {
                if let Some(tool_id) = payload.get("tool_id").and_then(Value::as_str) {
                    transcript.complete_tool(
                        tool_id,
                        payload
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    );
                }
            }
            _ => {}
        }
    }
    transcript
}

fn project_runtime_event(
    model: &mut Model,
    operation_token: crate::model::OperationToken,
    event: &RuntimeEventEnvelope,
) {
    if !runtime_token_is_active(model, operation_token) {
        record_ignored(
            model,
            "ignored runtime event with a stale operation token".into(),
        );
        return;
    }

    let payload = match serde_json::from_str::<Value>(event.payload().get()) {
        Ok(payload) => payload,
        Err(error) => {
            model.status_message = Some(format!("invalid runtime event payload: {error}"));
            return;
        }
    };

    if event.event_kind() == RuntimeEventKind::TurnStarted {
        bind_turn(model, event);
        return;
    }
    if is_projected_turn_event(event.event_kind()) && !belongs_to_active_turn(model, event) {
        return;
    }

    match event.event_kind() {
        RuntimeEventKind::TextDelta => {
            if payload.get("role").and_then(Value::as_str) == Some("assistant")
                && let Some(content) = payload.get("content").and_then(Value::as_str)
            {
                model.transcript.append_assistant(content);
            }
        }
        RuntimeEventKind::ApprovalRequested => request_approval(model, &payload),
        RuntimeEventKind::ApprovalResolved => resolve_approval_from_runtime(model, &payload),
        RuntimeEventKind::InputRequested => request_input(model, &payload),
        RuntimeEventKind::InputResolved => resolve_input_from_runtime(model, &payload),
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
        RuntimeEventKind::TurnEnded => finish_turn(model),
        RuntimeEventKind::ErrorRaised => {
            if let Some(message) = string(&payload, "message") {
                model.status_message = Some(message);
            }
            if payload.get("fatal").and_then(Value::as_bool) == Some(true) {
                finish_turn(model);
            }
        }
        RuntimeEventKind::StreamAborted => {
            finish_turn(model);
            model.status_message = string(&payload, "reason");
        }
        _ => {}
    }
}

fn bind_turn(model: &mut Model, event: &RuntimeEventEnvelope) {
    let operation_token = match &model.turn {
        TurnState::Starting { operation_token } => *operation_token,
        TurnState::Idle | TurnState::Streaming { .. } => {
            record_ignored(
                model,
                format!("ignored {} without a starting turn", event.kind()),
            );
            return;
        }
    };
    let Some(turn_id) = event.turn_id() else {
        record_ignored(model, "ignored turn.started without turn_id".into());
        return;
    };
    model.turn = TurnState::Streaming {
        operation_token,
        turn_id: turn_id.to_owned(),
    };
    model.overlay = Overlay::None;
    model.status_message = None;
    model.diagnostic_message = None;
}

fn belongs_to_active_turn(model: &mut Model, event: &RuntimeEventEnvelope) -> bool {
    let TurnState::Streaming { turn_id, .. } = &model.turn else {
        record_ignored(
            model,
            format!("ignored {} without an active turn", event.kind()),
        );
        return false;
    };
    if event.turn_id() != Some(turn_id.as_str()) {
        record_ignored(
            model,
            format!("ignored {} for a different turn", event.kind()),
        );
        return false;
    }
    true
}

fn is_projected_turn_event(kind: RuntimeEventKind) -> bool {
    matches!(
        kind,
        RuntimeEventKind::TurnEnded
            | RuntimeEventKind::ApprovalRequested
            | RuntimeEventKind::ApprovalResolved
            | RuntimeEventKind::InputRequested
            | RuntimeEventKind::InputResolved
            | RuntimeEventKind::ErrorRaised
            | RuntimeEventKind::StreamAborted
            | RuntimeEventKind::TextDelta
            | RuntimeEventKind::ToolCallStarted
            | RuntimeEventKind::ToolCallOutputDelta
            | RuntimeEventKind::ToolCallCompleted
    )
}

fn request_approval(model: &mut Model, payload: &Value) {
    model.overlay = Overlay::None;
    if model.interaction != Interaction::None {
        record_ignored(
            model,
            "ignored approval while another interaction is active".into(),
        );
        return;
    }
    if let (Some(approval_id), Some(tool), Some(summary), Some(options)) = (
        string(payload, "approval_id"),
        string(payload, "tool"),
        string(payload, "summary"),
        strings(payload, "options"),
    ) {
        let Some(turn_id) = active_turn_id(model) else {
            return;
        };
        let operation_token = model.allocate_operation_token();
        model.interaction = Interaction::ApprovalPending(ApprovalRequest {
            turn_id,
            approval_id,
            operation_token,
            tool,
            summary,
            options,
        });
        model.status_message = None;
        model.diagnostic_message = None;
    }
}

fn request_input(model: &mut Model, payload: &Value) {
    model.overlay = Overlay::None;
    if model.interaction != Interaction::None {
        record_ignored(
            model,
            "ignored input while another interaction is active".into(),
        );
        return;
    }
    if let (Some(input_id), Some(prompt)) = (string(payload, "input_id"), string(payload, "prompt"))
    {
        let Some(turn_id) = active_turn_id(model) else {
            return;
        };
        let operation_token = model.allocate_operation_token();
        model.interaction = Interaction::InputPending(InputRequest {
            turn_id,
            input_id,
            operation_token,
            prompt,
        });
        model.status_message = None;
        model.diagnostic_message = None;
    }
}

fn resolve_approval_from_runtime(model: &mut Model, payload: &Value) {
    let Some(approval_id) = payload.get("approval_id").and_then(Value::as_str) else {
        return;
    };
    let matches = match &model.interaction {
        Interaction::ApprovalPending(request) | Interaction::ApprovalResolving { request, .. } => {
            request.approval_id == approval_id
        }
        _ => false,
    };
    if matches {
        model.interaction = Interaction::None;
        model.status_message = None;
        model.diagnostic_message = None;
        model.last_backend_error = None;
    }
}

fn resolve_input_from_runtime(model: &mut Model, payload: &Value) {
    let Some(input_id) = payload.get("input_id").and_then(Value::as_str) else {
        return;
    };
    let matches = match &model.interaction {
        Interaction::InputPending(request) | Interaction::InputResolving { request, .. } => {
            request.input_id == input_id
        }
        _ => false,
    };
    if matches {
        model.interaction = Interaction::None;
        model.status_message = None;
        model.diagnostic_message = None;
        model.last_backend_error = None;
    }
}

fn finish_turn(model: &mut Model) {
    model.last_terminal_turn_token = match &model.turn {
        TurnState::Starting { operation_token }
        | TurnState::Streaming {
            operation_token, ..
        } => Some(*operation_token),
        TurnState::Idle => model.last_terminal_turn_token,
    };
    recover_turn_without_terminal(model);
}

fn recover_turn_without_terminal(model: &mut Model) {
    model.turn = TurnState::Idle;
    model.interaction = Interaction::None;
    model.pending_runtime_switch = None;
    model.pending_interrupt = None;
}

fn record_backend_error(model: &mut Model, error: BackendError) {
    if error.kind() == crate::backend::BackendErrorKind::WorkerPanic {
        model.connection = crate::model::ConnectionState::Failed(error.clone());
        recover_turn_without_terminal(model);
    }
    model.status_message = Some(error.safe_message().to_owned());
    model.diagnostic_message = None;
    model.last_backend_error = Some(error);
}

fn record_ignored(model: &mut Model, message: String) {
    model.diagnostic_message = Some(message);
}

fn runtime_token_is_active(model: &Model, operation_token: crate::model::OperationToken) -> bool {
    match &model.turn {
        TurnState::Starting {
            operation_token: active,
        }
        | TurnState::Streaming {
            operation_token: active,
            ..
        } => *active == operation_token,
        TurnState::Idle => false,
    }
}

fn active_turn_id(model: &Model) -> Option<String> {
    match &model.turn {
        TurnState::Streaming { turn_id, .. } => Some(turn_id.clone()),
        TurnState::Idle | TurnState::Starting { .. } => None,
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
