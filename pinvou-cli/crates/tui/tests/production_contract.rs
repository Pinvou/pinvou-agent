use std::{
    future::Future,
    io,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
};

use pinvou_tui::{
    app::{AppError, Driver, InputEvent, Key, KeyInput, Renderer},
    backend::{Backend, BackendError, EventEmitter, RuntimeList, RuntimeStatus},
    model::Model,
    renderer::RatatuiRenderer,
    run_with_parts,
    terminal::{TerminalGuard, TerminalOps},
};
use ratatui::backend::TestBackend;

#[test]
fn ratatui_renderer_draws_the_real_view_and_tracks_backend_resize() {
    let mut renderer = RatatuiRenderer::new(TestBackend::new(80, 24)).unwrap();
    let model = Model::new(
        PathBuf::from("workspace"),
        RuntimeStatus::new("codex", "Codex", true),
    );
    renderer.draw(&model).unwrap();
    let rendered = renderer.backend().buffer().content().to_vec();
    let text = rendered
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(text.contains("◆ PINVOU"));

    renderer.backend_mut().resize(100, 30);
    renderer.draw(&model).unwrap();
    assert_eq!(renderer.backend().buffer().area.width, 100);
    assert_eq!(renderer.backend().buffer().area.height, 30);
}

#[derive(Clone, Default)]
struct RecordingOps(Arc<Mutex<Vec<&'static str>>>);

impl TerminalOps for RecordingOps {
    fn enable_raw(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().push("enable_raw");
        Ok(())
    }
    fn enter_alt(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().push("enter_alt");
        Ok(())
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().push("hide_cursor");
        Ok(())
    }
    fn enable_bracketed_paste(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().push("enable_paste");
        Ok(())
    }
    fn disable_bracketed_paste(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().push("disable_paste");
        Ok(())
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().push("show_cursor");
        Ok(())
    }
    fn leave_alt(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().push("leave_alt");
        Ok(())
    }
    fn disable_raw(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().push("disable_raw");
        Ok(())
    }
}

struct ExitDriver;

impl Driver for ExitDriver {
    fn next_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<InputEvent>, AppError>> + Send + '_>> {
        Box::pin(async { Ok(Some(InputEvent::Key(KeyInput::ctrl(Key::Char('c'))))) })
    }
}

struct FailingRenderer;

impl Renderer for FailingRenderer {
    fn draw(&mut self, _model: &Model) -> Result<(), AppError> {
        Err(AppError::Render("flush failed".into()))
    }
}

struct IdleBackend;

impl Backend for IdleBackend {
    fn workspace(&self) -> Result<PathBuf, BackendError> {
        Ok(PathBuf::from("workspace"))
    }
    fn runtime_list(&self, _operation_token: u64) -> Result<RuntimeList, BackendError> {
        Ok(RuntimeList::new(
            Some("codex".into()),
            vec![RuntimeStatus::new("codex", "Codex", true)],
        ))
    }
    fn stream_turn(
        &self,
        _operation_token: u64,
        _prompt: String,
        _emit: EventEmitter,
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
        _operation_token: u64,
        _approval_id: String,
        _accepted: bool,
    ) -> Result<(), BackendError> {
        Ok(())
    }
    fn resolve_input(
        &self,
        _operation_token: u64,
        _input_id: String,
        _value: String,
    ) -> Result<(), BackendError> {
        Ok(())
    }
    fn interrupt(&self, _operation_token: u64, _turn_id: String) -> Result<(), BackendError> {
        Ok(())
    }
    fn switch_runtime(
        &self,
        _operation_token: u64,
        runtime: String,
    ) -> Result<RuntimeStatus, BackendError> {
        Ok(RuntimeStatus::new(runtime.clone(), runtime, true))
    }
}

#[test]
fn render_failure_restores_terminal_in_reverse_order_before_returning() {
    let ops = RecordingOps::default();
    let guard = TerminalGuard::enter(ops.clone()).unwrap();
    let error =
        run_with_parts(Arc::new(IdleBackend), ExitDriver, FailingRenderer, guard).unwrap_err();
    assert!(error.to_string().contains("flush failed"));
    assert_eq!(
        *ops.0.lock().unwrap(),
        [
            "enable_raw",
            "enter_alt",
            "hide_cursor",
            "enable_paste",
            "disable_paste",
            "show_cursor",
            "leave_alt",
            "disable_raw"
        ]
    );
}
