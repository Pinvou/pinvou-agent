//! macOS platform adapter.
//!
//! macOS currently keeps the explicit unsupported behavior for capabilities
//! that have not been implemented yet. Keeping a dedicated adapter makes each
//! future capability an intentional macOS change instead of falling through an
//! unknown-platform branch.

pub use super::unsupported::*;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn open_target(target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    Command::new("open")
        .arg(target.as_ref())
        .spawn()
        .map_err(|error| format!("系统打开失败({label}): {error}"))?;
    Ok(())
}

pub fn reveal_target(target: &Path) -> Result<(), String> {
    Command::new("open")
        .arg("-R")
        .arg(target)
        .spawn()
        .map_err(|error| format!("文件管理器定位失败: {error}"))?;
    Ok(())
}

pub fn obsidian_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support/obsidian/obsidian.json"))
}
