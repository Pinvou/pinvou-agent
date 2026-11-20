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
//! hidden desktop.
//!
//! **Why the cursor is a tracked virtual pointer:** `SetCursorPos` and
//! `GetCursorPos` are documented as requiring "the input desktop [to] be the
//! current desktop" (winuser.h remarks) — the hidden desktop is never the
//! input desktop (the user's real one is), so both fail with
//! ERROR_ACCESS_DENIED no matter which thread calls them (observed live:
//! 0x80070005 from an attached thread). There is no real cursor to move on a
//! hidden desktop, so this backend tracks its own pointer state: hit-testing
//! (`WindowFromPoint` from the attached thread works — it is a desktop-
//! relative query, not input-queue state) and the synthesized messages use
//! that tracked position, exactly the recipe mature HVNC stacks use. The
//! deprecated journal-playback hook was evaluated and rejected: it is
//! global-only, blocks real user input while active, and is documented as
//! debug-only.
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
//! BackendHandle lifecycle, a fresh dedicated worker thread per session,
//! de-facto gives a windowless worker) and restores its
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
//! fails with `ERROR_BUSY` is tolerated and logged: the desktop object is
//! reclaimed when the process exits (its job's kill-on-close frees the last
//! references); the creation-time `OpenDesktopW` probe is best-effort
//! same-pid leftover recovery, not a cross-process mechanism.
//!
//! Known fidelity limits (disclosed in `Capabilities`): message-level input
//! is app-level injection — apps that ignore synthetic messages cannot be
//! driven (Microsoft Store packaged apps — UWP/WinUI, including the Windows 11
//! Notepad — are the common case; the backend detects a packaged keyboard
//! target via its process image path and attaches an explicit warning to the
//! action's receipt instead of letting a silent no-op read as success);
//! non-client interactions follow the OS's OWN per-zone delivery (r5 — the
//! delivery model mirrors what DefWindowProc actually does with a real
//! press, whose modal states a uniform (down, up) post pair cannot drive):
//! title-bar double-click maximizes/restores (`WM_NCLBUTTONDBLCLK`, the
//! loop-free SC path), the caption min/max/close buttons deliver their
//! `WM_SYSCOMMAND` end state directly (a disabled/absent Close is refused,
//! not faked), a caption DOWN is completed with a client button-up so the
//! move mode it opens exits deterministically, menu-bar and system-menu
//! presses open the menu's tracking loop (its items are clicked with
//! SCREEN coordinates — the loop reads mouse lParams as screen points), and
//! clicks on window edges/borders/scrollbars are REFUSED (a press there
//! enters a modal move/size/scroll loop posted messages cannot complete);
//! a window still cannot be MOVED by dragging its caption nor resized by
//! dragging a border — both need the modal move/size loop that pumps live
//! pointer coordinates, which posted presses are not — and the manual
//! `left_mouse_down`/`left_mouse_up` primitives stay client-routed (use
//! `click` for non-client zones and menu items); UAC
//! elevation prompts appear on the secure desktop, which no hidden-desktop
//! API can drive; cross-desktop UI Automation runs in an ATTACHED HELPER
//! PROCESS (this executable re-spawned with the helper argv on the hidden
//! desktop, inside the session job): `ui_tree`/`element_at_point`/
//! `focused_element` serve the real semantic UIA tree from that helper, and
//! when the helper cannot start or has died (a transport failure) they
//! degrade to a window-class
//! walk (role is the window class, name is the caption, cross-process
//! control content and UIA-only properties unavailable, the classic
//! `ES_PASSWORD` style the one secure-field signal); programs that
//! resolve to a packaged-app activation (Paint on Windows 11 is the
//! observed case)
//! cannot join the session's kill-on-close job and fail the open with an
//! explicit error; a console program's window is delegated to the default
//! Windows Terminal host (packaged, broker-activated outside the hidden
//! desktop), so a console open can fail the same way or surface no visible
//! window; and
//! a Stop/revoke winds down the apps the session OPENED (each recorded
//! spawn process is terminated by its retained handle — no grace period,
//! unsaved work inside them is lost) while the hidden desktop itself — the
//! desktop object, the session job, and the shell — stays alive so the next
//! action, including a re-granted session, still works; the shell is only
//! reclaimed by the full teardown at Drop (session end), the same split the
//! Linux stack draws between its opened apps and Xvfb/WM.

// The window-class tree serializes with `writeln!` into a `String` — the
// trait import the physical stack's writer carries (the Linux gates cannot
// see this file's body, so an unimported `fmt::Write` would only fail on
// the Windows leg).
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::{JoinHandle, sleep};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, E_ACCESSDENIED, ERROR_BUSY, GENERIC_ALL, HANDLE, HWND, LPARAM, POINT, RECT, WPARAM,
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
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOW, TerminateProcess,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, MAPVK_VK_TO_VSC, MapVirtualKeyW, SetFocus, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN,
    VK_END, VK_ESCAPE, VK_F1, VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_OEM_1,
    VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS,
    VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CWP_SKIPDISABLED, CWP_SKIPINVISIBLE, ChildWindowFromPointEx, ES_PASSWORD,
    FindWindowW, GA_ROOT, GCL_STYLE, GUITHREADINFO, GW_CHILD, GW_HWNDNEXT, GWL_STYLE, GetAncestor,
    GetClassLongW, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetMenuState,
    GetSystemMenu, GetSystemMetrics, GetTopWindow, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HTBOTTOM, HTBOTTOMLEFT,
    HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTCLOSE, HTERROR, HTHSCROLL, HTLEFT, HTMAXBUTTON, HTMENU,
    HTMINBUTTON, HTNOWHERE, HTRIGHT, HTSYSMENU, HTTOP, HTTOPLEFT, HTTOPRIGHT, HTTRANSPARENT,
    HTVSCROLL, IsIconic, IsWindowVisible, MF_BYCOMMAND, MF_DISABLED, MF_GRAYED,
    PW_RENDERFULLCONTENT, PostMessageW, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SMTO_ABORTIFHUNG,
    SendMessageTimeoutW, SetForegroundWindow, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN,
    WM_NCLBUTTONUP, WM_NCMBUTTONDBLCLK, WM_NCMBUTTONDOWN, WM_NCMBUTTONUP, WM_NCMOUSEMOVE,
    WM_NCRBUTTONDBLCLK, WM_NCRBUTTONDOWN, WM_NCRBUTTONUP, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP, WS_DISABLED, WS_MAXIMIZE,
    WS_MAXIMIZEBOX, WS_MINIMIZE, WS_MINIMIZEBOX, WS_SYSMENU, WindowFromPoint,
};
use windows::core::{BOOL, HRESULT, PCWSTR, PWSTR};

use super::super::super::backend::ComputerUseBackend;
use super::super::super::guard::raw_element_binding;
use super::super::super::types::{
    Capabilities, Capture, ComputerUseError, DisplayTarget, ElementInfo, Key, MouseButton,
    ScrollDirection, UiTreeOptions,
};
use super::super::helpers::{
    CHORD_HOLD_MS, CLICK_SETTLE_MS, DRAG_PRESS_SETTLE_MS, DRAG_STEP_DELAY_MS, DRAG_STEPS,
    HOLD_CANCEL_POLL_MS, MAX_NAME_CHARS, MAX_OPEN_CHILDREN, MAX_SCROLL_CLICKS,
    MULTI_CLICK_INTERVAL_MS, TYPE_CHUNK_CHARS, TypeRun, append_tree_truncation_footers,
    combine_drag_errors, combine_hold_errors, drag_waypoints, logged_program,
    normalize_typed_newlines, open_capacity_rejected, sanitize_name, screening_hit,
    split_type_runs, tree_budget,
};
use super::super::uia_helper::{UiaHelperClient, UiaHelperOutcome};
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
/// is up (readiness probe).
const SHELL_TRAY_CLASS: &str = "Shell_TrayWnd";
/// Explorer's desktop-background host windows: printed with PrintWindow flag
/// 0 and never force-repainted (they are static bitmaps; forcing redraw is
/// wasted work — the rustyvnc recipe's refinement).
const SHELL_BACKGROUND_CLASSES: [&str; 2] = ["Progman", "WorkerW"];
/// Shell windows that must never become the keyboard-focus fallback target
/// (the secondary-monitor taskbar included: on a multi-monitor session the
/// blank desktop's fallback walk otherwise returns it as the topmost
/// visible "app window" and typing reports success into the taskbar).
/// `XamlExplorerHostIsolatedWindow` is the Windows 11 shell's XAML host
/// (Start/Widgets) — the session's explorer spawns it on the hidden desktop
/// and without the exclusion it appears as a tree root (r5). UWP-shell
/// hosts whose class is `Windows.UI.Core.CoreWindow` (Start menu, search)
/// are deliberately NOT listed: the same class carries real UWP app
/// windows (e.g. Settings), so a class-based exclusion would hide drivable
/// apps — that residual noise stays disclosed in the notes instead.
const SHELL_WINDOW_CLASSES: [&str; 5] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "XamlExplorerHostIsolatedWindow",
];

/// Hidden desktop name: fixed prefix + process pid + a
/// per-process sequence, so concurrent sessions in one process get distinct
/// names. The creation-time recovery probe can only ever see a leftover of
/// THIS pid (name-embedded) — cross-process leftovers that are JOB MEMBERS
/// are reclaimed by the kernel when the crashed process's job handles die
/// (kill-on-close), not by the next start. Residual (R12): a
/// broker-launched app lives OUTSIDE the job (disclosed in the capability
/// notes), so after a crashed run that broker-activated an app, that app
/// keeps the orphaned desktop object alive — no later process can see or
/// kill it (its pid is not ours), and repeated crash+broker cycles
/// accumulate desktop objects against the system desktop heap.
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
/// Shell (`Shell_TrayWnd`) readiness budget (≤5 s, warn on miss).
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
/// The old 2 s budget let a slow-dying leftover (a wedged app that only the
/// last TerminateProcess round reaches, a shell unwinding its windows)
/// survive into the reopened desktop — `CreateDesktopW` on an existing name
/// REOPENS the same desktop object, so ghost windows would sit in the new
/// session's captures and hit-tests. 10 s stays bounded and this path runs
/// only after a crashed run; the residual after exhaustion is warn-logged.
const RECOVERY_EMPTY_POLLS: u32 = 100;
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

/// `open` tool-layer bounds, REFERENCED (not mirrored) from the tool layer:
/// a second line of defense behind the tool's own validation that cannot
/// silently drift when the tool caps change (the Linux/macOS stacks defer
/// the length checks to the tool layer entirely).
use crate::features::computer_use::tool::{
    MAX_OPEN_ARG_CHARS, MAX_OPEN_ARGS, MAX_OPEN_PROGRAM_CHARS,
};

/// `WM_MOUSEWHEEL`'s one detent in `wParam` high word.
const WHEEL_DELTA: i32 = 120;
/// `wParam` button-state marks for posted mouse messages.
const MK_LBUTTON: usize = 0x0001;
const MK_RBUTTON: usize = 0x0002;
const MK_MBUTTON: usize = 0x0010;

/// Cheap availability probe: the hidden-desktop API set is part
/// of Win32, so the only hard external dependency is the shell binary the
/// desktop launches — probe %WINDIR%\explorer.exe (pure stat, no spawn, no
/// side effects; it runs at the refresh points — startup replay, preference
/// flips, creation reporting — and status reads consume the cached
/// snapshot; creation happens later via [`create_backend`]). A miss answers
/// Unavailable so the one-time target decision falls back to the physical
/// desktop instead of building a session that fails on its first spawn.
pub(crate) fn probe() -> VirtualAvailability {
    match explorer_presence() {
        ExplorerPresence::Found => VirtualAvailability::Available,
        ExplorerPresence::Missing { detail } => VirtualAvailability::Unavailable { detail },
    }
}

/// Pure probe core over an injectable existence check (unit-testable
/// without machine state).
fn explorer_missing_with(explorer_exists: impl Fn(&str) -> bool) -> Option<String> {
    // The SAME resolver `explorer_launch_plan` spawns through: a set-but-
    // empty WINDIR must not make the probe and the spawn disagree on the
    // binary path (probe/spawn one-resolution invariant — the R17 fix,
    // now structurally unable to drift because there is one resolution).
    let explorer = explorer_path(std::env::var("WINDIR").ok().as_deref());
    if explorer_exists(&explorer) {
        return None;
    }
    Some(format!(
        "the hidden-desktop shell {explorer} was not found; the virtual display needs \
         the Windows shell to run its session"
    ))
}

/// The hidden-desktop shell's path from a raw `WINDIR` env value: a
/// set-but-empty or absent value falls back to `C:\Windows`. ONE resolver
/// for the probe and the spawn plan (pure, unit-pinned — the probe test's
/// existence closures cannot observe the filter, so the filter is pinned
/// here).
fn explorer_path(windir: Option<&str>) -> String {
    let dir = windir
        .filter(|value| !value.is_empty())
        .unwrap_or("C:\\Windows");
    format!("{dir}\\explorer.exe")
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

/// Chord key order: modifiers lead, the rest keeps the given order.
/// Pure so the chord path and its pin share one implementation instead of
/// the test re-deriving the shape (which would stay green if the chord path
/// stopped partitioning at all).
fn partition_modifiers_first(keys: &[Key]) -> (Vec<&Key>, Vec<&Key>) {
    keys.iter().partition(|key| key.is_modifier())
}

/// The desktop access mask for `CreateDesktopW`: the narrow set
/// the desktop's own windows need — deliberately NOT `GENERIC_ALL`. The one
/// deliberate exception is the crashed-run recovery probe below, which
/// opens OUR OWN pid-named leftover with `GENERIC_ALL`: it is a belt-and-
/// braces reclamation of a desktop this process created (same pid in the
/// name), and a failed open there silently skips the reclamation, so the
/// broadest access is preferred over mask precision on that path.
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
/// `pub(crate)`: the UIA helper's spawn goes through the same builder, so
/// the window-station prefix lives in ONE place (r4: the helper copy was a
/// re-hardcoded literal).
pub(crate) fn desktop_target_path(name: &str) -> String {
    format!("{WINDOW_STATION}\\{name}")
}

/// Whether a window class is one of explorer's static desktop-background
/// hosts (never force-repainted; PrintWindow flag 0).
fn is_shell_background_class(class: &str) -> bool {
    SHELL_BACKGROUND_CLASSES.contains(&class)
}

/// Whether a window class is a shell window that must never become the
/// keyboard-focus fallback target.
pub(crate) fn is_shell_window_class(class: &str) -> bool {
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

/// `WM_KEYDOWN`/`WM_KEYUP`/`WM_CHAR` lParam: repeat count 1. Every
/// production call site posts the DOWN variant (`up == false` — the
/// WM_CHAR lane never posts an up); the UP arm (previous-key-state bit 30
/// + transition bit 31, the `TranslateMessage`-generated shape) stays for
/// the bit-layout completeness the `compose_key_lparam` pins cover.
fn key_lparam(up: bool) -> isize {
    if up {
        (1i32 << 30 | 1i32 << 31 | 1) as isize
    } else {
        1
    }
}

/// lParam bit assembly for the KEY messages (pure, so the shape is pinable
/// without a display): repeat count 1, the scan code at bits 16-23, the
/// extended-key flag at bit 24 (set for the navigation cluster, whose real
/// input always carries it — apps distinguishing navigation from
/// numpad/number-row keys read it), the context code at bit 29 (set while
/// Alt is held — every key delivered during an Alt chord rides it), and the
/// previous-key-state (bit 30) + transition (bit 31) flags on the UP
/// variant, exactly what real input carries. A scan code wider than 8 bits
/// masks down (the field is 8 bits).
fn compose_key_lparam(scan: u32, extended: bool, context: bool, up: bool) -> isize {
    let mut raw = 1u32 | ((scan & 0xFF) << 16);
    if extended {
        raw |= 1 << 24;
    }
    if context {
        raw |= 1 << 29;
    }
    if up {
        raw |= 1 << 30 | 1 << 31;
    }
    raw as isize
}

/// Whether `vk` is one of the extended keys whose real-input lParam carries
/// bit 24: the navigation cluster (the same VK also exists on the numpad, so
/// real input distinguishes them by this bit) plus the right-hand
/// Ctrl/Alt. The model's chord vocabulary only ever produces the
/// navigation cluster from this set (chords name `Control`/`Alt`, which map
/// to the shared left-side VKs). The table deliberately omits the
/// right-hand Win keys and RCONTROL/RMENU (VK_LWIN is also reachable — via
/// `Key::Meta` — and its real input does carry the bit): under the
/// disclosed message-level fidelity those keys' synthetic posts never
/// trigger shell-managed behavior anyway, so the omission is behaviorally
/// nil; it is stated here so the table is not mistaken for exhaustive.
/// Pure, unit-pinned.
fn key_is_extended(vk: u16) -> bool {
    // The VK_* constants are typed newtypes, not bare u16 patterns — the
    // set rides in a const table.
    const EXTENDED_KEYS: [u16; 10] = [
        VK_LEFT.0,
        VK_RIGHT.0,
        VK_UP.0,
        VK_DOWN.0,
        VK_HOME.0,
        VK_END.0,
        VK_PRIOR.0,
        VK_NEXT.0,
        VK_INSERT.0,
        VK_DELETE.0,
    ];
    EXTENDED_KEYS.contains(&vk)
}

/// Scan code for a virtual-key code (`MapVirtualKeyW`, VK → VSC); 0 when
/// the key has no scan mapping.
///
/// SAFETY: pure translation-table query; no pointers involved.
unsafe fn vk_scan(vk: u16) -> u32 {
    // SAFETY: see the fn contract.
    unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) }
}

/// The lParam for a key message about `vk` (the WM_KEYDOWN / WM_SYSKEYDOWN /
/// WM_KEYUP / WM_SYSKEYUP family): real scan code + extended-key bit +
/// context bit, unlike the WM_CHAR posts whose lParam the message loop
/// rarely validates.
fn key_message_lparam(vk: u16, alt_held: bool, up: bool) -> isize {
    // SAFETY: pure translation query.
    compose_key_lparam(unsafe { vk_scan(vk) }, key_is_extended(vk), alt_held, up)
}

/// The (down, up) message pair for a key press: EVERY key pressed while Alt
/// is held rides the WM_SYSKEYDOWN/WM_SYSKEYUP pair with the context bit —
/// not just VK_MENU itself. DefWindowProc's WM_SYSKEYDOWN handler is what
/// turns VK_F4 into SC_CLOSE and Alt+letter into SC_KEYMENU, so a companion
/// key posted as plain WM_KEYDOWN (the pre-fix shape) made Alt+F4 and
/// Alt-menu mnemonics never fire even with the Alt itself SYS-shaped.
/// Chords without Alt keep the plain pair — the system pair is Alt-specific.
/// Pure so the selection is unit-pinned: reverting a companion key to the
/// plain pair must fail a test, not pass every CI leg silently.
fn key_message_pair(alt_held: bool) -> (u32, u32) {
    if alt_held {
        (WM_SYSKEYDOWN, WM_SYSKEYUP)
    } else {
        (WM_KEYDOWN, WM_KEYUP)
    }
}

/// The message plan for one WHOLE chord: the (down, up) pair every key of
/// the chord rides plus the Alt-derived lParam flag, derived ONCE from the
/// full ordered key list. Deliberately takes the chord, not a per-key
/// answer — the pre-fix regression computed the selection per key (only
/// VK_MENU itself SYS-shaped) and the pair-fn pin could not see it; a
/// per-key reversion now has to bypass this documented whole-chord shape.
fn chord_message_plan(ordered: &[u16]) -> (u32, u32, bool) {
    // The chord keys are virtual-key codes (VIRTUAL_KEY's u16 payload —
    // key_to_vk maps straight into the windows-crate constants).
    let alt_held = ordered.contains(&VK_MENU.0);
    let (down_message, up_message) = key_message_pair(alt_held);
    (down_message, up_message, alt_held)
}

/// The window a chord's key messages target (pure, unit-pinned — HWND is a
/// transparent handle wrapper for this comparison). While Alt is held the
/// system pair goes to the top-level frame so frame-routed SC_CLOSE /
/// SC_KEYMENU handling (app menus, Alt+F4) fires; a plain pair is ordinary
/// keyboard input, which Windows delivers to the FOCUS window — posting it
/// to the frame makes frame-routed apps (Notepad's frame dispatches to
/// DefWindowProc) silently drop it while PostMessageW reports success.
/// Trade-off: apps whose focused child implements its own system-key
/// handling (MDI child system menus) see SYS chords on the frame instead
/// of the child.
fn chord_post_target(focus: HWND, top: HWND, alt_held: bool) -> HWND {
    if alt_held { top } else { focus }
}

/// Pack client coordinates into a mouse-message lParam (both i16).
fn pack_mouse_lparam(x: i32, y: i32) -> isize {
    (((y as u16 as u32) << 16) | (x as u16 as u32)) as isize
}

/// Whether the point can be encoded faithfully in a mouse-message lParam
/// (both coordinates ride signed 16-bit fields — the encoding
/// `GET_X_LPARAM` decodes on the receiving side). A point outside the
/// −32768..=32767 range occurs only on a virtual screen wider/taller than
/// 32767 px (legal multi-4K arrangements): delivering one would wrap
/// through the u16 truncation to a negative coordinate, so the hit-test
/// probe and the post would agree with each other and BOTH disagree with
/// reality — the action refuses instead of silently landing at
/// `coord − 65536`. Client-side posts stay in range while the window
/// origin lies on the same virtual screen (the normal shape). Pure; pinned.
fn mouse_point_encodable(x: i32, y: i32) -> bool {
    (i16::MIN as i32..=i16::MAX as i32).contains(&x)
        && (i16::MIN as i32..=i16::MAX as i32).contains(&y)
}

/// The refusal for a point [`mouse_point_encodable`] rejects.
fn unencodable_point_error() -> ComputerUseError {
    ComputerUseError::unavailable(
        "the point lies outside the −32768..=32767 coordinates a mouse \
         message can encode (an oversized multi-monitor virtual screen); \
         the action was not delivered",
    )
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

/// The `(down, double-click, up)` non-client message triple for a button —
/// the WM_NC* counterparts of [`mouse_button_messages`]. NC messages carry
/// the hit-test code in wParam (NOT the MK_* mark) and screen coordinates
/// in lParam (NOT client coordinates) — see [`WindowsVirtualDesktopBackend::
/// nc_click`]. Pure; pinned.
fn nc_mouse_button_messages(button: MouseButton) -> (u32, u32, u32) {
    match button {
        MouseButton::Left => (WM_NCLBUTTONDOWN, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONUP),
        MouseButton::Right => (WM_NCRBUTTONDOWN, WM_NCRBUTTONDBLCLK, WM_NCRBUTTONUP),
        MouseButton::Middle => (WM_NCMBUTTONDOWN, WM_NCMBUTTONDBLCLK, WM_NCMBUTTONUP),
    }
}

/// The NC message for press `index` (0-based) of a multi-click. The client
/// side ([`multi_click_message`]) gates the DBLCLK substitution on the
/// target's registered `CS_DBLCLKS` style; the NC side has NO such gate —
/// the OS synthesizes WM_NCLBUTTONDBLCLK for real double-clicks on any
/// window (title-bar double-click to maximize works on classes that never
/// registered CS_DBLCLKS), so the second press always substitutes. Pure;
/// pinned.
fn nc_multi_click_message(index: u8, down: u32, double: u32) -> u32 {
    if index % 2 == 1 { double } else { down }
}

/// The `(press, release)` NC message pair for press `index` (0-based) of a
/// multi-click on the zones delivered as plain pairs (menu bar / system
/// menu presses, and the DefWindowProc no-op zones). The second press
/// substitutes the DBLCLK message on every class — NC double-clicks are not
/// CS_DBLCLKS-gated (see [`nc_multi_click_message`]). Chrome buttons and
/// the caption do NOT use this pair (r5): their DefWindowProc handlers run
/// MODAL states a posted NC up (below `WM_MOUSEFIRST`, so below every
/// tracking pump's mouse-range filter) can never complete — those zones
/// deliver [`nc_chrome_system_command`] / [`nc_caption_press`] instead.
/// Pure; pinned.
fn nc_press_pair(index: u8, button: MouseButton) -> (u32, u32) {
    let (down, double, up) = nc_mouse_button_messages(button);
    (nc_multi_click_message(index, down, double), up)
}

/// What the OS does when a press lands in this zone — the delivery model
/// follows the OS per family instead of posting a uniform (down, up) pair:
/// several DefWindowProc NC handlers run modal loops that a fixed post
/// sequence cannot drive (verified against ReactOS `nonclient.c`, the NT
/// win32k mirror: `NC_DoButton`, `NC_HandleNCLButtonDown`,
/// `NC_HandleNCLButtonDblClk`, `DefWndDoSizeMove`). Pure; pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NcZoneDelivery {
    /// Caption buttons (HTCLOSE/HTMINBUTTON/HTMAXBUTTON): the DOWN enters
    /// a capture + modal pump filtered to `WM_MOUSEFIRST..WM_MOUSELAST`
    /// (0x200..0x20E on current SDK tiers; 0x20D before WM_MOUSEHWHEEL
    /// existed) that a posted `WM_NCLBUTTONUP` (0xA2, below the
    /// range) can NEVER satisfy — the system command fires only when some
    /// later stranded client `WM_LBUTTONUP` arrives (a deferred close in
    /// the middle of an unrelated action). Deliver the end state directly:
    /// `WM_SYSCOMMAND` with the exact SC code `NC_DoButton` itself derives
    /// from the window style.
    ChromeButton,
    /// The caption: a single DOWN enters `SC_MOVE`'s modal move loop
    /// (exits only on a client `WM_LBUTTONUP`, a no-button `WM_MOUSEMOVE`,
    /// or ESC — the posted NC up is retrieved by the loop's UNFILTERED
    /// pump and dispatched, changing nothing); the double-click press is
    /// loop-free (`NC_HandleNCLButtonDblClk` sends
    /// `SC_RESTORE`/`SC_MAXIMIZE` directly).
    Caption,
    /// Menu bar / system menu presses: the DOWN opens the menu's tracking
    /// loop — that loop IS the opened menu (the desired effect). The
    /// popup's item clicks are driven with screen-coordinate client posts
    /// ([`menu_popup_needs_screen_coords`]); a system-menu DOUBLE-click is
    /// loop-free (`SC_CLOSE`).
    Menu,
    /// Resize borders (`HTLEFT..HTBOTTOMRIGHT`) and scroll bars
    /// (`HTHSCROLL/HTVSCROLL`): a single press enters the
    /// `SC_SIZE`/`SC_HSCROLL`/`SC_VSCROLL` modal tracking loops that
    /// terminate only on live pointer/keyboard input — posted sequences
    /// cannot complete them, so these zones are REFUSED (disclosed)
    /// instead of leaving the target captured in move/size mode behind a
    /// reported success.
    ModalLoop,
    /// Zones DefWindowProc handles with no loop and no state
    /// (`HTBORDER`, `HTGROWBOX`, `HTOBJECT`): the plain
    /// (down, up) pair completes as a no-op, exactly like real input.
    /// `HTHELP` rides in this family too — a plain no-op in both win32k
    /// mirrors: neither ReactOS's nor Wine's NC switch has an HTHELP arm,
    /// `WM_APPCOMMAND` is generated from X-button UP only, and the
    /// documented caption-help path is `SC_CONTEXTHELP` — none of those is
    /// reachable from a synthetic HTHELP DOWN (the r7 "WM_APPCOMMAND"
    /// wording was itself wrong — r8).
    NoOp,
}

/// The delivery family for a hit-test zone. Pure; pinned.
fn nc_zone_delivery(zone: u32) -> NcZoneDelivery {
    match zone {
        HTMINBUTTON | HTMAXBUTTON | HTCLOSE => NcZoneDelivery::ChromeButton,
        HTCAPTION => NcZoneDelivery::Caption,
        HTMENU | HTSYSMENU => NcZoneDelivery::Menu,
        HTLEFT | HTRIGHT | HTTOP | HTBOTTOM | HTTOPLEFT | HTTOPRIGHT | HTBOTTOMLEFT
        | HTBOTTOMRIGHT | HTHSCROLL | HTVSCROLL => NcZoneDelivery::ModalLoop,
        // HTBORDER, HTGROWBOX, HTOBJECT: DefWindowProc's switch has no case
        // for these (or an explicit `break` for HTBORDER) — a no-op, like
        // real input. HTHELP lands here too: the mirrors' NC switches have
        // no HTHELP arm either, so the DOWN is a no-op exactly like real
        // input (the r7 WM_APPCOMMAND wording was itself wrong — r8).
        _ => NcZoneDelivery::NoOp,
    }
}

/// The `WM_SYSCOMMAND` code for a chrome-button press — the exact mapping
/// `NC_DoButton` performs from the window style (min → restore when already
/// minimized, max → restore when already maximized), INCLUDING the style
/// gates `NC_DoButton` enforces before it ever derives a command: a window
/// without `WS_MINIMIZEBOX`/`WS_MAXIMIZEBOX`/`WS_SYSMENU` has no working
/// button there (`WM_NCHITTEST` never returns the button zone for standard
/// frames, so this gate only bites custom-chrome windows that answer
/// HTMINBUTTON/HTMAXBUTTON/HTCLOSE without the backing styles — real input
/// is a no-op there, and synthesizing the SC would DELIVER what the OS
/// would refuse). `None` = the press must be refused, not faked. Pure;
/// pinned.
fn nc_chrome_system_command(zone: u32, style: isize) -> Option<u32> {
    let offers = |feature: isize| style & feature != 0;
    match zone {
        HTCLOSE if offers(WS_SYSMENU.0 as isize) => Some(SC_CLOSE),
        HTMINBUTTON if offers(WS_MINIMIZEBOX.0 as isize) => {
            if style & WS_MINIMIZE.0 as isize != 0 {
                Some(SC_RESTORE)
            } else {
                Some(SC_MINIMIZE)
            }
        }
        HTMAXBUTTON if offers(WS_MAXIMIZEBOX.0 as isize) => {
            if style & WS_MAXIMIZE.0 as isize != 0 {
                Some(SC_RESTORE)
            } else {
                Some(SC_MAXIMIZE)
            }
        }
        _ => None,
    }
}

/// Whether the window's system menu actually offers a working Close —
/// `NC_DoButton` refuses the grayed/disabled/absent close the same way
/// (`GetMenuState` answers `u32::MAX` for an absent item, and the
/// grayed/disabled style bits otherwise). Pure; pinned.
fn nc_close_command_enabled(menu_state: u32) -> bool {
    menu_state != u32::MAX && menu_state & (MF_GRAYED.0 | MF_DISABLED.0) == 0
}

/// What deterministically completes a caption press (r7: per button — the
/// old shape completed EVERY button's DOWN with a client `WM_LBUTTONUP`,
/// which is the right exit only for the LEFT press's `SC_MOVE` loop; a
/// RIGHT caption down opens the system-menu tracking loop, which reads
/// mouse lParams as SCREEN coordinates (the r5 menu contract), so a
/// client-coords UP ended tracking at a decoded point near the origin):
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptionCompletion {
    /// LEFT down: the `SC_MOVE` modal move loop — exits on a client
    /// `WM_LBUTTONUP` at the same point (the loop's own exit arm, zero
    /// movement, activation preserved; the loop retrieves it via
    /// `PM_REMOVE`, so the app never sees a stray client UP).
    MoveLoopExit,
    /// RIGHT down: the system-menu tracking loop — its capture pump
    /// exits on a `WM_RBUTTONUP` by message type alone (either mirror's
    /// `NC_HandleNCRButtonDown` breaks only on that message and never
    /// decodes the lParam), so the posted UP ends what the DOWN started
    /// deterministically. The UP carries the SCREEN caption point —
    /// faithful to a real release — but the pump reads its point from
    /// the queue's cursor at post time, so whether the system menu then
    /// opens follows where the physical cursor sits, not the delivered
    /// coordinates (r8 note).
    SystemMenuRelease,
    /// No loop opened (MIDDLE down is a DefWindowProc no-op, and every
    /// double-click press is loop-free): nothing to complete.
    None,
}

/// The caption press message for press `index`, plus the deterministic
/// completion its delivery needs (see [`CaptionCompletion`]). The
/// double-click press is loop-free for every button. NC double-clicks are
/// not CS_DBLCLKS-gated, so the substitution has no class gate (see
/// [`nc_multi_click_message`]). The NC `up` is deliberately omitted —
/// below `WM_MOUSEFIRST`, no modal pump can retrieve it and on the
/// loop-free path it is dead weight. Pure; pinned.
fn nc_caption_press(index: u8, button: MouseButton) -> (u32, CaptionCompletion) {
    let (down, double, _up) = nc_mouse_button_messages(button);
    let message = nc_multi_click_message(index, down, double);
    let completion = if message != down {
        CaptionCompletion::None
    } else {
        match button {
            MouseButton::Left => CaptionCompletion::MoveLoopExit,
            MouseButton::Right => CaptionCompletion::SystemMenuRelease,
            MouseButton::Middle => CaptionCompletion::None,
        }
    };
    (message, completion)
}

/// A menu popup's window class. Its tracking loop reads mouse-message
/// lParams as SCREEN coordinates (the synthetic-message contract —
/// ReactOS `menu.c` `MENU_TrackMenu`: "Use the mouse coordinates in
/// lParam … They are already in screen coordinates"), so the client
/// route's client translation would land the click at the wrong item.
const MENU_POPUP_CLASS: &str = "#32768";

/// Whether a click target of this class must be driven with screen
/// coordinates instead of client-translated ones. Pure; pinned.
fn menu_popup_needs_screen_coords(class: &str) -> bool {
    class == MENU_POPUP_CLASS
}

/// Whether the hit-test zone asked to be bypassed (HTTRANSPARENT: the
/// window passes input THROUGH itself to whatever is underneath — plain
/// Static labels and WS_EX_TRANSPARENT children answer this). Such a window
/// gets the pre-routing client posts, which it will usually swallow —
/// [`Self::click`] attaches a receipt warning for it (r4). Pure; pinned.
fn zone_is_transparent(zone: u32) -> bool {
    zone == HTTRANSPARENT as u32
}

/// The full set of hit-test answers with no honest WM_NC* sequence:
/// HTTRANSPARENT ([`zone_is_transparent`]), HTNOWHERE (nothing under the
/// point), and HTERROR (the hit-test itself failed). All three get the
/// pre-routing client posts AND the swallowed-click receipt warning (r7:
/// HTNOWHERE/HTERROR used to degrade silently — the same lost-click family
/// the r4 HTTRANSPARENT warning discloses). Pure; pinned.
fn zone_is_bypassed(zone: u32) -> bool {
    zone_is_transparent(zone) || zone == HTNOWHERE || zone == HTERROR as u32
}

/// Whether the hit-test code routes to the CLIENT message path (the message
/// route this backend drove exclusively before non-client routing existed).
/// `WM_NCHITTEST` answers HTCLIENT for client space and one of the HT* zone
/// codes (HTCAPTION, HTMINBUTTON, HTMENU, ...) for everything else — EXCEPT
/// the undrivable zones ([`zone_is_bypassed`]), which degrade to the
/// pre-routing client posts instead of an NC message with a meaningless
/// zone code that DefWindowProc would silently drop. A query that failed or
/// timed out answers "client" too (`nc_zone_of`), so an unresponsive target
/// degrades to the message posts that always worked instead of failing the
/// action. Pure; pinned.
fn zone_is_client(zone: u32) -> bool {
    matches!(zone, HTCLIENT) || zone_is_bypassed(zone)
}

/// Bound on the `WM_NCHITTEST` round-trip (a SendMessageTimeoutW to the
/// target's own thread — normally sub-millisecond). A hung target aborts
/// the wait (`SMTO_ABORTIFHUNG`) and the query degrades to the client
/// route; the click itself is never blocked behind a wedged app for more
/// than this budget.
const NC_HITTEST_TIMEOUT_MS: u32 = 200;

/// Top-level windows enumerated per window-class tree (the composite
/// capture walks the same Z-order with its own capture cap): the tree's
/// node budget can be raised by the caller, but the TOP-LEVEL fan-out is
/// fixed — a pathological window farm beyond this many top-level windows
/// serves the topmost ones, which are the windows a screenshot shows.
pub(crate) const TREE_TOP_LEVEL_CAP: usize = 64;

/// Minimum spacing between UIA-helper spawn ATTEMPTS: a broken environment
/// (helper launch refused, pipes unavailable) must not turn every
/// structural query into a fresh process storm. Within the cooldown the
/// queries degrade to the window-class walk.
const HELPER_RESPAWN_COOLDOWN_MS: u64 = 5_000;

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

/// Whether a character's US-layout face is the SHIFTED face of its key
/// position: `'+'` is Shift+=, and every uppercase letter is Shift+letter.
/// Posting such a key position PLAIN delivers the unshifted face — a plain
/// `VK_OEM_PLUS` decodes as `=`, so `key "plus"` typed `=` while reporting
/// success (R18; the physical stack derives the same property from
/// `VkKeyScanExW`'s shift-state high byte and reroutes it — its own test
/// calls the shape the defect scenario). `char_to_vk` maps the key
/// POSITION, and [`chord_shift_plan`] decides whether the Shift state must
/// ride the chord. Pure, unit-pinned.
fn char_requires_shift(c: char) -> bool {
    if c.is_ascii_uppercase() {
        return true;
    }
    // The shifted faces of the OEM positions `char_to_vk` maps. (The model's
    // chord vocabulary only reaches `'+'` today — the tokenizer normalizes
    // single characters to lowercase — but the table is the honest class,
    // not just the reachable case.)
    matches!(
        c,
        '+' | ':' | '"' | '<' | '>' | '?' | '~' | '{' | '|' | '}' | '_'
    )
}

/// Whether [`Self::chord_to`] must synthesize the Shift state for the
/// chord's character keys. A shifted character rides fine when `Shift` is
/// already among the modifiers (`shift+plus` posts `VK_OEM_PLUS` under a
/// held `VK_SHIFT` and always has). Solo, the shift wrap is synthesized:
/// `VK_SHIFT` joins the ordered list between the modifiers and the
/// character, so it presses first and releases (in reverse) right after —
/// and [`chord_message_plan`] still derives the pairs from the full list,
/// keeping an Alt-held chord's system pairs. Beside ANOTHER non-modifier
/// key the character is refused: the shift wrap cannot be scoped to just
/// that key, the same contract the physical stack applies to shifted
/// characters in multi-key chords (explicit refusal, `type` for text).
enum ShiftSynthesis {
    None,
    Insert,
}

/// Pure decision so the shape is pinable without a desktop (the whole file
/// is cfg(windows); this runs in the Windows CI leg). Refusals echo no
/// character (the message lands in the audit log verbatim).
fn chord_shift_plan(modifiers: &[&Key], rest: &[&Key]) -> Result<ShiftSynthesis, ComputerUseError> {
    let shifted = rest
        .iter()
        .any(|key| matches!(&**key, Key::Char(c) if char_requires_shift(*c)));
    if !shifted {
        return Ok(ShiftSynthesis::None);
    }
    let shift_held = modifiers.iter().any(|key| matches!(**key, Key::Shift));
    if shift_held {
        return Ok(ShiftSynthesis::None);
    }
    if rest.len() == 1 {
        Ok(ShiftSynthesis::Insert)
    } else {
        Err(ComputerUseError::failed(
            "a shifted character cannot share a chord with other keys (the \
             Shift state it needs cannot be scoped to just that key); use \
             the type action for text, or press it as a solo chord",
        ))
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
        '=' => VK_OEM_PLUS.0,
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
/// empty parts), escape embedded quotes, and double EVERY backslash run a
/// quote closes — embedded or trailing. The doubling is the Windows argv
/// rule (per the command-line parsing documented on
/// GetCommandLine/CommandLineToArgvW): inside quotes, backslashes directly
/// before a quote character are escape sequences (2n backslashes + quote
/// parse as n literal backslashes + a quote DELIMITER; 2n+1 as n literal
/// backslashes + a literal quote CHARACTER), so a quoted `C:\dir\` would
/// otherwise turn its CLOSING quote into a literal backslash-quote and
/// corrupt every following argument, and a `\"` inside a part would
/// silently EAT the part's own backslash. Bare (unquoted) parts keep their
/// backslashes literal — unquoted backslashes are not escape sequences, so
/// doubling them would corrupt the path instead.
fn quote_command_line_part(part: &str) -> String {
    if part.is_empty() || part.contains(' ') || part.contains('\t') || part.contains('"') {
        let trailing_backslashes = part.chars().rev().take_while(|&c| c == '\\').count();
        let mut quoted = String::with_capacity(part.len() + 2);
        for c in part.chars() {
            if c == '"' {
                // The backslash run this quote closes is an escape sequence
                // per the rule above — double it so it stays literal, then
                // escape the quote itself. (A run before an EMBEDDED quote
                // that stayed single would be consumed as the quote's own
                // escape, silently deleting the part's literal backslash.)
                let run = quoted.chars().rev().take_while(|&c| c == '\\').count();
                for _ in 0..run {
                    quoted.push('\\');
                }
                quoted.push('\\');
                quoted.push('"');
            } else {
                quoted.push(c);
            }
        }
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
/// Why this stack re-checks what the tool layer already capped (Linux and
/// macOS defer entirely): this is the one stack that turns the program and
/// its arguments into a QUOTED COMMAND LINE for CreateProcessW, so a bound
/// that drifted between the layers would first bite in the quoting path —
/// cheap defense at the exact seam where the transformation happens.
fn validate_open_request(program: &str, args: &[String]) -> Result<(), ComputerUseError> {
    if program.is_empty() {
        return Err(ComputerUseError::failed("open: program must not be empty"));
    }
    if program.chars().count() > MAX_OPEN_PROGRAM_CHARS {
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
        if arg.chars().count() > MAX_OPEN_ARG_CHARS {
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
    // The probe's resolver: a set-but-empty WINDIR cannot make the spawn
    // plan disagree with what the probe checked.
    (explorer_path(windir), "explorer.exe".to_string())
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
        // `&mut buffer[..]` does not compile). `tail` starts AT the bottom
        // row and extends to the end of the buffer, but only this one row is
        // ours to swizzle: everything below it was already swapped into the
        // top half by earlier iterations, and `swap(0, 2)` is self-inverse,
        // so re-swizzling would flip those rows' channels back.
        let (head, tail) = buffer.split_at_mut(bottom);
        swizzle_row(&mut head[top..top + stride]);
        swizzle_row(&mut tail[..stride]);
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
             (ERROR_BUSY); it is left behind and reclaimed when the process exits \
             (the kill-on-close job frees the last references)"
        );
    } else {
        log::warn!("[computer_use][virtual-win] {context}: {error}");
    }
}

/// Attach the CURRENT thread to the hidden desktop and verify the switch via
/// `GetThreadDesktop` discipline. The caller must have read the
/// thread's REAL previous desktop (via [`current_thread_desktop`]) BEFORE
/// `CreateDesktopW` — that call typically leaves the calling thread
/// attached to the new desktop (MSDN's assignment wording; a thread
/// already attached to another desktop may keep it), so a "previous" read
/// taken here afterwards could answer with the hidden desktop itself and
/// restore nothing.
///
/// SAFETY: the caller guarantees this thread owns no windows/hooks — a
/// thread with window/hook ownership cannot switch desktops, and the
/// backend worker (per BackendHandle) and a fresh capture thread both do.
unsafe fn attach_thread_to_desktop(desktop: HDESK) -> Result<(), ComputerUseError> {
    // SAFETY: thread-info queries + a desktop switch; see the fn contract.
    unsafe {
        SetThreadDesktop(desktop).map_err(|error| {
            ComputerUseError::unavailable(format!(
                "cannot attach the thread to the hidden desktop: {error}"
            ))
        })?;
        let current = GetThreadDesktop(GetCurrentThreadId()).map_err(|error| {
            ComputerUseError::failed(format!("cannot verify the thread's desktop: {error}"))
        })?;
        if current.0 != desktop.0 {
            return Err(ComputerUseError::unavailable(
                "the hidden desktop attachment did not take effect (GetThreadDesktop mismatch)",
            ));
        }
        Ok(())
    }
}

/// The calling thread's CURRENT desktop handle — the real "previous" desktop
/// for a later restore, which must be read BEFORE `CreateDesktopW` (the
/// call's thread-to-desktop assignment is what makes a later read answer
/// with the new desktop itself; the restore below is written to be honest
/// under either reading of the MSDN wording). The handle is a
/// pseudo one (`GetThreadDesktop`'s output needs no `CloseDesktop`, per
/// MSDN).
///
/// SAFETY: thread-info query; no pointers involved.
unsafe fn current_thread_desktop() -> Result<HDESK, ComputerUseError> {
    // SAFETY: see the fn contract.
    unsafe {
        GetThreadDesktop(GetCurrentThreadId()).map_err(|error| {
            ComputerUseError::failed(format!("cannot read the thread's desktop: {error}"))
        })
    }
}

/// The hidden desktop's virtual-screen bounding rect: origin and size of the
/// WHOLE multi-monitor session, not just the primary monitor. Both capture
/// and input are specified in this space — screen coordinates are
/// virtual-screen-global on Windows (`WindowFromPoint`/`GetWindowRect` all
/// take or return them, negatives included), so hit-testing already spans
/// every monitor; capture composites the same rect and reports it as the
/// `Capture` origin. Re-read per capture: a monitor joining or leaving
/// mid-session changes the metrics, and the query is cheap.
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

/// The virtual pointer's conventional start position: the center of the
/// session's virtual screen (`origin + size / 2`, integer-truncating like a
/// real cursor's default placement; a degenerate/zero-size rect — a session
/// whose monitors all vanished — degrades to the rect's origin, which is what
/// the coordinate space itself collapses to). The additions saturate at the
/// i32 edges: unreachable from `GetSystemMetrics`' 16-bit GDI values, but
/// the helper stays total for any caller. Pure; unit-pinned because it
/// feeds the first hit-test of every session.
fn pointer_center_of_rect(rect: &VirtualScreenRect) -> (i32, i32) {
    (
        rect.origin_x.saturating_add(rect.width / 2),
        rect.origin_y.saturating_add(rect.height / 2),
    )
}

/// Clamp a virtual-screen-global point into the rect — mirroring
/// `SetCursorPos`'s edge adjustment (whose bound is the `ClipCursor` rect,
/// the virtual screen by default), so a coordinate from a stale screenshot
/// taken before a monitor left cannot address a point with no window space
/// behind it. Pure; unit-pinned (negative origins are the normal
/// multi-monitor case; `saturating_add` bounds huge origins, and the
/// `.max(origin)` guard collapses inverted/degenerate rects — zero or
/// negative width — onto the origin instead of panicking in `clamp`).
fn clamp_point_to_rect(point: (i32, i32), rect: &VirtualScreenRect) -> (i32, i32) {
    let max_x = rect.origin_x.saturating_add(rect.width.saturating_sub(1));
    let max_y = rect.origin_y.saturating_add(rect.height.saturating_sub(1));
    (
        point.0.clamp(rect.origin_x, max_x.max(rect.origin_x)),
        point.1.clamp(rect.origin_y, max_y.max(rect.origin_y)),
    )
}

/// Whether a process image path identifies a Microsoft Store **packaged**
/// app: the OS installs those under the per-user/per-machine `WindowsApps`
/// tree, and their windows' processes are the ones whose input pipelines
/// (UWP/WinUI) ignore message-level injection. Pure string check (case-
/// insensitive; the drive/volume prefix plays no part — the match is a plain
/// substring) so it is unit-test pinable; the caller feeds it
/// `QueryFullProcessImageNameW` output. A query failure must NOT be
/// interpreted by this function — it returns no verdict at the call site
/// (best-effort detection, never a hard gate). Residual (advisory-only):
/// a classic app a user placed under a same-named `...\WindowsApps\...`
/// directory would also match — the cost is one spurious "may not have
/// landed" advisory, never a hard gate.
fn process_path_is_packaged(path: &str) -> bool {
    let lowered = path.to_ascii_lowercase();
    lowered.contains("\\windowsapps\\")
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

/// Recover a leftover desktop from a crashed run (name-based
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
            // immediately before the kill. This NARROWS the recycle window;
            // it cannot close it (recycle between the re-check and
            // OpenProcess stays possible — the body's admitted window).
            // The real protection is that the kill set only ever contains
            // pids that owned a window on THIS session's uniquely-named
            // desktop: a recycled innocent process has no window there and
            // never enters the set.
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
        let mut emptied = false;
        for _ in 0..RECOVERY_EMPTY_POLLS {
            if desktop_window_pids(desktop).is_empty() {
                emptied = true;
                break;
            }
            sleep(Duration::from_millis(RECOVERY_POLL_MS));
        }
        if !emptied {
            // Truthful residual: the reopened desktop may still hold ghost
            // windows from the crashed run — the new session's captures and
            // hit-tests can see them until they die.
            log::warn!(
                "[computer_use][virtual-win] the leftover desktop '{name}' still has \
                 windows after the recovery budget; reopening it — ghost windows from \
                 the crashed run may appear in this session's captures"
            );
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
/// documented BSOD recipe (a deliberate caveat) — containment here means "die
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
/// On success returns the child pid AND the spawn's own process handle:
/// the backend retains it so a mid-session wind-down can terminate exactly
/// the processes the session OPENED (by handle — no pid-recycling
/// ambiguity) while leaving the hidden desktop's own shell running; the
/// handle closes when the backend winds the process down or is dropped.
/// On every failure path the produced handles are closed here.
///
/// SAFETY: the two wide buffers live on this stack frame across the call.
pub(crate) unsafe fn launch_suspended_in_job(
    desktop_path_wide: Vec<u16>,
    application: Option<&[u16]>,
    command_line: &str,
    job: HANDLE,
    std_handles: Option<[HANDLE; 3]>,
) -> Result<(u32, HANDLE), ComputerUseError> {
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
        // Std-handle inheritance (the UIA helper's IPC pipes): BOTH halves
        // are required — `STARTF_USESTDHANDLES` wires the ends into the
        // child's std slots, and `bInheritHandles` must be TRUE or the OS
        // duplicates NO handle into the child, leaving the hStd* values
        // naming handles from the PARENT's table that are meaningless
        // there (the helper could neither read a request nor emit the
        // ready line). The child ends are the only inheritable handles in
        // this process's relevant set, so flipping the flag exposes
        // nothing else. Windowed GUI launches pass None and keep the
        // no-inherit default.
        let inherit_handles = std_handles.is_some();
        if let Some([stdin_read, stdout_write, stderr_write]) = std_handles {
            startup.dwFlags |= STARTF_USESTDHANDLES;
            startup.hStdInput = stdin_read;
            startup.hStdOutput = stdout_write;
            startup.hStdError = stderr_write;
        }
        let mut info = PROCESS_INFORMATION::default();
        let application_name = application.map(|path| PCWSTR(path.as_ptr()));
        CreateProcessW(
            application_name.unwrap_or(PCWSTR::null()),
            Some(PWSTR::from_raw(command_buffer.as_mut_ptr())),
            None,
            None,
            inherit_handles,
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
            // Terminate before reporting (fail-closed), and verify what the
            // message then claims: a FAILED terminate means the process may
            // still be running OUTSIDE the session job, where no
            // kill-on-close backstop can ever reach it — the verdict must
            // carry that residual, not assert "terminated" as fact (r4).
            let unconfirmed = match TerminateProcess(info.hProcess, 1) {
                Err(terminate_error) => format!(
                    "; TERMINATION COULD NOT BE CONFIRMED ({terminate_error}) \
                     — the process may still be running outside the session"
                ),
                Ok(()) => String::new(),
            };
            let _ = CloseHandle(info.hProcess);
            let _ = CloseHandle(info.hThread);
            // ACCESS_DENIED is the packaged-app activation shape: a Store
            // (MSIX) process is placed in an OS-owned package job at
            // creation — consistent with the access-denied shape of the
            // one-job rule in the AssignProcessToJobObject remarks (the
            // doc's one-job wording is version-gated; the live signal on
            // Windows 11 is 0x80070005 for programs like Paint whose exe is
            // an app-execution alias resolved by the store broker). The
            // containment is load-bearing — an
            // uncontained process outlives the session AND its window can
            // surface on the USER'S real desktop (broker activation ignores
            // lpDesktop) — so the launch stays fail-closed, but the verdict
            // is `unavailable` (an environment property, not a code fault)
            // and the message names the resolution path instead of a bare
            // OS error.
            if error.code() == E_ACCESSDENIED {
                // The verdict hedges rather than asserts the packaged shape:
                // ACCESS_DENIED is also the generic already-in-another-job
                // rule's code, so the message names the observed dominant
                // cause ("commonly"), keeps the alternative path, and
                // appends the raw OS error the fallback branch keeps.
                return Err(ComputerUseError::unavailable(format!(
                    "the program could not join the hidden-desktop session's \
                     kill-on-close job (ACCESS_DENIED) and was terminated \
                     while still suspended. On Windows 11 this is commonly a \
                     packaged-app (Microsoft Store) activation — e.g. Paint \
                     — whose process the OS places in its own job at \
                     creation; open a classic Win32 executable instead \
                     ({error}){unconfirmed}"
                )));
            }
            return Err(ComputerUseError::failed(format!(
                "the launched process could not be contained by the session job \
                 object and was terminated while still suspended: {error}{unconfirmed}"
            )));
        }
        let resumed = ResumeThread(info.hThread);
        if resumed == u32::MAX {
            // The process is contained (assigned to the kill-on-close job)
            // but still SUSPENDED — it would linger until session end.
            // Terminate it now, mirroring the assign-failure path above,
            // BEFORE the handles close — and this launch never joins the
            // wind-down list (nothing the model could interact with ran).
            // SAFETY: covered by the enclosing block (termination of our own
            // just-launched, still-suspended process handle); a failed
            // terminate is disclosed, not asserted away (r4).
            let terminated = TerminateProcess(info.hProcess, 1);
            let mut message = "the contained process could not be resumed and \
                               was terminated while still suspended"
                .to_string();
            if let Err(error) = terminated {
                message.push_str(&format!(
                    "; TERMINATION COULD NOT BE CONFIRMED ({error}) — the \
                     suspended process may still be lingering (it dies with \
                     the session job either way)"
                ));
            }
            let _ = CloseHandle(info.hProcess);
            let _ = CloseHandle(info.hThread);
            return Err(ComputerUseError::failed(message));
        }
        // The process handle is handed to the caller (the backend's
        // wind-down list); the thread handle has no further use.
        let _ = CloseHandle(info.hThread);
        Ok((info.dwProcessId, info.hProcess))
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
/// them as values). The wrapper lets the capture thread attach the desktop:
/// ownership is SEQUENCED, not exclusive — the backend field holds the same
/// handle value and closes it in `Drop`. The join is BOUNDED: when the
/// capture thread is still busy at the deadline, `Drop` detaches without
/// joining, and safety in that corner comes from the close-skip guard —
/// `Drop` records the detach and skips `CloseDesktop` for a detached
/// thread's desktop (the detached thread keeps using the handle value
/// until the process reclaims everything at exit).
struct SendableDesktop(HDESK);

// SAFETY: HDESK is a kernel object handle — a process-wide token usable from
// any thread; the copy in the moved wrapper is used only for the bounded
// attach at thread start. `Drop` joins the capture thread with a deadline
// and only closes the same handle value when the join SUCCEEDED — a wedged
// thread detaches, and the detached flag makes `Drop` skip the close
// entirely (nothing else ever touches the wrapper's copy after hand-off).
unsafe impl Send for SendableDesktop {}

/// Pooled GDI objects for the composite capture, owned by the capture thread
/// for its whole life (allocation per session — re-sized only when the
/// session's monitor layout changes — not per capture; the physical
/// backend's historical one-bitmap-per-screenshot leak is the trap this
/// pool + the `GetGuiResources` tripwire exist to prevent). The ONE
/// per-capture GDI-object allocation is the output byte buffer `read_rgba`
/// hands to the `Capture` (ownership transfers with the frame, so it cannot
/// live in this pool); it is a plain move-out `Vec`, not a GDI object — no
/// handle-leak class. (Per-capture transient scratch also exists — the
/// Z-walk's window list, per-window class names — but none of it is a GDI
/// object.)
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
            // The request comes from a session window's own GetWindowRect
            // and the pool bitmap is grow-only for the whole session —
            // clamp it to the composite rect (the legitimate maximum: no
            // window can usefully print larger than the session's virtual
            // screen) so a hostile or broken window cannot inflate the
            // pooled allocation once and keep it for the session (R12).
            let w = w.min(self.rect.width).max(1);
            let h = h.min(self.rect.height).max(1);
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
    ///
    /// The `Result` is the honesty rule this function's own comment states:
    /// a failed background clear must not answer a SUCCESSFUL capture of
    /// previous-frame pixels (every other absent-content path here either
    /// fails or is declared in the frame). Callers turn the error into a
    /// failed capture, not a silently stale frame.
    unsafe fn composite_windows(&mut self) -> Result<(), ComputerUseError> {
        // SAFETY: capture-thread composite; see above.
        unsafe {
            // A failed background clear must not composite silently over
            // stale previous-frame pixels (every other absent-content path
            // here fails loudly — the same honesty rule). The generated
            // PatBlt answers `windows_core::BOOL` (unlike the hand-declared
            // PrintWindow below, which returns a raw i32) — `.as_bool()`
            // false is the failure form, the same convention as the
            // IsWindowVisible checks.
            if !PatBlt(
                self.composite_dc,
                0,
                0,
                self.rect.width,
                self.rect.height,
                BLACKNESS,
            )
            .as_bool()
            {
                log::warn!(
                    "[computer_use][virtual-win] capture: the composite background \
                     clear failed; failing this frame instead of compositing \
                     over stale pixels"
                );
                return Err(ComputerUseError::unavailable(
                    "the hidden desktop's capture background clear failed — \
                     the frame would have shown stale pixels, so it was not taken",
                ));
            }
            // The composite's window set is unchanged from before the walk
            // gained the skip predicate: no exclusion, cap-bound stays
            // log-only (r5).
            for hwnd in walk_visible_toplevels(MAX_COMPOSITE_WINDOWS, &|_| false).0 {
                let mut rect = RECT::default();
                if GetWindowRect(hwnd, &mut rect).is_err() {
                    continue; // window died between enumeration and rect read
                }
                // Saturating like the UIA serializers (r8): a hostile app
                // can wrap its own window rect via SetWindowPos, and the
                // plain difference panics (debug) or wraps (release) on
                // the extreme pair.
                let w = rect.right.saturating_sub(rect.left);
                let h = rect.bottom.saturating_sub(rect.top);
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
                    log::warn!(
                        "[computer_use][virtual-win] capture: cannot size the tmp bitmap \
                         for a window; that window is ABSENT from this frame"
                    );
                    continue;
                }
                // Blit the CLAMPED size — the same clamp ensure_tmp_bitmap
                // applied to the allocation. For a window whose rect exceeds
                // the session screen, the unclamped request would read a band
                // of stale tmp pixels beyond what PrintWindow could have
                // rendered while GDI silently clips the overflowing
                // destination: a stale band reported as success, against the
                // file's absent-content honesty rule (R13).
                let blit_w = w.min(self.rect.width).max(1);
                let blit_h = h.min(self.rect.height).max(1);
                let flags = print_flags_for_class(&class);
                if PrintWindow(hwnd, self.tmp_dc, flags) == 0
                    && PrintWindow(hwnd, self.tmp_dc, 0) == 0
                {
                    // Window refuses PrintWindow entirely; skip without failing
                    // the whole frame (the rest still composites) — but say so:
                    // a silently-absent window would read as a successful
                    // capture of content that is not there.
                    log::warn!(
                        "[computer_use][virtual-win] capture: a window refused \
                         PrintWindow; that window is ABSENT from this frame"
                    );
                    continue;
                }
                // The final pixel transfer can fail under GDI pressure just
                // like the two skip paths above; swallowing it would leave a
                // black region that reads as a successful capture of content
                // that is not there — the same honesty rule applies.
                if let Err(error) = BitBlt(
                    self.composite_dc,
                    dest_x,
                    dest_y,
                    blit_w,
                    blit_h,
                    Some(self.tmp_dc),
                    0,
                    0,
                    SRCCOPY,
                ) {
                    log::warn!(
                        "[computer_use][virtual-win] capture: the composite blit \
                         for a window failed ({error}); that window is ABSENT \
                         from this frame"
                    );
                }
            }
        }
        Ok(())
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
    // The REAL previous desktop is read BEFORE the attach (on a fresh thread
    // this is the default desktop; the restore below is what keeps the
    // exit-time association honest).
    let previous = match unsafe { current_thread_desktop() } {
        Ok(previous) => previous,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    // SAFETY: fresh thread, no windows/hooks owned (module doc discipline).
    if let Err(error) = unsafe { attach_thread_to_desktop(desktop) } {
        let _ = ready.send(Err(error));
        return;
    }
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
            unsafe { pool.composite_windows()? };
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

/// The deepest window at a virtual-screen-global screen point — the exact
/// walk [`WindowsVirtualDesktopBackend::hit_test_at`] performs, shared with
/// the UIA helper process (same crate, separate process): both sides must
/// scope `element_at_point` to the SAME window, or the helper's answer and
/// the fallback's answer would disagree. `None` when no window is under the
/// point. MUST run on a thread attached to (or started on) the target
/// desktop — every query here is desktop-relative to the calling thread.
///
/// SAFETY: desktop-relative window queries on the calling thread's desktop.
///
/// Known shape limit (same as the hit-test doc): the `ChildWindowFromPointEx`
/// walk is not sibling-occlusion-aware; rare, accepted and noted.
#[allow(clippy::item_name_repetitions)]
pub(crate) unsafe fn deepest_window_at(x: i32, y: i32) -> Option<(HWND, i32, i32)> {
    // SAFETY: see the fn contract.
    unsafe {
        let top = WindowFromPoint(POINT { x, y });
        if top.is_invalid() {
            return None;
        }
        let mut current = top;
        loop {
            let mut point = POINT { x, y };
            let _ = ScreenToClient(current, &mut point);
            let child =
                ChildWindowFromPointEx(current, point, CWP_SKIPINVISIBLE | CWP_SKIPDISABLED);
            if child.is_invalid() || child.0 == current.0 {
                return Some((current, point.x, point.y));
            }
            current = child;
        }
    }
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
/// die before use — callers treat failures as churn). `skip` excludes
/// windows BEFORE they can consume a cap slot (r5: the tree callers exclude
/// shell classes; the capture composite passes `&|_| false` and keeps its
/// pre-sharing behavior byte-identical). The bool answers whether the cap
/// bound — an absence fact the tree callers must carry into the payload
/// (the model cannot see a missing bottom of the desktop) while the
/// capture caller keeps it log-only as before.
pub(crate) unsafe fn walk_visible_toplevels(
    cap: usize,
    skip: &dyn Fn(HWND) -> bool,
) -> (Vec<HWND>, bool) {
    // SAFETY: see the fn contract.
    unsafe {
        let mut out = Vec::new();
        let Ok(top) = GetTopWindow(None) else {
            return (out, false); // empty desktop
        };
        let mut current = top;
        let mut cap_bounded = false;
        loop {
            if IsWindowVisible(current).as_bool() && !is_cloaked(current) && !skip(current) {
                out.push(current);
                if out.len() >= cap {
                    cap_bounded = true;
                    // The cap binding is an absence path like the
                    // PrintWindow/BitBlt refusals: the bottom-most windows
                    // are silently missing from that frame unless we say so
                    // (the same honesty rule — R12). Shared by the capture
                    // composite and the window-class tree, so the text is
                    // caller-neutral.
                    log::warn!(
                        "[computer_use][virtual-win] the top-level window \
                         cap ({cap}) bound; bottom-most windows are ABSENT \
                         from this pass"
                    );
                    break;
                }
            }
            match GetWindow(current, GW_HWNDNEXT) {
                Ok(previous) => current = previous,
                Err(_) => break,
            }
        }
        out.reverse();
        (out, cap_bounded)
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
pub(crate) unsafe fn window_class_name(hwnd: HWND) -> String {
    // SAFETY: see the fn contract.
    unsafe {
        // Class names legally run to 256 units; a 64-unit buffer silently
        // truncated long framework class names (WinForms/WinUI families).
        let mut buffer = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut buffer);
        if len <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..(len as usize).min(buffer.len())])
    }
}

/// Upper bound on one caption read: captions are short (the display copy is
/// truncated to [`MAX_NAME_CHARS`] downstream), but the T3 screening verdict
/// and the raw binding must run on the FULL raw caption — the exact
/// padding-evasion the contract closes (tool.rs: a display-truncated match
/// lets ~80+ units of padding hide a consequential term past the denylist
/// window) reopens if the read is bounded BELOW the screening input (r5:
/// the 128-unit buffer did exactly that). 4096 units bounds the allocation
/// while being far past any real caption.
const MAX_CAPTION_UNITS: usize = 4096;

/// Window caption/text (length-bounded read; empty string on failure).
/// `GetWindowTextW` reads the OS-cached caption — it never pumps the target
/// thread, so it cannot wedge — which for controls means the static text or
/// button label; the CONTENT of another process's edit control is NOT its
/// caption and is not readable through this query (a disclosed degradation
/// of the window-class tree).
///
/// SAFETY: bounded-buffer string query on a window handle obtained from
/// desktop-relative enumeration on the attached calling thread.
unsafe fn window_text(hwnd: HWND) -> String {
    // SAFETY: see the fn contract.
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let cap = (len as usize).min(MAX_CAPTION_UNITS);
        // +1: GetWindowTextW writes at most `buffer.len()` units INCLUDING
        // the null terminator — without the extra slot an exact-fit caption
        // would lose its last character.
        let mut buffer = vec![0u16; cap + 1];
        let read = GetWindowTextW(hwnd, &mut buffer);
        if read <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..(read as usize).min(cap)])
    }
}

/// Whether a window is a classic password field: the `ES_PASSWORD` style
/// bit, checked ONLY for the classic edit classes (`Edit`, `RichEdit20W`,
/// `RICHEDIT50W`) — `ES_PASSWORD` is defined in those classes' style space,
/// and the same numeric bit names unrelated styles elsewhere (e.g.
/// `BS_LEFTTEXT` on buttons), so an unguarded check would flag arbitrary
/// controls. This covers classic Win32 password boxes including the rich
/// edit family; a password field inside a UIA-only framework (UWP/WinUI)
/// carries no classic style bit and answers false — the same secure=false a
/// non-password box answers, disclosed in the notes as a degradation of
/// the window-class tree. Pure; pinned.
fn password_style_for_class(role: &str, style: isize) -> bool {
    matches!(role, "Edit" | "RichEdit20W" | "RICHEDIT50W") && (style & ES_PASSWORD as isize) != 0
}

/// The class-tree row's effective `disabled` flag: `IsWindowEnabled`
/// semantics WITHOUT the helper (the WindowsAndMessaging module does not
/// export it — the style bit is the r3 replacement) — the enabled state is
/// inherited down the parent chain, so a child of a disabled pane answers
/// disabled even with its own style bit clear. The DFS carries each parent's
/// effective state down in `inherited_disabled`, matching the physical
/// stack's UIA `IsEnabled` at row level. Pure; pinned.
fn effective_disabled(own_style: isize, inherited_disabled: bool) -> bool {
    inherited_disabled || (own_style & (WS_DISABLED.0 as isize) != 0)
}

/// One serialized tree line, in the physical stack's format
/// (`[i] role "name" (x,y,w,h) flags`, two-space depth indents, flags
/// omitted when there are none): the model reads both stacks' trees with
/// one mental model, and `ui_tree`'s tool wrapper appends the same
/// coordinate-space footer either way. Pure; pinned.
fn format_class_tree_line(
    index: u32,
    depth: u32,
    role: &str,
    name: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    flags: &str,
) -> String {
    format!(
        // role renders as Display (a plain &str) — the physical stack's
        // `{control_type:?}` Debug form prints its enum WITHOUT quotes; a
        // Debug &str would print `"Button"` with quotes and break the
        // shared line shape.
        "{}[{index}] {role} \"{name}\" ({x},{y},{width},{height}){flags}",
        "  ".repeat(depth as usize),
    )
}

/// The node budget/cancellation bookkeeping for the window-class tree —
/// the shape of the physical stack's `TreeWriter` (spent budget, node
/// truncation flag, depth truncation flag).
struct HwndTreeWriter {
    next_index: u32,
    remaining: u32,
    truncated: bool,
    truncated_by_depth: bool,
}

/// Serialize one HWND subtree depth-first (the window itself, then each
/// child via `GW_CHILD`/`GW_HWNDNEXT`). A window whose info read fails (it
/// died between enumeration and the read) is skipped WITHOUT consuming
/// budget — destroyed windows must not crowd out slots for live ones, the
/// same churn rule the physical stack applies to fetch failures.
/// `inherited_disabled` rides the recursion so a row's `disabled` flag
/// carries `IsWindowEnabled` semantics (the enabled state is inherited down
/// the parent chain — see [`effective_disabled`]).
///
/// SAFETY: desktop-relative window enumeration + attribute queries on
/// handles obtained from the same walk, on the attached calling thread.
unsafe fn write_class_tree_node(
    hwnd: HWND,
    depth: u32,
    max_depth: u32,
    focused: HWND,
    inherited_disabled: bool,
    writer: &mut HwndTreeWriter,
    out: &mut String,
    cancel: Option<&AtomicBool>,
) -> Result<(), ComputerUseError> {
    if writer.remaining == 0 {
        writer.truncated = true;
        return Ok(());
    }
    // Cancellation checkpoint adjacent to each node read, matching the
    // physical stack: an abandoned request must not keep issuing window
    // queries up to the whole node budget.
    if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
        return Err(ComputerUseError::unavailable(
            "ui_tree was cancelled (caller timeout or stop); the remaining \
             nodes were not fetched",
        ));
    }
    // SAFETY: desktop-relative window queries on handles obtained from the
    // same walk (see the fn contract).
    unsafe {
        let role = window_class_name(hwnd);
        let raw_name = window_text(hwnd);
        let mut rect = RECT::default();
        // A bounds failure means the window died mid-walk: skip without
        // consuming budget (the churn rule above).
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return Ok(());
        }
        writer.remaining -= 1;
        let index = writer.next_index;
        writer.next_index += 1;
        let mut flags = String::new();
        // The disabled flag (via [`effective_disabled`] — the 0.62 crate's
        // WindowsAndMessaging module does not export IsWindowEnabled, and
        // the raw style bit alone would miss parent-inherited disabling): a
        // disabled control displays but refuses input, which the model must
        // see to skip it.
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let disabled = effective_disabled(style, inherited_disabled);
        if disabled {
            flags.push_str(" disabled");
        }
        if focused.0 == hwnd.0 {
            flags.push_str(" focused");
        }
        if !IsWindowVisible(hwnd).as_bool() {
            // An invisible child (a hidden pane, a placeholder control) must
            // not read as an ordinary clickable row: the click/element path
            // skips invisible windows (CWP_SKIPINVISIBLE), so flag the row
            // the way the physical stack flags off-screen elements instead
            // of advertising a phantom target.
            flags.push_str(" hidden");
        }
        if IsIconic(hwnd).as_bool() {
            // GetWindowRect on an iconic window reports the off-screen
            // iconic position (typically -32000,-32000), not its restore
            // rect; the physical stack reports the same shape as UIA's
            // IsOffscreen.
            flags.push_str(" offscreen");
        }
        if password_style_for_class(&role, style) {
            // The physical row's trailing `password` flag (windows.rs pushes
            // it last): the fallback's secure signal must be visible at the
            // row level too, not only on element reads — the notes name the
            // classic ES_PASSWORD style as the fallback's only secure
            // signal (r4).
            flags.push_str(" password");
        }
        // The role (window class) is app-controlled just like the caption:
        // sanitize it with the same bound so a pathological class name
        // cannot forge tree lines.
        let line = format_class_tree_line(
            index,
            depth,
            &sanitize_name(&role, MAX_NAME_CHARS),
            &sanitize_name(&raw_name, MAX_NAME_CHARS),
            rect.left,
            rect.top,
            rect.right.saturating_sub(rect.left).max(0),
            rect.bottom.saturating_sub(rect.top).max(0),
            &flags,
        );
        let _ = writeln!(out, "{line}");
        // Children: the first child (GW_CHILD), then its siblings
        // (GW_HWNDNEXT). An enumeration failure is "this branch ended",
        // the same churn rule.
        let Ok(first_child) = GetWindow(hwnd, GW_CHILD) else {
            return Ok(());
        };
        if depth >= max_depth {
            // Descendants exist below the depth budget but are skipped:
            // flag depth truncation so the footer tells the caller to
            // raise it (the physical stack's contract).
            if !first_child.is_invalid() {
                writer.truncated_by_depth = true;
            }
            return Ok(());
        }
        let mut current = first_child;
        loop {
            if current.is_invalid() {
                break;
            }
            // SAFETY: covered by the enclosing block (same-walk handle).
            write_class_tree_node(
                current,
                depth + 1,
                max_depth,
                focused,
                disabled,
                writer,
                out,
                cancel,
            )?;
            match GetWindow(current, GW_HWNDNEXT) {
                Ok(next) => current = next,
                Err(_) => break,
            }
        }
    }
    Ok(())
}

/// The focused control for the focused-element query and the tree's
/// `focused` flags: the foreground window's thread focus, read WITHOUT
/// stealing focus (the same `GetGUIThreadInfo` read the keyboard target
/// uses, minus the dance). `None` when there is no foreground app window
/// or no focused control on its thread — the caller decides between an
/// honest `None` and an honest error.
///
/// SAFETY: desktop-relative foreground/GUI-thread queries from the
/// attached calling thread.
unsafe fn current_focus_hwnd() -> Option<HWND> {
    // SAFETY: see the fn contract.
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() {
            return None;
        }
        if is_shell_window_class(&window_class_name(foreground)) {
            // The blank-start shell (Progman/WorkerW/Shell_TrayWnd) holds
            // foreground right after creation; its "focus" is not an app
            // focus (the same exclusion the keyboard target applies).
            return None;
        }
        let thread = GetWindowThreadProcessId(foreground, None);
        if thread == 0 {
            return None;
        }
        let mut info = GUITHREADINFO::default();
        info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        if GetGUIThreadInfo(thread, &mut info).is_ok() && !info.hwndFocus.is_invalid() {
            Some(info.hwndFocus)
        } else {
            None
        }
    }
}

/// One HWND as a [`ElementInfo`] for `element_at_point` / `focused_element`:
/// role is the window class, name is the sanitized caption, bounds are the
/// window rect in screen coordinates, and `secure` is the classic
/// `ES_PASSWORD` check (see [`password_style_for_class`]).
///
/// SAFETY: desktop-relative window queries on a handle obtained from a
/// desktop-relative hit test / focus query, on the attached calling thread.
unsafe fn window_element_info(hwnd: HWND, context: &str) -> Result<ElementInfo, ComputerUseError> {
    // SAFETY: see the fn contract.
    unsafe {
        let role = window_class_name(hwnd);
        let raw_name = window_text(hwnd);
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).map_err(|error| {
            ComputerUseError::failed(format!(
                "{context}: the window's bounds could not be read (the window \
                 may have closed): {error}"
            ))
        })?;
        Ok(ElementInfo {
            name_screening_hit: screening_hit(&raw_name),
            raw_binding: raw_element_binding(&raw_name, &role),
            name: sanitize_name(&raw_name, MAX_NAME_CHARS),
            x: rect.left,
            y: rect.top,
            width: rect.right.saturating_sub(rect.left).max(0),
            height: rect.bottom.saturating_sub(rect.top).max(0),
            secure: password_style_for_class(&role, GetWindowLongPtrW(hwnd, GWL_STYLE)),
            role,
        })
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
    /// Total apps this session has launched through [`Self::open_app`];
    /// launches past [`MAX_OPEN_CHILDREN`] are refused with a count-only
    /// error. The count is LIFETIME between releases: apps deliberately
    /// outlive their launch so the record never shrinks on its own, and
    /// [`Self::release_os_grant`] resets it together with the wind-down
    /// list (the Linux stack gates on its live children list instead) —
    /// a stop/revoke restores the full budget.
    opened_app_count: usize,
    /// The window (and client point) that received the split
    /// `mouse_down`'s last press, consumed by the matching `mouse_up`:
    /// real input is captured by the window that received the press
    /// (SetCapture semantics — the same rule the drag path pins its
    /// waypoints to), so the up must go back to THAT window even after the
    /// pointer moved, not to whatever the cursor now hovers over (the
    /// re-hit-test shape stranded the pressed state in the press target and
    /// delivered an orphan up to a bystander — R18). The recorded client
    /// point ADVANCES with every delivered captured move (`move_pointer_to`),
    /// so the up lands at the release-time position — real captured input
    /// posts the up with the current cursor point, and the drag path
    /// likewise delivers its up at the fresh drop point; an up at the stale
    /// press point would resolve release-position-sensitive targets (e.g. a
    /// context menu item) at the wrong spot. A stale target's post fails and
    /// surfaces honestly. The record is single-slot: a second press
    /// overwrites it (the notes disclose the orphaned first target), and
    /// `drag` finishes a held LEFT press before its own assembly.
    held_press: Option<(MouseButton, HWND, i32, i32)>,
    /// The session's **virtual pointer** in virtual-screen-global screen
    /// coordinates. A hidden desktop has no reachable OS cursor (`SetCursorPos`
    /// /`GetCursorPos` require the input desktop to be the current desktop —
    /// winuser.h remarks — and the hidden desktop is never it: both return
    /// ERROR_ACCESS_DENIED from an attached thread, observed live as
    /// 0x80070005), so hit-testing and the synthesized messages track this
    /// state instead. Initialized to the virtual screen's center at creation
    /// (the conventional "cursor at screen center" start), and clamped into
    /// the virtual screen — mirroring `SetCursorPos`'s own clamping — on
    /// every CONSUMED write: `move_to`, drag's start (via `move_to`) and
    /// drag's whole delivered path (the waypoints interpolate the clamped
    /// start into the clamped drop, and a segment between two in-rect points
    /// stays in the rect, so nothing ever observes a raw coordinate).
    pointer: (i32, i32),
    /// Backend-accumulated warnings drained by the trait's `take_warnings`
    /// after each executed action (the packaged-keyboard-target advisory —
    /// see [`Self::warn_if_packaged_keyboard_target`]).
    warnings: Vec<String>,
    /// The spawn-owned process handle of every app this session launched
    /// and left RUNNING (`open_app` successes; a launch that never resumed
    /// was already terminated and closed inside the spawn path). The
    /// wind-down (`release_os_grant`) terminates each recorded process BY
    /// HANDLE — the handle names the exact process object the session
    /// created, so pid recycling cannot redirect a terminate — while the
    /// hidden desktop's own shell stays alive (the Linux stack kills the
    /// opened apps and keeps Xvfb/WM; this is the same contract). Only
    /// these recorded ROOT processes are terminated: processes an opened
    /// app spawned on its own survive until session teardown (the
    /// kill-on-close job — except broker-escaped descendants, see the
    /// notes' broker disclosure). Handles
    /// close on the wind-down or in `Drop`, whichever comes first.
    opened_processes: Vec<HANDLE>,
    /// The attached UIA helper process (spawned lazily on the first
    /// structural query; see `ensure_uia_helper`). `None` = no live helper:
    /// the structural queries then degrade to the window-class walk below.
    uia_helper: Option<UiaHelperClient>,
    /// Earliest moment the helper may be re-spawned after a failure — a
    /// broken environment must not turn every query into a process storm.
    helper_cooldown_until: Option<Instant>,
}

// SAFETY: the raw OS handles (HDESK / HANDLE / HWND) are kernel objects that
// may legally move between threads; they are only ever PASSED to API calls on
// the worker thread (input/query paths) or the capture thread (the GDI pool,
// which never lives in this struct). The worker-thread-confined field
// (`cancel`) moves with the backend itself, which stays on the worker per the
// BackendHandle contract.
unsafe impl Send for WindowsVirtualDesktopBackend {}

impl WindowsVirtualDesktopBackend {
    /// Full creation sequence: recovery probe → CreateDesktopW →
    /// worker attachment → kill-on-close job → explorer shell → shell
    /// readiness → capture thread. Any failure after the desktop exists
    /// returns Err; the partially-built backend's Drop tears down whatever
    /// was already created (never a half-live desktop).
    fn new() -> Result<Self, ComputerUseError> {
        let sequence = DESKTOP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = desktop_name(sequence);
        let name_wide = to_wide(&name);

        // 1. Best-effort leftover recovery (no pidfile on Windows): the name
        //    embeds THIS process's pid + sequence, so a hit can only be a
        //    same-pid leftover (pid recycling + sequence reset — rare). A
        //    cross-process leftover is not recoverable this way; it is
        //    reclaimed by the kernel when the crashed process's job handles
        //    die (kill-on-close), so this probe is belt-and-braces, not the
        //    primary reclamation path.
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

        // The worker's REAL previous desktop, read BEFORE CreateDesktopW:
        // that call typically leaves the calling thread attached to the new
        // desktop (MSDN's assignment wording), so a read taken afterwards
        // would answer with the hidden desktop itself — the recorded
        // "previous" would restore nothing (the Drop restore would re-assign
        // the hidden desktop and report success).
        // SAFETY: thread-info query on this worker thread (no windows yet).
        let original_desktop = unsafe { current_thread_desktop() }?;

        // 2. Fresh desktop (also covers the rare recovery close-busy race:
        //    CreateDesktopW on an existing name opens the existing object).
        // SAFETY: flags/mask per the module's access-mask contract; lpsa None → the desktop inherits
        // its security descriptor from the parent window station (MSDN).
        // WinSta0's DACL grants the interactive user broad access, so other
        // same-user processes can still open it by name — which is the model
        // here: isolation by unique naming, not a security boundary.
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
            worker_original_desktop: Some(original_desktop),
            cancel: None,
            capture_wedges: 0,
            opened_app_count: 0,
            opened_processes: Vec::new(),
            uia_helper: None,
            helper_cooldown_until: None,
            held_press: None,
            pointer: (0, 0),
            warnings: Vec::new(),
        };

        // 3. Attach the worker thread (this thread — BackendHandle runs the
        //    factory on the dedicated worker, which owns no windows); the
        //    real original desktop was already recorded above. The virtual
        //    pointer starts at the session's screen center (the SM_* metrics
        //    describe the session-wide monitor topology; the read still sits
        //    after the attach so every desktop query in the setup sequence
        //    keeps the same ordering).
        // SAFETY: the worker thread owns no windows/hooks (contract).
        unsafe {
            attach_thread_to_desktop(desktop)?;
        }
        backend.pointer = pointer_center_of_rect(&unsafe { desktop_virtual_screen_rect() });

        // 4. Kill-on-close job (NO JOB_OBJECT_UILIMIT_* — BSOD caveat).
        // SAFETY: configuration only.
        unsafe {
            backend.job = create_kill_on_close_job()?;
        }

        // 5. Explorer shell (command shape; explicit
        //    lpApplicationName to avoid command-line parsing ambiguity).
        //    NO folder argument (see explorer_launch_plan: the old
        //    %USERPROFILE% default put the user's home listing into the
        //    first captures).
        let (application, command_line) =
            explorer_launch_plan(std::env::var("WINDIR").ok().as_deref());
        let application_wide = to_wide(&application);
        // SAFETY: contained suspended launch into our job. The shell's
        // process handle is closed immediately: the shell is session
        // infrastructure (it stays for a re-granted session and dies only
        // at Drop via the job), not a recorded app — the wind-down list is
        // for `open_app` launches only.
        let (explorer_pid, explorer_process) = unsafe {
            launch_suspended_in_job(
                to_wide(&desktop_target_path(&name)),
                Some(&application_wide),
                &command_line,
                backend.job,
                None,
            )
        }
        .map_err(|error| {
            ComputerUseError::unavailable(format!(
                "cannot start the hidden desktop's shell (explorer): {error}"
            ))
        })?;
        // SAFETY: close of the shell's own spawn handle — see the launch
        // comment above (the shell is not a wind-down entry).
        let _ = unsafe { CloseHandle(explorer_process) };
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
            // A Disconnected sender means the thread died before its first
            // send (a panic between spawn and ready) — report THAT, not a
            // timeout that never elapsed (R12: the old arm claimed a full
            // wait for an instant failure).
            Err(RecvTimeoutError::Disconnected) => {
                return Err(ComputerUseError::unavailable(
                    "the hidden desktop capture thread exited before signaling ready",
                ));
            }
            Err(RecvTimeoutError::Timeout) => {
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

    /// The virtual pointer's current position (see the `pointer` field doc —
    /// there is no reachable OS cursor on a hidden desktop; the tracked state
    /// IS the session's cursor). Returns virtual-screen-global screen
    /// coordinates, the same space the tool layer's input mapping consumes.
    fn cursor_point(&self) -> (i32, i32) {
        self.pointer
    }

    /// Move the virtual pointer and feed the hover state: updates the
    /// tracked position (clamped into the session's virtual screen — the
    /// same edge adjustment `SetCursorPos` performs), then posts
    /// WM_MOUSEMOVE so hover states/cursors update the way a real move's
    /// arrival does. With a button held (a manual down→move→up assembly),
    /// the move is CAPTURED input — it goes to the remembered press target
    /// with the pressed MK_* mark, exactly like drag's waypoints
    /// (SetCapture semantics; re-hit-testing would deliver moves to
    /// bystander windows and strip the mark, so sliders and canvas gestures
    /// assembled from the primitives would not track). No OS cursor is
    /// touched — `SetCursorPos` is documented as requiring the input
    /// desktop to be the current desktop and fails with
    /// ERROR_ACCESS_DENIED on a hidden desktop regardless of attachment.
    fn move_pointer_to(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        // SAFETY: metric query on the attached worker thread.
        let rect = unsafe { desktop_virtual_screen_rect() };
        self.pointer = clamp_point_to_rect((x, y), &rect);
        let (px, py) = self.pointer;
        // The clamp bounds the point into the virtual screen, but the
        // screen ITSELF can exceed the 16-bit message encoding — refuse the
        // unencodable point instead of silently wrapping it negative.
        if !mouse_point_encodable(px, py) {
            return Err(unencodable_point_error());
        }
        if let Some((pressed_button, press_target, _, _)) = self.held_press {
            // A held-button move carries the pressed mark to the press
            // target, exactly like real captured input (see the doc).
            let (_, _, _, mark) = mouse_button_messages(pressed_button);
            // SAFETY: coordinate translation on a desktop window from
            // the attached worker.
            let (move_x, move_y) = unsafe { Self::client_of(press_target, px, py) };
            let posted = self.post(
                press_target,
                WM_MOUSEMOVE,
                mark,
                pack_mouse_lparam(move_x, move_y),
            );
            // A DELIVERED captured move also advances the remembered press
            // point, so the matching up lands at the release-time position
            // (see the `held_press` field doc). A failed move leaves the
            // record at its last delivered position — the same honesty rule
            // as `mouse_down` (only what actually posted counts).
            if posted.is_ok() {
                self.held_press = Some((pressed_button, press_target, move_x, move_y));
            }
            return posted;
        }
        let Some((target, client_x, client_y)) = self.hit_test_at(px, py) else {
            return Ok(()); // pointer moved; nothing to notify
        };
        // Non-client hover (caption/button highlights): the WM_NCMOUSEMOVE
        // the OS delivers for a real pointer over that zone (wParam is the
        // zone, lParam screen coordinates). A client-zone or an
        // undeliverable zone query falls through to the client move.
        let zone = unsafe { Self::nc_zone_of(target, px, py) };
        if !zone_is_client(zone) {
            return self.post(
                target,
                WM_NCMOUSEMOVE,
                zone as usize,
                pack_mouse_lparam(px, py),
            );
        }
        // A menu popup's client space IS screen space for its tracking loop
        // (the same r5 contract the click route honors): the loop decodes
        // mouse-message lParams as SCREEN coordinates, so a hover posted
        // with translated client coordinates would highlight the wrong item
        // or spawn a stray submenu near the screen origin (r6 — the click
        // route had this, the hover route didn't).
        //
        // SAFETY: class-name query on the hit-tested desktop window.
        let move_lparam = if menu_popup_needs_screen_coords(&unsafe { window_class_name(target) }) {
            pack_mouse_lparam(px, py)
        } else {
            pack_mouse_lparam(client_x, client_y)
        };
        self.post(target, WM_MOUSEMOVE, 0, move_lparam)
    }

    /// Detect a packaged (Microsoft Store) keyboard target and record the
    /// receipt warning: packaged UWP/WinUI input pipelines ignore
    /// message-level injection, so a posted keystroke reports delivery while
    /// the app drops it — the warning is what turns that silent no-op into a
    /// told-to-the-model fact ("verify with a screenshot" is the follow-up
    /// the notes already advise). Best-effort: the process image query can
    /// fail (a dying process, a hardened target) and then no warning is
    /// recorded — the posting itself is unaffected either way.
    fn warn_if_packaged_keyboard_target(&mut self, window: HWND) {
        // SAFETY: pid query on a desktop window from the attached worker.
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        if pid == 0 {
            return;
        }
        // SAFETY: limited-information open of another process (query-only
        // access right); the handle is closed on every path below.
        let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        else {
            return;
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let queried = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
        };
        let _ = unsafe { CloseHandle(process) };
        let Ok(()) = queried else {
            return;
        };
        if process_path_is_packaged(&String::from_utf16_lossy(&buf[..len as usize])) {
            self.warnings.push(
                "the keyboard target is a Microsoft Store packaged app; UWP/WinUI \
                 input pipelines (the common packaged shape — e.g. the Windows 11 \
                 Notepad) ignore message-level keyboard injection, so the typed \
                 text or keys may not have landed (a full-trust Win32 app shipped \
                 via MSIX can still process it) — verify with a follow-up \
                 screenshot and prefer a classic Win32 app for keyboard input"
                    .to_string(),
            );
        }
    }

    /// Client coordinates of a screen point relative to `window` (the drag
    /// path's pinned-target posts need them without re-hit-testing).
    ///
    /// SAFETY: coordinate translation on a desktop window from the attached
    /// worker thread.
    unsafe fn client_of(window: HWND, x: i32, y: i32) -> (i32, i32) {
        // SAFETY: see the fn contract.
        let mut point = POINT { x, y };
        let _ = unsafe { ScreenToClient(window, &mut point) };
        (point.x, point.y)
    }

    /// Hit-test: top-level under the screen point (WindowFromPoint, the
    /// z-order-aware counterpart of the walk below), then the
    /// `ChildWindowFromPointEx` walk to the deepest child at the point,
    /// skipping hidden and disabled children (plain `ChildWindowFromPoint`
    /// is CWP_ALL semantics and hands the click to an invisible placeholder
    /// control — `WindowFromPoint` above skips them, and so does real
    /// input). Returns (deepest, client x, client y) — client coordinates
    /// relative to the deepest window. `None` when no window is under the
    /// point. Worker thread only (attached).
    ///
    /// Known shape limit: `ChildWindowFromPointEx` returns the first DIRECT
    /// child whose bounding rect contains the point, without sibling
    /// occlusion awareness (`WindowFromPoint` above IS z-order-aware) — a
    /// sibling overlapping a parent's child can be skipped over in the walk.
    /// Rare (overlapping siblings in one parent); accepted and noted.
    fn hit_test_at(&self, x: i32, y: i32) -> Option<(HWND, i32, i32)> {
        // SAFETY: desktop-relative queries on the attached worker thread.
        unsafe { deepest_window_at(x, y) }
    }

    /// The live UIA helper, spawning one if none exists (lazy: a session
    /// that never asks structural questions never pays for a second
    /// process). Spawn failures and the post-failure cooldown both answer
    /// `None` — the callers degrade to the window-class walk, and the
    /// cooldown keeps a broken environment from forking a process per
    /// query. The helper is session INFRASTRUCTURE: it is launched through
    /// the same contained path (kill-on-close job) as every child but never
    /// enters the wind-down list, so Stop/revoke leaves it alive like the
    /// shell.
    fn ensure_uia_helper(&mut self) -> Option<&mut UiaHelperClient> {
        if self.uia_helper.is_some() {
            return self.uia_helper.as_mut();
        }
        if let Some(until) = self.helper_cooldown_until {
            if Instant::now() < until {
                return None;
            }
        }
        self.helper_cooldown_until = None;
        match UiaHelperClient::spawn(&self.desktop_name, self.job) {
            Ok(client) => {
                log::info!(
                    "[computer_use][virtual-win] the UIA helper process is up \
                     on the hidden desktop"
                );
                self.uia_helper = Some(client);
                self.uia_helper.as_mut()
            }
            Err(error) => {
                log::warn!(
                    "[computer_use][virtual-win] the UIA helper could not be \
                     spawned or did not become ready (structural queries \
                     degrade to the window-class walk until the cooldown \
                     lapses): {error:?}"
                );
                self.helper_cooldown_until =
                    Some(Instant::now() + Duration::from_millis(HELPER_RESPAWN_COOLDOWN_MS));
                None
            }
        }
    }

    /// The window's hit-test ZONE at the screen point: the answer the OS
    /// itself would route real input by (`WM_NCHITTEST` — DefWindowProc
    /// classifies caption/buttons/menu/borders from the window's own
    /// non-client geometry, and self-drawn title bars override it with
    /// their own zones, so both standard and custom chrome classify the
    /// same way real input does). The deepest hit-tested window is queried
    /// with a bounded wait: unlike the PostMessageW posts, a SendMessage
    /// blocks on the target thread, so `SMTO_ABORTIFHUNG` plus
    /// [`NC_HITTEST_TIMEOUT_MS`] keeps a wedged app from stalling the
    /// click — a timeout/failure answers "client" ([`zone_is_client`]),
    /// the pre-routing behavior, instead of failing the action (a delivered
    /// HTTRANSPARENT/HTNOWHERE/HTERROR answer degrades the same way: zones
    /// that ask to be bypassed get the pre-routing client posts, not an NC
    /// message with a zone code they never asked for — a transparent window
    /// will usually swallow those posts, which [`Self::click`]'s receipt
    /// warning discloses rather than hiding).
    ///
    /// SAFETY: cross-desktop SendMessage with a timeout to a window handle
    /// from the attached worker's own hit test; the out-pointer is a valid
    /// usize slot.
    unsafe fn nc_zone_of(window: HWND, x: i32, y: i32) -> u32 {
        let mut result: usize = 0;
        // SAFETY: see the fn contract.
        let delivered = unsafe {
            SendMessageTimeoutW(
                window,
                WM_NCHITTEST,
                WPARAM(0),
                LPARAM(pack_mouse_lparam(x, y)),
                SMTO_ABORTIFHUNG,
                NC_HITTEST_TIMEOUT_MS,
                Some(&mut result),
            )
        };
        // A zero return means the message was NOT delivered (timeout, hung
        // target, dying window): the safe answer is the client route, which
        // is what this backend posted before routing existed.
        if delivered.0 == 0 {
            HTCLIENT
        } else {
            result as u32
        }
    }

    /// A click whose hit-test zone is a NON-CLIENT zone. wParam carries the
    /// hit-test code (not the MK_* mark) and lParam the SCREEN point — the
    /// NC message contract. The per-zone delivery follows what the OS
    /// actually does with a real press ([`NcZoneDelivery`], verified
    /// against ReactOS's win32k mirror), because the DefWindowProc handlers
    /// of several zones run MODAL states a uniform (down, up) post pair
    /// cannot drive:
    /// - chrome buttons (min/max/close): `WM_SYSCOMMAND` with the
    ///   style-derived SC code — the deterministic end state of the OS's
    ///   button tracking (whose modal pump a posted NC up can never
    ///   satisfy); a disabled/absent Close is refused, not faked;
    /// - caption: DOWN presses are completed with a client `WM_LBUTTONUP`
    ///   at the same point (deterministically exits the `SC_MOVE` loop the
    ///   DOWN opens — zero movement, activation kept); double-click presses
    ///   post `WM_NCLBUTTONDBLCLK` (loop-free `SC_RESTORE`/`SC_MAXIMIZE`);
    /// - menu bar / system menu: the press opens the menu's tracking loop
    ///   (that loop IS the opened menu); a menu POPUP's items are clicked
    ///   with screen coordinates through the client route
    ///   ([`menu_popup_needs_screen_coords`]);
    /// - resize borders and scroll bars: REFUSED before any post — a press
    ///   there enters a modal move/size/scroll loop posted sequences cannot
    ///   complete;
    /// - everything else: the plain NC (down, up) pair — DefWindowProc
    ///   treats those zones as no-ops.
    ///
    /// Deliberately NOT supported here (disclosed in the capability notes):
    /// window MOVE by caption drag and border-drag resize need the
    /// `SC_MOVE`-style modal loop pumping live coordinates — this backend
    /// delivers presses, not a live pointer stream; and the primitives
    /// `left_mouse_down`/`left_mouse_up` keep posting CLIENT messages, so a
    /// manual down→up assembly neither synthesizes these zones (use
    /// `click`) nor special-cases menu popups (use `click` there too).
    fn nc_click(
        &mut self,
        target: HWND,
        button: MouseButton,
        count: u8,
        zone: u32,
        screen_x: i32,
        screen_y: i32,
    ) -> Result<(), ComputerUseError> {
        // Refuse BEFORE any post: the modal-loop families enter the loop on
        // the very first press, leaving the target captured in move/size/
        // scroll mode behind a reported success — the disclosed
        // move/resize unsupportedness applies to CLICKS there too, so the
        // honest answer is the refusal, not a wedged window.
        if matches!(nc_zone_delivery(zone), NcZoneDelivery::ModalLoop) {
            return Err(ComputerUseError::unavailable(
                "this click lands on a window edge, border, or scrollbar; \
                 pressing there enters the modal move/size/scroll loop that \
                 only live pointer or keyboard input can complete — window \
                 move/resize by dragging is not supported on this stack \
                 (the same disclosure as title-bar dragging)",
            ));
        }
        // Caption buttons answer the primary button only — real input is
        // ignored on every other button, so a non-primary press must not
        // synthesize an SC the window would never have fired.
        if matches!(nc_zone_delivery(zone), NcZoneDelivery::ChromeButton)
            && button != MouseButton::Left
        {
            return Err(ComputerUseError::unavailable(
                "caption buttons answer the primary button only; the \
                 secondary press is a no-op like real input",
            ));
        }
        // SAFETY: style + system-menu state reads on the hit-tested desktop
        // window (server-side queries; neither pumps the target thread).
        let style = unsafe { GetWindowLongPtrW(target, GWL_STYLE) };
        // Real input is ignored on a disabled window ENTIRELY — the OS
        // routes no click to it, so no NC delivery may be synthesized here.
        // Least of all a system command: a posted `SC_CLOSE` would close a
        // window no real click could touch (the same over-delivery class
        // the chrome style gates close; r7). The hit-test walk skips
        // disabled CHILDREN but `WindowFromPoint` still answers a disabled
        // top-level, so this gate is reachable.
        if style & WS_DISABLED.0 as isize != 0 {
            return Err(ComputerUseError::unavailable(
                "the window is disabled; the OS routes no real input to it, \
                 so the action was not delivered",
            ));
        }
        if matches!(nc_zone_delivery(zone), NcZoneDelivery::ChromeButton) && zone == HTCLOSE {
            let menu = unsafe { GetSystemMenu(target, false) };
            let close_enabled = !menu.is_invalid()
                && nc_close_command_enabled(unsafe { GetMenuState(menu, SC_CLOSE, MF_BYCOMMAND) });
            if !close_enabled {
                return Err(ComputerUseError::unavailable(
                    "the window's close command is disabled or absent (the \
                     same check the OS button tracking performs) — there is \
                     nothing to press",
                ));
            }
        }
        // NC lParam is screen coordinates — no client translation.
        let lparam = pack_mouse_lparam(screen_x, screen_y);
        for index in 0..count.max(1) {
            if self.cancelled() {
                return Err(ComputerUseError::unavailable(
                    "click was cancelled (caller timeout or stop); already \
                     injected clicks are not undone",
                ));
            }
            if index == 0 {
                // Real input activates the window on the first press (a
                // title-bar click raises it before whatever the zone does);
                // same first-press rule as the client route, and behind the
                // cancel check like it — an already-cancelled click must not
                // steal foreground.
                self.focus_dance(target);
            }
            match nc_zone_delivery(zone) {
                NcZoneDelivery::ChromeButton => {
                    // The style gates run per press (like NC_DoButton, the
                    // style was snapshotted before the loop — a multi-click
                    // on a button whose window changed style mid-sequence
                    // is churn the multi-click warning below already
                    // discloses). None = the OS button tracking would
                    // ignore this press (missing WS_MINIMIZEBOX/
                    // WS_MAXIMIZEBOX/WS_SYSMENU): refuse, don't synthesize
                    // a command real input could never fire.
                    let Some(command) = nc_chrome_system_command(zone, style) else {
                        return Err(ComputerUseError::unavailable(
                            "the window's chrome does not offer this button \
                             (the style bits the OS button tracking requires \
                             are absent, so a real press there is a no-op)",
                        ));
                    };
                    self.post(target, WM_SYSCOMMAND, command as usize, lparam)?;
                    if index + 1 < count.max(1) {
                        // Deterministic churn disclosure: the first press
                        // already ran the command (minimized / maximized /
                        // closed); a second SC either re-applies it or —
                        // for close — lands on a window that is gone,
                        // which the failed post surfaces as the
                        // stranded-button honesty below.
                        self.warnings.push(
                            "multi-click on a caption button: the first \
                             press already ran its system command; the \
                             remaining presses may target a changed (or \
                             closed) window"
                                .to_string(),
                        );
                    }
                }
                NcZoneDelivery::Caption => {
                    let (message, completion) = nc_caption_press(index, button);
                    self.post(target, message, zone as usize, lparam)?;
                    if let CaptionCompletion::MoveLoopExit = completion {
                        // Deterministic completion of the `SC_MOVE` loop
                        // the DOWN opened: a client `WM_LBUTTONUP` at the
                        // same point — the loop's own exit arm — ends the
                        // move mode with zero movement and keeps the
                        // activation. The loop retrieves the message via
                        // PM_REMOVE, so the app never sees a stray client
                        // UP.
                        let mut point = POINT {
                            x: screen_x,
                            y: screen_y,
                        };
                        // SAFETY: client translation of the posted point on
                        // the hit-tested desktop window.
                        let _ = unsafe { ScreenToClient(target, &mut point) };
                        self.post(target, WM_LBUTTONUP, 0, pack_mouse_lparam(point.x, point.y))?;
                    }
                    if let CaptionCompletion::SystemMenuRelease = completion {
                        // The DOWN opened a capture pump filtered to
                        // WM_MOUSEFIRST..WM_MOUSELAST that exits on a
                        // WM_RBUTTONUP — by message type alone (the r5
                        // screen-coord contract belongs to the menu
                        // tracking loop, not this pump: the posted UP's
                        // decoded point is the queue's cursor at post
                        // time, not its lParam). The UP deterministically
                        // ends what the DOWN started; whether the system
                        // menu opens afterward follows the physical
                        // cursor, as for a real release (r8 note).
                        self.post(
                            target,
                            WM_RBUTTONUP,
                            0,
                            pack_mouse_lparam(screen_x, screen_y),
                        )?;
                    }
                }
                _ => {
                    // Menu bar / system menu / no-op zones: the plain NC
                    // (down, up) pair. The menu DOWN opens the tracking
                    // loop (the opened menu); the NC up is consumed or
                    // dispatched harmlessly by whatever state the target
                    // is in.
                    let (message, up) = nc_press_pair(index, button);
                    self.post(target, message, zone as usize, lparam)?;
                    // Mirror the client route's stranded-button honesty: a
                    // failed NC up must surface the still-held fact.
                    if let Err(error) = self.post(target, up, zone as usize, lparam) {
                        return Err(ComputerUseError::failed(format!(
                            "{error}; the button may still be pressed"
                        )));
                    }
                }
            }
            if index + 1 < count.max(1) {
                sleep(Duration::from_millis(MULTI_CLICK_INTERVAL_MS));
            }
        }
        Ok(())
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
            let foreground_app = if !foreground.is_invalid() {
                // The shell windows (Progman/WorkerW/Shell_TrayWnd) are
                // excluded here exactly as in the fallback walk below: right
                // after the blank-shell start, explorer's Progman HOLDS
                // foreground on the hidden desktop, and an unchecked
                // foreground return posted the keystrokes into the shell and
                // reported success — the honest "no window has keyboard
                // focus" error never got a chance to fire.
                let class = unsafe { window_class_name(foreground) };
                if !class.is_empty() && is_shell_window_class(&class) {
                    None
                } else {
                    Some(foreground)
                }
            } else {
                None
            };
            foreground_app
                .or_else(|| {
                    // SAFETY: same.
                    unsafe { topmost_visible_app_window() }
                })
                .ok_or_else(|| {
                    ComputerUseError::unavailable(
                        "no window on the hidden desktop has keyboard focus; \
                         click a window (or open an app with the open action) \
                         before typing",
                    )
                })?
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

    /// Press the chord on the keyboard target: the Alt-held system pair goes
    /// to the target's top-level window so frame-routed SC_CLOSE /
    /// SC_KEYMENU handling fires, the plain pair goes to the focused
    /// control — ordinary keyboard input is delivered to the focus window,
    /// and posting it to the frame makes frame-routed apps (Notepad's frame
    /// dispatches to DefWindowProc) silently drop it while PostMessageW
    /// reports success. Hold for `hold`, release in reverse to the SAME
    /// window the presses went to. Modifiers first; the whole chord is
    /// mapped before anything is pressed (a mapping failure presses nothing).
    fn chord_to(&mut self, keys: &[Key], hold: Duration) -> Result<(), ComputerUseError> {
        if keys.is_empty() {
            return Err(ComputerUseError::failed("key chord is empty"));
        }
        // Modifiers lead, the rest keep the given order (shared with the
        // pin via partition_modifiers_first).
        let (modifiers, rest) = partition_modifiers_first(keys);
        let synth_shift = matches!(chord_shift_plan(&modifiers, &rest)?, ShiftSynthesis::Insert);
        let mut ordered: Vec<u16> = modifiers
            .iter()
            .chain(rest.iter())
            .map(|key| key_to_vk(**key))
            .collect::<Result<_, _>>()?;
        // A shifted character's key position posted plain delivers its
        // UNSHIFTED face — `key "plus"` used to type `=` while reporting
        // success (R18). `VK_SHIFT` joins between the modifiers and the
        // character: pressed first, released (in reverse) right after the
        // character, mapped through the same scan-code/extended lparam
        // helper as every other key, and included in the list the
        // whole-chord message plan is derived from.
        if synth_shift {
            ordered.insert(modifiers.len(), VK_SHIFT.0);
        }
        let focus = self.keyboard_target()?;
        // Same packaged-target honesty as type_text: a Store-app chord target
        // drops the posted keys silently.
        self.warn_if_packaged_keyboard_target(focus);
        // SAFETY: parent-walk on a desktop window from the attached worker.
        let top = unsafe { top_level_of(focus) };
        // Real Windows delivers EVERY key pressed while Alt is held as the
        // system-key pair with the context bit set — not just VK_MENU
        // itself (see chord_message_plan; the selection is derived once per
        // chord from the full key list, never per key).
        let (down_message, up_message, alt_held) = chord_message_plan(&ordered);
        let post_target = chord_post_target(focus, top, alt_held);
        let mut pressed: Vec<u16> = Vec::with_capacity(ordered.len());
        let hold_result = (|| {
            for vk in &ordered {
                self.post(
                    post_target,
                    down_message,
                    usize::from(*vk),
                    key_message_lparam(*vk, alt_held, false),
                )?;
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
        // Release in reverse to the SAME window the presses went to,
        // regardless of how the hold went (a cancelled hold must not strand
        // modifiers).
        let mut release = Ok(());
        for vk in pressed.iter().rev() {
            if let Err(error) = self.post(
                post_target,
                up_message,
                usize::from(*vk),
                key_message_lparam(*vk, alt_held, true),
            ) {
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
    /// job. Returns the child pid AND its spawn process handle — the
    /// wind-down list's terminate handle (`open_app` retains it; the shell
    /// and helper spawn sites close theirs immediately, being session
    /// infrastructure rather than wind-down entries).
    fn launch_in_job(
        &self,
        application: Option<&str>,
        command_line: &str,
    ) -> Result<(u32, HANDLE), ComputerUseError> {
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
                None,
            )
        }
    }
}

/// The model-facing `Capabilities.notes` string, assembled by a pure helper
/// so the test module can pin the real text the model receives (the macOS
/// stack's `capability_notes_disclose_the_target_contract` shape — the R9
/// review found this disclosure could silently revert because no test
/// referenced it).
fn capability_notes() -> String {
    "windows hidden desktop: input is app-level message injection \
     (PostMessageW) — apps that ignore synthetic messages cannot be \
     driven (Microsoft Store packaged apps — UWP/WinUI, including the \
     Windows 11 Notepad — are the common case; the tool attaches an \
     explicit warning to the receipt when the keyboard target is a \
     packaged app), shell-managed chords (e.g. Alt+Tab) are handled by the \
     shell rather than the posted-to app and cannot fire at all, \
     typed dialog keys (Enter/Tab delivered by type_text) ride the \
     WM_CHAR text channel, which dialog managers do not act on — the \
     action reports success while the dialog's default button or \
     focus cycle does not fire, non-client clicks follow the OS's own \
     per-zone delivery (title-bar double-click maximizes/restores, the \
     caption min/max/close buttons fire their system commands — a \
     disabled or absent Close is refused, not faked — and a menu-bar or \
     system-menu press opens the menu, whose items are then clicked with \
     screen coordinates; while a menu's tracking loop is open it consumes \
     clicks aimed elsewhere and dismisses on Escape, so drive menus \
     deliberately one step at a time); clicks on window edges, borders, and \
     scrollbars are refused (a press there enters a modal move/size/\
     scroll loop posted messages cannot complete); the non-client route \
     also refuses disabled windows outright (the OS routes them no real \
     input), while clicks and drags that resolve to a client area are \
     still posted to such a window (its procedure receives posted \
     messages regardless of the enabled state, so the effect there is \
     app-defined rather than the guaranteed swallow real input would \
     produce); zones that ask the OS \
     to pass input through them (transparent static labels and group \
     boxes) and points whose hit-test cannot be classified fall back to \
     the plain client-area posts instead of non-client messages, and the \
     receipt warns that such a click may be swallowed (verify with a \
     screenshot; a probe that fails or times out on a busy target \
     answers client and degrades without this warning); but a \
     window cannot be moved \
     by dragging its title bar or resized by dragging a border (those \
     need a live pointer stream this injection cannot provide) and the \
     manual left_mouse_down/left_mouse_up primitives stay client-side \
     (use click for non-client zones and for menu items), \
     structural queries (ui_tree / element_at_point / focused_element) \
     run against a real UI Automation tree served by an attached helper \
     process (this app re-spawned on the hidden desktop, inside the \
     session job; the first structured query pays the helper's spawn \
     plus COM initialization); when that helper cannot start or has \
     died (a transport failure), the same queries fall back to a \
     window-class walk whose roles are window classes and names are \
     window captions (a helper that answers but cannot serve a query \
     surfaces its reason as that query's error instead of the walk; \
     cross-process control content is not read, UIA-only properties are \
     unavailable, and the fallback's only secure-field signal is the \
     classic edit-family ES_PASSWORD style (UIA-only password fields \
     answer secure=false); hidden panes appear flagged 'hidden' \
     rather than as clickable rows; shell windows are excluded (from \
     trees and from the keyboard-target fallback) and the \
     top level is capped, with a cap-bound walk disclosed in its \
     footer — Windows 11's shell XAML hosts may still appear as noise \
     roots, and the same-class caveat cuts the other way too: a \
     same-desktop app can name its window class like a shell window \
     and vanish from trees and keyboard-target selection that way; \
     the fallback's screening verdict runs on the raw caption \
     bounded at 4096 UTF-16 units (a caption past that bound is screened \
     on its first 4096 units — the bound exists so a hostile app cannot \
     make the walk allocate unboundedly, and any real caption is far \
     shorter)), \
     posted chords do not update the app's async \
     key-state table (apps that read modifier state from \
     GetKeyState may see unmodified keys), scroll events go \
     to the window under the pointer rather than the focus \
     window (a focused window that delegates scrolling then \
     misses a scroll real wheel input would deliver, and the \
     posted message carries no modifier state — Ctrl+wheel \
     zoom is unrepresentable), a held key posts a single \
     down/up pair rather than auto-repeating (hold on a \
     repeating key yields one event, not a stream), and \
     layered-window \
     translucency can print as opaque fringes (the hidden desktop \
     has no DWM to blend it); key chords press US-layout key \
     positions, so an app running a non-US keyboard layout may \
     resolve a chord to a different character than a US layout \
     would (typed text is not affected — it is delivered as text); \
     apps that delegate their launch to a system broker (e.g. a \
     calculator stub activating a store app) get their real \
     process created OUTSIDE the session's job object — such apps \
     are not contained by the session and are not terminated with \
     it, and a program that resolves to a packaged-app activation \
     outright (Paint on Windows 11 is the observed case) cannot join \
     the session job at all and fails the open with an explicit \
     error; a console program's window is delegated to the default \
     Windows Terminal host (packaged, broker-activated outside the \
     hidden desktop), so a console open can fail the same way or come \
     up with no visible window — verify with a screenshot instead of \
     retrying blindly; stopping the session (the Stop button, a grant \
     revoke, or the master switch) winds down the apps opened on the \
     hidden desktop — only the launched app processes themselves are \
     terminated, child processes an opened app spawned are not wound \
     down and keep running until the session is torn down (the \
     kill-on-close job reclaims them then — except descendants that \
     themselves escape the job through a system broker, which like the \
     broker-delegated opens above live outside it and outlive the \
     session) — and the launched \
     processes are ended without a grace period, so unsaved work \
     inside them is lost — while the desktop and its \
     shell stay alive for a re-granted session, and everything, shell \
     included, is reclaimed when the session is torn down; winding \
     the opened apps down also restores the open budget (fresh open \
     calls work again, up to the same launch cap); there is no \
     OS cursor on the hidden desktop \
     (the input-desktop cursor APIs are documented to fail there), \
     so the session tracks a virtual pointer that starts at the \
     screen center and follows every move and drag; a manual press \
     assembly tracks a single button — a second mouse_down replaces \
     the tracked press, the first target stays pressed, and only a \
     matching up that still reaches it (its button released while the \
     pointer hovers it) or the session end releases it — so release a \
     held button before pressing another; a drag finishes a \
     still-held left press first, delivering its recorded up at the \
     held point before the drag's own press; the hidden \
     desktop shares this user account's clipboard \
     with the user's own desktop (a window station has one \
     clipboard for all its desktops) — what the agent copies or \
     cuts inside the session reaches the user's real clipboard and \
     what the user holds on theirs reaches the session's apps; \
     the hidden desktop's name is derived from the owning process \
     (unique but unprivileged): isolation is by unique naming, not an \
     access boundary — a same-user process that pre-creates the name \
     could place its own windows on the session surface, and a program \
     INSIDE the session can open the desktop by its name and switch the \
     user's view to it (pulling the session surface onto the user's \
     screen); \
     the session's first action can take several seconds \
     while the hidden desktop and its shell start; the shell \
     (explorer) readiness is checked once at that start — if the \
     shell exits mid-session its wallpaper and taskbar vanish \
     from captures while opened apps keep working (they do not \
     depend on the shell) and nothing restarts it; capture and \
     input span the session's \
     whole virtual screen (every monitor of the hidden-desktop \
     session); capture is a per-window composite (a hidden desktop \
     has no DWM), so content can lag until the forced redraw \
     lands, a window that refuses capture is absent from that \
     frame, and a cloaked UWP app window can print as a blank \
     rectangle (DWM cloaking cannot be queried on a hidden \
     desktop); verify actions with a follow-up screenshot"
        .to_string()
}

/// The model-facing `Capabilities.target_note` string (see
/// [`capability_notes`] for why this is a pure helper).
fn capability_target_note() -> String {
    "isolated hidden Windows desktop — the user's desktop stays \
     untouched except that apps delegating their launch to a \
     system broker (e.g. a store-app stub) get their real \
     process created outside this session: such an app can \
     surface a window on the user's desktop, is not contained \
     by the session, and is not terminated with it; \
     the hidden desktop shares the user's clipboard with their \
     own desktop (agent copies reach the user's real clipboard \
     and theirs reach the session's apps); \
     desktop isolation is by unique naming, not an access boundary \
     (a same-user process that pre-creates the desktop name could \
     place windows on the session surface, and a program inside the \
     session can open it by name and pull the session surface onto \
     the user's screen); \
     keyboard/mouse reach apps as window messages and \
     some apps ignore synthetic messages; screenshots are \
     per-window composites (no DWM on a hidden desktop) covering \
     the session's whole virtual screen (all monitors); UAC \
     elevation prompts appear on the secure desktop and cannot \
     be driven; stopping the session winds down the apps opened on \
     the hidden desktop (the launched processes only — child \
     processes an app spawned keep running until session \
     teardown; no grace period — unsaved work inside the \
     terminated apps is lost) while the desktop and its shell \
     remain for a re-granted session until the backend is dropped \
     at session end"
        .to_string()
}

impl ComputerUseBackend for WindowsVirtualDesktopBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            screenshot: true,
            input: true,
            ui_tree: true,
            notes: capability_notes(),
            target: DisplayTarget::Virtual,
            target_note: capability_target_note(),
        }
    }

    fn capture(&mut self) -> Result<Capture, ComputerUseError> {
        self.capture_via_thread()
    }

    fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
        Ok(self.cursor_point())
    }

    fn move_to(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        // The virtual pointer is what click/scroll hit-test against; the
        // WM_MOUSEMOVE post additionally feeds hover states.
        self.move_pointer_to(x, y)
    }

    fn click(&mut self, button: MouseButton, count: u8) -> Result<(), ComputerUseError> {
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        let (x, y) = self.cursor_point();
        if !mouse_point_encodable(x, y) {
            return Err(unencodable_point_error());
        }
        let Some((target, client_x, client_y)) = self.hit_test_at(x, y) else {
            return Err(ComputerUseError::unavailable(
                "there is no window under the cursor on the hidden desktop; \
                 move_to a window first",
            ));
        };
        // Route by the window's own hit-test zone: a point over the caption,
        // a system button, the menu bar, or any other non-client area is
        // NC input the client-message posts never modeled (the pre-routing
        // click posted WM_LBUTTONDOWN into the deepest window and was
        // ignored by every standard non-client handler). A client zone —
        // or a zone query that failed/timed out — stays on the client
        // route below.
        let zone = unsafe { Self::nc_zone_of(target, x, y) };
        if !zone_is_client(zone) {
            return self.nc_click(target, button, count, zone, x, y);
        }
        if zone_is_bypassed(zone) {
            // No honest NC sequence exists for this answer: the window
            // asked the OS to pass input through (HTTRANSPARENT), or the
            // point answered nowhere/error. The client posts below land on
            // THIS window and are usually swallowed (the hit-test walk has
            // no transparent skip) — attach the receipt so a silently-lost
            // click is a told-to-the-model fact instead of a bare success
            // (r4; r7 extended the warning to HTNOWHERE/HTERROR, which
            // used to degrade silently).
            self.warnings.push(
                "the click routed to the client messages without a drivable \
                 zone (transparent, nowhere, or error); the posted client \
                 click may be swallowed — verify with a follow-up screenshot"
                    .to_string(),
            );
        }
        // A menu popup opened by an NC press is a tracking loop that reads
        // mouse-message lParams as SCREEN coordinates (r5; the menu item is
        // a client region of the popup, so the client translation below
        // would land the click near the screen origin instead of on the
        // item). Every other target keeps the client route's translated
        // point.
        //
        // SAFETY: class-name query on the hit-tested desktop window.
        let lparam = if menu_popup_needs_screen_coords(&unsafe { window_class_name(target) }) {
            pack_mouse_lparam(x, y)
        } else {
            pack_mouse_lparam(client_x, client_y)
        };
        let (down, double, up, mark) = mouse_button_messages(button);
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
        let (x, y) = self.cursor_point();
        if !mouse_point_encodable(x, y) {
            return Err(unencodable_point_error());
        }
        let Some((target, client_x, client_y)) = self.hit_test_at(x, y) else {
            return Err(ComputerUseError::unavailable(
                "there is no window under the cursor on the hidden desktop",
            ));
        };
        let (down, _, _, mark) = mouse_button_messages(button);
        self.focus_dance(target);
        self.post(target, down, mark, pack_mouse_lparam(client_x, client_y))?;
        // Only a successfully delivered press is remembered (a failed down
        // pressed nothing); a re-press overwrites — the newest press is the
        // one the matching up must release.
        self.held_press = Some((button, target, client_x, client_y));
        Ok(())
    }

    fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        let (_, _, up, _) = mouse_button_messages(button);
        // Deliver the up to the REMEMBERED press target at the recorded
        // point (SetCapture semantics for the target; the point advances
        // with delivered captured moves — see the `held_press` field doc).
        // A different button's up does not consume the record: the press it
        // names is still held.
        if let Some((pressed_button, target, client_x, client_y)) = self.held_press.take() {
            if pressed_button == button {
                return self.post(target, up, 0, pack_mouse_lparam(client_x, client_y));
            }
            self.held_press = Some((pressed_button, target, client_x, client_y));
        }
        let (x, y) = self.cursor_point();
        // No encodability guard here: the up is the button's release — the
        // emergency release path relies on it being best-effort and
        // non-fatal, so an exotic-coordinate release must attempt delivery
        // (a wrapped release on an oversized virtual screen beats a
        // refused one that strands the press).
        let Some((target, client_x, client_y)) = self.hit_test_at(x, y) else {
            // An up with no window under the cursor is a harmless no-op —
            // the emergency release path relies on this being non-fatal.
            return Ok(());
        };
        self.post(target, up, 0, pack_mouse_lparam(client_x, client_y))
    }

    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError> {
        self.move_to(from.0, from.1)?;
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        // Hit-test the CLAMPED pointer, not the raw `from`: `move_to` above
        // clamped an out-of-range start into the virtual screen (and posted
        // the hover for it), so the press must consume the same position or
        // a valid clamped start would falsely report "no window".
        let (start_x, start_y) = self.cursor_point();
        let Some((target, client_x, client_y)) = self.hit_test_at(start_x, start_y) else {
            return Err(ComputerUseError::unavailable(
                "there is no window at the drag start point on the hidden desktop",
            ));
        };
        // The drag start is non-client-blind no longer (r6): a border/
        // scrollbar press enters a modal move/size/scroll loop the client
        // posts cannot complete — the same refusal the click route gives —
        // and any other non-client start (title bar, menu bar, caption
        // button) gets the likely-no-effect receipt instead of a bare
        // success (client-area drag simulation cannot move windows or press
        // non-client controls; the notes disclose the same limitation).
        let start_zone = unsafe { Self::nc_zone_of(target, start_x, start_y) };
        if matches!(nc_zone_delivery(start_zone), NcZoneDelivery::ModalLoop) {
            return Err(ComputerUseError::unavailable(
                "the drag starts on a window edge, border, or scrollbar; \
                 pressing there enters the modal move/size/scroll loop that \
                 only live pointer or keyboard input can complete — window \
                 move/resize by dragging is not supported on this stack",
            ));
        }
        if zone_is_bypassed(start_zone) {
            // Same receipt family as the click route (r8): a transparent/
            // nowhere/error answer has no drivable zone, the popup class
            // check below does not cover it, and the presses land on THIS
            // window and are usually swallowed — a silently-lost press is
            // a told-to-the-model fact, not a bare success.
            self.warnings.push(
                "the drag starts on a point whose hit-test cannot be \
                 driven (transparent, nowhere, or error); the pressed-\
                 then-released client assembly may be swallowed — verify \
                 with a follow-up screenshot"
                    .to_string(),
            );
        } else if !zone_is_client(start_zone) {
            self.warnings.push(
                "the drag starts on non-client area (title bar, menu bar, \
                 or a caption button); client-area drag simulation cannot \
                 move a window or press non-client controls, so this drag \
                 likely had no effect beyond focusing the target — use \
                 click for non-client zones"
                    .to_string(),
            );
        }
        if !mouse_point_encodable(start_x, start_y) {
            return Err(unencodable_point_error());
        }
        // The clamped drop point is computed BEFORE the press (and guarded
        // with it): refusing after `WM_LBUTTONDOWN` would strand the pressed
        // button. The delivered drop is the CLAMPED endpoint, not the raw
        // `to`: the tool layer clamps model coordinates to the LAST
        // screenshot, which can exceed the live virtual screen once a
        // monitor leaves mid-session (the exact case the clamp doc names).
        // The press will be delivered at the clamped start, so the
        // waypoints and the button-up must interpolate between and land on
        // the clamped endpoints too — a raw endpoint would deliver the drop
        // outside the screen while `pointer` claims the clamped edge,
        // leaving state and delivered messages disagreeing about where the
        // drop landed. A segment between two in-rect points stays in the
        // rect (convex), so every waypoint is a clamped position by
        // construction.
        let drop_point = clamp_point_to_rect(
            to,
            // SAFETY: metric query on the attached worker thread.
            &unsafe { desktop_virtual_screen_rect() },
        );
        if !mouse_point_encodable(drop_point.0, drop_point.1) {
            return Err(unencodable_point_error());
        }
        self.focus_dance(target);
        // A drag completes a press+release assembly — the tool layer clears
        // its held-left flag on drag success on exactly that assumption — so
        // a LEFT press this session still holds is finished first: its
        // recorded up is delivered here (SetCapture target and advanced
        // client point) instead of being stranded by the drag's own press.
        // Without this, a manual down the model never released stays stuck
        // pressed on its target until teardown: its matching up was the only
        // thing that could release it, and after this drag succeeds the tool
        // layer considers the left button free (its failed-drag best-effort
        // release would only hit-test a bystander). A held non-left press is
        // restored untouched — drag never touches those buttons, and its
        // record still serves the matching up. The record is consumed once
        // the up is delivered; the only realistic post failure is a
        // destroyed press target, which already took the press with it.
        match self.held_press.take() {
            Some((MouseButton::Left, press_target, press_x, press_y)) => {
                let _ = self.post(
                    press_target,
                    WM_LBUTTONUP,
                    0,
                    pack_mouse_lparam(press_x, press_y),
                );
            }
            other => self.held_press = other,
        }
        // A menu popup's client space IS screen space for its tracking loop
        // (the same contract the click and hover routes honor — r7 closed
        // the drag route's omission): all three legs (press, waypoints,
        // release) post SCREEN coordinates for a #32768 target and
        // client-translated ones otherwise. The popup answers HTCLIENT, so
        // the NC-start warning above does not fire — the coordinates are
        // what keeps the delivery honest here.
        //
        // SAFETY: class-name query on the hit-tested desktop window.
        let popup_screen_coords =
            menu_popup_needs_screen_coords(&unsafe { window_class_name(target) });
        let press_lparam = if popup_screen_coords {
            pack_mouse_lparam(start_x, start_y)
        } else {
            pack_mouse_lparam(client_x, client_y)
        };
        self.post(target, WM_LBUTTONDOWN, MK_LBUTTON, press_lparam)?;
        sleep(Duration::from_millis(DRAG_PRESS_SETTLE_MS));
        // Real drag input is CAPTURED by the window that received the press
        // (SetCapture semantics): moves and the button-up keep going to the
        // press target no matter where the pointer travels. Re-hit-testing
        // each waypoint (the previous shape) delivered moves to whatever
        // window lies under the point — sliders that track beyond their
        // bounds, list reorders, and canvas pans crossing into a sibling
        // window all broke, and the button-up could land on a different
        // window than the press.
        let move_result = (|| {
            for (x, y) in drag_waypoints((start_x, start_y), drop_point, DRAG_STEPS) {
                if self.cancelled() {
                    return Err(ComputerUseError::unavailable(
                        "drag was cancelled (caller timeout or stop); the button \
                         is released and the virtual pointer is left at the \
                         (clamped) drop point",
                    ));
                }
                // The virtual pointer follows the drag path (no OS cursor to
                // move — see the `pointer` field doc).
                self.pointer = (x, y);
                // SAFETY: coordinate translation on a desktop window from
                // the attached worker (skipped for a menu popup, whose
                // tracking loop reads screen coordinates directly — r7).
                let (move_x, move_y) = if popup_screen_coords {
                    (x, y)
                } else {
                    unsafe { Self::client_of(target, x, y) }
                };
                // A held-button move carries the pressed mark, exactly like
                // real input.
                self.post(
                    target,
                    WM_MOUSEMOVE,
                    MK_LBUTTON,
                    pack_mouse_lparam(move_x, move_y),
                )?;
                sleep(Duration::from_millis(DRAG_STEP_DELAY_MS));
            }
            Ok(())
        })();
        // The button must be released regardless of whether the path
        // movement errored — to the press target (capture semantics above).
        // The virtual pointer ends at the clamped drop point, which is also
        // where the up is delivered: state and messages must agree on where
        // the drop landed.
        self.pointer = drop_point;
        // SAFETY: coordinate translation on a desktop window from
        // the attached worker (skipped for a menu popup — screen coords,
        // same contract as the press and the waypoints above, r7).
        let (up_x, up_y) = if popup_screen_coords {
            drop_point
        } else {
            unsafe { Self::client_of(target, drop_point.0, drop_point.1) }
        };
        let release = self.post(target, WM_LBUTTONUP, 0, pack_mouse_lparam(up_x, up_y));
        combine_drag_errors(move_result, release)
    }

    fn scroll(&mut self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError> {
        let clicks = clicks.min(MAX_SCROLL_CLICKS);
        let (x, y) = self.cursor_point();
        if !mouse_point_encodable(x, y) {
            return Err(unencodable_point_error());
        }
        let Some((target, _, _)) = self.hit_test_at(x, y) else {
            return Err(ComputerUseError::unavailable(
                "there is no window under the cursor on the hidden desktop",
            ));
        };
        let (message, delta) = wheel_message_and_delta(direction);
        // WM_MOUSEWHEEL/WM_MOUSEHWHEEL lParam carries SCREEN coordinates
        // (unlike the button messages' client coordinates). The wParam low
        // word stays 0: real wheel input carries MK_* marks only while
        // buttons are held. Delivery nuance: WM_MOUSEWHEEL is bubbled to
        // ancestors by DefWindowProc, so a deep child target still reaches
        // a scrollable parent — WM_MOUSEHWHEEL is NOT bubbled, so a deep
        // child that does not handle it silently drops the event.
        let lparam = pack_mouse_lparam(x, y);
        for _ in 0..clicks {
            self.post(target, message, wheel_wparam(delta), lparam)?;
        }
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), ComputerUseError> {
        let text = normalize_typed_newlines(text);
        let target = self.keyboard_target()?;
        // A packaged (Store) target drops message-level input silently; the
        // warning rides the receipt instead of a positive "typed N chars"
        // reading as success.
        self.warn_if_packaged_keyboard_target(target);
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

    fn ui_tree(&mut self, opts: &UiTreeOptions) -> Result<String, ComputerUseError> {
        // Full UIA first: the helper runs the client ON the hidden desktop
        // and serializes with the physical stack's exact writer. When the
        // helper cannot be spawned or answered, fall through to the
        // window-class walk (below) — the degradation stays honest either
        // way, and the fallback payload names its own semantics.
        if let Some(helper) = self.ensure_uia_helper() {
            match helper.tree(opts) {
                Ok(UiaHelperOutcome::Ok(tree)) => return Ok(tree),
                Ok(UiaHelperOutcome::Soft(reason)) => {
                    // A healthy helper's honest "cannot answer": surface it
                    // as this query's error WITHOUT tearing the client down
                    // (a soft verdict is the helper answering normally —
                    // treating it as transport death would respawn a good
                    // process on every cooldown lapse).
                    return Err(ComputerUseError::unavailable(format!(
                        "the UIA helper could not serve the tree: {reason}"
                    )));
                }
                Err(error) => {
                    // Transport failure: drop the client (a Drop later
                    // respawns after the cooldown) and degrade.
                    log::warn!(
                        "[computer_use][virtual-win] the UIA helper failed the \
                         tree query (falling back to the window-class walk): \
                         {error:?}"
                    );
                    self.uia_helper = None;
                    self.helper_cooldown_until =
                        Some(Instant::now() + Duration::from_millis(HELPER_RESPAWN_COOLDOWN_MS));
                }
            }
        }
        let (max_depth, max_nodes) = tree_budget(opts);
        let mut out = String::new();
        // The degradation must be visible in the payload itself, not only in
        // the capability notes: the model that reads this tree must know the
        // roles are window CLASSES (not UIA control types), the names are
        // window CAPTIONS, and cross-process control content is not read.
        out.push_str(
            "window-class tree (Win32 HWND walk fallback — the UIA helper \
             process was unavailable; role is the window class, name is the \
             window caption; control content is not read across processes, \
             UIA-only properties are unavailable)\n",
        );
        let mut writer = HwndTreeWriter {
            next_index: 0,
            remaining: max_nodes,
            truncated: false,
            truncated_by_depth: false,
        };
        // Shell windows (Progman/WorkerW/Shell_TrayWnd) carry no drivable
        // content and would otherwise crowd the top level of every tree
        // (the same exclusion the keyboard-target fallback applies).
        let focused = unsafe { current_focus_hwnd() };
        // SAFETY: desktop-relative Z-order enumeration on the attached
        // worker thread; the cap mirrors the composite's honesty (the
        // bottom-most windows silently absent under a pathological farm).
        // Shell windows are excluded BEFORE the cap so they cannot consume
        // top-level slots (r5), and a cap-bound walk discloses the absence
        // in the payload instead of only the log (the model cannot see a
        // missing bottom of the desktop — the same honesty rule as the
        // node/depth footers).
        let (roots, top_cap_bound) = unsafe {
            // SAFETY (closure body): the class-name query on a handle from
            // the enumeration above runs inside the walk's unsafe block —
            // the syntactic nesting covers it, so no inner `unsafe` is
            // needed.
            walk_visible_toplevels(TREE_TOP_LEVEL_CAP, &|hwnd| {
                is_shell_window_class(&window_class_name(hwnd))
            })
        };
        // The walk returns painter order (bottom→top, the capture
        // composite's direction); the TREE iterates the reverse so the
        // topmost windows — the ones a screenshot is for — render first
        // (r7): a node-budget cut then removes the bottom-most roots,
        // consistent with the cap footer's own direction claim, instead of
        // silently dropping the active windows first.
        for root in roots.into_iter().rev() {
            // SAFETY: see write_class_tree_node.
            unsafe {
                write_class_tree_node(
                    root,
                    0,
                    max_depth,
                    focused.unwrap_or(HWND::default()),
                    false,
                    &mut writer,
                    &mut out,
                    self.cancel.as_deref(),
                )?;
            }
        }
        append_tree_truncation_footers(
            writer.truncated,
            writer.truncated_by_depth,
            top_cap_bound,
            writer.next_index,
            &mut out,
        );
        Ok(out)
    }

    fn element_at_point(
        &mut self,
        x: i32,
        y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError> {
        // Full UIA first (semantic element, real password flag); the
        // fallback answers with the owning HWND's class/title pair.
        if let Some(helper) = self.ensure_uia_helper() {
            match helper.at_point(x, y) {
                Ok(UiaHelperOutcome::Ok(answered)) => return Ok(answered),
                Ok(UiaHelperOutcome::Soft(reason)) => {
                    // A healthy helper's honest "cannot answer" — the client
                    // stays up (see the tree arm).
                    return Err(ComputerUseError::unavailable(format!(
                        "the UIA helper could not serve element_at_point: {reason}"
                    )));
                }
                Err(error) => {
                    log::warn!(
                        "[computer_use][virtual-win] the UIA helper failed the \
                         point query (falling back to the window-class answer): \
                         {error:?}"
                    );
                    self.uia_helper = None;
                    self.helper_cooldown_until =
                        Some(Instant::now() + Duration::from_millis(HELPER_RESPAWN_COOLDOWN_MS));
                }
            }
        }
        // The same hit test the click route uses: the deepest window at the
        // point (client space) or the top-level window that owns a
        // non-client point. The window-class answer carries the owning
        // HWND's class and caption — the disclosed degradation.
        let Some((target, _, _)) = self.hit_test_at(x, y) else {
            return Ok(None);
        };
        // SAFETY: window queries on the hit-tested desktop window.
        unsafe { window_element_info(target, "element_at_point") }.map(Some)
    }

    fn focused_element(&mut self) -> Result<Option<ElementInfo>, ComputerUseError> {
        // Full UIA first: the helper reads the hidden desktop's foreground +
        // thread focus (NEVER UIA's input-desktop focused element) and maps
        // the HWND, so the confirm preview sees the real semantic name and
        // password flag. "Never masquerade as none" governs every leg: an
        // unidentified focus is an ERROR (tool layer masks as uncertain),
        // never a silent Ok(None) that would let full typed text ride a
        // confirmation while a password field actually holds focus.
        if let Some(helper) = self.ensure_uia_helper() {
            match helper.focus() {
                Ok(UiaHelperOutcome::Ok(answered)) => return Ok(answered),
                Ok(UiaHelperOutcome::Soft(reason)) => {
                    // The healthy helper's normal "nothing holds focus" (or
                    // equivalent) verdict: the honest uncertain error the
                    // tool layer masks on — WITHOUT tearing the client down.
                    // A fresh hidden desktop answers focus this way by
                    // default, so treating it as transport death would
                    // respawn a good process on every keystroke path.
                    return Err(ComputerUseError::unavailable(reason));
                }
                Err(error) => {
                    log::warn!(
                        "[computer_use][virtual-win] the UIA helper failed the \
                         focus query (falling back to the window-class answer): \
                         {error:?}"
                    );
                    self.uia_helper = None;
                    self.helper_cooldown_until =
                        Some(Instant::now() + Duration::from_millis(HELPER_RESPAWN_COOLDOWN_MS));
                }
            }
        }
        let Some(focused) = (unsafe { current_focus_hwnd() }) else {
            return Err(ComputerUseError::unavailable(
                "the focused control cannot be identified on the hidden \
                 desktop (no app window holds keyboard focus); treat the \
                 input target as sensitive",
            ));
        };
        // SAFETY: window queries on the focus-identified desktop window.
        unsafe { window_element_info(focused, "focused_element") }.map(Some)
    }

    fn set_cancel_flag(&mut self, flag: Option<Arc<AtomicBool>>) {
        self.cancel = flag;
    }

    fn release_os_grant(&mut self) -> Result<(), ComputerUseError> {
        // Triggered by the command layer when the user revokes the grant /
        // hits Stop / flips the master switch off (the same wiring that
        // drives the Linux stack's wind-down). What Stop must undo is the
        // apps the agent OPENED: each recorded spawn process is terminated
        // BY HANDLE (no pid-recycling ambiguity) with no grace period —
        // unsaved work inside those apps is lost, the same group-kill
        // honesty the Linux stack discloses. Only the recorded ROOT
        // processes are terminated: `TerminateProcess` does not reach the
        // processes an opened app spawned on its own, so those keep running
        // until the session is torn down (the kill-on-close job reclaims
        // them at teardown — except descendants that themselves escape the
        // job through a system broker, which like the broker-delegated
        // opens above live outside it and outlive the session) — this
        // stack's counterpart of the Linux notes'
        // process-group escape exception, disclosed in the notes. The
        // hidden desktop's own infrastructure — the desktop object, the
        // session job, and the shell — stays alive, so the next action
        // (including a re-granted session and fresh `open` calls) still
        // works without any respawn; the shell itself is only reclaimed by
        // Drop at session end. The kill-on-close job remains the backstop:
        // a process that somehow survived its terminate still dies with the
        // session. The open budget resets WITH the wind-down list (the
        // Linux stack's take-the-children shape): a re-granted session
        // starts with the full launch budget again, in every state.
        let processes = std::mem::take(&mut self.opened_processes);
        let total = processes.len();
        let mut wound_down = 0usize;
        for process in processes {
            // SAFETY: termination + close of our own recorded spawn handle
            // (PROCESS_TERMINATE right is part of CreateProcessW's return).
            // An already-exited target answers churn either way the OS
            // phrases it (the terminate may succeed as a no-op on a dead
            // process or fail) — never an error this channel reports; the
            // log's "N of M wound down" is therefore best-effort, which the
            // wording already hedges.
            let terminated = unsafe { TerminateProcess(process, 1) };
            if terminated.is_ok() {
                wound_down += 1;
            }
            let _ = unsafe { CloseHandle(process) };
        }
        // The launches the list recorded were the budget's substance: with
        // every recorded app terminated, the count restarts (a lifetime
        // cap that never reset would make "fresh open calls" a lie after
        // 32 lifetime launches, unlike the Linux release).
        self.opened_app_count = 0;
        if total > 0 {
            log::info!(
                "[computer_use][virtual-win] released the virtual grant — \
                 {wound_down} of {total} opened app process(es) wound down; \
                 the hidden desktop itself stays usable"
            );
        }
        Ok(())
    }

    fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    fn open_app(&mut self, program: &str, args: &[String]) -> Result<u32, ComputerUseError> {
        validate_open_request(program, args)?;
        if open_capacity_rejected(self.opened_app_count) {
            // Count-only text (the R7 rule: the model-supplied program name
            // never rides errors). The count is the session's whole LAUNCH
            // budget on this stack (opened apps deliberately outlive the
            // session, so entries never decrement — the R15 copy says so
            // instead of the old "recorded as running" over a lifetime
            // count, whose "reuse an opened app" advice could not lift the
            // cap).
            return Err(ComputerUseError::unavailable(format!(
                "open: the session has already launched {MAX_OPEN_CHILDREN} apps; \
                 past launches keep counting toward this limit even after an \
                 app exits; stopping the session (Stop, a grant revoke, or \
                 the master switch) winds the opened apps down and restores \
                 the budget"
            )));
        }
        // Search-order residual (R12): `lpApplicationName` is None here, so
        // a BARE program name resolves with the calling exe's directory
        // FIRST (CreateProcessW's standard search order) — a same-named
        // binary sitting in the app's install dir shadows the system one.
        // Accepted for the model-supplied `open` surface (user-account
        // scope, grant-screened); the shell launch resolves its binary
        // explicitly and pins `lpApplicationName` to dodge command-line
        // parsing ambiguity instead.
        let command_line = build_command_line(program, args);
        let (pid, process) = self.launch_in_job(None, &command_line)?;
        // The spawn's own handle joins the wind-down list (release_os_grant
        // terminates exactly these; see the field doc).
        self.opened_processes.push(process);
        self.opened_app_count += 1;
        // The program text is model-supplied; it renders ONLY through
        // `logged_program` (Debug escaping keeps a control character from
        // forging subsequent log lines — the redaction contract, pinned by
        // `recorded_program_names_never_render_plain_in_logs`).
        log::info!(
            "[computer_use][virtual-win] opened {} on the hidden \
             desktop (pid {pid})",
            logged_program(program)
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
            // Hoisted out of the loop: the post-loop budget warn must read
            // the query outcome, and a binding declared inside a loop body
            // does not survive it (the R16 in-loop `queried` was E0425 on
            // the Windows target — invisible to the Linux gates).
            let mut last_query_ok = true;
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
                    last_query_ok = false;
                    // R16: a query failure must not fall through to the
                    // budget-exhausted warn below (which would claim a wait
                    // that never elapsed) — say what actually happened.
                    log::warn!(
                        "[computer_use][virtual-win] cannot read the session \
                         job's process count ({error}); continuing teardown \
                         without an emptied-job confirmation (the job's \
                         kill-on-close flag still reclaims them when the \
                         handle closes)"
                    );
                    break;
                }
                if accounting.ActiveProcesses == 0 {
                    job_emptied = true;
                    break;
                }
                sleep(Duration::from_millis(TEARDOWN_JOB_POLL_MS));
            }
            if !job_emptied && last_query_ok {
                log::warn!(
                    "[computer_use][virtual-win] session job processes did not \
                     all exit within {TEARDOWN_JOB_WAIT_MS}ms; continuing \
                     teardown (the job's kill-on-close flag still reclaims \
                     them when the handle closes)"
                );
            }
        }
        // 2b. Close the wind-down list's retained spawn handles. A release
        //     (Stop/revoke) already terminated and closed them; anything
        //     still here was opened but never wound down, and the job
        //     termination above already ended those processes — the close
        //     is bookkeeping, never a second terminate.
        for process in self.opened_processes.drain(..) {
            // SAFETY: close of our own retained spawn handle.
            let _ = unsafe { CloseHandle(process) };
        }
        // 3. Capture thread: disconnect the channel, give it a bounded join
        //    (a thread wedged inside a PrintWindow/RedrawWindow to a dying
        //    window detaches and exits on its own rather than hanging the
        //    worker's teardown — same policy as BackendHandle's detach).
        let mut capture_detached = false;
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
                capture_detached = true;
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
        // 5. Close the desktop. ERROR_BUSY is tolerated by design: the
        //    desktop object is reclaimed when the process exits (the
        //    kill-on-close job frees the last references). Skipped when the
        //    capture thread detached (step 3): that thread's copy of the
        //    handle value may still be in use inside its attach/composite
        //    sequence, and closing the handle now would let the recycled
        //    handle-table slot alias another desktop object — the leaked
        //    desktop is reclaimed at process exit together with the job's
        //    last references.
        if !self.desktop.is_invalid() && !capture_detached {
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
    // The tree budget constants moved with their clamp into the shared
    // `helpers::tree_budget`; the production half no longer names them, so
    // the budget assertions import them directly. The `crate::` absolute
    // path is depth-proof — the relative depth this import needs is three
    // `super`s (tests → windows_virtual → virtual_display → platform), the
    // exact class of error the stub harness could not see (r5 B1).
    use crate::features::computer_use::platform::helpers::{
        DEFAULT_TREE_MAX_DEPTH, DEFAULT_TREE_MAX_NODES, MAX_TREE_DEPTH, MAX_TREE_NODES,
    };
    // The no-op-zone constants appear only in this module's tests (the
    // production routing deliberately folds them into the catch-all arm —
    // naming them there would imply individual handling that does not
    // exist).
    use windows::Win32::UI::WindowsAndMessaging::{HTBORDER, HTGROWBOX, HTHELP, HTOBJECT};

    /// The probe answers Available and the one-time dispatch therefore
    /// selects the virtual target when preferred (the stack's
    /// only external dependency is the shell itself — the probe answers
    /// Unavailable when `%WINDIR%\explorer.exe` is missing, which never
    /// happens on a Windows runner, so the assertion holds there).
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

    /// The desktop access mask is the narrow module set — deliberately
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
        assert_eq!(char_to_vk('=').ok(), Some(VK_OEM_PLUS.0));
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
    /// modifiers lead, plain keys keep their relative order ("modifiers
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

    /// Key-message lParam shapes: the pure composer places repeat=1, the
    /// scan code at bits 16-23, the extended-key flag at bit 24, the Alt
    /// context bit at 29, and the previous-state (bit 30) + transition
    /// (bit 31) flags on the UP variant — the shape real input carries
    /// (DefWindowProc's Alt+F4 / menu handling reads WM_SYSKEYDOWN, and
    /// scan-validating apps key off bits 16-23). The VK→scan translation
    /// itself is pinned for one well-known key. WINDOWS-CI-ONLY (this whole
    /// file is cfg(windows)).
    #[test]
    fn key_lparam_encodes_scan_extended_context_and_up_bits() {
        assert_eq!(compose_key_lparam(0, false, false, false), 1);
        let up = compose_key_lparam(0, false, false, true) as u32;
        assert_eq!(up & 1, 1, "repeat count stays 1");
        assert_eq!(up >> 30 & 1, 1, "previous key state must be set on up");
        assert_eq!(up >> 31 & 1, 1, "transition flag must be set on up");
        let with_scan = compose_key_lparam(0x3E, false, false, false) as u32;
        assert_eq!(with_scan >> 16 & 0xFF, 0x3E, "scan code rides bits 16-23");
        let context = compose_key_lparam(0x3E, false, true, false) as u32;
        assert_eq!(
            context >> 29 & 1,
            1,
            "Alt-held input must carry the context bit"
        );
        assert_eq!(context >> 16 & 0xFF, 0x3E);
        // The extended-key flag (bit 24) rides when the key is one of the
        // navigation cluster, and only then — real input for those keys
        // always carries it (the same VK exists on the numpad, so apps
        // distinguish by this bit).
        let extended = compose_key_lparam(0x48, true, false, false) as u32;
        assert_eq!(extended >> 24 & 1, 1, "bit 24 must carry the extended flag");
        let plain = compose_key_lparam(0x3E, false, false, false) as u32;
        assert_eq!(plain >> 24 & 1, 0, "non-extended keys must not set bit 24");
        // A scan value wider than the 8-bit field masks down (bit layout
        // stays intact for whatever lands above).
        let masked = compose_key_lparam(0x1_3E, false, false, false) as u32;
        assert_eq!(masked >> 16 & 0xFF, 0x3E);
        assert_eq!(masked >> 25, 0, "nothing leaks beyond the extended flag");
        // The translation the chord path performs: VK_F4 (0x73) → scan
        // 0x3E on the standard keyboard (SAFETY: pure translation query).
        assert_eq!(unsafe { vk_scan(0x73) }, 0x3E, "VK_F4 scan code");
    }

    /// The extended-key set is exactly the navigation cluster (plus the
    /// right-hand modifiers the chord vocabulary never produces): arrows,
    /// Home/End/Prior/Next, Insert/Delete ride bit 24; letters, digits,
    /// Enter, and the shared left-side modifiers do not.
    #[test]
    fn key_is_extended_pins_the_navigation_cluster() {
        for vk in [
            VK_LEFT.0,
            VK_RIGHT.0,
            VK_UP.0,
            VK_DOWN.0,
            VK_HOME.0,
            VK_END.0,
            VK_PRIOR.0,
            VK_NEXT.0,
            VK_INSERT.0,
            VK_DELETE.0,
        ] {
            assert!(
                key_is_extended(vk),
                "VK {vk:#x} must carry the extended flag"
            );
        }
        for vk in [VK_RETURN.0, VK_CONTROL.0, VK_MENU.0, VK_SPACE.0, VK_TAB.0] {
            assert!(
                !key_is_extended(vk),
                "VK {vk:#x} must not carry the extended flag"
            );
        }
    }

    /// The chord post target: the Alt-held system pair goes to the
    /// top-level frame (frame-routed SC_CLOSE/SC_KEYMENU handling),
    /// the plain pair goes to the focused control — ordinary
    /// keyboard input is delivered to the focus window, and posting it to
    /// the frame makes frame-routed apps silently drop it while PostMessageW
    /// reports success. WINDOWS-CI-ONLY (this whole file is cfg(windows)).
    #[test]
    fn chord_post_target_pins_focus_for_plain_and_top_for_alt() {
        let focus = HWND(0x1 as _);
        let top = HWND(0x2 as _);
        assert_eq!(chord_post_target(focus, top, false), focus);
        assert_eq!(chord_post_target(focus, top, true), top);
    }

    /// The Alt-chord message-pair selection (companion of the lparam pin
    /// above): EVERY key of an Alt chord — companions included — must ride
    /// the WM_SYSKEYDOWN/WM_SYSKEYUP pair, and a chord without Alt must keep
    /// the plain pair (the system pair is Alt-specific). Reverting a
    /// companion key to plain WM_KEYDOWN (the pre-fix shape that made
    /// Alt+F4/Alt-menu mnemonics never fire) must fail here, not pass every
    /// CI leg silently. WINDOWS-CI-ONLY (this whole file is cfg(windows)).
    #[test]
    fn key_message_pair_pins_the_sys_shape_while_alt_is_held() {
        assert_eq!(
            key_message_pair(true),
            (WM_SYSKEYDOWN, WM_SYSKEYUP),
            "Alt-held chords must use the system key pair for every key"
        );
        assert_eq!(
            key_message_pair(false),
            (WM_KEYDOWN, WM_KEYUP),
            "chords without Alt must keep the plain key pair"
        );
    }

    /// The whole-chord plan derives the pair ONCE from the full key list:
    /// an Alt chord yields the SYS pair for every member (not just VK_MENU
    /// itself) and a chord without Alt the plain pair. Pinning the
    /// chord-level shape is what the pair-fn pin above cannot see — the R9
    /// review noted the pre-fix per-key reversion (compute per key inside
    /// the loop) leaves the pair fn green on every leg.
    /// WINDOWS-CI-ONLY (this whole file is cfg(windows)).
    #[test]
    fn chord_message_plan_derives_the_pair_once_per_chord() {
        let alt_ctrl = [VK_MENU.0, VK_CONTROL.0];
        assert_eq!(
            chord_message_plan(&alt_ctrl),
            (WM_SYSKEYDOWN, WM_SYSKEYUP, true),
            "every key of an Alt chord rides the system pair"
        );
        assert_eq!(
            chord_message_plan(&[VK_CONTROL.0]),
            (WM_KEYDOWN, WM_KEYUP, false),
            "a chord without Alt keeps the plain pair"
        );
        assert_eq!(
            chord_message_plan(&[VK_CONTROL.0, b'C' as u16]),
            (WM_KEYDOWN, WM_KEYUP, false),
            "a non-Alt modifier chord keeps the plain pair"
        );
        // R16: the helper pin above cannot see a reversion that BYPASSES the
        // helper (computing key_message_pair per key inside chord_to's loop
        // restores the exact R9 regression while this test stays green).
        // The CALL SITE must exist in the production source, exactly once.
        let src = production_source();
        assert_eq!(
            src.matches("chord_message_plan(&ordered)").count(),
            1,
            "chord_to must derive its (down, up, alt_held) plan ONCE per chord \
             through the shared helper — a per-key reversion inside the loop \
             is the R9 regression shape (frame-routed SYS handling never \
             fires while PostMessageW reports success)"
        );
    }

    /// R18: a shifted character's key position posted plain delivers its
    /// UNSHIFTED face — `key "plus"` used to type `=` while reporting
    /// success (the physical stack's `map_char_with_plan` classifies the
    /// same input NeedsModifier and its own test calls the shape the defect
    /// scenario). Pins the shifted-face table, the chord plan (synthesize
    /// solo / already-held stays / refuse beside another key), and the call
    /// site that wires the plan into `chord_to` (the helper pin cannot see
    /// a bypass — the R16 `chord_message_plan` precedent).
    #[test]
    fn char_faces_pin_the_shift_table_and_the_chord_plan() {
        for c in [
            'A', 'Z', '+', ':', '"', '<', '>', '?', '~', '{', '|', '}', '_',
        ] {
            assert!(char_requires_shift(c), "'{c}' is a shifted face");
        }
        for c in [
            'a', 'z', '0', '9', ' ', '=', ',', '-', '.', '/', ';', '`', '[', '\\', ']', '\'',
        ] {
            assert!(!char_requires_shift(c), "'{c}' is an unshifted face");
        }
        let solo_plus: Vec<&Key> = vec![&Key::Char('+')];
        let solo_s: Vec<&Key> = vec![&Key::Char('s')];
        let ctrl: Vec<&Key> = vec![&Key::Control];
        assert!(
            matches!(
                chord_shift_plan(&[], &solo_plus),
                Ok(ShiftSynthesis::Insert)
            ),
            "a solo shifted character synthesizes the Shift wrap"
        );
        assert!(
            matches!(
                chord_shift_plan(&ctrl, &solo_plus),
                Ok(ShiftSynthesis::Insert)
            ),
            "ctrl+plus must still deliver '+' (synthesize the wrap around the \
             held modifier)"
        );
        assert!(
            matches!(
                chord_shift_plan(&[&Key::Shift], &solo_plus),
                Ok(ShiftSynthesis::None)
            ),
            "shift+plus holds the state already — no synthesis (the shape that \
             worked before the fix must keep working)"
        );
        assert!(
            matches!(chord_shift_plan(&ctrl, &solo_s), Ok(ShiftSynthesis::None)),
            "unshifted faces never synthesize"
        );
        let multi = vec![&Key::Char('+'), &Key::Function(4)];
        assert!(
            chord_shift_plan(&[], &multi).is_err(),
            "a shifted character beside another key is refused — the wrap \
             cannot be scoped to just that key (the physical contract)"
        );
        let src = production_source();
        assert_eq!(
            src.matches("chord_shift_plan(&modifiers, &rest)").count(),
            1,
            "chord_to must consult the shift plan — deleting the call or \
             mapping shifted faces straight to VKs (the R18 defect shape) \
             must fail this pin"
        );
    }

    /// The message-level delivery shapes stay wired: the modifier partition
    /// is consulted through the shared helper (a bypass restores the R9
    /// per-key shape), the type path's character runs stay WM_CHAR-only
    /// (re-adding a KEYDOWN/UP wrap doubles newlines/tabs through
    /// TranslateMessage), the double-click message only ever rides a class
    /// that registered CS_DBLCLKS (hardcoding `true` delivers DBLCLK to
    /// classes that never asked for it — the R3 regression), and the split
    /// release stays pinned to the remembered press target (the R18
    /// re-hit-test shape delivered orphan ups to bystander windows).
    #[test]
    fn delivery_shapes_stay_wired() {
        let src = production_source();
        assert_eq!(
            src.matches("partition_modifiers_first(").count(),
            2,
            "definition + the chord_to call site: an inline partition in \
             chord_to (the bypass shape) drops the count and fails here"
        );
        assert_eq!(
            src.matches("WM_KEYDOWN").count(),
            9,
            "the type path's character runs must ride WM_CHAR only — \
             re-wrapping them in KEYDOWN/UP pairs (TranslateMessage then \
             doubles the WM_CHAR) changes this count"
        );
        assert_eq!(
            src.matches("window_class_accepts_dblclk(").count(),
            2,
            "definition + the live class query at the click call site: \
             hardcoding accepts_dblclk = true delivers DBLCLK to classes \
             that never registered CS_DBLCLKS"
        );
        assert_eq!(
            src.matches("self.held_press").count(),
            7,
            "mouse_down stores the press target, mouse_up consumes (and \
             restores on a button mismatch) it, move_pointer_to routes \
             held-button moves to it (capture semantics) and advances its \
             recorded point on a delivered move, and drag finishes a held \
             LEFT press with its recorded up before its own assembly while \
             restoring a held non-left record untouched — a re-hit-test \
             mouse_up, a re-hit-test held move, an up at the stale press \
             point, a drag that strands the manual press, or a drag that \
             silently drops a non-left record loses the record or \
             misplaces the release and fails this pin"
        );
    }

    /// The model-facing disclosure strings must keep their residuals: the
    /// broker escape (R8 major), the shared-clipboard channel (R9 major),
    /// and the no-DWM composite / message-injection limits. Extracted into
    /// pure helpers exactly so this pin rides the REAL strings; deleting a
    /// sentence fails this test on the Windows leg instead of passing
    /// silently. WINDOWS-CI-ONLY (this whole file is cfg(windows)).
    #[test]
    fn capability_notes_disclose_the_target_contract() {
        let note = capability_target_note();
        assert!(
            note.contains("system broker"),
            "the broker-launched residual must be disclosed to the model"
        );
        assert!(
            note.contains("surface a window on the user's desktop"),
            "the broker window can land on the user's screen"
        );
        assert!(
            note.contains("is not contained") && note.contains("not terminated"),
            "the broker app's containment and lifecycle truth must be stated"
        );
        assert!(
            note.contains("shares the user's clipboard"),
            "the hidden desktop shares the window station's clipboard with \
             the user's own desktop — the R9 review found the channel \
             undisclosed in both directions"
        );
        assert!(
            note.contains("winds down the apps opened on the hidden desktop")
                && note.contains("unsaved work inside the terminated apps is lost"),
            "Stop winds down the opened apps (no grace) — the group-kill \
             honesty the Linux stack also discloses"
        );
        assert!(
            note.contains("child processes an app spawned keep running until session teardown"),
            "the wind-down's descendant residual must be disclosed (this \
             stack's counterpart of the Linux notes' group-kill escape \
             exception — r4 MAJOR-2)"
        );
        assert!(
            note.contains("the desktop and its shell remain"),
            "Stop leaves the desktop and shell alive for a re-granted \
             session until backend drop"
        );
        let notes = capability_notes();
        assert!(
            notes.contains("PostMessageW"),
            "the message-injection fidelity limit must be disclosed"
        );
        assert!(
            notes.contains("Alt+Tab"),
            "shell-managed chords can never fire (shell-side handling, not \
             app-dependent) — the R10 review found the generic \
             ignore-synthetic-messages wording implied app-dependence"
        );
        assert!(
            notes.contains("default button or focus cycle does not fire"),
            "typed dialog keys ride WM_CHAR, which dialog managers do not act \
             on — success-without-effect must be disclosed"
        );
        assert!(
            notes.contains("clipboard"),
            "the clipboard channel must be disclosed in the notes too"
        );
        assert!(
            notes.contains("per-window composite"),
            "the no-DWM capture shape must be disclosed"
        );
        assert!(
            notes.contains("absent from that frame"),
            "a capture-refusing window is absent from that frame (warn-logged, \
             not silently skipped) — must be disclosed"
        );
        assert!(
            notes.contains("blank rectangle"),
            "cloaked UWP windows can print as blank rectangles on a hidden \
             desktop — must be disclosed"
        );
        // R13: a mid-session shell death is neither detected nor restarted —
        // the fidelity consequence must be disclosed up front.
        assert!(
            notes.contains("exits mid-session"),
            "the shell-death consequence (wallpaper/taskbar vanish, apps keep \
             working, no restart) must be disclosed"
        );
        // R14: the desktop name is unique but unprivileged — the isolation
        // contract must not read as an access boundary.
        assert!(
            notes.contains("not an access boundary") && note.contains("not an access boundary"),
            "the naming-not-a-boundary residual must be disclosed in both note \
             surfaces (a same-user squatter can place windows on the session)"
        );
        // R12: the wheel-delivery and hold auto-repeat fidelity limits.
        assert!(
            notes.contains("window under the pointer rather than the focus"),
            "scroll targets the cursor window while real wheel input goes to \
             the focus window — must be disclosed"
        );
        assert!(
            notes.contains("no modifier state"),
            "posted wheel messages cannot carry Ctrl/Shift (Ctrl+wheel zoom \
             unrepresentable) — must be disclosed"
        );
        assert!(
            notes.contains("rather than auto-repeating"),
            "hold_key posts one down/up pair with no auto-repeat — must be \
             disclosed"
        );
        // R15: the naming residual covers both directions — a same-user
        // squatter can place windows on the session surface AND a program
        // inside the session can open it by name and pull the surface onto
        // the user's screen.
        assert!(
            notes.contains("pulling the session surface onto the user's screen")
                && note.contains("pull the session surface onto the user's screen"),
            "the pull-out direction of the naming residual must be disclosed \
             in both note surfaces (each in its shipped phrasing)"
        );
        // The virtual pointer replaces the unreachable OS cursor: the model
        // must know there is no real cursor and where the pointer starts.
        assert!(
            notes.contains("virtual pointer") && notes.contains("screen center"),
            "the no-OS-cursor/virtual-pointer contract must be disclosed"
        );
        // The packaged typing trap must be disclosed up front (the Win11
        // Notepad shape): the warning alone fires only after a failed type.
        assert!(
            notes.contains("packaged apps") && notes.contains("Windows 11 Notepad"),
            "packaged (UWP/WinUI) apps ignoring message-level input must be \
             disclosed as the common case"
        );
        // The packaged-activation open failure (Paint on Win11) and the
        // console-host delegation (Windows Terminal) must be disclosed: both
        // read as mystery failures without the OS context.
        assert!(
            notes.contains("Windows Terminal"),
            "the console-host delegation must be disclosed (cmd.exe windows \
             are delegated to the default Windows Terminal host)"
        );
        assert!(
            notes.contains("fails the open with an explicit error"),
            "the packaged-activation open failure must be disclosed"
        );
        // The single-slot manual-press record must be disclosed: a second
        // mouse_down orphans the first target's pressed state, whose
        // release is then up to a matching up that still reaches it or the
        // session end — and the drag finishing a held left press at its
        // held point must be disclosed alongside.
        assert!(
            notes.contains("tracks a single button"),
            "the single-slot manual-press record must be disclosed"
        );
        assert!(
            notes.contains("only a matching up that still reaches it")
                && notes.contains("finishes a still-held left press first"),
            "the orphaned-target recovery truth and the drag's held-left \
             finish must be disclosed, not a rosier promise"
        );

        // The non-client delivery must be disclosed in BOTH directions: the
        // per-zone model actually delivered (chrome SC commands with the
        // disabled-Close refusal, menu items via screen coordinates, the
        // modal-loop refusal) and what still does not work (caption-drag
        // move, border-drag resize, manual down/up over NC zones) — a
        // model that reads only "we support title bars" would burn actions
        // on drags this stack cannot deliver (r5: the fragments follow the
        // reworked delivery model).
        assert!(
            notes.contains("per-zone delivery")
                && notes.contains("disabled or absent Close is refused")
                && notes.contains("clicked with screen coordinates")
                && notes.contains("scrollbars are refused"),
            "the per-zone non-client delivery model must be disclosed"
        );
        assert!(
            notes.contains("cannot be moved") && notes.contains("left_mouse_down/left_mouse_up"),
            "the caption-drag/border-drag/manual-primitive residuals must be \
             disclosed"
        );
        // The wind-down contract must be disclosed with its full honesty:
        // stop kills the opened apps WITHOUT a grace period (unsaved work
        // is lost), spawned child processes are NOT wound down (the
        // counterpart of the Linux notes' group-kill escape exception —
        // r4 MAJOR-2), broker-escaped descendants are the one survivor the
        // job cannot reach (r5 hedge), the desktop survives for a
        // re-granted session, and the full teardown (shell included) waits
        // for session end — the same shape the Linux stack discloses.
        assert!(
            notes.contains("winds down the apps opened on the hidden desktop")
                && notes.contains("without a grace period")
                && notes.contains("unsaved work inside them is lost"),
            "the stop-time wind-down (group kill, no grace) must be disclosed"
        );
        assert!(
            notes.contains("child processes an opened app spawned are not wound down")
                && notes.contains("outlive the session"),
            "the wind-down's descendant residual AND its broker-escape \
             hedge must be disclosed (the Linux notes disclose their \
             process-group escape; this stack must disclose its \
             TerminateProcess scope the same way — r4 MAJOR-2)"
        );
        assert!(
            notes.contains("shell stay alive for a re-granted session")
                && notes.contains("reclaimed when the session is torn down"),
            "the survive-a-stop / reclaimed-at-teardown split must be disclosed"
        );
        // The structural-query contract must be disclosed both ways: the
        // real UIA tree comes from the attached helper process (with the
        // first-query spawn cost), a transport death degrades to the walk
        // while a SOFT verdict surfaces the helper's reason WITHOUT
        // degrading (r5: the old "cannot start or answer" phrasing
        // over-promised the fallback for soft verdicts), and the fallback
        // walk's degraded semantics must be visible to a model that lands
        // on it.
        assert!(
            notes.contains("attached helper process")
                && notes.contains("fall back to a window-class walk")
                && notes.contains("cross-process control")
                && notes.contains("secure-field signal"),
            "the UIA-helper contract and its window-class fallback must be \
             disclosed"
        );
        assert!(
            notes.contains("cannot start or has died (a transport failure)")
                && notes.contains("surfaces its reason as that query's error")
                && notes.contains("pays the helper's spawn"),
            "the transport-only fallback, the soft-verdict honest error, \
             and the first-query spawn cost must be disclosed (r5)"
        );
        // r6: the fallback screening input's bound must be stated AS a
        // bound (the old "runs on the full raw caption" phrasing
        // over-claimed a 4096-unit read as unbounded — the same
        // padding-evasion class the bound exists to bound).
        assert!(
            notes.contains("bounded at 4096 UTF-16 units"),
            "the fallback screening caption bound must be disclosed, not \
             over-claimed as the full caption"
        );
        // r6 (the r5 prescription that had been dropped): an open menu
        // tracking loop consumes clicks aimed elsewhere and dismisses on
        // Escape — a model clicking past an open menu sees its click eaten
        // by the menu, which reads as a mystery failure without this.
        assert!(
            notes.contains("consumes clicks aimed elsewhere")
                && notes.contains("dismisses on Escape"),
            "the menu tracking loop's click consumption and Escape \
             dismissal must be disclosed"
        );
        // r7: the swallowed-click receipt covers the whole unroutable
        // family (not just HTTRANSPARENT), and the shell exclusion's two
        // cross-surface directions must be stated — keyboard-target
        // selection consumes the same predicate, and a same-desktop app
        // can name itself into the exclusion (the hiding direction of the
        // same-class caveat the XAML-host sentence already discloses).
        assert!(
            notes.contains("may be swallowed"),
            "the unroutable-zone degrade must carry the swallowed-click \
             warning in the disclosure too"
        );
        assert!(
            notes.contains("keyboard-target fallback")
                && notes.contains(
                    "vanish from trees and keyboard-target \
                   selection"
                ),
            "the shell exclusion's keyboard-target consumer AND its \
             spoofing direction must be disclosed, not just the tree-noise \
             direction"
        );
    }

    /// The virtual pointer's geometry helpers: center placement (the
    /// conventional start) and SetCursorPos-style edge clamping, both across
    /// negative origins (the normal multi-monitor case) and degenerate rects.
    #[test]
    fn virtual_pointer_geometry_follows_the_rect() {
        // Center of a plain rect.
        assert_eq!(
            pointer_center_of_rect(&VirtualScreenRect {
                origin_x: 0,
                origin_y: 0,
                width: 1920,
                height: 1080
            }),
            (960, 540)
        );
        // Negative-origin union (secondary left of/above primary): center of
        // -1920..1920 × -1080..1080 is the shared corner.
        assert_eq!(
            pointer_center_of_rect(&VirtualScreenRect {
                origin_x: -1920,
                origin_y: -1080,
                width: 3840,
                height: 2160
            }),
            (0, 0)
        );
        // Odd sizes truncate toward the origin.
        assert_eq!(
            pointer_center_of_rect(&VirtualScreenRect {
                origin_x: 0,
                origin_y: 0,
                width: 3,
                height: 3
            }),
            (1, 1)
        );
        // A degenerate rect (no monitor space) degrades to its origin.
        assert_eq!(
            pointer_center_of_rect(&VirtualScreenRect {
                origin_x: 5,
                origin_y: 7,
                width: 0,
                height: 0
            }),
            (5, 7)
        );

        let rect = VirtualScreenRect {
            origin_x: -1920,
            origin_y: 0,
            width: 3840,
            height: 1080,
        };
        // Inside stays put.
        assert_eq!(clamp_point_to_rect((0, 500), &rect), (0, 500));
        // All four edges clamp (SetCursorPos's own virtual-screen clamping).
        assert_eq!(clamp_point_to_rect((-2000, 500), &rect), (-1920, 500));
        assert_eq!(clamp_point_to_rect((2000, 500), &rect), (1919, 500));
        assert_eq!(clamp_point_to_rect((100, -5), &rect), (100, 0));
        assert_eq!(clamp_point_to_rect((100, 1080), &rect), (100, 1079));
        // A degenerate rect collapses every point onto its origin (the
        // inverted-range guard, not a clamp panic).
        let degenerate = VirtualScreenRect {
            origin_x: 10,
            origin_y: 20,
            width: 0,
            height: 0,
        };
        assert_eq!(clamp_point_to_rect((0, 0), &degenerate), (10, 20));
        // A NEGATIVE width (a truly inverted range, not just empty) also
        // collapses via the same guard: deleting the `.max(origin)` guard
        // would make `clamp` panic on min > max instead of failing here.
        let inverted = VirtualScreenRect {
            origin_x: 10,
            origin_y: 20,
            width: -5,
            height: -5,
        };
        assert_eq!(clamp_point_to_rect((0, 0), &inverted), (10, 20));

        // Overflow-adjacent metrics cannot panic or wrap: the additions
        // saturate at the i32 edges (unreachable from GetSystemMetrics'
        // 16-bit GDI space, but the helpers stay total for any caller).
        let huge = VirtualScreenRect {
            origin_x: i32::MAX - 1,
            origin_y: i32::MIN,
            width: 4,
            height: 4,
        };
        // (MAX-1) + 4/2 = MAX+1 would overflow plain `+`; saturation pins
        // it at MAX.
        assert_eq!(pointer_center_of_rect(&huge), (i32::MAX, i32::MIN + 2));
        // max_x = (MAX-1) + 3 would also overflow; saturation keeps the
        // clamp inside i32 and inside the rect.
        assert_eq!(
            clamp_point_to_rect((i32::MAX, i32::MIN), &huge),
            (i32::MAX, i32::MIN)
        );
        assert_eq!(
            clamp_point_to_rect((i32::MIN, i32::MAX), &huge),
            (i32::MAX - 1, i32::MIN + 3)
        );
    }

    /// The packaged-app detection keys on the OS `WindowsApps` install tree
    /// (case-insensitive, both the per-machine Program Files layout and the
    /// per-user execution-alias layout) and stays false for classic Win32
    /// binaries — including System32's classic notepad, which a caller can
    /// no longer assume from the name alone on Windows 11.
    #[test]
    fn packaged_app_detection_keys_on_the_windowsapps_tree() {
        assert!(process_path_is_packaged(
            "C:\\Program Files\\WindowsApps\\Microsoft.WindowsNotepad_11.2407.9.0_x64__8wekyb3d8bbwe\\Notepad.exe"
        ));
        assert!(process_path_is_packaged(
            "c:\\users\\u\\appdata\\local\\microsoft\\windowsapps\\mspaint.exe"
        ));
        assert!(!process_path_is_packaged("C:\\Windows\\System32\\cmd.exe"));
        assert!(!process_path_is_packaged(
            "C:\\Windows\\System32\\notepad.exe"
        ));
        // A look-alike directory name is not the install tree.
        assert!(!process_path_is_packaged(
            "C:\\tools\\WindowsApps-mirror\\tool.exe"
        ));
    }

    /// The packaged-keyboard advisory has no runtime seam a CI leg could
    /// exercise, so its wiring is source-counted like the other stay-wired
    /// pins: deleting a push site (or the drain impl) silently weakens the
    /// honesty channel instead of failing loudly.
    #[test]
    fn packaged_keyboard_warning_stays_wired() {
        let src = production_source();
        assert_eq!(
            src.matches("warn_if_packaged_keyboard_target(").count(),
            3,
            "definition + the chord_to (key/hold) call site + the type_text \
             call site: an unwired keyboard action drops the count and fails \
             here"
        );
        assert_eq!(
            src.matches("fn take_warnings(&mut self) -> Vec<String> {")
                .count(),
            1,
            "the trait impl that drains the accumulated warnings must exist"
        );
        assert_eq!(
            src.matches("self.warnings.push(").count(),
            5,
            "exactly five producers: the packaged-keyboard-target \
             advisory, the transparent-zone swallowed-click receipt (r4), \
             the chrome multi-click churn receipt (r5 — the first SC \
             already ran the command), the non-client drag-start \
             receipt (r6 — client-area drag simulation cannot move \
             windows), and the bypassed-zone drag-start receipt (r8 — \
             transparent/nowhere/error starts get the click route's \
             swallowed-press receipt instead of a bare success)"
        );
    }

    /// The virtual-pointer wiring has no runtime seam a CI leg could
    /// exercise either (no hidden desktop in CI), so it is source-counted
    /// the same way: the tracked pointer replaced the unreachable OS
    /// cursor, and a revert to `SetCursorPos`/`GetCursorPos` — the pre-PR
    /// bug that failed the whole mouse class with 0x80070005 — would
    /// compile and pass every leg that cannot run a hidden desktop.
    #[test]
    fn virtual_pointer_stays_wired() {
        let src = production_source();
        assert_eq!(
            src.matches("clamp_point_to_rect(").count(),
            3,
            "definition + the move_pointer_to clamp + the drag's clamped \
             drop-point binding: an unclamped move or drag endpoint drops \
             the count and fails here"
        );
        assert_eq!(
            src.matches("self.pointer =").count(),
            3,
            "move_pointer_to's clamped write + drag's clamped-path waypoint \
             write + drag's clamped drop write: a pointer the cursor APIs \
             bypass fails here"
        );
        assert_eq!(
            src.matches("drop_point").count(),
            8,
            "the drag's clamped drop wiring — the binding, the encodability \
             guard's two coordinates (r6: the guard fires BEFORE the press, \
             refusing past-drop rather than stranding a pressed button), \
             the waypoint endpoint, the pointer's final write, and the \
             delivered button-up's coordinates (two in the client branch + \
             the menu-popup arm's direct drop, r7): reverting the drop or \
             its up to a raw endpoint drops the count and fails here"
        );
        assert_eq!(
            src.matches("pointer_center_of_rect(").count(),
            2,
            "definition + the constructor's screen-center start: a pointer \
             without its conventional start position fails here"
        );
        assert!(
            !src.contains("SetCursorPos(") && !src.contains("GetCursorPos("),
            "no input-desktop cursor-API call may return: both require the \
             input desktop and fail with ERROR_ACCESS_DENIED on the hidden \
             desktop regardless of attachment"
        );
    }

    /// The 16-bit lParam encoding guard (r6): mouse messages carry each
    /// coordinate in a signed 16-bit field, and a virtual screen wider/
    /// taller than 32767 px (legal multi-4K arrangements) produces points
    /// past that range that wrap through the u16 truncation to a negative
    /// coordinate — the hit-test probe and the post would agree with each
    /// other and BOTH disagree with reality. The action entries refuse
    /// instead; the release path deliberately does not (a refused up
    /// strands the press).
    #[test]
    fn mouse_point_encoding_refuses_points_past_the_16_bit_fields() {
        assert!(mouse_point_encodable(0, 0));
        assert!(
            mouse_point_encodable(-1920, 1079),
            "negative multi-monitor coordinates are the normal shape and \
             encode faithfully (GET_X_LPARAM's i16 reinterpret)"
        );
        assert!(
            mouse_point_encodable(32767, -32768),
            "the i16 range endpoints themselves encode"
        );
        assert!(
            !mouse_point_encodable(32768, 0),
            "one past the high end wraps to -32768 through the u16 \
             truncation"
        );
        assert!(!mouse_point_encodable(0, -32769), "one past the low end");
        assert!(!mouse_point_encodable(40000, 40000));
        // The wiring: every action entry that derives a screen point
        // guards it (production-only cut — the pure assertions above live
        // in the test half).
        let src = production_source();
        assert_eq!(
            src.matches("mouse_point_encodable(").count(),
            7,
            "definition + move_to + click + mouse_down + scroll + the \
             drag's clamped start and clamped drop: an unguarded entry \
             wraps the coordinate silently (mouse_up is deliberately \
             unguarded — a refused release strands the press)"
        );
        assert_eq!(
            src.matches("unencodable_point_error()").count(),
            7,
            "each guarded entry refuses with the explicit oversized-screen \
             error, not a generic failure"
        );
    }

    /// The NC message selection mirrors the client triples button-by-button,
    /// the NC double-click substitutes WITHOUT the CS_DBLCLKS gate (the OS
    /// synthesizes NC double-clicks for every class), and the client route
    /// is exactly `HTCLIENT` plus the undrivable zones (HTNOWHERE /
    /// HTERROR / HTTRANSPARENT — a window asking to be bypassed gets the
    /// honest pre-routing client posts, never an NC message with a
    /// meaningless zone code). Pure; pinned.
    #[test]
    fn nc_message_selection_follows_the_button_and_zone() {
        assert_eq!(
            nc_mouse_button_messages(MouseButton::Left),
            (WM_NCLBUTTONDOWN, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONUP)
        );
        assert_eq!(
            nc_mouse_button_messages(MouseButton::Right),
            (WM_NCRBUTTONDOWN, WM_NCRBUTTONDBLCLK, WM_NCRBUTTONUP)
        );
        assert_eq!(
            nc_mouse_button_messages(MouseButton::Middle),
            (WM_NCMBUTTONDOWN, WM_NCMBUTTONDBLCLK, WM_NCMBUTTONUP)
        );
        // Press 1 = down, press 2 = DBLCLK (no class-style gate on the NC
        // side), press 3 = down, press 4 = DBLCLK.
        assert_eq!(
            nc_multi_click_message(0, WM_NCLBUTTONDOWN, WM_NCLBUTTONDBLCLK),
            WM_NCLBUTTONDOWN
        );
        assert_eq!(
            nc_multi_click_message(1, WM_NCLBUTTONDOWN, WM_NCLBUTTONDBLCLK),
            WM_NCLBUTTONDBLCLK
        );
        assert_eq!(
            nc_multi_click_message(2, WM_NCLBUTTONDOWN, WM_NCLBUTTONDBLCLK),
            WM_NCLBUTTONDOWN
        );
        assert_eq!(
            nc_multi_click_message(3, WM_NCLBUTTONDOWN, WM_NCLBUTTONDBLCLK),
            WM_NCLBUTTONDBLCLK
        );
        // HTCLIENT is the client route, and so are the undrivable zones:
        // HTNOWHERE (0), HTERROR (-2 → u32::MAX-1), and HTTRANSPARENT
        // (-1 → u32::MAX — the window asks for input to pass THROUGH it to
        // whatever is underneath). Every other zone code routes to its
        // per-zone NC delivery (r5: chrome SC / caption / menu / modal
        // refusal / no-op — see nc_click_sequences_follow_the_os_shape);
        // the numeric neighbors stay distinct (HTCLIENT == 1;
        // HTCAPTION == 2).
        assert!(zone_is_client(HTCLIENT));
        assert!(zone_is_client(0));
        assert!(zone_is_client(u32::MAX));
        assert!(zone_is_client(u32::MAX - 1));
        assert!(!zone_is_client(2));
    }

    /// The non-client routing has no runtime seam a CI leg could exercise
    /// (no hidden desktop in CI), so it is source-counted like the other
    /// stay-wired pins: the zone query, the click/hover route points, and
    /// the NC click executor must all stay wired, or a revert to
    /// client-only posting — the pre-routing shape that silently ignored
    /// every caption/button/menu click — would compile and pass every leg
    /// that cannot run a hidden desktop.
    #[test]
    fn non_client_routing_stays_wired() {
        let src = production_source();
        assert_eq!(
            src.matches("nc_zone_of(").count(),
            4,
            "definition + the click route point + the hover route point + \
             the drag-start probe (r6): a revert that skips the zone query \
             and posts client messages everywhere fails here"
        );
        assert_eq!(
            src.matches("nc_click(").count(),
            2,
            "definition + the click dispatch: an unrouted NC zone falls back \
             to the client posts and fails here"
        );
        assert_eq!(
            src.matches("zone_is_client(").count(),
            4,
            "definition + both route verdicts + the drag-start receipt \
             check (r6): a route that treats NC zones as client space \
             fails here"
        );
        assert_eq!(
            src.matches("nc_multi_click_message(").count(),
            3,
            "definition + the plain-pair builder + the caption press \
             builder: the NC double-click substitution must stay wired on \
             BOTH consumers (r5 fixwave: the caption planner added the \
             third call site)"
        );
        assert_eq!(
            src.matches("nc_press_pair(").count(),
            2,
            "definition + the nc_click pair arm: the per-press (press, \
             release) pair must stay wired for the plain-pair zones (menu \
             opens, DefWindowProc no-ops)"
        );
        assert_eq!(
            src.matches("nc_zone_delivery(").count(),
            6,
            "definition + the pre-post refusal + the non-primary-button \
             gate + the Close-disabled gate + the per-press arm + the \
             drag-start ModalLoop refusal (r6): the per-zone delivery \
             model (chrome SC / caption / menu / modal refusal / no-op) \
             must stay wired — a revert to the uniform (down, up) pair \
             reopens the modal-loop wedges (r5)"
        );
        assert_eq!(
            src.matches("nc_chrome_system_command(").count(),
            2,
            "definition + the chrome arm: caption buttons must deliver \
             their WM_SYSCOMMAND end state — the posted NC up can never \
             complete the OS's button-tracking pump (r5)"
        );
        assert_eq!(
            src.matches("nc_close_command_enabled(").count(),
            2,
            "definition + the HTCLOSE gate: a disabled/absent Close must be \
             refused, not reported as a no-op success (r5)"
        );
        assert_eq!(
            src.matches("nc_caption_press(").count(),
            2,
            "definition + the caption arm: caption DOWN presses must get \
             their deterministic move-loop completion (r5)"
        );
        assert_eq!(
            src.matches("menu_popup_needs_screen_coords(").count(),
            4,
            "definition + the click route + the hover route (r6) + the drag \
             route (r7): a menu popup's items must be driven with SCREEN \
             coordinates (the tracking loop reads mouse lParams as screen \
             points — client-translated posts land on the wrong item; the \
             hover had the same defect as a wrong-item highlight and the \
             drag route delivered its whole press/move/release assembly at \
             the wrong screen points) (r5/r6/r7)"
        );
        assert_eq!(
            src.matches("zone_is_transparent(").count(),
            2,
            "definition + the bypassed-family judgment: the transparent \
             classification feeds the degraded-route warning through \
             zone_is_bypassed"
        );
        assert_eq!(
            src.matches("zone_is_bypassed(").count(),
            4,
            "definition + the client-route judgment + the click receipt \
             warning + the drag-start receipt (r8): every unroutable zone \
             answer (HTTRANSPARENT, HTNOWHERE, HTERROR) must degrade to \
             client posts WITH the swallowed-click warning — a silent \
             degradation is a bare success on an input that was likely \
             lost (r4 transparent; r7 extended to HTNOWHERE/HTERROR; r8 \
             extended the receipt to the drag route)"
        );
        assert_eq!(
            src.matches("WS_DISABLED.0 as isize != 0").count(),
            1,
            "the nc_click disabled-window gate (r7): the OS routes no real \
             input to a disabled window, so no NC delivery — least of all \
             an SC_CLOSE — may be synthesized on one (effective_disabled's \
             parenthesized form is the OTHER occurrence and is not counted \
             by this exact needle)"
        );
        // The probe's bounded wait, pinned at the CALL SHAPE (r5: the old
        // prose-count anchor counted doc comments and the import, and its
        // rationale cited caption/class reads that take no SMTO). The
        // anchors are single-line TOKENS, never multi-line shapes (r5
        // fixwave: Windows runners check out CRLF, so any `\n` anchor
        // counts 0 there and fails the leg while every Linux gate stays
        // green — the exact failure the first fixwave shipped).
        assert_eq!(
            src.matches("const NC_HITTEST_TIMEOUT_MS").count(),
            1,
            "the probe budget constant must exist exactly once"
        );
        assert_eq!(
            src.matches("NC_HITTEST_TIMEOUT_MS,").count(),
            1,
            "exactly one production consumer passes the budget to \
             SendMessageTimeoutW (the probe call; production-only cut): a \
             revert to unbounded SendMessageW stalls the action behind a \
             wedged app"
        );
        assert_eq!(
            src.matches("self.post(target, message, zone as usize, lparam)")
                .count(),
            2,
            "the NC press must carry the zone code in wParam — once in the \
             caption arm and once in the plain-pair arm (the NC message \
             contract; lParam carries the screen point)"
        );
        assert_eq!(
            src.matches("self.post(target, up, zone as usize, lparam)")
                .count(),
            1,
            "the NC release must carry the zone code in wParam too (and the \
             stranded-button honesty arm depends on this exact shape)"
        );
    }

    /// The UIA-helper wiring has no runtime seam a CI leg could exercise
    /// (no hidden desktop in CI), so it is source-counted: the lazy spawn
    /// gate and all three helper dispatches must stay wired, and the
    /// cooldown must stay between them — a revert that drops the helper
    /// (back to the degraded walk as primary) or that hammers a broken
    /// environment with one process per query fails here.
    #[test]
    fn uia_helper_wiring_stays_wired() {
        let src = production_source();
        assert_eq!(
            src.matches("fn ensure_uia_helper(&mut self)").count(),
            1,
            "the lazy spawn gate must exist and be the only entry to the \
             helper"
        );
        assert_eq!(
            src.matches("UiaHelperClient::spawn(").count(),
            1,
            "the spawn call must live inside the gate (a spawn on a hot path \
             without the cooldown check fails here)"
        );
        for dispatch in ["helper.tree(", "helper.at_point(", "helper.focus("] {
            assert_eq!(
                src.matches(dispatch).count(),
                1,
                "the {dispatch} dispatch must stay wired: dropping it silently \
                 degrades the answer to the window-class walk"
            );
        }
        assert_eq!(
            src.matches("HELPER_RESPAWN_COOLDOWN_MS").count(),
            5,
            "definition + the spawn-failure arm + one arm per query \
             (tree/at_point/focus): the cooldown must arm on EVERY helper \
             TRANSPORT failure (the gate itself compares the armed \
             deadline), not just the spawn failure"
        );
        assert_eq!(
            src.matches("UiaHelperOutcome::Soft").count(),
            3,
            "each query must distinguish the healthy helper's SOFT verdict \
             (an honest error, client kept) from a transport failure (client \
             dropped): a soft verdict that tore the client down would respawn \
             a good process on every cooldown lapse — and the focus verdict \
             rides the keystroke path"
        );
        assert_eq!(
            src.matches("self.uia_helper = None").count(),
            3,
            "exactly the three TRANSPORT-failure arms may tear the client \
             down: a fourth occurrence means a SOFT verdict (the healthy \
             helper's honest 'cannot answer') was conflated with transport \
             death — respawn churn on every cooldown lapse. Presence alone \
             is not retention: this pins which arms may discard (r4, the \
             mutation 'soft arm clears the client' is RED against it)"
        );
        assert!(
            src.contains("uia_helper: Option<UiaHelperClient>"),
            "the backend must hold the helper as an optional client (drop = \
             respawn-after-cooldown semantics)"
        );
        assert_eq!(
            src.matches("let inherit_handles = std_handles.is_some();")
                .count(),
            1,
            "the contained launcher must flip bInheritHandles ON exactly when \
             the std-handle triple is present: without inheritance the OS \
             duplicates NO handle into the child and the helper's pipes never \
             reach it (the GUI launch sites pass None and keep the default)"
        );
    }

    /// The stop-time wind-down has no runtime seam a CI leg could exercise
    /// either (no hidden desktop, and the release path only fires from the
    /// command layer), so its wiring is source-counted: the trait override,
    /// the retained-handle recording, the by-handle terminate, and the
    /// Drop-side close must all stay wired — a revert to "stop only stops
    /// driving" (the disclosed phase-1 shape) would compile and pass
    /// everything that cannot run a hidden desktop.
    #[test]
    fn release_wind_down_stays_wired() {
        let src = production_source();
        assert_eq!(
            src.matches("fn release_os_grant(&mut self)").count(),
            1,
            "the Windows stack must override the release hook (the trait \
             default is a no-op — a revert to it silently disables the \
             wind-down)"
        );
        assert_eq!(
            src.matches("self.opened_processes.push(process)").count(),
            1,
            "every successful open must record its spawn handle: without the \
             recording the wind-down terminates nothing"
        );
        assert_eq!(
            src.matches("std::mem::take(&mut self.opened_processes)")
                .count(),
            1,
            "the release path must consume the list (a re-stop after a \
             release finds it empty instead of re-terminating)"
        );
        // The fuller statement shape keeps this pin from matching the
        // crash-sweep's `if let Err(error) = TerminateProcess(process, 1)`
        // (a same-named binding that predates the wind-down).
        assert_eq!(
            src.matches("let terminated = unsafe { TerminateProcess(process, 1) };")
                .count(),
            1,
            "the by-handle terminate (no pid-recycling ambiguity) must stay \
             in the release path"
        );
        assert_eq!(
            src.matches("self.opened_processes.drain(..)").count(),
            1,
            "Drop must close whatever handles a release left behind (usually \
             none — the bookkeeping keeps the handle count honest)"
        );
        assert_eq!(
            src.matches("self.opened_app_count = 0").count(),
            1,
            "release must reset the launch budget WITH the list (the Linux \
             stack's take-the-children shape): deleting the reset re-creates \
             the lifetime-cap regression where a re-granted session has no \
             fresh opens after the cap's worth of lifetime launches — an \
             r3-MAJOR fix that would otherwise pass every gate silently"
        );
        // The shell is infrastructure, not a wind-down entry: its spawn
        // handle closes AT THE LAUNCH SITE (r5, mutation-proven unpinned —
        // deleting the close AND renaming the binding left every gate
        // green while leaking one HANDLE per session grant).
        assert_eq!(
            src.matches("let _ = unsafe { CloseHandle(explorer_process) };")
                .count(),
            1,
            "the session shell's spawn handle must close at creation (it is \
             never consulted and must neither join the wind-down list nor \
             leak)"
        );
        // The budget must be charged only for launches that SUCCEEDED:
        // incrementing before the launch would bill the budget for failed
        // opens and contradict the capacity error's own copy.
        assert_eq!(
            src.matches("self.opened_app_count += 1").count(),
            1,
            "exactly one budget charge, after a successful launch (an \
             increment before launch_in_job's `?` would charge for failures)"
        );
    }

    /// The window-class tree's pure helpers: the password verdict fires only
    /// for the classic edit classes (the same numeric style bit names
    /// unrelated styles elsewhere), the line format matches the physical
    /// stack's `[i] role "name" (x,y,w,h) flags` shape, and the budgets
    /// mirror the physical defaults/caps exactly. Pure; pinned.
    #[test]
    fn class_tree_helpers_follow_the_shared_contract() {
        // ES_PASSWORD on the classic edit classes is secure; the same bit on
        // another class is that class's unrelated style and must NOT read as
        // a password.
        assert!(password_style_for_class("Edit", ES_PASSWORD as isize));
        assert!(password_style_for_class(
            "RichEdit20W",
            ES_PASSWORD as isize
        ));
        assert!(password_style_for_class(
            "RICHEDIT50W",
            ES_PASSWORD as isize
        ));
        assert!(!password_style_for_class("Edit", 0));
        assert!(!password_style_for_class("RichEdit20W", 0));
        assert!(!password_style_for_class("Button", ES_PASSWORD as isize));
        // A window with no class name cannot be classified secure.
        assert!(!password_style_for_class("", ES_PASSWORD as isize));

        let line = format_class_tree_line(7, 2, "Button", "Cancel", -1920, 10, 120, 40, "");
        assert_eq!(line, "    [7] Button \"Cancel\" (-1920,10,120,40)");
        let flagged =
            format_class_tree_line(0, 0, "Edit", "Password", 0, 0, 80, 20, " disabled focused");
        assert_eq!(
            flagged,
            "[0] Edit \"Password\" (0,0,80,20) disabled focused"
        );

        let (depth, nodes) = tree_budget(&UiTreeOptions {
            max_depth: None,
            max_nodes: None,
        });
        assert_eq!(depth, DEFAULT_TREE_MAX_DEPTH);
        assert_eq!(nodes, DEFAULT_TREE_MAX_NODES);
        // Caller values ride through until a hard cap: the tool schema's
        // maximums equal these ceilings (24 / 2000 — r5 aligned them after
        // they advertised 64/10000 the clamp silently overrode), and an
        // over-cap request is clamped like the physical stack clamps it.
        let (depth, nodes) = tree_budget(&UiTreeOptions {
            max_depth: Some(2),
            max_nodes: Some(5),
        });
        assert_eq!((depth, nodes), (2, 5));
        let (depth, nodes) = tree_budget(&UiTreeOptions {
            max_depth: Some(u32::MAX),
            max_nodes: Some(u32::MAX),
        });
        assert_eq!((depth, nodes), (MAX_TREE_DEPTH, MAX_TREE_NODES));
    }

    /// The per-zone NC delivery — the contract the source-count pins above
    /// can only count (r5: the delivery follows what the OS does per zone,
    /// verified against ReactOS's win32k mirror):
    /// - the plain-pair zones (menu bar / system menu / DefWindowProc
    ///   no-ops) keep the OS's NC double-click shape (count=1 → DOWN,UP;
    ///   count=2 → DOWN,UP,DBLCLK,UP on EVERY class) with the per-button
    ///   triples wired correctly;
    /// - chrome buttons deliver their SC end state (style-derived min/max
    ///   restore, the Close-disabled gate);
    /// - the caption delivers DOWN presses completed by a client UP and
    ///   loop-free DBLCLK presses bare;
    /// - the modal-loop zones (resize borders, scroll bars) REFUSE.
    /// Pure.
    #[test]
    fn nc_click_sequences_follow_the_os_shape() {
        // Plain-pair zones keep the OS shape.
        fn sequence(count: u8, button: MouseButton) -> Vec<(u32, u32)> {
            (0..count.max(1))
                .map(|index| nc_press_pair(index, button))
                .collect()
        }
        assert_eq!(
            sequence(1, MouseButton::Left),
            vec![(WM_NCLBUTTONDOWN, WM_NCLBUTTONUP)],
            "single click: plain DOWN then UP"
        );
        assert_eq!(
            sequence(2, MouseButton::Left),
            vec![
                (WM_NCLBUTTONDOWN, WM_NCLBUTTONUP),
                (WM_NCLBUTTONDBLCLK, WM_NCLBUTTONUP),
            ],
            "double click: the second press substitutes the DBLCLK message on \
             every class (NC double-clicks are not CS_DBLCLKS-gated) and the \
             UP still follows it"
        );
        assert_eq!(
            sequence(3, MouseButton::Left),
            vec![
                (WM_NCLBUTTONDOWN, WM_NCLBUTTONUP),
                (WM_NCLBUTTONDBLCLK, WM_NCLBUTTONUP),
                (WM_NCLBUTTONDOWN, WM_NCLBUTTONUP),
            ],
            "triple click: the OS re-arms the double-click detector on every \
             odd press, so press 3 is a plain DOWN again"
        );
        assert_eq!(
            sequence(1, MouseButton::Right),
            vec![(WM_NCRBUTTONDOWN, WM_NCRBUTTONUP)],
            "the right-button triple must be wired, not aliased to left"
        );
        assert_eq!(
            sequence(1, MouseButton::Middle),
            vec![(WM_NCMBUTTONDOWN, WM_NCMBUTTONUP)],
            "the middle-button triple must be wired, not aliased to left"
        );

        // Zone families route as delivered.
        assert_eq!(
            nc_zone_delivery(HTCLOSE),
            NcZoneDelivery::ChromeButton,
            "the close button delivers its SC end state, not NC posts"
        );
        assert_eq!(nc_zone_delivery(HTMINBUTTON), NcZoneDelivery::ChromeButton);
        assert_eq!(nc_zone_delivery(HTMAXBUTTON), NcZoneDelivery::ChromeButton);
        assert_eq!(
            nc_zone_delivery(HTCAPTION),
            NcZoneDelivery::Caption,
            "the caption completes its move loop per press"
        );
        assert_eq!(nc_zone_delivery(HTMENU), NcZoneDelivery::Menu);
        assert_eq!(nc_zone_delivery(HTSYSMENU), NcZoneDelivery::Menu);
        assert_eq!(
            nc_zone_delivery(HTLEFT),
            NcZoneDelivery::ModalLoop,
            "resize borders refuse: the SC_SIZE loop cannot be completed by \
             posted messages"
        );
        assert_eq!(nc_zone_delivery(HTBOTTOMRIGHT), NcZoneDelivery::ModalLoop);
        assert_eq!(nc_zone_delivery(HTVSCROLL), NcZoneDelivery::ModalLoop);
        assert_eq!(nc_zone_delivery(HTHSCROLL), NcZoneDelivery::ModalLoop);
        assert_eq!(
            nc_zone_delivery(HTBORDER),
            NcZoneDelivery::NoOp,
            "HTBORDER is a DefWindowProc no-op (explicit `break`) — the \
             plain pair completes harmlessly"
        );
        assert_eq!(nc_zone_delivery(HTHELP), NcZoneDelivery::NoOp);
        assert_eq!(nc_zone_delivery(HTGROWBOX), NcZoneDelivery::NoOp);
        assert_eq!(nc_zone_delivery(HTOBJECT), NcZoneDelivery::NoOp);

        // Chrome SC codes mirror NC_DoButton's style mapping, including its
        // style gates: a window lacking the backing style bits has no
        // working button there (real input no-ops; custom chrome answering
        // the zone without the style must be refused, not over-delivered).
        assert_eq!(
            nc_chrome_system_command(HTCLOSE, WS_SYSMENU.0 as isize),
            Some(SC_CLOSE),
            "close on a window with a system menu is SC_CLOSE"
        );
        assert_eq!(
            nc_chrome_system_command(HTMINBUTTON, (WS_MINIMIZEBOX | WS_MINIMIZE).0 as isize),
            Some(SC_RESTORE),
            "min on an already-minimized window restores"
        );
        assert_eq!(
            nc_chrome_system_command(HTMINBUTTON, WS_MINIMIZEBOX.0 as isize),
            Some(SC_MINIMIZE),
            "min on a normal window minimizes"
        );
        assert_eq!(
            nc_chrome_system_command(HTMAXBUTTON, (WS_MAXIMIZEBOX | WS_MAXIMIZE).0 as isize),
            Some(SC_RESTORE),
            "max on an already-maximized window restores"
        );
        assert_eq!(
            nc_chrome_system_command(HTMAXBUTTON, WS_MAXIMIZEBOX.0 as isize),
            Some(SC_MAXIMIZE),
            "max on a normal window maximizes"
        );
        assert_eq!(
            nc_chrome_system_command(HTMINBUTTON, WS_MINIMIZE.0 as isize),
            None,
            "HTMINBUTTON without WS_MINIMIZEBOX refuses: NC_DoButton \
             no-ops the press (custom chrome answering the zone without \
             the backing style must not get a synthesized SC)"
        );
        assert_eq!(
            nc_chrome_system_command(HTMAXBUTTON, 0),
            None,
            "HTMAXBUTTON without WS_MAXIMIZEBOX refuses"
        );
        assert_eq!(
            nc_chrome_system_command(HTCLOSE, 0),
            None,
            "HTCLOSE without WS_SYSMENU refuses (the OS would not offer \
             the button; the GetMenuState close gate covers the remaining \
             grayed/disabled/CS_NOCLOSE shapes)"
        );

        // The Close-disabled gate mirrors NC_DoButton's own refusal.
        assert!(nc_close_command_enabled(0), "enabled item");
        assert!(
            !nc_close_command_enabled(u32::MAX),
            "an absent Close item (GetMenuState's -1) refuses"
        );
        assert!(!nc_close_command_enabled(MF_GRAYED.0), "grayed refuses");
        assert!(!nc_close_command_enabled(MF_DISABLED.0), "disabled refuses");

        // Caption presses: the LEFT DOWN needs its client-UP completion (the
        // SC_MOVE loop it opens exits on a client button-up), the RIGHT DOWN
        // needs a SCREEN-coords right-button release (the system-menu
        // tracker reads screen points — r7), MIDDLE opens nothing, the
        // DBLCLK press is loop-free, and the parity substitution has no
        // class gate.
        assert_eq!(
            nc_caption_press(0, MouseButton::Left),
            (WM_NCLBUTTONDOWN, CaptionCompletion::MoveLoopExit),
            "press 1: DOWN, completed by a client UP"
        );
        assert_eq!(
            nc_caption_press(1, MouseButton::Left),
            (WM_NCLBUTTONDBLCLK, CaptionCompletion::None),
            "press 2: loop-free DBLCLK (the SC fires directly)"
        );
        assert_eq!(
            nc_caption_press(2, MouseButton::Left),
            (WM_NCLBUTTONDOWN, CaptionCompletion::MoveLoopExit),
            "press 3 re-arms as a plain DOWN"
        );
        assert_eq!(
            nc_caption_press(0, MouseButton::Right),
            (WM_NCRBUTTONDOWN, CaptionCompletion::SystemMenuRelease),
            "the right-button caption triple must be wired, not aliased, and \
             its menu-tracking loop needs the screen-coords right release"
        );
        assert_eq!(
            nc_caption_press(1, MouseButton::Right),
            (WM_NCRBUTTONDBLCLK, CaptionCompletion::None),
            "the right dblclk is loop-free (r7)"
        );
        assert_eq!(
            nc_caption_press(0, MouseButton::Middle),
            (WM_NCMBUTTONDOWN, CaptionCompletion::None),
            "middle caption opens no loop (DefWindowProc no-op) — nothing to \
             complete (r7: the old shape completed it with a stray LEFT up)"
        );

        // Menu popups take screen coordinates.
        assert!(menu_popup_needs_screen_coords("#32768"));
        assert!(!menu_popup_needs_screen_coords("Edit"));
        assert!(!menu_popup_needs_screen_coords(""));
        assert!(
            !menu_popup_needs_screen_coords("#327679"),
            "prefix-lookalike"
        );
    }

    /// The class-tree row flags: `disabled` carries IsWindowEnabled
    /// semantics (the enabled state is inherited down the parent chain, so
    /// the DFS-carried effective state must fire the flag with the window's
    /// OWN style bit clear too — the exact case the raw GWL_STYLE read
    /// missed), and `password` rides LAST like the physical stack's trailing
    /// flag. Pure (r4).
    #[test]
    fn class_tree_row_flags_follow_is_enabled_and_password_semantics() {
        let ws_disabled = WS_DISABLED.0 as isize;
        assert!(effective_disabled(ws_disabled, false), "own bit set");
        assert!(
            !effective_disabled(0, false),
            "own bit clear, nothing inherited"
        );
        assert!(
            effective_disabled(0, true),
            "a child of a disabled pane is disabled with its own bit clear"
        );
        assert!(effective_disabled(ws_disabled, true));
        let line = format_class_tree_line(3, 1, "Edit", "pw", 0, 0, 10, 10, " disabled password");
        assert_eq!(
            line, "  [3] Edit \"pw\" (0,0,10,10) disabled password",
            "the password flag rides LAST, mirroring the physical stack's \
             flag order (disabled, focused, offscreen, password)"
        );
    }

    /// The receipt predicates: HTTRANSPARENT alone decides the transparent
    /// classification (sign-extended u32 encoding), and the bypassed family
    /// — the zones degraded to client posts WITH the swallowed-click
    /// warning — is exactly {HTTRANSPARENT, HTNOWHERE, HTERROR} (r7
    /// extended the warning to the latter two). Pure (r4/r7).
    #[test]
    fn transparent_zone_receipt_predicate_matches_the_zone_only() {
        assert!(zone_is_transparent(HTTRANSPARENT as u32));
        assert!(zone_is_transparent(u32::MAX), "-1 sign-extended");
        assert!(!zone_is_transparent(HTCLIENT));
        assert!(!zone_is_transparent(HTCAPTION), "HTCAPTION is its own case");
        assert!(!zone_is_transparent(HTNOWHERE), "HTNOWHERE is its own case");
        assert!(
            !zone_is_transparent(u32::MAX - 1),
            "HTERROR is its own case"
        );
        // The bypassed family: all three unroutable codes warn, nothing
        // else does.
        assert!(zone_is_bypassed(HTTRANSPARENT as u32));
        assert!(zone_is_bypassed(u32::MAX), "HTTRANSPARENT sign-extended");
        assert!(
            zone_is_bypassed(HTNOWHERE),
            "HTNOWHERE degrades with a warning"
        );
        assert!(zone_is_bypassed(u32::MAX - 1), "HTERROR sign-extended");
        assert!(
            !zone_is_bypassed(HTCLIENT),
            "a true client answer is the normal route, no warning"
        );
        assert!(
            !zone_is_bypassed(HTCAPTION) && !zone_is_bypassed(HTCLOSE),
            "routable NC zones are not bypassed"
        );
    }

    /// The window-class tree wiring has no runtime seam a CI leg could
    /// exercise (no hidden desktop in CI), so it is source-counted: the
    /// capability bit, the three query surfaces, the shared walk, the
    /// degradation header, and the shell exclusion must all stay wired —
    /// a revert to the explicit `unsupported` errors (or to a tree that
    /// hides its degradation from the model) would compile and pass every
    /// leg that cannot run a hidden desktop.
    #[test]
    fn class_tree_stays_wired() {
        let src = production_source();
        assert!(
            src.contains("ui_tree: true"),
            "the hidden-desktop stack must advertise the (degraded) tree — a \
             revert to `ui_tree: false` re-locks the queries behind the tool \
             gate"
        );
        assert_eq!(
            src.matches("fn ui_tree(&mut self, opts: &UiTreeOptions)")
                .count(),
            1,
            "the ui_tree override must replace the unsupported stub"
        );
        assert_eq!(
            src.matches("write_class_tree_node(").count(),
            3,
            "definition + the recursive child call + the per-root call: the \
             depth-first serialization must stay wired"
        );
        assert_eq!(
            src.matches("window_element_info(").count(),
            3,
            "definition + the element_at_point read + the focused_element \
             read: a query surface reverting to `unsupported` drops the \
             count and fails here"
        );
        assert_eq!(
            src.matches("current_focus_hwnd(").count(),
            3,
            "definition + the ui_tree focused flag + the focused_element \
             query: the no-focus-steal focus read must stay wired"
        );
        assert!(
            src.contains("window-class tree (Win32 HWND walk fallback"),
            "the fallback payload must name its own degradation (roles are \
             window classes, captions are names, content is unread) — a tree \
             that hides its semantics from the model fails here"
        );
        assert!(
            src.contains("tree_budget("),
            "the shared budget helper must stay wired (physical-stack parity)"
        );
        assert_eq!(
            src.matches("is_shell_window_class(&window_class_name(hwnd))")
                .count(),
            1,
            "the ui_tree walk must exclude shell windows BEFORE they consume \
             a top-level cap slot (Progman/WorkerW/Shell_TrayWnd carry no \
             drivable content and would crowd the top level of every tree) \
             — the exclusion lives in the walk's skip predicate (r5)"
        );
        assert_eq!(
            src.matches("append_tree_truncation_footers").count(),
            2,
            "the import + the walk call: all truncation footers (node, \
             depth, top-level cap) must be emitted through the ONE shared \
             builder the physical stack and the UIA helper also use \
             (byte-identical payloads across all three stacks; the literals \
             themselves are pinned by the Linux-runnable golden test in \
             helpers.rs) (r5)"
        );
        assert_eq!(
            src.matches("roots.into_iter().rev()").count(),
            1,
            "the ui_tree consumption must reverse the painter-order roots \
             into topmost-first (r8): the node budget then cuts BOTTOM-most \
             windows, the same direction the cap footer declares — a revert \
             to painter order silently drops the active (topmost) window \
             first, exactly the regression r7 closed; the helper leg carries \
             the same pin in uia_helper.rs"
        );
        assert_eq!(
            src.matches("rect.right.saturating_sub(rect.left)").count(),
            3,
            "the capture composite + the class-tree row + window_element_info \
             (r8): a hostile app can wrap its own GetWindowRect pair via \
             SetWindowPos, and the plain difference panics (debug) or wraps \
             (release) on the extreme pair — the same saturation rationale \
             the UIA serializers landed in r7"
        );
        assert_eq!(
            src.matches("rect.bottom.saturating_sub(rect.top)").count(),
            3,
            "the same three height subtractions, saturated for the same \
             wrapped-rect reason (r8)"
        );
        assert_eq!(
            src.matches("CaptionCompletion::MoveLoopExit").count(),
            2,
            "the caption arm's LEFT completion (planner + executor): without \
             the deterministic client button-up after a LEFT caption DOWN, \
             the target stays captured in the SC_MOVE modal loop behind a \
             reported success (r5, mutation-proven unpinned; r7 typed the \
             completion per button)"
        );
        assert_eq!(
            src.matches("CaptionCompletion::SystemMenuRelease").count(),
            2,
            "the caption arm's RIGHT completion (planner + executor): the \
             system-menu tracking loop reads SCREEN coordinates, so the \
             release rides at the screen caption point — the old shape \
             completed a right press with a client-coords LEFT up, ending \
             tracking at a decoded point near the origin (r7)"
        );
        assert_eq!(
            src.matches("self.post(target, WM_LBUTTONUP, 0, pack_mouse_lparam(point.x, point.y))?")
                .count(),
            1,
            "the LEFT completion post itself (a client WM_LBUTTONUP with no \
             button mark at the caption point) — deleting the post while \
             keeping the gate compiles and wedges caption presses in the \
             OS's move loop"
        );
        assert_eq!(
            src.matches("pack_mouse_lparam(screen_x, screen_y)").count(),
            2,
            "the SCREEN-coords posts: the NC lparam binding + the \
             system-menu release (r7) — a client-translated release in the \
             menu tracker decodes to a point near the screen origin"
        );
        assert_eq!(
            src.matches("GetWindowTextLengthW").count(),
            2,
            "the import + the caption read: the T3 screening verdict and \
             raw binding must run on the FULL raw caption — the 128-unit \
             fixed buffer let a padded caption screen as Clear on the \
             degraded path, the exact padding-evasion tool.rs's contract \
             closes (r5)"
        );
        assert!(
            src.contains("MAX_CAPTION_UNITS: usize = 4096"),
            "the full-caption read stays bounded (a hostile caption cannot \
             drive an unbounded allocation)"
        );
        assert!(
            src.contains("flags.push_str(\" hidden\")"),
            "invisible child windows must be flagged 'hidden', not rendered as \
             ordinary clickable rows (the click path skips invisible windows)"
        );
        assert_eq!(
            src.matches("flags.push_str(\" password\")").count(),
            1,
            "the fallback's only secure-field signal (the classic \
             edit-family ES_PASSWORD style) must be visible at the ROW level \
             like the physical stack's trailing 'password' flag — dropping it \
             hides the secure signal exactly where the model reads whole \
             trees (r4)"
        );
        assert_eq!(
            src.matches("effective_disabled(").count(),
            2,
            "definition + the row flag: the 'disabled' flag must carry \
             IsWindowEnabled semantics (parent-inherited), not just the raw \
             style bit — the DFS carries the computed state down (r4 parity \
             with the physical stack's UIA IsEnabled)"
        );
        assert_eq!(
            src.matches("effective_disabled(style, inherited_disabled)")
                .count(),
            1,
            "the walk must FEED the inherited state into the predicate — a \
             call site that passes `false` compiles, keeps every count, and \
             silently loses the disabled flag on children of disabled panes \
             (r5, mutation-proven unpinned)"
        );
    }

    /// R15: the capture-honesty fixes have no runtime seam and no behavior
    /// test on this leg — pin their CALL SITES in the production source so
    /// deleting the wiring (not just the pure helper) fails the Windows
    /// leg. Counts are production-only (the helper cuts at `mod tests {`).
    #[test]
    fn capture_honesty_call_sites_stay_wired() {
        let src = production_source();
        // PatBlt answers windows_core::BOOL: a failed background clear must
        // fail the frame instead of compositing over stale pixels (R14).
        assert_eq!(
            src.matches("if !PatBlt(").count(),
            1,
            "the background-clear failure gate must exist exactly once"
        );
        // The production literal wraps across a `\`-continuation, so the
        // raw `include_str!` bytes never hold the whole sentence (a
        // single-sentence needle was always false): pin BOTH fragments.
        assert!(
            src.contains("failing this frame instead of compositing")
                && src.contains("over stale pixels"),
            "the PatBlt failure arm must carry the honesty rationale"
        );
        // The GetDIBits short-read rejection must be CALLED in read_rgba —
        // a helper-only pin passed with the call deleted.
        assert_eq!(
            src.matches("short_read_reason(").count(),
            2,
            "short_read_reason must exist as the helper definition AND be \
             called at the GetDIBits site"
        );
        // read_rgba must run the row flip/swizzle on the GetDIBits buffer.
        assert_eq!(
            src.matches("flip_and_swizzle_bgrx_to_rgba(").count(),
            2,
            "flip_and_swizzle_bgrx_to_rgba must exist as the definition AND \
             be called on the composite buffer"
        );
        // The composite blit goes through the tmp bitmap clamped to the
        // session rect (a stale band above the clamped size used to read
        // as content — R13).
        assert_eq!(
            src.matches("ensure_tmp_bitmap(").count(),
            2,
            "ensure_tmp_bitmap must exist as the definition AND be called on \
             the blit path"
        );
        // R16: the CLAMP itself must be pinned — the count pin above cannot
        // see a reversion of the blit size to the raw window rect, which
        // re-creates the stale band beyond what PrintWindow rendered (the
        // exact R13 regression) while the allocation gate stays wired.
        assert_eq!(
            src.matches("let blit_w = w.min(self.rect.width).max(1);")
                .count(),
            1,
            "the blit width must be clamped to the session rect (the stale \
             band regression shape)"
        );
        assert_eq!(
            src.matches("let blit_h = h.min(self.rect.height).max(1);")
                .count(),
            1,
            "the blit height must be clamped to the session rect"
        );
        // R15: the open-capacity copy states the lifetime-budget truth.
        assert!(
            src.contains("past launches keep counting toward this limit"),
            "the capacity error must state that exited apps keep counting, \
             not the old 'recorded as running' over a lifetime count"
        );
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
        // A backslash directly before an EMBEDDED quote is an escape run
        // too: it doubles, so the part's literal backslash survives the
        // parse instead of being consumed as the quote's own escape. Per
        // the documented rule (3 backslashes + quote = 1 backslash +
        // literal quote character), `"C:\dir\\\"x"` parses back to the
        // original `C:\dir\"x`.
        assert_eq!(quote_command_line_part(r#"C:\dir\"x"#), r#""C:\dir\\\"x""#);
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
        // R18: the set-but-empty filter lives in the ONE resolver both the
        // probe and this plan share — the probe test's existence closures
        // cannot observe it, so the filter is pinned here. Deleting the
        // filter would make the probe answer "\explorer.exe" while the
        // spawn plan resolved "C:\Windows\explorer.exe" (the exact
        // probe/spawn disagreement the R17 wave closed).
        assert_eq!(explorer_path(Some("")), r"C:\Windows\explorer.exe");
        assert_eq!(explorer_path(None), r"C:\Windows\explorer.exe");
        assert_eq!(explorer_path(Some(r"D:\Wins")), r"D:\Wins\explorer.exe");
        assert_eq!(
            production_source().matches("explorer_path(").count(),
            3,
            "definition + the probe's resolution + the spawn plan's \
             resolution: two divergent resolvers is the drift this pin \
             refuses"
        );
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
        assert!(is_shell_window_class("Shell_SecondaryTrayWnd"));
        // The Windows 11 shell XAML host (r5): the session explorer spawns
        // it on the hidden desktop and without the exclusion it appears as
        // a tree root.
        assert!(is_shell_window_class("XamlExplorerHostIsolatedWindow"));
        // UWP app windows share the CoreWindow class with some shell
        // experiences — a class-based exclusion would hide drivable apps,
        // so they must stay OUT of the list (disclosed noise instead).
        assert!(!is_shell_window_class("Windows.UI.Core.CoreWindow"));
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
        // Heights ≥ 4 are where a whole-tail swizzle becomes visible: rows
        // swapped in below the midpoint would be re-swizzled on later
        // iterations (self-inverse channel swap ⇒ reverted), so every row of
        // a multi-row buffer is pinned, not just the 2-row shapes.
        let height = 5usize;
        let width = 2usize;
        let mut buffer: Vec<u8> = (0..(width * height * 4) as u32)
            .map(|i| (i * 37 % 251) as u8)
            .collect();
        let expected = {
            let mut reference = buffer.clone();
            for pixel in reference.chunks_exact_mut(4) {
                pixel.swap(0, 2);
                pixel[3] = 255;
            }
            let stride = width * 4;
            let rows: Vec<Vec<u8>> = reference.chunks(stride).rev().map(<[u8]>::to_vec).collect();
            rows.concat()
        };
        flip_and_swizzle_bgrx_to_rgba(&mut buffer, width, height);
        assert_eq!(buffer, expected);
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

    /// Mutation pin (R11 M2, Windows leg): the model-supplied program name
    /// must render ONLY through `logged_program`'s Debug escaping — a
    /// plain `{program}` / `{program:?}` revert at the launch log would
    /// re-open the newline log-forgery channel (the R10 fix shipped the
    /// `{:?}` inline with no pin anywhere).
    #[test]
    fn recorded_program_names_never_render_plain_in_logs() {
        let prod = production_source();
        assert_eq!(
            prod.matches("logged_program(program)").count(),
            1,
            "the launch log must render the program through logged_program (exactly one site)"
        );
        assert!(
            !prod.contains("opened {program:?}") && !prod.contains("opened {program}"),
            "the launch log must not interpolate the model-supplied program name directly"
        );
    }

    /// Wiring pin (R11 minor): the `open` cap gate must run through the
    /// shared `open_capacity_rejected` decision (one pinned home for the
    /// cap semantic; a bypass back to an inline comparison would drift the
    /// stacks apart unobserved).
    #[test]
    fn open_capacity_gate_runs_through_the_shared_decision() {
        let prod = production_source();
        assert!(
            prod.contains("open_capacity_rejected(self.opened_app_count)"),
            "the capacity gate must use the shared decision fn"
        );
        assert!(
            !prod.contains("opened_app_count >= MAX_OPEN_CHILDREN"),
            "the inline comparison shape must stay deleted"
        );
    }

    /// The production half of this file (everything above the test
    /// module), so source-shape pins assert against shipped code and not
    /// against their own assert strings.
    fn production_source() -> &'static str {
        let src = include_str!("windows_virtual.rs");
        let tests_start = src.find("mod tests {").expect("tests module marker");
        &src[..tests_start]
    }
}
