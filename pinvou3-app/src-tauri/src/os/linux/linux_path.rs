use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub fn user_home_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

pub fn validate_upload_location(canon: &Path) -> Result<(), String> {
    let home_raw = user_home_dir();
    let home = platform_compat_path(
        &std::fs::canonicalize(&home_raw)
            .unwrap_or_else(|_| home_raw.clone())
            .to_string_lossy(),
    );
    if !canon.starts_with(&home) {
        return Err(format!("path {} not under $HOME", canon.display()));
    }
    Ok(())
}

pub fn path_component_eq(component: &OsStr, expected: &str) -> bool {
    component == OsStr::new(expected)
}

pub fn python_command() -> String {
    if which_in_path("python3") {
        return "python3".to_string();
    }
    if which_in_path("python") {
        return "python".to_string();
    }
    "python3".to_string()
}

fn which_in_path(cmd: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if dir.join(cmd).is_file() {
                return true;
            }
        }
    }
    false
}

pub fn pdf_tool_path(command: &str) -> PathBuf {
    PathBuf::from(command)
}

pub fn pandoc_tool_path() -> PathBuf {
    PathBuf::from("pandoc")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_location_rejects_outside_home() {
        assert!(validate_upload_location(Path::new("/etc/passwd")).is_err());
    }
}
