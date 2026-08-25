//! 企业微信(`@wecom/cli`,腾讯官方·MIT)CLI 连接器 —— 启动引导 + 扫码鉴权。
//!
//! 路线同飞书([`crate::features::connectors::feishu`]):官方 CLI + 官方域技能,riding 在腾讯官方 app 上,
//! **纯扫码**接入(不需管理员建自建应用、不需手填 CorpID/Secret)。
//! 公共管道见 [`crate::features::connectors::connector_cli`];本文件只有企微特有的薄声明 + 单段连接编排。
//!
//! 连接(wecom-cli ≥1.1.0):`wecom-cli auth init --noninteractive --no-browser` 长驻 →
//! 抓二维码 URL → 用户扫码 → 进程退出后 `auth show --status` 判 ready。
//! 进度走事件 `wecom:qr` / `wecom:connected` / `wecom:error`。
//! 凭证落 `~/.config/wecom`(Win:`%USERPROFILE%\.config\wecom`),断开即删该目录。
//! 1.1.0 起命令模型重构(`msg`→`message`、`schedule`→`calendar`、入参改 flags),
//! 技能与判定都以 1.1.0 为基线,故 [`WECOM_MIN_VERSION`] 以下的旧安装会被替换升级。

use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::features::connectors::connector_cli::{self as cc, CliCtx, ConnectorConn};
use crate::features::connectors::skill_gate::ConnectorSkillGate;

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

/// 技能与鉴权命令面基线:1.1.0 重构了命令模型(`init`→`auth init`、
/// `msg`→`message`、`schedule`→`calendar`、入参 JSON→flags),旧安装必须替换。
const WECOM_MIN_VERSION: (u64, u64, u64) = (1, 1, 0);

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

/// wecom-cli 是否已装且 ≥ 1.1.0(命令模型基线);旧版视为未装,触发在线替换升级。
fn wecom_cli_present() -> bool {
    wecom_cli_version()
        .map(|v| v >= WECOM_MIN_VERSION)
        .unwrap_or(false)
}

/// `wecom-cli auth show --status` 是否输出 `authorized`(已扫码授权)。
fn status_is_authorized(s: &str) -> bool {
    s.trim().eq_ignore_ascii_case("authorized")
}

/// `auth show --status` 判当前是否已连接(已授权)。会 spawn wecom-cli。
fn is_ready() -> bool {
    if let Ok((ok, so, se)) = cc::run(wecom(&["auth", "show", "--status"])) {
        return ok && (status_is_authorized(&so) || status_is_authorized(&se));
    }
    false
}

// ───────────────────────────── Tauri commands ─────────────────────────────

/// 引导:首次使用(或安装低于 1.1.0 命令模型基线)时下载并校验锁定版本的 wecom-cli,
/// 已装且达标则秒返回;托管目录中的旧版按 lock 哈希不一致直接替换升级。
pub async fn wecom_ensure_cli() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        if wecom_cli_present() {
            return Ok::<Value, String>(json!({ "ok": true, "already": true }));
        }
        crate::features::connectors::native_installer::ensure_native_cli("wecom-cli")?;
        if !wecom_cli_present() {
            return Err("企微 CLI 安装完成但无法执行，请重试".to_string());
        }
        Ok::<Value, String>(json!({ "ok": true, "already": false }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 查询当前企微连接状态:`wecom-cli auth show --status`。
/// 装了但低于 [`WECOM_MIN_VERSION`] 时回 `upgrade_required:true`(tmeet 同款三态;
/// 前端 ToolStoreView 暂只读 connected,该字段待 tmeet/wecom 统一做「待升级」UI 后消费)。
pub async fn wecom_status() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        // 没装就别 spawn auth show —— 省掉没装连接器的用户每次白等一次子进程;
        // 装了则同一次 --version 判 installed 与 upgrade_required 两态,不重复 spawn。
        match wecom_cli_version() {
            None => Ok::<Value, String>(json!({
                "ok": false, "connected": false, "installed": false, "upgrade_required": false
            })),
            Some(v) if v < WECOM_MIN_VERSION => Ok::<Value, String>(json!({
                "ok": false, "connected": false, "installed": true, "upgrade_required": true
            })),
            Some(_) => {
                let (ok, so, se) = cc::run(wecom(&["auth", "show", "--status"]))?;
                let connected = ok && (status_is_authorized(&so) || status_is_authorized(&se));
                // 只回布尔:--status 单行输出虽不含身份信息,保持最小回传面
                Ok::<Value, String>(json!({
                    "ok": ok, "connected": connected, "installed": true, "upgrade_required": false
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

/// 单段:`auth init --noninteractive --no-browser` 长驻 → 抓 URL 出二维码 → 等进程退出 → 查 ready。
fn phase_scan(app: &AppHandle) -> Result<(), String> {
    let mut cmd = wecom(&["auth", "init", "--noninteractive", "--no-browser"]);
    // 独立进程组:npm shim(shell→node)派生的孙进程与 shim 同组,退出收割的
    // kill_pid_tree 按负 pid 组杀整棵树,单杀 shim pid 会把 node 孤儿化。
    crate::platform::process::std_process_group_leader(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("wecom-cli auth init 启动失败: {e}(需要先完成企微 CLI 在线安装)"))?;
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

/// 企微技能门控:停用标志文件机制走 [`ConnectorSkillGate`] 默认实现,
/// `apply_skills` 指向 `apply_wecom_skills`。
struct WecomGate;
impl ConnectorSkillGate for WecomGate {
    fn id(&self) -> &'static str {
        ID
    }
    fn disabled_filename(&self) -> &'static str {
        "wecom_disabled"
    }
    fn apply_skills(&self, visible: bool) -> Result<(), String> {
        crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
            .apply_wecom_skills(visible)
            .map_err(|e| format!("更新企微技能失败: {e}"))
    }
}
const GATE: WecomGate = WecomGate;

pub fn is_wecom_disabled() -> bool {
    GATE.is_disabled()
}

fn set_wecom_disabled_flag(disabled: bool) -> Result<(), String> {
    GATE.set_disabled_flag(disabled)
}

/// 企微技能此刻该不该出现在 skills_dir:**未手动停用 且 已连接**。
/// 注:会 spawn wecom-cli 查 auth show(未装则 false)。
pub fn wecom_skills_should_show() -> bool {
    !is_wecom_disabled() && is_ready()
}

/// 按当前"应否可见"状态写 / 删技能文件。前端在连接成功 / 断开 / 切开关后调。
pub async fn wecom_apply_skills() -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(|| -> Result<bool, String> {
        let show = wecom_skills_should_show();
        GATE.apply_skills(show)?;
        Ok(show)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))??;
    // scope 门禁同步：见 feishu_apply_skills 同名注释（code 默认关语义对齐）。
    if show {
        crate::features::marketplace::sync_deny_all_scopes_after_install("wecom");
    }
    Ok(json!({ "visible": show }))
}

/// composer 企微开关:写停用标志 → 按规则增删技能。
///
/// 注:停用标志写盘此前用 `let _ =` 静默忽略失败,现统一为 `Result` 传播
/// (Wave 1 批准的契约面变更)。
pub async fn set_wecom_enabled(enabled: bool) -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        set_wecom_disabled_flag(!enabled)?;
        // 停用标志 ↔ 统一禁用集桥接（见 set_feishu_enabled 同名注释）。
        crate::features::marketplace::sync_disabled_bundles_for_connector_switch("wecom", enabled);
        let show = wecom_skills_should_show();
        GATE.apply_skills(show)?;
        Ok(show)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))??;
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
        set_wecom_disabled_flag(false).unwrap();
        assert!(!is_wecom_disabled());
        // 置停用 → 文件在 → 停用
        set_wecom_disabled_flag(true).unwrap();
        assert!(is_wecom_disabled());
        // 复位 → 文件删 → 未停用
        set_wecom_disabled_flag(false).unwrap();
        assert!(!is_wecom_disabled());

        std::env::remove_var("PINVOU3_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
