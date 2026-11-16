//! macOS virtual display stack (private CGVirtualDisplay API, experimental
//! with one-time auto-fallback).
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
//!   `platform::create_backend`, a creation-time failure is an
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
//! window merely overhanging the parked bounds is never targeted). One
//! bounded exception: during the ~5 s `open` discovery budget, an unnamed
//! in-bounds NEWCOMER window can be claimed by intersects (not containment)
//! as a last resort — if that newcomer is the user's overhanging window it
//! is briefly drivable (disclosed in the operational notes). Keyboard
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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
    HOLD_CANCEL_POLL_MS, MACOS_UNICODE_STRING_UTF16_UNITS, MAX_OPEN_CHILDREN, MAX_SCROLL_CLICKS,
    MULTI_CLICK_INTERVAL_MS, TYPE_CHUNK_CHARS, TypeRun, combine_drag_errors, combine_hold_errors,
    drag_waypoints, logged_program, normalize_typed_newlines, open_capacity_rejected,
    ordered_chord_keys, split_type_runs, utf16_chunks,
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
/// display list (≤ 5 s budget; `CGGetActiveDisplayList` both refreshes
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
        // NSUInteger parameters: the explicit `as usize` keeps the ABI
        // zero-extension intent visible instead of resting on u32 register
        // writes (R9 review nit — untyped msg_send! does no encoding check).
        let _: () = msg_send![&*descriptor, setMaxPixelsWide: VD_WIDTH_PX as usize];
        let _: () = msg_send![&*descriptor, setMaxPixelsHigh: VD_HEIGHT_PX as usize];
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
            initWithWidth: VD_WIDTH_PX as usize,
            height: VD_HEIGHT_PX as usize,
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
/// `CGGetActiveDisplayList` call is REQUIRED here: it is the
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
        // Say so: an unreadable list reads as "no other displays", which
        // makes the park-time overlap verification pass vacuously — a
        // genuine overlap would be neither repaired nor reported.
        log::warn!(
            "[computer_use][virtual-mac] cannot read the active display list \
             (CGError {error:?}); overlap checks fall back to main-only and \
             pass vacuously"
        );
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
/// Delegates to the scalar form so the edge-exclusive rule lives in exactly
/// one place (the hit-test's [`rect_intersects`] and the placement checks
/// must never drift apart).
fn rects_intersect(a: CGRect, b: CGRect) -> bool {
    rect_intersects(
        a.origin.x,
        a.origin.y,
        a.size.width,
        a.size.height,
        b.origin.x,
        b.origin.y,
        b.size.width,
        b.size.height,
    )
}

/// The right edge of the union of every active display except `except`.
/// Parking beyond THIS edge — not merely beyond the main display's — is
/// what keeps the virtual display clear of the user's whole layout: in the
/// default macOS arrangement an external display sits exactly at
/// (main_width, 0), so parking at the main display's right edge would drop
/// the virtual display exactly on top of it.
fn union_right_edge(except: u32) -> f64 {
    let others = other_display_bounds(except);
    // No readable list: fall back to the main display alone.
    let fallback = CGDisplayBounds(CGMainDisplayID());
    union_right_edge_of(&others, fallback)
}

/// The pure shape behind [`union_right_edge`], extracted so the R2
/// regression (reverting to the main-only right edge — which dropped the
/// display onto an external monitor at `(main_width, 0)`) is
/// mutation-pinnable on the macOS leg instead of passing every suite
/// silently (R18). `main_bounds` answers only when `others` is empty.
fn union_right_edge_of(others: &[(u32, CGRect)], main_bounds: CGRect) -> f64 {
    if others.is_empty() {
        return main_bounds.origin.x + main_bounds.size.width;
    }
    others
        .iter()
        .map(|(_, bounds)| bounds.origin.x + bounds.size.width)
        .fold(f64::MIN, f64::max)
}

/// The first overlap the session-scope placement OWNS — a pair that both
/// intersects (edge-exclusive, via [`rects_intersect`]) and is this
/// placement's to repair or refuse:
/// - any pair involving the virtual display is owned (its own placement);
/// - a pair involving the main display is owned ONLY while the origin pin
///   is part of the placement (`pin_applied`): a main-involving overlap on
///   a placement that skipped the pin — a pre-existing mirror set, or any
///   of the user's own arrangements — predates the session and cannot be
///   repaired by a placement, so tripping the guard over it would only
///   churn a re-park on every capture (R11 M3; R12 M1).
///
/// The ownership test runs INSIDE the pair search, not as a filter on the
/// first intersecting pair: filtering afterwards let an earlier
/// user-vs-user pair shadow a later virtual-display pair, silently passing
/// an owned overlap (R12 M1). Pure so the park's applied-layout invariant
/// is pinnable without a display.
fn first_placement_owned_overlap(
    bounds: &[(u32, CGRect)],
    virtual_id: u32,
    pin_applied: bool,
) -> Option<((u32, CGRect), (u32, CGRect))> {
    let main = CGMainDisplayID();
    for (index, (id_a, a)) in bounds.iter().enumerate() {
        for (id_b, b) in &bounds[index + 1..] {
            let owned = *id_a == virtual_id
                || *id_b == virtual_id
                || (pin_applied && (*id_a == main || *id_b == main));
            if owned && rects_intersect(*a, *b) {
                return Some(((*id_a, *a), (*id_b, *b)));
            }
        }
    }
    None
}

/// Whether pinning the main display to the origin would OVERLAP one of the
/// other displays (R11 M3): on a main-not-at-origin arrangement — any
/// display sitting left of the menu-bar display — the unconditional pin
/// dropped the user's main display onto their other display for the whole
/// session, and the old applied check inspected only the virtual display's
/// bounds, so it could not see that. Pure so the decision is pinnable;
/// `others` must already exclude the main display itself (the pinned rect
/// otherwise intersects the main display's own current bounds when the
/// main display sits at the origin).
fn main_origin_pin_overlaps_others(main_size: CGSize, others: &[(u32, CGRect)]) -> bool {
    let pinned = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: main_size,
    };
    others
        .iter()
        .any(|(_, bounds)| rects_intersect(pinned, *bounds))
}

/// The in-bounds unnamed-newcomer fallback latch after one `open` discovery
/// pass (pure so the R10 shape is drivable — and pinnable — without live
/// CG). `sighted` is the pid of an in-bounds unnamed newcomer this pass
/// (`None` = none seen). A pid seen again keeps its FIRST sighting time (a
/// continuously visible candidate can still cross the name grace early); a
/// different pid restarts the clock on the newest evidence; a pass with NO
/// sighting resets the latch — a window seen once and then closed is never
/// returned as the owning pid (the R10 review's latch-first defect).
fn fallback_latch_after_pass(
    fallback: Option<(u32, Instant)>,
    sighted: Option<u32>,
) -> Option<(u32, Instant)> {
    match sighted {
        Some(pid) => match fallback {
            Some((existing, seen_at)) if existing == pid => Some((existing, seen_at)),
            _ => Some((pid, Instant::now())),
        },
        None => None,
    }
}

/// Parks the virtual display right of the union of ALL active displays with
/// a SESSION-scope display configuration (NEVER `kCGConfigurePermanently` —
/// but note what session scope actually means: `kCGConfigureForSession`
/// persists until the LOGIN session ends, so a process exit does NOT revert
/// the layout — the user's arrangement stays rearranged for the rest of the
/// login session, which is exactly what the user-facing disclosure states).
/// The virtual display goes right of every other
/// display's right edge, and the main display is pinned at (0,0) — but ONLY
/// when no other display already occupies the origin space (R11 M3: an
/// unconditional pin overlapped the user's own two displays on a
/// main-not-at-origin arrangement). The applied layout is VERIFIED free of
/// placement-owned overlaps — every pair checked against BOTH the geometry
/// and the ownership predicate ([`first_placement_owned_overlap`]) — before
/// returning: the central "the virtual bounds never overlap the user's
/// desktop" invariant is enforced, not assumed; any owned overlap is an
/// explicit error FROM THIS FUNCTION, and the overlapping session layout
/// STAYS APPLIED until a later placement succeeds (CG has already committed
/// it — the error reports, it does not roll back). At creation time the
/// caller logs it as a warning and continues (the display is fully
/// functional wherever it sits, and `ensure_parked` re-checks on every
/// capture — see the call-site comment), so a creation does not fail over a
/// placement the next capture would repair; the hit-test's containment
/// scope (see `pid_at_point`) keeps user windows outside the parked bounds
/// targeted regardless.
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
    // R11 M3: the origin pin applies ONLY when no OTHER display already
    // occupies the origin space. On a main-not-at-origin arrangement (a
    // display sitting left of the menu-bar display) the unconditional pin
    // dropped the user's main display onto their other display for the
    // whole session — invisible to the old applied check, which inspected
    // only the virtual display's bounds. Skipping the pin leaves every
    // display at the user's own arrangement; the placement is still
    // overlap-free by construction (the virtual display goes right of the
    // union's right edge, computed from those same real bounds). The
    // alternative shape — never touching main at all — needs native-
    // hardware verification and stays deferred; the pairwise applied check
    // below backstops this path either way.
    let main_size = CGDisplayBounds(main).size;
    let others_excluding_main: Vec<(u32, CGRect)> = other_display_bounds(display_id)
        .into_iter()
        .filter(|(id, _)| *id != main)
        .collect();
    let pin_main = !main_origin_pin_overlaps_others(main_size, &others_excluding_main);
    let main_result = if pin_main {
        // SAFETY: config is the live configuration from the successful
        // begin; `main` is the real main display id.
        unsafe { CGConfigureDisplayOrigin(config, main, 0, 0) }
    } else {
        CGError::Success
    };
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
        // SAFETY: a begun transaction is ended with Complete or Cancel — a
        // failed Complete is not documented to close it, so cancel
        // best-effort like the configure-failure leg above (R13: leaving
        // the transaction open would degrade every later re-park to the
        // warn path).
        let _cancel = unsafe { CGCancelDisplayConfiguration(config) };
        return Err(ComputerUseError::failed(format!(
            "CGCompleteDisplayConfiguration failed: CGError({})",
            complete.0
        )));
    }
    // Enforce the no-overlap invariant on the APPLIED layout — every pair
    // checked against geometry AND ownership in one search (R12 M1:
    // filtering the first intersecting pair afterwards let an earlier
    // user-vs-user pair shadow a later virtual-display pair). CG may clamp
    // or adjust origins, so trust only the post-configuration bounds. A
    // main-involving pair is owned only while this placement actually
    // applied the origin pin (`pin_main`) — with the pin skipped, a
    // main-involving overlap (a pre-existing mirror set, for one) is the
    // user's own arrangement and must not wedge the session (R12 M1).
    let mut applied: Vec<(u32, CGRect)> = other_display_bounds(display_id);
    applied.push((display_id, CGDisplayBounds(display_id)));
    if let Some(pair) = first_placement_owned_overlap(&applied, display_id, pin_main) {
        let ((id_a, bounds_a), (id_b, bounds_b)) = pair;
        return Err(ComputerUseError::unavailable(format!(
            "the applied session-scope layout still overlaps: display {id_a} and \
             display {id_b} ({bounds_a:?} vs {bounds_b:?}); the overlapping \
             session layout stays applied until a later placement succeeds"
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
///   asynchronously; we never force-wait for it, and no
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
    // The pid <= 0 guard its sibling predicates carry: a hypothetical
    // pid-0/None window must not become a target (posting to pid 0 is a
    // silent no-op, so the action would report success while nothing
    // received the event).
    window.pid > 0
        && window.normal_level
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
    on_screen_windows()
        .iter()
        .any(|window| pid_window_qualifies(pid, window, bounds, within_grace))
}

/// The pure decision inside [`pid_has_window_on_display`], extracted so the
/// grace/containment split is behaviorally pinnable on the macOS leg: a
/// window that merely INTERSECTS the display qualifies only within the
/// session's ownership grace; past the grace (or for an unknown pid) the
/// window must sit FULLY inside — reverting the predicate to bare
/// intersection let a recycled pid's window on the user's desktop receive
/// pid-posted input (the pre-R3 shape; until this pin the revert passed
/// every suite — R18). The `pid > 0` guard mirrors [`targetable_at`]'s.
fn pid_window_qualifies(pid: i32, window: &HitWindow, bounds: CGRect, within_grace: bool) -> bool {
    pid > 0
        && window.pid == pid
        && window.normal_level
        && (within_grace && window.intersects(bounds) || window.contained_in(bounds))
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
/// wildcard arm over `Key` (parity-tested exhaustive table).
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
/// build until it is mapped (the parity guarantee this table's exhaustive-match build enforces).
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
/// window's new position ("VD origin + 40px margin").
const WINDOW_MOVE_MARGIN_PX: f64 = 40.0;
/// Budget for finding the pid of an app launched via `open -a` (which
/// cannot report one) by watching for its window to appear.
const OPEN_PID_DISCOVER_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the launch path waits for the short-lived `/usr/bin/open`
/// helper's own exit before treating it as "slow, keep discovering". A
/// FAILED `open` (bad app name, missing bundle) exits in well under this.
const OPEN_EXIT_STATUS_BUDGET: Duration = Duration::from_secs(3);
/// How long an unconfirmed newcomer (owner name does not match the launched
/// program, but the window OVERLAPS the virtual display's bounds — the
/// discovery fallback's intersects-based exception, not strict containment)
/// is held while discovery keeps looking for the program's own window.
/// Bounded so the overall budget never extends past
/// [`OPEN_PID_DISCOVER_TIMEOUT`]; afterwards the first OVERLAPPING
/// newcomer wins (fully out-of-bounds windows — the user's own desktop
/// windows — are never claimed by the fallback; residual mis-attribution
/// among in-bounds windows is disclosed in `Capabilities::notes`).
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
    /// pins the same way). NOT a strict mirror of `held_buttons`: a
    /// single atomic `click` pushes/pops `held_buttons` alone — EXCEPT a
    /// click whose compensating release failed twice, which records the
    /// pid here so a later `mouse_up` targets the stranded press instead
    /// of re-hit-testing the stale cursor point. Otherwise the pid list
    /// only records presses that outlive their action
    /// (`mouse_down`/`drag`).
    held_pids: Vec<(MouseButton, i32)>,
    /// Cancel flag set after the caller times out (see the trait contract).
    cancel: Option<Arc<AtomicBool>>,
    /// Set when the session-scope parking could not be applied: at
    /// CREATION the latch keys on the OWNED-OVERLAP error kind (a pure CG
    /// API failure says nothing about the layout and must not latch — the
    /// R12/R14 rationale), while a capture-time re-park latches on the
    /// trip predicate's MEASURED overlap regardless of the repair error's
    /// kind (R16: the evidence and the failure can arrive together).
    /// Either way `capabilities` appends the honest overlap disclosure to
    /// the target note instead of leaving the failure log-only (R8
    /// minor). Plain field — never shared outside the backend.
    park_failed: AtomicBool,
    /// How many OWNED-OVERLAP placement failures this session has latched
    /// (creation included). The note's appended disclosure names the count
    /// once it recurs (`park_failures` > 1), so each recurrence changes the
    /// note CONTENT — the tool layer's late-delta channel is
    /// content-diffed since R15, and a deterministic byte-identical append
    /// (the pre-R15 shape) could not surface a second failure after a
    /// successful repair had shrunk and regrown the note to its previously
    /// rendered length.
    park_failures: AtomicU32,
    /// Whether the previous repair attempt failed. Episode marker for the
    /// recurrence count: `ensure_parked` runs on every capture, so a
    /// sustained collision re-attempts the repair each time — counting
    /// every attempt changed the note content on every action and flooded
    /// the late-delta channel with near-identical warnings (R16). The
    /// count moves only when this flag transitions false→true (a failure
    /// after a success, or the first failure after creation); a successful
    /// repair clears it, so the next latched failure is a new episode.
    last_repair_failed: AtomicBool,
    /// Total apps this session has launched through [`Self::open_app`];
    /// launches past [`MAX_OPEN_CHILDREN`] are refused with a count-only
    /// error. Opened apps deliberately outlive the session (disclosed), so
    /// unlike the Linux stack this counts the session's whole launch
    /// budget — a runaway open loop must hit the same wall on every stack.
    opened_app_count: usize,
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
    /// failure ⇒ the "last interacted" claim is no longer truthful).
    /// Targeting failures — including the "no application
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

    /// Session-scope placement guard, piggybacked on every capture): if the virtual display somehow became the main display, or got
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
        // R11 M3 + R12 M1: the trip predicate runs every pair through
        // geometry AND ownership in one search. A main-involving pair trips
        // only while the origin pin is part of the placement (pinning is
        // SAFE for the current layout — a re-park would apply it and repair
        // the overlap); with the pin unsafe (a pre-existing mirror set, a
        // main-not-at-origin arrangement) a re-park skips the pin, so a
        // main-involving overlap is the user's own arrangement, not
        // repairable, and must not trip — that would churn a re-park on
        // every capture without ever repairing anything (R12 M1).
        //
        // R16: that rule lives INSIDE the search's ownership arm
        // (`pin_applied` gates exactly the main-involving pairs), so the
        // trip layout must still CONTAIN the main display — the filtered
        // vec the pin predicate needs is not the trip layout. Reusing it
        // here used to make the ownership arm dead at this call site: a
        // main-vs-virtual overlap (the user moving their main display onto
        // the parked region, or a hotplug landing there as the new main)
        // never tripped, never re-parked, never latched the disclosure —
        // the same hole the park path's applied check never had (it feeds
        // the UNFILTERED bounds in).
        let main = CGMainDisplayID();
        let pin_scope: Vec<(u32, CGRect)> = other_display_bounds(self.display_id)
            .into_iter()
            .filter(|(id, _)| *id != main)
            .collect();
        let pin_applied = !main_origin_pin_overlaps_others(CGDisplayBounds(main).size, &pin_scope);
        let mut trip_layout: Vec<(u32, CGRect)> = other_display_bounds(self.display_id);
        trip_layout.push((self.display_id, CGDisplayBounds(self.display_id)));
        let overlapped =
            first_placement_owned_overlap(&trip_layout, self.display_id, pin_applied).is_some();
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
                // Latch policy, restated (R16): the TRIP predicate's own
                // measurement is the evidence. When `overlapped` was
                // measured, the layout really does overlap and the
                // disclosure the flag drives is true — latch it even when
                // the repair attempt itself died on a transient CG API
                // failure (a `Failed` repair says nothing about the layout,
                // but the measurement already in hand says everything; the
                // old rule keyed the latch on the error kind and so
                // withheld a true disclosure whenever the evidence and the
                // failure arrived together). When the trip was the
                // became-main arm without a measured overlap, a repair
                // failure still says nothing about the layout and must not
                // latch (the R12 creation rationale, unchanged there too).
                //
                // The recurrence count moves at EPISODE granularity, not
                // per attempt: a sustained collision re-parks on every
                // capture, and counting each attempt changed the note on
                // every action, flooding the delta channel with
                // "(again — N …)" warnings. `last_repair_failed` admits
                // one increment per failure EPISODE — the count (and so
                // the fresh delta content the R15 fix needs) moves again
                // only after a repair succeeded in between.
                if overlapped && !self.last_repair_failed.swap(true, Ordering::Relaxed) {
                    self.park_failed.store(true, Ordering::Relaxed);
                    self.park_failures.fetch_add(1, Ordering::Relaxed);
                } else if overlapped {
                    // Sustained collision: keep the disclosure (it is
                    // already latched or this is its first frame after a
                    // trip without a completed repair) without growing the
                    // count.
                    self.park_failed.store(true, Ordering::Relaxed);
                }
            } else if !mirrored {
                // A successful repair clears the flag so the capability note
                // returns to the base truth instead of carrying the overlap
                // disclosure for the rest of the session (the appended
                // sentence's own "until a placement succeeds" framing is
                // what makes the clear truthful; the already-rendered delta
                // stays in the transcript — nothing retracts what the model
                // already read). It also closes the failure episode: the
                // NEXT latched failure counts as a new one and its note
                // content changes again. The mirrored branch keeps both
                // untouched: the mirror hazard is disclosed for as long as
                // the mirror exists.
                self.park_failed.store(false, Ordering::Relaxed);
                self.last_repair_failed.store(false, Ordering::Relaxed);
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
    /// [`OPEN_PID_DISCOVER_TIMEOUT`]. When NO already-running app holds a
    /// window fully inside the display, the answer is the honest
    /// "launched, owner unknown" `Ok(0)` — such an app cannot be driven
    /// reliably (its window is targetable only while it sits INSIDE the
    /// display's bounds — the targeting rules' contained-window arm, the
    /// same arm the discovery fallback uses; once the window closes or
    /// moves off the display nothing retargets it; clicks reach only
    /// windows fully inside the display), the same truth the
    /// capability notes disclose for the pid-0 receipt. When an
    /// already-running app DOES hold such a window, that app's pid is
    /// attributed instead (owner-name match preferred): a crashed launch
    /// is indistinguishable from an activate-only re-open and may be
    /// attributed to a pre-existing app, the residual the capability
    /// notes disclose.
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
            match self.discover_new_pid(&known_pids, program) {
                Some(pid) => Some(pid),
                None => {
                    // R16: no NEW window appeared within the budget — but
                    // this can be an activate-only re-open of an app that
                    // was ALREADY running with its window fully inside the
                    // virtual display (the discovery snapshot deliberately
                    // ignores pre-existing windows, so a re-open of a
                    // working app used to fall through to pid 0 and the
                    // receipt told the model the app cannot be driven —
                    // false: a fully contained window is targetable by the
                    // normal rules). Attribute the app to a pre-existing
                    // in-bounds window's owner instead and return that pid
                    // (the common path below then registers the ownership
                    // grace and focuses it, exactly like a fresh launch).
                    // The pid-0 path stays for the shapes where nothing
                    // in-bounds exists — an app whose windows live on the
                    // user's desktop, or a silent launch failure when no
                    // pre-existing in-bounds window can be attributed.
                    let bounds = CGDisplayBounds(self.display_id);
                    let stem = launched_program_stem(program);
                    // Same targeting predicates as everywhere else:
                    // normal-level windows only, fully inside the display.
                    // Name match preferred (strong identity, the same rule
                    // discovery uses — re-opening the back-most of two
                    // known apps must report THAT app, not the front-most);
                    // without a name match the front-most candidate is the
                    // best available answer. A launch that crashed after
                    // `open` exited 0 is indistinguishable from an
                    // activate-only re-open here and can be attributed to
                    // a pre-existing app — the residual the capability
                    // notes disclose.
                    let candidates: Vec<HitWindow> = on_screen_windows()
                        .into_iter()
                        .filter(|window| {
                            window.pid != own_pid
                                && known_pids.contains(&window.pid)
                                && window.normal_level
                                && rect_contained_in(
                                    window.x,
                                    window.y,
                                    window.width,
                                    window.height,
                                    bounds.origin.x,
                                    bounds.origin.y,
                                    bounds.size.width,
                                    bounds.size.height,
                                )
                        })
                        .collect();
                    let existing = candidates
                        .iter()
                        .find(|window| owner_matches_program(window.owner_name.as_deref(), &stem))
                        .or_else(|| candidates.first());
                    match existing {
                        Some(window) => {
                            log::info!(
                                "[computer_use:macos-vd] {} was already running; \
                                 its window is already inside the virtual display, \
                                 attributing the app to pid {} (activate-only \
                                 re-open)",
                                logged_program(program),
                                window.pid
                            );
                            Some(u32::try_from(window.pid).unwrap_or(0))
                        }
                        None => None,
                    }
                }
            }
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
                        "[computer_use:macos-vd] launched {} but no window appeared \
                         within the budget; attempting the window move anyway",
                        logged_program(program)
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
                "[computer_use:macos-vd] launched {} but no new application window \
                 on the virtual display appeared within the discovery budget; reporting \
                 pid 0",
                logged_program(program)
            );
            return Ok(0);
        };
        let pid_i32 = i32::try_from(pid).unwrap_or(0);
        if pid_i32 <= 0 {
            log::warn!(
                "[computer_use:macos-vd] launched {} but the discovered pid is \
                 unusable ({pid}); reporting pid 0",
                logged_program(program)
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
    /// intersects the virtual display's bounds. Display-level disjointness
    /// bounds the risk, but it is not a window-level guarantee: a USER
    /// window STRADDLING into the parked region does intersect and is
    /// claimable — the same in-bounds mis-attribution residual the
    /// capability notes disclose. Truly out-of-bounds windows (the user's
    /// own desktop windows) are never claimed. When only out-of-bounds
    /// unnamed windows appear within
    /// the budget the answer is None (pid 0 upstream: launched, owner
    /// unknown — interact via clicks; residual mis-attribution among
    /// IN-BOUNDS windows is disclosed in `Capabilities::notes`).
    fn discover_new_pid(&self, known: &[i32], program: &str) -> Option<u32> {
        let stem = launched_program_stem(program);
        let bounds = CGDisplayBounds(self.display_id);
        let deadline = Instant::now() + OPEN_PID_DISCOVER_TIMEOUT;
        // The name-unconfirmed IN-BOUNDS fallback: the answer once the
        // name-grace expires or the budget runs out — but only while it is
        // still OBSERVED. The R10 review found the latch-first shape
        // returning a window seen once and then closed: a dead pid reported
        // as "launched, pid N". The latch therefore drops the first pass
        // that no longer sees the candidate (falling out to the honest
        // None — the caller's pid-0 / no-window path), while a continuously
        // visible candidate keeps its first sighting time so the grace can
        // still expire and return it early.
        let mut fallback: Option<(u32, Instant)> = None;
        loop {
            let mut sighted_this_pass: Option<u32> = None;
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
                if window.intersects(bounds) {
                    sighted_this_pass = Some(pid);
                }
            }
            // The latch update is the pure `fallback_latch_after_pass`
            // (R11: pinnable without live CG), with the same semantics the
            // inline shape shipped.
            fallback = fallback_latch_after_pass(fallback, sighted_this_pass);
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
        // SAFETY: window_ptr is the BORROWED element from the array
        // (`value_at_index` is a Get rule — the array keeps ownership); the
        // +1 is taken by the `CFRetained::retain` below, which adopts it and
        // releases it exactly once on drop. Do NOT drop the retain as a
        // "duplicate": without it the element dies with the array guard.
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
     applies to it too; a pid that exits inside the grace can be recycled by \
     one of the user's own processes, which inherits the exemption while its \
     window touches the display); screenshots are captured from the virtual display's \
     own bounds, so a window overhanging into those bounds contributes its \
     pixels to the capture, and if the display joins a mirror set mid-session \
     the captures may show the user's desktop and agent content may be visible \
     on the user's screen until the mirror is undone (until then the input \
     targeting rules also cannot fully distinguish the user's windows inside \
     the mirrored geometry); the session-scope placement puts the virtual \
     display right of every other display and may move the user's main \
     display to the origin for the rest of the login session (it is left in \
     place when another display already occupies that space, and the applied \
     layout is verified overlap-free — a placement that cannot be applied \
     cleanly surfaces as the overlap disclosure below); events are delivered \
     directly to the target app \
     and may not reach every app (e.g. WebKit content processes on recent \
     macOS may ignore them); apps opened with open launch into the user's \
     graphics session and their FRONT window is moved into the virtual display \
     best-effort — an app's other windows may stay outside, a window larger \
     than the display cannot sit fully inside it (such an app becomes \
     undrivable once its one-minute ownership grace expires), and \
     without the Accessibility grant an opened app's window may stay visible \
     on the user's desktop — an app whose window never appears inside the \
     display at all is reported as pid 0, is left on the user's desktop \
     even with the grant, and cannot be driven (a directly launched \
     binary that keeps running without a window reports its real pid and \
     is equally undrivable; re-opening an already-running app attributes \
     its existing in-display window's owner so the app stays drivable \
     (owner-name match preferred; a crashed `open` launch can be \
     attributed to a pre-existing app)); \
     the session shares the \
     user account's clipboard with their real desktop (the general \
     pasteboard is per login session, not per display) — what the agent \
     copies or cuts inside an opened app reaches the user's real clipboard \
     and what the user holds on theirs reaches the session's apps; \
     if this macOS lacks the \
     private API the session \
     falls back to your desktop, decided once at session start; apps opened \
     in the virtual display are NOT closed when a session stops or when the \
     backend is dropped — they keep running, and when the virtual display \
     disappears their windows move to the remaining displays; when this app \
     exits, a virtual-display action wedged mid-gesture cannot be waited \
     out — that gesture's synthetic press may stay held down in the target \
     app (the exit-time release can only wait out healthy sub-second \
     releases)"
        .to_string()
}

/// The target note plus the park-failure disclosure. Pure so the test
/// module can pin the appended sentence (R9: the append previously had no
/// test); the tool layer's late-delta channel delivers it to the model on
/// the next action when the flag flips mid-session. `park_failures` names
/// the recurrence once it is above 1: the disclosure must CHANGE content on
/// every later failure (R15) — a byte-identical append could not surface a
/// second failure after a successful repair shrank the note back, because
/// the regrown note equaled the previously rendered snapshot and the delta
/// channel had nothing new to say.
fn capability_target_note_with(park_failed: bool, park_failures: u32) -> String {
    let mut note = capability_target_note();
    // A failed session-scope placement is more than a log line: the model
    // must know the display may overlap the user's screens (and what that
    // means for captures/targeting) instead of the failure staying
    // invisible (R8 minor).
    if park_failed {
        note.push_str(
            "; the session-scope display placement could not be \
             applied and re-parking has failed — the virtual display \
             may overlap the user's screens, so its captures can \
             mix in the user's windows and agent content can be \
             visible on the user's screen until a placement succeeds, \
             and while the placement is off the input targeting rules \
             also cannot fully distinguish the user's windows inside \
             the overlapped geometry (the same blindness as a mirror join)",
        );
        if park_failures > 1 {
            note.push_str(&format!(
                " (again — {park_failures} failed placement attempts this session; \
                 every recurrence means the layout keeps colliding)"
            ));
        }
    }
    note
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
     app just activates it and drops the arguments), an opened app shares \
     the account's clipboard with the user's real one (copies and pastes \
     cross over in both directions — the pasteboard is per login session, \
     not per display), and the pid \
     reported for such an app is recovered by watching for a new window on the \
     virtual display (owner-name match preferred — a same-named window the USER \
     opens during the brief search would be claimed and moved into the virtual \
     display; unnamed windows only when their window already overlaps the \
     display's bounds) — in rare cases an unrelated in-bounds window appearing \
     during that search is attributed to the wrong app, and when no matching \
     window appears the pid is reported as 0 — unless the app was already \
     running with a window fully inside the display (an activate-only \
     re-open, owner-name match preferred): then that window's owner pid is \
     reported and the app stays drivable — a launch that crashed after \
     `open` exited 0 cannot be told apart from such a re-open and can be \
     attributed to a pre-existing app. A pid-0 app cannot be driven at \
     all (even clicks only reach windows fully inside the display or this \
     session's apps), and its window stays wherever it opened — on the \
     user's desktop, with or without the Accessibility grant — until the \
     user moves or quits it themselves (a directly launched binary that \
     keeps running without a window reports its real pid and is equally \
     undrivable)"
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
            target_note: capability_target_note_with(
                self.park_failed.load(Ordering::Relaxed),
                self.park_failures.load(Ordering::Relaxed),
            ),
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
            Some(remembered) => {
                // The held-press fast path skips input_pid's hit-test/focus
                // resolution but NOT its Accessibility preflight: the TCC
                // contract covers EVERY input entry point, and a grant
                // revoked mid-gesture must fail the action instead of
                // reporting success while WindowServer drops the events.
                if !accessibility_granted() {
                    let error = accessibility_error();
                    self.clear_focus_on_error(&error);
                    return Err(error);
                }
                match self.reverified_target_pid(remembered) {
                    Ok(pid) => pid,
                    Err(error) => {
                        self.clear_focus_on_error(&error);
                        return Err(error);
                    }
                }
            }
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
                    // The press is stranded (the held_buttons entry stays):
                    // record the pid so a later `mouse_up` targets THIS pid
                    // instead of re-hit-testing the stale cursor point —
                    // the same contract as a manual `mouse_down` press.
                    if we_pressed {
                        self.held_pids.push((button, pid));
                    }
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
        // A repeated press of an already-held button used to post a second
        // down to a NEW pid while the held bookkeeping kept only the FIRST
        // pid: when that pid later died, the up consumed the entry against
        // the dead pid and the second pid kept a stranded press with no up
        // ever coming. A physical mouse cannot re-press a held button —
        // refuse the out-of-contract composition explicitly (the same
        // honesty as the trait's Unsupported default).
        if self.held_buttons.contains(&button) {
            return Err(ComputerUseError::unavailable(
                "that button is already held down from an earlier mouse_down; send mouse_up first",
            ));
        }
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
            Some(remembered) => {
                // Same Accessibility preflight as `move_to`'s held-press
                // fast path: a grant revoked mid-gesture must fail the
                // action, not report success while the events are dropped.
                if !accessibility_granted() {
                    let error = accessibility_error();
                    self.clear_focus_on_error(&error);
                    return Err(error);
                }
                match self.reverified_target_pid(remembered) {
                    Ok(pid) => pid,
                    Err(error) => {
                        self.clear_focus_on_error(&error);
                        // The press target is gone (app quit / window closed):
                        // no future `mouse_up` can ever deliver this up, so
                        // keeping the held entries would strand THIS button —
                        // every later `move_to`/`mouse_up` would keep failing
                        // against the dead remembered pid. Consume the held
                        // state here; the error still reports the real failure.
                        // First, a best-effort compensating up to the
                        // REMEMBERED pid: CGEventPostToPid delivers to the
                        // PROCESS, so an app whose window merely moved away
                        // still gets the release and clears its pressed-button
                        // state, while an app that quit simply fails the post
                        // (R13: the up used to be dropped entirely).
                        let (_, up, _) = cg_mouse_event_types(button);
                        let _ = self.post_mouse_to_pid(
                            remembered,
                            up,
                            cg_mouse_button(button),
                            dest,
                            1,
                        );
                        self.held_buttons.retain(|held| *held != button);
                        self.held_pids.retain(|(held, _)| *held != button);
                        return Err(error);
                    }
                }
            }
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
        // orphan release follows it. The move follows the held-button rule
        // (R16): when this drag BORROWS an already-held Left press, the
        // move rides the Dragged type like move_to's own rule (apps
        // recognize drags only via the Dragged types); a fresh drag moves
        // with MouseMoved.
        let (pre_type, pre_button) = move_event_type(&self.held_buttons);
        if let Err(error) = self.post_mouse_to_pid(drag_pid, pre_type, pre_button, from, 0) {
            self.clear_focus_on_error(&error);
            return Err(error);
        }
        self.cursor = Some(from);
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        // A failed DOWN is answered directly (the same discipline as click):
        // the old shape swallowed the press error and kept posting waypoints
        // and a release for a button that was never pressed — the action
        // could report a successful drag whose press never happened. The
        // failure goes through the same stale-target cleanup as every other
        // gesture error path.
        if let Err(error) = self.post_mouse_to_pid(drag_pid, down, left, from, 1) {
            self.clear_focus_on_error(&error);
            return Err(error);
        }
        // Same `we_pressed` discipline as `click`: when a manual Left press
        // is already held, the drag borrows the button WITHOUT pushing its
        // own bookkeeping entry — so the release below must not wipe the
        // manual press's entry (it used to retain-clear ALL Left entries,
        // destroying the original press's pid and stranding its press with
        // no up ever coming).
        let we_pressed = !self.held_buttons.contains(&MouseButton::Left);
        if we_pressed {
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
            // Only the drag's own bookkeeping entry goes: with a manual
            // press still held (`we_pressed == false`) the up cancelled the
            // drag's own down, and the manual press's pid entry must
            // survive it.
            if we_pressed {
                self.held_buttons.retain(|held| *held != MouseButton::Left);
                self.held_pids
                    .retain(|(held, _)| *held != MouseButton::Left);
            }
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
                // Same cleanup discipline as the chord/drag cancel paths:
                // the error flows through the stale-target helper (worst
                // case it drops a still-valid focus pid and the next action
                // re-resolves — never a stranded entry).
                let error = ComputerUseError::unavailable(
                    "type text was cancelled (caller timeout or stop); characters already \
                     injected are not undone",
                );
                self.clear_focus_on_error(&error);
                return Err(error);
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
                 app exits, so stop the session and start a new one for a \
                 fresh budget"
            )));
        }
        let pid = self.launch(program, args)?;
        // Counted on every successful launch (pid-0 outcomes included — the
        // process DID start; only its pid discovery failed).
        self.opened_app_count += 1;
        Ok(pid)
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
///   fallback (the shared one-time decision) — SCK alone would strand 11–15.1 with no
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
        // The closure holds the main thread for the build + the ≤1 s
        // id-read grace (see DISPLAY_ID_GRACE); when the id does not appear
        // within that grace it runs the legacy full activation poll HERE —
        // up to ACTIVATION_POLL_TIMEOUT (5 s) on the main thread, exactly
        // the pre-split freeze ceiling (the sibling DISPLAY_ID_GRACE doc
        // states the same trade truthfully).
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
    // wherever it currently sits — but the failure is recorded so the
    // capability note discloses the overlap risk instead of only logging it.
    let park_failed = AtomicBool::new(false);
    let mut park_failures = 0u32;
    if let Err(error) = park_right_of_main(display_id) {
        log::warn!("[computer_use:macos-vd] initial parking failed: {error}");
        // Only an OWNED-OVERLAP failure is evidence the applied layout
        // actually overlaps — the disclosure the flag drives describes
        // exactly that. A CG API failure says nothing about the layout
        // and must not latch the overlap disclosure (R12: the flag used
        // to flip on ANY park error, over-disclosing through pure API
        // failures).
        if matches!(error, ComputerUseError::Unavailable { .. }) {
            park_failed.store(true, Ordering::Relaxed);
            park_failures = 1;
        }
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
        park_failed,
        park_failures: AtomicU32::new(park_failures),
        last_repair_failed: AtomicBool::new(false),
        opened_app_count: 0,
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

    /// R7 pin: the CGRect-taking strict-intersection predicate used by the
    /// park-time overlap verification and the capture guard. It must stay
    /// EDGE-EXCLUSIVE (a display parked flush against another's right edge
    /// is adjacent, not overlapping) — a silently added `<=` would re-allow
    /// flush parking and the drift would be invisible to every other test.
    #[test]
    fn rects_intersect_is_edge_exclusive() {
        let main = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: 1440.0,
                height: 900.0,
            },
        };
        // Flush right of the main display: adjacent, NOT overlapping.
        let parked = CGRect {
            origin: CGPoint { x: 1440.0, y: 0.0 },
            size: CGSize {
                width: 1280.0,
                height: 800.0,
            },
        };
        assert!(
            !rects_intersect(main, parked),
            "flush parking must not count as overlap"
        );
        // Flush below: also adjacent.
        let below = CGRect {
            origin: CGPoint { x: 0.0, y: 900.0 },
            ..parked
        };
        assert!(!rects_intersect(main, below));
        // One pixel of real overlap: intersecting.
        let intruding = CGRect {
            origin: CGPoint { x: 1439.0, y: 0.0 },
            ..parked
        };
        assert!(rects_intersect(main, intruding));
        // Disjoint and contained shapes.
        let far = CGRect {
            origin: CGPoint {
                x: 5000.0,
                y: 5000.0,
            },
            ..parked
        };
        assert!(!rects_intersect(main, far));
        let inside = CGRect {
            origin: CGPoint { x: 10.0, y: 10.0 },
            size: CGSize {
                width: 5.0,
                height: 5.0,
            },
        };
        assert!(rects_intersect(main, inside));
    }

    /// R11 M3 + R12 M1, the applied-layout predicate: the park's
    /// verification and the placement guard search every pair through
    /// geometry AND ownership in ONE pass — the old shape took the first
    /// intersecting pair and THEN filtered by ownership, so an earlier
    /// user-vs-user pair shadowed a later virtual-display pair, and any
    /// main-involving pair (including a pre-existing mirror set the
    /// placement cannot repair) counted as owned.
    #[test]
    fn first_placement_owned_overlap_finds_owned_pairs_in_order() {
        // The predicate resolves the HOST's main id internally — build the
        // synthetic ids around it so no fixture id can collide with it (R16:
        // vd_id derives from the user ids too, so the three-way
        // distinctness holds on any host instead of relying on the host's
        // main id not being exactly 9).
        let host_main = CGMainDisplayID();
        let user_a_id = 101u32.max(host_main + 1);
        let user_b_id = 102u32.max(host_main + 2);
        let vd_id = 9u32.max(user_b_id + 1);
        assert_ne!(
            vd_id, host_main,
            "the synthetic vd id must not collide with the host's main id"
        );
        let rect = |x: f64, w: f64| CGRect {
            origin: CGPoint { x, y: 0.0 },
            size: CGSize {
                width: w,
                height: 800.0,
            },
        };
        // Clear layout: flush edges are adjacent, not overlapping — None
        // with the pin applied AND skipped.
        let main_at_origin = (host_main, rect(0.0, 1920.0));
        let flush_secondary = (user_a_id, rect(-1920.0, 1920.0));
        let vd_clear = (vd_id, rect(1920.0, 1280.0));
        assert_eq!(
            first_placement_owned_overlap(
                &[vd_clear, main_at_origin, flush_secondary],
                vd_id,
                true
            ),
            None
        );
        assert_eq!(
            first_placement_owned_overlap(
                &[vd_clear, main_at_origin, flush_secondary],
                vd_id,
                false
            ),
            None
        );
        // The R11 M3 shape with the pin APPLIED: pinned main [0,1920)
        // overlaps the intruding secondary [-960,960) — the pair is owned
        // through the pin and must be found (and refused).
        let intruding_secondary = (user_b_id, rect(-960.0, 1920.0));
        let pair = first_placement_owned_overlap(
            &[vd_clear, main_at_origin, intruding_secondary],
            vd_id,
            true,
        )
        .expect("the pin-caused overlap must be found");
        assert_eq!(
            (pair.0.0, pair.1.0),
            (host_main, user_b_id),
            "the offending pair is main-vs-secondary, not anything involving the vd"
        );
        // R12 M1 pin-skipped shape: the SAME main-vs-secondary overlap,
        // but the placement skipped the pin (a pre-existing mirror set,
        // for one) — the pair is NOT owned and must answer None so the
        // guard does not churn a re-park on every capture.
        assert_eq!(
            first_placement_owned_overlap(&[main_at_origin, intruding_secondary], vd_id, false),
            None,
            "a main-involving overlap with the pin skipped is the user's own \
             arrangement — never owned"
        );
        // R12 M1 shadowing shape: a USER-vs-USER pair FIRST in list order
        // (user_a [0,1000) vs user_b [500,1500)), an OWNED virtual-display
        // pair later (vd [1400,2680) overlaps user_b) — the old shape took
        // the first intersecting pair and filtered it away afterwards,
        // silently passing the owned overlap. The search must skip the
        // unowned pair and still find the owned one.
        let shadow_user_a = (user_a_id, rect(0.0, 1000.0));
        let shadow_user_b = (user_b_id, rect(500.0, 1000.0));
        let shadow_vd = (vd_id, rect(1400.0, 1280.0));
        let found =
            first_placement_owned_overlap(&[shadow_user_a, shadow_user_b, shadow_vd], vd_id, false)
                .expect("the owned vd pair must not be shadowed by the earlier user pair");
        assert_eq!(
            (found.0.0, found.1.0),
            (user_b_id, vd_id),
            "the found pair must be the owned user_b-vs-vd overlap, not the \
             earlier user-vs-user one"
        );
        // A vd-involving pair is owned regardless of the pin.
        let vd_overlapping_main = (vd_id, rect(0.0, 1280.0));
        assert!(
            first_placement_owned_overlap(&[main_at_origin, vd_overlapping_main], vd_id, false)
                .is_some()
        );
    }

    /// R11 M3: the origin pin is SKIPPED when another display already
    /// occupies the origin space — pinning there would drop the user's
    /// main display onto their other display (a legal main-not-at-origin
    /// arrangement). The pinned rect must NOT be compared against the main
    /// display's own bounds (that would self-intersect and skip the pin on
    /// every normal at-origin arrangement).
    #[test]
    fn main_origin_pin_skips_when_the_space_is_taken() {
        let main_size = CGSize {
            width: 1920.0,
            height: 1080.0,
        };
        // Secondary far left of the pinned space: pin safe.
        let left_secondary = (
            3u32,
            CGRect {
                origin: CGPoint { x: -1920.0, y: 0.0 },
                size: CGSize {
                    width: 1920.0,
                    height: 1080.0,
                },
            },
        );
        assert!(!main_origin_pin_overlaps_others(
            main_size,
            &[left_secondary]
        ));
        // Secondary sitting AT the origin (the display left of main): the
        // pin would overlap it — skip.
        let at_origin = (
            3u32,
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: 1920.0,
                    height: 1080.0,
                },
            },
        );
        assert!(main_origin_pin_overlaps_others(main_size, &[at_origin]));
        // The main itself at the origin (the normal arrangement): its own
        // bounds must NOT be in the list — the caller filters them — so an
        // empty others list stays pin-safe.
        assert!(!main_origin_pin_overlaps_others(main_size, &[]));
        // Adjacent-but-flush does not block the pin (edge exclusivity).
        let flush_right = (
            3u32,
            CGRect {
                origin: CGPoint { x: 1920.0, y: 0.0 },
                size: CGSize {
                    width: 1920.0,
                    height: 1080.0,
                },
            },
        );
        assert!(!main_origin_pin_overlaps_others(main_size, &[flush_right]));
        // A one-pixel intrusion into the pinned space blocks it.
        let intruding = (
            3u32,
            CGRect {
                origin: CGPoint { x: 1919.0, y: 0.0 },
                size: CGSize {
                    width: 100.0,
                    height: 100.0,
                },
            },
        );
        assert!(main_origin_pin_overlaps_others(main_size, &[intruding]));
    }

    /// R11 pin for the unnamed-fallback re-observe (the R10 fix had no
    /// pin on any leg): a pass with no sighting RESETS the latch (a window
    /// seen once and closed is never returned as the owning pid), a
    /// continuously visible candidate keeps its FIRST sighting time (so
    /// the name grace can still expire early), and a different pid
    /// restarts the clock on the newest evidence.
    #[test]
    fn fallback_latch_drops_a_closed_window_and_keeps_first_sighting() {
        // First sighting starts the clock.
        let first = fallback_latch_after_pass(None, Some(7));
        assert!(
            first.is_some(),
            "an in-bounds sighting latches the candidate"
        );
        // Same pid seen again: the FIRST sighting time is kept.
        let second = fallback_latch_after_pass(first, Some(7));
        assert_eq!(
            second, first,
            "a continuously visible candidate keeps its first sighting"
        );
        // The candidate's window closed: the latch drops — the honest None.
        assert_eq!(
            fallback_latch_after_pass(second, None),
            None,
            "a window seen once and closed must never be returned as the owning pid"
        );
        // A different pid restarts the clock on the newest evidence.
        let switched = fallback_latch_after_pass(first, Some(8));
        assert_ne!(switched, first, "a different pid must restart the clock");
        assert_eq!(switched.map(|(pid, _)| pid), Some(8));
    }

    /// R11 pin for the held-press fast paths (the R8 fix had no pin on any
    /// leg): every held-press fast path keeps the Accessibility preflight
    /// the normal input_pid path performs — a grant revoked mid-gesture
    /// must fail the action, not report success while WindowServer drops
    /// the events. Source-shape pin (the preflights are real TCC calls
    /// with no test seam): the guard shape must appear EXACTLY four times —
    /// `input_pid`'s main path, `move_to`'s held-press fast path,
    /// `mouse_up`'s held-press fast path, and the `capabilities` notes'
    /// live-grant disclosure. The R11 shape used `>= 3`, which tolerated
    /// deleting either held-press preflight (four sites minus one still
    /// satisfied it) — an exact count fails any single-site deletion
    /// (R12).
    #[test]
    fn held_press_fast_paths_keep_the_accessibility_preflight() {
        let prod = production_source();
        let count = prod.matches("if !accessibility_granted() {").count();
        assert!(
            count == 4,
            "the Accessibility preflight must cover the main input path, BOTH \
             held-press fast paths, and the capabilities notes' grant \
             disclosure — exactly 4 guard sites (found {count})"
        );
    }

    /// R11 pin: a successful re-park must CLEAR the park-failed flag so the
    /// overlap disclosure stops riding the capability note after the layout
    /// actually repaired (the R10 fix had no pin on any leg). Source-shape
    /// pin: the clear site lives in `ensure_parked`'s repaired branch next
    /// to the no-retraction comment, and the flag flip site stays.
    #[test]
    fn ensure_parked_clears_the_park_failed_flag_on_a_successful_repair() {
        let prod = production_source();
        // BOTH store sites are load-bearing (the R16 measured-overlap latch
        // arm AND the sustained-collision arm): a find-only pin guarded the
        // first site, so the latch arm was deletable while green.
        assert_eq!(
            prod.matches("self.park_failed.store(true, Ordering::Relaxed)")
                .count(),
            2,
            "the latch arm and the sustained-collision arm must both store \
             the flag — deleting either must fail"
        );
        let flip = prod
            .find("self.park_failed.store(true, Ordering::Relaxed)")
            .expect("the park-failure flip must exist");
        let clear = prod
            .find("self.park_failed.store(false, Ordering::Relaxed)")
            .expect("the successful-repair clear must exist");
        let no_retraction = prod
            .find("the already-rendered delta")
            .expect("the no-retraction rationale must stay with the clear");
        assert!(
            clear > flip,
            "the clear must live after the flip site (repair path follows the failure path)"
        );
        assert!(
            no_retraction > 0 && no_retraction < clear,
            "the no-retraction rationale must stay with the clear site (the \
             comment precedes the store in ensure_parked's repaired branch)"
        );
    }

    /// R12 M1 + R16 pins: the park verification and the ensure_parked trip
    /// must route through the combined geometry+ownership search — the
    /// pre-R12 shape selected the first intersecting pair and filtered by
    /// ownership afterwards, so an earlier user-vs-user pair shadowed a
    /// later owned pair (silent bypass), and any main-involving pair (a
    /// pre-existing mirror set the placement cannot repair) churned a
    /// re-park on every capture. R16 adds the trip LAYOUT contract: the
    /// search must run over the UNFILTERED display bounds (main included)
    /// — the pre-R16 caller reused the main-filtered vec the pin predicate
    /// needs, which made the ownership arm dead at the trip site and left
    /// a main-vs-virtual overlap untripped, unrepaired, and undisclosed.
    #[test]
    fn overlap_guard_uses_the_combined_owned_pair_search() {
        let prod = production_source();
        let park_site = prod
            .find("first_placement_owned_overlap(&applied, display_id, pin_main)")
            .expect("the park's applied check must search owned pairs directly");
        let trip_site = prod
            .find("first_placement_owned_overlap(&trip_layout, self.display_id, pin_applied)")
            .expect("ensure_parked's trip must search owned pairs directly");
        let pin_scope = prod
            .find("let pin_applied = !main_origin_pin_overlaps_others(")
            .expect("the trip must scope main-involving pairs to the safe-pin case");
        assert!(
            pin_scope < trip_site,
            "ensure_parked must derive the pin-applied scope BEFORE the trip search"
        );
        assert!(
            park_site > 0,
            "the park call site must exist (ordering is not load-bearing here)"
        );
        // R16 MAJOR: the trip layout itself must be built from the
        // UNFILTERED bounds. The filtered vec exists for the pin predicate
        // only (main_origin_pin_overlaps_others requires main excluded);
        // feeding it to the trip search used to exclude every
        // main-involving pair from the guard.
        let trip_layout_site = prod
            .find("let mut trip_layout: Vec<(u32, CGRect)> = other_display_bounds(self.display_id)")
            .expect(
                "ensure_parked's trip layout must be built from the unfiltered \
                 other_display_bounds (main included)",
            );
        let trip_layout_tail = &prod[trip_layout_site..trip_site];
        assert!(
            !trip_layout_tail.contains("filter("),
            "the trip layout must not filter the main display out — the \
             ownership arm inside the search already scopes main-involving \
             pairs to the safe-pin case (reverting to the filtered reuse \
             reintroduces the R16 MAJOR hole)"
        );
        // R16 latch/count policy: the latch keys on the TRIP predicate's
        // own measurement (not the repair's error kind), and the
        // recurrence count moves at episode granularity via the
        // last_repair_failed swap.
        let latch = prod
            .find("if overlapped && !self.last_repair_failed.swap(true, Ordering::Relaxed)")
            .expect(
                "the re-park latch must key on the measured overlap (a Failed \
                 repair must not withhold a measured disclosure), with the \
                 episode marker gating the count",
            );
        assert!(
            latch > trip_site,
            "the latch lives in ensure_parked's repair-failure arm, after the trip"
        );
        let episode_clear = prod
            .find("self.last_repair_failed.store(false, Ordering::Relaxed)")
            .expect("a successful repair must close the failure episode");
        assert!(
            episode_clear > latch,
            "the episode clear must live after the latch site (repair path \
             follows the failure path)"
        );
    }

    /// R12 pins for the held-button bookkeeping: the drag borrows a held
    /// Left press with click's `we_pressed` discipline (its release must
    /// only wipe its OWN entry, never the manual press's pid), and
    /// `mouse_down` refuses a re-press of an already-held button (the
    /// second down used to strand a press on a new pid that no up would
    /// ever reach once the first pid died and the up consumed the entry).
    #[test]
    fn held_button_bookkeeping_stays_scoped_to_its_own_press() {
        let prod = production_source();
        assert_eq!(
            prod.matches("let we_pressed = !self.held_buttons.contains(&MouseButton::Left);")
                .count(),
            1,
            "the drag's we_pressed borrow gate must exist (click's version \
             tests the generic `button`, not MouseButton::Left)"
        );
        assert_eq!(
            prod.matches("if we_pressed {").count(),
            5,
            "all five held-button gates (click's push, click's stranded-pid \
             record, click's retain, drag's push and retain) must sit \
             inside a we_pressed gate — an unconditional retain wipes a \
             borrowed manual press's bookkeeping"
        );
        assert!(
            prod.contains("already held down from an earlier mouse_down"),
            "mouse_down must refuse a re-press of an already-held button"
        );
    }

    /// Mutation pin (R11 M2, macOS leg): the model-supplied program name
    /// must render ONLY through `logged_program`'s Debug escaping — a
    /// plain `{program}` / `{program:?}` revert at the four launch logs
    /// would re-open the newline log-forgery channel. (R17: the count
    /// moves with the R16 activate-only attribution log — the pin was
    /// left at 3 and failed every macOS suite run.)
    #[test]
    fn recorded_program_names_never_render_plain_in_logs() {
        let prod = production_source();
        assert_eq!(
            prod.matches("logged_program(program)").count(),
            4,
            "all four launch logs must render the program through logged_program (exact sites)"
        );
        assert!(
            !prod.contains("launched {program:?}") && !prod.contains("launched {program}"),
            "the launch logs must not interpolate the model-supplied program name directly"
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
        let src = include_str!("macos_virtual.rs");
        let tests_start = src.find("mod tests {").expect("tests module marker");
        &src[..tests_start]
    }

    /// R18: the park bounds' two wiring contracts were pinned nowhere. (a)
    /// Both origin-pin scopes must exclude main from `others` — dropping
    /// either filter makes the pinned rect self-intersect main's own
    /// at-origin bounds, so `pin_main`/`pin_applied` go permanently false:
    /// the origin pin never applies on main-not-at-origin hosts even when
    /// safe, and main-involving owned overlaps stop tripping the R16 trip
    /// guard (the same hole through a different door). (b) The park bound
    /// rides the union of ALL displays through `union_right_edge_of` —
    /// reverting to the main-only right edge dropped the virtual display
    /// exactly onto an external monitor at `(main_width, 0)` (the R2
    /// major), which this behavioral pin makes visible again.
    #[test]
    fn park_bounds_keep_their_main_exclusions_and_union() {
        let prod = production_source();
        assert_eq!(
            prod.matches(".filter(|(id, _)| *id != main)").count(),
            2,
            "park_right_of_main's origin-pin scope AND ensure_parked's \
             pin_applied scope must both exclude main"
        );
        assert_eq!(
            prod.matches("union_right_edge_of(").count(),
            2,
            "definition + the wrapper call: a revert of union_right_edge to \
             a main-only computation drops the pure fn and fails here"
        );
        let mk = |x: f64, w: f64| CGRect {
            origin: CGPoint { x, y: 0.0 },
            size: CGSize {
                width: w,
                height: 800.0,
            },
        };
        let main = mk(0.0, 1440.0);
        // The R2 regression shape: an external display at (main_width, 0).
        let external = [(2u32, mk(1440.0, 1920.0))];
        assert_eq!(
            union_right_edge_of(&external, main),
            1440.0 + 1920.0,
            "the union must reach past the external display's right edge — \
             parking at main's 1440 would drop the display onto it"
        );
        // Main participates too (the wrapper feeds every other display
        // except the virtual one — main included).
        let with_main = [(1u32, main), (2u32, mk(1440.0, 1920.0))];
        assert_eq!(union_right_edge_of(&with_main, main), 1440.0 + 1920.0);
        // No readable list → the main-only fallback.
        assert_eq!(union_right_edge_of(&[], main), 1440.0);
        // A display left of main (negative coords) contributes but cannot
        // pull the edge left of main's own.
        let left = [(3u32, mk(-1920.0, 1920.0))];
        assert_eq!(union_right_edge_of(&left, main), 0.0);
    }

    /// R18: the grace/containment split inside `pid_has_window_on_display`
    /// (the remembered-target re-verification) was pinned nowhere —
    /// reverting the predicate to bare intersection (the pre-R3 shape its
    /// own doc says it closes: a recycled pid's window on the user's
    /// desktop receiving pid-posted input) passed every suite. Extracted
    /// into `pid_window_qualifies` and pinned here.
    #[test]
    fn pid_window_qualification_pins_the_grace_and_containment_split() {
        let vd = CGRect {
            origin: CGPoint { x: 1440.0, y: 0.0 },
            size: CGSize {
                width: 1280.0,
                height: 800.0,
            },
        };
        let mk = |pid: i32, x: f64, normal: bool| HitWindow {
            pid,
            x,
            y: 10.0,
            width: 60.0,
            height: 40.0,
            normal_level: normal,
            owner_name: None,
        };
        // Contained (fully inside) — qualifies in and out of grace.
        let contained = mk(101, 1500.0, true);
        assert!(pid_window_qualifies(101, &contained, vd, true));
        assert!(pid_window_qualifies(101, &contained, vd, false));
        // Overhanging (intersects vd, hangs into the user's desktop):
        // grace only.
        let overhanging = mk(202, 1400.0, true);
        assert!(pid_window_qualifies(202, &overhanging, vd, true));
        assert!(
            !pid_window_qualifies(202, &overhanging, vd, false),
            "past the grace a merely-intersecting window must NOT qualify — \
             the bare-intersection revert (the pre-R3 recycled-pid shape) \
             fails here"
        );
        // The pid > 0 guard (posting to pid 0 is a silent no-op that would
        // report success while nothing received the event).
        assert!(!pid_window_qualifies(0, &mk(0, 1500.0, true), vd, true));
        assert!(!pid_window_qualifies(-7, &mk(-7, 1500.0, true), vd, true));
        // A different pid's window never qualifies; a non-normal-level
        // window (e.g. a floating overlay) never qualifies.
        assert!(!pid_window_qualifies(101, &overhanging, vd, true));
        assert!(!pid_window_qualifies(
            101,
            &mk(101, 1500.0, false),
            vd,
            true
        ));
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
        // R18: the pid > 0 guard (a hypothetical pid-0/None-owned window
        // must never become a target — posting to pid 0 is a silent no-op
        // that would report success while nothing received the event). No
        // fixture had pid <= 0, so deleting the guard passed every leg.
        let pidless = HitWindow {
            pid: 0,
            x: 1500.0,
            y: 10.0,
            width: 60.0,
            height: 40.0,
            normal_level: true,
            owner_name: None,
        };
        assert!(!targetable_at(&pidless, point_inside_both, vd, &[], now));
        assert!(!targetable_at(
            &pidless,
            point_inside_both,
            vd,
            &[(0, now)],
            now
        ));
        let negative = HitWindow { pid: -7, ..pidless };
        assert!(!targetable_at(&negative, point_inside_both, vd, &[], now));
        // The ownership exemption is DEADLINE-BOUND: past the grace the
        // session pid's window must sit inside the display like everyone
        // else's — a quit app's RECYCLED pid (whose new window lives on the
        // user's desktop) must not stay targetable forever. checked_sub:
        // `Instant - Duration` panics on underflow, and `now` is close to
        // the boot epoch on a machine running this within ~61 s of boot.
        let stale = now
            .checked_sub(SESSION_OWNERSHIP_GRACE + Duration::from_secs(1))
            .expect("test clock sits at least the grace past the boot epoch");
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
        assert!(
            note.contains("cannot fully distinguish the user's windows inside"),
            "until the mirror is undone the input targeting rules are blinded \
             too — the residual must be disclosed"
        );
        // R11 M3: the session-scope placement may move the user's main
        // display for the rest of the login session, and the pin-skip /
        // pairwise verification shape must be stated, not silent.
        assert!(
            note.contains("may move the user's main display to the origin"),
            "the main-display origin pin (session-scope display rearrangement) \
             must be disclosed in the target note"
        );
        assert!(
            note.contains("left in place when another display already occupies"),
            "the pin-skip (main-not-at-origin arrangements) must be disclosed"
        );
        // R11 minor: the grace's pid-recycling variant — a recycled pid
        // inheriting the exemption — must be named, not just the grace.
        assert!(
            note.contains("recycled by one of the user's own processes"),
            "the ownership-grace pid-recycle residual must be disclosed"
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
        // R7: the oversized-window residual — the AX move sets position
        // only, so a window larger than the display can never sit fully
        // inside the display-bounds rule and becomes undrivable after the
        // grace. The note must say so.
        assert!(
            note.contains("undrivable once its one-minute ownership grace"),
            "the oversized-window consequence must be disclosed"
        );
        // R16: the pid-0 sentence must state BOTH exceptions to the
        // pid⇒drivable invariant — the windowless direct-exec sub-case
        // (real pid, undrivable) and the activate-only re-open attribution
        // (existing in-display window keeps the app drivable) — instead of
        // leaving "is reported as pid 0 … cannot be driven" as an
        // overclaim.
        assert!(
            note.contains("reports its real pid and is equally undrivable"),
            "the windowless direct-exec sub-case must be disclosed in the \
             target note: {note}"
        );
        assert!(
            note.contains("attributes")
                && note.contains("in-display window's owner so the app stays drivable"),
            "the activate-only re-open attribution must be disclosed in the \
             target note: {note}"
        );
        // Apps stay after stop AND after the backend drop; the drop only
        // removes the display (surviving windows relocate to remaining
        // displays — nothing terminates them this round).
        assert!(note.contains("NOT closed when a session stops or when the backend is dropped"));
        assert!(note.contains("they keep running"));
        assert!(note.contains("their windows move to the remaining displays"));
        // The app-exit strand (R10 M1): a worker wedged mid-gesture cannot
        // be waited out at app exit, so the gesture's synthetic press may
        // stay held in the target app — lib.rs's exit harvest documents the
        // strand as "disclosed by the macOS target note", so the note must
        // actually carry it (deleting the sentence must fail this pin).
        assert!(
            note.contains("wedged mid-gesture"),
            "the app-exit wedged-worker strand must be disclosed in the target \
             note — the exit-harvest comment and the PR body claim it is"
        );
        assert!(
            note.contains("synthetic press may stay held down"),
            "the surviving synthetic press (CGEventPostToPid presses outlive \
             our process) must be disclosed"
        );
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
        // R16: the re-open carve-out — an already-running app whose window
        // is fully inside the display is attributed (stays drivable) instead
        // of falling to pid 0; and the direct-exec windowless sub-case
        // (real pid, equally undrivable) is stated instead of leaving the
        // pid⇒drivable invariant with an unstated exception.
        assert!(
            notes.contains(
                "unless the app was already running with a window fully inside the display"
            ),
            "the activate-only re-open attribution must be disclosed: {notes}"
        );
        assert!(
            notes.contains("keeps running without a window reports its real pid"),
            "the windowless direct-exec sub-case must be disclosed: {notes}"
        );
        // The attribution is wired, not just worded: the open path must
        // actually scan pre-existing in-bounds windows before answering 0.
        let prod = production_source();
        assert!(
            prod.find("known_pids.contains(&window.pid)").is_some(),
            "the re-open attribution must scan the pre-launch snapshot's pids"
        );
        // The grant contract is enforced now (input_pid preflight): the
        // note states the explicit error, not just "required".
        assert!(notes.contains("answers an explicit error without"));
        // The chord layout caveat: positional US codes vs the user's
        // system-wide layout, with the concrete misfire example.
        assert!(notes.contains("US-ANSI positional virtual-key codes"));
        assert!(notes.contains("YOUR current keyboard layout"));
        assert!(notes.contains("Cmd+A may act as Cmd+Q on AZERTY"));
        // R9: the pid-0 launch sub-case — a no-match app is not just
        // "interact via clicks": it is undrivable AND its window stays on
        // the user's desktop with or without the Accessibility grant.
        assert!(
            notes.contains("cannot be driven at all"),
            "the pid-0 app's undrivable truth must be disclosed: {notes}"
        );
        assert!(
            notes.contains("with or without the Accessibility grant"),
            "the with-grant visibility residual must be disclosed: {notes}"
        );
    }

    /// R15 MAJOR-1: the macOS clipboard channel is consent-critical (the
    /// R9 Windows precedent — the hidden desktop's twin was a major there).
    /// Opened apps run in the user's login session and the general
    /// pasteboard is per-login-session, so copies and pastes cross over in
    /// both directions; both model-facing note strings must carry it, with
    /// the same facts the user-facing caveat states trilingually.
    #[test]
    fn macos_notes_disclose_the_clipboard_channel() {
        let note = capability_target_note();
        assert!(
            note.contains("shares the user account's clipboard"),
            "the target note must disclose the shared pasteboard: {note}"
        );
        assert!(
            note.contains("reaches the user's real clipboard"),
            "the target note must state the agent-to-user direction: {note}"
        );
        assert!(
            note.contains("what the user holds on theirs reaches the session's apps"),
            "the target note must state the user-to-agent direction: {note}"
        );
        let notes = capability_notes();
        assert!(
            notes.contains("clipboard"),
            "the operational notes must disclose the clipboard channel too: {notes}"
        );
        assert!(
            notes.contains("cross over in both directions"),
            "the operational notes must state both directions: {notes}"
        );
    }

    /// The park-failure disclosure rides the REAL note assembly: with the
    /// flag set the overlap sentence must be present (the tool layer's
    /// late-delta channel then delivers it to the model on the next
    /// action). R9: the append previously had no test.
    #[test]
    fn park_failure_appends_the_overlap_disclosure() {
        let base = capability_target_note_with(false, 0);
        assert!(
            !base.contains("overlap the user's screens"),
            "the healthy note carries no overlap disclosure"
        );
        let failed = capability_target_note_with(true, 1);
        assert!(failed.contains("may overlap the user's screens"));
        assert!(failed.contains("captures can mix in the user's windows"));
        // R12: the park-failure disclosure must carry the input-targeting
        // blindness too (the mirror-join disclosure already does) — while
        // the placement is off, the containment arm of the hit-test can
        // drive a user's window inside the overlapped bounds.
        assert!(
            failed.contains(
                "the input targeting rules also cannot fully distinguish the user's windows"
            ),
            "the park-failure disclosure must state the input-targeting \
             blindness, not only the capture mixing"
        );
        assert!(
            failed.starts_with(&base),
            "the disclosure is an append to the base note — the delta \
             channel in the tool layer relies on that shape"
        );
        // R15: a RECURRING failure must change the note content, not repeat
        // it byte-identically — the delta channel is content-diffed, and a
        // byte-identical regrown note gave the second failure nothing new
        // to deliver. Each count produces a distinct note (so episode 3
        // differs from episode 2 and re-surfaces), while the healthy note
        // and the first-episode note stay append-shaped.
        let again_2 = capability_target_note_with(true, 2);
        let again_3 = capability_target_note_with(true, 3);
        assert!(
            again_2.contains("again — 2 failed placement attempts this session"),
            "the recurring disclosure must name the attempt count: {again_2}"
        );
        assert!(
            again_3.contains("again — 3 failed placement attempts this session")
                && again_3 != again_2,
            "every recurrence must produce fresh note content: {again_3}"
        );
        assert!(
            again_2.starts_with(&base) && again_3.starts_with(&base),
            "the recurrence clause rides the append — the delta channel \
             still sees a prefix extension"
        );
        assert!(
            !base.contains("failed placement attempts"),
            "the healthy note must carry no recurrence clause"
        );
    }
}
