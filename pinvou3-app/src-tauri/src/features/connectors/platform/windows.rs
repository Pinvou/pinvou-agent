use std::path::PathBuf;

pub(crate) fn eip_bin_path() -> Result<PathBuf, String> {
    binary_path("eip", "eip-cli.exe", "eip-cli")
}

pub(crate) fn zhidao_bin_path() -> Result<PathBuf, String> {
    binary_path("zhidao", "zhidao-cli.exe", "zhidao CLI")
}

fn binary_path(skill: &str, name: &str, label: &str) -> Result<PathBuf, String> {
    let path = crate::platform::paths::bundle_skills_dir()
        .join(skill)
        .join("bin")
        .join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "{label} 未找到: {}（需先把对应技能二进制打包进 bundle）",
            path.display()
        ))
    }
}
