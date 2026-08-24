use std::{
    collections::VecDeque,
    future::Future,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc as std_mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::{mpsc, oneshot};

use crate::{
    action::{Action, ApprovalDecision, Effect},
    backend::{Backend, BackendError, BackendErrorKind, RuntimeList, RuntimeStatus},
    model::{ConnectionState, Interaction, Model, OperationToken, Overlay, TurnState},
    update::update,
};

pub const CONTROL_CHANNEL_CAPACITY: usize = 64;
pub const RUNTIME_CHANNEL_CAPACITY: usize = 256;
pub const URGENT_INPUT_CHANNEL_CAPACITY: usize = 8;
pub const PREINIT_MAX_EVENTS: usize = 256;
pub const PREINIT_MAX_TEXT_BYTES: usize = 16 * 1024;
pub const DEFAULT_INITIALIZATION_TIMEOUT: Duration = Duration::from_secs(10);
const DETACH_WAIT: Duration = Duration::from_millis(500);
const TERMINAL_POLL_INTERVAL: Duration = Duration::from_millis(50);

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

pub trait Renderer: Send + 'static {
    fn draw(&mut self, model: &Model) -> Result<(), AppError>;
}

#[derive(Default)]
pub struct NoopRenderer;

impl Renderer for NoopRenderer {
    fn draw(&mut self, _model: &Model) -> Result<(), AppError> {
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("backend initialization failed: {0}")]
    Backend(#[from] BackendError),
    #[error("terminal input failed: {0}")]
    Input(String),
    #[error("render failed: {0}")]
    Render(String),
    #[error("{cause}; cleanup warnings: {cleanup_warnings:?}")]
    Cleanup {
        cause: Box<AppError>,
        cleanup_warnings: Vec<String>,
    },
}

impl AppError {
    pub fn cleanup_warnings(&self) -> &[String] {
        match self {
            Self::Cleanup {
                cleanup_warnings, ..
            } => cleanup_warnings,
            _ => &[],
        }
    }

    fn with_cleanup(self, cleanup_warnings: Vec<String>) -> Self {
        if cleanup_warnings.is_empty() {
            self
        } else {
            Self::Cleanup {
                cause: Box::new(self),
                cleanup_warnings,
            }
        }
    }
}

#[derive(Debug)]
pub struct RunResult {
    pub model: Model,
    pub detached: bool,
    pub cleanup_warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct AppConfig {
    pub initialization_timeout: Duration,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            initialization_timeout: DEFAULT_INITIALIZATION_TIMEOUT,
        }
    }
}

enum HighPriorityEvent {
    Input(Result<Option<InputEvent>, AppError>),
    Control(Box<Action>),
    Initialized(Result<(PathBuf, RuntimeList), BackendError>),
}

enum RuntimeStreamEvent {
    Event {
        operation_token: OperationToken,
        event: Box<pinvou_protocol::RuntimeEventEnvelope>,
    },
    Completed {
        operation_token: OperationToken,
        result: Result<(), BackendError>,
    },
}

#[derive(Default)]
struct PreInitBuffer {
    events: VecDeque<InputEvent>,
    text_bytes: usize,
    dropped: usize,
}

impl PreInitBuffer {
    fn push(&mut self, input: InputEvent, model: &mut Model) {
        if self.events.len() == PREINIT_MAX_EVENTS {
            self.note_dropped(1, model);
            return;
        }
        let input = match input {
            InputEvent::Paste(value) => {
                let remaining = PREINIT_MAX_TEXT_BYTES.saturating_sub(self.text_bytes);
                let keep = utf8_prefix_len(&value, remaining);
                if keep < value.len() {
                    self.note_dropped(1, model);
                }
                if keep == 0 {
                    return;
                }
                self.text_bytes += keep;
                InputEvent::Paste(value[..keep].to_owned())
            }
            InputEvent::Key(KeyInput {
                key: Key::Char(ch),
                control: false,
                kind: KeyKind::Press | KeyKind::Repeat,
            }) => {
                if self.text_bytes + ch.len_utf8() > PREINIT_MAX_TEXT_BYTES {
                    self.note_dropped(1, model);
                    return;
                }
                self.text_bytes += ch.len_utf8();
                InputEvent::Key(KeyInput {
                    key: Key::Char(ch),
                    control: false,
                    kind: KeyKind::Press,
                })
            }
            input => input,
        };
        self.events.push_back(input);
        self.project_preview(model);
    }

    fn note_dropped(&mut self, count: usize, model: &mut Model) {
        if count == 0 {
            return;
        }
        self.dropped = self.dropped.saturating_add(count);
        let message = format!("input buffer full; dropped {} inputs", self.dropped);
        model.status_message = Some(message.clone());
        model.diagnostic_message = Some(message);
    }

    fn project_preview(&self, model: &mut Model) {
        model.composer.input.clear();
        for input in &self.events {
            match input {
                InputEvent::Paste(value) => model.composer.input.push_str(value),
                InputEvent::Key(KeyInput {
                    key: Key::Char(ch),
                    control: false,
                    kind: KeyKind::Press | KeyKind::Repeat,
                }) => model.composer.input.push(*ch),
                InputEvent::Key(KeyInput {
                    key: Key::Backspace,
                    control: false,
                    kind: KeyKind::Press | KeyKind::Repeat,
                }) => {
                    model.composer.input.pop();
                }
                InputEvent::Key(KeyInput {
                    key: Key::Enter,
                    control: false,
                    kind: KeyKind::Press | KeyKind::Repeat,
                }) => model.composer.input.clear(),
                _ => {}
            }
        }
    }

    fn into_events(self) -> VecDeque<InputEvent> {
        self.events
    }
}

fn utf8_prefix_len(value: &str, max_bytes: usize) -> usize {
    if value.len() <= max_bytes {
        return value.len();
    }
    let mut end = max_bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    end
}

pub async fn run_with_driver<B, D>(backend: Arc<B>, driver: D) -> Result<RunResult, AppError>
where
    B: Backend,
    D: Driver,
{
    run_with_driver_and_renderer_config(backend, driver, NoopRenderer, AppConfig::default()).await
}

pub async fn run_with_driver_and_renderer<B, D, R>(
    backend: Arc<B>,
    driver: D,
    renderer: R,
) -> Result<RunResult, AppError>
where
    B: Backend,
    D: Driver,
    R: Renderer,
{
    run_with_driver_and_renderer_config(backend, driver, renderer, AppConfig::default()).await
}

pub async fn run_with_driver_and_renderer_config<B, D, R>(
    backend: Arc<B>,
    mut driver: D,
    mut renderer: R,
    config: AppConfig,
) -> Result<RunResult, AppError>
where
    B: Backend,
    D: Driver,
    R: Renderer,
{
    let mut model = Model::new(
        PathBuf::from("."),
        RuntimeStatus::new("initializing", "Initializing runtime", false),
    );
    model.connection = ConnectionState::Connecting;

    let (high_sender, mut high_receiver) = mpsc::channel(CONTROL_CHANNEL_CAPACITY);
    let (runtime_sender, mut runtime_receiver) = mpsc::channel(RUNTIME_CHANNEL_CAPACITY);
    let (urgent_sender, mut urgent_receiver) = mpsc::channel(URGENT_INPUT_CHANNEL_CAPACITY);
    let dropped_normal_inputs = Arc::new(AtomicUsize::new(0));
    let input_sender = high_sender.clone();
    let input_urgent_sender = urgent_sender.clone();
    let input_dropped = dropped_normal_inputs.clone();
    let (input_stop_sender, mut input_stop_receiver) = oneshot::channel();
    let input_worker = tokio::spawn(async move {
        loop {
            let result = tokio::select! {
                biased;
                _ = &mut input_stop_receiver => break,
                result = driver.next_event() => result,
            };
            match result {
                Ok(Some(input)) if is_urgent_input(&input) => {
                    if input_urgent_sender.send(Ok(Some(input))).await.is_err() {
                        break;
                    }
                }
                Ok(Some(input)) => {
                    match input_sender.try_send(HighPriorityEvent::Input(Ok(Some(input)))) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            input_dropped.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => break,
                    }
                }
                terminal @ (Ok(None) | Err(_)) => {
                    let _ = input_urgent_sender.send(terminal).await;
                    break;
                }
            }
        }
    });
    let (input_finished_sender, input_finished_receiver) = oneshot::channel();
    let input_supervisor = urgent_sender.clone();
    tokio::spawn(async move {
        if let Err(error) = input_worker.await {
            let _ = input_supervisor
                .send(Err(AppError::Input(format!(
                    "terminal input task failed: {error}"
                ))))
                .await;
        }
        let _ = input_finished_sender.send(());
    });
    let mut input_stop_sender = Some(input_stop_sender);
    let mut input_finished_receiver = Some(input_finished_receiver);

    if let Err(error) = renderer.draw(&model) {
        stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
        let cleanup_warnings =
            cleanup_backend(&backend, None, &mut high_receiver, &mut runtime_receiver);
        return Err(error.with_cleanup(cleanup_warnings));
    }

    let initialization_backend = backend.clone();
    let initialization_sender = high_sender.clone();
    thread::spawn(move || {
        let result = guarded_backend_call(|| {
            let workspace = initialization_backend.workspace()?;
            let runtimes = initialization_backend.runtime_list()?;
            Ok((workspace, runtimes))
        });
        let _ = initialization_sender.blocking_send(HighPriorityEvent::Initialized(result));
    });

    let initialization_deadline = tokio::time::sleep(config.initialization_timeout);
    tokio::pin!(initialization_deadline);
    let mut buffered_inputs = PreInitBuffer::default();
    loop {
        enum InitializationSelected {
            Urgent(Option<Result<Option<InputEvent>, AppError>>),
            High(Option<HighPriorityEvent>),
            Timeout,
        }
        let selected = tokio::select! {
            biased;
            event = urgent_receiver.recv() => InitializationSelected::Urgent(event),
            _ = &mut initialization_deadline => InitializationSelected::Timeout,
            event = high_receiver.recv() => InitializationSelected::High(event),
        };
        buffered_inputs.note_dropped(dropped_normal_inputs.swap(0, Ordering::Relaxed), &mut model);

        let event = match selected {
            InitializationSelected::Timeout => {
                let error = BackendError::new(
                    BackendErrorKind::Timeout,
                    "backend initialization timed out",
                );
                mark_initialization_failed(&mut model, &error);
                let render_result = renderer.draw(&model);
                stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
                let cleanup_warnings =
                    cleanup_backend(&backend, None, &mut high_receiver, &mut runtime_receiver);
                if let Err(render_error) = render_result {
                    return Err(render_error.with_cleanup(cleanup_warnings));
                }
                return Err(AppError::Backend(error).with_cleanup(cleanup_warnings));
            }
            InitializationSelected::Urgent(Some(Ok(Some(_))))
            | InitializationSelected::Urgent(Some(Ok(None)))
            | InitializationSelected::Urgent(None) => {
                drain_preinit_inputs(&mut high_receiver, &mut buffered_inputs, &mut model);
                buffered_inputs
                    .note_dropped(dropped_normal_inputs.swap(0, Ordering::Relaxed), &mut model);
                let render_result = renderer.draw(&model);
                stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
                let cleanup_warnings =
                    cleanup_backend(&backend, None, &mut high_receiver, &mut runtime_receiver);
                if let Err(error) = render_result {
                    return Err(error.with_cleanup(cleanup_warnings));
                }
                return Ok(RunResult {
                    model,
                    detached: true,
                    cleanup_warnings,
                });
            }
            InitializationSelected::Urgent(Some(Err(error))) => {
                stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
                let cleanup_warnings =
                    cleanup_backend(&backend, None, &mut high_receiver, &mut runtime_receiver);
                return Err(error.with_cleanup(cleanup_warnings));
            }
            InitializationSelected::High(event) => event,
        };

        match event {
            Some(HighPriorityEvent::Initialized(Ok((workspace, runtimes)))) => {
                model.workspace = workspace;
                model.runtime = select_initial_runtime(&runtimes);
                model.runtime_candidates = runtimes.runtimes;
                model.selected_runtime = model
                    .runtime_candidates
                    .iter()
                    .position(|runtime| runtime.id == model.runtime.id)
                    .unwrap_or(0);
                model.connection = ConnectionState::Connected;
                model.composer.input.clear();
                if let Err(error) = renderer.draw(&model) {
                    stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
                    let cleanup_warnings =
                        cleanup_backend(&backend, None, &mut high_receiver, &mut runtime_receiver);
                    return Err(error.with_cleanup(cleanup_warnings));
                }
                break;
            }
            Some(HighPriorityEvent::Initialized(Err(error))) => {
                mark_initialization_failed(&mut model, &error);
                let render_result = renderer.draw(&model);
                stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
                let cleanup_warnings =
                    cleanup_backend(&backend, None, &mut high_receiver, &mut runtime_receiver);
                if let Err(render_error) = render_result {
                    return Err(render_error.with_cleanup(cleanup_warnings));
                }
                return Err(AppError::Backend(error).with_cleanup(cleanup_warnings));
            }
            Some(HighPriorityEvent::Input(Ok(Some(InputEvent::Resize(width, height))))) => {
                model.terminal_size = Some((width, height));
                if let Err(error) = renderer.draw(&model) {
                    stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
                    let cleanup_warnings =
                        cleanup_backend(&backend, None, &mut high_receiver, &mut runtime_receiver);
                    return Err(error.with_cleanup(cleanup_warnings));
                }
            }
            Some(HighPriorityEvent::Input(Ok(Some(InputEvent::Eof))))
            | Some(HighPriorityEvent::Input(Ok(None)))
            | None => {
                stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
                let cleanup_warnings =
                    cleanup_backend(&backend, None, &mut high_receiver, &mut runtime_receiver);
                return Ok(RunResult {
                    model,
                    detached: true,
                    cleanup_warnings,
                });
            }
            Some(HighPriorityEvent::Input(Ok(Some(InputEvent::Key(key)))))
                if key.control && key.key == Key::Char('c') && key.kind != KeyKind::Release =>
            {
                stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
                let cleanup_warnings =
                    cleanup_backend(&backend, None, &mut high_receiver, &mut runtime_receiver);
                return Ok(RunResult {
                    model,
                    detached: true,
                    cleanup_warnings,
                });
            }
            Some(HighPriorityEvent::Input(Ok(Some(input)))) => {
                buffered_inputs.push(input, &mut model);
                if let Err(error) = renderer.draw(&model) {
                    stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
                    let cleanup_warnings =
                        cleanup_backend(&backend, None, &mut high_receiver, &mut runtime_receiver);
                    return Err(error.with_cleanup(cleanup_warnings));
                }
            }
            Some(HighPriorityEvent::Input(Err(error))) => {
                stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
                let cleanup_warnings =
                    cleanup_backend(&backend, None, &mut high_receiver, &mut runtime_receiver);
                return Err(error.with_cleanup(cleanup_warnings));
            }
            Some(HighPriorityEvent::Control(_)) => {}
        }
    }

    let mut detached = false;
    for input in buffered_inputs.into_events() {
        let outcome = handle_input(&mut model, input);
        detached |= outcome.detached;
        if let Err(error) = renderer.draw(&model) {
            stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
            let cleanup_warnings = cleanup_backend(
                &backend,
                active_stream_token(&model),
                &mut high_receiver,
                &mut runtime_receiver,
            );
            return Err(error.with_cleanup(cleanup_warnings));
        }
        dispatch_effects(
            backend.clone(),
            high_sender.clone(),
            runtime_sender.clone(),
            outcome.effects,
        );
        if model.should_quit || detached {
            break;
        }
    }

    while !model.should_quit && !detached {
        enum Selected {
            Urgent(Option<Result<Option<InputEvent>, AppError>>),
            High(Option<HighPriorityEvent>),
            Runtime(Option<RuntimeStreamEvent>),
        }
        let selected = tokio::select! {
            biased;
            event = urgent_receiver.recv() => Selected::Urgent(event),
            event = high_receiver.recv() => Selected::High(event),
            event = runtime_receiver.recv() => Selected::Runtime(event),
        };

        note_dropped_inputs(&mut model, dropped_normal_inputs.swap(0, Ordering::Relaxed));

        let effects = match selected {
            Selected::Urgent(Some(Ok(Some(input)))) => {
                drain_normal_before_detach(&mut high_receiver, &mut model);
                let outcome = handle_input(&mut model, input);
                detached = outcome.detached;
                outcome.effects
            }
            Selected::Urgent(Some(Ok(None))) => {
                drain_normal_before_driver_closed(&mut high_receiver, &mut model);
                detached = !model.should_quit;
                Vec::new()
            }
            Selected::Urgent(None) => {
                detached = true;
                Vec::new()
            }
            Selected::Urgent(Some(Err(error))) => {
                stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
                let cleanup_warnings = cleanup_backend(
                    &backend,
                    active_stream_token(&model),
                    &mut high_receiver,
                    &mut runtime_receiver,
                );
                return Err(error.with_cleanup(cleanup_warnings));
            }
            Selected::High(Some(HighPriorityEvent::Input(Ok(Some(input))))) => {
                let outcome = handle_input(&mut model, input);
                detached = outcome.detached;
                outcome.effects
            }
            Selected::High(Some(HighPriorityEvent::Input(Ok(None)))) => {
                detached = true;
                Vec::new()
            }
            Selected::High(Some(HighPriorityEvent::Input(Err(error)))) => {
                stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
                let cleanup_warnings = cleanup_backend(
                    &backend,
                    active_stream_token(&model),
                    &mut high_receiver,
                    &mut runtime_receiver,
                );
                return Err(error.with_cleanup(cleanup_warnings));
            }
            Selected::High(Some(HighPriorityEvent::Control(action))) => update(&mut model, *action),
            Selected::High(Some(HighPriorityEvent::Initialized(_))) => Vec::new(),
            Selected::High(None) => {
                detached = true;
                Vec::new()
            }
            Selected::Runtime(Some(RuntimeStreamEvent::Event {
                operation_token,
                event,
            })) => update(
                &mut model,
                Action::Runtime {
                    operation_token,
                    event: *event,
                },
            ),
            Selected::Runtime(Some(RuntimeStreamEvent::Completed {
                operation_token,
                result,
            })) => update(
                &mut model,
                Action::TurnStreamCompleted {
                    operation_token,
                    result,
                },
            ),
            Selected::Runtime(None) => Vec::new(),
        };

        if let Err(error) = renderer.draw(&model) {
            stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
            let cleanup_warnings = cleanup_backend(
                &backend,
                active_stream_token(&model),
                &mut high_receiver,
                &mut runtime_receiver,
            );
            return Err(error.with_cleanup(cleanup_warnings));
        }
        dispatch_effects(
            backend.clone(),
            high_sender.clone(),
            runtime_sender.clone(),
            effects,
        );
    }

    stop_input_worker(&mut input_stop_sender, &mut input_finished_receiver).await;
    let cleanup_warnings = if detached {
        cleanup_backend(
            &backend,
            active_stream_token(&model),
            &mut high_receiver,
            &mut runtime_receiver,
        )
    } else {
        Vec::new()
    };
    Ok(RunResult {
        model,
        detached,
        cleanup_warnings,
    })
}

async fn stop_input_worker(
    stop: &mut Option<oneshot::Sender<()>>,
    finished: &mut Option<oneshot::Receiver<()>>,
) {
    if let Some(stop) = stop.take() {
        let _ = stop.send(());
    }
    if let Some(finished) = finished.take() {
        let _ = tokio::time::timeout(Duration::from_millis(200), finished).await;
    }
}

fn select_initial_runtime(list: &RuntimeList) -> RuntimeStatus {
    list.active_runtime
        .as_ref()
        .and_then(|active| list.runtimes.iter().find(|runtime| &runtime.id == active))
        .or_else(|| list.runtimes.iter().find(|runtime| runtime.available))
        .cloned()
        .unwrap_or_else(|| RuntimeStatus::new("unavailable", "No runtime available", false))
}

fn mark_initialization_failed(model: &mut Model, error: &BackendError) {
    model.connection = ConnectionState::Failed(error.clone());
    model.status_message = Some(error.safe_message().to_owned());
    model.last_backend_error = Some(error.clone());
}

fn is_urgent_input(input: &InputEvent) -> bool {
    matches!(input, InputEvent::Eof)
        || matches!(
            input,
            InputEvent::Key(KeyInput {
                key: Key::Char('c'),
                control: true,
                kind: KeyKind::Press | KeyKind::Repeat,
            })
        )
}

fn note_dropped_inputs(model: &mut Model, count: usize) {
    if count > 0 {
        let message = format!("input queue full; dropped {count} inputs");
        model.status_message = Some(message.clone());
        model.diagnostic_message = Some(message);
    }
}

fn drain_preinit_inputs(
    receiver: &mut mpsc::Receiver<HighPriorityEvent>,
    buffered: &mut PreInitBuffer,
    model: &mut Model,
) {
    while let Ok(event) = receiver.try_recv() {
        match event {
            HighPriorityEvent::Input(Ok(Some(InputEvent::Resize(width, height)))) => {
                model.terminal_size = Some((width, height));
            }
            HighPriorityEvent::Input(Ok(Some(input))) => buffered.push(input, model),
            HighPriorityEvent::Input(Ok(None) | Err(_))
            | HighPriorityEvent::Control(_)
            | HighPriorityEvent::Initialized(_) => {}
        }
    }
}

fn drain_normal_before_detach(receiver: &mut mpsc::Receiver<HighPriorityEvent>, model: &mut Model) {
    while let Ok(event) = receiver.try_recv() {
        match event {
            HighPriorityEvent::Input(Ok(Some(
                input @ (InputEvent::Paste(_)
                | InputEvent::Resize(_, _)
                | InputEvent::Key(KeyInput {
                    key: Key::Char(_) | Key::Backspace,
                    ..
                })),
            ))) => {
                let _ = handle_input(model, input);
            }
            HighPriorityEvent::Input(Ok(Some(
                input @ InputEvent::Key(KeyInput {
                    key: Key::Enter, ..
                }),
            ))) if matches!(model.connection, ConnectionState::Failed(_)) => {
                let _ = handle_input(model, input);
            }
            HighPriorityEvent::Control(action) => {
                let _ = update(model, *action);
            }
            HighPriorityEvent::Input(_) | HighPriorityEvent::Initialized(_) => {}
        }
    }
}

fn drain_normal_before_driver_closed(
    receiver: &mut mpsc::Receiver<HighPriorityEvent>,
    model: &mut Model,
) {
    while let Ok(event) = receiver.try_recv() {
        match event {
            HighPriorityEvent::Input(Ok(Some(
                input @ (InputEvent::Paste(_)
                | InputEvent::Resize(_, _)
                | InputEvent::Key(KeyInput {
                    key: Key::Char(_) | Key::Backspace,
                    ..
                })),
            ))) => {
                let _ = handle_input(model, input);
            }
            HighPriorityEvent::Input(Ok(Some(
                input @ InputEvent::Key(KeyInput {
                    key: Key::Enter, ..
                }),
            ))) if matches!(model.connection, ConnectionState::Failed(_))
                || model.composer.input.starts_with('/') =>
            {
                let _ = handle_input(model, input);
            }
            HighPriorityEvent::Control(action) => {
                let _ = update(model, *action);
            }
            HighPriorityEvent::Input(_) | HighPriorityEvent::Initialized(_) => {}
        }
    }
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
    high_sender: mpsc::Sender<HighPriorityEvent>,
    runtime_sender: mpsc::Sender<RuntimeStreamEvent>,
    effects: Vec<Effect>,
) {
    for effect in effects {
        match effect {
            Effect::StartTurn {
                prompt,
                operation_token,
            } => start_stream_worker(
                backend.clone(),
                runtime_sender.clone(),
                prompt,
                operation_token,
            ),
            control => start_control_effect(backend.clone(), high_sender.clone(), control),
        }
    }
}

fn start_stream_worker<B: Backend>(
    backend: Arc<B>,
    runtime_sender: mpsc::Sender<RuntimeStreamEvent>,
    prompt: String,
    operation_token: OperationToken,
) {
    thread::spawn(move || {
        let event_sender = runtime_sender.clone();
        let result = catch_unwind(AssertUnwindSafe(|| {
            backend.stream_turn(
                operation_token.as_u64(),
                prompt,
                Box::new(move |event| {
                    event_sender
                        .blocking_send(RuntimeStreamEvent::Event {
                            operation_token,
                            event: Box::new(event),
                        })
                        .map_err(|_| channel_closed())
                }),
            )
        }))
        .unwrap_or_else(|_| {
            Err(BackendError::new(
                BackendErrorKind::WorkerPanic,
                "backend stream worker panicked",
            ))
        });
        let _ = runtime_sender.blocking_send(RuntimeStreamEvent::Completed {
            operation_token,
            result,
        });
    });
}

fn start_control_effect<B: Backend>(
    backend: Arc<B>,
    sender: mpsc::Sender<HighPriorityEvent>,
    effect: Effect,
) {
    thread::spawn(move || {
        let action = match effect {
            Effect::ResolveApproval {
                turn_id,
                approval_id,
                operation_token,
                accepted,
            } => {
                let call_id = approval_id.clone();
                let result = guarded_backend_call(|| backend.resolve_approval(call_id, accepted));
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
                let result = guarded_backend_call(|| backend.resolve_input(call_id, value));
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
                let result = guarded_backend_call(|| backend.interrupt(call_turn));
                Action::InterruptCompleted {
                    turn_id,
                    operation_token,
                    result,
                }
            }
            Effect::LoadRuntimeList { operation_token } => {
                let result = guarded_backend_call(|| backend.runtime_list());
                Action::RuntimeListLoaded {
                    operation_token,
                    result,
                }
            }
            Effect::SwitchRuntime {
                runtime,
                operation_token,
            } => {
                let result = guarded_backend_call(|| backend.switch_runtime(runtime));
                Action::RuntimeSwitched {
                    operation_token,
                    result,
                }
            }
            Effect::StartTurn { .. } => return,
        };
        let _ = sender.blocking_send(HighPriorityEvent::Control(Box::new(action)));
    });
}

fn guarded_backend_call<T>(
    call: impl FnOnce() -> Result<T, BackendError>,
) -> Result<T, BackendError> {
    catch_unwind(AssertUnwindSafe(call)).unwrap_or_else(|_| {
        Err(BackendError::new(
            BackendErrorKind::WorkerPanic,
            "backend worker panicked",
        ))
    })
}

fn active_stream_token(model: &Model) -> Option<OperationToken> {
    match model.turn {
        TurnState::Starting { operation_token }
        | TurnState::Streaming {
            operation_token, ..
        } => Some(operation_token),
        TurnState::Idle => None,
    }
}

fn cleanup_backend<B: Backend>(
    backend: &Arc<B>,
    operation_token: Option<OperationToken>,
    high_receiver: &mut mpsc::Receiver<HighPriorityEvent>,
    runtime_receiver: &mut mpsc::Receiver<RuntimeStreamEvent>,
) -> Vec<String> {
    high_receiver.close();
    runtime_receiver.close();
    let (completed, waiting) = std_mpsc::channel();
    let mut pending = Vec::new();
    if let Some(operation_token) = operation_token {
        pending.push("detach_stream");
        let backend = backend.clone();
        let completed = completed.clone();
        thread::spawn(move || {
            let result = guarded_backend_call(|| backend.detach_stream(operation_token.as_u64()));
            let _ = completed.send(("detach_stream", result));
        });
    }
    pending.push("detach_controls");
    let backend = backend.clone();
    thread::spawn(move || {
        let result = guarded_backend_call(|| backend.detach_controls());
        let _ = completed.send(("detach_controls", result));
    });

    let deadline = Instant::now() + DETACH_WAIT;
    let mut warnings = Vec::new();
    while !pending.is_empty() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match waiting.recv_timeout(remaining) {
            Ok((operation, Ok(()))) => pending.retain(|pending| *pending != operation),
            Ok((operation, Err(error))) => {
                pending.retain(|pending| *pending != operation);
                warnings.push(format!("{operation}: {}", error.safe_message()));
            }
            Err(_) => break,
        }
    }
    warnings.extend(
        pending
            .into_iter()
            .map(|operation| format!("{operation}: cleanup timed out")),
    );
    warnings
}

fn channel_closed() -> BackendError {
    BackendError::new(BackendErrorKind::Cancelled, "TUI runtime receiver closed")
}

pub trait TerminalEventSource: Send + 'static {
    /// Must return no later than `timeout`, so reader shutdown remains bounded.
    fn poll(&mut self, timeout: Duration) -> io::Result<bool>;
    /// After `poll` returns true, this must promptly return the ready event.
    fn read(&mut self) -> io::Result<Event>;
}

struct CrosstermEventSource;
impl TerminalEventSource for CrosstermEventSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        event::poll(timeout)
    }
    fn read(&mut self) -> io::Result<Event> {
        event::read()
    }
}

pub struct TerminalInputTask {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    finished: std_mpsc::Receiver<()>,
}

impl Drop for TerminalInputTask {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if self
            .finished
            .recv_timeout(TERMINAL_POLL_INTERVAL.saturating_mul(2))
            .is_ok()
            && let Some(thread) = self.thread.take()
        {
            let _ = thread.join();
        }
    }
}

pub fn terminal_input_task(sender: mpsc::Sender<InputEvent>) -> TerminalInputTask {
    terminal_input_task_with_source(sender, CrosstermEventSource)
}

pub fn terminal_input_task_with_source<S: TerminalEventSource>(
    sender: mpsc::Sender<InputEvent>,
    mut source: S,
) -> TerminalInputTask {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let (finished_sender, finished) = std_mpsc::sync_channel(1);
    let thread = thread::spawn(move || {
        while !thread_stop.load(Ordering::Acquire) {
            match source.poll(TERMINAL_POLL_INTERVAL) {
                Ok(true) => match source.read() {
                    Ok(raw) => {
                        if let Some(input) = map_terminal_event(raw)
                            && sender.blocking_send(input).is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
        let _ = finished_sender.send(());
    });
    TerminalInputTask {
        stop,
        thread: Some(thread),
        finished,
    }
}

pub struct TerminalDriver {
    receiver: mpsc::Receiver<InputEvent>,
    _reader: TerminalInputTask,
}

impl TerminalDriver {
    pub fn start() -> Self {
        let (sender, receiver) = mpsc::channel(CONTROL_CHANNEL_CAPACITY);
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

#[cfg(test)]
mod tests {
    use std::{
        io,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    use crossterm::event::Event;
    use tokio::sync::mpsc;

    use super::{
        InputEvent, Key, KeyInput, PREINIT_MAX_EVENTS, PREINIT_MAX_TEXT_BYTES, PreInitBuffer,
        TerminalEventSource, terminal_input_task_with_source,
    };
    use crate::{backend::RuntimeStatus, model::Model};

    struct PollingSource {
        polls: Arc<AtomicUsize>,
    }

    impl TerminalEventSource for PollingSource {
        fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
            self.polls.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(timeout.min(Duration::from_millis(5)));
            Ok(false)
        }

        fn read(&mut self) -> io::Result<Event> {
            unreachable!("read is only called after poll returns true")
        }
    }

    #[test]
    fn terminal_reader_drop_stops_and_joins_before_reentry() {
        for _ in 0..2 {
            let polls = Arc::new(AtomicUsize::new(0));
            let (sender, _receiver) = mpsc::channel(1);
            let task = terminal_input_task_with_source(
                sender,
                PollingSource {
                    polls: polls.clone(),
                },
            );
            while polls.load(Ordering::Relaxed) == 0 {
                std::thread::yield_now();
            }
            let started = Instant::now();
            drop(task);
            assert!(started.elapsed() < Duration::from_millis(200));
        }
    }

    #[test]
    fn preinit_buffer_caps_events_and_utf8_text_without_retaining_overflow() {
        let mut buffer = PreInitBuffer::default();
        let mut model = Model::new(
            PathBuf::from("workspace"),
            RuntimeStatus::new("codex", "Codex", true),
        );
        buffer.push(
            InputEvent::Paste("界".repeat(PREINIT_MAX_TEXT_BYTES / 3 + 2)),
            &mut model,
        );
        for _ in 1..=PREINIT_MAX_EVENTS {
            buffer.push(InputEvent::Key(KeyInput::plain(Key::Enter)), &mut model);
        }

        assert_eq!(buffer.events.len(), PREINIT_MAX_EVENTS);
        assert!(buffer.text_bytes <= PREINIT_MAX_TEXT_BYTES);
        assert!(matches!(
            buffer.events.front(),
            Some(InputEvent::Paste(value))
                if value.len() == PREINIT_MAX_TEXT_BYTES - PREINIT_MAX_TEXT_BYTES % '界'.len_utf8()
        ));
        assert_eq!(buffer.dropped, 2);
        assert!(
            model
                .status_message
                .as_deref()
                .unwrap()
                .contains("dropped 2 inputs")
        );
    }
}
