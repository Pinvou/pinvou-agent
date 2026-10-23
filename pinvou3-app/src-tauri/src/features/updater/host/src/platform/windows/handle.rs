//! Single-owned kernel handles. Mutex ownership uses a separate thread-affine guard.
use crate::error::{Error, Result};
use std::ptr::NonNull;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
pub(super) struct Handle(NonNull<std::ffi::c_void>);
impl Handle {
    /// Adopt only a newly owned kernel handle returned by a successful SDK call.
    pub(super) fn owned(raw: HANDLE) -> Result<Self> {
        if raw == INVALID_HANDLE_VALUE {
            return Err(Error::Unavailable);
        }
        NonNull::new(raw).map(Self).ok_or(Error::Unavailable)
    }
    pub(super) fn raw(&self) -> HANDLE {
        self.0.as_ptr()
    }
}
// SAFETY: these are OS reference-counted kernel handles. APIs remain responsible
// for object-specific synchronization. A held mutex guard is deliberately !Send.
#[allow(unsafe_code)]
unsafe impl Send for Handle {}
#[allow(unsafe_code)]
unsafe impl Sync for Handle {}
impl Drop for Handle {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.raw());
        }
    }
}
