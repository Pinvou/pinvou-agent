//! Application event fan-out boundary.
//!
//! Features publish events through this platform port without knowing which
//! optional transports consume them. The composition root wires the concrete
//! remote-control transport while Tauri UI emission remains at each call site.

use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, Manager};

type EventForwarder = dyn Fn(&str, Value) + Send + Sync + 'static;
type EventSubscriberProbe = dyn Fn(&str) -> bool + Send + Sync + 'static;

#[derive(Clone)]
pub struct AppEventBus {
    forwarder: Arc<EventForwarder>,
    subscriber_probe: Arc<EventSubscriberProbe>,
}

impl AppEventBus {
    pub fn new(
        forwarder: impl Fn(&str, Value) + Send + Sync + 'static,
        subscriber_probe: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            forwarder: Arc::new(forwarder),
            subscriber_probe: Arc::new(subscriber_probe),
        }
    }

    pub fn forward(&self, event: &str, payload: Value) {
        (self.forwarder)(event, payload);
    }

    pub fn has_active_subscriber(&self, event: &str) -> bool {
        (self.subscriber_probe)(event)
    }
}

/// Forward an event to optional non-UI transports. Missing state is allowed in
/// headless tests and during partial application startup.
pub fn forward_app_event(app: &AppHandle, event: &str, payload: Value) {
    if let Some(events) = app.try_state::<AppEventBus>() {
        events.forward(event, payload);
    }
}

/// Return whether an optional transport is currently able and subscribed to
/// consume an event. Producers use this as a cheap projection gate; delivery
/// still revalidates transport state at the forwarding boundary.
pub fn has_active_app_event_subscriber(app: &AppHandle, event: &str) -> bool {
    app.try_state::<AppEventBus>()
        .is_some_and(|events| events.has_active_subscriber(event))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn subscriber_probe_is_independent_from_forwarding() {
        let forwards = Arc::new(AtomicUsize::new(0));
        let forwarded = Arc::clone(&forwards);
        let bus = AppEventBus::new(
            move |_, _| {
                forwarded.fetch_add(1, Ordering::Relaxed);
            },
            |event| event == "acp:event",
        );

        assert!(bus.has_active_subscriber("acp:event"));
        assert!(!bus.has_active_subscriber("chat:delta"));
        assert_eq!(forwards.load(Ordering::Relaxed), 0);

        bus.forward("acp:event", Value::Null);
        assert_eq!(forwards.load(Ordering::Relaxed), 1);
    }
}
