//! Stub for the `voice` family (GUI-parity project). The implementation
//! replaces this file wholesale; see the family specification.

use crate::support::declare_stub_family;

declare_stub_family!(
    VoiceCommand,
    "voice",
    "transcribe|postprocess|asr-status|asr-install"
);
