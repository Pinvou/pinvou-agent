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
type TransportProbe = dyn Fn() -> bool + Send + Sync + 'static;

#[derive(Clone)]
pub struct AppEventBus {
    forwarder: Arc<EventForwarder>,
    subscriber_probe: Arc<EventSubscriberProbe>,
    transport_probe: Arc<TransportProbe>,
}

impl AppEventBus {
    pub fn new(
        forwarder: impl Fn(&str, Value) + Send + Sync + 'static,
        subscriber_probe: impl Fn(&str) -> bool + Send + Sync + 'static,
        transport_probe: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            forwarder: Arc::new(forwarder),
            subscriber_probe: Arc::new(subscriber_probe),
            transport_probe: Arc::new(transport_probe),
        }
    }

    pub fn forward(&self, event: &str, payload: Value) {
        (self.forwarder)(event, payload);
    }

    pub fn has_active_subscriber(&self, event: &str) -> bool {
        (self.subscriber_probe)(event)
    }

    /// Whether an optional transport is configured at all, independent of the
    /// current subscription handshake. Producers use this to decide whether
    /// journaling (replay coverage) is required while a consumer reconnects.
    pub fn has_active_transport(&self) -> bool {
        (self.transport_probe)()
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

/// Return whether an optional transport exists at all, even if its consumer is
/// momentarily disconnected. Events emitted now still need journaling so the
/// consumer's reconnect replay covers the disconnect window.
pub fn has_active_app_event_transport(app: &AppHandle) -> bool {
    app.try_state::<AppEventBus>()
        .is_some_and(|events| events.has_active_transport())
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
            || true,
        );

        assert!(bus.has_active_subscriber("acp:event"));
        assert!(!bus.has_active_subscriber("chat:delta"));
        assert_eq!(forwards.load(Ordering::Relaxed), 0);

        bus.forward("acp:event", Value::Null);
        assert_eq!(forwards.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn transport_probe_reports_configured_transport_without_subscribers() {
        let bus = AppEventBus::new(|_, _| (), |_event| false, || true);
        assert!(!bus.has_active_subscriber("acp:event"));
        assert!(bus.has_active_transport());
    }
}
