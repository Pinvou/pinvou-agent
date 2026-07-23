use std::path::PathBuf;

pub(crate) fn eip_bin_path() -> Result<PathBuf, String> {
    unsupported("eip-cli")
}

pub(crate) fn zhidao_bin_path() -> Result<PathBuf, String> {
    unsupported("zhidao-cli")
}

fn unsupported(label: &str) -> Result<PathBuf, String> {
    Err(format!("当前平台不支持内置 {label}"))
}
