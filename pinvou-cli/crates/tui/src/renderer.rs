use ratatui::{Terminal, backend::Backend as RatatuiBackend};

use crate::{
    app::{AppError, Renderer},
    model::Model,
    view,
};

pub struct RatatuiRenderer<B: RatatuiBackend + Send + 'static> {
    terminal: Terminal<B>,
}

impl<B: RatatuiBackend + Send + 'static> RatatuiRenderer<B> {
    pub fn new(backend: B) -> Result<Self, AppError> {
        Terminal::new(backend)
            .map(|terminal| Self { terminal })
            .map_err(|error| AppError::Render(error.to_string()))
    }

    pub fn backend(&self) -> &B {
        self.terminal.backend()
    }

    pub fn backend_mut(&mut self) -> &mut B {
        self.terminal.backend_mut()
    }
}

impl<B: RatatuiBackend + Send + 'static> Renderer for RatatuiRenderer<B> {
    fn draw(&mut self, model: &Model) -> Result<(), AppError> {
        self.terminal
            .autoresize()
            .and_then(|_| self.terminal.draw(|frame| view::render(frame, model)))
            .map(|_| ())
            .map_err(|error| AppError::Render(error.to_string()))
    }
}
