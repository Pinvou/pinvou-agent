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
//!   per session, only when `ComputerUseShared::is_enabled()` is true (the model never sees
//!   the schema while the settings switch is off);
//! - `ComputerUseShared` is the global singleton (Arc, shared via `.manage()`); the settings
//!   switch wires to `set_enabled`, grant/revoke/emergency-stop wire to `grant_session` /
//!   `revoke_session` / `stop_all`, and the T3 confirmation command wires to
//!   `mint_confirmation`;
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
pub use self::guard::ComputerUseShared;
pub use self::tool::ComputerUseTool;

// ---- Tool name (types): shared by ToolPolicy dynamic disabling and the command surface ----
pub use self::types::TOOL_NAME;

// ---- Platform capability entry points (platform): consumed by Tauri commands ----
pub(crate) use self::platform::{backend_supported, request_permissions};
