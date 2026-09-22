//! codex / claude / gemini(即 kimi) CLI 的安装、升级与版本/来源探测。

use super::runtime::run_version_probe;
use super::*;

/// 安装进度事件名(前端 composer 用它刷新「正在安装 X…」)。
const INSTALL_PROGRESS_EVENT: &str = "acp:install-progress";

pub(super) fn managed_runtime_dir() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("runtimes")
        .join(format!("codex-acp-{CODEX_ACP_VERSION}"))
}
pub(super) fn bundled_adapter_candidates(
    resource_root: &Path,
    development_bridge: &Path,
    package: &str,
) -> Vec<PathBuf> {
    let node_entry = |root: PathBuf| {
        root.join("node_modules")
            .join("@agentclientprotocol")
            .join(package)
            .join("dist")
            .join("index.js")
    };
    let package_entry = |root: PathBuf| node_entry(root.join("acp"));
    let mut candidates = vec![
        package_entry(resource_root.join("runtime").join("codex-bridge")),
        package_entry(resource_root.join("codex-bridge")),
        package_entry(resource_root.join("resources").join("codex-bridge")),
        package_entry(development_bridge.to_path_buf()),
    ];
    if package == "codex-acp" {
        let legacy_binary = if crate::platform::capabilities::is_windows() {
            "codex-acp.exe"
        } else {
            "codex-acp"
        };
        candidates.extend([
            node_entry(resource_root.join("codex-acp")),
            resource_root.join("codex-acp").join(legacy_binary),
            node_entry(resource_root.join("resources").join("codex-acp")),
            resource_root
                .join("resources")
                .join("codex-acp")
                .join(legacy_binary),
        ]);
    }
    candidates
}
pub(super) fn managed_adapter_path() -> PathBuf {
    managed_runtime_dir()
        .join("node_modules")
        .join(".bin")
        .join(platform::managed_adapter_name())
}
pub(super) fn codex_path_for_adapter(adapter: &Path) -> Option<PathBuf> {
    let name = platform::system_codex_name();
    if adapter
        .parent()?
        .file_name()
        .and_then(|value| value.to_str())
        == Some(".bin")
    {
        let candidate = adapter.parent()?.join(name);
        return candidate.is_file().then_some(candidate);
    }
    adapter.ancestors().find_map(|ancestor| {
        (ancestor.file_name().and_then(|value| value.to_str()) == Some("node_modules"))
            .then(|| ancestor.join(".bin").join(name))
            .filter(|candidate| candidate.is_file())
    })
}
pub(super) fn resolve_adapter_from(bundled: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PINVOU3_CODEX_ACP_BIN").map(PathBuf::from) {
        if nonempty_file(&path) {
            return Some(path);
        }
    }
    if let Some(path) = bundled {
        if nonempty_file(path) {
            return Some(path.to_path_buf());
        }
    }
    let managed = managed_adapter_path();
    if nonempty_file(&managed) {
        return Some(managed);
    }
    find_in_path(platform::managed_adapter_name())
}
pub(super) fn resolve_claude_adapter_from(bundled: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PINVOU3_CLAUDE_ACP_BIN").map(PathBuf::from) {
        if nonempty_file(&path) {
            return Some(path);
        }
    }
    if let Some(path) = bundled {
        if nonempty_file(path) {
            return Some(path.to_path_buf());
        }
    }
    find_in_path(if crate::platform::capabilities::is_windows() {
        "claude-agent-acp.cmd"
    } else {
        "claude-agent-acp"
    })
}
pub(super) fn resolve_codex_cli() -> Option<PathBuf> {
    // OpenAI Unix 安装器默认写入 ~/.local/bin；Windows 安装器写入
    // %LOCALAPPDATA%\Programs\OpenAI\Codex\bin。优先检查平台绝对路径，使脚本
    // 安装完成后无需重启桌面应用或依赖其启动时继承的 PATH。
    let script_installed = platform::codex_official_install_path();
    if nonempty_file(&script_installed) {
        return Some(script_installed);
    }
    find_agent_cli_in_path(AgentBackend::CodexAcp)
}
pub(super) fn resolve_claude_cli(adapter: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PINVOU3_CLAUDE_CLI_PATH").map(PathBuf::from) {
        if nonempty_file(&path) {
            return Some(path);
        }
    }
    let binary = if crate::platform::capabilities::is_windows() {
        "claude.exe"
    } else {
        "claude"
    };
    if let Some((package, binary)) = claude_native_runtime(
        std::env::consts::OS,
        std::env::consts::ARCH,
        crate::platform::capabilities::is_musl(),
    ) {
        if let Some(path) = adapter.and_then(|adapter| {
            adapter.ancestors().find_map(|ancestor| {
                (ancestor.file_name().and_then(|value| value.to_str()) == Some("node_modules"))
                    .then(|| ancestor.join("@anthropic-ai").join(&package).join(binary))
                    .filter(|candidate| nonempty_file(candidate))
            })
        }) {
            return Some(path);
        }
    }
    // 官方安装脚本默认目录（unix ~/.local/bin，Windows %USERPROFILE%\.local\bin），
    // 使脚本装完后无需重启 App 即可探测到。
    let script_installed = crate::platform::os::user_home_dir()
        .join(".local")
        .join("bin")
        .join(binary);
    if nonempty_file(&script_installed) {
        return Some(script_installed);
    }
    find_agent_cli_in_path(AgentBackend::ClaudeAcp)
}
pub(super) fn claude_native_runtime(
    os: &str,
    arch: &str,
    musl: bool,
) -> Option<(String, &'static str)> {
    let platform = match os {
        "windows" => "win32",
        "macos" => "darwin",
        "linux" => "linux",
        _ => return None,
    };
    let arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        _ => return None,
    };
    let libc = if os == "linux" && musl { "-musl" } else { "" };
    let binary = if os == "windows" {
        "claude.exe"
    } else {
        "claude"
    };
    Some((format!("claude-agent-sdk-{platform}-{arch}{libc}"), binary))
}
pub(super) fn resolve_kimi_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PINVOU3_KIMI_ACP_BIN").map(PathBuf::from) {
        if nonempty_file(&path) {
            return Some(path);
        }
    }
    let binary = if crate::platform::capabilities::is_windows() {
        "kimi.exe"
    } else {
        "kimi"
    };
    // 官方安装脚本默认目录（unix ~/.kimi-code/bin，Windows %USERPROFILE%\.kimi-code\bin）。
    // 优先于 PATH：脚本装完无需重启即可探测，且避开 PATH 中可能残留的废弃
    // Python 版 kimi-cli。
    let script_installed = crate::platform::os::user_home_dir()
        .join(".kimi-code")
        .join("bin")
        .join(binary);
    if nonempty_file(&script_installed) {
        return Some(script_installed);
    }
    find_agent_cli_in_path(AgentBackend::KimiAcp)
}
pub(super) fn agent_cli_names(backend: AgentBackend, windows: bool) -> &'static [&'static str] {
    match (backend, windows) {
        (AgentBackend::CodexAcp, true) => &["codex.cmd", "codex.exe"],
        (AgentBackend::ClaudeAcp, true) => &["claude.exe", "claude.cmd"],
        (AgentBackend::KimiAcp, true) => &["kimi.exe", "kimi.cmd"],
        (AgentBackend::CodexAcp, false) => &["codex"],
        (AgentBackend::ClaudeAcp, false) => &["claude"],
        (AgentBackend::KimiAcp, false) => &["kimi"],
        (AgentBackend::Deepseek, _) => &[],
    }
}
pub(super) fn find_agent_cli_in_path(backend: AgentBackend) -> Option<PathBuf> {
    agent_cli_names(backend, crate::platform::capabilities::is_windows())
        .iter()
        .find_map(|name| find_in_path(name))
}
pub(super) fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| nonempty_file(candidate))
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CliVersionProbe {
    Found(String),
    TimedOut,
    Failed,
}

/// `--version` 探测与登录态探测使用同一 15 秒上限。结果由上层按 Agent 缓存，
/// the process startup cost is only paid on the first selection or an explicit re-probe. The spawn/wait skeleton shares
/// `run_version_probe` with runtime.rs's Codex self-check; this probe nulls stdin and discards stderr,
/// with the result collapsed into a three-state enum (Found / TimedOut / Failed).
fn command_version_probe(executable: &Path) -> CliVersionProbe {
    let outcome = match run_version_probe(executable, |command| {
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
    }) {
        Ok(outcome) => outcome,
        Err(_) => return CliVersionProbe::Failed,
    };
    let Some(status) = outcome.status else {
        return CliVersionProbe::TimedOut;
    };
    if !status.success() {
        return CliVersionProbe::Failed;
    }
    let version = outcome.stdout.trim();
    if version.is_empty() {
        CliVersionProbe::Failed
    } else {
        CliVersionProbe::Found(version.to_string())
    }
}

fn version_from_probe(probe: CliVersionProbe) -> Option<String> {
    match probe {
        CliVersionProbe::Found(version) => Some(version),
        CliVersionProbe::TimedOut | CliVersionProbe::Failed => None,
    }
}

pub(super) fn command_version_output(executable: &Path) -> Option<String> {
    version_from_probe(command_version_probe(executable))
}

pub(super) fn probe_cli_version(executable: &Path) -> CliVersionProbe {
    command_version_probe(executable)
}
pub(super) fn probe_cli(backend: AgentBackend, path: Option<PathBuf>) -> Option<ResolvedCli> {
    let path = path?;
    // 官方 CLI 首次经过系统安全扫描时也可能偶发超过单次自检上限。仅超时
    // 时补一次重试；缺失或明确失败不额外 spawn。
    let probe = probe_cli_version(&path);
    let probe = if matches!(probe, CliVersionProbe::TimedOut) {
        probe_cli_version(&path)
    } else {
        probe
    };
    let version = version_from_probe(probe);
    let version_supported = version.as_deref().is_some_and(|version| match backend {
        AgentBackend::ClaudeAcp => claude_version_supported(version),
        AgentBackend::KimiAcp => kimi_version_supported(version),
        AgentBackend::Deepseek | AgentBackend::CodexAcp => false,
    });
    let install_source = if version_supported {
        path_install_source(backend, &path)
    } else {
        detect_install_source(backend, &path)
    };
    Some(ResolvedCli {
        path,
        version,
        install_source,
    })
}
/// 探测 brew 是否已安装该 Agent 的 CLI（macOS 版本过旧时走 brew upgrade）。
/// codex/claude-code 是 cask，kimi-code 是 formula（无 --cask）；
/// 非 macOS 平台 brew_available 恒 false。
pub(super) fn brew_package_installed(backend: AgentBackend) -> bool {
    if !platform::brew_available() {
        return false;
    }
    let args: &[&str] = match backend {
        AgentBackend::CodexAcp => &["list", "--cask", "codex"],
        AgentBackend::ClaudeAcp => &["list", "--cask", "claude-code"],
        AgentBackend::KimiAcp => &["list", "kimi-code"],
        AgentBackend::Deepseek => return false,
    };
    let mut command = std::process::Command::new(platform::brew_bin());
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    match child.wait_timeout(Duration::from_secs(10)) {
        Ok(Some(status)) => status.success(),
        Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            false
        }
    }
}
/// 各 Agent CLI 对应的 npm 全局包名。
pub(super) fn npm_package(backend: AgentBackend) -> Option<&'static str> {
    match backend {
        AgentBackend::CodexAcp => Some("@openai/codex"),
        AgentBackend::ClaudeAcp => Some("@anthropic-ai/claude-code"),
        AgentBackend::KimiAcp => Some("@moonshot-ai/kimi-code"),
        AgentBackend::Deepseek => None,
    }
}
/// npm 可执行文件：Windows 上是 npm.cmd。
pub(super) fn npm_executable() -> Option<PathBuf> {
    if crate::platform::capabilities::is_windows() {
        find_in_path("npm.cmd").or_else(|| find_in_path("npm"))
    } else {
        find_in_path("npm")
    }
}
/// `npm ls -g <pkg> --depth=0` 退出码 0 即视为 npm 全局安装；10 秒超时防挂住。
pub(super) fn npm_global_installed(package: &str) -> bool {
    let Some(npm) = npm_executable() else {
        return false;
    };
    let mut command = crate::platform::process::external_command(&npm);
    command
        .args(["ls", "-g", package, "--depth=0"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    match child.wait_timeout(Duration::from_secs(10)) {
        Ok(Some(status)) => status.success(),
        Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            false
        }
    }
}
/// 判定已解析 CLI 的安装来源。多份并存（如同机同时有 brew cask 与官方脚本版）
/// 时必须按「实际被解析使用的那一份」判定，否则升级会打到包管理器管理的另一份，
/// 正在使用的旧版原地不动。顺序：官方脚本目录前缀 → brew 前缀+brew 已装 →
/// npm 全局根+npm 已装 → 路径无法判定时回退 brew/npm 全局查询（维持原行为）。
pub(super) fn detect_install_source(backend: AgentBackend, path: &Path) -> Option<&'static str> {
    if let Some(source) = path_install_source(backend, path) {
        return Some(source);
    }
    let brew_path_match = platform::brew_prefix().is_some_and(|prefix| path.starts_with(prefix));
    let brew_installed = brew_package_installed(backend);
    let npm_path_match = npm_global_root().is_some_and(|root| {
        path_in_npm_global(path, &root, crate::platform::capabilities::is_windows())
    });
    let npm_installed = npm_package(backend).is_some_and(npm_global_installed);
    finalize_install_source(
        brew_path_match,
        brew_installed,
        npm_path_match,
        npm_installed,
    )
}
/// 路径与包管理器双重判定的优先级：路径命中且包管理器确认已装才认来源；
/// 路径无法判定时回退包管理器全局查询。纯函数，便于单测覆盖多份并存场景。
pub(super) fn finalize_install_source(
    brew_path_match: bool,
    brew_installed: bool,
    npm_path_match: bool,
    npm_installed: bool,
) -> Option<&'static str> {
    if brew_path_match && brew_installed {
        return Some("brew");
    }
    if npm_path_match && npm_installed {
        return Some("npm");
    }
    if brew_installed {
        return Some("brew");
    }
    if npm_installed {
        return Some("npm");
    }
    None
}
/// `npm prefix -g` 输出的全局根目录；npm 不可用或超时返回 None。
pub(super) fn npm_global_root() -> Option<PathBuf> {
    let npm = npm_executable()?;
    let mut command = crate::platform::process::external_command(&npm);
    command
        .args(["prefix", "-g"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().ok()?;
    match child.wait_timeout(Duration::from_secs(10)) {
        Ok(Some(status)) if status.success() => {
            let mut stdout = String::new();
            child.stdout.take()?.read_to_string(&mut stdout).ok()?;
            let root = stdout.trim();
            (!root.is_empty()).then_some(PathBuf::from(root))
        }
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}
/// 路径是否位于 npm 全局根下：unix 可执行文件链接在 <root>/bin，Windows 直接在根目录。
pub(super) fn path_in_npm_global(path: &Path, root: &Path, windows: bool) -> bool {
    let bin = if windows {
        root.to_path_buf()
    } else {
        root.join("bin")
    };
    path.starts_with(bin)
}
/// 按路径前缀判定官方脚本来源（codex/claude ~/.local/bin、kimi ~/.kimi-code/bin）。
pub(super) fn path_install_source(backend: AgentBackend, path: &Path) -> Option<&'static str> {
    let home = crate::platform::os::user_home_dir();
    match backend {
        AgentBackend::CodexAcp
            if platform::codex_official_install_path()
                .parent()
                .is_some_and(|directory| path.starts_with(directory)) =>
        {
            Some("script")
        }
        AgentBackend::ClaudeAcp if path.starts_with(home.join(".local").join("bin")) => {
            Some("script")
        }
        AgentBackend::KimiAcp if path.starts_with(home.join(".kimi-code").join("bin")) => {
            Some("script")
        }
        _ => None,
    }
}
/// installed=false 时的安装动作：探测到过旧 CLI 且来源可识别时优先包管理器
/// upgrade (brew/npm); other sources or a missing CLI keep the default install method.
pub(super) fn install_action_for(
    install_source: Option<&'static str>,
    npm_available: bool,
    official_script_supported: bool,
) -> &'static str {
    match install_source {
        Some("brew") => "brew_upgrade",
        Some("npm") if npm_available => "npm_upgrade",
        _ if official_script_supported => "official_script",
        _ => "manual",
    }
}
/// install_acp_agent 可选 action override 校验。
pub(super) fn parse_install_action(action: &str) -> Result<&'static str> {
    match action {
        "none" => Ok("none"),
        "brew_upgrade" => Ok("brew_upgrade"),
        "npm_upgrade" => Ok("npm_upgrade"),
        "official_script" => Ok("official_script"),
        "manual" => Ok("manual"),
        other => bail!(
            "非法安装动作: {other}（可选值: none / brew_upgrade / npm_upgrade / official_script / manual）"
        ),
    }
}
/// brew 安装/升级命令参数：codex 已通过 brew 安装走 upgrade，未安装走 install；
/// claude/kimi 只在探测到 brew 来源时调用，一律 upgrade（kimi-code 是 formula）。
/// 返回（命令描述，参数），供执行与诊断日志使用。
pub(super) fn brew_install_args(
    backend: AgentBackend,
    brew_installed: bool,
) -> Option<(&'static str, Vec<&'static str>)> {
    match backend {
        AgentBackend::CodexAcp if brew_installed => Some((
            "brew upgrade --cask codex",
            vec!["upgrade", "--cask", "codex"],
        )),
        AgentBackend::CodexAcp => Some((
            "brew install --cask codex",
            vec!["install", "--cask", "codex"],
        )),
        AgentBackend::ClaudeAcp => Some((
            "brew upgrade --cask claude-code",
            vec!["upgrade", "--cask", "claude-code"],
        )),
        AgentBackend::KimiAcp => Some(("brew upgrade kimi-code", vec!["upgrade", "kimi-code"])),
        AgentBackend::Deepseek => None,
    }
}
/// npm 全局升级参数：`npm install -g <pkg>@latest`。
pub(super) fn npm_upgrade_args(backend: AgentBackend) -> Option<Vec<String>> {
    Some(vec![
        "install".to_string(),
        "-g".to_string(),
        format!("{}@latest", npm_package(backend)?),
    ])
}
pub(super) fn codex_version_changed(previous: Option<&str>, current: Option<&str>) -> bool {
    current.is_some() && current != previous
}
/// 裸 semver（`0.31.1`）：claude/kimi 的版本门禁都要求完整三段数字，
/// 旧 Python 版 kimi-cli 等非标准输出一律判不合规。
pub(super) fn is_bare_semver(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
}
/// claude `--version` 输出形如 `2.1.163 (Claude Code)`，取首个空白分隔 token。
pub(super) fn claude_version_supported(version: &str) -> bool {
    version
        .split_whitespace()
        .next()
        .is_some_and(|token| is_bare_semver(token) && version_at_least(token, MIN_CLAUDE_VERSION))
}
/// kimi `--version` 输出为裸 semver（如 `0.31.1`），解析失败一律不合规。
pub(super) fn kimi_version_supported(version: &str) -> bool {
    is_bare_semver(version) && version_at_least(version, MIN_KIMI_VERSION)
}
/// 官方安装脚本覆盖的平台。Claude 脚本与其原生运行时平台集一致；
/// Kimi 脚本额外排除 Linux musl（官方只发布 glibc 构建）。
pub(super) fn official_script_supported(backend: AgentBackend) -> bool {
    let (os, arch, musl) = (
        std::env::consts::OS,
        std::env::consts::ARCH,
        crate::platform::capabilities::is_musl(),
    );
    match backend {
        AgentBackend::CodexAcp => {
            matches!(os, "macos" | "linux" | "windows") && matches!(arch, "x86_64" | "aarch64")
        }
        AgentBackend::ClaudeAcp => claude_native_runtime(os, arch, musl).is_some(),
        AgentBackend::KimiAcp => {
            matches!(arch, "x86_64" | "aarch64")
                && (matches!(os, "macos" | "windows") || (os == "linux" && !musl))
        }
        AgentBackend::Deepseek => false,
    }
}

struct InstallOutputReaders {
    child_finished: Arc<AtomicBool>,
    stdout: Option<tokio::task::JoinHandle<String>>,
    stderr: Option<tokio::task::JoinHandle<String>>,
}

impl InstallOutputReaders {
    fn spawn(
        app: &AppHandle,
        backend: AgentBackend,
        stdout: tokio::process::ChildStdout,
        stderr: tokio::process::ChildStderr,
    ) -> Self {
        let child_finished = Arc::new(AtomicBool::new(false));
        let stdout_app = app.clone();
        let stderr_app = app.clone();
        let stdout_child_finished = child_finished.clone();
        let stderr_child_finished = child_finished.clone();
        let stdout = tokio::spawn(async move {
            stream_install_lines(
                &stdout_app,
                backend,
                "stdout",
                stdout,
                &stdout_child_finished,
            )
            .await
        });
        let stderr = tokio::spawn(async move {
            stream_install_lines(
                &stderr_app,
                backend,
                "stderr",
                stderr,
                &stderr_child_finished,
            )
            .await
        });
        Self {
            child_finished,
            stdout: Some(stdout),
            stderr: Some(stderr),
        }
    }

    // finish consumes self by value; stdout/stderr are only set to Some in the
    // constructor with no other write site, so take must yield Some. Moving the
    // fields out is impossible due to Drop; the panic branch is unreachable.
    #[allow(clippy::expect_used)]
    async fn finish(mut self) -> (String, String) {
        self.child_finished.store(true, Ordering::Release);
        let stdout = self.stdout.take().expect("stdout reader missing");
        let stderr = self.stderr.take().expect("stderr reader missing");
        let (stdout, stderr) = tokio::join!(stdout, stderr);
        (stdout.unwrap_or_default(), stderr.unwrap_or_default())
    }
}

impl Drop for InstallOutputReaders {
    fn drop(&mut self) {
        // wait/timeout 任一异常出口也要通知已启动的读取任务收口，避免永久挂起。
        self.child_finished.store(true, Ordering::Release);
    }
}

/// Differences injected by `run_official_install_script` and `run_npm_global_upgrade` into
/// [`run_managed_install`]; the rest of the skeleton (register pid → post-spawn cancel recheck → progress →
/// streaming output → 600s timeout → diagnostics to disk → cancel rewrite → stderr-tail bail) is
/// owned by `run_managed_install`, keeping both paths byte-identical in wording.
struct ManagedInstallStage {
    /// Diagnostics stage prefix ("script" / "npm"), composed into `<stage>:timeout` / `<stage>:output`.
    diag_stage: &'static str,
    /// Command line shown in progress events.
    command_line: String,
    /// Whether to make the child a process-group leader (npm needs it; the script process does not).
    process_group: bool,
    spawn_context: String,
    stdout_context: &'static str,
    stderr_context: &'static str,
    wait_context: &'static str,
    timeout_message: String,
    /// Failure message subject (the "{display} install script exited" / "npm global upgrade of {display} exited" message).
    failure_subject: String,
    /// Whether to append a network troubleshooting hint when stderr has no usable content (official script path only).
    failure_hint: bool,
}

/// Shared execution skeleton for managed installs. Differences are injected only via [`ManagedInstallStage`]:
/// InstallChildGuard::register → post-spawn cancel recheck → emit_install_progress →
/// InstallOutputReaders::spawn → 600s tokio timeout (diagnostics `<stage>:timeout`) →
/// finish → diagnostics `<stage>:output` (output tail) → cancel rewrite and stderr-tail
/// bail on the failure exits.
#[allow(clippy::too_many_arguments)]
async fn run_managed_install(
    app: &AppHandle,
    backend: AgentBackend,
    operation_id: &str,
    install_children: &Arc<parking_lot::Mutex<HashMap<AgentBackend, u32>>>,
    install_cancelled: &Arc<parking_lot::Mutex<HashSet<AgentBackend>>>,
    mut command: tokio::process::Command,
    stage: ManagedInstallStage,
) -> Result<()> {
    if stage.process_group {
        // Separate process group: killed as a group on cancel so npm-spawned postinstall
        // scripts are not orphaned. (Platform details live in process.rs; this layer has no target-platform cfg.)
        crate::platform::process::tokio_process_group_leader(&mut command);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| stage.spawn_context.clone())?;
    // spawn 后登记 pid 供取消命令杀进程树；guard 在本函数任意出口注销。
    let _child_guard = InstallChildGuard::register(install_children, backend, child.id());
    // 取消可能发生在 spawn 与登记之间：登记后立即补检一次。
    if install_cancelled.lock().contains(&backend) {
        let _ = child.kill().await;
    }
    emit_install_progress(app, backend, "command", &stage.command_line);
    let stdout = child.stdout.take().context(stage.stdout_context)?;
    let stderr = child.stderr.take().context(stage.stderr_context)?;
    let output_readers = InstallOutputReaders::spawn(app, backend, stdout, stderr);
    const TIMEOUT: Duration = Duration::from_secs(600);
    let status = match tokio::time::timeout(TIMEOUT, child.wait()).await {
        Ok(result) => result.context(stage.wait_context)?,
        Err(_) => {
            diagnostics::write(
                operation_id,
                &format!("{}:timeout", stage.diag_stage),
                format!("timeout_seconds={}", TIMEOUT.as_secs()),
            );
            let _ = child.kill().await;
            let _ = child.wait().await;
            bail!("{}", stage.timeout_message);
        }
    };
    let (stdout, stderr) = output_readers.finish().await;
    diagnostics::write(
        operation_id,
        &format!("{}:output", stage.diag_stage),
        format!(
            "status={status} stdout_tail={} stderr_tail={}",
            output_tail(&stdout, 20),
            output_tail(&stderr, 20)
        ),
    );
    if !status.success() {
        // 进程被用户取消（taskkill/kill）会以失败状态走到这里：改写为已取消语义。
        if install_cancelled.lock().remove(&backend) {
            bail!(
                "{INSTALL_CANCELLED_MARKER}{} 安装已取消",
                backend.display_name()
            );
        }
        let stderr_tail = output_tail(&stderr, 4);
        // When stderr has no usable content (empty or only system noise like
        // "retry"), give an actionable hint: the official script depends on
        // releases.openai.com / GitHub, so download failures are mostly
        // network-related. The manual-install guidance is generated per agent
        // package name/script (it must not point at codex unconditionally).
        let hint = if stage.failure_hint
            && (stderr_tail.trim().is_empty() || stderr_tail.trim().chars().count() < 8)
        {
            let npm_pkg = npm_package(backend).unwrap_or("");
            let (unix_url, windows_url) = official_script_urls(backend);
            format!(
                "; check the network connection and retry. You can also install manually: npm install -g {npm_pkg}, or run the official install script (macOS/Linux: curl -fsSL {unix_url} | sh; Windows: irm {windows_url} | iex)"
            )
        } else {
            String::new()
        };
        bail!(
            "{}: {status}; stderr: {}{}",
            stage.failure_subject,
            stderr_tail,
            hint
        );
    }
    Ok(())
}

/// Runs the official install script (unix: `curl -fsSL <url> | bash`, Windows: `irm <url> | iex`),
/// 10-minute timeout, with the output tail written to the diagnostics log.
pub(super) async fn run_official_install_script(
    app: &AppHandle,
    backend: AgentBackend,
    operation_id: &str,
    install_children: &Arc<parking_lot::Mutex<HashMap<AgentBackend, u32>>>,
    install_cancelled: &Arc<parking_lot::Mutex<HashSet<AgentBackend>>>,
) -> Result<()> {
    let (unix_url, windows_url) = official_script_urls(backend);
    if unix_url.is_empty() {
        bail!("{} 不支持官方脚本安装", backend.display_name());
    }
    let mut command = crate::platform::process::install_script_command(unix_url, windows_url);
    if backend == AgentBackend::CodexAcp {
        // The official OpenAI script installs latest by default; non-interactive mode avoids the desktop app waiting on a background PATH-conflict prompt.
        command.env("CODEX_NON_INTERACTIVE", "1");
    }
    let command_line = if crate::platform::capabilities::is_windows() {
        format!("irm {windows_url} | iex")
    } else {
        format!("curl -fsSL {unix_url} | bash")
    };
    run_managed_install(
        app,
        backend,
        operation_id,
        install_children,
        install_cancelled,
        command,
        ManagedInstallStage {
            diag_stage: "script",
            command_line,
            process_group: false,
            spawn_context: format!("failed to spawn {} install script", backend.display_name()),
            stdout_context: "failed to read install script stdout",
            stderr_context: "failed to read install script stderr",
            wait_context: "failed to wait for install script process",
            timeout_message: format!(
                "{} install script did not finish within 10 minutes; check the network and retry",
                backend.display_name()
            ),
            failure_subject: format!("{} install script exited", backend.display_name()),
            failure_hint: true,
        },
    )
    .await
}
/// Runs `npm install -g <pkg>@latest` as a global upgrade (npm.cmd via cmd on
/// Windows), 10-minute timeout, with the output tail written to the
/// diagnostics log.
pub(super) async fn run_npm_global_upgrade(
    app: &AppHandle,
    backend: AgentBackend,
    operation_id: &str,
    install_children: &Arc<parking_lot::Mutex<HashMap<AgentBackend, u32>>>,
    install_cancelled: &Arc<parking_lot::Mutex<HashSet<AgentBackend>>>,
) -> Result<()> {
    let args = npm_upgrade_args(backend)
        .with_context(|| format!("{} 不支持 npm 全局升级", backend.display_name()))?;
    let npm = npm_executable().context("未检测到 npm，无法通过 npm 全局升级")?;
    let mut command = crate::platform::process::external_tokio_command(&npm);
    command.args(&args);
    run_managed_install(
        app,
        backend,
        operation_id,
        install_children,
        install_cancelled,
        command,
        ManagedInstallStage {
            diag_stage: "npm",
            command_line: format!("npm install -g {}", npm_package(backend).unwrap_or("")),
            process_group: true,
            spawn_context: "failed to spawn npm global upgrade".to_string(),
            stdout_context: "failed to read npm stdout",
            stderr_context: "failed to read npm stderr",
            wait_context: "failed to wait for npm global upgrade process",
            timeout_message: format!(
                "{} npm global upgrade did not finish within 10 minutes; check the network and retry",
                backend.display_name()
            ),
            failure_subject: format!("npm global upgrade of {} exited", backend.display_name()),
            failure_hint: false,
        },
    )
    .await
}

/// brew's idempotent notices do not count as failure: install reports already installed, upgrade reports already
/// up-to-date.
fn brew_already_done(stdout: &str, stderr: &str) -> bool {
    ["already installed", "already up-to-date"]
        .iter()
        .any(|marker| stdout.contains(marker) || stderr.contains(marker))
}

/// Execution skeleton for Homebrew install/upgrade (same shape as [`run_managed_install`]): register pid →
/// post-spawn cancel recheck → progress → streaming output → 600s timeout (brew previously had no timeout, so a
/// hang would hold the install mutex guard forever) → diagnostics to disk → idempotent notices pass through → cancel rewrite →
/// stderr-tail bail. `upgrade_via_homebrew` keeps the decision logic such as source detection and only delegates
/// subprocess execution.
pub(super) async fn run_brew_install(
    app: &AppHandle,
    backend: AgentBackend,
    operation_id: &str,
    install_children: &Arc<parking_lot::Mutex<HashMap<AgentBackend, u32>>>,
    install_cancelled: &Arc<parking_lot::Mutex<HashSet<AgentBackend>>>,
    args: &[&str],
    command_description: &str,
) -> Result<()> {
    let mut command = tokio::process::Command::new(platform::brew_bin());
    command.args(args);
    // Separate process group: killed as a group on cancel so brew-spawned processes are not orphaned.
    crate::platform::process::tokio_process_group_leader(&mut command);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("failed to spawn Homebrew")?;
    // Register the pid after spawn so the cancel command can kill the process
    // tree; the guard deregisters on every exit path of this function.
    let _child_guard = InstallChildGuard::register(install_children, backend, child.id());
    // Cancellation can land between spawn and registration: recheck right
    // after registering.
    if install_cancelled.lock().contains(&backend) {
        let _ = child.kill().await;
    }
    emit_install_progress(app, backend, "command", command_description);
    let stdout = child
        .stdout
        .take()
        .context("failed to read Homebrew stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to read Homebrew stderr")?;
    let output_readers = InstallOutputReaders::spawn(app, backend, stdout, stderr);
    const TIMEOUT: Duration = Duration::from_secs(600);
    let status = match tokio::time::timeout(TIMEOUT, child.wait()).await {
        Ok(result) => result.context("failed to wait for Homebrew process")?,
        Err(_) => {
            diagnostics::write(
                operation_id,
                "homebrew:timeout",
                format!("timeout_seconds={}", TIMEOUT.as_secs()),
            );
            let _ = child.kill().await;
            let _ = child.wait().await;
            bail!("Homebrew install did not finish within 10 minutes; please retry");
        }
    };
    let (stdout, stderr) = output_readers.finish().await;
    diagnostics::write(
        operation_id,
        "homebrew:output",
        format!(
            "status={status} stdout_tail={} stderr_tail={}",
            output_tail(&stdout, 20),
            output_tail(&stderr, 20)
        ),
    );
    if status.success() || brew_already_done(&stdout, &stderr) {
        return Ok(());
    }
    // The process was cancelled by the user (taskkill/kill) and reaches here with a failure status: rewrite to cancelled semantics.
    if install_cancelled.lock().remove(&backend) {
        bail!(
            "{INSTALL_CANCELLED_MARKER}{} 安装已取消",
            backend.display_name()
        );
    }
    bail!(
        "{command_description} failed (exit {}): {}",
        status.code().unwrap_or(-1),
        output_tail(&stderr, 4)
    );
}
pub(super) fn output_tail(output: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = output.lines().collect();
    lines[lines.len().saturating_sub(max_lines)..].join(" / ")
}
pub(super) fn nonempty_file(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}
pub(super) fn codex_upgrade_required(message: &str) -> bool {
    message
        .to_ascii_lowercase()
        .contains("requires a newer version of codex")
}
pub(super) fn installed_node_version(node: &Path) -> Option<String> {
    command_version_output(node).map(|version| version.trim_start_matches('v').to_string())
}
pub(super) fn node_major_version(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub(super) fn managed_path_is_versioned() {
        let path = managed_adapter_path().to_string_lossy().into_owned();
        assert!(path.contains(CODEX_ACP_VERSION));
        assert!(path.contains("codex-acp"));
    }

    #[test]
    pub(super) fn node_version_parser_requires_a_major() {
        assert_eq!(node_major_version("20.18.1"), Some(20));
        assert_eq!(node_major_version("v20.18.1"), None);
        assert_eq!(node_major_version("unknown"), None);
    }

    #[test]
    pub(super) fn claude_native_runtime_is_explicit_for_supported_platforms() {
        assert_eq!(
            claude_native_runtime("macos", "aarch64", false),
            Some(("claude-agent-sdk-darwin-arm64".to_string(), "claude"))
        );
        assert_eq!(
            claude_native_runtime("macos", "x86_64", false),
            Some(("claude-agent-sdk-darwin-x64".to_string(), "claude"))
        );
        assert_eq!(
            claude_native_runtime("windows", "x86_64", false),
            Some(("claude-agent-sdk-win32-x64".to_string(), "claude.exe"))
        );
        assert_eq!(
            claude_native_runtime("linux", "aarch64", true),
            Some(("claude-agent-sdk-linux-arm64-musl".to_string(), "claude"))
        );
        assert_eq!(claude_native_runtime("freebsd", "x86_64", false), None);
        assert_eq!(claude_native_runtime("windows", "riscv64", false), None);
    }

    #[test]
    pub(super) fn empty_adapter_file_is_not_treated_as_installed() {
        let root =
            std::env::temp_dir().join(format!("pinvou3-codex-adapter-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create adapter test directory");
        let adapter = root.join("codex-acp.js");
        std::fs::File::create(&adapter).expect("empty adapter");
        assert!(!nonempty_file(&adapter));
        std::fs::write(&adapter, "console.log('ok');").expect("write adapter");
        assert!(nonempty_file(&adapter));
        std::fs::remove_dir_all(root).expect("cleanup adapter test directory");
    }

    #[test]
    pub(super) fn status_serializes_install_contract_fields() {
        let status = CodexAcpStatus {
            agent_id: "codex",
            agent_name: "Codex",
            version: Some("0.146.0".to_string()),
            latest_version: Some("0.147.0".to_string()),
            installed: false,
            update_available: true,
            update_required: true,
            bridge_ready: false,
            node_supported: false,
            codex_available: false,
            runtime_source: None,
            min_version: MIN_CODEX_VERSION,
            install_action: "official_script",
            install_source: Some("npm".to_string()),
            authenticated: false,
            login_in_progress: false,
            login_url: None,
            login_code: None,
            login_input_required: false,
            installing: false,
            error: None,
            install_command: None,
            install_latest_line: None,
            setup_hint: None,
        };
        let value = serde_json::to_value(&status).expect("serialize CodexAcpStatus");
        assert_eq!(value["version"], json!("0.146.0"));
        assert_eq!(value["latest_version"], json!("0.147.0"));
        assert_eq!(value["min_version"], json!(MIN_CODEX_VERSION));
        assert_eq!(value["install_action"], json!("official_script"));
        assert_eq!(value["install_source"], json!("npm"));
        assert_eq!(value["update_available"], json!(true));
        assert_eq!(value["update_required"], json!(true));

        let mut cli_ready_without_bridge = status.clone();
        cli_ready_without_bridge.codex_available = true;
        cli_ready_without_bridge.update_available = false;
        cli_ready_without_bridge.update_required = false;
        assert!(
            latest::ensure_agent_cli_ready(AgentBackend::CodexAcp, &cli_ready_without_bridge)
                .is_ok(),
            "Codex CLI installation must not fail solely because its bundled bridge is unavailable"
        );
        cli_ready_without_bridge.update_required = true;
        assert!(
            latest::ensure_agent_cli_ready(AgentBackend::CodexAcp, &cli_ready_without_bridge)
                .is_err()
        );
        cli_ready_without_bridge.update_required = false;
        cli_ready_without_bridge.update_available = true;
        assert!(
            latest::ensure_agent_cli_ready(AgentBackend::CodexAcp, &cli_ready_without_bridge)
                .is_ok(),
            "advisory latest upgrade must not fail the install when the package manager lags behind"
        );
    }

    #[test]
    pub(super) fn install_source_follows_path_of_cli_in_use() {
        // 多份并存时以「正在使用的这份」的路径为准，避免升级打到另一份：
        // 路径命中 brew 前缀且 brew 确认已装 → brew；npm 全局根同理。
        assert_eq!(
            finalize_install_source(true, true, false, false),
            Some("brew")
        );
        assert_eq!(
            finalize_install_source(false, false, true, true),
            Some("npm")
        );
        // 路径命中但包管理器查无此包（如手动拷贝进前缀目录）→ 不冒认，继续回退。
        assert_eq!(
            finalize_install_source(true, false, false, true),
            Some("npm")
        );
        assert_eq!(finalize_install_source(true, false, false, false), None);
        // 路径无法判定时回退包管理器全局查询（维持原行为）。
        assert_eq!(
            finalize_install_source(false, true, false, false),
            Some("brew")
        );
        assert_eq!(
            finalize_install_source(false, false, false, true),
            Some("npm")
        );
        assert_eq!(finalize_install_source(false, false, false, false), None);
    }

    #[test]
    pub(super) fn npm_global_path_matches_platform_layout() {
        let root = Path::new("/home/user/.nvm/versions/node/v22.0.0");
        assert!(path_in_npm_global(
            &root.join("bin").join("claude"),
            root,
            false
        ));
        assert!(!path_in_npm_global(
            &root
                .join("lib")
                .join("node_modules")
                .join(".bin")
                .join("claude"),
            root,
            false
        ));
        assert!(!path_in_npm_global(
            Path::new("/usr/local/bin/claude"),
            root,
            false
        ));
        let win_root = Path::new("C:\\Users\\u\\AppData\\Roaming\\npm");
        assert!(path_in_npm_global(
            &win_root.join("claude.cmd"),
            win_root,
            true
        ));
        assert!(!path_in_npm_global(
            Path::new("C:\\tools\\claude.cmd"),
            win_root,
            true
        ));
    }

    #[test]
    pub(super) fn install_action_follows_detected_install_source() {
        // brew source always maps to brew_upgrade.
        assert_eq!(
            install_action_for(Some("brew"), false, true),
            "brew_upgrade"
        );
        // npm source with an executable npm maps to npm_upgrade.
        assert_eq!(install_action_for(Some("npm"), true, true), "npm_upgrade");
        // 官方脚本优先（原设计）：script/未知来源/首次安装（None）即使 npm
        // 可用也走官方脚本；npm 仅作为 npm 来源的升级通道；npm 不可用或脚本
        // 不支持时依次回退脚本/手动。
        assert_eq!(
            install_action_for(Some("script"), true, true),
            "official_script"
        );
        assert_eq!(install_action_for(None, true, true), "official_script");
        assert_eq!(install_action_for(Some("npm"), true, true), "npm_upgrade");
        assert_eq!(
            install_action_for(Some("script"), false, true),
            "official_script"
        );
        assert_eq!(install_action_for(None, false, true), "official_script");
        assert_eq!(
            install_action_for(Some("npm"), false, true),
            "official_script"
        );
        assert_eq!(install_action_for(None, false, false), "manual");
    }

    #[test]
    pub(super) fn install_action_override_accepts_only_known_actions() {
        for action in [
            "none",
            "brew_upgrade",
            "npm_upgrade",
            "official_script",
            "manual",
        ] {
            assert_eq!(parse_install_action(action).unwrap(), action);
        }
        assert!(parse_install_action("brew").is_err());
        assert!(parse_install_action("").is_err());
        assert!(parse_install_action("managed").is_err());
        assert!(parse_install_action(concat!("managed_", "download")).is_err());
    }

    #[test]
    pub(super) fn installing_state_is_isolated_per_agent() {
        let installing_agents = Arc::new(parking_lot::RwLock::new(HashSet::new()));
        let claude = AgentInstallGuard::try_start(&installing_agents, AgentBackend::ClaudeAcp)
            .expect("Claude install should start");

        assert!(installing_agents.read().contains(&AgentBackend::ClaudeAcp));
        assert!(!installing_agents.read().contains(&AgentBackend::CodexAcp));
        assert!(!installing_agents.read().contains(&AgentBackend::KimiAcp));
        assert!(
            AgentInstallGuard::try_start(&installing_agents, AgentBackend::ClaudeAcp).is_none(),
            "the same Agent must remain mutually exclusive"
        );

        let codex = AgentInstallGuard::try_start(&installing_agents, AgentBackend::CodexAcp)
            .expect("a different Agent should not share Claude's install lock");
        assert!(installing_agents.read().contains(&AgentBackend::CodexAcp));

        drop(claude);
        assert!(!installing_agents.read().contains(&AgentBackend::ClaudeAcp));
        assert!(installing_agents.read().contains(&AgentBackend::CodexAcp));
        drop(codex);
        assert!(installing_agents.read().is_empty());
    }

    #[test]
    pub(super) fn runtime_errors_are_isolated_per_agent() {
        let errors = AgentRuntimeErrors::default();
        errors.set(AgentBackend::ClaudeAcp, "Claude install failed".to_string());
        assert_eq!(
            errors.get(AgentBackend::ClaudeAcp).as_deref(),
            Some("Claude install failed")
        );
        assert_eq!(errors.get(AgentBackend::CodexAcp), None);
        assert_eq!(errors.get(AgentBackend::KimiAcp), None);

        errors.set(AgentBackend::CodexAcp, "Codex update required".to_string());
        errors.clear(AgentBackend::ClaudeAcp);
        assert_eq!(errors.get(AgentBackend::ClaudeAcp), None);
        assert_eq!(
            errors.get(AgentBackend::CodexAcp).as_deref(),
            Some("Codex update required")
        );
    }

    #[test]
    pub(super) fn windows_cli_candidates_cover_native_and_npm_shims() {
        assert_eq!(
            agent_cli_names(AgentBackend::CodexAcp, true),
            &["codex.cmd", "codex.exe"]
        );
        assert_eq!(
            agent_cli_names(AgentBackend::ClaudeAcp, true),
            &["claude.exe", "claude.cmd"]
        );
        assert_eq!(
            agent_cli_names(AgentBackend::KimiAcp, true),
            &["kimi.exe", "kimi.cmd"]
        );
        assert_eq!(agent_cli_names(AgentBackend::KimiAcp, false), &["kimi"]);
    }

    #[test]
    pub(super) fn dynamic_codex_upgrade_gate_requires_an_actual_version_change() {
        assert!(!codex_version_changed(Some("0.146.0"), Some("0.146.0")));
        assert!(codex_version_changed(Some("0.146.0"), Some("0.147.0")));
        assert!(!codex_version_changed(Some("0.146.0"), None));
        assert!(codex_version_changed(None, Some("0.146.0")));
    }

    #[test]
    pub(super) fn brew_install_args_cover_all_agents() {
        // codex：已装 brew cask 走 upgrade，未装走 install。
        assert_eq!(
            brew_install_args(AgentBackend::CodexAcp, true),
            Some((
                "brew upgrade --cask codex",
                vec!["upgrade", "--cask", "codex"]
            ))
        );
        assert_eq!(
            brew_install_args(AgentBackend::CodexAcp, false),
            Some((
                "brew install --cask codex",
                vec!["install", "--cask", "codex"]
            ))
        );
        assert_eq!(
            brew_install_args(AgentBackend::ClaudeAcp, true),
            Some((
                "brew upgrade --cask claude-code",
                vec!["upgrade", "--cask", "claude-code"]
            ))
        );
        // kimi-code 是 formula，无 --cask。
        assert_eq!(
            brew_install_args(AgentBackend::KimiAcp, true),
            Some(("brew upgrade kimi-code", vec!["upgrade", "kimi-code"]))
        );
        assert_eq!(brew_install_args(AgentBackend::Deepseek, true), None);
    }

    #[test]
    pub(super) fn npm_upgrade_args_target_latest_global_package() {
        assert_eq!(
            npm_upgrade_args(AgentBackend::CodexAcp),
            Some(vec![
                "install".to_string(),
                "-g".to_string(),
                "@openai/codex@latest".to_string()
            ])
        );
        assert_eq!(
            npm_upgrade_args(AgentBackend::ClaudeAcp),
            Some(vec![
                "install".to_string(),
                "-g".to_string(),
                "@anthropic-ai/claude-code@latest".to_string()
            ])
        );
        assert_eq!(
            npm_upgrade_args(AgentBackend::KimiAcp),
            Some(vec![
                "install".to_string(),
                "-g".to_string(),
                "@moonshot-ai/kimi-code@latest".to_string()
            ])
        );
        assert_eq!(npm_upgrade_args(AgentBackend::Deepseek), None);
    }

    #[test]
    pub(super) fn path_install_source_recognizes_official_script_dirs() {
        let home = crate::platform::os::user_home_dir();
        // Codex 官方安装路径平台分叉（Unix ~/.local/bin，Windows
        // %LOCALAPPDATA%\Programs\OpenAI\Codex\bin）：断言走平台函数自身，
        // 各平台语义一致且不引入 cfg 分支（架构守卫）。
        let codex_official = super::super::platform::codex_official_install_path();
        assert_eq!(
            path_install_source(AgentBackend::CodexAcp, &codex_official),
            Some("script")
        );
        assert_eq!(
            path_install_source(AgentBackend::ClaudeAcp, &home.join(".local/bin/claude")),
            Some("script")
        );
        assert_eq!(
            path_install_source(AgentBackend::KimiAcp, &home.join(".kimi-code/bin/kimi")),
            Some("script")
        );
        let legacy_managed = crate::platform::paths::pinvou3_home()
            .join("runtimes")
            .join("codex")
            .join("codex-0.144.6-macos-aarch64")
            .join("codex");
        assert_eq!(
            path_install_source(AgentBackend::CodexAcp, &legacy_managed),
            None
        );
        // 其他路径与其他 Agent 组合一律未知来源。
        assert_eq!(
            path_install_source(AgentBackend::CodexAcp, Path::new("/usr/local/bin/codex")),
            None
        );
        assert_eq!(
            path_install_source(AgentBackend::KimiAcp, &home.join(".local/bin/kimi")),
            None
        );
    }

    #[test]
    pub(super) fn claude_and_kimi_version_gates_enforce_minimums() {
        assert!(claude_version_supported("2.1.163 (Claude Code)"));
        assert!(claude_version_supported("2.0.0 (Claude Code)"));
        assert!(!claude_version_supported("1.9.9 (Claude Code)"));
        assert!(!claude_version_supported("unknown"));
        assert!(kimi_version_supported("0.31.1"));
        assert!(kimi_version_supported("0.9.0"));
        assert!(!kimi_version_supported("0.8.9"));
        // 旧 Python 版 kimi-cli 等非裸 semver 输出一律判不合规。
        assert!(!kimi_version_supported("kimi-cli 0.31.1"));
        assert!(!kimi_version_supported("0.31"));
        assert!(!kimi_version_supported("native"));
    }

    #[test]
    fn running_silent_installer_keeps_output_pipe_open() {
        let idle_timeout = Duration::from_secs(30);

        // Kimi 的大二进制下载期间可能超过 30 秒没有任何 stdout；主进程仍在
        // 运行时绝不能结束读取，否则安装器下一次写日志会收到 SIGPIPE。
        assert!(!install_output_idle_expired(
            false,
            Duration::from_secs(31),
            idle_timeout,
        ));
        // 只有主进程已经退出、且孙进程仍长期持有管道时才启用兜底。
        assert!(!install_output_idle_expired(
            true,
            Duration::from_secs(29),
            idle_timeout,
        ));
        assert!(install_output_idle_expired(
            true,
            Duration::from_secs(30),
            idle_timeout,
        ));
    }
}

pub(super) fn clear_install_progress(app: &AppHandle, backend: AgentBackend) {
    if let Some(store) = app.try_state::<InstallProgressStore>() {
        store.0.write().remove(&backend);
    }
}

pub(super) fn emit_install_progress(
    app: &AppHandle,
    backend: AgentBackend,
    kind: &str,
    value: &str,
) {
    if value.trim().is_empty() {
        return;
    }
    if let Some(store) = app.try_state::<InstallProgressStore>() {
        let mut guard = store.0.write();
        let entry = guard
            .entry(backend)
            .or_insert_with(InstallProgressInfo::default);
        match kind {
            "command" => entry.command = value.to_string(),
            _ => entry.latest_line = Some(value.to_string()),
        }
    }
    let _ = app.emit(
        INSTALL_PROGRESS_EVENT,
        json!({
            "agent": backend.agent_id().unwrap_or("unknown"),
            "kind": kind,
            "value": value,
        }),
    );
}

pub(super) fn fill_install_progress(
    app: &AppHandle,
    backend: AgentBackend,
    status: &mut CodexAcpStatus,
) {
    if let Some(store) = app.try_state::<InstallProgressStore>() {
        let guard = store.0.read();
        if let Some(info) = guard.get(&backend) {
            status.install_command = (!info.command.is_empty()).then(|| info.command.clone());
            status.install_latest_line = info.latest_line.clone();
        }
    }
}

pub(super) fn kill_install_process_tree(pid: u32) {
    if let Err(error) = crate::platform::process::kill_process_tree(pid) {
        eprintln!("[pinvou3-app] failed to kill install process tree {pid}: {error:#}");
    }
}

pub(super) fn move_official_binaries_aside(
    backend: AgentBackend,
) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut moved = Vec::new();
    for original in providers::lifecycle::official_script_paths(backend) {
        if !original.is_file() {
            continue;
        }
        let backup = original.with_extension("pre-upgrade");
        if backup.exists() {
            // 上一次升级中断可能留下备份：先清掉，避免 rename 失败。
            let _ = std::fs::remove_file(&backup);
        }
        std::fs::rename(&original, &backup).with_context(|| {
            format!(
                "升级前备份 {} 失败（文件可能被占用）。请关闭占用 {} 的进程后重试",
                original.display(),
                backend.display_name()
            )
        })?;
        moved.push((original, backup));
    }
    Ok(moved)
}

pub(super) fn official_script_urls(backend: AgentBackend) -> (&'static str, &'static str) {
    match backend {
        AgentBackend::CodexAcp => (CODEX_INSTALL_SCRIPT_UNIX, CODEX_INSTALL_SCRIPT_WINDOWS),
        AgentBackend::ClaudeAcp => (CLAUDE_INSTALL_SCRIPT_UNIX, CLAUDE_INSTALL_SCRIPT_WINDOWS),
        AgentBackend::KimiAcp => (KIMI_INSTALL_SCRIPT_UNIX, KIMI_INSTALL_SCRIPT_WINDOWS),
        AgentBackend::Deepseek => ("", ""),
    }
}

pub(super) async fn script_url_reachable(url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(5))
        .user_agent(concat!("Pinvou-Agent/", env!("CARGO_PKG_VERSION")))
        .build()
    else {
        return false;
    };
    client.head(url).send().await.is_ok()
}

pub(super) fn stale_official_target(target: &Path, resolved_ok: Option<&Path>) -> bool {
    if !target.exists() {
        return false;
    }
    let is_working_file = target
        .metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0);
    !is_working_file || resolved_ok.is_none_or(|path| path != target)
}

/// Shared line → progress-forwarding policy (used by the IO shells of install streaming output): skip empty lines,
/// emit truncated lines throttled at 80ms, and flush the pending tail line when the stream ends. Full-output accumulation and
/// reader-side idle expiry/teardown are each IO shell's own responsibility.
struct InstallLineProgress {
    app: AppHandle,
    backend: AgentBackend,
    kind: &'static str,
    pending: Option<String>,
    last_emit: Instant,
}

impl InstallLineProgress {
    fn new(app: &AppHandle, backend: AgentBackend, kind: &'static str) -> Self {
        Self {
            app: app.clone(),
            backend,
            kind,
            pending: None,
            last_emit: Instant::now(),
        }
    }

    fn push(&mut self, trimmed: &str) {
        self.pending = Some(truncate_install_line(trimmed));
        if self.last_emit.elapsed() >= Duration::from_millis(80) {
            if let Some(pending_line) = self.pending.take() {
                emit_install_progress(&self.app, self.backend, self.kind, &pending_line);
            }
            self.last_emit = Instant::now();
        }
    }

    fn flush(&mut self) {
        if let Some(pending_line) = self.pending.take() {
            emit_install_progress(&self.app, self.backend, self.kind, &pending_line);
        }
    }
}

pub(super) async fn stream_install_lines<R: AsyncRead + Unpin>(
    app: &AppHandle,
    backend: AgentBackend,
    kind: &'static str,
    reader: R,
    child_finished: &AtomicBool,
) -> String {
    const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let mut output = String::new();
    let mut progress = InstallLineProgress::new(app, backend, kind);
    let mut post_exit_idle_started: Option<Instant> = None;
    loop {
        line.clear();
        match tokio::time::timeout(Duration::from_millis(500), reader.read_line(&mut line)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(_)) => {
                let trimmed = line.trim_end().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                post_exit_idle_started = child_finished.load(Ordering::Acquire).then(Instant::now);
                output.push_str(&trimmed);
                output.push('\n');
                progress.push(&trimmed);
            }
            Ok(Err(_)) => break,
            // 主安装进程仍在运行时，静默下载可以远超 30 秒，不能提前关闭管道，
            // 否则安装器后续写 stdout 会收到 SIGPIPE（exit 141）。只有主进程
            // 已退出且孙进程仍持有管道时，才用空闲超时结束读取。
            Err(_) => {
                let child_finished = child_finished.load(Ordering::Acquire);
                if child_finished {
                    let idle_started = post_exit_idle_started.get_or_insert_with(Instant::now);
                    if install_output_idle_expired(
                        child_finished,
                        idle_started.elapsed(),
                        IDLE_TIMEOUT,
                    ) {
                        break;
                    }
                }
            }
        }
    }
    progress.flush();
    output
}

fn install_output_idle_expired(
    child_finished: bool,
    idle: Duration,
    idle_timeout: Duration,
) -> bool {
    child_finished && idle >= idle_timeout
}

pub(super) fn truncate_install_line(line: &str) -> String {
    line.chars().take(500).collect()
}
