use std::process::Command;

const KNOWN_DEP_PACKAGES: &[&str] = &[
    "poppler",
    "pandoc",
    "libreoffice",
    "tesseract",
    "tesseract-lang",
    "p7zip",
    "python@3.12",
    "python@3.13",
    "ffmpeg",
    // 多包字符串(ocr_dependency_packages 返回 "tesseract tesseract-lang")
    "tesseract tesseract-lang",
];

/// Cask 类包(brew install --cask),与 formula(brew install)分开调用。
/// libreoffice 是 cask(GUI 应用),brew install libreoffice 在部分 Homebrew 版本
/// 会报 "No available formula" 并导致整批安装失败。
const CASK_PACKAGES: &[&str] = &["libreoffice"];

/// 解析 brew 绝对路径。GUI 启动的 app 通常不继承 shell 的 PATH,
/// `Command::new("brew")` 会拿到 NotFound。先探测 Apple Silicon (/opt/homebrew/bin/brew)
/// 与 Intel (/usr/local/bin/brew) 两个标准位置,都没找到才回退 PATH 查找。
fn brew_bin() -> &'static str {
    for candidate in ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"] {
        if std::path::Path::new(candidate).is_file() {
            return candidate;
        }
    }
    "brew"
}

/// 检测 Homebrew 是否真的可用。brew_bin() 回退到裸 "brew" 时,
/// 仅靠 `Command::new("brew")` 的 NotFound 判断太晚——用户看到的是
/// 含「请确认已装 Homebrew」的技术性错误,而非可操作的指引。
/// 提前检测:没有 Homebrew 就直接返回友好错误,列出各工具官网。
fn brew_available() -> bool {
    // brew_bin() 返回非 "brew" 说明标准路径下找到了 brew,一定可用。
    if brew_bin() != "brew" {
        return true;
    }
    // 回退到裸 "brew":走 which 检查是否在 PATH 中(覆盖非标准安装位置)。
    Command::new("/usr/bin/which")
        .arg("brew")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// brew 不可用时返回的可操作错误:列出 Homebrew 安装页 + 各工具官网,
/// 让无 Homebrew 的用户也有路可走(而非只能装 Homebrew)。
fn brew_not_found_error(packages: &[String]) -> String {
    format!(
        "未检测到 Homebrew。一键安装依赖需要 Homebrew,可从 https://brew.sh 安装。\n\
         或手动安装以下工具: {}\n\
         各工具官网:\n\
         - poppler: https://poppler.freedesktop.org\n\
         - pandoc: https://pandoc.org/installing.html\n\
         - libreoffice: https://www.libreoffice.org/download\n\
         - tesseract: https://tesseract-ocr.github.io/tessdoc/Installation.html\n\
         - p7zip: https://www.7-zip.org\n\
         - python: https://www.python.org/downloads\n\
         - ffmpeg: https://ffmpeg.org/download.html",
        packages.join(", ")
    )
}

pub fn install_dependencies(packages: Vec<String>) -> Result<(), String> {
    if packages.is_empty() {
        return Err("没有需要安装的依赖".into());
    }
    // 部分依赖函数返回空字符串(如 email_dependency_packages),过滤掉避免
    // 误传空包名给 brew。
    let packages: Vec<String> = packages
        .into_iter()
        .filter(|p| !p.trim().is_empty())
        .collect();
    if packages.is_empty() {
        return Err("没有需要安装的依赖".into());
    }
    // 白名单校验 + 展开多包字符串(如 "tesseract tesseract-lang" → 两个独立包名)。
    let mut expanded: Vec<String> = Vec::new();
    for p in &packages {
        if !KNOWN_DEP_PACKAGES.contains(&p.as_str()) {
            return Err(format!("非法包名（不在依赖白名单内）: {p}"));
        }
        for part in p.split_whitespace() {
            expanded.push(part.to_string());
        }
    }
    let packages = expanded;
    // 不假定用户装了 Homebrew:brew 不可用时提前返回可操作错误,
    // 列出各工具官网让用户有替代安装路径(而非卡在 brew NotFound)。
    if !brew_available() {
        return Err(brew_not_found_error(&packages));
    }
    // 区分 formula 与 cask:libreoffice 是 cask,需 --cask;其余是 formula。
    // 不区分会导致 brew install libreoffice 在部分版本报错并中断整批安装。
    let (casks, formulas): (Vec<&String>, Vec<&String>) = packages
        .iter()
        .partition(|p| CASK_PACKAGES.contains(&p.as_str()));

    let mut errors: Vec<String> = Vec::new();

    // formula 安装(brew install)。
    if !formulas.is_empty() {
        let formula_names: Vec<&str> = formulas.iter().map(|s| s.as_str()).collect();
        let output = Command::new(brew_bin())
            .arg("install")
            .args(&formula_names)
            .output()
            .map_err(|e| {
                format!(
                    "brew 启动失败(请确认已装 Homebrew: https://brew.sh): {e}\n  探测路径: /opt/homebrew/bin/brew, /usr/local/bin/brew"
                )
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
            errors.push(format!(
                "formula 安装失败 (exit {}): {}",
                output.status.code().unwrap_or(-1),
                tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
            ));
        }
    }

    // cask 安装(brew install --cask)。
    if !casks.is_empty() {
        let cask_names: Vec<&str> = casks.iter().map(|s| s.as_str()).collect();
        let output = Command::new(brew_bin())
            .args(["install", "--cask"])
            .args(&cask_names)
            .output()
            .map_err(|e| format!("brew --cask 启动失败: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
            errors.push(format!(
                "cask 安装失败 (exit {}): {}",
                output.status.code().unwrap_or(-1),
                tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}
