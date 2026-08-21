//! Private stage-1 Node daemon and runtime-host boundary.

mod daemon;
mod error;
mod instance_lock;
mod local_ipc;
mod session;
mod spool;
#[cfg(windows)]
mod windows_security;

pub use daemon::run_from_env;
pub use error::NodeError;
pub use instance_lock::NodeInstanceLock;
pub use local_ipc::NodeTransportPolicy;
pub use session::NodeSession;
pub use spool::{NodeSpool, RawSpoolRecord, SpoolError, SpoolRecovery, TransportRecord};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_identity_is_stable() {
        assert_eq!(CRATE_NAME, "pinvou-node");
    }
}
