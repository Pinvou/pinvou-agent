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
use super::backend::BackendHandle;
use super::guard::{
    ComputerUseShared, ConfirmationCheck, GuardRejection, is_secure_role, matches_t3_denylist,
};
use super::platform;
use super::scaling::{self, ScaleMap, ScaledScreenshot};
use super::types::{
    ActionClass, ComputerUseAction, ComputerUseError, EVENT_CONFIRM_REQUIRED, EVENT_GRANT_REQUIRED,
    ElementInfo, Key, MouseButton, ScrollDirection, TOOL_NAME, UiTreeOptions, parse_key_chord,
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

impl ComputerUseTool {
    /// Production constructor: the backend starts lazily (the worker thread
    /// is spawned on the first request).
    pub fn new(app: AppHandle, session_id: String, shared: Arc<ComputerUseShared>) -> Self {
        let backend = BackendHandle::lazy(platform::create_backend);
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
        if registered
            .as_ref()
            .is_none_or(|current| current.is_same_backend(&self.parts.backend))
        {
            self.parts.shared.revoke_session(&self.parts.session_id);
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
        Some(Value::String(id)) => Some(id.clone()),
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

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

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
    let file_name = format!("{stamp}-{seq:04}.png");
    let abs_path = dir.join(&file_name);
    directory
        .atomic_write_private_file(std::ffi::OsStr::new(&file_name), &scaled.png)
        .map_err(|error| ComputerUseError::failed(format!("cannot write screenshot: {error}")))?;
    prune_old_screenshots(&dir, MAX_RETAINED_SCREENSHOTS);
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

fn shot_result_text(shot: &ShotOutcome) -> String {
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

fn with_image_metadata(result: ToolResult, shot: &ShotOutcome) -> ToolResult {
    result.with_metadata(json!({
        "images": [shot.abs_path.to_string_lossy()]
    }))
}

/// Keeps only the newest `keep` PNGs in the screenshot directory (filenames
/// sort chronologically: `%Y%m%d-%H%M%S`-{seq} — the stamp is fixed-width,
/// the per-session sequence zero-padded). Best-effort: any error (vanished
/// directory, unreadable entry, failed unlink) is ignored — retention must
/// never fail the action that just captured.
fn prune_old_screenshots(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    if names.len() <= keep {
        return;
    }
    names.sort();
    let excess = names.len() - keep;
    for name in names.iter().take(excess) {
        let _ = std::fs::remove_file(dir.join(name));
    }
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
    reason: &'static str,
}

/// Runs the denylist/password-field determination on one a11y element.
/// Coordinate screening (screen_point) and keyboard focus screening share the
/// same determination; the safety standard must be identical on both paths.
fn screen_element(element: &ElementInfo) -> T3Screening {
    if element.secure || is_secure_role(&element.role) {
        return T3Screening::Blocked(T3Hit {
            element_label: format!("{} ({})", element.name, element.role),
            reason: "a password/secure field",
        });
    }
    // The denylist matches the wider screening copy when the platform
    // provided one: `name` is display-truncated, so a padded
    // attacker-controlled label could otherwise push a consequential term
    // past the match window.
    let screening_text = element.screening_name.as_deref().unwrap_or(&element.name);
    if matches_t3_denylist(screening_text) || matches_t3_denylist(&element.role) {
        return T3Screening::Blocked(T3Hit {
            element_label: format!("{} ({})", element.name, element.role),
            reason: "a consequential control (financial/send/delete/submit/consent)",
        });
    }
    T3Screening::Clear
}

/// Screens the a11y element at one input coordinate against the
/// denylist/password fields.
fn screen_point(parts: &Parts, x: i32, y: i32) -> T3Screening {
    let element = match parts.backend.element_at_point(x, y) {
        Ok(element) => element,
        // Screening unavailable (a11y query failure) ≠ a denylist hit:
        // screening is best-effort category detection, and no mainstream
        // product asks for a confirmation over a screening infrastructure
        // failure (in AT-SPI-unavailable, multi-monitor and similar scenarios,
        // fail-closed would only cause confirmation storms). Let it execute.
        Err(_) => return T3Screening::Clear,
    };
    match element {
        Some(element) => screen_element(&element),
        // No element at the target point = nothing to check (Clear): unnamed
        // targets are everywhere on real desktops (canvas, hover targets,
        // custom widgets) and the denylist is name-based, so an absent name
        // is not a red flag.
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
///   cursor would miss the endpoint).
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
    for (x, y) in points {
        match screen_point(parts, x, y) {
            T3Screening::Clear => {}
            other => return other,
        }
    }
    T3Screening::Clear
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
            ComputerUseAction::MouseMove { .. } | ComputerUseAction::Scroll { .. }
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
/// builds does not matter — but the hash must be **unpredictable to the
/// model**: the adversary who could profit from a binding collision is the
/// code-executing model itself, so a fixed-key `DefaultHasher` (offline
/// birthday-collidable at ~2^32 work) would let it get a benign text
/// approved and spend the token on colliding malicious text. Truncated
/// SHA-256 keeps the same in-process semantics with no offline collision
/// shortcut.
fn action_binding(action: &ComputerUseAction) -> u64 {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("{action:?}").as_bytes());
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(prefix)
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
/// serialization path for the structured confirm fields).
fn build_confirm_payload(
    session_id: &str,
    confirm_id: &str,
    action: &ComputerUseAction,
    summary: &str,
    element_label: &str,
    type_preview_full: Option<&str>,
    masked_target: bool,
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

/// Whether the action attaches a screenshot (the single source of truth is
/// [`ComputerUseAction::attaches_screenshot`]).
fn attaches_screenshot(action: &ComputerUseAction) -> bool {
    action.attaches_screenshot()
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
        let _input_guard = (action.class() == ActionClass::Input)
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
            let auto = capture_and_store(&parts, &workspace).map_err(|e| backend_error_text(&e))?;
            warnings.push(
                "no screenshot had been taken this session; one was captured automatically and is attached"
                    .to_string(),
            );
            shot = Some(auto);
        }

        // Reject before any consent surface: a stop/revoke landing during the
        // automatic screenshot must not still pop a confirmation dialog
        // (review finding; the mint side refuses stopped/disabled as a second
        // line of defense).
        if action.class() == ActionClass::Input {
            if let Err(rejection) = parts.shared.verify_input_action(&parts.session_id) {
                return Err(rejection.message());
            }
        }

        // T3 confirmation token: proceed only when the model passes back a
        // confirm_id and the state holds a matching approval token (same
        // session, same action summary, same action content).
        if requires_t3_check(&action) {
            let summary = action_summary(&action);
            let binding = action_binding(&action);
            match &parsed.confirm_id {
                Some(id) => {
                    match parts
                        .shared
                        .take_confirmation(id, &parts.session_id, &summary, binding)
                    {
                        // A granted token proceeds directly to execution — no
                        // re-screen, no re-request arms (mainstream model: the
                        // API confirmation is one per-action id the client
                        // acknowledges; there is no crypto and no re-verification).
                        // No a11y query happens on this path at all. The token is
                        // bound to the full action content, so what executes is
                        // identical to what was approved (review finding: a
                        // summary-only binding let a same-length different text
                        // spend the token).
                        ConfirmationCheck::Granted => {
                            confirmed_t3 = true;
                        }
                        ConfirmationCheck::Unknown => {
                            return Err(
                                "the confirm_id is invalid, expired, or was already used. Ask the \
                             user to confirm again."
                                    .to_string(),
                            );
                        }
                    }
                }
                None => {
                    // One focused-element read feeds BOTH the masked-preview
                    // decision and keyboard screening below: two separate
                    // queries could race a focus change onto a password field
                    // and leak its full text into the confirm event.
                    let mut focused = None;
                    let focus_uncertain = if matches!(
                        &action,
                        ComputerUseAction::Type { .. }
                            | ComputerUseAction::KeyChord { .. }
                            | ComputerUseAction::HoldKey { .. }
                    ) {
                        match parts.backend.focused_element() {
                            Ok(value) => {
                                focused = value;
                                false
                            }
                            Err(_) => {
                                // The Linux layer deliberately returns Err when the
                                // focused search exhausts its budget without a
                                // verdict ("never masquerade as none"): an uncertain
                                // focus must not be flattened into "nothing focused",
                                // or the full typed text could ride the confirm event
                                // while a password field actually holds focus.
                                // Screening itself stays fail-open (focused stays
                                // None), but the preview is masked like a secure
                                // target.
                                true
                            }
                        }
                    } else {
                        false
                    };
                    // The masked-target decision only shapes the dialog
                    // payload (whether the full typed text may ride the
                    // confirm event). Keyboard chords that carry character
                    // keys type their characters too, so the secure-target
                    // check covers them just like type.
                    let secure_type_target = matches!(
                        &action,
                        ComputerUseAction::Type { .. }
                            | ComputerUseAction::KeyChord { .. }
                            | ComputerUseAction::HoldKey { .. }
                    ) && (focus_uncertain
                        || focused.as_ref().is_some_and(|element| {
                            element.secure || is_secure_role(&element.role)
                        }));
                    // Optional full text for the confirm event, fixed at mint
                    // time (never recomputed at spend time).
                    let type_preview_full = full_type_preview(&action, secure_type_target);
                    let map = parts.state.lock().last_map.clone();
                    match t3_screening(&parts, &action, map.as_ref(), focused.as_ref()) {
                        T3Screening::Clear => {}
                        T3Screening::Blocked(hit) => {
                            // The a11y queries above (focused element,
                            // screening) take real time: re-check the
                            // stop/disable/grant state BEFORE raising the
                            // dialog, so a stop landing during screening does
                            // not pop a consent surface for a feature that is
                            // now off (the mint-side refusal stays as the
                            // backstop for the race window that remains).
                            if let Err(rejection) =
                                parts.shared.verify_input_action(&parts.session_id)
                            {
                                return Err(rejection.message());
                            }
                            let mut blocked = request_confirmation(
                                &parts,
                                &action,
                                &summary,
                                &hit.element_label,
                                hit.reason,
                                type_preview_full.clone(),
                                secure_type_target,
                                binding,
                            );
                            // Warnings ride the blocked error too: the
                            // auto-captured screenshot was persisted and
                            // audited, but the success path that would attach
                            // it never runs on a blocked action (the same gap
                            // the drag failure path works around).
                            for warning in &warnings {
                                blocked.push_str(&format!("\nwarning: {warning}"));
                            }
                            return Err(blocked);
                        }
                    }
                }
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
                if let Err(rejection) = parts.shared.verify_input_action(&parts.session_id) {
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
        if attaches_screenshot(&action) {
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
                    outcome.push_str(&shot_result_text(&fresh));
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
        ComputerUseAction::Scroll {
            direction,
            amount,
            at,
        } => format!("scroll {} x{amount} at {at:?}", direction.as_str()),
        ComputerUseAction::MouseDown { button } => format!("{} mouse down", button.as_str()),
        ComputerUseAction::MouseUp { button } => format!("{} mouse up", button.as_str()),
        ComputerUseAction::MouseMove { x, y } => format!("mouse_move to ({x}, {y})"),
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
        if let Err(rejection) = parts.shared.verify_input_action(&parts.session_id) {
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
                    Ok(format!(
                        "element at ({x}, {y}): role=\"{}\" name=\"{}\" bounds=({rx}, {ry}, \
                         {rw}x{rh}) in screenshot space secure={}",
                        element.role, element.name, element.secure
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
                if let Err(rejection) = parts.shared.verify_input_action(&parts.session_id) {
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
                if let Err(rejection) = parts.shared.verify_input_action(&parts.session_id) {
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
        "Control the user's desktop: take screenshots, move/click the mouse, type text, \
         press key chords, scroll, and inspect the accessibility tree. \
         Coordinates are ALWAYS in the pixel space of the last screenshot this tool returned \
         (origin top-left); take a screenshot first and reuse its coordinate space. \
         Every action that touches the mouse or keyboard (mouse_move, scroll, clicks, keys, \
         typing) requires a per-session grant from the user; the grant stays valid until \
         revoked. Before an activation action runs (clicks, drags, mouse down/up, typing, \
         keys), its target is screened against a consequential-control denylist \
         (financial/send/delete/submit/consent categories) and password fields; a hit \
         pauses the action until the user confirms it via confirm_id. Screening is \
         best-effort category detection over the accessibility tree: when the target \
         cannot be read or screening is unavailable, the action still executes without \
         confirmation. Know the limits before relying on the screen: the category terms \
         match English, Chinese (Simplified/Traditional), and Japanese labels only — in \
         other UI languages only the language-independent password-field screen applies. \
         The target is located by a point lookup with no window identity, so a \
         same-label or same-position window/UI change between screening (or approval) \
         and injection cannot be detected; on Linux a target whose accessibility role \
         cannot be read may still require confirmation as a precaution. An approved \
         confirmation is bound to the exact action content but is NOT re-screened: it \
         executes on whatever occupies the target position when it runs, which can be \
         minutes after the user approved. The CONTENT you type is never screened, and \
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
                let record = record.finish(
                    "rejected",
                    Some(format!(
                        "parse failed: {}",
                        error_text.chars().take(160).collect::<String>()
                    )),
                    0,
                );
                match AuditLog::for_session(&self.parts.session_id) {
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
                return Err(error);
            }
        };

        // Capabilities first (review finding: prompting for the grant before
        // reporting the platform unsupported would induce the user to
        // authorize a platform that can never be used, while burning a grant
        // budget for nothing).
        if parsed.action.class() == ActionClass::Input {
            let backend = self.parts.backend.clone();
            let capabilities = tauri::async_runtime::spawn_blocking(move || backend.capabilities())
                .await
                .map_err(|error| {
                    ToolError::execution_failed(format!("computer use worker join failed: {error}"))
                })?
                .map_err(|error| ToolError::execution_failed(backend_error_text(&error)))?;
            if !capabilities.input {
                // Audit the rejected call (review finding: capability
                // rejections had no trace at all, inconsistent with the
                // established audit treatment of gate rejections).
                audit_rejected_call(
                    &self.parts,
                    &parsed.action,
                    "capability:input-unsupported",
                    &format!(
                        "input injection is unsupported on this platform/session ({})",
                        capabilities.notes
                    ),
                );
                return Ok(ToolResult::error(format!(
                    "input injection is unsupported on this platform/session ({})",
                    capabilities.notes
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
                self.parts
                    .shared
                    .mark_grant_requested(&self.parts.session_id);
                self.parts.events.emit(
                    EVENT_GRANT_REQUIRED,
                    json!({ "session_id": self.parts.session_id }),
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
            );
            return Ok(ToolResult::error(rejection.message()));
        }

        let workspace = resolve_workspace(context, &self.parts.session_id);
        let parts = self.parts.clone();
        let audit_session = self.parts.session_id.clone();
        let audit_action = parsed.action.name();
        tauri::async_runtime::spawn_blocking(move || run(parts, parsed, workspace))
            .await
            .map_err(|error| {
                // Worker join failure = run panicked, possibly after injection
                // happened: this is the only "action may have executed" path
                // with no audit record (review finding). The audit error
                // field uses fixed copy — the panic text carried by JoinError
                // may embed input content (e.g. a chord fragment from a char
                // boundary panic) and must not be echoed into the JSONL; the
                // full error goes only to the model.
                let record = AuditRecord::new(&audit_session, audit_action, "input").finish(
                    "error",
                    Some("worker join failed".to_string()),
                    0,
                );
                if let Err(audit_error) =
                    AuditLog::for_session(&audit_session).and_then(|log| log.append(&record))
                {
                    eprintln!("[computer_use] audit append failed: {audit_error}");
                }
                ToolError::execution_failed(format!("computer use worker join failed: {error}"))
            })
    }
}

fn rejection_name(rejection: GuardRejection) -> &'static str {
    match rejection {
        GuardRejection::Disabled => "disabled",
        GuardRejection::Stopped => "stopped",
        GuardRejection::GrantRequired => "grant-required",
        GuardRejection::InputBusy => "input-busy",
    }
}

/// Audit for rejected calls (gate rejection / capability rejection): the
/// action did not run and there is no injection consequence, but the error is
/// never swallowed silently (fail-open: an append failure only eprintlns).
fn audit_rejected_call(parts: &Parts, action: &ComputerUseAction, reason: &str, message: &str) {
    let record = AuditRecord::new(
        &parts.session_id,
        action.name(),
        format!("rejected:{reason}"),
    )
    .finish("rejected", Some(message.to_string()), 0);
    match AuditLog::for_session(&parts.session_id) {
        Ok(log) => {
            if let Err(error) = log.append(&record) {
                eprintln!("[computer_use] rejected-call audit append failed: {error}");
            }
        }
        Err(error) => {
            eprintln!("[computer_use] audit log unavailable for rejected call: {error}");
        }
    }
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
        let summary = action_summary(&action);
        build_confirm_payload(
            "s-test",
            "cu-1",
            &action,
            &summary,
            "element label",
            preview.as_deref(),
            masked_target,
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
        // No typing fields on a click.
        assert!(payload.get("text_length").is_none());
        assert_eq!(payload["text_preview_truncated"], false);
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
