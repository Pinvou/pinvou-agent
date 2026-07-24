//! Application event fan-out boundary.
//!
//! Features publish events through this platform port without knowing which
//! optional transports consume them. The composition root wires the concrete
//! remote-control transport while Tauri UI emission remains at each call site.

use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, Manager};

type EventForwarder = dyn Fn(&str, Value) + Send + Sync + 'static;

#[derive(Clone)]
pub struct AppEventBus {
    forwarder: Arc<EventForwarder>,
}

impl AppEventBus {
    pub fn new(forwarder: impl Fn(&str, Value) + Send + Sync + 'static) -> Self {
        Self {
            forwarder: Arc::new(forwarder),
        }
    }

    pub fn forward(&self, event: &str, payload: Value) {
        (self.forwarder)(event, payload);
    }
}

/// Forward an event to optional non-UI transports. Missing state is allowed in
/// headless tests and during partial application startup.
pub fn forward_app_event(app: &AppHandle, event: &str, payload: Value) {
    if let Some(events) = app.try_state::<AppEventBus>() {
        events.forward(event, payload);
    }
}
