use std::ffi::OsStr;
use std::process::Command;

pub fn open_target(target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(target.as_ref())
        .spawn()
        .map_err(|e| format!("系统打开失败({label}): {e}"))?;
    Ok(())
}

pub fn command_exists(command: &str) -> bool {
    Command::new("which")
        .arg(command)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    vec!["nvidia-smi", "/usr/bin/nvidia-smi", "/usr/local/bin/nvidia-smi"]
}
