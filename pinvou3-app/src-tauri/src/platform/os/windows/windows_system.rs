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

/// 专用有头 Chrome 的可执行候选（Windows 常见安装路径 + 用户级安装 + PATH `chrome`）。
/// 与 `browser-wrapper.mjs` 的 win32 候选保持一致：
/// - 用 `PROGRAMFILES`/`PROGRAMFILES(X86)`/`LOCALAPPDATA` 环境变量而非硬编码 C 盘
///   （系统盘非 C 或重定向安装时硬编码探测不到，会出现"Rust 报未检测到 Chrome
///   但 wrapper 实际启动成功"的自相矛盾）；
/// - 含 Edge 候选（提示文案宣称 Chrome/Chromium/Edge，仅装 Edge 的机器要能找到）。
pub fn chrome_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    let mut push = |candidates: &mut Vec<String>, dir: Option<String>, rel: &str| {
        if let Some(dir) = dir {
            candidates.push(format!(r"{dir}\{rel}"));
        }
    };
    let pf = std::env::var("PROGRAMFILES").ok();
    let pf86 = std::env::var("PROGRAMFILES(X86)").ok();
    let local = std::env::var("LOCALAPPDATA").ok();
    push(
        &mut candidates,
        pf.clone(),
        r"Google\Chrome\Application\chrome.exe",
    );
    push(
        &mut candidates,
        pf86.clone(),
        r"Google\Chrome\Application\chrome.exe",
    );
    push(
        &mut candidates,
        local.clone(),
        r"Google\Chrome\Application\chrome.exe",
    );
    push(
        &mut candidates,
        pf.clone(),
        r"Microsoft\Edge\Application\msedge.exe",
    );
    push(
        &mut candidates,
        pf86,
        r"Microsoft\Edge\Application\msedge.exe",
    );
    push(
        &mut candidates,
        local,
        r"Microsoft\Edge\Application\msedge.exe",
    );
    candidates.push("chrome".to_string());
    candidates.push("msedge".to_string());
    candidates
}

/// 随安装包捆绑的 node：Windows 安装器释放 `runtime/node/node.exe`（连接器链路
/// 同款解析，见 windows_path::bundled_node_dir）；无捆绑时 None，消费方回退
/// 系统 PATH 探测。
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
