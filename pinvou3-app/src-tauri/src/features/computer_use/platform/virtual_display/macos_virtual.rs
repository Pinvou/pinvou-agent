//! macOS virtual display stack (private CGVirtualDisplay API, experimental
//! with one-time auto-fallback) — virtual-display design §7.
//!
//! Target model: a private-API headless display (DeskPad/BetterDisplay
//! lineage) is created, parked right of the user's main display, apps are
//! launched into it, and all input is delivered **per-process** via
//! `CGEventPostToPid` — never a global `CGEventPost` at `kCGHIDEventTap`,
//! which would move the user's real cursor (forbidden for this target).
//! The user's physical session is never DRIVEN (input is pid-posted, capture
//! is display-id-pinned); apps opened with `open` launch into the user's
//! graphics session first and are AX-moved in best-effort. Teardown releases
//! the display object and the display disappears asynchronously — opened
//! apps are NOT terminated: they keep running, and their windows relocate to
//! the remaining displays when the virtual display disappears.
//!
//! Private API: `CGVirtualDisplay` / `CGVirtualDisplayDescriptor` /
//! `CGVirtualDisplaySettings` / `CGVirtualDisplayMode` are NOT bound by any
//! crate in the dependency tree, so every interaction goes through raw
//! `msg_send!` over `objc2` 0.6. Crash-safety contract: **probe what you
//! send** — [`probe`] checks every class and selector with `AnyClass::get` +
//! `AnyClass::responds_to` before anything is ever sent, and creation
//! re-runs the same probe. Sending an unverified selector to a private class
//! would raise an Obj-C exception with no landing pad (process abort — same
//! reasoning as `platform/screen_capture_kit.rs`'s version gate), so the
//! probe is the crash-safety mechanism, not best-effort politeness.
//!
//! Threading model:
//! - Creation runs on the computer_use worker thread (guaranteed by the
//!   shared dispatch, same contract as the physical backends). The
//!   descriptor→applySettings build is dispatched to the app's main thread
//!   via `platform::dispatch_to_main_thread` (the private display machinery
//!   activates reliably only with a pumped main run loop), and the closure
//!   deliberately does NOT block the main thread for the whole activation
//!   poll: it only reads the display id (bounded grace), then hands the
//!   display back and the worker runs the budgeted activation wait through
//!   global CG APIs while the main run loop keeps pumping — a closure that
//!   slept through the poll would freeze the UI for the entire budget and
//!   contradict the machinery's own pumped-loop premise. If the API refuses
//!   to assign an id within the grace, the closure falls back to the legacy
//!   whole sequence on the main thread (behavior preserved; the freeze is
//!   never worse than the pre-split code). Without a stored app handle the
//!   same sequence runs on the worker thread. On timeout the answer is an
//!   explicit `Unavailable` — there is NO mid-creation physical fallback:
//!   the ONE-TIME probe-based decision already ran in
//!   `platform::create_backend` (design §9), a creation-time failure is an
//!   explicit error that the availability snapshot records, and it is never
//!   a silent mid-session flip.
//! - The CGVirtualDisplay Obj-C object is created on the main thread (or the
//!   worker in the fallback) and crosses threads **once**, as a raw +1
//!   pointer, into [`VdHandle`]. From then on it is single-owner state of
//!   the backend, used on the worker thread only. Plain `objc_release` is
//!   thread-safe (atomic refcount; no autorelease pool is needed for a
//!   plain release — the same autorelease-pool invariant documented in
//!   `platform/macos.rs`), so the Drop on the worker thread is sound; the
//!   full argument lives in the `VdHandle` SAFETY comment.
//! - Every Obj-C/CF call below returns +1/CF-typed objects or plain
//!   primitives (no autoreleased returns), mirroring the invariant
//!   documented in `platform/macos.rs` for the pool-less worker thread.
//!
//! Coordinate contract (types.rs): input space = CGEvent global **points**.
//! The virtual display lives at `CGDisplayBounds(display_id)` in that same
//! space, so `Capture.origin` is that bounds origin, and `input_scale` is
//! derived backwards from the returned image (points / actual pixel width,
//! exactly like the physical macOS backend — v1 runs non-hiDPI so the
//! scale is 1.0, but the derivation never assumes it).
//!
//! Input fidelity: mouse events target the app owning the topmost on-screen
//! normal-level window under the point whose pid the session launched or
//! whose window sits fully inside the display bounds (hit-tested via
//! `CGWindowListCopyWindowInfo` — an ownership-scoped geometry test: a user
//! window merely overhanging the parked bounds is never targeted); keyboard
//! events target the last-interacted pid (`open` and successful clicks set
//! it; explicit failures clear it). Every input entry point preflights the
//! Accessibility TCC grant (the physical backend's required-for-correctness
//! preflight): without it input answers an explicit error instead of
//! succeeding silently while WindowServer drops the events.
//! CGEventPostToPid is public API but semantically private-adjacent: some
//! apps (e.g. WebKit content processes on recent macOS) may not receive
//! pid-posted mouse events — disclosed in `Capabilities::target_note`.

use std::ffi::{c_char, c_ulong, c_void};
use std::process::{Command, Stdio};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, sel};

use objc2_application_services::{AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFNumberType, CFRetained, CFString, CFType, CGPoint, CGRect,
    CGSize,
};
use objc2_core_graphics::{
    CGBeginDisplayConfiguration, CGCancelDisplayConfiguration, CGCompleteDisplayConfiguration,
    CGConfigureDisplayOrigin, CGConfigureOption, CGDisplayBounds, CGDisplayConfigRef,
    CGDisplayIsInMirrorSet, CGError, CGEvent, CGEventField, CGEventFlags, CGEventType,
    CGGetActiveDisplayList, CGImage, CGMainDisplayID, CGMouseButton, CGScrollEventUnit,
    CGWindowImageOption, CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID,
    kCGWindowBounds, kCGWindowLayer, kCGWindowOwnerName, kCGWindowOwnerPID,
};
// The chosen capture path (see `capture_display_image` for the pick-one
// justification): obsoleted in the macOS 15 SDK, so the deprecation lint is
// silenced at the import and the call.
#[allow(deprecated)]
use objc2_core_graphics::CGWindowListCreateImage;
use objc2_foundation::NSString;

use super::super::super::backend::ComputerUseBackend;
use super::super::super::types::{
    Capabilities, Capture, ComputerUseError, DisplayTarget, ElementInfo, Key, MouseButton,
    ScrollDirection, UiTreeOptions,
};
use super::super::helpers::{
    CHORD_HOLD_MS, CLICK_SETTLE_MS, DRAG_PRESS_SETTLE_MS, DRAG_STEP_DELAY_MS, DRAG_STEPS,
    HOLD_CANCEL_POLL_MS, MACOS_UNICODE_STRING_UTF16_UNITS, MAX_SCROLL_CLICKS,
    MULTI_CLICK_INTERVAL_MS, TYPE_CHUNK_CHARS, TypeRun, combine_drag_errors, combine_hold_errors,
    drag_waypoints, normalize_typed_newlines, ordered_chord_keys, split_type_runs, utf16_chunks,
};
// TCC preflights and their user-facing error strings are shared with the
// physical macOS backend (same grants, same System Settings guidance — a
// duplicated copy would drift; the extraction there is limited to widening
// visibility, no behavior change).
use super::super::macos::{
    accessibility_error, accessibility_granted, screen_recording_error, screen_recording_granted,
};
// CFRelease is shared via platform::cursor, the same FFI-sharing precedent
// platform/macos.rs documents for the AX "Copy rule" guards.
use super::VirtualAvailability;
use crate::platform::cursor::CFRelease;
use crate::platform::process::std_process_group_leader;

// ---------------------------------------------------------------------------
// Identity + geometry parameters (pure, host-pinned by tests)
// ---------------------------------------------------------------------------

/// Virtual display backing-pixel geometry. Matches the Linux Xvfb backend
/// (1280x800) so the model sees a consistent default workspace across
/// virtual targets.
const VD_WIDTH_PX: u32 = 1280;
const VD_HEIGHT_PX: u32 = 800;
const VD_REFRESH_HZ: f64 = 60.0;

/// Physical size in millimeters. Pinned ≥ 200 PPI: macOS sizes its virtual
/// desktop UI (menu bar, dock) from the physical size — a desk-sized
/// millimeter value makes every window tiny. 160.0×100.0 mm for 1280×800 px
/// gives ≈203 PPI and keeps the 16:10 aspect ratio of the pixels.
const VD_SIZE_MM: CGSize = CGSize {
    width: 160.0,
    height: 100.0,
};

/// Product identity reported to the OS (vendor/product/serial metadata).
const PRODUCT_NAME: &str = "Pinvou Virtual Display";
const SERIAL_STRING_PREFIX: &str = "PINVOU3-VD-";

/// Serial dispatch-queue label for the display's event queue. Must NOT be
/// the main queue (the private display machinery enqueues work there;
/// starving the main run loop would stall the whole app).
const QUEUE_LABEL: &std::ffi::CStr = c"dev.pinvou.agent.computer-use.vd";

/// FNV-1a 32-bit (the standard 0x811c9dc5 / 0x01000193 parameters) — used to
/// derive a stable u32 serial number from the product name so the display
/// reports consistent identity metadata across sessions.
fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// The u32 serial number handed to the display descriptor: derived from the
/// product identity, stable across sessions.
fn serial_number() -> u32 {
    fnv1a32(PRODUCT_NAME.as_bytes())
}

/// The serial string handed to the display descriptor: the stable prefix
/// plus the hex of the derived serial number (human-readable in system
/// reports about displays).
fn serial_string() -> String {
    format!("{SERIAL_STRING_PREFIX}{:08X}", serial_number())
}

/// Effective PPI of the virtual panel: pixels over physical inches
/// (25.4 mm per inch). Pure so the "≥ 200 PPI so the desktop UI stays
/// normally sized" decision is pinned by a test. Test-only: without the
/// cfg gate the non-test build carries it as dead code, which the hard
/// `-D warnings` gate would reject on macOS (the Linux gate never sees
/// this file).
#[cfg(test)]
fn effective_ppi(width_px: u32, mm_width: f64) -> f64 {
    if mm_width <= 0.0 {
        return 0.0;
    }
    f64::from(width_px) / (mm_width / 25.4)
}

// ---------------------------------------------------------------------------
// Private class + selector surface (probe what you send)
// ---------------------------------------------------------------------------

/// The private Obj-C class family (Apple CGVirtualDisplay private API).
/// Pinned by name in the tests; a future macOS renaming any of these makes
/// [`probe`] answer Unavailable and the session falls back to the physical
/// desktop — never a crash.
const CLASS_VIRTUAL_DISPLAY: &std::ffi::CStr = c"CGVirtualDisplay";
const CLASS_DESCRIPTOR: &std::ffi::CStr = c"CGVirtualDisplayDescriptor";
const CLASS_SETTINGS: &std::ffi::CStr = c"CGVirtualDisplaySettings";
const CLASS_MODE: &std::ffi::CStr = c"CGVirtualDisplayMode";

/// Every selector this module sends, with the class it is sent to. The probe
/// and the create path both walk this table — one table, so a future edit
/// that adds a `msg_send!` must extend the probe in the same commit (the
/// probe iterates exactly what creation sends). `init`/`alloc` are absent:
/// every Obj-C class has them.
fn required_selectors() -> Vec<(
    &'static std::ffi::CStr,
    &'static AnyClass,
    objc2::runtime::Sel,
)> {
    let classes = [
        (CLASS_VIRTUAL_DISPLAY, c"CGVirtualDisplay"),
        (CLASS_DESCRIPTOR, c"CGVirtualDisplayDescriptor"),
        (CLASS_SETTINGS, c"CGVirtualDisplaySettings"),
        (CLASS_MODE, c"CGVirtualDisplayMode"),
    ];
    let mut table: Vec<(
        &'static std::ffi::CStr,
        &'static AnyClass,
        objc2::runtime::Sel,
    )> = Vec::new();
    for (name, class) in classes {
        if let Some(class) = AnyClass::get(class) {
            let selectors: &[objc2::runtime::Sel] = match name {
                n if n == CLASS_DESCRIPTOR => &[
                    sel!(setMaxPixelsWide:),
                    sel!(setMaxPixelsHigh:),
                    sel!(setSizeInMillimeters:),
                    sel!(setSerialString:),
                    sel!(setProductName:),
                    sel!(setQueue:),
                    sel!(setModes:),
                ],
                n if n == CLASS_VIRTUAL_DISPLAY => {
                    &[
                        sel!(initWithDescriptor:),
                        sel!(applySettings:),
                        // The activation poll reads this property getter
                        // (wait_for_activation) — probed like everything else
                        // this module sends.
                        sel!(displayID),
                    ]
                }
                n if n == CLASS_SETTINGS => &[sel!(setModes:)],
                _ => &[sel!(initWithWidth:height:refreshRate:)],
            };
            for selector in selectors {
                table.push((name, class, *selector));
            }
        }
    }
    table
}

/// Cheap availability probe: private class + selector presence only. No side
/// effects, no display creation (it runs at the refresh points — startup
/// replay, preference flips, creation reporting; status reads consume the
/// cached snapshot). Unavailable carries the wording the status surface
/// shows the user.
pub(crate) fn probe() -> VirtualAvailability {
    for name in [
        CLASS_VIRTUAL_DISPLAY,
        CLASS_DESCRIPTOR,
        CLASS_SETTINGS,
        CLASS_MODE,
    ] {
        if AnyClass::get(name).is_none() {
            return VirtualAvailability::Unavailable {
                detail: "private macOS virtual display API not present on this macOS".to_string(),
            };
        }
    }
    for (class_name, _class, selector) in required_selectors() {
        // Unwrapping the class here is safe: probe only reaches the table
        // after every class resolved above.
        let Some(class) = AnyClass::get(class_name) else {
            continue;
        };
        if !class.responds_to(selector) {
            return VirtualAvailability::Unavailable {
                detail: format!(
                    "private macOS virtual display API is missing the selector \
                     {selector:?} on {class_name:?} on this macOS"
                ),
            };
        }
    }
    VirtualAvailability::Available
}

// ---------------------------------------------------------------------------
// Creation: descriptor → display → settings → activation poll
// ---------------------------------------------------------------------------

/// Activation poll ceiling for the new display to appear in the active
/// display list (design §7: ≤ 5 s; `CGGetActiveDisplayList` both refreshes
/// the per-process display list AND reports membership — this call is the
/// required refresh step, not just a query).
const ACTIVATION_POLL_TIMEOUT: Duration = Duration::from_secs(5);
/// Poll cadence for display activation and pid discovery.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Channel wait for the main-thread dispatch (covers the ≤5 s in-closure
/// poll plus queue hop latency; the total worst case stays under the
/// design's 8 s budget).
const MAIN_DISPATCH_WAIT: Duration = Duration::from_secs(7);
/// Main-thread grace for the post-build display-id read: normally the id is
/// assigned within a poll tick or two of `applySettings`, so the closure
/// typically returns in milliseconds and the main run loop pumps while the
/// worker runs the budgeted activation wait. A display that never reports
/// an id within this grace falls back to the legacy whole sequence on the
/// main thread (behavior preserved, freeze no worse than the pre-split
/// code) instead of failing a creation the API might still complete.
const DISPLAY_ID_GRACE: Duration = Duration::from_secs(1);

// libdispatch has no binding in the macOS dependency set this crate may use
// (dispatch2 is only a transitive dependency of objc2-core-graphics and not
// directly declared, and this change must not add Cargo.toml entries), so
// the single symbol is declared here, mirroring the hand-rolled extern
// style of `platform/macos.rs`. NULL attributes = a plain serial queue,
// which is exactly what the private display machinery wants (and NOT the
// main queue — see [`QUEUE_LABEL`]).
#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut c_void;
}

// CoreFoundation type checks. Declared here because objc2-core-foundation
// keeps `type_id` behind a crate-private trait (the same reason
// `platform/macos.rs` declares the identical symbols for its AX guards).
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFGetTypeID(cf: *const c_void) -> usize;
    fn CFArrayGetTypeID() -> usize;
}

/// A `msg_send!`'d `init` result, forced non-nil (the private classes return
/// nil only on allocation failure, which maps to an explicit error — never a
/// null deref).
unsafe fn init_or_err(
    allocated: Allocated<AnyObject>,
    detail: &'static str,
) -> Result<Retained<AnyObject>, ComputerUseError> {
    // SAFETY: sending an init-family selector to an Allocated receiver is
    // the canonical ownership transfer in objc2 0.6 (Allocated → +1
    // Retained); the selector itself was verified by the probe.
    let initialized: Option<Retained<AnyObject>> = unsafe { msg_send![allocated, init] };
    initialized.ok_or_else(|| ComputerUseError::unavailable(detail))
}

/// Builds the descriptor: physical backing pixels, the ≥200 PPI millimeter
/// size, vendor/product/serial identity, the serial dispatch queue, and an
/// empty mode list (the mode arrives via applySettings below — the proven
/// DeskPad sequence; `serialNum` stays at its default, only `serialString`
/// carries identity, exactly like DeskPad).
unsafe fn build_descriptor() -> Result<Retained<AnyObject>, ComputerUseError> {
    let Some(class) = AnyClass::get(CLASS_DESCRIPTOR) else {
        return Err(ComputerUseError::unavailable(
            "private macOS virtual display API not present on this macOS",
        ));
    };
    // SAFETY: alloc is sent to the class obtained from the Obj-C runtime;
    // the Allocated result is consumed by init_or_err immediately.
    let allocated: Allocated<AnyObject> = unsafe { msg_send![class, alloc] };
    // SAFETY: the receiver is an Allocated descriptor freshly built above;
    // init_or_err itself only sends the probed `init` selector.
    let descriptor =
        unsafe { init_or_err(allocated, "CGVirtualDisplayDescriptor alloc/init failed")? };

    // SAFETY: each setter below is probed (responds_to) on this exact class
    // before anything is sent; arguments are encoded primitives, CGSize, or
    // Obj-C objects.
    unsafe {
        let _: () = msg_send![&*descriptor, setMaxPixelsWide: VD_WIDTH_PX];
        let _: () = msg_send![&*descriptor, setMaxPixelsHigh: VD_HEIGHT_PX];
        let _: () = msg_send![&*descriptor, setSizeInMillimeters: VD_SIZE_MM];
        let serial = NSString::from_str(&serial_string());
        let _: () = msg_send![&*descriptor, setSerialString: &*serial];
        let product = NSString::from_str(PRODUCT_NAME);
        let _: () = msg_send![&*descriptor, setProductName: &*product];

        // Serial dispatch queue (label above). The +1 returned by
        // dispatch_queue_create is deliberately never released: the
        // descriptor retains its own reference, and the display's async
        // teardown may still touch the queue after our Drop — a bounded
        // one-queue-per-session leak (a few hundred bytes, reclaimed at
        // process death) is strictly safer than a potential over-release.
        // There is no pidfile/orphan machinery for the same reason (process
        // death reclaims the display).
        let queue = dispatch_queue_create(QUEUE_LABEL.as_ptr(), std::ptr::null());
        if queue.is_null() {
            return Err(ComputerUseError::unavailable(
                "cannot create the dispatch queue for the virtual display",
            ));
        }
        let _: () = msg_send![&*descriptor, setQueue: queue];

        // Empty mode list: the actual mode is applied via
        // CGVirtualDisplaySettings below.
        let modes: Retained<objc2_foundation::NSArray<AnyObject>> =
            objc2_foundation::NSArray::from_slice(&[]);
        let _: () = msg_send![&*descriptor, setModes: &*modes];
    }
    Ok(descriptor)
}

/// Builds the mode `1280x800 @ 60.0 Hz`. hiDPI stays 0 (v1 runs non-hiDPI:
/// the capture is 1280x800 physical px and the input scale derives to 1.0 —
/// but the derivation code does not assume it).
unsafe fn build_mode() -> Result<Retained<AnyObject>, ComputerUseError> {
    let Some(class) = AnyClass::get(CLASS_MODE) else {
        return Err(ComputerUseError::unavailable(
            "private macOS virtual display API not present on this macOS",
        ));
    };
    // SAFETY: probed selector; Allocated → init family ownership transfer
    // with three encoded primitive arguments (u32, u32, f64).
    let allocated: Allocated<AnyObject> = unsafe { msg_send![class, alloc] };
    // SAFETY: probed selector; the init consumes the allocated object and
    // answers +1 or null (the Option models the null).
    let mode: Option<Retained<AnyObject>> = unsafe {
        msg_send![
            allocated,
            initWithWidth: VD_WIDTH_PX,
            height: VD_HEIGHT_PX,
            refreshRate: VD_REFRESH_HZ
        ]
    };
    mode.ok_or_else(|| ComputerUseError::unavailable("CGVirtualDisplayMode init failed"))
}

/// Creates the display object from the descriptor (+1).
unsafe fn build_display(descriptor: &AnyObject) -> Result<Retained<AnyObject>, ComputerUseError> {
    let Some(class) = AnyClass::get(CLASS_VIRTUAL_DISPLAY) else {
        return Err(ComputerUseError::unavailable(
            "private macOS virtual display API not present on this macOS",
        ));
    };
    // SAFETY: probed selector; Allocated → init family ownership transfer.
    let allocated: Allocated<AnyObject> = unsafe { msg_send![class, alloc] };
    // SAFETY: probed selector; the init consumes the allocated object and
    // answers +1 or null (the Option models the null).
    let display: Option<Retained<AnyObject>> =
        unsafe { msg_send![allocated, initWithDescriptor: descriptor] };
    display.ok_or_else(|| {
        ComputerUseError::unavailable("CGVirtualDisplay rejected the display descriptor")
    })
}

/// Applies the mode via CGVirtualDisplaySettings (setModes + applySettings
/// → BOOL). A `false` return from the private API means the settings were
/// rejected (mode unsupported) — an explicit Unavailable so the one-time
/// dispatch can honestly fall back to the physical backend.
unsafe fn apply_mode(display: &AnyObject, mode: &AnyObject) -> Result<(), ComputerUseError> {
    let Some(class) = AnyClass::get(CLASS_SETTINGS) else {
        return Err(ComputerUseError::unavailable(
            "private macOS virtual display API not present on this macOS",
        ));
    };
    // SAFETY: alloc/init as above.
    let allocated: Allocated<AnyObject> = unsafe { msg_send![class, alloc] };
    // SAFETY: the receiver is an Allocated settings object freshly built
    // above; init_or_err itself only sends the probed `init` selector.
    let settings = unsafe { init_or_err(allocated, "CGVirtualDisplaySettings alloc/init failed")? };
    // SAFETY: setModes:/applySettings: are probed; the slice builds an
    // NSArray that retains the mode for its lifetime.
    unsafe {
        let modes: Retained<objc2_foundation::NSArray<AnyObject>> =
            objc2_foundation::NSArray::from_slice(&[mode]);
        let _: () = msg_send![&*settings, setModes: &*modes];
        let applied: bool = msg_send![display, applySettings: &*settings];
        if !applied {
            return Err(ComputerUseError::unavailable(
                "the virtual display rejected the display mode settings",
            ));
        }
    }
    Ok(())
}

/// Whether `display_id` is in the process's active display list. The
/// `CGGetActiveDisplayList` call is REQUIRED here per design §7: it is the
/// step that refreshes the per-process display list after a new display is
/// attached (a fresh display can be invisible to `CGDisplayBounds` until a
/// list call has run).
fn refresh_and_check_active(display_id: u32) -> bool {
    if display_id == 0 {
        return false;
    }
    // A session has a handful of displays; 16 is far above any real layout.
    let mut displays = [0u32; 16];
    let mut count: u32 = 0;
    // SAFETY: both pointers are valid out buffers of the declared lengths;
    // CGGetActiveDisplayList writes at most `max_displays` entries.
    let error =
        unsafe { CGGetActiveDisplayList(displays.len() as u32, displays.as_mut_ptr(), &mut count) };
    if error != CGError::Success {
        return false;
    }
    let count = usize::try_from(count).unwrap_or(0).min(displays.len());
    displays[..count].contains(&display_id)
}

/// Polls until the display reports a real id, appears in the refreshed
/// active display list, and reports non-empty bounds. Bounded; on timeout
/// returns explicit Unavailable (the creation-time fallback).
unsafe fn wait_for_activation(
    display: &AnyObject,
    deadline: Instant,
) -> Result<u32, ComputerUseError> {
    loop {
        // SAFETY: displayID is in required_selectors() (probe() verified it
        // with responds_to on CGVirtualDisplay before creation) and returns a
        // plain u32.
        let display_id: u32 = unsafe { msg_send![display, displayID] };
        if refresh_and_check_active(display_id) {
            let bounds = CGDisplayBounds(display_id);
            if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
                return Ok(display_id);
            }
        }
        if Instant::now() >= deadline {
            return Err(ComputerUseError::unavailable(
                "the virtual display did not become active within the creation budget",
            ));
        }
        sleep(POLL_INTERVAL);
    }
}

/// Everything the creation thread hands to the worker thread: the owned
/// display (wrapped in the Send handle below — a raw `*mut AnyObject` is
/// `!Send`, so the wrapper is what makes the channel payload Send) and its
/// id.
type CreatedDisplay = Result<(VdHandle, u32, bool), ComputerUseError>;

/// The creation sequence up to a usable display id (descriptor →
/// applySettings → bounded id read), running on whichever thread the caller
/// chose. The trailing `bool` answers "activation already polled ON THIS
/// thread": `false` means the caller must run the budgeted activation wait
/// (the worker-side [`wait_activation_on_worker`]), `true` means the legacy
/// in-closure poll already completed (the id-grace fallback path). Ends by
/// handing the +1 display into a [`VdHandle`] so it can cross the channel;
/// on error the locals (including the half-built display) drop on the
/// creating thread — the main thread when dispatched (it created them and
/// has a live run loop), the worker in the fallback.
fn create_and_read_id(grace_deadline: Instant, activation_deadline: Instant) -> CreatedDisplay {
    // SAFETY: the whole sequence is guarded by probe(); each call site
    // documents its selector. Any failure drops the locals on this thread
    // and returns an explicit error.
    unsafe {
        let descriptor = build_descriptor()?;
        let mode = build_mode()?;
        let display = build_display(&descriptor)?;
        apply_mode(&display, &mode)?;
        // Bounded id read: displayID is in required_selectors() (probe()
        // verified it with responds_to on CGVirtualDisplay before creation).
        // A fresh display usually reports its id within a poll tick or two
        // of applySettings; when it does, the closure returns immediately
        // and the main run loop pumps during the worker's activation wait.
        let mut display_id: u32 = msg_send![&*display, displayID];
        while display_id == 0 && Instant::now() < grace_deadline {
            sleep(POLL_INTERVAL);
            display_id = msg_send![&*display, displayID];
        }
        if display_id != 0 {
            // Ownership moves out of `display` (into_raw = +1 handed to the
            // handle, which releases exactly once on Drop); descriptor/mode/
            // settings drop here on this thread.
            let handle = VdHandle::from_raw(Retained::into_raw(display))
                .ok_or_else(|| ComputerUseError::unavailable("virtual display object was lost"))?;
            return Ok((handle, display_id, false));
        }
        // The API did not assign an id within the grace: run the legacy
        // whole activation poll HERE (this thread) — the same behavior as
        // the pre-split code, so a machinery that needs the main thread
        // keeps working and the freeze is never worse than before.
        let display_id = wait_for_activation(&display, activation_deadline)?;
        let handle = VdHandle::from_raw(Retained::into_raw(display))
            .ok_or_else(|| ComputerUseError::unavailable("virtual display object was lost"))?;
        Ok((handle, display_id, true))
    }
}

/// Worker-side activation wait: the same acceptance as
/// [`wait_for_activation`]'s refresh step (in the refreshed active display
/// list with non-empty bounds), but through GLOBAL CG APIs only — the
/// display object has crossed threads by now and the `VdHandle` contract
/// forbids messaging it again, so the poll never touches the object.
fn wait_activation_on_worker(display_id: u32, deadline: Instant) -> Result<(), ComputerUseError> {
    loop {
        if refresh_and_check_active(display_id) {
            let bounds = CGDisplayBounds(display_id);
            if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(ComputerUseError::unavailable(
                "the virtual display did not become active within the creation budget",
            ));
        }
        sleep(POLL_INTERVAL);
    }
}

/// Bounds of every active display except `except`, as (id, bounds) pairs.
/// Empty when the list cannot be read (callers fall back to main-only).
fn other_display_bounds(except: u32) -> Vec<(u32, CGRect)> {
    // A session has a handful of displays; 16 is far above any real layout.
    let mut displays = [0u32; 16];
    let mut count: u32 = 0;
    // SAFETY: both pointers are valid out buffers of the declared lengths;
    // CGGetActiveDisplayList writes at most `max_displays` entries.
    let error =
        unsafe { CGGetActiveDisplayList(displays.len() as u32, displays.as_mut_ptr(), &mut count) };
    if error != CGError::Success {
        return Vec::new();
    }
    let count = usize::try_from(count).unwrap_or(0).min(displays.len());
    displays[..count]
        .iter()
        .filter(|&&id| id != except)
        .map(|&id| (id, CGDisplayBounds(id)))
        .collect()
}

/// Strict rectangle intersection: shared edges do NOT count (a display
/// parked flush against another's right edge is adjacent, not overlapping).
fn rects_intersect(a: CGRect, b: CGRect) -> bool {
    a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}

/// The right edge of the union of every active display except `except`.
/// Parking beyond THIS edge — not merely beyond the main display's — is
/// what keeps the virtual display clear of the user's whole layout: in the
/// default macOS arrangement an external display sits exactly at
/// (main_width, 0), so parking at the main display's right edge would drop
/// the virtual display exactly on top of it.
fn union_right_edge(except: u32) -> f64 {
    let others = other_display_bounds(except);
    if others.is_empty() {
        // No readable list: fall back to the main display alone.
        let main_bounds = CGDisplayBounds(CGMainDisplayID());
        return main_bounds.origin.x + main_bounds.size.width;
    }
    others
        .iter()
        .map(|(_, bounds)| bounds.origin.x + bounds.size.width)
        .fold(f64::MIN, f64::max)
}

/// Parks the virtual display right of the union of ALL active displays with
/// a SESSION-scope display configuration (NEVER `kCGConfigurePermanently` —
/// the user's real layout must survive process death untouched; session
/// scope evaporates with it). The main display is pinned at (0,0) and the
/// virtual display right of every other display's right edge, and the
/// applied layout is VERIFIED overlap-free before returning — the central
/// "the virtual bounds never overlap the user's desktop" invariant is
/// enforced, not assumed: any residual overlap is an explicit error FROM
/// THIS FUNCTION. At creation time the caller logs it as a warning and
/// continues (the display is fully functional wherever it sits, and
/// `ensure_parked` re-checks on every capture — see the call-site
/// comment), so a creation does not fail over a placement the next capture
/// would repair; the hit-test's containment scope (see `pid_at_point`)
/// keeps user windows outside the parked bounds targeted regardless.
fn park_right_of_main(display_id: u32) -> Result<(), ComputerUseError> {
    let main = CGMainDisplayID();
    let mut config: CGDisplayConfigRef = std::ptr::null_mut();
    // SAFETY: config is a valid out pointer on the stack; on Success the
    // configuration is completed or cancelled exactly once (both paths
    // below), on failure there is nothing to release.
    let begin = unsafe { CGBeginDisplayConfiguration(&mut config) };
    if begin != CGError::Success {
        return Err(ComputerUseError::failed(format!(
            "CGBeginDisplayConfiguration failed: CGError({})",
            begin.0
        )));
    }
    let park_x = union_right_edge(display_id).ceil() as i32;
    // SAFETY: config is the live configuration from the successful begin;
    // both ids are real display ids.
    let main_result = unsafe { CGConfigureDisplayOrigin(config, main, 0, 0) };
    // SAFETY: same live configuration; display_id is a real display id and
    // park_x was rounded from the other displays' real bounds.
    let vd_result = unsafe { CGConfigureDisplayOrigin(config, display_id, park_x, 0) };
    if main_result != CGError::Success || vd_result != CGError::Success {
        // SAFETY: abandoning the configuration cancels it exactly once.
        let _cancel = unsafe { CGCancelDisplayConfiguration(config) };
        return Err(ComputerUseError::failed(format!(
            "CGConfigureDisplayOrigin failed (main: CGError({}), virtual: CGError({}))",
            main_result.0, vd_result.0
        )));
    }
    // SAFETY: config is the live configuration; ForSession keeps the change
    // out of the user's permanent display layout.
    let complete = unsafe { CGCompleteDisplayConfiguration(config, CGConfigureOption::ForSession) };
    if complete != CGError::Success {
        return Err(ComputerUseError::failed(format!(
            "CGCompleteDisplayConfiguration failed: CGError({})",
            complete.0
        )));
    }
    // Enforce the no-overlap invariant on the APPLIED layout: CG may clamp
    // or adjust origins, so trust only the post-configuration bounds.
    let applied = CGDisplayBounds(display_id);
    if let Some((id, bounds)) = other_display_bounds(display_id)
        .into_iter()
        .find(|(_, bounds)| rects_intersect(applied, *bounds))
    {
        return Err(ComputerUseError::unavailable(format!(
            "the virtual display still overlaps display {id} after parking \
             (virtual {:?}, other {:?}); refusing an overlapping placement",
            (
                applied.origin.x,
                applied.origin.y,
                applied.size.width,
                applied.size.height
            ),
            (
                bounds.origin.x,
                bounds.origin.y,
                bounds.size.width,
                bounds.size.height
            )
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The owned display handle (Send)
// ---------------------------------------------------------------------------

/// Single-owner handle to the live CGVirtualDisplay object. Doubles as the
/// channel payload of [`CreatedDisplay`]: the wrapper (not a raw pointer) is
/// what makes the hand-off Send.
///
/// SAFETY (Send): `Retained<AnyObject>` is `!Send` by policy (autorelease
/// pool / thread-affinity analysis cannot see through `AnyObject`), but
/// THIS object is specifically managed:
/// - created exactly once on the main thread (or the worker thread in the
///   no-main-dispatch fallback) by [`create_and_read_id`], which hands the
///   +1 across the bounded channel INSIDE this wrapper — a single, one-way
///   ownership transfer, never shared;
/// - owned exclusively by one `MacosVirtualDisplayBackend` on the
///   computer_use worker thread afterwards; the object is never messaged
///   again after creation (write-only lifecycle state), so the only
///   cross-thread operation is its Drop;
/// - dropped (released via the inner `Retained`'s `objc_release`) on
///   whichever thread loses the last reference — the worker on the normal
///   backend-Drop path; the worker when a timed-out `recv_timeout` drops a
///   still-queued value; the CREATING thread (main, or the worker in the
///   no-main-dispatch fallback) when the producer's `send` fails after the
///   receiver side is gone. All three are plain `objc_release` drops —
///   thread-safe for a plain release (atomic refcount; no autorelease pool
///   is required for `objc_release` itself, matching the invariant
///   documented in `platform/macos.rs`). The display then disappears
///   asynchronously; we never force-wait for it (design §7), and no
///   pidfile/orphan machinery is needed because process death reclaims the
///   display.
struct VdHandle(Retained<AnyObject>);

// SAFETY: the wrapped CGVirtualDisplay object may legally move between
// threads (plain objc_release has an atomic refcount and needs no pool —
// see the struct docs); the handle is messaged nowhere after creation and
// dropped on the worker thread, so Send only covers the one-way hand-off
// and the drop.
unsafe impl Send for VdHandle {}

impl VdHandle {
    /// Takes ownership of a +1 raw pointer produced by [`create_and_read_id`].
    /// Returns None on null (which the creation path never produces — it
    /// only hands out non-null pointers from `Retained::into_raw`).
    fn from_raw(ptr: *mut AnyObject) -> Option<Self> {
        // SAFETY: ptr is the +1 raw pointer from Retained::into_raw; the
        // +1 moves back into this Retained (released exactly once, on Drop).
        unsafe { Retained::from_raw(ptr) }.map(Self)
    }
}

impl Drop for VdHandle {
    fn drop(&mut self) {
        log::info!(
            "[computer_use:macos-vd] releasing the virtual display (it disappears \
             asynchronously; opened apps are not terminated — they keep running \
             and their windows relocate to remaining displays)"
        );
        // The inner Retained's Drop runs objc_release here; nothing further.
    }
}

// ---------------------------------------------------------------------------
// Window hit-testing (CGWindowListCopyWindowInfo)
// ---------------------------------------------------------------------------

/// Read one `CFNumber` value as f64 out of a window-info dictionary.
/// `CFNumberGetValue` converts between the stored and requested types, so
/// pid entries (stored as ints) and coordinate entries (stored as doubles)
/// both read through f64; integer callers round explicitly.
fn dictionary_number(dict: &CFDictionary, key: &CFString) -> Option<f64> {
    // SAFETY: key is a valid CFString pointer (toll-free bridged static or
    // NSString); the returned value pointer is owned by the dictionary and
    // checked non-null before any use.
    let raw = unsafe { dict.value(std::ptr::from_ref(key).cast::<c_void>()) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: raw points at a live CFNumber owned by `dict` (the kCGWindow*
    // values are all CFNumbers); `value()` only writes the out buffer.
    let number = unsafe { &*raw.cast::<CFNumber>() };
    let mut out = 0.0f64;
    // SAFETY: out is a valid f64 out buffer; CFNumberGetValue converts any
    // CFNumber to kCFNumberFloat64Type.
    let ok = unsafe {
        number.value(
            CFNumberType::Float64Type,
            NonNull::from(&mut out).cast().as_ptr(),
        )
    };
    ok.then_some(out)
}

/// Read one `CFString` value out of a window-info dictionary (the owner-name
/// entry; windows missing it yield None — name matching degrades, never
/// fails).
fn dictionary_string(dict: &CFDictionary, key: &CFString) -> Option<String> {
    // SAFETY: key is a valid CFString pointer (toll-free bridged static or
    // NSString); the returned value pointer is owned by the dictionary and
    // checked non-null before any use.
    let raw = unsafe { dict.value(std::ptr::from_ref(key).cast::<c_void>()) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: raw points at a live CFString owned by `dict` (the
    // kCGWindowOwnerName entry is a CFString), borrowed only for this copy.
    let string = unsafe { &*raw.cast::<CFString>() };
    Some(string.to_string())
}

/// One on-screen window's geometry and owner (all in CGEvent global points).
#[derive(Debug, Clone, PartialEq)]
struct HitWindow {
    pid: i32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    /// Layer 0 = normal application windows (desktop, dock, menu-bar, and
    /// overlay layers are excluded from targeting).
    normal_level: bool,
    /// Owner application name (`kCGWindowOwnerName`; None when the entry is
    /// missing). Used only by pid discovery's name preference — hit-testing
    /// stays geometry-only.
    owner_name: Option<String>,
}

impl HitWindow {
    fn contains(&self, x: i32, y: i32) -> bool {
        rect_contains(self.x, self.y, self.width, self.height, x, y)
    }

    fn intersects(&self, bounds: CGRect) -> bool {
        rect_intersects(
            self.x,
            self.y,
            self.width,
            self.height,
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
        )
    }

    /// Fully inside the display bounds (every edge within): the shape of a
    /// session app's window — and decisively NOT the shape of a USER window
    /// overhanging from their desktop, which merely INTERSECTS the parked
    /// display's bounds with a sliver.
    fn contained_in(&self, bounds: CGRect) -> bool {
        rect_contained_in(
            self.x,
            self.y,
            self.width,
            self.height,
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
        )
    }
}

/// Full containment (every edge of the inner rect within the outer one).
/// Touching edges count as inside (a window exactly filling the display is
/// contained).
fn rect_contained_in(
    ax: f64,
    ay: f64,
    aw: f64,
    ah: f64,
    bx: f64,
    by: f64,
    bw: f64,
    bh: f64,
) -> bool {
    ax >= bx && ay >= by && ax + aw <= bx + bw && ay + ah <= by + bh
}

/// Point-in-rect with half-open right/bottom edges (screen points).
fn rect_contains(x: f64, y: f64, width: f64, height: f64, px: i32, py: i32) -> bool {
    (px as f64) >= x && (py as f64) >= y && (px as f64) < x + width && (py as f64) < y + height
}

/// Axis-aligned rect intersection (touching edges do not count).
fn rect_intersects(ax: f64, ay: f64, aw: f64, ah: f64, bx: f64, by: f64, bw: f64, bh: f64) -> bool {
    ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
}

/// Reads one window-info dictionary into a [`HitWindow`]; entries missing
/// any required field are skipped (the hit-test degrades, never fails).
/// `kCGWindowBounds` is a nested dictionary (X/Y/Width/Height) — read as
/// such, NOT as a number.
fn parse_hit_window(dict: &CFDictionary) -> Option<HitWindow> {
    // SAFETY: the kCGWindow* statics are CoreFoundation-exported CFStrings
    // valid for the process lifetime; reading an extern static (a stable
    // address) is the only unsafe step — each value is then borrowed only
    // for the length of its lookup call.
    let layer = dictionary_number(dict, unsafe { kCGWindowLayer });
    // SAFETY: same extern-static read as kCGWindowLayer above.
    let pid = dictionary_number(dict, unsafe { kCGWindowOwnerPID })?;
    // SAFETY: same extern-static read as kCGWindowLayer above.
    let owner_name = dictionary_string(dict, unsafe { kCGWindowOwnerName });
    // SAFETY: same extern-static read as kCGWindowLayer above.
    let bounds_key = unsafe { kCGWindowBounds };
    // SAFETY: bounds_key is a valid CFString key; the value pointer is
    // owned by the parent dictionary and checked non-null before use.
    let bounds_raw = unsafe { dict.value(std::ptr::from_ref(bounds_key).cast::<c_void>()) };
    if bounds_raw.is_null() {
        return None;
    }
    // SAFETY: the bounds value is a live CFDictionary owned by the parent
    // dictionary, borrowed only within this function.
    let bounds_dict = unsafe { &*bounds_raw.cast::<CFDictionary>() };
    let x = dictionary_number(bounds_dict, NSString::from_str("X").as_ref())?;
    let y = dictionary_number(bounds_dict, NSString::from_str("Y").as_ref())?;
    let width = dictionary_number(bounds_dict, NSString::from_str("Width").as_ref())?;
    let height = dictionary_number(bounds_dict, NSString::from_str("Height").as_ref())?;
    Some(HitWindow {
        pid: pid as i32,
        x,
        y,
        width,
        height,
        normal_level: layer == Some(0.0),
        owner_name,
    })
}

/// Snapshots the on-screen, desktop-excluded windows front-to-back. Used
/// both for point hit-testing and for the "new pid" discovery after an
/// `open -a` launch (whose new window may initially be placed anywhere, not
/// yet inside the virtual display's bounds).
fn on_screen_windows() -> Vec<HitWindow> {
    let Some(list) = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        kCGNullWindowID,
    ) else {
        return Vec::new();
    };
    let count = usize::try_from(list.count()).unwrap_or(0);
    let mut windows = Vec::with_capacity(count);
    for index in 0..count as isize {
        // SAFETY: index < count from CFArrayGetCount on the same immutable
        // array; the element is owned by the array (kept alive by the
        // CFRetained guard) and borrowed only inside parse_hit_window.
        let raw = unsafe { list.value_at_index(index) };
        if raw.is_null() {
            continue;
        }
        // SAFETY: CGWindowListCopyWindowInfo produces a CFArray of
        // CFDictionary; the borrow below does not outlive `list`.
        let dict = unsafe { &*raw.cast::<CFDictionary>() };
        if let Some(window) = parse_hit_window(dict) {
            windows.push(window);
        }
    }
    windows
}

/// Topmost on-screen normal-level window containing the point and belonging
/// to the virtual display → owning pid. "Belonging" is ownership-scoped, not
/// bare geometry (the isolation promise is unconditional, so the hit-test
/// cannot be): the window's pid must be one the session launched WITHIN the
/// ownership grace (`session_pids` — includes transitional windows the AX
/// move has not yet relocated), or the window must sit FULLY INSIDE the
/// display bounds. A window that merely INTERSECTS the parked bounds is
/// excluded: that is the shape of a user window overhanging from the user's
/// desktop (partially dragged off-screen, or restored at a stale position
/// when the display materialized) — without this scope such a window could
/// win the hit-test and receive model input on the user's real desktop.
fn pid_at_point(
    point: (i32, i32),
    display_bounds: CGRect,
    session_pids: &[(i32, Instant)],
) -> Option<i32> {
    on_screen_windows()
        .into_iter()
        .find(|window| targetable_at(window, point, display_bounds, session_pids, Instant::now()))
        .map(|window| window.pid)
}

/// The targeting predicate behind [`pid_at_point`] (pure, unit-pinned): a
/// normal-level window containing the point is targetable when its pid is
/// one the session launched within [`SESSION_OWNERSHIP_GRACE`] (the AX-move
/// transition — the window may legitimately sit anywhere while it is being
/// relocated), or its window sits FULLY inside the display bounds. The
/// grace is what keeps a session pid from being targetable FOREVER: a
/// session app that quit frees its pid for recycling, and an exemption
/// without a deadline would let a recycled pid's window — wherever on the
/// user's desktop it sits — receive pid-posted input. `now` is injected so
/// the predicate stays pure (and pinnable without a live window list).
fn targetable_at(
    window: &HitWindow,
    point: (i32, i32),
    display_bounds: CGRect,
    session_pids: &[(i32, Instant)],
    now: Instant,
) -> bool {
    let session_owned_recently = session_pids.iter().any(|(pid, discovered)| {
        *pid == window.pid && now.duration_since(*discovered) < SESSION_OWNERSHIP_GRACE
    });
    window.normal_level
        && window.contains(point.0, point.1)
        && (session_owned_recently || window.contained_in(display_bounds))
}

/// Whether `pid` currently shows an on-screen normal-level window that the
/// OWNERSHIP rules make drivable on the virtual display — the check behind
/// the remembered-target verification (focus pid and held press pids). The
/// ownership scope mirrors the hit-test's [`targetable_at`] minus the
/// point-in-window part: a session pid within its
/// [`SESSION_OWNERSHIP_GRACE`] may keep its window anywhere intersecting
/// the display (the AX move may still be in flight), while everything else —
/// unknown pids AND session pids past the grace, whose pid the app may
/// since have RELEASED for recycling — must keep a window FULLY inside the
/// display bounds. Without the deadline the exemption would be forever: a
/// recycled pid's window, wherever on the user's desktop it sits, could
/// pass the old bare-intersection check and receive pid-posted input.
fn pid_has_window_on_display(pid: i32, display_id: u32, session_pids: &[(i32, Instant)]) -> bool {
    if pid <= 0 {
        return false;
    }
    let bounds = CGDisplayBounds(display_id);
    let now = Instant::now();
    let within_grace = session_pids.iter().any(|(recorded, discovered)| {
        *recorded == pid && now.duration_since(*discovered) < SESSION_OWNERSHIP_GRACE
    });
    on_screen_windows().iter().any(|window| {
        window.pid == pid
            && window.normal_level
            && (within_grace && window.intersects(bounds) || window.contained_in(bounds))
    })
}

/// Whether `pid` currently owns ANY on-screen normal-level window, on any
/// display. The direct-exec `open` path waits on this before its AX move:
/// a freshly spawned app maps its window onto the user's desktop first,
/// and the move is a structural no-op until that window exists.
fn pid_has_any_window(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    on_screen_windows()
        .iter()
        .any(|window| window.pid == pid && window.normal_level)
}

// ---------------------------------------------------------------------------
// Key tables (exhaustive, US-ANSI kVK virtual keycodes)
// ---------------------------------------------------------------------------

/// kVK constants (Apple HIToolbox Events.h; cross-checked against enigo's
/// vendored core-graphics 0.25 `KeyCode` table, which the physical path's
/// dependencies use). Values only — the [`virtual_keycode`] match has NO
/// wildcard arm over `Key` (design §7: parity-tested exhaustive table).
mod vk {
    pub const CONTROL: u16 = 0x3B;
    pub const OPTION: u16 = 0x3A;
    pub const SHIFT: u16 = 0x38;
    pub const COMMAND: u16 = 0x37;
    pub const RETURN: u16 = 0x24;
    pub const TAB: u16 = 0x30;
    pub const SPACE: u16 = 0x31;
    pub const DELETE: u16 = 0x33; // Backspace position
    pub const FORWARD_DELETE: u16 = 0x75;
    pub const HELP: u16 = 0x72; // Insert position (physical backend convention)
    pub const ESCAPE: u16 = 0x35;
    pub const UP_ARROW: u16 = 0x7E;
    pub const DOWN_ARROW: u16 = 0x7D;
    pub const LEFT_ARROW: u16 = 0x7B;
    pub const RIGHT_ARROW: u16 = 0x7C;
    pub const HOME: u16 = 0x73;
    pub const END: u16 = 0x77;
    pub const PAGE_UP: u16 = 0x74;
    pub const PAGE_DOWN: u16 = 0x79;
    pub const F1: u16 = 0x7A;
    pub const F2: u16 = 0x78;
    pub const F3: u16 = 0x63;
    pub const F4: u16 = 0x76;
    pub const F5: u16 = 0x60;
    pub const F6: u16 = 0x61;
    pub const F7: u16 = 0x62;
    pub const F8: u16 = 0x64;
    pub const F9: u16 = 0x65;
    pub const F10: u16 = 0x6D;
    pub const F11: u16 = 0x67;
    pub const F12: u16 = 0x6F;
}

/// US-ANSI character → (kVK keycode, needs Shift). Chord keycodes are
/// POSITIONAL US-ANSI codes; the receiving app resolves them through the
/// user's CURRENT system-wide input source (keyboard layout is system
/// state, not per-display), so on a non-US layout a letter chord can
/// trigger the key sitting at that US position — Cmd+A may act as Cmd+Q
/// on AZERTY. This misfire is DISCLOSED in `capability_notes` (prefer
/// named keys or typed text); the table stays fixed because reading the
/// live layout (Carbon UCKeyTranslate, the physical path's enigo
/// reverse-mapping) is a separate, hardware-validation-gated change.
/// Shifted characters (`+`, `A`) map to the base position plus the Shift
/// flag instead of failing closed like the physical path.
const CHAR_KEYS: &[(char, u16, bool)] = &[
    // Unshifted letters/digits/punctuation (kVK_ANSI_* positions).
    ('a', 0x00, false),
    ('s', 0x01, false),
    ('d', 0x02, false),
    ('f', 0x03, false),
    ('h', 0x04, false),
    ('g', 0x05, false),
    ('z', 0x06, false),
    ('x', 0x07, false),
    ('c', 0x08, false),
    ('v', 0x09, false),
    ('b', 0x0B, false),
    ('q', 0x0C, false),
    ('w', 0x0D, false),
    ('e', 0x0E, false),
    ('r', 0x0F, false),
    ('y', 0x10, false),
    ('t', 0x11, false),
    ('1', 0x12, false),
    ('2', 0x13, false),
    ('3', 0x14, false),
    ('4', 0x15, false),
    ('6', 0x16, false),
    ('5', 0x17, false),
    ('=', 0x18, false),
    ('9', 0x19, false),
    ('7', 0x1A, false),
    ('-', 0x1B, false),
    ('8', 0x1C, false),
    ('0', 0x1D, false),
    (']', 0x1E, false),
    ('o', 0x1F, false),
    ('u', 0x20, false),
    ('[', 0x21, false),
    ('i', 0x22, false),
    ('p', 0x23, false),
    ('l', 0x25, false),
    ('j', 0x26, false),
    ('\'', 0x27, false),
    ('k', 0x28, false),
    (';', 0x29, false),
    ('\\', 0x2A, false),
    (',', 0x2B, false),
    ('/', 0x2C, false),
    ('n', 0x2D, false),
    ('m', 0x2E, false),
    ('.', 0x2F, false),
    ('`', 0x32, false),
    (' ', vk::SPACE, false),
    // Shifted US-layout characters: base position + Shift flag.
    ('A', 0x00, true),
    ('S', 0x01, true),
    ('D', 0x02, true),
    ('F', 0x03, true),
    ('H', 0x04, true),
    ('G', 0x05, true),
    ('Z', 0x06, true),
    ('X', 0x07, true),
    ('C', 0x08, true),
    ('V', 0x09, true),
    ('B', 0x0B, true),
    ('Q', 0x0C, true),
    ('W', 0x0D, true),
    ('E', 0x0E, true),
    ('R', 0x0F, true),
    ('Y', 0x10, true),
    ('T', 0x11, true),
    ('!', 0x12, true),
    ('@', 0x13, true),
    ('#', 0x14, true),
    ('$', 0x15, true),
    ('^', 0x16, true),
    ('%', 0x17, true),
    ('+', 0x18, true),
    ('(', 0x19, true),
    ('&', 0x1A, true),
    ('_', 0x1B, true),
    ('*', 0x1C, true),
    (')', 0x1D, true),
    ('}', 0x1E, true),
    ('O', 0x1F, true),
    ('U', 0x20, true),
    ('{', 0x21, true),
    ('I', 0x22, true),
    ('P', 0x23, true),
    ('L', 0x25, true),
    ('J', 0x26, true),
    ('"', 0x27, true),
    ('K', 0x28, true),
    (':', 0x29, true),
    ('|', 0x2A, true),
    ('<', 0x2B, true),
    ('?', 0x2C, true),
    ('N', 0x2D, true),
    ('M', 0x2E, true),
    ('>', 0x2F, true),
    ('~', 0x32, true),
];

/// Exhaustive `Key` → (kVK keycode, needs Shift). No wildcard arm: every
/// variant of `types::Key` is listed, so adding a `Key` variant breaks this
/// build until it is mapped (the parity guarantee design §7 asks for).
/// Errors are explicit and never echo the offending character (audit-log
/// policy, same as the physical backend).
fn virtual_keycode(key: Key) -> Result<(u16, bool), ComputerUseError> {
    let (code, shift) = match key {
        Key::Control => (vk::CONTROL, false),
        Key::Alt => (vk::OPTION, false),
        Key::Shift => (vk::SHIFT, false),
        Key::Meta => (vk::COMMAND, false),
        Key::Enter => (vk::RETURN, false),
        Key::Escape => (vk::ESCAPE, false),
        Key::Tab => (vk::TAB, false),
        Key::Space => (vk::SPACE, false),
        Key::Backspace => (vk::DELETE, false),
        Key::Delete => (vk::FORWARD_DELETE, false),
        Key::Insert => (vk::HELP, false),
        Key::Up => (vk::UP_ARROW, false),
        Key::Down => (vk::DOWN_ARROW, false),
        Key::Left => (vk::LEFT_ARROW, false),
        Key::Right => (vk::RIGHT_ARROW, false),
        Key::Home => (vk::HOME, false),
        Key::End => (vk::END, false),
        Key::PageUp => (vk::PAGE_UP, false),
        Key::PageDown => (vk::PAGE_DOWN, false),
        Key::Function(n) => match n {
            1 => (vk::F1, false),
            2 => (vk::F2, false),
            3 => (vk::F3, false),
            4 => (vk::F4, false),
            5 => (vk::F5, false),
            6 => (vk::F6, false),
            7 => (vk::F7, false),
            8 => (vk::F8, false),
            9 => (vk::F9, false),
            10 => (vk::F10, false),
            11 => (vk::F11, false),
            12 => (vk::F12, false),
            // The parser (types.rs) already restricts f1..=f12; this arm is
            // defense in depth only.
            other => {
                return Err(ComputerUseError::failed(format!(
                    "function key f{other} is outside the supported f1..=f12 range"
                )));
            }
        },
        Key::Char(c) => match CHAR_KEYS.iter().find(|(candidate, _, _)| *candidate == c) {
            Some((_, code, shift)) => (*code, *shift),
            None => {
                return Err(ComputerUseError::failed(
                    "a character has no US-layout key position; \
                     use the type action for arbitrary text",
                ));
            }
        },
    };
    Ok((code, shift))
}

/// CGEventFlags for one modifier key (None = not a modifier).
fn modifier_flag(key: Key) -> Option<CGEventFlags> {
    match key {
        Key::Control => Some(CGEventFlags::MaskControl),
        Key::Alt => Some(CGEventFlags::MaskAlternate),
        Key::Shift => Some(CGEventFlags::MaskShift),
        Key::Meta => Some(CGEventFlags::MaskCommand),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Scroll mapping (sign parity with the physical path, enigo convention)
// ---------------------------------------------------------------------------

/// Direction + wheel clicks → (CGScrollWheel axis, signed delta). CG scroll
/// convention, as exercised by enigo's macOS backend (which the physical
/// path uses): axis 1 = vertical, axis 2 = horizontal; POSITIVE deltas
/// scroll up / left.
fn map_cg_scroll(direction: ScrollDirection, clicks: u32) -> (u32, i32) {
    let clicks = i32::try_from(clicks.min(MAX_SCROLL_CLICKS)).unwrap_or(i32::MAX);
    match direction {
        ScrollDirection::Up => (1, clicks),
        ScrollDirection::Down => (1, -clicks),
        ScrollDirection::Left => (2, clicks),
        ScrollDirection::Right => (2, -clicks),
    }
}

// ---------------------------------------------------------------------------
// Mouse event types (typed CGEventType constants)
// ---------------------------------------------------------------------------

fn cg_mouse_button(button: MouseButton) -> CGMouseButton {
    match button {
        MouseButton::Left => CGMouseButton::Left,
        MouseButton::Right => CGMouseButton::Right,
        MouseButton::Middle => CGMouseButton::Center,
    }
}

/// The (down, up, dragged) CGEventType values for a mouse button.
fn cg_mouse_event_types(button: MouseButton) -> (CGEventType, CGEventType, CGEventType) {
    match button {
        MouseButton::Left => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGEventType::LeftMouseDragged,
        ),
        MouseButton::Right => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGEventType::RightMouseDragged,
        ),
        MouseButton::Middle => (
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGEventType::OtherMouseDragged,
        ),
    }
}

/// The event type a synthetic move must use given the buttons this backend
/// itself holds pressed (apps recognize drags only via the Dragged types —
/// the same fix as the physical backend).
fn move_event_type(held: &[MouseButton]) -> (CGEventType, CGMouseButton) {
    if held.contains(&MouseButton::Left) {
        (CGEventType::LeftMouseDragged, CGMouseButton::Left)
    } else if held.contains(&MouseButton::Right) {
        (CGEventType::RightMouseDragged, CGMouseButton::Right)
    } else if held.contains(&MouseButton::Middle) {
        (CGEventType::OtherMouseDragged, CGMouseButton::Center)
    } else {
        (CGEventType::MouseMoved, CGMouseButton::Left)
    }
}

// ---------------------------------------------------------------------------
// open_app helpers
// ---------------------------------------------------------------------------

/// Launch-program classification: `.app` bundles (case-insensitive suffix)
/// go through `/usr/bin/open -a`; everything else is spawned directly as an
/// executable (no shell ever interprets the program string).
fn is_app_bundle(program: &str) -> bool {
    program.to_ascii_lowercase().ends_with(".app")
}

/// Reject empty programs and interior NULs (std Command panics on interior
/// NULs — an explicit error instead of a worker-thread panic).
fn validate_program(program: &str) -> Result<&str, ComputerUseError> {
    let program = program.trim();
    if program.is_empty() {
        return Err(ComputerUseError::failed(
            "open: program must be a non-empty executable or .app path",
        ));
    }
    if program.contains('\0') {
        return Err(ComputerUseError::failed(
            "open: program must not contain NUL bytes",
        ));
    }
    Ok(program)
}

/// The comparable stem of a launched program: its final path component with
/// a trailing `.app` removed, lowercased (e.g. `/Applications/Things.app` →
/// "things"). Input is the already-validated program string; compared only
/// against `kCGWindowOwnerName` during pid discovery, never used to launch.
fn launched_program_stem(program: &str) -> String {
    let name = program.rsplit('/').next().unwrap_or(program);
    // Lowercase first so the `.app` strip is case-insensitive ("My.APP"
    // bundles exist in the wild); the stem is only ever compared against a
    // lowercased owner name, so no original-case information is lost.
    let lowered = name.to_ascii_lowercase();
    lowered.strip_suffix(".app").unwrap_or(&lowered).to_string()
}

/// Whether a discovered window's owner name matches the launched program's
/// stem — case-insensitive containment, so "Google Chrome" matches "chrome".
/// An empty stem matches nothing (otherwise every newcomer would look
/// name-confirmed), and a missing owner name never matches.
fn owner_matches_program(owner: Option<&str>, stem: &str) -> bool {
    !stem.is_empty() && owner.is_some_and(|name| name.to_ascii_lowercase().contains(stem))
}

/// Margin between the virtual display's top-left corner and the AX-moved
/// window's new position (design §7: "VD origin + 40px margin").
const WINDOW_MOVE_MARGIN_PX: f64 = 40.0;
/// Budget for finding the pid of an app launched via `open -a` (which
/// cannot report one) by watching for its window to appear.
const OPEN_PID_DISCOVER_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the launch path waits for the short-lived `/usr/bin/open`
/// helper's own exit before treating it as "slow, keep discovering". A
/// FAILED `open` (bad app name, missing bundle) exits in well under this.
const OPEN_EXIT_STATUS_BUDGET: Duration = Duration::from_secs(3);
/// How long an unconfirmed newcomer (owner name does not match the launched
/// program, but the window IS inside the virtual display's bounds) is held
/// while discovery keeps looking for the program's own window. Bounded so
/// the overall budget never extends past [`OPEN_PID_DISCOVER_TIMEOUT`];
/// afterwards the first IN-BOUNDS newcomer wins (out-of-bounds windows —
/// the user's own desktop windows — are never claimed by the fallback;
/// residual mis-attribution among in-bounds windows is disclosed in
/// `Capabilities::notes`).
const DISCOVER_NAME_GRACE: Duration = Duration::from_millis(1000);
/// How long a recorded session pid stays drivable WHEREVER its window is
/// (the launch + AX-move transition; the direct-exec path waits for the
/// window and the AX move runs immediately, so this is generous). After
/// the grace, a session pid's windows must sit inside the display bounds
/// like every other pid's: a session app that QUIT freed its pid for
/// recycling, and a deadline-free ownership exemption would let a recycled
/// pid's window — wherever on the user's desktop it sits — receive
/// pid-posted input.
const SESSION_OWNERSHIP_GRACE: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Explicit unsupported answers (v1: no element targeting on virtual targets)
// ---------------------------------------------------------------------------

fn ui_tree_unsupported() -> ComputerUseError {
    ComputerUseError::unsupported(
        "ui_tree",
        "the virtual display target does not provide an accessibility tree; use screenshots",
    )
}

fn element_targeting_unsupported() -> ComputerUseError {
    ComputerUseError::unsupported(
        "element_at_point",
        "the virtual display target does not provide element targeting; use screenshots",
    )
}

// ---------------------------------------------------------------------------
// The backend
// ---------------------------------------------------------------------------

struct MacosVirtualDisplayBackend {
    /// The live private display object (single-owner, worker-thread only —
    /// see the `VdHandle` SAFETY comment). Never messaged after creation:
    /// lifecycle state only.
    display: VdHandle,
    /// CGDirectDisplayID of the virtual display (stable for its lifetime;
    /// bounds are re-read per capture so placement changes stay visible).
    display_id: u32,
    /// The last pid the backend interacted with (`open` + successful
    /// clicks): the keyboard target. Cleared on explicit input failures.
    focus_pid: Option<i32>,
    /// Every pid the session itself launched via `open` (both launch
    /// channels, recorded at discovery) with its discovery time: the
    /// hit-test's OWNERSHIP scope is deadline-bound — a fresh session pid
    /// is drivable wherever its window currently is (the AX move may still
    /// be in flight), while unknown pids — and session pids past the
    /// [`SESSION_OWNERSHIP_GRACE`], whose pid the app may since have
    /// RELEASED for recycling — must keep their window fully inside the
    /// display bounds to be targeted. A user window overhanging from the
    /// user's desktop must never win the hit-test.
    session_pids: Vec<(i32, Instant)>,
    /// The last synthesized point this backend moved to. The virtual
    /// display has NO user cursor (nobody's hand is on it), so
    /// `cursor_position` truthfully reports this synthesized state instead
    /// of reading a system cursor that does not apply to this target.
    cursor: Option<(i32, i32)>,
    /// Buttons this backend itself holds synthetically pressed (moves must
    /// use the Dragged event types while held — same fix as the physical
    /// backend).
    held_buttons: Vec<MouseButton>,
    /// The pid each held button was pressed against, in press order
    /// (`mouse_down` records, `mouse_up` consumes): the window that accepts
    /// a press keeps the whole gesture, so a manual
    /// down → move → up composition pins to the press pid instead of
    /// re-hit-testing its way onto a different window (the `drag` action
    /// pins the same way). Mirrors `held_buttons` (entries are pushed and
    /// removed together).
    held_pids: Vec<(MouseButton, i32)>,
    /// Cancel flag set after the caller times out (see the trait contract).
    cancel: Option<Arc<AtomicBool>>,
}

impl MacosVirtualDisplayBackend {
    /// The pid an input event should be posted to.
    ///
    /// - Mouse actions hit-test the topmost window under the point first
    ///   (the truthful target of a click at that point), falling back to the
    ///   last-interacted pid (so a click landing on desktop background still
    ///   reaches the app the session has been driving).
    /// - Keyboard/wheel actions pass no hit-test point and use the
    ///   last-interacted pid directly (keyboard focus semantics).
    ///
    /// The remembered-target fallback re-verifies the pid still shows a
    /// window the ownership rules make drivable on the virtual display
    /// ([`Self::reverified_target_pid`]): a quit app, a window the user
    /// dragged away, or a mis-attributed discovery must not receive input
    /// aimed at the virtual session (events are pid-targeted and would
    /// otherwise land wherever that app currently is — the user's desktop
    /// included). For a session pid within its grace the predicate is
    /// intersection ("touching" — a window the user dragged MOSTLY off the
    /// display still passes; the capability notes state this as "touching
    /// it", not "inside it"); past the grace the same deadline the hit-test
    /// applies kicks in and the window must sit fully inside the display
    /// bounds, so a recycled pid can never pass on a stray user window.
    fn input_pid(&self, hit_test: Option<(i32, i32)>) -> Result<i32, ComputerUseError> {
        // CGEventPostToPid needs the same Accessibility TCC grant as the
        // physical backend's CGEventPost — without it WindowServer silently
        // drops the events and EVERY input would report success while doing
        // nothing. The physical backend treats this preflight as required
        // for correctness; the virtual path shares it, at the one choke
        // point every pid-targeted input passes through (cheap TCC query,
        // per action, same cadence as the physical preflight).
        if !accessibility_granted() {
            return Err(accessibility_error());
        }
        if let Some(point) = hit_test {
            let bounds = CGDisplayBounds(self.display_id);
            if let Some(pid) = pid_at_point(point, bounds, &self.session_pids) {
                return Ok(pid);
            }
        }
        let pid = self.focus_pid.ok_or_else(|| {
            ComputerUseError::unavailable(
                "no application is available on the virtual display yet; \
                 open an app or click inside it first",
            )
        })?;
        self.reverified_target_pid(pid)
    }

    /// The ownership-scoped re-verification of a remembered target pid (the
    /// focus pid and held press pids): the pid must still show a normal-level
    /// window the ownership rules make drivable — a session pid within its
    /// grace may keep its window anywhere intersecting the display (the AX
    /// move may still be in flight), while everything else (unknown pids AND
    /// session pids past the grace) must keep a window fully inside the
    /// display bounds. See [`pid_has_window_on_display`].
    fn reverified_target_pid(&self, remembered: i32) -> Result<i32, ComputerUseError> {
        if pid_has_window_on_display(remembered, self.display_id, &self.session_pids) {
            Ok(remembered)
        } else {
            Err(ComputerUseError::unavailable(
                "the target application no longer has a window on the virtual display; \
                 click inside the display to focus a target first",
            ))
        }
    }

    /// Forget the keyboard target when an input action failed (explicit
    /// failure ⇒ the "last interacted" claim is no longer truthful —
    /// design §7). Targeting failures — including the "no application
    /// available yet" case chord routes here — and the "no longer has a
    /// window on the display" re-verification failure both clear the
    /// stale remembered target, so the next action re-resolves instead of
    /// repeating the same error.
    fn clear_focus_on_error(&mut self, error: &ComputerUseError) {
        if matches!(
            error,
            ComputerUseError::Failed { .. } | ComputerUseError::Unavailable { .. }
        ) {
            self.focus_pid = None;
        }
    }

    /// Targeting resolution with the stale-target cleanup: EVERY entry
    /// point routes its `input_pid` failure through `clear_focus_on_error`,
    /// so a stale remembered target clears instead of repeating the same
    /// error on every action until some unrelated click resets it (the
    /// contract `scroll` and `chord` already documented — the mouse entry
    /// points below used to skip it on their early `?` returns).
    fn input_pid_cleared(&mut self, hit_test: Option<(i32, i32)>) -> Result<i32, ComputerUseError> {
        match self.input_pid(hit_test) {
            Ok(pid) => Ok(pid),
            Err(error) => {
                self.clear_focus_on_error(&error);
                Err(error)
            }
        }
    }

    /// Session-scope placement guard, piggybacked on every capture (design
    /// §7): if the virtual display somehow became the main display, or got
    /// OVERLAPPED by a display the user connected or moved (both would put
    /// agent content on the user's screen or mix the user's windows into
    /// captures), re-apply the session-scope parking right of all displays —
    /// park_right_of_main re-verifies the applied layout overlap-free. A
    /// MIRROR JOIN is detected but NOT repairable by re-parking: a
    /// placement change does not unjoin an existing mirror set, so the
    /// hazard (agent content mirrored onto the user's screen) is reported
    /// loudly instead of pretended away. Bounded and non-fatal (warn only;
    /// the next capture re-checks).
    fn ensure_parked(&self) {
        let overlapped = other_display_bounds(self.display_id)
            .iter()
            .any(|(_, bounds)| rects_intersect(CGDisplayBounds(self.display_id), *bounds));
        let mirrored = CGDisplayIsInMirrorSet(self.display_id);
        if CGMainDisplayID() == self.display_id || mirrored || overlapped {
            log::warn!(
                "[computer_use:macos-vd] virtual display placement guard tripped \
                 (main: {}, mirrored: {mirrored}, overlapped: {overlapped}); {}",
                CGMainDisplayID() == self.display_id,
                if mirrored {
                    "a mirror join cannot be repaired by re-parking — agent content \
                     may be visible on the user's screen until the mirror is undone"
                } else {
                    "re-applying session-scope placement right of all displays"
                }
            );
            if !mirrored && let Err(error) = park_right_of_main(self.display_id) {
                log::warn!("[computer_use:macos-vd] re-parking failed: {error}");
            }
        }
    }

    /// Posts one mouse event **to a pid** (`CGEventPostToPid` — the public
    /// per-process delivery API; the global `CGEventPost` used by the
    /// physical backend is deliberately NOT used here: it moves the user's
    /// real cursor).
    fn post_mouse_to_pid(
        &self,
        pid: i32,
        event_type: CGEventType,
        button: CGMouseButton,
        at: (i32, i32),
        click_state: i64,
    ) -> Result<(), ComputerUseError> {
        let Some(event) = CGEvent::new_mouse_event(
            None,
            event_type,
            CGPoint {
                x: f64::from(at.0),
                y: f64::from(at.1),
            },
            button,
        ) else {
            return Err(ComputerUseError::failed(format!(
                "CGEventCreateMouseEvent returned null for event type {event_type:?}"
            )));
        };
        if click_state > 0 {
            CGEvent::set_integer_value_field(
                Some(&event),
                CGEventField::MouseEventClickState,
                click_state,
            );
        }
        CGEvent::post_to_pid(pid, Some(&event));
        Ok(())
    }

    /// Posts one keyboard event **to a pid**: kVK keycode + flag mask;
    /// `unicode` attaches a UTF-16 string to the event
    /// (`CGEventKeyboardSetUnicodeString`), bypassing the keymap entirely so
    /// typed text lands regardless of any keyboard layout.
    fn post_key_to_pid(
        &self,
        pid: i32,
        key_code: u16,
        down: bool,
        flags: CGEventFlags,
        unicode: Option<&[u16]>,
    ) -> Result<(), ComputerUseError> {
        let Some(event) = CGEvent::new_keyboard_event(None, key_code, down) else {
            return Err(ComputerUseError::failed(
                "CGEventCreateKeyboardEvent returned null",
            ));
        };
        CGEvent::set_flags(Some(&event), flags);
        if let Some(units) = unicode {
            // SAFETY: units is a valid UTF-16 buffer of the given length;
            // the event copies the string before this call returns.
            unsafe {
                CGEvent::keyboard_set_unicode_string(
                    Some(&event),
                    c_ulong::try_from(units.len()).unwrap_or(c_ulong::MAX),
                    units.as_ptr(),
                );
            }
        }
        CGEvent::post_to_pid(pid, Some(&event));
        Ok(())
    }

    /// One modifier press (down=true) / release (down=false) carrying the
    /// accumulated mask. NOTE: posted as a keyDown/keyUp
    /// (`CGEventCreateKeyboardEvent`), NOT a FlagsChanged event — an earlier
    /// doc draft claimed FlagsChanged, which the event factory here does
    /// not produce. Chords still work because the non-modifier keys carry
    /// the flag mask themselves; a lone-modifier hold (hold_key with only
    /// modifiers) may not register in apps tracking the FlagsChanged event
    /// type — moot in practice, because the tool layer rejects
    /// modifier-only chords at parse time.
    fn post_modifier(
        &self,
        pid: i32,
        key: Key,
        flags: CGEventFlags,
        down: bool,
    ) -> Result<(), ComputerUseError> {
        let (code, _) = virtual_keycode(key)?;
        self.post_key_to_pid(pid, code, down, flags, None)
    }

    /// Posts a plain key click (down + up) for one key with the flag mask
    /// (a Shift-needing character gets the Shift flag added).
    fn key_click(&self, pid: i32, key: Key, flags: CGEventFlags) -> Result<(), ComputerUseError> {
        let (code, needs_shift) = virtual_keycode(key)?;
        let flags = if needs_shift {
            flags | CGEventFlags::MaskShift
        } else {
            flags
        };
        self.post_key_to_pid(pid, code, true, flags, None)?;
        self.post_key_to_pid(pid, code, false, flags, None)
    }

    /// The keyboard target pid (last interacted), failing explicitly when
    /// the session has not interacted with anything yet.
    fn keyboard_pid(&self) -> Result<i32, ComputerUseError> {
        self.input_pid(None)
    }

    /// Presses all chord keys (modifiers first), holds for `hold` in
    /// cancel-checked slices, then releases in reverse. Modifier keys are
    /// pressed as keyDown/keyUp events carrying the accumulated flag mask
    /// (NOT FlagsChanged — see [`Self::post_modifier`]); character keys
    /// carry the mask on their down/up events.
    fn chord(&mut self, keys: &[Key], hold: Duration) -> Result<(), ComputerUseError> {
        // Map the whole chord first, then press (mapping failures must not
        // strand pressed modifiers — same rollback discipline as the
        // physical backend's chord()).
        let mapped = ordered_chord_keys(keys)
            .iter()
            .map(|key| virtual_keycode(*key).map(|(code, shift)| (*key, code, shift)))
            .collect::<Result<Vec<_>, _>>()?;
        let cancel = self.cancel.clone();
        let pid = match self.keyboard_pid() {
            Ok(pid) => pid,
            Err(error) => {
                // A targeting failure is the same error class the other
                // input entry points route through clear_focus_on_error —
                // chord must not skip it, or a stale remembered target
                // keeps failing every chord until some click resets it.
                self.clear_focus_on_error(&error);
                return Err(error);
            }
        };
        let mut flags = CGEventFlags::empty();
        // Modifiers pressed so far (flags), non-modifier keys held (codes +
        // their shift requirement for the release mask).
        let mut pressed_modifiers: Vec<CGEventFlags> = Vec::with_capacity(mapped.len());
        let mut pressed_keys: Vec<(u16, bool)> = Vec::with_capacity(mapped.len());
        let result = (|| {
            for (key, code, shift) in &mapped {
                if let Some(flag) = modifier_flag(*key) {
                    flags |= flag;
                    self.post_modifier(pid, *key, flags, true)?;
                    pressed_modifiers.push(flag);
                } else {
                    let key_flags = if *shift {
                        flags | CGEventFlags::MaskShift
                    } else {
                        flags
                    };
                    self.post_key_to_pid(pid, *code, true, key_flags, None)?;
                    pressed_keys.push((*code, *shift));
                }
            }
            // Chunk the hold so a stop/timeout cancel flag is polled within
            // HOLD_CANCEL_POLL_MS instead of waiting out the whole hold. The
            // base verdict carries only the cancel fact; the shared merge
            // below appends the truthful release outcome.
            let mut remaining = hold;
            while remaining > Duration::ZERO {
                if cancel
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::SeqCst))
                {
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
        // Release everything, best-effort, in reverse (characters first,
        // modifiers last, each modifier's flag leaving the mask as it goes).
        let mut release = Ok(());
        for (code, shift) in pressed_keys.iter().rev() {
            let release_flags = if *shift {
                flags | CGEventFlags::MaskShift
            } else {
                flags
            };
            if let Err(error) = self.post_key_to_pid(pid, *code, false, release_flags, None) {
                if release.is_ok() {
                    release = Err(error);
                }
            }
        }
        for flag in pressed_modifiers.iter().rev() {
            flags.remove(*flag);
            if let Err(error) = self.post_modifier(pid, released_modifier_key(*flag), flags, false)
            {
                if release.is_ok() {
                    release = Err(error);
                }
            }
        }
        // Shared truthful merge (the same helper as the Linux/Windows hold
        // paths — previously a private inline merge with different wording):
        // the cancel fact AND the release outcome surface together, never
        // the false "the keys have been released" all-clear.
        let result = combine_hold_errors(result, release);
        if let Err(error) = &result {
            self.clear_focus_on_error(error);
        }
        result
    }

    /// Posts one UTF-16 chunk as a unicode-string key-down to the keyboard
    /// target (enigo's proven fast_text shape: a single key-down event
    /// carrying the string with flags cleared, so ambient modifiers cannot
    /// corrupt the text; no key-up is posted for text, matching the
    /// physical backend's typing path, which goes through enigo).
    fn post_unicode_chunk(&self, units: &[u16]) -> Result<(), ComputerUseError> {
        let pid = self.keyboard_pid()?;
        self.post_key_to_pid(pid, 0, true, CGEventFlags::empty(), Some(units))
    }

    /// The click landing spot: the last synthesized position, or the center
    /// of the virtual display when nothing has been moved yet (there is no
    /// user cursor to fall back to — a fresh session clicking "wherever the
    /// pointer is" gets a well-defined center instead of an error).
    fn click_destination(&self) -> Result<(i32, i32), ComputerUseError> {
        if let Some(point) = self.cursor {
            return Ok(point);
        }
        let bounds = CGDisplayBounds(self.display_id);
        Ok((
            (bounds.origin.x + bounds.size.width / 2.0).round() as i32,
            (bounds.origin.y + bounds.size.height / 2.0).round() as i32,
        ))
    }

    /// Launches `program` inside the virtual display target.
    ///
    /// Return contract (honest): the returned u32 is the owning pid, or 0
    /// when the launch channel cannot report one. A FAILED launch fails
    /// explicitly, never a silent `Ok(0)`: the direct-exec path detects a
    /// binary that exits without ever showing a window (`Err`), and the
    /// `open -a` path reports the launcher's nonzero exit (`Err`). What the
    /// `open -a` arm CANNOT see is a bundle whose binary crashes
    /// AFTER the launcher reported success: `open` exits 0 without the app
    /// pid, pid discovery finds no window within
    /// [`OPEN_PID_DISCOVER_TIMEOUT`], and the answer is the honest
    /// "launched, owner unknown" `Ok(0)` — the session should interact via
    /// clicks first (a screenshot after `open` shows the result either
    /// way).
    fn launch(&mut self, program: &str, args: &[String]) -> Result<u32, ComputerUseError> {
        let program = validate_program(program)?;
        let launched_pid: Option<u32> = if is_app_bundle(program) {
            // Pre-launch pid snapshot (+ our own): what `open -a` pid
            // discovery subtracts to find the newcomer. Taken only on this
            // arm (the direct-exec arm knows its pid from the spawn).
            let own_pid = i32::try_from(std::process::id()).unwrap_or(0);
            let known_pids: Vec<i32> = on_screen_windows()
                .into_iter()
                .map(|window| window.pid)
                .chain(std::iter::once(own_pid))
                .collect();
            // .app bundles: `open -a <path> [--args ...]`. `open --env` is
            // deliberately NOT used: its availability varies across macOS
            // releases and cannot be verified from this codebase — the
            // .app path is documented to launch WITHOUT injected env vars
            // (bundles launched via launchd mostly ignore inherited env
            // anyway). `--args` passes argv for apps that read it.
            let mut command = Command::new("/usr/bin/open");
            command.arg("-a").arg(program);
            if !args.is_empty() {
                command.arg("--args");
                command.args(args);
            }
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let mut child = command.spawn().map_err(|error| {
                ComputerUseError::unavailable(format!("open: cannot launch the app: {error}"))
            })?;
            // `open` is short-lived: wait for its exit (bounded) so a FAILED
            // launch — a typo'd app name, a missing bundle — surfaces here
            // instead of falling through pid discovery and reporting the
            // false "launched, pid 0". A slow launcher (beyond the budget)
            // hands its Child to the detached reaper and keeps the old
            // discovery semantics.
            let deadline = Instant::now() + OPEN_EXIT_STATUS_BUDGET;
            let mut exit_status = None;
            while exit_status.is_none() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        exit_status = Some(status);
                        break;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        // The launcher's exit status is unreadable, so the
                        // FAILED-launch honesty contract cannot be evaluated:
                        // fail explicitly (with a detached reaper for the
                        // child) instead of degrading to pid discovery that
                        // could report a false "launched, pid 0".
                        let _ = std::thread::Builder::new()
                            .name("computer-use-open-reaper".to_string())
                            .spawn(move || {
                                let _ = child.wait();
                            });
                        return Err(ComputerUseError::failed(format!(
                            "open: cannot read the launcher's exit status ({error}); \
                             the launch could not be verified"
                        )));
                    }
                }
                if Instant::now() >= deadline {
                    break;
                }
                sleep(POLL_INTERVAL);
            }
            match exit_status {
                Some(status) if !status.success() => {
                    return Err(ComputerUseError::unavailable(format!(
                        "open: the launcher exited with {status} — a bad app name or a \
                         missing bundle fails here"
                    )));
                }
                Some(_) => {} // exited successfully; already reaped
                None => {
                    // Still running past the budget: detach a reaper so the
                    // Child never becomes a zombie.
                    let _ = std::thread::Builder::new()
                        .name("computer-use-open-reaper".to_string())
                        .spawn(move || {
                            let _ = child.wait();
                        });
                }
            }
            self.discover_new_pid(&known_pids, program)
        } else {
            // Plain executable: direct exec, process-group leader, stdin/out
            // null; the pid is known immediately.
            let mut command = Command::new(program);
            command.args(args);
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            std_process_group_leader(&mut command);
            let mut child = command.spawn().map_err(|error| {
                ComputerUseError::unavailable(format!("open: cannot launch the program: {error}"))
            })?;
            let pid = child.id();
            // Wait (bounded) for the app's first window BEFORE the AX move
            // below: the window is created asynchronously after spawn, and
            // moving immediately reads AXWindows = 0 — a structural no-op
            // that left the window on the user's desktop even WITH the
            // Accessibility grant. The wait also catches a binary that
            // exits without ever showing a window: reporting that as
            // "launched" would be dishonest.
            let deadline = Instant::now() + OPEN_PID_DISCOVER_TIMEOUT;
            let mut window_seen = false;
            let mut exit_status = None;
            while !window_seen && exit_status.is_none() {
                if pid_has_any_window(pid as i32) {
                    window_seen = true;
                    break;
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        exit_status = Some(status);
                        break;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        // The exit status is unreadable, so the honesty
                        // contract ("exited without a window" is an explicit
                        // error, never a silent success) cannot be evaluated:
                        // hand the child to a detached reaper and fail
                        // explicitly instead of degrading to the
                        // launch-and-pray path.
                        let _ = std::thread::Builder::new()
                            .name("computer-use-open-reaper".to_string())
                            .spawn(move || {
                                let _ = child.wait();
                            });
                        return Err(ComputerUseError::failed(format!(
                            "open: cannot read the launched program's exit status \
                             ({error}); the launch could not be verified"
                        )));
                    }
                }
                if Instant::now() >= deadline {
                    break;
                }
                sleep(POLL_INTERVAL);
            }
            match (window_seen, exit_status) {
                (false, Some(status)) => {
                    return Err(ComputerUseError::unavailable(format!(
                        "open: the program exited without showing a window ({status}) — a \
                         bad binary or a purely CLI tool fails here"
                    )));
                }
                (false, None) => {
                    log::warn!(
                        "[computer_use:macos-vd] launched {program:?} but no window appeared \
                         within the budget; attempting the window move anyway"
                    );
                }
                _ => {}
            }
            if exit_status.is_none() {
                // Still running: hand the Child to a detached reaper (the
                // already-exited case was reaped by try_wait above).
                let _ = std::thread::Builder::new()
                    .name("computer-use-open-reaper".to_string())
                    .spawn(move || {
                        let _ = child.wait();
                    });
            }
            Some(pid)
        };
        let Some(pid) = launched_pid else {
            log::warn!(
                "[computer_use:macos-vd] launched {program:?} but no new application window \
                 on the virtual display appeared within the discovery budget; reporting \
                 pid 0"
            );
            return Ok(0);
        };
        let pid_i32 = i32::try_from(pid).unwrap_or(0);
        if pid_i32 <= 0 {
            log::warn!(
                "[computer_use:macos-vd] launched {program:?} but the discovered pid is \
                 unusable ({pid}); reporting pid 0"
            );
            return Ok(0);
        }
        // Best-effort: move the app's front window into the virtual display
        // bounds (AX position set; needs the Accessibility grant — a missing
        // grant leaves the window wherever the system put it, warn only,
        // non-fatal. The direct-exec path waited for the window above, so a
        // granted machine reliably relocates it).
        self.move_front_window_into_display(pid_i32);
        self.focus_pid = Some(pid_i32);
        // Ownership record for the hit-test: this pid is the session's (see
        // `session_pids`) — its windows stay drivable wherever the AX move
        // has got to, until the ownership grace expires (then the display
        // bounds decide, like for every other pid).
        // Prune inert entries (past the ownership grace) so the vec stays
        // bounded by live grace windows instead of growing with every open
        // (the predicate itself treats stale entries as unknown pids).
        self.session_pids
            .retain(|(_, at)| at.elapsed() < SESSION_OWNERSHIP_GRACE);
        self.session_pids.push((pid_i32, Instant::now()));
        Ok(pid)
    }

    /// Polls the window list for a normal-level pid not in `known` (the
    /// pre-launch snapshot + our own pid). Bounded. A newcomer whose owner
    /// name matches `program` is preferred and returned immediately (a
    /// name match is strong identity; its window may still be outside the
    /// display bounds — the caller's AX move relocates it). The FALLBACK
    /// (no name match) only ever claims a newcomer whose window already
    /// intersects the virtual display's bounds: the user's own windows live
    /// on displays that do not intersect the virtual one, so a desktop
    /// window the user happens to open mid-search is never hijacked as the
    /// input target. When only out-of-bounds unnamed windows appear within
    /// the budget the answer is None (pid 0 upstream: launched, owner
    /// unknown — interact via clicks; residual mis-attribution among
    /// IN-BOUNDS windows is disclosed in `Capabilities::notes`).
    fn discover_new_pid(&self, known: &[i32], program: &str) -> Option<u32> {
        let stem = launched_program_stem(program);
        let bounds = CGDisplayBounds(self.display_id);
        let deadline = Instant::now() + OPEN_PID_DISCOVER_TIMEOUT;
        // First name-unconfirmed IN-BOUNDS newcomer and when it was seen
        // (the fallback answer once the grace expires or the budget runs
        // out).
        let mut fallback: Option<(u32, Instant)> = None;
        loop {
            for window in on_screen_windows() {
                if !window.normal_level || window.pid <= 0 || known.contains(&window.pid) {
                    continue;
                }
                let Ok(pid) = u32::try_from(window.pid) else {
                    continue;
                };
                // Name-confirmed newcomer: the preferred answer (a match
                // anywhere in this pass wins over the recorded fallback).
                if owner_matches_program(window.owner_name.as_deref(), &stem) {
                    return Some(pid);
                }
                if fallback.is_none() && window.intersects(bounds) {
                    fallback = Some((pid, Instant::now()));
                }
            }
            let now = Instant::now();
            match fallback {
                Some((pid, seen_at)) => {
                    if now >= deadline || now.duration_since(seen_at) >= DISCOVER_NAME_GRACE {
                        return Some(pid);
                    }
                }
                None if now >= deadline => return None,
                None => {}
            }
            sleep(POLL_INTERVAL);
        }
    }

    /// AX-moves the front window of `pid` to the virtual display origin +
    /// margin (AXWindows → AXPosition on window 0). Best-effort, warn only —
    /// but NOT cosmetic: the move is what keeps an opened app's window off
    /// the user's desktop. Without an Accessibility grant the reads error
    /// out and the window stays where the system put it (disclosed in the
    /// target note); input itself does not depend on AX (delivered by pid,
    /// capture by display id) — but see launch(): an app whose window never
    /// lands in the display bounds becomes uninputtable, so a failed move
    /// on a granted machine degrades that app, not just the layout.
    fn move_front_window_into_display(&self, pid: i32) {
        if pid <= 0 {
            return;
        }
        // SAFETY: AXUIElementCreateApplication returns a +1 element managed
        // by the CFRetained; callable on any thread (without the
        // Accessibility grant the later reads error — handled below).
        let app = unsafe { AXUIElement::new_application(pid) };
        // Bound the synchronous AX IPC on THIS element the same way the
        // physical backend bounds ui_tree/focused_element (2 s, failures
        // ignored = the default timeout): a hung target app must not drag
        // the best-effort move out to the ~6 s default per call.
        // SAFETY: plain attribute setter; failures fall back to the default.
        let _ = unsafe { app.set_messaging_timeout(2.0_f32) };
        let windows_name = NSString::from_str("AXWindows");
        let mut raw: *const CFType = std::ptr::null();
        // SAFETY: attribute is an NSString toll-free bridged to CFString;
        // the out pointer is a valid stack slot; the +1 result is consumed
        // by the guard below (released exactly once on all paths).
        let error =
            unsafe { app.copy_attribute_value(windows_name.as_ref(), NonNull::from(&mut raw)) };
        let Some(raw) = NonNull::new(raw.cast_mut()) else {
            if error != AXError::Success {
                log::warn!(
                    "[computer_use:macos-vd] cannot read the launched app's windows \
                     (AXError({:?}))",
                    error
                );
            }
            return;
        };
        // SAFETY: raw is the +1 object from the Copy rule (checked non-null
        // above); the guard releases it exactly once on every exit path.
        let guard = CfGuard(raw.cast());
        // AXWindows is a CFArray per the AX contract; verify the type before
        // treating it as one (same discipline as platform/macos.rs).
        // SAFETY: CFGetTypeID/CFArrayGetTypeID are pure type queries on a
        // live CF object held by the guard.
        if unsafe { CFGetTypeID(guard.0.as_ptr()) } != unsafe { CFArrayGetTypeID() } {
            log::warn!("[computer_use:macos-vd] the app's AXWindows value is not a window list");
            return;
        }
        // SAFETY: the guard holds the +1 and the type is verified; the
        // array is borrowed only within this function.
        let array = unsafe { &*guard.0.as_ptr().cast::<CFArray>() };
        let count = usize::try_from(array.count()).unwrap_or(0);
        if count == 0 {
            log::warn!("[computer_use:macos-vd] the launched app reports no windows yet");
            return;
        }
        // SAFETY: 0 < count from CFArrayGetCount on the same immutable
        // array; the element is owned by the array (kept alive by the
        // guard) and is an AXUIElement per the AX contract.
        let window_raw = unsafe { array.value_at_index(0) };
        if window_raw.is_null() {
            return;
        }
        // SAFETY: the element is kept alive by the array guard; AXUIElement
        // is toll-free bridged, and CFRetain is equivalent to its Obj-C
        // retain — the extra +1 is released by the CFRetained on drop.
        let Some(window_ptr) = NonNull::new(window_raw.cast::<AXUIElement>().cast_mut()) else {
            return;
        };
        // SAFETY: window_ptr is the +1 element taken from the array (Copy
        // rule, checked non-null above); CFRetained::retain adopts it and
        // releases it exactly once on drop.
        let window = unsafe { CFRetained::retain(window_ptr) };
        let bounds = CGDisplayBounds(self.display_id);
        let mut position = CGPoint {
            x: bounds.origin.x + WINDOW_MOVE_MARGIN_PX,
            y: bounds.origin.y + WINDOW_MOVE_MARGIN_PX,
        };
        // SAFETY: position is a valid CGPoint buffer on the stack;
        // AXValueCreate copies it before returning.
        let Some(value) =
            (unsafe { AXValue::new(AXValueType::CGPoint, NonNull::from(&mut position).cast()) })
        else {
            log::warn!("[computer_use:macos-vd] cannot build the AX position value");
            return;
        };
        let position_name = NSString::from_str("AXPosition");
        // SAFETY: attribute is an NSString bridged to CFString; value is a
        // live AXValue (a CFType subclass); the set is best-effort and its
        // error is reported, never ignored into silence.
        unsafe {
            let result = window.set_attribute_value(position_name.as_ref(), value.as_ref());
            if result != AXError::Success {
                log::warn!(
                    "[computer_use:macos-vd] moving the app window into the virtual display \
                     failed (AXError({:?}))",
                    result
                );
            }
        }
    }
}

/// RAII release for the +1 CF object returned by the AX Copy rule (the
/// minimal local equivalent of `platform/macos.rs`'s CfObject — that one
/// carries conversion helpers the tree traversal needs; this guard only
/// releases).
struct CfGuard(NonNull<c_void>);

impl Drop for CfGuard {
    fn drop(&mut self) {
        // SAFETY: self.0 is the +1 object returned by the AX Copy rule
        // (checked non-null at construction); released exactly once here
        // and never used afterwards.
        unsafe { CFRelease(self.0.as_ptr()) };
    }
}

/// The modifier Key a released CGEventFlags bit belongs to (only ever called
/// with the single-bit masks chord() pushes).
fn released_modifier_key(flag: CGEventFlags) -> Key {
    if flag == CGEventFlags::MaskControl {
        Key::Control
    } else if flag == CGEventFlags::MaskAlternate {
        Key::Alt
    } else if flag == CGEventFlags::MaskShift {
        Key::Shift
    } else {
        Key::Meta
    }
}

/// The model-facing target disclosure. A function (not inline text) so the
/// disclosure test below pins the EXACT strings the backend reports — a
/// local copy in the test could silently drift from what the model sees.
/// Self-truthful for macOS: input stays on the virtual display (the
/// remembered input target is verified to still show a window touching the
/// display's bounds before every post), screenshots are captured from the
/// display's own bounds (a window overhanging into them contributes its
/// pixels — window-level straddle is disclosed, display-level overlap is
/// verified away at park time), and apps opened with `open` launch into the
/// user's graphics session first and are AX-moved into the display
/// best-effort — without the Accessibility grant an opened app's window can
/// stay visible on the user's desktop. Apps opened into the virtual display
/// are not wound down this round: neither Stop nor dropping the backend
/// (which removes the display) terminates them — they keep running, and
/// their windows move to remaining displays when the display disappears.
fn capability_target_note() -> String {
    "experimental isolated virtual display (private macOS API): input stays on the \
     virtual display and input targets are re-verified to still show a window \
     touching it before every post (a just-opened app keeps a one-minute ownership \
     grace in which its window is drivable wherever it sits — including \
     overhanging onto the user's desktop — before the display-bounds rule \
     applies to it too); screenshots are captured from the virtual display's \
     own bounds, so a window overhanging into those bounds contributes its \
     pixels to the capture, and if the display joins a mirror set mid-session \
     the captures may show the user's desktop and agent content may be visible \
     on the user's screen until the mirror is undone; events are delivered \
     directly to the target app \
     and may not reach every app (e.g. WebKit content processes on recent \
     macOS may ignore them); apps opened with open launch into the user's \
     graphics session and their FRONT window is moved into the virtual display \
     best-effort — an app's other windows may stay outside, and \
     without the Accessibility grant an opened app's window may stay visible \
     on the user's desktop; if this macOS lacks the private API the session \
     falls back to your desktop, decided once at session start; apps opened \
     in the virtual display are NOT closed when a session stops or when the \
     backend is dropped — they keep running, and when the virtual display \
     disappears their windows move to the remaining displays"
        .to_string()
}

/// The model-facing operational notes (same pin-the-real-text rationale as
/// [`capability_target_note`]). Discloses the cursor_position semantics (a
/// virtual display has no cursor of its own) and the open-pid discovery
/// residual (a rare unrelated window inside the discovery budget can be
/// attributed to the wrong app).
fn capability_notes() -> String {
    "macos virtual display: coordinates are CGEvent global points relative to the \
     virtual display's own bounds; mouse events target the targetable app at \
     the point (windows outside this session's scope — e.g. a user window \
     overhanging the parked display — are never targeted; the targetable \
     window below such a cover receives the event) and keyboard events target \
     the last-interacted app, re-verified to still show a window on the \
     virtual display (a moved-away or quit target answers an error — click \
     inside the display to focus a new one); cursor_position reports the last \
     point this backend moved to (a virtual display has no cursor of its own), or the \
     display's top-left origin before the first move; Screen Recording and \
     Accessibility grants are required (input answers an explicit error without \
     Accessibility, same copy as the physical path); typed text bypasses the \
     keyboard layout (direct Unicode); key chords are injected as US-ANSI \
     positional virtual-key codes while the receiving app resolves them through \
     YOUR current keyboard layout — on a non-US layout a letter chord can trigger \
     the key sitting at that US position (e.g. Cmd+A may act as Cmd+Q on AZERTY), \
     so prefer named keys (Enter, arrows, F-keys) or typed text where possible; \
     the capture uses the CGWindowList path, which on Sequoia+ \
     sits behind the periodic keep-allowing-screen-recording confirmation; apps \
     launched with open take a moment before their windows appear, arguments \
     passed with open apply only on a fresh launch (opening an already-running \
     app just activates it and drops the arguments), and the pid \
     reported for such an app is recovered by watching for a new window on the \
     virtual display (owner-name match preferred — a same-named window the USER \
     opens during the brief search would be claimed and moved into the virtual \
     display; unnamed windows only when their window already overlaps the \
     display's bounds) — in rare cases an unrelated in-bounds window appearing \
     during that search is attributed to the wrong app, and when no matching \
     window appears the pid is reported as 0 (interact via clicks)"
        .to_string()
}

impl ComputerUseBackend for MacosVirtualDisplayBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            screenshot: true,
            input: true,
            // v1 keeps element actions Unsupported on virtual targets
            // (consistent with the other virtual backends): AX targeting of
            // windows on the virtual display could work, but the T3
            // screening fail-open path covers its absence, and an explicit
            // Unsupported beats a half-working tree.
            ui_tree: false,
            target: DisplayTarget::Virtual,
            target_note: capability_target_note(),
            notes: {
                let mut notes = capability_notes();
                // CGEventPostToPid needs the Accessibility TCC grant just
                // like the physical backend's CGEvent path; surface the
                // live grant state so the input note is honest per machine.
                if !accessibility_granted() {
                    notes.push_str("\ninput additionally requires the Accessibility permission: ");
                    notes.push_str(&accessibility_error().to_string());
                }
                notes
            },
        }
    }

    fn capture(&mut self) -> Result<Capture, ComputerUseError> {
        if !screen_recording_granted() {
            return Err(screen_recording_error());
        }
        self.ensure_parked();
        let bounds = CGDisplayBounds(self.display_id);
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return Err(ComputerUseError::unavailable(
                "the virtual display has empty bounds (it may have been detached)",
            ));
        }
        let image = capture_display_image(bounds)?;
        // Shared BGRA→RGBA conversion — the ScreenCaptureKit adapter's
        // tested helper, reused via sibling-module visibility instead of the
        // previous private near-verbatim copy; the stride pin in this
        // module's tests drives the SAME shared row-copy, so the two stacks
        // cannot drift.
        let converted =
            super::super::screen_capture_kit::bgra_image_to_rgba(&image).map_err(|reason| {
                ComputerUseError::failed(format!("virtual display capture: {reason}"))
            })?;
        let mut rgba = converted.rgba;
        let width = converted.width;
        let height = converted.height;
        // CGWindowListCreateImage's alpha depends on the content; force
        // opaque in one pass so the PNG never leaks meaningless alpha (the
        // same convention as both physical macOS capture paths).
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        // The scale is derived backwards from the returned image
        // (points / actual pixels), never assumed — v1 runs non-hiDPI so it
        // is 1.0, but future rotated/non-integer modes stay correct.
        let input_scale_x = bounds.size.width / (width.max(1) as f64);
        let input_scale_y = bounds.size.height / (height.max(1) as f64);
        Ok(Capture {
            rgba,
            width: width as u32,
            height: height as u32,
            origin_x: bounds.origin.x.round() as i32,
            origin_y: bounds.origin.y.round() as i32,
            input_scale_x,
            input_scale_y,
            input_aligned: true,
        })
    }

    fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
        // The virtual display has no user cursor: nobody's hand is on it.
        // The truthful reading is the last point this backend synthesized a
        // move to; before any move, the display's top-left corner (the
        // origin of its bounds) is the defined starting position.
        if let Some(point) = self.cursor {
            return Ok(point);
        }
        let bounds = CGDisplayBounds(self.display_id);
        Ok((
            bounds.origin.x.round() as i32,
            bounds.origin.y.round() as i32,
        ))
    }

    fn move_to(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        // While a button is held, the move belongs to the window that
        // accepted the press (real mouse semantics; the `drag` action pins
        // the same way) — re-hit-testing the waypoint would re-target the
        // gesture onto whatever window lies under it.
        let pressed_pid = self.held_pids.last().map(|&(_, pid)| pid);
        let pid = match pressed_pid {
            Some(remembered) => match self.reverified_target_pid(remembered) {
                Ok(pid) => pid,
                Err(error) => {
                    self.clear_focus_on_error(&error);
                    return Err(error);
                }
            },
            None => self.input_pid_cleared(Some((x, y)))?,
        };
        let (event_type, button) = move_event_type(&self.held_buttons);
        let posted = self.post_mouse_to_pid(pid, event_type, button, (x, y), 0);
        if posted.is_ok() {
            self.cursor = Some((x, y));
        } else if let Err(error) = &posted {
            self.clear_focus_on_error(error);
        }
        posted
    }

    fn click(&mut self, button: MouseButton, count: u8) -> Result<(), ComputerUseError> {
        let dest = self.click_destination()?;
        let pid = self.input_pid_cleared(Some(dest))?;
        // Settle before the first down (the physical path settles after a
        // move). Note the hit-test in input_pid has ALREADY run above — this
        // settle gives the target app time to process the synthesized
        // events, not to appear.
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        let (down, up, _) = cg_mouse_event_types(button);
        let cg_button = cg_mouse_button(button);
        let rounds = u32::from(count.max(1));
        // Track whether THIS click owns the held_buttons entry (a click
        // issued while mouse_down already holds the same button must not
        // steal the entry — same fix as the physical backend).
        let we_pressed = !self.held_buttons.contains(&button);
        let mut result = Ok(());
        for i in 0..rounds {
            let click_state = i64::from(i) + 1;
            // A failed DOWN is answered directly (the physical backend
            // propagates it before attempting any release): posting an up
            // for a button that was never pressed hands the app a spurious
            // release, and the old shape then blamed "the mouse button may
            // still be pressed" for a press that never happened.
            if let Err(error) = self.post_mouse_to_pid(pid, down, cg_button, dest, click_state) {
                result = Err(error);
                break;
            }
            if we_pressed {
                self.held_buttons.push(button);
            }
            // A stranded down (up failed twice) must never be silent — the
            // same wording contract as the physical backend.
            if let Err(error) = self.post_mouse_to_pid(pid, up, cg_button, dest, click_state) {
                sleep(Duration::from_millis(MULTI_CLICK_INTERVAL_MS));
                if let Err(retry) = self.post_mouse_to_pid(pid, up, cg_button, dest, click_state) {
                    result = Err(error.same_kind(format!(
                        "click release failed twice ({error}; {retry}); \
                         the mouse button may still be pressed"
                    )));
                    break;
                }
            }
            if we_pressed {
                self.held_buttons.retain(|held| *held != button);
            }
            if i + 1 < rounds {
                sleep(Duration::from_millis(MULTI_CLICK_INTERVAL_MS));
            }
        }
        if result.is_ok() {
            // A successful click makes the clicked app the keyboard target.
            self.focus_pid = Some(pid);
            self.cursor = Some(dest);
        } else if let Err(error) = &result {
            self.clear_focus_on_error(error);
        }
        result
    }

    fn mouse_down(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        let dest = self.click_destination()?;
        let pid = self.input_pid_cleared(Some(dest))?;
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        let (down, _, _) = cg_mouse_event_types(button);
        let posted = self.post_mouse_to_pid(pid, down, cg_mouse_button(button), dest, 1);
        if posted.is_ok() {
            if !self.held_buttons.contains(&button) {
                self.held_buttons.push(button);
                self.held_pids.push((button, pid));
            }
            self.focus_pid = Some(pid);
            self.cursor = Some(dest);
        } else if let Err(error) = &posted {
            self.clear_focus_on_error(error);
        }
        posted
    }

    fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        let dest = self.click_destination()?;
        // The up goes to the pid that accepted the press (the window that
        // accepts a press keeps the whole gesture): a manual
        // down → up composition must not re-hit-test its way onto a
        // different window than the one holding the synthetic press.
        let pressed_pid = self
            .held_pids
            .iter()
            .rev()
            .find(|(held, _)| *held == button)
            .map(|&(_, pid)| pid);
        let pid = match pressed_pid {
            Some(remembered) => match self.reverified_target_pid(remembered) {
                Ok(pid) => pid,
                Err(error) => {
                    self.clear_focus_on_error(&error);
                    return Err(error);
                }
            },
            None => self.input_pid_cleared(Some(dest))?,
        };
        let (_, up, _) = cg_mouse_event_types(button);
        let posted = self.post_mouse_to_pid(pid, up, cg_mouse_button(button), dest, 1);
        if posted.is_ok() {
            self.held_buttons.retain(|held| *held != button);
            self.held_pids.retain(|(held, _)| *held != button);
        } else if let Err(error) = &posted {
            self.clear_focus_on_error(error);
        }
        posted
    }

    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError> {
        let (down, up, dragged) = cg_mouse_event_types(MouseButton::Left);
        let left = cg_mouse_button(MouseButton::Left);
        // The window that accepts the press keeps the whole drag even when
        // the path leaves its bounds, so every event of this drag targets
        // the pid under the START point.
        let drag_pid = self.input_pid_cleared(Some(from))?;
        // A failed preliminary move is answered directly (the same cleanup
        // discipline as the move/click paths) — no press, no waypoints, no
        // orphan release follows it.
        if let Err(error) = self.post_mouse_to_pid(drag_pid, CGEventType::MouseMoved, left, from, 0)
        {
            self.clear_focus_on_error(&error);
            return Err(error);
        }
        self.cursor = Some(from);
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        // A failed DOWN is answered directly (the same discipline as click):
        // the old shape swallowed the press error and kept posting waypoints
        // and a release for a button that was never pressed — the action
        // could report a successful drag whose press never happened.
        self.post_mouse_to_pid(drag_pid, down, left, from, 1)?;
        if !self.held_buttons.contains(&MouseButton::Left) {
            self.held_buttons.push(MouseButton::Left);
            self.held_pids.push((MouseButton::Left, drag_pid));
        }
        sleep(Duration::from_millis(DRAG_PRESS_SETTLE_MS));
        // Track the last waypoint actually posted: the release must land
        // where the drag really reached, never at an unreached endpoint
        // (same contract as the physical backend).
        let mut last_posted = from;
        let result = (|| {
            for (x, y) in drag_waypoints(from, to, DRAG_STEPS) {
                if self
                    .cancel
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::SeqCst))
                {
                    return Err(ComputerUseError::unavailable(
                        "drag was cancelled (caller timeout or stop); the button is released \
                         and the pointer stays at the last waypoint",
                    ));
                }
                self.post_mouse_to_pid(drag_pid, dragged, left, (x, y), 1)?;
                last_posted = (x, y);
                sleep(Duration::from_millis(DRAG_STEP_DELAY_MS));
            }
            Ok(())
        })();
        let release_at = if result.is_ok() { to } else { last_posted };
        let release = self.post_mouse_to_pid(drag_pid, up, left, release_at, 1);
        if release.is_ok() {
            self.held_buttons.retain(|held| *held != MouseButton::Left);
            self.held_pids
                .retain(|(held, _)| *held != MouseButton::Left);
            self.cursor = Some(release_at);
            if result.is_ok() {
                self.focus_pid = Some(drag_pid);
            }
        }
        let combined = combine_drag_errors(result, release);
        if let Err(error) = &combined {
            self.clear_focus_on_error(error);
        }
        combined
    }

    fn scroll(&mut self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError> {
        // A wheel event goes to the window under the pointer, like a real
        // wheel: target the pid under the last synthesized point. Without a
        // pointer position yet, the wheel targets the last-interacted app
        // (keyboard focus semantics) — both fail explicitly on a fresh
        // session with no app on the display. Targeting failures route
        // through clear_focus_on_error like every other entry point: a
        // stale remembered target must not keep failing every scroll until
        // some click resets it (the same contract `chord` documents).
        let pid = match self.cursor {
            Some(point) => self.input_pid(Some(point)),
            None => self.keyboard_pid(),
        };
        let pid = match pid {
            Ok(pid) => pid,
            Err(error) => {
                self.clear_focus_on_error(&error);
                return Err(error);
            }
        };
        let (axis, delta) = map_cg_scroll(direction, clicks);
        let (wheel1, wheel2) = if axis == 1 { (delta, 0) } else { (0, delta) };
        let Some(event) =
            CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Line, 2, wheel1, wheel2, 0)
        else {
            return Err(ComputerUseError::failed(
                "CGEventCreateScrollWheelEvent returned null",
            ));
        };
        CGEvent::post_to_pid(pid, Some(&event));
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), ComputerUseError> {
        // Direct Unicode injection (CGEventKeyboardSetUnicodeString via
        // key-down events), chunked by the shared helpers with between-run
        // cancel checks — the same shape as the physical backend's typing
        // path, minus enigo (enigo posts globally at kCGHIDEventTap, which
        // is forbidden for this target). CR/CRLF normalize to '\n' first
        // (shared helper; newlines become real Return key clicks).
        let text = normalize_typed_newlines(text);
        let mut result = Ok(());
        for run in split_type_runs(&text, TYPE_CHUNK_CHARS) {
            if self
                .cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
            {
                return Err(ComputerUseError::unavailable(
                    "type text was cancelled (caller timeout or stop); characters already \
                     injected are not undone",
                ));
            }
            let run_result = match run {
                TypeRun::Text(chunk) => utf16_chunks(&chunk, MACOS_UNICODE_STRING_UTF16_UNITS)
                    .into_iter()
                    .try_for_each(|part| {
                        let units: Vec<u16> = part.encode_utf16().collect();
                        self.post_unicode_chunk(&units)
                    }),
                TypeRun::Return => self
                    .keyboard_pid()
                    .and_then(|pid| self.key_click(pid, Key::Enter, CGEventFlags::empty())),
                TypeRun::Tab => self
                    .keyboard_pid()
                    .and_then(|pid| self.key_click(pid, Key::Tab, CGEventFlags::empty())),
            };
            if let Err(error) = run_result {
                result = Err(error);
                break;
            }
        }
        if let Err(error) = &result {
            self.clear_focus_on_error(error);
        }
        result
    }

    fn key_chord(&mut self, keys: &[Key]) -> Result<(), ComputerUseError> {
        self.chord(keys, Duration::from_millis(CHORD_HOLD_MS))
    }

    fn hold_key(&mut self, keys: &[Key], ms: u64) -> Result<(), ComputerUseError> {
        self.chord(keys, Duration::from_millis(ms))
    }

    fn ui_tree(&mut self, _opts: &UiTreeOptions) -> Result<String, ComputerUseError> {
        Err(ui_tree_unsupported())
    }

    fn element_at_point(
        &mut self,
        _x: i32,
        _y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError> {
        Err(element_targeting_unsupported())
    }

    fn set_cancel_flag(&mut self, flag: Option<Arc<AtomicBool>>) {
        self.cancel = flag;
    }

    fn open_app(&mut self, program: &str, args: &[String]) -> Result<u32, ComputerUseError> {
        self.launch(program, args)
    }
}

/// Captures the virtual display with `CGWindowListCreateImage` over
/// `CGDisplayBounds(display_id)` (NOT Monitor::all ordering — the physical
/// backend's enumeration has no stable mapping to a display id).
///
/// Why this path (the ONE capture mechanism, per the design's pick-one
/// requirement):
/// - `SCScreenshotManager::captureImageInRect` only exists on macOS 15.2+;
///   this backend supports macOS 11+ and there is no mid-session capture
///   fallback (design §9) — SCK alone would strand 11–15.1 with no
///   screenshots, and carrying SCK + a second mechanism is two paths.
/// - SCK also takes a rect, not a display id — its targeting would be
///   derived from `CGDisplayBounds` exactly like this path, so it has no
///   targeting advantage.
/// - `CGWindowListCreateImage` is the same call family the xcap fallback of
///   the physical backend uses. Trade-off: on Sequoia+ it sits behind the
///   periodic "keep allowing screen recording" re-confirmation — disclosed
///   in `capabilities().notes`; the Screen Recording grant itself is shared
///   with the physical path (the feature cannot run at all without it).
#[allow(deprecated)] // CGWindowListCreateImage: obsoleted in the macOS 15 SDK, still functional (see above)
fn capture_display_image(bounds: CGRect) -> Result<CFRetained<CGImage>, ComputerUseError> {
    let image = CGWindowListCreateImage(
        bounds,
        CGWindowListOption::OptionOnScreenOnly,
        kCGNullWindowID,
        CGWindowImageOption::Default,
    );
    image.ok_or_else(|| {
        ComputerUseError::unavailable(
            "the virtual display capture returned no image (Screen Recording grant or \
             display state changed)",
        )
    })
}

/// Create the virtual display backend. Called only after the shared dispatch
/// decided Virtual (probe green + preference on); **must run on the
/// computer_use worker thread** (a main-thread call would deadlock on the
/// main-thread dispatch below).
pub(crate) fn create_backend() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    // Defense in depth: the shared dispatch only calls here after a green
    // probe; re-checking keeps the "probe what you send" contract local.
    if !probe().is_available() {
        return Err(ComputerUseError::unavailable(
            "private macOS virtual display API not present on this macOS",
        ));
    }
    let deadline = Instant::now() + ACTIVATION_POLL_TIMEOUT;
    let (tx, rx) = mpsc::channel::<CreatedDisplay>();
    let dispatched = super::super::dispatch_to_main_thread(Box::new(move || {
        // The closure holds the main thread only for the build + the ≤1 s
        // id-read grace (see DISPLAY_ID_GRACE); when the id does not appear
        // it runs the legacy full poll here, exactly the pre-split behavior.
        let _ = tx.send(create_and_read_id(
            deadline.min(Instant::now() + DISPLAY_ID_GRACE),
            deadline,
        ));
    }));
    let (display, display_id, activated) = if dispatched {
        // The Tauri main thread keeps servicing its run loop while this
        // worker waits. Timeout → an explicit `Unavailable`: there is NO
        // mid-creation physical fallback — the ONE-TIME probe-based decision
        // already ran in platform::create_backend, and a creation-time
        // failure is an explicit error the availability snapshot records
        // (tool.rs create_backend_reporting_availability); it is never a
        // silent mid-session flip. A hand-off that lands after the timeout
        // is still released (the channel value drops with the receiver, on
        // this worker thread — see `VdHandle`).
        match rx.recv_timeout(MAIN_DISPATCH_WAIT) {
            Ok(created) => created?,
            Err(_) => {
                return Err(ComputerUseError::unavailable(
                    "virtual display creation did not complete on the main thread within \
                     the budget",
                ));
            }
        }
    } else {
        // No app handle stored (creation before setup finished): the same
        // bounded sequence runs right here on the worker thread.
        create_and_read_id(deadline.min(Instant::now() + DISPLAY_ID_GRACE), deadline)?
    };
    // Budgeted activation wait on the worker when the closure returned
    // early: the run loop kept pumping the whole time, and the poll uses
    // only global CG APIs (the VdHandle contract forbids messaging the
    // crossed-thread object).
    if !activated {
        wait_activation_on_worker(display_id, deadline)?;
    }
    // Park right of the main display (session scope). Non-fatal: the guard
    // re-checks on every capture, and the display is fully functional
    // wherever it currently sits.
    if let Err(error) = park_right_of_main(display_id) {
        log::warn!("[computer_use:macos-vd] initial parking failed: {error}");
    }
    log::info!(
        "[computer_use:macos-vd] virtual display {display_id} active \
         ({VD_WIDTH_PX}x{VD_HEIGHT_PX}@{})",
        VD_REFRESH_HZ
    );
    Ok(Box::new(MacosVirtualDisplayBackend {
        display,
        display_id,
        focus_pid: None,
        session_pids: Vec::new(),
        cursor: None,
        held_buttons: Vec::new(),
        held_pids: Vec::new(),
        cancel: None,
    }))
}

// ---------------------------------------------------------------------------
// Tests. The shared dispatch cfg-gates this whole module to
// target_os = "macos", so every test here runs on macOS hosts / macOS CI
// only (none run on the linux/windows dev hosts — noted in the PR report).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    // map_scroll is only exercised by the scroll-parity pins; keeping it
    // out of the lib-target imports avoids an unused-import deny there.
    use super::super::super::helpers::map_scroll;

    /// The private class-name table is load-bearing: these exact names are
    /// the Apple CGVirtualDisplay private API surface (DeskPad lineage).
    /// A typo here would silently turn every session into physical fallback.
    #[test]
    fn class_names_pin_the_private_api_surface() {
        assert_eq!(CLASS_VIRTUAL_DISPLAY.to_str().unwrap(), "CGVirtualDisplay");
        assert_eq!(
            CLASS_DESCRIPTOR.to_str().unwrap(),
            "CGVirtualDisplayDescriptor"
        );
        assert_eq!(CLASS_SETTINGS.to_str().unwrap(), "CGVirtualDisplaySettings");
        assert_eq!(CLASS_MODE.to_str().unwrap(), "CGVirtualDisplayMode");
        assert_eq!(
            QUEUE_LABEL.to_str().unwrap(),
            "dev.pinvou.agent.computer-use.vd"
        );
    }

    /// The probe verdict must be decidable without crashing on any macOS:
    /// on hosts with the private API it answers Available; without (or on a
    /// future rename) it answers Unavailable with the user-facing detail.
    /// (The verdict itself is host-dependent, so only the shape is pinned.)
    #[test]
    fn probe_answers_an_explicit_verdict() {
        match probe() {
            VirtualAvailability::Available => {}
            VirtualAvailability::Unavailable { detail } => {
                assert!(
                    detail.contains("private macOS virtual display API"),
                    "the unavailable detail must explain the missing private API: {detail}"
                );
            }
        }
    }

    /// The probe is exactly the table walk: when all four private classes
    /// exist, the probe's verdict must equal "every table selector
    /// responds", so extending one without the other cannot drift.
    #[test]
    fn probe_is_the_selector_table_walk() {
        let all_classes_present = [
            CLASS_VIRTUAL_DISPLAY,
            CLASS_DESCRIPTOR,
            CLASS_SETTINGS,
            CLASS_MODE,
        ]
        .iter()
        .all(|name| AnyClass::get(name).is_some());
        if !all_classes_present {
            // Host without the private API: the probe must answer
            // Unavailable without touching the table.
            assert!(!probe().is_available());
            return;
        }
        let all_respond = required_selectors()
            .iter()
            .all(|(_, class, selector)| class.responds_to(*selector));
        assert_eq!(
            probe().is_available(),
            all_respond,
            "the probe must walk exactly the required-selector table"
        );
        // Every selector name is a non-empty Obj-C selector.
        for (_, _, selector) in required_selectors() {
            assert!(!selector.name().is_empty());
        }
        // The create path sends displayID (wait_for_activation); it must stay
        // in the table so the probe guards it — the one getter this module
        // sends besides init/alloc (which every Obj-C class carries).
        assert!(
            required_selectors().iter().any(|(name, _, selector)| {
                *name == CLASS_VIRTUAL_DISPLAY && selector.name() == c"displayID"
            }),
            "displayID is sent by wait_for_activation and must be probed"
        );
    }

    /// Serial identity: stable derivation from the product name, pinned
    /// byte-for-byte so system reports stay consistent across releases (a
    /// drifting serial would look like a display swap).
    #[test]
    fn serial_identity_is_stable_and_derived() {
        assert_eq!(fnv1a32(b""), 0x811c_9dc5, "FNV-1a offset basis");
        assert_eq!(fnv1a32(b"a"), 0xe40c_292c, "FNV-1a 'a'");
        let serial = serial_number();
        assert_eq!(serial, fnv1a32(PRODUCT_NAME.as_bytes()));
        assert_eq!(serial_string(), format!("PINVOU3-VD-{serial:08X}"));
        assert!(serial_string().starts_with(SERIAL_STRING_PREFIX));
        // Deterministic across calls (no hidden entropy).
        assert_eq!(serial_number(), serial_number());
    }

    /// The panel is sized for a sane desktop UI: ≥ 200 PPI at the pinned
    /// geometry (macOS scales menu bar/dock from the millimeter size).
    #[test]
    fn display_geometry_is_hi_density_and_aspect_correct() {
        assert_eq!(VD_WIDTH_PX, 1280);
        assert_eq!(VD_HEIGHT_PX, 800);
        assert_eq!(VD_REFRESH_HZ, 60.0);
        let ppi = effective_ppi(VD_WIDTH_PX, VD_SIZE_MM.width);
        assert!(ppi >= 200.0, "expected ≥ 200 PPI, got {ppi}");
        assert!(ppi < 400.0, "unexpectedly dense panel: {ppi}");
        // 16:10 pixels ↔ 16:10 millimeters.
        let px_ratio = f64::from(VD_WIDTH_PX) / f64::from(VD_HEIGHT_PX);
        let mm_ratio = VD_SIZE_MM.width / VD_SIZE_MM.height;
        assert!((px_ratio - mm_ratio).abs() < 0.01);
        assert_eq!(effective_ppi(1280, 0.0), 0.0, "zero size must not divide");
    }

    /// The kVK table has no wildcard arm over `Key`; every variant the
    /// parser can produce resolves, out-of-range function keys and unmapped
    /// characters fail explicitly, and spot values match Apple's HIToolbox
    /// constants (source: enigo's vendored core-graphics 0.25 KeyCode).
    #[test]
    fn vk_table_is_exhaustive_and_parity_checked() {
        let named: &[Key] = &[
            Key::Control,
            Key::Alt,
            Key::Shift,
            Key::Meta,
            Key::Enter,
            Key::Escape,
            Key::Tab,
            Key::Space,
            Key::Backspace,
            Key::Delete,
            Key::Insert,
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
        ];
        for key in named {
            assert!(virtual_keycode(*key).is_ok(), "no mapping for {key:?}");
        }
        for n in 1..=12u8 {
            assert!(
                virtual_keycode(Key::Function(n)).is_ok(),
                "no mapping for f{n}"
            );
        }
        assert!(virtual_keycode(Key::Function(0)).is_err());
        assert!(virtual_keycode(Key::Function(13)).is_err());
        // kVK spot checks (HIToolbox Events.h values).
        assert_eq!(virtual_keycode(Key::Enter).unwrap().0, 0x24);
        assert_eq!(virtual_keycode(Key::Tab).unwrap().0, 0x30);
        assert_eq!(virtual_keycode(Key::Space).unwrap().0, 0x31);
        assert_eq!(virtual_keycode(Key::Backspace).unwrap().0, 0x33);
        assert_eq!(virtual_keycode(Key::Escape).unwrap().0, 0x35);
        assert_eq!(virtual_keycode(Key::Meta).unwrap().0, 0x37);
        assert_eq!(virtual_keycode(Key::Shift).unwrap().0, 0x38);
        assert_eq!(virtual_keycode(Key::Alt).unwrap().0, 0x3A);
        assert_eq!(virtual_keycode(Key::Control).unwrap().0, 0x3B);
        assert_eq!(virtual_keycode(Key::Delete).unwrap().0, 0x75);
        assert_eq!(virtual_keycode(Key::Insert).unwrap().0, 0x72);
        assert_eq!(virtual_keycode(Key::Up).unwrap().0, 0x7E);
        assert_eq!(virtual_keycode(Key::Down).unwrap().0, 0x7D);
        assert_eq!(virtual_keycode(Key::Left).unwrap().0, 0x7B);
        assert_eq!(virtual_keycode(Key::Right).unwrap().0, 0x7C);
        assert_eq!(virtual_keycode(Key::Home).unwrap().0, 0x73);
        assert_eq!(virtual_keycode(Key::End).unwrap().0, 0x77);
        assert_eq!(virtual_keycode(Key::PageUp).unwrap().0, 0x74);
        assert_eq!(virtual_keycode(Key::PageDown).unwrap().0, 0x79);
        assert_eq!(virtual_keycode(Key::Function(1)).unwrap().0, 0x7A);
        assert_eq!(virtual_keycode(Key::Function(12)).unwrap().0, 0x6F);
        // Modifier flags agree with the CGEventFlagMask constants.
        assert_eq!(modifier_flag(Key::Control), Some(CGEventFlags::MaskControl));
        assert_eq!(modifier_flag(Key::Alt), Some(CGEventFlags::MaskAlternate));
        assert_eq!(modifier_flag(Key::Shift), Some(CGEventFlags::MaskShift));
        assert_eq!(modifier_flag(Key::Meta), Some(CGEventFlags::MaskCommand));
        assert_eq!(modifier_flag(Key::Enter), None);
        // The release-side inverse mapping round-trips.
        for key in [Key::Control, Key::Alt, Key::Shift, Key::Meta] {
            let flag = modifier_flag(key).unwrap();
            assert_eq!(released_modifier_key(flag), key);
        }
    }

    /// The character table is a bijective char → key-position map covering
    /// the full printable US-ANSI range (unshifted + shifted), with no
    /// duplicates, shifted entries on their base positions, and explicit
    /// failures for out-of-table characters.
    #[test]
    fn char_table_covers_printable_us_ansi_without_duplicates() {
        let mut seen: Vec<char> = Vec::new();
        for (c, _, _) in CHAR_KEYS {
            assert!(!seen.contains(c), "duplicate char entry {c:?}");
            seen.push(*c);
        }
        // Every unshifted US key (typeable without Shift)…
        for c in "abcdefghijklmnopqrstuvwxyz0123456789 `-=[]\\;',./".chars() {
            let entry = CHAR_KEYS
                .iter()
                .find(|(candidate, _, _)| candidate == &c)
                .unwrap_or_else(|| panic!("unshifted character {c:?} missing from the table"));
            assert!(!entry.2, "{c:?} must not require Shift");
            assert_eq!(virtual_keycode(Key::Char(c)).unwrap().1, false);
        }
        // …and every Shift-combined character on those same positions.
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ!@#$%^&*()_+{}|:\"<>?~".chars() {
            let entry = CHAR_KEYS
                .iter()
                .find(|(candidate, _, _)| candidate == &c)
                .unwrap_or_else(|| panic!("shifted character {c:?} missing from the table"));
            assert!(entry.2, "{c:?} must require Shift");
        }
        // Shifted entries match their base position (A shares a's keycode).
        let a_lower = CHAR_KEYS.iter().find(|(c, _, _)| *c == 'a').unwrap();
        let a_upper = CHAR_KEYS.iter().find(|(c, _, _)| *c == 'A').unwrap();
        assert_eq!(a_lower.1, a_upper.1, "A must share a's key position");
        assert!(!a_lower.2 && a_upper.2);
        // Out-of-table characters fail explicitly (no wrong-key injection).
        assert!(virtual_keycode(Key::Char('中')).is_err());
        assert!(virtual_keycode(Key::Char('\u{1F600}')).is_err());
        assert!(virtual_keycode(Key::Char('\n')).is_err());
        assert!(virtual_keycode(Key::Char('\0')).is_err());
    }

    /// CGEvent typed constants used for delivery (raw-value parity with
    /// platform/macos.rs's pinned table).
    #[test]
    fn cg_event_types_match_core_graphics_constants() {
        assert_eq!(
            cg_mouse_event_types(MouseButton::Left).0,
            CGEventType::LeftMouseDown
        );
        assert_eq!(
            cg_mouse_event_types(MouseButton::Left).1,
            CGEventType::LeftMouseUp
        );
        assert_eq!(
            cg_mouse_event_types(MouseButton::Left).2,
            CGEventType::LeftMouseDragged
        );
        assert_eq!(
            cg_mouse_event_types(MouseButton::Right).0,
            CGEventType::RightMouseDown
        );
        assert_eq!(
            cg_mouse_event_types(MouseButton::Middle).0,
            CGEventType::OtherMouseDown
        );
        assert_eq!(cg_mouse_button(MouseButton::Left), CGMouseButton::Left);
        assert_eq!(cg_mouse_button(MouseButton::Right), CGMouseButton::Right);
        assert_eq!(cg_mouse_button(MouseButton::Middle), CGMouseButton::Center);
        assert_eq!(CGEventType::LeftMouseDown.0, 1);
        assert_eq!(CGEventType::LeftMouseUp.0, 2);
        assert_eq!(CGEventType::RightMouseDown.0, 3);
        assert_eq!(CGEventType::RightMouseUp.0, 4);
        assert_eq!(CGEventType::MouseMoved.0, 5);
        assert_eq!(CGEventType::LeftMouseDragged.0, 6);
        assert_eq!(CGEventType::RightMouseDragged.0, 7);
        assert_eq!(CGEventType::OtherMouseDown.0, 25);
        assert_eq!(CGEventType::OtherMouseUp.0, 26);
        assert_eq!(CGEventType::OtherMouseDragged.0, 27);
        assert_eq!(CGMouseButton::Left.0, 0);
        assert_eq!(CGMouseButton::Right.0, 1);
        assert_eq!(CGMouseButton::Center.0, 2);
    }

    /// Moves use the Dragged event types while one of this backend's own
    /// buttons is held (apps only recognize drags via those).
    #[test]
    fn move_event_type_follows_held_buttons() {
        assert_eq!(
            move_event_type(&[]),
            (CGEventType::MouseMoved, CGMouseButton::Left)
        );
        assert_eq!(
            move_event_type(&[MouseButton::Left]),
            (CGEventType::LeftMouseDragged, CGMouseButton::Left)
        );
        assert_eq!(
            move_event_type(&[MouseButton::Right]),
            (CGEventType::RightMouseDragged, CGMouseButton::Right)
        );
        assert_eq!(
            move_event_type(&[MouseButton::Middle]),
            (CGEventType::OtherMouseDragged, CGMouseButton::Center)
        );
        assert_eq!(
            move_event_type(&[MouseButton::Middle, MouseButton::Left]),
            (CGEventType::LeftMouseDragged, CGMouseButton::Left)
        );
    }

    /// Scroll sign parity with the physical path: translating the shared
    /// `helpers::map_scroll` (enigo convention) through enigo's macOS
    /// scroll_unit (axis1 = -length vertical, axis2 = -length horizontal)
    /// must agree with this table for every direction — the virtual target
    /// scrolls the same way the physical one does.
    #[test]
    fn scroll_mapping_matches_the_enigo_convention() {
        for direction in [
            ScrollDirection::Up,
            ScrollDirection::Down,
            ScrollDirection::Left,
            ScrollDirection::Right,
        ] {
            let clicks = 3;
            // helpers::map_scroll: (enigo axis, signed length).
            let (_, length) = map_scroll(direction, clicks);
            // enigo scroll_unit: Vertical → axis 1, delta = -length;
            // Horizontal → axis 2, delta = -length.
            let (enigo_axis, enigo_delta) = match map_scroll(direction, clicks).0 {
                enigo::Axis::Vertical => (1, -length),
                enigo::Axis::Horizontal => (2, -length),
            };
            assert_eq!(
                map_cg_scroll(direction, clicks),
                (enigo_axis, enigo_delta),
                "scroll mapping for {direction:?} must agree with the enigo convention"
            );
        }
        // Bound: clicks are clamped like the physical path.
        assert_eq!(map_cg_scroll(ScrollDirection::Up, u32::MAX).1, 100);
        assert_eq!(map_cg_scroll(ScrollDirection::Down, u32::MAX).1, -100);
    }

    /// Geometry predicates used by the point hit-test.
    #[test]
    fn hit_test_geometry() {
        assert!(rect_contains(0.0, 0.0, 100.0, 50.0, 0, 0));
        assert!(rect_contains(0.0, 0.0, 100.0, 50.0, 99, 49));
        assert!(
            !rect_contains(0.0, 0.0, 100.0, 50.0, 100, 0),
            "right edge is exclusive"
        );
        assert!(
            !rect_contains(0.0, 0.0, 100.0, 50.0, 0, 50),
            "bottom edge is exclusive"
        );
        assert!(!rect_contains(10.0, 10.0, 100.0, 50.0, -1, 10));
        assert!(rect_intersects(0.0, 0.0, 10.0, 10.0, 5.0, 5.0, 10.0, 10.0));
        assert!(
            !rect_intersects(0.0, 0.0, 10.0, 10.0, 10.0, 0.0, 10.0, 10.0),
            "touching"
        );
        assert!(!rect_intersects(0.0, 0.0, 10.0, 10.0, 20.0, 20.0, 5.0, 5.0));
        let window = HitWindow {
            pid: 42,
            x: 100.0,
            y: 0.0,
            width: 50.0,
            height: 40.0,
            normal_level: true,
            owner_name: None,
        };
        assert!(window.contains(120, 10));
        assert!(!window.contains(20, 10));
        // Overlaps the virtual display parked right of a 1440-wide main.
        let vd = CGRect {
            origin: CGPoint { x: 1440.0, y: 0.0 },
            size: CGSize {
                width: 1280.0,
                height: 800.0,
            },
        };
        let overlapping = HitWindow {
            pid: 42,
            x: 1500.0,
            y: 0.0,
            width: 50.0,
            height: 40.0,
            normal_level: true,
            owner_name: None,
        };
        assert!(overlapping.intersects(vd));
        // `HitWindow` is not `Copy` (owner_name), so each struct-update
        // consumes its base: chain far -> touching instead of branching
        // from `overlapping` twice.
        let far = HitWindow {
            x: 0.0,
            y: 0.0,
            ..overlapping
        };
        assert!(!far.intersects(vd));
        // Touching the VD's left edge is exclusive, mirroring rect_intersects.
        let touching = HitWindow { x: 1390.0, ..far };
        assert!(!touching.intersects(vd));
    }

    /// The hit-test's OWNERSHIP scope (the isolation contract, pinned pure):
    /// a window containing the point is targetable when its pid is one the
    /// session launched, or its window sits fully inside the display bounds;
    /// a window that merely INTERSECTS the parked bounds — the shape of a
    /// user window overhanging from the user's desktop — is never targeted,
    /// however close to the point, however big the overlap.
    #[test]
    fn pid_targeting_is_ownership_scoped() {
        let vd = CGRect {
            origin: CGPoint { x: 1440.0, y: 0.0 },
            size: CGSize {
                width: 1280.0,
                height: 800.0,
            },
        };
        let session_app = HitWindow {
            pid: 101,
            x: 1500.0,
            y: 10.0,
            width: 60.0,
            height: 40.0,
            normal_level: true,
            owner_name: None,
        };
        let overhanging_user_window = HitWindow {
            pid: 202,
            x: 1400.0,
            y: 0.0,
            width: 200.0,
            height: 100.0,
            normal_level: true,
            owner_name: None,
        };
        // Fixture sanity: the app is contained; the user window intersects
        // but is NOT contained (it hangs 40pt into the user's desktop).
        assert!(session_app.contained_in(vd));
        assert!(overhanging_user_window.intersects(vd));
        assert!(!overhanging_user_window.contained_in(vd));
        let point_inside_both = (1505, 12);
        let now = Instant::now();
        // Unknown pid + fully contained → targetable (covers the pid-0
        // discovery dead end: an app whose pid was never recovered stays
        // drivable while it lives on the display).
        assert!(targetable_at(&session_app, point_inside_both, vd, &[], now));
        // Fresh session pid → targetable even when the window is somewhere
        // transitional (the AX move has not relocated it yet).
        assert!(targetable_at(
            &overhanging_user_window,
            point_inside_both,
            vd,
            &[(202, now)],
            now
        ));
        // The isolation core: unknown pid + merely intersecting → never.
        assert!(!targetable_at(
            &overhanging_user_window,
            point_inside_both,
            vd,
            &[],
            now
        ));
        // The ownership exemption is DEADLINE-BOUND: past the grace the
        // session pid's window must sit inside the display like everyone
        // else's — a quit app's RECYCLED pid (whose new window lives on the
        // user's desktop) must not stay targetable forever.
        let stale = now - (SESSION_OWNERSHIP_GRACE + Duration::from_secs(1));
        assert!(!targetable_at(
            &overhanging_user_window,
            point_inside_both,
            vd,
            &[(202, stale)],
            now
        ));
        // ...while a contained window stays targetable after the grace.
        assert!(targetable_at(
            &session_app,
            point_inside_both,
            vd,
            &[(101, stale)],
            now
        ));
        // Overlay/high-level windows are never targetable either way.
        let overlay = HitWindow {
            pid: 303,
            x: 1500.0,
            y: 10.0,
            width: 60.0,
            height: 40.0,
            normal_level: false,
            owner_name: None,
        };
        assert!(!targetable_at(&overlay, point_inside_both, vd, &[], now));
        assert!(!targetable_at(
            &overlay,
            point_inside_both,
            vd,
            &[(303, now)],
            now
        ));
    }

    /// Launch-program classification: `.app` bundles (case-insensitive) go
    /// to `open -a`, plain executables exec directly, empties and NULs are
    /// rejected before they can panic std::process::Command.
    #[test]
    fn open_program_classification_and_validation() {
        assert!(is_app_bundle("/Applications/Safari.app"));
        assert!(is_app_bundle("/tmp/thing.APP"));
        assert!(!is_app_bundle("/usr/bin/ffmpeg"));
        assert!(!is_app_bundle("my.app.txt"));
        assert!(!is_app_bundle("appdir.app/binary"));
        assert!(validate_program("").is_err());
        assert!(validate_program("   ").is_err());
        assert!(validate_program("a\0b").is_err());
        assert_eq!(validate_program(" /usr/bin/top ").unwrap(), "/usr/bin/top");
    }

    /// Pid-discovery name preference: the program stem is the final path
    /// component minus a case-insensitive `.app`, and the owner-name match is
    /// case-insensitive containment; an empty stem and a missing owner name
    /// never match (no blanket "every newcomer looks confirmed").
    #[test]
    fn pid_discovery_program_matching() {
        assert_eq!(launched_program_stem("/Applications/Safari.app"), "safari");
        assert_eq!(launched_program_stem("Safari"), "safari");
        assert_eq!(launched_program_stem("/tmp/My.APP"), "my");
        assert_eq!(launched_program_stem("/opt/tools/thing"), "thing");
        assert_eq!(launched_program_stem("/weird/.app"), "");
        assert!(owner_matches_program(Some("Safari"), "safari"));
        assert!(owner_matches_program(Some("Google Chrome"), "chrome"));
        assert!(!owner_matches_program(Some("Finder"), "safari"));
        assert!(!owner_matches_program(None, "safari"));
        assert!(!owner_matches_program(Some("Safari"), ""));
    }

    /// The SHARED BGRA→RGBA row copy (the ScreenCaptureKit adapter's helper,
    /// now consumed by the capture path above too) is stride-correct — the
    /// same pinned bytes as that module's own test, one implementation for
    /// both stacks.
    #[test]
    fn bgra_to_rgba_handles_stride_and_channel_swap() {
        let src = [
            1u8, 2, 3, 255, 4, 5, 6, 255, 0xAA, 0xBB, 0xCC, 0xDD, //
            10, 20, 30, 255, 40, 50, 60, 255, 0x11, 0x22, 0x33, 0x44,
        ];
        let mut dst = vec![0u8; 16];
        super::super::super::screen_capture_kit::copy_bgra_to_rgba(&mut dst, &src, 2, 12, 2);
        assert_eq!(
            dst,
            [
                3, 2, 1, 255, 6, 5, 4, 255, //
                30, 20, 10, 255, 60, 50, 40, 255,
            ]
        );
    }

    /// Chord ordering: modifiers first, stable otherwise (the physical
    /// backend's rule, duplicated as a pure fn here).
    #[test]
    fn chord_keys_press_modifiers_first() {
        assert_eq!(
            ordered_chord_keys(&[Key::Char('s'), Key::Control]),
            vec![Key::Control, Key::Char('s')]
        );
        assert_eq!(
            ordered_chord_keys(&[Key::Char('h'), Key::Meta, Key::Char('u'), Key::Shift]),
            vec![Key::Meta, Key::Shift, Key::Char('h'), Key::Char('u')]
        );
        assert_eq!(
            ordered_chord_keys(&[Key::Shift, Key::Char('a')]),
            vec![Key::Shift, Key::Char('a')]
        );
    }

    /// The v1 element-targeting answers are explicit Unsupported (never a
    /// silent empty result), with model-facing guidance pointing at
    /// screenshots.
    #[test]
    fn element_targeting_is_explicitly_unsupported() {
        let tree = ui_tree_unsupported();
        let element = element_targeting_unsupported();
        for error in [tree, element] {
            assert!(
                matches!(error, ComputerUseError::Unsupported { .. }),
                "must be Unsupported, got {error}"
            );
            assert!(error.to_string().contains("screenshots"));
        }
    }

    /// The capabilities contract text: virtual target, experimental
    /// disclosure, the in-display guarantees (input/capture, verified
    /// targets), the launchd/best-effort-move open residual, the
    /// pid-posted-event fidelity caveat, and the honesty additions (no
    /// wind-down on stop this round; the open-pid discovery residual;
    /// cursor_position semantics). Pins the REAL strings via the shared
    /// helpers, so the disclosure cannot drift from what the model actually
    /// receives.
    #[test]
    fn capability_notes_disclose_the_target_contract() {
        let note = capability_target_note();
        // In-display guarantees... ("touching" not "inside": the
        // re-verification predicate is an intersection, and the copy says
        // so)
        assert!(note.contains("input stays on the virtual display"));
        assert!(note.contains("re-verified to still show a window touching it"));
        assert!(
            note.contains("a window overhanging into those bounds contributes its pixels"),
            "the capture is rect-scoped: an overhanging window's pixels reaching \
             the capture must be disclosed"
        );
        assert!(
            note.contains("joins a mirror set mid-session"),
            "the mirror-join residual (user pixels reach the capture, agent \
             content reaches the user's screen) must be disclosed"
        );
        assert!(note.contains("WebKit content processes"));
        assert!(note.contains("falls back to your desktop"));
        // ...and the honest open residual: launchd starts the app in the
        // user's session; the AX move is best-effort and FRONT-WINDOW-ONLY
        // (an app's other windows may stay outside).
        assert!(note.contains("launch into the user's graphics session"));
        assert!(note.contains("moved into the virtual display best-effort"));
        assert!(
            note.contains("FRONT window"),
            "the AX move relocates only the app's front window; the scope must \
             be disclosed"
        );
        assert!(note.contains("may stay visible on the user's desktop"));
        assert!(
            note.contains("one-minute ownership grace"),
            "the grace is 60 seconds — the note must not undersell it as \
             momentary"
        );
        // Apps stay after stop AND after the backend drop; the drop only
        // removes the display (surviving windows relocate to remaining
        // displays — nothing terminates them this round).
        assert!(note.contains("NOT closed when a session stops or when the backend is dropped"));
        assert!(note.contains("they keep running"));
        assert!(note.contains("their windows move to the remaining displays"));
        let notes = capability_notes();
        // cursor_position is the synthesized pointer (last move, else origin).
        assert!(notes.contains("cursor_position reports the last point this backend moved to"));
        assert!(notes.contains("top-left origin before the first move"));
        // The remembered-target re-verification and its recovery hint.
        assert!(notes.contains("re-verified to still show a window"));
        assert!(notes.contains("click inside the display to focus a new one"));
        // The open-pid discovery shape: overlap-only for unnamed windows,
        // name preferred (with the same-named-user-window residual stated),
        // pid 0 when nothing matches.
        assert!(notes.contains("watching for a new window on the virtual display"));
        assert!(notes.contains("only when their window already overlaps the display's bounds"));
        assert!(
            notes.contains("the USER opens during the brief search"),
            "the name-match residual (a user window can be claimed) must be disclosed"
        );
        assert!(notes.contains("attributed to the wrong app"));
        assert!(notes.contains("the pid is reported as 0"));
        // The grant contract is enforced now (input_pid preflight): the
        // note states the explicit error, not just "required".
        assert!(notes.contains("answers an explicit error without"));
        // The chord layout caveat: positional US codes vs the user's
        // system-wide layout, with the concrete misfire example.
        assert!(notes.contains("US-ANSI positional virtual-key codes"));
        assert!(notes.contains("YOUR current keyboard layout"));
        assert!(notes.contains("Cmd+A may act as Cmd+Q on AZERTY"));
    }
}
