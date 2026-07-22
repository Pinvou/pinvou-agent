//! macOS platform adapter.
//!
//! macOS currently keeps the explicit unsupported behavior for capabilities
//! that have not been implemented yet. Keeping a dedicated adapter makes each
//! future capability an intentional macOS change instead of falling through an
//! unknown-platform branch.

pub use super::unsupported::*;
