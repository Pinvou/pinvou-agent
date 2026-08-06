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
            .arg(format!("irm {windows_url} | iex"));
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
        command
    }
}

/// 按 pid 杀进程树：Windows 用 taskkill 杀整棵树（脚本会再起子 shell，单杀
/// 父进程会留下继续运行的子进程）；其他平台 kill -9（尽力而为）。
pub(crate) fn kill_process_tree(pid: u32) -> std::io::Result<Output> {
    let pid_arg = pid.to_string();
    if crate::platform::capabilities::is_windows() {
        external_command(Path::new("taskkill"))
            .args(["/PID", pid_arg.as_str(), "/T", "/F"])
            .output()
    } else {
        external_command(Path::new("kill"))
            .args(["-9", pid_arg.as_str()])
            .output()
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
