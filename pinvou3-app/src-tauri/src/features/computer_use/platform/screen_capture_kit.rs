//! macOS ScreenCaptureKit static screenshot adapter (macOS 15.2+; preferred by the `macos`
//! backend when [`available`] is true).
//!
//! Why the migration: xcap 0.9.8's macOS static screenshot goes through
//! CGWindowListCreateImage — that API is obsoleted in the macOS 15 SDK ("Please use
//! ScreenCaptureKit instead"), and captures based on it trigger the periodic "continue to
//! allow screen recording" system confirmation on Sequoia+. ScreenCaptureKit is Apple's
//! designated replacement and shares the same Screen Recording TCC grant (the preflight
//! still uses CGPreflightScreenCaptureAccess, no change needed).
//!
//! The implementation uses [`SCScreenshotManager::captureImageInRect`]: it directly takes a
//! **global screen point rectangle** (the same space as `Capture.origin_x/y`'s
//! CGDisplayBounds), with monitor enumeration, content filtering, and pixel-size negotiation
//! all done by the system, so the ShareableContent → Filter → StreamConfiguration three-step
//! chain is unnecessary. The returned CGImage is BGRA with premultiplied alpha; this module
//! does a BGRA→RGBA row copy (including stride handling), and alpha is still set to 255 by
//! the caller in a single pass (consistent with the xcap path).
//!
//! Threading contract: the worker thread has no Obj-C runloop; the completion callback is
//! delivered on SCK's internal queue, so we wait on an mpsc channel, bounded by
//! [`CAPTURE_CALLBACK_TIMEOUT`]; a timeout is treated as failure and does not pin the worker
//! thread. The CGImage delivered by the callback is only valid during the block's execution,
//! so the BGRA→RGBA conversion happens inside the block and only `Vec<u8>` crosses the
//! channel (no CF/NS objects across threads).
//!
//! OS version gate: the app supports macOS 11 at minimum (Cargo.toml), while the static
//! screenshot API needs 15.2 (see the [`MIN_MACOS_VERSION`] comment: the class arrived in
//! 14.0, the class method only in 15.2); below that the caller falls back to xcap's
//! CGWindowList path.

use std::sync::mpsc::RecvTimeoutError;
use std::sync::{OnceLock, mpsc};
use std::time::Duration;

use block2::RcBlock;
use objc2_core_foundation::{CFRange, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGDataProvider, CGImage};
use objc2_foundation::{NSError, NSProcessInfo};
use objc2_screen_capture_kit::SCScreenshotManager;

use super::super::types::ComputerUseError;

/// The minimum OS version required by `SCScreenshotManager::captureImageInRect`.
///
/// Note this is not 14.0: the class itself arrived in macOS 14.0, but
/// `captureImageInRect:completionHandler:` is `API_AVAILABLE(macos(15.2))` (verified against
/// the SDK headers). If we admitted major version 14 only, on 14.0–15.1 we would send a
/// nonexistent selector to an already-validly-registered class → an Obj-C exception with no
/// landing pad → process abort. Below that (including 14.x) take xcap's CGWindowList
/// fallback path.
const MIN_MACOS_VERSION: (isize, isize) = (15, 2);

/// Per-capture wait ceiling for the completion callback. Deliberately far
/// below the backend's per-call budget (`BACKEND_CALL_TIMEOUT` in
/// `super::super::backend`); on timeout the capture fails (a late callback
/// sends into a disconnected channel and is silently dropped).
const CAPTURE_CALLBACK_TIMEOUT: Duration = Duration::from_secs(10);

/// Decide at runtime whether ScreenCaptureKit static screenshots are available on this
/// system (macOS >= 15.2).
///
/// The OS version is immutable per process, so the verdict is cached in a
/// `OnceLock`: this avoids a per-capture `NSProcessInfo::processInfo()` call
/// on the pool-less worker thread — that call returns an autoreleased
/// shared instance with no autorelease pool in sight, so caching also keeps
/// the autorelease-pool invariant documented in [`super::macos`] airtight.
pub(super) fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let version = NSProcessInfo::processInfo().operatingSystemVersion();
        satisfies_min_version(version.majorVersion, version.minorVersion)
    })
}

/// Pure-function form of the version comparison (so boundaries like 14.x/15.0/15.1 that
/// cannot be constructed on this machine can be unit tested). Field types match
/// NSOperatingSystemVersion (isize).
fn satisfies_min_version(major: isize, minor: isize) -> bool {
    (major, minor) >= MIN_MACOS_VERSION
}

/// Raw pixels of one screenshot (RGBA, 4 bytes per pixel, tightly packed rows without stride).
pub(super) struct CapturedScreen {
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Capture the global screen point rectangle `(origin_x, origin_y, width_pts, height_pts)`
/// (top-left origin, may span monitors / negative coordinates). Requires the Screen
/// Recording TCC grant (the caller preflights).
pub(super) fn capture_region(
    origin_x: i32,
    origin_y: i32,
    width_pts: i32,
    height_pts: i32,
) -> Result<CapturedScreen, ComputerUseError> {
    if width_pts <= 0 || height_pts <= 0 {
        return Err(ComputerUseError::failed(format!(
            "invalid capture region {width_pts}x{height_pts} at ({origin_x}, {origin_y})"
        )));
    }
    let (tx, rx) = mpsc::channel::<Result<CapturedScreen, String>>();
    let block = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let result = if image.is_null() {
            let detail = if error.is_null() {
                "no image and no error".to_string()
            } else {
                // SAFETY: the callback argument is non-null and valid for the block's
                // execution; code() is a pure read whose borrow does not outlive the block.
                format!("NSError code {}", unsafe { (*error).code() })
            };
            Err(format!("ScreenCaptureKit returned no image: {detail}"))
        } else {
            // SAFETY: image is non-null and valid for the block's execution (the strong
            // reference semantics of callback arguments are guaranteed by the Obj-C runtime);
            // the conversion does not touch the reference count.
            bgra_image_to_rgba(unsafe { &*image })
        };
        let _ = tx.send(result);
    });
    // SAFETY: the block is retained by SCK after the call returns (Obj-C block copy
    // semantics); dropping the local RcBlock only releases our own reference; rect is a pure
    // by-value argument.
    unsafe {
        SCScreenshotManager::captureImageInRect_completionHandler(
            CGRect {
                origin: CGPoint {
                    x: f64::from(origin_x),
                    y: f64::from(origin_y),
                },
                size: CGSize {
                    width: f64::from(width_pts),
                    height: f64::from(height_pts),
                },
            },
            Some(&block),
        );
    }
    match rx.recv_timeout(CAPTURE_CALLBACK_TIMEOUT) {
        Ok(Ok(shot)) => Ok(shot),
        Ok(Err(detail)) => Err(ComputerUseError::failed(format!(
            "ScreenCaptureKit capture failed: {detail}"
        ))),
        // Distinguish the two failure shapes (review finding): a timeout
        // means the completion handler never ran; a disconnected channel
        // means the block was released without sending — different faults,
        // and the old catch-all "timed out" misled diagnosis.
        Err(RecvTimeoutError::Timeout) => Err(ComputerUseError::failed(
            "ScreenCaptureKit capture timed out",
        )),
        Err(RecvTimeoutError::Disconnected) => Err(ComputerUseError::failed(
            "ScreenCaptureKit capture completed without a frame",
        )),
    }
}

/// CGImage (BGRA, premultiplied alpha) → tightly packed RGBA. The row copy handles stride
/// (the source row pitch bytes_per_row may exceed width*4), swapping the R/B channels
/// per pixel.
fn bgra_image_to_rgba(image: &CGImage) -> Result<CapturedScreen, String> {
    let (width, height, bytes_per_row, bits_per_pixel) = (
        CGImage::width(Some(image)),
        CGImage::height(Some(image)),
        CGImage::bytes_per_row(Some(image)),
        CGImage::bits_per_pixel(Some(image)),
    );
    if width == 0 || height == 0 {
        return Err(format!("empty CGImage {width}x{height}"));
    }
    if bits_per_pixel != 32 {
        return Err(format!(
            "unexpected CGImage bit depth {bits_per_pixel} (expected 32-bit BGRA)"
        ));
    }
    // Precondition of the row-copy slices (round-12 review): bytes_per_row >= width*4 is
    // CG's convention for 32bpp images; rows are sliced as `row*bpr .. row*bpr + width*4`,
    // and a violating image would panic inside the callback block — an unwind on a foreign
    // call stack is not protected by the worker's catch_unwind. The explicit check turns it
    // into an ordinary error.
    if bytes_per_row < width.checked_mul(4).ok_or("CGImage width overflows")? {
        return Err(format!(
            "CGImage row stride {bytes_per_row} < width*4 ({width}); not a packed 32-bit image"
        ));
    }
    let provider = CGImage::data_provider(Some(image))
        .ok_or_else(|| "CGImage has no data provider".to_string())?;
    // CGDataProvider::data copies out an independent CFData, managed by the CFRetained smart
    // pointer's +1 (dropped automatically releases it; this function must not CFRelease
    // manually).
    let data = CGDataProvider::data(Some(&provider))
        .ok_or_else(|| "CGDataProvider::data returned null".to_string())?;
    let expected = bytes_per_row
        .checked_mul(height)
        .ok_or_else(|| "CGImage row stride overflows".to_string())?;
    let len = data.length();
    if len < 0 || (len as usize) < expected {
        return Err(format!(
            "CGImage data too small: {len} bytes, need {expected}"
        ));
    }
    let mut raw = vec![0u8; len as usize];
    // SAFETY: the raw buffer holds len bytes; the range 0..len is fully covered and bytes
    // writes only that range.
    unsafe {
        data.bytes(
            CFRange {
                location: 0,
                length: len,
            },
            raw.as_mut_ptr(),
        )
    };
    let mut rgba = vec![0u8; width * height * 4];
    copy_bgra_to_rgba(&mut rgba, &raw, width, bytes_per_row, height);
    Ok(CapturedScreen {
        rgba,
        width,
        height,
    })
}

/// Row-copy BGRA→RGBA: `src` has a row pitch of `bytes_per_row` (may exceed `width*4`;
/// stride padding is discarded), `dst` is tightly packed `width*height*4`.
fn copy_bgra_to_rgba(
    dst: &mut [u8],
    src: &[u8],
    width: usize,
    bytes_per_row: usize,
    height: usize,
) {
    for row in 0..height {
        let src_row = &src[row * bytes_per_row..row * bytes_per_row + width * 4];
        let dst_row = &mut dst[row * width * 4..(row + 1) * width * 4];
        for (out, px) in dst_row.chunks_exact_mut(4).zip(src_row.chunks_exact(4)) {
            out[0] = px[2];
            out[1] = px[1];
            out[2] = px[0];
            out[3] = px[3];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_gate_requires_15_2() {
        // captureImageInRect is API_AVAILABLE(macos(15.2)): the class exists since 14.0,
        // but sending this selector on 14.0-15.1 aborts outright — those versions must take
        // the fallback.
        assert!(!satisfies_min_version(14, 0));
        assert!(!satisfies_min_version(14, 4));
        assert!(!satisfies_min_version(15, 1));
        assert!(satisfies_min_version(15, 2));
        assert!(satisfies_min_version(15, 7));
        assert!(satisfies_min_version(16, 0));
        // This machine (being able to run tests implies >= 11): available() must agree with
        // the real version.
        let version = NSProcessInfo::processInfo().operatingSystemVersion();
        assert_eq!(
            available(),
            satisfies_min_version(version.majorVersion, version.minorVersion)
        );
    }

    #[test]
    fn bgra_to_rgba_handles_stride_and_channel_swap() {
        // 2x2 image with row pitch 12 (4 bytes of padding beyond 2*4): BGRA→RGBA with
        // padding discarded.
        let src = [
            1u8, 2, 3, 255, 4, 5, 6, 255, 0xAA, 0xBB, 0xCC, 0xDD, //
            10, 20, 30, 255, 40, 50, 60, 255, 0x11, 0x22, 0x33, 0x44,
        ];
        let mut dst = vec![0u8; 16];
        copy_bgra_to_rgba(&mut dst, &src, 2, 12, 2);
        assert_eq!(
            dst,
            [
                3, 2, 1, 255, 6, 5, 4, 255, //
                30, 20, 10, 255, 60, 50, 40, 255,
            ]
        );
    }

    #[test]
    #[ignore = "live probe: needs Screen Recording TCC grant on a real desktop"]
    fn live_capture_region_returns_matching_pixel_buffer() {
        if !available() {
            return;
        }
        let shot = capture_region(0, 0, 800, 600).expect("live ScreenCaptureKit capture");
        assert!(shot.width > 0 && shot.height > 0);
        assert_eq!(shot.rgba.len(), shot.width * shot.height * 4);
        // A real desktop is never pure black (menu bar/wallpaper/windows: at least one
        // nonzero pixel).
        assert!(
            shot.rgba.chunks_exact(4).any(|px| px[..3] != [0, 0, 0]),
            "capture is entirely black"
        );
    }
}
