//! Shared protocol boundary for controller, node, and runtime adapters.

mod clock;
mod event;
mod exit_code;
mod frame;

pub use clock::{ClockError, HostMonotonicClock, HostMonotonicTimestamp};
pub use event::{
    EventSchemaError, RateClass, ResourceAccess, ResourceKind, ResourceLifecycle, ResourceRef,
    RuntimeEventEnvelope, RuntimeEventKind, SourceSpan, StreamId,
};
pub use exit_code::{ExitCause, StableExitCode};
pub use frame::{
    FrameError, HelloClient, HelloServer, IpcMessage, IpcMessageKind, MAX_FRAME_LEN, decode_frame,
    decode_length_prefix, encode_frame,
};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_identity_is_stable() {
        assert_eq!(CRATE_NAME, "pinvou-protocol");
    }
}
