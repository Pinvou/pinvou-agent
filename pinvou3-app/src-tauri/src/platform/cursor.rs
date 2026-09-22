//! macOS-only shared CoreGraphics cursor access (computer_use input/capture
//! backend + pet tear-off drag polling). The cursor-read extern declarations
//! live here exactly once: duplicate `extern` blocks of the same symbols in
//! different modules must stay type-for-type identical within one crate or
//! the compiler warns (clashing_extern_declarations), so sharing them removes
//! the drift hazard instead of having to document it at every copy.

use std::ffi::c_void;

use objc2_foundation::NSPoint;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub(crate) fn CGEventCreate(source: *const c_void) -> *const c_void;
    pub(crate) fn CGEventGetLocation(event: *const c_void) -> NSPoint;
    /// The first parameter is a CGEventSourceStateID (int32 enum value, e.g.
    /// kCGEventSourceStateHIDSystemState = 1), NOT a CGEventSourceRef pointer:
    /// declaring it as a pointer made the arm64 ABI read the heap pointer's
    /// low 32 bits as the state id (an invalid enum value), so the call always
    /// returned false and macOS tear-off drag detection silently broke. The C
    /// prototype returns Boolean (unsigned char); declared as u8 and checked
    /// with `!= 0` (the same convention as the CFBoolean binding elsewhere).
    pub(crate) fn CGEventSourceButtonState(state_id: i32, button: u32) -> u8;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub(crate) fn CFRelease(cf: *const c_void);
}

/// Failure modes of [`cursor_position`], reported separately so callers can
/// phrase their own error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorPositionError {
    /// CGEventCreate returned null (window server failure).
    NullEvent,
    /// CGEventGetLocation returned non-finite coordinates.
    NonFinite,
}

/// Reads the cursor position in CGEvent global point coordinates (top-left
/// origin, negative values possible on multi-monitor layouts). Grant-free
/// (only event synthesis requires Accessibility), callable on any thread.
pub(crate) fn cursor_position() -> Result<(i32, i32), CursorPositionError> {
    // SAFETY: CGEventCreate accepts NULL (the default event source); after a
    // null check the returned event is only read for its coordinates;
    // CFRelease releases the successfully created object exactly once.
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return Err(CursorPositionError::NullEvent);
        }
        let loc = CGEventGetLocation(event);
        CFRelease(event);
        if !loc.x.is_finite() || !loc.y.is_finite() {
            return Err(CursorPositionError::NonFinite);
        }
        Ok((loc.x.round() as i32, loc.y.round() as i32))
    }
}

/// CGEventSourceStateID: kCGEventSourceStateHIDSystemState = 1. Reads the HID
/// hardware state (not this app's session state) so the pressed state is not
/// masked while the app itself captures events.
const HID_SYSTEM_STATE: i32 = 1;
/// CGMouseButton: kCGMouseButtonLeft = 0.
const MOUSE_BUTTON_LEFT: u32 = 0;

/// Whether the left mouse button is currently pressed, read straight from the
/// HID system state. Callable on any thread, grant-free.
pub(crate) fn left_button_down() -> bool {
    // SAFETY: callable on any thread without authorization; the first
    // parameter is the int32 enum value HID_SYSTEM_STATE, not a pointer.
    unsafe { CGEventSourceButtonState(HID_SYSTEM_STATE, MOUSE_BUTTON_LEFT) != 0 }
}
