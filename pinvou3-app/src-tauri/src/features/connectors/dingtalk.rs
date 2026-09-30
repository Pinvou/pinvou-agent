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

use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use crate::features::connectors::connector_cli::{self as cc, CliCtx, ConnectorConn};
use crate::features::connectors::skill_gate::ConnectorGate;

const ID: &str = "dingtalk";
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
    dws_cli_probe().unwrap_or(false)
}

/// `--version` 探测三态:`Ok(true)` 已安装可用;`Ok(false)` 已安装但版本探测
/// 退出非零;`Err(ProbeError)` 探测本身失败,按 Spawn/Timeout/Other 分型。
/// 状态轮询把失败都折叠成「未连接」;断开登录路径必须按分型与探测结果
/// 区别对待,见 [`dingtalk_logout`]。
fn dws_cli_probe() -> Result<bool, cc::ProbeError> {
    cc::run_probe(dws(&["--version"])).map(|(ok, _, _)| ok)
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
        .rfind(|s| {
            let len = s.len();
            (6..=32).contains(&len)
                && s.chars()
                    .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && s.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
        })
        .map(|s| s.to_string())
}

fn safe_auth_log_line(line: &str) -> Option<String> {
    // 不开兜底 "token"：钉钉 CLI 的正常 JSON 行含 camelCase token 字段名，
    // 开了会把非敏感行也整体吞掉（与旧本地实现行为一致）。
    cc::safe_auth_log_line(line, false)
}

/// The organization-level CLI block: the CLI's auth output reports that CLI
/// data access is not enabled for this organization. Only an org admin can
/// fix this, so it gets its own stable card code with actionable localized
/// copy — the generic "reconnect and try again" advice can never fix it. The
/// detection deliberately matches anywhere in the raw text so both the
/// pretty-printed and embedded-JSON output shapes hit.
fn cli_data_access_blocked(text: &str) -> bool {
    text.contains("CLI data access is not enabled")
}

/// Best raw-cause line for the failure trail: prefer the CLI's structured
/// `error.message`, then the last failure-ish output line. The card never
/// renders this — it keys localized copy off the stable code.
fn dingtalk_cli_auth_reason(text: &str) -> Option<String> {
    cc::parse_json(text)
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| format!("dingtalk CLI auth failed: {s}"))
        })
        .or_else(|| {
            text.lines()
                .rev()
                .find(|line| {
                    let l = line.to_ascii_lowercase();
                    l.contains("failed") || l.contains("error") || line.contains("失败")
                })
                .map(|line| format!("dingtalk CLI auth failed: {}", line.trim()))
        })
}

/// A phase-scan failure pairs the stable card code with the raw cause: the
/// card renders localized dictionary copy keyed by the code, and the raw
/// cause goes to the failure trail. Known failure categories carry their own
/// code; everything else defaults to `auth_failed`.
struct FlowError {
    code: &'static str,
    message: String,
}

impl From<String> for FlowError {
    fn from(message: String) -> Self {
        FlowError {
            code: "auth_failed",
            message,
        }
    }
}

impl From<&str> for FlowError {
    fn from(message: &str) -> Self {
        FlowError::from(message.to_string())
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
                    // 管道读取错误通常不可恢复；继续迭代可能反复返回 Err 并空转。
                    log::warn!("[dingtalk] 授权输出读取失败，停止排空：{error}");
                    break;
                }
            };
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

/// `dws auth status --format json` 判当前是否已登录。会 spawn dws。
pub(crate) fn is_authenticated() -> bool {
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

/// 引导:首次使用时下载并校验锁定版本的 dws，已装则秒返回。
pub async fn dingtalk_ensure_cli() -> Result<Value, String> {
    cc::ensure_cli_with(
        "dingtalk",
        dws_cli_present,
        "钉钉 CLI 安装完成但无法执行，请重试",
        || crate::features::connectors::native_installer::ensure_native_cli("dws"),
    )
    .await
}

/// 查询当前钉钉连接状态。只返回布尔,不把身份信息带进 webview。
/// (Only called internally by the command layer's `bundle_readiness` CLI dispatch; there is no standalone Tauri command anymore.)
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
pub async fn dingtalk_connect_begin(app: AppHandle) -> Result<Value, String> {
    let conn = app.state::<ConnectorConn>();
    // A reconnect without an intervening cancel leaves the previous round's
    // long-running child registered in the pid slot; the reset below overwrites
    // that slot, which would make the orphan invisible to cancel's tree-kill
    // and to kill_all_pids at exit. Kill it before resetting.
    if let Some(pid) = conn.cancel(ID) {
        let _ = tokio::task::spawn_blocking(move || cc::kill_pid_tree(pid)).await;
    }
    let generation = conn.reset(ID);
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || run_connect_flow(&app2, generation));
    Ok(json!({ "started": true }))
}

fn run_connect_flow(app: &AppHandle, generation: u64) {
    let conn = app.state::<ConnectorConn>();
    if let Err(e) = phase_scan(app, generation) {
        // The card renders a localized category message only; the raw cause
        // is logged here (stdout in dev runs, the app log in packaged builds).
        log::warn!("[dingtalk] connect flow failed ({}): {}", e.code, e.message);
        // reset() clears the cancelled flag, so the flag alone cannot stop a
        // late emit in the cancel-then-reconnect window; a cancelled or
        // superseded round stays silent instead of polluting the new card.
        if conn.is_cancelled(ID) || conn.flow_stale(ID, generation) {
            return;
        }
        cc::emit(
            app,
            "dingtalk:error",
            json!({ "phase": "authorize", "code": e.code, "message": e.message }),
        );
    }
}

fn phase_scan(app: &AppHandle, generation: u64) -> Result<(), FlowError> {
    let mut cmd = dws(&["auth", "login", "--device"]);
    // 独立进程组:npm shim(shell→node)派生的孙进程与 shim 同组,退出收割的
    // kill_pid_tree 按负 pid 组杀整棵树,单杀 shim pid 会把 node 孤儿化。
    crate::platform::process::std_process_group_leader(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("dws auth login 启动失败: {e}(需要先完成钉钉 CLI 在线安装)"))?;
    let conn = app.state::<ConnectorConn>();
    let pid = child.id();
    conn.set_pid(ID, Some(pid));

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
            cc::reap_after_kill(&mut child);
            conn.clear_pid_if(ID, pid);
            // Aligned with tmeet: a cancel landing at the deadline instant is
            // handled as a cancel, silent.
            if conn.is_cancelled(ID) {
                return Ok(());
            }
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
                cc::reap_after_kill(&mut child);
                conn.clear_pid_if(ID, pid);
                // Cancel tree-kills the child → pipe EOF lands here: the user stopped
                // on purpose, so finish silently instead of misreporting a link timeout.
                if conn.is_cancelled(ID) {
                    return Ok(());
                }
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
    log::info!(
        "[dingtalk] auth device url ready: has_user_code_param={}, has_user_code={}",
        url.contains("user_code="),
        user_code.is_some()
    );
    // A cancelled round must not re-open the scan modal the user dismissed,
    // and after a reconnect a superseded round's QR would be dead on arrival.
    if !(conn.is_cancelled(ID) || conn.flow_stale(ID, generation)) {
        cc::emit(
            app,
            "dingtalk:qr",
            json!({ "phase": "authorize", "url": url, "user_code": user_code, "qr_data_url": cc::make_qr(&url) }),
        );
    }

    loop {
        if conn.is_cancelled(ID) {
            let _ = child.kill();
            cc::reap_after_kill(&mut child);
            conn.clear_pid_if(ID, pid);
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
                conn.clear_pid_if(ID, pid);
                if conn.is_cancelled(ID) {
                    // Cancel race: a kill-induced failed exit is handled as a cancel, silent.
                    log::info!("[dingtalk] cancelled; child exit={status}");
                    return Ok(());
                }
                if is_authenticated() {
                    // The auth probe is a subprocess call and can span a
                    // cancel: finish silently — no connected onto a closed
                    // card, no resurrected error card.
                    if conn.is_cancelled(ID) {
                        return Ok(());
                    }
                    cc::bundle_store_on_connected(ID);
                    cc::emit(app, "dingtalk:connected", json!({ "ok": true }));
                    return Ok(());
                }
                log::warn!(
                    "[dingtalk] auth login exited without authenticated status: exit={status}, {}",
                    auth_status_message()
                );
                let raw = auth_lines.iter().cloned().collect::<Vec<_>>().join("\n");
                // Known org-level block: the card dictionary carries actionable
                // copy under this stable code; only an org admin can fix it, so
                // the generic retry advice would be wrong.
                if cli_data_access_blocked(&raw) {
                    return Err(FlowError {
                        code: "cli_data_access_disabled",
                        message: "dingtalk org has not enabled CLI data access".into(),
                    });
                }
                return Err(FlowError::from(
                    dingtalk_cli_auth_reason(&raw)
                        .unwrap_or_else(|| "授权未完成(可能已取消或超时)".into()),
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(400)),
            Err(e) => {
                conn.clear_pid_if(ID, pid);
                return Err(format!("auth login 等待失败: {e}").into());
            }
        }
    }
}
pub async fn dingtalk_cancel(app: AppHandle) -> Result<Value, String> {
    let pid = app.state::<ConnectorConn>().cancel(ID);
    if let Some(pid) = pid {
        let _ = tokio::task::spawn_blocking(move || cc::kill_pid_tree(pid)).await;
    }
    Ok(json!({ "ok": true }))
}

/// 断开钉钉:`dws auth logout`。未安装时也视为已断开。
///
/// 探测的任何非成功结果都不能沿用状态轮询的「按未安装降级」:无论是
/// 超时/执行异常(CLI 挂死),还是 `--version` 成功执行但退出非零
/// (CLI 损坏/版本过旧——只证明它没能正常自报版本,不能证明不存在),
/// `auth logout` 都未曾执行、token 未撤销,返回 `ok:true/installed:false`
/// 会向用户谎报已断开。只有 [`cc::ProbeError::Spawn`](≈二进制不存在,
/// 真未安装)保留原降级。裁决统一走 [`cc::logout_probe_verdict`],
/// 其余按分型转为人类可读文案原样上抛(超时文案自带重试指引)。
pub async fn dingtalk_logout() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let not_installed = || {
            cc::bundle_store_on_disconnected(ID);
            Ok::<Value, String>(json!({ "ok": true, "installed": false }))
        };
        match cc::logout_probe_verdict("钉钉", dws_cli_probe()) {
            cc::LogoutProbeVerdict::Installed => {}
            cc::LogoutProbeVerdict::NotInstalled => return not_installed(),
            cc::LogoutProbeVerdict::Unconfirmed(message) => return Err(message),
        }
        let (ok, _, _) = cc::run(dws(&["auth", "logout", "--yes"]))?;
        if !ok {
            return Err("钉钉 CLI 退出登录失败，请重试".to_string());
        }
        cc::bundle_store_on_disconnected(ID);
        Ok::<Value, String>(json!({ "ok": ok, "installed": true }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ─────────────────────── 钉钉 skill 门控(对齐飞书 / 企微)───────────────────────

/// 按 visible 写 / 删钉钉技能文件(调 [`Pinvou3Bundle::apply_dingtalk_skills`])。
pub(crate) fn apply_bundle_skills(visible: bool) -> std::io::Result<()> {
    crate::features::runtime_bundle::platform::Pinvou3Bundle::paths().apply_dingtalk_skills(visible)
}

/// 钉钉门控表项:停用标志 + 就绪探测 + 技能落盘;
/// apply/skills_state 等命令公共体见 [`ConnectorGate`]。
pub(crate) static DINGTALK_GATE: ConnectorGate = ConnectorGate {
    id: "dingtalk",
    disabled_filename: "dingtalk_disabled",
    display_name: "钉钉",
    ready_probe: is_authenticated,
    apply_bundle_skills: apply_bundle_skills,
};

pub async fn dingtalk_apply_skills() -> Result<Value, String> {
    DINGTALK_GATE.apply_skills_command().await
}

pub async fn dingtalk_skills_state() -> Result<Value, String> {
    DINGTALK_GATE.skills_state_command().await
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn cli_data_access_error_gets_its_own_code() {
        let raw = r#"组织主管理员：xuyajing
{"error":{"category":"auth","code":2,"message":"device authorization failed: CLI data access is not enabled for this organization, please contact admin to enable it"}}"#;
        assert!(cli_data_access_blocked(raw));
        let reason = dingtalk_cli_auth_reason(raw).unwrap();
        assert!(reason.contains("CLI data access is not enabled"));
        // A plain signed-out JSON is not the org block; the reason extractor
        // still prefers its structured message for the log trail.
        assert!(!cli_data_access_blocked(
            r#"{"error":{"message":"未登录"}}"#
        ));
        assert_eq!(
            dingtalk_cli_auth_reason(r#"{"error":{"message":"未登录"}}"#).as_deref(),
            Some("dingtalk CLI auth failed: 未登录")
        );
    }
}
