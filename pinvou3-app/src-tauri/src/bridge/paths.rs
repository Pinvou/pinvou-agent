//! `~/.pinvou3/` 目录布局解析。
//!
//! pinvou3-app 不读 `~/.deepseek/`（隔离），所有 deepseek-tui 默认会写到
//! 全局/cwd 的字段都映射到这个独立目录树。布局参见 plan「目录布局」一节。
//!
//! `PINVOU3_HOME` 环境变量可整体重定位（主要用于测试）。

use std::path::PathBuf;

/// `~/.pinvou3/` 根目录。
pub fn pinvou3_home() -> PathBuf {
    if let Ok(custom) = std::env::var("PINVOU3_HOME") {
        return PathBuf::from(custom);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".pinvou3")
}

pub fn settings_path() -> PathBuf {
    pinvou3_home().join("settings.json")
}

pub fn bundle_root() -> PathBuf {
    pinvou3_home().join("bundle")
}
pub fn bundle_instructions() -> PathBuf {
    bundle_root().join("instructions.md")
}
pub fn bundle_skills_dir() -> PathBuf {
    bundle_root().join("skills")
}
pub fn bundle_mcp_json() -> PathBuf {
    bundle_root().join("mcp.json")
}
pub fn bundle_version_file() -> PathBuf {
    bundle_root().join("VERSION")
}

pub fn user_root() -> PathBuf {
    pinvou3_home().join("user")
}
pub fn user_instructions() -> PathBuf {
    user_root().join("instructions.md")
}
pub fn user_skills_dir() -> PathBuf {
    user_root().join("skills")
}

pub fn workspace_dir() -> PathBuf {
    pinvou3_home().join("workspace")
}
pub fn notes_path() -> PathBuf {
    pinvou3_home().join("notes.md")
}
pub fn memory_path() -> PathBuf {
    pinvou3_home().join("memory.md")
}
pub fn mcp_config_path() -> PathBuf {
    bundle_mcp_json()
}

/// 首次启动确保所有目录存在。bundle/skills 等子目录在解包时还会再 ensure 一次。
pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(bundle_skills_dir())?;
    std::fs::create_dir_all(user_skills_dir())?;
    std::fs::create_dir_all(workspace_dir())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinvou3_home_respects_env_override() {
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-test-override");
        assert_eq!(pinvou3_home(), PathBuf::from("/tmp/pinvou3-test-override"));
        assert_eq!(
            settings_path(),
            PathBuf::from("/tmp/pinvou3-test-override/settings.json")
        );
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }
}
