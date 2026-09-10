//! Stub for the `scheduled` family (GUI-parity project). The implementation
//! replaces this file wholesale; see the family specification.

use crate::support::declare_stub_family;

declare_stub_family!(
    ScheduledCommand,
    "scheduled",
    "list|show|create|update|delete|pause|resume|pin|unpin|run|runs|runs-all|mark-viewed|chat-prompt"
);
