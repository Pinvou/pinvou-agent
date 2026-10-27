//! Actual Windows host version and native architecture, without caller input.
//! The observation is data only and does not qualify an upgrade source.
use crate::error::{Error, Result};
use windows_sys::Win32::Foundation::{FreeLibrary, HANDLE, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::SystemInformation::{
    IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64, IMAGE_FILE_MACHINE_ARMNT,
    IMAGE_FILE_MACHINE_I386, IMAGE_FILE_MACHINE_UNKNOWN, OSVERSIONINFOEXW, OSVERSIONINFOW,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

// RtlGetVersion reports the running NT kernel version without depending on a
// process compatibility manifest. ntdll is a Windows system library.
#[allow(unsafe_code)]
#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetVersion(version: *mut OSVERSIONINFOW) -> i32;
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct HostFacts {
    pub(super) architecture: &'static str,
    pub(super) os_version: String,
    pub(super) workstation: bool,
}

fn native_architecture(process: u16, native: u16) -> Result<&'static str> {
    if !matches!(
        process,
        IMAGE_FILE_MACHINE_UNKNOWN
            | IMAGE_FILE_MACHINE_I386
            | IMAGE_FILE_MACHINE_AMD64
            | IMAGE_FILE_MACHINE_ARMNT
            | IMAGE_FILE_MACHINE_ARM64
    ) {
        return Err(Error::Unsupported);
    }
    match native {
        IMAGE_FILE_MACHINE_AMD64 => Ok("x86_64"),
        IMAGE_FILE_MACHINE_ARM64 => Ok("arm64"),
        _ => Err(Error::Unsupported),
    }
}

fn machine_result(success: bool, process: u16, native: u16) -> Result<()> {
    if !success {
        return Err(Error::SourceUnavailable);
    }
    native_architecture(process, native).map(|_| ())
}

type MachineFunction = unsafe extern "system" fn(HANDLE, *mut u16, *mut u16) -> i32;
struct SystemLibrary(HMODULE);
impl Drop for SystemLibrary {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0);
        }
    }
}

/// Resolve only the System32 kernel32 export. Missing library/export and a
/// failing call remain unavailable; there is no emulated-architecture fallback.
#[allow(unsafe_code)]
fn read_machine(api: Option<MachineFunction>) -> Result<(u16, u16)> {
    let api = api.ok_or(Error::SourceUnavailable)?;
    let (mut process, mut native) = (0u16, 0u16);
    let success = unsafe { api(GetCurrentProcess(), &mut process, &mut native) } != 0;
    machine_result(success, process, native)?;
    Ok((process, native))
}

#[allow(unsafe_code)]
fn observe_machine() -> Result<(u16, u16)> {
    let library_name: Vec<u16> = "kernel32.dll".encode_utf16().chain(Some(0)).collect();
    let raw = unsafe {
        LoadLibraryExW(
            library_name.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if raw.is_null() {
        return Err(Error::SourceUnavailable);
    }
    let library = SystemLibrary(raw);
    let export = unsafe { GetProcAddress(library.0, c"IsWow64Process2".as_ptr().cast()) };
    // SAFETY: the name identifies this documented Windows API and its exact ABI.
    let api = export.map(|function| unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, MachineFunction>(function)
    });
    read_machine(api)
}

fn project(
    process: u16,
    native: u16,
    platform: u32,
    major: u32,
    minor: u32,
    build: u32,
    product_type: u8,
) -> Result<HostFacts> {
    const VER_PLATFORM_WIN32_NT: u32 = 2;
    if platform != VER_PLATFORM_WIN32_NT || major == 0 || build == 0 {
        return Err(Error::SourceUnavailable);
    }
    let architecture = native_architecture(process, native)?;
    // VER_NT_WORKSTATION, VER_NT_DOMAIN_CONTROLLER and VER_NT_SERVER.
    // Preserve this observation for later target qualification; this module
    // does not itself certify that the host is supported.
    let workstation = match product_type {
        1 => true,
        2 | 3 => false,
        _ => return Err(Error::SourceUnavailable),
    };
    Ok(HostFacts {
        architecture,
        os_version: format!("{major}.{minor}.{build}"),
        workstation,
    })
}

/// Obtain an actual native observation; callers must still bind it to their
/// authenticated service incarnation and recheck before using it.
#[allow(unsafe_code)]
pub(super) fn capture() -> Result<HostFacts> {
    let mut version = OSVERSIONINFOEXW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOEXW>() as u32,
        ..Default::default()
    };
    // The extended Windows structure has the same prefix and is selected by
    // dwOSVersionInfoSize, allowing the native API to report product type.
    if unsafe { RtlGetVersion((&mut version as *mut OSVERSIONINFOEXW).cast()) } != 0 {
        return Err(Error::SourceUnavailable);
    }
    let (process, native) = observe_machine()?;
    project(
        process,
        native,
        version.dwPlatformId,
        version.dwMajorVersion,
        version.dwMinorVersion,
        version.dwBuildNumber,
        version.wProductType,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_projection_rejects_unknown_architecture_and_invalid_windows_data() {
        assert_eq!(
            project(
                IMAGE_FILE_MACHINE_UNKNOWN,
                IMAGE_FILE_MACHINE_AMD64,
                2,
                10,
                0,
                26100,
                1
            )
            .unwrap(),
            HostFacts {
                architecture: "x86_64",
                os_version: "10.0.26100".into(),
                workstation: true,
            }
        );
        assert_eq!(
            project(
                IMAGE_FILE_MACHINE_UNKNOWN,
                IMAGE_FILE_MACHINE_ARM64,
                2,
                10,
                0,
                26100,
                3
            )
            .unwrap()
            .architecture,
            "arm64"
        );
        assert!(
            !project(
                IMAGE_FILE_MACHINE_UNKNOWN,
                IMAGE_FILE_MACHINE_AMD64,
                2,
                10,
                0,
                26100,
                2
            )
            .unwrap()
            .workstation
        );
        assert_eq!(
            project(IMAGE_FILE_MACHINE_UNKNOWN, u16::MAX, 2, 10, 0, 26100, 1),
            Err(Error::Unsupported)
        );
        assert_eq!(
            project(
                IMAGE_FILE_MACHINE_UNKNOWN,
                IMAGE_FILE_MACHINE_AMD64,
                1,
                10,
                0,
                26100,
                1
            ),
            Err(Error::SourceUnavailable)
        );
        assert_eq!(
            project(
                IMAGE_FILE_MACHINE_UNKNOWN,
                IMAGE_FILE_MACHINE_AMD64,
                2,
                0,
                0,
                26100,
                1
            ),
            Err(Error::SourceUnavailable)
        );
        assert_eq!(
            project(
                IMAGE_FILE_MACHINE_UNKNOWN,
                IMAGE_FILE_MACHINE_AMD64,
                2,
                10,
                0,
                0,
                1
            ),
            Err(Error::SourceUnavailable)
        );
        assert_eq!(
            project(
                IMAGE_FILE_MACHINE_UNKNOWN,
                IMAGE_FILE_MACHINE_AMD64,
                2,
                10,
                0,
                26100,
                0
            ),
            Err(Error::SourceUnavailable)
        );
    }

    #[test]
    fn current_native_host_is_observed_without_environment_overrides() {
        let actual = capture().unwrap();
        assert!(matches!(actual.architecture, "x86_64" | "arm64"));
        let components: Vec<_> = actual.os_version.split('.').collect();
        assert_eq!(components.len(), 3);
        assert!(components.iter().all(|part| part.parse::<u32>().is_ok()));
    }

    #[test]
    fn native_machine_not_emulated_process_machine_controls_architecture() {
        assert_eq!(read_machine(None), Err(Error::SourceUnavailable));
        assert_eq!(
            machine_result(false, IMAGE_FILE_MACHINE_UNKNOWN, IMAGE_FILE_MACHINE_AMD64),
            Err(Error::SourceUnavailable)
        );
        assert_eq!(
            native_architecture(IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64),
            Ok("arm64")
        );
        assert_eq!(
            native_architecture(IMAGE_FILE_MACHINE_I386, IMAGE_FILE_MACHINE_AMD64),
            Ok("x86_64")
        );
        assert_eq!(
            native_architecture(IMAGE_FILE_MACHINE_UNKNOWN, IMAGE_FILE_MACHINE_AMD64),
            Ok("x86_64")
        );
        assert_eq!(
            native_architecture(u16::MAX, IMAGE_FILE_MACHINE_AMD64),
            Err(Error::Unsupported)
        );
        assert_eq!(
            native_architecture(IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_UNKNOWN),
            Err(Error::Unsupported)
        );
        assert!(
            project(
                IMAGE_FILE_MACHINE_UNKNOWN,
                IMAGE_FILE_MACHINE_AMD64,
                2,
                10,
                0,
                19044,
                1
            )
            .is_ok()
        );
        assert!(
            !project(
                IMAGE_FILE_MACHINE_UNKNOWN,
                IMAGE_FILE_MACHINE_AMD64,
                2,
                10,
                0,
                26100,
                3
            )
            .unwrap()
            .workstation
        );
    }
}
