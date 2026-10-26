//! Native pre-Tauri error presentation. No AppHandle, runtime, browser, log,
//! preference initialization, process launch or filesystem write is required.
#[allow(unsafe_code)]
pub(in crate::features::updater) fn show_startup_admission_failure(title: &str, message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TASKMODAL, MessageBoxW,
    };
    let title_wide: Vec<_> = title.encode_utf16().chain(Some(0)).collect();
    let message_wide: Vec<_> = message.encode_utf16().chain(Some(0)).collect();
    // The caller uses the immutable shared three-language catalog plus a fixed
    // nonsecret host error code. No received strings reach this modal surface.
    let shown = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message_wide.as_ptr(),
            title_wide.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TASKMODAL,
        )
    };
    if shown == 0 {
        eprintln!("{title}\n\n{message}");
    }
}
