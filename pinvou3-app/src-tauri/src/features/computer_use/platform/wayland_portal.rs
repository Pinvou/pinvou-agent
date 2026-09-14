//! Wayland input injection and same-session screen capture: xdg-desktop-portal RemoteDesktop
//! (+ScreenCast).
//!
//! enigo has no Wayland backend, and XTest-through-XWayland can only reach X11 clients, so the
//! canonical path for Wayland input synthesis is the RemoteDesktop portal (implemented by both
//! GNOME and KDE; compositors whose `AvailableDeviceTypes` lacks keyboard/pointer are explicitly
//! unavailable at probe time and never degrade silently). The portal's system authorization
//! dialog is a second layer of user consent, independent of the in-app consent flow.
//!
//! **Why a hand-written portal client instead of `ashpd`** (round-12 review: this trade-off was
//! previously undocumented, and the next maintainer would inevitably ask again): ashpd is the
//! officially recommended Rust portal client, but this module needs to impose bounded timeouts
//! separately on the **three independent phases** of every portal round-trip (AddMatch
//! subscription, method call, Response signal wait), while ashpd's wait APIs expose neither
//! per-phase timeouts nor explicit subscription ordering for the handle_token-driven
//! Request/Response signals (subscribe-before-call, guarding against the Response-arrives-first
//! race). These two points are exactly what the `BACKEND_CALL_TIMEOUT` derivation (see
//! backend.rs) relies on: unbounded or coarse-grained waits would rob the "worst-case legitimate
//! lazy-start sum" of meaning, and zombie requests plus double injection would come back. If
//! ashpd ever provides per-phase timeouts, the migration should be re-evaluated.
//!
//! Known gaps (need real-machine portal verification, disclosed since round-10):
//! - `AvailableCursorModes` is not probed: SelectSources always requests cursor_mode=hidden,
//!   and compositor behavior when that mode is unsupported (reject vs degrade) is unverified;
//! - the **Response wait timeout** branch of `CreateSession`: the portal side may still be
//!   creating the session while we can neither obtain the handle nor close it — a half-created
//!   session may linger on the compositor side until its own timeout
//!   ([`PortalInner::abandon`] only covers failure paths that already obtained the handle).
//!
//! Session flow (lazy start, triggered by the first input action or screen capture, driven
//! throughout by `handle_token` Request/Response signal round-trips):
//! 1. `CreateSession` → take `session_handle` from the Response result (the return value itself
//!    is a Request object, historical baggage);
//! 2. `RemoteDesktop.SelectDevices` (types = KEYBOARD|POINTER);
//! 3. `ScreenCast.SelectSources` (monitor, multiple=false, cursor_mode=hidden)
//!    — the coordinate space for absolute motion is the logical space bound to the stream;
//!    without a stream there is no absolute motion (mutter errors out on unknown streams);
//! 4. `RemoteDesktop.Start` (pops the system authorization dialog; once the user decides, the
//!    Response carries `devices` and `streams`);
//! 5. `ScreenCast.OpenPipeWireRemote`: trade the same session for the capture stream's PipeWire
//!    fd and hand it to [`super::wayland_capture`] to build the frame receiver — capture and
//!    input share this one session/authorization, no longer relying on xcap's legacy capture
//!    chain (see the fallback note in `linux.rs`);
//! 6. all input goes through `Notify*`; `Session.Close` is best-effort called when the backend
//!    is dropped — any failure path after CreateSession succeeds (request error/timeout/user
//!    cancel/authorization without devices/no stream) likewise best-effort closes, so a
//!    half-authorized session does not leak on the compositor side
//!    (see [`PortalInner::abandon`]).
//!
//! Semantics follow mutter's `meta-remote-desktop-session.c`:
//! - `NotifyPointerMotionAbsolute` x/y (the d of oa{sv}udd) is **stream-local pixels**
//!   (mutter `transform_position`: global logical = display layout origin + x/scale; KDE's
//!   portal layer adds the stream's logical origin, and at scale=1 the two coincide).
//!   Stream-local pixels are the PipeWire-negotiated buffer pixels, so capture (`linux.rs`
//!   builds Capture with the negotiated size) and input share the same coordinate space, and
//!   the origin is always (0,0).
//! - `NotifyPointerAxisDiscrete` (oa{sv}ui): axis 0=vertical 1=horizontal; steps positive=
//!   down/right, negative=up/left (`discrete_steps_to_scroll_direction`); one call may carry
//!   multiple steps; steps=0 makes mutter report Invalid and trigger a session reset, so the
//!   caller must block it beforehand (see the clicks=0 no-op in `linux.rs` scroll).
//! - Keys use `NotifyKeyboardKeysym` (X keysyms; only named keys and the Latin-1 range — other
//!   Unicode does have a `0x01000000 | code point` encoding, but mutter silently drops keysyms
//!   **not in the current keymap**, so injection "succeeds" with no input; therefore
//!   [`char_keysym`] errors explicitly for characters beyond Latin-1, fail-closed).
//! - Pointer buttons are evdev button codes; pressed/released state=1/0.
//!
//! Threading contract: objects are constructed/used on the computer_use dedicated worker
//! thread; externally they are synchronous methods, internally driving async zbus via a
//! self-held current-thread runtime `block_on` (the same pattern as linux.rs's AT-SPI
//! handling, no nested runtime).

use std::collections::HashMap;
use std::future::poll_fn;
use std::os::fd::{AsFd as _, OwnedFd};
use std::pin::Pin;
use std::time::{Duration, Instant};

use zbus::export::futures_core;
use zbus::message::Type as MessageType;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use super::super::types::{ComputerUseError, Key, MouseButton, ScrollDirection};
use super::wayland_capture::{PortalFrame, PwCapture};

const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const REMOTE_DESKTOP_IFACE: &str = "org.freedesktop.portal.RemoteDesktop";
const SCREEN_CAST_IFACE: &str = "org.freedesktop.portal.ScreenCast";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";
const SESSION_IFACE: &str = "org.freedesktop.portal.Session";

/// Device type bits (shared by SelectDevices `types` and the Start response `devices`).
const DEVICE_KEYBOARD: u32 = 1;
const DEVICE_POINTER: u32 = 2;
/// ScreenCast source type: monitor.
const SOURCE_MONITOR: u32 = 1;
/// ScreenCast cursor_mode: hidden (input synthesis does not need the cursor baked into the stream).
const CURSOR_MODE_HIDDEN: u32 = 1;
/// Portal state values for press/release (shared by pointer buttons and keyboard).
const STATE_RELEASED: u32 = 0;
const STATE_PRESSED: u32 = 1;

/// Timeout for regular portal request round-trips (the authorization dialog is on Start,
/// not on these steps).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Start pops the system authorization dialog and the user may step away; on timeout close
/// the session and retry next time.
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// Timeout for Notify* event injection (a revoked session or similar must not hang the worker).
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(10);
/// Best-effort timeout for `Session.Close` (drop/failure cleanup paths; must not block long).
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
/// Overall timeout for portal probe/connection setup. zbus's default method_timeout for the
/// session bus is very generous; a hung portal service must not suspend backend construction
/// indefinitely (review finding: unbounded property queries would pin the worker).
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// evdev button codes (portal spec: Linux evdev button codes).
const EVDEV_BTN_LEFT: i32 = 0x110;
const EVDEV_BTN_RIGHT: i32 = 0x111;
const EVDEV_BTN_MIDDLE: i32 = 0x112;

/// Normalized key from types → X keysym (the encoding space of NotifyKeyboardKeysym).
pub(super) fn map_keysym(key: Key) -> Result<i32, ComputerUseError> {
    let keysym: i32 = match key {
        Key::Control => 0xffe3, // Control_L
        Key::Alt => 0xffe9,     // Alt_L
        Key::Shift => 0xffe1,   // Shift_L
        // macOS Cmd / Windows Win / Linux Super.
        Key::Meta => 0xffeb,
        Key::Enter => 0xff0d,
        Key::Escape => 0xff1b,
        Key::Tab => 0xff09,
        Key::Space => 0x0020,
        Key::Backspace => 0xff08,
        Key::Delete => 0xffff,
        Key::Insert => 0xff63,
        Key::Up => 0xff52,
        Key::Down => 0xff54,
        Key::Left => 0xff51,
        Key::Right => 0xff53,
        Key::Home => 0xff50,
        Key::End => 0xff57,
        Key::PageUp => 0xff55,
        Key::PageDown => 0xff56,
        Key::Function(n) => match n {
            1..=12 => (0xffbe + u32::from(n) - 1) as i32, // contiguous F1..F12 range
            _ => {
                return Err(ComputerUseError::unsupported(
                    "input",
                    format!("function key F{n} is out of the mappable range F1-F12"),
                ));
            }
        },
        Key::Char(c) => char_keysym(c)?,
    };
    Ok(keysym)
}

/// Single-character keysym shared by type_text's per-character injection and `Key::Char`
/// (\n → Return, \t → Tab; in the Latin-1 range the keysym equals the code point).
///
/// Characters beyond Latin-1 **error out explicitly** (fail-closed): X keysyms do have a
/// `0x01000000 | code point` encoding for them, but mutter silently drops keysyms **not in
/// the current keymap** — every per-character call "succeeds" yet not a single Chinese
/// character gets in (the silent loss found in review). Prefer explicit failure over fake
/// success.
pub(super) fn char_keysym(c: char) -> Result<i32, ComputerUseError> {
    let keysym = keysym_for_char(c).ok_or_else(|| {
        if u32::from(c) > 0xff {
            // No echo of the character itself: execution errors land
            // verbatim in the audit JSONL error field, so a password typed
            // one call at a time would leak through the error text.
            ComputerUseError::unsupported(
                "input",
                "cannot type a character outside the injectable keysym range via the \
                 Wayland portal: mutter silently drops keysyms outside the compositor \
                 keymap, so non-Latin-1 text (CJK etc.) would be lost; type ASCII/Latin-1 \
                 text or switch to a keymap that contains the character",
            )
        } else {
            ComputerUseError::unsupported(
                "input",
                "cannot type this control character via the Wayland portal: it has no \
                 injectable X keysym (only newline, tab, printable ASCII and Latin-1 do)",
            )
        }
    })?;
    i32::try_from(keysym)
        .map_err(|_| ComputerUseError::failed("keysym does not fit the portal i32 argument"))
}

/// Single char → keysym pure mapping, failing closed on everything outside
/// real X keysym coverage: '\n' → Return and '\t' → Tab, then the two ranges
/// with direct keysyms — printable ASCII 0x20..=0x7e and Latin-1 0xa0..=0xff
/// (keysym == code point). C0/C1 controls (0x01-0x1f, 0x7f, 0x80-0x9f) have
/// NO keysyms; the old 0x01..=0xff acceptance handed mutter nonexistent
/// keysyms that it silently drops — a fake-success no-op contradicting this
/// module's fail-closed design. '\r' never reaches here (type_text folds
/// CRLF/CR to '\n' first) and returns None. Non-Latin-1 also returns None:
/// no `0x01000000 | code point` encoding, since mutter drops keysyms outside
/// the active keymap (rationale and error text in [`char_keysym`]).
fn keysym_for_char(c: char) -> Option<u32> {
    let code = u32::from(c);
    match code {
        0x0a => Some(0xff0d), // type_text newline injection → Return
        0x09 => Some(0xff09), // Tab
        0x20..=0x7e | 0xa0..=0xff => Some(code),
        _ => None,
    }
}

pub(super) fn map_button(button: MouseButton) -> i32 {
    match button {
        MouseButton::Left => EVDEV_BTN_LEFT,
        MouseButton::Right => EVDEV_BTN_RIGHT,
        MouseButton::Middle => EVDEV_BTN_MIDDLE,
    }
}

/// Scroll direction → (portal axis, steps). Signs match mutter's
/// `discrete_steps_to_scroll_direction`: positive = down/right.
pub(super) fn map_discrete_scroll(direction: ScrollDirection, clicks: u32) -> (u32, i32) {
    let steps = i32::try_from(clicks).unwrap_or(i32::MAX);
    match direction {
        ScrollDirection::Up => (0, -steps),
        ScrollDirection::Down => (0, steps),
        ScrollDirection::Left => (1, -steps),
        ScrollDirection::Right => (1, steps),
    }
}

/// Portal request object path: `/org/freedesktop/portal/desktop/request/
/// <sender unique name minus the colon, dots to underscores>/<handle_token>`.
fn request_object_path(
    unique_name: &str,
    token: &str,
) -> Result<OwnedObjectPath, ComputerUseError> {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    let path = ObjectPath::try_from(format!(
        "/org/freedesktop/portal/desktop/request/{sender}/{token}"
    ))
    .map_err(|error| ComputerUseError::failed(format!("portal request path: {error}")))?;
    Ok(OwnedObjectPath::from(path))
}

/// Error for when the authorization dialog is cancelled (1) or ends abnormally (other codes).
/// Response code 1 can also occur in phases that show no dialog (CreateSession etc.), so the
/// message does not assume a dialog was ever shown.
fn response_error(code: u32) -> ComputerUseError {
    match code {
        1 => ComputerUseError::unavailable(
            "the portal request was cancelled (response code 1; if an authorization dialog \
             was shown, it was dismissed) — the action was not granted",
        ),
        code => ComputerUseError::unavailable(format!(
            "the system authorization flow ended unexpectedly (portal response code {code})"
        )),
    }
}

/// Extract a u32 from a Value (OwnedValue derefs to Value, so call sites get auto-deref for free).
fn value_u32(value: &Value<'_>) -> Option<u32> {
    match value {
        Value::U32(value) => Some(*value),
        _ => None,
    }
}

/// MessageStream only implements `futures_core::Stream` (re-exported via `zbus::export`);
/// without pulling in a new dependency, hand-write `next` with `poll_fn`.
async fn next_message(stream: &mut zbus::MessageStream) -> Option<zbus::Result<zbus::Message>> {
    poll_fn(|cx| futures_core::stream::Stream::poll_next(Pin::new(stream), cx)).await
}

/// Strip any number of variant wrappers (zvariant may store a{sv} values as Value::Value).
fn unwrap_variant<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
    let mut value = value;
    while let Value::Value(inner) = value {
        value = inner;
    }
    value
}

/// Extract an (i32, i32) from an a{sv} (accepts either a struct or a two-element array).
fn dict_i32_pair(dict: &zbus::zvariant::Dict<'_, '_>, key: &str) -> Option<(i32, i32)> {
    let entry = dict
        .iter()
        .find(|(dict_key, _)| {
            matches!(unwrap_variant(dict_key), Value::Str(name) if name.as_str() == key)
        })
        .map(|(_, value)| value)?;
    // The dict value may also be variant-wrapped; strip that uniformly.
    let value = unwrap_variant(entry);
    let ints = |fields: &[Value<'_>]| -> Option<(i32, i32)> {
        match fields {
            [Value::I32(a), Value::I32(b)] => Some((*a, *b)),
            _ => None,
        }
    };
    match value {
        Value::Structure(structure) => ints(structure.fields()),
        Value::Array(array) => {
            let fields: Vec<Value<'_>> = array.iter().cloned().collect();
            ints(&fields)
        }
        _ => None,
    }
}

/// The Start response's streams (a(ua{sv})): take the first stream's PipeWire node id and the
/// stream logical size reported by the compositor (`size`, used for the KDE input scale
/// conversion; None when the compositor leaves it unset, in which case the input scale is
/// treated as 1.0). vardict values may all be variant-wrapped.
fn first_stream_info(results: &HashMap<String, OwnedValue>) -> Option<(u32, Option<(i32, i32)>)> {
    let Value::Array(array) = unwrap_variant(&**results.get("streams")?) else {
        return None;
    };
    let Value::Structure(structure) = array.iter().next()? else {
        return None;
    };
    let node = value_u32(unwrap_variant(structure.fields().first()?))?;
    let logical = structure.fields().get(1).and_then(|entry| {
        let Value::Dict(dict) = unwrap_variant(entry) else {
            return None;
        };
        dict_i32_pair(dict, "size")
    });
    Some((node, logical))
}

/// State of a started portal session.
struct PortalSession {
    path: OwnedObjectPath,
    /// Device bitmask actually granted by the user in the authorization dialog.
    devices: u32,
    /// Bound ScreenCast stream (PipeWire node id), the coordinate reference for absolute motion.
    stream: u32,
    /// Stream logical size reported by the compositor (for diagnostics and KDE input scale
    /// conversion; optional).
    stream_logical: Option<(i32, i32)>,
    /// PipeWire frame receiver for same-session capture (None when OpenPipeWireRemote fails;
    /// capture falls back to the xcap chain, input is unaffected).
    capture: Option<PwCapture>,
}

/// Wayland portal input backend (synchronous facade). Lazy start: session establishment and
/// the system authorization dialog only happen on the first input action or screen capture;
/// the session is then reused and rebuilt automatically when it becomes invalid.
pub(super) struct PortalInput {
    /// Dedicated current-thread runtime: drives async zbus inside synchronous methods (same as AT-SPI).
    runtime: tokio::runtime::Runtime,
    inner: PortalInner,
}

impl PortalInput {
    /// Probe the portal and RemoteDesktop input support (pure property queries, no dialogs
    /// popped). On failure returns a human-readable reason, which the caller stores as a
    /// sticky error.
    ///
    /// Bounded overall by [`PROBE_TIMEOUT`]: zbus's default method_timeout is very generous,
    /// and a hung portal service must not suspend backend construction indefinitely (review
    /// finding).
    pub(super) fn probe() -> Result<(), String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot create probe runtime: {error}"))?;
        runtime.block_on(async {
            match tokio::time::timeout(PROBE_TIMEOUT, Self::probe_async()).await {
                Ok(result) => result,
                Err(_) => Err(format!(
                    "portal probe did not finish within {PROBE_TIMEOUT:?}"
                )),
            }
        })
    }

    async fn probe_async() -> Result<(), String> {
        let conn = zbus::Connection::session()
            .await
            .map_err(|error| format!("cannot connect to the session bus: {error}"))?;
        let proxy = zbus::Proxy::new(&conn, PORTAL_DEST, PORTAL_PATH, REMOTE_DESKTOP_IFACE)
            .await
            .map_err(|error| format!("portal proxy: {error}"))?;
        let version: u32 = proxy
            .get_property("version")
            .await
            .map_err(|error| format!("cannot read portal RemoteDesktop version: {error}"))?;
        if version < 1 {
            return Err(format!(
                "xdg-desktop-portal RemoteDesktop is version {version}; this implementation \
                 needs the session-handle based interface (version >= 1)"
            ));
        }
        let available: u32 = proxy
            .get_property("AvailableDeviceTypes")
            .await
            .map_err(|error| format!("cannot read AvailableDeviceTypes: {error}"))?;
        if available & (DEVICE_KEYBOARD | DEVICE_POINTER) == 0 {
            return Err(
                "xdg-desktop-portal RemoteDesktop advertises no keyboard/pointer support for \
                 this compositor"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub(super) fn new() -> Result<Self, ComputerUseError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                ComputerUseError::unavailable(format!("cannot create portal runtime: {error}"))
            })?;
        // Connection setup is likewise bounded by PROBE_TIMEOUT (the session bus handshake may hang).
        let inner = runtime.block_on(async {
            match tokio::time::timeout(PROBE_TIMEOUT, PortalInner::new()).await {
                Ok(result) => result,
                Err(_) => Err(ComputerUseError::unavailable(format!(
                    "portal session bus connection did not finish within {PROBE_TIMEOUT:?}"
                ))),
            }
        })?;
        Ok(Self { runtime, inner })
    }

    /// Return directly if the session is already started; otherwise run the full establishment
    /// flow (pops the system authorization dialog).
    pub(super) fn ensure_started(&mut self) -> Result<(), ComputerUseError> {
        self.runtime.block_on(self.inner.ensure_started())
    }

    /// Whether the session object is still open (including poisoned, not-yet-recycled
    /// sessions). The query itself never lazy-starts. Poisoning is only our own marker: the
    /// session is still alive on the compositor side and Notify can still be delivered (notify
    /// does not short-circuit on poison). The emergency mouse_up uses this rather than a
    /// "healthy" check — mutter's Session.Close only destroys the virtual device and does not
    /// synthesize releases, so skipping poisoned sessions would strand already-pressed buttons
    /// forever (round-12 review M1); with no session the caller must return directly and must
    /// not trigger the full establishment flow (that would pop a new system authorization
    /// dialog — review finding: the revoke/emergency-stop's emergency release demanding
    /// authorization in reverse).
    pub(super) fn has_open_session(&self) -> bool {
        self.inner.session.is_some()
    }

    pub(super) fn motion_absolute(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        self.runtime.block_on(self.inner.motion_absolute(x, y))
    }

    pub(super) fn button(
        &mut self,
        evdev_button: i32,
        pressed: bool,
    ) -> Result<(), ComputerUseError> {
        self.runtime
            .block_on(self.inner.button(evdev_button, pressed))
    }

    pub(super) fn axis_discrete(&mut self, axis: u32, steps: i32) -> Result<(), ComputerUseError> {
        self.runtime.block_on(self.inner.axis_discrete(axis, steps))
    }

    pub(super) fn keysym_event(
        &mut self,
        keysym: i32,
        pressed: bool,
    ) -> Result<(), ComputerUseError> {
        self.runtime
            .block_on(self.inner.keysym_event(keysym, pressed))
    }

    /// Best-effort close of the portal session when the backend is dropped (an authorized
    /// session must not leak).
    pub(super) fn close(&mut self) {
        self.runtime.block_on(self.inner.close());
    }

    /// Cursor position produced by our own injections (logical global coordinates); Wayland
    /// has no query API, so this is tracking only.
    pub(super) fn last_pointer(&self) -> Option<(i32, i32)> {
        self.inner.last_pointer
    }

    pub(super) fn track_pointer(&mut self, x: i32, y: i32) {
        self.inner.last_pointer = Some((x, y));
    }

    /// Same-session capture: start the session if not started (may pop the system
    /// authorization dialog), then take the latest frame from the bound ScreenCast stream.
    /// See [`PwCapture::latest_frame`] for `not_before` semantics.
    pub(super) fn capture_frame(
        &mut self,
        not_before: Option<Instant>,
    ) -> Result<PortalFrame, ComputerUseError> {
        self.runtime.block_on(async {
            self.inner.ensure_started().await?;
            let session = self
                .inner
                .session
                .as_mut()
                .ok_or_else(|| ComputerUseError::unavailable("portal session not started"))?;
            match session.capture.as_mut() {
                Some(capture) => capture.latest_frame(not_before),
                None => Err(ComputerUseError::unavailable(format!(
                    "same-session capture is not available: {}",
                    self.inner
                        .capture_error
                        .as_deref()
                        .unwrap_or("OpenPipeWireRemote failed")
                ))),
            }
        })
    }

    /// Stream logical size reported by the compositor (for KDE input scale conversion; optional).
    pub(super) fn stream_logical_size(&self) -> Option<(i32, i32)> {
        self.inner
            .session
            .as_ref()
            .and_then(|session| session.stream_logical)
    }
}

/// The async body of the portal session (state and D-Bus connection; `PortalInput`'s runtime
/// is kept separate here so the `block_on` borrow and the future body borrow never overlap).
struct PortalInner {
    conn: zbus::Connection,
    session: Option<PortalSession>,
    /// Why capture initialization failed for the current session (input unaffected; capture
    /// falls back to the xcap chain).
    capture_error: Option<String>,
    /// Cursor position produced by our own injections (logical global coordinates). Wayland
    /// has no query API, so this can only be tracked.
    last_pointer: Option<(i32, i32)>,
    request_counter: u32,
    /// Counter for CreateSession's session_handle_token (the router uses it to name the session).
    session_counter: u32,
    /// A notify failure/timeout midway through the previous action: the session **may** still
    /// be alive and authorized, but is no longer trustworthy. A poisoned session is closed
    /// only after the action's mandatory release is delivered (see `button`'s action-boundary
    /// recycling) and is recycled/rebuilt by the next `ensure_started` — closing before
    /// releasing would make the forced release of a stuck drag key impossible to deliver
    /// (review finding).
    poisoned: bool,
}

impl PortalInner {
    async fn new() -> Result<Self, ComputerUseError> {
        let conn = zbus::Connection::session()
            .await
            .map_err(|error| ComputerUseError::unavailable(format!("session bus: {error}")))?;
        Ok(Self {
            conn,
            session: None,
            capture_error: None,
            last_pointer: None,
            request_counter: 0,
            session_counter: 0,
            poisoned: false,
        })
    }

    fn next_handle_token(&mut self) -> String {
        self.request_counter = self.request_counter.wrapping_add(1);
        format!("pinvou_cu_{}", self.request_counter)
    }

    fn unique_sender(&self) -> Result<String, ComputerUseError> {
        self.conn
            .unique_name()
            .map(|name| name.to_string())
            .ok_or_else(|| ComputerUseError::unavailable("portal connection has no unique name"))
    }

    /// Issue one portal request and wait for the Response signal on the corresponding Request
    /// object. Subscribe before calling, to avoid the race where the Response arrives before
    /// the subscription does.
    ///
    /// When `session` is `Some` the method signature is `(o session_handle, a{sv} options)`
    /// (everything except CreateSession); when `None` it is `(a{sv} options)`.
    async fn request(
        &mut self,
        interface: &str,
        method: &'static str,
        session: Option<&OwnedObjectPath>,
        extra: Vec<(&'static str, OwnedValue)>,
        timeout: Duration,
    ) -> Result<HashMap<String, OwnedValue>, ComputerUseError> {
        self.request_impl(interface, method, session, None, extra, timeout)
            .await
    }

    /// When `parent_window` is `Some` the method signature is `(o session_handle,
    /// s parent_window, a{sv} options)`: matches the parameter order of the spec XML
    /// (verified in practice against xdg-desktop-portal 1.18 too). Missing arguments are
    /// rejected with InvalidArgs.
    #[allow(clippy::too_many_arguments)]
    async fn request_impl(
        &mut self,
        interface: &str,
        method: &'static str,
        session: Option<&OwnedObjectPath>,
        parent_window: Option<&str>,
        extra: Vec<(&'static str, OwnedValue)>,
        timeout: Duration,
    ) -> Result<HashMap<String, OwnedValue>, ComputerUseError> {
        let token = self.next_handle_token();
        let request_path = request_object_path(&self.unique_sender()?, &token)?;

        let rule = zbus::MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(PORTAL_DEST)
            .and_then(|builder| builder.path(request_path.clone()))
            .and_then(|builder| builder.interface(REQUEST_IFACE))
            .and_then(|builder| builder.member("Response"))
            .map(|builder| builder.build())
            .map_err(|error| {
                ComputerUseError::unavailable(format!("portal response match rule: {error}"))
            })?;
        // AddMatch is itself a D-Bus round-trip, and zbus's default method
        // timeout is unbounded: without this wrap it is the only unbounded
        // await on the lazy-start path.
        let mut stream = tokio::time::timeout(
            REQUEST_TIMEOUT,
            zbus::MessageStream::for_match_rule(rule, &self.conn, None),
        )
        .await
        .map_err(|_| {
            ComputerUseError::unavailable(format!(
                "portal {method} response subscription timed out after {REQUEST_TIMEOUT:?}"
            ))
        })?
        .map_err(|error| {
            ComputerUseError::unavailable(format!("portal response subscription: {error}"))
        })?;

        let mut options: HashMap<&str, OwnedValue> = HashMap::new();
        let token_value = Value::from(token.as_str())
            .try_to_owned()
            .map_err(|error| ComputerUseError::failed(format!("portal handle_token: {error}")))?;
        options.insert("handle_token", token_value);
        for (key, value) in extra {
            options.insert(key, value);
        }

        // The two branches' call_method produce different opaque future types; each is awaited
        // to completion and then unified into Result<Message>. The method call itself should
        // return in milliseconds, capped at REQUEST_TIMEOUT; the authorization-dialog wait
        // happens on the Response signal (next phase) — the old shape of giving each phase its
        // own full timeout made Start's worst-case waits stack up multiplicatively and blow
        // through the backend layer's call budget (review finding).
        let call_deadline = timeout.min(REQUEST_TIMEOUT);
        tokio::time::timeout(call_deadline, async {
            match session {
                Some(path) if parent_window.is_some() => {
                    self.conn
                        .call_method(
                            Some(PORTAL_DEST),
                            PORTAL_PATH,
                            Some(interface),
                            method,
                            &(path, parent_window.unwrap_or_default(), &options),
                        )
                        .await
                }
                Some(path) => {
                    self.conn
                        .call_method(
                            Some(PORTAL_DEST),
                            PORTAL_PATH,
                            Some(interface),
                            method,
                            &(path, &options),
                        )
                        .await
                }
                None => {
                    self.conn
                        .call_method(
                            Some(PORTAL_DEST),
                            PORTAL_PATH,
                            Some(interface),
                            method,
                            &(&options,),
                        )
                        .await
                }
            }
        })
        .await
        .map_err(|_| {
            // Report the actually effective method-phase cap (timeout.min(REQUEST_TIMEOUT)),
            // otherwise Start would falsely claim "timed out after 120s" (it actually fires at 30s).
            ComputerUseError::unavailable(format!(
                "portal {method} timed out after {call_deadline:?}"
            ))
        })?
        .map_err(|error| ComputerUseError::unavailable(format!("portal {method}: {error}")))?;

        let message = tokio::time::timeout(timeout, next_message(&mut stream))
            .await
            .map_err(|_| {
                ComputerUseError::unavailable(format!(
                    "portal {method} response timed out after {timeout:?}"
                ))
            })?
            .ok_or_else(|| {
                ComputerUseError::unavailable(format!("portal {method} response stream closed"))
            })?
            .map_err(|error| {
                ComputerUseError::unavailable(format!("portal {method} response: {error}"))
            })?;
        let (code, results): (u32, HashMap<String, OwnedValue>) =
            message.body().deserialize().map_err(|error| {
                ComputerUseError::unavailable(format!("portal {method} response body: {error}"))
            })?;
        if code != 0 {
            return Err(response_error(code));
        }
        Ok(results)
    }

    /// Full establishment flow (pops the system authorization dialog). After CreateSession
    /// succeeds, any later step failing (request error/timeout/user cancel/authorization
    /// without devices/no stream) must close the created session object before returning
    /// (the leak path found in review, see [`PortalInner::abandon`]).
    async fn ensure_started(&mut self) -> Result<(), ComputerUseError> {
        // Poisoned-session recycling: the close deferred after the previous action's failure
        // is made up here (bounded best-effort) before the full establishment flow — never
        // coexist with the leftover session, and never pop a second authorization dialog.
        if self.poisoned {
            self.reset_closed().await;
        }
        if self.session.is_some() {
            return Ok(());
        }

        // 1) CreateSession. `session_handle_token` is what the router uses to name the session
        //    object; xdg-desktop-portal >= 1.17 rejects the call without it ("Missing token").
        self.session_counter = self.session_counter.wrapping_add(1);
        let results = self
            .request(
                REMOTE_DESKTOP_IFACE,
                "CreateSession",
                None,
                vec![(
                    "session_handle_token",
                    Value::from(format!("pinvou_cu_s{}", self.session_counter))
                        .try_to_owned()
                        .map_err(|error| {
                            ComputerUseError::failed(format!("portal session token: {error}"))
                        })?,
                )],
                REQUEST_TIMEOUT,
            )
            .await?;
        // Verified in practice: xdg-desktop-portal 1.18 puts session_handle into the response
        // as a string (the spec says o); accept both forms. When no valid handle can be parsed
        // there is nothing to close, and the malformed response is itself an error (raised
        // directly).
        let session_path = match results
            .get("session_handle")
            .map(|owned| unwrap_variant(owned))
        {
            Some(Value::ObjectPath(path)) => OwnedObjectPath::from(path.clone()),
            Some(Value::Str(path)) => {
                let path = ObjectPath::try_from(path.as_str()).map_err(|error| {
                    ComputerUseError::unavailable(format!(
                        "portal CreateSession response has an invalid session_handle: {error}"
                    ))
                })?;
                OwnedObjectPath::from(path)
            }
            _ => {
                return Err(ComputerUseError::unavailable(
                    "portal CreateSession response has a non-object session_handle",
                ));
            }
        };

        // 2) SelectDevices: keyboard + pointer.
        if let Err(error) = self
            .request(
                REMOTE_DESKTOP_IFACE,
                "SelectDevices",
                Some(&session_path),
                vec![("types", OwnedValue::from(DEVICE_KEYBOARD | DEVICE_POINTER))],
                REQUEST_TIMEOUT,
            )
            .await
        {
            self.abandon(session_path).await;
            return Err(error);
        }

        // 3) ScreenCast.SelectSources: bind one monitor stream, otherwise absolute-motion
        // coordinates have no reference.
        if let Err(error) = self
            .request(
                SCREEN_CAST_IFACE,
                "SelectSources",
                Some(&session_path),
                vec![
                    ("types", OwnedValue::from(SOURCE_MONITOR)),
                    ("multiple", OwnedValue::from(false)),
                    ("cursor_mode", OwnedValue::from(CURSOR_MODE_HIDDEN)),
                ],
                REQUEST_TIMEOUT,
            )
            .await
        {
            self.abandon(session_path).await;
            return Err(error);
        }

        // 4) Start: system authorization dialog (the second, user-visible layer of consent).
        //    The signature includes the parent window; cancel/timeout must also close the session.
        let results = match self
            .request_impl(
                REMOTE_DESKTOP_IFACE,
                "Start",
                Some(&session_path),
                Some(""),
                vec![],
                START_TIMEOUT,
            )
            .await
        {
            Ok(results) => results,
            Err(error) => {
                self.abandon(session_path).await;
                return Err(error);
            }
        };
        let devices = results
            .get("devices")
            .and_then(|owned| value_u32(unwrap_variant(owned)))
            .unwrap_or(0);
        if devices & (DEVICE_KEYBOARD | DEVICE_POINTER) == 0 {
            self.abandon(session_path).await;
            return Err(ComputerUseError::unavailable(
                "the system authorization dialog granted no keyboard/pointer devices",
            ));
        }
        let Some((stream, stream_logical)) = first_stream_info(&results) else {
            self.abandon(session_path).await;
            return Err(ComputerUseError::unavailable(
                "portal Start response has no ScreenCast stream; absolute pointer motion has \
                 no coordinate reference",
            ));
        };

        // 5) Same-session capture: OpenPipeWireRemote trades for the PipeWire fd and builds
        //    the frame receiver. Failure does not give up the session: input stays usable,
        //    capture falls back to the xcap chain.
        let (capture, capture_error) = match self.open_pipewire_remote(&session_path).await {
            Ok(fd) => match PwCapture::spawn(stream, fd) {
                Ok(capture) => (Some(capture), None),
                Err(error) => (None, Some(error.to_string())),
            },
            Err(error) => (None, Some(error.to_string())),
        };
        self.capture_error = capture_error;

        self.session = Some(PortalSession {
            path: session_path,
            devices,
            stream,
            stream_logical,
            capture,
        });
        self.last_pointer = None;
        Ok(())
    }

    /// `ScreenCast.OpenPipeWireRemote`: trade the current session for a private PipeWire
    /// connection fd to the capture stream (a synchronous quick method, not a Request
    /// round-trip).
    async fn open_pipewire_remote(
        &self,
        session_path: &OwnedObjectPath,
    ) -> Result<OwnedFd, ComputerUseError> {
        let options: HashMap<&str, OwnedValue> = HashMap::new();
        let reply = tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.conn.call_method(
                Some(PORTAL_DEST),
                PORTAL_PATH,
                Some(SCREEN_CAST_IFACE),
                "OpenPipeWireRemote",
                &(&session_path, &options),
            ),
        )
        .await
        .map_err(|_| ComputerUseError::unavailable("portal OpenPipeWireRemote timed out"))?
        .map_err(|error| {
            ComputerUseError::unavailable(format!("portal OpenPipeWireRemote: {error}"))
        })?;
        let fd = reply
            .body()
            .deserialize::<zbus::zvariant::OwnedFd>()
            .map_err(|error| {
                ComputerUseError::unavailable(format!(
                    "portal OpenPipeWireRemote returned no fd: {error}"
                ))
            })?;
        // Duplicate the portal-side fd for the PipeWire thread; the zvariant wrapper is freed
        // with the message.
        fd.as_fd()
            .try_clone_to_owned()
            .map_err(|error| ComputerUseError::unavailable(format!("fd clone: {error}")))
    }

    /// Resolve and clone the session path (the borrow cannot span `notify`'s `&mut self`:
    /// the call body also borrows self.conn again to issue the method call).
    fn session_path(&self) -> Result<OwnedObjectPath, ComputerUseError> {
        self.session
            .as_ref()
            .map(|session| session.path.clone())
            .ok_or_else(|| ComputerUseError::unavailable("portal session not started"))
    }

    /// Notify* event injection (no Response round-trip; the method returning means delivered).
    /// When the session becomes invalid midway (compositor revoke, portal restart), clear the
    /// session state; the next action rebuilds it and re-authorizes.
    async fn notify(
        &mut self,
        method: &'static str,
        body: impl serde::ser::Serialize + zbus::zvariant::DynamicType,
    ) -> Result<(), ComputerUseError> {
        let call = self.conn.call_method(
            Some(PORTAL_DEST),
            PORTAL_PATH,
            Some(REMOTE_DESKTOP_IFACE),
            method,
            &body,
        );
        match tokio::time::timeout(NOTIFY_TIMEOUT, call).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => {
                // The session may still be alive and authorized on the
                // compositor side, so it must eventually be closed — but
                // closing it right now would also strand the mandatory
                // release that follows within the same action (a drag's
                // interpolated move fails → the drag's button-up could no
                // longer be delivered, leaving the button held at the
                // compositor). Mark the session poisoned instead: the
                // in-flight action's release is still delivered against the
                // open session, and the next `ensure_started` recycles it
                // (close + fresh start), so no second authorization dialog
                // stacks up beside an abandoned session.
                self.poisoned = true;
                Err(ComputerUseError::unavailable(format!(
                    "portal {method}: {error} (session poisoned; it is closed after the \
                     in-flight action's release, and the next input action reopens the \
                     authorization dialog)"
                )))
            }
            Err(_) => {
                // Same reasoning as the error branch: a timeout says nothing
                // about the session itself (it is probably still live and
                // authorized), which is exactly why it must be closed after
                // the action's release rather than immediately.
                self.poisoned = true;
                Err(ComputerUseError::unavailable(format!(
                    "portal {method} timed out after {NOTIFY_TIMEOUT:?} (session poisoned; \
                     closed after the in-flight action's release)"
                )))
            }
        }
    }

    /// Close and clear the current session's session-level state. Called from: `ensure_started`'s
    /// poisoned-session recycling (notify failure/timeout marks the session poisoned, deferred
    /// until after the action's mandatory release is delivered — review finding: closing
    /// immediately would make the forced release of a stuck drag key impossible to deliver),
    /// and `button`'s action-boundary recycling after release. `PortalSession` has no Drop impl, so
    /// `Session.Close` must be sent explicitly: take the session and close
    /// it via `abandon` (bounded by CLOSE_TIMEOUT, close errors logged — the
    /// caller's original error is what matters), then clear the remaining
    /// per-session state. Dropping the taken `PortalSession` also drops its
    /// `PwCapture`, stopping the stream and joining its thread.
    async fn reset_closed(&mut self) {
        if let Some(session) = self.session.take() {
            self.abandon(session.path).await;
        }
        self.capture_error = None;
        self.last_pointer = None;
        self.poisoned = false;
    }

    /// Close a portal session that was created but not yet registered (`self.session`) or is
    /// being destroyed: best-effort `Session.Close` (short timeout), and clear pointer
    /// tracking. Review finding: if a failure path after CreateSession succeeds only resets
    /// local state, the half-authorized session leaks on the compositor side. Failures/
    /// timeouts of the close itself are logged (review finding: when revoke reports success
    /// but OS-level authorization actually lingers, neither the user nor the logs have any
    /// clue; when the portal disconnects, the compositor side cleans up as a fallback, so
    /// only log, no retry).
    async fn abandon(&mut self, session_path: OwnedObjectPath) {
        let path = session_path.as_str().to_owned();
        let closed = tokio::time::timeout(
            CLOSE_TIMEOUT,
            self.conn.call_method(
                Some(PORTAL_DEST),
                session_path,
                Some(SESSION_IFACE),
                "Close",
                &(),
            ),
        )
        .await;
        match closed {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                eprintln!("[computer_use] portal Session.Close failed for {path}: {error}")
            }
            Err(_) => eprintln!(
                "[computer_use] portal Session.Close timed out after {CLOSE_TIMEOUT:?} for {path}"
            ),
        }
        self.last_pointer = None;
    }

    /// Best-effort close of the portal session (when the backend is dropped).
    async fn close(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        self.abandon(session.path).await;
    }

    /// When the session is started, verify the device bits the user actually granted (review
    /// finding: GNOME's authorization dialog has per-device toggles; checking only the union
    /// makes a pointer-only authorization's keyboard action error on `NotifyKeyboardKeysym` →
    /// reset → the next action pops the authorization dialog again, trapping the user in a
    /// dialog loop; erroring explicitly before injection breaks the loop).
    fn require_device(&self, device: u32, what: &str) -> Result<(), ComputerUseError> {
        let Some(session) = self.session.as_ref() else {
            // Not-started is guaranteed by ensure_started up the call chain; do not duplicate
            // the error here.
            return Ok(());
        };
        if session.devices & device == 0 {
            return Err(ComputerUseError::unavailable(format!(
                "the RemoteDesktop authorization did not include the {what}; \
                 ask the user to re-grant control and enable it in the system dialog"
            )));
        }
        Ok(())
    }

    async fn motion_absolute(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        self.require_device(DEVICE_POINTER, "pointer")?;
        let path = self.session_path()?;
        let options: HashMap<&str, OwnedValue> = HashMap::new();
        let stream = self.session.as_ref().map(|s| s.stream).unwrap_or_default();
        let body = (&path, &options, stream, f64::from(x), f64::from(y));
        self.notify("NotifyPointerMotionAbsolute", body).await
    }

    async fn button(&mut self, evdev_button: i32, pressed: bool) -> Result<(), ComputerUseError> {
        self.require_device(DEVICE_POINTER, "pointer")?;
        let state = if pressed {
            STATE_PRESSED
        } else {
            STATE_RELEASED
        };
        let path = self.session_path()?;
        let options: HashMap<&str, OwnedValue> = HashMap::new();
        let body = (&path, &options, evdev_button, state);
        let result = self.notify("NotifyPointerButton", body).await;
        // Action-boundary recycling: once the release (pressed=false) succeeds, the poisoned
        // session has served its purpose; close it here — not before the release (review
        // finding: closing first would make the drag's forced release fail with "portal
        // session not started", leaving the button stuck on the compositor side). On release
        // failure stay poisoned; the next ensure_started recycles as a fallback.
        if !pressed && self.poisoned && result.is_ok() {
            self.reset_closed().await;
        }
        result
    }

    async fn axis_discrete(&mut self, axis: u32, steps: i32) -> Result<(), ComputerUseError> {
        self.require_device(DEVICE_POINTER, "pointer")?;
        let path = self.session_path()?;
        let options: HashMap<&str, OwnedValue> = HashMap::new();
        let body = (&path, &options, axis, steps);
        self.notify("NotifyPointerAxisDiscrete", body).await
    }

    async fn keysym_event(&mut self, keysym: i32, pressed: bool) -> Result<(), ComputerUseError> {
        self.require_device(DEVICE_KEYBOARD, "keyboard")?;
        let state = if pressed {
            STATE_PRESSED
        } else {
            STATE_RELEASED
        };
        let path = self.session_path()?;
        let options: HashMap<&str, OwnedValue> = HashMap::new();
        let body = (&path, &options, keysym, state);
        self.notify("NotifyKeyboardKeysym", body).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysym_covers_named_keys() {
        assert_eq!(map_keysym(Key::Enter).ok(), Some(0xff0d));
        assert_eq!(map_keysym(Key::Control).ok(), Some(0xffe3));
        assert_eq!(map_keysym(Key::Meta).ok(), Some(0xffeb));
        assert_eq!(map_keysym(Key::Up).ok(), Some(0xff52));
        assert_eq!(map_keysym(Key::PageDown).ok(), Some(0xff56));
        assert_eq!(map_keysym(Key::Space).ok(), Some(0x20));
    }

    #[test]
    fn latin1_chars_map_and_non_latin1_fail_closed() {
        // Latin-1 range: keysym equals the code point, injection kept.
        assert_eq!(map_keysym(Key::Char('s')).ok(), Some(0x73));
        assert_eq!(map_keysym(Key::Char('S')).ok(), Some(0x53));
        assert_eq!(map_keysym(Key::Char('+')).ok(), Some(0x2b));
        // U+4E2D: the old implementation encoded it as 0x01000000|code point and sent it
        // anyway, but mutter silently drops keysyms outside the keymap — per-character
        // "success" with no input (the silent CJK loss found in review). It now errors
        // explicitly, with the error text explaining the Wayland limitation.
        let error = map_keysym(Key::Char('中')).unwrap_err().to_string();
        assert!(error.contains("Wayland"), "{error}");
        assert!(error.contains("drops keysyms"), "{error}");
        // Echo hygiene: the character (and its code point) must NOT appear
        // in the error — execution errors land verbatim in the audit JSONL,
        // so a password typed one call at a time would leak through it.
        assert!(!error.contains('中'), "{error}");
        assert!(!error.contains("U+4E2D"), "{error}");
        let error = char_keysym('\u{1F600}').unwrap_err().to_string();
        assert!(error.contains("drops keysyms"), "{error}");
        assert!(!error.contains("\u{1F600}"), "{error}");
        // Newline/tab map to named keys; NUL and DEL have no keysym.
        assert_eq!(keysym_for_char('\n'), Some(0xff0d));
        assert_eq!(keysym_for_char('\t'), Some(0xff09));
        assert_eq!(keysym_for_char('\0'), None);
        // Keysym range fails closed: C0/C1 controls have no X keysyms (the
        // old 0x01..=0xff acceptance produced keysyms mutter silently
        // drops — a fake-success no-op).
        assert_eq!(keysym_for_char('\r'), None); // unreachable after type_text folding
        assert_eq!(keysym_for_char('\u{1b}'), None); // ESC
        assert_eq!(keysym_for_char('\u{7f}'), None); // DEL
        assert_eq!(keysym_for_char('\u{80}'), None); // C1 range
        assert_eq!(keysym_for_char('\u{9f}'), None);
        // The injectable ranges themselves: printable ASCII and Latin-1.
        assert_eq!(keysym_for_char('\u{20}'), Some(0x20));
        assert_eq!(keysym_for_char('\u{7e}'), Some(0x7e));
        assert_eq!(keysym_for_char('\u{a0}'), Some(0xa0));
        assert_eq!(keysym_for_char('\u{ff}'), Some(0xff));
        // Latin-1 boundary: 0xFF (ÿ) is injectable, 0x100 (Ā) errors.
        assert_eq!(char_keysym('\u{FF}').ok(), Some(0xff));
        assert!(char_keysym('\u{100}').is_err());
    }

    #[test]
    fn keysym_function_keys_f1_to_f12() {
        assert_eq!(map_keysym(Key::Function(1)).ok(), Some(0xffbe));
        assert_eq!(map_keysym(Key::Function(12)).ok(), Some(0xffc9));
        assert!(map_keysym(Key::Function(13)).is_err());
        assert!(map_keysym(Key::Function(0)).is_err());
    }

    #[test]
    fn buttons_use_evdev_codes() {
        assert_eq!(map_button(MouseButton::Left), 0x110);
        assert_eq!(map_button(MouseButton::Right), 0x111);
        assert_eq!(map_button(MouseButton::Middle), 0x112);
    }

    #[test]
    fn discrete_scroll_signs_match_mutter() {
        // mutter discrete_steps_to_scroll_direction: positive=down/right, negative=up/left.
        assert_eq!(map_discrete_scroll(ScrollDirection::Up, 3), (0, -3));
        assert_eq!(map_discrete_scroll(ScrollDirection::Down, 3), (0, 3));
        assert_eq!(map_discrete_scroll(ScrollDirection::Left, 1), (1, -1));
        assert_eq!(map_discrete_scroll(ScrollDirection::Right, 1), (1, 1));
        // amount=0 maps to 0 steps: the caller (linux.rs) must no-op first in the portal
        // branch, otherwise mutter reports Invalid for steps=0 and notify() resets the whole
        // session.
        assert_eq!(map_discrete_scroll(ScrollDirection::Up, 0), (0, 0));
        assert_eq!(map_discrete_scroll(ScrollDirection::Right, 0), (1, 0));
    }

    #[test]
    fn request_path_mangles_sender_and_token() {
        let path = request_object_path(":1.42", "pinvou_cu_1").unwrap();
        assert_eq!(
            path.as_str(),
            "/org/freedesktop/portal/desktop/request/1_42/pinvou_cu_1"
        );
    }

    #[test]
    fn response_codes_map_to_unavailable() {
        for code in [1u32, 2, 7] {
            let error = response_error(code);
            assert!(error.to_string().starts_with("unavailable:"), "{error}");
        }
    }

    #[test]
    fn stream_info_is_extracted_from_start_results() {
        // A real Start response's streams is a(ua{sv}): elements are (node u, a{sv}) structs.
        // Note it must not be built via Value::from(Vec<Value>) — that wraps every element in
        // another variant (Value::Value), turning the shape into a(v).
        let element_signature = zbus::zvariant::Signature::try_from("(ua{sv})").expect("sig");
        let mut array = zbus::zvariant::Array::new(&element_signature);
        let mut stream_props: HashMap<String, Value<'static>> = HashMap::new();
        // vardict values are variant-wrapped on the wire: Value::Value((i32,i32)).
        stream_props.insert(
            "size".to_string(),
            Value::Value(Box::new(Value::from((1920, 1080)))),
        );
        array
            .append(Value::Structure(zbus::zvariant::Structure::from((
                7u32,
                stream_props,
            ))))
            .expect("append");
        let mut results: HashMap<String, OwnedValue> = HashMap::new();
        // The Response result dict's values are likewise variant-wrapped: Value::Value(a(ua{sv})).
        results.insert(
            "streams".to_string(),
            Value::Value(Box::new(Value::Array(array)))
                .try_to_owned()
                .expect("owned"),
        );
        let (node, logical) = first_stream_info(&results).expect("stream info");
        assert_eq!(node, 7);
        assert_eq!(logical, Some((1920, 1080)));
        // Without a size property (the compositor leaves it unset) the node is still usable.
        let bare = first_stream_info(&empty_streams(7, true));
        assert_eq!(bare.map(|(node, _)| node), Some(7));
        assert_eq!(first_stream_info(&HashMap::new()), None);
    }

    /// Build a streams response holding only a node id and an (optional) empty property dict.
    fn empty_streams(node: u32, with_props: bool) -> HashMap<String, OwnedValue> {
        let element_signature = zbus::zvariant::Signature::try_from("(ua{sv})").expect("sig");
        let mut array = zbus::zvariant::Array::new(&element_signature);
        let props: HashMap<String, Value<'static>> = HashMap::new();
        array
            .append(Value::Structure(zbus::zvariant::Structure::from((
                node,
                if with_props { props } else { HashMap::new() },
            ))))
            .expect("append");
        let mut results: HashMap<String, OwnedValue> = HashMap::new();
        results.insert(
            "streams".to_string(),
            Value::Array(array).try_to_owned().expect("owned"),
        );
        results
    }

    /// Live verification: the portal RemoteDesktop is reachable and advertises keyboard/
    /// pointer support (read-only properties; no session created, no dialogs). CI has no user
    /// session bus, so run locally only:
    /// `cargo test --lib computer_use::platform::wayland_portal -- --ignored`
    #[test]
    #[ignore = "needs a desktop session bus with xdg-desktop-portal"]
    fn portal_probe_succeeds_on_live_session() {
        PortalInput::probe().expect("portal RemoteDesktop input should be available");
    }
}
