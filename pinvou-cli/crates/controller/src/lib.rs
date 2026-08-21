//! Controller daemon skeleton and its narrow local IPC boundary.

mod daemon;
mod error;
mod instance_lock;
mod local_ipc;
mod local_node_client;
mod local_node_supervisor;
mod logging;
mod paths;
mod session;
mod wal;
#[cfg(windows)]
mod windows_security;

#[cfg(debug_assertions)]
pub use daemon::run_controller_once_for_test;
pub use daemon::{DetachedLaunch, run_from_env};
pub use error::ControllerError;
pub use instance_lock::InstanceLock;
pub use local_ipc::{LocalIpcListener, LocalIpcPolicy};
pub use local_node_client::LocalNodeClient;
pub use local_node_supervisor::{
    LocalNodeLauncher, LocalNodeProbe, LocalNodeSpec, LocalNodeSupervisor, NodeProcessStatus,
    ProcessNodeLauncher, ProcessNodeProbe, SupervisedChild,
};
pub use logging::RollingLog;
pub use paths::{ControllerPaths, HostPlatform, LocalEndpoint};
pub use session::ControllerSession;
pub use wal::{BatchAck, ControllerWal, IngestOutcome, WalError};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_identity_is_stable() {
        assert_eq!(CRATE_NAME, "pinvou-controller");
    }
}
