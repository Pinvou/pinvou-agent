//! Process-local search hardening for the TWO standalone roles only. This is
//! deliberately never called by the host library inside the GUI application.
use crate::error::{Error, Result};
#[allow(unsafe_code)]
pub(super) fn harden() -> Result<()> {
    use windows_sys::Win32::{
        System::LibraryLoader::{
            LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
            SetDefaultDllDirectories, SetDllDirectoryW,
        },
        System::{Environment::SetCurrentDirectoryW, SystemInformation::GetSystemDirectoryW},
    };
    let empty = [0u16];
    let mut system = [0u16; 32768];
    if unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32)
    } == 0
        || unsafe { SetDllDirectoryW(empty.as_ptr()) } == 0
    {
        return Err(Error::SecurityRefused);
    }
    let length = unsafe { GetSystemDirectoryW(system.as_mut_ptr(), system.len() as u32) };
    if length == 0
        || length as usize >= system.len()
        || unsafe { SetCurrentDirectoryW(system.as_ptr()) } == 0
    {
        return Err(Error::SecurityRefused);
    }
    Ok(())
}
