//! Stub for the `models` + `settings` families (GUI-parity project). The
//! implementation replaces this file wholesale; see the family specification.

use crate::support::declare_stub_family;

declare_stub_family!(
    ModelsCommand,
    "models",
    "list|add|remove|use|show|test|probe-local|get|set"
);
