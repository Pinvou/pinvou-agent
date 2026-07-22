use std::process::Command;

use super::super::linux_packages::validate_packages;

pub fn install_dependencies(packages: Vec<String>) -> Result<(), String> {
    validate_packages(&packages)?;
    let script = format!(
        "DEBIAN_FRONTEND=noninteractive apt-get install -y {}",
        packages.join(" ")
    );
    let output = Command::new("pkexec")
        .args(["sh", "-c", &script])
        .output()
        .map_err(|e| format!("pkexec 启动失败: {e}"))?;

    if output.status.success() {
        return Ok(());
    }
    let code = output.status.code().unwrap_or(-1);
    Err(match code {
        126 => "用户取消授权".to_string(),
        127 => "未授权或 pkexec 不可用".to_string(),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            format!("安装失败 (exit {code}): {}", tail.join(" / "))
        }
    })
}
