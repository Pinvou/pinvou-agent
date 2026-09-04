use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub(crate) struct HiddenCommand;

impl HiddenCommand {
    #[allow(clippy::new_ret_no_self)]
    pub(crate) fn new<S: AsRef<OsStr>>(program: S) -> Command {
        let mut command = Command::new(program);
        hide_std_console(&mut command);
        command
    }
}

fn is_windows_command_script(executable: &Path) -> bool {
    executable
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
}

fn external_command_for(executable: &Path, windows: bool) -> Command {
    let executable = crate::platform::os::external_application_path(executable);
    if windows && is_windows_command_script(&executable) {
        let mut command = HiddenCommand::new("cmd");
        command.args(["/D", "/S", "/C"]).arg(executable);
        command
    } else {
        HiddenCommand::new(executable)
    }
}

/// 构造隐藏窗口的外部 CLI 命令。Windows npm 生成的 `.cmd` / `.bat` shim
/// 必须经 `cmd /D /S /C`，否则探测、登录或启动 Agent 时会被当成原生可执行文件。
pub(crate) fn external_command(executable: &Path) -> Command {
    external_command_for(executable, crate::platform::capabilities::is_windows())
}

fn external_tokio_command_for(executable: &Path, windows: bool) -> tokio::process::Command {
    let executable = crate::platform::os::external_application_path(executable);
    if windows && is_windows_command_script(&executable) {
        let mut command = HiddenTokioCommand::new("cmd");
        command.args(["/D", "/S", "/C"]).arg(executable);
        command
    } else {
        HiddenTokioCommand::new(executable)
    }
}

/// `external_command` 的 Tokio 版本。
pub(crate) fn external_tokio_command(executable: &Path) -> tokio::process::Command {
    external_tokio_command_for(executable, crate::platform::capabilities::is_windows())
}

/// Capture a subprocess without pipe deadlocks and enforce a wall-clock timeout.
pub(crate) fn output_with_timeout(command: Command, timeout: Duration) -> Result<Output, String> {
    output_with_timeout_inner(command, timeout, false)
}

/// Capture a subprocess with a wall-clock timeout and terminate its process tree
/// on timeout. Use this for helpers that can launch privileged descendants: killing
/// only the wrapper can otherwise leave the real operation running with inherited
/// stdout/stderr pipes after the caller has reported a timeout.
pub(crate) fn output_with_timeout_and_kill_tree(
    mut command: Command,
    timeout: Duration,
) -> Result<Output, String> {
    std_process_group_leader(&mut command);
    output_with_timeout_inner(command, timeout, true)
}

fn output_with_timeout_inner(
    mut command: Command,
    timeout: Duration,
    kill_tree_on_timeout: bool,
) -> Result<Output, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("spawn {program} failed: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{program}: no stdout pipe"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{program}: no stderr pipe"))?;
    let stdout_reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let stderr_reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() <= timeout => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                if kill_tree_on_timeout {
                    let _ = kill_process_tree(child.id());
                }
                let _ = child.kill();
                let _ = child.wait();
                if kill_tree_on_timeout {
                    // A privileged descendant may be outside the caller's signal
                    // permission even after its wrapper is gone. Never block the
                    // timeout path by joining pipe readers that such a process kept.
                    drop(stdout_reader);
                    drop(stderr_reader);
                    return Err(format!(
                        "{program} timed out after {}s: subprocess tree termination requested",
                        timeout.as_secs()
                    ));
                }
                let stdout = stdout_reader.join().unwrap_or_default();
                let stderr = stderr_reader.join().unwrap_or_default();
                let detail = subprocess_output_detail(&stdout, &stderr);
                return Err(format!(
                    "{program} timed out after {}s: {detail}",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                if kill_tree_on_timeout {
                    let _ = kill_process_tree(child.id());
                }
                let _ = child.kill();
                let _ = child.wait();
                if kill_tree_on_timeout {
                    drop(stdout_reader);
                    drop(stderr_reader);
                    return Err(format!("{program} wait error: {error}"));
                }
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("{program} wait error: {error}"));
            }
        }
    };

    Ok(Output {
        status,
        stdout: stdout_reader.join().unwrap_or_default(),
        stderr: stderr_reader.join().unwrap_or_default(),
    })
}

fn subprocess_output_detail(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let stdout = stdout.trim();
    let stderr = stderr.trim();
    let detail = match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => "no subprocess output".to_string(),
        (true, false) => stderr.to_string(),
        (false, true) => stdout.to_string(),
        (false, false) => format!("stderr:\n{stderr}\nstdout:\n{stdout}"),
    };
    detail.chars().take(4000).collect()
}

/// 构造执行远程安装脚本的异步子进程命令：Windows 用 PowerShell
/// `irm <url> | iex`，其他平台用 `sh -c "curl -fsSL <url> | bash"`。
pub(crate) fn install_script_command(unix_url: &str, windows_url: &str) -> tokio::process::Command {
    if crate::platform::capabilities::is_windows() {
        let mut command = HiddenTokioCommand::new("powershell");
        command
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            // 先下载校验内容再执行：claude.ai 等官方站点会对非浏览器客户端
            // 间歇返回 Cloudflare 验证页（HTML/JS），直接 iex 会变成莫名其妙的
            // 解析错误且 stderr 为空；校验到 HTML 就给出可操作的中文错误。
            // 注意不能匹配任意 `<` 开头：kimi 官方脚本第一行是 `<#`（PowerShell
            // 块注释），`^\s*<` 会把它误判成验证页导致 kimi 永远装不上（实测）。
            // 只匹配真实 HTML 文档特征（Cloudflare 页以 <!DOCTYPE html> 开头）。
            .arg(format!(
                "$s = irm {windows_url}; if ($s -match '^\\s*<(html|!doctype|head|body|script)') {{ throw '官方站点返回了验证页而非安装脚本（可能是网络拦截或频控），请稍后重试' }}; iex $s"
            ));
        // Windows 开发机常见 PATH 顺序：Git for Windows 的 usr/bin 排在 System32
        // 前面，官方安装脚本调 tar 会命中 MSYS tar——盘符路径（C:\...）被当成
        // 「远程主机:路径」语法而失败（实测报错 Cannot execute remote shell）。
        // 把 System32 提到 PATH 最前，保证脚本拿到 Windows 原生工具。
        let system32 = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"))
            .join("System32");
        let mut sanitized_path = system32.into_os_string();
        if let Some(existing) = std::env::var_os("PATH") {
            sanitized_path.push(";");
            sanitized_path.push(existing);
        }
        command.env("PATH", sanitized_path);
        command
    } else {
        let mut command = HiddenTokioCommand::new("sh");
        command
            .arg("-c")
            .arg(format!("curl -fsSL {unix_url} | bash"));
        // Unix 上把安装进程放进**独立进程组**（组长 pid = 子进程 pid）：取消时
        // 按组杀（kill -9 -pgid）才能真正终止 curl | bash 派生的子进程，否则
        // 子 shell 孤儿化继续安装（评审中危项）。tokio 的 process_group 是
        // inherent 方法，无需 import。
        #[cfg(unix)]
        {
            command.process_group(0);
        }
        command
    }
}

/// Unix 上把（tokio）命令设为独立进程组组长（组长 pid = 子进程 pid）：
/// 取消安装时按组杀（kill -9 -pgid）能连 curl | bash / npm 派生的子进程
/// 一起终止，不孤儿化（评审中危项）。Windows no-op（taskkill /T 已杀整树）。
pub(crate) fn tokio_process_group_leader(command: &mut tokio::process::Command) {
    #[cfg(unix)]
    {
        // tokio 的 process_group 是 inherent 方法，无需 import。
        command.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

/// `tokio_process_group_leader` 的 std 版本（spawn_blocking 场景，如 Homebrew）。
pub(crate) fn std_process_group_leader(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

/// 按 pid 杀进程树：Windows 用 taskkill 杀整棵树（脚本会再起子 shell，单杀
/// 父进程会留下继续运行的子进程）；其他平台按进程组杀（负 pid）——安装进程
/// 以 `process_group(0)` 独立成组，组杀连 curl | bash / npm 派生的子进程一起
/// 终止，不孤儿化（评审中危项）。
///
/// **严禁**在本 crate 任何位置直接或间接（如经 connector_cli_command 以 kill 为
/// 程序名）spawn 外部 kill 可执行文件执行组杀，后续模块开发一律
/// 使用本函数 / `kill_pid_tree`（内部直接走 kill(2)）。这是硬性规则并由
/// `scripts/architecture-guard.py` 强制：procps-ng 4.0.4 的参数解析会把
/// `kill -9 -<pgid>` 的合法负 pid 错当 `-1` 处理（向内核发起 kill(-1)，杀光
/// 当前用户全部进程——2026-09-04 本机桌面会话两次被整台带走，audit 取证
/// argv 正确而系统调用为 kill(-1)，实锤）。任何新平台的组杀实现同理必须
/// 直调系统调用，不得委托外部工具。
///
/// 进程组已不存在（ESRCH）视为成功——目标已死即目的达成，调用方无需为
/// 「取消时进程恰好已退出」记失败日志。
///
/// `pid <= 1` 或无法以正数收入 `i32` 的 pid 一律拒绝（`InvalidInput`）：
/// kill(2) 对 0 与 -1 有特殊语义（0 = 调用方所在整组，-1 = 当前用户全部
/// 进程），`as i32` 回绕出的负 pid 同理。边界在本函数自检，不依赖调用方
/// 审计。
pub(crate) fn kill_process_tree(pid: u32) -> std::io::Result<()> {
    if crate::platform::capabilities::is_windows() {
        external_command(Path::new("taskkill"))
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output()?;
        return Ok(());
    }
    #[cfg(unix)]
    {
        let Some(group) = i32::try_from(pid).ok().filter(|group| *group > 1) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing user-wide process group kill for pid {pid}"),
            ));
        };
        // SAFETY: libc::kill is a direct kill(2) wrapper; no memory is touched.
        let sent = unsafe { libc::kill(-group, libc::SIGKILL) };
        if sent != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(error);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
    Ok(())
}

pub(crate) struct HiddenTokioCommand;

impl HiddenTokioCommand {
    #[allow(clippy::new_ret_no_self)]
    pub(crate) fn new<S: AsRef<OsStr>>(program: S) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(program);
        hide_tokio_console(&mut command);
        command
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn hide_std_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn hide_std_console(_command: &mut Command) {}

#[cfg(target_os = "windows")]
pub(crate) fn hide_tokio_console(command: &mut tokio::process::Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
pub(crate) fn hide_tokio_console(_command: &mut tokio::process::Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_command_shims_use_command_interpreter() {
        let command = external_command_for(Path::new(r"C:\Users\u\npm\kimi.cmd"), true);
        assert_eq!(command.get_program(), "cmd");
        assert_eq!(
            command
                .get_args()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["/D", "/S", "/C", r"C:\Users\u\npm\kimi.cmd"]
        );

        let command = external_tokio_command_for(Path::new(r"C:\Users\u\npm\claude.cmd"), true);
        assert_eq!(command.as_std().get_program(), "cmd");
        assert_eq!(
            command
                .as_std()
                .get_args()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["/D", "/S", "/C", r"C:\Users\u\npm\claude.cmd"]
        );
    }

    #[test]
    fn native_executables_do_not_use_command_interpreter() {
        let command = external_command_for(Path::new(r"C:\tools\kimi.exe"), true);
        assert_eq!(command.get_program(), r"C:\tools\kimi.exe");
        assert!(command.get_args().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn tree_timeout_does_not_wait_for_a_descendant_holding_the_pipes() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & wait"]);
        let started = Instant::now();

        let error =
            output_with_timeout_and_kill_tree(command, Duration::from_millis(100)).unwrap_err();

        assert!(error.contains("timed out after"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// Unix group kills must go through kill(2) directly. This is the
    /// regression test for the 2026-09-04 desktop-session massacres: the
    /// timeout path spawned external `/usr/bin/kill -9 -<pgid>` and
    /// procps-ng 4.0.4 misparsed the valid negative pid as -1 (kill(-1)
    /// signals every process of this user). A fake `kill` is placed first
    /// on PATH: if the implementation ever spawns an external kill again,
    /// the marker file appears and the test fails — before considering
    /// what that binary would do to the machine.
    #[cfg(unix)]
    #[test]
    fn kill_process_tree_terminates_group_without_spawning_external_kill() {
        use std::sync::Mutex;

        static PATH_LOCK: Mutex<()> = Mutex::new(());
        let _path_lock = PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        let work = std::env::temp_dir().join(format!(
            "pinvou3-kill-process-tree-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&work).expect("create test work dir");
        let marker = work.join("external-kill-invoked");
        let sleep_pid_file = work.join("sleep-pid");
        let fake_kill_bin = work.join("kill");
        std::fs::write(
            &fake_kill_bin,
            format!("#!/bin/sh\necho \"$@\" >> {}\nexit 42\n", marker.display()),
        )
        .expect("write fake kill");
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&fake_kill_bin, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake kill");

        let previous_path = std::env::var_os("PATH");
        let mut split_path = previous_path
            .as_ref()
            .map(|value| std::env::split_paths(value).collect::<Vec<_>>())
            .unwrap_or_default();
        split_path.insert(0, work.clone());
        // SAFETY: this test serializes on PATH_LOCK; a prepended dir cannot
        // break other tests that merely resolve commands through PATH.
        unsafe { std::env::set_var("PATH", std::env::join_paths(&split_path).unwrap()) };

        let mut command = Command::new("sh");
        command.args([
            "-c",
            &format!("sleep 30 & echo $! > {}; wait", sleep_pid_file.display()),
        ]);
        std_process_group_leader(&mut command);
        let mut child = command.spawn().expect("spawn sh group leader");
        let sh_pid = child.id();

        let mut sleep_pid = None;
        for _ in 0..40 {
            if let Ok(text) = std::fs::read_to_string(&sleep_pid_file) {
                if let Ok(value) = text.trim().parse::<u32>() {
                    sleep_pid = Some(value);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let sleep_pid = sleep_pid.expect("descendant must report its pid");

        let kill_result = kill_process_tree(sh_pid);
        let _ = child.wait();

        // SAFETY: libc::kill with sig=0 only probes liveness; no memory is touched.
        let alive = |pid: u32| unsafe { libc::kill(pid as i32, 0) == 0 };
        let mut both_dead = !alive(sh_pid) && !alive(sleep_pid);
        for _ in 0..100 {
            if both_dead {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
            both_dead = !alive(sh_pid) && !alive(sleep_pid);
        }
        // The orphaned descendant is a member of this test's process tree;
        // make sure no stray sleeper survives even if the asserts fail.
        // SAFETY: libc::kill is a direct kill(2) wrapper; no memory is touched.
        let _ = unsafe { libc::kill(sleep_pid as i32, libc::SIGKILL) };

        // SAFETY: holding PATH_LOCK; restoring the saved value.
        unsafe {
            match previous_path {
                Some(value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
        }
        let _ = std::fs::remove_dir_all(&work);

        assert!(
            kill_result.is_ok(),
            "group kill must succeed for a live group: {kill_result:?}"
        );
        assert!(
            both_dead,
            "group leader {sh_pid} and descendant {sleep_pid} must both die"
        );
        assert!(
            !marker.exists(),
            "an external kill was spawned; group kills must use kill(2) directly"
        );
    }
}
