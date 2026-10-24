//! Scheduled-message limits shared with product clients. Runtime services stay internal.

pub(crate) mod executor;
pub(crate) mod tasks;

mod message;

pub use message::SCHEDULED_MESSAGE_MAX_CHARS;
