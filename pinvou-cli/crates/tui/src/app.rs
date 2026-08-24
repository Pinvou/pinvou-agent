use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    action::{Action, ApprovalDecision, Effect},
    backend::{Backend, BackendError, BackendErrorKind, RuntimeList, RuntimeStatus},
    model::{Interaction, Model, Overlay, TurnState},
    update::update,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Esc,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyKind {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyInput {
    pub key: Key,
    pub control: bool,
    pub kind: KeyKind,
}

impl KeyInput {
    pub const fn plain(key: Key) -> Self {
        Self {
            key,
            control: false,
            kind: KeyKind::Press,
        }
    }

    pub const fn ctrl(key: Key) -> Self {
        Self {
            key,
            control: true,
            kind: KeyKind::Press,
        }
    }
}

#[derive(Debug)]
pub enum InputEvent {
    Key(KeyInput),
    Paste(String),
    Resize(u16, u16),
    Eof,
}

pub trait Driver: Send + 'static {
    fn next_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<InputEvent>, AppError>> + Send + '_>>;
}

pub struct TerminalDriver {
    receiver: mpsc::UnboundedReceiver<InputEvent>,
    _reader: std::thread::JoinHandle<()>,
}

impl TerminalDriver {
    pub fn start() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let reader = terminal_input_task(sender);
        Self {
            receiver,
            _reader: reader,
        }
    }
}

impl Driver for TerminalDriver {
    fn next_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<InputEvent>, AppError>> + Send + '_>> {
        Box::pin(async move { Ok(self.receiver.recv().await) })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("backend initialization failed: {0}")]
    Backend(#[from] BackendError),
    #[error("terminal input failed: {0}")]
    Input(String),
}

#[derive(Debug)]
pub struct RunResult {
    pub model: Model,
    pub detached: bool,
}

enum AppEvent {
    Input(Result<Option<InputEvent>, AppError>),
    Action(Box<Action>),
}

pub async fn run_with_driver<B, D>(backend: Arc<B>, mut driver: D) -> Result<RunResult, AppError>
where
    B: Backend,
    D: Driver,
{
    let (workspace, runtimes) = initialize(backend.clone()).await?;
    let runtime = select_initial_runtime(&runtimes);
    let mut model = Model::new(workspace, runtime);
    model.runtime_candidates = runtimes.runtimes;
    model.selected_runtime = model
        .runtime_candidates
        .iter()
        .position(|candidate| candidate.id == model.runtime.id)
        .unwrap_or(0);

    let (events, mut incoming) = mpsc::unbounded_channel();
    let input_sender = events.clone();
    tokio::spawn(async move {
        loop {
            let result = driver.next_event().await;
            let stop = !matches!(result, Ok(Some(_)));
            if input_sender.send(AppEvent::Input(result)).is_err() || stop {
                break;
            }
        }
    });

    let mut detached = false;
    while !model.should_quit && !detached {
        let Some(event) = incoming.recv().await else {
            detached = true;
            break;
        };
        match event {
            AppEvent::Input(Ok(Some(input))) => {
                let outcome = handle_input(&mut model, input);
                detached = outcome.detached;
                dispatch_effects(backend.clone(), events.clone(), outcome.effects);
            }
            AppEvent::Input(Ok(None)) => detached = true,
            AppEvent::Input(Err(error)) => return Err(error),
            AppEvent::Action(action) => {
                let effects = update(&mut model, *action);
                dispatch_effects(backend.clone(), events.clone(), effects);
            }
        }
    }
    Ok(RunResult { model, detached })
}

async fn initialize<B: Backend>(backend: Arc<B>) -> Result<(PathBuf, RuntimeList), AppError> {
    join_backend(tokio::task::spawn_blocking(move || {
        let workspace = backend.workspace()?;
        let runtimes = backend.runtime_list()?;
        Ok((workspace, runtimes))
    }))
    .await
    .map_err(AppError::Backend)
}

fn select_initial_runtime(list: &RuntimeList) -> RuntimeStatus {
    list.active_runtime
        .as_ref()
        .and_then(|active| list.runtimes.iter().find(|runtime| &runtime.id == active))
        .or_else(|| list.runtimes.iter().find(|runtime| runtime.available))
        .cloned()
        .unwrap_or_else(|| RuntimeStatus::new("unavailable", "No runtime available", false))
}

struct InputOutcome {
    effects: Vec<Effect>,
    detached: bool,
}

fn outcome(effects: Vec<Effect>) -> InputOutcome {
    InputOutcome {
        effects,
        detached: false,
    }
}

fn handle_input(model: &mut Model, input: InputEvent) -> InputOutcome {
    match input {
        InputEvent::Eof => InputOutcome {
            effects: Vec::new(),
            detached: true,
        },
        InputEvent::Resize(width, height) => {
            model.terminal_size = Some((width, height));
            outcome(Vec::new())
        }
        InputEvent::Paste(value) => {
            insert_text(model, &value);
            outcome(Vec::new())
        }
        InputEvent::Key(key) => handle_key(model, key),
    }
}

fn handle_key(model: &mut Model, input: KeyInput) -> InputOutcome {
    if input.kind == KeyKind::Release {
        return outcome(Vec::new());
    }
    if input.control && input.key == Key::Char('c') {
        return InputOutcome {
            effects: Vec::new(),
            detached: true,
        };
    }
    if input.control && input.key == Key::Char('r') {
        return outcome(update(model, Action::LoadRuntimeList));
    }

    match &model.interaction {
        Interaction::ApprovalPending(_) => {
            let action = match input.key {
                Key::Char('1') => Some(Action::ApprovalChosen(ApprovalDecision::AllowOnce)),
                Key::Char('3') => Some(Action::ApprovalChosen(ApprovalDecision::Deny)),
                _ => None,
            };
            return outcome(action.map_or_else(Vec::new, |action| update(model, action)));
        }
        Interaction::ApprovalResolving { .. } | Interaction::InputResolving { .. } => {
            return outcome(Vec::new());
        }
        Interaction::InputPending(_) => {
            let effects = match input.key {
                Key::Enter => {
                    let value = std::mem::take(&mut model.input_composer.input);
                    update(model, Action::InputSubmitted(value))
                }
                Key::Backspace => {
                    model.input_composer.input.pop();
                    Vec::new()
                }
                Key::Char(ch) if !input.control => {
                    model.input_composer.input.push(ch);
                    Vec::new()
                }
                _ => Vec::new(),
            };
            return outcome(effects);
        }
        Interaction::None => {}
    }

    if model.overlay == Overlay::RuntimeList {
        let effects = match input.key {
            Key::Esc => {
                model.overlay = Overlay::None;
                Vec::new()
            }
            Key::Up => {
                model.selected_runtime = model.selected_runtime.saturating_sub(1);
                Vec::new()
            }
            Key::Down => {
                if model.selected_runtime + 1 < model.runtime_candidates.len() {
                    model.selected_runtime += 1;
                }
                Vec::new()
            }
            Key::Enter if model.pending_runtime_list.is_none() => model
                .runtime_candidates
                .get(model.selected_runtime)
                .filter(|runtime| runtime.available)
                .map(|runtime| runtime.id.clone())
                .map_or_else(Vec::new, |runtime| {
                    update(model, Action::RuntimeSwitch(runtime))
                }),
            _ => Vec::new(),
        };
        return outcome(effects);
    }

    if model.overlay != Overlay::None && input.key == Key::Esc {
        model.overlay = Overlay::None;
        return outcome(Vec::new());
    }

    if matches!(model.turn, TurnState::Streaming { .. }) && input.key == Key::Esc {
        return outcome(update(model, Action::Interrupt));
    }
    if !matches!(model.turn, TurnState::Idle) {
        return outcome(Vec::new());
    }

    let effects = match input.key {
        Key::Enter => {
            let value = std::mem::take(&mut model.composer.input);
            update(model, Action::Submit(value))
        }
        Key::Backspace => {
            model.composer.input.pop();
            Vec::new()
        }
        Key::Char(ch) if !input.control => {
            model.composer.input.push(ch);
            Vec::new()
        }
        Key::Up => {
            model.transcript_scroll = model.transcript_scroll.saturating_add(1);
            Vec::new()
        }
        Key::Down => {
            model.transcript_scroll = model.transcript_scroll.saturating_sub(1);
            Vec::new()
        }
        _ => Vec::new(),
    };
    outcome(effects)
}

fn insert_text(model: &mut Model, value: &str) {
    match model.interaction {
        Interaction::InputPending(_) => model.input_composer.input.push_str(value),
        Interaction::None if matches!(model.turn, TurnState::Idle) => {
            model.composer.input.push_str(value)
        }
        _ => {}
    }
}

fn dispatch_effects<B: Backend>(
    backend: Arc<B>,
    sender: mpsc::UnboundedSender<AppEvent>,
    effects: Vec<Effect>,
) {
    for effect in effects {
        run_effect(backend.clone(), sender.clone(), effect);
    }
}

fn run_effect<B: Backend>(
    backend: Arc<B>,
    sender: mpsc::UnboundedSender<AppEvent>,
    effect: Effect,
) {
    tokio::spawn(async move {
        let action = match effect {
            Effect::StartTurn {
                prompt,
                operation_token,
            } => {
                let event_sender = sender.clone();
                let result = join_backend(tokio::task::spawn_blocking(move || {
                    backend.stream_turn(
                        prompt,
                        Box::new(move |event| {
                            event_sender
                                .send(AppEvent::Action(Box::new(Action::Runtime {
                                    operation_token,
                                    event,
                                })))
                                .map_err(|_| channel_closed())
                        }),
                    )
                }))
                .await;
                Action::TurnStreamCompleted {
                    operation_token,
                    result,
                }
            }
            Effect::ResolveApproval {
                turn_id,
                approval_id,
                operation_token,
                accepted,
            } => {
                let call_id = approval_id.clone();
                let result = join_backend(tokio::task::spawn_blocking(move || {
                    backend.resolve_approval(call_id, accepted)
                }))
                .await;
                Action::ApprovalResolutionCompleted {
                    turn_id,
                    approval_id,
                    operation_token,
                    result,
                }
            }
            Effect::ResolveInput {
                turn_id,
                input_id,
                operation_token,
                value,
            } => {
                let call_id = input_id.clone();
                let result = join_backend(tokio::task::spawn_blocking(move || {
                    backend.resolve_input(call_id, value)
                }))
                .await;
                Action::InputResolutionCompleted {
                    turn_id,
                    input_id,
                    operation_token,
                    result,
                }
            }
            Effect::Interrupt {
                turn_id,
                operation_token,
            } => {
                let call_turn = turn_id.clone();
                let result = join_backend(tokio::task::spawn_blocking(move || {
                    backend.interrupt(call_turn)
                }))
                .await;
                Action::InterruptCompleted {
                    turn_id,
                    operation_token,
                    result,
                }
            }
            Effect::LoadRuntimeList { operation_token } => {
                let result =
                    join_backend(tokio::task::spawn_blocking(move || backend.runtime_list())).await;
                Action::RuntimeListLoaded {
                    operation_token,
                    result,
                }
            }
            Effect::SwitchRuntime {
                runtime,
                operation_token,
            } => {
                let result = join_backend(tokio::task::spawn_blocking(move || {
                    backend.switch_runtime(runtime)
                }))
                .await;
                Action::RuntimeSwitched {
                    operation_token,
                    result,
                }
            }
        };
        let _ = sender.send(AppEvent::Action(Box::new(action)));
    });
}

async fn join_backend<T>(task: JoinHandle<Result<T, BackendError>>) -> Result<T, BackendError> {
    match task.await {
        Ok(result) => result,
        Err(error) => Err(BackendError::new(
            BackendErrorKind::Operation,
            format!("backend task failed: {error}"),
        )),
    }
}

fn channel_closed() -> BackendError {
    BackendError::new(BackendErrorKind::Cancelled, "TUI event loop closed")
}

/// The sole production terminal reader. It translates crossterm events onto the app channel.
pub fn terminal_input_task(
    sender: mpsc::UnboundedSender<InputEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(raw) = event::read() {
            if let Some(input) = map_terminal_event(raw)
                && sender.send(input).is_err()
            {
                break;
            }
        }
        let _ = sender.send(InputEvent::Eof);
    })
}

fn map_terminal_event(event: Event) -> Option<InputEvent> {
    match event {
        Event::Key(key) => map_key(key).map(InputEvent::Key),
        Event::Paste(value) => Some(InputEvent::Paste(value)),
        Event::Resize(width, height) => Some(InputEvent::Resize(width, height)),
        _ => None,
    }
}

fn map_key(event: KeyEvent) -> Option<KeyInput> {
    let key = match event.code {
        KeyCode::Char(ch) => Key::Char(ch),
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Esc => Key::Esc,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        _ => return None,
    };
    let kind = match event.kind {
        KeyEventKind::Press => KeyKind::Press,
        KeyEventKind::Repeat => KeyKind::Repeat,
        KeyEventKind::Release => KeyKind::Release,
    };
    Some(KeyInput {
        key,
        control: event.modifiers.contains(KeyModifiers::CONTROL),
        kind,
    })
}
