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

pub(crate) fn python_command() -> Command {
    HiddenCommand::new(crate::platform::paths::python_command())
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
pub(crate) fn output_with_timeout(
    mut command: Command,
    timeout: Duration,
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
                let _ = child.kill();
                let _ = child.wait();
                let stdout = stdout_reader.join().unwrap_or_default();
                let stderr = stderr_reader.join().unwrap_or_default();
                let detail = subprocess_output_detail(&stdout, &stderr);
                return Err(format!(
                    "{program} timed out after {}s: {detail}",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
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
/// 父进程会留下继续运行的子进程）；Unix 通过 `kill(2)` 按进程组杀（负 pid）——安装进程
/// 以 `process_group(0)` 独立成组，`SIGKILL` 连 curl | bash / npm 派生
/// 的子进程一起终止，不孤儿化（评审中危项）。
pub(crate) fn kill_process_tree(pid: u32) -> std::io::Result<()> {
    if pid == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "process-group id must be positive",
        ));
    }

    #[cfg(unix)]
    {
        let process_group = i32::try_from(pid).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "process-group id exceeds the platform pid range",
            )
        })?;
        // 负 pid 是 POSIX 进程组寻址。直接使用内核接口，避免受用户 PATH、shell
        // alias 或同名 shim 影响；调用方只会传由我们创建并登记的 group leader。
        if unsafe { libc::kill(-process_group, libc::SIGKILL) } == 0 {
            Ok(())
        } else {
            let error = std::io::Error::last_os_error();
            // 幂等 stop：组已经自然退出也等价于目标状态已达到。其他错误（尤其 EPERM）
            // 必须保留给上层，不能清掉 ownership 后假报已停止。
            if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(())
            } else {
                Err(error)
            }
        }
    }

    #[cfg(windows)]
    {
        let system_root = std::env::var_os("SystemRoot")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "SystemRoot is unavailable for fixed taskkill path",
                )
            })?;
        let taskkill = std::path::PathBuf::from(system_root)
            .join("System32")
            .join("taskkill.exe");
        let pid_arg = pid.to_string();
        let output = HiddenCommand::new(taskkill)
            .args(["/PID", pid_arg.as_str(), "/T", "/F"])
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "taskkill exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "process-tree termination is unsupported on this platform",
        ))
    }
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
    fn process_tree_rejects_zero_pid_before_platform_dispatch() {
        let error = kill_process_tree(0).expect_err("pid zero must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_process_group_kills_a_real_descendant() {
        let mut command = Command::new("sh");
        std_process_group_leader(&mut command);
        command
            .args(["-c", "sleep 30 & wait"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().expect("spawn owned process group");
        let root_pid = child.id();
        let process_group = unsafe { libc::getpgid(root_pid as libc::pid_t) };
        assert_eq!(process_group, root_pid as libc::pid_t);

        let children_path = format!("/proc/{root_pid}/task/{root_pid}/children");
        let deadline = Instant::now() + Duration::from_secs(2);
        let descendant_pid = loop {
            if let Ok(children) = std::fs::read_to_string(&children_path) {
                if let Some(pid) = children
                    .split_ascii_whitespace()
                    .find_map(|pid| pid.parse::<u32>().ok())
                {
                    break pid;
                }
            }
            assert!(
                Instant::now() < deadline,
                "owned process group did not spawn a descendant"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(linux_process_is_running(descendant_pid));

        kill_process_tree(root_pid).expect("kill owned process group");
        child.wait().expect("reap process group leader");
        let exit_deadline = Instant::now() + Duration::from_secs(2);
        while linux_process_is_running(descendant_pid) && Instant::now() < exit_deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !linux_process_is_running(descendant_pid),
            "owned descendant survived process-group termination"
        );
    }

    #[cfg(target_os = "linux")]
    fn linux_process_is_running(pid: u32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        // `/proc/<pid>/stat` is `pid (comm) state ...`; comm may contain spaces or `)`.
        let state = stat
            .rsplit_once(") ")
            .and_then(|(_, tail)| tail.chars().next());
        !matches!(state, Some('Z' | 'X'))
    }

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
}
