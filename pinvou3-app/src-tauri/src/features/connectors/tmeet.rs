//! 腾讯会议(`@tencentcloud/tmeet`) CLI 连接器 —— 随包 Node/npm 在线安装 + OAuth 授权。
//!
//! 路线同钉钉 / 企微:官方 CLI + 官方 skill,不要求用户填写 API Key。
//! 连接:`tmeet auth login --no-browser` 长驻 → 抓腾讯会议授权 URL → 用户扫码 / 浏览器授权 →
//! 进程退出后 `tmeet auth status` 包含 `Logged in` 判 connected。

use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::features::connectors::connector_cli::{self as cc, CliCtx, ConnectorConn};
use crate::features::connectors::skill_gate::ConnectorSkillGate;

const ID: &str = "tmeet";
const TMEET_NPM_SPEC: &str = "@tencentcloud/tmeet@1.0.15";
const TMEET_MIN_VERSION: (u64, u64, u64) = (1, 0, 15);

const TMEET_CTX: CliCtx = CliCtx {
    cli_bin: "tmeet",
    envs: &[("TMEET_AGENT", "Pinvou"), ("TMEET_MODEL", "Pinvou")],
    auth_domains: &["meeting.tencent.com"],
};

fn tmeet(args: &[&str]) -> std::process::Command {
    TMEET_CTX.cli(args)
}

fn parse_tmeet_version(s: &str) -> Option<(u64, u64, u64)> {
    let lower = s.to_ascii_lowercase();
    let marker = lower
        .find("version")
        .map(|i| i + "version".len())
        .unwrap_or(0);
    let tail = &s[marker..];
    let start = tail
        .char_indices()
        .find(|(_, c)| c.is_ascii_digit() || *c == 'v')?
        .0;
    let version = tail[start..].trim_start_matches(['v', 'V']);
    let mut nums = version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .take(3)
        .filter_map(|p| p.parse::<u64>().ok());
    Some((
        nums.next()?,
        nums.next().unwrap_or(0),
        nums.next().unwrap_or(0),
    ))
}

fn version_at_least(v: (u64, u64, u64), min: (u64, u64, u64)) -> bool {
    v >= min
}

fn tmeet_cli_version() -> Option<(u64, u64, u64)> {
    let Ok((ok, so, se)) = cc::run(tmeet(&["--version"])) else {
        return None;
    };
    if !ok {
        return None;
    }
    parse_tmeet_version(&so).or_else(|| parse_tmeet_version(&se))
}

fn tmeet_cli_present() -> bool {
    tmeet_cli_version()
        .map(|v| version_at_least(v, TMEET_MIN_VERSION))
        .unwrap_or(false)
}

fn status_is_logged_in(s: &str) -> bool {
    s.contains("Logged in")
}

fn is_logged_in() -> bool {
    if !tmeet_cli_present() {
        return false;
    }
    if let Ok((_, so, se)) = cc::run(tmeet(&["auth", "status"])) {
        return status_is_logged_in(&so) || status_is_logged_in(&se);
    }
    false
}

fn wait_logged_in(timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if is_logged_in() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

fn auth_output_says_already_logged_in(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("user has been login")
        || lower.contains("user has been logged in")
        || lower.contains("already logged in")
}

fn auth_lines_say_already_logged_in(auth_lines: &std::collections::VecDeque<String>) -> bool {
    auth_lines
        .iter()
        .any(|line| auth_output_says_already_logged_in(line))
}

fn safe_auth_log_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("access_token")
        || lower.contains("refresh_token")
        || lower.contains("authorization:")
        || lower.contains("bearer ")
        || lower.contains("token")
    {
        return Some("[redacted credential line]".to_string());
    }
    Some(trimmed.chars().take(320).collect())
}

fn install_tmeet_cli() -> Result<bool, String> {
    let mut c = TMEET_CTX.base_cmd("npm");
    cc::apply_user_npm_prefix(&mut c);
    c.args(["install", "-g", TMEET_NPM_SPEC]);
    cc::run_with_timeout(c, 180)
}

/// 引导:确保 tmeet 装好且版本不低于 1.0.15。
pub async fn tmeet_ensure_cli() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        if tmeet_cli_present() {
            return Ok::<Value, String>(json!({ "ok": true, "already": true }));
        }
        if !install_tmeet_cli()? {
            return Err("腾讯会议 CLI 安装失败，请查看 ~/.pinvou3/cli-install.log".to_string());
        }
        if !tmeet_cli_present() {
            return Err("腾讯会议 CLI 安装完成但无法执行，请重试或修复应用运行时".to_string());
        }
        Ok::<Value, String>(json!({ "ok": true, "already": false }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 查询当前腾讯会议连接状态。只返回布尔,不把身份 / token 信息带进 webview。
pub async fn tmeet_status() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        if tmeet_cli_version().is_none() {
            return Ok::<Value, String>(json!({
                "ok": false, "connected": false, "installed": false
            }));
        }
        let supported = tmeet_cli_present();
        if !supported {
            return Ok::<Value, String>(json!({
                "ok": false, "connected": false, "installed": true, "upgrade_required": true
            }));
        }
        let (ok, so, se) = cc::run(tmeet(&["auth", "status"]))?;
        let connected = status_is_logged_in(&so) || status_is_logged_in(&se);
        Ok::<Value, String>(json!({
            "ok": ok,
            "connected": connected,
            "installed": true,
            "upgrade_required": false
        }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 开始连接腾讯会议(单段 OAuth)。立即返回 `{started:true}`,前端 listen 事件驱动 UI。
pub async fn tmeet_connect_begin(app: AppHandle) -> Result<Value, String> {
    let conn = app.state::<ConnectorConn>();
    if let Some(pid) = conn.cancel(ID) {
        let _ = tokio::task::spawn_blocking(move || cc::kill_pid_tree(pid)).await;
    }
    conn.reset(ID);
    let already_logged_in = tokio::task::spawn_blocking(is_logged_in)
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))?;
    if already_logged_in {
        cc::emit(
            &app,
            "tmeet:connected",
            json!({ "ok": true, "already": true }),
        );
        return Ok(json!({ "started": true, "already_connected": true }));
    }
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || run_connect_flow(&app2));
    Ok(json!({ "started": true }))
}

fn run_connect_flow(app: &AppHandle) {
    if let Err(e) = phase_scan(app) {
        cc::emit(
            app,
            "tmeet:error",
            json!({ "phase": "authorize", "message": e }),
        );
    }
}

fn drain_for_auth_url<R: std::io::Read + Send + 'static>(
    r: R,
    tx: mpsc::Sender<(Option<String>, Option<String>)>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in std::io::BufRead::lines(std::io::BufReader::new(r)) {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    // 管道读取错误通常不可恢复；继续迭代可能反复返回 Err 并空转。
                    log::warn!("[tmeet] 授权输出读取失败，停止排空：{error}");
                    break;
                }
            };
            let safe = safe_auth_log_line(&line);
            let url = TMEET_CTX.extract_url(&line);
            let _ = tx.send((url, safe));
        }
    })
}

fn phase_scan(app: &AppHandle) -> Result<(), String> {
    let mut cmd = tmeet(&["auth", "login", "--no-browser"]);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("tmeet auth login 启动失败: {e}(需要 tmeet CLI)"))?;
    let conn = app.state::<ConnectorConn>();
    conn.set_pid(ID, Some(child.id()));

    let (tx, rx) = mpsc::channel::<(Option<String>, Option<String>)>();
    if let Some(o) = child.stdout.take() {
        drain_for_auth_url(o, tx.clone());
    }
    if let Some(e) = child.stderr.take() {
        drain_for_auth_url(e, tx.clone());
    }
    drop(tx);

    let mut auth_lines = std::collections::VecDeque::with_capacity(32);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let url = loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            let _ = child.kill();
            conn.set_pid(ID, None);
            return Err(auth_failure_message(
                &auth_lines,
                "60s 内未拿到腾讯会议授权链接(检查网络 / 代理)",
            ));
        }
        match rx.recv_timeout(std::cmp::min(
            Duration::from_millis(400),
            deadline.saturating_duration_since(now),
        )) {
            Ok((Some(u), line)) => {
                remember_auth_line(&mut auth_lines, line);
                break u;
            }
            Ok((None, line)) => remember_auth_line(&mut auth_lines, line),
            Err(_) => {
                if let Ok(Some(status)) = child.try_wait() {
                    conn.set_pid(ID, None);
                    eprintln!("[tmeet] auth login exited before auth url: exit={status}");
                    if auth_lines_say_already_logged_in(&auth_lines)
                        && wait_logged_in(Duration::from_secs(5))
                    {
                        cc::emit(
                            app,
                            "tmeet:connected",
                            json!({ "ok": true, "already": true }),
                        );
                        return Ok(());
                    }
                    return Err(auth_failure_message(
                        &auth_lines,
                        "腾讯会议授权进程提前退出，未拿到授权链接",
                    ));
                }
            }
        }
    };

    cc::emit(
        app,
        "tmeet:qr",
        json!({ "phase": "authorize", "url": url, "qr_data_url": cc::make_qr(&url) }),
    );

    loop {
        if conn.is_cancelled(ID) {
            let _ = child.kill();
            conn.set_pid(ID, None);
            return Ok(());
        }
        while let Ok((_, line)) = rx.try_recv() {
            remember_auth_line(&mut auth_lines, line);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                conn.set_pid(ID, None);
                if wait_logged_in(Duration::from_secs(5)) {
                    cc::emit(app, "tmeet:connected", json!({ "ok": true }));
                    return Ok(());
                }
                if auth_lines_say_already_logged_in(&auth_lines)
                    && wait_logged_in(Duration::from_secs(5))
                {
                    cc::emit(
                        app,
                        "tmeet:connected",
                        json!({ "ok": true, "already": true }),
                    );
                    return Ok(());
                }
                eprintln!("[tmeet] auth login exited without logged-in status: exit={status}");
                let last_line = auth_lines
                    .iter()
                    .rev()
                    .find(|line| {
                        let l = line.to_ascii_lowercase();
                        l.contains("failed") || l.contains("error") || line.contains("失败")
                    })
                    .cloned()
                    .unwrap_or_default();
                if last_line.is_empty() {
                    return Err("腾讯会议授权未完成(可能已取消或超时)".into());
                }
                return Err(format!("腾讯会议授权失败：{last_line}"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(400)),
            Err(e) => {
                conn.set_pid(ID, None);
                return Err(format!("auth login 等待失败: {e}"));
            }
        }
    }
}

fn remember_auth_line(auth_lines: &mut std::collections::VecDeque<String>, line: Option<String>) {
    if let Some(line) = line {
        if auth_lines.len() >= 32 {
            auth_lines.pop_front();
        }
        auth_lines.push_back(line);
    }
}

fn auth_failure_message(auth_lines: &std::collections::VecDeque<String>, fallback: &str) -> String {
    let last_line = auth_lines
        .iter()
        .rev()
        .find(|line| {
            let l = line.to_ascii_lowercase();
            l.contains("failed")
                || l.contains("error")
                || l.contains("timeout")
                || l.contains("lock")
                || line.contains("失败")
        })
        .cloned()
        .or_else(|| auth_lines.back().cloned())
        .unwrap_or_default();
    if last_line.is_empty() {
        fallback.to_string()
    } else {
        format!("{fallback}：{last_line}")
    }
}

pub async fn tmeet_cancel(app: AppHandle) -> Result<Value, String> {
    let pid = app.state::<ConnectorConn>().cancel(ID);
    if let Some(pid) = pid {
        let _ = tokio::task::spawn_blocking(move || cc::kill_pid_tree(pid)).await;
    }
    Ok(json!({ "ok": true }))
}

/// 断开腾讯会议:`tmeet auth logout`。未安装时也视为已断开。
pub async fn tmeet_logout() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        if tmeet_cli_version().is_none() {
            return Ok::<Value, String>(json!({ "ok": true, "installed": false }));
        }
        let (ok, _, _) = cc::run(tmeet(&["auth", "logout"]))?;
        if !ok {
            return Err("腾讯会议 CLI 退出登录失败，请重试".to_string());
        }
        Ok::<Value, String>(json!({ "ok": true, "installed": true }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ─────────────────────── 腾讯会议 skill 门控 ────────────────────────

/// 腾讯会议技能门控:停用标志文件机制走 [`ConnectorSkillGate`] 默认实现,
/// `apply_skills` 指向 `apply_tmeet_skills`。
struct TmeetGate;
impl ConnectorSkillGate for TmeetGate {
    fn id(&self) -> &'static str {
        ID
    }
    fn display_name(&self) -> &'static str {
        "腾讯会议"
    }
    fn disabled_filename(&self) -> &'static str {
        "tmeet_disabled"
    }
    fn apply_skills(&self, visible: bool) -> Result<(), String> {
        crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
            .apply_tmeet_skills(visible)
            .map_err(|e| format!("更新腾讯会议技能失败: {e}"))
    }
}
const GATE: TmeetGate = TmeetGate;

pub fn is_tmeet_disabled() -> bool {
    GATE.is_disabled()
}

fn set_tmeet_disabled_flag(disabled: bool) -> Result<(), String> {
    GATE.set_disabled_flag(disabled)
}

pub fn tmeet_skills_should_show() -> bool {
    !is_tmeet_disabled() && is_logged_in()
}

pub async fn tmeet_apply_skills() -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(|| -> Result<bool, String> {
        let show = tmeet_skills_should_show();
        GATE.apply_skills(show)?;
        Ok(show)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))??;
    Ok(json!({ "visible": show }))
}

pub async fn set_tmeet_enabled(enabled: bool) -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        set_tmeet_disabled_flag(!enabled)?;
        let show = tmeet_skills_should_show();
        GATE.apply_skills(show)?;
        Ok(show)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))??;
    Ok(json!({ "ok": true, "visible": show }))
}

pub async fn tmeet_skills_state() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let disabled = is_tmeet_disabled();
        let connected = is_logged_in();
        Ok::<Value, String>(json!({
            "connected": connected,
            "enabled": !disabled,
            "visible": connected && !disabled,
        }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::paths::tests::ENV_LOCK;

    #[test]
    fn status_detects_logged_in() {
        assert!(status_is_logged_in("Logged in as user@example.com"));
        assert!(!status_is_logged_in(
            "Not logged in. Please use 'tmeet auth login' to authenticate."
        ));
        assert!(!status_is_logged_in(""));
    }

    #[test]
    fn parses_tmeet_versions() {
        assert_eq!(
            parse_tmeet_version("tmeet version v1.0.15"),
            Some((1, 0, 15))
        );
        assert_eq!(parse_tmeet_version("v2.3.4"), Some((2, 3, 4)));
        assert_eq!(parse_tmeet_version("tmeet version 1.2"), Some((1, 2, 0)));
        assert_eq!(parse_tmeet_version("hello"), None);
    }

    #[test]
    fn version_comparison_uses_semver_order() {
        assert!(version_at_least((1, 0, 15), TMEET_MIN_VERSION));
        assert!(version_at_least((1, 1, 0), TMEET_MIN_VERSION));
        assert!(!version_at_least((1, 0, 14), TMEET_MIN_VERSION));
        assert!(!version_at_least((0, 9, 99), TMEET_MIN_VERSION));
    }

    #[test]
    fn auth_output_detects_already_logged_in() {
        assert!(auth_output_says_already_logged_in(
            "Error: user has been login, please use 'tmeet cmd [flags]' to use"
        ));
        assert!(auth_output_says_already_logged_in(
            "Error: user has been logged in"
        ));
        assert!(auth_output_says_already_logged_in("already logged in"));
        assert!(!auth_output_says_already_logged_in("network timeout"));
    }

    #[test]
    fn auth_failure_message_keeps_cli_reason() {
        let mut lines = std::collections::VecDeque::new();
        lines.push_back("starting auth".to_string());
        lines.push_back("Error: file lock timeout (5s)".to_string());
        assert_eq!(
            auth_failure_message(&lines, "腾讯会议授权进程提前退出，未拿到授权链接"),
            "腾讯会议授权进程提前退出，未拿到授权链接：Error: file lock timeout (5s)"
        );
    }

    #[test]
    fn safe_auth_log_line_redacts_tokens() {
        assert_eq!(
            safe_auth_log_line("access_token=secret").as_deref(),
            Some("[redacted credential line]")
        );
        assert_eq!(
            safe_auth_log_line("Authorization: Bearer secret").as_deref(),
            Some("[redacted credential line]")
        );
        assert_eq!(safe_auth_log_line("  hello  ").as_deref(), Some("hello"));
        assert_eq!(safe_auth_log_line("   "), None);
    }

    #[test]
    fn tmeet_disabled_flag_roundtrip() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = format!(
            "{}/pinvou3-tmeet-test-{}",
            std::env::temp_dir().display(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::create_dir_all(crate::platform::paths::pinvou3_home());

        set_tmeet_disabled_flag(false).unwrap();
        assert!(!is_tmeet_disabled());
        set_tmeet_disabled_flag(true).unwrap();
        assert!(is_tmeet_disabled());
        set_tmeet_disabled_flag(false).unwrap();
        assert!(!is_tmeet_disabled());

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
