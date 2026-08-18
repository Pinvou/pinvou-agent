use std::io::BufReader;
use std::io::Read;
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
///
/// stdout 与 stderr 用两个作用域线程**并发**排空,而非先排空一个再排空另一个:
/// 否则若 brew 在未被读取的那个管道上写满 OS 管道缓冲(macOS 通常 64KB),
/// 写入会阻塞并连带拖死正在读取的另一端 —— 这是 `std::process::Child` 文档明确
/// 警告的反模式。connectors(`connector_cli::drain_for_url`)正是给每个管道各开
/// 一个线程来规避;这里在 `spawn_blocking` 线程内用 `thread::scope` 同模式处理,
/// scope 保证两个排空线程在 `wait()` 前都 join 完,借用无需 `'static`。
fn run_brew(
    args: &[&str],
    progress: Option<&(dyn Fn(&str, usize, usize, Option<&str>) + Sync)>,
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
    // 两个作用域线程并发排空 stdout/stderr:每读到非空行就回调一次,让前端看到
    // brew 实时输出(libreoffice 下载进度等);stderr 另存累积文本用于失败汇总。
    // scope 在返回前 join 两个线程,排空完毕后才 wait(),不存在管道写满死锁。
    let (stdout, stderr) = drain_child_pipes(&mut child, progress, package, current, total);
    let _ = stdout; // stdout 仅用于实时回调,这里不参与错误汇总。
    let output = child.wait().map_err(|e| format!("brew 等待失败: {e}"))?;
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

/// 两个作用域线程并发排空子进程的 stdout/stderr,返回 `(stdout, stderr)` 累积文本。
/// stderr 线程负责累积全部文本(失败时取最后几行汇总);stdout 线程只回调不存。
/// scope 在返回前 join 两个线程,借用无需 `'static`;必须排空完毕后才 `wait()`,
/// 否则未读管道写满 OS 缓冲会阻塞子进程(见 `run_brew` 注释)。
fn drain_child_pipes(
    child: &mut std::process::Child,
    progress: Option<&(dyn Fn(&str, usize, usize, Option<&str>) + Sync)>,
    package: &str,
    current: usize,
    total: usize,
) -> (String, String) {
    std::thread::scope(|s| {
        let stderr_handle = child
            .stderr
            .take()
            .map(|pipe| s.spawn(move || drain_lines(pipe, progress, package, current, total)));
        let stdout_handle = child
            .stdout
            .take()
            .map(|pipe| s.spawn(move || drain_lines(pipe, progress, package, current, total)));
        let stderr = stderr_handle
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        let stdout = stdout_handle
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        (stdout, stderr)
    })
}

/// 逐行读取一个管道,对每条非空行触发进度回调;返回累积的全部文本。
/// 对 stdout 与 stderr 各在一个作用域线程里调用一次(见 `run_brew`),故 `progress`
/// 必须可跨线程共享调用(`+ Sync`)。
///
/// 除 `\n` 外也按 `\r` 切分:curl 式下载进度(`Downloading … 45%`)用 `\r` 在同一行
/// 内覆盖刷新、中间不带 `\n`。按 chunk 读取并逐字节扫描两个分隔符,每个 `\r` 快照
/// 一到就立刻回调,无需等行尾 `\n`;只按 `\n`(或先 read_until 再 split)会把所有
/// 百分比缓冲到该行结束才一次性吐出,回调时机不是实时的。
fn drain_lines<R: std::io::Read>(
    stream: R,
    progress: Option<&(dyn Fn(&str, usize, usize, Option<&str>) + Sync)>,
    package: &str,
    current: usize,
    total: usize,
) -> String {
    let mut buf = String::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut reader = BufReader::new(stream);
    let mut chunk = [0u8; 4096];
    loop {
        let n = match reader.read(&mut chunk) {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(_) => break, // 管道读取错误不可恢复,停止排空。
        };
        // 逐字节扫描:遇到 `\r` 或 `\n` 即切分上报当前累积段(跨 chunk 继续累积),
        // 让 curl 式 `\r` 进度快照在到达时立即回调,而非缓冲到行尾批量 split。
        for &b in &chunk[..n] {
            if b == b'\r' || b == b'\n' {
                let segment = String::from_utf8_lossy(&pending);
                let line = segment.trim();
                if !line.is_empty() {
                    if let Some(report) = progress {
                        report(package, current, total, Some(line));
                    }
                }
                // 保留分隔符到累积文本(失败时按行汇总 stderr 用),再清空当前段。
                buf.push_str(&segment);
                buf.push(b as char);
                pending.clear();
            } else {
                pending.push(b);
            }
        }
    }
    // EOF 前的最后一段(无 `\r`/`\n` 结尾,如进程被 kill)也要上报并保留。
    let segment = String::from_utf8_lossy(&pending);
    let line = segment.trim();
    if !line.is_empty() {
        if let Some(report) = progress {
            report(package, current, total, Some(line));
        }
    }
    buf.push_str(&segment);
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
    progress: Option<&(dyn Fn(&str, usize, usize, Option<&str>) + Sync)>,
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

    // curl 式下载进度用 \r 在同一行内覆盖刷新百分比(中间无 \n)。
    // drain_lines 应按 \r 切分,让每个百分比快照都实时上报,而非缓冲到行尾。
    #[test]
    fn drain_lines_splits_carriage_return_progress_segments() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let calls_clone = calls.clone();
        let report = move |_pkg: &str, _cur: usize, _total: usize, detail: Option<&str>| {
            if let Some(line) = detail {
                calls_clone.lock().unwrap().push(line.to_string());
            }
        };
        // 模拟 curl 下载:同一行用 \r 反复覆盖刷新百分比,最后 \n 结束。
        let input = "Downloading X\rDownloading X  45%\rDownloading X done\n";
        let _ = drain_lines(input.as_bytes(), Some(&report), "X", 1, 1);
        let recorded = calls.lock().unwrap();
        assert_eq!(
            recorded.as_slice(),
            &[
                "Downloading X".to_string(),
                "Downloading X  45%".to_string(),
                "Downloading X done".to_string(),
            ]
        );
    }

    // 真实管道延迟写入:`\r` 段先到达、`\n` 段 200ms 后才到达。若 drain_lines 等到
    // 行尾 `\n` 才批量切分,`\r` 段会被缓冲到最终换行才一次性回调(时间差≈0);
    // 流式实现应在 `\r` 段到达时立即回调,因此两条回调的时间差应显著大于零。
    #[test]
    fn drain_lines_reports_cr_segment_before_final_newline() {
        let calls: Arc<Mutex<Vec<(String, std::time::Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let calls_clone = calls.clone();
        let report = move |_pkg: &str, _cur: usize, _total: usize, detail: Option<&str>| {
            if let Some(line) = detail {
                calls_clone
                    .lock()
                    .unwrap()
                    .push((line.to_string(), std::time::Instant::now()));
            }
        };
        let report: &(dyn Fn(&str, usize, usize, Option<&str>) + Sync) = &report;
        // 用真实子进程模拟 brew 的 curl 进度:stdout 先写 "A\r"(无换行),sleep 后再写 "B\n"。
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "printf 'A\\r'; sleep 0.2; printf 'B\\n'"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sh");
        let pipe = child.stdout.take().expect("take stdout");
        let _ = drain_lines(pipe, Some(report), "x", 1, 1);
        child.wait().expect("wait sh");
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].0, "A");
        assert_eq!(recorded[1].0, "B");
        // A(\r 段)在 B(\n 段)之前到达;若实现把 \r 缓冲到行尾,二者几乎同时回调。
        assert!(
            recorded[1].1.duration_since(recorded[0].1) >= std::time::Duration::from_millis(100),
            "\\r 进度段应在最终换行前实时回调,而不是缓冲到行尾批量上报"
        );
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

    // run_brew 用两个作用域线程并发排空 stdout/stderr(防管道写满死锁)。
    // 两侧的行都应能到达回调,且回调从不同线程并发调用 —— 本测试用足够小的输入
    // 验证「两侧都被读到、无丢行」,死锁则会表现为 hang(测试超时)。
    #[test]
    fn run_brew_concurrent_drain_reports_lines_from_both_pipes() {
        // 用一个会快速结束的 sh 子进程作为 brew 替身(stdout 写两条、stderr 写一条),
        // 直接驱动 run_brew 使用的 drain_child_pipes 编排,验证两个管道的行都进了回调。
        let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let calls_clone = calls.clone();
        let report = move |_pkg: &str, _cur: usize, _total: usize, detail: Option<&str>| {
            if let Some(line) = detail {
                calls_clone.lock().unwrap().push(line.to_string());
            }
        };
        // `&report` 是 Copy,可被两个 move 闭包各自复制一份 —— 与 run_brew 里
        // `progress: Option<&dyn … + Sync>` 在两个 s.spawn 间被复制同构。
        let report: &(dyn Fn(&str, usize, usize, Option<&str>) + Sync) = &report;
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "printf 'out-a\\nout-b\\n' >&1; printf 'err-c\\n' >&2"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn sh");
        let (stdout, stderr) = drain_child_pipes(&mut child, Some(report), "x", 1, 1);
        child.wait().expect("wait sh");
        // 三行都到达回调(stdout 两行 + stderr 一行),无丢行。
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 3);
        let mut all = recorded.clone();
        all.sort();
        assert_eq!(
            all,
            vec![
                "err-c".to_string(),
                "out-a".to_string(),
                "out-b".to_string()
            ]
        );
        // 累积文本分别只含各自管道的内容。
        assert!(stdout.contains("out-a") && stdout.contains("out-b") && !stdout.contains("err-c"));
        assert!(stderr.contains("err-c") && !stderr.contains("out-a"));
    }
}
