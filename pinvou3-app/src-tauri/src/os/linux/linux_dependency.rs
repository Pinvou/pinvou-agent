use std::process::Command;

const KNOWN_DEP_PACKAGES: &[&str] = &[
    "poppler-utils",
    "pandoc",
    "libreoffice",
    "tesseract-ocr",
    "tesseract-ocr-chi-sim",
    "p7zip-full",
    "python3",
    "libemail-outlook-message-perl",
];

pub fn install_dependencies(packages: Vec<String>) -> Result<(), String> {
    if packages.is_empty() {
        return Err("没有需要安装的依赖".into());
    }
    for p in &packages {
        if !KNOWN_DEP_PACKAGES.contains(&p.as_str()) {
            return Err(format!("非法包名（不在依赖白名单内）: {p}"));
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
