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

/// 注入 system prompt 的引导段。
///
/// **开启**：告诉 LLM 可以直接 sudo（避免反复跟用户确认权限）。
/// **关闭**：显式负面引导，告诉 LLM sudo 会卡 120s 超时，不要盲试 ——
/// 否则 LLM 按训练数据默认就用 `sudo apt remove` 卡死,白白等超时(实测 case)。
pub fn instruction_block() -> &'static str {
    if is_enabled() {
        "\n## 7. 超级权限已开启\n\n用户已授权 sudo 免密。装包/配环境/改系统服务时直接用 sudo（如 `sudo apt install`、`sudo systemctl enable`），不要让用户去手动输密码。**仍不要碰** §5 禁令的敏感目录。"
    } else {
        "\n## 7. 超级权限当前关闭\n\n**禁止用 sudo**（会卡 120s 等密码后超时被杀,白白浪费一轮)。需要 root 操作(apt install/remove、systemctl、改 /etc 等)时:\n1. 优先尝试免 root 替代(`pip install --user`、`~/.local/bin` 等)\n2. 真需要 root → 告诉用户「请去【设置 → 系统权限】打开开关后重试」,或把命令贴出来让用户自己跑\n3. 不要盲试 `sudo xxx` 或 `echo '' | sudo -S xxx` —— 两种都会超时"
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
