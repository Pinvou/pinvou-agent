//! ACP Agent CLI 卸载路径规划。
//!
//! 只负责「按安装来源计算卸载命令/路径」的纯逻辑；执行、会话前置检查与状态
//! 刷新在 `AcpPool::uninstall_agent`（codex_acp/mod.rs）。

use std::path::PathBuf;

use crate::features::codex_acp::store::AgentBackend;

pub struct UninstallPlan {
    /// (brew package, is_cask)。codex/claude-code 是 cask，kimi-code 是 formula。
    pub brew_package: Option<(&'static str, bool)>,
    pub npm_package: Option<&'static str>,
    /// 官方安装脚本写入的目录/二进制（安装来源未知或为 script 时清理这些路径）。
    pub script_paths: Vec<PathBuf>,
}

/// 按安装来源生成卸载计划。`install_source` 为 status 探测结果
/// （"brew" / "npm" / "script" / None）。
pub fn uninstall_plan(backend: AgentBackend, install_source: Option<&str>) -> UninstallPlan {
    let brew_package = match backend {
        AgentBackend::CodexAcp => Some(("codex", true)),
        AgentBackend::ClaudeAcp => Some(("claude-code", true)),
        AgentBackend::KimiAcp => Some(("kimi-code", false)),
        AgentBackend::Deepseek => None,
    };
    let npm_package = match backend {
        AgentBackend::CodexAcp => Some("@openai/codex"),
        AgentBackend::ClaudeAcp => Some("@anthropic-ai/claude-code"),
        AgentBackend::KimiAcp => Some("@moonshot-ai/kimi-code"),
        AgentBackend::Deepseek => None,
    };
    let _ = install_source;
    UninstallPlan {
        brew_package,
        npm_package,
        script_paths: official_script_paths(backend),
    }
}

/// 官方安装脚本的目标路径（与 resolve_*_path 的官方目录一致）。
pub fn official_script_paths(backend: AgentBackend) -> Vec<PathBuf> {
    let home = crate::platform::os::user_home_dir();
    let executable = match std::env::consts::OS {
        "windows" => ".exe",
        _ => "",
    };
    match backend {
        AgentBackend::CodexAcp => {
            vec![super::super::platform::codex_official_install_path()]
        }
        AgentBackend::ClaudeAcp => {
            vec![home
                .join(".local")
                .join("bin")
                .join(format!("claude{executable}"))]
        }
        AgentBackend::KimiAcp => {
            vec![home
                .join(".kimi-code")
                .join("bin")
                .join(format!("kimi{executable}"))]
        }
        AgentBackend::Deepseek => Vec::new(),
    }
}

/// 按安装来源构造卸载命令参数；None 表示该来源不适用（回退 script 路径清理）。
pub fn brew_uninstall_args(plan: &UninstallPlan) -> Option<(String, Vec<String>)> {
    let (package, is_cask) = plan.brew_package?;
    let mut args = vec!["uninstall".to_string()];
    if is_cask {
        args.push("--cask".to_string());
    }
    args.push(package.to_string());
    Some(("brew".to_string(), args))
}

pub fn npm_uninstall_args(plan: &UninstallPlan) -> Option<(String, Vec<String>)> {
    let package = plan.npm_package?;
    // Windows 上 npm 是 npm.cmd：必须用解析后的完整路径（裸名 "npm" 会被当成
    // 原生可执行文件直接 CreateProcess，报 program not found）。
    let npm = crate::features::codex_acp::npm_executable()?;
    Some((
        npm.to_string_lossy().into_owned(),
        vec![
            "uninstall".to_string(),
            "-g".to_string(),
            package.to_string(),
        ],
    ))
}

/// 按来源选择卸载动作：brew / npm / script 路径清理。
pub fn uninstall_command(backend: AgentBackend, install_source: Option<&str>) -> UninstallCommand {
    let plan = uninstall_plan(backend, install_source);
    match install_source {
        Some("brew") => brew_uninstall_args(&plan)
            .map(UninstallCommand::Spawn)
            .unwrap_or_else(|| UninstallCommand::RemovePaths(plan.script_paths)),
        Some("npm") => npm_uninstall_args(&plan)
            .map(UninstallCommand::Spawn)
            .unwrap_or_else(|| UninstallCommand::RemovePaths(plan.script_paths)),
        _ => UninstallCommand::RemovePaths(plan.script_paths),
    }
}

pub enum UninstallCommand {
    Spawn((String, Vec<String>)),
    RemovePaths(Vec<PathBuf>),
}

/// 官方登出命令参数；None 表示该 Agent 的 CLI 不支持非交互登出。
/// kimi 没有 `kimi logout`，但 `kimi provider remove managed:kimi-code`
/// 可非交互移除官方 OAuth provider（已实测：执行后 provider list 为空，
/// 恢复 config 后状态还原），等价于登出。
pub fn logout_args(backend: AgentBackend) -> Option<Vec<String>> {
    let args: &[&str] = match backend {
        AgentBackend::CodexAcp => &["logout"],
        AgentBackend::ClaudeAcp => &["auth", "logout"],
        AgentBackend::KimiAcp => &["provider", "remove", "managed:kimi-code"],
        AgentBackend::Deepseek => return None,
    };
    Some(args.iter().map(|arg| arg.to_string()).collect())
}

/// 用户卸载后残留的配置目录（供 `cleanup` 选项使用）。
pub fn config_paths(backend: AgentBackend) -> Vec<PathBuf> {
    let home = crate::platform::os::user_home_dir();
    match backend {
        AgentBackend::CodexAcp => vec![home.join(".codex")],
        AgentBackend::ClaudeAcp => vec![home.join(".claude")],
        AgentBackend::KimiAcp => vec![home.join(".kimi-code")],
        AgentBackend::Deepseek => Vec::new(),
    }
}
