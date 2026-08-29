//! 微博(`@weibo-ai/weibo-cli`) CLI 连接器 —— 随包 Node/npm 在线安装 + device-code 授权。
//!
//! 连接:`weibo-cli auth login --device --name Pinvou` 长驻 → 抓微博授权 URL 和 user code →
//! 用户浏览器授权 → 进程退出后 `weibo-cli auth whoami --output json` 判 connected。

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::features::connectors::connector_cli::{self as cc, CliCtx, ConnectorConn};
use crate::features::connectors::skill_gate::ConnectorSkillGate;

const ID: &str = "weibo";
const WEIBO_NPM_SPEC: &str = "@weibo-ai/weibo-cli@0.9.1";
const WEIBO_MIN_VERSION: (u64, u64, u64) = (0, 9, 1);
const SHORT_CMD_TIMEOUT_SECS: u64 = 8;

const WEIBO_CTX: CliCtx = CliCtx {
    cli_bin: "weibo-cli",
    envs: &[],
    auth_domains: &["open.weibo.com", "open-dev.weibo.com"],
};

#[derive(Debug)]
enum AuthEvent {
    Url(String),
    UserCode(String),
    Line(String),
}

fn weibo(args: &[&str]) -> std::process::Command {
    let mut c = WEIBO_CTX.cli(args);
    remove_weibo_token_envs(&mut c);
    c
}

fn remove_weibo_token_envs(c: &mut std::process::Command) {
    for key in [
        "WEIBO_CLI_TOKEN",
        "WEIBO_TOKEN",
        "WEIBO_CLI_REFRESH_TOKEN",
        "WEIBO_REFRESH_TOKEN",
    ] {
        c.env_remove(key);
    }
}

fn run_capture_timeout(
    mut cmd: std::process::Command,
    secs: u64,
) -> Result<(bool, String, String), String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动失败: {e}(需要先完成微博 CLI 的在线安装)"))?;
    let start = Instant::now();
    loop {
        match child.try_wait().map_err(|e| format!("wait: {e}"))? {
            Some(_) => {
                let out = child
                    .wait_with_output()
                    .map_err(|e| format!("收集微博 CLI 输出失败: {e}"))?;
                return Ok((
                    out.status.success(),
                    String::from_utf8_lossy(&out.stdout).into_owned(),
                    String::from_utf8_lossy(&out.stderr).into_owned(),
                ));
            }
            None if start.elapsed() > Duration::from_secs(secs) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("微博 CLI 命令超时({secs}s)"));
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn parse_weibo_version(s: &str) -> Option<(u64, u64, u64)> {
    let start = s
        .char_indices()
        .find(|(_, c)| c.is_ascii_digit() || *c == 'v' || *c == 'V')?
        .0;
    let version = s[start..].trim_start_matches(['v', 'V']);
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

fn weibo_cli_version() -> Option<(u64, u64, u64)> {
    let Ok((ok, so, se)) = run_capture_timeout(weibo(&["--version"]), SHORT_CMD_TIMEOUT_SECS)
    else {
        return None;
    };
    if !ok {
        return None;
    }
    parse_weibo_version(&so).or_else(|| parse_weibo_version(&se))
}

fn weibo_cli_present() -> bool {
    weibo_cli_version()
        .map(|v| version_at_least(v, WEIBO_MIN_VERSION))
        .unwrap_or(false)
}

fn whoami_is_logged_in(stdout: &str, stderr: &str) -> bool {
    let text = format!("{stdout}\n{stderr}");
    if text.contains("缺少登录令牌") || text.to_ascii_lowercase().contains("missing token") {
        return false;
    }
    if let Some(v) = cc::parse_json(&text) {
        if v.get("authenticated").and_then(|v| v.as_bool()) == Some(true) {
            return true;
        }
        if v.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            return true;
        }
        if v.get("account").is_some()
            || v.get("user").is_some()
            || v.get("screen_name").is_some()
            || v.get("id").is_some()
        {
            return true;
        }
    }
    false
}

fn is_logged_in() -> bool {
    if !weibo_cli_present() {
        return false;
    }
    match run_capture_timeout(
        weibo(&["auth", "whoami", "--output", "json"]),
        SHORT_CMD_TIMEOUT_SECS,
    ) {
        Ok((ok, so, se)) => ok && whoami_is_logged_in(&so, &se),
        Err(_) => false,
    }
}

fn wait_logged_in(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if is_logged_in() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

fn install_weibo_cli() -> Result<bool, String> {
    let mut c = WEIBO_CTX.base_cmd("npm");
    remove_weibo_token_envs(&mut c);
    cc::apply_user_npm_prefix(&mut c);
    c.args(["install", "-g", WEIBO_NPM_SPEC]);
    cc::run_with_timeout(c, 180)
}

/// 引导:确保 weibo-cli 装好且版本不低于 0.9.1。
pub async fn weibo_ensure_cli() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        if weibo_cli_present() {
            return Ok::<Value, String>(json!({ "ok": true, "already": true }));
        }
        if !install_weibo_cli()? {
            return Err("微博 CLI 安装失败，请查看 ~/.pinvou3/cli-install.log".to_string());
        }
        if !weibo_cli_present() {
            return Err("微博 CLI 安装完成但无法执行，请重试或修复应用运行时".to_string());
        }
        Ok::<Value, String>(json!({ "ok": true, "already": false }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 查询当前微博连接状态。只返回布尔，不把身份 / token 信息带进 webview。
pub async fn weibo_status() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        if weibo_cli_version().is_none() {
            return Ok::<Value, String>(json!({
                "ok": false, "connected": false, "installed": false
            }));
        }
        let supported = weibo_cli_present();
        if !supported {
            return Ok::<Value, String>(json!({
                "ok": false, "connected": false, "installed": true, "upgrade_required": true
            }));
        }
        let connected = is_logged_in();
        Ok::<Value, String>(json!({
            "ok": connected,
            "connected": connected,
            "installed": true,
            "upgrade_required": false
        }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 开始连接微博。立即返回 `{started:true}`，前端 listen 事件驱动 UI。
pub async fn weibo_connect_begin(app: AppHandle) -> Result<Value, String> {
    let conn = app.state::<ConnectorConn>();
    if let Some(pid) = conn.cancel(ID) {
        let _ = tokio::task::spawn_blocking(move || cc::kill_pid_tree(pid)).await;
    }
    conn.reset(ID);
    let already_logged_in = tokio::task::spawn_blocking(is_logged_in)
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))?;
    if already_logged_in {
        cc::bundle_store_on_connected(ID);
        cc::emit(
            &app,
            "weibo:connected",
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
            "weibo:error",
            json!({ "phase": "authorize", "message": e }),
        );
    }
}

fn drain_for_auth_event<R: std::io::Read + Send + 'static>(
    r: R,
    tx: mpsc::Sender<AuthEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in std::io::BufRead::lines(std::io::BufReader::new(r)) {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    log::warn!("[weibo] 授权输出读取失败，停止排空：{error}");
                    break;
                }
            };
            let line = strip_ansi_control_sequences(&line);
            if let Some(url) = WEIBO_CTX.extract_url(&line) {
                let _ = tx.send(AuthEvent::Url(url));
            }
            if let Some(code) = extract_user_code(&line) {
                let _ = tx.send(AuthEvent::UserCode(code));
            }
            if let Some(safe) = safe_auth_log_line(&line) {
                let _ = tx.send(AuthEvent::Line(safe));
            }
        }
    })
}

fn phase_scan(app: &AppHandle) -> Result<(), String> {
    let mut cmd = weibo(&["auth", "login", "--device", "--name", "Pinvou"]);
    crate::platform::process::std_process_group_leader(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("weibo-cli auth login 启动失败: {e}(需要微博 CLI)"))?;
    let conn = app.state::<ConnectorConn>();
    conn.set_pid(ID, Some(child.id()));

    let (tx, rx) = mpsc::channel::<AuthEvent>();
    if let Some(o) = child.stdout.take() {
        drain_for_auth_event(o, tx.clone());
    }
    if let Some(e) = child.stderr.take() {
        drain_for_auth_event(e, tx.clone());
    }
    drop(tx);

    let mut auth_lines = VecDeque::with_capacity(32);
    let mut user_code: Option<String> = None;
    let deadline = Instant::now() + Duration::from_secs(60);
    let url = loop {
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            conn.set_pid(ID, None);
            return Err(auth_failure_message(
                &auth_lines,
                "60s 内未拿到微博授权链接(检查网络 / 代理)",
            ));
        }
        match rx.recv_timeout(std::cmp::min(
            Duration::from_millis(400),
            deadline.saturating_duration_since(now),
        )) {
            Ok(AuthEvent::Url(u)) => {
                break match &user_code {
                    Some(code) => with_user_code_param(&u, code),
                    None => u,
                };
            }
            Ok(AuthEvent::UserCode(code)) => {
                user_code = Some(code);
            }
            Ok(AuthEvent::Line(line)) => remember_auth_line(&mut auth_lines, line),
            Err(_) => {
                if let Ok(Some(status)) = child.try_wait() {
                    conn.set_pid(ID, None);
                    eprintln!("[weibo] auth login exited before auth url: exit={status}");
                    if wait_logged_in(Duration::from_secs(5)) {
                        cc::bundle_store_on_connected(ID);
                        cc::emit(
                            app,
                            "weibo:connected",
                            json!({ "ok": true, "already": true }),
                        );
                        return Ok(());
                    }
                    return Err(auth_failure_message(
                        &auth_lines,
                        "微博授权进程提前退出，未拿到授权链接",
                    ));
                }
            }
        }
    };

    if user_code.is_none() {
        user_code = user_code_from_url(&url);
    }
    cc::emit(
        app,
        "weibo:qr",
        json!({ "phase": "authorize", "url": url, "user_code": user_code, "qr_data_url": cc::make_qr(&url) }),
    );

    loop {
        if conn.is_cancelled(ID) {
            let _ = child.kill();
            conn.set_pid(ID, None);
            return Ok(());
        }
        while let Ok(event) = rx.try_recv() {
            if let AuthEvent::Line(line) = event {
                remember_auth_line(&mut auth_lines, line);
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                conn.set_pid(ID, None);
                if wait_logged_in(Duration::from_secs(5)) {
                    cc::bundle_store_on_connected(ID);
                    cc::emit(app, "weibo:connected", json!({ "ok": true }));
                    return Ok(());
                }
                eprintln!("[weibo] auth login exited without logged-in status: exit={status}");
                return Err(auth_failure_message(
                    &auth_lines,
                    "微博授权未完成(可能已取消或超时)",
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(400)),
            Err(e) => {
                conn.set_pid(ID, None);
                return Err(format!("auth login 等待失败: {e}"));
            }
        }
    }
}

fn with_user_code_param(url: &str, user_code: &str) -> String {
    if url.contains("user_code=") {
        return url.to_string();
    }
    let Some(user_code) = normalize_user_code(user_code) else {
        return url.to_string();
    };
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}user_code={user_code}")
}

fn user_code_from_url(url: &str) -> Option<String> {
    let (_, code) = url.split_once("user_code=")?;
    code.split('&').next().and_then(normalize_user_code)
}

fn extract_user_code(line: &str) -> Option<String> {
    let line = strip_ansi_control_sequences(line);
    let lower = line.to_ascii_lowercase();
    let has_code_label = lower.contains("user_code")
        || lower.contains("user code")
        || lower.contains("device code")
        || lower.contains("code:")
        || line.contains("用户码")
        || line.contains("验证码")
        || line.contains("授权码");
    if !has_code_label {
        return None;
    }
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .rfind(|s| {
            let len = s.len();
            (4..=32).contains(&len)
                && s.chars()
                    .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && s.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
        })
        .and_then(normalize_user_code)
}

fn normalize_user_code(raw: &str) -> Option<String> {
    let code: String = strip_ansi_control_sequences(raw)
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if (4..=32).contains(&code.len()) {
        Some(code.to_ascii_uppercase())
    } else {
        None
    }
}

fn strip_ansi_control_sequences(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && chars.peek().copied() == Some('\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
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

fn remember_auth_line(auth_lines: &mut VecDeque<String>, line: String) {
    if auth_lines.len() >= 32 {
        auth_lines.pop_front();
    }
    auth_lines.push_back(line);
}

fn auth_failure_message(auth_lines: &VecDeque<String>, fallback: &str) -> String {
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

pub async fn weibo_cancel(app: AppHandle) -> Result<Value, String> {
    let pid = app.state::<ConnectorConn>().cancel(ID);
    if let Some(pid) = pid {
        let _ = tokio::task::spawn_blocking(move || cc::kill_pid_tree(pid)).await;
    }
    Ok(json!({ "ok": true }))
}

/// 断开微博:`weibo-cli auth logout`。未安装时也视为已断开。
pub async fn weibo_logout() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        if weibo_cli_version().is_none() {
            cc::bundle_store_on_disconnected(ID);
            return Ok::<Value, String>(json!({ "ok": true, "installed": false }));
        }
        let (ok, _, _) = run_capture_timeout(weibo(&["auth", "logout"]), SHORT_CMD_TIMEOUT_SECS)?;
        if !ok {
            return Err("微博 CLI 退出登录失败，请重试".to_string());
        }
        cc::bundle_store_on_disconnected(ID);
        Ok::<Value, String>(json!({ "ok": true, "installed": true }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ─────────────────────── 微博 skill 门控 ────────────────────────

struct WeiboGate;
impl ConnectorSkillGate for WeiboGate {
    fn id(&self) -> &'static str {
        ID
    }
    fn display_name(&self) -> &'static str {
        "微博"
    }
    fn disabled_filename(&self) -> &'static str {
        "weibo_disabled"
    }
    fn apply_skills(&self, visible: bool) -> Result<(), String> {
        crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
            .apply_weibo_skills(visible)
            .map_err(|e| format!("更新微博技能失败: {e}"))
    }
}
const GATE: WeiboGate = WeiboGate;

pub fn is_weibo_disabled() -> bool {
    GATE.is_disabled()
}

fn set_weibo_disabled_flag(disabled: bool) -> Result<(), String> {
    GATE.set_disabled_flag(disabled)
}

pub fn weibo_skills_should_show() -> bool {
    !is_weibo_disabled() && is_logged_in()
}

pub async fn weibo_apply_skills() -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(|| -> Result<bool, String> {
        let show = weibo_skills_should_show();
        GATE.apply_skills(show)?;
        Ok(show)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))??;
    if show {
        crate::features::marketplace::sync_deny_all_scopes_after_install("weibo");
    }
    Ok(json!({ "visible": show }))
}

pub async fn set_weibo_enabled(enabled: bool) -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        set_weibo_disabled_flag(!enabled)?;
        crate::features::marketplace::sync_disabled_bundles_for_connector_switch("weibo", enabled);
        let show = weibo_skills_should_show();
        GATE.apply_skills(show)?;
        Ok(show)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))??;
    Ok(json!({ "ok": true, "visible": show }))
}

pub async fn weibo_skills_state() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let disabled = is_weibo_disabled();
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
    fn weibo_commands_strip_token_envs_keep_unrelated() {
        // env 消毒契约:remove_weibo_token_envs 必须剥离全部 4 个 WEIBO 令牌环境变量
        // (防用户环境里的 token 静默绕过 device-code 授权与技能门控),
        // 同时不得波及无关变量(env_clear 式过度实现会让 CLI 丢 PATH/HOME)。
        let mut c = WEIBO_CTX.cli(&["--version"]);
        for k in [
            "WEIBO_CLI_TOKEN",
            "WEIBO_TOKEN",
            "WEIBO_CLI_REFRESH_TOKEN",
            "WEIBO_REFRESH_TOKEN",
        ] {
            c.env(k, "leak-me");
        }
        c.env("PINVOU_UNRELATED_ENV", "keep-me");
        remove_weibo_token_envs(&mut c);
        let envs: std::collections::HashMap<_, _> = c.get_envs().collect();
        for k in [
            "WEIBO_CLI_TOKEN",
            "WEIBO_TOKEN",
            "WEIBO_CLI_REFRESH_TOKEN",
            "WEIBO_REFRESH_TOKEN",
        ] {
            // get_envs 的值是 Option:None = 已被 env_remove 挂起删除
            let stripped = envs
                .get(std::ffi::OsStr::new(k))
                .is_none_or(|v| v.is_none());
            assert!(stripped, "{k} must be stripped");
        }
        assert_eq!(
            envs.get(std::ffi::OsStr::new("PINVOU_UNRELATED_ENV"))
                .and_then(|v| *v)
                .map(|v| v.to_string_lossy()),
            Some("keep-me".into())
        );
    }

    #[test]
    fn parses_weibo_versions() {
        assert_eq!(parse_weibo_version("0.9.1"), Some((0, 9, 1)));
        assert_eq!(
            parse_weibo_version("weibo-cli version v1.2"),
            Some((1, 2, 0))
        );
        assert_eq!(parse_weibo_version("hello"), None);
    }

    #[test]
    fn version_comparison_uses_semver_order() {
        assert!(version_at_least((0, 9, 1), WEIBO_MIN_VERSION));
        assert!(version_at_least((1, 0, 0), WEIBO_MIN_VERSION));
        assert!(!version_at_least((0, 9, 0), WEIBO_MIN_VERSION));
    }

    #[test]
    fn parses_and_appends_user_code() {
        assert_eq!(
            extract_user_code("User code: AB12-CD34").as_deref(),
            Some("AB12-CD34")
        );
        assert_eq!(
            extract_user_code("User code: \u{1b}[1m1B66-F7C7\u{1b}[0m").as_deref(),
            Some("1B66-F7C7")
        );
        assert_eq!(
            with_user_code_param("https://open.weibo.com/cli/device", "ABCD"),
            "https://open.weibo.com/cli/device?user_code=ABCD"
        );
        assert_eq!(
            with_user_code_param(
                "https://open.weibo.com/cli/device",
                "\u{1b}[1m1B66-F7C7\u{1b}[0m"
            ),
            "https://open.weibo.com/cli/device?user_code=1B66-F7C7"
        );
        assert_eq!(
            user_code_from_url("https://open.weibo.com/cli/device?user_code=ABCD&x=1").as_deref(),
            Some("ABCD")
        );
        assert_eq!(
            user_code_from_url(
                "https://open.weibo.com/cli/device?user_code=1B66-F7C7\u{1b}[0m&x=1"
            )
            .as_deref(),
            Some("1B66-F7C7")
        );
    }

    #[test]
    fn whoami_detects_login_without_identity_leak() {
        assert!(whoami_is_logged_in(r#"{"ok":true,"screen_name":"u"}"#, ""));
        assert!(whoami_is_logged_in(r#"{"user":{"id":"1"}}"#, ""));
        assert!(!whoami_is_logged_in(
            "",
            "缺少登录令牌。请运行 `weibo-cli auth login`"
        ));
    }

    #[test]
    fn safe_auth_log_line_redacts_tokens() {
        assert_eq!(
            safe_auth_log_line("WEIBO_CLI_TOKEN=secret").as_deref(),
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
    fn auth_failure_message_keeps_cli_reason() {
        let mut lines = VecDeque::new();
        lines.push_back("starting auth".to_string());
        lines.push_back("Error: network timeout".to_string());
        assert_eq!(
            auth_failure_message(&lines, "微博授权进程提前退出，未拿到授权链接"),
            "微博授权进程提前退出，未拿到授权链接：Error: network timeout"
        );
    }

    #[test]
    fn weibo_disabled_flag_roundtrip() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = format!(
            "{}/pinvou3-weibo-test-{}",
            std::env::temp_dir().display(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::create_dir_all(crate::platform::paths::pinvou3_home());

        set_weibo_disabled_flag(false).unwrap();
        assert!(!is_weibo_disabled());
        set_weibo_disabled_flag(true).unwrap();
        assert!(is_weibo_disabled());
        set_weibo_disabled_flag(false).unwrap();
        assert!(!is_weibo_disabled());

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn status_probe_timeout_is_treated_as_not_connected() {
        // 状态探测契约:命令超时 → Err,调用方(is_logged_in/weibo_status)按未连接处理,
        // 不把超时错误抛给工具商店或首屏门控刷新。裸 ping 不带次数限制在三个平台
        // 都必然超过 1s 超时(macOS/Linux 无限,Windows 默认 4 次 ≈3s),无需平台分支。
        let mut cmd = std::process::Command::new("ping");
        cmd.arg("127.0.0.1");
        let result = run_capture_timeout(cmd, 1);
        assert!(result.is_err(), "hung probe must time out, got {result:?}");
    }
}
