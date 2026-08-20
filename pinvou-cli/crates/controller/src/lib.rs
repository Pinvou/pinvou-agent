//! Controller daemon skeleton and its narrow local IPC boundary.

mod daemon;
mod error;
mod instance_lock;
mod local_ipc;
mod logging;
mod paths;
mod session;
#[cfg(windows)]
mod windows_security;

pub use daemon::{DetachedLaunch, run_from_env};
pub use error::ControllerError;
pub use instance_lock::InstanceLock;
pub use local_ipc::{LocalIpcListener, LocalIpcPolicy};
pub use logging::RollingLog;
pub use paths::{ControllerPaths, HostPlatform, LocalEndpoint};
pub use session::ControllerSession;

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_identity_is_stable() {
        assert_eq!(CRATE_NAME, "pinvou-controller");
    }
}
