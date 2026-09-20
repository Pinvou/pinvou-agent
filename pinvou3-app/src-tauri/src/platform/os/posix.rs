//! Unix 通用 helper —— linux 与 macos 共享的纯 POSIX 逻辑。
//!
//! Wave 3 起分批把核对过逐语义一致的实现收口到此，`linux/linux_path.rs`、
//! `macos/macos_path.rs` 与 `unsupported.rs`（macOS 经 glob
//! `pub use super::unsupported::*` 继承）只保留 re-export 与平台特有差异，
//! 调用面（各平台 mod.rs 的 `pub use`）不变。
//!
//! **保留在各自文件的 helper**（有真实平台差异，不强行合并）：
//! - `user_home_dir`：linux HOME 缺失时硬编码 `/tmp`（品悟临时产物目录），
//!   macOS/兜底平台用 `std::env::temp_dir()`。因此
//!   [`validate_upload_location_under_home`] 把 home 作为参数，由各适配器先
//!   解析自己的 `user_home_dir` 再传入，本文件不做平台探测。

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Spawn a short-lived fire-and-forget external process and reap it, avoiding Unix zombies.
///
/// Dropping the Child returned by std's `Command::spawn()` does **not**
/// reclaim the child (unlike tokio's kill_on_drop) and the parent never
/// auto-reaps; every open/xdg-open would leave a zombie until the parent
/// exits. This starts a detached reaper thread to `wait()`; open-file and
/// notification commands usually exit in milliseconds, ending the thread
/// immediately, so the steady-state cost is negligible.
/// The exception is agent-login browser launches
/// (`codex_acp::open_agent_login_url`): the first firefox/chrome instance
/// is itself a long-lived browser process, so the reaper thread parks in
/// `wait()` until the browser exits — one parked thread per long-lived
/// instance, still an acceptable cost; only the synchronous fallback on
/// thread-creation failure blocks for just as long (see below), which
/// remains preferable to zombie accumulation.
/// When reaper-thread creation fails (thread-count/memory-constrained edge
/// cases), `wait()` synchronously on the calling thread: the command has
/// already launched successfully, and a rare stall on a slow command beats
/// zombie accumulation plus a bogus "open failed" report.
pub fn spawn_detached_and_reap(command: &mut std::process::Command) -> std::io::Result<()> {
    use std::sync::{Arc, Mutex};
    // When `Builder::spawn` fails the closure is dropped (not returned),
    // so ownership of the Child cannot be taken back; route it through a
    // shared Option instead: the reaper thread and the failure fallback
    // race for it, and only one side can take it.
    let child = Arc::new(Mutex::new(Some(command.spawn()?)));
    let thread_child = Arc::clone(&child);
    match std::thread::Builder::new()
        .name("unix-child-reaper".to_string())
        .spawn(move || {
            if let Some(mut owned) = thread_child.lock().ok().and_then(|mut slot| slot.take()) {
                let _ = owned.wait();
            }
        }) {
        Ok(_) => Ok(()),
        Err(_) => {
            if let Some(mut owned) = child.lock().ok().and_then(|mut slot| slot.take()) {
                let _ = owned.wait();
            }
            Ok(())
        }
    }
}

/// 把外部传入的路径字符串原样转为 `PathBuf`。
/// linux 与 macOS 实现相同（皆 `PathBuf::from(value)`），收口于此。
pub fn platform_compat_path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

/// 比较路径组件是否等于预期字符串。
/// Unix 上大小写敏感，linux 与 macOS 实现相同。
pub fn path_component_eq(component: &OsStr, expected: &str) -> bool {
    component == OsStr::new(expected)
}

/// Unix 文件系统路径的稳定标识 key(大小写敏感,直接用原串)。
pub fn filesystem_path_identity_key(path: &str) -> String {
    path.to_string()
}

/// Probes process liveness with `kill(pid, 0)` without sending a signal.
/// Success and EPERM both mean the process exists; ESRCH means it exited.
/// Browser watch uses this through interface/system.rs before removing a stale port file.
pub fn process_alive(pid: u32) -> bool {
    // pid 0 would probe this process's own process group (always "alive");
    // callers own non-zero pids, so guard the sentinel instead of reporting it.
    if pid == 0 {
        return false;
    }
    // SAFETY: Signal 0 only queries process existence and is safe for any pid.
    if unsafe { libc::kill(pid as i32, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Restricts a sensitive directory to the current user on POSIX systems.
pub fn make_private_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    if let Err(error) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)) {
        eprintln!(
            "[platform] failed to restrict directory permissions for {}: {error}",
            path.display()
        );
    }
}

/// 连接器 CLI 命令构造（linux 与 macOS 同策略，收口于此；unsupported.rs 的
/// 兜底桩仅在非 unix 目标保留）。优先级：
/// 1. `npm` 优先走随包 Node + npm-cli.js（GUI 无系统 PATH 保证）；
/// 2. 命令即 CLI 本体时，版本化资产库 lock 表单点解析优先，旧布局其次；
/// 3. 再扫常见 npm 全局前缀（NPM_CONFIG_PREFIX / ~/.npm-global / ~/.local）；
/// 4. 最后交给 PATH。
pub fn connector_cli_command(cli_bin: &str, program: &str) -> Command {
    if program == "npm" {
        if let (Some(node), Some(npm_cli)) = (
            crate::platform::paths::bundled_connector_node(),
            crate::platform::paths::bundled_connector_npm_cli(),
        ) {
            let mut command = Command::new(node);
            command.arg(npm_cli);
            return command;
        }
    }
    let resolved = connector_cli_program(cli_bin, program);
    // npm 的 Unix bin 是带 `#!/usr/bin/env node` 的 JS shim；GUI 环境不保证系统
    // PATH 有 node，因此腾讯会议也显式交给随包 Node 执行。
    if program == cli_bin && cli_bin == "tmeet" {
        let script = PathBuf::from(&resolved);
        if script.is_file() {
            if let Some(node) = crate::platform::paths::bundled_connector_node() {
                let mut command = Command::new(node);
                command.arg(script);
                return command;
            }
        }
    }
    Command::new(resolved)
}

fn connector_cli_program(cli_bin: &str, program: &str) -> OsString {
    if program == cli_bin {
        // 版本化资产库（lock 表单点解析）优先；旧布局（未迁移存量）其次。
        if let Some(path) = crate::platform::connector_lock::locked_cli_path(cli_bin) {
            if path.is_file() {
                return path.into_os_string();
            }
        }
        if let Some(bin_dir) = crate::platform::paths::managed_connector_bin_dir() {
            let bundled = bin_dir.join(cli_bin);
            if bundled.is_file() {
                return bundled.into_os_string();
            }
        }
        // 常见 npm 全局前缀。原 linux 实现把这段放在第二个同条件的
        // `if program == cli_bin` 块里（早退语义相同），macOS 放同块内，现统一。
        let mut candidates = Vec::new();
        if let Ok(prefix) = std::env::var("NPM_CONFIG_PREFIX") {
            candidates.push(Path::new(&prefix).join("bin").join(program));
        }
        if let Ok(home) = std::env::var("HOME") {
            let home = Path::new(&home);
            candidates.push(home.join(".npm-global").join("bin").join(program));
            candidates.push(home.join(".local").join("bin").join(program));
        }
        for p in candidates {
            if p.is_file() {
                return p.into_os_string();
            }
        }
    }
    program.into()
}

/// 退出收割用的树杀（linux 与 macOS 共用，收口于此；非 unix 兜底目标由
/// unsupported.rs 保留显式 no-op 合约分支）。连接器 CLI 是 npm shim(shell 脚本
/// →node 子进程),spawn 侧已用 `process_group(0)` 让 shim 独立成组,这里按负
/// pid 杀整组,否则单杀 shim 的 pid 会把 node 孙进程孤儿化(与
/// platform::process::kill_process_tree 同语义)。若进程恰未成组(旧登记),
/// 追加一次单 pid 兜底。
///
/// **严禁**委托外部 kill 可执行文件(直接 spawn 或经 connector_cli_command 间接
/// 调用均含)执行组杀,后续模块开发一律直调系统调用,并由
/// `scripts/architecture-guard.py` 强制检查:procps-ng 4.0.4 的参数解析会把
/// `kill -9 -<pgid>` 的合法负 pid 错当 `-1` 处理(kill(-1) 杀光当前用户全部
/// 进程,2026-09-04 本机桌面会话两次被整台带走,audit 取证实锤)。
///
/// `pid <= 1` 或无法以正数收入 `i32` 的 pid 直接忽略:kill(2) 对 0 与 -1 有
/// 特殊语义(0 = 调用方所在整组,-1 = 当前用户全部进程),边界在本函数自检,
/// 不依赖调用方审计。
pub fn kill_pid_tree(pid: u32) {
    let Some(group) = i32::try_from(pid).ok().filter(|group| *group > 1) else {
        return;
    };
    // SAFETY: libc::kill is a direct kill(2) wrapper; no memory is touched.
    let group_ok = unsafe { libc::kill(-group, libc::SIGKILL) } == 0;
    if !group_ok {
        // SAFETY: libc::kill is a direct kill(2) wrapper; no memory is touched.
        let _ = unsafe { libc::kill(group, libc::SIGKILL) };
    }
}

/// 校验上传位置必须位于用户主目录之下。`home_raw` 由调用方按平台解析
/// （linux 的 `user_home_dir` 回退硬编码 `/tmp`，macOS/兜底平台回退
/// `std::env::temp_dir()`，见模块注释），这里只做 canonicalize + 前缀校验。
pub fn validate_upload_location_under_home(home_raw: &Path, canon: &Path) -> Result<(), String> {
    let home = platform_compat_path(
        &std::fs::canonicalize(home_raw)
            .unwrap_or_else(|_| home_raw.to_path_buf())
            .to_string_lossy(),
    );
    if !canon.starts_with(&home) {
        return Err(format!("path {} not under $HOME", canon.display()));
    }
    Ok(())
}

/// 腾讯会议首次使用时的 npm 安装目录：把 prefix 收到用户可写的
/// `~/.npm-global`，避免 GUI 无 sudo 写 `/usr/local` 失败（linux 与 macOS
/// 同策略，原 linux 版经 `prepend_connector_path_entries` 实现，行为一致）。
pub fn apply_user_npm_prefix(cmd: &mut Command) {
    if std::env::var_os("NPM_CONFIG_PREFIX").is_some()
        || std::env::var_os("npm_config_prefix").is_some()
    {
        return;
    }

    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let prefix = Path::new(&home).join(".npm-global");
    let bin = prefix.join("bin");
    let _ = std::fs::create_dir_all(&bin);
    cmd.env("NPM_CONFIG_PREFIX", &prefix)
        .env("npm_config_prefix", &prefix);
    prepend_connector_path_entries(cmd, [bin]);
}

/// 把给定目录插到 `PATH` 最前（原 PATH 依次跟在后面）；join 失败（非法字节）
/// 时保持 `cmd` 的 `PATH` 不动。
fn prepend_connector_path_entries(cmd: &mut Command, dirs: impl IntoIterator<Item = PathBuf>) {
    let mut paths: Vec<PathBuf> = dirs
        .into_iter()
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    if let Some(current) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&current));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }
}

/// 探测 PATH 中第一个可用的 python 解释器名。
/// 优先 `python3`，回退 `python`，最终默认 `python3`。
pub fn python_command() -> String {
    if which_in_path("python3") {
        return "python3".to_string();
    }
    if which_in_path("python") {
        return "python".to_string();
    }
    "python3".to_string()
}

/// POSIX 空设备路径。
pub fn null_device() -> &'static str {
    "/dev/null"
}

/// 在 `PATH` 环境变量中逐目录扫描给定命令是否可执行。
fn which_in_path(cmd: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if dir.join(cmd).is_file() {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_compat_path_is_identity() {
        assert_eq!(platform_compat_path("/usr/bin"), PathBuf::from("/usr/bin"));
    }

    #[test]
    fn upload_location_rejects_outside_home() {
        // home_raw 会被 canonicalize(macOS 的 /tmp 是 →/private/tmp 的符号链接)。
        assert!(
            validate_upload_location_under_home(Path::new("/tmp"), Path::new("/etc/passwd"))
                .is_err()
        );
    }

    #[test]
    fn path_component_eq_is_case_sensitive() {
        assert!(path_component_eq(OsStr::new("home"), "home"));
        assert!(!path_component_eq(OsStr::new("Home"), "home"));
    }

    #[test]
    fn python_command_defaults_to_python3() {
        // 无论 PATH 状态如何，至少返回 python3
        let cmd = python_command();
        assert!(cmd == "python3" || cmd == "python");
    }

    #[test]
    fn spawn_detached_and_reap_reaps_true_command() {
        // `true` (PATH lookup, usually /bin/true) exits in milliseconds:
        // verifies the spawn succeeded and the reaper thread does not
        // panic.
        // Whether the zombie is actually reaped is invisible without
        // reading the proc table; this at least pins the interface
        // contract (Ok + no deadlock).
        let mut command = std::process::Command::new("true");
        spawn_detached_and_reap(&mut command).expect("spawn true");
        // Give the reaper thread a moment to finish its wait; the test itself makes no blocking assertion.
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    #[test]
    fn spawn_detached_and_reap_reports_missing_binary() {
        let mut command = std::process::Command::new("/nonexistent/pinvou3-reaper-test");
        assert!(spawn_detached_and_reap(&mut command).is_err());
    }
}
