use std::{
    collections::VecDeque,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use pinvou_protocol::{RuntimeEventEnvelope, RuntimeEventKind};
use pinvou_tui::{
    app::{
        AppConfig, AppError, Driver, InputEvent, Key, KeyInput, KeyKind, PREINIT_MAX_EVENTS,
        PREINIT_MAX_TEXT_BYTES, Renderer, run_with_driver, run_with_driver_and_renderer,
        run_with_driver_and_renderer_config,
    },
    backend::{Backend, BackendError, BackendErrorKind, EventEmitter, RuntimeList, RuntimeStatus},
    model::{Interaction, Overlay, TranscriptEntry, TurnState},
};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControlKind {
    Approval,
    Input,
    RuntimeList,
    Switch,
    Interrupt,
}

#[derive(Debug, Default)]
struct Calls {
    prompts: Vec<String>,
    approvals: Vec<(String, bool)>,
    inputs: Vec<(String, String)>,
    interrupts: Vec<String>,
    switches: Vec<String>,
    stream_gate: usize,
    stream_started: usize,
    approval_emitted: bool,
    input_emitted: bool,
    runtime_list_calls: usize,
    failure_returned: bool,
    stream_completed: usize,
    detaches: Vec<u64>,
    control_detaches: usize,
    control_started: Vec<ControlKind>,
    workspace_started: bool,
    initial_runtime_list_started: bool,
}

#[derive(Clone, Copy)]
enum StreamPlan {
    ApprovalThenHold,
    ApprovalThenInput,
    Hold,
    Flood,
}

#[derive(Clone)]
struct FakeBackend {
    calls: Arc<(Mutex<Calls>, Condvar)>,
    fail_next_stream: Arc<Mutex<Option<BackendError>>>,
    panic_next_stream: Arc<Mutex<bool>>,
    plan: StreamPlan,
    fail_runtime_reload: bool,
    fail_initial_runtime_list: bool,
    block_workspace: bool,
    block_initial_runtime_list: bool,
    blocked_control: Option<ControlKind>,
    panic_control: Option<ControlKind>,
    detach_stream_error: bool,
    detach_controls_error: bool,
    detach_stream_delay: Option<Duration>,
    detach_controls_delay: Option<Duration>,
    workspace_delay: Option<Duration>,
}

impl FakeBackend {
    fn new() -> Self {
        Self {
            calls: Arc::new((Mutex::new(Calls::default()), Condvar::new())),
            fail_next_stream: Arc::new(Mutex::new(None)),
            panic_next_stream: Arc::new(Mutex::new(false)),
            plan: StreamPlan::ApprovalThenHold,
            fail_runtime_reload: false,
            fail_initial_runtime_list: false,
            block_workspace: false,
            block_initial_runtime_list: false,
            blocked_control: None,
            panic_control: None,
            detach_stream_error: false,
            detach_controls_error: false,
            detach_stream_delay: None,
            detach_controls_delay: None,
            workspace_delay: None,
        }
    }

    fn with_plan(plan: StreamPlan) -> Self {
        Self {
            plan,
            ..Self::new()
        }
    }

    fn with_failed_runtime_reload() -> Self {
        Self {
            fail_runtime_reload: true,
            ..Self::new()
        }
    }

    fn with_failed_initialization() -> Self {
        Self {
            fail_initial_runtime_list: true,
            ..Self::new()
        }
    }

    fn with_blocked_initialization(block_workspace: bool, block_runtime_list: bool) -> Self {
        Self {
            block_workspace,
            block_initial_runtime_list: block_runtime_list,
            ..Self::new()
        }
    }

    fn with_blocked_workspace_and_cleanup_error() -> Self {
        Self {
            block_workspace: true,
            detach_controls_error: true,
            ..Self::new()
        }
    }

    fn with_workspace_delay(delay: Duration) -> Self {
        Self {
            workspace_delay: Some(delay),
            ..Self::new()
        }
    }

    fn with_blocked_control(plan: StreamPlan, control: ControlKind) -> Self {
        Self {
            plan,
            blocked_control: Some(control),
            ..Self::new()
        }
    }

    fn with_cleanup_behavior(
        plan: StreamPlan,
        stream_error: bool,
        controls_error: bool,
        stream_delay: Option<Duration>,
        controls_delay: Option<Duration>,
    ) -> Self {
        Self {
            plan,
            detach_stream_error: stream_error,
            detach_controls_error: controls_error,
            detach_stream_delay: stream_delay,
            detach_controls_delay: controls_delay,
            ..Self::new()
        }
    }

    fn with_panicking_control(control: ControlKind) -> Self {
        Self {
            panic_control: Some(control),
            ..Self::new()
        }
    }

    fn block_control(&self, control: ControlKind) -> Result<(), BackendError> {
        if self.panic_control == Some(control) {
            self.calls.0.lock().unwrap().control_started.push(control);
            panic!("scripted control panic");
        }
        if self.blocked_control != Some(control) {
            return Ok(());
        }
        let (calls, wake) = &*self.calls;
        let mut calls = calls.lock().unwrap();
        calls.control_started.push(control);
        wake.notify_all();
        while calls.control_detaches == 0 {
            calls = wake.wait(calls).unwrap();
        }
        Err(BackendError::new(
            BackendErrorKind::Cancelled,
            "control detached",
        ))
    }

    fn calls(&self) -> std::sync::MutexGuard<'_, Calls> {
        self.calls.0.lock().unwrap()
    }

    fn release_stream(&self) {
        let (calls, wake) = &*self.calls;
        calls.lock().unwrap().stream_gate += 1;
        wake.notify_all();
    }

    fn fail_once(&self, message: &str) {
        *self.fail_next_stream.lock().unwrap() = Some(BackendError::new(
            BackendErrorKind::ControllerUnavailable,
            message,
        ));
    }

    fn panic_once(&self) {
        *self.panic_next_stream.lock().unwrap() = true;
    }
}

impl Backend for FakeBackend {
    fn workspace(&self) -> Result<PathBuf, BackendError> {
        if let Some(delay) = self.workspace_delay {
            std::thread::sleep(delay);
        }
        let (calls, wake) = &*self.calls;
        let mut calls = calls.lock().unwrap();
        calls.workspace_started = true;
        wake.notify_all();
        if self.block_workspace {
            while calls.control_detaches == 0 {
                calls = wake.wait(calls).unwrap();
            }
            return Err(BackendError::new(
                BackendErrorKind::Cancelled,
                "workspace initialization detached",
            ));
        }
        Ok(PathBuf::from("workspace"))
    }

    fn runtime_list(&self, _operation_token: u64) -> Result<RuntimeList, BackendError> {
        let call = {
            let (calls, wake) = &*self.calls;
            let mut calls = calls.lock().unwrap();
            calls.runtime_list_calls += 1;
            if calls.runtime_list_calls == 1 {
                calls.initial_runtime_list_started = true;
                wake.notify_all();
            }
            calls.runtime_list_calls
        };
        if self.block_initial_runtime_list && call == 1 {
            let (calls, wake) = &*self.calls;
            let mut calls = calls.lock().unwrap();
            while calls.control_detaches == 0 {
                calls = wake.wait(calls).unwrap();
            }
            return Err(BackendError::new(
                BackendErrorKind::Cancelled,
                "runtime initialization detached",
            ));
        }
        if self.fail_runtime_reload && call > 1 {
            return Err(BackendError::new(
                BackendErrorKind::ControllerUnavailable,
                "runtime catalog unavailable",
            ));
        }
        if self.fail_initial_runtime_list && call == 1 {
            return Err(BackendError::new(
                BackendErrorKind::ControllerUnavailable,
                "initial probe failed",
            ));
        }
        if call > 1 {
            self.block_control(ControlKind::RuntimeList)?;
        }
        Ok(RuntimeList::new(
            Some("codex".into()),
            vec![
                RuntimeStatus::new("codex", "OpenAI Codex", true),
                RuntimeStatus::new("claude", "Claude Code", true),
                RuntimeStatus::new("kimi", "Kimi Code", false),
            ],
        ))
    }

    fn stream_turn(
        &self,
        _operation_token: u64,
        prompt: String,
        mut emit: EventEmitter,
    ) -> Result<(), BackendError> {
        if let Some(error) = self.fail_next_stream.lock().unwrap().take() {
            self.calls.0.lock().unwrap().failure_returned = true;
            return Err(error);
        }
        let call = {
            let mut calls = self.calls.0.lock().unwrap();
            calls.prompts.push(prompt);
            calls.stream_started += 1;
            calls.prompts.len()
        };
        if std::mem::take(&mut *self.panic_next_stream.lock().unwrap()) {
            panic!("scripted stream panic");
        }
        emit(event(RuntimeEventKind::TurnStarted, json!({}), 1, call))?;
        if matches!(self.plan, StreamPlan::Flood) {
            for seq in 2..=5_001 {
                emit(event(
                    RuntimeEventKind::TextDelta,
                    json!({"role":"assistant", "content":"x"}),
                    seq,
                    call,
                ))?;
            }
            let (calls, wake) = &*self.calls;
            let mut state = calls.lock().unwrap();
            while state.stream_gate == 0 {
                state = wake.wait(state).unwrap();
            }
        } else if call == 1 && !matches!(self.plan, StreamPlan::Hold) {
            emit(event(
                RuntimeEventKind::TextDelta,
                json!({"role":"assistant", "content":"first "}),
                2,
                call,
            ))?;
            emit(event(
                RuntimeEventKind::ApprovalRequested,
                json!({
                    "approval_id":"approval-1", "tool":"shell",
                    "summary":"run tests", "options":["allow", "deny"]
                }),
                3,
                call,
            ))?;
            let (calls, wake) = &*self.calls;
            let mut state = calls.lock().unwrap();
            state.approval_emitted = true;
            wake.notify_all();
            while state.approvals.is_empty() && state.detaches.is_empty() {
                state = wake.wait(state).unwrap();
            }
            if !state.detaches.is_empty() {
                state.stream_completed += 1;
                return Err(BackendError::new(
                    BackendErrorKind::Cancelled,
                    "stream detached",
                ));
            }
            drop(state);
            if matches!(self.plan, StreamPlan::ApprovalThenInput) {
                emit(event(
                    RuntimeEventKind::ApprovalResolved,
                    json!({"approval_id":"approval-1", "outcome":"denied"}),
                    4,
                    call,
                ))?;
                emit(event(
                    RuntimeEventKind::InputRequested,
                    json!({"input_id":"input-1", "prompt":"value?"}),
                    5,
                    call,
                ))?;
                calls.lock().unwrap().stream_completed += 1;
                let mut state = calls.lock().unwrap();
                state.input_emitted = true;
                wake.notify_all();
                while state.inputs.is_empty() && state.detaches.is_empty() {
                    state = wake.wait(state).unwrap();
                }
            } else {
                emit(event(
                    RuntimeEventKind::TextDelta,
                    json!({"role":"assistant", "content":"done"}),
                    4,
                    call,
                ))?;
                emit(event(
                    RuntimeEventKind::TurnEnded,
                    json!({"end_reason":"completed"}),
                    5,
                    call,
                ))?;
                calls.lock().unwrap().stream_completed += 1;
            }
        } else {
            let (calls, wake) = &*self.calls;
            let mut state = calls.lock().unwrap();
            while state.stream_gate == 0 {
                state = wake.wait(state).unwrap();
            }
        }
        Ok(())
    }

    fn resolve_approval(
        &self,
        _operation_token: u64,
        id: String,
        accepted: bool,
    ) -> Result<(), BackendError> {
        self.block_control(ControlKind::Approval)?;
        let (calls, wake) = &*self.calls;
        calls.lock().unwrap().approvals.push((id, accepted));
        wake.notify_all();
        Ok(())
    }

    fn resolve_input(
        &self,
        _operation_token: u64,
        id: String,
        value: String,
    ) -> Result<(), BackendError> {
        self.block_control(ControlKind::Input)?;
        let (calls, wake) = &*self.calls;
        calls.lock().unwrap().inputs.push((id, value));
        wake.notify_all();
        Ok(())
    }

    fn interrupt(&self, _operation_token: u64, turn_id: String) -> Result<(), BackendError> {
        self.calls.0.lock().unwrap().interrupts.push(turn_id);
        self.block_control(ControlKind::Interrupt)?;
        Ok(())
    }

    fn detach_stream(&self, operation_token: u64) -> Result<(), BackendError> {
        if let Some(delay) = self.detach_stream_delay {
            std::thread::sleep(delay);
        }
        let (calls, wake) = &*self.calls;
        let mut calls = calls.lock().unwrap();
        calls.detaches.push(operation_token);
        calls.stream_gate += 1;
        wake.notify_all();
        if self.detach_stream_error {
            Err(BackendError::new(
                BackendErrorKind::Operation,
                "stream cleanup failed",
            ))
        } else {
            Ok(())
        }
    }

    fn detach_controls(&self) -> Result<(), BackendError> {
        if let Some(delay) = self.detach_controls_delay {
            std::thread::sleep(delay);
        }
        let (calls, wake) = &*self.calls;
        calls.lock().unwrap().control_detaches += 1;
        wake.notify_all();
        if self.detach_controls_error {
            Err(BackendError::new(
                BackendErrorKind::Operation,
                "control cleanup failed",
            ))
        } else {
            Ok(())
        }
    }

    fn switch_runtime(
        &self,
        _operation_token: u64,
        runtime: String,
    ) -> Result<RuntimeStatus, BackendError> {
        self.block_control(ControlKind::Switch)?;
        self.calls.0.lock().unwrap().switches.push(runtime.clone());
        Ok(RuntimeStatus::new(
            runtime.clone(),
            format!("{runtime} runtime"),
            true,
        ))
    }
}

enum Step {
    Input(InputEvent),
    Wait(Arc<dyn Fn() -> bool + Send + Sync>),
    Release(FakeBackend),
    Delay(Duration),
}

struct ScriptDriver(VecDeque<Step>);

impl ScriptDriver {
    fn new(steps: Vec<Step>) -> Self {
        Self(steps.into())
    }
}

impl Driver for ScriptDriver {
    fn next_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<InputEvent>, AppError>> + Send + '_>> {
        Box::pin(async move {
            loop {
                match self.0.pop_front() {
                    Some(Step::Input(event)) => return Ok(Some(event)),
                    Some(Step::Wait(predicate)) => {
                        while !predicate() {
                            tokio::task::yield_now().await;
                        }
                    }
                    Some(Step::Release(backend)) => backend.release_stream(),
                    Some(Step::Delay(duration)) => tokio::time::sleep(duration).await,
                    None => return Ok(None),
                }
            }
        })
    }
}

fn key(key: Key) -> Step {
    Step::Input(InputEvent::Key(KeyInput::plain(key)))
}

fn text(value: &str) -> Vec<Step> {
    value.chars().map(|ch| key(Key::Char(ch))).collect()
}

fn wait_for<F>(predicate: F) -> Step
where
    F: Fn() -> bool + Send + Sync + 'static,
{
    Step::Wait(Arc::new(predicate))
}

#[tokio::test(flavor = "current_thread")]
async fn scripted_chat_approval_second_turn_and_detach_form_one_loop() {
    let backend = FakeBackend::new();
    let approval_backend = backend.clone();
    let first_done_backend = backend.clone();
    let second_started_backend = backend.clone();
    let mut steps = text("hello");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || approval_backend.calls().approval_emitted));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(key(Key::Char('1')));
    steps.push(wait_for(move || {
        first_done_backend.calls().stream_completed == 1
    }));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.extend(text("again"));
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || {
        second_started_backend.calls().prompts.len() == 2
    }));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));
    steps.push(Step::Release(backend.clone()));

    let result = run_script(&backend, steps).await;

    assert!(result.detached);
    assert_eq!(backend.calls().prompts, ["hello", "again"]);
    assert_eq!(backend.calls().approvals, [("approval-1".into(), true)]);
    assert!(backend.calls().interrupts.is_empty());
    assert_eq!(result.model.transcript.assistant_text(), "first done");
    assert!(
        result
            .model
            .transcript
            .entries()
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::User(value) if value == "again"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn approval_deny_and_input_editor_use_control_specific_composer() {
    let backend = FakeBackend::with_plan(StreamPlan::ApprovalThenInput);
    let approval = backend.clone();
    let input = backend.clone();
    let mut steps = text("start");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || approval.calls().approval_emitted));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(key(Key::Char('3')));
    steps.push(wait_for(move || input.calls().input_emitted));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(Step::Input(InputEvent::Paste("你\n好".into())));
    steps.push(key(Key::Backspace));
    steps.push(key(Key::Char('界')));
    steps.push(key(Key::Enter));
    steps.push(wait_for({
        let backend = backend.clone();
        move || !backend.calls().inputs.is_empty()
    }));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));
    let result = run_script(&backend, steps).await;
    assert!(result.detached);
    assert_eq!(backend.calls().approvals, [("approval-1".into(), false)]);
    assert_eq!(
        backend.calls().inputs,
        [("input-1".into(), "你\n界".into())]
    );
    assert!(result.model.composer.input.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn escape_interrupts_stream_but_ctrl_c_only_detaches() {
    let backend = FakeBackend::with_plan(StreamPlan::Hold);
    let started = backend.clone();
    let interrupted = backend.clone();
    let mut steps = text("start");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || started.calls().stream_started == 1));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(key(Key::Esc));
    steps.push(wait_for(move || !interrupted.calls().interrupts.is_empty()));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));
    steps.push(Step::Release(backend.clone()));
    let result = run_script(&backend, steps).await;
    assert!(result.detached);
    assert_eq!(backend.calls().interrupts, ["turn-1"]);
    assert_eq!(backend.calls().detaches.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_overlay_loads_candidates_navigates_and_switches() {
    let backend = FakeBackend::new();
    let loaded = backend.clone();
    let switched = backend.clone();
    let driver = ScriptDriver::new(vec![
        Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('r')))),
        wait_for(move || loaded.calls().runtime_list_calls >= 2),
        Step::Delay(Duration::from_millis(10)),
        key(Key::Down),
        key(Key::Enter),
        wait_for(move || !switched.calls().switches.is_empty()),
        Step::Delay(Duration::from_millis(10)),
        key(Key::Esc),
        Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))),
    ]);
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        run_with_driver(Arc::new(backend.clone()), driver),
    )
    .await
    .expect("runtime script timed out")
    .unwrap();
    assert_eq!(backend.calls().switches, ["claude"]);
    assert_eq!(result.model.runtime.id, "claude");
    assert_eq!(result.model.runtime_candidates.len(), 3);
    assert_eq!(result.model.overlay, Overlay::None);
}

#[tokio::test(flavor = "current_thread")]
async fn backend_failure_is_visible_and_idle_editor_recovers() {
    let backend = FakeBackend::new();
    backend.fail_once("controller offline");
    let mut steps = text("hello");
    steps.push(key(Key::Enter));
    steps.push(wait_for({
        let backend = backend.clone();
        move || backend.calls().failure_returned
    }));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.extend(text("next"));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));
    let result = run_script(&backend, steps).await;
    assert!(matches!(result.model.turn, TurnState::Idle));
    assert_eq!(result.model.composer.input, "next");
    assert_eq!(
        result.model.last_backend_error.unwrap().safe_message(),
        "controller offline"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn editor_slash_resize_release_repeat_and_clean_exit_are_deterministic() {
    let backend = FakeBackend::new();
    let initialized = backend.clone();
    let steps = vec![
        Step::Input(InputEvent::Resize(120, 40)),
        wait_for(move || initialized.calls().runtime_list_calls == 1),
        Step::Delay(Duration::from_millis(10)),
        Step::Input(InputEvent::Key(KeyInput {
            key: Key::Char('x'),
            control: false,
            kind: KeyKind::Release,
        })),
        Step::Input(InputEvent::Key(KeyInput {
            key: Key::Char('你'),
            control: false,
            kind: KeyKind::Repeat,
        })),
        key(Key::Backspace),
        Step::Input(InputEvent::Paste("/wat".into())),
        key(Key::Enter),
        Step::Input(InputEvent::Paste("/exit".into())),
        key(Key::Enter),
    ];
    let result = run_script(&backend, steps).await;
    assert!(!result.detached);
    assert!(result.model.should_quit);
    assert_eq!(result.model.terminal_size, Some((120, 40)));
    assert!(result.model.transcript.entries().is_empty());
    assert_eq!(
        result.model.status_message.as_deref(),
        Some("unknown command: /wat")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn eof_detaches_without_submitting_the_unicode_composer() {
    let backend = FakeBackend::new();
    let result = run_script(
        &backend,
        vec![
            Step::Input(InputEvent::Paste("你好".into())),
            Step::Input(InputEvent::Eof),
        ],
    )
    .await;
    assert!(result.detached);
    assert_eq!(result.model.composer.input, "你好");
    assert!(backend.calls().prompts.is_empty());
}

#[derive(Clone, Default)]
struct RecordingRenderer(Arc<Mutex<Vec<pinvou_tui::model::Model>>>);

impl Renderer for RecordingRenderer {
    fn draw(&mut self, model: &pinvou_tui::model::Model) -> Result<(), AppError> {
        self.0.lock().unwrap().push(model.clone());
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn renderer_observes_stream_approval_resolution_and_terminal_progression() {
    let backend = FakeBackend::new();
    let approval = backend.clone();
    let completed = backend.clone();
    let renderer = RecordingRenderer::default();
    let snapshots = renderer.0.clone();
    let mut steps = text("render");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || approval.calls().approval_emitted));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(key(Key::Char('1')));
    steps.push(wait_for(move || completed.calls().stream_completed == 1));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(Step::Input(InputEvent::Eof));
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        run_with_driver_and_renderer(Arc::new(backend), ScriptDriver::new(steps), renderer),
    )
    .await
    .expect("rendering loop timed out")
    .unwrap();
    assert!(result.detached);
    let snapshots = snapshots.lock().unwrap();
    assert!(
        snapshots.first().unwrap().connection == pinvou_tui::model::ConnectionState::Connecting
    );
    assert!(
        snapshots
            .iter()
            .any(|model| { model.connection == pinvou_tui::model::ConnectionState::Connected })
    );
    assert!(
        snapshots
            .iter()
            .any(|model| model.transcript.assistant_text() == "first ")
    );
    assert!(
        snapshots
            .iter()
            .any(|model| matches!(model.interaction, Interaction::ApprovalPending(_)))
    );
    assert!(
        snapshots
            .iter()
            .any(|model| matches!(model.interaction, Interaction::ApprovalResolving { .. }))
    );
    assert!(
        snapshots
            .iter()
            .any(|model| model.transcript.assistant_text() == "first done"
                && model.turn == TurnState::Idle)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn initialization_failure_renders_failed_connection_before_typed_error() {
    let backend = FakeBackend::with_failed_initialization();
    let initialization = backend.clone();
    let renderer = RecordingRenderer::default();
    let snapshots = renderer.0.clone();
    let error = run_with_driver_and_renderer(
        Arc::new(backend),
        ScriptDriver::new(vec![
            wait_for(move || initialization.calls().runtime_list_calls == 1),
            Step::Delay(Duration::from_millis(10)),
        ]),
        renderer,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, AppError::Backend(ref error) if error.safe_message() == "initial probe failed")
    );
    let snapshots = snapshots.lock().unwrap();
    assert!(snapshots.len() >= 2);
    assert_eq!(
        snapshots.first().unwrap().connection,
        pinvou_tui::model::ConnectionState::Connecting
    );
    assert!(matches!(
        snapshots.last().unwrap().connection,
        pinvou_tui::model::ConnectionState::Failed(_)
    ));
}

#[test]
fn blocked_initialization_ctrl_c_and_eof_detach_before_runtime_drop() {
    for (block_workspace, input) in [
        (true, InputEvent::Key(KeyInput::ctrl(Key::Char('c')))),
        (false, InputEvent::Key(KeyInput::ctrl(Key::Char('c')))),
        (true, InputEvent::Eof),
    ] {
        let started_at = std::time::Instant::now();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let backend = FakeBackend::with_blocked_initialization(block_workspace, !block_workspace);
        let initialization = backend.clone();
        let wait = wait_for(move || {
            let calls = initialization.calls();
            if block_workspace {
                calls.workspace_started
            } else {
                calls.initial_runtime_list_started
            }
        });
        let result = runtime
            .block_on(run_with_driver(
                Arc::new(backend.clone()),
                ScriptDriver::new(vec![wait, Step::Input(input)]),
            ))
            .unwrap();
        drop(runtime);
        assert!(result.detached);
        let calls = backend.calls();
        assert_eq!(calls.control_detaches, 1);
        assert!(calls.detaches.is_empty());
        assert!(calls.interrupts.is_empty());
        assert!(started_at.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn initialization_deadline_fails_visibly_and_detaches_controls() {
    let started_at = std::time::Instant::now();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let backend = FakeBackend::with_blocked_initialization(true, false);
    let initialization = backend.clone();
    let renderer = RecordingRenderer::default();
    let snapshots = renderer.0.clone();
    let error = runtime
        .block_on(run_with_driver_and_renderer_config(
            Arc::new(backend.clone()),
            ScriptDriver::new(vec![
                wait_for(move || initialization.calls().workspace_started),
                Step::Delay(Duration::from_secs(1)),
            ]),
            renderer,
            AppConfig {
                initialization_timeout: Duration::from_millis(50),
            },
        ))
        .unwrap_err();
    drop(runtime);
    assert!(matches!(
        error,
        AppError::Backend(ref error) if error.kind() == BackendErrorKind::Timeout
    ));
    assert_eq!(backend.calls().control_detaches, 1);
    let snapshots = snapshots.lock().unwrap();
    assert_eq!(
        snapshots.first().unwrap().connection,
        pinvou_tui::model::ConnectionState::Connecting
    );
    assert!(matches!(
        snapshots.last().unwrap().connection,
        pinvou_tui::model::ConnectionState::Failed(_)
    ));
    assert!(started_at.elapsed() < Duration::from_secs(2));
}

#[test]
fn initialization_detach_preserves_cleanup_warnings() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let backend = FakeBackend::with_blocked_workspace_and_cleanup_error();
    let initialization = backend.clone();
    let result = runtime
        .block_on(run_with_driver(
            Arc::new(backend),
            ScriptDriver::new(vec![
                wait_for(move || initialization.calls().workspace_started),
                Step::Input(InputEvent::Eof),
            ]),
        ))
        .unwrap();
    drop(runtime);
    assert!(result.detached);
    assert!(
        result
            .cleanup_warnings
            .iter()
            .any(|warning| warning.contains("detach_controls: control cleanup failed"))
    );
}

#[test]
fn preinit_input_flood_is_bounded_and_ctrl_c_remains_urgent() {
    let started_at = std::time::Instant::now();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let backend = FakeBackend::with_blocked_initialization(true, false);
    let initialization = backend.clone();
    let renderer = RecordingRenderer::default();
    let snapshots = renderer.0.clone();
    let mut steps = vec![wait_for(move || initialization.calls().workspace_started)];
    steps.push(Step::Input(InputEvent::Paste(
        "界".repeat(PREINIT_MAX_TEXT_BYTES / 3 + 32),
    )));
    steps.extend((0..PREINIT_MAX_EVENTS + 32).map(|_| {
        Step::Input(InputEvent::Key(KeyInput {
            key: Key::Char('x'),
            control: false,
            kind: KeyKind::Repeat,
        }))
    }));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));

    let result = runtime
        .block_on(run_with_driver_and_renderer(
            Arc::new(backend.clone()),
            ScriptDriver::new(steps),
            renderer,
        ))
        .unwrap();
    drop(runtime);

    assert!(result.detached);
    assert!(result.model.composer.input.len() <= PREINIT_MAX_TEXT_BYTES);
    assert!(
        result
            .model
            .status_message
            .as_deref()
            .is_some_and(|status| status.contains("input buffer full"))
    );
    assert_eq!(backend.calls().control_detaches, 1);
    assert!(backend.calls().interrupts.is_empty());
    assert!(snapshots.lock().unwrap().iter().any(|model| {
        model.connection == pinvou_tui::model::ConnectionState::Connecting
            && !model.composer.input.is_empty()
    }));
    assert!(snapshots.lock().unwrap().iter().any(|model| {
        model.connection == pinvou_tui::model::ConnectionState::Connecting
            && model
                .status_message
                .as_deref()
                .is_some_and(|status| status.contains("input buffer full"))
    }));
    assert!(started_at.elapsed() < Duration::from_secs(2));
}

#[tokio::test(flavor = "current_thread")]
async fn preinit_preview_and_bounded_replay_preserve_editor_order() {
    let backend = FakeBackend::with_workspace_delay(Duration::from_millis(50));
    let renderer = RecordingRenderer::default();
    let snapshots = renderer.0.clone();
    let result = run_with_driver_and_renderer(
        Arc::new(backend.clone()),
        ScriptDriver::new(vec![
            Step::Input(InputEvent::Paste("/wat".into())),
            key(Key::Enter),
            Step::Input(InputEvent::Paste("/exit".into())),
            key(Key::Enter),
            Step::Delay(Duration::from_millis(100)),
        ]),
        renderer,
    )
    .await
    .unwrap();
    assert!(result.model.should_quit);
    assert!(!result.detached);
    assert!(backend.calls().prompts.is_empty());
    let snapshots = snapshots.lock().unwrap();
    assert!(snapshots.iter().any(|model| {
        model.connection == pinvou_tui::model::ConnectionState::Connecting
            && model.composer.input == "/wat"
    }));
    assert_eq!(
        result.model.status_message.as_deref(),
        Some("unknown command: /wat")
    );
}

struct FailOnFailedRenderer;

impl Renderer for FailOnFailedRenderer {
    fn draw(&mut self, model: &pinvou_tui::model::Model) -> Result<(), AppError> {
        if matches!(
            model.connection,
            pinvou_tui::model::ConnectionState::Failed(_)
        ) {
            Err(AppError::Render("failed-state draw failed".into()))
        } else {
            Ok(())
        }
    }
}

#[test]
fn initialization_timeout_returns_failed_state_render_error() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let backend = FakeBackend::with_blocked_workspace_and_cleanup_error();
    let initialization = backend.clone();
    let error = runtime
        .block_on(run_with_driver_and_renderer_config(
            Arc::new(backend.clone()),
            ScriptDriver::new(vec![
                wait_for(move || initialization.calls().workspace_started),
                Step::Delay(Duration::from_secs(1)),
            ]),
            FailOnFailedRenderer,
            AppConfig {
                initialization_timeout: Duration::from_millis(50),
            },
        ))
        .unwrap_err();
    drop(runtime);
    assert!(matches!(
        error,
        AppError::Cleanup {
            ref cause,
            ref cleanup_warnings,
        } if matches!(cause.as_ref(), AppError::Render(message) if message == "failed-state draw failed")
            && cleanup_warnings
                .iter()
                .any(|warning| warning.contains("control cleanup failed"))
    ));
    assert_eq!(backend.calls().control_detaches, 1);
}

#[test]
fn blocked_stream_ctrl_c_detaches_and_runtime_drops_within_bound() {
    let started_at = std::time::Instant::now();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let backend = FakeBackend::with_plan(StreamPlan::Hold);
    let started = backend.clone();
    let mut steps = text("blocked");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || started.calls().stream_started == 1));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));
    let result = runtime
        .block_on(run_with_driver(
            Arc::new(backend.clone()),
            ScriptDriver::new(steps),
        ))
        .unwrap();
    drop(runtime);
    assert!(result.detached);
    assert!(backend.calls().interrupts.is_empty());
    assert_eq!(backend.calls().detaches.len(), 1);
    assert!(started_at.elapsed() < Duration::from_secs(2));
}

#[test]
fn active_stream_eof_detaches_and_runtime_drops_within_bound() {
    let started_at = std::time::Instant::now();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let backend = FakeBackend::with_plan(StreamPlan::Hold);
    let started = backend.clone();
    let mut steps = text("blocked");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || started.calls().stream_started == 1));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(Step::Input(InputEvent::Eof));
    let result = runtime
        .block_on(run_with_driver(
            Arc::new(backend.clone()),
            ScriptDriver::new(steps),
        ))
        .unwrap();
    drop(runtime);
    assert!(result.detached);
    assert!(backend.calls().interrupts.is_empty());
    assert_eq!(backend.calls().detaches.len(), 1);
    assert!(started_at.elapsed() < Duration::from_secs(2));
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_flood_cannot_starve_interrupt_or_ctrl_c() {
    let backend = FakeBackend::with_plan(StreamPlan::Flood);
    let started = backend.clone();
    let interrupted = backend.clone();
    let mut steps = text("flood");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || started.calls().stream_started == 1));
    steps.push(Step::Delay(Duration::from_millis(5)));
    steps.push(key(Key::Esc));
    steps.push(Step::Input(InputEvent::Key(KeyInput {
        key: Key::Esc,
        control: false,
        kind: KeyKind::Repeat,
    })));
    steps.push(wait_for(move || !interrupted.calls().interrupts.is_empty()));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));
    let started_at = std::time::Instant::now();
    let result = run_script(&backend, steps).await;
    assert!(result.detached);
    assert_eq!(backend.calls().interrupts.len(), 1);
    assert_eq!(backend.calls().detaches.len(), 1);
    assert!(started_at.elapsed() < Duration::from_secs(2));
    assert!(result.model.transcript.assistant_text().len() < 5_000);
}

#[test]
fn approval_wait_ctrl_c_detaches_without_runtime_interrupt() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let backend = FakeBackend::new();
    let approval = backend.clone();
    let mut steps = text("approval wait");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || approval.calls().approval_emitted));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));
    let result = runtime
        .block_on(run_with_driver(
            Arc::new(backend.clone()),
            ScriptDriver::new(steps),
        ))
        .unwrap();
    drop(runtime);
    assert!(result.detached);
    assert!(backend.calls().interrupts.is_empty());
    assert_eq!(backend.calls().detaches.len(), 1);
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while backend.calls().stream_completed == 0 && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert_eq!(backend.calls().stream_completed, 1);
}

#[test]
fn every_blocked_control_effect_detaches_without_delaying_runtime_drop() {
    for control in [
        ControlKind::Approval,
        ControlKind::Input,
        ControlKind::RuntimeList,
        ControlKind::Switch,
        ControlKind::Interrupt,
    ] {
        let plan = match control {
            ControlKind::Input => StreamPlan::ApprovalThenInput,
            ControlKind::Interrupt => StreamPlan::Hold,
            _ => StreamPlan::ApprovalThenHold,
        };
        let backend = FakeBackend::with_blocked_control(plan, control);
        let mut steps = Vec::new();
        match control {
            ControlKind::Approval => {
                steps.extend(text("approval"));
                steps.push(key(Key::Enter));
                steps.push(wait_for({
                    let backend = backend.clone();
                    move || backend.calls().approval_emitted
                }));
                steps.push(Step::Delay(Duration::from_millis(10)));
                steps.push(key(Key::Char('1')));
            }
            ControlKind::Input => {
                steps.extend(text("input"));
                steps.push(key(Key::Enter));
                steps.push(wait_for({
                    let backend = backend.clone();
                    move || backend.calls().approval_emitted
                }));
                steps.push(Step::Delay(Duration::from_millis(10)));
                steps.push(key(Key::Char('1')));
                steps.push(wait_for({
                    let backend = backend.clone();
                    move || backend.calls().input_emitted
                }));
                steps.push(Step::Delay(Duration::from_millis(10)));
                steps.push(Step::Input(InputEvent::Paste("answer".into())));
                steps.push(key(Key::Enter));
            }
            ControlKind::RuntimeList => {
                steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('r')))));
            }
            ControlKind::Switch => {
                steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('r')))));
                steps.push(wait_for({
                    let backend = backend.clone();
                    move || backend.calls().runtime_list_calls >= 2
                }));
                steps.push(Step::Delay(Duration::from_millis(10)));
                steps.push(key(Key::Down));
                steps.push(key(Key::Enter));
            }
            ControlKind::Interrupt => {
                steps.extend(text("interrupt"));
                steps.push(key(Key::Enter));
                steps.push(wait_for({
                    let backend = backend.clone();
                    move || backend.calls().stream_started == 1
                }));
                steps.push(Step::Delay(Duration::from_millis(10)));
                steps.push(key(Key::Esc));
            }
        }
        steps.push(wait_for({
            let backend = backend.clone();
            move || backend.calls().control_started.contains(&control)
        }));
        steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let started = std::time::Instant::now();
        let result = runtime
            .block_on(run_with_driver(
                Arc::new(backend.clone()),
                ScriptDriver::new(steps),
            ))
            .unwrap();
        drop(runtime);
        assert!(result.detached, "{control:?}");
        assert_eq!(backend.calls().control_detaches, 1, "{control:?}");
        assert!(started.elapsed() < Duration::from_secs(2), "{control:?}");
        let expected_interrupts = usize::from(control == ControlKind::Interrupt);
        assert_eq!(
            backend.calls().interrupts.len(),
            expected_interrupts,
            "{control:?}"
        );
    }
}

#[test]
fn cleanup_errors_are_reported_without_preventing_detach() {
    let backend = FakeBackend::with_cleanup_behavior(StreamPlan::Hold, true, true, None, None);
    let started = backend.clone();
    let mut steps = text("cleanup error");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || started.calls().stream_started == 1));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime
        .block_on(run_with_driver(Arc::new(backend), ScriptDriver::new(steps)))
        .unwrap();
    drop(runtime);
    assert!(result.detached);
    assert!(
        result
            .cleanup_warnings
            .iter()
            .any(|warning| warning.contains("detach_stream: stream cleanup failed"))
    );
    assert!(
        result
            .cleanup_warnings
            .iter()
            .any(|warning| warning.contains("detach_controls: control cleanup failed"))
    );
}

#[test]
fn cleanup_timeout_is_reported_and_exit_remains_bounded() {
    let backend = FakeBackend::with_cleanup_behavior(
        StreamPlan::Hold,
        false,
        false,
        Some(Duration::from_millis(800)),
        None,
    );
    let started = backend.clone();
    let mut steps = text("cleanup timeout");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || started.calls().stream_started == 1));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let started_at = std::time::Instant::now();
    let result = runtime
        .block_on(run_with_driver(Arc::new(backend), ScriptDriver::new(steps)))
        .unwrap();
    drop(runtime);
    assert!(started_at.elapsed() < Duration::from_secs(2));
    assert!(
        result
            .cleanup_warnings
            .iter()
            .any(|warning| warning == "detach_stream: cleanup timed out")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn stream_worker_panic_becomes_typed_backend_error() {
    let backend = FakeBackend::new();
    backend.panic_once();
    let started = backend.clone();
    let mut steps = text("panic");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || started.calls().stream_started == 1));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));
    let result = run_script(&backend, steps).await;
    assert!(matches!(result.model.turn, TurnState::Idle));
    assert!(
        result
            .model
            .last_backend_error
            .as_ref()
            .unwrap()
            .safe_message()
            .contains("panicked")
    );
    assert_eq!(
        result.model.last_backend_error.as_ref().unwrap().kind(),
        BackendErrorKind::WorkerPanic
    );
    assert!(matches!(
        result.model.connection,
        pinvou_tui::model::ConnectionState::Failed(ref error)
            if error.kind() == BackendErrorKind::WorkerPanic
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn control_worker_panic_marks_connection_failed_and_blocks_reuse() {
    let backend = FakeBackend::with_panicking_control(ControlKind::Approval);
    let approval = backend.clone();
    let panicked = backend.clone();
    let mut steps = text("control panic");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || approval.calls().approval_emitted));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(key(Key::Char('1')));
    steps.push(wait_for(move || {
        panicked
            .calls()
            .control_started
            .contains(&ControlKind::Approval)
    }));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.extend(text("must not send"));
    steps.push(key(Key::Enter));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));
    let result = run_script(&backend, steps).await;
    assert!(matches!(
        result.model.connection,
        pinvou_tui::model::ConnectionState::Failed(ref error)
            if error.kind() == BackendErrorKind::WorkerPanic
    ));
    assert_eq!(backend.calls().prompts, ["control panic"]);
    assert!(
        result
            .model
            .status_message
            .as_deref()
            .unwrap()
            .contains("reinitialized")
    );
}

#[derive(Clone)]
struct FailOnStreamingRenderer {
    calls: Arc<Mutex<usize>>,
}

impl Renderer for FailOnStreamingRenderer {
    fn draw(&mut self, model: &pinvou_tui::model::Model) -> Result<(), AppError> {
        *self.calls.lock().unwrap() += 1;
        if matches!(model.turn, TurnState::Streaming { .. }) {
            Err(AppError::Render("draw failed".into()))
        } else {
            Ok(())
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn renderer_failure_detaches_active_stream_before_returning_error() {
    let backend = FakeBackend::with_plan(StreamPlan::Hold);
    let started = backend.clone();
    let mut steps = text("render failure");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || started.calls().stream_started == 1));
    steps.push(Step::Delay(Duration::from_millis(10)));
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        run_with_driver_and_renderer(
            Arc::new(backend.clone()),
            ScriptDriver::new(steps),
            FailOnStreamingRenderer {
                calls: Arc::new(Mutex::new(0)),
            },
        ),
    )
    .await
    .expect("renderer failure timed out")
    .unwrap_err();
    assert!(matches!(error, AppError::Render(ref message) if message == "draw failed"));
    assert_eq!(backend.calls().detaches.len(), 1);
    assert!(backend.calls().interrupts.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_catalog_failure_is_typed_visible_and_retryable() {
    let backend = FakeBackend::with_failed_runtime_reload();
    let reloaded = backend.clone();
    let result = run_script(
        &backend,
        vec![
            Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('r')))),
            wait_for(move || reloaded.calls().runtime_list_calls >= 2),
            Step::Delay(Duration::from_millis(10)),
            Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))),
        ],
    )
    .await;
    assert!(result.detached);
    assert_eq!(
        result.model.last_backend_error.unwrap().safe_message(),
        "runtime catalog unavailable"
    );
    assert!(result.model.pending_runtime_list.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_selector_is_blocked_during_an_active_turn() {
    let backend = FakeBackend::with_plan(StreamPlan::Hold);
    let started = backend.clone();
    let mut steps = text("hold");
    steps.push(key(Key::Enter));
    steps.push(wait_for(move || started.calls().stream_started == 1));
    steps.push(Step::Delay(Duration::from_millis(10)));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('r')))));
    steps.push(Step::Input(InputEvent::Key(KeyInput::ctrl(Key::Char('c')))));
    steps.push(Step::Release(backend.clone()));
    let result = run_script(&backend, steps).await;
    assert_eq!(backend.calls().runtime_list_calls, 1);
    assert_ne!(result.model.overlay, Overlay::RuntimeList);
}

async fn run_script(backend: &FakeBackend, steps: Vec<Step>) -> pinvou_tui::app::RunResult {
    match tokio::time::timeout(
        Duration::from_secs(2),
        run_with_driver(Arc::new(backend.clone()), ScriptDriver::new(steps)),
    )
    .await
    {
        Ok(result) => result.unwrap(),
        Err(_) => {
            backend.release_stream();
            panic!(
                "script timed out with backend calls: {:?}",
                *backend.calls()
            );
        }
    }
}

fn event(
    kind: RuntimeEventKind,
    mut payload: Value,
    seq: u64,
    turn: usize,
) -> RuntimeEventEnvelope {
    if kind == RuntimeEventKind::TurnStarted {
        payload["user_input_ref"] = json!(format!("prompt-{turn}"));
    }
    RuntimeEventEnvelope::from_value(json!({
        "protocol_version": 1,
        "schema_version": 1,
        "node_id": "node-local",
        "logical_session_id": "session-1",
        "attachment_id": "attachment-1",
        "work_id": null,
        "collaborative_run_id": null,
        "stream_id": if matches!(kind, RuntimeEventKind::TextDelta) { "main" } else { "control" },
        "turn_id": format!("turn-{turn}"),
        "seq": seq,
        "source_span": null,
        "timestamp": "2026-08-24T00:00:00Z",
        "rate_class": if matches!(kind, RuntimeEventKind::TextDelta) { "R1" } else { "R0" },
        "kind": kind,
        "payload": payload
    }))
    .unwrap()
}
