use std::path::Path;
use std::process::Command;

const SUDOERS_PATH: &str = "/etc/sudoers.d/pinvou3";

pub fn super_permission_is_enabled() -> bool {
    Path::new(SUDOERS_PATH).exists()
}

pub fn enable_super_permission() -> Result<(), String> {
    let user = std::env::var("USER").map_err(|_| "USER 环境变量未设置".to_string())?;
    validate_username(&user)?;
    let script = format!(
        "set -e; printf '%s ALL=(ALL) NOPASSWD: ALL\\n' '{user}' > {SUDOERS_PATH}; chmod 0440 {SUDOERS_PATH}"
    );
    run_pkexec(&["bash", "-c", &script])
}

pub fn disable_super_permission() -> Result<(), String> {
    if !super_permission_is_enabled() {
        return Ok(());
    }
    run_pkexec(&["rm", "-f", SUDOERS_PATH])
}

pub fn super_permission_turn_reminder() -> &'static str {
    if super_permission_is_enabled() {
        "超级权限【已开启】(sudo 免密)。需要 root 时**直接用 sudo 一步到位,绝不先试不带 sudo 的命令再回头补**:写系统路径用 `sudo touch`/`sudo tee`/`sudo mkdir -p`/`sudo rm`,装包/服务用 `sudo apt install`/`sudo systemctl`。仍不要碰密钥/凭证(`~/.ssh`、含 `credentials`/`id_rsa`/`token` 的路径、`/etc/shadow`、`/etc/sudoers`),开 root 也禁。"
    } else {
        "超级权限【已关闭】。**禁止用 sudo**(会被立即拒绝（execpolicy Deny）,别试 `sudo xxx` 也别试 `echo '' | sudo -S xxx`)。需要 root(写 `/etc`、`apt`、`systemctl` 等)时:告诉用户去【设置 → 系统权限】打开开关后重试,或把命令贴给用户自己跑;优先找免 root 替代(`--user`、`~/.local`)。"
    }
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
        return Err(format!("USER 值非法: {user:#}"));
    }
    if !user
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!("USER 含非法字符: {user:#}"));
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
