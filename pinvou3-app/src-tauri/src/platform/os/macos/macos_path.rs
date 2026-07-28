use std::path::PathBuf;

pub fn user_home_dir() -> PathBuf {
    // 与 unsupported.rs 对齐:HOME 缺失时用 std::env::temp_dir()(而非硬编码 "/tmp")。
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

pub fn pandoc_tool_path() -> PathBuf {
    PathBuf::from("pandoc")
}
