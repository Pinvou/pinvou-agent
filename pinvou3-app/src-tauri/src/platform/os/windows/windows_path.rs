use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use std::process::Command;

const PDF_TOOLS: &[&str] = &["pdftotext", "pdftoppm"];
const PANDOC_TOOL: &str = "pandoc";
const TESSERACT_TOOL: &str = "tesseract";
const ARCHIVE_TOOL: &str = "7z";

pub fn user_home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("USERPROFILE") {
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        let home = format!("{drive}{path}");
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return platform_compat_path(&home);
        }
    }
    std::env::temp_dir()
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    let trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = trimmed.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }

    let normalized = trimmed.replace('\\', "/");
    if normalized == "/tmp" || normalized.starts_with("/tmp/") {
        let rest = normalized
            .trim_start_matches("/tmp")
            .trim_start_matches('/');
        return if rest.is_empty() {
            std::env::temp_dir()
        } else {
            std::env::temp_dir().join(rest.replace('/', "\\"))
        };
    }

    PathBuf::from(trimmed)
}

pub fn validate_upload_location(canon: &Path) -> Result<(), String> {
    for (label, value) in windows_sensitive_roots() {
        let root = platform_compat_path(
            &std::fs::canonicalize(&value)
                .unwrap_or_else(|_| value.clone())
                .to_string_lossy(),
        );
        if path_starts_with_case_insensitive(canon, &root) {
            return Err(format!(
                "path {} under sensitive system dir {}",
                canon.display(),
                label
            ));
        }
    }
    Ok(())
}

pub fn path_component_eq(component: &OsStr, expected: &str) -> bool {
    component.to_string_lossy().eq_ignore_ascii_case(expected)
}

pub fn filesystem_path_identity_key(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

pub fn python_command() -> String {
    if let Ok(p) = std::env::var("PINVOU3_PYTHON") {
        if !p.is_empty() && is_valid_python_candidate(Path::new(&p)) {
            return p;
        }
    }

    if let Some(bundled) = bundled_python_path() {
        return bundled.to_string_lossy().into_owned();
    }

    resolve_system_python().unwrap_or_else(|| "pythonw".to_string())
}

pub fn connector_cli_command(cli_bin: &str, program: &str) -> Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut cmd = Command::new(connector_cli_program(cli_bin, program));
    cmd.creation_flags(CREATE_NO_WINDOW);
    apply_windows_connector_path(&mut cmd);
    cmd
}

fn connector_cli_program(cli_bin: &str, program: &str) -> OsString {
    if matches!(program, "npm" | "npx") {
        if let Some(path) = bundled_node_command(program) {
            return path.into_os_string();
        }
        return windows_npm_shim(program);
    }

    if !cli_bin.is_empty() && program == cli_bin {
        // 应用按需下载并校验的连接器 CLI 优先,缺失时回退已有 npm 全局 shim。
        if let Some(bin_dir) = crate::platform::paths::managed_connector_bin_dir() {
            let bundled = bin_dir.join(format!("{cli_bin}.exe"));
            if bundled.is_file() {
                return bundled.into_os_string();
            }
        }
        return windows_npm_shim(program);
    }

    program.into()
}

fn windows_npm_shim(program: &str) -> OsString {
    let shim = format!("{program}.cmd");
    for prefix in windows_npm_prefix_candidates() {
        let path = prefix.join(&shim);
        if path.is_file() {
            return path.into_os_string();
        }
    }
    shim.into()
}

pub fn apply_user_npm_prefix(cmd: &mut Command) {
    let prefix_from_env = windows_npm_prefix_from_env();
    let prefix = prefix_from_env.clone().or_else(default_windows_npm_prefix);

    if let Some(prefix) = prefix {
        let _ = std::fs::create_dir_all(&prefix);
        if prefix_from_env.is_none() {
            cmd.env("NPM_CONFIG_PREFIX", &prefix)
                .env("npm_config_prefix", &prefix);
        }

        let mut path_entries = vec![prefix];
        if let Some(node_dir) = bundled_node_dir() {
            path_entries.push(node_dir);
        }
        prepend_connector_path_entries(cmd, path_entries);
    } else if let Some(node_dir) = bundled_node_dir() {
        prepend_connector_path_entries(cmd, [node_dir]);
    }

    if std::env::var_os("NPM_CONFIG_CACHE").is_none()
        && std::env::var_os("npm_config_cache").is_none()
    {
        let cache = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("pinvou3")
            .join("npm-cache");
        let _ = std::fs::create_dir_all(&cache);
        cmd.env("NPM_CONFIG_CACHE", &cache)
            .env("npm_config_cache", &cache);
    }
}

pub fn kill_pid_tree(pid: u32) {
    let _ = connector_cli_command("", "taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .output();
}

fn apply_windows_connector_path(cmd: &mut Command) {
    let mut entries = windows_npm_prefix_candidates();
    if let Some(node_dir) = bundled_node_dir() {
        entries.push(node_dir);
    }
    prepend_connector_path_entries(cmd, entries);
}

fn windows_npm_prefix_from_env() -> Option<PathBuf> {
    ["NPM_CONFIG_PREFIX", "npm_config_prefix"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .find(|path| !path.as_os_str().is_empty())
}

fn default_windows_npm_prefix() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("npm"))
}

fn windows_npm_prefix_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(prefix) = windows_npm_prefix_from_env() {
        candidates.push(prefix);
    }
    if let Some(prefix) = default_windows_npm_prefix() {
        if !candidates
            .iter()
            .any(|existing| same_path(existing, &prefix))
        {
            candidates.push(prefix);
        }
    }
    candidates
}

pub(crate) fn bundled_node_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| bundled_runtime_dir_for_exe(&exe, "node"))
        .filter(|path| path.is_dir())
}

fn bundled_node_command(program: &str) -> Option<PathBuf> {
    let filename = match program {
        "npm" => "npm.cmd",
        "npx" => "npx.cmd",
        _ => return None,
    };
    bundled_node_dir()
        .map(|dir| dir.join(filename))
        .filter(|path| path.is_file())
}

fn prepend_connector_path_entries(cmd: &mut Command, dirs: impl IntoIterator<Item = PathBuf>) {
    let mut paths: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        if dir.as_os_str().is_empty() || paths.iter().any(|existing| same_path(existing, &dir)) {
            continue;
        }
        paths.push(dir);
    }

    if let Some(current) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&current) {
            if paths.iter().any(|existing| same_path(existing, &dir)) {
                continue;
            }
            paths.push(dir);
        }
    }

    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .eq_ignore_ascii_case(right.to_string_lossy().trim_end_matches(['\\', '/']))
}

pub fn bundled_python_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_python_path_for_exe(&exe))
}

pub fn bundled_onnxruntime_dylib_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| bundled_runtime_dir_for_exe(&exe, "onnxruntime").join("onnxruntime.dll"))
        .filter(|path| path.is_file())
}

pub fn configure_onnxruntime_dylib() -> Result<(), String> {
    if std::env::var("ORT_DYLIB_PATH")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(());
    }
    let path = bundled_onnxruntime_dylib_path().ok_or_else(|| {
        "Windows ONNX Runtime CPU runtime is missing: runtime/onnxruntime/onnxruntime.dll"
            .to_string()
    })?;
    std::env::set_var("ORT_DYLIB_PATH", &path);
    eprintln!(
        "[platform] ONNX Runtime dynamic library pinned: {}",
        path.display()
    );
    Ok(())
}

pub fn obsidian_config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|app_data| app_data.join("obsidian").join("obsidian.json"))
}

pub fn bundled_python_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "python").join("pythonw.exe");
    is_valid_python_candidate(&path).then_some(path)
}

fn resolve_system_python() -> Option<String> {
    if let Ok(path_var) = std::env::var("PATH") {
        for name in ["pythonw.exe", "python.exe"] {
            for dir in std::env::split_paths(&path_var) {
                let cand = dir.join(name);
                if is_valid_python_candidate(&cand) {
                    return Some(cand.to_string_lossy().into_owned());
                }
            }
        }
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        roots.push(PathBuf::from(la).join("Programs").join("Python"));
    }
    if let Ok(pf) = std::env::var("ProgramFiles") {
        roots.push(PathBuf::from(pf));
    }
    for root in roots {
        if let Ok(rd) = std::fs::read_dir(&root) {
            let mut vers: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| n.starts_with("Python3"))
                            .unwrap_or(false)
                })
                .collect();
            vers.sort();
            for d in vers.iter().rev() {
                for name in ["pythonw.exe", "python.exe"] {
                    let cand = d.join(name);
                    if is_valid_python_candidate(&cand) {
                        return Some(cand.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let cand = dir.join("py.exe");
            if cand.is_file() {
                return Some(cand.to_string_lossy().into_owned());
            }
        }
    }
    let winpy = PathBuf::from(r"C:\Windows\py.exe");
    if winpy.is_file() {
        return Some(winpy.to_string_lossy().into_owned());
    }
    None
}

fn is_valid_python_candidate(path: &Path) -> bool {
    path.is_file() && !is_windowsapps_python_alias(path)
}

fn is_windowsapps_python_alias(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if !matches!(
        name.to_ascii_lowercase().as_str(),
        "python.exe" | "pythonw.exe" | "python3.exe" | "python3w.exe"
    ) {
        return false;
    }
    path.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("WindowsApps")
    })
}

fn windows_sensitive_roots() -> Vec<(&'static str, PathBuf)> {
    [
        ("WINDIR", "WINDIR"),
        ("SystemRoot", "SystemRoot"),
        ("ProgramFiles", "ProgramFiles"),
        ("ProgramFiles(x86)", "ProgramFiles(x86)"),
        ("ProgramData", "ProgramData"),
    ]
    .into_iter()
    .filter_map(|(label, key)| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| (label, PathBuf::from(value)))
    })
    .collect()
}

fn path_starts_with_case_insensitive(path: &Path, base: &Path) -> bool {
    let path = normalized_path_key(path);
    let base = normalized_path_key(base);
    path == base || path.starts_with(&(base + "/"))
}

fn normalized_path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

pub fn bundled_tesseract_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| bundled_runtime_dir_for_exe(&exe, "tesseract"))
        .filter(|path| path.is_dir())
}

#[cfg(test)]
fn bundled_poppler_dir_for_exe(exe_path: &Path) -> PathBuf {
    bundled_runtime_dir_for_exe(exe_path, "poppler")
}

#[cfg(test)]
fn bundled_pandoc_dir_for_exe(exe_path: &Path) -> PathBuf {
    bundled_runtime_dir_for_exe(exe_path, "pandoc")
}

#[cfg(test)]
fn bundled_tesseract_dir_for_exe(exe_path: &Path) -> PathBuf {
    bundled_runtime_dir_for_exe(exe_path, "tesseract")
}

#[cfg(test)]
fn bundled_archive_dir_for_exe(exe_path: &Path) -> PathBuf {
    bundled_runtime_dir_for_exe(exe_path, "7zip")
}

pub fn bundled_pdf_tool_path(command: &str) -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_pdf_tool_path_for_exe(&exe, command))
}

pub fn bundled_pandoc_tool_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_pandoc_tool_path_for_exe(&exe))
}

pub fn bundled_asr_tool_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_asr_tool_path_for_exe(&exe))
}

pub fn bundled_asr_backend_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_asr_backend_path_for_exe(&exe))
}

pub fn bundled_asr_model_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_asr_model_path_for_exe(&exe))
}

pub fn user_asr_model_path() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("asr")
        .join(asr_q8_model_filename())
}

pub fn asr_model_path() -> PathBuf {
    if let Ok(path) = std::env::var("PINVOU3_SENSEVOICE_MODEL") {
        if !path.trim().is_empty() {
            return PathBuf::from(path.trim());
        }
    }
    std::env::current_exe()
        .ok()
        .map(|exe| {
            asr_model_path_for_exe_and_user_home(&exe, &crate::platform::paths::pinvou3_home())
        })
        .unwrap_or_else(user_asr_model_path)
}

pub fn asr_model_path_for_exe_and_user_home(exe_path: &Path, pinvou_home: &Path) -> PathBuf {
    let user_model = pinvou_home.join("asr").join(asr_q8_model_filename());
    if user_model.is_file() {
        return user_model;
    }
    bundled_asr_model_path_for_exe(exe_path).unwrap_or(user_model)
}

pub fn bundled_tesseract_tool_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_tesseract_tool_path_for_exe(&exe))
}

pub fn bundled_archive_tool_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_archive_tool_path_for_exe(&exe))
}

pub fn bundled_tessdata_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_tessdata_dir_for_exe(&exe))
}

pub fn bundled_pdf_tool_path_for_exe(exe_path: &Path, command: &str) -> Option<PathBuf> {
    let filename = pdf_tool_filename(command)?;
    let path = bundled_runtime_dir_for_exe(exe_path, "poppler").join(filename);
    path.is_file().then_some(path)
}

pub fn bundled_pandoc_tool_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "pandoc").join(pandoc_tool_filename());
    path.is_file().then_some(path)
}

pub fn bundled_asr_tool_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "asr").join(asr_tool_filename());
    path.is_file().then_some(path)
}

pub fn bundled_asr_backend_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "asr").join("llama-funasr-sensevoice.exe");
    path.is_file().then_some(path)
}

pub fn bundled_asr_model_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let dir = bundled_runtime_dir_for_exe(exe_path, "asr");
    [
        dir.join("models").join(asr_q8_model_filename()),
        dir.join("gguf").join(asr_q8_model_filename()),
        dir.join(asr_q8_model_filename()),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

pub fn bundled_tesseract_tool_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "tesseract").join(tesseract_tool_filename());
    path.is_file().then_some(path)
}

pub fn bundled_archive_tool_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "7zip").join(archive_tool_filename());
    path.is_file().then_some(path)
}

pub fn bundled_tessdata_dir_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "tesseract").join("tessdata");
    path.is_dir().then_some(path)
}

pub fn pdf_tool_path(command: &str) -> PathBuf {
    bundled_pdf_tool_path(command).unwrap_or_else(|| PathBuf::from(command))
}

pub fn pandoc_tool_path() -> PathBuf {
    bundled_pandoc_tool_path().unwrap_or_else(|| PathBuf::from(PANDOC_TOOL))
}

pub fn tesseract_tool_path() -> PathBuf {
    bundled_tesseract_tool_path().unwrap_or_else(|| PathBuf::from(TESSERACT_TOOL))
}

pub fn archive_tool_path() -> PathBuf {
    bundled_archive_tool_path().unwrap_or_else(|| PathBuf::from(ARCHIVE_TOOL))
}

pub fn bundled_tessdata_has_required_languages() -> bool {
    std::env::current_exe()
        .ok()
        .map(|exe| bundled_tessdata_has_required_languages_for_exe(&exe))
        .unwrap_or(false)
}

pub fn bundled_tessdata_has_required_languages_for_exe(exe_path: &Path) -> bool {
    let Some(dir) = bundled_tessdata_dir_for_exe(exe_path) else {
        return false;
    };
    dir.join("chi_sim.traineddata").is_file() && dir.join("eng.traineddata").is_file()
}

fn bundled_runtime_dir_for_exe(exe_path: &Path, name: &str) -> PathBuf {
    install_dir_for_exe(exe_path).join("runtime").join(name)
}

fn install_dir_for_exe(exe_path: &Path) -> &Path {
    exe_path.parent().unwrap_or_else(|| Path::new("."))
}

fn pdf_tool_filename(command: &str) -> Option<String> {
    if command.contains(['/', '\\', MAIN_SEPARATOR]) {
        return None;
    }
    let stem = command.strip_suffix(".exe").unwrap_or(command);
    PDF_TOOLS
        .iter()
        .any(|tool| tool.eq_ignore_ascii_case(stem))
        .then(|| format!("{stem}.exe"))
}

fn pandoc_tool_filename() -> &'static str {
    "pandoc.exe"
}

fn asr_tool_filename() -> &'static str {
    "pinvou-asr.exe"
}

fn asr_q8_model_filename() -> &'static str {
    "sensevoice-small-q8.gguf"
}

fn tesseract_tool_filename() -> &'static str {
    "tesseract.exe"
}

fn archive_tool_filename() -> &'static str {
    "7z.exe"
}

#[cfg(test)]
mod tests {
    use super::*;
    // 不再自建局部 ENV_LOCK:改借 crate 级唯一的 platform::paths::tests::ENV_LOCK,
    // 与所有 mutate env var 的测试共享同一把锁(此处 PINVOU3_PYTHON 属 env 写),
    // 消除并行测试的数据竞争。

    fn test_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-windows-path-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn unix_tmp_path_maps_to_temp_dir() {
        assert_eq!(
            platform_compat_path("/tmp/pinvou3-test-override"),
            std::env::temp_dir().join("pinvou3-test-override")
        );
    }

    #[test]
    fn bundled_poppler_dir_handles_spaces_and_chinese_chars() {
        let exe = Path::new(r"C:\Program Files\品眸 pinvou\pinvou3.exe");
        assert_eq!(
            bundled_poppler_dir_for_exe(exe),
            PathBuf::from(r"C:\Program Files\品眸 pinvou")
                .join("runtime")
                .join("poppler")
        );
    }

    #[test]
    fn bundled_pandoc_dir_handles_spaces_and_chinese_chars() {
        let exe = Path::new(r"C:\Program Files\品眸 pinvou\pinvou3.exe");
        assert_eq!(
            bundled_pandoc_dir_for_exe(exe),
            PathBuf::from(r"C:\Program Files\品眸 pinvou")
                .join("runtime")
                .join("pandoc")
        );
    }

    #[test]
    fn python_command_respects_valid_pinvou3_python() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = test_temp_dir("pinvou3-python-env");
        let python = dir.join("pythonw.exe");
        std::fs::write(&python, b"").unwrap();
        let prev = std::env::var("PINVOU3_PYTHON").ok();

        std::env::set_var("PINVOU3_PYTHON", &python);
        assert_eq!(python_command(), python.to_string_lossy());

        match prev {
            Some(v) => std::env::set_var("PINVOU3_PYTHON", v),
            None => std::env::remove_var("PINVOU3_PYTHON"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bundled_python_uses_runtime_python_dir() {
        let dir = test_temp_dir("bundled-python");
        let exe = dir.join("pinvou3-tauri.exe");
        let python_dir = dir.join("runtime").join("python");
        std::fs::create_dir_all(&python_dir).unwrap();
        let python = python_dir.join("pythonw.exe");
        std::fs::write(&exe, b"").unwrap();
        std::fs::write(&python, b"").unwrap();

        assert_eq!(bundled_python_path_for_exe(&exe), Some(python));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bundled_onnxruntime_uses_runtime_onnxruntime_dir() {
        let dir = test_temp_dir("bundled-onnxruntime");
        let exe = dir.join("pinvou3-tauri.exe");
        let ort_dir = dir.join("runtime").join("onnxruntime");
        std::fs::create_dir_all(&ort_dir).unwrap();
        let ort = ort_dir.join("onnxruntime.dll");
        std::fs::write(&exe, b"").unwrap();
        std::fs::write(&ort, b"").unwrap();

        let resolved = bundled_runtime_dir_for_exe(&exe, "onnxruntime").join("onnxruntime.dll");
        assert_eq!(resolved, ort);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn windowsapps_python_alias_is_not_valid_candidate() {
        let dir = test_temp_dir("windowsapps-alias");
        let alias_dir = dir.join("Microsoft").join("WindowsApps");
        std::fs::create_dir_all(&alias_dir).unwrap();
        let alias = alias_dir.join("python.exe");
        std::fs::write(&alias, b"").unwrap();

        assert!(is_windowsapps_python_alias(&alias));
        assert!(!is_valid_python_candidate(&alias));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn extended_length_path_maps_to_normal_windows_path() {
        assert_eq!(
            platform_compat_path(r"\\?\C:\Users\z27014\Downloads\a.pdf"),
            PathBuf::from(r"C:\Users\z27014\Downloads\a.pdf")
        );
        assert_eq!(
            platform_compat_path(r"\\?\UNC\server\share\a.pdf"),
            PathBuf::from(r"\\server\share\a.pdf")
        );
    }

    #[test]
    fn upload_location_allows_windows_upload_file_outside_home() {
        let file = Path::new(r"D:\company docs\report.pptx");
        assert!(validate_upload_location(file).is_ok());
    }

    #[test]
    fn upload_location_matches_windows_roots_case_insensitively() {
        assert!(path_starts_with_case_insensitive(
            Path::new(r"C:\WINDOWS\System32\drivers\etc\hosts"),
            Path::new(r"c:\windows")
        ));
        assert!(!path_starts_with_case_insensitive(
            Path::new(r"D:\company docs\report.pptx"),
            Path::new(r"C:\Windows")
        ));
    }

    #[test]
    fn bundled_pdf_tool_path_prefers_bundled_exe() {
        let root =
            std::env::temp_dir().join(format!("pinvou3 poppler 路径测试 {}", std::process::id()));
        let poppler = root.join("runtime").join("poppler");
        std::fs::create_dir_all(&poppler).unwrap();
        let tool = poppler.join("pdftotext.exe");
        std::fs::write(&tool, b"fake exe").unwrap();
        let exe = root.join("pinvou3.exe");

        assert_eq!(bundled_pdf_tool_path_for_exe(&exe, "pdftotext"), Some(tool));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bundled_pandoc_tool_path_prefers_bundled_exe() {
        let root =
            std::env::temp_dir().join(format!("pinvou3 pandoc 路径测试 {}", std::process::id()));
        let pandoc = root.join("runtime").join("pandoc");
        std::fs::create_dir_all(&pandoc).unwrap();
        let tool = pandoc.join("pandoc.exe");
        std::fs::write(&tool, b"fake exe").unwrap();
        let exe = root.join("pinvou3.exe");

        assert_eq!(bundled_pandoc_tool_path_for_exe(&exe), Some(tool));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bundled_tesseract_dir_handles_spaces_and_chinese_chars() {
        let exe = Path::new(r"C:\Program Files\pinvou app\pinvou3.exe");
        assert_eq!(
            bundled_tesseract_dir_for_exe(exe),
            PathBuf::from(r"C:\Program Files\pinvou app")
                .join("runtime")
                .join("tesseract")
        );
    }

    #[test]
    fn bundled_tesseract_tool_and_tessdata_paths_prefer_bundled_runtime() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3 tesseract path test {}",
            std::process::id()
        ));
        let tesseract = root.join("runtime").join("tesseract");
        let tessdata = tesseract.join("tessdata");
        std::fs::create_dir_all(&tessdata).unwrap();
        let tool = tesseract.join("tesseract.exe");
        let chi = tessdata.join("chi_sim.traineddata");
        let eng = tessdata.join("eng.traineddata");
        std::fs::write(&tool, b"fake exe").unwrap();
        std::fs::write(&chi, b"fake chi").unwrap();
        std::fs::write(&eng, b"fake eng").unwrap();
        let exe = root.join("pinvou3.exe");

        assert_eq!(bundled_tesseract_tool_path_for_exe(&exe), Some(tool));
        assert_eq!(bundled_tessdata_dir_for_exe(&exe), Some(tessdata));
        assert!(bundled_tessdata_has_required_languages_for_exe(&exe));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bundled_asr_paths_allow_runtime_without_q8_model() {
        let root =
            std::env::temp_dir().join(format!("pinvou3 asr path test {}", std::process::id()));
        let asr = root.join("runtime").join("asr");
        let models = asr.join("models");
        std::fs::create_dir_all(&models).unwrap();
        let wrapper = asr.join("pinvou-asr.exe");
        let backend = asr.join("llama-funasr-sensevoice.exe");
        let model = models.join("sensevoice-small-q8.gguf");
        std::fs::write(&wrapper, b"fake wrapper").unwrap();
        std::fs::write(&backend, b"fake backend").unwrap();
        std::fs::write(&model, b"fake model").unwrap();
        let exe = root.join("pinvou3.exe");

        assert_eq!(bundled_asr_tool_path_for_exe(&exe), Some(wrapper));
        assert_eq!(bundled_asr_backend_path_for_exe(&exe), Some(backend));
        assert_eq!(bundled_asr_model_path_for_exe(&exe), Some(model));

        std::fs::remove_file(models.join("sensevoice-small-q8.gguf")).unwrap();
        assert!(bundled_asr_tool_path_for_exe(&exe).is_some());
        assert!(bundled_asr_backend_path_for_exe(&exe).is_some());
        assert_eq!(bundled_asr_model_path_for_exe(&exe), None);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn asr_model_path_prefers_user_model_over_bundled_model() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3 asr user model path {}",
            std::process::id()
        ));
        let install_asr = root
            .join("install")
            .join("runtime")
            .join("asr")
            .join("models");
        let user_asr = root.join("home").join("asr");
        std::fs::create_dir_all(&install_asr).unwrap();
        std::fs::create_dir_all(&user_asr).unwrap();
        let bundled = install_asr.join("sensevoice-small-q8.gguf");
        let user = user_asr.join("sensevoice-small-q8.gguf");
        std::fs::write(&bundled, b"bundled").unwrap();
        std::fs::write(&user, b"user").unwrap();
        let exe = root.join("install").join("pinvou3.exe");

        assert_eq!(
            asr_model_path_for_exe_and_user_home(&exe, &root.join("home")),
            user
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn asr_model_path_falls_back_to_bundled_runtime_model() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3 asr bundled model path {}",
            std::process::id()
        ));
        let install_asr = root
            .join("install")
            .join("runtime")
            .join("asr")
            .join("models");
        std::fs::create_dir_all(&install_asr).unwrap();
        let bundled = install_asr.join("sensevoice-small-q8.gguf");
        std::fs::write(&bundled, b"bundled").unwrap();
        let exe = root.join("install").join("pinvou3.exe");

        assert_eq!(
            asr_model_path_for_exe_and_user_home(&exe, &root.join("home")),
            bundled
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bundled_archive_tool_path_prefers_bundled_7z() {
        let root =
            std::env::temp_dir().join(format!("pinvou3 7zip path test {}", std::process::id()));
        let archive = root.join("runtime").join("7zip");
        std::fs::create_dir_all(&archive).unwrap();
        let tool = archive.join("7z.exe");
        std::fs::write(&tool, b"fake exe").unwrap();
        let exe = root.join("pinvou3.exe");

        assert_eq!(bundled_archive_dir_for_exe(&exe), archive);
        assert_eq!(bundled_archive_tool_path_for_exe(&exe), Some(tool));

        std::fs::remove_dir_all(&root).ok();
    }
}
