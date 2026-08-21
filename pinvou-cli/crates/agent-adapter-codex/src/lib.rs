//! Long-lived Codex `app-server` JSON-RPC adapter.

mod process;
mod projector;
mod redact;

pub use process::{CodexAdapter, CodexAdapterConfig};
pub use projector::{ApprovalResponse, CodexEventProjector, PendingControl, ProjectedFrame};
pub use redact::redact_diagnostic;

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const RUNTIME_API_CRATE_NAME: &str = pinvou_runtime_api::CRATE_NAME;
pub const MAX_JSON_LINE_BYTES: usize = 16 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_identity_and_runtime_boundary_are_stable() {
        assert_eq!(CRATE_NAME, "pinvou-agent-adapter-codex");
        assert_eq!(RUNTIME_API_CRATE_NAME, "pinvou-runtime-api");
    }
}
