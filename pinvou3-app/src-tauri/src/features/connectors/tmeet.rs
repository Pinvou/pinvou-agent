//! 腾讯会议(`@tencentcloud/tmeet`) CLI 连接器 —— 随包 Node/npm 在线安装 + OAuth 授权。
//!
//! 路线同钉钉 / 企微:官方 CLI + 官方 skill,不要求用户填写 API Key。
//! 连接:`tmeet auth login --no-browser` 长驻 → 抓腾讯会议授权 URL → 用户扫码 / 浏览器授权 →
//! 进程退出后 `tmeet auth status` 包含 `Logged in` 判 connected。

use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use crate::features::connectors::connector_cli::{self as cc, CliCtx, ConnectorConn};
use crate::features::connectors::skill_gate::ConnectorGate;

const ID: &str = "tmeet";
const TMEET_NPM_SPEC: &str = "@tencentcloud/tmeet@1.0.18";
const TMEET_MIN_VERSION: (u64, u64, u64) = (1, 0, 18);

const TMEET_CTX: CliCtx = CliCtx {
    cli_bin: "tmeet",
    envs: &[("TMEET_AGENT", "Pinvou"), ("TMEET_MODEL", "Pinvou")],
    auth_domains: &["meeting.tencent.com"],
};

fn tmeet(args: &[&str]) -> std::process::Command {
    TMEET_CTX.cli(args)
}

fn parse_tmeet_version(s: &str) -> Option<(u64, u64, u64)> {
    // tmeet 的 --version 程序名侧可能带数字,先定位到 "version" 标记之后
    // (v 前缀也吃掉),再交给共享的三段解析。
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
    cc::parse_semver3(version)
}

fn tmeet_cli_version() -> Option<(u64, u64, u64)> {
    tmeet_cli_version_probe().ok().flatten()
}

/// `--version` 探测三态:`Ok(Some(v))` 已安装可用;`Ok(None)` 已安装但退出
/// 非零/版本无法解析;`Err(ProbeError)` 探测本身失败,按 Spawn/Timeout/Other
/// 分型。状态轮询把失败都折叠成「未连接」;断开登录路径必须按分型与探测
/// 结果区别对待,见 [`tmeet_logout`]。
fn tmeet_cli_version_probe() -> Result<Option<(u64, u64, u64)>, cc::ProbeError> {
    let (ok, so, se) = cc::run_probe(tmeet(&["--version"]))?;
    if !ok {
        return Ok(None);
    }
    Ok(parse_tmeet_version(&so).or_else(|| parse_tmeet_version(&se)))
}

fn tmeet_cli_present() -> bool {
    tmeet_cli_version()
        .map(|v| v >= TMEET_MIN_VERSION)
        .unwrap_or(false)
}

fn status_is_logged_in(s: &str) -> bool {
    s.contains("Logged in")
}

/// `tmeet auth status` 判当前是否已登录。会 spawn tmeet。
pub(crate) fn is_logged_in() -> bool {
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
    cc::safe_auth_log_line(line, true)
}

fn install_tmeet_cli() -> Result<(), String> {
    // registry.npmjs.org is often unreachable on China networks: after the
    // default registry fails outright, retry this invocation once via
    // npmmirror (only appending --registry, never reading or writing the
    // user's npm config), the same policy as the codex/claude npm upgrade
    // mirror retry; both attempts' output is separated by marker lines and
    // appended in order to cli-install.log, so the first failure's cause is
    // not lost.
    let attempt = |registry: Option<&str>| -> Result<bool, String> {
        let mut c = TMEET_CTX.base_cmd("npm");
        cc::apply_user_npm_prefix(&mut c);
        c.args(["install", "-g", TMEET_NPM_SPEC]);
        if let Some(registry) = registry {
            c.arg(format!("--registry={registry}"));
        }
        // run_with_timeout only returns a success boolean; output is
        // uniformly appended to cli-install.log for diagnosis.
        cc::run_with_timeout(c, 180)
    };
    cc::append_cli_install_log("── npm install @tencentcloud/tmeet (default npm registry) ──");
    let first = attempt(None);
    if first.as_ref().is_ok_and(|ok| *ok) {
        return Ok(());
    }
    cc::append_cli_install_log("── default registry failed, retrying via npmmirror ──");
    let second = attempt(Some(crate::platform::download::NPM_MIRROR_REGISTRY));
    if second.as_ref().is_ok_and(|ok| *ok) {
        return Ok(());
    }
    // When both attempts fail, preserve the causal chain starting from the
    // first error: reporting only the retry error would bury a first failure
    // unrelated to the network (EACCES / disk full etc.) in the log.
    let mut causes: Vec<String> = Vec::new();
    if let Err(primary) = &first {
        causes.push(format!("default registry error: {primary}"));
    }
    if let Err(retry) = &second {
        causes.push(format!("npmmirror retry error: {retry}"));
    }
    let detail = if causes.is_empty() {
        "both the default registry and the npmmirror mirror failed".to_string()
    } else {
        causes.join("; ")
    };
    Err(format!(
        "Tencent Meeting CLI install failed: {detail}; see ~/.pinvou3/cli-install.log for details"
    ))
}

/// Bootstrap: ensure the tmeet CLI is installed and at least 1.0.18.
pub async fn tmeet_ensure_cli() -> Result<Value, String> {
    cc::ensure_cli_with(
        "tmeet",
        tmeet_cli_present,
        "腾讯会议 CLI 安装完成但无法执行，请重试或修复应用运行时",
        install_tmeet_cli,
    )
    .await
}

/// 查询当前腾讯会议连接状态。只返回布尔,不把身份 / token 信息带进 webview。
/// (Only called internally by the command layer's `bundle_readiness` CLI dispatch; there is no standalone Tauri command anymore.)
pub async fn tmeet_status() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        // Don't spawn auth status when the CLI isn't installed. Overly old versions report installed:true
        // just like usable ones; upgrade guidance is handled by ensure_cli's version gate (tmeet_cli_present).
        if tmeet_cli_version().is_none() {
            return Ok::<Value, String>(json!({
                "ok": false, "connected": false, "installed": false
            }));
        }
        let (ok, so, se) = cc::run(tmeet(&["auth", "status"]))?;
        let connected = status_is_logged_in(&so) || status_is_logged_in(&se);
        Ok::<Value, String>(json!({
            "ok": ok,
            "connected": connected,
            "installed": true
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
    // The generation is captured at reset: the already_logged_in branch belongs
    // to this round (login state is a machine-level fact, harmless when late),
    // only the spawned flow thread needs the staleness self-check.
    let generation = conn.reset(ID);
    let already_logged_in = tokio::task::spawn_blocking(is_logged_in)
        .await
        .map_err(|e| {
            // The frontend renders only the derived error code, so without this
            // line the join failure's cause would be lost entirely.
            log::warn!("[tmeet] logged-in probe task failed: {e}");
            format!("spawn_blocking: {e}")
        })?;
    if already_logged_in {
        cc::bundle_store_on_connected(ID);
        cc::emit(
            &app,
            "tmeet:connected",
            json!({ "ok": true, "already": true }),
        );
        return Ok(json!({ "started": true, "already_connected": true }));
    }
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || run_connect_flow(&app2, generation));
    Ok(json!({ "started": true }))
}

fn run_connect_flow(app: &AppHandle, generation: u64) {
    let conn = app.state::<ConnectorConn>();
    if let Err(e) = phase_scan(app, generation) {
        // The card renders a localized category message only; the raw cause
        // is logged here (stdout in dev runs, the app log in packaged builds).
        log::warn!("[tmeet] connect flow failed: {e}");
        // reset() clears the cancelled flag, so the flag alone cannot stop a
        // late emit in the cancel-then-reconnect window; a cancelled or
        // superseded round stays silent instead of polluting the new card.
        if conn.is_cancelled(ID) || conn.flow_stale(ID, generation) {
            return;
        }
        cc::emit(
            app,
            "tmeet:error",
            json!({ "phase": "authorize", "code": "auth_failed", "message": e }),
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

fn phase_scan(app: &AppHandle, generation: u64) -> Result<(), String> {
    let mut cmd = tmeet(&["auth", "login", "--no-browser"]);
    // 独立进程组:npm shim(shell→node)派生的孙进程与 shim 同组,退出收割的
    // kill_pid_tree 按负 pid 组杀整棵树,单杀 shim pid 会把 node 孤儿化。
    crate::platform::process::std_process_group_leader(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("tmeet auth login 启动失败: {e}(需要 tmeet CLI)"))?;
    let conn = app.state::<ConnectorConn>();
    let pid = child.id();
    conn.set_pid(ID, Some(pid));

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
            cc::reap_after_kill(&mut child);
            conn.clear_pid_if(ID, pid);
            // Cancel tree-kills the child; a cancel landing in this window must
            // finish silently instead of misreporting an authorization timeout.
            if conn.is_cancelled(ID) {
                return Ok(());
            }
            return Err(cc::auth_failure_reason(
                &auth_lines,
                "60s 内未拿到腾讯会议授权链接(检查网络 / 代理)",
            ));
        }
        match rx.recv_timeout(std::cmp::min(
            Duration::from_millis(400),
            deadline.saturating_duration_since(now),
        )) {
            Ok((Some(u), line)) => {
                cc::remember_auth_line(&mut auth_lines, line);
                break u;
            }
            Ok((None, line)) => cc::remember_auth_line(&mut auth_lines, line),
            Err(_) => {
                // Cancel tree-kills the child → pipe EOF lands here: the user stopped
                // on purpose, so finish silently instead of misreporting an auth failure.
                if conn.is_cancelled(ID) {
                    let _ = child.kill();
                    cc::reap_after_kill(&mut child);
                    conn.clear_pid_if(ID, pid);
                    return Ok(());
                }
                if let Ok(Some(status)) = child.try_wait() {
                    conn.clear_pid_if(ID, pid);
                    log::info!("[tmeet] auth login exited before auth url: exit={status}");
                    // A single status wait is enough: the already flag is decided
                    // from the captured output lines, with no second 5s polling
                    // run just to fill in already:true.
                    let already = auth_lines_say_already_logged_in(&auth_lines)
                        && wait_logged_in(Duration::from_secs(5));
                    // wait_logged_in polls a subprocess for up to 5s; a cancel
                    // landing inside it must finish silently instead of emitting
                    // onto the just-closed card.
                    if conn.is_cancelled(ID) {
                        return Ok(());
                    }
                    if already {
                        cc::bundle_store_on_connected(ID);
                        cc::emit(
                            app,
                            "tmeet:connected",
                            json!({ "ok": true, "already": true }),
                        );
                        return Ok(());
                    }
                    return Err(cc::auth_failure_reason(
                        &auth_lines,
                        "腾讯会议授权进程提前退出，未拿到授权链接",
                    ));
                }
            }
        }
    };

    // A cancelled round must not re-open the scan modal the user dismissed,
    // and after a reconnect a superseded round's QR would be dead on arrival.
    if !(conn.is_cancelled(ID) || conn.flow_stale(ID, generation)) {
        cc::emit(
            app,
            "tmeet:qr",
            json!({ "phase": "authorize", "url": url, "qr_data_url": cc::make_qr(&url) }),
        );
    }

    loop {
        if conn.is_cancelled(ID) {
            let _ = child.kill();
            cc::reap_after_kill(&mut child);
            conn.clear_pid_if(ID, pid);
            return Ok(());
        }
        while let Ok((_, line)) = rx.try_recv() {
            cc::remember_auth_line(&mut auth_lines, line);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                conn.clear_pid_if(ID, pid);
                // Cancel race: a kill-induced failed exit is handled as a cancel, silent.
                if conn.is_cancelled(ID) {
                    log::info!("[tmeet] cancelled; child exit={status}");
                    return Ok(());
                }
                // A single status wait is enough: the already flag is decided from the captured output lines,
                // with no second 5s polling run just to fill in already:true.
                let logged_in = wait_logged_in(Duration::from_secs(5));
                // wait_logged_in polls a subprocess for up to 5s; a cancel landing
                // inside it must finish silently instead of emitting onto the
                // just-closed card.
                if conn.is_cancelled(ID) {
                    return Ok(());
                }
                if logged_in {
                    cc::bundle_store_on_connected(ID);
                    let already = auth_lines_say_already_logged_in(&auth_lines);
                    cc::emit(
                        app,
                        "tmeet:connected",
                        json!({ "ok": true, "already": already }),
                    );
                    return Ok(());
                }
                log::info!("[tmeet] auth login exited without logged-in status: exit={status}");
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
                conn.clear_pid_if(ID, pid);
                return Err(format!("auth login 等待失败: {e}"));
            }
        }
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
///
/// 探测的任何非成功结果都不能沿用状态轮询的「按未安装降级」:无论是
/// 超时/执行异常(CLI 挂死),还是版本探测成功执行但报非成功(`--version`
/// 退出非零、或版本无法解析/输出格式变更——只证明它没能正常自报版本,
/// 不能证明不存在),`auth logout` 都未曾执行、token 未撤销,返回
/// `ok:true/installed:false` 会向用户谎报已断开。只有
/// [`cc::ProbeError::Spawn`](≈二进制不存在,真未安装)保留原降级。
/// 裁决统一走 [`cc::logout_probe_verdict`],其余按分型转为人类可读
/// 文案原样上抛(超时文案自带重试指引)。
pub async fn tmeet_logout() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let not_installed = || {
            cc::bundle_store_on_disconnected(ID);
            Ok::<Value, String>(json!({ "ok": true, "installed": false }))
        };
        let probe = tmeet_cli_version_probe().map(|version| version.is_some());
        match cc::logout_probe_verdict("腾讯会议", probe) {
            cc::LogoutProbeVerdict::Installed => {}
            cc::LogoutProbeVerdict::NotInstalled => return not_installed(),
            cc::LogoutProbeVerdict::Unconfirmed(message) => return Err(message),
        }
        let (ok, _, _) = cc::run(tmeet(&["auth", "logout"]))?;
        if !ok {
            return Err("腾讯会议 CLI 退出登录失败，请重试".to_string());
        }
        cc::bundle_store_on_disconnected(ID);
        Ok::<Value, String>(json!({ "ok": true, "installed": true }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ─────────────────────── 腾讯会议 skill 门控 ────────────────────────

/// 按 visible 写 / 删腾讯会议技能文件(调 [`Pinvou3Bundle::apply_tmeet_skills`])。
pub(crate) fn apply_bundle_skills(visible: bool) -> std::io::Result<()> {
    crate::features::runtime_bundle::platform::Pinvou3Bundle::paths().apply_tmeet_skills(visible)
}

/// 腾讯会议门控表项:停用标志 + 就绪探测 + 技能落盘;
/// apply/skills_state 等命令公共体见 [`ConnectorGate`]。
pub(crate) static TMEET_GATE: ConnectorGate = ConnectorGate {
    id: "tmeet",
    disabled_filename: "tmeet_disabled",
    display_name: "腾讯会议",
    ready_probe: is_logged_in,
    apply_bundle_skills: apply_bundle_skills,
};

pub async fn tmeet_apply_skills() -> Result<Value, String> {
    TMEET_GATE.apply_skills_command().await
}

pub async fn tmeet_skills_state() -> Result<Value, String> {
    TMEET_GATE.skills_state_command().await
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
