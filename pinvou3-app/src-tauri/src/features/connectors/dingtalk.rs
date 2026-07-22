//! 钉钉(`dws`,钉钉官方 Apache-2.0)CLI 连接器 —— 安装引导 + 扫码鉴权。
//!
//! 路线同企微([`crate::features::connectors::wecom`]):官方 CLI + 官方 mono skill,纯扫码接入,
//! 不要求用户填写 client_id/client_secret。公共管道见 [`crate::features::connectors::connector_cli`]。
//!
//! 连接:`dws auth login --device` 长驻 → 抓二维码 URL → 用户扫码 → 进程退出后
//! `dws auth status --format json` 判 ready。进度走事件
//! `dingtalk:qr` / `dingtalk:connected` / `dingtalk:error`。

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::features::connectors::connector_cli::{self as cc, CliCtx, ConnectorConn};

const ID: &str = "dingtalk";
const DWS_NPM_SPEC: &str = "dingtalk-workspace-cli@1.0.51";

const DINGTALK_CTX: CliCtx = CliCtx {
    cli_bin: "dws",
    envs: &[],
    auth_domains: &[
        "dingtalk.com",
        "login.dingtalk.com",
        "open.dingtalk.com",
        "oauth.dingtalk.com",
    ],
};

fn dws(args: &[&str]) -> std::process::Command {
    DINGTALK_CTX.cli(args)
}

fn dws_cli_present() -> bool {
    matches!(cc::run(dws(&["--version"])), Ok((true, _, _)))
}

/// `dws auth status --format json` 的已登录判定。
/// 只认官方 JSON 中 `authenticated: true`,避免从身份字段/提示文本误判。
fn auth_is_authenticated_str(s: &str) -> bool {
    cc::parse_json(s)
        .and_then(|v| v.get("authenticated").and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

#[derive(Debug)]
enum AuthEvent {
    Url(String),
    UserCode(String),
    Line(String),
}

fn with_user_code_param(url: &str, user_code: &str) -> String {
    if url.contains("user_code=") {
        return url.to_string();
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}user_code={user_code}")
}

fn extract_user_code(line: &str) -> Option<String> {
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
        .filter(|s| {
            let len = s.len();
            (6..=32).contains(&len)
                && s.chars()
                    .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && s.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
        })
        .last()
        .map(|s| s.to_string())
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
    {
        return Some("[redacted credential line]".to_string());
    }
    Some(trimmed.chars().take(320).collect())
}

fn dingtalk_auth_error_hint(text: &str) -> Option<String> {
    let parsed = cc::parse_json(text);
    let msg = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|v| v.get("message"))
        .and_then(|v| v.as_str())
        .unwrap_or(text);
    if msg.contains("CLI data access is not enabled")
        || text.contains("CLI data access is not enabled")
    {
        let admin = text
            .lines()
            .find_map(|line| line.split_once("组织主管理员").map(|(_, rest)| rest))
            .map(|s| s.trim_matches(|c: char| c == ':' || c == '：' || c.is_whitespace()))
            .filter(|s| !s.is_empty())
            .map(|s| format!("组织主管理员：{s}。"))
            .unwrap_or_default();
        return Some(format!(
            "钉钉组织未开启 CLI 数据访问。{admin}请联系组织主管理员在钉钉开放平台开发者设置中开启“Allow members to access their personal data via CLI”，然后重新登录。"
        ));
    }
    parsed
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| format!("钉钉 CLI 授权失败：{s}"))
        })
        .or_else(|| {
            text.lines()
                .rev()
                .find(|line| {
                    let l = line.to_ascii_lowercase();
                    l.contains("failed") || l.contains("error") || line.contains("失败")
                })
                .map(|line| format!("钉钉 CLI 授权失败：{}", line.trim()))
        })
}

fn drain_for_auth_event<R: std::io::Read + Send + 'static>(
    r: R,
    tx: mpsc::Sender<AuthEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in std::io::BufRead::lines(std::io::BufReader::new(r)).flatten() {
            if let Some(safe_line) = safe_auth_log_line(&line) {
                let _ = tx.send(AuthEvent::Line(safe_line));
            }
            if let Some(code) = extract_user_code(&line) {
                let _ = tx.send(AuthEvent::UserCode(code));
            }
            if let Some(url) = DINGTALK_CTX.extract_url(&line) {
                let _ = tx.send(AuthEvent::Url(url));
            }
        }
    })
}

fn is_authenticated() -> bool {
    if !dws_cli_present() {
        return false;
    }
    if let Ok((_, so, se)) = cc::run(dws(&["auth", "status", "--format", "json"])) {
        return auth_is_authenticated_str(&so) || auth_is_authenticated_str(&se);
    }
    false
}

fn auth_status_message() -> String {
    match cc::run(dws(&["auth", "status", "--format", "json"])) {
        Ok((ok, so, se)) => {
            let p = cc::parse_json(&so).or_else(|| cc::parse_json(&se));
            let msg = p
                .as_ref()
                .and_then(|v| v.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let authenticated = p
                .as_ref()
                .and_then(|v| v.get("authenticated"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            format!("ok={ok}, authenticated={authenticated}, message={msg}")
        }
        Err(e) => format!("status failed: {e}"),
    }
}

// ───────────────────────────── Tauri commands ─────────────────────────────

/// 官方 npm 包的 postinstall 除了解压 CLI，还会把 dws skill 写进
/// `~/.agents/.codex/.claude/...`。安装时给它一个隔离 HOME，避免绕过 Pinvou3 的
/// “已连接且未停用才展示 skill”门控；CLI 本体仍安装到 npm 的用户级全局 prefix。
fn isolated_npm_install_command() -> Result<(std::process::Command, std::path::PathBuf), String> {
    let install_home = crate::platform::paths::pinvou3_home()
        .join("tmp")
        .join("dingtalk-npm-install-home");
    if install_home.exists() {
        std::fs::remove_dir_all(&install_home)
            .map_err(|e| format!("清理钉钉 CLI 安装暂存目录失败: {e}"))?;
    }
    std::fs::create_dir_all(&install_home)
        .map_err(|e| format!("创建钉钉 CLI 安装暂存目录失败: {e}"))?;

    let mut command = DINGTALK_CTX.base_cmd("npm");
    cc::apply_user_npm_prefix(&mut command);
    // HOME 隔离后 npm 默认不再读取真实用户的 ~/.npmrc。显式保留 userconfig，
    // 避免国内镜像 / 企业代理配置被静默丢掉；postinstall 的 os.homedir() 仍指向暂存目录。
    if std::env::var_os("NPM_CONFIG_USERCONFIG").is_none()
        && std::env::var_os("npm_config_userconfig").is_none()
    {
        let user_config = crate::platform::paths::user_home_dir().join(".npmrc");
        if user_config.is_file() {
            command.env("NPM_CONFIG_USERCONFIG", user_config);
        }
    }
    command
        .env("HOME", &install_home)
        .env("USERPROFILE", &install_home)
        .args(["install", "-g", DWS_NPM_SPEC]);
    Ok((command, install_home))
}

fn install_dws_cli() -> Result<bool, String> {
    let (command, install_home) = isolated_npm_install_command()?;
    let result = cc::run_with_timeout(command, 180);
    if let Err(err) = std::fs::remove_dir_all(&install_home) {
        eprintln!(
            "[dingtalk] cleanup isolated npm install home failed ({}): {err}",
            install_home.display()
        );
    }
    result
}

/// 引导:确保 dws 装好。Linux ARM64 优先使用内置二进制;Windows 走 npm 全局 shim。
#[tauri::command]
pub async fn dingtalk_ensure_cli() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        if dws_cli_present() {
            return Ok::<Value, String>(json!({ "ok": true, "already": true }));
        }
        if !install_dws_cli()? {
            return Err("钉钉 CLI 安装失败，请查看 ~/.pinvou3/cli-install.log".to_string());
        }
        if !dws_cli_present() {
            return Err("钉钉 CLI 安装完成但无法执行，请重试或检查 npm 全局目录".to_string());
        }
        Ok::<Value, String>(json!({ "ok": true, "already": false }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 查询当前钉钉连接状态。只返回布尔,不把身份信息带进 webview。
#[tauri::command]
pub async fn dingtalk_status() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        if !dws_cli_present() {
            return Ok::<Value, String>(json!({
                "ok": false, "connected": false, "installed": false
            }));
        }
        let (ok, so, se) = cc::run(dws(&["auth", "status", "--format", "json"]))?;
        let connected = auth_is_authenticated_str(&so) || auth_is_authenticated_str(&se);
        Ok::<Value, String>(json!({
            "ok": ok, "connected": connected, "installed": true
        }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 开始连接钉钉(单段扫码)。立即返回 `{started:true}`,前端 listen 事件驱动 UI。
#[tauri::command]
pub async fn dingtalk_connect_begin(app: AppHandle) -> Result<Value, String> {
    app.state::<ConnectorConn>().reset(ID);
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || run_connect_flow(&app2));
    Ok(json!({ "started": true }))
}

fn run_connect_flow(app: &AppHandle) {
    if let Err(e) = phase_scan(app) {
        cc::emit(
            app,
            "dingtalk:error",
            json!({ "phase": "authorize", "message": e }),
        );
    }
}

fn phase_scan(app: &AppHandle) -> Result<(), String> {
    let mut cmd = dws(&["auth", "login", "--device"]);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        format!("dws auth login 启动失败: {e}(需要 dws；Linux ARM64 会优先使用内置 CLI)")
    })?;
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

    let mut user_code: Option<String> = None;
    let mut plain_url: Option<String> = None;
    let mut auth_lines = VecDeque::with_capacity(32);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let url = loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            let _ = child.kill();
            conn.set_pid(ID, None);
            return Err("60s 内未拿到二维码链接(检查网络 / 代理)".into());
        }
        match rx.recv_timeout(deadline.saturating_duration_since(now)) {
            Ok(AuthEvent::Url(u)) => {
                if u.contains("user_code=") {
                    break u;
                }
                if let Some(code) = user_code.as_deref() {
                    break with_user_code_param(&u, code);
                }
                plain_url = Some(u);
            }
            Ok(AuthEvent::UserCode(c)) => {
                if let Some(u) = plain_url.as_deref() {
                    let full = with_user_code_param(u, &c);
                    user_code = Some(c);
                    break full;
                }
                user_code = Some(c);
            }
            Ok(AuthEvent::Line(line)) => {
                if auth_lines.len() >= 32 {
                    auth_lines.pop_front();
                }
                auth_lines.push_back(line);
            }
            Err(_) => {
                let _ = child.kill();
                conn.set_pid(ID, None);
                return Err("60s 内未拿到二维码链接(检查网络 / 代理)".into());
            }
        }
    };

    if user_code.is_none() {
        if let Some((_, code)) = url.split_once("user_code=") {
            user_code = code
                .split('&')
                .next()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
        }
    }
    eprintln!(
        "[dingtalk] auth device url ready: has_user_code_param={}, has_user_code={}",
        url.contains("user_code="),
        user_code.is_some()
    );
    cc::emit(
        app,
        "dingtalk:qr",
        json!({ "phase": "authorize", "url": url, "user_code": user_code, "qr_data_url": cc::make_qr(&url) }),
    );

    loop {
        if conn.is_cancelled(ID) {
            let _ = child.kill();
            conn.set_pid(ID, None);
            return Ok(());
        }
        while let Ok(event) = rx.try_recv() {
            match event {
                AuthEvent::Line(line) => {
                    if auth_lines.len() >= 32 {
                        auth_lines.pop_front();
                    }
                    auth_lines.push_back(line);
                }
                AuthEvent::Url(_) | AuthEvent::UserCode(_) => {}
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                conn.set_pid(ID, None);
                if is_authenticated() {
                    cc::emit(app, "dingtalk:connected", json!({ "ok": true }));
                    return Ok(());
                }
                eprintln!(
                    "[dingtalk] auth login exited without authenticated status: exit={status}, {}",
                    auth_status_message()
                );
                let raw = auth_lines.iter().cloned().collect::<Vec<_>>().join("\n");
                return Err(dingtalk_auth_error_hint(&raw)
                    .unwrap_or_else(|| "授权未完成(可能已取消或超时)".into()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(400)),
            Err(e) => {
                conn.set_pid(ID, None);
                return Err(format!("auth login 等待失败: {e}"));
            }
        }
    }
}

#[tauri::command]
pub async fn dingtalk_cancel(app: AppHandle) -> Result<Value, String> {
    let pid = app.state::<ConnectorConn>().cancel(ID);
    if let Some(pid) = pid {
        let _ = tokio::task::spawn_blocking(move || cc::kill_pid_tree(pid)).await;
    }
    Ok(json!({ "ok": true }))
}

/// 断开钉钉:`dws auth logout`。未安装时也视为已断开。
#[tauri::command]
pub async fn dingtalk_logout() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        if !dws_cli_present() {
            return Ok::<Value, String>(json!({ "ok": true, "installed": false }));
        }
        let (ok, _, _) = cc::run(dws(&["auth", "logout", "--yes"]))?;
        if !ok {
            return Err("钉钉 CLI 退出登录失败，请重试".to_string());
        }
        Ok::<Value, String>(json!({ "ok": ok, "installed": true }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ─────────────────────── 钉钉 skill 门控(对齐飞书 / 企微)───────────────────────

fn dingtalk_disabled_path() -> std::path::PathBuf {
    crate::platform::paths::pinvou3_home().join("dingtalk_disabled")
}

pub fn is_dingtalk_disabled() -> bool {
    dingtalk_disabled_path().exists()
}

fn set_dingtalk_disabled_flag(disabled: bool) -> Result<(), String> {
    let p = dingtalk_disabled_path();
    if disabled {
        std::fs::write(&p, b"1").map_err(|e| format!("保存钉钉技能停用状态失败: {e}"))?;
    } else if p.exists() {
        std::fs::remove_file(&p).map_err(|e| format!("清除钉钉技能停用状态失败: {e}"))?;
    }
    Ok(())
}

pub fn dingtalk_skills_should_show() -> bool {
    !is_dingtalk_disabled() && is_authenticated()
}

#[tauri::command]
pub async fn dingtalk_apply_skills() -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(|| -> Result<bool, String> {
        let show = dingtalk_skills_should_show();
        crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
            .apply_dingtalk_skills(show)
            .map_err(|e| format!("更新钉钉技能失败: {e}"))?;
        Ok(show)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))??;
    Ok(json!({ "visible": show }))
}

#[tauri::command]
pub async fn set_dingtalk_enabled(enabled: bool) -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        set_dingtalk_disabled_flag(!enabled)?;
        let show = dingtalk_skills_should_show();
        crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
            .apply_dingtalk_skills(show)
            .map_err(|e| format!("更新钉钉技能失败: {e}"))?;
        Ok(show)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))??;
    Ok(json!({ "ok": true, "visible": show }))
}

#[tauri::command]
pub async fn dingtalk_skills_state() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let disabled = is_dingtalk_disabled();
        let connected = is_authenticated();
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
    fn auth_status_detects_authenticated() {
        assert!(auth_is_authenticated_str(
            r#"{"success":true,"authenticated":true}"#
        ));
        assert!(!auth_is_authenticated_str(
            r#"{"success":true,"authenticated":false,"message":"未登录"}"#
        ));
        assert!(!auth_is_authenticated_str(r#"{"authenticated":"true"}"#));
        assert!(!auth_is_authenticated_str(""));
    }

    #[test]
    fn user_code_is_extracted_from_device_flow_text() {
        assert_eq!(
            extract_user_code("User Code: ABCD-EFGH"),
            Some("ABCD-EFGH".to_string())
        );
        assert_eq!(
            extract_user_code("请在页面输入验证码：ZXCV1234"),
            Some("ZXCV1234".to_string())
        );
        assert_eq!(
            extract_user_code("open https://login.dingtalk.com/foo"),
            None
        );
    }

    #[test]
    fn cli_data_access_error_is_explained() {
        let hint = dingtalk_auth_error_hint(
            r#"组织主管理员：xuyajing
{"error":{"category":"auth","code":2,"message":"device authorization failed: CLI data access is not enabled for this organization, please contact admin to enable it"}}"#,
        )
        .unwrap();
        assert!(hint.contains("钉钉组织未开启 CLI 数据访问"));
        assert!(hint.contains("xuyajing"));
    }

    #[test]
    fn dingtalk_disabled_flag_roundtrip() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = format!(
            "{}/pinvou3-dingtalk-test-{}",
            std::env::temp_dir().display(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::create_dir_all(crate::platform::paths::pinvou3_home());

        set_dingtalk_disabled_flag(false).unwrap();
        assert!(!is_dingtalk_disabled());
        set_dingtalk_disabled_flag(true).unwrap();
        assert!(is_dingtalk_disabled());
        set_dingtalk_disabled_flag(false).unwrap();
        assert!(!is_dingtalk_disabled());

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn npm_install_uses_isolated_home_for_official_postinstall() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = std::env::temp_dir().join(format!(
            "pinvou3-dingtalk-npm-home-test-{}",
            std::process::id()
        ));
        let previous = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("PINVOU3_HOME", &root);

        let (command, install_home) = isolated_npm_install_command().unwrap();
        let envs = command
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key.to_owned(), value.to_owned())))
            .collect::<std::collections::HashMap<_, _>>();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, ["install", "-g", DWS_NPM_SPEC]);
        assert_eq!(
            envs.get(std::ffi::OsStr::new("HOME")),
            Some(&install_home.clone().into_os_string())
        );
        assert_eq!(
            envs.get(std::ffi::OsStr::new("USERPROFILE")),
            Some(&install_home.clone().into_os_string())
        );
        let user_config = crate::platform::paths::user_home_dir().join(".npmrc");
        if user_config.is_file()
            && std::env::var_os("NPM_CONFIG_USERCONFIG").is_none()
            && std::env::var_os("npm_config_userconfig").is_none()
        {
            assert_eq!(
                envs.get(std::ffi::OsStr::new("NPM_CONFIG_USERCONFIG")),
                Some(&user_config.into_os_string())
            );
        }
        assert!(install_home.starts_with(&root));

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
