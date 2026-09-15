use std::process::Command;

use super::linux_packages::validate_packages;

/// `progress` 回调签名见 macOS 侧文档 `(package, current, total, detail)`。
/// Linux 用 pkexec apt 一次性安装整批,只在执行前发一次粗粒度进度(无逐行 brew
/// 输出可流式),保持既有行为不变。
pub fn install_dependencies(
    packages: Vec<String>,
    progress: Option<&(dyn Fn(&str, usize, usize, Option<&str>) + Sync)>,
) -> Result<(), String> {
    validate_packages(&packages)?;
    if let Some(report) = progress {
        if let Some(first) = packages.first() {
            report(first, 1, 1, None);
        }
    }
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
