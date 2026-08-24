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
    app::{AppError, Driver, InputEvent, Key, KeyInput, KeyKind, run_with_driver},
    backend::{Backend, BackendError, BackendErrorKind, EventEmitter, RuntimeList, RuntimeStatus},
    model::{Overlay, TranscriptEntry, TurnState},
};
use serde_json::{Value, json};

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
}

#[derive(Clone, Copy)]
enum StreamPlan {
    ApprovalThenHold,
    ApprovalThenInput,
    Hold,
}

#[derive(Clone)]
struct FakeBackend {
    calls: Arc<(Mutex<Calls>, Condvar)>,
    fail_next_stream: Arc<Mutex<Option<BackendError>>>,
    plan: StreamPlan,
    fail_runtime_reload: bool,
}

impl FakeBackend {
    fn new() -> Self {
        Self {
            calls: Arc::new((Mutex::new(Calls::default()), Condvar::new())),
            fail_next_stream: Arc::new(Mutex::new(None)),
            plan: StreamPlan::ApprovalThenHold,
            fail_runtime_reload: false,
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
}

impl Backend for FakeBackend {
    fn workspace(&self) -> Result<PathBuf, BackendError> {
        Ok(PathBuf::from("workspace"))
    }

    fn runtime_list(&self) -> Result<RuntimeList, BackendError> {
        let call = {
            let mut calls = self.calls.0.lock().unwrap();
            calls.runtime_list_calls += 1;
            calls.runtime_list_calls
        };
        if self.fail_runtime_reload && call > 1 {
            return Err(BackendError::new(
                BackendErrorKind::ControllerUnavailable,
                "runtime catalog unavailable",
            ));
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

    fn stream_turn(&self, prompt: String, mut emit: EventEmitter) -> Result<(), BackendError> {
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
        emit(event(RuntimeEventKind::TurnStarted, json!({}), 1, call))?;
        if call == 1 && !matches!(self.plan, StreamPlan::Hold) {
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
            while state.approvals.is_empty() {
                state = wake.wait(state).unwrap();
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
                while state.inputs.is_empty() {
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

    fn resolve_approval(&self, id: String, accepted: bool) -> Result<(), BackendError> {
        let (calls, wake) = &*self.calls;
        calls.lock().unwrap().approvals.push((id, accepted));
        wake.notify_all();
        Ok(())
    }

    fn resolve_input(&self, id: String, value: String) -> Result<(), BackendError> {
        let (calls, wake) = &*self.calls;
        calls.lock().unwrap().inputs.push((id, value));
        wake.notify_all();
        Ok(())
    }

    fn interrupt(&self, turn_id: String) -> Result<(), BackendError> {
        self.calls.0.lock().unwrap().interrupts.push(turn_id);
        Ok(())
    }

    fn switch_runtime(&self, runtime: String) -> Result<RuntimeStatus, BackendError> {
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
    let steps = vec![
        Step::Input(InputEvent::Resize(120, 40)),
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
