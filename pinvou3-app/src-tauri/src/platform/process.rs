use std::ffi::OsStr;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

pub(crate) struct HiddenCommand;

impl HiddenCommand {
    #[allow(clippy::new_ret_no_self)]
    pub(crate) fn new<S: AsRef<OsStr>>(program: S) -> Command {
        let mut command = Command::new(program);
        hide_std_console(&mut command);
        command
    }
}

/// Strips `GIT_*` override variables (redirection/config-injection classes)
/// injected by the host environment. `GIT_DIR`/`GIT_WORK_TREE`/
/// `GIT_INDEX_FILE`/`GIT_OBJECT_DIRECTORY` exported by the launching shell
/// redirect the module's internal git operations to unrelated repositories,
/// indexes, or object stores; `GIT_CONFIG*` injection overrides the target
/// repository's own configuration; `GIT_CEILING_DIRECTORIES` and friends alter
/// repository discovery — these variables really exist when the GUI is
/// launched from a development shell. Features that spawn git internally
/// (code_checkpoints shadow repositories, codex_acp workspace branch/diff
/// operations) must call this before spawning.
///
/// **Must remove keys one by one from a fixed key list; do not iterate
/// `env::vars_os()` first and delete matches**: iterating `environ` while other
/// threads run `setenv`/`remove_var` concurrently can miss keys (glibc environ
/// mutation is not thread-safe); parallel tests have occasionally lost
/// isolation entirely this way (the 2026-09-12 flaky family). Once
/// `GIT_CONFIG_COUNT` is removed, git ignores the `GIT_CONFIG_KEY_n`/
/// `GIT_CONFIG_VALUE_n` numbered pairs, so they need no enumeration.
///
/// Non-redirection variables such as `GIT_AUTHOR_*`/`GIT_COMMITTER_*`/
/// `GIT_SSH*` are kept: operations on the user's real worktree should follow
/// the behavior of the user's own git; code_checkpoints shadow repositories
/// need stronger isolation (identity and global config pinned too) and should
/// use [`strip_all_git_env`] instead.
pub(crate) fn strip_git_override_env(command: &mut Command) {
    for key in GIT_OVERRIDE_KEYS {
        command.env_remove(key);
    }
}

/// Shadow-repository hardened variant of [`strip_git_override_env`]: also
/// removes identity/date variables. The shadow repository's commit identity is
/// provided explicitly via `-c` by the caller; `GIT_AUTHOR_*` exported by the
/// host shell must not leak into snapshot commits.
pub(crate) fn strip_all_git_env(command: &mut Command) {
    for key in GIT_OVERRIDE_KEYS.iter().copied().chain(GIT_IDENTITY_KEYS) {
        command.env_remove(key);
    }
}

/// Once `GIT_CONFIG_COUNT` is removed, the `GIT_CONFIG_KEY_n`/
/// `GIT_CONFIG_VALUE_n` numbered pairs become ineffective, so the numbered
/// keys need no enumeration.
const GIT_OVERRIDE_KEYS: [&str; 20] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_GRAFT_FILE",
    "GIT_SHALLOW_FILE",
    "GIT_REPLACE_REF_BASE",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_CONFIG",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    // GIT_CONFIG_KEY_n / GIT_CONFIG_VALUE_n pairs are gated by
    // GIT_CONFIG_COUNT: removing COUNT disables the whole group. Key/value 0
    // is still removed to guard against extreme host injections that bypass
    // COUNT.
    "GIT_CONFIG_KEY_0",
    "GIT_CONFIG_VALUE_0",
];

const GIT_IDENTITY_KEYS: [&str; 6] = [
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
];

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
    // 与 `.output()` 的语义保持一致（stdin 显式接 null）：本模块的调用方全是
    // git/探测/转换类非交互命令，而 app 是无窗口 GUI 进程，继承来的 stdin 是
    // 坏句柄，CLI 读它会死等（同 `run_with_timeout` 注释记录过的安装器卡死）。
    command.stdin(Stdio::null());
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
                // A failed tree-kill request (Windows taskkill can overrun
                // its own budget) must reach the caller's error message:
                // the termination note would otherwise claim a tree
                // termination that did not happen.
                let kill_note = if kill_tree_on_timeout {
                    kill_process_tree(child.id())
                        .err()
                        .map(|error| format!("kill tree request failed: {error}"))
                } else {
                    None
                };
                let _ = child.kill();
                let reap_note = match reap_killed_child(&mut child, REAP_GRACE) {
                    Reap::Reaped => None,
                    Reap::Abandoned => Some(String::from(
                        "termination requested but the child has not exited; it may linger",
                    )),
                    Reap::Failed(error) => Some(format!("reaping the child failed: {error}")),
                };
                // Never block the timeout path by joining pipe readers: a
                // privileged descendant may be outside the caller's signal
                // permission even after its wrapper is gone, and a grandchild
                // that survived the kill (plain variant) can hold the pipes
                // open — either way, joining would block this call forever
                // past the very deadline the caller asked for. Output is
                // lost on timeout — acceptable, the caller only gets Err.
                drop(stdout_reader);
                drop(stderr_reader);
                let termination_note = if kill_tree_on_timeout {
                    "subprocess tree termination requested"
                } else {
                    "subprocess termination requested"
                };
                let reap_suffix = match (kill_note, reap_note) {
                    (Some(a), Some(b)) => format!("; {a}; {b}"),
                    (Some(a), None) => format!("; {a}"),
                    (None, Some(b)) => format!("; {b}"),
                    (None, None) => String::new(),
                };
                return Err(format!(
                    "{program} timed out after {}s: {termination_note}{reap_suffix}",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let kill_note = if kill_tree_on_timeout {
                    kill_process_tree(child.id())
                        .err()
                        .map(|kill_error| format!("kill tree request failed: {kill_error}"))
                } else {
                    None
                };
                let _ = child.kill();
                // Reap promptly but never block on it, and never join the
                // readers: the child may refuse to die, and a surviving
                // descendant can hold the pipes open past the caller's
                // deadline. Output is dropped on this path; kill/reap
                // failures are folded into the reported error.
                let reap_note = match reap_killed_child(&mut child, REAP_GRACE) {
                    Reap::Reaped => None,
                    Reap::Abandoned => Some(String::from(
                        "termination requested but the child has not exited",
                    )),
                    Reap::Failed(reap_error) => {
                        Some(format!("reaping the child failed: {reap_error}"))
                    }
                };
                drop(stdout_reader);
                drop(stderr_reader);
                let reap_suffix = match (kill_note, reap_note) {
                    (Some(a), Some(b)) => format!("; {a}; {b}"),
                    (Some(a), None) => format!("; {a}"),
                    (None, Some(b)) => format!("; {b}"),
                    (None, None) => String::new(),
                };
                return Err(format!("{program} wait error: {error}{reap_suffix}"));
            }
        }
    };

    Ok(Output {
        status,
        stdout: stdout_reader.join().unwrap_or_default(),
        stderr: stderr_reader.join().unwrap_or_default(),
    })
}

/// How long to keep reaping a child after a termination request. A
/// successful `kill()` does not force a prompt exit: a process stuck in
/// uninterruptible kernel sleep (Unix D state) never observes SIGKILL, so
/// a blocking `wait()` could hang the caller past its own timeout budget.
/// Bounded reaping trades that unbounded hang for a bounded, reported
/// leak.
pub(crate) const REAP_GRACE: Duration = Duration::from_secs(2);

/// Outcome of [`reap_killed_child`].
pub(crate) enum Reap {
    /// Child exited and was reaped.
    Reaped,
    /// Grace elapsed without an observed exit; the child may linger.
    Abandoned,
    /// Waiting for the child errored; the reap is not established.
    Failed(std::io::Error),
}

/// Reap a killed child, bounded by `grace`. Waits via `wait_timeout`
/// instead of a blocking `wait()` so the caller keeps its timeout
/// guarantee even when the child cannot exit promptly, and returns the
/// outcome instead of swallowing wait errors. The child does not have to
/// have accepted the kill for this to stay bounded: at most `grace` is
/// spent even on a still-live child. Note that `wait_timeout` silently
/// takes and drops a piped stdin, closing it; callers that feed the
/// child a pipe must not rely on the handle surviving the reap.
pub(crate) fn reap_killed_child(child: &mut Child, grace: Duration) -> Reap {
    match child.wait_timeout(grace) {
        Ok(Some(_)) => Reap::Reaped,
        Ok(None) => Reap::Abandoned,
        Err(error) => Reap::Failed(error),
    }
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
        // taskkill 自身也可能卡死（WMI/RPC 停摆）：它无界，本模块所有"有界"
        // 等待的超时路径都会汇入这里，等于把预算重新变成无界。给它 2s 预算
        // （与底座 hooks 执行器的同类兜底一致），超时杀掉 taskkill 自身并
        // 上报——目标树可能只被部分终止，但残余孙进程不再能把调用方拖过
        // 截止时间（超时路径从不 join 管道读端）。
        const WINDOWS_TASKKILL_TIMEOUT: Duration = Duration::from_secs(2);
        let mut taskkill = external_command(Path::new("taskkill"))
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .spawn()?;
        if taskkill
            .wait_timeout(WINDOWS_TASKKILL_TIMEOUT)
            .is_ok_and(|finished| finished.is_none())
        {
            let _ = taskkill.kill();
            let _ = taskkill.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("taskkill for pid {pid} exceeded the kill budget"),
            ));
        }
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
pub(crate) fn hide_tokio_console(_command: &mut tokio::process::Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// reap_killed_child collects an exited child promptly instead of
    /// waiting out the grace deadline. Soft-skips when no `sleep` binary
    /// is on PATH (bare Windows hosts; windows-latest CI bash steps have
    /// Git Bash's `sleep.exe` on PATH, so the test runs for real there).
    #[test]
    fn reap_killed_child_reaps_exited_child() {
        if Command::new("sleep").arg("0").status().is_err() {
            eprintln!("skipping: no `sleep` binary on this platform");
            return;
        }
        let mut child = Command::new("sleep")
            .arg("0")
            .spawn()
            .expect("spawn sleep 0");
        let started = Instant::now();
        assert!(
            matches!(reap_killed_child(&mut child, REAP_GRACE), Reap::Reaped),
            "an exited child must be reaped successfully"
        );
        assert!(
            started.elapsed() < REAP_GRACE,
            "reaping an exited child must not wait out the grace deadline"
        );
    }

    /// The Abandoned branch is induced deterministically by a zero grace
    /// budget on a live child: no SIGKILL-surviving process is needed. Only
    /// the Failed branch is not inducible in-process (std caches the exit
    /// status, so a collected child can never report a wait error).
    #[test]
    fn reap_killed_child_abandons_a_live_child_on_zero_grace() {
        if Command::new("sleep").arg("0").status().is_err() {
            eprintln!("skipping: no `sleep` binary on this platform");
            return;
        }
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep 30");
        assert!(
            matches!(
                reap_killed_child(&mut child, Duration::ZERO),
                Reap::Abandoned
            ),
            "a live child with a zero grace budget must be abandoned"
        );
        // Clean up so the test leaves no zombie or stray sleeper behind.
        let _ = child.kill();
        assert!(
            matches!(reap_killed_child(&mut child, REAP_GRACE), Reap::Reaped),
            "cleanup kill must reap the child"
        );
    }

    #[test]
    fn git_override_keys_cover_redirection_and_config_injection_without_identity() {
        // Redirection and config-injection keys must be on the strip list.
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_COMMON_DIR",
            "GIT_GRAFT_FILE",
            "GIT_SHALLOW_FILE",
            "GIT_REPLACE_REF_BASE",
            "GIT_NAMESPACE",
            "GIT_CEILING_DIRECTORIES",
            "GIT_DISCOVERY_ACROSS_FILESYSTEM",
            "GIT_CONFIG",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_NOSYSTEM",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_PARAMETERS",
        ] {
            assert!(
                GIT_OVERRIDE_KEYS.contains(&key),
                "override keys must cover {key}"
            );
        }
        // Identity, transport, and terminal-behavior variables are not on the
        // subset list: operations on the user's real worktree follow the
        // behavior of the user's own git (the shadow-repository hardened list
        // covers identity separately).
        for key in [
            "GIT_AUTHOR_NAME",
            "GIT_AUTHOR_EMAIL",
            "GIT_COMMITTER_DATE",
            "GIT_SSH_COMMAND",
            "GIT_ASKPASS",
            "GIT_TERMINAL_PROMPT",
            "GIT_EDITOR",
            "GIT_PAGER",
            "GIT_TRACE",
            "HOME",
            "GITHUB_TOKEN",
        ] {
            assert!(
                !GIT_OVERRIDE_KEYS.contains(&key),
                "override keys must not contain {key}"
            );
        }
        // Shadow-repository hardened list = subset list + identity keys, and
        // the identity keys must actually take effect in the hardened list.
        let strengthened: std::collections::BTreeSet<&str> = GIT_OVERRIDE_KEYS
            .iter()
            .copied()
            .chain(GIT_IDENTITY_KEYS)
            .collect();
        for key in GIT_IDENTITY_KEYS {
            assert!(strengthened.contains(key), "full strip must cover {key}");
        }
        assert!(!GIT_OVERRIDE_KEYS.contains(&"GIT_AUTHOR_NAME"));
    }

    /// The strip helpers must translate the key lists into explicit
    /// `env_remove` entries on the Command (observable via `get_envs`); the
    /// code_checkpoints test only spot-checks representatives, so the full
    /// list coverage lives here, next to the lists themselves.
    #[test]
    fn strip_git_env_helpers_remove_every_listed_key() {
        let removed_entries = |command: &std::process::Command| -> Vec<std::ffi::OsString> {
            command
                .get_envs()
                .filter_map(|(name, value)| value.is_none().then(|| name.to_os_string()))
                .collect()
        };

        let mut hardened = std::process::Command::new("git");
        strip_all_git_env(&mut hardened);
        let removed = removed_entries(&hardened);
        for key in GIT_OVERRIDE_KEYS.iter().copied().chain(GIT_IDENTITY_KEYS) {
            assert!(
                removed.iter().any(|entry| entry == key),
                "strip_all_git_env must env_remove {key}"
            );
        }

        let mut soft = std::process::Command::new("git");
        strip_git_override_env(&mut soft);
        let removed = removed_entries(&soft);
        for key in GIT_OVERRIDE_KEYS {
            assert!(
                removed.iter().any(|entry| entry == key),
                "strip_git_override_env must env_remove {key}"
            );
        }
        for key in GIT_IDENTITY_KEYS {
            assert!(
                !soft
                    .get_envs()
                    .any(|(name, _)| name == std::ffi::OsStr::new(key)),
                "strip_git_override_env must not touch identity key {key}"
            );
        }
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

    /// The plain variant (no tree kill) leaves the `sleep` grandchild alive
    /// and holding the inherited pipes: joining the readers would block this
    /// call forever past its own deadline. The timeout error must come back
    /// promptly with the plain (non-tree) termination note.
    #[cfg(unix)]
    #[test]
    fn plain_timeout_does_not_wait_for_a_descendant_holding_the_pipes() {
        let work = std::env::temp_dir().join(format!(
            "pinvou3-plain-timeout-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&work).expect("create test work dir");
        let sleep_pid_file = work.join("sleep-pid");
        let mut command = Command::new("sh");
        command.args([
            "-c",
            &format!("sleep 30 & echo $! > {}; wait", sleep_pid_file.display()),
        ]);
        let started = Instant::now();

        let error = output_with_timeout(command, Duration::from_millis(100)).unwrap_err();

        assert!(error.contains("timed out after"));
        assert!(error.contains("subprocess termination requested"));
        assert!(!error.contains("tree termination"));
        assert!(started.elapsed() < Duration::from_secs(5));

        // The plain variant deliberately leaves the grandchild alive; reap
        // the stray sleeper instead of letting it occupy the CI runner for
        // the rest of its 30s (same hygiene as the tree-variant test).
        if let Ok(text) = std::fs::read_to_string(&sleep_pid_file) {
            if let Ok(pid) = text.trim().parse::<i32>() {
                // SAFETY: libc::kill is a direct kill(2) wrapper; no memory
                // is touched.
                let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
        let _ = std::fs::remove_dir_all(&work);
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
        // Read before cleanup: removing the work dir deletes the marker with
        // it, which would make the assertion below vacuously pass.
        let marker_content = std::fs::read_to_string(&marker).ok();

        assert!(
            kill_result.is_ok(),
            "group kill must succeed for a live group: {kill_result:?}"
        );
        assert!(
            both_dead,
            "group leader {sh_pid} and descendant {sleep_pid} must both die"
        );
        assert!(
            marker_content.is_none(),
            "an external kill was spawned (argv log: {marker_content:?}); \
             group kills must use kill(2) directly"
        );

        let _ = std::fs::remove_dir_all(&work);
    }
}
