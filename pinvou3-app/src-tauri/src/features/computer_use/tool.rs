//! The `computer_use` tool: a single ToolSpec; the `action` field selects the
//! action (the Anthropic computer_20250124 action set plus the a11y
//! extensions ui_tree / element_at_point).
//!
//! Key invariants:
//! - Model coordinates are always "screenshot space" (the pixels of the most
//!   recently returned PNG, top-left origin).
//! - Every screenshot generates a new ScaleMap stored in session state; a
//!   coordinate-carrying action without a ScaleMap captures one automatically
//!   first.
//! - After an action that changes screen content, wait
//!   [`POST_ACTION_SETTLE_MS`], take a fresh screenshot and attach it, so the
//!   model always sees the latest state; which actions attach one is derived
//!   from [`ComputerUseAction::attaches_screenshot`] (bare pointer moves do
//!   not).
//! - Consent gating ([`ComputerUseShared`]) is checked before every
//!   injection; a target hitting the consequential denylist or a password
//!   field must be confirmed by the user (a single-use token bound to the
//!   action summary). Screening is best-effort category detection: screening
//!   being unavailable does not block execution, and typed content is never
//!   screened.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter as _};

use deepseek_tui::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

use super::audit::{AuditLog, AuditRecord};
use super::backend::{BackendHandle, ComputerUseBackend};
use super::guard::{
    ComputerUseShared, GuardRejection, is_secure_role, keyed_binding_u64, matches_t3_denylist,
    raw_element_binding,
};
use super::platform;
use super::scaling::{self, ScaleMap, ScaledScreenshot};
use super::types::{
    ActionClass, Capabilities, ComputerUseAction, ComputerUseError, DisplayTarget,
    EVENT_CONFIRM_REQUIRED, EVENT_GRANT_REQUIRED, ElementInfo, Key, MouseButton, ScrollDirection,
    TOOL_NAME, UiTreeOptions, parse_key_chord,
};

/// UI settle wait after input/scroll actions execute (the single definition
/// point of the settle constant).
pub const POST_ACTION_SETTLE_MS: u64 = 350;
/// `wait` cap: 30 seconds. wait and hold_key intentionally share one upper
/// bound: the tool schema exposes a single `ms` field for both actions, so
/// keep this and [`MAX_HOLD_KEY_MS`] equal.
pub const MAX_WAIT_MS: u64 = 30_000;
/// `hold_key` cap: 30 seconds (the same shared upper bound as
/// [`MAX_WAIT_MS`] — the schema's single `ms` field serves both actions).
pub const MAX_HOLD_KEY_MS: u64 = 30_000;
/// Scroll clicks per call cap.
pub const MAX_SCROLL_AMOUNT: u32 = 100;
/// `type` text length cap (characters). Over the cap is rejected explicitly —
/// an out-of-control oversized injection would both drag down the input loop
/// and make the confirmation summary unreadable.
pub const MAX_TYPE_TEXT_CHARS: usize = 10_000;
/// `key`/`hold_key` chord raw-text length cap (characters). The parsed chord
/// itself is already limited to ≤4 key names, but the raw text rides verbatim
/// into the summary/confirm event/result: without a cap, the model could use
/// whitespace to smuggle a string of any size into the event payload (review
/// finding). Legal chords (e.g. "ctrl+shift+alt+delete") are far below this
/// value; over the cap is rejected as a parse error.
pub const MAX_KEY_CHORD_TEXT_CHARS: usize = 128;
/// `confirm_id` length cap (characters), same shape as the chord cap: the id
/// is model-supplied free text and is looked up in the approval-token map as
/// given, so an uncapped string would be pure memory abuse. Real minted ids
/// (`cu-<hex>`) are far below this; over the cap is rejected as a parse
/// error.
pub const MAX_CONFIRM_ID_CHARS: usize = 128;
/// `ui_tree` argument caps. Bounds mirror the schema's `max_depth`/`max_nodes`
/// properties (`minimum: 1`, maximums = these constants); the parser rejects
/// out-of-range values explicitly ([`opt_u32_range`]) instead of silently
/// clamping or truncating. Note the macOS/Windows backends additionally cap
/// their own tree walks below these values (slower cross-process a11y APIs):
/// a parser-accepted `max_nodes` above their cap is clamped there — this is
/// disclosed in the tool description so the model is not surprised by a
/// smaller tree than requested.
pub const MAX_UI_TREE_DEPTH: u32 = 64;
pub const MAX_UI_TREE_NODES: u32 = 10_000;
/// `open` program-name cap (characters), mirroring the parse-time cap
/// convention of the other free-text fields: the program rides the consent
/// summary/payload and the audit trail, so an uncapped model-supplied string
/// would be pure memory/abuse surface. Real executables are far below.
pub const MAX_OPEN_PROGRAM_CHARS: usize = 1024;
/// `open` per-argument cap (characters) and argument-count cap: file paths
/// and URLs are the legitimate shape; anything past these is a model error,
/// rejected at parse time (schema declares the same bounds).
pub const MAX_OPEN_ARG_CHARS: usize = 4096;
pub const MAX_OPEN_ARGS: usize = 64;

/// Stable audit code for the T3 "confirmation required" error. The full
/// error message carries the element label (on-screen content may be
/// sensitive), so the audit record's error field holds only this code; the
/// model still receives the full message verbatim.
const T3_CONFIRM_REQUIRED_ERROR: &str = "t3-confirmation-required";

/// Subdirectory where screenshots are stored (relative to the workspace): the
/// engine's image_analyze fallback resolves workspace-relative paths, and
/// `attachments/` is its established root.
const ATTACHMENTS_DIR: &str = "attachments/computer_use";

/// Retention cap for stored screenshots (per workspace directory). Screens
/// can show passwords/secrets, so unbounded accumulation is a privacy
/// liability the audit log does not need (it keeps the SHA-256 + path
/// trail; old files simply expire out of the store).
const MAX_RETAINED_SCREENSHOTS: usize = 100;

/// The full action set exposed by the tool schema. Single source of truth:
/// the schema's enum, the unknown-action error text, and `parse_action`'s
/// dispatch must agree — the parity test (tool/tests.rs) pins all three so
/// adding an action cannot change only one of them.
pub const SUPPORTED_ACTIONS: &[&str] = &[
    "screenshot",
    "cursor_position",
    "wait",
    "ui_tree",
    "element_at_point",
    "mouse_move",
    "scroll",
    "left_click",
    "right_click",
    "middle_click",
    "double_click",
    "triple_click",
    "left_mouse_down",
    "left_mouse_up",
    "left_click_drag",
    "type",
    "key",
    "hold_key",
    "open",
];

/// The Tauri event sink (tests inject a recorder in its place).
pub trait ComputerUseEventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

pub struct TauriEventSink {
    app: AppHandle,
}

impl TauriEventSink {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl ComputerUseEventSink for TauriEventSink {
    fn emit(&self, event: &str, payload: Value) {
        // Emit to the Tauri UI + forward to the optional remote-control
        // transport.
        let _ = self.app.emit(event, payload.clone());
        crate::platform::app_events::forward_app_event(&self.app, event, payload);
    }
}

#[derive(Default)]
struct ToolState {
    last_map: Option<ScaleMap>,
    shot_seq: u64,
    /// Whether this tool physically pressed the left button (set on a
    /// successful down, cleared on a successful up/drag). Its only purpose is
    /// the fallback release after a failed `left_click_drag`: only the button
    /// **this tool itself pressed** is released, never a button the user is
    /// physically holding. No longer a screening input (a mouse_move while
    /// held gets no confirmation just like any other mouse_move; mainstream
    /// products have no held-move re-screening).
    mouse_buttons_held: bool,
    /// The display target this session's backend was created for (the
    /// ONE-TIME creation decision recorded in `Capabilities.target`), cached
    /// at the first capability read. `None` until then: the payload/audit
    /// boundaries fall back to the session's registry row, then to the
    /// resolved preference (see [`resolved_display_target`]) instead of
    /// guessing.
    display_target: Option<DisplayTarget>,
    /// Whether the backend's model-facing fidelity note
    /// (`Capabilities.target_note`) has been surfaced on an action result
    /// already — once per session, not on every result.
    target_note_surfaced: bool,
    /// The target note as the model last saw it (the successful-render
    /// snapshot). A disclosure the backend appends to the note AFTER the
    /// once-per-session channel spent itself — e.g. a mid-session macOS
    /// park failure — rides the next action's warnings as this delta;
    /// without it the appended sentence would never reach the model while
    /// its captures mix in the user's windows. Full content, not a byte
    /// length (R15): a note that SHRANK (the macOS re-park repair clears
    /// the disclosure) and later regrew to its previously-rendered length
    /// was invisible to a strictly-greater length comparison, so a second
    /// — and every later — park-failure episode stayed undisclosed while
    /// the stale "until a placement succeeds" framing sat in the
    /// transcript.
    surfaced_target_note: Option<String>,
}

/// The cloneable execution parts (execute is async while backend calls are
/// all synchronous, so the whole thing moves into `spawn_blocking`; tests
/// inject a mock backend and an event recorder via `with_parts`).
#[derive(Clone)]
struct Parts {
    session_id: String,
    shared: Arc<ComputerUseShared>,
    backend: BackendHandle,
    events: Arc<dyn ComputerUseEventSink>,
    state: Arc<Mutex<ToolState>>,
}

pub struct ComputerUseTool {
    parts: Parts,
}

/// The lazy factory's one-time backend creation, with the cached global
/// availability snapshot refresh pinned to BOTH creation outcomes. The
/// snapshot (`virtualDisplayState`'s global input) is only recomputed at its
/// documented refresh points; routing the creation result through this helper
/// means installing (or removing) the platform's virtual stack — e.g. `xvfb`
/// on linux — while the app runs flips the settings hint on the next status
/// read instead of waiting for a restart. A successful virtual creation
/// refreshes too (the probe inside `create_backend` just answered green);
/// a live session keeps reporting "active" through the per-session registry,
/// which the global snapshot never overrides.
fn create_backend_reporting_availability(
    shared: &ComputerUseShared,
    prefer_virtual: bool,
    create: impl FnOnce(
        bool,
    ) -> (
        Result<Box<dyn ComputerUseBackend>, ComputerUseError>,
        platform::TargetDecision,
    ),
) -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    let (created, decision) = create(prefer_virtual);
    match &created {
        // A FAILED creation under an on preference must not leave the probe's
        // green verdict in the snapshot: the probe only answers
        // binary/class presence, so the plain refresh would flip the hint
        // right back to "ready" for a stack that just demonstrably failed to
        // start. Record the failure instead — the next legitimate refresh
        // point (a preference flip, the next creation, startup replay)
        // re-evaluates it. The ATTRIBUTION follows the decision, not the
        // preference: the decision happens before the build, so a failed
        // Physical build (probe said unavailable) is a physical-backend
        // failure and must not be recorded as "the last virtual session
        // failed to start". The guard reads the CURRENT preference, not the
        // factory-time capture: a virtual creation can take seconds, and a
        // user flipping OFF mid-flight must get the plain refresh (the
        // truthful "the preference is off" snapshot) rather than an
        // "unavailable + the last virtual session failed to start" record
        // describing a target they just declined (R12).
        Err(error) if shared.prefer_virtual_display() => {
            let detail = match decision {
                platform::TargetDecision::Virtual => {
                    format!("the last virtual session failed to start: {error}")
                }
                platform::TargetDecision::Physical => {
                    format!("the computer use backend failed to start: {error}")
                }
            };
            shared.set_virtual_availability(platform::VirtualAvailabilitySnapshot {
                state: "unavailable",
                detail,
            });
        }
        _ => shared.refresh_virtual_availability(),
    }
    created
}

impl ComputerUseTool {
    /// Production constructor: the backend starts lazily (the worker thread
    /// is spawned on the first request).
    pub fn new(app: AppHandle, session_id: String, shared: Arc<ComputerUseShared>) -> Self {
        // The virtual-vs-physical preference is read at FACTORY EXECUTION
        // time (the lazy first request), not at tool construction: that is
        // the "backend creation" moment the one-time fallback decision is
        // pinned to, so a preference flip between construction and first use
        // is still honored. The resolved target is registered on the shared
        // per-session registry (status "active" input) right here on the
        // worker thread, where the backend object already exists — a failed
        // creation registers nothing.
        let shared_for_factory = Arc::clone(&shared);
        let session_for_factory = session_id.clone();
        let backend = BackendHandle::lazy(move || {
            let prefer_virtual = shared_for_factory.prefer_virtual_display();
            let backend = create_backend_reporting_availability(
                &shared_for_factory,
                prefer_virtual,
                platform::create_backend_with_decision,
            )?;
            let target = backend.capabilities().target;
            // The guarded writer: a stop or master-switch-off landing during
            // this (possibly seconds-long) creation must not re-win the row
            // the sweeps just cleared — the flags are checked under the
            // `session_targets` lock (R16; this was the one unguarded
            // writer). The next admitted action re-asserts the row honestly.
            let _ = shared_for_factory.set_session_target_if_live(&session_for_factory, target);
            Ok(backend)
        });
        // Register the handle: on revoke/stop/master-switch off, the command
        // layer goes through shared.backends to have the backend close its
        // persistent OS-level grant (the Wayland portal session); Drop
        // unregisters.
        shared.backends.insert(&session_id, backend.clone());
        Self {
            parts: Parts {
                session_id,
                shared,
                backend,
                events: Arc::new(TauriEventSink::new(app)),
                state: Arc::new(Mutex::new(ToolState::default())),
            },
        }
    }

    /// Test constructor: injects a mock backend and an event sink.
    #[cfg(test)]
    pub(crate) fn with_parts(
        session_id: String,
        shared: Arc<ComputerUseShared>,
        backend: BackendHandle,
        events: Arc<dyn ComputerUseEventSink>,
    ) -> Self {
        shared.backends.insert(&session_id, backend.clone());
        Self {
            parts: Parts {
                session_id,
                shared,
                backend,
                events,
                state: Arc::new(Mutex::new(ToolState::default())),
            },
        }
    }
}

impl Drop for ComputerUseTool {
    fn drop(&mut self) {
        // Session end: dropping the tool is the engine reclaiming it, which
        // is one of the documented grant-lifetime ends (guard.rs). Revoke
        // the grant, this session's pending confirmations and its minted
        // approval tokens — but ONLY when this tool is still the session's
        // active one: the same-session factory race is identity-checked on
        // the registry side below, and a late stale tool's Drop must not
        // wipe the grant and dialogs of a same-session successor the user
        // just approved. An absent registry entry still revokes (defensive
        // cleanup; nothing else owns the session's consent state at that
        // point).
        let registered = self
            .parts
            .shared
            .backends
            .registered_handle(&self.parts.session_id);
        let is_active_tool = registered
            .as_ref()
            .is_none_or(|current| current.is_same_backend(&self.parts.backend));
        if is_active_tool {
            self.parts.shared.revoke_session(&self.parts.session_id);
        }
        // Drop this session's display-target row (the status "active" input)
        // under the SAME identity rule as the revoke: a same-session
        // successor's factory may have registered ITS target already, and a
        // stale tool's Drop must not clear the successor's row.
        if is_active_tool {
            self.parts
                .shared
                .clear_session_target(&self.parts.session_id);
        }
        // Unregister + emergency cleanup: otherwise the registry entry would
        // pin the worker thread (and its persistent portal session) until
        // process exit. A model that pressed the left button and was dropped
        // mid-drag must not leave the user's machine in button-held drag
        // state, and the persistent OS-level grant (Wayland portal session)
        // must close with the tool. The detached thread keeps its own handle
        // clone, so the worker lives until the cleanup finishes and only
        // then is torn down. Identity-checked: a successor's registry entry
        // is restored, only this tool's handle is cleaned up.
        self.parts
            .shared
            .backends
            .release_and_unregister(&self.parts.session_id, &self.parts.backend);
    }
}

// ---------------------------------------------------------------------------
// Input validation (Anthropic style: descriptive errors for invalid argument
// combinations so the model can self-correct)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ParsedCall {
    action: ComputerUseAction,
    confirm_id: Option<String>,
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::invalid_input(message.into())
}

fn field_type_error(field: &str, expected: &str) -> ToolError {
    invalid(format!("{field} must be {expected}"))
}

fn opt_u64(input: &Value, field: &str, max: u64) -> Result<Option<u64>, ToolError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let number = value
                .as_u64()
                .ok_or_else(|| field_type_error(field, "a non-negative integer"))?;
            if number > max {
                return Err(invalid(format!("{field} must be <= {max}; got {number}")));
            }
            Ok(Some(number))
        }
    }
}

fn opt_i64(input: &Value, field: &str) -> Result<Option<i64>, ToolError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| field_type_error(field, "an integer")),
    }
}

/// Narrows an `Option<u64>` argument to `Option<u32>` within `min..=max`.
/// The bounds mirror the tool schema's `minimum`/`maximum` for the field; a
/// value outside either is rejected explicitly instead of `as u32` silently
/// truncating (truncation would silently turn max_nodes/max_depth into
/// different values).
fn opt_u32_range(input: &Value, field: &str, min: u32, max: u32) -> Result<Option<u32>, ToolError> {
    match opt_u64(input, field, u64::from(max))? {
        None => Ok(None),
        Some(value) if value < u64::from(min) => {
            Err(invalid(format!("{field} must be >= {min}; got {value}")))
        }
        Some(value) => u32::try_from(value)
            .map(Some)
            .map_err(|_| invalid(format!("{field} out of range; got {value}"))),
    }
}

/// x/y appear as a pair and are non-negative (screenshot-space coordinates).
fn opt_coord(input: &Value) -> Result<Option<(i64, i64)>, ToolError> {
    let x = opt_i64(input, "x")?;
    let y = opt_i64(input, "y")?;
    match (x, y) {
        (None, None) => Ok(None),
        (Some(x), Some(y)) => {
            if x < 0 || y < 0 {
                return Err(invalid(format!(
                    "coordinates must be non-negative; got ({x}, {y})"
                )));
            }
            Ok(Some((x, y)))
        }
        _ => Err(invalid(
            "x and y must be provided together (or neither)".to_string(),
        )),
    }
}

fn req_text(input: &Value, action: &str) -> Result<String, ToolError> {
    match input.get("text") {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        Some(Value::String(_)) => Err(invalid(format!(
            "text must be a non-empty string for {action}"
        ))),
        None | Some(Value::Null) => Err(invalid(format!("text is required for {action}"))),
        Some(_) => Err(field_type_error("text", "a string")),
    }
}

/// Rejects fields the action does not accept (`action`/`confirm_id` are
/// globally common and not counted). An explicit `null` is rejected too — a
/// field the schema does not define is a signal of model hallucination /
/// protocol drift even when its value is null; letting it through silently
/// would make schema drift unobservable (review finding).
fn reject_unexpected(input: &Value, action: &str, allowed: &[&str]) -> Result<(), ToolError> {
    let Some(object) = input.as_object() else {
        return Err(invalid("input must be a JSON object"));
    };
    for key in object.keys() {
        if key == "action" || key == "confirm_id" || allowed.contains(&key.as_str()) {
            continue;
        }
        let label = match key.as_str() {
            "x" | "y" => "coordinate",
            // The error string goes into the audit log's error field; unknown
            // field names (model input) are not echoed. The action here has
            // necessarily matched a literal branch, so it is a safe known
            // action name.
            _ => "unexpected field",
        };
        return Err(invalid(format!("{label} is not accepted for {action}")));
    }
    Ok(())
}

fn parse_action(input: &Value) -> Result<ParsedCall, ToolError> {
    let action_name = input
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::missing_field("action"))?;
    let confirm_id = match input.get("confirm_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => {
            // Cap mirrors the other raw-text fields: the id is model-supplied
            // and would otherwise ride into the peek lookup at any size.
            // Real ids (`cu-<hex>`) are far below the cap; over it is
            // rejected as a parse error.
            if id.chars().count() > MAX_CONFIRM_ID_CHARS {
                return Err(invalid(format!(
                    "confirm_id exceeds {MAX_CONFIRM_ID_CHARS} characters"
                )));
            }
            Some(id.clone())
        }
        Some(_) => return Err(field_type_error("confirm_id", "a string")),
    };

    let action = match action_name {
        "screenshot" => {
            reject_unexpected(input, action_name, &[])?;
            ComputerUseAction::Screenshot
        }
        "cursor_position" => {
            reject_unexpected(input, action_name, &[])?;
            ComputerUseAction::CursorPosition
        }
        "wait" => {
            reject_unexpected(input, action_name, &["ms"])?;
            let ms = opt_u64(input, "ms", MAX_WAIT_MS)?
                .ok_or_else(|| invalid("ms is required for wait"))?;
            // Reject 0 explicitly: the schema declares `minimum: 1` for the
            // shared ms field, so a strict client can never send the 0 this
            // branch used to tolerate as a no-op (same lower bound as
            // hold_key/scroll).
            if ms == 0 {
                return Err(invalid("ms must be >= 1 for wait"));
            }
            ComputerUseAction::Wait { ms }
        }
        "ui_tree" => {
            reject_unexpected(input, action_name, &["max_depth", "max_nodes"])?;
            // Bounds mirror the schema (`minimum: 1`, maximums
            // MAX_UI_TREE_DEPTH/MAX_UI_TREE_NODES); 0 is rejected explicitly
            // rather than silently accepting a no-op tree query.
            let max_depth = opt_u32_range(input, "max_depth", 1, MAX_UI_TREE_DEPTH)?;
            let max_nodes = opt_u32_range(input, "max_nodes", 1, MAX_UI_TREE_NODES)?;
            ComputerUseAction::UiTree {
                opts: UiTreeOptions {
                    max_depth,
                    max_nodes,
                },
            }
        }
        "element_at_point" => {
            reject_unexpected(input, action_name, &["x", "y"])?;
            let (x, y) = opt_coord(input)?
                .ok_or_else(|| invalid("x and y are required for element_at_point"))?;
            ComputerUseAction::ElementAtPoint { x, y }
        }
        "mouse_move" => {
            reject_unexpected(input, action_name, &["x", "y"])?;
            let (x, y) =
                opt_coord(input)?.ok_or_else(|| invalid("x and y are required for mouse_move"))?;
            ComputerUseAction::MouseMove { x, y }
        }
        "scroll" => {
            reject_unexpected(input, action_name, &["direction", "amount", "x", "y"])?;
            let direction = input
                .get("direction")
                .and_then(Value::as_str)
                .and_then(ScrollDirection::parse)
                .ok_or_else(|| {
                    invalid("direction is required for scroll and must be up|down|left|right")
                })?;
            let amount = match input.get("amount") {
                None | Some(Value::Null) => {
                    return Err(invalid("amount is required for scroll"));
                }
                Some(value) => value
                    .as_u64()
                    .ok_or_else(|| field_type_error("amount", "a non-negative integer"))?,
            };
            // Lower bound 1: scrolling 0 clicks is a model error; reject
            // explicitly rather than silently no-op.
            if amount == 0 {
                return Err(invalid("amount must be >= 1 for scroll"));
            }
            if amount > u64::from(MAX_SCROLL_AMOUNT) {
                return Err(invalid(format!(
                    "amount must be <= {MAX_SCROLL_AMOUNT}; got {amount}"
                )));
            }
            ComputerUseAction::Scroll {
                direction,
                amount: u32::try_from(amount)
                    .map_err(|_| invalid("amount out of range for scroll"))?,
                at: opt_coord(input)?,
            }
        }
        "left_click" | "right_click" | "middle_click" | "double_click" | "triple_click" => {
            reject_unexpected(input, action_name, &["x", "y"])?;
            let (button, count) = match action_name {
                "left_click" => (MouseButton::Left, 1),
                "right_click" => (MouseButton::Right, 1),
                "middle_click" => (MouseButton::Middle, 1),
                "double_click" => (MouseButton::Left, 2),
                _ => (MouseButton::Left, 3),
            };
            ComputerUseAction::Click {
                button,
                count,
                at: opt_coord(input)?,
            }
        }
        "left_mouse_down" => {
            reject_unexpected(input, action_name, &[])?;
            ComputerUseAction::MouseDown {
                button: MouseButton::Left,
            }
        }
        "left_mouse_up" => {
            reject_unexpected(input, action_name, &[])?;
            ComputerUseAction::MouseUp {
                button: MouseButton::Left,
            }
        }
        "left_click_drag" => {
            reject_unexpected(input, action_name, &["start_x", "start_y", "x", "y"])?;
            let start_x = opt_i64(input, "start_x")?
                .ok_or_else(|| invalid("start_x is required for left_click_drag"))?;
            let start_y = opt_i64(input, "start_y")?
                .ok_or_else(|| invalid("start_y is required for left_click_drag"))?;
            let (x, y) = opt_coord(input)?
                .ok_or_else(|| invalid("x and y are required for left_click_drag"))?;
            if start_x < 0 || start_y < 0 {
                return Err(invalid("coordinates must be non-negative"));
            }
            ComputerUseAction::Drag {
                start: (start_x, start_y),
                end: (x, y),
            }
        }
        "type" => {
            reject_unexpected(input, action_name, &["text"])?;
            let text = req_text(input, action_name)?;
            // NUL cannot be typed meaningfully and would pollute downstream
            // length / audit statistics: reject explicitly.
            if text.contains('\0') {
                return Err(invalid("text must not contain NUL characters for type"));
            }
            // Normalize CR/CRLF once here so every downstream consumer sees
            // the text that will actually be injected: the length cap, the
            // confirm-dialog length/preview, the audit "typed N characters"
            // and the chunk splitting all measure the normalized text (each
            // CRLF is one Return, not two characters). The per-platform
            // normalize calls stay as a no-op second line of defense.
            let text = platform::normalize_typed_newlines(&text).into_owned();
            let count = text.chars().count();
            if count > MAX_TYPE_TEXT_CHARS {
                return Err(invalid(format!(
                    "text is too long for type: {count} chars (max {MAX_TYPE_TEXT_CHARS}); \
                     split it into smaller chunks"
                )));
            }
            ComputerUseAction::Type { text }
        }
        "key" => {
            reject_unexpected(input, action_name, &["text"])?;
            let text = req_text(input, action_name)?;
            if text.chars().count() > MAX_KEY_CHORD_TEXT_CHARS {
                return Err(invalid(format!(
                    "text is too long for key (max {MAX_KEY_CHORD_TEXT_CHARS} characters)"
                )));
            }
            let keys = parse_key_chord(&text).map_err(invalid)?;
            ComputerUseAction::KeyChord { keys, chord: text }
        }
        "hold_key" => {
            reject_unexpected(input, action_name, &["text", "ms"])?;
            let text = req_text(input, action_name)?;
            if text.chars().count() > MAX_KEY_CHORD_TEXT_CHARS {
                return Err(invalid(format!(
                    "text is too long for hold_key (max {MAX_KEY_CHORD_TEXT_CHARS} characters)"
                )));
            }
            let keys = parse_key_chord(&text).map_err(invalid)?;
            let ms = opt_u64(input, "ms", MAX_HOLD_KEY_MS)?
                .ok_or_else(|| invalid("ms is required for hold_key"))?;
            if ms == 0 {
                return Err(invalid("ms must be >= 1 for hold_key"));
            }
            ComputerUseAction::HoldKey {
                keys,
                chord: text,
                ms,
            }
        }
        "open" => {
            reject_unexpected(input, action_name, &["program", "args"])?;
            // The program is spawned directly (no shell), so a bare name or
            // absolute path is the legitimate shape — the OS resolves bare
            // names through the platform's usual search order at spawn time
            // (on Windows that search is not a plain PATH walk; the model-
            // facing schema says the same).
            let program = match input.get("program") {
                Some(Value::String(program)) if !program.is_empty() => program.clone(),
                Some(Value::String(_)) => {
                    return Err(invalid("program must be a non-empty string for open"));
                }
                None | Some(Value::Null) => {
                    return Err(invalid("program is required for open"));
                }
                Some(_) => return Err(field_type_error("program", "a string")),
            };
            // NUL cannot appear in an argv entry on unix (spawn would fail
            // with an obscure libc error) and has no business in a program
            // name: reject at parse time with the same shape as `type`.
            if program.contains('\0') {
                return Err(invalid("program must not contain NUL characters for open"));
            }
            let count = program.chars().count();
            if count > MAX_OPEN_PROGRAM_CHARS {
                return Err(invalid(format!(
                    "program is too long for open: {count} chars (max {MAX_OPEN_PROGRAM_CHARS})"
                )));
            }
            let args = match input.get("args") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(values)) => {
                    if values.len() > MAX_OPEN_ARGS {
                        return Err(invalid(format!(
                            "args has more than {MAX_OPEN_ARGS} entries for open"
                        )));
                    }
                    let mut args = Vec::with_capacity(values.len());
                    for value in values {
                        match value {
                            Value::String(arg) => {
                                if arg.contains('\0') {
                                    return Err(invalid(
                                        "args must not contain NUL characters for open",
                                    ));
                                }
                                let count = arg.chars().count();
                                if count > MAX_OPEN_ARG_CHARS {
                                    return Err(invalid(format!(
                                        "each arg must be <= {MAX_OPEN_ARG_CHARS} characters for open"
                                    )));
                                }
                                args.push(arg.clone());
                            }
                            _ => return Err(field_type_error("args", "an array of strings")),
                        }
                    }
                    args
                }
                Some(_) => return Err(field_type_error("args", "an array of strings")),
            };
            ComputerUseAction::Open { program, args }
        }
        _ => {
            // The error string goes into the audit log's error field; the
            // model-supplied action name is not echoed (a model could smuggle
            // text into the log by putting its input into the action name; it
            // already knows what it sent, so echoing adds no information).
            return Err(invalid(format!(
                "unknown action; supported: {}",
                SUPPORTED_ACTIONS.join(", ")
            )));
        }
    };
    Ok(ParsedCall { action, confirm_id })
}

/// Screenshot artifacts (the persisted PNG + the scale map).
struct ShotOutcome {
    abs_path: PathBuf,
    rel_path: String,
    png: Vec<u8>,
    map: ScaleMap,
}

fn resolve_workspace(context: &ToolContext, session_id: &str) -> PathBuf {
    if context.workspace.as_os_str().is_empty() {
        crate::platform::paths::session_workspace_dir(session_id)
    } else {
        context.workspace.clone()
    }
}

fn capture_and_store(parts: &Parts, workspace: &Path) -> Result<ShotOutcome, ComputerUseError> {
    let capture = parts.backend.capture()?;
    // Invariant: rgba holds exactly width*height*4 bytes. Every consumer
    // (downscale slicing, platform row copies) assumes it, so a malformed
    // backend capture must fail explicitly here instead of panicking deep in
    // the scaling layer.
    let expected = u64::from(capture.width) * u64::from(capture.height) * 4;
    debug_assert_eq!(
        capture.rgba.len() as u64,
        expected,
        "capture rgba length must equal width*height*4"
    );
    if capture.rgba.len() as u64 != expected {
        return Err(ComputerUseError::failed(format!(
            "backend capture rgba buffer is {} bytes, expected {expected} ({}x{}x4)",
            capture.rgba.len(),
            capture.width,
            capture.height
        )));
    }
    let scaled: ScaledScreenshot = scaling::downscale_and_encode(&capture)?;
    let dir = workspace.join(ATTACHMENTS_DIR);
    // Persist on the private-file foundation (review finding: `std::fs::write`
    // used to write with umask default permissions while screen content may
    // contain passwords and the audit record itself is 0600 — a
    // self-contradictory privacy position). A 0700 directory + a 0600 file
    // (profile ACL semantics on Windows); the frontend's same-user
    // `openArtifactExternal` reads are unaffected.
    let directory =
        crate::platform::filesystem::open_private_file_directory(&dir).map_err(|error| {
            ComputerUseError::failed(format!("cannot create {ATTACHMENTS_DIR}: {error}"))
        })?;
    let mut state = parts.state.lock();
    state.shot_seq += 1;
    let seq = state.shot_seq;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let file_name = format!("{stamp}-{seq:04}-{}.png", session_tag(&parts.session_id));
    let abs_path = dir.join(&file_name);
    directory
        .atomic_write_private_file(std::ffi::OsStr::new(&file_name), &scaled.png)
        .map_err(|error| ComputerUseError::failed(format!("cannot write screenshot: {error}")))?;
    prune_old_screenshots(&dir, MAX_RETAINED_SCREENSHOTS, &file_name);
    let map = scaled.map;
    state.last_map = Some(map.clone());
    drop(state);
    Ok(ShotOutcome {
        abs_path,
        rel_path: format!("{ATTACHMENTS_DIR}/{file_name}"),
        png: scaled.png,
        map,
    })
}

fn shot_result_text(shot: &ShotOutcome, target: DisplayTarget) -> String {
    let mut text = format!(
        "screenshot saved: {}\nimage: {}x{} px (scale {:.3} of device {}x{}, monitor origin ({}, {}))\n\
         All coordinates you pass to computer_use are in this image's pixel space, origin top-left.\n\
         If the image is not attached to this result, call image_analyze with image_path=\"{}\" to view it.",
        shot.abs_path.display(),
        shot.map.shot_w,
        shot.map.shot_h,
        shot.map.factor(),
        shot.map.dev_w,
        shot.map.dev_h,
        shot.map.origin_x,
        shot.map.origin_y,
        shot.rel_path,
    );
    // Stable target marker (frozen cross-check contract — see
    // `target_marker` below): tells the model which
    // display it is acting on without any parsing cost — the frontend only
    // parses the path, so this string is frozen. On the virtual target the
    // user's mouse and keyboard are untouched (NO stack keeps every session
    // pixel off the user's screen, though — each platform's escape residual
    // lives in the target note: Linux raw-Wayland, Windows broker-launched
    // apps and the desktop-by-name pull-out, macOS the AX move); on the
    // physical target disruption should be minimized and announced.
    text.push('\n');
    text.push_str(target_marker(target));
    // Review finding: at the minimum scale floor the PNG can still exceed
    // the foundation's attach cap, which silently skips it — the model must
    // be told instead of going blind. The warning keys on the foundation's
    // 5 MB hard cap (a 4–5 MB result IS attached despite missing the 4 MB
    // scaling target above), not on the scaling target.
    if shot.png.len() > scaling::FOUNDATION_ATTACH_CAP_BYTES {
        text.push_str(
            "\nwarning: this encoded image exceeds the attach size cap even at the minimum \
             scale, so it will NOT be attached; use image_analyze with image_path to view it.",
        );
    }
    text
}

/// The stable per-target marker strings appended to every screenshot result.
/// Frozen spellings — do not reword without a cross-check of
/// frontend/consumers.
fn target_marker(target: DisplayTarget) -> &'static str {
    match target {
        DisplayTarget::Virtual => "[target: isolated virtual display]",
        DisplayTarget::Physical => "[target: user's desktop]",
    }
}

/// The resolved display target for this session: the backend's recorded
/// one-time creation decision once capabilities have been read (cached in
/// [`ToolState`]), else the session's registered target — written at
/// creation, BEFORE any audit record can exist, so a preference or probe
/// flip between creation and the first capability read cannot make an
/// error-path record mislabel a live virtual session. The preference+probe
/// re-derivation below is the last resort for records outside a registered
/// session (the same rule the creation-time decision uses). Used by the
/// payload / audit / screenshot-marker boundaries.
fn resolved_display_target(parts: &Parts) -> DisplayTarget {
    if let Some(target) = parts.state.lock().display_target {
        return target;
    }
    if let Some(target) = parts.shared.session_target(&parts.session_id) {
        return target;
    }
    let availability = platform::probe_virtual_display();
    match platform::resolve_target(parts.shared.prefer_virtual_display(), &availability) {
        platform::TargetDecision::Virtual => DisplayTarget::Virtual,
        platform::TargetDecision::Physical => DisplayTarget::Physical,
    }
}

/// Caches the backend's recorded target at every capability read (both the
/// early Input read in `execute` and the per-call read in `run`): the first
/// read replaces the preference fallback with the backend's actual decision.
fn cache_display_target(parts: &Parts, capabilities: &Capabilities) {
    parts.state.lock().display_target = Some(capabilities.target);
}

/// An admitted run re-asserts the session's display-target row: a stop
/// clears the rows (no consent state survives a stop) while the tool itself
/// stays registered, so the same session re-granting and driving again would
/// otherwise have NO row — the settings hint would map the (physical) live
/// session to the isolated-display vocabulary ("ready") and lose the
/// banner's server truth, exactly the contradiction class the R8/R11/R12
/// rounds closed. The row is written from the backend's recorded decision
/// (the cache this run just primed), under the same identity shape the
/// factory and Drop use; a stopped session that is never re-granted keeps
/// no row (the R12 contract is unchanged for it).
///
/// Returns whether the row was (re-)written. A stop (or master-switch-off)
/// that landed between the gate checks above and here must not win the row
/// back: the R12 contract says no state survives a stop, and the settings
/// hint must not report the stopped session as live until idle reaping
/// drops the tool (R15: the re-assert used to run unconditionally; R16:
/// the stop/enable flags are now checked by the writer itself WHILE the
/// `session_targets` lock is held — a stop_all landing between the earlier
/// latch read and the insert used to leave a microseconds-wide window in
/// which the row was re-won after the sweep). The next admitted run
/// re-asserts honestly — the latch is re-read under the same lock.
fn reassert_display_target_row(parts: &Parts) -> bool {
    if parts.shared.session_target(&parts.session_id).is_none() {
        if let Some(target) = parts.state.lock().display_target {
            return parts
                .shared
                .set_session_target_if_live(&parts.session_id, target);
        }
    }
    false
}

fn with_image_metadata(result: ToolResult, shot: &ShotOutcome) -> ToolResult {
    result.with_metadata(json!({
        "images": [shot.abs_path.to_string_lossy()]
    }))
}

/// Keeps only the newest `keep` PNGs in the screenshot directory (filenames
/// sort chronologically: `%Y%m%d-%H%M%S`-{seq}-{tag} — the stamp is
/// fixed-width, the per-session sequence zero-padded, the session tag
/// disambiguates sessions sharing one explicit workspace). The directory
/// never holds more than `keep` PNGs afterwards. `just_written` always
/// survives and counts toward that cap (at most `keep - 1` older files stay
/// beside it): a backwards local-clock step (DST fall-back, NTP correction,
/// VM snapshot resume) used to make the fresh capture sort oldest, so it was
/// unlinked moments after the model was told its path — every capture during
/// the backwards window was destroyed the same way. Leaving the fresh file
/// outside the budget would instead have let every such capture push the
/// directory one file over the promised maximum.
/// Also sweeps crash-orphaned `.pinvou-private-write-*.tmp` files older than
/// [`TMP_SWEEP_AGE`]: a crash mid-write strands a complete screen image in a
/// temp file that retention never reclaimed. Best-effort: any error
/// (vanished directory, unreadable entry, failed unlink) is ignored —
/// retention must never fail the action that just captured.
fn prune_old_screenshots(dir: &Path, keep: usize, just_written: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut fresh_present = false;
    let mut names: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        // Crash-orphaned atomic-write temps hold complete screen PNGs; once
        // old enough not to be a live write, sweep them too.
        if name.starts_with(".pinvou-private-write-") && name.ends_with(".tmp") {
            if let Ok(age) = now.duration_since(
                entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .unwrap_or(std::time::SystemTime::now()),
            ) {
                if age >= TMP_SWEEP_AGE {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            continue;
        }
        if entry.path().extension().is_some_and(|ext| ext == "png") {
            if name == just_written {
                fresh_present = true;
            } else {
                names.push(name);
            }
        }
    }
    let budget = if fresh_present {
        keep.saturating_sub(1)
    } else {
        keep
    };
    if names.len() <= budget {
        return;
    }
    names.sort();
    let excess = names.len() - budget;
    for name in names.iter().take(excess) {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

/// Age at which a crash-orphaned atomic-write temp file in the screenshot
/// directory becomes sweepable (a live write holds its temp for far less).
const TMP_SWEEP_AGE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// A filesystem-safe 8-hex tag of the session id, mixed into screenshot
/// filenames: two sessions sharing one explicit workspace used to collide on
/// the same-second, same-sequence name (the second rename silently replaced
/// the first session's screenshot, whose audit record then pointed at the
/// wrong pixels).
fn session_tag(session_id: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(session_id.as_bytes());
    let mut tag = String::with_capacity(8);
    for byte in &digest[..4] {
        let _ = std::fmt::Write::write_fmt(&mut tag, format_args!("{byte:02x}"));
    }
    tag
}

fn backend_error_text(error: &ComputerUseError) -> String {
    format!("computer use action failed: {error}")
}

/// Resolves the target of a coordinate-carrying action: clamps to the
/// screenshot range (out-of-bounds gives a warning, not a failure) → input
/// coordinates. Fails closed when the capture came from a path whose
/// shot→input mapping is unverified (Wayland xcap fallback at scale ≠ 1):
/// injecting a believed-wrong position executes an action on whatever is
/// really under the cursor, so the refusal — not a disclosure — is the safe
/// direction.
fn resolve_targets(
    map: &ScaleMap,
    coords: &[(i64, i64)],
    warnings: &mut Vec<String>,
) -> Result<Vec<(i32, i32)>, ComputerUseError> {
    if !map.input_aligned {
        return Err(ComputerUseError::unavailable(
            "input coordinates cannot be aligned with this screenshot: it was captured \
             on the Wayland xcap fallback at a display scale other than 100%, whose \
             mapping into the portal input space is unverified; coordinate input is \
             refused until a portal capture is available (restart capture or the app \
             to re-probe)",
        ));
    }
    Ok(coords
        .iter()
        .map(|(x, y)| {
            let (cx, cy, clamped) = map.clamp_shot(*x, *y);
            if clamped {
                warnings.push(format!(
                    "coordinate ({x}, {y}) is outside the screenshot ({}x{}); clamped to ({cx}, {cy})",
                    map.shot_w, map.shot_h
                ));
            }
            map.shot_to_input(cx, cy)
        })
        .collect())
}

/// The T3 screening verdict. Screening is best-effort category detection:
/// when screening is **impossible** (a11y query failure, unknown cursor,
/// missing screenshot map, cursor outside the captured monitor, etc.), it is
/// always treated as Clear — no mainstream product asks for a confirmation
/// over a screening infrastructure failure, and fail-closed would only cause
/// confirmation storms in AT-SPI-unavailable, multi-monitor and similar
/// scenarios; only a **positive hit** on the denylist/a password field asks
/// for confirmation.
enum T3Screening {
    Clear,
    Blocked(T3Hit),
}

struct T3Hit {
    element_label: String,
    /// Keyed hash of the **raw** screening identity behind `element_label`.
    /// The label is display-sanitized and truncated to
    /// [`platform::MAX_NAME_CHARS`], so two different elements can render the same
    /// line — a label differing only past the truncation tail, or only in
    /// characters sanitization rewrites. The replay comparison therefore
    /// checks this binding too: the token must name the same *element*, not
    /// merely the same *line*.
    element_binding: u64,
    reason: &'static str,
}

/// How a screened coordinate is named when no element could be read there.
/// A target still occupies a slot in the label so the approval binding
/// notices a control *appearing* where the user was shown empty space.
const UNREADABLE_TARGET_LABEL: &str = "(no readable target)";

/// The dialog-facing name of one element: `name (role)`, bounded so a hostile
/// free-form role or name cannot stretch the consent dialog's target line.
///
/// The bound lives here, at the display site, rather than on
/// [`ElementInfo::role`]: the role is *also* a screening input, and truncating
/// it before screening would re-create on the role exactly the padding evasion
/// `name_screening_hit` exists to close on the name.
fn element_label(element: &ElementInfo) -> String {
    format!(
        "{} ({})",
        platform::sanitize_name(&element.name, platform::MAX_NAME_CHARS),
        platform::sanitize_name(&element.role, platform::MAX_NAME_CHARS)
    )
}

/// Runs the denylist/password-field determination on one a11y element.
/// Coordinate screening (screen_points) and keyboard focus screening share the
/// same determination; the safety standard must be identical on both paths.
fn screen_element(element: &ElementInfo) -> T3Screening {
    // Both role predicates take the **raw** role. Linux and Windows build it
    // from a platform enum so it is bounded by construction, but macOS passes
    // `AXRole`/`AXSubrole` through and those are free-form strings supplied by
    // the target app: matching a display-truncated copy would let ~80
    // characters of padding hide `…SecureTextField` from the `contains` test.
    if element.secure || is_secure_role(&element.role) {
        return T3Screening::Blocked(T3Hit {
            element_label: element_label(element),
            element_binding: element.raw_binding,
            reason: "a password/secure field",
        });
    }
    // `name_screening_hit` is the platform layer's denylist verdict on the
    // **raw** accessible name: `name` is display-truncated, so matching only
    // it here would let a padded attacker-controlled label push a
    // consequential term past the window.
    //
    // The display name is matched too rather than trusting the flag alone.
    // Folding drops exactly what sanitization rewrites, so the display name
    // can only ever be the narrower signal — this adds no verdict a correct
    // backend would have missed, but it means a backend that forgets to set
    // the flag degrades to the old, narrower screening instead of silently
    // disabling name screening altogether.
    //
    // The role goes through the streaming matcher (`screening_hit`) rather
    // than the whole-string fold: on macOS it is a free-form app-supplied
    // AXRole, and a hostile multi-megabyte role must not buy a proportional
    // fold allocation on every screening pass. The verdict is identical —
    // same fold, same denylist, chunked.
    if element.name_screening_hit
        || matches_t3_denylist(&element.name)
        || platform::screening_hit(&element.role)
    {
        return T3Screening::Blocked(T3Hit {
            element_label: element_label(element),
            element_binding: element.raw_binding,
            reason: "a consequential control (financial/send/delete/submit/consent)",
        });
    }
    T3Screening::Clear
}

/// Screens every coordinate an action touches, and binds the verdict to **all**
/// of them.
///
/// A multi-point action is one consent decision over several targets:
/// `left_click_drag` screens its start and its drop point, and approving a
/// drag whose start is consequential must not also approve whatever the drop
/// point has become since. The label therefore names every screened target in
/// order, not only the one that hit — it is both what the dialog shows and
/// what the approval token is bound to, so a change at *either* end fails the
/// spend-time comparison and re-raises confirmation.
///
/// For the single-point actions the label is exactly the one element's, so the
/// binding is unchanged for them.
fn screen_points(parts: &Parts, points: &[(i32, i32)]) -> T3Screening {
    let mut labels: Vec<String> = Vec::with_capacity(points.len());
    let mut bindings: Vec<u64> = Vec::with_capacity(points.len());
    let mut reason: Option<&'static str> = None;
    for &(x, y) in points {
        // A screening failure and an absent element are deliberately the same
        // verdict here. Screening unavailable (a11y query failure) is not a
        // denylist hit — it is best-effort category detection, and no
        // mainstream product asks for a confirmation over a screening
        // infrastructure failure (in AT-SPI-unavailable, multi-monitor and
        // similar scenarios, fail-closed would only cause confirmation
        // storms). An absent element is not a red flag either: unnamed targets
        // are everywhere on real desktops (canvas, hover targets, custom
        // widgets) and the denylist is name-based.
        let Ok(Some(element)) = parts.backend.element_at_point(x, y) else {
            labels.push(UNREADABLE_TARGET_LABEL.to_string());
            // Same sentinel on the raw side, so both replays of an
            // unreadable point bind identically.
            bindings.push(raw_element_binding(UNREADABLE_TARGET_LABEL, ""));
            continue;
        };
        let label = element_label(&element);
        if let T3Screening::Blocked(hit) = screen_element(&element) {
            // First hit wins the reason line; the label still names them all.
            reason.get_or_insert(hit.reason);
        }
        bindings.push(element.raw_binding);
        labels.push(label);
    }
    match reason {
        Some(reason) => T3Screening::Blocked(T3Hit {
            element_label: labels.join(" → "),
            element_binding: element_binding(&bindings),
            reason,
        }),
        None => T3Screening::Clear,
    }
}

/// Whether a key chord is effectively typed text: it contains at least one
/// plain character key. Character keys are injected as literal keystrokes
/// even when the chord mixes in named keys (`p+a+s+Return` types `pas`
/// followed by Enter), so ANY chord carrying a character key logs a key
/// count only — a password can be spelled in chunks of up to
/// [`MAX_KEY_CHORD_TOKENS`] characters per call, and each chunk would
/// otherwise land in the audit log as plaintext `keys: …` records.
/// Classification must be content-based, not token-position based —
/// `shift+shift+h` parses to `[Shift, Shift, Char('h')]`, which positional
/// patterns miss while the injection still types a capital H. Chords made
/// only of named keys (Return, Tab, ctrl+Delete, F5, …) inject no characters
/// and keep the readable `keys: <chord>` target.
fn is_typed_text_chord(keys: &[Key]) -> bool {
    keys.iter().any(|k| matches!(k, Key::Char(_)))
}

/// T3 consequential screening: runs only for **activation-class** Input
/// actions (see [`requires_t3_check`]).
///
/// Screening point selection:
/// - Coordinate-carrying clicks check the target point; `left_click_drag`
///   checks **both the start and the drop point** (dragging into the recycle
///   bin/Delete area is a typical consequential action; checking only the
///   cursor would miss the endpoint). Both points are named in the hit label,
///   so an approval for a drag is bound to both ends — see [`screen_points`].
/// - Keyboard actions (type/key/hold_key) screen the focused element passed
///   in by `run` — keyboard input lands on the focus, not at the cursor, so
///   screening the cursor would miss a password field under focus. `run`
///   queries the focused element exactly once and passes it here; the same
///   read also decides the masked type preview, so a focus change between
///   two queries cannot put a password field's full text into the confirm
///   event. `None` = no focus or the query failed: screening is unavailable
///   and clears (fail-open). Chords never get a form-based confirmation —
///   chord editing is reversible; the focused element's denylist/password
///   screening still applies.
/// - Mouse down/up and coordinate-less clicks act at the current cursor.
///   `cursor_position` reports input coordinates directly (points on macOS,
///   physical pixels on Windows/X11); the raw value is gated with
///   `contains_input_point`. Input rects are disjoint per monitor (unlike
///   device pixels on mixed-DPI setups), so containment is exact and a hit
///   screens the real cursor location; a point outside this map's rect
///   belongs to another monitor and mapping it here would screen a location
///   far from the real cursor — no target, Clear.
/// - mouse_move and scroll never enter this function at all (hover/scroll
///   have no action consequences; mainstream products put no gate on
///   hover/scroll).
///
/// Every "screening unavailable" path is Clear (best-effort category
/// detection: only a positive hit confirms; screening infrastructure failures
/// do not cause confirmation storms). One deliberate exception: the Linux
/// AT-SPI read marks a target whose ROLE query failed as possibly-secure
/// (it cannot prove the field is not a password field), so that single
/// unreadable-target shape blocks instead of Clearing — disclosed in the
/// tool description.
fn t3_screening(
    parts: &Parts,
    action: &ComputerUseAction,
    map: Option<&ScaleMap>,
    focused: Option<&ElementInfo>,
) -> T3Screening {
    // Keyboard-class actions: screen the focused element (where keyboard
    // input really lands).
    if matches!(
        action,
        ComputerUseAction::Type { .. }
            | ComputerUseAction::KeyChord { .. }
            | ComputerUseAction::HoldKey { .. }
    ) {
        return match focused {
            Some(element) => screen_element(element),
            None => T3Screening::Clear,
        };
    }
    let points: Vec<(i32, i32)> = match action {
        ComputerUseAction::Click {
            at: Some((x, y)), ..
        } => {
            let Some(m) = map else {
                return T3Screening::Clear;
            };
            let (cx, cy, _) = m.clamp_shot(*x, *y);
            vec![m.shot_to_input(cx, cy)]
        }
        ComputerUseAction::Drag { start, end } => {
            let Some(m) = map else {
                return T3Screening::Clear;
            };
            let (sx, sy, _) = m.clamp_shot(start.0, start.1);
            let (ex, ey, _) = m.clamp_shot(end.0, end.1);
            vec![m.shot_to_input(sx, sy), m.shot_to_input(ex, ey)]
        }
        // mouse down/up and coordinate-less clicks act at the current cursor.
        _ => match parts.backend.cursor_position() {
            Ok((ix, iy)) => {
                let Some(m) = map else {
                    return T3Screening::Clear;
                };
                if !m.contains_input_point(ix, iy) {
                    return T3Screening::Clear;
                }
                vec![(ix, iy)]
            }
            Err(_) => return T3Screening::Clear,
        },
    };
    screen_points(parts, &points)
}

/// Everything the T3 gate needs about the target as it is right now: the
/// screening verdict plus the two dialog-shaping decisions that must be taken
/// from the same focused-element read.
struct T3Context {
    screening: T3Screening,
    /// Whether keyboard input would land on a password/secure field, which
    /// decides whether the full typed text may ride the confirm event.
    secure_type_target: bool,
    type_preview_full: Option<String>,
}

/// Screens the current target and derives the dialog-shaping decisions.
///
/// Used by both the first attempt and the confirm_id replay: the replay has
/// to re-screen because an approval is bound to a target, and the target is
/// ambient state the model can move between the dialog and the replay.
///
/// One focused-element read feeds BOTH the masked-preview decision and
/// keyboard screening: two separate queries could race a focus change onto a
/// password field and leak its full text into the confirm event.
fn t3_context(parts: &Parts, action: &ComputerUseAction) -> T3Context {
    let keyboard_class = matches!(
        action,
        ComputerUseAction::Type { .. }
            | ComputerUseAction::KeyChord { .. }
            | ComputerUseAction::HoldKey { .. }
    );
    let mut focused = None;
    let focus_uncertain = if keyboard_class {
        match parts.backend.focused_element() {
            Ok(value) => {
                focused = value;
                false
            }
            Err(_) => {
                // The Linux layer deliberately returns Err when the focused
                // search exhausts its budget without a verdict ("never
                // masquerade as none"): an uncertain focus must not be
                // flattened into "nothing focused", or the full typed text
                // could ride the confirm event while a password field actually
                // holds focus. Screening itself stays fail-open (focused stays
                // None), but the preview is masked like a secure target.
                true
            }
        }
    } else {
        false
    };
    // The masked-target decision only shapes the dialog payload (whether the
    // full typed text may ride the confirm event). Keyboard chords that carry
    // character keys type their characters too, so the secure-target check
    // covers them just like type.
    let secure_type_target = keyboard_class
        && (focus_uncertain
            || focused
                .as_ref()
                .is_some_and(|element| element.secure || is_secure_role(&element.role)));
    let type_preview_full = full_type_preview(action, secure_type_target);
    let map = parts.state.lock().last_map.clone();
    T3Context {
        screening: t3_screening(parts, action, map.as_ref(), focused.as_ref()),
        secure_type_target,
        type_preview_full,
    }
}

/// T3 screening covers only **activation-class** actions: clicks/press/
/// release/drag/keyboard — actions that can trigger a control or produce an
/// input consequence. mouse_move and scroll are still Input class (they need
/// a session grant and are audited as usual), but hover and scroll produce no
/// action consequence and get no screening confirmation — mainstream products
/// put no gate on hover/scroll.
///
/// Derived from [`ComputerUseAction::class`], not an independent action list:
/// activation-class = Input class minus the hover/scroll no-gate set, so a
/// future Input variant cannot silently drift between `class()` and this
/// check — an explicit decision is required here.
fn requires_t3_check(action: &ComputerUseAction) -> bool {
    action.class() == ActionClass::Input
        && !matches!(
            action,
            ComputerUseAction::MouseMove { .. }
                | ComputerUseAction::Scroll { .. }
                // `open` launches an app before anything is on screen to
                // screen: there is no target element to read, so the
                // per-action T3 gate has nothing to screen (documented in
                // the tool description). The grant gate still applies —
                // `open` is Input class — and on the Linux and Windows
                // virtual stacks the launch stays off the user's desktop
                // entirely (on macOS the app passes through the user's
                // graphics session while its front window is moved in —
                // the tool description says so).
                | ComputerUseAction::Open { .. }
        )
}

/// T3 actions whose screening resolves the target through a SCREEN POINT
/// (element_at_point / cursor mapping): those need the session's ScaleMap
/// even when the action itself carries no coordinates. Keyboard actions
/// (type/key/hold_key) screen through the FOCUSED element only, so they must
/// not inherit a capture dependency: a broken capture path (e.g. macOS
/// Accessibility granted without Screen Recording) must not stop typing.
fn t3_screening_needs_scale_map(action: &ComputerUseAction) -> bool {
    matches!(
        action,
        ComputerUseAction::Click { .. }
            | ComputerUseAction::MouseDown { .. }
            | ComputerUseAction::MouseUp { .. }
            | ComputerUseAction::Drag { .. }
    )
}

/// Whether the full typed text may ride the confirm event
/// (`type_preview_full`), or `None` when it must not. Included ONLY when:
/// - the action is `Type` (other actions have no typed text),
/// - the target is NOT secure/masked for this mint — the masked summary
///   exists precisely so password text is never revealed in the confirm
///   dialog or event stream,
/// - the text is at most 4096 chars — the cap bounds the payload size and
///   thus the abuse surface that reaches the dialog. Short texts ride along
///   too: the dialog otherwise shows only "type N characters", and approving
///   a length with zero visible content is not informed consent.
fn full_type_preview(action: &ComputerUseAction, secure_type_target: bool) -> Option<String> {
    if secure_type_target {
        return None;
    }
    match action {
        ComputerUseAction::Type { text } => {
            let count = text.chars().count();
            (count <= 4096).then(|| text.clone())
        }
        // Character-key chords are typing too: `key "h+a+c+k"` on a labeled
        // non-secure target shows the character-sequence preview (the same
        // transparency as type, the same 4096 cap) — both are typing input
        // and must disclose equally; a count-only chord summary would leave
        // the user blind-signing chunk by chunk. Pure named-key chords
        // (ctrl+s, Return) inject no characters and produce no preview.
        // Password and other secure targets are already caught by
        // secure_type_target at the call site.
        ComputerUseAction::KeyChord { keys, .. } | ComputerUseAction::HoldKey { keys, .. }
            if is_typed_text_chord(keys) =>
        {
            let text: String = keys
                .iter()
                .filter_map(|key| match key {
                    Key::Char(c) => Some(*c),
                    _ => None,
                })
                .collect();
            let count = text.chars().count();
            (count <= 4096).then_some(text)
        }
        _ => None,
    }
}

/// Content hash of the full blocked action, bound into the pending and the
/// approval token (review finding: the human-readable summary deliberately
/// masks typed content, which let a token minted for one `type N characters`
/// be spent on a different same-length text; the binding closes that without
/// changing what the user sees). Hashes the action's Debug rendering — every
/// parameter participates, nothing user-visible changes. In-process only
/// (tokens are memory-bound with a 5-minute TTL), so hash stability across
/// builds does not matter — but the digest must be **keyed**: any
/// *deterministic* 64-bit hash (the fixed-key `DefaultHasher` this scheme
/// replaced as much as an unkeyed truncated SHA-256 an earlier revision of
/// this comment wrongly called collision-free) has a ~2^32 birthday surface
/// an offline attacker can walk. The random per-process key takes the digest
/// out of offline reach entirely: the model never sees a binding value, so
/// it has no oracle to forge against, and reading the key would require the
/// same process-memory access the approval tokens themselves already rely
/// on staying private.
fn action_binding(action: &ComputerUseAction) -> u64 {
    keyed_binding_u64(&[format!("{action:?}").as_bytes()])
}

/// Combines the per-element raw bindings ([`ElementInfo::raw_binding`],
/// computed at the platform boundary where the raw strings still exist) into
/// one target-set binding, in screening order, each as fixed-width
/// big-endian bytes (self-delimiting, so distinct binding vectors always
/// hash distinct byte streams). Same key as [`action_binding`]; in-process
/// only, never shown to the model — the same no-oracle argument.
fn element_binding(bindings: &[u64]) -> u64 {
    let byte_streams: Vec<[u8; 8]> = bindings.iter().map(|b| b.to_be_bytes()).collect();
    let parts: Vec<&[u8]> = byte_streams.iter().map(|bytes| bytes.as_slice()).collect();
    keyed_binding_u64(&parts)
}

/// Structured i18n source for the consent dialog, serialized into the
/// confirm payload alongside the English `summary` string (the summary stays
/// as fallback; the dialog renders localized copy from these fields).
/// Coordinates are shot-space, matching the tool contract. Typed content
/// follows the same masking contract as the summary / `type_preview_full`:
/// never for secure targets, capped at 4096 characters.
#[derive(serde::Serialize)]
struct ConfirmActionDetails {
    /// Canonical action name (one of [`SUPPORTED_ACTIONS`]).
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    button: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    click_count: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    point: Option<ConfirmPoint>,
    /// Drag drop point (drag): the localized line renders
    /// "drag from `point` to `end_point`". Absent for every other action.
    #[serde(skip_serializing_if = "Option::is_none")]
    end_point: Option<ConfirmPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text_length: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text_preview: Option<String>,
    /// True when typing exceeded the 4096 preview cap (the dialog then shows
    /// "text too long to preview in full" next to the length); always false
    /// for non-typing actions and for masked (secure) targets, where the
    /// masking contract — not the length — is why there is no preview.
    text_preview_truncated: bool,
    /// The chord string (key/hold_key) for the localized confirm line. On
    /// masked (secure) targets only the named keys survive here — character
    /// keys are typed content, exactly what the masking contract must never
    /// ride the confirm payload or the get_status replay with.
    #[serde(skip_serializing_if = "Option::is_none")]
    chord: Option<String>,
    /// Number of masked character keys (key/hold_key, secure targets only):
    /// the dialog renders "N masked characters" from the localized templates
    /// — the same count-only vocabulary the summary and the audit use.
    #[serde(skip_serializing_if = "Option::is_none")]
    chord_masked_chars: Option<usize>,
    /// Hold duration in ms (hold_key).
    #[serde(skip_serializing_if = "Option::is_none")]
    hold_ms: Option<u64>,
}

#[derive(serde::Serialize)]
struct ConfirmPoint {
    x: i64,
    y: i64,
}

impl ConfirmActionDetails {
    /// Builds the structured fields for one blocked action. `preview` is
    /// exactly what [`full_type_preview`] allowed for this mint (None for
    /// secure/masked targets and over-cap texts); `masked_target` is the
    /// secure-target decision, needed to tell "masked" apart from
    /// "truncated".
    fn from_action(action: &ComputerUseAction, preview: Option<&str>, masked_target: bool) -> Self {
        let mut details = Self {
            action: action.name(),
            button: None,
            click_count: None,
            point: None,
            end_point: None,
            text_length: None,
            text_preview: preview.map(str::to_string),
            text_preview_truncated: false,
            chord: None,
            chord_masked_chars: None,
            hold_ms: None,
        };
        match action {
            ComputerUseAction::Click {
                button, count, at, ..
            } => {
                details.button = Some(button.as_str());
                details.click_count = Some(*count);
                details.point = at.map(|(x, y)| ConfirmPoint { x, y });
            }
            ComputerUseAction::MouseDown { button } | ComputerUseAction::MouseUp { button } => {
                details.button = Some(button.as_str());
                details.click_count = Some(1);
            }
            ComputerUseAction::Drag { start, end, .. } => {
                // The localized drag line renders start → drop point (the
                // drop is the consequential half: dragging into delete/drop
                // zones).
                details.point = Some(ConfirmPoint {
                    x: start.0,
                    y: start.1,
                });
                details.end_point = Some(ConfirmPoint { x: end.0, y: end.1 });
            }
            // T3 gate excludes hover/scroll/observe, so these arms are currently unreachable.
            ComputerUseAction::MouseMove { x, y } | ComputerUseAction::ElementAtPoint { x, y } => {
                details.point = Some(ConfirmPoint { x: *x, y: *y });
            }
            ComputerUseAction::Type { text } => {
                let count = text.chars().count();
                details.text_length = Some(count);
                details.text_preview_truncated = !masked_target && count > 4096;
            }
            ComputerUseAction::KeyChord {
                keys, chord: text, ..
            }
            | ComputerUseAction::HoldKey {
                keys, chord: text, ..
            } => {
                // The localized chord/hold lines render the chord string (and
                // hold duration). Character keys ARE typed content: on a
                // masked (secure) target the raw chord must not reach the
                // dialog or the event/status stream (a password can be
                // spelled in chunks of up to MAX_KEY_CHORD_TOKENS chars), so
                // the payload degrades to named keys plus a masked-character
                // count — the same count-only vocabulary as the summary and
                // the audit.
                let char_keys = keys
                    .iter()
                    .filter(|key| matches!(key, Key::Char(_)))
                    .count();
                if masked_target && char_keys > 0 {
                    let named: Vec<String> = keys
                        .iter()
                        .filter_map(|key| {
                            (!matches!(key, Key::Char(_))).then(|| key.canonical_name())
                        })
                        .collect();
                    details.chord = (!named.is_empty()).then(|| named.join(" + "));
                    details.chord_masked_chars = Some(char_keys);
                } else {
                    details.chord = Some(text.clone());
                }
                if let ComputerUseAction::HoldKey { ms, .. } = action {
                    details.hold_ms = Some(*ms);
                }
                // Same typing classification boundary as the summary and the
                // audit redaction: only character-carrying chords disclose
                // (a count of) typed content.
                if is_typed_text_chord(keys) {
                    details.text_length = Some(char_keys);
                    details.text_preview_truncated = !masked_target && char_keys > 4096;
                }
            }
            _ => {}
        }
        details
    }
}

/// Builds the `computer_use:confirm_required` event payload: identity fields
/// plus the structured [`ConfirmActionDetails`] merged in (the single
/// serialization path for the structured confirm fields). `target` is the
/// frozen "virtual" | "physical" disclosure (the same frozen vocabulary the
/// status snapshot and audit records use) — the frontend renders
/// target-aware dialog copy and tolerates its absence (old payloads,
/// fail-neutral).
fn build_confirm_payload(
    session_id: &str,
    confirm_id: &str,
    action: &ComputerUseAction,
    summary: &str,
    element_label: &str,
    type_preview_full: Option<&str>,
    masked_target: bool,
    target: &str,
) -> Value {
    let details = ConfirmActionDetails::from_action(action, type_preview_full, masked_target);
    let mut payload = json!({
        "session_id": session_id,
        // Canonical action name; the English parameter summary rides under
        // `summary` as fallback for older consumers.
        "action": details.action,
        "summary": summary,
        "element": element_label,
        "confirm_id": confirm_id,
        // Which display the action targets ("virtual" | "physical").
        "target": target,
    });
    if let Ok(Value::Object(map)) = serde_json::to_value(&details) {
        if let Some(object) = payload.as_object_mut() {
            object.extend(map);
        }
    }
    // Optional full typed text for the confirm dialog, rendered inline
    // (absent = the dialog shows the count-only summary). See
    // `full_type_preview` for when it may exist (never for secure/masked
    // targets).
    if let Some(full) = type_preview_full {
        payload["type_preview_full"] = Value::String(full.to_string());
    }
    payload
}

/// Mints a pending confirmation request, emits the event, and returns the
/// "not executed, go ask for confirmation" error to the model. At most one
/// pending per session: a new request replaces the old one directly (newest
/// wins, like an ordinary dialog).
fn request_confirmation(
    parts: &Parts,
    action: &ComputerUseAction,
    summary: &str,
    element_label: &str,
    reason_phrase: &str,
    type_preview_full: Option<String>,
    masked_target: bool,
    binding: u64,
    element_binding: u64,
) -> String {
    // The payload is built once and serves two consumers: the event
    // broadcast and the guard's server-truth store (re-served through
    // `computer_use_get_status` so every window can reconstruct or collapse
    // the dialog). The confirm_id comes from the mint, so the payload is
    // attached to the stored pending right after building it. The guard
    // refuses to register while disabled/stopped (an in-flight run racing a
    // toggle/stop must not raise an unapprovable dialog): report the same
    // stable audit prefix with the actual state, and no confirm_id.
    let Some(confirm_id) = parts.shared.new_pending_confirmation(
        &parts.session_id,
        summary.to_string(),
        element_label.to_string(),
        binding,
        element_binding,
    ) else {
        return format!(
            "{T3_CONFIRM_REQUIRED_ERROR}: this action targets {reason_phrase}: \
             \"{element_label}\". It was NOT executed. Computer use was disabled or \
             stopped while the confirmation was being asked for; it must be re-enabled \
             (or the stop resumed) before this action can be confirmed."
        );
    };
    let payload = build_confirm_payload(
        &parts.session_id,
        &confirm_id,
        action,
        summary,
        element_label,
        type_preview_full.as_deref(),
        masked_target,
        resolved_display_target(parts).as_str(),
    );
    parts
        .shared
        .set_pending_payload(&confirm_id, payload.clone());
    parts.events.emit(EVENT_CONFIRM_REQUIRED, payload);
    // The prefix is [`T3_CONFIRM_REQUIRED_ERROR`]: the error message carries
    // the element label, so the audit record's error field holds only the
    // stable code (see the tail of `run`); the model still receives the full
    // message.
    format!(
        "{T3_CONFIRM_REQUIRED_ERROR}: this action targets {reason_phrase}: \
         \"{element_label}\". It was NOT executed. \
         Ask the user to confirm in the app; then retry the same action with \
         confirm_id=\"{confirm_id}\"."
    )
}

fn consent_label(action: &ComputerUseAction, confirmed: bool) -> String {
    match action.class() {
        ActionClass::Observe => "observe".to_string(),
        ActionClass::Input => {
            if confirmed {
                "input:session-grant+t3-confirmed".to_string()
            } else {
                "input:session-grant".to_string()
            }
        }
    }
}

fn run(parts: Parts, parsed: ParsedCall, workspace: PathBuf) -> ToolResult {
    let started = Instant::now();
    let action = parsed.action;

    let mut warnings: Vec<String> = Vec::new();
    let mut shot: Option<ShotOutcome> = None;
    let mut confirmed_t3 = false;

    let body: Result<String, String> = (|| {
        // Physical input is a globally exclusive resource: Input-class actions
        // hold the process-level mutex from screening through injection, so
        // two concurrent sessions cannot interleave typing/clicks (review
        // finding). Acquisition is a bounded wait: when the lock is held by
        // another session, it times out with an explicit error (InputBusy)
        // instead of waiting unboundedly and silently wedging other sessions
        // (review finding).
        //
        // The exclusivity protects the USER'S physical desktop. A virtual
        // target's input stays inside the isolated stack by construction (a
        // private X server behind a private cookie / pid-targeted CGEvents
        // re-verified against the display's bounds before every post,
        // within the disclosed ownership-grace residual / messages posted
        // to hidden-desktop windows), so a resolved virtual target skips
        // the lock: serializing independent isolated displays would cap
        // process-wide input throughput and stall a second session up to
        // the timeout for no safety gain. Unknown target ⇒ lock, the
        // conservative direction.
        //
        // The skip's safety rests on a coupling that must stay true: a
        // pending confirmation can only be minted from a Blocked screening
        // verdict, and every virtual stack answers the accessibility
        // queries (`element_at_point` / `focused_element`) with an explicit
        // unsupported — a virtual session can never mint a pending, so
        // nothing mints outside the lock (grant dialogs stay serialized via
        // `mark_grant_requested_serialized` regardless of target). If a
        // virtual stack ever grows a real accessibility tree, restore the
        // lock for at least the mint path — the dialog-under-injection
        // window the physical lock exists to close would otherwise reopen.
        let target_is_virtual = parts.state.lock().display_target == Some(DisplayTarget::Virtual);
        let _input_guard = (action.class() == ActionClass::Input && !target_is_virtual)
            .then(|| parts.shared.lock_physical_input())
            .transpose()
            .map_err(|rejection| rejection.message())?;

        // Coordinate-carrying actions auto-capture first when there is no
        // ScaleMap (session's first); point-screened T3 actions need the map
        // too — the screening point's coordinate conversion and cursor
        // mapping both depend on it. Keyboard T3 actions screen through the
        // focused element only, so they proceed without a map and must not
        // hard-fail when capture is unavailable.
        if (action.needs_scale_map() || t3_screening_needs_scale_map(&action))
            && parts.state.lock().last_map.is_none()
        {
            // The target marker must reflect the backend's ONE-TIME creation
            // decision, not a fresh probe: the probe can flip after creation
            // (a virtual stack installed mid-app-run), and stamping a
            // physical session's first screenshot with the virtual marker
            // would misreport the target. Prime the cache from the backend's
            // stamped capabilities once, here, so this and every later
            // boundary read the same decision.
            if let Ok(capabilities) = parts.backend.capabilities() {
                cache_display_target(&parts, &capabilities);
            }
            let auto = capture_and_store(&parts, &workspace).map_err(|e| backend_error_text(&e))?;
            // Route the auto-captured outcome through the SAME shot_result_text
            // path as explicit/follow-up captures (review finding): every
            // screenshot the model sees must carry the frozen "[target: …]"
            // marker and the coordinate-space header, not just the bare
            // auto-capture notice. Rendered here, appended with the warnings
            // below, so actions without a follow-up capture (bare pointer
            // moves) still surface the marker.
            warnings.push(format!(
                "no screenshot had been taken this session; one was captured automatically and is \
                 attached.\n{}",
                shot_result_text(&auto, resolved_display_target(&parts))
            ));
            shot = Some(auto);
        }

        // Reject before any consent surface: a stop/revoke landing during the
        // automatic screenshot must not still pop a confirmation dialog
        // (review finding; the mint side refuses stopped/disabled as a second
        // line of defense).
        if action.class() == ActionClass::Input {
            if let Err(rejection) = parts.shared.begin_input_action(&parts.session_id) {
                return Err(rejection.message());
            }
        }

        // T3 confirmation gate. An action executes only when the current
        // target screens Clear, or when the model passes back a confirm_id
        // whose token matches this session, the action summary, the action
        // content hash **and** the target the dialog actually named.
        if requires_t3_check(&action) {
            let summary = action_summary(&action);
            let binding = action_binding(&action);
            // Validate the token WITHOUT consuming it: the target still has
            // to be re-screened, and a run that never reaches the backend
            // must not burn the user's approval.
            let approved = match &parsed.confirm_id {
                Some(id) => {
                    match parts
                        .shared
                        .peek_confirmation(id, &parts.session_id, &summary, binding)
                    {
                        Some((label, element_binding)) => {
                            Some((id.clone(), label, element_binding))
                        }
                        None => {
                            return Err(
                                "the confirm_id is invalid, expired, or was already used. Ask the \
                             user to confirm again."
                                    .to_string(),
                            );
                        }
                    }
                }
                None => None,
            };
            let had_token = approved.is_some();
            // Screen the target as it is NOW, on both paths. The action's own
            // parameters do not pin a target: mouse_down and a
            // coordinate-less click act wherever the cursor happens to be,
            // and even a coordinate-carrying click lands on whatever occupies
            // that point. Spending a token without re-screening therefore let
            // an approval granted for one control authorize a click on
            // another — the model only had to move the cursor or scroll the
            // page (neither is screened) between the dialog and the replay.
            let context = t3_context(&parts, &action);
            match context.screening {
                // Nothing consequential under the target. Execution is what an
                // unapproved run would already do, so a carried token is
                // simply spent.
                T3Screening::Clear => {
                    if let Some((id, _, _)) = &approved {
                        // consume_confirmation returning false means the token
                        // was retracted between peek and spend — a Deny
                        // ("approve → changed my mind") landing during the
                        // re-screen's a11y queries. Stop rather than execute
                        // over a retracted approval.
                        if !parts.shared.consume_confirmation(id) {
                            return Err(
                                "the confirmation was denied by the user before this action ran. \
                                 Ask the user to confirm again."
                                    .to_string(),
                            );
                        }
                        confirmed_t3 = true;
                    }
                }
                T3Screening::Blocked(hit) => match approved {
                    // Approved, and still the same target: spend and execute.
                    // The label comparison is what the dialog named; the
                    // element-binding comparison catches the twin the label
                    // cannot see — a different element whose sanitized,
                    // 80-char-truncated line renders identically (a payload
                    // differing only past the truncation tail or only in
                    // characters sanitization rewrites).
                    Some((id, label, element_binding))
                        if label == hit.element_label && element_binding == hit.element_binding =>
                    {
                        // Same deny race as the Clear arm: false means the
                        // user retracted the approval while the re-screen ran.
                        if !parts.shared.consume_confirmation(&id) {
                            return Err(
                                "the confirmation was denied by the user before this action ran. \
                                 Ask the user to confirm again."
                                    .to_string(),
                            );
                        }
                        confirmed_t3 = true;
                    }
                    // Either no token at all, or a token approved for a
                    // different target (different label, or the same label
                    // over a different raw element). Both raise a fresh
                    // confirmation naming what is under the target now; the
                    // stale token is left alone so a correct retry can still
                    // spend it.
                    _ => {
                        // The a11y queries above (focused element, screening)
                        // take real time: re-check the stop/disable/grant
                        // state BEFORE raising the dialog, so a stop landing
                        // during screening does not pop a consent surface for
                        // a feature that is now off (the mint-side refusal
                        // stays as the backstop for the race window that
                        // remains).
                        if let Err(rejection) = parts.shared.begin_input_action(&parts.session_id) {
                            return Err(rejection.message());
                        }
                        let mut blocked = request_confirmation(
                            &parts,
                            &action,
                            &summary,
                            &hit.element_label,
                            hit.reason,
                            context.type_preview_full.clone(),
                            context.secure_type_target,
                            binding,
                            hit.element_binding,
                        );
                        if had_token {
                            blocked.push_str(
                                "\nnote: the approved confirmation named a different target, so it \
                                 was NOT spent. The target under this action changed after the \
                                 user approved it.",
                            );
                        }
                        // Warnings ride the blocked error too: the
                        // auto-captured screenshot was persisted and audited,
                        // but the success path that would attach it never runs
                        // on a blocked action (the same gap the drag failure
                        // path works around).
                        for warning in &warnings {
                            blocked.push_str(&format!("\nwarning: {warning}"));
                        }
                        return Err(blocked);
                    }
                },
            }
        }

        // Last-moment re-check before the backend is engaged (stop flag — and
        // for input, the grant — still valid): after the gate there can be
        // time-consuming steps such as an automatic screenshot, during which
        // the user may revoke/stop (review finding). For input this gates the
        // injection; for observe it gates the backend start itself — a stop
        // landing while the run is still queued on the blocking pool must not
        // let the queued request lazily construct the platform backend and
        // capture after the stop has returned. The request's cancel flag
        // registers only at send time (it cannot inherit that stop), and the
        // emergency cleanup's Pending fast path does nothing for a
        // never-started handle (nothing to release), so this re-check is the
        // only gate left for the first screenshot. A mapless coordinate
        // observe (element_at_point) auto-captures above, ahead of this gate,
        // on the same accepted line as the input lane: that lane's automatic
        // screenshot also runs before its re-checks, and the capture is
        // discarded when the gate rejects.
        match action.class() {
            ActionClass::Input => {
                if let Err(rejection) = parts.shared.begin_input_action(&parts.session_id) {
                    return Err(rejection.message());
                }
            }
            ActionClass::Observe => {
                if let Err(rejection) = parts.shared.check_readonly() {
                    return Err(rejection.message());
                }
            }
        }

        // Capability check.
        let capabilities = parts
            .backend
            .capabilities()
            .map_err(|e| backend_error_text(&e))?;
        // The backend's recorded one-time target decision replaces the
        // preference fallback from here on (payloads, audit, markers).
        cache_display_target(&parts, &capabilities);
        // An admitted run re-asserts the session's display-target row from
        // the backend's recorded decision — unless a stop landed while this
        // run was in flight (see [`reassert_display_target_row`]).
        reassert_display_target_row(&parts);
        // The per-OS fidelity note (experimental private API, message-level
        // input, WM-less degradation) is model-facing but had no channel of
        // its own: surface it ONCE per session on the first action's
        // warnings, so the model learns the target's limits before it
        // wonders why an input did not land. The `surfaced` flag is set only
        // where the warnings are actually RENDERED (the success return
        // below): `warnings` never reach the model on an error return, so
        // flagging at push time could lose the note for the whole session
        // when the first action fails. On a virtual target the deeper
        // `notes` list rides the same once-per-session channel: those
        // residuals (broker-launched apps outside the session, --args
        // semantics, US-ANSI chords, WM-less degradation) were previously
        // reachable only through the handful of unsupported/error
        // interpolations, so the healthy path never delivered them.
        // Physical targets keep their shape unchanged: their `notes` stay on
        // the error interpolations only.
        let note_pending = {
            let state = parts.state.lock();
            !state.target_note_surfaced && !capabilities.target_note.is_empty()
        };
        // A disclosure the backend appends to the target note AFTER the
        // once-per-session channel spent itself (today: a mid-session macOS
        // park failure, whose honest overlap disclosure lands in the note
        // while captures already mix in the user's windows) must still
        // reach the model: the delta beyond the already-rendered note rides
        // this action's warnings. The snapshot advances only where the
        // warnings are actually rendered (the success return below).
        let note_delta = {
            let state = parts.state.lock();
            if let (true, Some(rendered)) = (
                state.target_note_surfaced,
                state.surfaced_target_note.as_deref(),
            ) {
                if let Some(extra) = capabilities.target_note.strip_prefix(rendered) {
                    // Prefix-stable append (the only shape today's producers
                    // emit): surface exactly the appended sentence. Full
                    // content (R15), not a byte length — the old
                    // strictly-greater length comparison went blind when a
                    // macOS re-park repair SHRANK the note and a later
                    // failure regrew it to the previously rendered length.
                    let extra = extra.trim_start_matches([';', ' ']);
                    (!extra.is_empty()).then(|| extra.to_string())
                } else if rendered != capabilities.target_note.as_str() {
                    // The note shrank (a re-park repaired the layout and
                    // cleared the disclosure) or was rewritten: re-render
                    // the whole note so the model's stale copy is replaced
                    // by current truth. A later recurrence then differs
                    // from THIS snapshot and surfaces as an append again.
                    Some(capabilities.target_note.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };
        let delta_pending = note_delta.is_some();
        if note_pending {
            warnings.push(capabilities.target_note.clone());
            if capabilities.target == DisplayTarget::Virtual && !capabilities.notes.is_empty() {
                warnings.push(capabilities.notes.clone());
            }
        } else if let Some(delta) = note_delta {
            warnings.push(delta);
        }
        match action.class() {
            ActionClass::Input if !capabilities.input => {
                return Err(format!(
                    "input injection is unsupported on this platform/session ({})",
                    capabilities.notes
                ));
            }
            ActionClass::Observe
                if matches!(
                    action,
                    ComputerUseAction::UiTree { .. } | ComputerUseAction::ElementAtPoint { .. }
                ) && !capabilities.ui_tree =>
            {
                return Err(format!(
                    "the accessibility tree is unsupported on this platform/session ({})",
                    capabilities.notes
                ));
            }
            // Without the screenshot capability bit, a platform without
            // capture would run screenshot all the way to the post-capture
            // degraded warning, and the model would get success=true with no
            // image.
            ActionClass::Observe
                if matches!(action, ComputerUseAction::Screenshot) && !capabilities.screenshot =>
            {
                return Err(format!(
                    "screen capture is unsupported on this platform/session ({})",
                    capabilities.notes
                ));
            }
            _ => {}
        }

        // Execute the action.
        let map = parts.state.lock().last_map.clone();
        let mut outcome = execute_action(&parts, &action, map.as_ref(), &mut warnings)
            .map_err(|e| backend_error_text(&e))?;

        // Follow-up screenshot.
        if action.attaches_screenshot() {
            match &action {
                ComputerUseAction::Screenshot => {}
                ComputerUseAction::Wait { ms } => {
                    std::thread::sleep(std::time::Duration::from_millis(*ms));
                }
                _ => std::thread::sleep(std::time::Duration::from_millis(POST_ACTION_SETTLE_MS)),
            }
            // For an observe action the follow-up capture IS the outcome (a
            // wait's whole product is the fresh screenshot), and its sleep is
            // the one wide pre-capture window in this lane (up to MAX_WAIT_MS
            // versus the fixed settle above), so a stop landing during the
            // sleep must still abort the capture. Input actions keep their
            // established design: their capture documents the aftermath of an
            // already-gated injection, with only the narrow settle in
            // between.
            if action.class() == ActionClass::Observe {
                if let Err(rejection) = parts.shared.check_readonly() {
                    return Err(rejection.message());
                }
            }
            match capture_and_store(&parts, &workspace) {
                Ok(fresh) => {
                    if !outcome.is_empty() {
                        outcome.push('\n');
                    }
                    outcome.push_str(&shot_result_text(&fresh, resolved_display_target(&parts)));
                    shot = Some(fresh);
                }
                Err(error) if matches!(action, ComputerUseAction::Screenshot) => {
                    // For Screenshot, the capture IS the action: a failure
                    // must propagate. Degrading to a warning would give the
                    // model success with an empty result and an ok audit
                    // record — diverging from the auto-capture path (hard
                    // failure, the action does not run) and leaving the
                    // "capture unavailable" platform state invisible to the
                    // model.
                    return Err(backend_error_text(&error));
                }
                Err(error) => warnings.push(format!("post-action capture failed: {error}")),
            }
        }

        for warning in &warnings {
            outcome.push_str(&format!("\nwarning: {warning}"));
        }
        // The note reached the model with this outcome — exactly-once
        // guaranteed from here (a first action that failed above left the
        // flag unset, so the next action retries the surfacing). The delta
        // snapshot advances the same way: a delta rendered on a failed
        // action is re-rendered by the next one.
        {
            let mut state = parts.state.lock();
            if note_pending {
                state.target_note_surfaced = true;
            }
            if note_pending || delta_pending {
                state.surfaced_target_note = Some(capabilities.target_note.clone());
            }
        }
        Ok(outcome)
    })();

    let duration_ms = started.elapsed().as_millis() as u64;
    // Single informational audit record per call. Redaction lives here: typed
    // text is never logged (length only), typing-form chords log a key count,
    // screenshots log SHA-256 + path. Fail-open: a write failure warns and
    // NEVER blocks the action, for every action class.
    let mut record = AuditRecord::new(
        &parts.session_id,
        action.name(),
        consent_label(&action, confirmed_t3),
    );
    // Which display the action targeted ("virtual" | "physical"): the
    // backend's recorded creation decision, or the resolved preference
    // fallback for runs that never reached a capability read.
    record.with_display_target(resolved_display_target(&parts).as_str());
    match &action {
        // Privacy: never log typed text — length only.
        ComputerUseAction::Type { text } => {
            record.with_target(&format!("typed {} characters", text.chars().count()));
        }
        // Typing-form chords (any character key among the keys, any
        // modifier/order arrangement) could spell out a password in
        // <=4-character chunks — mixed chords like `p+a+s+Return` type their
        // characters too — so only the key count is logged, never the
        // characters themselves. Chords made only of named keys (Return,
        // ctrl+Delete, F5, …) inject no characters and keep plaintext
        // (readability for the user wins).
        ComputerUseAction::KeyChord { keys, .. } | ComputerUseAction::HoldKey { keys, .. }
            if is_typed_text_chord(keys) =>
        {
            let count = keys.iter().filter(|key| !key.is_modifier()).count();
            let plural = if count == 1 { "" } else { "s" };
            record.with_target(&format!(
                "pressed {count} key{plural}{}",
                held_key_suffix(&action)
            ));
        }
        ComputerUseAction::KeyChord { chord, .. } => {
            record.with_target(&format!("keys: {chord}"));
        }
        // hold_key logs the held duration (review finding: the audit could
        // not previously answer "how long was it held").
        ComputerUseAction::HoldKey { chord, ms, .. } => {
            record.with_target(&format!("keys: {chord} held for {ms}ms"));
        }
        ComputerUseAction::Click { at, button, count } => {
            record.with_target(&format!("{} click x{count} at {at:?}", button.as_str()));
        }
        ComputerUseAction::Scroll {
            direction,
            amount,
            at,
        } => {
            record.with_target(&format!(
                "scroll {} x{amount} at {at:?}",
                direction.as_str()
            ));
        }
        ComputerUseAction::MouseDown { button } => {
            record.with_target(&format!("{} mouse button down", button.as_str()));
        }
        ComputerUseAction::MouseUp { button } => {
            record.with_target(&format!("{} mouse button up", button.as_str()));
        }
        ComputerUseAction::Drag { start, end } => {
            record.with_target(&format!("drag {start:?} -> {end:?}"));
        }
        ComputerUseAction::MouseMove { x, y } | ComputerUseAction::ElementAtPoint { x, y } => {
            record.with_target(&format!("({x}, {y})"));
        }
        // open: the program name is model-supplied FREE TEXT, so — like
        // chords and typed text — it never enters the audit (a secret can be
        // chunk-spelled across open calls the same way it can across chord
        // chunks). The arg COUNT and the display_target field carry the
        // audit story; the model-facing receipt still names the program.
        ComputerUseAction::Open { args, .. } => {
            record.with_target(&format!("open with {} args", args.len()));
        }
        _ => {}
    }
    if let Some(shot) = &shot {
        record.with_screenshot(&shot.png, &shot.abs_path);
    }
    let record = match &body {
        Ok(_) => record.finish("ok", None, duration_ms),
        // The full T3 "confirmation required" message carries the element
        // label, so the audit error field holds only the stable code; every
        // other error path keeps the full message (the model always receives
        // the full message).
        Err(message) => {
            let audit_error = if message.starts_with(T3_CONFIRM_REQUIRED_ERROR) {
                Some(T3_CONFIRM_REQUIRED_ERROR.to_string())
            } else {
                Some(message.clone())
            };
            record.finish("error", audit_error, duration_ms)
        }
    };
    if let Err(error) = AuditLog::for_session(&parts.session_id).and_then(|log| log.append(&record))
    {
        eprintln!("[computer_use] audit append failed: {error}");
    }

    match body {
        Ok(text) => {
            let mut result = ToolResult::success(text);
            if let Some(shot) = &shot {
                result = with_image_metadata(result, shot);
            }
            result
        }
        Err(message) => ToolResult::error(message),
    }
}

/// The audit target suffix for hold_key (held duration); empty for non-hold
/// actions.
fn held_key_suffix(action: &ComputerUseAction) -> String {
    match action {
        ComputerUseAction::HoldKey { ms, .. } => format!(" held for {ms}ms"),
        _ => String::new(),
    }
}

/// Chord rendering in keyboard action summaries. A chord containing
/// character keys is "typing" (it can spell out password chunks); characters
/// never enter the summary/confirm event — only modifiers and named keys are
/// rendered, with characters expressed as a count (`ctrl+s` →
/// `ctrl + 1 character`, `esc+h+u+n` → `esc + 3 characters`); pure named-key
/// chords keep the readable original (`ctrl+Delete`). Same classification
/// boundary as the audit's counting rule ([`is_typed_text_chord`]). The
/// summary is bound into the approval token (both mint and spend go through
/// [`action_summary`]), so it must depend only on the action itself and never
/// on focus state.
fn summarize_chord(keys: &[Key], chord: &str) -> String {
    let chars = keys
        .iter()
        .filter(|key| matches!(key, Key::Char(_)))
        .count();
    if chars == 0 {
        return chord.to_string();
    }
    let named: Vec<String> = keys
        .iter()
        .filter_map(|key| (!matches!(key, Key::Char(_))).then(|| key.canonical_name()))
        .collect();
    let unit = if chars == 1 {
        "character"
    } else {
        "characters"
    };
    if named.is_empty() {
        format!("{chars} {unit}")
    } else {
        format!("{} + {chars} {unit}", named.join(" + "))
    }
}

/// Action summary: a purely human-readable parameter summary
/// (`left click x1 at Some((5, 6))`, `type 3 characters`, …). The summary is
/// shown to the user (informed approval) and bound into the approval token;
/// typed content never appears (Type records only the character count; chords
/// record only named keys and a character count via [`summarize_chord`]).
fn action_summary(action: &ComputerUseAction) -> String {
    match action {
        ComputerUseAction::Click { button, count, at } => {
            format!("{} click x{count} at {at:?}", button.as_str())
        }
        ComputerUseAction::Drag { start, end } => format!("drag {start:?} -> {end:?}"),
        ComputerUseAction::Type { text } => {
            format!("type {} characters", text.chars().count())
        }
        ComputerUseAction::KeyChord { chord, keys, .. } => {
            format!("key {}", summarize_chord(keys, chord))
        }
        ComputerUseAction::HoldKey {
            chord, keys, ms, ..
        } => {
            format!("hold {} for {ms}ms", summarize_chord(keys, chord))
        }
        // Scroll is excluded by the T3 gate, so this arm is currently unreachable.
        ComputerUseAction::Scroll {
            direction,
            amount,
            at,
        } => format!("scroll {} x{amount} at {at:?}", direction.as_str()),
        ComputerUseAction::MouseDown { button } => format!("{} mouse down", button.as_str()),
        ComputerUseAction::MouseUp { button } => format!("{} mouse up", button.as_str()),
        // T3 gate excludes hover/scroll/observe, so these arms are currently unreachable.
        ComputerUseAction::MouseMove { x, y } => format!("mouse_move to ({x}, {y})"),
        // The program name (never the args) identifies the launch for the
        // user; `open` is T3-excluded, so this summary is informational.
        ComputerUseAction::Open { program, .. } => format!("open {program}"),
        other => other.name().to_string(),
    }
}

fn execute_action(
    parts: &Parts,
    action: &ComputerUseAction,
    map: Option<&ScaleMap>,
    warnings: &mut Vec<String>,
) -> Result<String, ComputerUseError> {
    let backend = &parts.backend;
    // Last-instant gate: the guard was verified at run() start and again at
    // mint/spend, but a stop/disable/revoke landing after that point — or a
    // request registering with the backend worker after the stop latched the
    // cancel registry — would otherwise still inject, landing AFTER the
    // emergency releases that the same stop issued. This gates the first
    // injecting request; composite actions that inject through a second
    // backend request (a coordinate click/scroll: move_to, then the
    // click/scroll) re-verify between the two, because a stop landing in
    // that gap cancels only the finished move and this gate is stale by
    // then. Observe actions never touch hardware and skip this.
    if action.class() == ActionClass::Input {
        if let Err(rejection) = parts.shared.begin_input_action(&parts.session_id) {
            return Err(ComputerUseError::failed(rejection.message()));
        }
    }
    match action {
        ComputerUseAction::Screenshot => Ok(String::new()),
        ComputerUseAction::CursorPosition => {
            // cursor_position reports input coordinates directly (points on
            // macOS, physical pixels on Windows/X11) — no device round trip.
            let (ix, iy) = backend.cursor_position()?;
            match map {
                Some(m) => {
                    // Input rects are disjoint per monitor, so this
                    // containment gate is exact even on mixed-DPI setups; a
                    // point outside the rect belongs to another monitor and
                    // mapping it through this map would report junk, so the
                    // raw input coordinates are returned with a warning
                    // instead of a screenshot-space conversion.
                    if !m.contains_input_point(ix, iy) {
                        warnings.push(format!(
                            "cursor is outside the captured monitor (origin ({}, {}), {}x{}); \
                             screenshot-space coordinates are unavailable for it this turn",
                            m.origin_x, m.origin_y, m.dev_w, m.dev_h
                        ));
                        return Ok(format!(
                            "cursor is at ({ix}, {iy}) in global input coordinates, which is \
                             outside the captured monitor"
                        ));
                    }
                    let (sx, sy) = m.input_to_shot(ix, iy);
                    Ok(format!("cursor is at ({sx}, {sy}) in screenshot space"))
                }
                None => Ok(format!(
                    "cursor is at ({ix}, {iy}) in global input coordinates; no screenshot has been taken this session, so screenshot-space coordinates are unavailable"
                )),
            }
        }
        ComputerUseAction::Wait { ms } => Ok(format!("waited {ms} ms")),
        ComputerUseAction::UiTree { opts } => backend.ui_tree(*opts).map(|tree| {
            // a11y rectangles are the backend's input/screen coordinates, not
            // this tool contract's screenshot pixel space (review finding:
            // screenshots with a long edge over 1440px get downsampled and
            // the two coordinate sets silently drift by the scale factor —
            // cursor_position has a conversion, the tree output did not). The
            // tool layer cannot parse the backend text, so declare the space
            // explicitly and keep the model from treating the rectangles as
            // screenshot coordinates.
            format!(
                "{tree}\n(bounds above are global input/screen coordinates, not screenshot \
                 pixel space; use element_at_point on the screenshot to resolve screenshot-space \
                 positions)"
            )
        }),
        ComputerUseAction::ElementAtPoint { x, y } => {
            let Some(m) = map else {
                return Err(ComputerUseError::failed(
                    "no screenshot has been taken this session; take one before element_at_point",
                ));
            };
            let targets = resolve_targets(m, &[(*x, *y)], warnings)?;
            let Some(&(ix, iy)) = targets.first() else {
                return Err(ComputerUseError::failed("missing element target"));
            };
            match backend.element_at_point(ix, iy)? {
                Some(element) => {
                    // Convert both corners and take the bounding box: bounds
                    // are the backend's input/screen coordinates and must be
                    // converted back to screenshot pixel space before being
                    // handed to the model (contract consistency, review
                    // finding — otherwise a click using bounds as screenshot
                    // coordinates would land offset by the scale factor).
                    let (ax, ay) = m.input_to_shot(element.x, element.y);
                    let (bx, by) = m.input_to_shot(
                        element.x.saturating_add(element.width),
                        element.y.saturating_add(element.height),
                    );
                    let (rx, ry) = (ax.min(bx), ay.min(by));
                    let (rw, rh) = (ax.abs_diff(bx), ay.abs_diff(by));
                    // The backends carry raw role/name so screening sees them
                    // untruncated; bound them here, at the model-facing render.
                    Ok(format!(
                        "element at ({x}, {y}): role=\"{}\" name=\"{}\" bounds=({rx}, {ry}, \
                         {rw}x{rh}) in screenshot space secure={}",
                        platform::sanitize_name(&element.role, platform::MAX_NAME_CHARS),
                        platform::sanitize_name(&element.name, platform::MAX_NAME_CHARS),
                        element.secure
                    ))
                }
                None => Ok(format!("no accessibility element found at ({x}, {y})")),
            }
        }
        ComputerUseAction::MouseMove { x, y } => {
            let Some(m) = map else {
                return Err(ComputerUseError::failed(
                    "no screenshot has been taken this session; take one before mouse_move",
                ));
            };
            let targets = resolve_targets(m, &[(*x, *y)], warnings)?;
            let Some(&(ix, iy)) = targets.first() else {
                return Err(ComputerUseError::failed("missing move target"));
            };
            backend.move_to(ix, iy)?;
            Ok(format!("mouse moved to ({x}, {y}) in screenshot space"))
        }
        ComputerUseAction::Scroll {
            direction,
            amount,
            at,
        } => {
            if let Some((x, y)) = at {
                let Some(m) = map else {
                    return Err(ComputerUseError::failed(
                        "no screenshot has been taken this session; take one before scroll with coordinates",
                    ));
                };
                let targets = resolve_targets(m, &[(*x, *y)], warnings)?;
                let Some(&(ix, iy)) = targets.first() else {
                    return Err(ComputerUseError::failed("missing scroll target"));
                };
                backend.move_to(ix, iy)?;
                // Second injection request of a composite action: once
                // move_to's request completed, the entry gate above is stale
                // and the scroll request below would not inherit a stop that
                // landed in between (its cancel flag registers only now).
                // Re-verify so a post-stop scroll cannot inject.
                if let Err(rejection) = parts.shared.begin_input_action(&parts.session_id) {
                    return Err(ComputerUseError::failed(rejection.message()));
                }
            }
            backend.scroll(*direction, *amount)?;
            Ok(format!(
                "scrolled {} {} click(s)",
                direction.as_str(),
                amount
            ))
        }
        ComputerUseAction::Click { button, count, at } => {
            if let Some((x, y)) = at {
                let Some(m) = map else {
                    return Err(ComputerUseError::failed(
                        "no screenshot has been taken this session; take one before clicking with coordinates",
                    ));
                };
                let targets = resolve_targets(m, &[(*x, *y)], warnings)?;
                let Some(&(ix, iy)) = targets.first() else {
                    return Err(ComputerUseError::failed("missing click target"));
                };
                backend.move_to(ix, iy)?;
                // Second injection request of a composite action: once
                // move_to's request completed, the entry gate above is stale
                // and the click request below would not inherit a stop that
                // landed in between (its cancel flag registers only now).
                // Re-verify so a post-stop click cannot land after the
                // stop's emergency releases.
                if let Err(rejection) = parts.shared.begin_input_action(&parts.session_id) {
                    return Err(ComputerUseError::failed(rejection.message()));
                }
            }
            backend.click(*button, *count)?;
            // A click completes press+release inside the backend: like drag,
            // it leaves no button physically held — clear the
            // fallback-release flag so a later failed drag never "releases" a
            // button this tool did not hold (a left_click after
            // left_mouse_down used to leave the flag latched).
            if *button == MouseButton::Left {
                parts.state.lock().mouse_buttons_held = false;
            }
            let verb = match count {
                2 => "double-",
                3 => "triple-",
                _ => "",
            };
            Ok(format!("{} {verb}click executed", button.as_str()))
        }
        ComputerUseAction::MouseDown { button } => {
            backend.mouse_down(*button)?;
            // The held state serves only the fallback release after a failed
            // drag (the failure path does not set it: a failed press =
            // physically not held); screening no longer uses it.
            if *button == MouseButton::Left {
                parts.state.lock().mouse_buttons_held = true;
            }
            Ok(format!("{} mouse button is down", button.as_str()))
        }
        ComputerUseAction::MouseUp { button } => {
            backend.mouse_up(*button)?;
            if *button == MouseButton::Left {
                parts.state.lock().mouse_buttons_held = false;
            }
            Ok(format!("{} mouse button is up", button.as_str()))
        }
        ComputerUseAction::Drag { start, end } => {
            let Some(m) = map else {
                return Err(ComputerUseError::failed(
                    "no screenshot has been taken this session; take one before dragging",
                ));
            };
            let targets = resolve_targets(m, &[*start, *end], warnings)?;
            let (Some(&from), Some(&to)) = (targets.first(), targets.get(1)) else {
                return Err(ComputerUseError::failed("missing drag targets"));
            };
            if let Err(error) = backend.drag(from, to) {
                // Tool-layer safety net: a failed drag must not leave the
                // physical left button stuck held when this tool pressed it
                // earlier. Best-effort release (platform backends may clean
                // up on their own; its error is ignored — if it also fails,
                // revoke/stop/tool-drop cleanup retries via the control
                // lane). The note rides on the error message because the
                // warnings vector is dropped on the error path in `run`.
                let mut note = String::new();
                if parts.state.lock().mouse_buttons_held
                    && backend.mouse_up(MouseButton::Left).is_ok()
                {
                    parts.state.lock().mouse_buttons_held = false;
                    note.push_str(
                        "\nwarning: the drag failed; a best-effort mouse-button release was \
                         issued — verify no button is stuck held",
                    );
                }
                return Err(ComputerUseError::failed(format!("{error}{note}")));
            }
            // A drag completes press + release internally: regardless of
            // prior state, the left button is no longer held.
            parts.state.lock().mouse_buttons_held = false;
            Ok(format!("dragged from {start:?} to {end:?}"))
        }
        ComputerUseAction::Type { text } => {
            backend.type_text(text)?;
            Ok(format!("typed {} chars", text.chars().count()))
        }
        ComputerUseAction::KeyChord { keys, chord } => {
            backend.key_chord(keys)?;
            Ok(format!("pressed {chord}"))
        }
        ComputerUseAction::HoldKey { keys, chord, ms } => {
            backend.hold_key(keys, *ms)?;
            Ok(format!("held {chord} for {ms} ms"))
        }
        ComputerUseAction::Open { program, args } => {
            // Virtual backends spawn inside their display; physical backends
            // answer the trait's explicit Unsupported (opening apps on the
            // user's real desktop is out of scope) — the model sees the
            // honest error instead of a silent desktop launch. The receipt
            // says "spawned", not "opened": on Linux/Windows the pid is
            // reported right after a successful spawn with no
            // window-appeared check (the macOS stack is the one that fails
            // an exited-without-window launch explicitly), so "opened"
            // would overclaim; the attached follow-up screenshot is the
            // truthful verdict.
            let pid = backend.open_app(program, args)?;
            let args_note = if args.is_empty() {
                String::new()
            } else {
                format!(" with {} arg(s)", args.len())
            };
            // pid 0 is the macOS stack's honest "launched, owner unknown"
            // (window recovery found nothing to attribute): the semantics
            // ride the once-per-session target note, and a model that
            // missed that note would treat 0 as a handle — state it in the
            // receipt itself (R15).
            let unowned_note = if pid == 0 {
                "; pid 0 means this app's window never appeared inside the \
                 display — it cannot be driven and its window stays \
                 wherever it opened"
            } else {
                ""
            };
            Ok(format!(
                "spawned {program}{args_note} (pid {pid}){unowned_note}; a follow-up \
                 screenshot is attached but apps often take a moment to draw \
                 their window — take a fresh screenshot once the app settles \
                 (a program that exits immediately — e.g. a CLI tool — \
                 simply will not appear)"
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// ToolSpec
// ---------------------------------------------------------------------------

#[async_trait]
impl ToolSpec for ComputerUseTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn description(&self) -> &str {
        "Control a desktop: take screenshots, move/click the mouse, type text, \
         press key chords, scroll, open applications (`open`, spawns a program \
         directly — no shell — virtual-display targets only), and inspect the accessibility tree. \
         When the app reports an isolated virtual display is in use (see the \
         [target: …] line on every screenshot), everything you do happens on that \
         isolated display and the user's real desktop is untouched — launch apps \
         there with `open` and work freely; this isolation is subject to the \
         platform residuals the target note discloses (e.g. on macOS an app \
         you just opened passes through the user's desktop while it is being \
         moved in), and per-action screening cannot read the isolated \
         display (its apps cannot be screened, so nothing there is \
         confirmed against a user-facing control). When the target is the user's real \
         desktop, every action is visible to the user: minimize disruption, keep \
         actions to the minimum the task needs, and say what you are about to do \
         before doing it. \
         Coordinates are ALWAYS in the pixel space of the last screenshot this tool returned \
         (origin top-left); take a screenshot first and reuse its coordinate space. \
         Every action that touches the mouse or keyboard (mouse_move, scroll, clicks, keys, \
         typing, open) requires a per-session grant from the user; the grant stays valid until \
         revoked or until the session's engine is recycled (e.g. reaped after idle), in \
         which case the user must grant again. Before an activation action runs (clicks, drags, mouse down/up, typing, \
         keys), its target is screened against a consequential-control denylist \
         (financial/send/delete/submit/consent categories) and password fields; a hit \
         pauses the action until the user confirms it via confirm_id. Screening is \
         best-effort category detection over the accessibility tree: when the target \
         cannot be read or screening is unavailable, the action still executes without \
         confirmation (open is not screened at all: nothing is on screen yet — its \
         arguments are spawned directly, never through a shell). Know the limits before relying on the screen: the category terms \
         match English, Chinese (Simplified/Traditional), and Japanese labels only — in \
         other UI languages only the language-independent password-field screen applies. \
         The target is located by a point lookup with no window identity, so a \
         same-label or same-position window/UI change between screening (or approval) \
         and injection cannot be detected; on Linux a target whose accessibility role \
         cannot be read may still require confirmation as a precaution. An approved \
         confirmation is bound to the exact action content AND to the target the user \
         was shown, and the target is re-screened when you spend the confirm_id. If the \
         target is now a DIFFERENT consequential control, the action does not run and a \
         fresh confirmation is raised for whatever is there now (your unspent confirm_id \
         stays valid for a correct retry). If the re-screen comes back clear — nothing \
         consequential under the target — the action executes and the confirm_id is \
         spent, because an unapproved run of the same action would have executed \
         without any dialog; note a target that cannot be read at all screens the same \
         as a clear one, and every unreadable location binds identically, so the \
         replay only detects a readable target appearing. The CONTENT you type is never \
         screened, and \
         key chords are not screened for destructiveness. On macOS and Windows the \
         accessibility tree walk is additionally capped below the max_depth/max_nodes \
         arguments (24 levels / 2000 nodes). Observation (screenshots, ui_tree) reads on-screen \
         and focused-window content while the feature is enabled, which may include \
         private information. After actions that change the screen a fresh screenshot is \
         attached; if it is not visible, call image_analyze with the returned attachments \
         path."
    }

    fn input_schema(&self) -> Value {
        // Bounds parity: every parser-enforced cap is declared here so a
        // schema-validating client sees the same domain the parser enforces
        // (ms 1-30000 shared by wait/hold_key, amount 1-100, text ≤ 10000
        // chars for type, max_depth ≤ 64, max_nodes ≤ 10000). The `text`
        // maxLength covers type; chord text for key/hold_key is capped
        // tighter (128) at parse time but shares this field, so the schema
        // declares only the common bound.
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": SUPPORTED_ACTIONS,
                    "description": "The computer action to perform"
                },
                "x": { "type": "integer", "minimum": 0, "description": "X coordinate in the last screenshot's pixel space" },
                "y": { "type": "integer", "minimum": 0, "description": "Y coordinate in the last screenshot's pixel space" },
                "start_x": { "type": "integer", "minimum": 0, "description": "Drag start X (left_click_drag only)" },
                "start_y": { "type": "integer", "minimum": 0, "description": "Drag start Y (left_click_drag only)" },
                "text": { "type": "string", "maxLength": 10000, "description": "Text to type (type) or xdotool-style key chord like \"ctrl+s\", \"Return\", \"alt+Tab\" (key, hold_key)" },
                "ms": { "type": "integer", "minimum": 1, "maximum": 30000, "description": "Duration in milliseconds (wait, hold_key; hold_key requires 1-30000)" },
                "direction": { "type": "string", "enum": ["up", "down", "left", "right"], "description": "Scroll direction (scroll)" },
                "amount": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Scroll wheel clicks, 1-100 (scroll)" },
                "max_depth": { "type": "integer", "minimum": 1, "maximum": 64, "description": "Max accessibility tree depth (ui_tree)" },
                "max_nodes": { "type": "integer", "minimum": 1, "maximum": 10000, "description": "Max accessibility tree nodes (ui_tree)" },
                "program": { "type": "string", "maxLength": 1024, "description": "Program to launch, spawned directly without a shell (open): a bare name resolved by the platform's usual search order, or an absolute path" },
                "args": { "type": "array", "items": { "type": "string", "maxLength": 4096 }, "maxItems": 64, "description": "Arguments passed verbatim to the program (open)" },
                "confirm_id": { "type": "string", "description": "Single-use user-confirmation token for a blocked consequential action" }
            },
            "required": ["action"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        // The engine currently ignores this metadata and consent gating runs
        // inside the tool; marked Required so it composes when a future
        // engine supports it.
        ApprovalRequirement::Required
    }

    fn supports_parallel(&self) -> bool {
        // The mouse/keyboard is a globally exclusive resource: never parallel
        // with other tools.
        false
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        // Parse failures leave a trace too: rejections at the parse stage
        // would otherwise have zero audit, letting the model probe the action
        // surface without a trace. Only the truncated action name and error
        // are recorded; the full call arguments (which may contain typed
        // text) are never recorded.
        let parsed = match parse_action(&input) {
            Ok(parsed) => parsed,
            Err(error) => {
                // The audit record holds only stable shape information: an
                // unknown action name is recorded as "unknown"
                // (model-controlled text does not enter the log), and the
                // error text itself is guaranteed by the parse layer not to
                // echo model input (see parse_action's unknown-action /
                // not-accepted branches and parse_key_chord's shape errors).
                let raw_action = input.get("action").and_then(Value::as_str);
                let audit_target = match raw_action {
                    Some(name) if SUPPORTED_ACTIONS.contains(&name) => {
                        format!("action:{name}")
                    }
                    Some(_) => "action:unknown".to_string(),
                    None => "action:missing".to_string(),
                };
                let error_text = error.to_string();
                let mut record = AuditRecord::new(&self.parts.session_id, "unparseable", "n/a");
                record.with_target(&audit_target);
                // Display target like every other boundary record (the cache
                // is unprimed this early, so this is the resolved-preference
                // answer — the audit vocabulary treats absent/unknown as
                // distinct, and the resolved preference is what the session
                // WOULD target).
                record.with_display_target(resolved_display_target(&self.parts).as_str());
                let record = record.finish(
                    "rejected",
                    Some(format!(
                        "parse failed: {}",
                        error_text.chars().take(160).collect::<String>()
                    )),
                    0,
                );
                // The append (with its fsync) runs on the blocking pool with
                // an inline await — the same discipline as
                // `audit_rejected_call`: the parse failure is the most
                // model-triggerable rejection path, so a slow disk must not
                // park an async runtime worker here either (R10 review: this
                // site was still inline). The synchronous position in the
                // call keeps the per-session JSONL order deterministic.
                let session_id = self.parts.session_id.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    match AuditLog::for_session(&session_id) {
                        Ok(log) => {
                            if let Err(audit_error) = log.append(&record) {
                                eprintln!("[computer_use] audit append failed: {audit_error}");
                            }
                        }
                        Err(log_error) => {
                            eprintln!(
                                "[computer_use] audit log unavailable for unparseable call: \
                             {log_error}"
                            );
                        }
                    }
                })
                .await
                .ok();
                return Err(error);
            }
        };

        // Capabilities BEFORE the gate for Input actions (review finding:
        // prompting for the grant before reporting the platform unsupported
        // would induce the user to authorize a platform that can never be
        // used, while burning a grant budget for nothing) — the read's
        // target cache is what the consent gate's grant event and the
        // rejected-call audits below report, so a rejection carries the
        // backend's recorded decision instead of the live probe's answer
        // (which a mid-run stack install could flip). Observe actions read
        // AFTER the gate, inside `run`: the capability read constructs the
        // backend (a real side effect — on a virtual-target host it spawns
        // the whole display stack), and a stale-engine Observe call must
        // not boot that stack just to be told the feature is off; its
        // rejection audit carries the resolved-preference answer like every
        // other pre-backend boundary record (see audit.rs).
        let input_class = parsed.action.class() == ActionClass::Input;
        if input_class {
            // Gate PRECHECK before the capability read (R9 review): the
            // read constructs the backend — on a virtual-target host its
            // factory spawns the whole display stack and registers the
            // session "active" — so a call the gate refuses anyway (switch
            // off, stop pending, a dialog up) must not pay that cost nor
            // leave the settings hint saying a session is live.
            // GrantRequired is the one shape that still needs the read
            // first: the grant event carries the backend's recorded
            // decision, and the unsupported-platform answer must precede
            // the grant prompt (the finding this precheck deliberately
            // preserves). `begin_input_action` is a read-only check (its
            // doc), so the real gate below re-runs it unchanged.
            // (No `InputBusy` arm: that rejection comes solely from
            // `lock_physical_input` in run(), which this precheck never
            // calls — the R9 wave's arm here was unreachable.)
            if let Err(
                rejection @ (GuardRejection::Disabled
                | GuardRejection::Stopped
                | GuardRejection::ConfirmationPending
                | GuardRejection::GrantDialogPending),
            ) = self.parts.shared.begin_input_action(&self.parts.session_id)
            {
                audit_rejected_call(
                    &self.parts,
                    &parsed.action,
                    rejection_name(rejection),
                    &rejection.message(),
                )
                .await;
                return Ok(ToolResult::error(rejection.message()));
            }
            let backend = self.parts.backend.clone();
            // The two failure arms below audit on the blocking pool before
            // returning (the read itself failed — the call dies here without
            // reaching run()'s audit, so leave the trace the audit
            // discipline promises instead of a silent short-circuit); the
            // match shape (not map_err) is what lets those audits await.
            let read =
                match tauri::async_runtime::spawn_blocking(move || backend.capabilities()).await {
                    Ok(Ok(capabilities)) => capabilities,
                    Ok(Err(error)) => {
                        let text = backend_error_text(&error);
                        audit_failed_call(&self.parts, &parsed.action, &text).await;
                        return Err(ToolError::execution_failed(text));
                    }
                    Err(error) => {
                        let text = format!("computer use worker join failed: {error}");
                        audit_failed_call(&self.parts, &parsed.action, &text).await;
                        return Err(ToolError::execution_failed(text));
                    }
                };
            // Cache the backend's recorded target at the earliest read so
            // the grant payload below (and every later surface) reflects
            // the backend's actual decision, not the preference fallback.
            cache_display_target(&self.parts, &read);
            // The R9 rule at action granularity: the unsupported answer must
            // precede the grant prompt. `open` is virtual-only by trait
            // contract (physical backends answer the trait default), so a
            // physical-target session must be refused here — before the
            // consent gate — instead of spending a real-desktop grant
            // dialog on an action that structurally cannot run.
            if matches!(parsed.action, ComputerUseAction::Open { .. })
                && read.target == DisplayTarget::Physical
            {
                let text = "open is only available on a virtual display target — \
                            this session's backend drives the user's real desktop, \
                            so launching applications into it is out of scope"
                    .to_string();
                audit_rejected_call(
                    &self.parts,
                    &parsed.action,
                    "capability:open-physical-target",
                    &text,
                )
                .await;
                return Ok(ToolResult::error(text));
            }
            if !read.input {
                // Audit the rejected call (review finding: capability
                // rejections had no trace at all, inconsistent with the
                // established audit treatment of gate rejections).
                audit_rejected_call(
                    &self.parts,
                    &parsed.action,
                    "capability:input-unsupported",
                    &format!(
                        "input injection is unsupported on this platform/session ({})",
                        read.notes
                    ),
                )
                .await;
                return Ok(ToolResult::error(format!(
                    "input injection is unsupported on this platform/session ({})",
                    read.notes
                )));
            }
        }

        // Consent gate (synchronous fast path; emits an event on rejection).
        // Observe-class actions only need the switch on and no stop
        // (check_readonly).
        let gate = match parsed.action.class() {
            ActionClass::Observe => self.parts.shared.check_readonly(),
            ActionClass::Input => self.parts.shared.begin_input_action(&self.parts.session_id),
        };
        if let Err(rejection) = gate {
            if rejection == GuardRejection::GrantRequired {
                // Server truth for the consent UI: mark the request before
                // broadcasting so a get_status in any window already sees
                // the pending grant (cleared by grant/revoke/stop/disable).
                // The serialized variant keeps the dialog from appearing
                // under another session's in-flight injection (see the
                // guard method's doc); it runs on the blocking pool because
                // the bounded lock wait must not stall an executor worker.
                let shared = std::sync::Arc::clone(&self.parts.shared);
                let session_id = self.parts.session_id.clone();
                // The only failure mode of the join is the task being
                // cancelled with the runtime shutting down — the action's
                // GrantRequired error still goes out, and the retry path
                // re-asks. Nothing useful to do with the error here.
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    shared.mark_grant_requested_serialized(&session_id);
                })
                .await;
                self.parts.events.emit(
                    EVENT_GRANT_REQUIRED,
                    json!({
                        "session_id": self.parts.session_id,
                        // Which display the grant would authorize control of
                        // ("virtual" | "physical", frozen): the dialog copy is
                        // target-aware; old frontends tolerate the missing
                        // field (fail-neutral).
                        "target": resolved_display_target(&self.parts).as_str(),
                    }),
                );
            }
            // Audit the rejected call (non-input classes may degrade to
            // eprintln; the action did not run and there is no injection
            // consequence, but the error is never swallowed silently —
            // review finding).
            audit_rejected_call(
                &self.parts,
                &parsed.action,
                rejection_name(rejection),
                &rejection.message(),
            )
            .await;
            return Ok(ToolResult::error(rejection.message()));
        }

        let workspace = resolve_workspace(context, &self.parts.session_id);
        let parts = self.parts.clone();
        let audit_session = self.parts.session_id.clone();
        let audit_action = parsed.action.name();
        // The join-failure audit record must carry the display target like
        // every other boundary record — this is the "action may have
        // executed" path, exactly where knowing the target matters most.
        // `resolved_display_target` reads the cached backend decision (the
        // capability read in `execute` already primed it for Input actions);
        // for the rare pre-cache panic it falls back to the probe/preference
        // answer, which is still better than an untargeted record.
        let audit_target = resolved_display_target(&self.parts).as_str().to_string();
        let joined =
            tauri::async_runtime::spawn_blocking(move || run(parts, parsed, workspace)).await;
        match joined {
            Ok(value) => Ok(value),
            Err(error) => {
                // Worker join failure = run panicked, possibly after injection
                // happened: this is the only "action may have executed" path
                // with no audit record (review finding). The audit error
                // field uses fixed copy — the panic text carried by JoinError
                // may embed input content (e.g. a chord fragment from a char
                // boundary panic) and must not be echoed into the JSONL; the
                // full error goes only to the model. The append (with its
                // fsync) runs on the blocking pool with an inline await —
                // the match shape (not map_err) is what lets it await (the
                // R10 review closed the last inline-on-worker append site).
                let mut record = AuditRecord::new(&audit_session, audit_action, "input");
                record.with_display_target(audit_target.as_str());
                let record = record.finish("error", Some("worker join failed".to_string()), 0);
                let session_id = audit_session.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    if let Err(audit_error) =
                        AuditLog::for_session(&session_id).and_then(|log| log.append(&record))
                    {
                        eprintln!("[computer_use] audit append failed: {audit_error}");
                    }
                })
                .await
                .ok();
                Err(ToolError::execution_failed(format!(
                    "computer use worker join failed: {error}"
                )))
            }
        }
    }
}

fn rejection_name(rejection: GuardRejection) -> &'static str {
    match rejection {
        GuardRejection::Disabled => "disabled",
        GuardRejection::Stopped => "stopped",
        GuardRejection::GrantRequired => "grant-required",
        GuardRejection::ConfirmationPending => "confirmation-pending",
        GuardRejection::GrantDialogPending => "grant-dialog-pending",
        GuardRejection::InputBusy => "input-busy",
    }
}

/// Audit for rejected calls (gate rejection / capability rejection): the
/// action did not run and there is no injection consequence, but the error is
/// never swallowed silently (fail-open: an append failure only eprintlns).
/// The record carries the resolved display target like every other
/// boundary record. The target is the backend's recorded decision WHEN THE
/// CACHE ALREADY HOLDS ONE; a call refused by the read-only gate precheck
/// (or an Observe call, whose read happens inside `run`) has no primed
/// cache yet, so its record falls back to the documented chain — session
/// registry row, then the preference+probe answer (what the session WOULD
/// target). The append (with its fsync) runs on the blocking pool like the
/// grant-request write: a slow disk must not park an async runtime worker
/// (R9 review; the R10 review closed the last two inline sites — the
/// parse-failure and worker-join appends). Every call site awaits inline,
/// which — together with `run()`'s synchronous appends that happen
/// strictly after the awaited gate audits — keeps the per-session JSONL
/// record order deterministic.
async fn audit_rejected_call(
    parts: &Parts,
    action: &ComputerUseAction,
    reason: &str,
    message: &str,
) {
    let mut record = AuditRecord::new(
        &parts.session_id,
        action.name(),
        format!("rejected:{reason}"),
    );
    record.with_display_target(resolved_display_target(parts).as_str());
    let record = record.finish("rejected", Some(message.to_string()), 0);
    let session_id = parts.session_id.clone();
    tauri::async_runtime::spawn_blocking(move || match AuditLog::for_session(&session_id) {
        Ok(log) => {
            if let Err(error) = log.append(&record) {
                eprintln!("[computer_use] rejected-call audit append failed: {error}");
            }
        }
        Err(error) => {
            eprintln!("[computer_use] audit log unavailable for rejected call: {error}");
        }
    })
    .await
    .ok(); // pool shutdown at app death: the eprintlns above already logged
}

/// Audit a call that failed BEFORE `run()`'s per-call audit record exists —
/// a pre-gate capability-read failure (backend start failed, worker died).
/// The record is an `error` (the call was not refused by a gate — it
/// failed), with the display target from the same resolution chain as
/// `run()` (cached backend decision → session registry row → the
/// preference+probe fallback, so a prior successful action still pins this
/// record to the session's real target).
/// Fail-open like every audit append: a write failure warns, never blocks.
/// The append runs on the blocking pool (see [`audit_rejected_call`]).
async fn audit_failed_call(parts: &Parts, action: &ComputerUseAction, message: &str) {
    let mut record = AuditRecord::new(&parts.session_id, action.name(), "pre-run");
    record.with_display_target(resolved_display_target(parts).as_str());
    let record = record.finish("error", Some(message.to_string()), 0);
    let session_id = parts.session_id.clone();
    tauri::async_runtime::spawn_blocking(move || match AuditLog::for_session(&session_id) {
        Ok(log) => {
            if let Err(error) = log.append(&record) {
                eprintln!("[computer_use] failed-call audit append failed: {error}");
            }
        }
        Err(error) => {
            eprintln!("[computer_use] audit log unavailable for failed call: {error}");
        }
    })
    .await
    .ok();
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod confirm_payload_tests {
    //! Structured confirm-payload contract (the consent dialog's i18n
    //! source): the payload carries the canonical action name plus the
    //! structured fields, and keeps the English summary as fallback.
    use super::*;
    use serde_json::json;

    fn payload_for(
        action: ComputerUseAction,
        preview: Option<String>,
        masked_target: bool,
    ) -> Value {
        payload_for_target(action, preview, masked_target, "physical")
    }

    fn payload_for_target(
        action: ComputerUseAction,
        preview: Option<String>,
        masked_target: bool,
        target: &str,
    ) -> Value {
        let summary = action_summary(&action);
        build_confirm_payload(
            "s-test",
            "cu-1",
            &action,
            &summary,
            "element label",
            preview.as_deref(),
            masked_target,
            target,
        )
    }

    #[test]
    fn click_payload_carries_structured_fields_matching_the_summary() {
        let action = ComputerUseAction::Click {
            button: MouseButton::Left,
            count: 2,
            at: Some((5, 6)),
        };
        let payload = payload_for(action, None, false);
        assert_eq!(payload["action"], "double_click");
        assert_eq!(payload["summary"], "left click x2 at Some((5, 6))");
        assert_eq!(payload["button"], "left");
        assert_eq!(payload["click_count"], 2);
        assert_eq!(payload["point"], json!({ "x": 5, "y": 6 }));
        // Target disclosure: a frozen "virtual" | "physical" string.
        assert_eq!(payload["target"], "physical");
        // No typing fields on a click.
        assert!(payload.get("text_length").is_none());
        assert_eq!(payload["text_preview_truncated"], false);
        // The virtual spelling is the other half of the frozen vocabulary.
        let virtual_payload = payload_for_target(
            ComputerUseAction::Click {
                button: MouseButton::Left,
                count: 1,
                at: None,
            },
            None,
            false,
            "virtual",
        );
        assert_eq!(virtual_payload["target"], "virtual", "{virtual_payload}");
    }

    #[test]
    fn coordinate_less_click_omits_point() {
        let action = ComputerUseAction::Click {
            button: MouseButton::Right,
            count: 1,
            at: None,
        };
        let payload = payload_for(action, None, false);
        assert_eq!(payload["action"], "right_click");
        assert_eq!(payload["button"], "right");
        assert_eq!(payload["click_count"], 1);
        assert!(payload.get("point").is_none());
    }

    #[test]
    fn type_over_4096_marks_preview_truncated_but_keeps_the_length() {
        let text = "x".repeat(4097);
        let action = ComputerUseAction::Type { text };
        let payload = payload_for(action, None, false);
        assert_eq!(payload["action"], "type");
        assert_eq!(payload["summary"], "type 4097 characters");
        assert_eq!(payload["text_length"], 4097);
        assert_eq!(payload["text_preview_truncated"], true);
        assert!(
            payload.get("text_preview").is_none(),
            "over-cap text must not ride the payload: {payload}"
        );
    }

    #[test]
    fn secure_type_target_stays_masked_without_a_truncation_flag() {
        let action = ComputerUseAction::Type {
            text: "secret".into(),
        };
        let payload = payload_for(action, None, true);
        assert_eq!(payload["text_length"], 6);
        assert_eq!(payload["text_preview_truncated"], false);
        assert!(
            payload.get("text_preview").is_none(),
            "masked targets must not carry a preview: {payload}"
        );
    }

    #[test]
    fn short_non_secure_type_carries_the_preview() {
        let action = ComputerUseAction::Type {
            text: "hello".into(),
        };
        let payload = payload_for(action, Some("hello".into()), false);
        assert_eq!(payload["text_preview"], "hello");
        assert_eq!(payload["text_length"], 5);
        assert_eq!(payload["text_preview_truncated"], false);
        // Backward-compatible full-text key stays in sync with text_preview.
        assert_eq!(payload["type_preview_full"], "hello");
    }

    #[test]
    fn typed_text_chord_discloses_char_count_and_preview() {
        let action = ComputerUseAction::KeyChord {
            keys: parse_key_chord("h+a+c+k").expect("chord parses"),
            chord: "h+a+c+k".to_string(),
        };
        let payload = payload_for(action, Some("hack".into()), false);
        assert_eq!(payload["action"], "key");
        assert_eq!(payload["text_length"], 4);
        assert_eq!(payload["text_preview"], "hack");
        assert_eq!(payload["text_preview_truncated"], false);
    }

    #[test]
    fn named_key_chord_carries_no_typing_fields() {
        let action = ComputerUseAction::HoldKey {
            keys: parse_key_chord("Return").expect("chord parses"),
            chord: "Return".to_string(),
            ms: 500,
        };
        let payload = payload_for(action, None, false);
        assert_eq!(payload["action"], "hold_key");
        assert!(payload.get("text_length").is_none());
        assert!(payload.get("text_preview").is_none());
        assert_eq!(payload["text_preview_truncated"], false);
    }
}
