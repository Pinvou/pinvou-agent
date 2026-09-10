//! Stub for the `code` family (GUI-parity project). The implementation
//! replaces this file wholesale; see the family specification.

use crate::support::declare_stub_family;

declare_stub_family!(
    CodeCommand,
    "code",
    "agents|login|logout|providers|sessions|workspace|checkpoints|run|permissions|respond"
);
