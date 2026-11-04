//! Desktop background lifecycle. OS event wiring lives in the adapter.

mod platform;
pub(crate) use platform::{before_restart, install, on_exit, on_window_event, refresh_language};
