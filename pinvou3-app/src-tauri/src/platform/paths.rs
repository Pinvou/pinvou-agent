//! `~/.pinvou3/` 目录布局解析。
//!
//! pinvou3-app 不读 `~/.deepseek/`（隔离），所有 deepseek-tui 默认会写到
//! 全局/cwd 的字段都映射到这个独立目录树。布局参见 plan「目录布局」一节。
//!
//! `PINVOU3_HOME` 环境变量可整体重定位（主要用于测试）。

use std::path::PathBuf;
use std::sync::OnceLock;

static RUNTIME_RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 用户家目录 `$HOME`，是 pinvou3-app 的 engine workspace 根。
/// AI 通过相对路径访问 → 落在家目录下；通过绝对路径访问 → trust_mode 放行
/// 但敏感子目录由 path filter / instructions 引导拦截。
pub fn user_home_dir() -> PathBuf {
    crate::platform::os::user_home_dir()
}

/// `~/.pinvou3/` 根目录。
pub fn pinvou3_home() -> PathBuf {
    if let Ok(custom) = std::env::var("PINVOU3_HOME") {
        return crate::platform::os::platform_compat_path(&custom);
    }
    user_home_dir().join(".pinvou3")
}

pub fn settings_path() -> PathBuf {
    pinvou3_home().join("settings.json")
}

/// `~/.pinvou3/logs/memory-review.log` —— 对话记忆复盘诊断日志。
/// 只记录阶段、分类和计数，不记录对话原文或记忆正文。
pub fn memory_review_log() -> PathBuf {
    pinvou3_home().join("logs").join("memory-review.log")
}

pub fn bundle_root() -> PathBuf {
    pinvou3_home().join("bundle")
}
pub fn bundle_instructions() -> PathBuf {
    bundle_root().join("instructions.md")
}
pub fn bundle_skills_dir() -> PathBuf {
    bundle_root().join("skills")
}
pub fn bundle_mcp_json() -> PathBuf {
    bundle_root().join("mcp.json")
}
/// `~/.pinvou3/bundle/mcp-servers/` —— pinvou3 内置 MCP server 脚本目录。
pub fn bundle_mcp_servers_dir() -> PathBuf {
    bundle_root().join("mcp-servers")
}
pub fn managed_connectors_dir() -> PathBuf {
    pinvou3_home().join("connectors")
}
pub fn managed_connector_bin_dir() -> Option<PathBuf> {
    managed_connector_bin_dir_for(std::env::consts::OS, std::env::consts::ARCH)
}
pub fn managed_connector_bin_dir_for(os: &str, arch: &str) -> Option<PathBuf> {
    connector_platform_dir(os, arch)
        .map(|platform| managed_connectors_dir().join(platform).join("bin"))
}

/// 支持按需下载安装连接器 CLI 的平台目录名。目录名同时用于选择编译期锁文件，
/// 以及运行时落盘到 `~/.pinvou3/connectors/<platform>/bin/`。
pub fn connector_platform_dir(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "aarch64") => Some("linux-arm64"),
        ("linux", "x86_64") => Some("linux-x64"),
        ("macos", "aarch64") => Some("darwin-arm64"),
        ("macos", "x86_64") => Some("darwin-x64"),
        ("windows", "x86_64") => Some("windows-x64"),
        _ => None,
    }
}

/// Tauri 的真实 resource_dir。由 setup 在任何前端命令可执行前写入一次；
/// Linux/macOS 的连接器 npm 命令用它定位随包 Node 与 npm CLI。
pub fn set_runtime_resource_dir(path: PathBuf) {
    let _ = RUNTIME_RESOURCE_DIR.set(path);
}

pub fn runtime_resource_dir() -> Option<PathBuf> {
    RUNTIME_RESOURCE_DIR
        .get()
        .cloned()
        .or_else(|| std::env::var_os("PINVOU3_RESOURCE_DIR").map(PathBuf::from))
}

pub fn bundled_connector_node() -> Option<PathBuf> {
    let (path, _) = bundled_connector_runtime_paths_for(
        &runtime_resource_dir()?,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )?;
    path.is_file().then_some(path)
}

pub fn bundled_connector_npm_cli() -> Option<PathBuf> {
    let (_, path) = bundled_connector_runtime_paths_for(
        &runtime_resource_dir()?,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )?;
    path.is_file().then_some(path)
}

fn bundled_connector_runtime_paths_for(
    resource_dir: &std::path::Path,
    os: &str,
    arch: &str,
) -> Option<(PathBuf, PathBuf)> {
    let root = resource_dir.join("runtime/codex-bridge/node");
    let node = match (os, arch) {
        ("linux", "aarch64" | "x86_64") => root.join("bin/node"),
        ("macos", "aarch64") => root.join("darwin-arm64/bin/node"),
        ("macos", "x86_64") => root.join("darwin-x64/bin/node"),
        _ => return None,
    };
    let npm = root.join("lib/node_modules/npm/bin/npm-cli.js");
    Some((node, npm))
}
/// present_artifact MCP server 脚本绝对路径(mcp.json 的 args 指向它)。
pub fn bundle_present_artifact_server() -> PathBuf {
    bundle_mcp_servers_dir().join("present_artifact_server.py")
}
pub fn bundle_version_file() -> PathBuf {
    bundle_root().join("VERSION")
}

// --- 浏览器功能（features/browser）路径 ---

/// `~/.pinvou3/browser/` —— 专用有头 Chrome 的状态目录（端口文件/profile/启动锁）。
pub fn browser_home() -> PathBuf {
    pinvou3_home().join("browser")
}
/// CDP 端口协调文件：{ port, pid, owner: "app"|"mcp", started_at }。
/// Rust BrowserManager 与 MCP wrapper（browser-wrapper.mjs）通过它幂等协调同一实例。
pub fn browser_cdp_port_json() -> PathBuf {
    browser_home().join("cdp-port.json")
}
/// 专用 Chrome 的独立 user-data-dir（与用户日常浏览器隔离）。
pub fn browser_profile_dir() -> PathBuf {
    browser_home().join("profile")
}
/// 浏览器启动独占锁（node 的 `openSync(lock, 'wx')` 与 Rust `create_new` 同语义）。
pub fn browser_start_lock() -> PathBuf {
    browser_home().join("start.lock")
}
/// 浏览器最近一次动态启动失败记录：{ reason, at }，由 browser-wrapper.mjs 在
/// Chrome 缺失/启动失败/CDP 未就绪退出前写入；Rust 侧（browser_unavailability_reason）
/// 读取后注入模型可见的 instructions（24h 新鲜度）。
pub fn browser_last_error_json() -> PathBuf {
    browser_home().join("last-error.json")
}
/// 工作模式会话专用 mcp.json（全局 mcp.json + browser 条目）。
/// 门控语义：browser MCP 工具只对工作模式（assistant 引擎）会话暴露，全局
/// mcp.json 永不注册 browser 条目；codex ACP 等外部 Agent 不读本文件。
pub fn browser_work_mcp_json() -> PathBuf {
    browser_home().join("mcp.work.json")
}
/// 浏览器 MCP wrapper 脚本（编译期内嵌，释放到 `~/.pinvou3/bundle/mcp-servers/`）。
pub fn bundle_browser_wrapper() -> PathBuf {
    bundle_mcp_servers_dir().join("browser-wrapper.mjs")
}
/// vendor 的 chrome-devtools-mcp 入口（随安装包 resource_dir 分发）。
pub fn bundled_chrome_devtools_mcp_bin() -> Option<PathBuf> {
    let res = runtime_resource_dir()?;
    let bin = res.join("runtime/chrome-devtools-mcp/build/src/bin/chrome-devtools-mcp.js");
    bin.is_file().then_some(bin)
}

/// 拉起 python MCP server(present_artifact / pptx 等)用的解释器命令。
///
/// - **Windows**:优先用安装器写入的 `PINVOU3_PYTHON`,其次用随安装包内置的
///   `python/pythonw.exe`(无控制台窗口,不依赖用户机器上的 python),再回退真实系统
///   Python。Microsoft Store `WindowsApps\python.exe` 占位符会被跳过。
/// - **其他平台**(Linux/macOS):用系统 `python3`(Linux 几乎自带;GUI 子进程不弹窗;
///   依赖由 marketplace 的自动 pip 安装)。
pub fn python_command() -> String {
    crate::platform::os::python_command()
}

pub fn user_root() -> PathBuf {
    pinvou3_home().join("user")
}
pub fn user_instructions() -> PathBuf {
    user_root().join("instructions.md")
}
pub fn user_skills_dir() -> PathBuf {
    user_root().join("skills")
}
/// `~/.pinvou3/user/personas/` —— 用户自创专家卡牌（卡牌池）。每张卡一个
/// `<id>.json`（PersonaCard 序列化）。跟 bundle 内嵌的内置卡分离，**永不被覆写**。
pub fn user_personas_dir() -> PathBuf {
    user_root().join("personas")
}

/// `~/.deepseek/skills/` — CodeWhale 标准用户 skills 目录；通过
/// `/skill install` 安装的 skill 都在这里。它与 Pinvou 私有的
/// [`user_skills_dir`] 平行；会话物化时用户 Skill 覆盖同名 bundle Skill。
pub fn deepseek_skills_dir() -> PathBuf {
    user_home_dir().join(".deepseek").join("skills")
}

/// 兼容字段：阶段 B 旧 sandbox workspace（已不作为 engine workspace 使用，
/// 但保留作为 "AI 私人沙盒" 兜底——某些场景如 monitor 测试还在用）。
pub fn workspace_dir() -> PathBuf {
    pinvou3_home().join("workspace")
}
pub fn notes_path() -> PathBuf {
    pinvou3_home().join("notes.md")
}
pub fn memory_path() -> PathBuf {
    pinvou3_home().join("memory.md")
}
pub fn user_memory_dir() -> PathBuf {
    user_root().join("memory")
}
pub fn user_memory_profile() -> PathBuf {
    user_memory_dir().join("profile.json")
}
pub fn user_memory_preferences_dir() -> PathBuf {
    user_memory_dir().join("preferences")
}
pub fn user_memory_work_context_dir() -> PathBuf {
    user_memory_dir().join("work_context")
}
pub fn user_memory_pending() -> PathBuf {
    user_memory_dir().join("_pending.jsonl")
}
pub fn user_memory_never() -> PathBuf {
    user_memory_dir().join("_never.jsonl")
}
pub fn user_memory_recent_work() -> PathBuf {
    user_memory_dir().join("recent_work.jsonl")
}
pub fn user_memory_current_focus() -> PathBuf {
    user_memory_dir().join("current_focus.jsonl")
}
pub fn user_memory_recent_activity() -> PathBuf {
    user_memory_dir().join("recent_activity.jsonl")
}
pub fn user_memory_runtime_dir() -> PathBuf {
    user_memory_dir().join("runtime")
}
pub fn user_memory_snapshot() -> PathBuf {
    user_memory_dir().join("snapshot.md")
}
pub fn user_memory_runtime_prompt(session_id: &str) -> PathBuf {
    user_memory_runtime_dir().join(format!("{}.md", sanitize_memory_runtime_id(session_id)))
}
pub fn mcp_config_path() -> PathBuf {
    bundle_mcp_json()
}

/// `~/.pinvou3/sessions/` —— 所有对话历史落盘的根目录。
pub fn sessions_root() -> PathBuf {
    pinvou3_home().join("sessions")
}

/// Scheduled-run data is separated from ordinary chat history.
pub fn scheduled_runs_root() -> PathBuf {
    pinvou3_home().join("scheduled-runs")
}

/// `~/.pinvou3/scheduled/` —— 定时任务工作间根目录。
/// 每个 automation 在 `<automation_id>/workspace/` 下拥有独立工作间；
/// 该任务的多次运行对话共享它，不同 automation 之间互不共享。
pub fn scheduled_tasks_root() -> PathBuf {
    pinvou3_home().join("scheduled")
}

pub fn scheduled_task_workspace_dir(automation_id: &str) -> PathBuf {
    scheduled_tasks_root().join(automation_id).join("workspace")
}

/// App-owned classification and immutable runtime settings for scheduled sessions.
pub fn scheduled_run_profiles_path() -> PathBuf {
    scheduled_runs_root().join("session-profiles.json")
}

/// App-owned viewed state for independent scheduled-run conversations.
pub fn scheduled_run_read_state_path() -> PathBuf {
    scheduled_runs_root().join("read-state.json")
}

fn sanitize_memory_runtime_id(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

/// `~/.pinvou3/updates/` —— 应用内升级下载的 deb 暂存目录。
/// 不用 /tmp：tmpfs 受内存限制 + 重启清空（下载完提示重启后文件就没了）。
pub fn updates_dir() -> PathBuf {
    pinvou3_home().join("updates")
}

/// `~/.pinvou3/feedback/` —— 用户主动提交的反馈包、失败待重试内容和提交回执。
pub fn feedback_root() -> PathBuf {
    pinvou3_home().join("feedback")
}

/// `~/.pinvou3/feedback/pending/` —— 上传失败或正在准备的反馈包目录。
pub fn feedback_pending_dir() -> PathBuf {
    feedback_root().join("pending")
}

/// `~/.pinvou3/feedback/receipts/` —— 成功提交后保留的轻量回执。
pub fn feedback_receipts_dir() -> PathBuf {
    feedback_root().join("receipts")
}

/// `~/.pinvou3/updates/update-feedback.json` —— Windows OTA 安装器启动后
/// 跨进程保留的待反馈记录。Linux .deb 更新不使用此文件。
pub fn update_feedback_record_path() -> PathBuf {
    updates_dir().join("update-feedback.json")
}

/// `~/.pinvou3/sessions/<session_id>/artifacts/` —— AI 默认产物落地目录。
/// `$PINVOU3_SESSION_ARTIFACTS` 环境变量注入这个值给 engine + LLM。
pub fn session_artifacts_dir(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("artifacts")
}

/// `~/.pinvou3/sessions/<session_id>/workspace/` —— 每个 session 独立的工作目录。
/// engine workspace 跟随当前 active session 切换，避免多 session 共享文件冲突。
/// 切换 session 时 bridge 调 `Op::SyncSession { workspace }` 重置。
pub fn session_workspace_dir(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("workspace")
}

/// `~/.pinvou3/sessions/<session_id>/skills/` —— 该会话按 scope 物化的技能组合目录
/// （skill 双 scope 治理：见 `features/assistant/skill_materialization.rs`）。
/// 会话私有目录随会话删除一起清理，无需单独清理逻辑。
pub fn session_skills_dir(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("skills")
}

/// `~/.pinvou3/sessions/<session_id>/instructions.md` —— 每个 session 独立的
/// Legacy `~/.pinvou3/sessions/<sid>/instructions.md` 路径。
///
/// C 方案(P-no-disk)前用作 per-session prompt 文件,EngineConfig.instructions
/// 指向它。改成 `InstructionSource::Inline` 后这个 disk 文件**不再被生产代码读**,
/// 仅用于 boot 时 legacy 清理(早期 pinvou3 版本写下的残留)。新版 pinvou3 不再写。
pub fn session_instructions_path(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("instructions.md")
}

/// `~/.pinvou3/sessions/<session_id>/persona_events.json` —— 该 session 的卡牌
/// 加持/卸下事件时间线(sidecar)。**刻意独立于 messages**:messages 在 engine
/// 冷启动时会被 sync_session 注水回 LLM,而卡牌事件是纯前端展示,绝不能进 LLM 上下文。
/// 前端按 `pos`(事件发生时的 messages 数)在 rerenderFromMessages 里插回原位。
pub fn session_persona_events(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("persona_events.json")
}

/// `~/.pinvou3/sessions/<session_id>/pinvou_reviews.json` —— 该 session 的 Pinvou
/// 召唤检阅时间线（每条 {pos, review}）。同 persona_events 一样**刻意独立于 messages**:
/// 审查卡是纯前端展示、绝不能进 LLM 上下文（那会污染主 AI），前端按 `pos` 在
/// rerenderFromMessages 里插回。Boss 要主 AI 看审阅,走「转交」按钮发成 Boss 消息。
pub fn session_pinvou_reviews(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("pinvou_reviews.json")
}

/// `~/.pinvou3/sessions/<session_id>/pinvou_scene_events.json` —— 用户消息的
/// 专业场景展示标签（每条 `{pos, scene}`）。与 persona/review sidecar 一样独立于
/// messages，避免把纯 UI 元数据注入 LLM 上下文，同时允许桌面端与 WebUI 共享恢复。
pub fn session_pinvou_scene_events(session_id: &str) -> PathBuf {
    sessions_root()
        .join(session_id)
        .join("pinvou_scene_events.json")
}

/// `~/.pinvou3/sessions/<session_id>/timing_events.jsonl` —— 每轮对话端到端耗时
/// 事件(sidecar)。刻意独立于 messages/session schema, 避免影响上下文和产物逻辑。
pub fn session_timing_events(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("timing_events.jsonl")
}

/// `~/.pinvou3/sessions/default/artifacts/` —— PPT / 公文等 MCP stdio server
/// 的公共产物落点。stdio server 不能可靠感知当前 GUI session，具体归属由
/// 带 `session_id` 的工具事件归档到具体会话。
pub fn default_session_artifacts_dir() -> PathBuf {
    session_artifacts_dir("default")
}

/// 首次启动确保所有目录存在。bundle/skills 等子目录在解包时还会再 ensure 一次。
pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(bundle_skills_dir())?;
    std::fs::create_dir_all(user_skills_dir())?;
    std::fs::create_dir_all(user_personas_dir())?;
    std::fs::create_dir_all(workspace_dir())?;
    std::fs::create_dir_all(scheduled_tasks_root())?;
    std::fs::create_dir_all(default_session_artifacts_dir())?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 进程级 env var 是测试的硬隔离障碍:cargo test 默认并行跑,多个测试
    /// 同时改 PINVOU3_HOME 会互相覆盖断言。这是 **crate 级唯一的 env 锁源**:
    /// bridge/mod.rs(EnvGuard,DEEPSEEK_*)、feedback、notifications 等模块
    /// 所有 mutate env var 的测试都借用这把锁串行执行,使 env 写测试彼此串行。
    ///
    /// 注意:锁源单一只让 **持锁的 env 写测试** 之间互斥,**不代表** 可以撤掉
    /// `--test-threads=1`——未持锁的 env 读取者仍可能观察到其他测试的临时值。
    /// Mutex poison 后通过 `PoisonError::into_inner()` 取得的仍是已加锁 guard,
    /// 不会绕过互斥。CI 暂时串行执行；根治需消除测试对进程级 env 的依赖,
    /// 或让所有读写都通过同一隔离层。
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 生成**进程内**唯一的单调递增后缀,供测试临时目录/会话 ID 命名用。
    ///
    /// 纯纳秒时间戳在高并发下会碰撞(热缓存 + 线程调度抖动让两次调用落同纳秒),
    /// 导致临时目录/会话 ID 重复 → 测试互相覆盖文件。叠加这个原子计数器后,
    /// 同一**进程内**每次调用都唯一,消除并行测试的命名碰撞。
    ///
    /// 注意:计数器是进程级的——两个并发的 `cargo test` **进程**(如本地双终端)各自从 0
    /// 起计数,若临时目录名只含本后缀(不含 pid)仍会跨进程碰撞。因此临时目录命名应同时
    /// 叠加 `std::process::id()`(见 `scheduled/tasks.rs::temp_home` 的做法);仅靠本后缀
    /// 只保证单进程内唯一。
    pub(crate) fn unique_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn pinvou3_home_respects_env_override() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-test-override");
        assert_eq!(
            pinvou3_home(),
            crate::platform::os::platform_compat_path("/tmp/pinvou3-test-override")
        );
        assert_eq!(
            settings_path(),
            crate::platform::os::platform_compat_path("/tmp/pinvou3-test-override")
                .join("settings.json")
        );
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    #[test]
    fn scheduled_paths_are_derived_from_pinvou_home() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-scheduled-paths");

        assert_eq!(scheduled_runs_root(), pinvou3_home().join("scheduled-runs"));
        assert_eq!(scheduled_tasks_root(), pinvou3_home().join("scheduled"));
        assert_eq!(
            scheduled_task_workspace_dir("automation-1"),
            pinvou3_home()
                .join("scheduled")
                .join("automation-1")
                .join("workspace")
        );
        assert_eq!(
            scheduled_run_profiles_path(),
            pinvou3_home()
                .join("scheduled-runs")
                .join("session-profiles.json")
        );
        assert_eq!(
            scheduled_run_read_state_path(),
            pinvou3_home()
                .join("scheduled-runs")
                .join("read-state.json")
        );
        if let Some(value) = previous {
            std::env::set_var("PINVOU3_HOME", value);
        } else {
            std::env::remove_var("PINVOU3_HOME");
        }
    }

    /// `user_home_dir` 应该读 $HOME（pinvou3 engine workspace 之根）。
    #[test]
    fn user_home_dir_reads_home_env() {
        assert!(!user_home_dir().as_os_str().is_empty());
    }

    #[test]
    fn connector_bin_dir_covers_all_on_demand_platforms() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-connector-path-test");
        let root = crate::platform::os::platform_compat_path("/tmp/pinvou3-connector-path-test");
        let expected = |platform: &str| Some(root.join("connectors").join(platform).join("bin"));
        assert_eq!(
            managed_connector_bin_dir_for("linux", "aarch64"),
            expected("linux-arm64")
        );
        assert_eq!(
            managed_connector_bin_dir_for("linux", "x86_64"),
            expected("linux-x64")
        );
        assert_eq!(
            managed_connector_bin_dir_for("macos", "aarch64"),
            expected("darwin-arm64")
        );
        assert_eq!(
            managed_connector_bin_dir_for("macos", "x86_64"),
            expected("darwin-x64")
        );
        assert_eq!(
            managed_connector_bin_dir_for("windows", "x86_64"),
            expected("windows-x64")
        );
        assert_eq!(managed_connector_bin_dir_for("windows", "aarch64"), None);
        assert_eq!(managed_connector_bin_dir_for("freebsd", "x86_64"), None);
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    #[test]
    fn connector_runtime_paths_cover_packaged_linux_and_macos_layouts() {
        let resource = PathBuf::from("/opt/pinvou/resources");
        let npm = resource.join("runtime/codex-bridge/node/lib/node_modules/npm/bin/npm-cli.js");
        assert_eq!(
            bundled_connector_runtime_paths_for(&resource, "linux", "x86_64"),
            Some((
                resource.join("runtime/codex-bridge/node/bin/node"),
                npm.clone()
            ))
        );
        assert_eq!(
            bundled_connector_runtime_paths_for(&resource, "macos", "aarch64"),
            Some((
                resource.join("runtime/codex-bridge/node/darwin-arm64/bin/node"),
                npm.clone()
            ))
        );
        assert_eq!(
            bundled_connector_runtime_paths_for(&resource, "macos", "x86_64"),
            Some((
                resource.join("runtime/codex-bridge/node/darwin-x64/bin/node"),
                npm
            ))
        );
        assert_eq!(
            bundled_connector_runtime_paths_for(&resource, "windows", "x86_64"),
            None
        );
    }

    /// session artifacts 路径必须落在 ~/.pinvou3/sessions/<id>/artifacts/ 下。
    #[test]
    fn session_artifacts_layout() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-artifacts-layout-test");
        let root = crate::platform::os::platform_compat_path("/tmp/pinvou3-artifacts-layout-test");
        assert_eq!(
            session_artifacts_dir("abc123"),
            root.join("sessions").join("abc123").join("artifacts")
        );
        assert_eq!(
            default_session_artifacts_dir(),
            root.join("sessions").join("default").join("artifacts")
        );
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }
}
