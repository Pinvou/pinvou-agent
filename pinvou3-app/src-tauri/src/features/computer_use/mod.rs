//! Computer Use: the model observes and operates the user's desktop through the single
//! `computer_use` tool.
//!
//! Module structure:
//! - `types` — action enums, key chord parsing, errors and backend result structs, the
//!   coordinate space contract
//! - `scaling` — screenshot scaling (long edge ≤1440) and the screenshot↔device↔input
//!   three-layer coordinate mapping
//! - `backend` — the backend trait + one dedicated worker thread per session (xcap/enigo
//!   have thread affinity)
//! - `guard` — consent gating: the settings switch, session grant, stop flag, T3 confirmation
//! - `audit` — append-only JSONL audit (a sanitized, purely informational local log that
//!   never records plaintext)
//! - `tool` — the `ComputerUseTool` ToolSpec implementation
//! - `platform` — the three platform backends and permission onboarding
//!   (`request_permissions`)
//!
//! Integration (composition root: lib.rs assembly / app::commands command surface):
//! - the factory constructs `ComputerUseTool::new(app_handle, session_id, shared.clone())`
//!   per session **unconditionally**; while the settings switch is off the tool stays
//!   out of the model's sight purely through the dynamic disallow list (the ToolPolicy
//!   hook in guard.rs), and the consent guard's `Disabled` rejection is the second
//!   layer — safety while disabled = disallow list + guard, in that order;
//! - `ComputerUseShared` is the global singleton (Arc, shared via `.manage()`); the settings
//!   switch wires to `set_enabled`, grant/revoke/emergency-stop wire to `grant_session` /
//!   `revoke_session` / `stop_all`, and the T3 confirmation command wires to
//!   `mint_confirmation`;
//! - a session grant is bound to the tool instance's lifetime: when the engine pool's
//!   idle reaper reclaims an idle engine, the tool's `Drop` revokes the grant, so a
//!   grant can lapse without any user action — the next input attempt then reads
//!   `GrantRequired` and the user must grant again;
//! - the frontend listens to `computer_use:grant_required` and
//!   `computer_use:confirm_required`.

mod audit;
mod backend;
mod guard;
mod platform;
mod scaling;
mod tool;
mod types;

// ---- Tool and shared state (tool / guard): consumed by the composition root and Tauri commands ----
pub use self::guard::{ComputerUseShared, GrantOutcome};
pub use self::tool::ComputerUseTool;

// ---- Tool name (types): the canonical "computer_use" spelling, re-exported
// ---- for the composition root and command surface ----
pub use self::types::{EVENT_STATE_CHANGED, TOOL_NAME};

// ---- Test-only guard knobs (guard): unit tests across the feature adjust timeouts ----
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use self::guard::set_physical_input_lock_timeout_for_tests;

// ---- Platform capability entry points (platform): consumed by Tauri commands ----
pub(crate) use self::platform::{backend_supported, request_permissions};
