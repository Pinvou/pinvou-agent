//! Native same-object locking and CLI cancellation only. No path reopen,
//! certificate-store mutation, process launch or asynchronous orphaned verifier.
use crate::{Error, SignatureEvidence};
use std::{
    fs::File,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle},
    sync::atomic::{AtomicBool, Ordering},
};
use windows_sys::{
    Wdk::Storage::FileSystem::{FileFsDeviceInformation, NtQueryVolumeInformationFile},
    Win32::{
        Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_TYPE_DISK, GetFileType, ReOpenFile,
        },
        System::{
            Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler},
            IO::IO_STATUS_BLOCK,
        },
    },
};

mod provider;
mod wintrust;

// FILE_FS_DEVICE_INFORMATION (MS-FSCC): the SDK exposes the information class
// and Nt query but not this two-u32 result structure in windows-sys 0.61.2.
#[repr(C)]
#[derive(Default)]
struct DeviceInformation {
    device_type: u32,
    characteristics: u32,
}

#[allow(unsafe_code)]
pub(crate) fn lock(file: &File) -> Result<File, Error> {
    // SAFETY: an owned live File handle; ReOpenFile creates a new handle to the
    // SAME object. The fresh share-read reservation rejects existing writers
    // and deleters. Duplicating a handle would not change its share contract.
    let handle = unsafe {
        ReOpenFile(
            file.as_raw_handle(),
            GENERIC_READ,
            FILE_SHARE_READ,
            FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(match std::io::Error::last_os_error().raw_os_error() {
            Some(32 | 33) => Error::Busy,
            _ => Error::IoUnavailable,
        });
    }
    if handle.is_null() {
        return Err(Error::IoUnavailable);
    }
    // SAFETY: exactly one adoption of the successful new handle.
    let locked = unsafe { File::from_raw_handle(handle) };
    pinvou_upgrade_custody::identity(&locked)?;
    // SAFETY: same live file, exact fixed initialized native output buffer, and
    // synchronous IO status block. No name or user-provided device is queried.
    if unsafe { GetFileType(locked.as_raw_handle()) } != FILE_TYPE_DISK {
        return Err(Error::Unsupported);
    }
    let mut device = DeviceInformation::default();
    let mut io = IO_STATUS_BLOCK::default();
    let status = unsafe {
        NtQueryVolumeInformationFile(
            locked.as_raw_handle(),
            &mut io,
            (&mut device as *mut DeviceInformation).cast(),
            size_of::<DeviceInformation>() as u32,
            FileFsDeviceInformation,
        )
    };
    // FILE_DEVICE_DISK / FILE_DEVICE_DISK_FILE_SYSTEM; FILE_REMOTE_DEVICE is
    // explicitly refused. Network/mapped/special providers are not supported.
    if status < 0
        || io.Information != size_of::<DeviceInformation>()
        || !matches!(device.device_type, 7 | 8)
        || device.characteristics & 0x10 != 0
    {
        return Err(Error::Unsupported);
    }
    Ok(locked)
}
pub(crate) fn inspect(file: &File, cancel: &AtomicBool) -> Result<Vec<SignatureEvidence>, Error> {
    wintrust::inspect(file, cancel)
}

static CANCELLED: AtomicBool = AtomicBool::new(false);
static REGISTERED: AtomicBool = AtomicBool::new(false);
#[allow(unsafe_code)]
unsafe extern "system" fn cancel_handler(event: u32) -> i32 {
    if matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
        CANCELLED.store(true, Ordering::Release);
        1
    } else {
        0
    }
}
/// One CLI registration per process. Libraries instead use their own flag.
pub struct ConsoleCancellation;
impl ConsoleCancellation {
    #[allow(unsafe_code)]
    pub fn install() -> Result<Self, Error> {
        if REGISTERED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Error::Busy);
        }
        CANCELLED.store(false, Ordering::Release);
        // SAFETY: static callback with the Win32 ABI; no borrowed data captured.
        if unsafe { SetConsoleCtrlHandler(Some(cancel_handler), 1) } == 0 {
            REGISTERED.store(false, Ordering::Release);
            return Err(Error::IoUnavailable);
        }
        Ok(Self)
    }
    pub fn flag(&self) -> &AtomicBool {
        &CANCELLED
    }
}
impl Drop for ConsoleCancellation {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: unregister exactly this static handler before releasing slot.
        unsafe { SetConsoleCtrlHandler(Some(cancel_handler), 0) };
        REGISTERED.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
