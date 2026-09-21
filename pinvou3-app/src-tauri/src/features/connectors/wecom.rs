//! 企业微信(`@wecom/cli`,腾讯官方·MIT)CLI 连接器 —— 启动引导 + 扫码鉴权。
//!
//! 路线同飞书([`crate::features::connectors::feishu`]):官方 CLI + 官方域技能,riding 在腾讯官方 app 上,
//! **纯扫码**接入(不需管理员建自建应用、不需手填 CorpID/Secret)。
//! 公共管道见 [`crate::features::connectors::connector_cli`];本文件只有企微特有的薄声明 + 单段连接编排。
//!
//! Connection flow (wecom-cli ≥1.2.1): `wecom-cli auth init --noninteractive
//! --no-browser` runs persistently → grab the QR-code URL → the user scans →
//! once the process exits, `auth show --status` decides readiness.
//! Progress is reported via the `wecom:qr` / `wecom:connected` / `wecom:error`
//! events. Credentials live under `~/.config/wecom`
//! (Windows: `%USERPROFILE%\.config\wecom`) and are removed on disconnect.
//! Since 1.1.0 the command model was reshaped (`msg`→`message`,
//! `schedule`→`calendar`, arguments became flags); the current skill baseline
//! is the wecom-cli 1.2.1 domain skills and every platform lock pins 1.2.1,
//! so installs below [`WECOM_MIN_VERSION`] are replaced/upgraded.

use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use crate::features::connectors::connector_cli::{self as cc, CliCtx, ConnectorConn};
use crate::features::connectors::skill_gate::ConnectorGate;

/// 连接器 id(事件前缀 + ConnectorConn 槽位键 + 停用标志名)。
const ID: &str = "wecom";

/// 企微 CLI 薄声明。
/// envs 暂空:`work.weixin.qq.com` 为国内站,若实测被 Clash 等代理劫持,
/// 在此补 wecom-cli 的绕代理 env(参见飞书 `LARK_CLI_NO_PROXY`)。
const WECOM_CTX: CliCtx = CliCtx {
    cli_bin: "wecom-cli",
    envs: &[],
    auth_domains: &["work.weixin.qq.com", "weixin.qq.com"],
};

/// Minimum acceptable version for the skill and auth command surface: 1.1.0
/// reshaped the command model (`init`→`auth init`, `msg`→`message`,
/// `schedule`→`calendar`, JSON arguments→flags); the skill baseline is now
/// 1.2.1 and every platform lock pins 1.2.1, so the minimum is 1.2.1 —
/// installs below it must be replaced, while a minimum above a platform's
/// lowest pinned version would cause a replace/upgrade loop on that platform.
const WECOM_MIN_VERSION: (u64, u64, u64) = (1, 2, 1);

fn wecom(args: &[&str]) -> std::process::Command {
    WECOM_CTX.cli(args)
}

/// 解析 `wecom-cli --version` 输出(1.1.0 起格式为
/// `wecom-cli 1.1.0 (wecom 2026-08-17T03:14:38Z 889c555)`)。
/// 按 [`cc::parse_semver3`] 的契约先自行切片:只解析程序名 `wecom-cli`
/// 后随的那一段,构建时间戳等数字噪声不参与解析。
fn parse_wecom_version(s: &str) -> Option<(u64, u64, u64)> {
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let idx = tokens.iter().position(|t| t.contains("wecom-cli"))?;
    cc::parse_semver3(tokens.get(idx + 1).copied().unwrap_or(""))
}

fn wecom_cli_version() -> Option<(u64, u64, u64)> {
    let Ok((ok, so, se)) = cc::run(wecom(&["--version"])) else {
        return None;
    };
    if !ok {
        return None;
    }
    parse_wecom_version(&so).or_else(|| parse_wecom_version(&se))
}

/// Whether a wecom-cli is installed and ≥ 1.2.1 (minimum acceptable version);
/// older versions count as not installed and trigger the online replacement.
fn wecom_cli_present() -> bool {
    wecom_cli_version()
        .map(|v| v >= WECOM_MIN_VERSION)
        .unwrap_or(false)
}

/// `wecom-cli auth show --status` 是否输出 `authorized`(已扫码授权)。
fn status_is_authorized(s: &str) -> bool {
    s.trim().eq_ignore_ascii_case("authorized")
}

/// `wecom-cli auth show --status` 判当前是否已连接(已授权)。会 spawn wecom-cli。
pub(crate) fn is_ready() -> bool {
    if let Ok((ok, so, se)) = cc::run(wecom(&["auth", "show", "--status"])) {
        return ok && (status_is_authorized(&so) || status_is_authorized(&se));
    }
    false
}

// ───────────────────────────── Tauri commands ─────────────────────────────

/// Bootstrap: on first use (or when the installed CLI is below the 1.2.1
/// minimum acceptable version), download and verify the locked wecom-cli
/// version; returns immediately when a sufficient CLI is present, and an
/// old CLI in the managed directory is replaced outright on lock-hash mismatch.
pub async fn wecom_ensure_cli() -> Result<Value, String> {
    cc::ensure_cli_with(
        "wecom",
        wecom_cli_present,
        "企微 CLI 安装完成但无法执行，请重试",
        || crate::features::connectors::native_installer::ensure_native_cli("wecom-cli"),
    )
    .await
}

/// 查询当前企微连接状态:`wecom-cli auth show --status`。
/// (Only called internally by the command layer's `bundle_readiness` CLI dispatch; there is no standalone Tauri command anymore.)
pub async fn wecom_status() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        // 没装就别 spawn auth show —— 省掉没装连接器的用户每次白等一次子进程;
        // 装了则用同一次 --version 判 installed,不重复 spawn。过低版本与可用版本
        // 同样报告 installed:true,升级引导由 ensure_cli 的版本门槛负责。
        match wecom_cli_version() {
            None => Ok::<Value, String>(json!({
                "ok": false, "connected": false, "installed": false
            })),
            Some(_) => {
                let (ok, so, se) = cc::run(wecom(&["auth", "show", "--status"]))?;
                let connected = ok && (status_is_authorized(&so) || status_is_authorized(&se));
                // 只回布尔:--status 单行输出虽不含身份信息,保持最小回传面
                Ok::<Value, String>(json!({
                    "ok": ok, "connected": connected, "installed": true
                }))
            }
        }
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 开始连接企微(单段扫码)。立即返回 `{started:true}`,前端 listen 事件驱动 UI。
pub async fn wecom_connect_begin(app: AppHandle) -> Result<Value, String> {
    app.state::<ConnectorConn>().reset(ID);
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || run_connect_flow(&app2));
    Ok(json!({ "started": true }))
}

fn run_connect_flow(app: &AppHandle) {
    if let Err(e) = phase_scan(app) {
        cc::emit(
            app,
            "wecom:error",
            json!({ "phase": "authorize", "message": e }),
        );
    }
}

/// Waits for the QR PNG written to disk by the CLI (read once it exists and its
/// length is stable, to avoid grabbing a half-written file).
/// Returns `None` on timeout (the caller falls back to drawing the URL as a QR).
fn poll_qr_png(dir: &std::path::Path, timeout: Duration) -> Option<Vec<u8>> {
    let path = dir.join("qr.png");
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(bytes) = std::fs::read(&path) {
            if !bytes.is_empty() {
                std::thread::sleep(Duration::from_millis(200));
                if let Ok(settled) = std::fs::read(&path) {
                    if settled.len() == bytes.len() {
                        return Some(settled);
                    }
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// 单段:`auth init --noninteractive --no-browser` 长驻 → 抓 URL 出二维码 → 等进程退出 → 查 ready。
fn phase_scan(app: &AppHandle) -> Result<(), String> {
    // CLI 1.1.0's --output-qrcode only accepts a path relative to the current
    // directory, so the real auth QR PNG is written into a temp dir. The stdout
    // URL is the /ai/qc/gen landing page (which would ask the user to scan
    // again) and cannot be encoded as the QR directly; only the PNG QR reaches
    // authorization in one scan. Any CLI that reaches this point is ≥1.2.1
    // (wecom_ensure_cli's version gate force-replaces older ones), so the
    // self-drawn fallback only covers PNG write failure / poll timeout.
    let qr_dir = std::env::temp_dir().join(format!(
        "pinvou3-wecom-qr-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    if let Err(e) = std::fs::create_dir_all(&qr_dir) {
        // Not swallowed: spawn would also fail on the missing cwd, but its cause
        // would be misread as "CLI not installed".
        return Err(format!("failed to create the scan QR temp dir: {e}"));
    }
    let mut cmd = wecom(&[
        "auth",
        "init",
        "--noninteractive",
        "--no-browser",
        "--output-qrcode",
        "qr.png",
    ]);
    cmd.current_dir(&qr_dir);
    // 独立进程组:npm shim(shell→node)派生的孙进程与 shim 同组,退出收割的
    // kill_pid_tree 按负 pid 组杀整棵树,单杀 shim pid 会把 node 孤儿化。
    crate::platform::process::std_process_group_leader(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&qr_dir);
            return Err(format!(
                "failed to start wecom-cli auth init: {e} (install the WeCom CLI online first)"
            ));
        }
    };
    let conn = app.state::<ConnectorConn>();
    conn.set_pid(ID, Some(child.id()));

    // 排空 stdout+stderr,抓首个企微 URL(channel 送回)。主线程 tx 丢掉,
    // 两个管道都 EOF 后 rx 自动断开,不会永久阻塞。
    let (tx, rx) = mpsc::channel::<String>();
    if let Some(o) = child.stdout.take() {
        cc::drain_for_url(WECOM_CTX, o, tx.clone());
    }
    if let Some(e) = child.stderr.take() {
        cc::drain_for_url(WECOM_CTX, e, tx.clone());
    }
    drop(tx);

    let url = match rx.recv_timeout(Duration::from_secs(40)) {
        Ok(u) => u,
        Err(_) => {
            let _ = child.kill();
            cc::reap_after_kill(&mut child);
            conn.set_pid(ID, None);
            let _ = std::fs::remove_dir_all(&qr_dir);
            // Cancel tree-kills the child → pipe EOF lands here: the user stopped
            // on purpose, so finish silently instead of misreporting a link timeout.
            if conn.is_cancelled(ID) {
                return Ok(());
            }
            return Err("no QR link within 40s (check network / proxy)".into());
        }
    };
    // Prefer the real auth QR written by the CLI; on failure (write failure /
    // poll timeout) fall back to the self-drawn landing-page QR.
    let qr = poll_qr_png(&qr_dir, Duration::from_secs(6))
        .and_then(|bytes| cc::png_data_url(&bytes))
        .or_else(|| cc::make_qr(&url));
    let _ = std::fs::remove_dir_all(&qr_dir);
    // Cancel during the PNG wait window: exit silently. Emitting wecom:qr now
    // would re-open the scan modal the user already dismissed.
    // (wecom_cancel already tree-killed by pid; the extra kill + pid slot reset
    // here covers the race.)
    if conn.is_cancelled(ID) {
        let _ = child.kill();
        cc::reap_after_kill(&mut child);
        conn.set_pid(ID, None);
        return Ok(());
    }
    cc::emit(
        app,
        "wecom:qr",
        json!({ "phase": "authorize", "url": url, "qr_data_url": qr }),
    );

    // 等进程退出(用户扫码完成);期间轮询取消标志。退出后查 ready 收尾。
    loop {
        if conn.is_cancelled(ID) {
            let _ = child.kill();
            cc::reap_after_kill(&mut child);
            conn.set_pid(ID, None);
            return Ok(()); // 取消:静默
        }
        match child.try_wait() {
            Ok(Some(_status)) => {
                conn.set_pid(ID, None);
                if is_ready() {
                    cc::bundle_store_on_connected(ID);
                    cc::emit(app, "wecom:connected", json!({ "ok": true }));
                    return Ok(());
                }
                return Err("授权未完成(可能已取消或超时)".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(400)),
            Err(e) => {
                conn.set_pid(ID, None);
                return Err(format!("init 等待失败: {e}"));
            }
        }
    }
}

/// 取消连接:置取消标志 + tree-kill 当前长驻子进程。
pub async fn wecom_cancel(app: AppHandle) -> Result<Value, String> {
    let pid = app.state::<ConnectorConn>().cancel(ID);
    if let Some(pid) = pid {
        let _ = tokio::task::spawn_blocking(move || cc::kill_pid_tree(pid)).await;
    }
    Ok(json!({ "ok": true }))
}

/// wecom-cli 凭证目录(扫码后落盘在此)。
fn wecom_config_dir() -> std::path::PathBuf {
    crate::platform::os::user_home_dir()
        .join(".config")
        .join("wecom")
}

/// 断开企微:删凭证目录 `~/.config/wecom`(飞书是 `auth logout`,企微无 logout 子命令)。
pub async fn wecom_logout() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let dir = wecom_config_dir();
        let existed = dir.exists();
        let _ = std::fs::remove_dir_all(&dir);
        cc::bundle_store_on_disconnected(ID);
        Ok::<Value, String>(json!({ "ok": true, "removed": existed }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ─────────────────────── 企微域技能门控(对齐飞书 §八.4)───────────────────────
//
// 企微技能可见性 = `skills_dir` 里 wecomcli-* 目录在不在(引擎 SkillRegistry 扫目录)。
// 规则:**已连接(ready) 且 未手动停用** 才写技能;否则删掉(省 token / 关闭)。
// 手动停用标志:`~/.pinvou3/wecom_disabled` 文件存在 = 停用。与连接状态正交。

/// 按 visible 写 / 删企微技能文件(调 [`Pinvou3Bundle::apply_wecom_skills`])。
pub(crate) fn apply_bundle_skills(visible: bool) -> std::io::Result<()> {
    crate::features::runtime_bundle::platform::Pinvou3Bundle::paths().apply_wecom_skills(visible)
}

/// 企微门控表项:停用标志 + 就绪探测 + 技能落盘;
/// apply/skills_state 等命令公共体见 [`ConnectorGate`]。
pub(crate) static WECOM_GATE: ConnectorGate = ConnectorGate {
    id: "wecom",
    disabled_filename: "wecom_disabled",
    display_name: "企微",
    ready_probe: is_ready,
    apply_bundle_skills: apply_bundle_skills,
};

/// 按当前"应否可见"状态写 / 删技能文件。前端在连接成功 / 断开 / 切开关后调。
pub async fn wecom_apply_skills() -> Result<Value, String> {
    WECOM_GATE.apply_skills_command().await
}

/// 给前端渲染开关态:`{connected, enabled(=未停用), visible}`。
pub async fn wecom_skills_state() -> Result<Value, String> {
    WECOM_GATE.skills_state_command().await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--version` 输出 → 三段版本号。1.1.0 起输出带构建信息尾巴。
    /// 两段式按共享口径补 0(不因假想的「2.0」误判未装触发降级重装)。
    #[test]
    fn parses_wecom_versions() {
        assert_eq!(
            parse_wecom_version("wecom-cli 1.1.0 (wecom 2026-08-17T03:14:38Z 889c555)"),
            Some((1, 1, 0)),
        );
        assert_eq!(
            parse_wecom_version("wecom-cli 1.1.0 (wecom 2026-08-17T03:14:38Z 889c555)\r\n"),
            Some((1, 1, 0)),
        );
        assert_eq!(parse_wecom_version("wecom-cli 0.1.9"), Some((0, 1, 9)));
        assert_eq!(parse_wecom_version("wecom-cli 2.0"), Some((2, 0, 0)));
        assert_eq!(parse_wecom_version("hello"), None);
        // 遵守 parse_semver3「调用方先切片」契约:程序名外的数字噪声不当版本。
        assert_eq!(parse_wecom_version("error: something 404"), None);
        assert_eq!(parse_wecom_version("node 22 wecom-cli"), None);
    }

    /// `auth show --status` 输出 → 已授权判定(仅整行 authorized,大小写不敏感)。
    #[test]
    fn status_is_authorized_detects_authorization() {
        assert!(status_is_authorized("authorized"));
        assert!(status_is_authorized("authorized\n"));
        assert!(status_is_authorized("authorized\r\n")); // Windows npm shim 的 CRLF
        assert!(status_is_authorized("  Authorized "));
        assert!(!status_is_authorized("unauthorized")); // 前缀相同不能误判
        assert!(!status_is_authorized("Status: unauthorized"));
        assert!(!status_is_authorized(""));
    }

    /// Polls the QR PNG written by the CLI: read once it appears; a missing
    /// file / empty dir waits until timeout and returns None.
    #[test]
    fn poll_qr_png_reads_written_file_and_times_out() {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-wecom-poll-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // No file → timeout None (a very short timeout keeps the test fast)
        assert!(poll_qr_png(&dir, Duration::from_millis(300)).is_none());
        // After the file lands → stable bytes are read
        let png = b"\x89PNG\r\n\x1a\npayload";
        std::fs::write(dir.join("qr.png"), png).unwrap();
        assert_eq!(
            poll_qr_png(&dir, Duration::from_secs(3)).as_deref(),
            Some(&png[..])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
