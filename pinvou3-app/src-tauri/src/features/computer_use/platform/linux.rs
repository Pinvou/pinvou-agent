//! Linux backend: full support on X11; on Wayland, screenshots come from the
//! ScreenCast stream bound to a portal RemoteDesktop session (PipeWire, see
//! [`super::wayland_portal`]/[`super::wayland_capture`]) and input goes
//! through xdg-desktop-portal RemoteDesktop.
//!
//! Session detection decides the capability surface (`detect_session` is a
//! pure function, easy to unit-test):
//! - Primary signal `XDG_SESSION_TYPE` (`x11`/`wayland`/`tty`), corroborated
//!   by `WAYLAND_DISPLAY` + `XDG_RUNTIME_DIR`; `DISPLAY` is never used alone
//!   as the X11 criterion (it is also set under XWayland). If neither is
//!   reachable → explicit `unsupported` at construction.
//! - X11: screenshots via `xcap::Monitor` (xcb/XGetImage, root-window
//!   physical pixels); input via `enigo` (x11rb XTEST, coordinates also in
//!   root-window pixels, `Capture.input_scale_x/y = 1.0`). Note that on X11
//!   xcap reports RandR geometry divided by Xft.dpi/96 (logical coordinates),
//!   while `capture_image` captures in root-window pixels and XTEST also
//!   injects root-window pixels, so `origin_x/y` must multiply xcap's logical
//!   origin back by `scale_factor`; the input scale stays 1.0.
//! - Wayland: screenshots and input share a portal RemoteDesktop session
//!   (lazily started; the first screenshot or input action opens the system
//!   authorization dialog once): screenshots take frames from the PipeWire
//!   stream of `OpenPipewireRemote` (input coordinates = stream-local pixels,
//!   origin (0,0); KDE's input unit is stream-logical pixels, with the scale
//!   derived from the stream size reported by the compositor). When the
//!   in-session screenshot stream is unavailable, fall back to xcap's
//!   GNOME-Shell/portal/wlroots chain (probed at construction; the portal
//!   path may show an authorization dialog per capture, noted in
//!   `capabilities().notes`); if both paths fail, explicit `unsupported`.
//!   Input synthesis goes through portal RemoteDesktop; when no portal can be
//!   found, input is explicitly unavailable.
//! - Accessibility tree: `atspi` (AT-SPI over D-Bus, separate a11y bus, works
//!   on both X11/Wayland). The a11y bus connection is built by this module
//!   itself (`zbus::connection::Builder` + `method_timeout`): atspi's
//!   `AccessibilityConnection` does not allow injecting a self-built
//!   connection (`new`/`from_address` each build internally, with no timeout
//!   setting point), while zbus's default timeout is very generous and tree
//!   traversal makes several calls per node — a hung app could permanently
//!   pin the worker. So the thin wrapper is bypassed and atspi's proxy types
//!   are used with a self-managed connection, plus an operation-level overall
//!   deadline as a second backstop. The trait is synchronous while atspi is
//!   async: the backend holds a dedicated current-thread tokio runtime and
//!   `block_on`s on the worker thread (an ordinary std::thread, not the
//!   engine's main runtime), so there is no nested-runtime deadlock risk.
//!   On Wayland, Component extents are best-effort (compositor/toolkit
//!   dependent); the ui_tree output header says so.
//! - Threading contract as on the other platforms: objects are constructed
//!   on the worker thread and must not be moved across threads.

use std::future::Future;
use std::thread::sleep;
use std::time::{Duration, Instant};

use atspi::proxy::accessible::{AccessibleProxy, ObjectRefExt};
use atspi::proxy::bus::BusProxy;
use atspi::proxy::component::ComponentProxy;
use atspi::{CoordType, Role, State, StateSet};
use enigo::{Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
use xcap::Monitor;
use zbus::proxy::CacheProperties;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::super::backend::ComputerUseBackend;
use super::super::types::{
    Capabilities, Capture, ComputerUseError, ElementInfo, Key, MouseButton, ScrollDirection,
    UiTreeOptions,
};
use super::helpers::{
    MAX_SCROLL_CLICKS, drag_waypoints, normalize_typed_newlines, sanitize_name, screening_hit,
};
use super::wayland_portal::{self, PortalInput};

/// Settle time between a move and the click that follows it (applied in
/// `click` and at drag start; `mouse_down` does not settle) — a race buffer
/// between injection and compositor processing.
const SETTLE_MS: u64 = 40;
/// Gap between multiple clicks (double/triple click).
const CLICK_GAP_MS: u64 = 40;
/// Interpolated drag steps and per-step delay (instantaneous moves that are
/// too fast are recognized as non-drags by some apps).
const DRAG_STEPS: usize = 12;
const DRAG_STEP_MS: u64 = 10;
/// Granularity of the hold_key sleep: a stop/timeout cancel flag is polled
/// at least this often, so an abandoned hold stops within this window
/// instead of the full hold duration.
const HOLD_CANCEL_POLL_MS: u64 = 100;
/// Delay between scroll clicks.
const SCROLL_GAP_MS: u64 = 15;
/// ui_tree default capture caps.
const DEFAULT_MAX_DEPTH: u32 = 8;
const DEFAULT_MAX_NODES: usize = 200;
/// Maximum characters kept for a single node name (keeps oversized text from
/// blowing up the tool result).
const MAX_NAME_CHARS: usize = 80;
/// Timeout for a single a11y D-Bus method call (zbus connection-level
/// `method_timeout`).
/// zbus's default timeout is very generous, tree traversal
/// makes 4-5 calls per node, and a single hung app can leave the worker
/// pending forever.
const A11Y_METHOD_TIMEOUT: Duration = Duration::from_secs(3);
/// Operation-level deadline for single-point a11y queries
/// (`element_at_point`/`focused_element`): a second backstop on top of the
/// method-level 3s, guarding against "every step is fast but the total runs
/// away".
const A11Y_POINT_DEADLINE: Duration = Duration::from_secs(6);
/// Operation-level deadline for the whole ui_tree traversal (the node count
/// is capped but each node takes several calls; 20s covers a healthy desktop
/// and fails fast in hung environments, keeping the worker responsive).
const A11Y_TREE_DEADLINE: Duration = Duration::from_secs(20);
/// Node budget for the focused_element tree search (dual constraint with the
/// operation-level deadline).
const FOCUSED_SEARCH_MAX_NODES: usize = 300;

/// Session type (detection result). Pure data, for `detect_session` unit-test
/// assertions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionKind {
    X11,
    Wayland,
    /// tty / no graphical-session signal at all.
    NoDisplay,
}

/// Session detection result (with diagnostics, used in errors and
/// capabilities copy).
#[derive(Debug, Clone)]
struct SessionInfo {
    kind: SessionKind,
    /// Raw `XDG_SESSION_TYPE` (lowercased); None when unset/unrecognized.
    session_type: Option<String>,
    /// `XDG_CURRENT_DESKTOP` (for diagnostics, e.g. "GNOME"/"KDE"/"sway").
    desktop: Option<String>,
    has_wayland_display: bool,
    has_display: bool,
}

impl SessionInfo {
    fn desktop_label(&self) -> &str {
        self.desktop.as_deref().unwrap_or("unknown")
    }
}

/// Pure-function session detection: maps the environment variables to X11 /
/// Wayland / no display.
///
/// Rules: the primary signal is `XDG_SESSION_TYPE`; when unset or
/// unrecognized, fall back to `WAYLAND_DISPLAY` (corroborated by
/// `XDG_RUNTIME_DIR`); `DISPLAY` counts as the X11 criterion only when there
/// is no Wayland signal at all (it also exists in XWayland sessions).
fn detect_session(env: &dyn Fn(&str) -> Option<String>) -> SessionInfo {
    let nonempty = |key: &str| env(key).filter(|value| !value.trim().is_empty());
    let session_type = nonempty("XDG_SESSION_TYPE").map(|value| value.trim().to_ascii_lowercase());
    let wayland_display = nonempty("WAYLAND_DISPLAY");
    let runtime_dir = nonempty("XDG_RUNTIME_DIR");
    let display = nonempty("DISPLAY");
    let desktop = nonempty("XDG_CURRENT_DESKTOP");

    let kind = match session_type.as_deref() {
        Some("wayland") => SessionKind::Wayland,
        Some("x11") => SessionKind::X11,
        Some("tty") => SessionKind::NoDisplay,
        _ => {
            if wayland_display.is_some() && runtime_dir.is_some() {
                // Strong corroboration: both the Wayland socket name and the
                // runtime dir are present.
                SessionKind::Wayland
            } else if wayland_display.is_some() {
                // Weak corroboration (XDG_RUNTIME_DIR missing) is still
                // treated as Wayland: the screenshot probe and the explicit
                // unsupported path backstop it.
                SessionKind::Wayland
            } else if display.is_some() {
                SessionKind::X11
            } else {
                SessionKind::NoDisplay
            }
        }
    };

    SessionInfo {
        kind,
        session_type,
        desktop,
        has_wayland_display: wayland_display.is_some(),
        has_display: display.is_some(),
    }
}

/// types' normalized key → enigo key. `Char` goes through Unicode (enigo
/// injects via a temporary keycode remap on X11, so arbitrary characters like
/// CJK work). F13-F20 are mappable; beyond F20 fails explicitly.
fn map_enigo_key(key: Key) -> Result<enigo::Key, ComputerUseError> {
    let mapped = match key {
        Key::Control => enigo::Key::Control,
        Key::Alt => enigo::Key::Alt,
        Key::Shift => enigo::Key::Shift,
        Key::Meta => enigo::Key::Meta,
        Key::Enter => enigo::Key::Return,
        Key::Escape => enigo::Key::Escape,
        Key::Tab => enigo::Key::Tab,
        Key::Space => enigo::Key::Space,
        Key::Backspace => enigo::Key::Backspace,
        Key::Delete => enigo::Key::Delete,
        Key::Insert => enigo::Key::Insert,
        Key::Up => enigo::Key::UpArrow,
        Key::Down => enigo::Key::DownArrow,
        Key::Left => enigo::Key::LeftArrow,
        Key::Right => enigo::Key::RightArrow,
        Key::Home => enigo::Key::Home,
        Key::End => enigo::Key::End,
        Key::PageUp => enigo::Key::PageUp,
        Key::PageDown => enigo::Key::PageDown,
        Key::Function(n) => match n {
            1 => enigo::Key::F1,
            2 => enigo::Key::F2,
            3 => enigo::Key::F3,
            4 => enigo::Key::F4,
            5 => enigo::Key::F5,
            6 => enigo::Key::F6,
            7 => enigo::Key::F7,
            8 => enigo::Key::F8,
            9 => enigo::Key::F9,
            10 => enigo::Key::F10,
            11 => enigo::Key::F11,
            12 => enigo::Key::F12,
            13 => enigo::Key::F13,
            14 => enigo::Key::F14,
            15 => enigo::Key::F15,
            16 => enigo::Key::F16,
            17 => enigo::Key::F17,
            18 => enigo::Key::F18,
            19 => enigo::Key::F19,
            20 => enigo::Key::F20,
            _ => {
                return Err(ComputerUseError::unsupported(
                    "input",
                    format!("function key F{n} is out of the mappable range F1-F20"),
                ));
            }
        },
        Key::Char(c) => enigo::Key::Unicode(c),
    };
    Ok(mapped)
}

fn map_enigo_button(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    }
}

fn input_failed(context: &str, error: impl std::fmt::Display) -> ComputerUseError {
    ComputerUseError::failed(format!("{context}: {error}"))
}

/// Merges drag-finalization errors: when the move and the release **both
/// fail**, surfacing only the move error would leave the caller unaware the
/// left button was still stuck pressed. When both fail, explicitly note the
/// button may not have been released.
fn combine_drag_errors(
    move_result: Result<(), ComputerUseError>,
    release_result: Result<(), ComputerUseError>,
) -> Result<(), ComputerUseError> {
    match (move_result, release_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Ok(())) => Err(error),
        (Err(move_error), Err(release_error)) => Err(move_error.same_kind(format!(
            "{move_error}; additionally the drag release failed ({release_error}) — \
             the left mouse button may still be pressed"
        ))),
    }
}

fn settle() {
    sleep(Duration::from_millis(SETTLE_MS));
}

/// Fallback for a failed Wayland chord press (shared by `key_chord`/
/// `hold_key`): press all keysyms in order; when a press fails, best-effort
/// release the already-pressed keysyms in reverse (release errors swallowed),
/// then propagate the original error — so a mid-way failure cannot strand
/// modifiers pressed (same fallback as X11's `press_chord`). Generic over the
/// injection side so unit tests can drive it with a recording stand-in.
fn press_keysyms_unwind(
    keysyms: &[i32],
    mut event: impl FnMut(i32, bool) -> Result<(), ComputerUseError>,
) -> Result<(), ComputerUseError> {
    for (index, keysym) in keysyms.iter().enumerate() {
        if let Err(error) = event(*keysym, true) {
            // Release the already-pressed keysyms in reverse, best-effort
            // (release errors swallowed): the caller's original press error
            // is what matters, but stranded modifiers would corrupt every
            // subsequent input action. The *failing* keysym is included: a
            // timed-out notify "says nothing" about delivery — the press may
            // still have been delivered, and releasing an un-landed key is
            // a compositor-side no-op.
            for held in keysyms[..=index].iter().rev() {
                let _ = event(*held, false);
            }
            return Err(error);
        }
    }
    Ok(())
}

/// Best-effort release of every keysym (reverse order), with the pointer
/// path's compensating retry per keysym: a failed release notify may or may
/// not have been delivered — one extra release on the still-open (poisoned)
/// session is a compositor-side no-op when the first landed, and unstrands
/// the key when it did not (mutter never synthesizes the missing release
/// when the session closes). Attempts EVERY release (modifiers must not
/// strand) and returns the first release error.
fn release_keysyms_with_retry(
    mut event: impl FnMut(i32, bool) -> Result<(), ComputerUseError>,
    keysyms: &[i32],
) -> Result<(), ComputerUseError> {
    let mut first_err = None;
    for keysym in keysyms.iter().rev() {
        if let Err(error) = event(*keysym, false) {
            let _ = event(*keysym, false);
            if first_err.is_none() {
                first_err = Some(error);
            }
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// X11 type_text segmentation (pure, unit-tested): split at every '\n'
/// boundary. Empty segments are kept so a trailing newline ("text\n") still
/// produces the closing Enter; the caller skips empty runs for text
/// injection but still emits the Enter between segments.
fn x11_type_runs(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_string).collect()
}

/// secure determination (pure function): `PasswordText` decides directly; a
/// **failed role query** (`role_unknown`) conservatively falls back to secure
/// — unable to prove it is not a password field, prefer making the tool layer
/// ask for one more confirmation over letting a password field through as a
/// normal element.
fn is_secure_role(role: Role, role_unknown: bool) -> bool {
    role == Role::PasswordText || role_unknown
}

/// Node flag bits (for the compact single-line output). `secure` is supplied
/// by the caller per [`is_secure_role`]; in that case the caller must also
/// erase the name (see `write_node`/`element_info_of`).
fn state_flags(secure: bool, state: Option<StateSet>) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if secure {
        flags.push("secure");
    }
    if let Some(set) = state {
        if set.contains(State::Active) {
            flags.push("active");
        }
        if set.contains(State::Focused) {
            flags.push("focused");
        }
        if set.contains(State::Editable) {
            flags.push("editable");
        }
        if set.contains(State::Modal) {
            flags.push("modal");
        }
        if !set.contains(State::Enabled) {
            flags.push("disabled");
        }
    }
    flags
}

/// The AccessibleProxy of the a11y registry root. Copies the construction
/// essentials of atspi's
/// `AccessibilityConnection::root_accessible_on_registry`: the registry's
/// DBus property interface is incompletely implemented, so property caching
/// must be explicitly disabled.
async fn root_accessible(conn: &zbus::Connection) -> Result<AccessibleProxy<'_>, ComputerUseError> {
    AccessibleProxy::builder(conn)
        .destination("org.a11y.atspi.Registry")
        .map_err(|error| {
            ComputerUseError::unavailable(format!("AT-SPI registry destination: {error}"))
        })?
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .map_err(|error| ComputerUseError::unavailable(format!("AT-SPI registry root: {error}")))
}

/// Blindly builds a ComponentProxy for the same object from an
/// AccessibleProxy (does not check GetInterfaces per node first, avoiding an
/// extra D-Bus round trip; objects without Component support fail at
/// get_extents, treated as "no bounds").
async fn component_of<'c>(
    conn: &'c zbus::Connection,
    proxy: &AccessibleProxy<'_>,
) -> Option<ComponentProxy<'c>> {
    ComponentProxy::builder(conn)
        .destination(proxy.inner().destination().as_str().to_string())
        .ok()?
        .path(proxy.inner().path().as_str().to_string())
        .ok()?
        .build()
        .await
        .ok()
}

async fn screen_extents(
    conn: &zbus::Connection,
    proxy: &AccessibleProxy<'_>,
) -> Option<(i32, i32, i32, i32)> {
    component_of(conn, proxy)
        .await?
        .get_extents(CoordType::Screen)
        .await
        .ok()
}

/// Strict extents: hit-testing relies on window extents to decide "is the
/// point inside this window", so an undecidable window must be reported as a
/// query failure (clearly distinct from the "no element" Ok(None) — Ok(None)
/// is let through by the tool layer under the None policy). It must never be
/// answered as "the window does not cover the point", which would swallow a
/// query failure into a pass justification.
///
/// What the *caller* does with that failure is the caller's decision, and
/// `element_at_point_async` deliberately does not propagate every one of them:
/// a non-active window that cannot be decided is skipped so a single hidden
/// helper cannot disable screening for the whole desktop. This function's
/// contract is only that the failure is never disguised as a negative answer.
async fn screen_extents_strict(
    conn: &zbus::Connection,
    proxy: &AccessibleProxy<'_>,
) -> Result<(i32, i32, i32, i32), ComputerUseError> {
    let component = component_of(conn, proxy).await.ok_or_else(|| {
        ComputerUseError::unavailable("AT-SPI Component proxy unavailable for window")
    })?;
    let extents = component
        .get_extents(CoordType::Screen)
        .await
        .map_err(|error| {
            ComputerUseError::unavailable(format!("AT-SPI window extents: {error}"))
        })?;
    // "Success but zero size" cannot decide containment: on Wayland AT-SPI
    // commonly reports all-zero extents (see the e2e comment at the bottom of
    // this module); letting it through would judge **every** window as "does
    // not cover the point" → Ok(None), swallowing a query failure into a pass
    // justification, and the backend could never answer a trustworthy hit
    // result again. Raise it as a query
    // failure, clearly distinct from "no element"; how screening handles
    // faults (let the action execute) is decided uniformly by the tool layer.
    if extents.2 <= 0 || extents.3 <= 0 {
        return Err(ComputerUseError::unavailable(format!(
            "AT-SPI window extents are empty ({},{},{},{}); the window exposes no usable \
             coordinate space (common on Wayland), so hit-testing cannot be trusted",
            extents.0, extents.1, extents.2, extents.3
        )));
    }
    Ok(extents)
}

/// Whether a point falls inside window extents (right/bottom edges
/// exclusive). Pure function, easy to unit-test.
fn extents_contain(extents: (i32, i32, i32, i32), x: i32, y: i32) -> bool {
    let (wx, wy, ww, wh) = extents;
    x >= wx && x < wx + ww && y >= wy && y < wy + wh
}

/// All application top-level windows under the registry root (active or
/// not).
///
/// With `strict = true` (screening paths like element_at_point /
/// focused_element), a query failure at any level is propagated — never
/// silently swallowed into "no windows"; with `strict = false` (the ui_tree
/// observation path) it stays best-effort: a single hung app is skipped
/// without dragging down the whole tree.
async fn app_windows<'a>(
    conn: &'a zbus::Connection,
    root: &AccessibleProxy<'_>,
    strict: bool,
) -> Result<Vec<AccessibleProxy<'a>>, ComputerUseError> {
    let mut windows = Vec::new();
    let apps = match root.get_children().await {
        Ok(apps) => apps,
        Err(error) => {
            if strict {
                return Err(ComputerUseError::unavailable(format!(
                    "AT-SPI registry root children: {error}"
                )));
            }
            // Best-effort path: root children failure → empty window list
            // (the tree degrades to a shallow root).
            return Ok(windows);
        }
    };
    for app_ref in apps {
        if app_ref.is_null() {
            continue;
        }
        let app = match app_ref.into_accessible_proxy(conn).await {
            Ok(app) => app,
            Err(error) => {
                if strict {
                    return Err(ComputerUseError::unavailable(format!(
                        "AT-SPI application proxy: {error}"
                    )));
                }
                continue;
            }
        };
        let children = match app.get_children().await {
            Ok(children) => children,
            Err(error) => {
                if strict {
                    return Err(ComputerUseError::unavailable(format!(
                        "AT-SPI application children: {error}"
                    )));
                }
                continue;
            }
        };
        for child_ref in children {
            if child_ref.is_null() {
                continue;
            }
            match child_ref.into_accessible_proxy(conn).await {
                Ok(window) => windows.push(window),
                Err(error) => {
                    if strict {
                        return Err(ComputerUseError::unavailable(format!(
                            "AT-SPI window proxy: {error}"
                        )));
                    }
                }
            }
        }
    }
    Ok(windows)
}

/// Moves the window with `State::Active` to the front (hit-testing prefers
/// the active window). Returns whether an active window was found, so the
/// caller can tell "index 0 is the active window" from "index 0 is merely
/// first on the bus" — bus order is not z-order, so that distinction is the
/// only ordering signal available here.
async fn active_first(windows: &mut [AccessibleProxy<'_>]) -> bool {
    for (index, window) in windows.iter().enumerate() {
        if let Ok(state) = window.get_state().await {
            if state.contains(State::Active) {
                windows.swap(0, index);
                return true;
            }
        }
    }
    false
}

/// Builds an [`ElementInfo`] from an AccessibleProxy.
///
/// secure determination: `PasswordText` decides directly; a **failed role
/// query** conservatively falls back to secure — unable to prove it is not a
/// password field, prefer making the tool layer ask for one more confirmation
/// over letting a password field through as a normal element; the name is
/// erased as well, so password content cannot leak via ElementInfo.
/// `fallback_bounds`: the bounds used when the extents query fails (the hit
/// path passes the hit point, the search path passes 0; bounds are display
/// only, the hit already happened).
async fn element_info_of(
    conn: &zbus::Connection,
    proxy: &AccessibleProxy<'_>,
    fallback_bounds: (i32, i32, i32, i32),
) -> ElementInfo {
    let (role, role_unknown) = match proxy.get_role().await {
        Ok(role) => (role, false),
        Err(_) => (Role::Unknown, true),
    };
    let secure = is_secure_role(role, role_unknown);
    // A secure element's name is erased outright (privacy): the wider
    // screening copy must respect that erasure and never re-introduce the
    // raw text through a side channel — secure fields always confirm via the
    // password screen anyway.
    let (name, name_screening_hit) = if secure {
        (String::new(), false)
    } else {
        let raw = proxy.name().await.unwrap_or_default();
        (sanitize_name(&raw, MAX_NAME_CHARS), screening_hit(&raw))
    };
    let (x, y, width, height) = screen_extents(conn, proxy).await.unwrap_or(fallback_bounds);
    ElementInfo {
        role: role.name().to_string(),
        name_screening_hit,
        name,
        x,
        y,
        width,
        height,
        secure,
    }
}

/// Compact text-tree writer: `[i] role "name" (x,y,w,h) flags`, dual caps on
/// depth and node count.
struct TreeWriter<'c> {
    conn: &'c zbus::Connection,
    out: String,
    next_index: usize,
    max_nodes: usize,
    max_depth: u32,
}

impl TreeWriter<'_> {
    fn is_full(&self) -> bool {
        self.next_index >= self.max_nodes
    }

    async fn write_node(&mut self, proxy: &AccessibleProxy<'_>, depth: u32) {
        if self.is_full() {
            return;
        }
        let index = self.next_index;
        self.next_index += 1;

        // A failed role query is handled conservatively as Unknown: secure
        // fallback + name erasure (an Unknown role cannot prove it is not a
        // password field).
        let (role, role_unknown) = match proxy.get_role().await {
            Ok(role) => (role, false),
            Err(_) => (Role::Unknown, true),
        };
        let secure = is_secure_role(role, role_unknown);
        // Nodes that are password fields or of unknown role do not output a
        // name in the tree (the name may be the password content itself);
        // this also saves one D-Bus property query.
        let name = if secure {
            String::new()
        } else {
            sanitize_name(&proxy.name().await.unwrap_or_default(), MAX_NAME_CHARS)
        };
        let state = proxy.get_state().await.ok();
        let extents = screen_extents(self.conn, proxy).await;

        let indent = "  ".repeat(depth as usize);
        let mut line = format!("{indent}[{index}] {} \"{name}\"", role.name());
        if let Some((x, y, width, height)) = extents {
            line.push_str(&format!(" ({x},{y},{width},{height})"));
        }
        let flags = state_flags(secure, state);
        if !flags.is_empty() {
            line.push(' ');
            line.push_str(&flags.join(" "));
        }
        self.out.push_str(&line);
        self.out.push('\n');

        if depth >= self.max_depth {
            return;
        }
        let Ok(children) = proxy.get_children().await else {
            return;
        };
        for child_ref in children {
            if self.is_full() {
                return;
            }
            if child_ref.is_null() {
                continue;
            }
            if let Ok(child) = child_ref.into_accessible_proxy(self.conn).await {
                Box::pin(self.write_node(&child, depth + 1)).await;
            }
        }
    }
}

async fn ui_tree_async(
    conn: &zbus::Connection,
    opts: &UiTreeOptions,
    wayland: bool,
    desktop: &str,
) -> Result<String, ComputerUseError> {
    let root = root_accessible(conn).await?;

    let session = if wayland { "wayland" } else { "x11" };
    // Wayland has no global coordinate space; AT-SPI Component extents are
    // best-effort (Qt is known to be off; on X11/XWayland they are reliable
    // global root-window pixels).
    let extents_note = if wayland {
        "best-effort (wayland: no global coordinate space)"
    } else {
        "screen pixels"
    };
    let mut writer = TreeWriter {
        conn,
        out: format!("# atspi tree session={session} desktop={desktop} extents={extents_note}\n"),
        next_index: 0,
        max_nodes: opts.max_nodes.map_or(DEFAULT_MAX_NODES, |n| n as usize),
        max_depth: opts.max_depth.unwrap_or(DEFAULT_MAX_DEPTH),
    };

    // Observation path: best-effort enumeration (a single hung app is
    // skipped).
    let mut windows = app_windows(conn, &root, false).await?;
    active_first(&mut windows).await;
    if let Some(active) = windows.first() {
        if let Ok(state) = active.get_state().await {
            if state.contains(State::Active) {
                // Regular path: serialize only the active window subtree.
                writer.write_node(active, 0).await;
                if writer.is_full() {
                    writer.out.push_str("# truncated: node cap reached\n");
                }
                return Ok(writer.out);
            }
        }
    }
    // No active window (fullscreen lock screen / empty desktop etc.):
    // degrade to a shallow tree of the registry root (the app list).
    writer.write_node(&root, 0).await;
    if writer.is_full() {
        writer.out.push_str("# truncated: node cap reached\n");
    }
    Ok(writer.out)
}

/// Hit test. The two outcomes have clearly distinct semantics (the review's
/// fail-open fix):
/// - `Ok(None)`: **no element** — none of the enumerated windows covers the
///   point, the AT-SPI hit inside the covering window is empty (null
///   ObjectRef), **or the reachable tree is empty** (the registry answers but
///   no application registered — the normal state when the target app's
///   toolkit a11y is disabled). An empty tree and "the element is not at this
///   point" are indistinguishable here; both are let through under the
///   mainstream None policy (no element → no forced confirmation); this is a
///   policy statement, not a screening proof.
/// - `Err`: **query failure** — a breakdown in any link of root/app/window
///   enumeration, extents, or the hit query is propagated; a query fault is
///   never swallowed into Ok(None)'s "no element" pass justification; how
///   screening handles faults (let it execute, no confirmation) is decided
///   uniformly by the tool layer.
async fn element_at_point_async(
    conn: &zbus::Connection,
    x: i32,
    y: i32,
) -> Result<Option<ElementInfo>, ComputerUseError> {
    let root = root_accessible(conn).await?;
    let mut windows = app_windows(conn, &root, true).await?;
    let has_active = active_first(&mut windows).await;
    // One undecidable window must not abandon the whole hit test. AT-SPI
    // reports all-zero extents for unmapped top-levels — and commonly for
    // every window on Wayland — while `app_windows` enumerates without a
    // visibility filter, so propagating the first extents failure meant a
    // single hidden helper window anywhere on the bus disabled coordinate
    // screening for the entire desktop, which the tool layer then reads as
    // Clear. Skip the undecidable window, remember the fault, and surface it
    // only when no window produced an answer.
    //
    // The **active** window is the one exception. Skipping it and letting a
    // window behind it answer does not just lose screening, it screens the
    // wrong element: on a mixed session a native-Wayland foreground window
    // reports zero extents while an XWayland window underneath reports real
    // ones, so the hit test would return the occluded element — and this
    // backend's verdict now also names the target in the consent dialog and
    // binds the approval token to it. Reporting the fault is honest; a
    // confident wrong answer is not.
    let mut first_fault = None;
    for (index, window) in windows.iter().enumerate() {
        let extents = match screen_extents_strict(conn, window).await {
            Ok(extents) => extents,
            Err(error) => {
                if has_active && index == 0 {
                    return Err(error);
                }
                if first_fault.is_none() {
                    first_fault = Some(error);
                }
                continue;
            }
        };
        if !extents_contain(extents, x, y) {
            continue; // definitively does not cover the point.
        }
        let component = component_of(conn, window).await.ok_or_else(|| {
            ComputerUseError::unavailable("AT-SPI Component proxy unavailable for window")
        })?;
        let target_ref = component
            .get_accessible_at_point(x, y, CoordType::Screen)
            .await
            .map_err(|error| {
                ComputerUseError::unavailable(format!("AT-SPI hit test at ({x}, {y}): {error}"))
            })?;
        if target_ref.is_null() {
            continue; // AT-SPI explicitly returned empty: no element inside this window.
        }
        let target = target_ref
            .into_accessible_proxy(conn)
            .await
            .map_err(|error| {
                ComputerUseError::unavailable(format!("AT-SPI target proxy: {error}"))
            })?;
        let info = element_info_of(conn, &target, (x, y, 0, 0)).await;
        return Ok(Some(info));
    }
    // No window covered the point. If some window was undecidable, the answer
    // is "query failure", not "nothing there" — the fault is only swallowed
    // when another window answered.
    match first_fault {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

/// Focused element. atspi 0.30's proxy layer has no GetFocusedObject-style
/// query (focus can only be accumulated asynchronously from the event
/// stream), so the next best thing is used: **find the deepest node whose
/// state contains FOCUSED in the reachable tree of the active window**, under
/// the dual constraint of the [`FOCUSED_SEARCH_MAX_NODES`] node budget and
/// the operation-level deadline (this route is chosen over returning
/// `Err(unsupported)`: tree search works on both X11 and Wayland, and the
/// existing AT-SPI channel should not go to waste).
///
/// Two constraints that bear directly on whether the password field promise
/// holds:
/// - **Search only inside the active window.** Keyboard input necessarily
///   lands in the active window; a stale FOCUSED in a background window
///   (some toolkits never clear that state) would make the query "succeed"
///   but answer wrong — screening would evaluate a dead node while the input
///   lands in the active window's password field.
/// - **Window/container nodes themselves are not answers; take the deepest
///   FOCUSED.** Some toolkits (e.g. Qt with a top-level holding focus) put
///   FOCUSED on the top-level container; returning the container would make
///   keyboard screening and password determination evaluate the whole window
///   instead of the component where the input focus really is.
///
/// Outcome semantics: not found in the active window → `Ok(None)`
/// (confirmed no focused element: some toolkits do not implement the FOCUSED
/// state; the tool layer treats screening as unavailable and fails open,
/// consistent with the established semantics); budget exhausted without a
/// verdict → `Err` (when the result is uncertain, never masquerade as
/// "none" — the error is clearly distinct from "no element", disposition is
/// decided by the tool layer). The same strictness covers the active-window
/// selection below: when no active window is found but at least one
/// per-window state query faulted, the fault is raised instead of a
/// misleading `Ok(None)` (a query fault is never swallowed into a pass
/// justification).
async fn focused_element_async(
    conn: &zbus::Connection,
) -> Result<Option<ElementInfo>, ComputerUseError> {
    let root = root_accessible(conn).await?;
    let windows = app_windows(conn, &root, true).await?;
    let mut active = None;
    // First per-window state fault, kept so a total fault-out is not
    // mistaken for a confirmed "no active window" (see the None arm below).
    let mut state_fault = None;
    for window in &windows {
        match window.get_state().await {
            Ok(state) => {
                if state.contains(State::Active) {
                    active = Some(window);
                    break;
                }
            }
            Err(error) => {
                if state_fault.is_none() {
                    state_fault = Some(ComputerUseError::unavailable(format!(
                        "AT-SPI window state query: {error}"
                    )));
                }
            }
        }
    }
    let Some(window) = active else {
        // No active window AND at least one state query faulted (e.g. a
        // transient a11y-bus fault inside the method timeout): Ok(None) here
        // would claim "confirmed no focused element" off an unverified
        // enumeration — raise the fault instead, matching the strict rule
        // this module applies to every other screening query. The tool layer
        // fails open on Err (the action still executes) while masking the
        // typed-text preview as a secure target.
        if let Some(fault) = state_fault {
            return Err(fault);
        }
        return Ok(None);
    };
    let mut budget = FOCUSED_SEARCH_MAX_NODES;
    let found = find_focused_in_subtree(conn, window, true, &mut budget).await?;
    if found.is_none() && budget == 0 {
        return Err(ComputerUseError::unavailable(
            "focused element search exhausted its node budget without a definitive answer",
        ));
    }
    Ok(found)
}

/// DFS through the subtree for the **deepest** node whose state contains
/// FOCUSED; `budget` limits the number of visited nodes. `is_root` marks the
/// window-root call: the window node's own FOCUSED is not an answer (see the
/// M2 notes in [`focused_element_async`]).
async fn find_focused_in_subtree(
    conn: &zbus::Connection,
    proxy: &AccessibleProxy<'_>,
    is_root: bool,
    budget: &mut usize,
) -> Result<Option<ElementInfo>, ComputerUseError> {
    if *budget == 0 {
        return Ok(None);
    }
    *budget -= 1;
    let state = proxy
        .get_state()
        .await
        .map_err(|error| ComputerUseError::unavailable(format!("AT-SPI state query: {error}")))?;
    if !state.contains(State::Focused) {
        return find_focused_among_children(conn, proxy, budget).await;
    }
    // FOCUSED on this node: first look deeper to confirm whether a deeper
    // focus exists (a container and its focused descendants may both carry
    // FOCUSED; the deepest one is the real input focus); without a deeper
    // one, this node is the answer — except the window root itself
    // (container-level FOCUSED cannot locate a password field, M2).
    if let Some(found) = find_focused_among_children(conn, proxy, budget).await? {
        return Ok(Some(found));
    }
    if is_root {
        return Ok(None);
    }
    Ok(Some(element_info_of(conn, proxy, (0, 0, 0, 0)).await))
}

/// Iterates the direct children, returning the first FOCUSED node found
/// within a subtree.
async fn find_focused_among_children(
    conn: &zbus::Connection,
    proxy: &AccessibleProxy<'_>,
    budget: &mut usize,
) -> Result<Option<ElementInfo>, ComputerUseError> {
    let children = proxy
        .get_children()
        .await
        .map_err(|error| ComputerUseError::unavailable(format!("AT-SPI node children: {error}")))?;
    for child_ref in children {
        if child_ref.is_null() {
            continue;
        }
        let child = child_ref
            .into_accessible_proxy(conn)
            .await
            .map_err(|error| {
                ComputerUseError::unavailable(format!("AT-SPI child proxy: {error}"))
            })?;
        if let Some(found) = Box::pin(find_focused_in_subtree(conn, &child, false, budget)).await? {
            return Ok(Some(found));
        }
        if *budget == 0 {
            return Ok(None);
        }
    }
    Ok(None)
}

/// AT-SPI init: first turn on the session a11y switch (Electron/Chromium
/// only build the accessibility tree once an AT has registered; this call
/// failing is non-fatal), then **self-build** the a11y bus connection.
///
/// The connection is built with `zbus::connection::Builder` and gets a
/// `method_timeout` (3s). zbus's default timeout is very
/// generous, and atspi's `AccessibilityConnection` does not allow injecting
/// a self-built connection — so, following the same flow as atspi's
/// `AccessibilityConnection::new`, the bus address is fetched ourselves
/// (`org.a11y.Bus.GetAddress`) and the connection built, using atspi's proxy
/// types directly. (zbus 5's connection::Builder only has method_timeout as
/// a timeout setting point; the operation-level overall deadline is
/// backstopped by [`LinuxComputerUseBackend::block_on_a11y`].)
async fn a11y_connect() -> Result<zbus::Connection, String> {
    // This call goes over zbus's default connection
    // (method_timeout None — exactly the hazard described in the comment
    // below), so it must be time-bounded overall — a bus that accepts the
    // connection but never answers would pin create_backend on the worker
    // thread forever. Failure is already handled as non-fatal.
    // Deliberately not calling set_session_accessibility(false) at teardown:
    // the switch is session-global state that a real
    // screen reader user may be relying on — removing it would directly
    // break their assistive technology; the cost of leaving it on is only
    // that desktop apps keep maintaining their a11y trees).
    let _ = tokio::time::timeout(
        2 * A11Y_METHOD_TIMEOUT,
        atspi::connection::set_session_accessibility(true),
    )
    .await;
    // The bootstrap session-bus connection needs a deadline too: zbus 5
    // defaults method_timeout to None, so a wedged session bus would hang
    // `create_backend` on the worker thread forever. Bound it with the same
    // 3s method timeout as the self-built a11y bus connection below.
    let session = zbus::connection::Builder::session()
        .map_err(|error| format!("session bus builder: {error}"))?
        .method_timeout(A11Y_METHOD_TIMEOUT)
        .build()
        .await
        .map_err(|error| format!("session bus: {error}"))?;
    let bus = BusProxy::new(&session)
        .await
        .map_err(|error| format!("a11y bus address proxy: {error}"))?;
    let address: zbus::Address = bus
        .get_address()
        .await
        .map_err(|error| format!("a11y bus address: {error}"))?
        .parse()
        .map_err(|error| format!("a11y bus address parse: {error}"))?;
    zbus::connection::Builder::address(address)
        .map_err(|error| format!("a11y connection builder: {error}"))?
        .method_timeout(A11Y_METHOD_TIMEOUT)
        .build()
        .await
        .map_err(|error| format!("a11y bus connection: {error}"))
}

/// Wayland screenshot probe: enumerates monitors and actually grabs a frame
/// from the primary (fallback first) monitor. Any link of xcap's Wayland
/// chain (GNOME Shell D-Bus → portal Screenshot → wlroots wayshot) being
/// available means success; the portal path may show an authorization dialog
/// to the user.
fn probe_wayland_screenshot() -> Result<(), String> {
    let monitors =
        Monitor::all().map_err(|error| format!("monitor enumeration failed: {error}"))?;
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())
        .ok_or_else(|| "no monitors reported".to_string())?;
    let image = monitor
        .capture_image()
        .map_err(|error| format!("capture failed: {error}"))?;
    if image.width() == 0 || image.height() == 0 {
        return Err("capture returned an empty image".to_string());
    }
    Ok(())
}

/// Wayland probe time limit. xcap's portal answer is an **unbounded** D-Bus
/// wait (internally `receiver.recv()??`, and KDE may pop an interactive
/// dialog each time) — once it is waiting on a human, the probe never
/// returns, and catch_unwind cannot stop a hang: the backend worker would be
/// pinned forever, and once the backend layer's call budget is exhausted the
/// in-flight gate and the control channel (emergency release, grant release)
/// all jam. On timeout, give up and abandon the probe
/// thread (pure capture, no shared state touched — better than a stuck
/// worker). Each abandonment is counted in [`WAYLAND_PROBE_ABANDONED`]:
/// `ensure_wayland_capture` re-probes on every capture while the probe has
/// not succeeded, so a permanently wedged portal would otherwise accumulate
/// one non-terminating thread per capture, forever — after
/// [`WAYLAND_PROBE_ABANDONMENT_CAP`] timeouts the retry stops and screenshot
/// capture sticks unavailable until restart (a bounded leak beats an
/// unbounded one, and beats the alternative of one hung worker).
const WAYLAND_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Count of probe threads abandoned to an unbounded portal wait (process
/// global: a restart is the documented recovery, see
/// [`WAYLAND_PROBE_TIMEOUT`]).
static WAYLAND_PROBE_ABANDONED: AtomicUsize = AtomicUsize::new(0);

/// Abandonment cap after which re-probing stops: at most this many
/// non-terminating probe threads can accumulate against a wedged portal.
const WAYLAND_PROBE_ABANDONMENT_CAP: usize = 3;

/// xcap's Wayland path contains `.expect(...)` internally (PNG re-encode);
/// wrapping it in catch_unwind turns a potential panic into an explicit
/// error instead of blowing up the backend worker thread; the whole probe
/// runs under a time limit (see [`WAYLAND_PROBE_TIMEOUT`]).
fn probe_wayland_screenshot_guarded() -> Result<(), String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("computer-use-wayland-probe".to_string())
        .spawn(move || {
            let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                probe_wayland_screenshot,
            )) {
                Ok(inner) => inner,
                Err(_) => Err("capture panicked inside xcap's Wayland fallback chain".to_string()),
            };
            let _ = sender.send(result);
        })
        .map_err(|error| format!("capture probe thread spawn failed: {error}"))?;
    match receiver.recv_timeout(WAYLAND_PROBE_TIMEOUT) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // The abandoned thread stays blocked in xcap's unbounded portal
            // wait; count it so ensure_wayland_capture can stop re-probing
            // once the accumulation cap is reached.
            WAYLAND_PROBE_ABANDONED.fetch_add(1, Ordering::Relaxed);
            Err(format!(
                "the Wayland capture probe did not answer within {}s (the portal screenshot \
                 request may be waiting on an interactive dialog)",
                WAYLAND_PROBE_TIMEOUT.as_secs()
            ))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("capture probe thread terminated unexpectedly".to_string())
        }
    }
}

pub(super) struct LinuxComputerUseBackend {
    session: SessionInfo,
    /// Dedicated current-thread runtime: the bridge for atspi (async/zbus)
    /// inside the synchronous trait.
    runtime: tokio::runtime::Runtime,
    /// Self-built a11y bus connection (with method_timeout, see
    /// [`a11y_connect`]).
    a11y: Option<zbus::Connection>,
    a11y_init_error: Option<String>,
    /// Constructed on X11 only; Wayland input goes through `wayland_portal`.
    input: Option<Enigo>,
    input_init_error: Option<String>,
    /// Constructed on Wayland only (portal RemoteDesktop, lazily started
    /// session).
    wayland_portal: Option<PortalInput>,
    wayland_portal_error: Option<String>,
    wayland_screenshot_ok: bool,
    wayland_screenshot_error: Option<String>,
    /// The most recent failure reason of the same-session screenshot stream
    /// (surfaced alongside the error after falling back to the xcap chain).
    wayland_portal_capture_error: Option<String>,
    /// Set once a portal-stream capture attempt failed while a portal
    /// session was alive: capabilities then still claimed
    /// stream capture while actual captures silently fell back to the xcap
    /// primary-screen chain, whose multi-monitor coordinates do not align
    /// with input — the degradation must surface in the capabilities note).
    wayland_portal_capture_degraded: bool,
    /// When the most recent input action started: the same-session
    /// screenshot stream is damage-driven, so a follow-up screenshot must
    /// wait for a frame newer than it to see the post-action picture (when
    /// nothing changed visually, the existing frame is reused).
    last_input_at: Option<Instant>,
    /// Cancel flag of the current request (set by the worker before
    /// dispatching, cleared after; see
    /// [`ComputerUseBackend::set_cancel_flag`]): type's per-event injection
    /// checks it between events, so a request the caller already abandoned
    /// after its timeout stops injecting immediately.
    cancel: Option<Arc<AtomicBool>>,
}

impl LinuxComputerUseBackend {
    fn is_wayland(&self) -> bool {
        self.session.kind == SessionKind::Wayland
    }

    /// Timestamps the entry of an input action (a follow-up screenshot waits
    /// for frames newer than it).
    fn note_input(&mut self) {
        self.last_input_at = Some(Instant::now());
    }

    /// Wayland's preferred capture: the portal same-session ScreenCast
    /// stream (PipeWire). Starts the session if not started (the first
    /// screenshot opens the system authorization dialog once, shared with
    /// input). On failure, returns None and records the reason
    /// (`wayland_portal_capture_error`, surfaced alongside the error after
    /// falling back to the xcap chain).
    fn wayland_portal_capture(&mut self) -> Option<Capture> {
        let portal = self.wayland_portal.as_mut()?;
        let frame = match portal.capture_frame(self.last_input_at) {
            Ok(frame) => frame,
            Err(error) => {
                self.wayland_portal_capture_error = Some(error.to_string());
                self.wayland_portal_capture_degraded = true;
                return None;
            }
        };
        // A healthy portal capture clears the degraded note: previously a
        // single transient failure (e.g. one missed first-frame budget under
        // compositor load) left the capability notes describing the
        // fallback/refusal regime for the rest of the session even after
        // full-aligned captures resumed (round-17 minor).
        self.wayland_portal_capture_error = None;
        self.wayland_portal_capture_degraded = false;
        // Input coordinates = stream-local pixels (mutter semantics; the
        // origin is handled inside the compositor/portal layer). KDE's input
        // unit is stream-local logical pixels while its buffer is physical
        // pixels, so the scale is derived from the stream size reported by
        // the compositor; other compositors (including KDE at scale=1) use a
        // scale of 1.
        let input_scale = if self
            .session
            .desktop
            .as_deref()
            .map_or(false, |desktop| desktop.contains("KDE"))
        {
            match portal.stream_logical_size() {
                Some((lw, lh)) if lw > 0 && lh > 0 && frame.width > 0 && frame.height > 0 => (
                    f64::from(lw) / f64::from(frame.width),
                    f64::from(lh) / f64::from(frame.height),
                ),
                _ => (1.0, 1.0),
            }
        } else {
            (1.0, 1.0)
        };
        Some(Capture {
            rgba: frame.rgba,
            width: frame.width,
            height: frame.height,
            origin_x: 0,
            origin_y: 0,
            input_scale_x: input_scale.0,
            input_scale_y: input_scale.1,
            input_aligned: true,
        })
    }
    fn require_enigo(&mut self) -> Result<&mut Enigo, ComputerUseError> {
        if self.is_wayland() {
            return Err(ComputerUseError::unsupported(
                "input",
                "XTEST input is not used on Wayland sessions (XWayland would only reach \
                 X11 clients); input goes through the portal RemoteDesktop backend",
            ));
        }
        match self.input.as_mut() {
            Some(enigo) => Ok(enigo),
            None => {
                let detail = self
                    .input_init_error
                    .as_deref()
                    .unwrap_or("unknown error")
                    .to_string();
                Err(ComputerUseError::unavailable(format!(
                    "X11 input connection (XTEST) failed at backend init: {detail}"
                )))
            }
        }
    }

    /// Wayland portal input backend (the sticky probe-failure error lives in
    /// `wayland_portal_error`). After a failed Wayland screenshot probe,
    /// retry on the next capture: a single probe failure must not stick for
    /// the whole backend lifetime — a
    /// portal dialog accidentally dismissed by the user or a brief compositor
    /// hiccup would otherwise lose screenshots permanently, with no retry
    /// entry point.
    fn ensure_wayland_capture(&mut self) -> Result<(), ComputerUseError> {
        if !self.is_wayland() || self.wayland_screenshot_ok {
            return Ok(());
        }
        // Sticky abandonment cap: each timed-out probe leaves one thread
        // blocked in xcap's unbounded portal wait, so past the cap a wedged
        // portal would keep leaking one non-terminating thread per capture —
        // stop re-probing and fail with the distinct sticky error (the
        // process-global counter resets only on restart; that is the
        // documented recovery, see WAYLAND_PROBE_TIMEOUT).
        let abandoned = WAYLAND_PROBE_ABANDONED.load(Ordering::Relaxed);
        if abandoned >= WAYLAND_PROBE_ABANDONMENT_CAP {
            let message = format!(
                "portal probe timed out {abandoned} times; screenshot capture disabled \
                 until restart"
            );
            self.wayland_screenshot_error = Some(message.clone());
            return Err(ComputerUseError::unsupported("screenshot", message));
        }
        match probe_wayland_screenshot_guarded() {
            Ok(()) => {
                self.wayland_screenshot_ok = true;
                self.wayland_screenshot_error = None;
            }
            Err(error) => self.wayland_screenshot_error = Some(error),
        }
        if !self.wayland_screenshot_ok {
            let portal_note = self
                .wayland_portal_capture_error
                .as_deref()
                .map(|error| format!("; same-session stream capture failed: {error}"))
                .unwrap_or_default();
            return Err(ComputerUseError::unsupported(
                "screenshot",
                format!(
                    "screenshot on Wayland compositor {}: {}{portal_note}",
                    self.session.desktop_label(),
                    self.wayland_screenshot_error
                        .as_deref()
                        .unwrap_or("probe failed")
                ),
            ));
        }
        Ok(())
    }

    fn require_portal(&mut self) -> Result<&mut PortalInput, ComputerUseError> {
        match self.wayland_portal.as_mut() {
            Some(portal) => Ok(portal),
            None => {
                let detail = self
                    .wayland_portal_error
                    .as_deref()
                    .unwrap_or("unknown error");
                Err(ComputerUseError::unavailable(format!(
                    "Wayland input via xdg-desktop-portal RemoteDesktop is not available: \
                     {detail}"
                )))
            }
        }
    }

    fn require_a11y(&self) -> Result<&zbus::Connection, ComputerUseError> {
        match self.a11y.as_ref() {
            Some(conn) => Ok(conn),
            None => {
                let detail = self.a11y_init_error.as_deref().unwrap_or("unknown error");
                Err(ComputerUseError::unavailable(format!(
                    "AT-SPI bus connection failed at backend init: {detail}"
                )))
            }
        }
    }

    /// Bridge for a11y async operations: current-thread runtime `block_on` +
    /// an operation-level overall deadline (a second backstop beyond zbus's
    /// `method_timeout`; a timeout cannot distinguish "no result" from
    /// "failure", so it is always reported as unavailable and never
    /// masquerades as "no result").
    fn block_on_a11y<T>(
        &self,
        deadline: Duration,
        what: &str,
        fut: impl Future<Output = Result<T, ComputerUseError>>,
    ) -> Result<T, ComputerUseError> {
        self.runtime.block_on(async move {
            match tokio::time::timeout(deadline, fut).await {
                Ok(result) => result,
                Err(_) => Err(ComputerUseError::unavailable(format!(
                    "AT-SPI operation '{what}' did not finish within {deadline:?}"
                ))),
            }
        })
    }

    fn press_chord(enigo: &mut Enigo, keys: &[enigo::Key]) -> Result<(), ComputerUseError> {
        for (index, key) in keys.iter().enumerate() {
            if let Err(error) = enigo.key(*key, Direction::Press) {
                // On the error path, release the already-pressed keys so
                // modifiers cannot strand pressed.
                for held in keys[..index].iter().rev() {
                    let _ = enigo.key(*held, Direction::Release);
                }
                return Err(input_failed("key press", error));
            }
        }
        Ok(())
    }

    fn release_chord(enigo: &mut Enigo, keys: &[enigo::Key]) -> Result<(), ComputerUseError> {
        // Even on a mid-way failure, best-effort release every key, otherwise
        // stranded modifiers corrupt all subsequent input; the first error is
        // returned so the upper layer can notice.
        let mut first_err = None;
        for key in keys.iter().rev() {
            if let Err(error) = enigo.key(*key, Direction::Release) {
                if first_err.is_none() {
                    first_err = Some(error);
                }
            }
        }
        match first_err {
            Some(error) => Err(input_failed("key release", error)),
            None => Ok(()),
        }
    }
}

impl ComputerUseBackend for LinuxComputerUseBackend {
    /// Triggered by the command layer via the registry's (BackendRegistry)
    /// `release_os_grant` request when the user revokes the grant / global
    /// stop / master switch off: closes the portal session while keeping the
    /// backend usable (after `close`, `session` is None and the next input
    /// action lazily rebuilds through the existing path; if needed the user
    /// will see the system authorization dialog again). X11 sessions have no
    /// persistent grant to begin with; this is a no-op when `wayland_portal`
    /// is None.
    fn release_os_grant(&mut self) -> Result<(), ComputerUseError> {
        if let Some(portal) = self.wayland_portal.as_mut() {
            portal.close();
        }
        Ok(())
    }

    fn set_cancel_flag(&mut self, flag: Option<Arc<AtomicBool>>) {
        self.cancel = flag;
    }

    fn capabilities(&self) -> Capabilities {
        let ui_tree = self.a11y.is_some();
        if self.is_wayland() {
            let portal_ok = self.wayland_portal.is_some();
            let screenshot_note = if portal_ok {
                "screenshot via the same-session ScreenCast stream (PipeWire; the first \
                 screenshot or input action opens one system authorization dialog)"
                    .to_string()
            } else if self.wayland_screenshot_ok {
                "screenshot via the xcap GNOME-Shell/portal/wlroots fallback chain \
                 (the portal path may show a compositor permission dialog per capture)"
                    .to_string()
            } else {
                format!(
                    "screenshot unavailable: {}",
                    self.wayland_screenshot_error
                        .as_deref()
                        .unwrap_or("probe failed")
                )
            };
            let input_note = match (&self.wayland_portal, &self.wayland_portal_error) {
                (Some(_), _) => "input via xdg-desktop-portal RemoteDesktop (the first \
                                 screenshot or input action opens a system authorization \
                                 dialog)"
                    .to_string(),
                (None, Some(error)) => format!("input unavailable: {error}"),
                (None, None) => "input unavailable".to_string(),
            };
            // Honest disclosure: Wayland input is treated as
            // experimental overall (compositor implementation differences);
            // injection of non-Latin-1 text is explicitly rejected (mutter
            // silently drops keysyms outside the keymap, see
            // wayland_portal::char_keysym). Latin-1 characters are always
            // sent as-is — the portal path cannot check the active keymap —
            // so they can equally be silently dropped when the keymap lacks
            // them; only the explicit rejection is a guarantee.
            let experimental = "Wayland input is experimental (compositor implementations \
                 differ), typing non-Latin-1 text (CJK etc.) is explicitly rejected: \
                 mutter silently drops keysyms outside the active keymap, and Latin-1 \
                 characters are sent unmapped and may equally be dropped when the active \
                 keymap lacks them";
            // Honest disclosure: the portal keysym mapping covers F1-F12 only
            // (wayland_portal::map_keysym fails closed beyond, deliberately —
            // mutter silently drops what the active keymap lacks), narrower
            // than X11's F1-F20; the model must not assume F13-F20 work here.
            let fkey_range = "function keys above F12 are rejected (the portal mapping \
                 covers F1-F12, unlike X11's F1-F20)";
            // Honest disclosure: when the portal stream is unavailable,
            // the xcap fallback goes through XCB/XWayland or a compositor
            // screenshot protocol, whose multi-monitor coordinate system is
            // not aligned with input (stream-logical coordinates); the
            // fallback path also always captures the primary monitor
            // (self.input is always None → cursor anchoring unavailable), so
            // the model is never told the other screens are invisible.
            let fallback_note = if (self.wayland_screenshot_ok && !portal_ok)
                || self.wayland_portal_capture_degraded
            {
                "; capture is on the xcap fallback: coordinate input is REFUSED while the \
                 fallback runs at a display scale other than 100% (the fallback's map into \
                 the portal input space is unverified and mispointing is unsafe, so the \
                 refusal is deliberate); at 100% scale multi-monitor coordinate alignment \
                 with input is best-effort, only the PRIMARY monitor is captured/input-able, \
                 and the compositor may prompt per capture"
            } else {
                ""
            };
            Capabilities {
                screenshot: portal_ok || self.wayland_screenshot_ok,
                input: self.wayland_portal.is_some(),
                ui_tree,
                notes: format!(
                    "Wayland session ({}): {input_note}; {screenshot_note}{fallback_note}; \
                     {experimental}; {fkey_range}; \
                     AT-SPI bounds best-effort on Wayland",
                    self.session.desktop_label()
                ),
            }
        } else {
            let input_note = if self.input.is_some() {
                "XTEST input ready".to_string()
            } else {
                format!(
                    "input unavailable: {}",
                    self.input_init_error.as_deref().unwrap_or("unknown error")
                )
            };
            // Honest disclosure: when WAYLAND_DISPLAY
            // is set in an X11 session, xcap's own detection prefers the
            // Wayland chain — capture and XTEST input would run on different
            // planes.
            let mismatch = if self.session.has_wayland_display {
                "; WARNING: WAYLAND_DISPLAY is set in this X11 session, so capture may be \
                 routed through the Wayland portal chain while input targets X11 (unset \
                 WAYLAND_DISPLAY for consistent behavior)"
            } else {
                ""
            };
            // Honest disclosure (matching the PR body): enigo's X11 per-char
            // Unicode injection resolves each character at keymap column 0
            // and does not clear held modifiers — with CapsLock on or Shift
            // held, typed lowercase can come out corrupted (the same class
            // as xdotool without --clearmodifiers). The model must see this
            // before typing under those modifier states.
            let typing_caveat = "; typing caveat: typed lowercase can corrupt while CapsLock is \
                 on or Shift is held (enigo's X11 injection does keymap column-0 lookups and \
                 does not clear modifiers — same class as xdotool without --clearmodifiers)";
            // Honest disclosure (matching the optimistic `screenshot: true`):
            // the capability bit is declared without a probe; capture errors
            // surface per call. On X11 some RandR layouts — notably a
            // monitor at a negative origin — cannot be captured at all, so
            // the model must expect a named per-call error instead of
            // assuming every capture succeeds. And an unredirected fullscreen
            // GL window (compositor bypass, routine on NVIDIA) can capture as
            // black or frozen WITHOUT an error — the silent-wrong case is the
            // dangerous one, so it is disclosed rather than promised away.
            let capture_caveat = "; the screenshot capability is declared without probing \
                 and capture errors surface per call (some X11 RandR layouts, e.g. a \
                 negative-origin monitor, cannot be captured); a fullscreen unredirected \
                 GL window may capture as black or a frozen frame with NO error — \
                 re-capture and verify before acting on its content";
            Capabilities {
                screenshot: true,
                input: self.input.is_some(),
                ui_tree,
                notes: format!(
                    "X11 session ({}): full support (xcap capture, {input_note}, AT-SPI \
                     tree{}){mismatch}; capture is cursor-anchored, so on multi-monitor setups \
                     only the monitor holding the cursor is visible/clickable this turn \
                     (move the pointer there via an initial screenshot on that \
                     monitor){typing_caveat}{capture_caveat}",
                    self.session.desktop_label(),
                    if ui_tree { "" } else { " unavailable" },
                ),
            }
        }
    }

    fn capture(&mut self) -> Result<Capture, ComputerUseError> {
        // Wayland preference: the portal same-session screenshot stream
        // (shares one authorization with input, no per-capture dialogs).
        if self.is_wayland() {
            if let Some(capture) = self.wayland_portal_capture() {
                return Ok(capture);
            }
        }
        // Fallback: the xcap chain (the X11 main path; on Wayland a failed
        // probe can retry on a later capture — a single probe failure must
        // not stick for the whole backend lifetime, otherwise a portal
        // dialog accidentally dismissed by the user or a brief compositor
        // hiccup loses screenshots permanently, with no retry entry point).
        self.ensure_wayland_capture()?;
        let monitors = Monitor::all().map_err(|error| {
            ComputerUseError::unavailable(format!("monitor enumeration: {error}"))
        })?;
        // On X11 xcap uses Xft.dpi/96 as the scale and divides RandR geometry
        // by it; capture and XTEST both operate on the root-window physical
        // pixel plane, so the input scale is always 1.0 and the origin is
        // multiplied back by the scale.
        let scale = monitors
            .iter()
            .find_map(|m| m.scale_factor().ok())
            .filter(|s| *s > 0.0)
            .unwrap_or(1.0);
        let cursor = self.input.as_mut().and_then(|enigo| enigo.location().ok());
        let contains = |monitor: &Monitor, lx: i32, ly: i32| -> bool {
            match (monitor.x(), monitor.y(), monitor.width(), monitor.height()) {
                (Ok(x), Ok(y), Ok(w), Ok(h)) => {
                    lx >= x && lx < x + w as i32 && ly >= y && ly < y + h as i32
                }
                _ => false,
            }
        };
        let monitor = cursor
            .and_then(|(cx, cy)| {
                // The cursor is in root-window pixels; convert to xcap's
                // logical coordinates before the containment test.
                let lx = (cx as f32 / scale) as i32;
                let ly = (cy as f32 / scale) as i32;
                monitors.iter().find(|m| contains(m, lx, ly)).cloned()
            })
            .or_else(|| {
                monitors
                    .iter()
                    .find(|m| m.is_primary().unwrap_or(false))
                    .cloned()
            })
            .or_else(|| monitors.first().cloned())
            .ok_or_else(|| {
                ComputerUseError::unavailable("no monitors reported by the display server")
            })?;
        // The origin math below must use the CAPTURED monitor's own scale, not
        // whichever monitor enumerated first: on mixed-DPI multi-monitor
        // layouts a secondary at a different scale would otherwise get a
        // mis-multiplied origin. Residual (xcap 0.9.8): its X11 scale_factor
        // is a global Xft.dpi/96 lookup — the same value for every monitor —
        // and its Wayland path reports the max over all outputs, so a true
        // per-output mixed-DPI scale cannot be expressed through this API and
        // the origin stays wrong when the captured monitor's real per-output
        // scale differs (disclosed; no per-monitor scale API exists to call).
        // The cursor→logical conversion above has the same limitation (the
        // captured monitor is not yet known there).
        let scale = monitor
            .scale_factor()
            .ok()
            .filter(|s| *s > 0.0)
            .unwrap_or(scale);
        // xcap's Wayland chain contains `.expect(...)` internally (PNG
        // re-encode, see the same wrapping at the probe): per-frame capture is
        // likewise wrapped in catch_unwind, turning a potential panic into an
        // explicit error instead of blowing up the backend worker thread.
        let captured =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| monitor.capture_image()));
        let image = match captured {
            Ok(result) => result.map_err(|error| {
                if self.is_wayland() {
                    ComputerUseError::unsupported(
                        "screenshot",
                        format!(
                            "screenshot on Wayland compositor {}: capture failed: {error}",
                            self.session.desktop_label()
                        ),
                    )
                } else {
                    // The capture library passes the RandR
                    // monitor's raw x/y into GetImage on the root window, so
                    // a monitor placed left/above the primary (negative
                    // origin) fails unconditionally — name the cause instead
                    // of an opaque failure.
                    let (mx, my) = (monitor.x().unwrap_or(0), monitor.y().unwrap_or(0));
                    if mx < 0 || my < 0 {
                        ComputerUseError::unavailable(format!(
                            "the monitor at negative origin ({mx},{my}) cannot be captured: \
                             the capture library cannot address screens placed left/above \
                             the primary monitor; capture the primary screen instead, or \
                             arrange the displays so this one is not left of/above the \
                             primary (underlying error: {error})"
                        ))
                    } else {
                        ComputerUseError::unavailable(format!("monitor capture failed: {error}"))
                    }
                }
            })?,
            Err(_) => {
                return Err(if self.is_wayland() {
                    ComputerUseError::unsupported(
                        "screenshot",
                        format!(
                            "screenshot on Wayland compositor {}: capture panicked inside \
                             xcap's fallback chain",
                            self.session.desktop_label()
                        ),
                    )
                } else {
                    ComputerUseError::unavailable("monitor capture panicked")
                });
            }
        };
        let width = image.width();
        let height = image.height();
        // X11: xcap reports a logical origin; multiplying back by the scale
        // gives root-window physical pixels (= the input space, input scale
        // 1.0). Wayland: this fallback path is ONLY reached when the portal
        // capture stream is dead (the live PipeWire path pipes buffer-pixel
        // space directly from the stream). It assumes xcap's logical
        // geometry and the portal input space coincide (origin not
        // multiplied, input scale 1/scale — see the wayland_portal module
        // docs, which describe NotifyPointerMotionAbsolute as stream-LOCAL
        // BUFFER pixels under mutter). Under that reading this path is
        // inverted at scale != 1 (mispoint ~scale²) and double-counts the
        // monitor origin on multi-monitor, the same class of unverified gap
        // as the AvailableCursorModes probe: it needs a live-portal check
        // before the math is trusted or changed. Until then the map is only
        // handed out as `input_aligned` at scale == 1.0; at any other scale
        // the capture is marked unaligned and coordinate input REFUSES
        // instead of injecting at a believed-wrong position (a capabilities
        // note the model may disregard does not make a mispointed click
        // safe).
        let (origin_x, origin_y, input_scale_x, input_scale_y, input_aligned) = if self.is_wayland()
        {
            let scale = f64::from(scale);
            let origin_x = monitor.x().unwrap_or(0);
            let origin_y = monitor.y().unwrap_or(0);
            (origin_x, origin_y, 1.0 / scale, 1.0 / scale, scale == 1.0)
        } else {
            let origin_x = monitor
                .x()
                .map(|x| (x as f32 * scale).round() as i32)
                .unwrap_or(0);
            let origin_y = monitor
                .y()
                .map(|y| (y as f32 * scale).round() as i32)
                .unwrap_or(0);
            (origin_x, origin_y, 1.0, 1.0, true)
        };
        Ok(Capture {
            rgba: image.into_raw(),
            width,
            height,
            origin_x,
            origin_y,
            input_scale_x,
            input_scale_y,
            input_aligned,
        })
    }

    fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
        if self.is_wayland() {
            return match self.require_portal()?.last_pointer() {
                // track_pointer records input-space coordinates (the same
                // space move_to/click consume): stream-logical pixels under
                // KDE fractional scaling, buffer pixels elsewhere. The
                // trait contract returns input space directly — globally
                // unambiguous, with no round-trip through the ill-defined
                // device-pixel space (the old division by the last
                // capture's input scale double-scaled under KDE fractional
                // scaling whenever the captured map differed).
                Some(position) => Ok(position),
                None => Err(ComputerUseError::unsupported(
                    "cursor_position",
                    "Wayland exposes no cursor query API; the position becomes known after \
                     the first mouse_move of an authorized portal session",
                )),
            };
        }
        let enigo = self.require_enigo()?;
        enigo
            .location()
            .map_err(|error| input_failed("cursor position query", error))
    }

    fn move_to(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            portal.ensure_started()?;
            portal.motion_absolute(x, y)?;
            portal.track_pointer(x, y);
            return Ok(());
        }
        let enigo = self.require_enigo()?;
        enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|error| input_failed("mouse move", error))
    }

    fn click(&mut self, button: MouseButton, count: u8) -> Result<(), ComputerUseError> {
        let count = count.max(1);
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            portal.ensure_started()?;
            let evdev = wayland_portal::map_button(button);
            // Leave a settle window between the move and the click (the same
            // injection race buffer as XTEST).
            settle();
            for i in 0..count {
                if let Err(error) = portal.button(evdev, true) {
                    // A failed press ≠ not delivered (the timeout says
                    // nothing about delivery): best-effort
                    // issue one extra release on the still-open poisoned
                    // session — a release that did not land is a
                    // compositor-side no-op, and one that did land avoids a
                    // stranded button (mutter does not synthesize release
                    // events when closing a session).
                    let _ = portal.button(evdev, false);
                    return Err(error);
                }
                if let Err(error) = portal.button(evdev, false) {
                    // Same best-effort retry as the press path above: a
                    // failed release notify may or may not have been
                    // delivered. Issuing one extra release on the still-open
                    // (poisoned) session is a compositor-side no-op when the
                    // first one landed, and unstrands the button when it did
                    // not — after the poisoned-session recycle there is no
                    // second chance, because the emergency mouse-up no-ops on
                    // a closed session and mutter never synthesizes the
                    // missing release when the session closes.
                    let _ = portal.button(evdev, false);
                    return Err(error);
                }
                if i + 1 < count {
                    sleep(Duration::from_millis(CLICK_GAP_MS));
                }
            }
            return Ok(());
        }
        let enigo = self.require_enigo()?;
        let button = map_enigo_button(button);
        // Leave a settle window between the move and the click, reducing the
        // XTEST injection race.
        settle();
        for i in 0..count {
            enigo
                .button(button, Direction::Click)
                .map_err(|error| input_failed("mouse click", error))?;
            if i + 1 < count {
                sleep(Duration::from_millis(CLICK_GAP_MS));
            }
        }
        Ok(())
    }

    fn mouse_down(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            portal.ensure_started()?;
            if let Err(error) = portal.button(wayland_portal::map_button(button), true) {
                // A failed press ≠ not delivered (same reason as click):
                // best-effort issue one extra release.
                let _ = portal.button(wayland_portal::map_button(button), false);
                return Err(error);
            }
            return Ok(());
        }
        let enigo = self.require_enigo()?;
        enigo
            .button(map_enigo_button(button), Direction::Press)
            .map_err(|error| input_failed("mouse down", error))
    }

    fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            // Release only on a still-open session: never
            // lazily start when there is no session — ensure_started's full
            // establishment flow would pop the system authorization dialog,
            // turning the emergency mouse_up of a revoke/emergency-stop/
            // session end into a reverse permission grab. A poisoned but not
            // closed session is included too (has_open_session rather than
            // is_active): Notify can still be delivered on it, while mutter's
            // Session.Close only destroys the virtual device and does not
            // synthesize releases — skipping on is_active would strand a
            // genuinely pressed button on the compositor side, with no
            // release path left until the next ensure_started.
            if portal.has_open_session() {
                return portal.button(wayland_portal::map_button(button), false);
            }
            return Ok(());
        }
        let enigo = self.require_enigo()?;
        enigo
            .button(map_enigo_button(button), Direction::Release)
            .map_err(|error| input_failed("mouse up", error))
    }

    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            self.note_input();
            // The flag Arc is cloned before require_portal: portal is a
            // mutable borrow of self that outlives the whole interpolation
            // loop, so the flag cannot be read through &self again.
            let cancel = self.cancel.clone();
            let portal = self.require_portal()?;
            portal.ensure_started()?;
            portal.motion_absolute(from.0, from.1)?;
            settle();
            if let Err(error) = portal.button(wayland_portal::map_button(MouseButton::Left), true) {
                // A failed press ≠ not delivered (same reason as click): the
                // "release at the end no matter what happened midway"
                // guarantee below applies to the press failing as well.
                let _ = portal.button(wayland_portal::map_button(MouseButton::Left), false);
                // The motion_absolute above verifiably moved the pointer to
                // the drag start: record it the same way move_to does, so
                // cursor_position does not keep reporting the stale
                // pre-drag spot.
                portal.track_pointer(from.0, from.1);
                return Err(error);
            }
            // Interpolated movement; the button must be released at the end
            // no matter what happened midway.
            let cancelled = |cancel: &Option<Arc<AtomicBool>>| {
                cancel
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::SeqCst))
            };
            let mut result = Ok(());
            let mut last_reached = (from.0, from.1);
            for (x, y) in drag_waypoints(from, to, DRAG_STEPS) {
                // Same abandonment contract as type: a request the caller
                // already abandoned must stop at the next waypoint —
                // otherwise it keeps holding the physical button for the
                // whole interpolated path. Release the button, report the
                // abandonment.
                if cancelled(&cancel) {
                    result = Err(ComputerUseError::unavailable(
                        "drag was cancelled (caller timeout or stop); the button is \
                         released and the pointer stays at the last waypoint",
                    ));
                    break;
                }
                if let Err(error) = portal.motion_absolute(x, y) {
                    result = Err(error);
                    break;
                }
                last_reached = (x, y);
                sleep(Duration::from_millis(DRAG_STEP_MS));
            }
            let release = portal.button(wayland_portal::map_button(MouseButton::Left), false);
            // Record the destination only when the interpolated move fully
            // succeeded; on failure keep the furthest waypoint the pointer
            // verifiably reached (or the drag start) so cursor_position
            // never reports a spot the pointer never touched.
            if result.is_ok() {
                portal.track_pointer(to.0, to.1);
            } else {
                portal.track_pointer(last_reached.0, last_reached.1);
            }
            combine_drag_errors(result, release)?;
            return Ok(());
        }
        // The flag Arc is cloned before require_enigo: enigo is a mutable
        // borrow of self that outlives the interpolation loop, so the flag
        // cannot be read through &self inside it.
        let cancel = self.cancel.clone();
        let enigo = self.require_enigo()?;
        enigo
            .move_mouse(from.0, from.1, Coordinate::Abs)
            .map_err(|error| input_failed("drag: move to start", error))?;
        settle();
        enigo
            .button(Button::Left, Direction::Press)
            .map_err(|error| input_failed("drag: button press", error))?;
        // Interpolated movement; the button must be released at the end no
        // matter what happened midway.
        let mut result = Ok(());
        for (x, y) in drag_waypoints(from, to, DRAG_STEPS) {
            // Same abandonment contract as the Wayland branch: stop at the
            // next waypoint for a request the caller already abandoned, then
            // release the button below and report the abandonment.
            if cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
            {
                result = Err(ComputerUseError::unavailable(
                    "drag was cancelled (caller timeout or stop); the button is \
                     released and the pointer stays at the last waypoint",
                ));
                break;
            }
            if let Err(error) = enigo.move_mouse(x, y, Coordinate::Abs) {
                result = Err(input_failed("drag: interpolated move", error));
                break;
            }
            sleep(Duration::from_millis(DRAG_STEP_MS));
        }
        let release = enigo
            .button(Button::Left, Direction::Release)
            .map_err(|error| input_failed("drag: button release", error));
        combine_drag_errors(result, release)
    }

    fn scroll(&mut self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            // amount=0 is a straight no-op: mutter reports Invalid for axis
            // steps=0, and notify()'s error path marks the session poisoned
            // (the next action reclaims and rebuilds it, popping the
            // authorization dialog again) — an empty scroll must not cost a
            // session rebuild.
            if clicks == 0 {
                return Ok(());
            }
            self.note_input();
            let portal = self.require_portal()?;
            portal.ensure_started()?;
            // One discrete scroll event can carry multiple clicks (the
            // compositor injects them click by click internally). Clamp
            // exactly like the X11 branch below: the tool layer already
            // caps at 100; this backstops callers that reach the Backend
            // directly.
            let clicks = clicks.min(MAX_SCROLL_CLICKS);
            let (axis, steps) = wayland_portal::map_discrete_scroll(direction, clicks);
            return portal.axis_discrete(axis, steps);
        }
        let enigo = self.require_enigo()?;
        // Use scroll-wheel buttons explicitly rather than Mouse::scroll()/
        // helpers::map_scroll(): the sign convention of axis scrolling varies
        // by platform (x11rb positive = down), while button cycling is
        // unambiguous. The divergence from map_scroll is intentional (button
        // mechanism vs axis mechanism), but the clamp applies all the same
        // as on the Wayland branch above: the tool layer already caps at
        // 100; this backstops callers that reach the Backend directly.
        let clicks = clicks.min(MAX_SCROLL_CLICKS);
        let button = match direction {
            ScrollDirection::Up => Button::ScrollUp,
            ScrollDirection::Down => Button::ScrollDown,
            ScrollDirection::Left => Button::ScrollLeft,
            ScrollDirection::Right => Button::ScrollRight,
        };
        for i in 0..clicks {
            enigo
                .button(button, Direction::Click)
                .map_err(|error| input_failed("scroll", error))?;
            if i + 1 < clicks {
                sleep(Duration::from_millis(SCROLL_GAP_MS));
            }
        }
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), ComputerUseError> {
        // Fold CRLF/CR to '\n' first (the shared helper; same reason as the
        // Windows/macOS paths): a raw CR would submit twice / inject a stray
        // key on both injection paths, and the '\n'-only form is what the
        // Enter handling below keys off.
        let text = normalize_typed_newlines(text);
        if self.is_wayland() {
            self.note_input();
            // The flag Arc is cloned before require_portal: portal is a
            // mutable borrow of self that outlives the whole injection loop,
            // so the flag cannot be read through &self again.
            let cancel = self.cancel.clone();
            let portal = self.require_portal()?;
            // Per-character keysym injection (\n→Return, \t→Tab). Mapping
            // happens first: non-Latin-1 characters (CJK etc.) fail
            // explicitly (fail-closed) — mutter silently drops keysyms
            // outside the keymap, so sending anyway would "succeed" with no
            // input; by the time we error, the authorization dialog must not
            // already have been shown.
            let keysyms = text
                .chars()
                .map(wayland_portal::char_keysym)
                .collect::<Result<Vec<_>, _>>()?;
            portal.ensure_started()?;
            // A failed press aborts (nothing landed for that char); a failed
            // release must not strand the loop since the press already
            // landed: remember the first error, keep typing the remaining
            // chars, and report the first error at the end (same semantics
            // as the X11 `release_chord` helper).
            let mut first_err = None;
            for keysym in keysyms {
                // Stop injecting for a request the caller already abandoned:
                // each character costs two bounded portal notifications, long
                // text on a degraded bus far exceeds the call budget, and
                // dequeue-time checks cannot stop it — a zombie request would
                // double-inject alongside the retry.
                if cancel
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::SeqCst))
                {
                    return Err(ComputerUseError::unavailable(
                        "type text was cancelled (caller timeout or stop); characters \
                         already injected are not undone",
                    ));
                }
                if let Err(error) = portal.keysym_event(keysym, true) {
                    // A failed press ≠ not delivered (the timeout says
                    // nothing about delivery): best-effort
                    // issue one extra release on the still-open poisoned
                    // session — a release that did not land is a
                    // compositor-side no-op, and one that did land avoids a
                    // stranded button (mutter does not synthesize releases
                    // when closing a session).
                    let _ = portal.keysym_event(keysym, false);
                    return Err(error);
                }
                if let Err(error) = release_keysyms_with_retry(
                    |k, pressed| portal.keysym_event(k, pressed),
                    std::slice::from_ref(&keysym),
                ) {
                    if first_err.is_none() {
                        first_err = Some(error);
                    }
                }
            }
            match first_err {
                Some(error) => return Err(error),
                None => return Ok(()),
            }
        }
        // The flag Arc is cloned before require_enigo: enigo is a mutable
        // borrow of self that outlives the whole injection loop, so the flag
        // cannot be read through &self inside the loop (nor at the clone
        // point).
        let cancel = self.cancel.clone();
        let enigo = self.require_enigo()?;
        // enigo's text() types a run via per-char Unicode injection (a
        // temporary keycode remap on X11, the xdotool trick), so CJK etc.
        // reach the focused field without IME synthesis. But that same
        // per-char path maps '\n' to Linefeed (0xff0a, Ctrl+J semantics),
        // NOT Return, so multi-line text would never submit: inject each
        // run via text() and an explicit Enter between runs (an empty
        // trailing run still produces the closing Enter, so "text\n"
        // submits).
        let enter = map_enigo_key(Key::Enter)?;
        for (index, run) in x11_type_runs(&text).into_iter().enumerate() {
            // Stop injecting for a request the caller already abandoned (the
            // same guarantee as the Wayland per-character path).
            if cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
            {
                return Err(ComputerUseError::unavailable(
                    "type text was cancelled (caller timeout or stop); characters \
                     already injected are not undone",
                ));
            }
            if index > 0 {
                Self::press_chord(enigo, &[enter])?;
                Self::release_chord(enigo, &[enter])?;
            }
            // Inject per character instead of one whole text(): enigo's
            // text() on X11 is already per-character Unicode injection
            // internally (one remap + server sync per character); calling it
            // with the whole run would widen the cancel check's granularity
            // to the entire run — a single-line abandoned long text on a
            // degraded bus would keep injecting for minutes and double-inject
            // alongside the retry.
            // text(&c) is per-character equivalent to text's internal per-char
            // path; semantics unchanged.
            if !run.is_empty() {
                for ch in run.chars() {
                    if cancel
                        .as_ref()
                        .is_some_and(|flag| flag.load(Ordering::SeqCst))
                    {
                        return Err(ComputerUseError::unavailable(
                            "type text was cancelled (caller timeout or stop); characters \
                             already injected are not undone",
                        ));
                    }
                    let mut buf = [0u8; 4];
                    enigo
                        .text(ch.encode_utf8(&mut buf))
                        .map_err(|error| input_failed("type text", error))?;
                }
            }
        }
        Ok(())
    }

    fn key_chord(&mut self, keys: &[Key]) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            let mapped = keys
                .iter()
                .map(|key| wayland_portal::map_keysym(*key))
                .collect::<Result<Vec<_>, _>>()?;
            portal.ensure_started()?;
            press_keysyms_unwind(&mapped, |keysym, pressed| {
                portal.keysym_event(keysym, pressed)
            })?;
            // Best-effort release every key (modifiers cannot strand) with
            // the pointer path's compensating retry; returns the first
            // release error.
            release_keysyms_with_retry(
                |keysym, pressed| portal.keysym_event(keysym, pressed),
                &mapped,
            )?;
            return Ok(());
        }
        let mapped = keys
            .iter()
            .map(|key| map_enigo_key(*key))
            .collect::<Result<Vec<_>, _>>()?;
        let enigo = self.require_enigo()?;
        Self::press_chord(enigo, &mapped)?;
        Self::release_chord(enigo, &mapped)
    }

    fn hold_key(&mut self, keys: &[Key], ms: u64) -> Result<(), ComputerUseError> {
        // Clone the flag before require_portal/require_enigo: those hold a
        // mutable borrow of self that outlives the hold loop (the same
        // ordering as type_text).
        let cancel = self.cancel.clone();
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            let mapped = keys
                .iter()
                .map(|key| wayland_portal::map_keysym(*key))
                .collect::<Result<Vec<_>, _>>()?;
            portal.ensure_started()?;
            press_keysyms_unwind(&mapped, |keysym, pressed| {
                portal.keysym_event(keysym, pressed)
            })?;
            // Chunk the hold so a stop/timeout cancel flag is polled within
            // HOLD_CANCEL_POLL_MS instead of waiting out the whole hold (up
            // to 30 s); keys are always released below either way.
            let mut remaining = ms;
            while remaining > 0 {
                if cancel
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::SeqCst))
                {
                    // Same compensating retry as the normal release below:
                    // the cancel path is exactly the "caller timeout or
                    // stop" case the retry exists for, and a single failed
                    // notify may strand the key on the compositor (mutter
                    // never synthesizes the missing release when the session
                    // closes).
                    let release = release_keysyms_with_retry(
                        |keysym, pressed| portal.keysym_event(keysym, pressed),
                        &mapped,
                    );
                    return Err(match release {
                        Ok(()) => ComputerUseError::unavailable(
                            "hold was cancelled (caller timeout or stop); the keys have been released",
                        ),
                        Err(release_error) => ComputerUseError::unavailable(format!(
                            "hold was cancelled (caller timeout or stop); the compensating \
                             release failed ({release_error}); the keys may still be held down"
                        )),
                    });
                }
                let step = remaining.min(HOLD_CANCEL_POLL_MS);
                sleep(Duration::from_millis(step));
                remaining -= step;
            }
            // Same release contract as key_chord above.
            release_keysyms_with_retry(
                |keysym, pressed| portal.keysym_event(keysym, pressed),
                &mapped,
            )?;
            return Ok(());
        }
        let mapped = keys
            .iter()
            .map(|key| map_enigo_key(*key))
            .collect::<Result<Vec<_>, _>>()?;
        let enigo = self.require_enigo()?;
        Self::press_chord(enigo, &mapped)?;
        let mut remaining = ms;
        while remaining > 0 {
            if cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
            {
                // Surface a failed release instead of claiming success: an
                // XTEST release error usually means the connection is dead,
                // and the caller must know the keys may be stranded down.
                return Err(match Self::release_chord(enigo, &mapped) {
                    Ok(()) => ComputerUseError::unavailable(
                        "hold was cancelled (caller timeout or stop); the keys have been released",
                    ),
                    Err(release_error) => ComputerUseError::unavailable(format!(
                        "hold was cancelled (caller timeout or stop); the compensating \
                         release failed ({release_error}); the keys may still be held down"
                    )),
                });
            }
            let step = remaining.min(HOLD_CANCEL_POLL_MS);
            sleep(Duration::from_millis(step));
            remaining -= step;
        }
        Self::release_chord(enigo, &mapped)
    }

    fn ui_tree(&mut self, opts: &UiTreeOptions) -> Result<String, ComputerUseError> {
        let a11y = self.require_a11y()?;
        let wayland = self.is_wayland();
        let desktop = self.session.desktop_label().to_string();
        self.block_on_a11y(
            A11Y_TREE_DEADLINE,
            "ui_tree",
            ui_tree_async(a11y, opts, wayland, &desktop),
        )
    }

    fn element_at_point(
        &mut self,
        x: i32,
        y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError> {
        let a11y = self.require_a11y()?;
        self.block_on_a11y(
            A11Y_POINT_DEADLINE,
            "element_at_point",
            element_at_point_async(a11y, x, y),
        )
    }

    fn focused_element(&mut self) -> Result<Option<ElementInfo>, ComputerUseError> {
        // The portal/Wayland branch is the same as X11: AT-SPI is the path
        // that works across session types.
        let a11y = self.require_a11y()?;
        self.block_on_a11y(
            A11Y_POINT_DEADLINE,
            "focused_element",
            focused_element_async(a11y),
        )
    }
}

pub(super) fn create_backend() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    let session = detect_session(&|key| std::env::var(key).ok());
    if session.kind == SessionKind::NoDisplay {
        return Err(ComputerUseError::unsupported(
            "computer_use",
            format!(
                "no graphical session reachable (XDG_SESSION_TYPE={:?}, WAYLAND_DISPLAY set: {}, \
                 DISPLAY set: {}); computer use needs an X11 or Wayland desktop session",
                session.session_type, session.has_wayland_display, session.has_display
            ),
        ));
    }
    let wayland = session.kind == SessionKind::Wayland;

    // Capture/input plane mismatch warning: xcap's own Wayland detector keys
    // off XDG_SESSION_TYPE=wayland or a WAYLAND_DISPLAY containing
    // "wayland", so capture can be routed through the Wayland portal chain
    // while XTEST input targets X11. The classification itself is deliberate
    // (pinned by tests) — warn once so the split is diagnosable instead of
    // silent. Predicate matches capabilities() notes (any non-empty value):
    // deliberately BROADER than xcap's routing (e.g. "wl-0"/"sway-1" do NOT
    // match xcap's substring check and still take the XCB path), because a
    // non-X11 socket name signals a Wayland-colored environment whose
    // behavior the user should double-check; over-warning is the safe
    // direction, and the warning must not be narrower than the behavior it
    // diagnoses.
    if session.kind == SessionKind::X11
        && std::env::var("WAYLAND_DISPLAY")
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    {
        eprintln!(
            "[computer_use] XDG_SESSION_TYPE=x11 but WAYLAND_DISPLAY is set: capture may be \
             routed through the Wayland portal chain while input targets X11; unset \
             WAYLAND_DISPLAY for consistent behavior"
        );
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            ComputerUseError::unavailable(format!(
                "cannot create tokio runtime for AT-SPI: {error}"
            ))
        })?;
    let (a11y, a11y_init_error) = match runtime.block_on(tokio::time::timeout(
        // Overall cap on connection establishment (the SASL/Hello handshake
        // is not covered by zbus's method_timeout: a wedged
        // a11y bus that accepts the connection but never answers would hang
        // create_backend indefinitely, and the startup-timeout cleanup
        // thread.join() would then block the first caller forever; same
        // theme as the bounding of set_session_accessibility / the two bus
        // builders below). An a11y failure is non-fatal anyway (degrades to
        // a11y_init_error); a timeout takes the same path.
        4 * A11Y_METHOD_TIMEOUT,
        a11y_connect(),
    )) {
        Ok(Ok(conn)) => (Some(conn), None),
        Ok(Err(error)) => (None, Some(error)),
        Err(_) => (
            None,
            Some(format!(
                "a11y bus connection did not finish within {:?}",
                4 * A11Y_METHOD_TIMEOUT
            )),
        ),
    };

    // On Wayland, enigo is not constructed: XTEST through XWayland can only
    // reach X11 clients, and enigo's wayland/libei backends are both
    // experimental — input goes through portal RemoteDesktop.
    let (input, input_init_error) = if wayland {
        (None, None)
    } else {
        match Enigo::new(&Settings::default()) {
            Ok(enigo) => (Some(enigo), None),
            Err(error) => (None, Some(error.to_string())),
        }
    };

    // Wayland input: probe the portal's RemoteDesktop support (a pure
    // property query, no dialog); a failed probe is recorded as a sticky
    // error and input actions are explicitly unavailable.
    let (wayland_portal, wayland_portal_error) = if wayland {
        match PortalInput::probe() {
            Ok(()) => match PortalInput::new() {
                Ok(portal) => (Some(portal), None),
                Err(error) => (None, Some(error.to_string())),
            },
            Err(detail) => (None, Some(detail)),
        }
    } else {
        (None, None)
    };

    let (wayland_screenshot_ok, wayland_screenshot_error) = if wayland {
        match probe_wayland_screenshot_guarded() {
            Ok(()) => (true, None),
            Err(error) => (false, Some(error)),
        }
    } else {
        (true, None)
    };

    Ok(Box::new(LinuxComputerUseBackend {
        session,
        runtime,
        a11y,
        a11y_init_error,
        input,
        input_init_error,
        wayland_portal,
        wayland_portal_error,
        wayland_screenshot_ok,
        wayland_screenshot_error,
        wayland_portal_capture_error: None,
        last_input_at: None,
        cancel: None,
        wayland_portal_capture_degraded: false,
    }))
}

/// The backend is dropped on the worker thread (Shutdown): the portal
/// session created by the granted authorization is best-effort closed here so
/// it does not leak across processes.
impl Drop for LinuxComputerUseBackend {
    fn drop(&mut self) {
        if let Some(portal) = self.wayland_portal.as_mut() {
            portal.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn detect(pairs: &[(&str, &str)]) -> SessionInfo {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        detect_session(&|key| map.get(key).cloned())
    }

    #[test]
    fn session_type_wayland_wins_over_display() {
        // DISPLAY is also set under XWayland; a Wayland session must never be
        // misjudged as X11.
        let session = detect(&[
            ("XDG_SESSION_TYPE", "wayland"),
            ("DISPLAY", ":0"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("XDG_CURRENT_DESKTOP", "GNOME"),
        ]);
        assert_eq!(session.kind, SessionKind::Wayland);
        assert_eq!(session.desktop_label(), "GNOME");
    }

    #[test]
    fn session_type_x11_selects_x11() {
        let session = detect(&[("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":0")]);
        assert_eq!(session.kind, SessionKind::X11);
    }

    #[test]
    fn session_type_tty_means_no_display() {
        let session = detect(&[("XDG_SESSION_TYPE", "tty")]);
        assert_eq!(session.kind, SessionKind::NoDisplay);
    }

    #[test]
    fn wayland_display_with_runtime_dir_corroborates_wayland() {
        let session = detect(&[
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("DISPLAY", ":0"),
        ]);
        assert_eq!(session.kind, SessionKind::Wayland);
    }

    #[test]
    fn wayland_display_alone_still_wayland() {
        let session = detect(&[("WAYLAND_DISPLAY", "wayland-1")]);
        assert_eq!(session.kind, SessionKind::Wayland);
    }

    #[test]
    fn display_without_wayland_signals_is_x11() {
        let session = detect(&[("DISPLAY", ":0")]);
        assert_eq!(session.kind, SessionKind::X11);
    }

    #[test]
    fn unknown_session_type_falls_back_to_heuristics() {
        let session = detect(&[("XDG_SESSION_TYPE", "mir"), ("DISPLAY", ":0")]);
        assert_eq!(session.kind, SessionKind::X11);
    }

    #[test]
    fn session_type_is_case_insensitive_and_trimmed() {
        let session = detect(&[("XDG_SESSION_TYPE", " Wayland ")]);
        assert_eq!(session.kind, SessionKind::Wayland);
    }

    #[test]
    fn empty_environment_is_no_display() {
        let session = detect(&[]);
        assert_eq!(session.kind, SessionKind::NoDisplay);
        let blank = detect(&[("XDG_SESSION_TYPE", ""), ("DISPLAY", "  ")]);
        assert_eq!(blank.kind, SessionKind::NoDisplay);
    }

    #[test]
    fn key_mapping_covers_named_keys() {
        assert_eq!(map_enigo_key(Key::Enter).ok(), Some(enigo::Key::Return));
        assert_eq!(map_enigo_key(Key::Control).ok(), Some(enigo::Key::Control));
        assert_eq!(map_enigo_key(Key::Meta).ok(), Some(enigo::Key::Meta));
        assert_eq!(map_enigo_key(Key::Up).ok(), Some(enigo::Key::UpArrow));
        assert_eq!(
            map_enigo_key(Key::PageDown).ok(),
            Some(enigo::Key::PageDown)
        );
        assert_eq!(
            map_enigo_key(Key::Char('s')).ok(),
            Some(enigo::Key::Unicode('s'))
        );
        assert_eq!(
            map_enigo_key(Key::Char('中')).ok(),
            Some(enigo::Key::Unicode('中'))
        );
    }

    #[test]
    fn key_mapping_function_keys_f1_to_f20() {
        assert_eq!(map_enigo_key(Key::Function(1)).ok(), Some(enigo::Key::F1));
        assert_eq!(map_enigo_key(Key::Function(12)).ok(), Some(enigo::Key::F12));
        assert_eq!(map_enigo_key(Key::Function(20)).ok(), Some(enigo::Key::F20));
        assert!(map_enigo_key(Key::Function(21)).is_err());
        assert!(map_enigo_key(Key::Function(0)).is_err());
    }

    #[test]
    fn sanitize_name_strips_quotes_and_truncates() {
        // Shared helper order (map → truncate → trim): quotes straightened,
        // control characters folded to space, result trimmed.
        assert_eq!(
            sanitize_name("say \"hi\"\nnow", MAX_NAME_CHARS),
            "say 'hi' now"
        );
        // Tab and C0 controls fold to space (the old local copy kept them)
        // and the result is trimmed.
        assert_eq!(sanitize_name("a\tb\u{1}c ", MAX_NAME_CHARS), "a b c");
        let long = "x".repeat(MAX_NAME_CHARS + 20);
        assert_eq!(
            sanitize_name(&long, MAX_NAME_CHARS).chars().count(),
            MAX_NAME_CHARS
        );
    }

    #[test]
    fn x11_type_runs_split_on_newline_and_keep_trailing_segment() {
        // The injection loop emits an Enter before every segment after the
        // first, so the trailing empty segment is what makes "text\n" still
        // submit.
        assert_eq!(
            x11_type_runs("a\nb"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(x11_type_runs("a\n"), vec!["a".to_string(), String::new()]);
        // split always yields at least one segment: empty text is a no-op
        // run, not an error.
        assert_eq!(x11_type_runs(""), vec![String::new()]);
        // Consecutive newlines: empty runs carry no text but separate the
        // two Enters.
        assert_eq!(
            x11_type_runs("\n\n"),
            vec![String::new(), String::new(), String::new()]
        );
    }

    #[test]
    fn secure_role_decision_is_conservative() {
        // PasswordText decides directly.
        assert!(is_secure_role(Role::PasswordText, false));
        // A failed role query (unknown) falls back conservatively: unable to
        // prove it is not a password field.
        assert!(is_secure_role(Role::Unknown, true));
        // An object genuinely reporting Unknown (not a query failure) does
        // not count as secure; an ordinary button role does not either.
        assert!(!is_secure_role(Role::Unknown, false));
        assert!(!is_secure_role(Role::Button, false));
    }

    #[test]
    fn extents_contain_is_half_open_and_rejects_zero_sized() {
        // Regular containment: the origin, interior points; right/bottom
        // edges exclusive (a point exactly on the edge is not covered).
        let extents = (10, 20, 100, 50);
        assert!(extents_contain(extents, 10, 20));
        assert!(extents_contain(extents, 109, 69));
        assert!(!extents_contain(extents, 110, 69));
        assert!(!extents_contain(extents, 109, 70));
        assert!(!extents_contain(extents, 9, 20));
        assert!(!extents_contain(extents, 10, 19));
        // Negative origins (multi-monitor layout, secondary screen to the
        // left/above).
        let negative = (-1920, -400, 1920, 1080);
        assert!(extents_contain(negative, -1, -1));
        assert!(!extents_contain(negative, -1921, 0));
        // Zero-sized extents contain no point (screen_extents_strict already
        // errors on them; this guarantees that even if one slipped into the
        // loop it would not be misjudged as covering).
        let zero = (0, 0, 0, 0);
        assert!(!extents_contain(zero, 0, 0));
        assert!(!extents_contain(zero, i32::MAX, i32::MAX));
    }

    #[test]
    fn state_flags_mark_secure_and_states() {
        // The secure flag is supplied by the caller per is_secure_role.
        assert!(state_flags(true, None).contains(&"secure"));
        assert!(!state_flags(false, None).contains(&"secure"));
        // Regular state bits are unaffected by the secure determination.
        let flags = state_flags(false, Some(StateSet::new(State::Focused)));
        assert!(flags.contains(&"focused"));
        assert!(!flags.contains(&"secure"));
        let flags = state_flags(true, Some(StateSet::new(State::Active | State::Editable)));
        assert!(flags.contains(&"active") && flags.contains(&"editable"));
        // Enabled missing → disabled.
        assert!(state_flags(false, Some(StateSet::empty())).contains(&"disabled"));
    }

    #[test]
    fn wayland_chord_press_unwinds_held_keysyms_on_failure() {
        // Recording stand-in: pressing the second keysym fails.
        let mut events: Vec<(i32, bool)> = Vec::new();
        let result = press_keysyms_unwind(&[0xffe3, 0x63, 0x64], |keysym, pressed| {
            let failed = pressed && keysym == 0x63;
            events.push((keysym, pressed));
            if failed {
                Err(ComputerUseError::failed("portal injection failed"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        // The modifier pressed before the failure is released in reverse;
        // the FAILING keysym itself is also released best-effort (a timed-out
        // notify may still have been delivered; releasing an un-landed key
        // is a compositor-side no-op); keysyms after
        // the failing one are never attempted.
        assert_eq!(
            events,
            vec![(0xffe3, true), (0x63, true), (0x63, false), (0xffe3, false)]
        );
    }

    #[test]
    fn wayland_chord_press_success_does_not_release_anything() {
        let mut events: Vec<(i32, bool)> = Vec::new();
        let result = press_keysyms_unwind(&[0xffe3, 0x63], |keysym, pressed| {
            events.push((keysym, pressed));
            Ok(())
        });
        assert!(result.is_ok());
        assert_eq!(events, vec![(0xffe3, true), (0x63, true)]);
    }

    #[test]
    fn wayland_chord_press_unwind_release_errors_are_swallowed() {
        // Release on the unwind failing too must not mask the press error.
        let mut releases = 0;
        let result = press_keysyms_unwind(&[0xffe3, 0x63], |keysym, pressed| {
            if pressed {
                if keysym == 0x63 {
                    return Err(ComputerUseError::failed("press failed"));
                }
            } else {
                releases += 1;
                return Err(ComputerUseError::failed("release failed"));
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "failed: press failed");
        // Both the failing keysym and the held modifier are released;
        // their failures stay swallowed.
        assert_eq!(releases, 2);
    }
}

#[cfg(test)]
mod wayland_e2e_tests {
    //! Live Wayland E2E (dialog → grant → same-session capture →
    //! move/click/type).
    //! Needs a real Wayland session + xdg-desktop-portal
    //! (RemoteDesktop/ScreenCast), and the system authorization dialog must be
    //! confirmed — the verification environment uses a root uinput script to
    //! simulate the user pressing Enter (see the verification section of the
    //! PR description). Ignored by default:
    //! `PINVOU3_CU_WAYLAND_LIVE=1 cargo test --lib computer_use::platform::linux::wayland_e2e_tests -- --ignored --nocapture`
    //!
    //! Same double opt-in as the X11 live suite: `--ignored` is only a
    //! conventional guard — a bare `--ignored`
    //! on a Wayland dev machine with a real desktop session would move the
    //! real pointer, click, and type test text.

    use super::*;
    use atspi::proxy::text::TextProxy;

    /// The same environment gate as X11's `live_display()`: returns None
    /// unless `PINVOU3_CU_WAYLAND_LIVE=1` is explicitly exported, so every
    /// case skips on its first line.
    fn live_wayland() -> Option<()> {
        std::env::var("PINVOU3_CU_WAYLAND_LIVE")
            .ok()
            .filter(|v| v == "1")
            .map(|_| ())
    }

    /// DFS-collects the text of all role=Text nodes (a11y verification of the
    /// typed result).
    async fn read_texts_via_a11y(conn: &zbus::Connection) -> Result<Vec<String>, ComputerUseError> {
        let mut texts = Vec::new();
        walk_texts(conn, root_accessible(conn).await?, 18, &mut texts).await?;
        Ok(texts)
    }

    async fn walk_texts(
        conn: &zbus::Connection,
        proxy: AccessibleProxy<'_>,
        depth: u32,
        out: &mut Vec<String>,
    ) -> Result<(), ComputerUseError> {
        if depth == 0 || out.len() >= 64 {
            return Ok(());
        }
        let role = proxy.get_role().await.unwrap_or(Role::Unknown);
        if role == Role::Text {
            let text = TextProxy::builder(conn)
                .destination(proxy.inner().destination().as_str().to_string())
                .map_err(|e| ComputerUseError::failed(e.to_string()))?
                .path(proxy.inner().path().as_str().to_string())
                .map_err(|e| ComputerUseError::failed(e.to_string()))?
                .build()
                .await
                .map_err(|e| ComputerUseError::failed(format!("text proxy: {e}")))?;
            let content = text.get_text(0, -1).await.unwrap_or_default();
            if !content.trim().is_empty() {
                out.push(content);
            }
        }
        if let Ok(children) = proxy.get_children().await {
            for child in children {
                if child.is_null() {
                    continue;
                }
                if let Ok(child) = child.into_accessible_proxy(conn).await {
                    Box::pin(walk_texts(conn, child, depth - 1, out)).await?;
                }
            }
        }
        Ok(())
    }

    /// Difference bounding box of two RGBA frames (None when there is no
    /// difference). Samples every 4 pixels — enough to locate a window-level
    /// bounding box while saving time.
    fn diff_bounding_box(a: &[u8], b: &[u8]) -> Option<(i32, i32, u32, u32)> {
        assert_eq!(a.len(), b.len());
        let width = ((a.len() / 4) as f64).sqrt() as usize;
        let height = (a.len() / 4) / width;
        let mut x0 = usize::MAX;
        let mut y0 = usize::MAX;
        let mut x1 = 0;
        let mut y1 = 0;
        for y in (0..height).step_by(4) {
            for x in (0..width).step_by(4) {
                let i = (y * width + x) * 4;
                if a[i..i + 3] != b[i..i + 3] {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        if x0 == usize::MAX {
            return None;
        }
        Some((
            x0 as i32,
            y0 as i32,
            (x1 - x0 + 4) as u32,
            (y1 - y0 + 4) as u32,
        ))
    }

    #[test]
    #[ignore = "needs a live Wayland session with xdg-desktop-portal and the system \
                authorization dialog confirmed (uinput Enter in the verification env)"]
    fn e2e_dialog_grant_capture_and_input() {
        if live_wayland().is_none() {
            println!("skipped: set PINVOU3_CU_WAYLAND_LIVE=1 to run against a live desktop");
            return;
        }
        let session = detect_session(&|key| std::env::var(key).ok());
        assert_eq!(
            session.kind,
            SessionKind::Wayland,
            "run inside a live Wayland session"
        );

        // Backend construction establishes the AT-SPI connection (a11y bus)
        // so that target apps started afterwards can register on the a11y
        // tree.
        let mut backend = create_backend().expect("backend on a live Wayland session");
        let caps = backend.capabilities();
        println!(
            "capabilities: screenshot={} input={} ui_tree={}\n  {}",
            caps.screenshot, caps.input, caps.ui_tree, caps.notes
        );
        assert!(
            caps.screenshot,
            "portal same-session capture must be available"
        );
        assert!(caps.input, "portal input must be available");

        // 1) First capture: establishes the portal session and pops the
        //    system authorization dialog (confirmed by the verification
        //    environment's dialog script); the frame comes from the
        //    same-session ScreenCast stream.
        let shot1 = backend
            .capture()
            .expect("first capture establishes the portal session (dialog must be granted)");
        println!(
            "capture #1: {}x{} origin=({},{}) input_scale={}",
            shot1.width, shot1.height, shot1.origin_x, shot1.origin_y, shot1.input_scale_x
        );
        assert_eq!((shot1.origin_x, shot1.origin_y), (0, 0));
        assert_eq!(shot1.input_scale_x, 1.0);
        assert_eq!(
            shot1.rgba.len(),
            shot1.width as usize * shot1.height as usize * 4
        );
        assert!(shot1.rgba.iter().any(|byte| *byte != 0), "frame not blank");

        // 2) Input target: the GNOME Shell top-bar clock (known position:
        //    center of the top bar).
        //    On Wayland AT-SPI extents are unreliable (all 0); computer-use's
        //    proper path is to look at screenshots: after clicking, verify
        //    with a pixel diff that the UI really responded.
        let shot_w = shot1.width;
        let shot_h = shot1.height;
        let clock = (i64::from(shot_w) / 2, 8);
        backend
            .move_to(clock.0 as i32, clock.1)
            .expect("mouse_move");
        assert_eq!(
            backend.cursor_position().ok(),
            Some((clock.0 as i32, clock.1)),
            "cursor_position tracks our injected move"
        );
        backend.click(MouseButton::Left, 1).expect("click");

        // 3) Wait for frames newer than the click: the calendar/notification
        // dropdown must appear in the top half of the screen.
        wait_for_big_change(&mut backend, &shot1, 40, shot_h / 2)
            .expect("clicking the clock must open the calendar dropdown");

        // 4) Keyboard path: Escape closes the dropdown, Super opens the
        // overview (the search box auto-focuses).
        backend.key_chord(&[Key::Escape]).expect("escape key chord");
        std::thread::sleep(Duration::from_millis(600));
        let baseline = backend.capture().expect("capture before overview");
        backend.key_chord(&[Key::Meta]).expect("super key chord");

        // 5) Type into the shell search box: after the overview settles,
        // type "fire"; the search results (Firefox) appearing = the key
        // injection really reached the shell search box (a second
        // window-level diff against the overview baseline).
        let typed = "fire";
        let mut typed_shot = None;
        for i in 0..40 {
            std::thread::sleep(Duration::from_millis(400));
            if i == 6 {
                backend.type_text(typed).expect("type_text");
            }
            let Ok(shot) = backend.capture() else {
                continue;
            };
            if i >= 8 {
                if let Some(box_) = diff_bounding_box(&baseline.rgba, &shot.rgba) {
                    if box_.2 > 200 && box_.3 > 200 {
                        typed_shot = Some(box_);
                        break;
                    }
                }
            }
        }
        assert!(
            typed_shot.is_some(),
            "search results must appear after typing into the overview search"
        );

        // 6) a11y text read-back (the cally bridge of the nested shell is not
        // enabled at startup, so the tree may be shallow; diagnostic output
        // only, no assertions).
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let conn = runtime.block_on(a11y_connect()).expect("a11y connection");
        let texts = runtime
            .block_on(read_texts_via_a11y(&conn))
            .expect("read texts via a11y");
        println!(
            "a11y texts ({} entries, first 3): {:?}",
            texts.len(),
            texts
                .iter()
                .take(3)
                .map(|t| t.chars().take(80).collect::<String>())
                .collect::<Vec<_>>()
        );
    }

    /// Captures repeatedly until a window-level diff against `baseline`
    /// appears (ignoring the fixed small-change regions with prefixes in
    /// `ignore_prefixes`), returning the diff bounding box.
    fn wait_for_big_change(
        backend: &mut Box<dyn ComputerUseBackend>,
        baseline: &Capture,
        rounds: usize,
        max_y: u32,
    ) -> Option<(i32, i32, u32, u32)> {
        for _ in 0..rounds {
            std::thread::sleep(Duration::from_millis(400));
            let Ok(shot) = backend.capture() else {
                continue;
            };
            if shot.width != baseline.width || shot.height != baseline.height {
                continue;
            }
            if let Some(box_) = diff_bounding_box(&baseline.rgba, &shot.rgba) {
                if box_.2 > 200 && box_.3 > 200 && (box_.1 as u32) < max_y {
                    return Some(box_);
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// X11 live E2E: the full consent pipeline against a real X server (Xvfb).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod x11_live_tests {
    //! Live X11 E2E for the real backend plus the consent pipeline
    //! (settings toggle → session grant → best-effort screening →
    //! execute → redacted audit), with the confirmation-token lifecycle
    //! (mint / spend / single-use / wipe) driven through the same guard API
    //! the Tauri commands call.
    //!
    //! WARNING: each test takes over the X server named by `$DISPLAY` and
    //! injects real XTEST input into it — run them ONLY against a sandboxed
    //! Xvfb, never against a desktop someone is using. As a hard guard (not
    //! just this warning), every live test also requires an explicit opt-in
    //! environment variable:
    //!
    //! ```text
    //! Xvfb :99 -screen 0 1280x800x24 &
    //! cd pinvou3-app/src-tauri
    //! DISPLAY=:99 PINVOU3_CU_X11_LIVE=1 \
    //!     cargo test --lib computer_use -- --ignored --test-threads=1
    //! ```
    //!
    //! Every injection is verified EXTERNALLY: the X server itself reports the
    //! pointer position via `xdotool getmouselocation`, so the assertions
    //! cannot be fooled by anything inside the process.
    //!
    //! The AT-SPI (a11y) stack is intentionally absent under Xvfb: the test
    //! points the D-Bus session bus at a nonexistent socket, so the backend's
    //! AT-SPI connection fails at init and screening is unavailable for every
    //! target. That exercises the best-effort fail-open path end to end: with
    //! a session grant, input actions execute against a real X server without
    //! any confirmation prompt (unavailable screening must not produce a
    //! confirmation storm); confirmation mechanics themselves are covered by
    //! the guard-level assertions and the mocked tool tests.
    //!
    //! Tests are `#[ignore]` so CI never runs them; each additionally skips
    //! cleanly (early return) unless `$DISPLAY` answers
    //! `xdotool getdisplaygeometry`.

    #![allow(clippy::await_holding_lock)]

    use super::*;
    use crate::features::computer_use::backend::BackendHandle;
    use crate::features::computer_use::guard::ComputerUseShared;
    use crate::features::computer_use::tool::{ComputerUseEventSink, ComputerUseTool};
    use crate::features::computer_use::types::{EVENT_CONFIRM_REQUIRED, EVENT_GRANT_REQUIRED};
    use deepseek_tui::tools::spec::{ToolContext, ToolSpec};

    /// Validate-then-consume, mirroring what the tool does around its
    /// target re-screen. Returns the approved element label on success.
    fn spend(shared: &ComputerUseShared, confirm_id: &str, summary: &str) -> Option<String> {
        let label = shared.peek_confirmation(confirm_id, SESSION, summary, 0)?;
        assert!(
            shared.consume_confirmation(confirm_id),
            "a peeked token must still be there to consume"
        );
        Some(label)
    }
    use serde_json::{Value, json};
    use std::process::Command;
    use std::sync::{Arc, Mutex as StdMutex};
    use xcap::image;

    /// Session id used by every test (drives the audit file name too).
    const SESSION: &str = "s-x11";

    // ---- external X server verification helpers ---------------------------

    /// Run xdotool against `display`; `None` when xdotool is missing or the
    /// display does not answer (the caller then skips the test).
    fn xdotool(display: &str, args: &[&str]) -> Option<String> {
        let output = Command::new("xdotool")
            .env("DISPLAY", display)
            .args(args)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// The pointer position as reported by the X server itself (external
    /// ground truth, not anything the backend claims).
    fn pointer_at(display: &str) -> Option<(i32, i32)> {
        let out = xdotool(display, &["getmouselocation"])?;
        let mut x = None;
        let mut y = None;
        for token in out.split_whitespace() {
            if let Some(value) = token.strip_prefix("x:") {
                x = value.parse().ok();
            }
            if let Some(value) = token.strip_prefix("y:") {
                y = value.parse().ok();
            }
        }
        Some((x?, y?))
    }

    /// The live-display gate: `Some((width, height, display))` when `$DISPLAY`
    /// names an X server xdotool can reach, `None` otherwise (tests skip).
    fn live_display() -> Option<(u32, u32, String)> {
        // Double opt-in: `#[ignore]` is only a conventional
        // guard; the documented `--ignored` command on a Linux dev machine
        // with a real desktop session would move the real pointer, click, and
        // type test text. CI/headless usage of the test suite exports the
        // variable explicitly; any ordinary dev machine's DISPLAY skips.
        std::env::var("PINVOU3_CU_X11_LIVE")
            .ok()
            .filter(|v| v == "1")?;
        let display = std::env::var("DISPLAY").ok()?;
        let geometry = xdotool(&display, &["getdisplaygeometry"])?;
        let mut parts = geometry.split_whitespace();
        let width = parts.next()?.parse().ok()?;
        let height = parts.next()?.parse().ok()?;
        Some((width, height, display))
    }

    fn assert_pointer_unchanged(display: &str, before: (i32, i32), what: &str) {
        let after = pointer_at(display).expect("xdotool can read the pointer");
        assert_eq!(
            after, before,
            "{what}: the X server pointer must not move (before {before:?}, after {after:?})"
        );
    }

    /// Expected PNG dimensions for a `screen` of `size` under the tool's
    /// scaling cap ([`crate::features::computer_use::scaling::MAX_LONG_EDGE`];
    /// smaller screens are never upscaled).
    fn expected_png_size(size: (u32, u32)) -> (u32, u32) {
        use crate::features::computer_use::scaling::MAX_LONG_EDGE;
        let long = size.0.max(size.1);
        if long <= MAX_LONG_EDGE {
            return size;
        }
        let factor = f64::from(MAX_LONG_EDGE) / f64::from(long);
        (
            (f64::from(size.0) * factor).round() as u32,
            (f64::from(size.1) * factor).round() as u32,
        )
    }

    // ---- fixture (isolated PINVOU3_HOME + real X11 backend) ---------------

    /// Records the Tauri events the tool emits (grant/confirm prompts) so the
    /// tests can assert which prompts fired.
    #[derive(Clone)]
    struct EventRecorder(Arc<StdMutex<Vec<(String, Value)>>>);

    impl EventRecorder {
        fn new() -> Self {
            Self(Arc::new(StdMutex::new(Vec::new())))
        }

        fn emitted(&self, event: &str) -> bool {
            self.0.lock().unwrap().iter().any(|(name, _)| name == event)
        }
    }

    impl ComputerUseEventSink for EventRecorder {
        fn emit(&self, event: &str, payload: Value) {
            if let Ok(mut events) = self.0.lock() {
                events.push((event.to_string(), payload));
            }
        }
    }

    /// Process-env takeover for one test: `$DISPLAY` → the live X server,
    /// session type forced to X11, the D-Bus session bus pointed at a
    /// nonexistent socket (the AT-SPI stack is intentionally absent under
    /// Xvfb — this is what makes the screening genuinely unscreenable),
    /// `PINVOU3_HOME` → an isolated temp root so the audit JSONL and
    /// screenshots never touch the developer's real data. Everything is
    /// restored on drop; the platform env lock is held for the whole test so
    /// env writes stay serialized in-process.
    struct LiveEnv {
        _env_lock: std::sync::MutexGuard<'static, ()>,
        previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
        home: std::path::PathBuf,
    }

    impl LiveEnv {
        fn take(display: &str) -> Self {
            let env_lock = crate::platform::paths::tests::ENV_LOCK
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let mut previous: Vec<(&'static str, Option<std::ffi::OsString>)> = Vec::new();
            let mut set = |key: &'static str, value: Option<std::ffi::OsString>| {
                previous.push((key, std::env::var_os(key)));
                // SAFETY: ENV_LOCK is held; env writes are serialized in-process.
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            };
            let home = std::env::temp_dir().join(format!(
                "pinvou3-cu-x11-live-{}-{}",
                std::process::id(),
                crate::platform::paths::tests::unique_suffix()
            ));
            set("PINVOU3_HOME", Some(home.clone().into_os_string()));
            set("DISPLAY", Some(display.into()));
            // Force X11 detection even when the host session is Wayland.
            set("XDG_SESSION_TYPE", Some("x11".into()));
            set("WAYLAND_DISPLAY", None);
            // No a11y bus in the sandbox: the backend's AT-SPI connect must
            // fail at init so screening is genuinely unavailable for every
            // target (the tool layer then executes without confirmation).
            set(
                "DBUS_SESSION_BUS_ADDRESS",
                Some("unix:path=/tmp/pinvou3-cu-x11-live-no-a11y-bus".into()),
            );
            Self {
                _env_lock: env_lock,
                previous,
                home,
            }
        }

        fn audit_path(&self) -> std::path::PathBuf {
            self.home.join("computer-use").join(
                crate::features::computer_use::audit::audit_file_name(SESSION),
            )
        }
    }

    impl Drop for LiveEnv {
        fn drop(&mut self) {
            for (key, value) in self.previous.drain(..) {
                // SAFETY: ENV_LOCK is still held by self._env_lock.
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    struct Fixture {
        tool: ComputerUseTool,
        shared: Arc<ComputerUseShared>,
        events: EventRecorder,
        env: LiveEnv,
        workspace: std::path::PathBuf,
        display: String,
    }

    /// Build the real stack: `platform::create_backend` (xcap capture + enigo
    /// XTEST on `$DISPLAY`, AT-SPI absent) behind the same lazy BackendHandle
    /// the production tool uses, with an isolated audit home and a recording
    /// event sink. Declared field order matters on drop: the tool (which
    /// triggers the backend emergency release) drops before the env is
    /// restored.
    fn fixture(display: String) -> Fixture {
        let env = LiveEnv::take(&display);
        let workspace = env.home.join("sessions").join(SESSION).join("workspace");
        let _ = std::fs::create_dir_all(&workspace);
        let shared = Arc::new(ComputerUseShared::new());
        // The settings toggle (computer_use_set_enabled) mirrors here.
        shared.set_enabled(true);
        let events = EventRecorder::new();
        let backend = BackendHandle::lazy(move || create_backend());
        let tool = ComputerUseTool::with_parts(
            SESSION.to_string(),
            Arc::clone(&shared),
            backend,
            Arc::new(events.clone()),
        );
        Fixture {
            tool,
            shared,
            events,
            env,
            workspace,
            display,
        }
    }

    impl Fixture {
        async fn execute_raw(
            &self,
            input: Value,
        ) -> Result<deepseek_tui::tools::spec::ToolResult, deepseek_tui::tools::spec::ToolError>
        {
            self.tool
                .execute(input, &ToolContext::new(self.workspace.as_path()))
                .await
        }

        /// Run a tool call and flatten to (success, model-visible text).
        async fn execute(&self, input: Value) -> (bool, String) {
            match self.execute_raw(input).await {
                Ok(result) => (result.success, result.content),
                Err(error) => (false, error.to_string()),
            }
        }
    }

    #[tokio::test]
    #[ignore = "drives the X server named by $DISPLAY (run against a sandboxed Xvfb)"]
    async fn x11_live_capabilities_are_reported() {
        let Some((_, _, display)) = live_display() else {
            eprintln!("SKIP x11_live_capabilities_are_reported: $DISPLAY does not answer xdotool");
            return;
        };
        let fx = fixture(display);

        // The real backend, asked directly: X11 must report full input and
        // capture support, and must honestly report the absent AT-SPI stack.
        let probe = BackendHandle::lazy(create_backend);
        let caps = probe
            .capabilities()
            .expect("the real X11 backend must construct on the live display");
        println!(
            "x11_live capabilities: screenshot={} input={} ui_tree={}\n  notes: {}",
            caps.screenshot, caps.input, caps.ui_tree, caps.notes
        );
        assert!(caps.input, "X11 XTEST input must be reported available");
        assert!(
            caps.screenshot,
            "X11 xcap capture must be reported available"
        );
        assert!(
            !caps.ui_tree,
            "AT-SPI is absent in the sandbox; ui_tree must be reported unavailable: {}",
            caps.notes
        );

        // The tool-level ui_tree action must fail with the documented
        // unavailable error instead of silently returning a tree.
        let (success, text) = fx.execute(json!({"action": "ui_tree"})).await;
        println!("x11_live ui_tree action: success={success} text={text}");
        assert!(!success, "ui_tree must not succeed without AT-SPI: {text}");
        assert!(
            text.contains("accessibility tree is unsupported"),
            "ui_tree must fail with the documented unavailable error: {text}"
        );
        assert!(
            !text.contains("[0]"),
            "ui_tree must not return any serialized tree: {text}"
        );
    }

    #[tokio::test]
    #[ignore = "drives the X server named by $DISPLAY (run against a sandboxed Xvfb)"]
    async fn x11_live_capture_returns_screen_image() {
        let Some((width, height, display)) = live_display() else {
            eprintln!(
                "SKIP x11_live_capture_returns_screen_image: $DISPLAY does not answer xdotool"
            );
            return;
        };
        let fx = fixture(display);

        let result = fx
            .execute_raw(json!({"action": "screenshot"}))
            .await
            .expect("screenshot must not return a tool error");
        assert!(result.success, "{}", result.content);

        // PNG decodes and matches the externally verified geometry, modulo
        // the tool's long-edge scaling cap (small screens are not upscaled).
        let abs_path = result
            .metadata
            .as_ref()
            .and_then(|m| m.get("images"))
            .and_then(Value::as_array)
            .and_then(|images| images.first())
            .and_then(Value::as_str)
            .map(std::path::PathBuf::from)
            .expect("screenshot must attach an image path");
        let png = std::fs::read(&abs_path).expect("screenshot file must exist");
        let decoded = image::load_from_memory(&png).expect("the attachment must be a valid PNG");
        let (expected_w, expected_h) = expected_png_size((width, height));
        println!(
            "x11_live capture: xdotool geometry {width}x{height}, png {}x{} (expected {expected_w}x{expected_h})",
            decoded.width(),
            decoded.height()
        );
        assert_eq!(
            (decoded.width(), decoded.height()),
            (expected_w, expected_h),
            "capture must match the X server geometry modulo the long-edge cap"
        );
        assert!(
            result
                .content
                .contains(&format!("{expected_w}x{expected_h} px")),
            "the model-visible text must report the real geometry: {}",
            result.content
        );

        // Privacy: the screenshot (may contain on-screen secrets) lands 0600
        // inside a 0700 directory, under the isolated home.
        use std::os::unix::fs::PermissionsExt;
        let file_mode = std::fs::metadata(&abs_path)
            .expect("screenshot metadata")
            .permissions()
            .mode();
        assert_eq!(file_mode & 0o7777, 0o600, "screenshot file must be 0600");
        let dir_mode = std::fs::metadata(abs_path.parent().expect("screenshot has a parent"))
            .expect("screenshot dir metadata")
            .permissions()
            .mode();
        assert_eq!(
            dir_mode & 0o7777,
            0o700,
            "screenshot directory must be 0700"
        );
    }

    /// The core consent E2E against a real X server: no grant → nothing
    /// injected; grant → hover moves execute (deliberately unscreened); a
    /// pointer CLICK is T3-screened and, with AT-SPI absent, screening is
    /// unavailable → the click still executes (best-effort fail-open, no
    /// confirmation storm); the confirmation-token lifecycle (mint → spend →
    /// single-use) is driven through the live guard exactly as the Tauri
    /// commands do.
    #[tokio::test]
    #[ignore = "drives the X server named by $DISPLAY (run against a sandboxed Xvfb)"]
    async fn x11_live_input_requires_grant_then_executes() {
        let Some((_, _, display)) = live_display() else {
            eprintln!("SKIP x11_live_input_requires_grant…: $DISPLAY does not answer xdotool");
            return;
        };
        let fx = fixture(display);

        // (1) No session grant: the gate rejects before any injection, the
        // grant_required event fires, and the X server pointer stays put.
        let before = pointer_at(&fx.display).expect("xdotool can read the pointer");
        let move_to = json!({"action": "mouse_move", "x": 600, "y": 400});
        let (success, text) = fx.execute(move_to.clone()).await;
        assert!(!success, "an ungranted input action must fail: {text}");
        assert!(text.contains("has not granted control"), "{text}");
        assert!(
            fx.events.emitted(EVENT_GRANT_REQUIRED),
            "the grant_required event must fire for the frontend prompt"
        );
        assert_pointer_unchanged(&fx.display, before, "mouse_move without a grant");

        // (2) Grant (the same guard call the computer_use_grant command
        // makes). A plain mouse_move is a hover — deliberately NOT
        // T3-screened — so it executes; the X server must report the target.
        fx.shared.grant_session(SESSION);
        let (success, text) = fx.execute(move_to).await;
        assert!(success, "a granted hover move must execute: {text}");
        let at = pointer_at(&fx.display).expect("xdotool");
        assert!(
            (at.0 - 600).abs() <= 2 && (at.1 - 400).abs() <= 2,
            "the granted hover must land at (600, 400); xdotool reports {at:?}"
        );

        // (3) A pointer click is a screened action, but with the a11y stack
        // absent screening is unavailable — best-effort fail-open: the click
        // executes and provably no confirmation was requested.
        let click = json!({"action": "left_click", "x": 200, "y": 300});
        let (success, text) = fx.execute(click.clone()).await;
        assert!(
            success,
            "a granted click must execute while screening is unavailable: {text}"
        );
        assert!(!text.contains("NOT executed"), "{text}");
        assert!(
            !fx.events.emitted(EVENT_CONFIRM_REQUIRED),
            "unavailable screening must not raise a confirmation"
        );
        let at = pointer_at(&fx.display).expect("xdotool");
        assert!(
            (at.0 - 200).abs() <= 2 && (at.1 - 300).abs() <= 2,
            "the click must land at (200, 300); xdotool reports {at:?}"
        );

        // (4) Token lifecycle on the live guard (the same guard calls
        // computer_use_confirm / the confirmed-retry path use): mint → spend
        // → single-use.
        let summary = "left click x1 at Some((200, 300))";
        let confirm_id = fx
            .shared
            .new_pending_confirmation(SESSION, summary, "Live", 0)
            .expect("pending registered");
        assert!(fx.shared.pending_confirmation(&confirm_id).is_some());
        assert!(
            spend(&fx.shared, &confirm_id, summary).is_none(),
            "an un-minted token must not be spendable"
        );
        assert!(
            fx.shared.mint_confirmation(&confirm_id),
            "minting must succeed while the pending exists"
        );
        assert_eq!(
            spend(&fx.shared, &confirm_id, summary).as_deref(),
            Some("Live"),
            "the minted token must be spendable and carry the approved target"
        );
        assert!(
            spend(&fx.shared, &confirm_id, summary).is_none(),
            "the token is single-use"
        );
        // The bookkeeping performs no injection: the pointer must still be
        // parked at step (3)'s landing point.
        assert_pointer_unchanged(&fx.display, at, "token bookkeeping");
    }

    /// B2 regression, live: typed text (and typing-form chords) must reach
    /// the X server but never the audit JSONL in plaintext — the audit
    /// stores redacted targets only (`typed N characters` / `pressed 1
    /// key`), and the audit file itself must be 0600.
    #[tokio::test]
    #[ignore = "drives the X server named by $DISPLAY (run against a sandboxed Xvfb)"]
    async fn x11_live_type_audit_stays_redacted() {
        let Some((_, _, display)) = live_display() else {
            eprintln!("SKIP x11_live_type_audit_stays_redacted: $DISPLAY does not answer xdotool");
            return;
        };
        let fx = fixture(display);
        fx.shared.grant_session(SESSION);

        let secret = "p@ssw0rd-shift-test";
        let type_call = json!({"action": "type", "text": secret});
        // Typing target screening is unavailable (no AT-SPI) → best-effort
        // fail-open: the type executes directly, no confirmation round-trip.
        let (success, text) = fx.execute(type_call.clone()).await;
        assert!(success, "the typing must execute: {text}");
        assert!(!text.contains("NOT executed"), "{text}");

        // A duplicate-modifier typing-form chord goes through the same
        // pipeline; chords are never gated for destructiveness (the current
        // parser accepts shift+shift+h and audits it as typed text, exactly
        // like a bare single character).
        let chord = "shift+shift+h";
        let key_call = json!({"action": "key", "text": chord});
        let (success, text) = fx.execute(key_call).await;
        assert!(success, "the chord must execute: {text}");
        assert!(!text.contains("NOT executed"), "{text}");

        // Audit privacy: the plaintexts must appear nowhere in the JSONL;
        // every record carries a redacted target only.
        let audit_path = fx.env.audit_path();
        let raw = std::fs::read_to_string(&audit_path).expect("the audit JSONL must exist");
        assert!(
            !raw.contains(secret),
            "typed plaintext leaked into the audit log"
        );
        assert!(
            !raw.contains(chord),
            "chord plaintext leaked into the audit log"
        );
        assert!(
            !raw.contains("keys: shift+shift+h"),
            "a typing-form chord must not be audited as a plaintext keys: shortcut"
        );
        let records: Vec<Value> = raw
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).expect("valid JSONL line"))
            .collect();
        let typed: Vec<&Value> = records
            .iter()
            .filter(|r| r["action"] == "type" && r["target"] == "typed 19 characters")
            .collect();
        assert_eq!(
            typed.len(),
            1,
            "the executed type call must audit the redacted count: {records:?}"
        );
        let chords: Vec<&Value> = records
            .iter()
            .filter(|r| r["action"] == "key" && r["target"] == "pressed 1 key")
            .collect();
        assert_eq!(
            chords.len(),
            1,
            "the executed chord call must audit the key count: {records:?}"
        );
        // The audit is a plain log: no crypto or begin/end phase fields.
        for absent in ["salt", "text_hmac", "text_len", "phase"] {
            assert!(!raw.contains(absent), "{absent} in {raw}");
        }

        // The audit file itself is private.
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&audit_path)
            .expect("audit metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o7777, 0o600, "the audit JSONL must be 0600");
    }

    /// The computer_use_stop path (guard stop_all + registry
    /// emergency_release_all) wipes grants and consent state on a live
    /// backend: after stop + reopen, input is grant-required again.
    #[tokio::test]
    #[ignore = "drives the X server named by $DISPLAY (run against a sandboxed Xvfb)"]
    async fn x11_live_stop_releases_and_wipes() {
        let Some((_, _, display)) = live_display() else {
            eprintln!("SKIP x11_live_stop_releases_and_wipes: $DISPLAY does not answer xdotool");
            return;
        };
        let fx = fixture(display);
        fx.shared.grant_session(SESSION);

        // A live pending confirmation (raised via the guard exactly as the
        // tool's blocked-action path raises one; with AT-SPI absent no denylist
        // element is reachable here, so the guard API is used directly).
        let confirm_id = fx
            .shared
            .new_pending_confirmation(SESSION, "left click x1 at Some((300, 200))", "Live", 0)
            .expect("pending registered");
        assert!(fx.shared.pending_confirmation(&confirm_id).is_some());

        // The computer_use_stop command path, verbatim.
        fx.shared.stop_all();
        fx.shared.backends.emergency_release_all();

        assert!(
            !fx.shared.has_active_grant(SESSION),
            "stop must wipe the session grant"
        );
        assert!(
            fx.shared.pending_confirmation(&confirm_id).is_none(),
            "stop must wipe the pending confirmation"
        );
        assert!(
            !fx.shared.mint_confirmation(&confirm_id),
            "minting on a wiped pending must fail"
        );
        assert!(fx.shared.is_stopped(), "stop must raise the stop flag");

        // Reopen (computer_use_set_enabled(true) resets the stop flag): the
        // grant must still be gone — input is grant-required again.
        fx.shared.reset_stop();
        let before = pointer_at(&fx.display).expect("xdotool");
        let (success, text) = fx
            .execute(json!({"action": "left_click", "x": 300, "y": 200}))
            .await;
        assert!(!success, "{text}");
        assert!(text.contains("has not granted control"), "{text}");
        assert_pointer_unchanged(&fx.display, before, "click after stop + reopen");

        // Best-effort check that no XTEST button is left pressed: xdotool has
        // no button-state query, so this is reported rather than asserted —
        // the emergency release path (physical button up first, then the OS
        // grant close) was requested above for every registered backend.
        eprintln!(
            "x11_live_stop: XTEST button state is not verifiable via xdotool; \
             the emergency release (mouse-up + OS grant close) was requested"
        );
    }

    /// Boundary rejections through the real stack: overlong/empty/unknown
    /// chords are rejected at parse, and an out-of-bounds pointer move is
    /// clamped — the X server must never report an out-of-screen pointer.
    #[tokio::test]
    #[ignore = "drives the X server named by $DISPLAY (run against a sandboxed Xvfb)"]
    async fn x11_live_boundary_rejections() {
        let Some((width, height, display)) = live_display() else {
            eprintln!("SKIP x11_live_boundary_rejections: $DISPLAY does not answer xdotool");
            return;
        };
        let fx = fixture(display);
        fx.shared.grant_session(SESSION);

        // (1) More than 4 chord tokens → rejected at parse (use type instead).
        let (success, text) = fx
            .execute(json!({"action": "key", "text": "a+b+c+d+e"}))
            .await;
        assert!(!success, "{text}");
        assert!(
            text.contains("key chord has more than 4"),
            "overlong chord must be rejected at parse: {text}"
        );

        // (2) Empty chord → rejected.
        let (success, text) = fx.execute(json!({"action": "key", "text": ""})).await;
        assert!(!success, "{text}");
        assert!(
            text.contains("non-empty"),
            "an empty chord must be rejected: {text}"
        );

        // (3) Unknown key name → rejected.
        let (success, text) = fx
            .execute(json!({"action": "key", "text": "ctrl+nosuchkey"}))
            .await;
        assert!(!success, "{text}");
        assert!(
            text.contains("unknown key"),
            "an unknown key name must be rejected: {text}"
        );

        // (4) Out-of-bounds pointer move: clamped with a warning (a hover is
        // not T3-screened), and in NO case may the pointer land outside the
        // screen — verified by the X server itself.
        let (success, text) = fx
            .execute(json!({"action": "mouse_move", "x": 99999, "y": 99999}))
            .await;
        println!("x11_live out-of-bounds move: success={success} text={text}");
        assert!(
            success,
            "an out-of-bounds move is clamped, not rejected: {text}"
        );
        assert!(
            text.contains("clamped"),
            "the clamping must be reported as a warning: {text}"
        );
        let at = pointer_at(&fx.display).expect("xdotool");
        println!("x11_live out-of-bounds move landed at {at:?}");
        assert!(
            at.0 >= 0 && at.0 <= width as i32 - 1 && at.1 >= 0 && at.1 <= height as i32 - 1,
            "the pointer must stay on screen ({width}x{height}); xdotool reports {at:?}"
        );
        assert!(
            (at.0 - width as i32 + 1).abs() <= 2 && (at.1 - height as i32 + 1).abs() <= 2,
            "the move must be clamped to the bottom-right corner ({},{}); xdotool reports {at:?}",
            width - 1,
            height - 1
        );
    }

    #[test]
    fn release_keysyms_with_retry_retries_and_reports_first_error() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_closure = Arc::clone(&calls);
        let error = ComputerUseError::unavailable("notify timed out");
        let mut attempts = 0;
        let result = release_keysyms_with_retry(
            |keysym, pressed| {
                assert!(!pressed, "release loop must only send releases");
                let n = calls_for_closure.fetch_add(1, Ordering::SeqCst);
                // Events, in order: 0xff48 (H, release fails once), retry
                // succeeds, then 0xff1b (Esc, succeeds).
                match n {
                    0 => {
                        assert_eq!(keysym, 0xff48);
                        attempts += 1;
                        Err(error.clone())
                    }
                    1 => {
                        assert_eq!(keysym, 0xff48);
                        Ok(())
                    }
                    2 => {
                        assert_eq!(keysym, 0xff1b);
                        Ok(())
                    }
                    _ => panic!("unexpected extra event {n}"),
                }
            },
            &[0xff1b, 0xff48],
        );
        assert!(result.is_err(), "the first release error must be reported");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(attempts, 1);
    }

    #[test]
    fn release_keysyms_with_retry_all_clear_returns_ok() {
        let result = release_keysyms_with_retry(
            |keysym, pressed| {
                assert!(!pressed);
                assert_eq!(keysym, 0xff0d);
                Ok(())
            },
            &[0xff0d],
        );
        assert!(result.is_ok());
    }
}
