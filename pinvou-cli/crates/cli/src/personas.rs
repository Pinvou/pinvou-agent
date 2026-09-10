//! Stub for the `personas` family (GUI-parity project). The implementation
//! replaces this file wholesale; see the family specification.

use crate::support::declare_stub_family;

declare_stub_family!(
    PersonasCommand,
    "personas",
    "list|show|create|update|delete|equip|unequip|active"
);
