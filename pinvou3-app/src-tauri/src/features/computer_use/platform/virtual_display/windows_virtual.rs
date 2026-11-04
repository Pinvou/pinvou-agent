//! Windows virtual display stack: an isolated **hidden Win32 desktop**
//! (`WinSta0\PinvouAgentDesk-…`) wrapped in a kill-on-close job object — the
//! rustyvnc HVNC recipe (desktop create → `lpDesktop` explorer shell →
//! `Shell_TrayWnd` readiness → PrintWindow composite capture → PostMessageW
//! input), tightened for a long-lived assistant session.
//!
//! **Why not the physical backend's enigo/SendInput lane:** SendInput injects
//! at the kernel input layer and is REJECTED for desktops that are not the
//! currently active one — every event would silently no-op (or land on the
//! user's real desktop when it happens to be active). This module therefore
//! never borrows the physical backend's input path: all input is delivered
//! as window messages (`PostMessageW`) directly to the windows living on the
//! hidden desktop, and the cursor is the hidden desktop's own per-desktop
//! cursor (`SetCursorPos`/`GetCursorPos` from an attached thread).
//!
//! **Why capture is a per-window composite:** a hidden desktop has no DWM
//! composition, so a plain `BitBlt` from the screen DC produces an all-black
//! frame. Each capture walks the desktop's visible top-level windows,
//! forces a synchronous repaint (`RedrawWindow`), pulls each window's
//! pixels with `PrintWindow` (`PW_RENDERFULLCONTENT`, falling back to flag 0),
//! composites them bottom-to-top into one bitmap, and reads the result with
//! `GetDIBits` (bottom-up BGRX → flipped RGBA). The composite covers the
//! session's whole virtual screen (`SM_X/YVIRTUALSCREEN`,
//! `SM_CX/CYVIRTUALSCREEN` — re-read every capture, since monitors can join
//! or leave mid-session), and window rects are offset by the virtual origin
//! when blitted, so capture and input share one coordinate space: screen
//! coordinates (`Capture.origin_x/origin_y` carries the virtual origin).
//!
//! Thread discipline (`SetThreadDesktop`): only threads attached to the
//! hidden desktop can see its windows, GDI objects, and input state. The
//! backend worker thread attaches at creation (it owns no windows — the
//! BackendHandle contract guarantees a windowless worker) and restores its
//! original desktop at drop; capture runs on a dedicated thread that
//! attaches FIRST, before touching any USER/GDI object, and owns the pooled
//! GDI objects for its whole life. Both attachments verify via
//! `GetThreadDesktop` comparison.
//!
//! Teardown (drop): `TerminateJobObject` (kills the shell and every app
//! launched into the session) → bounded wait for the job to empty (polled
//! via the job's `ActiveProcesses` accounting counter — a job handle never
//! signals on emptying, only on time-limit expiry) → capture thread exit →
//! restore the worker's desktop → `CloseDesktop`. A `CloseDesktop` that
//! fails with `ERROR_BUSY` is tolerated and logged: the named desktop is
//! recovered at the next start (name-based recovery, no pidfile —
//! `OpenDesktopW` probe at creation).
//!
//! Known fidelity limits (disclosed in `Capabilities`): message-level input
//! is app-level injection — apps that ignore synthetic messages cannot be
//! driven; title-bar/system-button (non-client) interactions are not
//! injected; UAC elevation prompts appear on the secure desktop, which no
//! hidden-desktop API can drive; cross-desktop UI Automation needs an
//! attached helper process and stays explicitly Unsupported; and there is
//! no mid-session wind-down: a Stop/revoke only stops driving — the hidden
//! desktop and its apps remain until the backend is dropped at session end
//! (the Linux backend winds down on stop; Windows does not, this release).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::{JoinHandle, sleep};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_BUSY, GENERIC_ALL, HANDLE, HWND, LPARAM, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLACKNESS, BitBlt, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HBITMAP, HDC,
    HGDIOBJ, PatBlt, RDW_ALLCHILDREN, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow, ReleaseDC,
    SRCCOPY, ScreenToClient, SelectObject,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, CreateDesktopW, DESKTOP_CONTROL_FLAGS, DESKTOP_CREATEMENU, DESKTOP_CREATEWINDOW,
    DESKTOP_ENUMERATE, DESKTOP_READOBJECTS, DESKTOP_WRITEOBJECTS, EnumDesktopWindows,
    GetThreadDesktop, HDESK, OpenDesktopW, SetThreadDesktop,
};
use windows::Win32::System::Threading::{
    AttachThreadInput, CREATE_SUSPENDED, CreateProcessW, GR_GDIOBJECTS, GR_USEROBJECTS,
    GetCurrentProcess, GetCurrentThreadId, GetGuiResources, OpenProcess, PROCESS_INFORMATION,
    PROCESS_TERMINATE, ResumeThread, STARTUPINFOW, TerminateProcess,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, SetFocus, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME,
    VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4,
    VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR,
    VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, ChildWindowFromPoint, FindWindowW, GA_ROOT, GCL_STYLE, GUITHREADINFO, GW_HWNDNEXT,
    GetAncestor, GetClassLongW, GetClassNameW, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo,
    GetSystemMetrics, GetTopWindow, GetWindow, GetWindowRect, GetWindowThreadProcessId,
    IsWindowVisible, PW_RENDERFULLCONTENT, PostMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SetCursorPos, SetForegroundWindow, WM_CHAR, WM_KEYDOWN,
    WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WindowFromPoint,
};
use windows::core::{BOOL, HRESULT, PCWSTR, PWSTR};

use super::super::super::backend::ComputerUseBackend;
use super::super::super::types::{
    Capabilities, Capture, ComputerUseError, DisplayTarget, ElementInfo, Key, MouseButton,
    ScrollDirection, UiTreeOptions,
};
use super::super::helpers::{
    CHORD_HOLD_MS, CLICK_SETTLE_MS, DRAG_PRESS_SETTLE_MS, DRAG_STEP_DELAY_MS, DRAG_STEPS,
    HOLD_CANCEL_POLL_MS, MAX_SCROLL_CLICKS, MULTI_CLICK_INTERVAL_MS, TYPE_CHUNK_CHARS, TypeRun,
    combine_drag_errors, combine_hold_errors, drag_waypoints, normalize_typed_newlines,
    split_type_runs,
};
use super::VirtualAvailability;

// `PrintWindow` is exported by user32 but the `windows` 0.62 crate houses it
// behind the `Win32_Storage_Xps` feature, which the app does not enable (the
// feature exists for the XPS document stack, not for this one function).
// Declared by hand rather than adding a feature — same precedent as the
// HiDpi entry points the physical backend declares in `platform/windows.rs`.
// The signature matches the generated `Win32::Storage::Xps::PrintWindow`
// exactly (HWND/HDC are `repr(transparent)` pointer newtypes, ABI-identical).
#[allow(non_snake_case)]
#[link(name = "user32")]
unsafe extern "system" {
    fn PrintWindow(hwnd: HWND, hdcblt: HDC, nflags: u32) -> i32;
}

/// Shell window whose presence signals the hidden desktop's explorer shell
/// is up (design §6 readiness probe).
const SHELL_TRAY_CLASS: &str = "Shell_TrayWnd";
/// Explorer's desktop-background host windows: printed with PrintWindow flag
/// 0 and never force-repainted (they are static bitmaps; forcing redraw is
/// wasted work — the rustyvnc recipe's refinement).
const SHELL_BACKGROUND_CLASSES: [&str; 2] = ["Progman", "WorkerW"];
/// Shell windows that must never become the keyboard-focus fallback target.
const SHELL_WINDOW_CLASSES: [&str; 3] = ["Progman", "WorkerW", "Shell_TrayWnd"];

/// Hidden desktop name (design §6): fixed prefix + process pid + a
/// per-process sequence, so concurrent sessions in one process and crashed
/// runs from previous processes all get distinct, recoverable names.
const DESKTOP_NAME_PREFIX: &str = "PinvouAgentDesk-";
/// Per-process sequence for desktop names.
static DESKTOP_SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// The window-station-qualified desktop path handed to child processes via
/// `STARTUPINFOW.lpDesktop`.
const WINDOW_STATION: &str = "WinSta0";

/// Bound on the visible top-level windows composited per capture (a hidden
/// desktop hosts a handful; the cap only guards pathological window farms).
const MAX_COMPOSITE_WINDOWS: usize = 64;
/// GDI-leak tripwire tolerance (object counts may jitter by a couple between
/// captures; monotonic growth beyond this signals a leak — the physical
/// backend once leaked one bitmap per screenshot, never again).
const GDI_LEAK_TOLERANCE: u32 = 4;

/// How long the capture thread may take to attach + allocate its pool before
/// backend creation reports Unavailable (physical fallback at creation).
const CAPTURE_READY_TIMEOUT_MS: u64 = 5000;
/// Shell (`Shell_TrayWnd`) readiness budget (design §6: ≤5 s, warn on miss).
const SHELL_WAIT_MS: u64 = 5000;
const SHELL_WAIT_POLL_MS: u64 = 250;
/// Teardown: how long to wait for the job's processes to die after
/// `TerminateJobObject`, and how long to give the capture thread to exit.
const TEARDOWN_JOB_WAIT_MS: u32 = 5000;
/// Teardown poll interval while confirming the job emptied (see the Drop
/// block: a job handle never signals on emptying, so emptiness is polled
/// via the accounting counter).
const TEARDOWN_JOB_POLL_MS: u64 = 100;
const TEARDOWN_CAPTURE_JOIN_MS: u64 = 2000;
/// Orphan recovery: bounded polls waiting for a leftover desktop to empty.
const RECOVERY_EMPTY_POLLS: u32 = 20;
const RECOVERY_POLL_MS: u64 = 100;

/// Capture: how long to wait for a capture reply before the capture thread
/// is treated as wedged (bounded here so a thread hung inside a
/// RedrawWindow/PrintWindow to a dying window surfaces as an explicit
/// error instead of parking the caller until the handle layer's larger
/// budget expires).
const CAPTURE_REPLY_TIMEOUT_MS: u64 = 5000;
/// Consecutive wedged replies before captures are disabled for the rest of
/// the session (honest degradation: a twice-unresponsive capture thread is
/// not going to start answering — later captures fail immediately and
/// honestly instead of hanging for the timeout every time).
const CAPTURE_WEDGE_LIMIT: u32 = 2;

/// `open` tool-layer bounds mirrored here (the backend is a second line of
/// defense behind the tool's own validation).
const MAX_OPEN_PROGRAM_CHARS: usize = 1024;
const MAX_OPEN_ARGS: usize = 64;
const MAX_OPEN_ARG_CHARS: usize = 4096;

/// `WM_MOUSEWHEEL`'s one detent in `wParam` high word.
const WHEEL_DELTA: i32 = 120;
/// `wParam` button-state marks for posted mouse messages.
const MK_LBUTTON: usize = 0x0001;
const MK_RBUTTON: usize = 0x0002;
const MK_MBUTTON: usize = 0x0010;

/// Cheap availability probe (design §6): the hidden-desktop API set is part
/// of Win32, so the only hard external dependency is the shell binary the
/// desktop launches — probe %WINDIR%\explorer.exe (pure stat, no spawn, no
/// side effects; this runs on every status read, creation happens later via
/// [`create_backend`]). A miss answers Unavailable so the one-time target
/// decision falls back to the physical desktop instead of building a session
/// that fails on its first spawn.
pub(crate) fn probe() -> VirtualAvailability {
    match explorer_presence() {
        ExplorerPresence::Found => VirtualAvailability::Available,
        ExplorerPresence::Missing { detail } => VirtualAvailability::Unavailable { detail },
    }
}

/// Pure probe core over an injectable existence check (unit-testable
/// without machine state).
fn explorer_missing_with(explorer_exists: impl Fn(&str) -> bool) -> Option<String> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_string());
    let explorer = format!("{windir}\\explorer.exe");
    if explorer_exists(&explorer) {
        return None;
    }
    Some(format!(
        "the hidden-desktop shell {explorer} was not found; the virtual display needs \
         the Windows shell to run its session"
    ))
}

/// Real existence check for the probe.
fn explorer_presence() -> ExplorerPresence {
    match explorer_missing_with(|path| {
        std::fs::metadata(path)
            .map(|m| m.is_file())
            .unwrap_or(false)
    }) {
        None => ExplorerPresence::Found,
        Some(detail) => ExplorerPresence::Missing { detail },
    }
}

enum ExplorerPresence {
    Found,
    Missing { detail: String },
}

// ── pure helpers (unit-tested below; no OS calls) ──────────────────────────

/// NUL-terminated UTF-16 for Win32 string parameters. `CreateProcessW` may
/// WRITE into the command-line buffer, so the caller must keep the returned
/// `Vec` mutable and alive across the call.
fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The UTF-16 units one character injects as `WM_CHAR` payloads: a BMP
/// character is one unit, an astral character (emoji) is the full surrogate
/// pair — a dropped low surrogate would deliver a lone half the target app
/// renders as garbage or drops. Pure so the injection loop and its pin
/// share one implementation.
fn char_utf16_units(character: char) -> (u16, Option<u16>) {
    let mut units = [0u16; 2];
    // `char::encode_utf16` writes into the buffer and returns the FILLED
    // SLICE (not an iterator): one unit for BMP characters, both surrogate
    // units for astral ones.
    let encoded = character.encode_utf16(&mut units);
    let second = encoded.get(1).copied();
    (encoded[0], second)
}

/// Chord key order (§6): modifiers lead, the rest keeps the given order.
/// Pure so the chord path and its pin share one implementation instead of
/// the test re-deriving the shape (which would stay green if the chord path
/// stopped partitioning at all).
fn partition_modifiers_first(keys: &[Key]) -> (Vec<&Key>, Vec<&Key>) {
    keys.iter().partition(|key| key.is_modifier())
}

/// The desktop access mask for `CreateDesktopW` (design §6): the narrow set
/// the desktop's own windows need — deliberately NOT `GENERIC_ALL`.
fn desktop_access_mask() -> u32 {
    DESKTOP_CREATEWINDOW.0
        | DESKTOP_CREATEMENU.0
        | DESKTOP_READOBJECTS.0
        | DESKTOP_WRITEOBJECTS.0
        | DESKTOP_ENUMERATE.0
}

/// Hidden desktop name for a sequence number (pure; uniqueness comes from
/// pid + sequence, charset kept to hex + dash for recovery-probe safety).
fn desktop_name(sequence: u32) -> String {
    format!("{DESKTOP_NAME_PREFIX}{:x}-{sequence:x}", std::process::id())
}

/// The `STARTUPINFOW.lpDesktop` value targeting the hidden desktop.
fn desktop_target_path(name: &str) -> String {
    format!("{WINDOW_STATION}\\{name}")
}

/// Whether a window class is one of explorer's static desktop-background
/// hosts (never force-repainted; PrintWindow flag 0).
fn is_shell_background_class(class: &str) -> bool {
    SHELL_BACKGROUND_CLASSES.contains(&class)
}

/// Whether a window class is a shell window that must never become the
/// keyboard-focus fallback target.
fn is_shell_window_class(class: &str) -> bool {
    SHELL_WINDOW_CLASSES.contains(&class)
}

/// `PrintWindow` flags for a window class: the static desktop backgrounds
/// take flag 0 (they don't render layered/DX content), everything else takes
/// `PW_RENDERFULLCONTENT`.
fn print_flags_for_class(class: &str) -> u32 {
    if is_shell_background_class(class) {
        0
    } else {
        PW_RENDERFULLCONTENT
    }
}

/// GDI-leak tripwire (pure): did either object count grow beyond the jitter
/// tolerance since the last capture? (Counts are re-baselined after each
/// warning so the warning stays exceptional.)
fn gdi_leak_suspected(baseline: (u32, u32), current: (u32, u32)) -> bool {
    current.0 > baseline.0 + GDI_LEAK_TOLERANCE || current.1 > baseline.1 + GDI_LEAK_TOLERANCE
}

/// `WM_KEYDOWN`/`WM_KEYUP`/`WM_CHAR` lParam: repeat count 1; the UP variant
/// sets the previous-key-state (bit 30) and transition (bit 31) flags the
/// way `TranslateMessage`-generated input does.
fn key_lparam(up: bool) -> isize {
    if up {
        (1i32 << 30 | 1i32 << 31 | 1) as isize
    } else {
        1
    }
}

/// Pack client coordinates into a mouse-message lParam (both i16).
fn pack_mouse_lparam(x: i32, y: i32) -> isize {
    (((y as u16 as u32) << 16) | (x as u16 as u32)) as isize
}

/// Unpack a mouse lParam (inverse of [`pack_mouse_lparam`]; test-only shape
/// mirror of how targets decode it).
#[cfg(test)]
fn unpack_mouse_lparam(lparam: isize) -> (i32, i32) {
    let raw = lparam as u32;
    (raw as u16 as i16 as i32, (raw >> 16) as u16 as i16 as i32)
}

/// The WM_CHAR payload for a key-shaped type run: `'\r'` for Return, `'\t'`
/// for Tab — the characters real keyboard input synthesizes for those keys.
/// Delivered as WM_CHAR ONLY (never wrapped in a WM_KEYDOWN/WM_KEYUP triple):
/// a posted WM_KEYDOWN makes the target's own `TranslateMessage` generate a
/// SECOND WM_CHAR, doubling newlines/tabs in any standard GUI app — the same
/// doubling that confines the printable path to WM_CHAR-only posts.
/// `None` for text runs (they post per-character). Pure; pinned.
fn key_run_char(run: &TypeRun) -> Option<u16> {
    match run {
        TypeRun::Return => Some(u16::from(b'\r')),
        TypeRun::Tab => Some(u16::from(b'\t')),
        TypeRun::Text(_) => None,
    }
}

/// Wheel message + per-detent delta for a scroll direction. `WM_MOUSEWHEEL`
/// positive delta scrolls up/away; `WM_MOUSEHWHEEL` positive scrolls right
/// (the same conventions `map_scroll` encodes for the physical backend).
fn wheel_message_and_delta(direction: ScrollDirection) -> (u32, i32) {
    match direction {
        ScrollDirection::Up => (WM_MOUSEWHEEL, WHEEL_DELTA),
        ScrollDirection::Down => (WM_MOUSEWHEEL, -WHEEL_DELTA),
        ScrollDirection::Left => (WM_MOUSEHWHEEL, -WHEEL_DELTA),
        ScrollDirection::Right => (WM_MOUSEHWHEEL, WHEEL_DELTA),
    }
}

/// Wheel `wParam`: the signed delta in the high word.
fn wheel_wparam(delta: i32) -> usize {
    ((delta as u16 as u32) << 16) as usize
}

/// The `(down, double-click, up, wParam mark)` message tuple for a button.
/// The double-click message substitutes for the SECOND down, mirroring what
/// the OS generates for real input — see [`multi_click_message`].
fn mouse_button_messages(button: MouseButton) -> (u32, u32, u32, usize) {
    match button {
        MouseButton::Left => (WM_LBUTTONDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONUP, MK_LBUTTON),
        MouseButton::Right => (WM_RBUTTONDOWN, WM_RBUTTONDBLCLK, WM_RBUTTONUP, MK_RBUTTON),
        MouseButton::Middle => (WM_MBUTTONDOWN, WM_MBUTTONDBLCLK, WM_MBUTTONUP, MK_MBUTTON),
    }
}

/// The message for press `index` (0-based) of a multi-click. The OS turns
/// the second (and fourth, ...) press into a DBLCLK **only** when the
/// target's window class registered `CS_DBLCLKS`; every other class
/// receives another plain down (to it, no double-click ever happened).
/// Mirroring that per target keeps `click(count=2)` meaningful for
/// non-DBLCLKS apps instead of delivering one effective click.
fn multi_click_message(index: u8, down: u32, double: u32, accepts_dblclk: bool) -> u32 {
    if index % 2 == 1 && accepts_dblclk {
        double
    } else {
        down
    }
}

/// Whether `target`'s window class registered `CS_DBLCLKS`. The OS sends
/// `WM_*BUTTONDBLCLK` only to classes that registered the style, so the
/// conservative answer for BOTH a query failure (GetClassLongW → 0) and a
/// genuinely style-0 class is "not DBLCLK-capable": the alternating-down
/// sequence still delivers the click; posting a DBLCLK message to a class
/// that never registered it is delivery the app never asked for and never
/// handles.
///
/// SAFETY: class-style query on a desktop window from the attached worker.
unsafe fn window_class_accepts_dblclk(target: HWND) -> bool {
    // SAFETY: see the fn contract.
    let style = unsafe { GetClassLongW(target, GCL_STYLE) };
    style != 0 && (style & CS_DBLCLKS.0) != 0
}

/// This crate's [`Key`] → Win32 virtual-key code. EXHAUSTIVE over `Key`
/// (no wildcard arm — a new variant fails compile until it is mapped). The
/// `Char` arm delegates to [`char_to_vk`], whose table covers the standard
/// US-layout key positions; anything else refuses explicitly (the type
/// action's WM_CHAR path carries arbitrary Unicode).
fn key_to_vk(key: Key) -> Result<u16, ComputerUseError> {
    match key {
        Key::Control => Ok(VK_CONTROL.0),
        Key::Alt => Ok(VK_MENU.0),
        Key::Shift => Ok(VK_SHIFT.0),
        Key::Meta => Ok(VK_LWIN.0),
        Key::Enter => Ok(VK_RETURN.0),
        Key::Escape => Ok(VK_ESCAPE.0),
        Key::Tab => Ok(VK_TAB.0),
        Key::Space => Ok(VK_SPACE.0),
        Key::Backspace => Ok(VK_BACK.0),
        Key::Delete => Ok(VK_DELETE.0),
        Key::Insert => Ok(VK_INSERT.0),
        Key::Up => Ok(VK_UP.0),
        Key::Down => Ok(VK_DOWN.0),
        Key::Left => Ok(VK_LEFT.0),
        Key::Right => Ok(VK_RIGHT.0),
        Key::Home => Ok(VK_HOME.0),
        Key::End => Ok(VK_END.0),
        Key::PageUp => Ok(VK_PRIOR.0),
        Key::PageDown => Ok(VK_NEXT.0),
        Key::Function(n) => {
            if (1..=12).contains(&n) {
                Ok(VK_F1.0 + u16::from(n - 1))
            } else {
                Err(ComputerUseError::failed(format!(
                    "function key f{n} is outside the supported f1..=f12 range"
                )))
            }
        }
        Key::Char(c) => char_to_vk(c),
    }
}

/// Single-character key → VK for the standard US layout's key positions
/// (VK codes for letters/digits equal their ASCII; punctuation maps to the
/// OEM VKs). Chars without a key position refuse explicitly — never a
/// wildcard mapping (the type action carries arbitrary Unicode via WM_CHAR).
fn char_to_vk(c: char) -> Result<u16, ComputerUseError> {
    let upper = c.to_ascii_uppercase();
    let vk = match upper {
        'A'..='Z' => upper as u16,
        '0'..='9' => upper as u16,
        ' ' => VK_SPACE.0,
        '+' => VK_OEM_PLUS.0,
        ',' => VK_OEM_COMMA.0,
        '-' => VK_OEM_MINUS.0,
        '.' => VK_OEM_PERIOD.0,
        '/' => VK_OEM_2.0,
        ';' => VK_OEM_1.0,
        '`' => VK_OEM_3.0,
        '[' => VK_OEM_4.0,
        '\\' => VK_OEM_5.0,
        ']' => VK_OEM_6.0,
        '\'' => VK_OEM_7.0,
        _ => {
            return Err(ComputerUseError::failed(
                "character is not a standard keyboard key position; \
                 use the type action for text input",
            ));
        }
    };
    Ok(vk)
}

/// Windows command-line quoting: wrap parts with spaces/tabs/quotes (or
/// empty parts), escape embedded quotes, and double the trailing-backslash
/// run of quoted parts. The doubling follows the Windows argv rule (per the
/// command-line parsing documented on GetCommandLine/CommandLineToArgvW):
/// inside quotes, backslashes directly before a quote character are escape
/// sequences, so a quoted `C:\dir\` would otherwise turn its CLOSING quote
/// into a literal backslash-quote and corrupt every following argument.
/// Bare (unquoted) parts keep their backslashes literal — unquoted
/// backslashes are not escape sequences, so doubling them would corrupt the
/// path instead. Best-effort single-level quoting — backslash runs before
/// EMBEDDED quotes inside a part remain out of scope (the model's `args`
/// are plain strings; CreateProcessW receives the joined line unparsed).
fn quote_command_line_part(part: &str) -> String {
    if part.is_empty() || part.contains(' ') || part.contains('\t') || part.contains('"') {
        let trailing_backslashes = part.chars().rev().take_while(|&c| c == '\\').count();
        let mut quoted = part.replace('"', "\\\"");
        quoted.push_str(&"\\".repeat(trailing_backslashes));
        format!("\"{quoted}\"")
    } else {
        part.to_string()
    }
}

/// Build the `lpCommandLine` string for a launch.
fn build_command_line(program: &str, args: &[String]) -> String {
    let mut line = quote_command_line_part(program);
    for arg in args {
        line.push(' ');
        line.push_str(&quote_command_line_part(arg));
    }
    line
}

/// Validate an `open` request against the same bounds the tool layer
/// enforces (non-empty, NUL-free, size-capped program; capped argument
/// count/length). Error messages stay generic (no echo of the program text).
fn validate_open_request(program: &str, args: &[String]) -> Result<(), ComputerUseError> {
    if program.is_empty() {
        return Err(ComputerUseError::failed("open: program must not be empty"));
    }
    if program.len() > MAX_OPEN_PROGRAM_CHARS {
        return Err(ComputerUseError::failed(format!(
            "open: the program path exceeds {MAX_OPEN_PROGRAM_CHARS} characters"
        )));
    }
    if program.contains('\0') {
        return Err(ComputerUseError::failed(
            "open: the program path must not contain NUL",
        ));
    }
    if args.len() > MAX_OPEN_ARGS {
        return Err(ComputerUseError::failed(format!(
            "open: more than {MAX_OPEN_ARGS} arguments"
        )));
    }
    for arg in args {
        if arg.contains('\0') {
            return Err(ComputerUseError::failed(
                "open: arguments must not contain NUL",
            ));
        }
        if arg.len() > MAX_OPEN_ARG_CHARS {
            return Err(ComputerUseError::failed(format!(
                "open: an argument exceeds {MAX_OPEN_ARG_CHARS} characters"
            )));
        }
    }
    Ok(())
}

/// The hidden desktop's shell launch plan: (lpApplicationName, lpCommandLine).
/// The application path is passed RAW (lpApplicationName is never parsed, so
/// quoting it would corrupt the path). NO folder argument: the desktop starts
/// blank and `wait_for_shell_tray` gates readiness — opening a default folder
/// (the old plan used %USERPROFILE%) would put the user's home-directory file
/// listing into every early screenshot, leaking private names into
/// agent-visible output. The tray window appears with a bare launch too.
fn explorer_launch_plan(windir: Option<&str>) -> (String, String) {
    let windir = windir
        .filter(|value| !value.is_empty())
        .unwrap_or("C:\\Windows");
    (
        format!("{windir}\\explorer.exe"),
        "explorer.exe".to_string(),
    )
}

/// `GetDIBits` returns the number of scan lines copied as an `i32` (it can
/// also fail outright with a negative value). A short read — fewer lines
/// than requested, e.g. under GDI pressure — would leave the buffer tail
/// zeroed: half-black frames reported as good captures. Returns the
/// explicit failure reason when the read is incomplete, `None` on a full
/// read. Pure, so the rejection is pinable without a live desktop.
fn short_read_reason(lines: i32, height: i32) -> Option<String> {
    if lines < 0 {
        Some(format!(
            "hidden desktop capture: GetDIBits failed with code {lines}"
        ))
    } else if lines != height {
        Some(format!(
            "hidden desktop capture: GetDIBits returned {lines} of {height} scan lines"
        ))
    } else {
        None
    }
}

/// Flip bottom-up rows to top-down and swizzle BGRX → RGBA in place (a
/// 32bpp `GetDIBits` read). Pure, byte-exact-tested below.
fn flip_and_swizzle_bgrx_to_rgba(buffer: &mut [u8], width: usize, height: usize) {
    fn swizzle_row(row: &mut [u8]) {
        for pixel in row.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
    }
    let stride = width * 4;
    for y in 0..height / 2 {
        let top = y * stride;
        let bottom = (height - 1 - y) * stride;
        // split_at_mut makes the two row borrows disjoint (a direct double
        // `&mut buffer[..]` does not compile).
        let (head, tail) = buffer.split_at_mut(bottom);
        swizzle_row(&mut head[top..top + stride]);
        swizzle_row(tail);
        head[top..top + stride].swap_with_slice(&mut tail[..stride]);
    }
    if height % 2 == 1 {
        let middle = (height / 2) * stride;
        swizzle_row(&mut buffer[middle..middle + stride]);
    }
}

// ── Win32 call wrappers (thin, mapped to ComputerUseError) ─────────────────

fn log_desktop_close_error(context: &str, error: windows::core::Error) {
    if error.code() == HRESULT::from_win32(ERROR_BUSY.0) {
        log::warn!(
            "[computer_use][virtual-win] {context}: the desktop is still referenced \
             (ERROR_BUSY); it is left behind and recovered by the next start's \
             OpenDesktopW probe"
        );
    } else {
        log::warn!("[computer_use][virtual-win] {context}: {error}");
    }
}

/// Attach the CURRENT thread to the hidden desktop and verify the switch via
/// `GetThreadDesktop` comparison (design §6 discipline). Returns the
/// thread's previous desktop for restoration.
///
/// SAFETY: the caller guarantees this thread owns no windows/hooks — a
/// thread with window/hook ownership cannot switch desktops, and the
/// backend worker (per BackendHandle) and a fresh capture thread both do.
unsafe fn attach_thread_to_desktop(desktop: HDESK) -> Result<HDESK, ComputerUseError> {
    // SAFETY: thread-info queries + a desktop switch; see the fn contract.
    unsafe {
        let tid = GetCurrentThreadId();
        let previous = GetThreadDesktop(tid).map_err(|error| {
            ComputerUseError::failed(format!("cannot read the thread's desktop: {error}"))
        })?;
        SetThreadDesktop(desktop).map_err(|error| {
            ComputerUseError::unavailable(format!(
                "cannot attach the thread to the hidden desktop: {error}"
            ))
        })?;
        let current = GetThreadDesktop(tid).map_err(|error| {
            ComputerUseError::failed(format!("cannot verify the thread's desktop: {error}"))
        })?;
        if current.0 != desktop.0 {
            let _ = SetThreadDesktop(previous);
            return Err(ComputerUseError::unavailable(
                "the hidden desktop attachment did not take effect (GetThreadDesktop mismatch)",
            ));
        }
        Ok(previous)
    }
}

/// The hidden desktop's virtual-screen bounding rect: origin and size of the
/// WHOLE multi-monitor session, not just the primary monitor. Both capture
/// and input are specified in this space — screen coordinates are
/// virtual-screen-global on Windows (`WindowFromPoint`/`SetCursorPos`/
/// `GetCursorPos`/`GetWindowRect` all take or return them, negatives
/// included), so hit-testing already spans every monitor; capture composites
/// the same rect and reports it as the `Capture` origin. Re-read per capture:
/// a monitor joining or leaving mid-session changes the metrics, and the
/// query is cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VirtualScreenRect {
    origin_x: i32,
    origin_y: i32,
    width: i32,
    height: i32,
}

/// Read the virtual-screen rect from the calling thread's attached desktop.
/// `SM_*VIRTUALSCREEN` metrics are PER-SESSION (every desktop in the session
/// shares the session's monitor topology), so on the worker — attached to the
/// hidden desktop — they describe exactly the monitor layout the hidden
/// desktop's windows are positioned in (identical to the user's physical
/// layout; the isolation is per-desktop, not per-monitor-topology).
///
/// SAFETY: pure metric query; no pointers involved.
unsafe fn desktop_virtual_screen_rect() -> VirtualScreenRect {
    // SAFETY: GetSystemMetrics with SM_* indices returns i32 values.
    unsafe {
        VirtualScreenRect {
            origin_x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            origin_y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// Composite destination position for a window whose top-left sits at
/// screen-space `(window_x, window_y)` inside a virtual screen with origin
/// `(origin_x, origin_y)`: subtract the virtual origin (the composite bitmap
/// is virtual-screen-local; window rects are virtual-screen-global).
/// Pure; unit-pinned because a sign error here silently captures the wrong
/// region on every multi-monitor session (any monitor left of/above the
/// primary makes real window coordinates negative).
fn composite_pos(window_x: i32, window_y: i32, origin_x: i32, origin_y: i32) -> (i32, i32) {
    (window_x - origin_x, window_y - origin_y)
}

/// Per-process GDI/user object counts for the leak tripwire.
///
/// SAFETY: pseudo-handle query; no ownership transfer.
unsafe fn gui_object_counts() -> (u32, u32) {
    // SAFETY: GetCurrentProcess returns the constant pseudo-handle.
    unsafe {
        let process = GetCurrentProcess();
        (
            GetGuiResources(process, GR_GDIOBJECTS),
            GetGuiResources(process, GR_USEROBJECTS),
        )
    }
}

/// `EnumDesktopWindows` collector: push each desktop window's pid. Always
/// continues (`BOOL(1)`), so `EnumDesktopWindows` ends by natural exhaustion.
///
/// SAFETY: `lparam` carries the collector `Vec` pointer handed in by
/// [`desktop_window_pids`]; the callback reads a pid and pushes — no
/// lifetime escapes the enumeration.
unsafe extern "system" fn desktop_pid_collector(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: the raw pointer is valid for the whole enumeration (the caller
    // keeps the Vec alive across the EnumDesktopWindows call).
    unsafe {
        let list = lparam.0 as *mut Vec<u32>;
        if !list.is_null() {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid != 0 {
                (*list).push(pid);
            }
        }
    }
    BOOL(1)
}

/// All process ids that currently own windows on the desktop.
///
/// SAFETY: enumeration over our own named desktop; the collector Vec
/// outlives the call.
unsafe fn desktop_window_pids(desktop: HDESK) -> Vec<u32> {
    // SAFETY: see the fn contract.
    unsafe {
        let mut pids: Vec<u32> = Vec::new();
        let result = EnumDesktopWindows(
            Some(desktop),
            Some(desktop_pid_collector),
            LPARAM(std::ptr::addr_of_mut!(pids) as isize),
        );
        if result.is_err() {
            // A partially-collected list is still usable for recovery; report it.
            log::warn!(
                "[computer_use][virtual-win] desktop window enumeration ended early \
                 ({} pids collected)",
                pids.len()
            );
        }
        pids
    }
}

/// Recover a leftover desktop from a crashed run (design §6: name-based
/// recovery, no pidfile): kill every process owning a window on it, wait
/// bounded for the desktop to empty, close the probe handle.
///
/// SAFETY: bounded kill/close sequence on our own uniquely-named desktop;
/// `OpenProcess`/`TerminateProcess`/`CloseHandle` operate on handles the
/// calls themselves produce and release.
unsafe fn recover_leftover_desktop(name: &str, desktop: HDESK) {
    log::warn!(
        "[computer_use][virtual-win] hidden desktop '{name}' already exists \
         (leftover from a crashed run); recovering"
    );
    // SAFETY: enumeration + handle-scoped kills, see the fn contract.
    unsafe {
        for pid in desktop_window_pids(desktop) {
            // Pid-recycling guard (same fail-safe shape as the Linux
            // identity gate, minus a recorded argv to compare): a pid seen
            // in the collection may have exited and been RECYCLED by the
            // time OpenProcess runs — re-check against a FRESH enumeration
            // immediately before the kill, so a number-only kill cannot
            // take out an unrelated process that reused the number.
            if !desktop_window_pids(desktop).contains(&pid) {
                log::debug!(
                    "[computer_use][virtual-win] leftover pid {pid} no longer owns a \
                     window on the desktop (exited or recycled); skipping"
                );
                continue;
            }
            match OpenProcess(PROCESS_TERMINATE, false, pid) {
                Ok(process) => {
                    if let Err(error) = TerminateProcess(process, 1) {
                        log::warn!(
                            "[computer_use][virtual-win] cannot terminate the leftover \
                             process {pid}: {error}"
                        );
                    }
                    let _ = CloseHandle(process);
                }
                Err(error) => log::warn!(
                    "[computer_use][virtual-win] cannot open the leftover process \
                     {pid} for termination: {error}"
                ),
            }
        }
        for _ in 0..RECOVERY_EMPTY_POLLS {
            if desktop_window_pids(desktop).is_empty() {
                break;
            }
            sleep(Duration::from_millis(RECOVERY_POLL_MS));
        }
        if let Err(error) = CloseDesktop(desktop) {
            log_desktop_close_error(
                &format!("cannot close the recovered leftover desktop '{name}'"),
                error,
            );
        }
    }
}

/// Create the kill-on-close job object. Deliberately NO `JOB_OBJECT_UILIMIT_*`
/// flags: combining UI restrictions with certain window procedures is the
/// documented BSOD recipe (design §6 caveat) — containment here means "die
/// with the session", not "sandbox the app".
///
/// SAFETY: configuration-only job-object calls on a fresh unnamed handle.
unsafe fn create_kill_on_close_job() -> Result<HANDLE, ComputerUseError> {
    // SAFETY: see the fn contract; the info pointer is `ptr::from_ref().cast()`
    // (never a `as *const _ as *const` chain) and outlives the call.
    unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null()).map_err(|error| {
            ComputerUseError::unavailable(format!("cannot create the session job object: {error}"))
        })?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Err(error) = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&limits).cast::<core::ffi::c_void>(),
            core::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) {
            let _ = CloseHandle(job);
            return Err(ComputerUseError::unavailable(format!(
                "cannot configure the session job object (kill-on-close): {error}"
            )));
        }
        Ok(job)
    }
}

/// Launch one process ON the hidden desktop and contain it in the session
/// job, in the fail-safe order: `CREATE_SUSPENDED` →
/// `AssignProcessToJobObject` → `ResumeThread` — a process that cannot be
/// contained is terminated while it is still suspended (it has run no code).
///
/// `desktop_path_wide` (`WinSta0\name`) and the command line are copied into
/// mutable, NUL-terminated buffers that outlive the `CreateProcessW` call
/// (`lpDesktop` is a `PWSTR`; the OS may write into the command line).
/// Returns the child pid.
///
/// SAFETY: the two wide buffers live on this stack frame across the call;
/// every produced handle is closed on every path.
unsafe fn launch_suspended_in_job(
    desktop_path_wide: Vec<u16>,
    application: Option<&[u16]>,
    command_line: &str,
    job: HANDLE,
) -> Result<u32, ComputerUseError> {
    // SAFETY: see the fn contract.
    unsafe {
        let mut desktop_buffer = desktop_path_wide;
        let mut command_buffer = to_wide(command_line);
        let mut startup = STARTUPINFOW::default();
        // The documented contract requires cb = sizeof(STARTUPINFOW); without
        // it the system is free to ignore lpDesktop and start the process on
        // the caller's desktop instead of the hidden one.
        startup.cb = core::mem::size_of::<STARTUPINFOW>() as u32;
        startup.lpDesktop = PWSTR::from_raw(desktop_buffer.as_mut_ptr());
        let mut info = PROCESS_INFORMATION::default();
        let application_name = application.map(|path| PCWSTR(path.as_ptr()));
        CreateProcessW(
            application_name.unwrap_or(PCWSTR::null()),
            Some(PWSTR::from_raw(command_buffer.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_SUSPENDED,
            None,
            PCWSTR::null(),
            &startup,
            &mut info,
        )
        .map_err(|error| {
            ComputerUseError::failed(format!(
                "cannot launch the process on the hidden desktop: {error}"
            ))
        })?;
        if let Err(error) = AssignProcessToJobObject(job, info.hProcess) {
            let _ = TerminateProcess(info.hProcess, 1);
            let _ = CloseHandle(info.hProcess);
            let _ = CloseHandle(info.hThread);
            return Err(ComputerUseError::failed(format!(
                "the launched process could not be contained by the session job \
                 object and was terminated while still suspended: {error}"
            )));
        }
        let resumed = ResumeThread(info.hThread);
        if resumed == u32::MAX {
            // The process is contained (assigned to the kill-on-close job)
            // but still SUSPENDED — it would linger until session end.
            // Terminate it now, mirroring the assign-failure path above,
            // BEFORE the handles close.
            // SAFETY: termination of our own just-launched, still-suspended
            // process handle (never resumed, so nothing ran).
            unsafe {
                let _ = TerminateProcess(info.hProcess, 1);
            }
        }
        let _ = CloseHandle(info.hProcess);
        let _ = CloseHandle(info.hThread);
        if resumed == u32::MAX {
            return Err(ComputerUseError::failed(
                "the contained process could not be resumed and was terminated \
                 while still suspended",
            ));
        }
        Ok(info.dwProcessId)
    }
}

/// Wait for the hidden desktop's shell (`Shell_TrayWnd`) up to `deadline`.
/// MUST run on a thread already attached to the desktop (`FindWindowW` is
/// desktop-relative to the calling thread). True when found.
///
/// SAFETY: desktop-relative window lookup from the attached calling thread.
unsafe fn wait_for_shell_tray(deadline: Instant) -> bool {
    // SAFETY: see the fn contract.
    unsafe {
        let class = to_wide(SHELL_TRAY_CLASS);
        while Instant::now() < deadline {
            if FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()).is_ok() {
                return true;
            }
            sleep(Duration::from_millis(SHELL_WAIT_POLL_MS));
        }
    }
    false
}

// ── capture thread ──────────────────────────────────────────────────────────

/// One capture request (reply channel inside).
struct CaptureJob {
    reply: Sender<Result<Capture, ComputerUseError>>,
}

/// A desktop handle on its way to the capture thread.
///
/// SAFETY (Send): `HDESK` is a kernel object handle; handles are process-wide
/// tokens that may legally be used from any thread (the API set this module
/// uses — `SetThreadDesktop`, `CloseDesktop`, `EnumDesktopWindows` — takes
/// them as values). The wrapper transfers ownership to the capture thread,
/// which is its only user after the move; the creating thread never touches
/// the handle again (Drop tears down via the backend's own field).
struct SendableDesktop(HDESK);

// SAFETY: HDESK is a kernel object handle — a process-wide token usable from
// any thread; the move transfers exclusive ownership to the capture thread
// (the creator never touches the handle afterwards).
unsafe impl Send for SendableDesktop {}

/// Pooled GDI objects for the composite capture, owned by the capture thread
/// for its whole life (allocation per session — re-sized only when the
/// session's monitor layout changes — not per capture; the physical
/// backend's historical one-bitmap-per-screenshot leak is the trap this
/// pool + the `GetGuiResources` tripwire exist to prevent).
struct GdiPool {
    /// The desktop's screen DC — a compatible-DC reference ONLY. Never
    /// blitted FROM (a hidden desktop has no DWM; that read is black).
    screen_dc: HDC,
    composite_dc: HDC,
    composite_bitmap: HBITMAP,
    /// The default bitmap `SelectObject` displaced (restored before
    /// `GetDIBits`, which requires the bitmap deselected, and at teardown —
    /// the deselect-then-delete ordering that closed the physical backend's
    /// xcap leak).
    composite_default: HGDIOBJ,
    tmp_dc: HDC,
    tmp_bitmap: HBITMAP,
    tmp_default: HGDIOBJ,
    tmp_width: i32,
    tmp_height: i32,
    /// The virtual-screen rect the composite currently covers (see
    /// [`desktop_virtual_screen_rect`]: window rects are virtual-screen-
    /// global, the composite is virtual-screen-local, so every blit offsets
    /// by this origin and every `Capture` reports it).
    rect: VirtualScreenRect,
}

impl GdiPool {
    /// Allocate the pool for a virtual-screen rect.
    ///
    /// SAFETY: caller is the capture thread, already attached to the desktop;
    /// partial-failure paths release every acquired object in reverse order.
    unsafe fn new(rect: VirtualScreenRect) -> Option<Self> {
        // SAFETY: DC/bitmap acquisition with per-step rollback, see above.
        unsafe {
            let screen_dc = GetDC(None);
            if screen_dc.is_invalid() {
                return None;
            }
            let composite_dc = CreateCompatibleDC(Some(screen_dc));
            if composite_dc.is_invalid() {
                ReleaseDC(None, screen_dc);
                return None;
            }
            let composite_bitmap = CreateCompatibleBitmap(screen_dc, rect.width, rect.height);
            if composite_bitmap.is_invalid() {
                let _ = DeleteDC(composite_dc);
                ReleaseDC(None, screen_dc);
                return None;
            }
            let composite_default = SelectObject(composite_dc, HGDIOBJ(composite_bitmap.0));
            let tmp_dc = CreateCompatibleDC(Some(screen_dc));
            if tmp_dc.is_invalid() {
                let _ = SelectObject(composite_dc, composite_default);
                let _ = DeleteObject(HGDIOBJ(composite_bitmap.0));
                let _ = DeleteDC(composite_dc);
                ReleaseDC(None, screen_dc);
                return None;
            }
            Some(Self {
                screen_dc,
                composite_dc,
                composite_bitmap,
                composite_default,
                tmp_dc,
                tmp_bitmap: HBITMAP(std::ptr::null_mut()),
                tmp_default: HGDIOBJ(std::ptr::null_mut()),
                tmp_width: 0,
                tmp_height: 0,
                rect,
            })
        }
    }

    /// Re-target the composite to a (possibly changed) virtual-screen rect.
    /// Monitors can join or leave a session mid-flight, so the capture loop
    /// re-reads the metrics every frame and rebuilds the pooled bitmap only
    /// on change (allocate-then-swap keeps the old bitmap deletable and the
    /// pool usable if the reallocation fails: this frame errors, the next
    /// one retries).
    ///
    /// SAFETY: capture thread only; deselect-before-delete ordering keeps
    /// every deleted object deletable.
    unsafe fn ensure_composite_rect(&mut self, rect: VirtualScreenRect) -> bool {
        if rect.width <= 0 || rect.height <= 0 {
            return false;
        }
        if self.rect == rect && !self.composite_bitmap.is_invalid() {
            return true;
        }
        // SAFETY: capture-thread GDI object lifecycle, see above.
        unsafe {
            SelectObject(self.composite_dc, self.composite_default);
            let bitmap = CreateCompatibleBitmap(self.screen_dc, rect.width, rect.height);
            if bitmap.is_invalid() {
                // Keep serving the old rect's composite if it still exists;
                // the caller errors this frame either way.
                SelectObject(self.composite_dc, HGDIOBJ(self.composite_bitmap.0));
                return false;
            }
            let _ = DeleteObject(HGDIOBJ(self.composite_bitmap.0));
            self.composite_bitmap = bitmap;
            self.composite_default = SelectObject(self.composite_dc, HGDIOBJ(bitmap.0));
            self.rect = rect;
            true
        }
    }

    /// Ensure the pooled PrintWindow bitmap covers `w × h` (grow-only).
    /// SAFETY: capture thread only; deselect-before-delete ordering keeps
    /// every deleted object deletable.
    unsafe fn ensure_tmp_bitmap(&mut self, w: i32, h: i32) -> bool {
        // SAFETY: capture-thread GDI object lifecycle, see above.
        unsafe {
            if w <= self.tmp_width && h <= self.tmp_height && !self.tmp_bitmap.is_invalid() {
                return true;
            }
            if !self.tmp_bitmap.is_invalid() {
                let _ = SelectObject(self.tmp_dc, self.tmp_default);
                let _ = DeleteObject(HGDIOBJ(self.tmp_bitmap.0));
                self.tmp_bitmap = HBITMAP(std::ptr::null_mut());
            }
            let new_width = w.max(self.tmp_width);
            let new_height = h.max(self.tmp_height);
            let bitmap = CreateCompatibleBitmap(self.screen_dc, new_width, new_height);
            if bitmap.is_invalid() {
                return false;
            }
            self.tmp_width = new_width;
            self.tmp_height = new_height;
            self.tmp_default = SelectObject(self.tmp_dc, HGDIOBJ(bitmap.0));
            self.tmp_bitmap = bitmap;
            true
        }
    }

    /// Composite every visible top-level window (bottom-to-top painter's
    /// order) into the pooled bitmap.
    /// SAFETY: capture thread only; handles come from desktop enumeration.
    unsafe fn composite_windows(&mut self) {
        // SAFETY: capture-thread composite; see above.
        unsafe {
            let _ = PatBlt(
                self.composite_dc,
                0,
                0,
                self.rect.width,
                self.rect.height,
                BLACKNESS,
            );
            for hwnd in walk_visible_toplevels(MAX_COMPOSITE_WINDOWS) {
                let mut rect = RECT::default();
                if GetWindowRect(hwnd, &mut rect).is_err() {
                    continue; // window died between enumeration and rect read
                }
                let w = rect.right - rect.left;
                let h = rect.bottom - rect.top;
                if w <= 0 || h <= 0 {
                    continue;
                }
                let class = window_class_name(hwnd);
                // Window rects are virtual-screen-global; the composite is
                // virtual-screen-local — offset by the virtual origin so
                // windows on monitors left of/above the primary (negative
                // coordinates) land inside the bitmap instead of being
                // clipped away.
                let (dest_x, dest_y) =
                    composite_pos(rect.left, rect.top, self.rect.origin_x, self.rect.origin_y);
                if !is_shell_background_class(&class) {
                    // Force a synchronous repaint: without DWM nothing refreshes
                    // hidden-desktop windows on its own. Send-on-hung-app can
                    // block here (same class of risk as the physical backend's
                    // UIA/PrintWindow calls; bounded by the caller's timeout).
                    let _ = RedrawWindow(
                        Some(hwnd),
                        None,
                        None,
                        RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
                    );
                }
                if !self.ensure_tmp_bitmap(w, h) {
                    continue;
                }
                let flags = print_flags_for_class(&class);
                if PrintWindow(hwnd, self.tmp_dc, flags) == 0
                    && PrintWindow(hwnd, self.tmp_dc, 0) == 0
                {
                    // Window refuses PrintWindow entirely; skip without failing
                    // the whole frame (the rest still composites).
                    continue;
                }
                let _ = BitBlt(
                    self.composite_dc,
                    dest_x,
                    dest_y,
                    w,
                    h,
                    Some(self.tmp_dc),
                    0,
                    0,
                    SRCCOPY,
                );
            }
        }
    }

    /// Read the composite into RGBA (flip + swizzle).
    /// SAFETY: capture thread only; the bitmap is deselected around GetDIBits
    /// (its documented requirement).
    unsafe fn read_rgba(&mut self) -> Result<Capture, ComputerUseError> {
        // SAFETY: capture-thread GDI read; see above.
        unsafe {
            // GetDIBits requires the bitmap NOT selected into a DC (the exact
            // ordering violation that leaked xcap's bitmaps upstream).
            SelectObject(self.composite_dc, self.composite_default);
            let mut info = BITMAPINFO::default();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: core::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: self.rect.width,
                biHeight: self.rect.height, // positive: bottom-up read
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            };
            let mut buffer =
                vec![0u8; (self.rect.width as usize) * (self.rect.height as usize) * 4];
            let lines = GetDIBits(
                self.composite_dc,
                self.composite_bitmap,
                0,
                self.rect.height as u32,
                Some(buffer.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            );
            SelectObject(self.composite_dc, HGDIOBJ(self.composite_bitmap.0));
            // A short read (fewer scan lines than requested, e.g. under GDI
            // pressure) would leave the buffer tail zeroed — half-black
            // frames reported as good captures. Fail explicitly instead.
            if let Some(reason) = short_read_reason(lines, self.rect.height) {
                return Err(ComputerUseError::unavailable(reason));
            }
            flip_and_swizzle_bgrx_to_rgba(
                &mut buffer,
                self.rect.width as usize,
                self.rect.height as usize,
            );
            Ok(Capture {
                rgba: buffer,
                width: self.rect.width as u32,
                height: self.rect.height as u32,
                // The composite covers the session's whole virtual screen:
                // origin in input coordinate space (= screen coordinates, the
                // space hit-testing consumes), so a capture pixel maps to
                // `origin + pixel` — consistent across every monitor layout.
                origin_x: self.rect.origin_x,
                origin_y: self.rect.origin_y,
                input_scale_x: 1.0,
                input_scale_y: 1.0,
                input_aligned: true,
            })
        }
    }

    /// Release every pooled object (reverse acquisition order, deselect
    /// before delete).
    /// SAFETY: capture thread only; the pool is dropped right after.
    unsafe fn release(&mut self) {
        // SAFETY: capture-thread GDI teardown; see above.
        unsafe {
            if !self.tmp_bitmap.is_invalid() {
                let _ = SelectObject(self.tmp_dc, self.tmp_default);
                let _ = DeleteObject(HGDIOBJ(self.tmp_bitmap.0));
                self.tmp_bitmap = HBITMAP(std::ptr::null_mut());
            }
            let _ = DeleteDC(self.tmp_dc);
            let _ = SelectObject(self.composite_dc, self.composite_default);
            let _ = DeleteObject(HGDIOBJ(self.composite_bitmap.0));
            self.composite_bitmap = HBITMAP(std::ptr::null_mut());
            let _ = DeleteDC(self.composite_dc);
            ReleaseDC(None, self.screen_dc);
            self.screen_dc = HDC(std::ptr::null_mut());
        }
    }
}

/// The capture thread body: attach FIRST (the thread owns zero windows —
/// it was spawned microseconds ago and has touched no USER/GDI object),
/// build the pool once, serve capture jobs until the channel disconnects.
fn capture_thread_main(
    desktop: SendableDesktop,
    jobs: Receiver<CaptureJob>,
    ready: Sender<Result<(), ComputerUseError>>,
) {
    let desktop = desktop.0;
    // SAFETY: fresh thread, no windows/hooks owned (module doc discipline).
    let previous = match unsafe { attach_thread_to_desktop(desktop) } {
        Ok(previous) => previous,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    // SAFETY: pure metric query on the attached capture thread.
    let initial_rect = unsafe { desktop_virtual_screen_rect() };
    if initial_rect.width <= 0 || initial_rect.height <= 0 {
        let _ = ready.send(Err(ComputerUseError::unavailable(format!(
            "the hidden desktop reports a degenerate virtual screen \
             {}x{} (origin {}, {})",
            initial_rect.width, initial_rect.height, initial_rect.origin_x, initial_rect.origin_y
        ))));
        return;
    }
    // SAFETY: capture thread, attached above.
    let mut pool = match unsafe { GdiPool::new(initial_rect) } {
        Some(pool) => pool,
        None => {
            let _ = ready.send(Err(ComputerUseError::unavailable(
                "cannot allocate the composite capture GDI pool on the hidden desktop",
            )));
            return;
        }
    };
    // SAFETY: pseudo-handle query for the leak-tripwire baseline.
    let mut counts = unsafe { gui_object_counts() };
    let _ = ready.send(Ok(()));
    while let Ok(job) = jobs.recv() {
        let result = (|| {
            // Metrics are re-read EVERY capture: a monitor appended or
            // removed mid-session changes the virtual-screen rect, and the
            // query is a handful of GetSystemMetrics calls. Stale metrics
            // would silently capture the wrong region (or a stale size).
            // SAFETY: cheap metric query on the attached capture thread.
            let rect = unsafe { desktop_virtual_screen_rect() };
            if rect.width <= 0 || rect.height <= 0 {
                return Err(ComputerUseError::unavailable(format!(
                    "the hidden desktop reports a degenerate virtual screen \
                     {}x{} (origin {}, {})",
                    rect.width, rect.height, rect.origin_x, rect.origin_y
                )));
            }
            // SAFETY: capture thread, pool confined to this thread.
            if !unsafe { pool.ensure_composite_rect(rect) } {
                return Err(ComputerUseError::unavailable(
                    "cannot size the composite capture bitmap for the hidden \
                     desktop's current virtual screen",
                ));
            }
            // SAFETY: capture thread, pool confined to this thread.
            unsafe { pool.composite_windows() };
            // SAFETY: capture thread, pool confined to this thread.
            unsafe { pool.read_rgba() }
        })();
        // SAFETY: pseudo-handle query for the leak-tripwire check.
        let current = unsafe { gui_object_counts() };
        if gdi_leak_suspected(counts, current) {
            log::warn!(
                "[computer_use][virtual-win] GDI object counts grew from \
                 {:?} to {current:?} across captures — a handle is leaking in \
                 the composite capture path",
                counts
            );
            counts = current;
        }
        let _ = job.reply.send(result);
    }
    // SAFETY: capture thread, pool confined to this thread.
    unsafe { pool.release() };
    // Restore before exit (best-effort; the association also dies with the
    // thread itself).
    // SAFETY: this thread attached above and owns no windows.
    let _ = unsafe { SetThreadDesktop(previous) };
}

/// Visible top-level windows in BOTTOM-TO-TOP order (painter's algorithm).
/// The Z-walk itself runs TOP-DOWN (`GetTopWindow`, then `GW_HWNDNEXT`
/// toward the bottom: GW_HWNDNEXT is the window BELOW in the Z order,
/// GW_HWNDPREV the window above — above the topmost window there is none,
/// so a PREV walk from `GetTopWindow` would end after a single window and
/// the composite would capture just that one over a black fill) so the
/// `cap` keeps the TOPMOST visible windows when a pathological window farm
/// exceeds it — those are the active windows the screenshot is for; a
/// bottom-first cap silently dropped exactly them. The collected order is
/// reversed before returning, preserving the painter's algorithm the
/// composite expects (bottom blitted first, top last). The generated
/// wrappers answer Err where the raw API returns NULL (end of the Z-walk),
/// so an Err terminates the walk.
///
/// SAFETY: desktop-relative Z-order enumeration from the attached calling
/// thread; the returned handles are valid at return time (windows may still
/// die before use — callers treat failures as churn).
unsafe fn walk_visible_toplevels(cap: usize) -> Vec<HWND> {
    // SAFETY: see the fn contract.
    unsafe {
        let mut out = Vec::new();
        let Ok(top) = GetTopWindow(None) else {
            return out; // empty desktop
        };
        let mut current = top;
        loop {
            if IsWindowVisible(current).as_bool() && !is_cloaked(current) {
                out.push(current);
                if out.len() >= cap {
                    break;
                }
            }
            match GetWindow(current, GW_HWNDNEXT) {
                Ok(previous) => current = previous,
                Err(_) => break,
            }
        }
        out.reverse();
        out
    }
}

/// Whether the window is cloaked (`DWMWA_CLOAKED`): suspended UWP app
/// windows answer `IsWindowVisible` = true while owning no rendered
/// surface, and `PrintWindow` then delivers a blank rectangle into the
/// composite. Best-effort in the safe-for-coverage direction: a DWM
/// failure answers "not cloaked" (the pre-check behavior) rather than
/// skipping a real window.
///
/// SAFETY: DWM attribute query on a desktop window from the attached
/// calling thread.
unsafe fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    // SAFETY: valid out-pointer for the u32 attribute.
    let _ = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    cloaked != 0
}

/// Topmost-to-bottom first visible non-shell window (the keyboard-focus
/// fallback when nothing was ever clicked and there is no foreground).
///
/// SAFETY: desktop-relative Z-order enumeration from the attached calling
/// thread (same contract as [`walk_visible_toplevels`]).
unsafe fn topmost_visible_app_window() -> Option<HWND> {
    // SAFETY: see the fn contract.
    unsafe {
        let Ok(top) = GetTopWindow(None) else {
            return None;
        };
        let mut current = top;
        loop {
            if IsWindowVisible(current).as_bool()
                && !is_cloaked(current)
                && !is_shell_window_class(&window_class_name(current))
            {
                return Some(current);
            }
            match GetWindow(current, GW_HWNDNEXT) {
                Ok(next) => current = next,
                Err(_) => return None,
            }
        }
    }
}

/// Window class name (bounded buffer; empty string on failure).
///
/// SAFETY: bounded-buffer string query on a window handle obtained from
/// desktop-relative enumeration on the attached calling thread.
unsafe fn window_class_name(hwnd: HWND) -> String {
    // SAFETY: see the fn contract.
    unsafe {
        let mut buffer = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut buffer);
        if len <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..(len as usize).min(buffer.len())])
    }
}

/// Top-level ancestor of a window. `GetAncestor(GA_ROOT)` is the documented
/// root walk — deliberately NOT `GetParent`, which "despite its name" also
/// returns the OWNER for an owned top-level window (dialogs, popups, message
/// boxes): an owner resolution sent dialog chords/focus to the modal's owner
/// instead of the modal. GA_ROOT returns the window itself for a top-level.
///
/// SAFETY: ancestor query on desktop windows from the attached calling
/// thread; a NULL answer (invalid input) falls back to the input handle.
unsafe fn top_level_of(hwnd: HWND) -> HWND {
    // SAFETY: ancestor query on a desktop window from the attached calling
    // thread.
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    if root.is_invalid() { hwnd } else { root }
}

// ── backend ─────────────────────────────────────────────────────────────────

/// The hidden-desktop backend. Constructed, used, and dropped on
/// computer_use's dedicated worker thread (BackendHandle contract).
struct WindowsVirtualDesktopBackend {
    /// The hidden desktop handle (zero handle while partially constructed —
    /// every use site guards with `is_invalid`).
    desktop: HDESK,
    desktop_name: String,
    /// The kill-on-close job every launched process lives in.
    job: windows::Win32::Foundation::HANDLE,
    /// Capture thread plumbing (worker → capture thread).
    capture_tx: Option<Sender<CaptureJob>>,
    capture_thread: Option<JoinHandle<()>>,
    /// The worker thread's desktop before attachment (restored at drop).
    worker_original_desktop: Option<HDESK>,
    /// Caller-timeout cancel flag (mirrors the physical backend's
    /// between-event checks in type/drag/hold/click).
    cancel: Option<Arc<AtomicBool>>,
    /// Consecutive capture replies that timed out (the wedge policy in
    /// [`Self::capture_via_thread`]; reset by any successful reply).
    capture_wedges: u32,
}

// SAFETY: the raw OS handles (HDESK / HANDLE / HWND) are kernel objects that
// may legally move between threads; they are only ever PASSED to API calls on
// the worker thread (input/query paths) or the capture thread (the GDI pool,
// which never lives in this struct). The worker-thread-confined field
// (`cancel`) moves with the backend itself, which stays on the worker per the
// BackendHandle contract.
unsafe impl Send for WindowsVirtualDesktopBackend {}

impl WindowsVirtualDesktopBackend {
    /// Full creation sequence (§6): recovery probe → CreateDesktopW →
    /// worker attachment → kill-on-close job → explorer shell → shell
    /// readiness → capture thread. Any failure after the desktop exists
    /// returns Err; the partially-built backend's Drop tears down whatever
    /// was already created (never a half-live desktop).
    fn new() -> Result<Self, ComputerUseError> {
        let sequence = DESKTOP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = desktop_name(sequence);
        let name_wide = to_wide(&name);

        // 1. Name-based orphan recovery (no pidfile on Windows): a desktop
        //    with our name can only be a previous crashed run's.
        // SAFETY: probe + bounded kill/close sequence on our own named desktop.
        if let Ok(leftover) =
            // SAFETY: probe for our own uniquely-named crashed-run leftover.
            unsafe {
                OpenDesktopW(
                    PCWSTR(name_wide.as_ptr()),
                    DESKTOP_CONTROL_FLAGS(0),
                    false,
                    GENERIC_ALL.0,
                )
            }
        {
            // SAFETY: bounded recovery of our own uniquely-named leftover.
            unsafe { recover_leftover_desktop(&name, leftover) }
        } // absent: nothing to recover

        // 2. Fresh desktop (also covers the rare recovery close-busy race:
        //    CreateDesktopW on an existing name opens the existing object).
        // SAFETY: flags/mask per design §6; lpsa None → the desktop gets the
        // creating process's DEFAULT DACL (not the window station's — other
        // same-user processes can still open it by name, which is the model
        // here: isolation by unique naming, not a security boundary).
        let desktop = unsafe {
            CreateDesktopW(
                PCWSTR(name_wide.as_ptr()),
                PCWSTR::null(),
                None,
                DESKTOP_CONTROL_FLAGS(0),
                desktop_access_mask(),
                None,
            )
        }
        .map_err(|error| {
            ComputerUseError::unavailable(format!(
                "cannot create the hidden desktop '{name}': {error}"
            ))
        })?;
        let mut backend = Self {
            desktop,
            desktop_name: name.clone(),
            job: HANDLE(std::ptr::null_mut()),
            capture_tx: None,
            capture_thread: None,
            worker_original_desktop: None,
            cancel: None,
            capture_wedges: 0,
        };

        // 3. Attach the worker thread (this thread — BackendHandle runs the
        //    factory on the dedicated worker, which owns no windows).
        // SAFETY: the worker thread owns no windows/hooks (contract).
        unsafe {
            backend.worker_original_desktop = Some(attach_thread_to_desktop(desktop)?);
        }

        // 4. Kill-on-close job (NO JOB_OBJECT_UILIMIT_* — BSOD caveat).
        // SAFETY: configuration only.
        unsafe {
            backend.job = create_kill_on_close_job()?;
        }

        // 5. Explorer shell (design §6 command shape; explicit
        //    lpApplicationName to avoid command-line parsing ambiguity).
        //    NO folder argument (see explorer_launch_plan: the old
        //    %USERPROFILE% default put the user's home listing into the
        //    first captures).
        let (application, command_line) =
            explorer_launch_plan(std::env::var("WINDIR").ok().as_deref());
        let application_wide = to_wide(&application);
        // SAFETY: contained suspended launch into our job.
        let explorer_pid = unsafe {
            launch_suspended_in_job(
                to_wide(&desktop_target_path(&name)),
                Some(&application_wide),
                &command_line,
                backend.job,
            )
        }
        .map_err(|error| {
            ComputerUseError::unavailable(format!(
                "cannot start the hidden desktop's shell (explorer): {error}"
            ))
        })?;
        log::info!(
            "[computer_use][virtual-win] explorer shell launched on the hidden \
             desktop '{name}' (pid {explorer_pid})"
        );

        // 6. Shell readiness: FindWindowW("Shell_TrayWnd") ≤5 s from THIS
        //    (already attached) thread. A miss is a warning, not a failure —
        //    apps can still be opened and driven; the first capture will
        //    simply show the desktop as it is.
        let shell_deadline = Instant::now() + Duration::from_millis(SHELL_WAIT_MS);
        // SAFETY: desktop-relative FindWindowW from the attached worker thread.
        if unsafe { wait_for_shell_tray(shell_deadline) } {
            log::info!(
                "[computer_use][virtual-win] hidden desktop shell is ready \
                 (Shell_TrayWnd present)"
            );
        } else {
            log::warn!(
                "[computer_use][virtual-win] the hidden desktop's shell did not \
                 signal readiness within {SHELL_WAIT_MS}ms; continuing (apps can \
                 still be opened and driven)"
            );
        }

        // 7. Capture thread + bounded ready handshake.
        let (job_tx, job_rx) = channel::<CaptureJob>();
        let (ready_tx, ready_rx) = channel::<Result<(), ComputerUseError>>();
        let capture_desktop = SendableDesktop(desktop);
        let capture_thread = std::thread::Builder::new()
            .name("computer-use-vd-capture".to_string())
            .spawn(move || capture_thread_main(capture_desktop, job_rx, ready_tx))
            .map_err(|error| {
                ComputerUseError::unavailable(format!(
                    "cannot spawn the hidden desktop capture thread: {error}"
                ))
            })?;
        backend.capture_tx = Some(job_tx);
        backend.capture_thread = Some(capture_thread);
        match ready_rx.recv_timeout(Duration::from_millis(CAPTURE_READY_TIMEOUT_MS)) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(ComputerUseError::unavailable(format!(
                    "the hidden desktop capture thread did not become ready \
                     within {CAPTURE_READY_TIMEOUT_MS}ms"
                )));
            }
        }

        Ok(backend)
    }

    /// Post one window message to a desktop window. PostMessageW fails when
    /// the target is gone (or UIPI refuses) — surfaced, never swallowed.
    fn post(
        &self,
        target: HWND,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> Result<(), ComputerUseError> {
        // SAFETY: message post to a window handle obtained from desktop-
        // relative queries on this attached thread.
        unsafe { PostMessageW(Some(target), message, WPARAM(wparam), LPARAM(lparam)) }.map_err(
            |error| {
                ComputerUseError::failed(format!(
                    "the hidden desktop target rejected a window message \
                     (the window may have closed): {error}"
                ))
            },
        )
    }

    /// The hidden desktop's cursor position (per-desktop input state; this
    /// thread is attached).
    fn cursor_point(&self) -> Result<POINT, ComputerUseError> {
        let mut point = POINT { x: 0, y: 0 };
        // SAFETY: out-pointer write; thread attached.
        unsafe { GetCursorPos(&mut point) }.map_err(|error| {
            ComputerUseError::unavailable(format!(
                "cannot read the hidden desktop's cursor position: {error}"
            ))
        })?;
        Ok(point)
    }

    /// Hit-test: top-level under the screen point (WindowFromPoint, the
    /// z-order-aware counterpart of the walk below), then the
    /// `ChildWindowFromPoint` walk to the deepest child at the point.
    /// Returns (top-level, deepest, client x, client y) — client coordinates
    /// relative to the deepest window. `None` when no window is under the
    /// point. Worker thread only (attached).
    fn hit_test_at(&self, x: i32, y: i32) -> Option<(HWND, HWND, i32, i32)> {
        // SAFETY: desktop-relative queries on the attached worker thread.
        unsafe {
            let top = WindowFromPoint(POINT { x, y });
            if top.is_invalid() {
                return None;
            }
            let mut current = top;
            loop {
                let mut point = POINT { x, y };
                let _ = ScreenToClient(current, &mut point);
                let child = ChildWindowFromPoint(current, point);
                if child.is_invalid() || child.0 == current.0 {
                    return Some((top, current, point.x, point.y));
                }
                current = child;
            }
        }
    }

    /// The focus dance (rustyvnc recipe): attach the input queues, set the
    /// foreground top-level + focus, read the queue's real focused window
    /// while attached, detach. Returns the window keyboard input should
    /// target (the focused control, or the fallback). Worker thread only.
    fn focus_dance(&self, target: HWND) -> HWND {
        // SAFETY: queue attach + focus set on desktop windows from the
        // attached worker thread; the detach always runs.
        unsafe {
            let top = top_level_of(target);
            let ours = GetCurrentThreadId();
            let theirs = GetWindowThreadProcessId(top, None);
            let attached = if ours != theirs && theirs != 0 {
                AttachThreadInput(ours, theirs, true).as_bool()
            } else {
                false
            };
            let _ = SetForegroundWindow(top);
            let _ = SetFocus(Some(target));
            let focused = GetFocus();
            let effective = if focused.is_invalid() {
                target
            } else {
                focused
            };
            if attached {
                let _ = AttachThreadInput(ours, theirs, false);
            }
            effective
        }
    }

    /// The keyboard target for type/chord paths: where REAL keyboard input
    /// would land right now. The foreground window's own focused control is
    /// read via `GetGUIThreadInfo` WITHOUT stealing focus first — classic
    /// frame+child apps keep their focus on a child control, and a posted
    /// `WM_CHAR` must reach that control, not the frame (a read that runs
    /// `SetFocus(frame)` first would answer the frame and displace the
    /// app's focus, which is exactly what a real keystroke never does).
    /// When the GUI-thread info yields nothing (no focused window on the
    /// target thread), the legacy dance answers the fallback. Errors
    /// explicitly when the desktop has no drivable window at all.
    fn keyboard_target(&self) -> Result<HWND, ComputerUseError> {
        let base = {
            // SAFETY: desktop-relative queries on the attached worker.
            let foreground = unsafe { GetForegroundWindow() };
            if !foreground.is_invalid() {
                foreground
            } else {
                // SAFETY: same.
                unsafe { topmost_visible_app_window() }.ok_or_else(|| {
                    ComputerUseError::unavailable(
                        "no window on the hidden desktop has keyboard focus; \
                         click a window (or open an app with the open action) \
                         before typing",
                    )
                })?
            }
        };
        // SAFETY: queue attach + GUI-thread info on desktop windows from the
        // attached worker thread; the detach always runs.
        unsafe {
            let top = top_level_of(base);
            let ours = GetCurrentThreadId();
            let theirs = GetWindowThreadProcessId(top, None);
            let attached = if ours != theirs && theirs != 0 {
                AttachThreadInput(ours, theirs, true).as_bool()
            } else {
                false
            };
            let mut info = GUITHREADINFO::default();
            info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
            let real_focus =
                if GetGUIThreadInfo(theirs, &mut info).is_ok() && !info.hwndFocus.is_invalid() {
                    Some(info.hwndFocus)
                } else {
                    None
                };
            if attached {
                let _ = AttachThreadInput(ours, theirs, false);
            }
            match real_focus {
                Some(focused) => Ok(focused),
                // No focused window on the target thread: fall back to the
                // click-path dance (foreground + focus set), which at least
                // puts the frame on record.
                None => Ok(self.focus_dance(base)),
            }
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    /// Press the chord on the keyboard target's top-level window (accelerator
    /// messages must reach the top-level), hold for `hold`, release in
    /// reverse. Modifiers first (§6); the whole chord is mapped before
    /// anything is pressed (a mapping failure presses nothing).
    fn chord_to(&mut self, keys: &[Key], hold: Duration) -> Result<(), ComputerUseError> {
        if keys.is_empty() {
            return Err(ComputerUseError::failed("key chord is empty"));
        }
        // Modifiers lead, the rest keep the given order (§6, shared with the
        // pin via partition_modifiers_first).
        let (modifiers, rest) = partition_modifiers_first(keys);
        let ordered: Vec<u16> = modifiers
            .iter()
            .chain(rest.iter())
            .map(|key| key_to_vk(**key))
            .collect::<Result<_, _>>()?;
        let target = self.keyboard_target()?;
        // SAFETY: parent-walk on a desktop window from the attached worker.
        let top = unsafe { top_level_of(target) };
        let mut pressed: Vec<u16> = Vec::with_capacity(ordered.len());
        let hold_result = (|| {
            for vk in &ordered {
                // Real Alt input travels as WM_SYSKEYDOWN/WM_SYSKEYUP (the
                // system key pair); posting plain WM_KEYDOWN(VK_MENU) made
                // Alt chords register as ordinary key presses and Alt+F4 /
                // Alt-menu semantics never fired. Ctrl/Shift chords keep the
                // plain pair — the system pair is Alt-specific.
                let down = if *vk == VK_MENU.0 {
                    WM_SYSKEYDOWN
                } else {
                    WM_KEYDOWN
                };
                self.post(top, down, usize::from(*vk), key_lparam(false))?;
                pressed.push(*vk);
            }
            let mut remaining = hold;
            while remaining > Duration::ZERO {
                if self.cancelled() {
                    // The base verdict carries only the cancel fact; the
                    // combine below appends the truthful release outcome.
                    return Err(ComputerUseError::unavailable(
                        "hold was cancelled (caller timeout or stop)",
                    ));
                }
                let step = remaining.min(Duration::from_millis(HOLD_CANCEL_POLL_MS));
                sleep(step);
                remaining -= step;
            }
            Ok(())
        })();
        // Release in reverse regardless of how the hold went (a cancelled
        // hold must not strand modifiers).
        let mut release = Ok(());
        for vk in pressed.iter().rev() {
            let up = if *vk == VK_MENU.0 {
                WM_SYSKEYUP
            } else {
                WM_KEYUP
            };
            if let Err(error) = self.post(top, up, usize::from(*vk), key_lparam(true)) {
                if release.is_ok() {
                    release = Err(error);
                }
            }
        }
        // Merge truthfully (same helper as the Linux hold path): the old
        // inline merge kept only the hold error, so a cancel plus a failed
        // release reported "the keys have been released" while the release
        // had actually failed — stranded keys undisclosed.
        combine_hold_errors(hold_result, release)
    }

    /// Route a capture through the capture thread.
    ///
    /// The reply wait is bounded ([`CAPTURE_REPLY_TIMEOUT_MS`]): a thread
    /// wedged inside a hung `RedrawWindow`/`PrintWindow` must surface as an
    /// explicit error, not park the caller until the handle layer's larger
    /// budget expires. After [`CAPTURE_WEDGE_LIMIT`] consecutive timeouts the
    /// capture lane is disabled for the rest of the session (honest
    /// degradation — later captures answer Unavailable immediately instead of
    /// re-hanging): the sender is dropped, which disconnects the job channel,
    /// and the thread releases its GDI pool and detaches on its own whenever
    /// it un-wedges (it is never joined or killed from here — teardown
    /// policy in `Drop`). One successful reply resets the counter (a single
    /// slow frame is churn, not a wedge).
    fn capture_via_thread(&mut self) -> Result<Capture, ComputerUseError> {
        let Some(sender) = self.capture_tx.as_ref() else {
            return Err(ComputerUseError::unavailable(
                "the hidden desktop capture thread is gone or was disabled \
                 for this session (it stopped answering captures)",
            ));
        };
        let (reply_tx, reply_rx) = channel::<Result<Capture, ComputerUseError>>();
        sender.send(CaptureJob { reply: reply_tx }).map_err(|_| {
            ComputerUseError::unavailable("the hidden desktop capture thread exited unexpectedly")
        })?;
        // The reply channel is per-request so a stale reply can never be
        // attributed to a later request.
        match reply_rx.recv_timeout(Duration::from_millis(CAPTURE_REPLY_TIMEOUT_MS)) {
            Ok(result) => {
                self.capture_wedges = 0;
                result
            }
            Err(RecvTimeoutError::Timeout) => {
                self.capture_wedges += 1;
                if self.capture_wedges >= CAPTURE_WEDGE_LIMIT {
                    log::warn!(
                        "[computer_use][virtual-win] the capture thread did not \
                         answer within {CAPTURE_REPLY_TIMEOUT_MS}ms \
                         {CAPTURE_WEDGE_LIMIT} times in a row; captures are \
                         unavailable for the rest of the session"
                    );
                    // Drop the sender: the capture thread's jobs.recv()
                    // disconnects and it cleans up on its own; later
                    // captures take the early Unavailable path above.
                    self.capture_tx = None;
                }
                Err(ComputerUseError::failed(format!(
                    "the hidden desktop capture thread wedged (no reply within \
                     {CAPTURE_REPLY_TIMEOUT_MS}ms)"
                )))
            }
            Err(RecvTimeoutError::Disconnected) => Err(ComputerUseError::unavailable(
                "the hidden desktop capture thread died before answering",
            )),
        }
    }

    /// Launch an application on the hidden desktop, contained in the session
    /// job. Returns the child pid.
    fn launch_in_job(
        &self,
        application: Option<&str>,
        command_line: &str,
    ) -> Result<u32, ComputerUseError> {
        if self.job.is_invalid() {
            return Err(ComputerUseError::unavailable(
                "the hidden desktop session job object is gone",
            ));
        }
        // SAFETY: contained suspended launch into our own job.
        unsafe {
            launch_suspended_in_job(
                to_wide(&desktop_target_path(&self.desktop_name)),
                application.map(|path| to_wide(path)).as_deref(),
                command_line,
                self.job,
            )
        }
    }
}

impl ComputerUseBackend for WindowsVirtualDesktopBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            screenshot: true,
            input: true,
            ui_tree: false,
            notes: "windows hidden desktop: input is app-level message injection \
                    (PostMessageW) — apps that ignore synthetic messages cannot be \
                    driven, title-bar/system-button (non-client) interactions are \
                    not injected, posted chords do not update the app's async \
                    key-state table (apps that read modifier state from \
                    GetKeyState may see unmodified keys), and layered-window \
                    translucency can print as opaque fringes (the hidden desktop \
                    has no DWM to blend it); capture and input span the session's \
                    whole virtual screen (every monitor of the hidden-desktop \
                    session); capture is a per-window composite (a hidden desktop \
                    has no DWM), so content can lag until the forced redraw \
                    lands; verify actions with a follow-up screenshot"
                .to_string(),
            target: DisplayTarget::Virtual,
            target_note: "isolated hidden Windows desktop — the user's desktop is \
                          untouched; keyboard/mouse reach apps as window messages and \
                          some apps ignore synthetic messages; screenshots are \
                          per-window composites (no DWM on a hidden desktop) covering \
                          the session's whole virtual screen (all monitors); UAC \
                          elevation prompts appear on the secure desktop and cannot \
                          be driven; stopping the session does not wind the hidden \
                          desktop down mid-session — its desktop and apps remain \
                          until the backend is dropped at session end"
                .to_string(),
        }
    }

    fn capture(&mut self) -> Result<Capture, ComputerUseError> {
        self.capture_via_thread()
    }

    fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
        let point = self.cursor_point()?;
        Ok((point.x, point.y))
    }

    fn move_to(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        // The per-desktop cursor is what click/scroll hit-test against; the
        // WM_MOUSEMOVE post additionally feeds hover states.
        // SAFETY: per-desktop cursor move on the attached worker thread.
        unsafe { SetCursorPos(x, y) }.map_err(|error| {
            ComputerUseError::failed(format!("cannot move the hidden desktop's cursor: {error}"))
        })?;
        let Some((_, target, client_x, client_y)) = self.hit_test_at(x, y) else {
            return Ok(()); // cursor moved; nothing to notify
        };
        self.post(
            target,
            WM_MOUSEMOVE,
            0,
            pack_mouse_lparam(client_x, client_y),
        )
    }

    fn click(&mut self, button: MouseButton, count: u8) -> Result<(), ComputerUseError> {
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        let point = self.cursor_point()?;
        let Some((_, target, client_x, client_y)) = self.hit_test_at(point.x, point.y) else {
            return Err(ComputerUseError::unavailable(
                "there is no window under the cursor on the hidden desktop; \
                 move_to a window first",
            ));
        };
        let (down, double, up, mark) = mouse_button_messages(button);
        let lparam = pack_mouse_lparam(client_x, client_y);
        // SAFETY: class-style query on the hit-tested desktop window.
        let accepts_dblclk = unsafe { window_class_accepts_dblclk(target) };
        for index in 0..count.max(1) {
            if self.cancelled() {
                return Err(ComputerUseError::unavailable(
                    "click was cancelled (caller timeout or stop); already \
                     injected clicks are not undone",
                ));
            }
            if index == 0 {
                // Real input focuses the clicked control; dance on the first
                // press only (subsequent clicks of a double/triple must not
                // re-focus and steal the first click's effect).
                self.focus_dance(target);
            }
            // The double-click message alternates with the plain down the
            // way the OS generates real multi-clicks: press 1 = down,
            // press 2 = DBLCLK, press 3 = down, press 4 = DBLCLK (a
            // quadruple click is down/dblclk/down/dblclk — the OS re-arms
            // the double-click detector on every odd press, it does not
            // revert to plain downs after the second) — but only for
            // classes that registered CS_DBLCLKS; others get another plain
            // down (see multi_click_message).
            let message = multi_click_message(index, down, double, accepts_dblclk);
            self.post(target, message, mark, lparam)?;
            // A failed UP must surface the stranded-button fact like the
            // macOS click path does — "click failed" alone reads as if the
            // button state is clean.
            if let Err(error) = self.post(target, up, 0, lparam) {
                return Err(ComputerUseError::failed(format!(
                    "{error}; the button may still be pressed"
                )));
            }
            if index + 1 < count.max(1) {
                sleep(Duration::from_millis(MULTI_CLICK_INTERVAL_MS));
            }
        }
        Ok(())
    }

    fn mouse_down(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        let point = self.cursor_point()?;
        let Some((_, target, client_x, client_y)) = self.hit_test_at(point.x, point.y) else {
            return Err(ComputerUseError::unavailable(
                "there is no window under the cursor on the hidden desktop",
            ));
        };
        let (down, _, _, mark) = mouse_button_messages(button);
        self.focus_dance(target);
        self.post(target, down, mark, pack_mouse_lparam(client_x, client_y))
    }

    fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        let point = self.cursor_point()?;
        let Some((_, target, client_x, client_y)) = self.hit_test_at(point.x, point.y) else {
            // An up with no window under the cursor is a harmless no-op —
            // the emergency release path relies on this being non-fatal.
            return Ok(());
        };
        let (_, _, up, _) = mouse_button_messages(button);
        self.post(target, up, 0, pack_mouse_lparam(client_x, client_y))
    }

    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError> {
        self.move_to(from.0, from.1)?;
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        let Some((_, target, client_x, client_y)) = self.hit_test_at(from.0, from.1) else {
            return Err(ComputerUseError::unavailable(
                "there is no window at the drag start point on the hidden desktop",
            ));
        };
        self.focus_dance(target);
        self.post(
            target,
            WM_LBUTTONDOWN,
            MK_LBUTTON,
            pack_mouse_lparam(client_x, client_y),
        )?;
        sleep(Duration::from_millis(DRAG_PRESS_SETTLE_MS));
        let move_result = (|| {
            for (x, y) in drag_waypoints(from, to, DRAG_STEPS) {
                if self.cancelled() {
                    return Err(ComputerUseError::unavailable(
                        "drag was cancelled (caller timeout or stop); the button \
                         is released and the pointer stays at the last waypoint",
                    ));
                }
                // SAFETY: per-desktop cursor move on the attached worker.
                unsafe { SetCursorPos(x, y) }.map_err(|error| {
                    ComputerUseError::failed(format!(
                        "cannot move the hidden desktop's cursor during drag: {error}"
                    ))
                })?;
                if let Some((_, move_target, move_x, move_y)) = self.hit_test_at(x, y) {
                    // A held-button move carries the pressed mark, exactly
                    // like real input.
                    self.post(
                        move_target,
                        WM_MOUSEMOVE,
                        MK_LBUTTON,
                        pack_mouse_lparam(move_x, move_y),
                    )?;
                }
                sleep(Duration::from_millis(DRAG_STEP_DELAY_MS));
            }
            Ok(())
        })();
        // The button must be released regardless of whether the path
        // movement errored.
        // SAFETY: per-desktop cursor move on the attached worker.
        unsafe {
            let _ = SetCursorPos(to.0, to.1);
        }
        let release = match self.hit_test_at(to.0, to.1) {
            Some((_, target, client_x, client_y)) => self.post(
                target,
                WM_LBUTTONUP,
                0,
                pack_mouse_lparam(client_x, client_y),
            ),
            None => Err(ComputerUseError::unavailable(
                "no window at the drag end point; the button-up was not delivered",
            )),
        };
        combine_drag_errors(move_result, release)
    }

    fn scroll(&mut self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError> {
        let clicks = clicks.min(MAX_SCROLL_CLICKS);
        let point = self.cursor_point()?;
        let Some((_, target, _, _)) = self.hit_test_at(point.x, point.y) else {
            return Err(ComputerUseError::unavailable(
                "there is no window under the cursor on the hidden desktop",
            ));
        };
        let (message, delta) = wheel_message_and_delta(direction);
        // WM_MOUSEWHEEL/WM_MOUSEHWHEEL lParam carries SCREEN coordinates
        // (unlike the button messages' client coordinates).
        let lparam = pack_mouse_lparam(point.x, point.y);
        for _ in 0..clicks {
            self.post(target, message, wheel_wparam(delta), lparam)?;
        }
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), ComputerUseError> {
        let text = normalize_typed_newlines(text);
        let target = self.keyboard_target()?;
        for run in split_type_runs(&text, TYPE_CHUNK_CHARS) {
            if self.cancelled() {
                return Err(ComputerUseError::unavailable(
                    "type text was cancelled (caller timeout or stop); \
                     characters already injected are not undone",
                ));
            }
            // Return/Tab runs ride the SAME WM_CHAR-only lane as the
            // printable path below, with the payload real input synthesizes
            // ('\r' / '\t' — see [`key_run_char`]). Wrapping them in the
            // WM_KEYDOWN/WM_CHAR/WM_KEYUP triple would make the target's own
            // TranslateMessage turn the key-down into a SECOND WM_CHAR,
            // doubling every newline/tab in any standard GUI app.
            if let Some(unit) = key_run_char(&run) {
                self.post(target, WM_CHAR, usize::from(unit), key_lparam(false))?;
                continue;
            }
            let TypeRun::Text(chunk) = run else {
                continue; // key_run_char answered every non-text run above
            };
            for character in chunk.chars() {
                if self.cancelled() {
                    return Err(ComputerUseError::unavailable(
                        "type text was cancelled (caller timeout or stop); \
                         characters already injected are not undone",
                    ));
                }
                // Printable characters post ONLY WM_CHAR (per the rustyvnc
                // finding: also posting WM_KEYDOWN makes the target's
                // TranslateMessage generate a second WM_CHAR, doubling
                // input). Surrogate pairs post both units.
                let (first, second) = char_utf16_units(character);
                self.post(target, WM_CHAR, usize::from(first), key_lparam(false))?;
                if let Some(unit) = second {
                    self.post(target, WM_CHAR, usize::from(unit), key_lparam(false))?;
                }
            }
        }
        Ok(())
    }

    fn key_chord(&mut self, keys: &[Key]) -> Result<(), ComputerUseError> {
        self.chord_to(keys, Duration::from_millis(CHORD_HOLD_MS))
    }

    fn hold_key(&mut self, keys: &[Key], ms: u64) -> Result<(), ComputerUseError> {
        self.chord_to(keys, Duration::from_millis(ms))
    }

    fn ui_tree(&mut self, _opts: &UiTreeOptions) -> Result<String, ComputerUseError> {
        Err(ComputerUseError::unsupported(
            "ui_tree",
            "the hidden-desktop virtual display cannot query UI Automation: \
             cross-desktop UIA needs an attached helper process, which is not \
             implemented in this release; use screenshots and coordinate-based \
             actions instead",
        ))
    }

    fn element_at_point(
        &mut self,
        _x: i32,
        _y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError> {
        Err(ComputerUseError::unsupported(
            "element_at_point",
            "the hidden-desktop virtual display cannot query UI Automation: \
             cross-desktop UIA needs an attached helper process, which is not \
             implemented in this release; use screenshots and coordinate-based \
             actions instead",
        ))
    }

    fn focused_element(&mut self) -> Result<Option<ElementInfo>, ComputerUseError> {
        Err(ComputerUseError::unsupported(
            "focused_element",
            "the hidden-desktop virtual display cannot query UI Automation: \
             cross-desktop UIA needs an attached helper process, which is not \
             implemented in this release",
        ))
    }

    fn set_cancel_flag(&mut self, flag: Option<Arc<AtomicBool>>) {
        self.cancel = flag;
    }

    fn open_app(&mut self, program: &str, args: &[String]) -> Result<u32, ComputerUseError> {
        validate_open_request(program, args)?;
        let command_line = build_command_line(program, args);
        let pid = self.launch_in_job(None, &command_line)?;
        log::info!(
            "[computer_use][virtual-win] opened '{program}' on the hidden \
             desktop (pid {pid})"
        );
        Ok(pid)
    }
}

impl Drop for WindowsVirtualDesktopBackend {
    fn drop(&mut self) {
        // 1. Kill everything the session launched (shell included). A failed
        //    terminate is logged: the kill-on-close flag on the job handle
        //    below still fires when the last handle closes.
        if !self.job.is_invalid() {
            // SAFETY: job termination of our own session job.
            if let Err(error) = unsafe { TerminateJobObject(self.job, 1) } {
                log::warn!(
                    "[computer_use][virtual-win] cannot terminate the session \
                     job: {error}"
                );
            }
            // 2. Bounded wait until the job is EMPTY — by POLLING the job's
            //    accounting counter. A job handle is only signaled when a
            //    JOB_OBJECT_END_OF_JOB_TIME time limit expires; it NEVER
            //    signals merely because its last process exited (so the
            //    previous WaitForSingleObject here always burned the whole
            //    budget while confirming nothing — Raymond Chen, "How do I
            //    wait until all processes in a job have exited?").
            //    TerminateJobObject initiates termination asynchronously, so
            //    the counter is the only way to confirm the processes really
            //    finished before the desktop below them is closed.
            let mut job_emptied = false;
            let deadline = Instant::now() + Duration::from_millis(u64::from(TEARDOWN_JOB_WAIT_MS));
            while Instant::now() < deadline {
                let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
                // SAFETY: accounting query on our own job handle; the info
                // pointer is `ptr::from_mut().cast()` sized to the struct.
                let queried = unsafe {
                    QueryInformationJobObject(
                        Some(self.job),
                        JobObjectBasicAccountingInformation,
                        std::ptr::from_mut(&mut accounting).cast::<core::ffi::c_void>(),
                        core::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                        None,
                    )
                };
                if let Err(error) = queried {
                    log::warn!(
                        "[computer_use][virtual-win] cannot read the session \
                         job's process count: {error}"
                    );
                    break;
                }
                if accounting.ActiveProcesses == 0 {
                    job_emptied = true;
                    break;
                }
                sleep(Duration::from_millis(TEARDOWN_JOB_POLL_MS));
            }
            if !job_emptied {
                log::warn!(
                    "[computer_use][virtual-win] session job processes did not \
                     all exit within {TEARDOWN_JOB_WAIT_MS}ms; continuing \
                     teardown (the job's kill-on-close flag still reclaims \
                     them when the handle closes)"
                );
            }
        }
        // 3. Capture thread: disconnect the channel, give it a bounded join
        //    (a thread wedged inside a PrintWindow/RedrawWindow to a dying
        //    window detaches and exits on its own rather than hanging the
        //    worker's teardown — same policy as BackendHandle's detach).
        drop(self.capture_tx.take());
        if let Some(thread) = self.capture_thread.take() {
            let deadline = Instant::now() + Duration::from_millis(TEARDOWN_CAPTURE_JOIN_MS);
            while !thread.is_finished() && Instant::now() < deadline {
                sleep(Duration::from_millis(10));
            }
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                log::warn!(
                    "[computer_use][virtual-win] the capture thread is still \
                     busy; leaving it to exit on its own"
                );
                drop(thread);
            }
        }
        // 4. Restore the worker thread's original desktop — only when this
        //    really is the attached worker thread (the association of a dead
        //    thread dies with it anyway; restore is hygiene, never a hard
        //    failure).
        if let Some(previous) = self.worker_original_desktop.take() {
            // SAFETY: read + restore of this thread's desktop association.
            unsafe {
                if let Ok(current) = GetThreadDesktop(GetCurrentThreadId()) {
                    if current.0 == self.desktop.0
                        && let Err(error) = SetThreadDesktop(previous)
                    {
                        log::warn!(
                            "[computer_use][virtual-win] cannot restore the worker \
                             thread's original desktop: {error}"
                        );
                    }
                }
            }
        }
        // 5. Close the desktop. ERROR_BUSY is tolerated by design (§6): the
        //    named desktop is recovered at the next start.
        if !self.desktop.is_invalid() {
            // SAFETY: close of our own desktop handle.
            if let Err(error) = unsafe { CloseDesktop(self.desktop) } {
                log_desktop_close_error(
                    &format!("cannot close the hidden desktop '{}'", self.desktop_name),
                    error,
                );
            }
        }
        // 6. Release the job handle (kill-on-close already ran; the close
        //    just frees the kernel object).
        if !self.job.is_invalid() {
            // SAFETY: close of our own job handle.
            unsafe {
                let _ = CloseHandle(self.job);
            }
        }
    }
}

/// Wave-2 entry points (signatures fixed by the shared dispatch in
/// `super::virtual_display`).
pub(crate) fn create_backend() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    Ok(Box::new(WindowsVirtualDesktopBackend::new()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe answers Available and the one-time dispatch therefore
    /// selects the virtual target when preferred (design §6: always
    /// available on Windows — the API set has no external dependency).
    /// Replaces the Wave-1 stub pin, which pinned the opposite.
    #[test]
    fn probe_answers_available_and_dispatch_selects_virtual() {
        let availability = probe();
        assert_eq!(availability, VirtualAvailability::Available);
        assert!(
            availability.is_available(),
            "the Windows probe must answer Available: {availability:?}"
        );
        assert_eq!(
            super::super::resolve_target(true, &availability),
            super::super::TargetDecision::Virtual,
            "pref on + green probe must select the virtual target on Windows"
        );
        assert_eq!(
            super::super::resolve_target(false, &availability),
            super::super::TargetDecision::Physical,
            "pref off must stay physical"
        );
    }

    /// The desktop access mask is the narrow design §6 set — deliberately
    /// NOT GENERIC_ALL (byte-exact pin: 2|4|1|128|64).
    #[test]
    fn desktop_access_mask_is_the_narrow_design_set() {
        assert_eq!(
            desktop_access_mask(),
            DESKTOP_CREATEWINDOW.0
                | DESKTOP_CREATEMENU.0
                | DESKTOP_READOBJECTS.0
                | DESKTOP_WRITEOBJECTS.0
                | DESKTOP_ENUMERATE.0
        );
        assert_eq!(desktop_access_mask(), 199u32);
        assert_ne!(desktop_access_mask(), GENERIC_ALL.0);
    }

    /// Desktop names: fixed prefix, pid-unique, sequence-unique, short, and
    /// restricted to the recovery-safe charset.
    #[test]
    fn desktop_names_are_prefixed_unique_and_short() {
        let first = desktop_name(0);
        let second = desktop_name(1);
        assert!(first.starts_with(DESKTOP_NAME_PREFIX));
        assert!(second.starts_with(DESKTOP_NAME_PREFIX));
        assert_ne!(first, second, "sequence must distinguish names");
        assert!(first.len() <= 63, "desktop names stay short: {first}");
        assert!(
            first
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "recovery-safe charset only: {first}"
        );
    }

    /// The `lpDesktop` value targets the hidden desktop inside WinSta0.
    #[test]
    fn desktop_target_path_is_window_station_qualified() {
        assert_eq!(
            desktop_target_path("PinvouAgentDesk-1-0"),
            "WinSta0\\PinvouAgentDesk-1-0"
        );
    }

    /// NUL-terminated UTF-16 (CreateProcessW requires the terminator, and
    /// its command-line buffer is written to, hence `to_wide` returning an
    /// owned Vec).
    #[test]
    fn to_wide_is_nul_terminated_utf16() {
        let wide = to_wide("ab");
        assert_eq!(wide, vec![b'a' as u16, b'b' as u16, 0]);
        assert!(wide.last() == Some(&0));
    }

    /// The exhaustive Key → VK table (compile-enforced: `key_to_vk` has no
    /// wildcard arm, so a new `Key` variant breaks this file's build until
    /// mapped; the value pins below keep the mapping itself honest).
    /// WINDOWS-CI-ONLY (this whole file is cfg(windows)).
    #[test]
    fn key_to_vk_maps_every_named_key_and_function_range() {
        let named: &[(Key, u16)] = &[
            (Key::Control, VK_CONTROL.0),
            (Key::Alt, VK_MENU.0),
            (Key::Shift, VK_SHIFT.0),
            (Key::Meta, VK_LWIN.0),
            (Key::Enter, VK_RETURN.0),
            (Key::Escape, VK_ESCAPE.0),
            (Key::Tab, VK_TAB.0),
            (Key::Space, VK_SPACE.0),
            (Key::Backspace, VK_BACK.0),
            (Key::Delete, VK_DELETE.0),
            (Key::Insert, VK_INSERT.0),
            (Key::Up, VK_UP.0),
            (Key::Down, VK_DOWN.0),
            (Key::Left, VK_LEFT.0),
            (Key::Right, VK_RIGHT.0),
            (Key::Home, VK_HOME.0),
            (Key::End, VK_END.0),
            (Key::PageUp, VK_PRIOR.0),
            (Key::PageDown, VK_NEXT.0),
        ];
        for (key, vk) in named {
            assert_eq!(key_to_vk(*key).ok(), Some(*vk), "mapping {key:?}");
        }
        // F1..F12 are the contiguous VK_F1 block; 0 and 13 refuse.
        assert_eq!(key_to_vk(Key::Function(1)).ok(), Some(VK_F1.0));
        // F12 is the last of the contiguous VK_F1 block (0x70 + 11).
        assert_eq!(
            key_to_vk(Key::Function(12)).ok(),
            Some(VK_F1.0 + 11),
            "the F-key block must stay contiguous"
        );
        for n in 1..=12u8 {
            assert!(key_to_vk(Key::Function(n)).is_ok(), "f{n} must map");
        }
        assert!(key_to_vk(Key::Function(0)).is_err());
        assert!(key_to_vk(Key::Function(13)).is_err());
    }

    /// The char table: letters/digits equal their ASCII codes (any case),
    /// punctuation maps to OEM VKs, space maps to VK_SPACE, and non-key
    /// characters refuse explicitly (never silently mapped).
    #[test]
    fn char_to_vk_covers_the_us_layout_key_positions() {
        assert_eq!(char_to_vk('a').ok(), Some(0x41));
        assert_eq!(char_to_vk('A').ok(), Some(0x41));
        assert_eq!(char_to_vk('z').ok(), Some(0x5A));
        assert_eq!(char_to_vk('0').ok(), Some(0x30));
        assert_eq!(char_to_vk('9').ok(), Some(0x39));
        assert_eq!(char_to_vk(' ').ok(), Some(VK_SPACE.0));
        assert_eq!(char_to_vk('+').ok(), Some(VK_OEM_PLUS.0));
        assert_eq!(char_to_vk(',').ok(), Some(VK_OEM_COMMA.0));
        assert_eq!(char_to_vk('-').ok(), Some(VK_OEM_MINUS.0));
        assert_eq!(char_to_vk('.').ok(), Some(VK_OEM_PERIOD.0));
        assert_eq!(char_to_vk('/').ok(), Some(VK_OEM_2.0));
        assert_eq!(char_to_vk(';').ok(), Some(VK_OEM_1.0));
        assert_eq!(char_to_vk('`').ok(), Some(VK_OEM_3.0));
        assert_eq!(char_to_vk('[').ok(), Some(VK_OEM_4.0));
        assert_eq!(char_to_vk('\\').ok(), Some(VK_OEM_5.0));
        assert_eq!(char_to_vk(']').ok(), Some(VK_OEM_6.0));
        assert_eq!(char_to_vk('\'').ok(), Some(VK_OEM_7.0));
        // Refusals: the error text must not echo the character (it can
        // carry content the audit log must not leak).
        for c in ['中', 'é', '\u{7f}'] {
            let error = match char_to_vk(c) {
                Err(error) => error.to_string(),
                Ok(_) => panic!("non-key character {c:?} must refuse"),
            };
            assert!(
                error.contains("type action"),
                "refusal must redirect to the type action: {error}"
            );
        }
    }

    /// The chord-ordering partition runs through the IMPLEMENTATION's shared
    /// helper (`partition_modifiers_first`, the fn `chord_to` itself uses):
    /// modifiers lead, plain keys keep their relative order (§6 "modifiers
    /// sent first"). Driving the impl fn keeps a regression that removes the
    /// partition from `chord_to` from passing silently.
    #[test]
    fn modifier_partition_orders_modifiers_first() {
        let keys = [Key::Char('s'), Key::Control, Key::Char('a'), Key::Shift];
        let (modifiers, rest) = partition_modifiers_first(&keys);
        let ordered: Vec<Key> = modifiers.into_iter().chain(rest).copied().collect();
        assert_eq!(
            ordered,
            vec![Key::Control, Key::Shift, Key::Char('s'), Key::Char('a')]
        );
        // The empty edge stays empty (chord_to rejects empty before this).
        let (modifiers, rest) = partition_modifiers_first(&[]);
        assert!(modifiers.is_empty() && rest.is_empty());
    }

    /// Key-message lParam shapes: down = repeat 1; up = repeat 1 with
    /// previous-state (bit 30) + transition (bit 31) — what TranslateMessage
    /// generates for real input.
    #[test]
    fn key_lparam_encodes_down_and_up_bit_shapes() {
        assert_eq!(key_lparam(false), 1);
        let up = key_lparam(true) as u32;
        assert_eq!(up & 1, 1, "repeat count stays 1");
        assert_eq!(up >> 30 & 1, 1, "previous key state must be set on up");
        assert_eq!(up >> 31 & 1, 1, "transition flag must be set on up");
    }

    /// Return/Tab type runs deliver WM_CHAR with the payload real input
    /// synthesizes ('\r' = 0x0D for Return, '\t' = 0x09 for Tab) — and
    /// nothing else: they must NEVER ride a WM_KEYDOWN/WM_KEYUP triple (the
    /// target's own TranslateMessage would turn the key-down into a second
    /// WM_CHAR, doubling newlines/tabs). Text runs carry no key payload.
    /// WINDOWS-CI-ONLY (this whole file is cfg(windows)).
    #[test]
    fn key_runs_deliver_the_real_input_char_and_nothing_else() {
        assert_eq!(key_run_char(&TypeRun::Return), Some(0x0D));
        assert_eq!(key_run_char(&TypeRun::Tab), Some(0x09));
        assert_eq!(
            key_run_char(&TypeRun::Text("abc".into())),
            None,
            "text runs post per-character WM_CHARs, not a key payload"
        );
    }

    /// Mouse lParam packing round-trips, including negative client
    /// coordinates (windows partially off-screen produce them).
    #[test]
    fn mouse_lparam_packing_round_trips_negative_coords() {
        for (x, y) in [
            (0, 0),
            (10, 20),
            (-1, 5),
            (7, -3),
            (-9, -9),
            (i16::MAX as i32, i16::MIN as i32),
        ] {
            let packed = pack_mouse_lparam(x, y);
            assert_eq!(unpack_mouse_lparam(packed), (x, y), "packing ({x},{y})");
        }
    }

    /// Wheel table: up/away is the positive WM_MOUSEWHEEL delta, down is
    /// negative; horizontal uses WM_MOUSEHWHEEL with right positive — the
    /// same sign conventions `map_scroll` encodes for the physical backend.
    #[test]
    fn wheel_messages_and_deltas_follow_the_scroll_direction_table() {
        assert_eq!(
            wheel_message_and_delta(ScrollDirection::Up),
            (WM_MOUSEWHEEL, 120)
        );
        assert_eq!(
            wheel_message_and_delta(ScrollDirection::Down),
            (WM_MOUSEWHEEL, -120)
        );
        assert_eq!(
            wheel_message_and_delta(ScrollDirection::Left),
            (WM_MOUSEHWHEEL, -120)
        );
        assert_eq!(
            wheel_message_and_delta(ScrollDirection::Right),
            (WM_MOUSEHWHEEL, 120)
        );
        // Delta rides the wParam high word (negative deltas truncate to the
        // u16 two's-complement pattern, exactly how targets decode it).
        assert_eq!(wheel_wparam(120), 120 << 16);
        assert_eq!(
            wheel_wparam(-120),
            (((-120i32) as u16 as u32) << 16) as usize
        );
        assert_eq!(wheel_wparam(-120), (0xFF88u32 << 16) as usize);
    }

    /// The multi-click message sequence mirrors real input: the
    /// double-click message ALTERNATES with the plain down (press 2 and 4
    /// are DBLCLK, presses 1 and 3 plain downs — the OS re-arms the
    /// double-click detector every odd press, so a quadruple click is
    /// down/dblclk/down/dblclk) — but only for classes that registered
    /// CS_DBLCLKS; other classes receive another plain down on every press.
    /// WINDOWS-CI-ONLY (this whole file is cfg(windows)).
    #[test]
    fn click_sequences_alternate_down_and_double_click_like_the_os() {
        let (down, double, up, mark) = mouse_button_messages(MouseButton::Left);
        assert_eq!(down, WM_LBUTTONDOWN);
        assert_eq!(double, WM_LBUTTONDBLCLK);
        assert_eq!(up, WM_LBUTTONUP);
        assert_eq!(mark, MK_LBUTTON);
        let messages_for = |count: u8, accepts_dblclk: bool| -> Vec<u32> {
            (0..count.max(1))
                .flat_map(|index| {
                    let press = multi_click_message(index, down, double, accepts_dblclk);
                    [press, up]
                })
                .collect()
        };
        assert_eq!(messages_for(1, true), vec![WM_LBUTTONDOWN, WM_LBUTTONUP]);
        assert_eq!(
            messages_for(2, true),
            vec![WM_LBUTTONDOWN, WM_LBUTTONUP, WM_LBUTTONDBLCLK, WM_LBUTTONUP]
        );
        assert_eq!(
            messages_for(3, true),
            vec![
                WM_LBUTTONDOWN,
                WM_LBUTTONUP,
                WM_LBUTTONDBLCLK,
                WM_LBUTTONUP,
                WM_LBUTTONDOWN,
                WM_LBUTTONUP
            ]
        );
        assert_eq!(
            messages_for(4, true),
            vec![
                WM_LBUTTONDOWN,
                WM_LBUTTONUP,
                WM_LBUTTONDBLCLK,
                WM_LBUTTONUP,
                WM_LBUTTONDOWN,
                WM_LBUTTONUP,
                WM_LBUTTONDBLCLK,
                WM_LBUTTONUP
            ],
            "the fourth press must be a double-click again (OS alternation)"
        );
        // A non-CS_DBLCLKS class never sees a double-click: every press is
        // a plain down (click(count=2) stays TWO effective clicks there).
        assert_eq!(
            messages_for(2, false),
            vec![WM_LBUTTONDOWN, WM_LBUTTONUP, WM_LBUTTONDOWN, WM_LBUTTONUP],
            "without CS_DBLCLKS the second press must stay a plain down"
        );
        assert_eq!(
            messages_for(4, false),
            vec![
                WM_LBUTTONDOWN,
                WM_LBUTTONUP,
                WM_LBUTTONDOWN,
                WM_LBUTTONUP,
                WM_LBUTTONDOWN,
                WM_LBUTTONUP,
                WM_LBUTTONDOWN,
                WM_LBUTTONUP
            ]
        );
        let (right_down, right_double, right_up, right_mark) =
            mouse_button_messages(MouseButton::Right);
        assert_eq!(right_down, WM_RBUTTONDOWN);
        assert_eq!(right_double, WM_RBUTTONDBLCLK);
        assert_eq!(right_up, WM_RBUTTONUP);
        assert_eq!(right_mark, MK_RBUTTON);
    }

    /// Short-read rejection of the GetDIBits capture (pure helper): a
    /// truncated read must fail explicitly instead of reporting a half-black
    /// frame as a good capture. WINDOWS-CI-ONLY (this whole file is
    /// cfg(windows)).
    #[test]
    fn getdibits_short_read_fails_explicitly() {
        assert_eq!(short_read_reason(800, 800), None, "a full read passes");
        assert_eq!(
            short_read_reason(0, 800).is_some(),
            true,
            "zero lines is a short read"
        );
        assert_eq!(
            short_read_reason(-1, 800).map(|reason| reason.contains("failed")),
            Some(true),
            "a GetDIBits failure code must be reported"
        );
        assert_eq!(
            short_read_reason(743, 800).map(|reason| reason.contains("743 of 800")),
            Some(true),
            "a short read must name both counts"
        );
    }

    /// Command-line quoting: bare parts stay bare; parts with spaces or
    /// quotes wrap and escape; the trailing-backslash run of QUOTED parts is
    /// doubled (the Windows argv rule: backslashes directly before a quote
    /// are escape sequences, so a quoted `...\` would otherwise consume the
    /// closing quote). WINDOWS-CI-ONLY (this whole file is cfg(windows)).
    #[test]
    fn command_line_quoting_wraps_only_when_needed() {
        assert_eq!(quote_command_line_part("notepad.exe"), "notepad.exe");
        assert_eq!(quote_command_line_part(""), "\"\"");
        assert_eq!(quote_command_line_part("my app.exe"), "\"my app.exe\"");
        assert_eq!(
            quote_command_line_part("he said \"hi\""),
            "\"he said \\\"hi\\\"\""
        );
        // Bare parts keep backslashes literal (unquoted backslashes are not
        // escape sequences — doubling them would corrupt the path).
        assert_eq!(quote_command_line_part(r"C:\dir\"), r"C:\dir\");
        assert_eq!(quote_command_line_part(r"C:\dir"), r"C:\dir");
        assert_eq!(quote_command_line_part(r"C:\\"), r"C:\\");
        // Quoted parts double the trailing backslash run so it cannot
        // swallow the closing quote (round-trips to a single backslash).
        assert_eq!(
            quote_command_line_part(r"C:\dir with space\"),
            r#""C:\dir with space\\""#
        );
        assert_eq!(
            quote_command_line_part(r"a b\\"),
            r#""a b\\\\""#,
            "two trailing backslashes double to four (parse back to two)"
        );
        // Embedded quotes AND a trailing backslash: the quote escapes and
        // the trailing run doubles together.
        assert_eq!(quote_command_line_part(r#"say "hi"\"#), r#""say \"hi\"\\""#);
        assert_eq!(
            build_command_line("notepad.exe", &["my file.txt".to_string()]),
            "notepad.exe \"my file.txt\""
        );
        assert_eq!(build_command_line("app", &[]), "app");
    }

    /// `open` bounds mirror the tool layer (non-empty, NUL-free, capped).
    #[test]
    fn open_validation_enforces_tool_layer_bounds() {
        assert!(validate_open_request("notepad.exe", &[]).is_ok());
        assert!(validate_open_request("", &[]).is_err(), "empty program");
        assert!(validate_open_request("a\0b", &[]).is_err(), "NUL program");
        assert!(
            validate_open_request(&"x".repeat(MAX_OPEN_PROGRAM_CHARS + 1), &[]).is_err(),
            "over-long program"
        );
        assert!(
            validate_open_request("app", &["ok".to_string()]).is_ok(),
            "valid arg"
        );
        let too_many: Vec<String> = (0..=MAX_OPEN_ARGS).map(|i| i.to_string()).collect();
        assert!(
            validate_open_request("app", &too_many).is_err(),
            "arg count bound"
        );
        assert!(
            validate_open_request("app", &["a\0b".to_string()]).is_err(),
            "NUL arg"
        );
        assert!(
            validate_open_request("app", &["x".repeat(MAX_OPEN_ARG_CHARS + 1)]).is_err(),
            "arg length bound"
        );
        // Error messages stay generic — no echo of the program text (the
        // audit log carries the parse failure verbatim).
        let long_program = "secret-app-".repeat(200);
        let error = match validate_open_request(&long_program, &[]) {
            Err(error) => error.to_string(),
            Ok(()) => panic!("an over-long program must refuse"),
        };
        assert!(
            !error.contains("secret-app"),
            "the error must not echo the program text: {error}"
        );
    }

    /// The shell launch plan: %WINDIR%\explorer.exe, NO folder argument.
    /// The application path stays RAW (it feeds lpApplicationName, which is
    /// never command-line-parsed — quoting it would corrupt paths with
    /// spaces). The old plan opened %USERPROFILE% so the desktop would not
    /// be blank; that put the user's home-directory listing into every
    /// early capture, so the plan now launches bare (readiness comes from
    /// the Shell_TrayWnd wait, not from a folder window).
    #[test]
    fn explorer_launch_plan_resolves_env_fallbacks() {
        let (application, command_line) = explorer_launch_plan(Some(r"C:\Windows"));
        assert_eq!(application, r"C:\Windows\explorer.exe");
        assert_eq!(command_line, "explorer.exe");
        let (fallback_app, fallback_cmd) = explorer_launch_plan(None);
        assert_eq!(fallback_app, r"C:\Windows\explorer.exe");
        assert_eq!(fallback_cmd, "explorer.exe");
        // Empty env values fall back like missing ones.
        let (empty_env_app, empty_env_cmd) = explorer_launch_plan(Some(""));
        assert_eq!(empty_env_app, r"C:\Windows\explorer.exe");
        assert_eq!(empty_env_cmd, "explorer.exe");
        // A spaced windir stays raw for lpApplicationName.
        let (spaced_app, spaced_cmd) = explorer_launch_plan(Some(r"D:\ Win"));
        assert_eq!(spaced_app, "D:\\ Win\\explorer.exe");
        assert_eq!(spaced_cmd, "explorer.exe");
    }

    /// The probe core answers the actionable detail exactly when the shell
    /// binary is missing (and None when present) — the condition behind the
    /// documented physical fallback.
    #[test]
    fn probe_core_requires_the_shell_binary() {
        assert!(explorer_missing_with(|_| true).is_none());
        let detail = explorer_missing_with(|_| false).expect("missing shell");
        assert!(detail.contains("explorer.exe"), "{detail}");
    }

    /// The GDI-leak tripwire tolerance: small jitter stays quiet, monotonic
    /// growth trips.
    #[test]
    fn gdi_leak_tripwire_tolerates_jitter_only() {
        let baseline = (100u32, 50u32);
        assert!(!gdi_leak_suspected(baseline, (100, 50)));
        assert!(!gdi_leak_suspected(baseline, (103, 52)));
        assert!(gdi_leak_suspected(baseline, (105, 50)), "gdi growth trips");
        assert!(
            gdi_leak_suspected(baseline, (100, 55)),
            "user-object growth trips"
        );
        assert!(!gdi_leak_suspected(baseline, (99, 49)), "shrink is fine");
    }

    /// Composite ordering decisions: desktop backgrounds never force a
    /// repaint and print with flag 0; app windows print with
    /// PW_RENDERFULLCONTENT.
    #[test]
    fn print_flags_and_repaint_decisions_follow_window_class() {
        assert!(is_shell_background_class("Progman"));
        assert!(is_shell_background_class("WorkerW"));
        assert!(!is_shell_background_class("Notepad"));
        assert_eq!(print_flags_for_class("Progman"), 0);
        assert_eq!(print_flags_for_class("WorkerW"), 0);
        assert_eq!(print_flags_for_class("Notepad"), PW_RENDERFULLCONTENT);
        assert!(is_shell_window_class("Shell_TrayWnd"));
        assert!(!is_shell_window_class("Notepad"));
    }

    /// 32bpp GetDIBits decode: bottom-up BGRX rows flip to top-down RGBA
    /// with opaque alpha (2×2, byte-exact).
    #[test]
    fn flip_and_swizzle_produces_top_down_opaque_rgba() {
        // Bottom-up source: row 0 = BOTTOM scanline (blue-ish), row 1 = TOP
        // scanline (red-ish); each pixel B,G,R,X.
        let mut buffer = vec![
            255, 0, 0, 70, // bottom-left: B=255
            0, 255, 0, 71, // bottom-right: G=255
            0, 0, 255, 72, // top-left: R=255
            10, 20, 30, 73, // top-right
        ];
        flip_and_swizzle_bgrx_to_rgba(&mut buffer, 2, 2);
        assert_eq!(
            buffer,
            vec![
                255, 0, 0, 255, // out row 0 = TOP scanline, R=255 pixel
                30, 20, 10, 255, // out row 0 = TOP scanline, swizzled pixel
                0, 0, 255, 255, // out row 1 = BOTTOM scanline, B=255 pixel
                0, 255, 0, 255, // out row 1 = BOTTOM scanline, G=255 pixel
            ]
        );
        // Odd height: middle row swizzles in place.
        let mut odd = vec![1, 2, 3, 40, 4, 5, 6, 41, 7, 8, 9, 42];
        flip_and_swizzle_bgrx_to_rgba(&mut odd, 1, 3);
        assert_eq!(odd, vec![9, 8, 7, 255, 6, 5, 4, 255, 3, 2, 1, 255]);
    }

    /// WM_CHAR text encoding runs through the IMPLEMENTATION's encoder
    /// (`char_utf16_units`, the fn the injection loop itself calls): a BMP
    /// char yields exactly one unit, a non-BMP char yields BOTH surrogate
    /// units, and the pair re-decodes to the original character. Asserting
    /// on the impl fn (not std's `encode_utf16` directly) keeps a mutation
    /// that drops the low surrogate — or skips non-BMP characters entirely —
    /// red instead of silently green.
    #[test]
    fn char_utf16_units_cover_the_surrogate_range() {
        assert_eq!(char_utf16_units('a'), (0x61, None));
        assert_eq!(char_utf16_units('中'), (0x4E2D, None));
        let (high, low) = char_utf16_units('\u{1F600}');
        let low = low.expect("an astral char must yield both surrogate units");
        assert_eq!(
            String::from_utf16(&[high, low]).ok().as_deref(),
            Some("\u{1F600}")
        );
    }

    /// Composite blit offsets on a multi-monitor session: window rects are
    /// virtual-screen-global, the composite bitmap is virtual-screen-local,
    /// so the window position minus the virtual origin is the blit
    /// destination. Pins the sign convention both ways (a monitor left of /
    /// above the primary makes window coordinates negative while the origin
    /// is negative too; the primary-only layout has origin (0, 0)).
    /// WINDOWS-CI-ONLY (this whole file is cfg(windows)).
    #[test]
    fn composite_pos_offsets_by_the_virtual_origin() {
        // Single-monitor layout: origin (0, 0), positions pass through.
        assert_eq!(composite_pos(100, 50, 0, 0), (100, 50));
        assert_eq!(composite_pos(0, 0, 0, 0), (0, 0));
        // Left-of-primary monitor: windows at negative screen coordinates
        // with a negative virtual origin land inside the bitmap.
        assert_eq!(composite_pos(-1920, 0, -1920, 0), (0, 0));
        assert_eq!(composite_pos(-1000, 300, -1920, 0), (920, 300));
        // Above-primary monitor: negative y with negative y origin.
        assert_eq!(composite_pos(0, -1080, 0, -1080), (0, 0));
        // Mixed layout (one monitor left of AND one above the primary).
        assert_eq!(composite_pos(-3840, -2160, -3840, -1080), (0, -1080));
        // The virtual-screen rect read is pinned to the SM_* indices the
        // metrics helper consumes (order matters in the struct literal).
        assert_eq!(SM_XVIRTUALSCREEN.0, 76);
        assert_eq!(SM_YVIRTUALSCREEN.0, 77);
        assert_eq!(SM_CXVIRTUALSCREEN.0, 78);
        assert_eq!(SM_CYVIRTUALSCREEN.0, 79);
    }
}
