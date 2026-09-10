//! Stub for the `sessions` family (GUI-parity project). The implementation
//! replaces this file wholesale; see the family specification.

use crate::support::declare_stub_family;

declare_stub_family!(
    SessionsCommand,
    "sessions",
    "list|show|rename|pin|unpin|archive|restore|delete|export|timeline|subagents|folder"
);
