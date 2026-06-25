//! 超级权限开关：通过 pkexec 写入 `/etc/sudoers.d/pinvou3` 让当前用户跑 sudo 免密。
//!
//! 设计要点：
//! - **源真相是文件系统**：`/etc/sudoers.d/pinvou3` 存在 = 开；不存在 = 关。不在 settings.json 里冗余存。
//! - **写/删都走 pkexec**：用户拨开关 → 系统密码框 → root 写文件，零终端命令。
//! - **不动 careful hook**：DeepSeek-TUI shell.rs 的 5 条硬拦（`rm -rf /` 等）跨所有模式默认开启，
//!   sudo 也过不去。本开关只是把「sudo 卡密码」变成「sudo 直接跑」。
//!
//! pkexec 退出码约定：
//! - 0  成功
//! - 126 用户取消授权（dismissed dialog）
//! - 127 未授权或 pkexec 不可用
//!
//! 维护：卸载 .deb 时由 `prerm` 删 `/etc/sudoers.d/pinvou3`，避免遗留授权。

use std::path::Path;
use std::process::Command;

const SUDOERS_PATH: &str = "/etc/sudoers.d/pinvou3";

pub fn is_enabled() -> bool {
    Path::new(SUDOERS_PATH).exists()
}

/// 静态 system prompt 的 §7 占位段。**不含开关状态**。
///
/// 状态是动态的:静态 prompt 在 engine spawn 时只渲染一次,之后切开关无法热刷
/// (`refresh_all_instructions` 是 no-op —— 见 `engine_pool.rs`)。把状态写进
/// 静态段会导致"UI 显示已开启,但会话 prompt 还停在关闭态"的 desync(实测 case)。
/// 真正的开/关指令由 [`turn_reminder`] 每 turn 注入(`build_send_message_op`),
/// 始终实时。这里只留一句指引,把状态判断交给 per-turn reminder。
pub fn instruction_block() -> &'static str {
    "\n## 7. 超级权限(sudo)\n\n当前是否开启、以及对应该怎么做,见每轮对话顶部的 `<system-reminder>`(实时,以那里为准)。"
}

/// 每 turn 注入 `<system-reminder>` 的超级权限状态指令。
///
/// `is_enabled()` 每次实时读 `/etc/sudoers.d/pinvou3`,所以用户切换开关后**下一
/// turn 即生效**,不需要重启会话或 GUI —— 这是绕开 `refresh_all_instructions`
/// no-op 的关键(静态 prompt 刷不动,就每 turn 重新注入)。
///
/// **开启**:直接 sudo 一步到位,别先试裸命令(否则模型对 `/etc` 写只会裸 touch 然后放弃)。
/// **关闭**:禁 sudo(会被 deny hook 拦或卡超时),引导用户开开关。
pub fn turn_reminder() -> &'static str {
    if is_enabled() {
        "超级权限【已开启】(sudo 免密),需要 root 时直接用 `sudo` 一步到位,别先试不带 sudo 再回头补。仍禁碰密钥/凭证(`~/.ssh`、`credentials`、`id_rsa`、`token`、`/etc/shadow`、`/etc/sudoers`)。"
    } else {
        "超级权限【已关闭】,**别用 sudo**(会被拦/卡超时,白费一轮)。需要 root 时引导用户去【设置 → 系统权限】开开关,或优先找免 root 替代(`--user`/`~/.local`)。"
    }
}

pub fn enable() -> Result<(), String> {
    let user = std::env::var("USER").map_err(|_| "USER 环境变量未设置".to_string())?;
    validate_username(&user)?;
    // sudoers 文件内容固定一行 + chmod 0440（visudo 标准权限）
    let script = format!(
        "set -e; printf '%s ALL=(ALL) NOPASSWD: ALL\\n' '{user}' > {SUDOERS_PATH}; chmod 0440 {SUDOERS_PATH}"
    );
    run_pkexec(&["bash", "-c", &script])
}

pub fn disable() -> Result<(), String> {
    if !is_enabled() {
        return Ok(());
    }
    run_pkexec(&["rm", "-f", SUDOERS_PATH])
}

fn run_pkexec(args: &[&str]) -> Result<(), String> {
    let status = Command::new("pkexec")
        .args(args)
        .status()
        .map_err(|e| format!("pkexec 启动失败: {e}"))?;
    if status.success() {
        return Ok(());
    }
    let code = status.code().unwrap_or(-1);
    Err(match code {
        126 => "用户取消授权".to_string(),
        127 => "未授权或 pkexec 不可用".to_string(),
        _ => format!("pkexec 失败 (exit {code})"),
    })
}

fn validate_username(user: &str) -> Result<(), String> {
    if user.is_empty() || user.len() > 32 {
        return Err(format!("USER 值非法: {user:?}"));
    }
    if !user
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!("USER 含非法字符: {user:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_username_accepts_typical() {
        assert!(validate_username("hexin").is_ok());
        assert!(validate_username("user-1_test").is_ok());
    }

    #[test]
    fn validate_username_rejects_injection() {
        assert!(validate_username("foo;rm -rf /").is_err());
        assert!(validate_username("foo'\"bar").is_err());
        assert!(validate_username("").is_err());
        assert!(validate_username(&"a".repeat(33)).is_err());
    }
}
