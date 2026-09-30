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

/// The model-facing availability announcement, rendered into the session
/// system prompt by the bridge while the master switch is on (Work **and**
/// Code sessions — the tool is registered for every native Engine session).
///
/// Without it the tool is invisible in practice: the model is never told the
/// capability exists, so it only finds `computer_use` if the user asks for
/// computer use by name and the model thinks of `tool_search` (observed
/// 2026-09-30: the model confidently claimed the environment had no screen
/// access). The section therefore names the tool directly — which, per the
/// tool-policy admission criterion, is also why the tool ships
/// non-deferred: static text that names a tool must not describe an absent
/// first-turn catalog entry. Consent/grant mechanics live in the tool's own
/// (activated-with) description; this block carries only what the model
/// must know BEFORE the first call.
pub(crate) fn instruction_block() -> &'static str {
    // Deliberately not translated: the session system prompt is the
    // Chinese-language static prompt plus English capability sections (the
    // Browser capabilities section is English too).
    "## Computer use
- This session can see and operate the real desktop through the `computer_use` tool: `screenshot` shows the actual screen, and further actions move the mouse, click, scroll, and type on this computer. The tool is in your tool list; take a `screenshot` first and work in its pixel coordinates.
- Reading the screen (screenshot, cursor position, element tree) needs no approval. The FIRST action that moves the mouse or presses a key fails on purpose while the app shows the user a control-grant dialog: tell the user to approve it, and retry only after they say they did (the grant covers the rest of the session). If control was granted earlier but a new action reports it missing (the session went idle or the app restarted), ask the user to grant again — do not loop retries.
- A pending per-action confirmation (delete / submit / pay / accept-class targets) blocks every input action until the user answers the dialog; wait instead of retrying, and never try to click the dialog itself.
- On macOS, `accessibility_denied` / `screen_recording_denied` mean the user must grant the permission in System Settings → Privacy & Security (the Accessibility pane is labeled \"Device Control and Data Access\" on recent macOS; Screen Recording keeps its name) and then FULLY QUIT AND REOPEN the app — a grant reaches an already-running process only at launch. Say both steps, not just the settings path.
- Stay inside the user's explicit request; do not propose driving the desktop when a normal tool (file, shell, browser) fits the task."
}
