//! Stub for the `memory` family (GUI-parity project). The implementation
//! replaces this file wholesale; see the family specification.

use crate::support::declare_stub_family;

declare_stub_family!(
    MemoryCommand,
    "memory",
    "overview|profile|list|add|update|delete|archive|pending|organize|organize-history"
);
