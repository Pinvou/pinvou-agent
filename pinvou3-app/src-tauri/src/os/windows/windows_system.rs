use std::ffi::OsStr;
use std::path::Path;
use crate::process::HiddenCommand;

pub fn open_target(target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    HiddenCommand::new("cmd")
        .args(["/C", "start", ""])
        .arg(target.as_ref())
        .spawn()
        .map_err(|e| format!("系统打开失败({label}): {e}"))?;
    Ok(())
}

pub fn command_exists(command: &str) -> bool {
    let command_path = Path::new(command);
    if command_path.components().count() > 1 || command_path.extension().is_some() {
        return command_path.is_file();
    }

    let path = match std::env::var_os("PATH") {
        Some(path) => path,
        None => return false,
    };
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
    false
}

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    vec![
        "nvidia-smi",
        r"C:\Windows\System32\nvidia-smi.exe",
        r"C:\Program Files\NVIDIA Corporation\NVSMI\nvidia-smi.exe",
    ]
}
