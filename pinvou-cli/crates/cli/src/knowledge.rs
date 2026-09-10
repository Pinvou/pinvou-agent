//! Stub for the `knowledge` family (GUI-parity project). The implementation
//! replaces this file wholesale; see the family specification.

use crate::support::declare_stub_family;

declare_stub_family!(
    KnowledgeCommand,
    "knowledge",
    "scan|stats|type-counts|collections|documents|index|search|model|mounts|mount|unmount|remote"
);
