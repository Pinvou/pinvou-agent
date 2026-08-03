//! 企业微信(`@wecom/cli`,腾讯官方·MIT)CLI 连接器 —— 启动引导 + 扫码鉴权。
//!
//! 路线同飞书([`crate::features::connectors::feishu`]):官方 CLI + 官方域技能,riding 在腾讯官方 app 上,
//! **纯扫码**接入(不需管理员建自建应用、不需手填 CorpID/Secret)。
//! 公共管道见 [`crate::features::connectors::connector_cli`];本文件只有企微特有的薄声明 + 单段连接编排。
//!
//! 连接:`wecom-cli init --noninteractive --no-open` 长驻 → 抓二维码 URL → 用户扫码 →
//! 进程退出后 `auth show` 判 ready。进度走事件 `wecom:qr` / `wecom:connected` / `wecom:error`。
//! 凭证落 `~/.config/wecom`(Win:`%USERPROFILE%\.config\wecom`),断开即删该目录。

use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::features::connectors::connector_cli::{self as cc, CliCtx, ConnectorConn};

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

fn wecom(args: &[&str]) -> std::process::Command {
    WECOM_CTX.cli(args)
}

/// wecom-cli 是否已在 PATH(快速,~秒级)。
fn wecom_cli_present() -> bool {
    matches!(cc::run(wecom(&["--version"])), Ok((true, _, _)))
}

/// `wecom-cli auth show` 输出里是否含非空 `id`(= 已授权)。
/// 等价 WorkBuddy cli.json 的 `statusMatch: "id"\s*:\s*"`。
fn status_has_id(s: &str) -> bool {
    if let Some(v) = cc::parse_json(s) {
        if let Some(id) = v.get("id").and_then(|v| v.as_str()) {
            return !id.is_empty();
        }
    }
    // 回退:去掉空白后子串匹配(输出非纯 JSON 时)。
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("\"id\":\"")
}

/// `auth show` 判当前是否已连接(已授权)。会 spawn wecom-cli。
fn is_ready() -> bool {
    if let Ok((_, so, se)) = cc::run(wecom(&["auth", "show"])) {
        return status_has_id(&so) || status_has_id(&se);
    }
    false
}

// ───────────────────────────── Tauri commands ─────────────────────────────

/// 引导:确保 wecom-cli 装好(全局 shim 在 PATH 上),幂等。已装则秒返回。
/// 未装则 `npm install -g @wecom/cli`,带 180s 超时防卡死(网络 / 代理)。需要 Node。
pub async fn wecom_ensure_cli() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        if wecom_cli_present() {
            return Ok::<Value, String>(json!({ "ok": true, "already": true }));
        }
        let mut c = WECOM_CTX.base_cmd("npm");
        cc::apply_user_npm_prefix(&mut c);
        c.args(["install", "-g", "@wecom/cli"]);
        let mut ok = cc::run_with_timeout(c, 180)?;
        if ok && !wecom_cli_present() {
            ok = false;
        }
        Ok::<Value, String>(json!({ "ok": ok, "already": false }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 查询当前企微连接状态:`wecom-cli auth show`。未装则 `installed:false`。
pub async fn wecom_status() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        // 没装就别 spawn auth show —— 省掉没装连接器的用户每次白等一次子进程。
        if !wecom_cli_present() {
            return Ok::<Value, String>(json!({
                "ok": false, "connected": false, "installed": false
            }));
        }
        let (ok, so, se) = cc::run(wecom(&["auth", "show"]))?;
        let connected = status_has_id(&so) || status_has_id(&se);
        // 只回布尔:auth show 的 stdout/stderr 含身份信息,不进 webview
        Ok::<Value, String>(json!({
            "ok": ok, "connected": connected, "installed": true
        }))
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

/// 单段:`init --noninteractive --no-open` 长驻 → 抓 URL 出二维码 → 等进程退出 → 查 ready。
fn phase_scan(app: &AppHandle) -> Result<(), String> {
    let mut cmd = wecom(&["init", "--noninteractive", "--no-open"]);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        format!("wecom-cli init 启动失败: {e}(需要 wecom-cli；支持的平台会优先使用随包内置 CLI,其余走 npm 全局安装)")
    })?;
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
            conn.set_pid(ID, None);
            return Err("40s 内未拿到二维码链接(检查网络 / 代理)".into());
        }
    };
    cc::emit(
        app,
        "wecom:qr",
        json!({ "phase": "authorize", "url": url, "qr_data_url": cc::make_qr(&url) }),
    );

    // 等进程退出(用户扫码完成);期间轮询取消标志。退出后查 ready 收尾。
    loop {
        if conn.is_cancelled(ID) {
            let _ = child.kill();
            conn.set_pid(ID, None);
            return Ok(()); // 取消:静默
        }
        match child.try_wait() {
            Ok(Some(_status)) => {
                conn.set_pid(ID, None);
                if is_ready() {
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
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    std::path::Path::new(&home).join(".config").join("wecom")
}

/// 断开企微:删凭证目录 `~/.config/wecom`(飞书是 `auth logout`,企微无 logout 子命令)。
pub async fn wecom_logout() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let dir = wecom_config_dir();
        let existed = dir.exists();
        let _ = std::fs::remove_dir_all(&dir);
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

fn wecom_disabled_path() -> std::path::PathBuf {
    crate::platform::paths::pinvou3_home().join("wecom_disabled")
}

pub fn is_wecom_disabled() -> bool {
    wecom_disabled_path().exists()
}

fn set_wecom_disabled_flag(disabled: bool) {
    let p = wecom_disabled_path();
    if disabled {
        let _ = std::fs::write(&p, b"1");
    } else {
        let _ = std::fs::remove_file(&p);
    }
}

/// 企微技能此刻该不该出现在 skills_dir:**未手动停用 且 已连接**。
/// 注:会 spawn wecom-cli 查 auth show(未装则 false)。
pub fn wecom_skills_should_show() -> bool {
    !is_wecom_disabled() && is_ready()
}

/// 按当前"应否可见"状态写 / 删技能文件。前端在连接成功 / 断开 / 切开关后调。
pub async fn wecom_apply_skills() -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(|| {
        let show = wecom_skills_should_show();
        let _ = crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
            .apply_wecom_skills(show);
        show
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?;
    Ok(json!({ "visible": show }))
}

/// composer 企微开关:写停用标志 → 按规则增删技能。
pub async fn set_wecom_enabled(enabled: bool) -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(move || {
        set_wecom_disabled_flag(!enabled);
        let show = wecom_skills_should_show();
        let _ = crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
            .apply_wecom_skills(show);
        show
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?;
    Ok(json!({ "ok": true, "visible": show }))
}

/// 给前端渲染开关态:`{connected, enabled(=未停用), visible}`。
pub async fn wecom_skills_state() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let disabled = is_wecom_disabled();
        let connected = is_ready();
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

    /// `auth show` 输出 → 已授权判定。空 id 不能被回退子串匹配误判为已连接。
    #[test]
    fn status_has_id_detects_authorization() {
        // 纯 JSON,非空 id = 已授权
        assert!(status_has_id(r#"{"id":"abc-123"}"#));
        // 纯 JSON,空 id = 未授权
        assert!(!status_has_id(r#"{"id":""}"#));
        // 无 id 字段 = 未授权
        assert!(!status_has_id(r#"{"create_time":1}"#));
        // 非纯 JSON,但含 "id":"x" 子串 → 回退匹配命中
        assert!(status_has_id("noise {\"id\": \"x\"} tail"));
        // 空输出 = 未授权
        assert!(!status_has_id(""));
    }

    /// 手动停用标志:文件存在=停用,与连接状态正交。写/删一轮。
    #[test]
    fn wecom_disabled_flag_roundtrip() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = format!(
            "{}/pinvou3-wecom-test-{}",
            std::env::temp_dir().display(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::create_dir_all(crate::platform::paths::pinvou3_home());

        // 默认(无文件)= 未停用
        set_wecom_disabled_flag(false);
        assert!(!is_wecom_disabled());
        // 置停用 → 文件在 → 停用
        set_wecom_disabled_flag(true);
        assert!(is_wecom_disabled());
        // 复位 → 文件删 → 未停用
        set_wecom_disabled_flag(false);
        assert!(!is_wecom_disabled());

        std::env::remove_var("PINVOU3_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
