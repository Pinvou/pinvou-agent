use std::path::PathBuf;

pub fn user_home_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

pub fn pdf_tool_path(command: &str) -> PathBuf {
    PathBuf::from(command)
}

pub fn pandoc_tool_path() -> PathBuf {
    PathBuf::from("pandoc")
}
