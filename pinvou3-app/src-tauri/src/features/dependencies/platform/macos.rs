use std::io::BufRead;
use std::io::BufReader;
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

/// 运行一次 brew 调用,逐行流式上报 stdout/stderr 给进度回调,
/// 并在失败时汇总最后几行 stderr。`args` 以 `["install"[, "--cask"], name]` 形式传入。
///
/// 返回 `Ok(())` 表示该包安装成功(exit 0),`Err(message)` 表示失败。
fn run_brew(
    args: &[&str],
    progress: Option<&dyn Fn(&str, usize, usize, Option<&str>)>,
    package: &str,
    current: usize,
    total: usize,
) -> Result<(), String> {
    let mut child = Command::new(brew_bin())
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "brew 启动失败(请确认已装 Homebrew: https://brew.sh): {e}\n  探测路径: /opt/homebrew/bin/brew, /usr/local/bin/brew"
            )
        })?;
    // 逐行排空两个管道。stderr 收集最后几行用于失败汇总;stdout/stderr 每读到非空
    // 行就回调一次,让前端看到 brew 实时输出(libreoffice 下载进度等)。
    let stderr = child
        .stderr
        .take()
        .map(|s| drain_lines(s, progress, package, current, total))
        .unwrap_or_default();
    if let Some(out) = child.stdout.take() {
        drain_lines(out, progress, package, current, total);
    }
    let output = child
        .wait()
        .map_err(|e| format!("brew 等待失败: {e}"))?;
    if output.success() {
        return Ok(());
    }
    let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
    Err(format!(
        "{} 安装失败 (exit {}): {}",
        package,
        output.code().unwrap_or(-1),
        tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
    ))
}

/// 逐行读取一个管道,对每条非空行触发进度回调;返回累积的全部文本。
/// 对 stdout 与 stderr 各调一次。与 connectors 的 `drain_for_url` 同模式
/// (BufReader::new(..).lines()),但这里在阻塞线程内联读,无需另起线程。
fn drain_lines<R: std::io::Read>(
    stream: R,
    progress: Option<&dyn Fn(&str, usize, usize, Option<&str>)>,
    package: &str,
    current: usize,
    total: usize,
) -> String {
    let mut buf = String::new();
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break, // 管道读取错误不可恢复,停止排空。
        };
        if let Some(report) = progress {
            if !line.trim().is_empty() {
                report(package, current, total, Some(line.as_str()));
            }
        }
        buf.push_str(&line);
        buf.push('\n');
    }
    buf
}


/// - `package`: 当前正在安装的包名
/// - `current` / `total`: 1-based 序号 / 本批待装总数(含 formula 与 cask)
/// - `detail`: brew 输出的最新一行(如 `Downloading libreoffice … 45%`),
///   安装开始前为 `None`
///
/// 平台适配器只持有这个纯 Rust 回调,不依赖 Tauri;由 features 域层
/// (file_ingest.rs)把它转成 `app.emit("deps:install_progress", …)`。
pub fn install_dependencies(
    packages: Vec<String>,
    progress: Option<&dyn Fn(&str, usize, usize, Option<&str>)>,
) -> Result<(), String> {
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

    // 逐包安装并流式上报进度,而非一次性 `brew install a b c` 阻塞到整批结束。
    // 1) 逐包:每装完一个包就推进 `current`,给出「正在安装 X (n/总数)」的真实进度;
    //    对 ~6 个小批次,逐包调用的额外开销可忽略。
    // 2) 流式:逐行读 brew stdout/stderr 并回调,让长尾包(libreoffice cask 数十分钟)
    //    的「Downloading … 45%」实时可见,不再像卡死。BufReader.lines() 是阻塞读,
    //    本函数已运行在 spawn_blocking 线程,内联读即可,无需另起线程。
    //
    // current 是全局 1-based 序号(跨 formula 与 cask 连续),total 是本批待装总数。
    let total = formulas.len() + casks.len();
    let mut current = 0usize;

    // formula 安装(brew install),逐包。
    for name in &formulas {
        current += 1;
        if let Some(report) = progress {
            report(name, current, total, None);
        }
        match run_brew(&["install", name], progress, name, current, total) {
            Ok(()) => {}
            Err(err) => errors.push(err),
        }
    }

    // cask 安装(brew install --cask),逐包。
    for name in &casks {
        current += 1;
        if let Some(report) = progress {
            report(name, current, total, None);
        }
        match run_brew(&["install", "--cask", name], progress, name, current, total) {
            Ok(()) => {}
            Err(err) => errors.push(err),
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // drain_lines 应对每条非空行触发一次进度回调,并把全部文本累积返回。
    // 这覆盖流式进度的核心机制——让前端看到 brew 实时输出(libreoffice 下载进度等)。
    #[test]
    fn drain_lines_reports_each_non_empty_line() {
        let calls: Arc<Mutex<Vec<(String, usize, usize, Option<String>)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let calls_clone = calls.clone();
        let report = move |pkg: &str, cur: usize, total: usize, detail: Option<&str>| {
            calls_clone.lock().unwrap().push((
                pkg.to_string(),
                cur,
                total,
                detail.map(str::to_string),
            ));
        };
        let input = "Downloading foo\n\nInstalling foo\n";
        let buf = drain_lines(input.as_bytes(), Some(&report), "foo", 1, 2);
        // 空行被跳过,两行非空各回调一次。
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].0, "foo");
        assert_eq!(recorded[0].1, 1);
        assert_eq!(recorded[0].2, 2);
        assert_eq!(recorded[0].3.as_deref(), Some("Downloading foo"));
        assert_eq!(recorded[1].3.as_deref(), Some("Installing foo"));
        // 累积文本含两行(各带换行)。
        assert!(buf.contains("Downloading foo"));
        assert!(buf.contains("Installing foo"));
    }

    // 白名单应拒绝未知包名(安全护栏,防注入任意 brew 包)。
    #[test]
    fn rejects_unknown_package() {
        let err = install_dependencies(vec!["not-a-real-package".into()], None).unwrap_err();
        assert!(err.contains("非法包名"));
    }

    // 空包名(如 email_dependency_packages 返回 "")应被过滤,而非误传 brew。
    #[test]
    fn filters_empty_package_names() {
        let err = install_dependencies(vec!["".into()], None).unwrap_err();
        assert_eq!(err, "没有需要安装的依赖");
    }
}
