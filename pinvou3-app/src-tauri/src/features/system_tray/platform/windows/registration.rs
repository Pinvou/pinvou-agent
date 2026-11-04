//! Query the owned notification identity without discarding successful HRESULTs.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::{
    Foundation::RECT,
    UI::Shell::{NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect},
};
use windows_sys::core::GUID;

const ICON_NAMESPACE: uuid::Uuid = uuid::Uuid::from_u128(0x7b9e132d_ce54_4c84_ae2c_25fab417aa03);

pub(super) fn icon_guid(path: &OsStr) -> u128 {
    // Windows binds unsigned GUID icons to their executable path. Different
    // portable/dev paths get distinct identities; same-path launches retain
    // one identity. Hash native UTF-16 without lossy conversion or path logs.
    let bytes = path
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    uuid::Uuid::new_v5(&ICON_NAMESPACE, &bytes).as_u128()
}

fn registration_succeeded(status: i32) -> bool {
    // SUCCEEDED(hr), as in Microsoft's NotificationIcon sample. In particular,
    // S_FALSE is success too; an overflow icon need not produce exact S_OK.
    status >= 0
}

pub(super) fn registered(guid: u128) -> bool {
    let identifier = NOTIFYICONIDENTIFIER {
        cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
        guidItem: GUID::from_u128(guid),
        ..Default::default()
    };
    let mut rect = RECT::default();
    // SAFETY: both pointers refer to initialized, correctly sized native
    // structs for this call. The GUID identifies only our live owned icon.
    registration_succeeded(unsafe { Shell_NotifyIconGetRect(&identifier, &mut rect) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_registration_includes_overflow_success() {
        assert!(registration_succeeded(0)); // S_OK
        assert!(registration_succeeded(1)); // S_FALSE
        assert!(!registration_succeeded(0x80004005_u32 as i32)); // E_FAIL
        assert!(!registration_succeeded(0x80070005_u32 as i32)); // access denied
    }

    #[test]
    fn icon_identity_is_stable_per_native_executable_path() {
        let path = OsStr::new(r"C:\Apps\品悟\pinvou.exe");
        assert_eq!(icon_guid(path), icon_guid(path));
        assert_ne!(
            icon_guid(path),
            icon_guid(OsStr::new(r"D:\Portable\pinvou.exe"))
        );
    }

    /// Owns only a disposable test icon/window. It never restarts Explorer,
    /// changes notification settings, or constructs the actual application.
    #[test]
    #[ignore = "requires an interactive Explorer notification area"]
    fn shell_registration_failure_and_recovery_keep_close_safe() {
        use tray_icon::{Icon, TrayIconBuilder};
        use windows_sys::Win32::UI::{
            Shell::{NIF_GUID, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW},
            WindowsAndMessaging::{IsWindow, RegisterWindowMessageW, SendMessageW},
        };

        let guid = uuid::Uuid::new_v4().as_u128();
        assert!(!registered(guid));
        let icon = TrayIconBuilder::new()
            .with_guid(guid)
            .with_icon(Icon::from_rgba([120, 60, 220, 255].repeat(16 * 16), 16, 16).unwrap())
            .with_tooltip("Pinvou isolated registration test")
            .build()
            .unwrap();
        assert!(
            registered(guid),
            "interactive notification area is required"
        );
        let taskbar_created = "TaskbarCreated\0".encode_utf16().collect::<Vec<_>>();
        // SAFETY: terminated UTF-16 string; the returned registered message is
        // sent only to this test's helper HWND on its owning thread.
        let restart_message = unsafe { RegisterWindowMessageW(taskbar_created.as_ptr()) };
        assert_ne!(restart_message, 0);

        for _ in 0..2 {
            let data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                uFlags: NIF_GUID,
                guidItem: GUID::from_u128(guid),
                ..Default::default()
            };
            // SAFETY: initialized native struct identifies only our test icon.
            assert_ne!(unsafe { Shell_NotifyIconW(NIM_DELETE, &data) }, 0);
            // The owned native resource survives failed/absent registration.
            assert_ne!(unsafe { IsWindow(icon.window_handle()) }, 0);
            assert!(!registered(guid));
            assert_eq!(
                super::super::super::close_policy::close_to_tray::<()>(
                    "main",
                    registered(guid),
                    || panic!("an allocated resource is insufficient for hiding"),
                ),
                Ok(false)
            );
            unsafe { SendMessageW(icon.window_handle(), restart_message, 0, 0) };
            assert!(
                registered(guid),
                "same owned icon must recover registration"
            );
            assert_eq!(
                super::super::super::close_policy::close_to_tray("main", registered(guid), || {
                    Ok::<_, ()>(())
                }),
                Ok(true)
            );
        }
        drop(icon);
        assert!(!registered(guid));
    }
}
