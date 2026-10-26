use std::ffi::OsStr;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

/// Hard ceiling for captured stdout/stderr per stream. A child (or a
/// descendant) writing past it gets its whole process tree terminated, so a
/// runaway writer can neither exhaust memory nor outlive the caller.
const MAX_CAPTURE_BYTES: u64 = 4 * 1024 * 1024;
/// Appended by [`read_capture`] when a stream was clipped to
/// [`MAX_CAPTURE_BYTES`], so a truncated capture is never mistaken for the
/// child's complete output.
const CAPTURE_TRUNCATED_MARKER: &[u8] = b"\n[pinvou: subprocess output truncated]\n";

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
///
/// `pub` so the same-repo `pinvou-cli` workspace lanes can import it through
/// the `features::codex_acp` facade re-export instead of mirroring the list
/// (a CLI-side mirror had already drifted by three keys); the GUI's own
/// callers keep using it through the crate-private strip_git_override_env.
pub const GIT_OVERRIDE_KEYS: [&str; 25] = [
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
    // GIT_TEMPLATE_DIR would seed a fresh `git init` of the shadow repo with
    // hook-carrying templates (pre-auto-gc runs under `gc --auto`).
    "GIT_TEMPLATE_DIR",
    // The pathspec-magic trio, stripped together for one reason: the secret
    // purge's pathspecs are `:(icase)*.pem`-shaped, so anything that changes
    // how default magic is interpreted can silently turn the purge into a
    // no-op. GIT_LITERAL_PATHSPECS=1 disables magic outright (the `:(icase)`
    // prefix is then matched as literal text); GIT_NOGLOB_PATHSPECS=1 makes
    // the default no-glob, so `*.pem` matches only a file literally named
    // `*.pem`. GIT_ICASE_PATHSPECS only broadens matching — the safe
    // direction — but goes with its siblings so the purge's matching rules
    // come from the command line rather than from whatever the host exported.
    "GIT_LITERAL_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
    // GIT_EXTERNAL_DIFF replaces the diff driver with an arbitrary command.
    // `--no-ext-diff` closes it at the one diff call site, but the isolation
    // primitive must not depend on every future call site remembering a flag.
    "GIT_EXTERNAL_DIFF",
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

/// Published through the `features::codex_acp` facade for the same-repo
/// `pinvou-cli`'s commit lane: the CLI must strip the exact identity list the
/// GUI's commit lane strips, or an ambient `GIT_AUTHOR_*` could outrank the
/// `-c user.name=` the CLI passes (same drift-prevention as
/// `GIT_OVERRIDE_KEYS` above); behavior is unchanged.
pub const GIT_IDENTITY_KEYS: [&str; 6] = [
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
/// Shared with the pinvou-cli kill tree (`support.rs` Windows arm) through
/// the targeted `platform::external_command` re-export — the `process`
/// module itself stays crate-private. The point of sharing is *resolution
/// parity*, not hardening: `external_application_path` only normalizes
/// `\\?\`/UNC spellings and returns a bare name like `taskkill` verbatim,
/// so a bare-name caller resolves exactly as it does for the GUI's own
/// `kill_pid_tree` (app dir → CWD → System32 → PATH). A caller that needs a
/// planted-`taskkill.exe`-in-CWD defense must resolve
/// `%SystemRoot%\System32\taskkill.exe` itself — this helper deliberately
/// does not invent a stricter PATH policy the GUI does not have.
///
/// The Windows `.cmd`/`.bat` branch keeps the manual `cmd /D /S /C` wrap
/// (unlike the CLI's own `build_command`, which delegates to std's hardened
/// batch quoting): these shims NEED the cmd interpretation to run at all
/// — an npm `.cmd` spawned directly is treated as a non-executable image —
/// and every current caller passes a fixed, first-party-resolved path with
/// no attacker-shaped argument material, so the BatBadBut argument-quoting
/// class the CLI pin guards against has no vector here. If a caller ever
/// forwards external arguments through this helper, route that caller
/// through `build_command`'s std delegation instead of extending the wrap.
pub fn external_command(executable: &Path) -> Command {
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
///
/// stdout/stderr are captured into temporary files instead of pipes: a
/// completed child's output is returned in full even when a daemonized
/// descendant inherited the capture handles, and the wall-clock timeout
/// stays effective regardless of descendants (issue #536). Capture is
/// bounded: if either stream exceeds [`MAX_CAPTURE_BYTES`] while this call
/// is running, the whole process tree is terminated regardless of the
/// kill-tree flag, and the error carries the truncated output. The bound is
/// an in-call guarantee: a descendant that escapes the tree kill (e.g. it
/// called setsid) can keep writing to the unlinked capture file after this
/// call returns — the space is reclaimed when it exits, and the planned
/// Windows Job-Object follow-up closes that residual gap.
pub(crate) fn output_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> Result<Output, String> {
    std_process_group_leader(&mut command);
    output_with_timeout_inner(command, timeout, false)
}

/// Capture a subprocess with a wall-clock timeout and terminate its process tree
/// on timeout. Use this for helpers that can launch privileged descendants: killing
/// only the wrapper can otherwise leave the real operation running with inherited
/// stdout/stderr pipes after the caller has reported a timeout.
///
/// The capture bound documented on [`output_with_timeout`] applies here too.
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
    // Regular files cannot fill up and block the child the way a pipe can.
    // They also let a completed parent's output be read back without
    // waiting for a descendant that inherited the same handles to exit, so
    // the wall-clock timeout stays effective (issue #536).
    let mut stdout_file = tempfile::tempfile()
        .map_err(|error| format!("create {program} stdout capture failed: {error}"))?;
    let mut stderr_file = tempfile::tempfile()
        .map_err(|error| format!("create {program} stderr capture failed: {error}"))?;
    let stdout_target = stdout_file
        .try_clone()
        .map_err(|error| format!("clone {program} stdout capture failed: {error}"))?;
    let stderr_target = stderr_file
        .try_clone()
        .map_err(|error| format!("clone {program} stderr capture failed: {error}"))?;
    // 与 `.output()` 的语义保持一致（stdin 显式接 null）：本模块的调用方全是
    // git/探测/转换类非交互命令，而 app 是无窗口 GUI 进程，继承来的 stdin 是
    // 坏句柄，CLI 读它会死等（同 `run_with_timeout` 注释记录过的安装器卡死）。
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_target))
        .stderr(Stdio::from(stderr_target));
    let mut child = command
        .spawn()
        .map_err(|error| format!("spawn {program} failed: {error}"))?;

    let started = Instant::now();
    let status = loop {
        // Check the capture bound before the exit poll so a runaway writer
        // is terminated mid-flight: with [`MAX_CAPTURE_BYTES`] crossed, the
        // whole tree is killed regardless of the kill-tree flag — an
        // overflowing writer never gets to outlive the caller.
        if capture_limit_exceeded(&stdout_file) || capture_limit_exceeded(&stderr_file) {
            let kill_note = kill_note_suffix(kill_process_tree(child.id()));
            let _ = child.kill();
            let reap_note = reap_note_suffix(reap_killed_child(&mut child, REAP_GRACE));
            let stdout = read_capture(&mut stdout_file);
            let stderr = read_capture(&mut stderr_file);
            let detail = subprocess_output_detail(&stdout, &stderr);
            return Err(format!(
                "{program} output exceeded {MAX_CAPTURE_BYTES} bytes; subprocess tree termination requested: {detail}{reap_note}{kill_note}"
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() <= timeout => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let kill_note = if kill_tree_on_timeout {
                    kill_note_suffix(kill_process_tree(child.id()))
                } else {
                    String::new()
                };
                let _ = child.kill();
                let reap_note = reap_note_suffix(reap_killed_child(&mut child, REAP_GRACE));
                let stdout = read_capture(&mut stdout_file);
                let stderr = read_capture(&mut stderr_file);
                let detail = subprocess_output_detail(&stdout, &stderr);
                return Err(format!(
                    "{program} timed out after {}s: {detail}{reap_note}{kill_note}",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let kill_note = if kill_tree_on_timeout {
                    kill_note_suffix(kill_process_tree(child.id()))
                } else {
                    String::new()
                };
                let _ = child.kill();
                // The wait error itself is the caller's primary diagnostic,
                // so the captured output stays out of this message; kill and
                // reap failures are still folded in. Bounded reaping keeps
                // this path within its time budget too: capture lives in
                // temp files, so neither a child that refuses to die nor a
                // surviving descendant can extend this call (issue #536).
                let reap_note = reap_note_suffix(reap_killed_child(&mut child, REAP_GRACE));
                return Err(format!(
                    "{program} wait error: {error}{reap_note}{kill_note}"
                ));
            }
        }
    };

    // A fast writer can cross the limit and exit between the last size check
    // and `try_wait()`. Re-check after completion so overflow is never
    // reported as a silently truncated success.
    if capture_limit_exceeded(&stdout_file) || capture_limit_exceeded(&stderr_file) {
        let kill_note = kill_note_suffix(kill_process_tree(child.id()));
        let stdout = read_capture(&mut stdout_file);
        let stderr = read_capture(&mut stderr_file);
        let detail = subprocess_output_detail(&stdout, &stderr);
        return Err(format!(
            "{program} output exceeded {MAX_CAPTURE_BYTES} bytes; subprocess tree termination requested: {detail}{kill_note}"
        ));
    }

    Ok(Output {
        status,
        stdout: read_capture(&mut stdout_file),
        stderr: read_capture(&mut stderr_file),
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

/// Renders a reap outcome as an error-message suffix; an empty string when
/// the child was reaped cleanly.
fn reap_note_suffix(reap: Reap) -> String {
    match reap {
        Reap::Reaped => String::new(),
        Reap::Abandoned => {
            String::from("; termination requested but the child has not exited; it may linger")
        }
        Reap::Failed(error) => format!("; reaping the child failed: {error}"),
    }
}

/// Renders a failed tree-kill request as an error-message suffix; an empty
/// string when the request succeeded. A failed request must reach the
/// caller's error message: the termination note would otherwise claim a
/// tree termination that did not happen (the Windows taskkill helper can
/// overrun its own budget and give up).
fn kill_note_suffix(result: std::io::Result<()>) -> String {
    result
        .err()
        .map(|error| format!("; kill tree request failed: {error}"))
        .unwrap_or_default()
}

/// True once the capture file has grown past [`MAX_CAPTURE_BYTES`]. The
/// child's own file offset is irrelevant: the file length is observed, so
/// the check works while the child is running and after it exited.
fn capture_limit_exceeded(file: &std::fs::File) -> bool {
    file.metadata()
        .is_ok_and(|metadata| metadata.len() > MAX_CAPTURE_BYTES)
}

/// Read a capture file back, clipped to [`MAX_CAPTURE_BYTES`]. The extra
/// probe byte distinguishes "exactly at the limit" from "clipped", and a
/// clipped read — or a capture that cannot be read back in full — gets
/// [`CAPTURE_TRUNCATED_MARKER`] appended so an incomplete capture is never
/// mistaken for the child's complete output.
fn read_capture(file: &mut std::fs::File) -> Vec<u8> {
    let mut buffer = Vec::new();
    // A capture that cannot be read back in full must never pass for the
    // child's complete output: a failed seek or read gets the same
    // truncation marker as a clipped read.
    let mut read_failed = file.seek(SeekFrom::Start(0)).is_err();
    if !read_failed {
        read_failed = file
            .take(MAX_CAPTURE_BYTES + 1)
            .read_to_end(&mut buffer)
            .is_err();
    }
    let clipped = buffer.len() as u64 > MAX_CAPTURE_BYTES;
    if clipped {
        buffer.truncate(MAX_CAPTURE_BYTES as usize);
    }
    if read_failed || clipped {
        buffer.extend_from_slice(CAPTURE_TRUNCATED_MARKER);
    }
    buffer
}

/// Condenses a captured stdout/stderr pair into an error-message excerpt:
/// whichever stream has content, both when both do, capped at 4000 chars so
/// a flooding child cannot turn the error itself into a memory hog.
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
        // taskkill 自身也可能卡死（WMI/RPC 停摆）：它无界，本模块所有"有界"
        // 等待的超时路径都会汇入这里，等于把预算重新变成无界。给它 2s 预算
        // （与底座 hooks 执行器的同类兜底一致），超时杀掉 taskkill 自身并
        // 上报——目标树可能只被部分终止，但捕获输出已写入临时文件，残余
        // 孙进程无法再把调用方拖过截止时间。
        const WINDOWS_TASKKILL_TIMEOUT: Duration = Duration::from_secs(2);
        let mut taskkill = external_command(Path::new("taskkill"))
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            // The helper reads nothing and no one consumes its output:
            // wire all three standard streams to null so its output cannot
            // leak into the app's own stdio (a plain spawn would inherit
            // them), matching this module's non-interactive stdin-null
            // convention.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let finished = match taskkill.wait_timeout(WINDOWS_TASKKILL_TIMEOUT) {
            Ok(finished) => finished,
            Err(error) => {
                return Err(std::io::Error::other(format!(
                    "taskkill for pid {pid} wait failed: {error}"
                )));
            }
        };
        let Some(status) = finished else {
            let _ = taskkill.kill();
            let _ = taskkill.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("taskkill for pid {pid} exceeded the kill budget"),
            ));
        };
        // A failing taskkill must not be folded into a successful
        // termination claim: `taskkill /T` walks live parent→child links, so
        // once the root process is gone it cannot reach orphaned
        // descendants — a nonzero exit (typically "process not found") means
        // the tree kill did NOT happen, not that there was nothing to kill.
        let code = status.code().unwrap_or(-1);
        if code != 0 {
            return Err(std::io::Error::other(format!(
                "taskkill for pid {pid} exited with status {code}; descendants of the exited process were not terminated"
            )));
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
    use std::io::Write as _;

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

    const OUTPUT_FIXTURE_ENV: &str = "PINVOU_PROCESS_OUTPUT_FIXTURE";
    const OUTPUT_FIXTURE_MARKER_ENV: &str = "PINVOU_PROCESS_OUTPUT_FIXTURE_MARKER";
    const OUTPUT_FIXTURE_TEST: &str = "platform::process::tests::command_output_fixture";
    const DESCENDANT_STARTUP_HANDSHAKE: Duration = Duration::from_secs(20);
    static PROCESS_FIXTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Serializes the tests that re-exec this test binary as a fixture so
    /// their marker files and CPU-heavy copies never interleave. This lock
    /// deliberately does NOT compose with the kill-tree test's `PATH_LOCK`:
    /// the fixture copies resolve only `current_exe()` (an absolute path)
    /// and touch only marker files, so a concurrently mutated `PATH` cannot
    /// affect them and no shared lock is needed.
    fn lock_process_fixture() -> std::sync::MutexGuard<'static, ()> {
        PROCESS_FIXTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn output_fixture_command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", OUTPUT_FIXTURE_TEST, "--nocapture"])
            .env(OUTPUT_FIXTURE_ENV, mode);
        command
    }

    fn fixture_marker(extension: &str) -> Option<std::path::PathBuf> {
        std::env::var_os(OUTPUT_FIXTURE_MARKER_ENV)
            .map(std::path::PathBuf::from)
            .map(|path| path.with_extension(extension))
    }

    fn wait_for_fixture_marker(extension: &str, timeout: Duration) -> bool {
        let Some(marker) = fixture_marker(extension) else {
            return false;
        };
        let started = Instant::now();
        while !marker.exists() && started.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(25));
        }
        marker.exists()
    }

    /// Spawns the long-lived `descendant_success` copy with inherited
    /// capture handles and waits for its running marker. Shared by the
    /// retention ("descendant") and tree-kill ("overflow_tree") modes.
    fn spawn_descendant_and_wait_for_running_marker() {
        let mut descendant = output_fixture_command("descendant_success");
        descendant.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;

            // Give the successful descendant an independent console
            // lifecycle so this fixture models a real helper that outlives
            // its completed launcher while retaining the launcher's capture
            // handles.
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            descendant.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }
        let _child = descendant.spawn().unwrap();
        assert!(wait_for_fixture_marker(
            "running",
            DESCENDANT_STARTUP_HANDSHAKE
        ));
    }

    /// Re-exec fixture for the capture tests: with OUTPUT_FIXTURE_ENV set,
    /// this same test binary runs a scripted mode instead of the normal
    /// test pass — writing real output volumes to whatever its stdout/stderr
    /// are wired to, spawning descendants that outlive their launcher, and
    /// exiting on cue. A no-op when the variable is unset (the normal
    /// `cargo test` invocation, which is why this test is harmless to run).
    #[test]
    fn command_output_fixture() {
        match std::env::var(OUTPUT_FIXTURE_ENV).as_deref() {
            Ok("flood") => {
                let payload = vec![b'x'; 256 * 1024];
                std::io::stdout().write_all(&payload).unwrap();
                std::io::stderr().write_all(&payload).unwrap();
            }
            Ok("overflow") => {
                let payload = vec![b'x'; 64 * 1024];
                loop {
                    std::io::stdout().write_all(&payload).unwrap();
                    std::io::stderr().write_all(&payload).unwrap();
                }
            }
            Ok("overflow_exit") => {
                // Writes past the capture limit in one burst and exits
                // cleanly. Whether the in-flight check or the post-exit
                // re-check fires depends on scheduling; the pinned contract
                // is the caller-visible one: an overflow is an error, never
                // a silently truncated success.
                let payload = vec![b'x'; MAX_CAPTURE_BYTES as usize + 64 * 1024];
                std::io::stdout().write_all(&payload).unwrap();
                std::io::stderr().write_all(&payload).unwrap();
            }
            Ok("descendant") => {
                spawn_descendant_and_wait_for_running_marker();
                println!("descendant-started");
            }
            Ok("overflow_tree") => {
                // Overflow while a live descendant holds the inherited
                // capture handles: the whole group must die with the
                // launcher, so the tree-kill tests can pin actual
                // descendant death instead of just the error text. The
                // descendant records its pid in the pid marker before
                // raising the running marker.
                spawn_descendant_and_wait_for_running_marker();
                let payload = vec![b'x'; 64 * 1024];
                loop {
                    std::io::stdout().write_all(&payload).unwrap();
                }
            }
            Ok("descendant_success") => {
                if let Some(pid) = fixture_marker("pid") {
                    std::fs::write(pid, std::process::id().to_string()).unwrap();
                }
                if let Some(running) = fixture_marker("running") {
                    std::fs::write(running, b"running").unwrap();
                }
                // Outlive the launcher so its capture handles are held by a
                // live process when the launcher's output is read back.
                std::thread::sleep(Duration::from_secs(5));
                if let Some(finished) = fixture_marker("finished") {
                    std::fs::write(finished, b"finished").unwrap();
                }
            }
            _ => {}
        }
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
            // Named explicitly so removing one goes red here. The sibling
            // test iterates GIT_OVERRIDE_KEYS itself and is therefore
            // tautological; only the array length would otherwise object, and
            // it objects to every edit equally.
            "GIT_TEMPLATE_DIR",
            "GIT_LITERAL_PATHSPECS",
            "GIT_NOGLOB_PATHSPECS",
            "GIT_ICASE_PATHSPECS",
            "GIT_EXTERNAL_DIFF",
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
    /// and holding the inherited capture handles. The timeout error must
    /// still come back promptly, without claiming a process-tree
    /// termination that was never requested.
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
        // Pins the real detail text end-to-end: the two connector/pkexec
        // fixture files hard-code this exact wording, so a wording change
        // in `subprocess_output_detail` must fail here first.
        assert!(error.contains("no subprocess output"), "{error}");
        // The plain variant never requests a process-tree termination on
        // timeout: no tree-termination claim may appear (only the
        // capture-limit paths request one unconditionally), and a *failed*
        // request would surface as an explicit kill-tree failure clause.
        // The retired "subprocess termination requested" note was replaced
        // by the captured-output detail in the ported tempfile design.
        assert!(!error.contains("tree termination"));
        assert!(!error.contains("kill tree request failed"));
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

    /// Regression for issue #536: a daemonized descendant that inherited
    /// the capture handles must not extend the call past the child's own
    /// exit. Here `sh` exits immediately while its backgrounded `sleep 30`
    /// keeps holding the inherited stdout/stderr handles: the call must
    /// return promptly with a success status, not wait out the sleeper.
    /// (Under the tempfile capture this holds by construction — the child's
    /// output is read back from regular files — but the deadline must keep
    /// holding too.)
    #[cfg(unix)]
    #[test]
    fn ok_path_does_not_wait_for_a_descendant_holding_the_pipes() {
        if Command::new("sleep").arg("0").status().is_err() {
            eprintln!("skipping: no `sleep` binary on this platform");
            return;
        }
        let work = std::env::temp_dir().join(format!(
            "pinvou3-ok-path-pipes-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&work).expect("create test work dir");
        let sleep_pid_file = work.join("sleep-pid");
        let mut command = Command::new("sh");
        command.args([
            "-c",
            // Background first so `$!` is the sleeper; the echo targets a
            // file so the captured stdout stays clean.
            &format!("sleep 30 & echo $! > {}; exit 0", sleep_pid_file.display()),
        ]);
        let started = Instant::now();

        let output = output_with_timeout(command, Duration::from_secs(15));

        let elapsed = started.elapsed();
        // The grandchild keeps the capture handles open for 30s; before
        // the pipe-era fix the success path blocked for that whole time.
        assert!(
            elapsed < Duration::from_secs(10),
            "the success path must not block on descendants holding the capture handles (took {elapsed:?})"
        );
        let output = output.expect("the child exited within the deadline");
        assert!(output.status.success(), "sh must exit cleanly");
        // The launcher wrote nothing to the captured streams itself (the
        // echo targets a file), so the captures are empty — but output it
        // HAD written would come back in full despite the live descendant;
        // completed_parent_does_not_wait_for_descendant_capture_handles
        // pins exactly that.
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());

        // Cleanup: kill the stray sleeper so it occupies nothing for the
        // rest of its 30s.
        if let Ok(text) = std::fs::read_to_string(&sleep_pid_file) {
            if let Ok(pid) = text.trim().parse::<u32>() {
                // SAFETY: libc::kill is a direct kill(2) wrapper; no memory is touched.
                let _ = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
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

    /// A large but bounded burst of output must be captured completely:
    /// regular-file capture cannot fill up and stall the writer the way a
    /// full pipe buffer would, so a chatty child neither deadlocks nor
    /// loses output. Guarded by the fixture lock because it re-execs the
    /// test binary.
    #[test]
    fn output_capture_does_not_deadlock_when_buffers_are_large() {
        let _fixture_lock = lock_process_fixture();
        // The budget must absorb a cold re-exec of the test binary
        // (multi-second on a loaded/AV-scanned Windows runner); on success
        // paths the child exits on its own, so a larger ceiling costs
        // nothing.
        let output = output_with_timeout_and_kill_tree(
            output_fixture_command("flood"),
            Duration::from_secs(30),
        )
        .unwrap();

        assert!(output.status.success());
        assert!(output.stdout.len() >= 256 * 1024);
        assert!(output.stderr.len() >= 256 * 1024);
    }

    /// A child that keeps writing past the capture limit must be stopped:
    /// the poll loop notices the crossed bound mid-flight and terminates
    /// the whole tree, reporting an overflow error instead of a truncated
    /// success. The fixture holds a live descendant while overflowing, so
    /// the descendant-death assertion below pins an actual tree kill, not
    /// just the error text.
    #[test]
    fn output_capture_limit_terminates_process_tree() {
        let _fixture_lock = lock_process_fixture();
        let temporary = tempfile::tempdir().unwrap();
        let marker = temporary.path().join("tree-overflow-marker");
        let mut command = output_fixture_command("overflow_tree");
        command.env(OUTPUT_FIXTURE_MARKER_ENV, &marker);
        let error =
            output_with_timeout_and_kill_tree(command, Duration::from_secs(30)).unwrap_err();

        assert!(error.contains("output exceeded"), "{error}");
        assert_descendant_fixture_died(&marker);
    }

    /// Pins the load-bearing wiring for "the tree is terminated on overflow
    /// regardless of the kill-tree flag": the plain variant must also make
    /// its child a process-group leader, or the unconditional group kill
    /// degenerates into a no-op (the leader's pid names no group) and the
    /// descendant survives. Removing the `std_process_group_leader` call in
    /// `output_with_timeout` fails the descendant-death assertion below.
    #[test]
    fn plain_output_limit_also_terminates_the_process_tree() {
        let _fixture_lock = lock_process_fixture();
        let temporary = tempfile::tempdir().unwrap();
        let marker = temporary.path().join("plain-overflow-marker");
        let mut command = output_fixture_command("overflow_tree");
        command.env(OUTPUT_FIXTURE_MARKER_ENV, &marker);
        let error = output_with_timeout(command, Duration::from_secs(30)).unwrap_err();

        assert!(error.contains("output exceeded"), "{error}");
        assert_descendant_fixture_died(&marker);
    }

    /// Asserts the overflow_tree fixture's descendant actually died. The
    /// descendant records its pid before raising the running marker, so the
    /// pid file is stable once the fixture started. The window must stay
    /// below the descendant's 5s natural lifetime, so a surviving
    /// descendant cannot pass the assertion by exiting on its own. Unix-only
    /// because std has no portable liveness probe; on Windows the in-flight
    /// tree kill rides on `taskkill /T` and its outcome is reported through
    /// the error message (real descendant-death pinning lands with the
    /// Job-Object follow-up).
    #[cfg(unix)]
    fn assert_descendant_fixture_died(marker: &std::path::Path) {
        let pid: u32 = std::fs::read_to_string(marker.with_extension("pid"))
            .expect("descendant pid marker")
            .trim()
            .parse()
            .expect("descendant pid");
        // SAFETY: libc::kill with sig=0 only probes liveness; no memory is touched.
        let alive = |pid: u32| unsafe { libc::kill(pid as i32, 0) == 0 };
        let started = Instant::now();
        while alive(pid) && started.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !alive(pid),
            "descendant {pid} survived the overflow tree kill"
        );
    }

    /// Windows counterpart: the overflow error above is the pin here; see
    /// the unix variant for why descendant death is not assertable.
    #[cfg(not(unix))]
    fn assert_descendant_fixture_died(_marker: &std::path::Path) {}

    /// A writer that crosses the limit and exits cleanly must still be
    /// errored, never given a silently truncated success. The in-flight
    /// check catches a crossing seen before the exit poll; the post-exit
    /// re-check covers a write landing inside the final iteration, after
    /// the size check but before `try_wait` reports the exit. This test
    /// pins the caller-visible contract, not which check fires.
    #[test]
    fn completed_process_cannot_bypass_output_capture_limit() {
        let _fixture_lock = lock_process_fixture();
        let error = output_with_timeout_and_kill_tree(
            output_fixture_command("overflow_exit"),
            Duration::from_secs(30),
        )
        .unwrap_err();

        assert!(error.contains("output exceeded"), "{error}");
    }

    /// The core output-retention regression: a launcher that exits on time
    /// while a live descendant still holds the inherited capture handles
    /// must have its own output returned IN FULL (the retired pipe handoff
    /// reported empty/partial here), and the call must not wait out the
    /// descendant. Timing is measured from the descendant's own running
    /// marker so test-binary startup time cannot masquerade as waiting for
    /// the capture handles to close.
    #[test]
    fn completed_parent_does_not_wait_for_descendant_capture_handles() {
        let _fixture_lock = lock_process_fixture();
        let temporary = tempfile::tempdir().unwrap();
        let marker = temporary.path().join("descendant-marker");
        let running = marker.with_extension("running");
        let finished = marker.with_extension("finished");
        let mut command = output_fixture_command("descendant");
        command.env(OUTPUT_FIXTURE_MARKER_ENV, &marker);
        // Starting another copy of the large test binary can take several
        // seconds on a cold Windows machine. Measure from the descendant's
        // own running marker so startup time cannot masquerade as waiting
        // for the inherited capture handles to close.
        let (marker_sender, marker_receiver) = std::sync::mpsc::sync_channel(1);
        let marker_waiter = std::thread::spawn(move || {
            // The launcher's own handshake starts after ITS startup, so this
            // waiter's deadline must be strictly larger or a slow start
            // lands in the gap: the launcher succeeds, but the channel
            // already holds a miss and the expect below fails spuriously.
            let deadline = DESCENDANT_STARTUP_HANDSHAKE + Duration::from_secs(15);
            let started = Instant::now();
            while !running.exists() && started.elapsed() < deadline {
                std::thread::sleep(Duration::from_millis(25));
            }
            let _ = marker_sender.send(running.exists().then(Instant::now));
        });
        // The outer launcher and the descendant are separate copies of the
        // large test binary. Give their explicit startup handshake enough
        // room, while the assertion below still measures the real contract
        // strictly from the descendant's running marker.
        let output = output_with_timeout_and_kill_tree(command, Duration::from_secs(30)).unwrap();
        // Upper-bounds the waiter thread's remaining wait (its deadline is
        // launcher handshake + 15s, and the call itself gives the launcher
        // 30s), so the recv cannot expire before the thread reports.
        let marker_seen_at = marker_receiver
            .recv_timeout(Duration::from_secs(20))
            .unwrap()
            .expect("descendant did not start before launcher exit");
        marker_waiter.join().unwrap();

        assert!(
            output.status.success(),
            "fixture failed with {:?}: stdout={} stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("descendant-started"));
        assert!(marker_seen_at.elapsed() < Duration::from_secs(3));
        let wait_started = Instant::now();
        while !finished.exists() && wait_started.elapsed() < Duration::from_secs(7) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            finished.exists(),
            "descendant fixture did not exit naturally"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    /// kill_note_suffix is the platform-independent half of the tree-kill
    /// error folding: empty on success, and a failure that names the request
    /// (the capture-limit paths otherwise claim a termination that did not
    /// happen). Pure function, so both branches run on every platform —
    /// unlike the runtime-Windows taskkill branch it annotates.
    #[test]
    fn kill_note_suffix_is_empty_on_success_and_names_the_failure() {
        assert_eq!(kill_note_suffix(Ok(())), "");
        let note = kill_note_suffix(Err(std::io::Error::other("taskkill budget overrun")));
        assert!(
            note.starts_with("; kill tree request failed: "),
            "the note must read as an appended failure clause, got: {note}"
        );
        assert!(
            note.contains("taskkill budget overrun"),
            "the note must carry the underlying error, got: {note}"
        );
    }

    /// reap_note_suffix is the counterpart of kill_note_suffix: empty when
    /// the child was reaped cleanly, an explicit lingering note on
    /// abandon, and a named failure clause otherwise. Pure function, so
    /// every branch runs on every platform.
    #[test]
    fn reap_note_suffix_covers_every_reap_outcome() {
        assert_eq!(reap_note_suffix(Reap::Reaped), "");
        let abandoned = reap_note_suffix(Reap::Abandoned);
        assert!(
            abandoned.starts_with("; termination requested"),
            "the note must read as an appended clause, got: {abandoned}"
        );
        assert!(
            abandoned.contains("may linger"),
            "the note must disclose the lingering child, got: {abandoned}"
        );
        let failed = reap_note_suffix(Reap::Failed(std::io::Error::other("wait interrupted")));
        assert!(
            failed.starts_with("; reaping the child failed: "),
            "the note must read as an appended failure clause, got: {failed}"
        );
        assert!(
            failed.contains("wait interrupted"),
            "the note must carry the underlying error, got: {failed}"
        );
    }

    /// read_capture must clip at the limit with the truncation marker, and
    /// treat exactly-at-limit as complete — the boundary the overflow
    /// checks and the "never mistaken for complete output" contract lean
    /// on.
    #[test]
    fn read_capture_clips_at_the_limit_and_marks_truncation() {
        let mut file = tempfile::tempfile().unwrap();
        let payload = vec![b'a'; (MAX_CAPTURE_BYTES as usize) + 128];
        file.write_all(&payload).unwrap();
        let clipped = read_capture(&mut file);
        assert_eq!(
            clipped.len(),
            MAX_CAPTURE_BYTES as usize + CAPTURE_TRUNCATED_MARKER.len()
        );
        assert!(
            clipped.ends_with(CAPTURE_TRUNCATED_MARKER),
            "a clipped capture must carry the truncation marker"
        );
        assert!(
            clipped.starts_with(&payload[..MAX_CAPTURE_BYTES as usize]),
            "the clipped capture must keep the first MAX_CAPTURE_BYTES bytes"
        );

        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![b'b'; MAX_CAPTURE_BYTES as usize])
            .unwrap();
        let exact = read_capture(&mut file);
        assert_eq!(exact.len(), MAX_CAPTURE_BYTES as usize);
        assert!(
            !exact.ends_with(CAPTURE_TRUNCATED_MARKER),
            "exactly-at-limit output is complete and must not be marked"
        );
    }
}
