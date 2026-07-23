//! Cross-platform tear-off API.
//!
//! Native pointer polling and platform selection live in the platform adapter.

pub use super::platform::detach::{begin_detach_drag, open_detached_window, point_in_rect};
