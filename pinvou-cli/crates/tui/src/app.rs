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
    commands::suggestions,
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
const UI_TICK_INTERVAL: Duration = Duration::from_millis(200);
const INITIALIZATION_OPERATION_TOKEN: u64 = 0;

trait ThreadSpawner: Clone + Send + Sync + 'static {
    fn spawn(&self, job: Box<dyn FnOnce() + Send>);
}

#[derive(Clone, Copy)]
struct OsThreadSpawner;

impl ThreadSpawner for OsThreadSpawner {
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        thread::spawn(job);
    }
}

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
    Tick(Instant),
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
        if matches!(input, InputEvent::Tick(_)) {
            return;
        }
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
    driver: D,
    renderer: R,
    config: AppConfig,
) -> Result<RunResult, AppError>
where
    B: Backend,
    D: Driver,
    R: Renderer,
{
    run_with_driver_and_renderer_config_spawner(backend, driver, renderer, config, OsThreadSpawner)
        .await
}

async fn run_with_driver_and_renderer_config_spawner<B, D, R, S>(
    backend: Arc<B>,
    mut driver: D,
    mut renderer: R,
    config: AppConfig,
    spawner: S,
) -> Result<RunResult, AppError>
where
    B: Backend,
    D: Driver,
    R: Renderer,
    S: ThreadSpawner,
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
                Ok(Some(input @ InputEvent::Tick(_))) => {
                    match input_sender.try_send(HighPriorityEvent::Input(Ok(Some(input)))) {
                        Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                        Err(mpsc::error::TrySendError::Closed(_)) => break,
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

    let mut immediate_initialization =
        start_initialization_worker(backend.clone(), high_sender.clone(), spawner.clone());

    let initialization_deadline = tokio::time::sleep(config.initialization_timeout);
    tokio::pin!(initialization_deadline);
    let mut buffered_inputs = PreInitBuffer::default();
    loop {
        enum InitializationSelected {
            Urgent(Option<Result<Option<InputEvent>, AppError>>),
            High(Option<HighPriorityEvent>),
            Timeout,
        }
        let selected = if let Some(event) = immediate_initialization.take() {
            InitializationSelected::High(Some(event))
        } else {
            tokio::select! {
                biased;
                event = urgent_receiver.recv() => InitializationSelected::Urgent(event),
                _ = &mut initialization_deadline => InitializationSelected::Timeout,
                event = high_receiver.recv() => InitializationSelected::High(event),
            }
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
            Some(HighPriorityEvent::Input(Ok(Some(InputEvent::Tick(_))))) => continue,
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

    let model_status_effects = update(&mut model, Action::RefreshModelStatus);
    dispatch_effects(
        backend.clone(),
        high_sender.clone(),
        runtime_sender.clone(),
        &mut model,
        model_status_effects,
        spawner.clone(),
    );

    let mut detached = false;
    for input in buffered_inputs.into_events() {
        let outcome = handle_input(&mut model, input);
        detached |= outcome.detached;
        dispatch_effects(
            backend.clone(),
            high_sender.clone(),
            runtime_sender.clone(),
            &mut model,
            outcome.effects,
            spawner.clone(),
        );
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
                if matches!(input, InputEvent::Tick(_))
                    && (model.turn == TurnState::Idle
                        || !matches!(model.interaction, Interaction::None))
                {
                    continue;
                }
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

        dispatch_effects(
            backend.clone(),
            high_sender.clone(),
            runtime_sender.clone(),
            &mut model,
            effects,
            spawner.clone(),
        );
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
    model.transcript.push_error(error.safe_message());
}

fn start_initialization_worker<B: Backend, S: ThreadSpawner>(
    backend: Arc<B>,
    sender: mpsc::Sender<HighPriorityEvent>,
    spawner: S,
) -> Option<HighPriorityEvent> {
    match backend.begin_control(INITIALIZATION_OPERATION_TOKEN) {
        Ok(()) => {
            spawner.spawn(Box::new(move || {
                let result = guarded_backend_call(|| {
                    let workspace = backend.workspace()?;
                    let runtimes = backend.runtime_list(INITIALIZATION_OPERATION_TOKEN)?;
                    Ok((workspace, runtimes))
                });
                let _ = sender.blocking_send(HighPriorityEvent::Initialized(result));
            }));
            None
        }
        Err(error) => {
            let event = HighPriorityEvent::Initialized(Err(error));
            match sender.try_send(event) {
                Ok(()) => None,
                Err(error) => Some(error.into_inner()),
            }
        }
    }
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
        InputEvent::Tick(now) => {
            model.advance_activity(now);
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
    if model.overlay == Overlay::ResumeList {
        let matching = model
            .session_candidates
            .iter()
            .filter(|session| {
                let query = model.session_query.to_ascii_lowercase();
                query.is_empty()
                    || session.title.to_ascii_lowercase().contains(&query)
                    || session.id.to_ascii_lowercase().contains(&query)
            })
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        let effects = match input.key {
            Key::Esc => {
                model.overlay = Overlay::None;
                Vec::new()
            }
            Key::Up => {
                model.selected_session = model.selected_session.saturating_sub(1);
                Vec::new()
            }
            Key::Down => {
                if model.selected_session + 1 < matching.len() {
                    model.selected_session += 1;
                }
                Vec::new()
            }
            Key::Backspace => {
                model.session_query.pop();
                model.selected_session = 0;
                Vec::new()
            }
            Key::Char(ch) if !input.control => {
                model.session_query.push(ch);
                model.selected_session = 0;
                Vec::new()
            }
            Key::Enter if model.pending_session_list.is_none() => matching
                .get(model.selected_session)
                .cloned()
                .map_or_else(Vec::new, |id| update(model, Action::ResumeSession(id))),
            _ => Vec::new(),
        };
        return outcome(effects);
    }
    if model.overlay == Overlay::ModelList {
        let effects = match input.key {
            Key::Esc => {
                model.overlay = Overlay::None;
                Vec::new()
            }
            Key::Up => {
                model.selected_model = model.selected_model.saturating_sub(1);
                Vec::new()
            }
            Key::Down => {
                if model.selected_model + 1 < model.model_candidates.len() {
                    model.selected_model += 1;
                }
                Vec::new()
            }
            Key::Enter if model.pending_model_list.is_none() => model
                .model_candidates
                .get(model.selected_model)
                .filter(|candidate| candidate.available)
                .map(|candidate| {
                    (
                        candidate.id.clone(),
                        candidate.configured,
                        candidate.supported_reasoning_levels.clone(),
                        candidate.default_reasoning_level.clone(),
                    )
                })
                .map_or_else(Vec::new, |(model_id, configured, levels, default_level)| {
                    if !configured {
                        model.credential_composer.clear();
                        model.overlay = Overlay::ApiKeyInput;
                        return Vec::new();
                    }
                    if levels.is_empty() {
                        update(
                            model,
                            Action::ModelSwitch {
                                model_id,
                                reasoning_level: None,
                            },
                        )
                    } else {
                        let selected = model
                            .model_level
                            .as_ref()
                            .filter(|_| Some(model_id.as_str()) == model.model_id.as_deref())
                            .or(default_level.as_ref())
                            .and_then(|level| levels.iter().position(|item| item == level))
                            .unwrap_or(0);
                        model.selected_model_level = selected;
                        model.overlay = Overlay::ModelLevelList;
                        Vec::new()
                    }
                }),
            _ => Vec::new(),
        };
        return outcome(effects);
    }
    if model.overlay == Overlay::ApiKeyInput {
        let effects = match input.key {
            Key::Esc if model.pending_model_credential.is_none() => {
                model.credential_composer.clear();
                model.overlay = Overlay::ModelList;
                Vec::new()
            }
            Key::Backspace if model.pending_model_credential.is_none() => {
                model.credential_composer.pop();
                Vec::new()
            }
            Key::Char(character) if !input.control && model.pending_model_credential.is_none() => {
                model.credential_composer.push(character);
                Vec::new()
            }
            Key::Enter
                if model.pending_model_credential.is_none()
                    && model.credential_composer.len() > 0 =>
            {
                let api_key = crate::action::SecretInput::new(model.credential_composer.take());
                model
                    .model_candidates
                    .get(model.selected_model)
                    .map(|candidate| candidate.id.clone())
                    .map_or_else(Vec::new, |model_id| {
                        update(
                            model,
                            Action::ModelCredentialSubmitted { model_id, api_key },
                        )
                    })
            }
            _ => Vec::new(),
        };
        return outcome(effects);
    }
    if model.overlay == Overlay::ModelLevelList {
        let levels = model
            .model_candidates
            .get(model.selected_model)
            .map(|candidate| candidate.supported_reasoning_levels.clone())
            .unwrap_or_default();
        let effects = match input.key {
            Key::Esc => {
                model.overlay = Overlay::ModelList;
                Vec::new()
            }
            Key::Up => {
                model.selected_model_level = model.selected_model_level.saturating_sub(1);
                Vec::new()
            }
            Key::Down => {
                if model.selected_model_level + 1 < levels.len() {
                    model.selected_model_level += 1;
                }
                Vec::new()
            }
            Key::Enter => model
                .model_candidates
                .get(model.selected_model)
                .zip(levels.get(model.selected_model_level))
                .map(|(candidate, level)| (candidate.id.clone(), level.clone()))
                .map_or_else(Vec::new, |(model_id, reasoning_level)| {
                    update(
                        model,
                        Action::ModelSwitch {
                            model_id,
                            reasoning_level: Some(reasoning_level),
                        },
                    )
                }),
            _ => Vec::new(),
        };
        return outcome(effects);
    }
    if model.overlay == Overlay::PermissionList {
        let profiles = model
            .permission_status
            .as_ref()
            .map(|status| status.supported_profiles.clone())
            .unwrap_or_default();
        let effects = match input.key {
            Key::Esc => {
                model.overlay = Overlay::None;
                Vec::new()
            }
            Key::Up => {
                model.selected_permission = model.selected_permission.saturating_sub(1);
                Vec::new()
            }
            Key::Down => {
                if model.selected_permission + 1 < profiles.len() {
                    model.selected_permission += 1;
                }
                Vec::new()
            }
            Key::Enter if model.pending_permissions.is_none() => profiles
                .get(model.selected_permission)
                .copied()
                .map_or_else(Vec::new, |profile| {
                    update(
                        model,
                        Action::PermissionSwitch {
                            profile,
                            full_access_confirmed: false,
                        },
                    )
                }),
            _ => Vec::new(),
        };
        return outcome(effects);
    }
    if model.overlay == Overlay::FullAccessConfirmation {
        let effects = match input.key {
            Key::Esc => {
                model.overlay = Overlay::PermissionList;
                model.status_message = None;
                Vec::new()
            }
            Key::Enter => update(
                model,
                Action::PermissionSwitch {
                    profile: crate::backend::PermissionMode::FullAccess,
                    full_access_confirmed: true,
                },
            ),
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

    let command_suggestions = suggestions(&model.composer.input);
    if !command_suggestions.is_empty() {
        let effects = match input.key {
            Key::Esc => {
                model.composer.input.clear();
                model.selected_command = 0;
                Vec::new()
            }
            Key::Up => {
                model.selected_command = model.selected_command.saturating_sub(1);
                Vec::new()
            }
            Key::Down => {
                if model.selected_command + 1 < command_suggestions.len() {
                    model.selected_command += 1;
                }
                Vec::new()
            }
            Key::Enter => {
                let selected = model
                    .selected_command
                    .min(command_suggestions.len().saturating_sub(1));
                let command = command_suggestions[selected].name.to_owned();
                model.composer.input.clear();
                model.selected_command = 0;
                update(model, Action::Submit(command))
            }
            _ => Vec::new(),
        };
        if matches!(input.key, Key::Esc | Key::Up | Key::Down | Key::Enter) {
            return outcome(effects);
        }
    }

    let effects = match input.key {
        Key::Enter => {
            let value = std::mem::take(&mut model.composer.input);
            model.selected_command = 0;
            update(model, Action::Submit(value))
        }
        Key::Backspace => {
            model.composer.input.pop();
            model.selected_command = 0;
            Vec::new()
        }
        Key::Char(ch) if !input.control => {
            model.composer.input.push(ch);
            model.selected_command = 0;
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
    if model.overlay == Overlay::ApiKeyInput && model.pending_model_credential.is_none() {
        model.credential_composer.push_str(value);
        return;
    }
    match model.interaction {
        Interaction::InputPending(_) => model.input_composer.input.push_str(value),
        Interaction::None if matches!(model.turn, TurnState::Idle) => {
            model.composer.input.push_str(value);
            model.selected_command = 0;
        }
        _ => {}
    }
}

fn dispatch_effects<B: Backend, S: ThreadSpawner>(
    backend: Arc<B>,
    high_sender: mpsc::Sender<HighPriorityEvent>,
    runtime_sender: mpsc::Sender<RuntimeStreamEvent>,
    model: &mut Model,
    effects: Vec<Effect>,
    spawner: S,
) {
    let mut pending = VecDeque::from(effects);
    while let Some(effect) = pending.pop_front() {
        let immediate = match effect {
            Effect::StartTurn {
                prompt,
                operation_token,
            } => start_stream_worker_with_spawner(
                backend.clone(),
                runtime_sender.clone(),
                prompt,
                operation_token,
                spawner.clone(),
            ),
            control => start_control_effect_with_spawner(
                backend.clone(),
                high_sender.clone(),
                control,
                spawner.clone(),
            ),
        };
        if let Some(action) = immediate {
            pending.extend(update(model, action));
        }
    }
}

fn start_stream_worker_with_spawner<B: Backend, S: ThreadSpawner>(
    backend: Arc<B>,
    runtime_sender: mpsc::Sender<RuntimeStreamEvent>,
    prompt: String,
    operation_token: OperationToken,
    spawner: S,
) -> Option<Action> {
    if let Err(error) = backend.begin_stream(operation_token.as_u64()) {
        let completion = RuntimeStreamEvent::Completed {
            operation_token,
            result: Err(error.clone()),
        };
        return match runtime_sender.try_send(completion) {
            Ok(()) => None,
            Err(_) => Some(Action::TurnStreamCompleted {
                operation_token,
                result: Err(error),
            }),
        };
    }
    spawner.spawn(Box::new(move || {
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
    }));
    None
}

fn start_control_effect_with_spawner<B: Backend, S: ThreadSpawner>(
    backend: Arc<B>,
    sender: mpsc::Sender<HighPriorityEvent>,
    effect: Effect,
    spawner: S,
) -> Option<Action> {
    let operation_token = match &effect {
        Effect::ResolveApproval {
            operation_token, ..
        }
        | Effect::ResolveInput {
            operation_token, ..
        }
        | Effect::Interrupt {
            operation_token, ..
        }
        | Effect::LoadRuntimeList { operation_token }
        | Effect::SwitchRuntime {
            operation_token, ..
        }
        | Effect::LoadSessionList {
            operation_token, ..
        }
        | Effect::ResumeSession {
            operation_token, ..
        }
        | Effect::LoadModelList { operation_token }
        | Effect::SwitchModel {
            operation_token, ..
        }
        | Effect::SaveModelCredential {
            operation_token, ..
        }
        | Effect::LoadPermissions { operation_token }
        | Effect::SwitchPermissions {
            operation_token, ..
        }
        | Effect::StartTurn {
            operation_token, ..
        } => operation_token.as_u64(),
    };
    if let Err(error) = backend.begin_control(operation_token) {
        let action = control_failure_action(effect, error);
        return match sender.try_send(HighPriorityEvent::Control(Box::new(action))) {
            Ok(()) => None,
            Err(error) => match error.into_inner() {
                HighPriorityEvent::Control(action) => Some(*action),
                _ => unreachable!("only a control completion is sent here"),
            },
        };
    }
    spawner.spawn(Box::new(move || {
        let action = match effect {
            Effect::ResolveApproval {
                turn_id,
                approval_id,
                operation_token,
                accepted,
            } => {
                let call_id = approval_id.clone();
                let result = guarded_backend_call(|| {
                    backend.resolve_approval(operation_token.as_u64(), call_id, accepted)
                });
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
                let result = guarded_backend_call(|| {
                    backend.resolve_input(operation_token.as_u64(), call_id, value)
                });
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
                let result =
                    guarded_backend_call(|| backend.interrupt(operation_token.as_u64(), call_turn));
                Action::InterruptCompleted {
                    turn_id,
                    operation_token,
                    result,
                }
            }
            Effect::LoadRuntimeList { operation_token } => {
                let result =
                    guarded_backend_call(|| backend.runtime_list(operation_token.as_u64()));
                Action::RuntimeListLoaded {
                    operation_token,
                    result,
                }
            }
            Effect::SwitchRuntime {
                runtime,
                operation_token,
            } => {
                let result = guarded_backend_call(|| {
                    backend.switch_runtime(operation_token.as_u64(), runtime)
                });
                Action::RuntimeSwitched {
                    operation_token,
                    result,
                }
            }
            Effect::LoadSessionList {
                operation_token,
                query,
            } => Action::SessionListLoaded {
                operation_token,
                result: guarded_backend_call(|| {
                    backend.session_list(operation_token.as_u64(), Some(query))
                }),
            },
            Effect::ResumeSession {
                operation_token,
                session_id,
            } => Action::SessionResumed {
                operation_token,
                result: guarded_backend_call(|| {
                    backend.resume_session(operation_token.as_u64(), session_id)
                }),
            },
            Effect::LoadModelList { operation_token } => Action::ModelListLoaded {
                operation_token,
                result: guarded_backend_call(|| backend.model_list(operation_token.as_u64())),
            },
            Effect::SwitchModel {
                operation_token,
                model_id,
                reasoning_level,
            } => Action::ModelSwitched {
                operation_token,
                result: guarded_backend_call(|| {
                    backend.switch_model(operation_token.as_u64(), model_id, reasoning_level)
                }),
            },
            Effect::SaveModelCredential {
                operation_token,
                model_id,
                api_key,
            } => Action::ModelCredentialSaved {
                operation_token,
                result: guarded_backend_call(|| {
                    backend.save_model_credential(
                        operation_token.as_u64(),
                        model_id,
                        api_key.expose(),
                    )
                }),
            },
            Effect::LoadPermissions { operation_token } => Action::PermissionsLoaded {
                operation_token,
                result: guarded_backend_call(|| backend.permissions(operation_token.as_u64())),
            },
            Effect::SwitchPermissions {
                operation_token,
                profile,
                full_access_confirmed,
            } => Action::PermissionSwitched {
                operation_token,
                result: guarded_backend_call(|| {
                    backend.switch_permissions(
                        operation_token.as_u64(),
                        profile,
                        full_access_confirmed,
                    )
                }),
            },
            Effect::StartTurn { .. } => return,
        };
        let _ = sender.blocking_send(HighPriorityEvent::Control(Box::new(action)));
    }));
    None
}

fn control_failure_action(effect: Effect, error: BackendError) -> Action {
    match effect {
        Effect::ResolveApproval {
            turn_id,
            approval_id,
            operation_token,
            ..
        } => Action::ApprovalResolutionCompleted {
            turn_id,
            approval_id,
            operation_token,
            result: Err(error),
        },
        Effect::ResolveInput {
            turn_id,
            input_id,
            operation_token,
            ..
        } => Action::InputResolutionCompleted {
            turn_id,
            input_id,
            operation_token,
            result: Err(error),
        },
        Effect::Interrupt {
            turn_id,
            operation_token,
        } => Action::InterruptCompleted {
            turn_id,
            operation_token,
            result: Err(error),
        },
        Effect::LoadRuntimeList { operation_token } => Action::RuntimeListLoaded {
            operation_token,
            result: Err(error),
        },
        Effect::SwitchRuntime {
            operation_token, ..
        } => Action::RuntimeSwitched {
            operation_token,
            result: Err(error),
        },
        Effect::LoadSessionList {
            operation_token, ..
        } => Action::SessionListLoaded {
            operation_token,
            result: Err(error),
        },
        Effect::ResumeSession {
            operation_token, ..
        } => Action::SessionResumed {
            operation_token,
            result: Err(error),
        },
        Effect::LoadModelList { operation_token } => Action::ModelListLoaded {
            operation_token,
            result: Err(error),
        },
        Effect::SwitchModel {
            operation_token, ..
        } => Action::ModelSwitched {
            operation_token,
            result: Err(error),
        },
        Effect::SaveModelCredential {
            operation_token, ..
        } => Action::ModelCredentialSaved {
            operation_token,
            result: Err(error),
        },
        Effect::LoadPermissions { operation_token } => Action::PermissionsLoaded {
            operation_token,
            result: Err(error),
        },
        Effect::SwitchPermissions {
            operation_token, ..
        } => Action::PermissionSwitched {
            operation_token,
            result: Err(error),
        },
        Effect::StartTurn { .. } => unreachable!("stream effects use the stream runner"),
    }
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
        let mut next_tick = Instant::now() + UI_TICK_INTERVAL;
        while !thread_stop.load(Ordering::Acquire) {
            let now = Instant::now();
            let poll_interval =
                TERMINAL_POLL_INTERVAL.min(next_tick.saturating_duration_since(now));
            match source.poll(poll_interval) {
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
            let now = Instant::now();
            if now >= next_tick {
                match sender.try_send(InputEvent::Tick(now)) {
                    Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                }
                next_tick = now + UI_TICK_INTERVAL;
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
        future::Future,
        io,
        path::PathBuf,
        pin::Pin,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    use crossterm::event::Event;
    use tokio::sync::mpsc;

    use super::{
        AppConfig, AppError, Driver, HighPriorityEvent, InputEvent, Key, KeyInput,
        PREINIT_MAX_EVENTS, PREINIT_MAX_TEXT_BYTES, PreInitBuffer, Renderer, RuntimeStreamEvent,
        TerminalEventSource, ThreadSpawner, run_with_driver_and_renderer_config_spawner,
        start_control_effect_with_spawner, start_initialization_worker,
        start_stream_worker_with_spawner, terminal_input_task_with_source,
    };
    use crate::{
        action::Effect,
        backend::{
            Backend, BackendError, BackendErrorKind, EventEmitter, ModelCandidate, RuntimeList,
            RuntimeStatus,
        },
        model::{
            ApprovalRequest, ConnectionState, InputRequest, Interaction, Model, OperationToken,
            Overlay, PendingInterrupt, PendingRuntimeSwitch, TurnState,
        },
        update::update,
    };

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
    fn terminal_reader_emits_bounded_droppable_ticks() {
        let polls = Arc::new(AtomicUsize::new(0));
        let (sender, mut receiver) = mpsc::channel(1);
        let task = terminal_input_task_with_source(
            sender,
            PollingSource {
                polls: polls.clone(),
            },
        );

        let deadline = Instant::now() + Duration::from_secs(1);
        let tick = loop {
            if let Ok(input) = receiver.try_recv() {
                break input;
            }
            assert!(Instant::now() < deadline, "reader did not emit a UI tick");
            std::thread::yield_now();
        };

        assert!(matches!(tick, InputEvent::Tick(_)));
        assert!(polls.load(Ordering::Relaxed) > 1);
        drop(task);
    }

    #[test]
    fn activity_ticks_advance_only_active_turns_and_reset_when_idle() {
        let mut model = Model::new(
            PathBuf::from("workspace"),
            RuntimeStatus::new("codex", "Codex", true),
        );
        let started = Instant::now();

        let _ = super::handle_input(&mut model, InputEvent::Tick(started));
        assert_eq!(model.activity_frame(), 0);
        assert_eq!(model.activity_elapsed(), Duration::ZERO);

        model.turn = TurnState::Starting {
            operation_token: OperationToken::new(1),
        };
        let _ = super::handle_input(&mut model, InputEvent::Tick(started));
        let _ = super::handle_input(
            &mut model,
            InputEvent::Tick(started + Duration::from_millis(2_250)),
        );
        assert_eq!(model.activity_frame(), 2);
        assert_eq!(model.activity_elapsed(), Duration::from_millis(2_250));

        model.turn = TurnState::Idle;
        let _ = super::handle_input(
            &mut model,
            InputEvent::Tick(started + Duration::from_secs(3)),
        );
        assert_eq!(model.activity_frame(), 0);
        assert_eq!(model.activity_elapsed(), Duration::ZERO);
    }

    #[test]
    fn slash_menu_navigates_executes_and_closes_without_submitting_unknown_text() {
        let mut model = Model::new(
            PathBuf::from("workspace"),
            RuntimeStatus::new("codex", "Codex", true),
        );
        model.connection = ConnectionState::Connected;

        let _ = super::handle_input(&mut model, InputEvent::Key(KeyInput::plain(Key::Char('/'))));
        let _ = super::handle_input(&mut model, InputEvent::Key(KeyInput::plain(Key::Down)));
        let selected =
            super::handle_input(&mut model, InputEvent::Key(KeyInput::plain(Key::Enter)));

        assert!(model.composer.input.is_empty());
        assert_eq!(model.selected_command, 0);
        assert_eq!(model.overlay, Overlay::RuntimeList);
        assert!(matches!(
            selected.effects.as_slice(),
            [Effect::LoadRuntimeList { .. }]
        ));

        model.overlay = Overlay::None;
        let _ = super::handle_input(&mut model, InputEvent::Key(KeyInput::plain(Key::Char('/'))));
        let closed = super::handle_input(&mut model, InputEvent::Key(KeyInput::plain(Key::Esc)));
        assert!(closed.effects.is_empty());
        assert!(model.composer.input.is_empty());
    }

    #[test]
    fn model_with_reasoning_levels_opens_level_selector_and_switches_selected_level() {
        let mut model = Model::new(
            PathBuf::from("workspace"),
            RuntimeStatus::new("codex", "Codex", true),
        );
        model.connection = ConnectionState::Connected;
        model.overlay = Overlay::ModelList;
        model.model_candidates = vec![ModelCandidate {
            id: "gpt-5.6-sol".into(),
            display_name: "GPT-5.6 Sol".into(),
            is_default: true,
            available: true,
            provider_id: None,
            provider_display_name: None,
            configured: true,
            requires_api_key: false,
            supported_reasoning_levels: vec!["medium".into(), "high".into()],
            default_reasoning_level: Some("medium".into()),
        }];
        model.model_id = Some("gpt-5.6-sol".into());
        model.model_level = Some("medium".into());

        let opened = super::handle_input(&mut model, InputEvent::Key(KeyInput::plain(Key::Enter)));
        assert!(opened.effects.is_empty());
        assert_eq!(model.overlay, Overlay::ModelLevelList);

        let _ = super::handle_input(&mut model, InputEvent::Key(KeyInput::plain(Key::Down)));
        let switched =
            super::handle_input(&mut model, InputEvent::Key(KeyInput::plain(Key::Enter)));
        assert!(matches!(
            switched.effects.as_slice(),
            [Effect::SwitchModel {
                model_id,
                reasoning_level: Some(level),
                ..
            }] if model_id == "gpt-5.6-sol" && level == "high"
        ));
    }

    #[test]
    fn unconfigured_provider_opens_masked_api_key_flow() {
        let mut model = Model::new(
            PathBuf::from("workspace"),
            RuntimeStatus::new("codewhale", "Pinvou Agent", true),
        );
        model.connection = ConnectionState::Connected;
        model.overlay = Overlay::ModelList;
        model.model_candidates = vec![ModelCandidate {
            id: "deepseek-default".into(),
            display_name: "DeepSeek V4 Pro".into(),
            is_default: false,
            available: true,
            provider_id: Some("deepseek".into()),
            provider_display_name: Some("DeepSeek".into()),
            configured: false,
            requires_api_key: true,
            supported_reasoning_levels: Vec::new(),
            default_reasoning_level: None,
        }];

        let opened = super::handle_input(&mut model, InputEvent::Key(KeyInput::plain(Key::Enter)));
        assert!(opened.effects.is_empty());
        assert_eq!(model.overlay, Overlay::ApiKeyInput);
        let _ = super::handle_input(
            &mut model,
            InputEvent::Paste("sk-secret-must-not-appear".into()),
        );
        let submitted =
            super::handle_input(&mut model, InputEvent::Key(KeyInput::plain(Key::Enter)));
        assert!(matches!(
            submitted.effects.as_slice(),
            [Effect::SaveModelCredential { model_id, .. }] if model_id == "deepseek-default"
        ));
        assert!(!format!("{:?}", submitted.effects).contains("sk-secret-must-not-appear"));
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

    #[test]
    fn preinit_buffer_ignores_ui_ticks_without_reporting_input_loss() {
        let mut buffer = PreInitBuffer::default();
        let mut model = Model::new(
            PathBuf::from("workspace"),
            RuntimeStatus::new("codex", "Codex", true),
        );

        buffer.push(InputEvent::Tick(Instant::now()), &mut model);

        assert!(buffer.events.is_empty());
        assert_eq!(buffer.dropped, 0);
        assert!(model.status_message.is_none());
    }

    #[derive(Clone, Default)]
    struct CountingThreadSpawner(Arc<AtomicUsize>);

    impl ThreadSpawner for CountingThreadSpawner {
        fn spawn(&self, _job: Box<dyn FnOnce() + Send>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[derive(Clone, Default)]
    struct LeaseRejectingBackend {
        method_calls: Arc<AtomicUsize>,
    }

    impl LeaseRejectingBackend {
        fn rejected() -> BackendError {
            BackendError::new(BackendErrorKind::ControllerUnavailable, "lease rejected")
        }

        fn called(&self) -> Result<(), BackendError> {
            self.method_calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    impl Backend for LeaseRejectingBackend {
        fn workspace(&self) -> Result<PathBuf, BackendError> {
            self.called()?;
            Ok(PathBuf::from("workspace"))
        }

        fn begin_stream(&self, _operation_token: u64) -> Result<(), BackendError> {
            Err(Self::rejected())
        }

        fn begin_control(&self, _operation_token: u64) -> Result<(), BackendError> {
            Err(Self::rejected())
        }

        fn runtime_list(&self, _operation_token: u64) -> Result<RuntimeList, BackendError> {
            self.called()?;
            Ok(RuntimeList::new(None, Vec::new()))
        }

        fn stream_turn(
            &self,
            _operation_token: u64,
            _prompt: String,
            _emit: EventEmitter,
        ) -> Result<(), BackendError> {
            self.called()
        }

        fn detach_stream(&self, _operation_token: u64) -> Result<(), BackendError> {
            Ok(())
        }

        fn detach_controls(&self) -> Result<(), BackendError> {
            Ok(())
        }

        fn resolve_approval(
            &self,
            _operation_token: u64,
            _approval_id: String,
            _accepted: bool,
        ) -> Result<(), BackendError> {
            self.called()
        }

        fn resolve_input(
            &self,
            _operation_token: u64,
            _input_id: String,
            _value: String,
        ) -> Result<(), BackendError> {
            self.called()
        }

        fn interrupt(&self, _operation_token: u64, _turn_id: String) -> Result<(), BackendError> {
            self.called()
        }

        fn switch_runtime(
            &self,
            _operation_token: u64,
            _runtime: String,
        ) -> Result<RuntimeStatus, BackendError> {
            self.called()?;
            Ok(RuntimeStatus::new("unused", "unused", true))
        }
    }

    #[test]
    fn rejected_begin_never_spawns_or_calls_stream_backend() {
        let backend = Arc::new(LeaseRejectingBackend::default());
        let spawner = CountingThreadSpawner::default();
        let (sender, mut receiver) = mpsc::channel(1);
        let token = OperationToken::new(7);
        start_stream_worker_with_spawner(
            backend.clone(),
            sender,
            "prompt".into(),
            token,
            spawner.clone(),
        );
        assert_eq!(spawner.0.load(Ordering::Relaxed), 0);
        assert_eq!(backend.method_calls.load(Ordering::Relaxed), 0);
        let RuntimeStreamEvent::Completed {
            operation_token,
            result,
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected stream completion")
        };
        assert!(matches!(
            &result,
            Err(error) if operation_token == token && error.safe_message() == "lease rejected"
        ));
        let mut model = Model::new(
            PathBuf::from("workspace"),
            RuntimeStatus::new("codex", "Codex", true),
        );
        model.turn = TurnState::Starting {
            operation_token: token,
        };
        update(
            &mut model,
            crate::action::Action::TurnStreamCompleted {
                operation_token,
                result,
            },
        );
        assert_eq!(model.turn, TurnState::Idle);
        assert!(
            model
                .last_backend_error
                .as_ref()
                .is_some_and(|error| { error.safe_message() == "lease rejected" })
        );
    }

    #[test]
    fn rejected_begin_never_spawns_or_calls_any_control_backend() {
        let effects = [
            Effect::ResolveApproval {
                turn_id: "turn".into(),
                approval_id: "approval".into(),
                operation_token: OperationToken::new(1),
                accepted: true,
            },
            Effect::ResolveInput {
                turn_id: "turn".into(),
                input_id: "input".into(),
                operation_token: OperationToken::new(2),
                value: "value".into(),
            },
            Effect::Interrupt {
                turn_id: "turn".into(),
                operation_token: OperationToken::new(3),
            },
            Effect::LoadRuntimeList {
                operation_token: OperationToken::new(4),
            },
            Effect::SwitchRuntime {
                runtime: "claude".into(),
                operation_token: OperationToken::new(5),
            },
        ];
        for effect in effects {
            let backend = Arc::new(LeaseRejectingBackend::default());
            let spawner = CountingThreadSpawner::default();
            let (sender, mut receiver) = mpsc::channel(1);
            let mut model = model_with_pending_effect(&effect);
            start_control_effect_with_spawner(
                backend.clone(),
                sender,
                effect.clone(),
                spawner.clone(),
            );
            assert_eq!(spawner.0.load(Ordering::Relaxed), 0);
            assert_eq!(backend.method_calls.load(Ordering::Relaxed), 0);
            let HighPriorityEvent::Control(action) = receiver.try_recv().unwrap() else {
                panic!("expected control completion")
            };
            assert!(action_result_is_rejected(&action));
            update(&mut model, *action);
            assert!(
                model
                    .last_backend_error
                    .as_ref()
                    .is_some_and(|error| { error.safe_message() == "lease rejected" })
            );
            assert!(effect_is_retryable_after_failure(&effect, &model));
        }
    }

    #[test]
    fn rejected_initialization_begin_never_spawns_or_calls_backend() {
        let backend = Arc::new(LeaseRejectingBackend::default());
        let spawner = CountingThreadSpawner::default();
        let (sender, mut receiver) = mpsc::channel(1);
        start_initialization_worker(backend.clone(), sender, spawner.clone());
        assert_eq!(spawner.0.load(Ordering::Relaxed), 0);
        assert_eq!(backend.method_calls.load(Ordering::Relaxed), 0);
        assert!(matches!(
            receiver.try_recv(),
            Ok(HighPriorityEvent::Initialized(Err(error)))
                if error.safe_message() == "lease rejected"
        ));

        let spawner = CountingThreadSpawner::default();
        let (full_sender, _full_receiver) = mpsc::channel(1);
        full_sender
            .try_send(HighPriorityEvent::Input(Ok(None)))
            .unwrap();
        assert!(matches!(
            start_initialization_worker(backend, full_sender, spawner.clone()),
            Some(HighPriorityEvent::Initialized(Err(error)))
                if error.safe_message() == "lease rejected"
        ));
        assert_eq!(spawner.0.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn rejected_begin_returns_typed_completion_when_bounded_channel_is_full() {
        let backend = Arc::new(LeaseRejectingBackend::default());
        let spawner = CountingThreadSpawner::default();
        let (runtime_sender, _runtime_receiver) = mpsc::channel(1);
        runtime_sender
            .try_send(RuntimeStreamEvent::Completed {
                operation_token: OperationToken::new(99),
                result: Ok(()),
            })
            .unwrap();
        assert!(matches!(
            start_stream_worker_with_spawner(
                backend.clone(),
                runtime_sender,
                "prompt".into(),
                OperationToken::new(1),
                spawner.clone(),
            ),
            Some(crate::action::Action::TurnStreamCompleted {
                result: Err(error), ..
            }) if error.safe_message() == "lease rejected"
        ));

        let (control_sender, _control_receiver) = mpsc::channel(1);
        control_sender
            .try_send(HighPriorityEvent::Input(Ok(None)))
            .unwrap();
        assert!(matches!(
            start_control_effect_with_spawner(
                backend,
                control_sender,
                Effect::LoadRuntimeList {
                    operation_token: OperationToken::new(2),
                },
                spawner.clone(),
            ),
            Some(crate::action::Action::RuntimeListLoaded {
                result: Err(error), ..
            }) if error.safe_message() == "lease rejected"
        ));
        assert_eq!(spawner.0.load(Ordering::Relaxed), 0);
    }

    struct PendingDriver;

    impl Driver for PendingDriver {
        fn next_event(
            &mut self,
        ) -> Pin<Box<dyn Future<Output = Result<Option<InputEvent>, AppError>> + Send + '_>>
        {
            Box::pin(std::future::pending())
        }
    }

    #[derive(Clone, Default)]
    struct ConnectionRecorder(Arc<Mutex<Vec<ConnectionState>>>);

    impl Renderer for ConnectionRecorder {
        fn draw(&mut self, model: &Model) -> Result<(), AppError> {
            self.0.lock().unwrap().push(model.connection.clone());
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rejected_initialization_begin_marks_model_failed_without_spawning() {
        let backend = Arc::new(LeaseRejectingBackend::default());
        let spawner = CountingThreadSpawner::default();
        let renderer = ConnectionRecorder::default();
        let connections = renderer.0.clone();
        let error = run_with_driver_and_renderer_config_spawner(
            backend.clone(),
            PendingDriver,
            renderer,
            AppConfig::default(),
            spawner.clone(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            AppError::Backend(ref error) if error.safe_message() == "lease rejected"
        ));
        assert_eq!(spawner.0.load(Ordering::Relaxed), 0);
        assert_eq!(backend.method_calls.load(Ordering::Relaxed), 0);
        assert!(matches!(
            connections.lock().unwrap().last(),
            Some(ConnectionState::Failed(error)) if error.safe_message() == "lease rejected"
        ));
    }

    fn action_result_is_rejected(action: &crate::action::Action) -> bool {
        match action {
            crate::action::Action::ApprovalResolutionCompleted { result, .. }
            | crate::action::Action::InputResolutionCompleted { result, .. }
            | crate::action::Action::InterruptCompleted { result, .. } => result
                .as_ref()
                .is_err_and(|error| error.safe_message() == "lease rejected"),
            crate::action::Action::RuntimeListLoaded { result, .. } => result
                .as_ref()
                .is_err_and(|error| error.safe_message() == "lease rejected"),
            crate::action::Action::RuntimeSwitched { result, .. } => result
                .as_ref()
                .is_err_and(|error| error.safe_message() == "lease rejected"),
            _ => false,
        }
    }

    fn model_with_pending_effect(effect: &Effect) -> Model {
        let mut model = Model::new(
            PathBuf::from("workspace"),
            RuntimeStatus::new("codex", "Codex", true),
        );
        match effect {
            Effect::ResolveApproval {
                turn_id,
                approval_id,
                operation_token,
                ..
            } => {
                let request = ApprovalRequest {
                    turn_id: turn_id.clone(),
                    approval_id: approval_id.clone(),
                    operation_token: *operation_token,
                    tool: "shell".into(),
                    summary: "test".into(),
                    options: Vec::new(),
                };
                model.turn = TurnState::Streaming {
                    operation_token: OperationToken::new(99),
                    turn_id: turn_id.clone(),
                };
                model.interaction = Interaction::ApprovalResolving {
                    request,
                    decision: crate::action::ApprovalDecision::AllowOnce,
                };
            }
            Effect::ResolveInput {
                turn_id,
                input_id,
                operation_token,
                value,
            } => {
                let request = InputRequest {
                    turn_id: turn_id.clone(),
                    input_id: input_id.clone(),
                    operation_token: *operation_token,
                    prompt: "test".into(),
                };
                model.turn = TurnState::Streaming {
                    operation_token: OperationToken::new(99),
                    turn_id: turn_id.clone(),
                };
                model.interaction = Interaction::InputResolving {
                    request,
                    value: value.clone(),
                };
            }
            Effect::Interrupt {
                turn_id,
                operation_token,
            } => {
                model.pending_interrupt = Some(PendingInterrupt {
                    turn_id: turn_id.clone(),
                    operation_token: *operation_token,
                });
            }
            Effect::LoadRuntimeList { operation_token } => {
                model.overlay = Overlay::RuntimeList;
                model.pending_runtime_list = Some(*operation_token);
            }
            Effect::SwitchRuntime {
                runtime,
                operation_token,
            } => {
                model.pending_runtime_switch = Some(PendingRuntimeSwitch {
                    target: runtime.clone(),
                    operation_token: *operation_token,
                });
            }
            Effect::StartTurn { .. } => unreachable!(),
            _ => unreachable!("effect is not part of this focused fixture"),
        }
        model
    }

    fn effect_is_retryable_after_failure(effect: &Effect, model: &Model) -> bool {
        match effect {
            Effect::ResolveApproval { .. } => {
                matches!(model.interaction, Interaction::ApprovalPending(_))
            }
            Effect::ResolveInput { .. } => {
                matches!(model.interaction, Interaction::InputPending(_))
            }
            Effect::Interrupt { .. } => model.pending_interrupt.is_none(),
            Effect::LoadRuntimeList { .. } => model.pending_runtime_list.is_none(),
            Effect::SwitchRuntime { .. } => model.pending_runtime_switch.is_none(),
            Effect::StartTurn { .. } => false,
            _ => unreachable!("effect is not part of this focused fixture"),
        }
    }
}
