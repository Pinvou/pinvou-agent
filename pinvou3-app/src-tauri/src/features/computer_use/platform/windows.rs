//! Windows backend: xcap (GDI BitBlt) screen capture + enigo (SendInput) input injection
//! + uiautomation (UI Automation) accessibility tree.
//!
//! Coordinate contract: the process is made Per-Monitor-V2 DPI aware via tao (the thread DPI
//! awareness is read and checked at `new()` time, see the `capabilities` notes), so capture
//! pixels, UIA BoundingRectangle, and SendInput absolute coordinates are all **device physical
//! pixels**; hence `Capture.input_scale_x/y = 1.0`. The multi-monitor virtual desktop origin
//! can be negative and is passed through as-is.
//!
//! **Absolute mouse movement bypasses enigo**: enigo 0.6.1's `Coordinate::Abs` normalizes only
//! against the primary screen (`SM_CXSCREEN`) and does not set `MOUSEEVENTF_VIRTUALDESK`, so
//! when the cursor is on a secondary screen the coordinates get folded onto the primary screen.
//! This module calls `SendInput` directly via windows-sys, normalized against the entire
//! virtual desktop (`move_mouse_abs`); button press/release and keyboard still go through
//! enigo — its click events are relative to the current cursor position (dx=dy=0, no ABSOLUTE
//! flag), so an absolute move landing first is correct.
//!
//! Threads and COM: all objects are constructed, used, and dropped on the backend worker
//! thread (see `super::super::backend::BackendHandle`) and never move across threads. `Enigo`
//! is held inside the struct; `uiautomation::UIAutomation` does not enter the struct because
//! the windows-rs COM interfaces are `!Send` — `new()` initializes COM once on that thread as
//! MTA, and afterwards every UIA call builds and drops a client on the fly. All UIA calls stay
//! on this windowless worker thread, avoiding deadlocks with the WebView UI thread.
//!
//! Known limitations (UIPI / secure desktop / UIA timeouts):
//! - This process runs at medium IL without `uiAccess`, so for target windows **run as
//!   administrator**, SendInput injection is dropped by UIPI and UIA reads return access
//!   denied. Both failure classes are detectable and mapped to explicit `unavailable`: UIA's
//!   access denied (HRESULT 0x80070005) and enigo's reported injection shortfall
//!   (see `map_input_err`).
//! - During the secure desktop (lock screen / UAC consent dialog) and display sleep, capture
//!   is all black and injection and UIA all stop working: an all-black frame is returned as an
//!   explicit `unavailable` error, and things recover automatically after wake.
//! - UIA reads have no transaction timeout: a hung target application can block the call until
//!   the system default COM timeout. The precise semantics have two phases (round-10 review
//!   m9): while a call is in flight, subsequent requests of that session are **rejected
//!   immediately** by the in-flight flag (no queueing); only after the caller gives up at the
//!   call budget timeout do subsequent requests queue and each burn the entire call budget
//!   before erroring. The worker thread may remain stuck inside the OS call up to the system
//!   COM timeout (inherent limitation).
//! - enigo's `text()` sends both the Return/Tab keystrokes **and** the corresponding Unicode
//!   events for `\n`/`\t` (upstream implementation, see enigo win_impl); target applications
//!   that handle both message kinds may insert the newline/tab twice. type_text does not split
//!   to work around this (split-injection equivalence would need to be verified on real
//!   Windows); disclosed as-is.
//! - `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` and DRM-protected content appear as
//!   black blocks in captures; this is expected system behavior.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

use enigo::{Button, Direction, Enigo, Keyboard, Mouse, Settings};
use uiautomation::UIAutomation;
use uiautomation::types::{ControlType, Point, TreeScope, UIProperty};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, HKL, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput, VkKeyScanExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};
use xcap::Monitor;

use super::super::backend::ComputerUseBackend;
use super::super::types::{
    Capabilities, Capture, ComputerUseError, ElementInfo, Key, MouseButton, ScrollDirection,
    UiTreeOptions,
};
use super::helpers::{TYPE_CHUNK_CHARS, char_chunks, drag_waypoints, map_scroll, sanitize_name};

/// Wait before a click so the previous move has settled (the target process consumes mouse
/// move events).
const CLICK_SETTLE_MS: u64 = 30;
/// Interval between adjacent clicks of a double/triple click (far below the system 500ms
/// double-click threshold).
const MULTI_CLICK_INTERVAL_MS: u64 = 40;
/// Pause after the drag press and before movement starts, giving the target window time to
/// enter drag-recognition state.
const DRAG_PRESS_SETTLE_MS: u64 = 60;
/// Number of interpolated steps along the drag path (some apps only recognize a drag after
/// receiving enough motion events).
const DRAG_STEPS: usize = 8;
/// Interval between adjacent drag waypoints.
const DRAG_STEP_DELAY_MS: u64 = 12;
/// Pause between pressing all keys and releasing them in reverse, improving the hit rate of
/// modifier chords (alt+Tab etc.).
const CHORD_HOLD_MS: u64 = 20;

/// `ui_tree` defaults/caps: depth and node count (prevents giant trees from overwhelming the
/// worker thread and the text budget).
const DEFAULT_TREE_MAX_DEPTH: u32 = 8;
const DEFAULT_TREE_MAX_NODES: u32 = 400;
const MAX_TREE_DEPTH: u32 = 24;
const MAX_TREE_NODES: u32 = 2000;
/// Character cap for a single name line.
const MAX_NODE_NAME_CHARS: usize = 80;
/// Defensive cap for climbing from the focused element up to its owning window.
const MAX_ANCESTOR_CLIMB: u32 = 16;

/// Modifier bits of the high byte returned by `VkKeyScanExW` (Win32 docs): bit0=Shift (0x01),
/// bit1=Ctrl (0x02), bit2=Alt (0x04, AltGr = Ctrl+Alt). A nonzero high byte means producing
/// the character requires modifier keys; high bits beyond bit0-2 are undefined in Win32, and
/// the injection plan treats them all fail-closed as "needs a modifier key"
/// (see `char_injection_from_scan`); the check relies only on "high byte nonzero", so no
/// per-bit constants are needed.

/// Return value of `GetAwarenessFromDpiAwarenessContext`: Per-Monitor awareness.
/// (windows-sys `Win32_UI_HiDpi`'s `DPI_AWARENESS_PER_MONITOR_AWARE`.)
const DPI_AWARENESS_PER_MONITOR_AWARE: i32 = 2;
/// `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2` pseudo-handle (-4).
const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;

// These three user32 entry points live behind the `Win32_UI_HiDpi` feature in
// windows-sys 0.61, which Cargo.toml does not enable; declare them by hand here rather than
// adding the dependency feature (signatures verbatim-identical to the windows-sys HiDpi
// module). Minimum requirement is Windows 10 1607, above the Tauri 2 / WebView2 Win10 baseline.
#[allow(non_snake_case)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetThreadDpiAwarenessContext() -> *mut core::ffi::c_void;
    fn GetAwarenessFromDpiAwarenessContext(value: *mut core::ffi::c_void) -> i32;
    fn AreDpiAwarenessContextsEqual(a: *mut core::ffi::c_void, b: *mut core::ffi::c_void) -> i32;
}

/// Win32 `E_ACCESSDENIED` (0x80070005): the typical signal of a UIA read denied across
/// integrity levels.
const E_ACCESSDENIED: i32 = -2147024891;

/// Whether the thread DPI awareness is Per-Monitor-V2: the awareness level is
/// PER_MONITOR_AWARE(2) and the thread context really is the PMv2 pseudo-handle
/// (Per-Monitor v1 satisfies the former but not the latter). Pure function, easy to unit
/// test; `context_is_pmv2` is `AreDpiAwarenessContextsEqual(ctx, PMv2)`.
fn thread_is_pmv2(awareness: i32, context_is_pmv2: bool) -> bool {
    awareness == DPI_AWARENESS_PER_MONITOR_AWARE && context_is_pmv2
}

/// Whether the current thread's DPI awareness is PMv2 (see `thread_is_pmv2`).
fn thread_dpi_is_pmv2() -> bool {
    // SAFETY: GetThreadDpiAwarenessContext/GetAwarenessFromDpiAwarenessContext
    // are thread-info queries with no lifetime or ownership constraints; a
    // null context is explicitly checked below.
    unsafe {
        let ctx = GetThreadDpiAwarenessContext();
        if ctx.is_null() {
            return false;
        }
        let awareness = GetAwarenessFromDpiAwarenessContext(ctx);
        let is_pmv2_context = AreDpiAwarenessContextsEqual(
            ctx,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 as *mut core::ffi::c_void,
        ) != 0;
        thread_is_pmv2(awareness, is_pmv2_context)
    }
}

/// Injection plan for a single-character key (from the `VkKeyScanExW` layout query).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharInjection {
    /// Layout sends it directly: no modifier needed (plain ASCII letters/digits/punctuation
    /// that needs no Shift), via the existing enigo virtual-key path (shift-state high byte
    /// is 0, enigo's VK conversion is correct).
    Direct,
    /// Producing the character requires a modifier key (Shift/AltGr etc.). enigo sends the
    /// `VkKeyScanExW` shift-state high byte along as a VK too (SendInput succeeds but the
    /// target never receives the character), so the VK chord path must not be used.
    NeedsModifier,
    /// The layout has no such character (e.g. CJK on an English layout); enigo automatically
    /// falls back to KEYEVENTF_UNICODE events on mapping failure, so keep the current path.
    NotInLayout,
}

/// `VkKeyScanExW` return value → injection plan (pure function, easy to unit test). The low
/// byte is the VK, the high byte the modifier bits (`VKSHIFT_*`); a negative return means the
/// layout cannot produce the character. Any nonzero bit of the high byte is treated as
/// "needs a modifier key" (fail-closed): Win32 only defines bit0-2, and unknown high-bit
/// combinations ask for the modifier rather than injecting directly (third-round review
/// finding: the `& 0x07` mask treated unknown high bits as Direct, contradicting the
/// fail-closed promise made by the module docs/unit tests; runtime semantics were unchanged
/// (unknown bits never really occur), but the code must deliver the promised behavior).
fn char_injection_from_scan(scan: i16) -> CharInjection {
    if scan < 0 {
        return CharInjection::NotInLayout;
    }
    if (scan >> 8) != 0 {
        CharInjection::NeedsModifier
    } else {
        CharInjection::Direct
    }
}

/// Query the injection plan for a character under the target thread's keyboard layout. The
/// layout is taken from the foreground window's thread (consistent with enigo's conversion
/// target), falling back to the current thread's layout when no foreground window exists.
fn char_injection(c: char) -> CharInjection {
    let mut utf16 = [0u16; 2];
    if c.encode_utf16(&mut utf16).len() != 1 {
        // Characters outside the BMP (surrogate pairs) cannot map to a single VK.
        return CharInjection::NotInLayout;
    }
    // SAFETY: GetForegroundWindow/GetWindowThreadProcessId/GetKeyboardLayout
    // are read-only queries; a null hwnd falls back to the thread's layout,
    // and the null_mut pointer arg is the documented "no pid out" form.
    let layout: HKL = unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            GetKeyboardLayout(0)
        } else {
            GetKeyboardLayout(GetWindowThreadProcessId(hwnd, std::ptr::null_mut()))
        }
    };
    // SAFETY: VkKeyScanExW only reads the layout handle and the single
    // UTF-16 code unit produced above.
    char_injection_from_scan(unsafe { VkKeyScanExW(utf16[0], layout) })
}

/// Single axis of a screen/virtual-desktop coordinate → SendInput absolute normalized
/// coordinate (0..=65535). `origin`/`extent` are the origin and size of the normalization
/// domain (the virtual desktop origin can be negative). Pure function, easy to unit test.
fn normalize_abs_axis(value: i32, origin: i32, extent: i32) -> i32 {
    if extent <= 1 {
        // Degenerate (single-pixel/invalid): 0 and 65535 are equivalent, take 0.
        return 0;
    }
    let delta = i64::from(value - origin);
    let den = i64::from(extent - 1);
    // Round half up; negative values (outside the domain) are clamped right after.
    let scaled = (delta * i64::from(65535) + den / 2) / den;
    scaled.clamp(0, i64::from(65535)) as i32
}

/// Read the virtual desktop metrics: (origin x, origin y, width, height), covering all
/// monitors (the origin can be negative).
fn virtual_desktop_metrics() -> (i32, i32, i32, i32) {
    // SAFETY: GetSystemMetrics with SM_*VIRTUALSCREEN indices are pure
    // metric queries returning i32 values; no pointers involved.
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// Validate the SendInput injection result: a mismatch between injected and requested counts
/// (typically an elevated target window with UIPI blocking the injection) maps to
/// `unavailable` instead of a generic failed — the upper layer can use this to advise the user.
fn check_injection(context: &str, requested: u32, sent: u32) -> Result<(), ComputerUseError> {
    if sent == requested {
        return Ok(());
    }
    Err(ComputerUseError::unavailable(format!(
        "{context}: SendInput injected {sent}/{requested} events — the input was likely blocked \
         by UIPI because the target window is elevated (run as administrator); \
         run Pinvou elevated or use an unelevated target window"
    )))
}

fn map_xcap_err(context: &str, err: xcap::XCapError) -> ComputerUseError {
    ComputerUseError::failed(format!("{context}: {err}"))
}

fn map_input_err(context: &str, err: enigo::InputError) -> ComputerUseError {
    // enigo returns Simulate("...blocked by UIPI") when SendInput's injected count falls
    // short — the typical scenario is a target window running as administrator with UIPI
    // silently dropping the injected events. Map it to `unavailable` (detectable, with
    // guidance) instead of a generic failed.
    if let enigo::InputError::Simulate(msg) = &err {
        if msg.contains("UIPI") {
            return ComputerUseError::unavailable(format!(
                "{context}: input was blocked by UIPI — the target window is likely elevated \
                 (run as administrator); run Pinvou elevated or use an unelevated target window"
            ));
        }
    }
    ComputerUseError::failed(format!("{context}: {err}"))
}

fn map_uia_err(context: &str, err: uiautomation::Error) -> ComputerUseError {
    if err.code() == E_ACCESSDENIED {
        ComputerUseError::unavailable(format!(
            "{context}: access denied reading UI Automation data — the target window is likely \
             elevated (run as administrator); UIPI blocks cross-integrity-level access"
        ))
    } else {
        ComputerUseError::failed(format!("{context}: {err}"))
    }
}

/// The result of `map_key`. Ordinary keys yield an enigo key; characters that need a modifier
/// (like '+') require switching to the direct Unicode injection channel when they are the
/// whole chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MappedKey {
    Key(enigo::Key),
    /// The character needs a modifier to be produced and the chord contains only it: inject
    /// directly via `Keyboard::text` and KEYEVENTF_UNICODE (independent of layout and IME).
    /// Note this channel does down+up immediately, so `hold_key`'s held semantics do not apply.
    UnicodeOnly(char),
}

/// Map according to the `Key::Char` layout injection plan and the chord length (pure
/// function, easy to unit test). Characters that need a modifier (like '+'; enigo would send
/// the `VkKeyScanExW` shift-state high byte along as a VK and the target would never receive
/// the character):
/// - the chord has other keys besides the character → error explicitly;
/// - the chord is only the character → return [`MappedKey::UnicodeOnly`] and let the caller
///   take the Unicode channel.
fn map_char_with_plan(
    c: char,
    plan: CharInjection,
    chord_len: usize,
) -> Result<MappedKey, ComputerUseError> {
    match plan {
        // Direct: enigo's VK conversion is correct; NotInLayout: enigo automatically falls
        // back to Unicode events.
        CharInjection::Direct | CharInjection::NotInLayout => {
            Ok(MappedKey::Key(enigo::Key::Unicode(c)))
        }
        CharInjection::NeedsModifier => {
            if chord_len > 1 {
                // No echo of the character (this string lands in the audit
                // log's error field verbatim), and no "requires shift"
                // claim: the blocker is any modifier keyboard state
                // (Shift/AltGr) that a chord cannot express.
                Err(ComputerUseError::failed(
                    "character cannot be typed as part of a modifier chord (it needs a \
                     Shift/AltGr keyboard state chords cannot express); use the type action \
                     for text",
                ))
            } else {
                Ok(MappedKey::UnicodeOnly(c))
            }
        }
    }
}

/// This crate's `Key` → enigo key. `Char` first goes through `VkKeyScanExW` for the layout
/// injection plan (see [`map_char_with_plan`]); named keys map directly. The layout is taken
/// from the foreground window's thread, consistent with enigo's conversion target.
fn map_key(key: Key, chord_len: usize) -> Result<MappedKey, ComputerUseError> {
    use enigo::Key as EK;
    let mapped = match key {
        Key::Control => EK::Control,
        Key::Alt => EK::Alt,
        Key::Shift => EK::Shift,
        Key::Meta => EK::Meta,
        Key::Enter => EK::Return,
        Key::Escape => EK::Escape,
        Key::Tab => EK::Tab,
        Key::Space => EK::Space,
        Key::Backspace => EK::Backspace,
        Key::Delete => EK::Delete,
        Key::Insert => EK::Insert,
        Key::Up => EK::UpArrow,
        Key::Down => EK::DownArrow,
        Key::Left => EK::LeftArrow,
        Key::Right => EK::RightArrow,
        Key::Home => EK::Home,
        Key::End => EK::End,
        Key::PageUp => EK::PageUp,
        Key::PageDown => EK::PageDown,
        Key::Function(n) => match n {
            1 => EK::F1,
            2 => EK::F2,
            3 => EK::F3,
            4 => EK::F4,
            5 => EK::F5,
            6 => EK::F6,
            7 => EK::F7,
            8 => EK::F8,
            9 => EK::F9,
            10 => EK::F10,
            11 => EK::F11,
            12 => EK::F12,
            other => {
                return Err(ComputerUseError::failed(format!(
                    "function key f{other} is outside the supported f1..=f12 range"
                )));
            }
        },
        Key::Char(c) => return map_char_with_plan(c, char_injection(c), chord_len),
    };
    Ok(MappedKey::Key(mapped))
}

fn map_mouse_button(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    }
}

/// Node budget and sequence numbers for tree serialization.
struct TreeWriter {
    next_index: u32,
    remaining: u32,
    truncated: bool,
}

/// Single-line format: `[i] role "name" (x,y,w,h) flags`, with flags omitted when there are none.
fn format_tree_line(
    index: u32,
    depth: u32,
    element: &uiautomation::UIElement,
) -> Result<String, ComputerUseError> {
    let control_type = element
        .get_cached_control_type()
        .map_err(|e| map_uia_err("ui_tree control type", e))?;
    let name = sanitize_name(
        &element
            .get_cached_name()
            .map_err(|e| map_uia_err("ui_tree name", e))?,
        MAX_NODE_NAME_CHARS,
    );
    let rect = element
        .get_cached_bounding_rectangle()
        .map_err(|e| map_uia_err("ui_tree bounds", e))?;
    let mut flags = String::new();
    if !element.is_cached_enabled().unwrap_or(true) {
        flags.push_str(" disabled");
    }
    if element.has_cached_keyboard_focus().unwrap_or(false) {
        flags.push_str(" focused");
    }
    if element.is_cached_offscreen().unwrap_or(false) {
        flags.push_str(" offscreen");
    }
    if element.is_cached_password().unwrap_or(false) {
        flags.push_str(" password");
    }
    let mut line = String::new();
    let _ = write!(
        line,
        "{indent}[{index}] {control_type:?} \"{name}\" ({x},{y},{w},{h}){flags}",
        indent = "  ".repeat(depth as usize),
        x = rect.get_left(),
        y = rect.get_top(),
        w = (rect.get_right() - rect.get_left()).max(0),
        h = (rect.get_bottom() - rect.get_top()).max(0),
    );
    Ok(line)
}

/// Serialize a subtree: fetch "this node + direct children" per node via a Children-scope
/// cache request; the node budget (`writer.remaining`) bounds the number of cross-process
/// round-trips — the total fetch scale of the whole tree is thereby bounded. Elements may be
/// destroyed while queued: a single-node fetch failure is treated as "this branch ended" and
/// does not sink the whole fetch.
fn write_tree_node(
    element: &uiautomation::UIElement,
    cache: &uiautomation::core::UICacheRequest,
    depth: u32,
    max_depth: u32,
    writer: &mut TreeWriter,
    out: &mut String,
) -> Result<(), ComputerUseError> {
    if writer.remaining == 0 {
        writer.truncated = true;
        return Ok(());
    }
    // Fetch failures do not consume budget: destroyed elements must not crowd out slots for
    // visible nodes.
    let Ok(cached) = element.build_updated_cache(cache) else {
        return Ok(());
    };
    writer.remaining -= 1;
    let index = writer.next_index;
    writer.next_index += 1;
    let line = format_tree_line(index, depth, &cached)?;
    let _ = writeln!(out, "{line}");
    if depth >= max_depth {
        return Ok(());
    }
    // Children were already fetched in the same cache request; enumeration failure is likewise
    // treated as "this branch ended".
    let Ok(children) = cached.get_cached_children() else {
        return Ok(());
    };
    let mut children = children.iter();
    while let Some(child) = children.next() {
        write_tree_node(child, cache, depth + 1, max_depth, writer, out)?;
        // Budget spent is real truncation only when a further child actually
        // exists and is skipped: hitting zero on the last child means the
        // tree was serialized completely (flagging on `remaining == 0` alone
        // used to emit a false "(truncated)" footer; the entry check above is
        // the authoritative signal for a genuinely skipped node).
        if writer.remaining == 0 && children.next().is_some() {
            writer.truncated = true;
            break;
        }
    }
    Ok(())
}

/// Merge the interpolated-move and button-release results of `drag`. The
/// release always runs, but `Result::and` kept only the first error: when the
/// release failed too, callers never learned that the mouse button may still
/// be pressed (the macOS/Linux backends already surface this signal).
fn combine_drag_errors(
    move_result: Result<(), ComputerUseError>,
    release_result: Result<(), ComputerUseError>,
) -> Result<(), ComputerUseError> {
    match (move_result, release_result) {
        (Ok(()), Ok(())) => Ok(()),
        // Only the path move failed and the release succeeded: report the
        // move error unchanged.
        (Err(move_error), Ok(())) => Err(move_error),
        // Single-failure cases keep the ORIGINAL error kind (same_kind,
        // round-12 review): a UIPI-blocked release is `unavailable` — the
        // "run elevated" classification upstreams rely on must survive the
        // stranded-button annotation.
        (Ok(()), Err(release)) => {
            Err(release.same_kind(format!("{release}; the mouse button may still be pressed")))
        }
        (Err(move_error), Err(release)) => Err(move_error.same_kind(format!(
            "drag move failed ({move_error}); its release also failed ({release}); \
             the mouse button may still be pressed"
        ))),
    }
}

pub(super) struct WindowsComputerUseBackend {
    enigo: Enigo,
    /// The thread DPI awareness conclusion read once at `new()`: when not Per-Monitor-V2,
    /// `capabilities().notes` declares that the coordinate assumption is doubtful (no hard failure).
    dpi_pmv2: bool,
    /// Cancel flag set after the caller times out (round-12 review M5): the type chunked
    /// injection checks it between chunks, so abandoned requests stop injecting.
    cancel: Option<Arc<AtomicBool>>,
    // Note: does not hold uiautomation::UIAutomation — the windows-rs COM interface types are
    // !Send (IUIAutomation contains NonNull), while ComputerUseBackend requires Send.
    // Instead, new() initializes COM MTA once on the worker thread, and every UIA call builds
    // and drops a client via uia_client() on the fly (CoCreateInstance overhead is negligible
    // for once-per-turn calls).
}

impl WindowsComputerUseBackend {
    fn new() -> Result<Self, ComputerUseError> {
        let enigo = Enigo::new(&Settings::default()).map_err(|err| {
            ComputerUseError::unavailable(format!("cannot initialize SendInput backend: {err}"))
        })?;
        // Initialize COM as MTA on the worker thread (UIAutomation::new includes
        // CoInitializeEx), then drop the client; the apartment lives until thread end
        // (the crate does not CoUninitialize).
        let _com_mta_init = UIAutomation::new().map_err(|err| {
            ComputerUseError::unavailable(format!(
                "cannot initialize UI Automation (COM MTA): {err}"
            ))
        })?;
        // The coordinate contract presumes the process is Per-Monitor-V2 DPI aware (set by
        // tao). Read the thread awareness once to verify; when not PMv2, only declare it in
        // the capabilities notes (no hard failure).
        let dpi_pmv2 = thread_dpi_is_pmv2();
        Ok(Self {
            enigo,
            dpi_pmv2,
            cancel: None,
        })
    }

    /// Build a UIA client on the fly. Requires COM to be initialized on the current thread
    /// (guaranteed by new()).
    fn uia_client(&self) -> Result<UIAutomation, ComputerUseError> {
        UIAutomation::new_direct().map_err(|err| {
            ComputerUseError::unavailable(format!("cannot create UI Automation client: {err}"))
        })
    }

    /// Pick the monitor to capture: the screen under the cursor, falling back to the primary
    /// screen, and finally degrading to the first screen.
    fn pick_monitor(cursor: Option<(i32, i32)>) -> Result<Monitor, ComputerUseError> {
        if let Some((x, y)) = cursor {
            if let Ok(monitor) = Monitor::from_point(x, y) {
                return Ok(monitor);
            }
        }
        let mut monitors =
            Monitor::all().map_err(|err| map_xcap_err("cannot enumerate monitors", err))?;
        if let Some(primary) = monitors
            .iter()
            .position(|m| m.is_primary().unwrap_or(false))
        {
            return Ok(monitors.swap_remove(primary));
        }
        monitors
            .drain(..)
            .next()
            .ok_or_else(|| ComputerUseError::unavailable("no monitor available for screen capture"))
    }

    /// Property cache request: merges many per-node cross-process COM round-trips into a
    /// single batched read. `scope` gives the tree range of the cached fetch (`ui_tree`'s
    /// per-node descent uses `Children`; single-element reads use `Element`); scopes beyond
    /// `Element` apply the control-view filter, consistent with the UIA standard view.
    fn property_cache(
        uia: &UIAutomation,
        scope: TreeScope,
    ) -> Result<uiautomation::core::UICacheRequest, ComputerUseError> {
        let cache = uia
            .create_cache_request()
            .map_err(|e| map_uia_err("ui_tree cache request", e))?;
        for property in [
            UIProperty::Name,
            UIProperty::ControlType,
            UIProperty::BoundingRectangle,
            UIProperty::IsEnabled,
            UIProperty::HasKeyboardFocus,
            UIProperty::IsOffscreen,
            UIProperty::IsPassword,
        ] {
            cache
                .add_property(property)
                .map_err(|e| map_uia_err("ui_tree cache request", e))?;
        }
        cache
            .set_tree_scope(scope)
            .map_err(|e| map_uia_err("ui_tree cache scope", e))?;
        if scope != TreeScope::Element {
            let control_view = uia
                .get_control_view_condition()
                .map_err(|e| map_uia_err("ui_tree cache filter", e))?;
            cache
                .set_tree_filter(control_view)
                .map_err(|e| map_uia_err("ui_tree cache filter", e))?;
        }
        Ok(cache)
    }

    /// The tree's root: the top-level window owning the focused element. When the focus
    /// cannot be obtained, fail explicitly (fail-closed), **never fall back to the desktop
    /// root** — the full desktop tree can reach tens of thousands of nodes, and a fallback
    /// would render the per-level budgets meaningless.
    fn focused_window_root(
        uia: &UIAutomation,
    ) -> Result<uiautomation::UIElement, ComputerUseError> {
        let focused = uia
            .get_focused_element()
            .map_err(|e| map_uia_err("ui_tree (no focused element)", e))?;
        let walker = match uia.get_raw_view_walker() {
            Ok(walker) => walker,
            Err(_) => return Ok(focused),
        };
        let mut current = focused;
        for _ in 0..MAX_ANCESTOR_CLIMB {
            if current.get_control_type() == Ok(ControlType::Window) {
                return Ok(current);
            }
            match walker.get_parent(&current) {
                Ok(parent) => {
                    // Defend against a self-looping parent chain (the desktop root's parent
                    // is itself or the call fails outright).
                    if uia.compare_elements(&parent, &current).unwrap_or(false) {
                        return Ok(current);
                    }
                    current = parent;
                }
                Err(_) => return Ok(current),
            }
        }
        Ok(current)
    }

    /// Release the pressed keys in reverse; even on a mid-way error, best-effort release
    /// everything and return the first error.
    fn release_reverse(pressed: &[enigo::Key], enigo: &mut Enigo) -> Result<(), enigo::InputError> {
        let mut first_err = None;
        for key in pressed.iter().rev() {
            if let Err(err) = enigo.key(*key, Direction::Release) {
                if first_err.is_none() {
                    first_err = Some(err);
                }
            }
        }
        match first_err {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Press all keys (rolling back those already pressed on error), pause, then release in reverse.
    fn chord(&mut self, keys: &[Key], hold: Duration) -> Result<(), ComputerUseError> {
        let mut pressed: Vec<enigo::Key> = Vec::with_capacity(keys.len());
        for key in keys {
            let mapped = match map_key(*key, keys.len()) {
                Ok(MappedKey::Key(mapped)) => mapped,
                // Only possible when the chord is this single character: switch to the direct
                // Unicode injection (same channel as type_text; KEYEVENTF_UNICODE does not
                // depend on the layout).
                Ok(MappedKey::UnicodeOnly(c)) => {
                    return self
                        .enigo
                        .text(&c.to_string())
                        .map_err(|err| map_input_err("key chord", err));
                }
                Err(err) => {
                    // Chord resolution failed midway: release the pressed modifiers first to
                    // avoid stuck keys.
                    let _ = Self::release_reverse(&pressed, &mut self.enigo);
                    return Err(err);
                }
            };
            if let Err(err) = self.enigo.key(mapped, Direction::Press) {
                let _ = Self::release_reverse(&pressed, &mut self.enigo);
                return Err(map_input_err("key press", err));
            }
            pressed.push(mapped);
        }
        sleep(hold);
        Self::release_reverse(&pressed, &mut self.enigo)
            .map_err(|err| map_input_err("key release", err))
    }

    /// Multi-monitor-correct absolute movement: calls `SendInput` directly via windows-sys,
    /// normalized against the entire virtual desktop (`MOUSEEVENTF_VIRTUALDESK`, whose origin
    /// can be negative). enigo 0.6.1's `Coordinate::Abs` normalizes only against the primary
    /// screen (`SM_CXSCREEN`), so a click while the cursor is on a secondary screen would land
    /// on the primary screen; hence absolute movement must be self-implemented. Button events
    /// are relative to the current cursor position, so landing the move first suffices.
    fn move_mouse_abs(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        let (vx, vy, vw, vh) = virtual_desktop_metrics();
        // Construct the union via field literals (safe) rather than zeroing and patching.
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: normalize_abs_axis(x, vx, vw),
                    dy: normalize_abs_axis(y, vy, vh),
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        // SAFETY: SendInput copies the single fully-initialized INPUT by
        // value from a valid reference; the size matches the pointed-to type.
        let sent = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
        check_injection("mouse move", 1, sent)
    }
}

impl ComputerUseBackend for WindowsComputerUseBackend {
    fn capabilities(&self) -> Capabilities {
        let mut notes = "windows: physical-pixel coordinates on the full virtual desktop \
                         (multi-monitor safe); elevated (run as administrator) target windows \
                         are blocked by UIPI — detection is best-effort (injection shortfalls \
                         and UIA access-denied are reported as unavailable), actions should be \
                         verified with a follow-up screenshot; display sleep / secure desktop \
                         (lock screen / UAC) surfaces as black-frame capture errors; UIA reads \
                         have no transaction timeout — a hung target app can stall the call up \
                         to the system default COM timeout (session-level call timeout still \
                         applies)"
            .to_string();
        if !self.dpi_pmv2 {
            notes.push_str(
                "; DPI: thread is not per-monitor-v2 aware — the physical-pixel coordinate \
                 assumption may not hold and coordinates may be virtualized",
            );
        }
        Capabilities {
            screenshot: true,
            input: true,
            ui_tree: true,
            notes,
        }
    }

    fn capture(&mut self) -> Result<Capture, ComputerUseError> {
        let cursor = self.cursor_position().ok();
        let monitor = Self::pick_monitor(cursor)?;
        let origin_x = monitor
            .x()
            .map_err(|err| map_xcap_err("monitor origin", err))?;
        let origin_y = monitor
            .y()
            .map_err(|err| map_xcap_err("monitor origin", err))?;
        let image = monitor
            .capture_image()
            .map_err(|err| map_xcap_err("screen capture", err))?;
        let width = image.width();
        let height = image.height();
        let mut rgba = image.into_raw();
        // xcap's GDI path does not fix up alpha on Win8+ (it can be 0 throughout, making the
        // downstream PNG fully transparent); the single pass also sets alpha opaque and
        // detects all-black frames (secure desktop/protected content).
        let mut all_black = true;
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
            if all_black && (pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0) {
                all_black = false;
            }
        }
        if all_black {
            return Err(ComputerUseError::unavailable(
                "captured frame is entirely black — likely the secure desktop (lock screen or \
                 UAC prompt) or capture-protected content; capture will resume on the normal desktop",
            ));
        }
        Ok(Capture {
            rgba,
            width,
            height,
            origin_x,
            origin_y,
            input_scale_x: 1.0,
            input_scale_y: 1.0,
        })
    }

    fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
        self.enigo
            .location()
            .map_err(|err| map_input_err("cursor position", err))
    }

    fn move_to(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        // Absolute movement takes the own SendInput path (full virtual-desktop normalization,
        // multi-monitor safe).
        self.move_mouse_abs(x, y)
    }

    fn click(&mut self, button: MouseButton, count: u8) -> Result<(), ComputerUseError> {
        let button = map_mouse_button(button);
        // Let the previous move settle before clicking, avoiding a click at the stale cursor position.
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        for i in 0..count.max(1) {
            self.enigo
                .button(button, Direction::Click)
                .map_err(|err| map_input_err("mouse click", err))?;
            if i + 1 < count {
                sleep(Duration::from_millis(MULTI_CLICK_INTERVAL_MS));
            }
        }
        Ok(())
    }

    fn mouse_down(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        self.enigo
            .button(map_mouse_button(button), Direction::Press)
            .map_err(|err| map_input_err("mouse down", err))
    }

    fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        self.enigo
            .button(map_mouse_button(button), Direction::Release)
            .map_err(|err| map_input_err("mouse up", err))
    }

    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError> {
        self.move_mouse_abs(from.0, from.1)?;
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        self.enigo
            .button(Button::Left, Direction::Press)
            .map_err(|err| map_input_err("drag press", err))?;
        sleep(Duration::from_millis(DRAG_PRESS_SETTLE_MS));
        let result = (|| {
            for (x, y) in drag_waypoints(from, to, DRAG_STEPS) {
                self.move_mouse_abs(x, y)?;
                sleep(Duration::from_millis(DRAG_STEP_DELAY_MS));
            }
            Ok(())
        })();
        // The button must be released regardless of whether the path movement errored, so the
        // mouse does not stay stuck pressed.
        let release = self
            .enigo
            .button(Button::Left, Direction::Release)
            .map_err(|err| map_input_err("drag release", err));
        combine_drag_errors(result, release)
    }

    fn scroll(&mut self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError> {
        let (axis, length) = map_scroll(direction, clicks);
        self.enigo
            .scroll(length, axis)
            .map_err(|err| map_input_err("scroll", err))
    }

    fn type_text(&mut self, text: &str) -> Result<(), ComputerUseError> {
        // Direct Unicode injection (KEYEVENTF_UNICODE), bypassing the IME — Chinese lands
        // straight in the focused field. Chunked injection (round-12 review M5): enigo's
        // text() builds the whole text into one SendInput, but low-level keyboard hooks
        // (AV/anti-keylogger products) process each event synchronously, and long text can
        // legitimately exceed the call budget in such environments — chunking plus
        // between-chunk cancellation checks shrink the upper bound of zombie injection after
        // a caller timeout from the whole text to one chunk. Per-character semantics inside a
        // chunk are equivalent to the whole-text call ('\n'/'\t' queue behavior does not
        // change with chunking).
        for chunk in char_chunks(text, TYPE_CHUNK_CHARS) {
            if let Some(flag) = &self.cancel {
                if flag.load(Ordering::SeqCst) {
                    return Err(ComputerUseError::unavailable(
                        "type text was cancelled after the caller timed out; characters \
                         already injected are not undone",
                    ));
                }
            }
            self.enigo
                .text(chunk)
                .map_err(|err| map_input_err("type text", err))?;
        }
        Ok(())
    }

    fn set_cancel_flag(&mut self, flag: Option<Arc<AtomicBool>>) {
        self.cancel = flag;
    }

    fn key_chord(&mut self, keys: &[Key]) -> Result<(), ComputerUseError> {
        self.chord(keys, Duration::from_millis(CHORD_HOLD_MS))
    }

    fn hold_key(&mut self, keys: &[Key], ms: u64) -> Result<(), ComputerUseError> {
        self.chord(keys, Duration::from_millis(ms))
    }

    fn ui_tree(&mut self, opts: &UiTreeOptions) -> Result<String, ComputerUseError> {
        let max_depth = opts
            .max_depth
            .unwrap_or(DEFAULT_TREE_MAX_DEPTH)
            .min(MAX_TREE_DEPTH);
        let max_nodes = opts
            .max_nodes
            .unwrap_or(DEFAULT_TREE_MAX_NODES)
            .min(MAX_TREE_NODES);
        let uia = self.uia_client()?;
        let root = Self::focused_window_root(&uia)?;
        // Per-node descent: each node fetches "itself + direct children" via a Children-scope
        // cache request; the number of cross-process round-trips is bounded by the max_nodes
        // budget. Previously one Subtree fetch grabbed the whole tree — the fetch scale was
        // unbounded by the budget (a desktop-level fallback can be tens of thousands of
        // nodes) and MAX_TREE_NODES only governed serialization. The serialization-layer
        // MAX_TREE_NODES/MAX_TREE_DEPTH semantics stay unchanged.
        let cache = Self::property_cache(&uia, TreeScope::Children)?;
        let mut out = String::new();
        let mut writer = TreeWriter {
            next_index: 0,
            remaining: max_nodes,
            truncated: false,
        };
        write_tree_node(&root, &cache, 0, max_depth, &mut writer, &mut out)?;
        if writer.truncated {
            let _ = writeln!(
                out,
                "... (truncated at {} nodes; pass a larger max_nodes to see more)",
                writer.next_index
            );
        }
        Ok(out)
    }

    fn focused_element(&mut self) -> Result<Option<ElementInfo>, ComputerUseError> {
        let uia = self.uia_client()?;
        let cache = Self::property_cache(&uia, TreeScope::Element)?;
        let element = uia
            .get_focused_element_build_cache(&cache)
            .map_err(|e| map_uia_err("focused_element", e))?;
        // fail-closed: any property read failure errors out, never producing a partial ElementInfo.
        let control_type = element
            .get_cached_control_type()
            .map_err(|e| map_uia_err("focused_element control type", e))?;
        let name = element
            .get_cached_name()
            .map_err(|e| map_uia_err("focused_element name", e))?;
        let rect = element
            .get_cached_bounding_rectangle()
            .map_err(|e| map_uia_err("focused_element bounds", e))?;
        // IsPassword is the authoritative signal for a password field; a read failure is
        // also fail-closed.
        let secure = element
            .is_cached_password()
            .map_err(|e| map_uia_err("focused_element password", e))?;
        Ok(Some(ElementInfo {
            role: format!("{control_type:?}"),
            name: sanitize_name(&name, MAX_NODE_NAME_CHARS),
            x: rect.get_left(),
            y: rect.get_top(),
            width: (rect.get_right() - rect.get_left()).max(0),
            height: (rect.get_bottom() - rect.get_top()).max(0),
            secure,
        }))
    }

    fn element_at_point(
        &mut self,
        x: i32,
        y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError> {
        let uia = self.uia_client()?;
        let cache = Self::property_cache(&uia, TreeScope::Element)?;
        let element = uia
            .element_from_point_build_cache(Point::new(x, y), &cache)
            .map_err(|e| map_uia_err("element_at_point", e))?;
        let control_type = element
            .get_cached_control_type()
            .map_err(|e| map_uia_err("element_at_point control type", e))?;
        let name = element
            .get_cached_name()
            .map_err(|e| map_uia_err("element_at_point name", e))?;
        let rect = element
            .get_cached_bounding_rectangle()
            .map_err(|e| map_uia_err("element_at_point bounds", e))?;
        // IsPassword is the authoritative signal for a password field (the password variant
        // of an Edit control must have it set; browser-drawn password fields expose it too).
        // A read failure must error out (fail-closed) — unwrap_or(false) would silently pass
        // password fields off as ordinary elements, defeating the T3 screening.
        let secure = element
            .is_cached_password()
            .map_err(|e| map_uia_err("element_at_point password", e))?;
        Ok(Some(ElementInfo {
            role: format!("{control_type:?}"),
            name: sanitize_name(&name, MAX_NODE_NAME_CHARS),
            x: rect.get_left(),
            y: rect.get_top(),
            width: (rect.get_right() - rect.get_left()).max(0),
            height: (rect.get_bottom() - rect.get_top()).max(0),
            secure,
        }))
    }
}

pub(super) fn create_backend() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    Ok(Box::new(WindowsComputerUseBackend::new()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unwrap [`MappedKey::Key`] (test helper).
    fn unwrap_mapped(plan: Result<MappedKey, ComputerUseError>) -> Option<enigo::Key> {
        match plan {
            Ok(MappedKey::Key(key)) => Some(key),
            Ok(MappedKey::UnicodeOnly(c)) => panic!("unexpected UnicodeOnly({c})"),
            Err(_) => None,
        }
    }

    /// A move-failure sample for [`combine_drag_errors`] (test helper).
    fn failed_move() -> ComputerUseError {
        ComputerUseError::failed("move aborted")
    }

    /// A release-failure sample for [`combine_drag_errors`] (with map_input_err-style context).
    fn failed_release() -> ComputerUseError {
        ComputerUseError::failed("drag release: injected 0/1 events")
    }

    #[test]
    fn key_mapping_covers_named_keys_and_functions() {
        use enigo::Key as EK;
        let cases: &[(Key, EK)] = &[
            (Key::Control, EK::Control),
            (Key::Alt, EK::Alt),
            (Key::Shift, EK::Shift),
            (Key::Meta, EK::Meta),
            (Key::Enter, EK::Return),
            (Key::Escape, EK::Escape),
            (Key::Tab, EK::Tab),
            (Key::Space, EK::Space),
            (Key::Backspace, EK::Backspace),
            (Key::Delete, EK::Delete),
            (Key::Insert, EK::Insert),
            (Key::Up, EK::UpArrow),
            (Key::Down, EK::DownArrow),
            (Key::Left, EK::LeftArrow),
            (Key::Right, EK::RightArrow),
            (Key::Home, EK::Home),
            (Key::End, EK::End),
            (Key::PageUp, EK::PageUp),
            (Key::PageDown, EK::PageDown),
            (Key::Function(1), EK::F1),
            (Key::Function(6), EK::F6),
            (Key::Function(12), EK::F12),
        ];
        for (input, expected) in cases {
            assert_eq!(
                unwrap_mapped(map_key(*input, 1)),
                Some(*expected),
                "mapping {input:?}"
            );
        }
        // Character keys: regardless of whether the layout can map them (Direct or
        // NotInLayout → enigo falls back to Unicode), they take enigo's virtual-key path;
        // only characters needing a modifier return UnicodeOnly.
        assert_eq!(
            unwrap_mapped(map_key(Key::Char('s'), 1)),
            Some(EK::Unicode('s'))
        );
        assert_eq!(
            unwrap_mapped(map_key(Key::Char('中'), 1)),
            Some(EK::Unicode('中'))
        );
        assert!(map_key(Key::Function(0), 1).is_err());
        assert!(map_key(Key::Function(13), 1).is_err());
    }

    #[test]
    fn char_injection_plan_follows_vkkeyscan_shift_bits() {
        // Layout mapping failure (returns -1): e.g. CJK on an English layout → enigo
        // automatically falls back to Unicode.
        assert_eq!(char_injection_from_scan(-1), CharInjection::NotInLayout);
        assert_eq!(
            char_injection_from_scan(i16::MIN),
            CharInjection::NotInLayout
        );
        // No modifiers: 'a' (VK 0x41, high byte 0).
        assert_eq!(char_injection_from_scan(0x0041), CharInjection::Direct);
        // VK only, OEM key with a zero high byte.
        assert_eq!(char_injection_from_scan(0x001B), CharInjection::Direct);
        // Shift bit (bit0): the US keyboard's '+' = Shift + VK_OEM_PLUS(0xBB) → 0x01BB.
        // This is the defect scenario where enigo treats the high byte as a VK; must reroute.
        assert_eq!(
            char_injection_from_scan(0x01BB),
            CharInjection::NeedsModifier
        );
        // Ctrl bit (bit1) and Alt bit (bit2, AltGr): also need rerouting.
        assert_eq!(
            char_injection_from_scan(0x0241),
            CharInjection::NeedsModifier
        );
        assert_eq!(
            char_injection_from_scan(0x0451),
            CharInjection::NeedsModifier
        );
        // Shift+Ctrl+Alt combined bits.
        assert_eq!(
            char_injection_from_scan(0x0741),
            CharInjection::NeedsModifier
        );
        // Unknown other high-bit combinations are also treated as needing a modifier
        // (fail-closed).
        assert_eq!(
            char_injection_from_scan(0x0841),
            CharInjection::NeedsModifier
        );
    }

    #[test]
    fn shift_char_maps_to_unicode_only_alone_and_errors_in_chords() {
        use enigo::Key as EK;
        // A modifier-needing character (like the US keyboard's '+' = Shift+VK_OEM_PLUS) as a
        // whole chord on its own: reroute to direct Unicode injection, bypassing enigo's
        // defective VK conversion.
        assert_eq!(
            map_char_with_plan('+', CharInjection::NeedsModifier, 1).ok(),
            Some(MappedKey::UnicodeOnly('+'))
        );
        // Combined with modifier keys: error explicitly (enigo would treat the shift-state
        // high byte as a VK and the target would not receive it).
        let err = map_char_with_plan('+', CharInjection::NeedsModifier, 2).unwrap_err();
        assert!(err.to_string().contains("modifier chord"));
        assert!(err.to_string().contains("use the type action"));
        // The model-provided character itself must never be echoed into the audit log's
        // error field.
        assert!(!err.to_string().contains('+'), "{}", err);
        // Direct / NotInLayout keep the enigo path, regardless of chord length.
        assert_eq!(
            map_char_with_plan('s', CharInjection::Direct, 2).ok(),
            Some(MappedKey::Key(EK::Unicode('s')))
        );
        assert_eq!(
            map_char_with_plan('中', CharInjection::NotInLayout, 1).ok(),
            Some(MappedKey::Key(EK::Unicode('中')))
        );
    }

    #[test]
    fn thread_pmv2_requires_both_awareness_and_context() {
        // Per-Monitor v1: awareness == 2 but the context is not the PMv2 pseudo-handle.
        assert!(!thread_is_pmv2(2, false));
        // Non-PM awareness: unaware(0) / system(1) / invalid(-1) / gdiscaled(3).
        assert!(!thread_is_pmv2(0, false));
        assert!(!thread_is_pmv2(1, false));
        assert!(!thread_is_pmv2(-1, false));
        assert!(!thread_is_pmv2(3, false));
        // Only awareness == 2 with a context that really is PMv2 passes.
        assert!(thread_is_pmv2(2, true));
        assert!(!thread_is_pmv2(1, true));
    }

    #[test]
    fn drag_error_merging_surfaces_stranded_button() {
        // Both succeed: nothing to merge.
        assert!(combine_drag_errors(Ok(()), Ok(())).is_ok());
        // Only the move failed: the move error passes through unchanged.
        assert_eq!(
            combine_drag_errors(Err(failed_move()), Ok(()))
                .unwrap_err()
                .to_string(),
            "failed: move aborted"
        );
        // Only the release failed: the stranded-button warning must surface.
        let only_release = combine_drag_errors(Ok(()), Err(failed_release()))
            .unwrap_err()
            .to_string();
        assert!(
            only_release.contains("the mouse button may still be pressed"),
            "{only_release}"
        );
        // Both fail: the move failure AND the stranded-button warning must
        // both be present (`result.and(release)` used to drop the latter).
        let both = combine_drag_errors(Err(failed_move()), Err(failed_release()))
            .unwrap_err()
            .to_string();
        assert!(
            both.contains("drag move failed (failed: move aborted)"),
            "{both}"
        );
        assert!(both.contains("its release also failed"), "{both}");
        assert!(
            both.contains("the mouse button may still be pressed"),
            "{both}"
        );
    }

    #[test]
    fn abs_axis_normalization_maps_full_virtual_desktop() {
        // Primary screen (0,0) 1920x1080: both endpoints and the exact midpoint (round half up).
        assert_eq!(normalize_abs_axis(0, 0, 1920), 0);
        assert_eq!(normalize_abs_axis(1919, 0, 1920), 65535);
        assert_eq!(normalize_abs_axis(960, 0, 1921), 32768); // 32767.5 → 32768
        // Out-of-domain clamping: beyond the right edge / below the left edge both clamp to
        // the boundary.
        assert_eq!(normalize_abs_axis(1920, 0, 1920), 65535);
        assert_eq!(normalize_abs_axis(-1, 0, 1920), 0);
        assert_eq!(normalize_abs_axis(5000, 0, 1920), 65535);
        // Negative origin (left-hand secondary screen -1920..=-1): both ends map correctly.
        assert_eq!(normalize_abs_axis(-1920, -1920, 1920), 0);
        assert_eq!(normalize_abs_axis(-1, -1920, 1920), 65535);
        // Coordinates inside the secondary screen are not folded onto the primary screen
        // (the 0..65535 domain covers only that secondary screen).
        let mid = normalize_abs_axis(-960, -1920, 1920);
        assert!((1..65534).contains(&mid), "midpoint {mid} out of range");
        assert_eq!(normalize_abs_axis(0, -1920, 1920), 65535); // a point past the right edge clamps
        // Top negative origin (secondary screen above the primary).
        assert_eq!(normalize_abs_axis(-1080, -1080, 1080), 0);
        assert_eq!(normalize_abs_axis(-1, -1080, 1080), 65535);
        // Degenerate size (single-pixel/invalid).
        assert_eq!(normalize_abs_axis(100, 0, 1), 0);
        assert_eq!(normalize_abs_axis(100, 0, 0), 0);
        assert_eq!(normalize_abs_axis(100, 0, -5), 0);
    }
}
