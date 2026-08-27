use crate::platform::process::HiddenCommand;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::windows_path;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER,
    KEY_READ, REG_EXPAND_SZ, REG_SZ,
};

pub fn current_system_locale() -> Option<String> {
    let mut language_count = 0;
    let mut buffer_len = 0;
    // MSDN sizing call: null buffer with *pcchLanguagesBuffer = 0 returns TRUE
    // and stores the required size (trailing double NUL included). Still check
    // the return value so a failed call leaving buffer_len undefined cannot
    // trigger a bogus large allocation below.
    let sized = unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut language_count,
            std::ptr::null_mut(),
            &mut buffer_len,
        )
    };
    if sized == 0 || buffer_len <= 1 {
        return None;
    }
    let mut locale_names = vec![0u16; buffer_len as usize];
    let ok = unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut language_count,
            locale_names.as_mut_ptr(),
            &mut buffer_len,
        )
    };
    if ok == 0 || language_count == 0 {
        return None;
    }
    let first_len = locale_names.iter().position(|unit| *unit == 0)?;
    String::from_utf16(&locale_names[..first_len]).ok()
}

/// Probes process liveness with `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)`.
/// Access denied also means the process exists under another user or integrity level.
/// Browser watch uses this before removing a stale port file.
pub fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ACCESS_DENIED};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: This only queries existence, and every non-null handle is closed immediately.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if !handle.is_null() {
        unsafe {
            CloseHandle(handle);
        }
        return true;
    }
    let err = unsafe { GetLastError() };
    err == ERROR_ACCESS_DENIED
}

/// Windows user-profile ACLs provide the directory privacy boundary.
pub fn make_private_dir(_path: &Path) {}

pub fn open_target(target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    HiddenCommand::new("cmd")
        .args(["/C", "start", ""])
        .arg(target.as_ref())
        .spawn()
        .map_err(|e| format!("系统打开失败({label}): {e}"))?;
    Ok(())
}

pub fn reveal_target(target: &Path) -> Result<(), String> {
    let target = super::windows_path::platform_compat_path(&target.to_string_lossy());
    HiddenCommand::new("explorer.exe")
        .arg(format!("/select,{}", target.display()))
        .spawn()
        .map_err(|e| format!("文件管理器定位失败: {e}"))?;
    Ok(())
}

pub fn command_exists(command: &str) -> bool {
    let command_path = Path::new(command);
    if command_path.components().count() > 1 || command_path.extension().is_some() {
        return command_path.is_file();
    }

    let path = std::env::var_os("PATH").unwrap_or_default();
    let pathext = std::env::var_os("PATHEXT")
        .and_then(|v| v.into_string().ok())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
    let mut extensions: Vec<String> = pathext
        .split(';')
        .filter_map(|ext| {
            let ext = ext.trim();
            if ext.is_empty() {
                None
            } else if ext.starts_with('.') {
                Some(ext.to_string())
            } else {
                Some(format!(".{ext}"))
            }
        })
        .collect();
    extensions.insert(0, String::new());

    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for ext in &extensions {
            if dir.join(format!("{command}{ext}")).is_file() {
                return true;
            }
        }
    }
    if let Some(path) = common_libreoffice_tool_path(command) {
        if let Some(dir) = path.parent() {
            ensure_dir_on_process_path(dir.to_path_buf());
        }
        return true;
    }
    false
}

pub fn bios_serial_number() -> Result<String, String> {
    [
        read_bios_serial_from_powershell(),
        read_bios_serial_from_wmic(),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| normalize_bios_serial_for_binding(&value))
    .ok_or_else(|| "Unable to read a valid Windows BIOS serial number".to_string())
}

pub fn pdf_tool_path(command: &str) -> std::path::PathBuf {
    windows_path::pdf_tool_path(command)
}

pub fn pandoc_tool_path() -> std::path::PathBuf {
    windows_path::pandoc_tool_path()
}

pub fn libreoffice_tool_path() -> PathBuf {
    if let Ok(path) = std::env::var("PINVOU3_LIBREOFFICE_CMD") {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Some(path) = common_libreoffice_tool_path("soffice") {
        if let Some(dir) = path.parent() {
            ensure_dir_on_process_path(dir.to_path_buf());
        }
        return path;
    }
    PathBuf::from("soffice")
}

pub fn ocr_tool_path() -> PathBuf {
    windows_path::tesseract_tool_path()
}

pub fn ocr_tessdata_dir() -> Option<PathBuf> {
    windows_path::bundled_tessdata_dir()
}

pub fn archive_tool_path() -> PathBuf {
    windows_path::archive_tool_path()
}

pub fn pdf_tool_exists(command: &str) -> bool {
    windows_path::bundled_pdf_tool_path(command).is_some() || command_exists(command)
}

pub fn pandoc_tool_exists() -> bool {
    windows_path::bundled_pandoc_tool_path().is_some() || command_exists("pandoc")
}

pub fn ocr_tool_exists() -> bool {
    if windows_path::bundled_tesseract_dir().is_some() {
        return windows_path::bundled_tesseract_tool_path().is_some()
            && windows_path::bundled_tessdata_has_required_languages();
    }
    command_exists("tesseract")
}

pub fn archive_tool_exists() -> bool {
    windows_path::bundled_archive_tool_path().is_some() || command_exists("7z")
}

pub fn msg_native_supported() -> bool {
    true
}

pub fn msg_converter_required() -> bool {
    false
}

pub fn email_tool_exists() -> bool {
    msg_native_supported()
}

pub fn show_pdf_dependency_check() -> bool {
    false
}

pub fn show_pandoc_dependency_check() -> bool {
    false
}

pub fn show_ocr_dependency_check() -> bool {
    false
}

pub fn show_archive_dependency_check() -> bool {
    false
}

pub fn pdf_dependency_packages() -> &'static str {
    ""
}

pub fn pandoc_dependency_packages() -> &'static str {
    ""
}

pub fn archive_dependency_packages() -> &'static str {
    ""
}

pub fn email_dependency_packages() -> &'static str {
    ""
}

/// Windows 社区版当前不展示邮件依赖检测(show 标志为 false),无需手动指引。
pub fn email_manual_hint() -> Option<&'static str> {
    None
}

pub fn ocr_dependency_packages() -> &'static str {
    ""
}

pub fn pandoc_missing_message() -> &'static str {
    "文档解析组件缺失或不可用：内置 Pandoc 未在安装目录 pandoc 下找到，请修复或重新安装 pinvou。"
}

pub fn libreoffice_missing_message() -> &'static str {
    "Office 文档预览需要 LibreOffice，可前往设置 - 依赖体检安装。"
}

pub fn pdf_text_missing_message() -> &'static str {
    "PDF 解析组件缺失或不可用：内置 Poppler 未在安装目录 poppler 下找到，请修复或重新安装 pinvou。"
}

pub fn pdf_render_missing_message() -> &'static str {
    "PDF 渲染组件缺失或不可用：内置 Poppler 未在安装目录 poppler 下找到，请修复或重新安装 pinvou。"
}

pub fn pdf_ocr_missing_message() -> &'static str {
    "扫描件 PDF OCR 需要 Tesseract；PDF 渲染组件由内置 Poppler 提供，如仍失败请修复或重新安装 pinvou。"
}

pub fn presentation_pdf_missing_message() -> &'static str {
    "演示文稿解析需要 LibreOffice；PDF 文本组件由内置 Poppler 提供，如缺失请修复或重新安装 pinvou。"
}

pub fn system_default_open_supported(path: &Path) -> bool {
    let Some(ext) = normalized_presentation_extension(path) else {
        return false;
    };
    windows_open_command_for_extension(&ext).is_some()
}

pub fn libreoffice_open_fallback_needed(path: &Path) -> bool {
    normalized_presentation_extension(path).is_some() && !system_default_open_supported(path)
}

fn normalized_presentation_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "pptx" | "ppt" | "odp" | "dps" => Some(format!(".{ext}")),
        _ => None,
    }
}

fn windows_open_command_for_extension(ext: &str) -> Option<String> {
    let user_choice_key =
        format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{ext}\UserChoice");
    if let Some(prog_id) = read_registry_string(HKEY_CURRENT_USER, &user_choice_key, Some("ProgId"))
    {
        if let Some(command) = open_command_for_prog_id(&prog_id) {
            return Some(command);
        }
    }

    let prog_id = read_registry_string(HKEY_CLASSES_ROOT, ext, None)?;
    open_command_for_prog_id(&prog_id)
}

fn open_command_for_prog_id(prog_id: &str) -> Option<String> {
    let command_key = format!(r"{prog_id}\shell\open\command");
    read_registry_string(HKEY_CLASSES_ROOT, &command_key, None)
}

fn read_registry_string(root: HKEY, key_path: &str, value_name: Option<&str>) -> Option<String> {
    let key_path = wide_null(key_path);
    let value_name = value_name.map(wide_null);
    let value_name_ptr = value_name
        .as_ref()
        .map(|value| value.as_ptr())
        .unwrap_or(std::ptr::null());

    let mut key: HKEY = std::ptr::null_mut();
    let opened = unsafe { RegOpenKeyExW(root, key_path.as_ptr(), 0, KEY_READ, &mut key) };
    if opened != ERROR_SUCCESS {
        return None;
    }

    let mut value_type = 0;
    let mut byte_len = 0;
    let queried = unsafe {
        RegQueryValueExW(
            key,
            value_name_ptr,
            std::ptr::null_mut(),
            &mut value_type,
            std::ptr::null_mut(),
            &mut byte_len,
        )
    };
    if queried != ERROR_SUCCESS || byte_len < 2 || !matches!(value_type, REG_SZ | REG_EXPAND_SZ) {
        unsafe {
            RegCloseKey(key);
        }
        return None;
    }

    let mut data = vec![0u16; (byte_len as usize + 1) / 2];
    let queried = unsafe {
        RegQueryValueExW(
            key,
            value_name_ptr,
            std::ptr::null_mut(),
            &mut value_type,
            data.as_mut_ptr().cast::<u8>(),
            &mut byte_len,
        )
    };
    unsafe {
        RegCloseKey(key);
    }

    if queried != ERROR_SUCCESS || !matches!(value_type, REG_SZ | REG_EXPAND_SZ) {
        return None;
    }

    let len = data.iter().position(|&ch| ch == 0).unwrap_or(data.len());
    let value = String::from_utf16_lossy(&data[..len]).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn read_bios_serial_from_powershell() -> Option<String> {
    let output = HiddenCommand::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "try { (Get-CimInstance -ClassName Win32_BIOS -ErrorAction Stop).SerialNumber } catch { '' }",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    non_empty_stdout(output.stdout)
}

fn read_bios_serial_from_wmic() -> Option<String> {
    let output = HiddenCommand::new("wmic")
        .args(["bios", "get", "serialnumber", "/value"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("SerialNumber="))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn non_empty_stdout(stdout: Vec<u8>) -> Option<String> {
    let value = String::from_utf8_lossy(&stdout).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn normalize_bios_serial_for_binding(input: &str) -> Option<String> {
    let normalized = input
        .trim()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    if normalized.is_empty()
        || matches!(
            normalized.as_str(),
            "DEFAULTSTRING" | "TOBEFILLEDBYO.E.M." | "SYSTEMSERIALNUMBER" | "NONE" | "UNKNOWN"
        )
    {
        None
    } else {
        Some(normalized)
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

fn ensure_dir_on_process_path(dir: std::path::PathBuf) {
    let current = std::env::var_os("PATH").unwrap_or_default();
    if std::env::split_paths(&current).any(|path| same_path(&path, &dir)) {
        return;
    }
    let mut paths = vec![dir];
    paths.extend(std::env::split_paths(&current));
    if let Ok(joined) = std::env::join_paths(paths) {
        std::env::set_var("PATH", joined);
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

fn common_libreoffice_tool_path(command: &str) -> Option<PathBuf> {
    if !is_libreoffice_command(command) {
        return None;
    }
    let mut roots = Vec::new();
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        roots.push(PathBuf::from(program_files));
    }
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        roots.push(PathBuf::from(program_files_x86));
    }
    roots.push(PathBuf::from(r"C:\Program Files"));
    roots.push(PathBuf::from(r"C:\Program Files (x86)"));

    roots.into_iter().find_map(|root| {
        let program = root.join("LibreOffice").join("program");
        [program.join("soffice.com"), program.join("soffice.exe")]
            .into_iter()
            .find(|path| path.is_file())
    })
}

fn is_libreoffice_command(command: &str) -> bool {
    let name = Path::new(command)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "soffice" | "soffice.exe" | "soffice.com" | "libreoffice" | "libreoffice.exe"
    )
}

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    Vec::new()
}

/// Returns the installer-provisioned `runtime/node/node.exe`, resolved through
/// windows_path::bundled_node_dir. Consumers fall back to PATH when it is absent.
pub fn bundled_node() -> Option<PathBuf> {
    windows_path::bundled_node_dir()
        .map(|dir| dir.join("node.exe"))
        .filter(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_hides_archive_dependency_check() {
        assert!(!show_archive_dependency_check());
        assert_eq!(archive_dependency_packages(), "");
    }

    #[test]
    fn detects_libreoffice_command_names() {
        assert!(is_libreoffice_command("soffice"));
        assert!(is_libreoffice_command("soffice.exe"));
        assert!(is_libreoffice_command("soffice.com"));
        assert!(is_libreoffice_command("libreoffice"));
        assert!(!is_libreoffice_command("pandoc"));
    }

    #[test]
    fn libreoffice_missing_message_is_windows_specific() {
        let message = libreoffice_missing_message();
        assert!(message.contains("可前往设置 - 依赖体检"));
        assert!(!message.contains("Office/WPS"));
        assert!(!message.contains("sudo apt"));
    }

    #[test]
    fn system_default_open_check_is_limited_to_presentations() {
        assert_eq!(
            normalized_presentation_extension(Path::new("slides.pptx")).as_deref(),
            Some(".pptx")
        );
        assert_eq!(
            normalized_presentation_extension(Path::new("slides.PPT")).as_deref(),
            Some(".ppt")
        );
        assert!(normalized_presentation_extension(Path::new("notes.txt")).is_none());
    }
}

// ---------------- 本地引擎硬件探测 ----------------

/// 独显专用显存阈值 5.6GB：低于此档跑 4B Q4_K_M + KV 很吃力，按核显对待。
const DEDICATED_VRAM_MIN_BYTES: u64 = 5_600_000_000;

/// GPU 分级（本地引擎设备自动选择）：任一适配器专用显存 ≥5.6GB → 独显档；
/// 名称命中强核显白名单（Radeon 680M/780M/880M/890M、Iris Xe、Arc Graphics）
/// → 强核显档；其余核显 → 无 GPU。枚举失败一律回落无 GPU（CPU 推理）。
/// GPU 判定前提：vulkan-1.dll 存在（引擎 win-vulkan 包走 Vulkan 后端，
/// 缺运行时必然起不来，此时按 CPU 计）。
pub fn gpu_class() -> crate::platform::os::GpuClass {
    use crate::platform::os::GpuClass;
    if !vulkan_runtime_present() {
        return GpuClass::None;
    }
    enum_gpu_class().unwrap_or(GpuClass::None)
}

fn vulkan_runtime_present() -> bool {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("System32").join("vulkan-1.dll").is_file()
}

fn enum_gpu_class() -> Option<crate::platform::os::GpuClass> {
    use crate::platform::os::GpuClass;
    use dxgi::*;
    unsafe {
        let mut factory: *mut IDXGIFactory1 = std::ptr::null_mut();
        if CreateDXGIFactory1(&IID_IDXGIFactory1, &mut factory as *mut _ as *mut _) != 0
            || factory.is_null()
        {
            return None;
        }
        let mut index = 0u32;
        let mut best = GpuClass::None;
        loop {
            let mut adapter: *mut IDXGIAdapter1 = std::ptr::null_mut();
            // S_OK(0) 表示还有适配器;DXGI_ERROR_NOT_FOUND 即枚举完毕。
            let hr = ((*(*factory).lpVtbl).EnumAdapters1)(factory, index, &mut adapter);
            index += 1;
            if hr != 0 || adapter.is_null() {
                break;
            }
            let mut desc: DXGI_ADAPTER_DESC1 = std::mem::zeroed();
            let hr_desc = ((*(*adapter).lpVtbl).GetDesc1)(adapter, &mut desc);
            com_release(adapter);
            if hr_desc != 0 {
                continue;
            }
            // 跳过 Basic Render 等软件适配器。
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE != 0 {
                continue;
            }
            if desc.DedicatedVideoMemory as u64 >= DEDICATED_VRAM_MIN_BYTES {
                com_release(factory);
                return Some(GpuClass::Dedicated);
            }
            if is_strong_igpu(&adapter_name(&desc.Description)) {
                best = GpuClass::StrongIgpu;
            }
        }
        com_release(factory);
        Some(best)
    }
}

/// IUnknown::Release(vtable 第三槽),任何 DXGI 对象头都是 IUnknown 布局。
unsafe fn com_release<T>(obj: *mut T) {
    use windows_sys::core::IUnknown_Vtbl;
    if obj.is_null() {
        return;
    }
    let unknown = obj.cast::<core::ffi::c_void>();
    unsafe {
        let vtbl = unknown.cast::<*const IUnknown_Vtbl>().read();
        ((*vtbl).Release)(unknown);
    }
}

/// DXGI 最小手写绑定:windows-sys 0.61 未导出 Win32_Graphics_Dxgi(该版本收窄了
/// API 面),而完整 windows crate 仅为 GPU 枚举过重(见 Cargo.toml 依赖注释)。
/// vtable 槽序与 GUID/布局沿用 windows-sys::core 的同代 ABI,只保留
/// EnumAdapters1/GetDesc1 必需的完整槽位,未调用的槽仅作占位(槽位计数必须正确)。
#[allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]
mod dxgi {
    use core::ffi::c_void;
    use windows_sys::core::{IUnknown_Vtbl, GUID, HRESULT};
    use windows_sys::Win32::Foundation::LUID;

    pub const IID_IDXGIFactory1: GUID = GUID::from_u128(0x770aae78_f26f_4dba_a829_253c83d1b387);

    pub const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 2;

    #[link(name = "dxgi")]
    extern "system" {
        pub fn CreateDXGIFactory1(riid: *const GUID, pfactory: *mut *mut c_void) -> HRESULT;
    }

    #[repr(C)]
    pub struct DXGI_ADAPTER_DESC1 {
        pub Description: [u16; 128],
        pub VendorId: u32,
        pub DeviceId: u32,
        pub SubSysId: u32,
        pub Revision: u32,
        pub DedicatedVideoMemory: usize,
        pub DedicatedSystemMemory: usize,
        pub SharedSystemMemory: usize,
        pub AdapterLuid: LUID,
        pub Flags: u32,
    }

    #[repr(C)]
    pub struct IDXGIFactory1 {
        pub lpVtbl: *const IDXGIFactory1_Vtbl,
    }

    #[repr(C)]
    pub struct IDXGIAdapter1 {
        pub lpVtbl: *const IDXGIAdapter1_Vtbl,
    }

    /// IUnknown → IDXGIObject(4)→ IDXGIFactory(5)→ IDXGIFactory1(2)。
    #[repr(C)]
    pub struct IDXGIFactory1_Vtbl {
        pub base: IUnknown_Vtbl,
        pub SetPrivateData: usize,
        pub SetPrivateDataInterface: usize,
        pub GetPrivateData: usize,
        pub GetParent: usize,
        pub EnumAdapters: usize,
        pub MakeWindowAssociation: usize,
        pub GetWindowAssociation: usize,
        pub CreateSwapChain: usize,
        pub CreateSoftwareAdapter: usize,
        pub EnumAdapters1: unsafe extern "system" fn(
            this: *mut IDXGIFactory1,
            adapter: u32,
            ppadapter: *mut *mut IDXGIAdapter1,
        ) -> HRESULT,
        pub IsCurrent: usize,
    }

    /// IUnknown → IDXGIObject(4)→ IDXGIAdapter(3)→ IDXGIAdapter1(1)。
    #[repr(C)]
    pub struct IDXGIAdapter1_Vtbl {
        pub base: IUnknown_Vtbl,
        pub SetPrivateData: usize,
        pub SetPrivateDataInterface: usize,
        pub GetPrivateData: usize,
        pub GetParent: usize,
        pub EnumOutputs: usize,
        pub GetDesc: usize,
        pub CheckInterfaceSupport: usize,
        pub GetDesc1: unsafe extern "system" fn(
            this: *mut IDXGIAdapter1,
            pdesc: *mut DXGI_ADAPTER_DESC1,
        ) -> HRESULT,
    }
}

fn adapter_name(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn is_strong_igpu(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "radeon 680m",
        "radeon 780m",
        "radeon 880m",
        "radeon 890m",
        "iris xe",
        "arc graphics",
    ]
    .iter()
    .any(|key| name.contains(key))
}

/// 物理核数（llama-server `-t` 用）：GetLogicalProcessorInformation 按
/// RelationProcessorCore 条目计数（每条目对应一个物理核）；失败回落逻辑核数。
pub fn physical_core_count() -> usize {
    physical_cores_via_processor_info().unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    })
}

fn physical_cores_via_processor_info() -> Option<usize> {
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformation, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
    };
    unsafe {
        let mut len: u32 = 0;
        GetLogicalProcessorInformation(std::ptr::null_mut(), &mut len);
        if len == 0 {
            return None;
        }
        let count = len as usize / std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
        let mut buf: Vec<SYSTEM_LOGICAL_PROCESSOR_INFORMATION> = Vec::with_capacity(count);
        if GetLogicalProcessorInformation(buf.as_mut_ptr(), &mut len) == 0 {
            return None;
        }
        buf.set_len(count);
        let cores = buf
            .iter()
            .filter(|info| info.Relationship == RelationProcessorCore)
            .count();
        (cores > 0).then_some(cores)
    }
}
