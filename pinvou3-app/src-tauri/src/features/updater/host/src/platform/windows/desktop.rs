//! The optional original-installer finish action asks the real ordinary desktop
//! shell to dispatch one fixed launcher. Dispatch is never child observation.
use super::{
    layout::ordinal_equal, os_image::Explorer, process::RetainedProcess,
    source::SourceInstallerContext as Source,
};
use crate::error::{Error, Result};
use std::{
    ptr,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use windows::{
    Win32::{
        System::{
            Com::{
                CLSCTX_DISABLE_AAA, CLSCTX_LOCAL_SERVER, CLSCTX_NO_CODE_DOWNLOAD,
                COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
                IDispatch, IServiceProvider,
            },
            Variant::VARIANT,
        },
        UI::Shell::{
            IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellWindows,
            SID_STopLevelBrowser, SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH, ShellWindows,
        },
    },
    core::{BSTR, Interface},
};
use windows_sys::Win32::{
    System::{
        StationsAndDesktops::{
            GetProcessWindowStation, GetThreadDesktop, GetUserObjectInformationW, UOI_NAME,
        },
        Threading::GetCurrentThreadId,
    },
    UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId},
};

/// Only this transient, already-Ready source helper may use the watchdog. It
/// owns no transaction/bootstrap mutex. A hung external COM server has an
/// unknown outcome; no redispatch or target rollback follows process exit.
struct OptionalDispatchBudget {
    signal: Arc<(Mutex<bool>, Condvar)>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl OptionalDispatchBudget {
    fn start() -> Result<Self> {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let signal = Arc::new((Mutex::new(false), Condvar::new()));
        let wait = signal.clone();
        let worker = std::thread::Builder::new()
            .name("host-desktop-budget".into())
            .spawn(move || {
                let (lock, changed) = &*wait;
                let Ok(done) = lock.lock() else { return };
                let Ok((done, _)) = changed.wait_timeout_while(
                    done,
                    deadline.saturating_duration_since(std::time::Instant::now()),
                    |done| !*done,
                ) else {
                    return;
                };
                if !*done {
                    // SAFETY: fixed unknown-result exit of this source helper only.
                    // No other process or installation state is modified.
                    #[allow(unsafe_code)]
                    unsafe {
                        windows_sys::Win32::System::Threading::ExitProcess(14);
                    }
                }
            })
            .map_err(|_| Error::OptionalLaunchUnavailable)?;
        Ok(Self {
            signal,
            worker: Some(worker),
        })
    }
}
impl Drop for OptionalDispatchBudget {
    fn drop(&mut self) {
        let (lock, changed) = &*self.signal;
        if let Ok(mut done) = lock.lock() {
            *done = true;
            changed.notify_one();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
struct Apartment;
impl Drop for Apartment {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
#[allow(unsafe_code)]
fn object_name(handle: *mut std::ffi::c_void) -> Result<String> {
    if handle.is_null() {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let mut needed = 0;
    unsafe {
        GetUserObjectInformationW(handle, UOI_NAME, ptr::null_mut(), 0, &mut needed);
    }
    if needed < 2 || needed > 512 || needed % 2 != 0 {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let mut name = vec![0u16; needed as usize / 2];
    let capacity = needed;
    if unsafe {
        GetUserObjectInformationW(
            handle,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            capacity,
            &mut needed,
        )
    } == 0
        || needed > capacity
    {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let length = name
        .iter()
        .position(|unit| *unit == 0)
        .ok_or(Error::OptionalLaunchUnavailable)?;
    if length == 0 || name[length + 1..].iter().any(|unit| *unit != 0) {
        return Err(Error::OptionalLaunchUnavailable);
    }
    String::from_utf16(&name[..length]).map_err(|_| Error::OptionalLaunchUnavailable)
}
#[allow(unsafe_code)]
fn desktop(
    source: &Source,
) -> Result<(
    windows_sys::Win32::Foundation::HWND,
    RetainedProcess,
    Explorer,
)> {
    let window = unsafe { GetShellWindow() };
    if window.is_null() {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let mut pid = 0;
    let thread = unsafe { GetWindowThreadProcessId(window, &mut pid) };
    if thread == 0 || pid == 0 {
        return Err(Error::OptionalLaunchUnavailable);
    }
    if !ordinal_equal(
        &object_name(unsafe { GetProcessWindowStation() })?,
        "WinSta0",
    )? || !ordinal_equal(
        &object_name(unsafe { GetThreadDesktop(GetCurrentThreadId()) })?,
        "Default",
    )? || !ordinal_equal(
        &object_name(unsafe { GetThreadDesktop(thread) })?,
        "Default",
    )? {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let shell = RetainedProcess::open(pid, None)?;
    let token = shell.fresh_token(None)?;
    // Standard-user UAC may use another administrator account. The actual
    // current desktop/session selects the shell user; installer SID is NOT
    // substituted for that ordinary interactive user's token.
    if token.user.session_id == 0
        || token.user.session_id != source.installer.token.user.session_id
        || token.elevated
        || token.administrator
        || token.app_container
        || token.network
        || !token.interactive
        || token.integrity != 0x2000
        || token.user.sid == "S-1-5-18"
    {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let explorer = Explorer::open()?;
    if !ordinal_equal(
        shell
            .image
            .to_str()
            .ok_or(Error::OptionalLaunchUnavailable)?,
        explorer
            .path()
            .to_str()
            .ok_or(Error::OptionalLaunchUnavailable)?,
    )? {
        return Err(Error::OptionalLaunchUnavailable);
    }
    Ok((window, shell, explorer))
}
/// Caller must already have authenticated the Ready registration and holds no
/// bootstrap mutex. `ready` repeats the same source/current/native-ready check
/// immediately before and after dispatch; no caller controls a COM target/path.
#[allow(unsafe_code)]
pub(super) fn dispatch(source: &Source, ready: impl Fn() -> Result<()>) -> Result<()> {
    ready()?;
    let _budget = OptionalDispatchBudget::start()?;
    let (window, shell, explorer) = desktop(source)?;
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .map_err(|_| Error::OptionalLaunchUnavailable)?;
    let _apartment = Apartment;
    let windows: IShellWindows = unsafe {
        CoCreateInstance(
            &ShellWindows,
            None,
            CLSCTX_LOCAL_SERVER | CLSCTX_DISABLE_AAA | CLSCTX_NO_CODE_DOWNLOAD,
        )
    }
    .map_err(|_| Error::OptionalLaunchUnavailable)?;
    let empty = VARIANT::default();
    let mut dispatch_window = 0i32;
    let desktop = unsafe {
        windows.FindWindowSW(
            &empty,
            &empty,
            SWC_DESKTOP,
            &mut dispatch_window,
            SWFO_NEEDDISPATCH,
        )
    }
    .map_err(|_| Error::OptionalLaunchUnavailable)?;
    if dispatch_window as u32 != window as usize as u32 {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let provider: IServiceProvider = desktop
        .cast()
        .map_err(|_| Error::OptionalLaunchUnavailable)?;
    let browser: IShellBrowser = unsafe { provider.QueryService(&SID_STopLevelBrowser) }
        .map_err(|_| Error::OptionalLaunchUnavailable)?;
    let view =
        unsafe { browser.QueryActiveShellView() }.map_err(|_| Error::OptionalLaunchUnavailable)?;
    let background: IDispatch = unsafe { view.GetItemObject(SVGIO_BACKGROUND) }
        .map_err(|_| Error::OptionalLaunchUnavailable)?;
    let folder: IShellFolderViewDual = background
        .cast()
        .map_err(|_| Error::OptionalLaunchUnavailable)?;
    let application =
        unsafe { folder.Application() }.map_err(|_| Error::OptionalLaunchUnavailable)?;
    let automation: IShellDispatch2 = application
        .cast()
        .map_err(|_| Error::OptionalLaunchUnavailable)?;
    ready()?;
    explorer.recheck()?;
    let (_, fresh_shell, _) = desktop_identity(source, window)?;
    if fresh_shell.selector != shell.selector || fresh_shell.token.user != shell.token.user {
        return Err(Error::OptionalLaunchUnavailable);
    }
    let launcher = source.layout.launcher_path();
    let path = launcher.to_str().ok_or(Error::OptionalLaunchUnavailable)?;
    let directory = source
        .layout
        .code
        .to_str()
        .ok_or(Error::OptionalLaunchUnavailable)?;
    unsafe {
        automation.ShellExecute(
            &BSTR::from(path),
            &VARIANT::from(BSTR::from("")),
            &VARIANT::from(BSTR::from(directory)),
            &VARIANT::from(BSTR::from("open")),
            &VARIANT::from(1i32),
        )
    }
    .map_err(|_| Error::HandoffOutcomeUnknown)?;
    // An external shell may have dispatched even when a later observation
    // fails. Preserve unknown; never retry or claim an observed child.
    if ready().is_err()
        || !shell.exited().is_ok_and(|exited| !exited)
        || explorer.recheck().is_err()
        || unsafe { GetShellWindow() } != window
    {
        return Err(Error::HandoffOutcomeUnknown);
    }
    Ok(())
}
#[allow(unsafe_code)]
fn desktop_identity(
    source: &Source,
    expected: windows_sys::Win32::Foundation::HWND,
) -> Result<(
    windows_sys::Win32::Foundation::HWND,
    RetainedProcess,
    Explorer,
)> {
    let value = desktop(source)?;
    if value.0 != expected {
        return Err(Error::OptionalLaunchUnavailable);
    }
    Ok(value)
}
