//! 飞书(Lark)CLI 接入 —— 启动引导 + 鉴权编排。
//!
//! 路线:`lark-cli`(飞书官方 CLI)+ 官方域技能(bundle 在
//! `~/.pinvou3/bundle/skills/lark-*`,见 `bridge::bundle`)。模型通过 shell 跑
//! `lark-cli <域> ...`,技能渐进披露教它用法。
//!
//! 公共的"起子进程 / 抑黑窗 / 抓 URL / 出二维码 / 收发事件 / 取消"逻辑见
//! [`crate::features::connectors::connector_cli`](开发方案 C 抽公共管道);本文件只留飞书特有的薄声明
//! [`FEISHU_CTX`] + 两段连接编排 + 技能门控。
//!
//! 凭证模型(用户零找 key):pinvou3 作为产品方**一次性**用自己的飞书 app
//! (app-id/secret)`config init`,用户只走浏览器 OAuth(`auth login` device flow)。
//!
//! ⚠️ C 端注意:app secret 当前从环境变量读、随 `config init` 落到本机 lark-cli 配置。
//! 生产应迁到安全配置 / 后端代理(secret 不落客户端)。

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::features::connectors::connector_cli::{self as cc, CliCtx, ConnectorConn};
use crate::features::connectors::skill_gate::ConnectorSkillGate;

/// 连接器 id(事件前缀 + ConnectorConn 槽位键)。
const ID: &str = "feishu";

/// 飞书 CLI 薄声明。`LARK_CLI_NO_PROXY=1`:飞书是国内站,被 Clash 等代理走国外
/// 节点会 EOF,设了让 lark-cli 直连。
const FEISHU_CTX: CliCtx = CliCtx {
    cli_bin: "lark-cli",
    envs: &[("LARK_CLI_NO_PROXY", "1")],
    auth_domains: &["feishu", "larksuite"],
};

/// `lark-cli <args>`(抑黑窗 + env)。
fn lark(args: &[&str]) -> Command {
    FEISHU_CTX.cli(args)
}

/// lark-cli 是否已在 PATH(快速,~秒级)。
fn lark_cli_present() -> bool {
    matches!(cc::run(lark(&["--version"])), Ok((true, _, _)))
}

/// `auth status` 里用户身份是否 ready(已授权)。
fn is_user_ready() -> bool {
    if let Ok((_, so, se)) = cc::run(lark(&["auth", "status", "--json"])) {
        let p = cc::parse_json(&so).or_else(|| cc::parse_json(&se));
        return p
            .as_ref()
            .and_then(|v| v.pointer("/identities/user/status"))
            .and_then(|v| v.as_str())
            .map(|s| s == "ready")
            .unwrap_or(false);
    }
    false
}

// ───────────────────────────── Tauri commands ─────────────────────────────

/// 引导:首次使用时下载并校验锁定版本的 lark-cli，已装则秒返回。
pub async fn feishu_ensure_cli() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let t = std::time::Instant::now();
        // 已装则秒返回 —— 不跑慢吞吞、可能卡死的 npx install。
        let present = lark_cli_present();
        eprintln!(
            "[feishu] ensure_cli: lark_cli_present={present} in {}ms",
            t.elapsed().as_millis()
        );
        if present {
            return Ok::<Value, String>(json!({ "ok": true, "already": true }));
        }
        crate::features::connectors::native_installer::ensure_native_cli("lark-cli")?;
        if !lark_cli_present() {
            return Err("飞书 CLI 安装完成但无法执行，请重试".to_string());
        }
        Ok::<Value, String>(json!({ "ok": true, "already": false }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 查询当前飞书连接状态:`lark-cli auth status --json`。
/// 返回 lark-cli 的原始 JSON(含 appId / identities.user.status 等);未配置 app
/// 或未登录则 connected=false。未装 CLI 时返回结构化 `installed:false`
/// (与 wecom/dingtalk/tmeet 一致),不向消费方抛 Err。
pub async fn feishu_status() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        // 没装就别 spawn auth status —— 未装用户每次白等子进程且拿到的是 Err,
        // 统一返回结构化未装态(其余 CLI 连接器同款短路)。
        if !lark_cli_present() {
            return Ok::<Value, String>(json!({
                "ok": false, "connected": false, "configured": false, "installed": false
            }));
        }
        let (ok, so, se) = cc::run(lark(&["auth", "status", "--json"]))?;
        let parsed = cc::parse_json(&so).or_else(|| cc::parse_json(&se));
        let connected = parsed
            .as_ref()
            .and_then(|v| v.pointer("/identities/user/status"))
            .and_then(|v| v.as_str())
            .map(|s| s == "ready")
            .unwrap_or(false);
        // 是否已配过 app:看 auth status 里有没有非空 appId。
        let configured = parsed
            .as_ref()
            .and_then(|v| v.get("appId"))
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        // 只回布尔:auth status 的 raw/stderr 可能含身份/凭证信息,不进 webview
        Ok::<Value, String>(json!({
            "ok": ok,
            "connected": connected,
            "configured": configured,
            "installed": true,
        }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 开始连接飞书(`config init --new` 自建 app,两段扫码):
/// 段① `config init --new` 长驻 → 抓二维码 URL(emit `feishu:qr` phase=register)→ 用户扫码注册 app。
/// 段② `auth login --recommend` → 二维码(emit phase=authorize)→ 轮询 device-code → user:ready。
/// 进度全程走事件:`feishu:qr` / `feishu:phase` / `feishu:connected` / `feishu:error`。
/// 立即返回 `{started:true}`;前端 listen 事件驱动 UI。
pub async fn feishu_connect_begin(app: AppHandle) -> Result<Value, String> {
    app.state::<ConnectorConn>().reset(ID);
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || run_connect_flow(&app2));
    Ok(json!({ "started": true }))
}

/// 编排:段① 注册 app → 段② 授权用户。任一段出错 / 取消即停,错误经事件上报。
fn run_connect_flow(app: &AppHandle) {
    match phase_register(app) {
        Ok(true) => {}
        Ok(false) => return, // 取消,静默
        Err(e) => {
            cc::emit(
                app,
                "feishu:error",
                json!({ "phase": "register", "message": e }),
            );
            return;
        }
    }
    if let Err(e) = phase_authorize(app) {
        cc::emit(
            app,
            "feishu:error",
            json!({ "phase": "authorize", "message": e }),
        );
    }
}

/// 段①:`config init --new` 长驻 → 抓 URL 出二维码 → 等用户扫码完成(进程退出)。
/// 返回 Ok(true)=注册成功;Ok(false)=被取消;Err=失败。
fn phase_register(app: &AppHandle) -> Result<bool, String> {
    let mut cmd = lark(&["config", "init", "--new"]);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("config init --new 启动失败: {e}(需要先完成飞书 CLI 在线安装)"))?;
    let conn = app.state::<ConnectorConn>();
    conn.set_pid(ID, Some(child.id()));

    // 排空 stdout+stderr,抓首个飞书 URL(channel 送回)。主线程的 tx 丢掉,
    // 这样两个管道都 EOF 后 rx 自动断开,不会永久阻塞。
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    if let Some(o) = child.stdout.take() {
        cc::drain_for_url(FEISHU_CTX, o, tx.clone());
    }
    if let Some(e) = child.stderr.take() {
        cc::drain_for_url(FEISHU_CTX, e, tx.clone());
    }
    drop(tx);

    let url = match rx.recv_timeout(Duration::from_secs(40)) {
        Ok(u) => u,
        Err(_) => {
            let _ = child.kill();
            conn.set_pid(ID, None);
            return Err("注册:40s 内未拿到二维码链接(检查网络 / 代理)".into());
        }
    };
    let qr = cc::make_qr(&url);
    cc::emit(
        app,
        "feishu:qr",
        json!({ "phase": "register", "url": url, "qr_data_url": qr }),
    );

    // 等进程退出(用户扫码完成);期间轮询取消标志。
    loop {
        if conn.is_cancelled(ID) {
            let _ = child.kill();
            conn.set_pid(ID, None);
            return Ok(false);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                conn.set_pid(ID, None);
                if !status.success() {
                    return Err("注册应用未完成(可能已取消或超时)".into());
                }
                cc::emit(app, "feishu:phase", json!({ "phase": "registered" }));
                return Ok(true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(400)),
            Err(e) => {
                conn.set_pid(ID, None);
                return Err(format!("config init 等待失败: {e}"));
            }
        }
    }
}

/// 段②:`auth login --no-wait --json --recommend` 拿 URL+device_code → 二维码 →
/// 轮询 `auth login --device-code`(兼容它阻塞或立即返回)直到 user:ready / 超时。
fn phase_authorize(app: &AppHandle) -> Result<(), String> {
    let (_ok, so, se) = cc::run(lark(&[
        "auth",
        "login",
        "--no-wait",
        "--json",
        "--recommend",
    ]))?;
    let p = cc::parse_json(&so)
        .or_else(|| cc::parse_json(&se))
        .unwrap_or(Value::Null);
    let url = [
        "verification_uri_complete",
        "verification_url",
        "verificationUrl",
        "url",
    ]
    .iter()
    .find_map(|k| p.get(*k).and_then(|v| v.as_str()))
    .map(String::from)
    .ok_or("auth login 未返回授权链接")?;
    let device_code = ["device_code", "deviceCode"]
        .iter()
        .find_map(|k| p.get(*k).and_then(|v| v.as_str()))
        .map(String::from)
        .ok_or("auth login 未返回 device_code")?;
    let qr = cc::make_qr(&url);
    cc::emit(
        app,
        "feishu:qr",
        json!({ "phase": "authorize", "url": url, "qr_data_url": qr }),
    );

    let start = Instant::now();
    let conn = app.state::<ConnectorConn>();
    loop {
        if conn.is_cancelled(ID) {
            return Ok(()); // 取消:静默(run_connect_flow 不再 emit)
        }
        if start.elapsed() > Duration::from_secs(300) {
            return Err("授权超时(5 分钟内未完成扫码)".into());
        }
        std::thread::sleep(Duration::from_secs(3));
        // 这步可能阻塞到完成、也可能立即返回 pending —— 两种都兼容,靠 auth status 判 ready。
        let _ = cc::run(lark(&[
            "auth",
            "login",
            "--device-code",
            &device_code,
            "--json",
        ]));
        if is_user_ready() {
            cc::emit(app, "feishu:connected", json!({ "ok": true }));
            return Ok(());
        }
    }
}

/// 取消连接:置取消标志 + tree-kill 当前长驻子进程(关二维码弹窗 / 超时时调)。
pub async fn feishu_cancel(app: AppHandle) -> Result<Value, String> {
    let pid = app.state::<ConnectorConn>().cancel(ID);
    if let Some(pid) = pid {
        let _ = tokio::task::spawn_blocking(move || cc::kill_pid_tree(pid)).await;
    }
    Ok(json!({ "ok": true }))
}

/// 断开飞书:`lark-cli auth logout`(清 token)。
pub async fn feishu_logout() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let (ok, so, se) = cc::run(lark(&["auth", "logout"]))?;
        Ok::<Value, String>(json!({ "ok": ok, "stdout": so, "stderr": se }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ─────────────────────── 飞书技能门控(§八.4 + composer 开关)───────────────────────
//
// 飞书技能可见性 = `skills_dir` 里 lark 目录在不在(引擎 SkillRegistry 扫目录)。
// 规则:**已连接(user:ready) 且 未手动停用** 才写技能;否则删掉(省 token / 关闭)。
// 手动停用标志:`~/.pinvou3/feishu_disabled` 文件存在 = 停用。与连接状态正交。

/// 飞书技能门控:停用标志文件机制走 [`ConnectorSkillGate`] 默认实现,
/// `apply_skills` 指向 `apply_feishu_skills`。
struct FeishuGate;
impl ConnectorSkillGate for FeishuGate {
    fn id(&self) -> &'static str {
        ID
    }
    fn disabled_filename(&self) -> &'static str {
        "feishu_disabled"
    }
    fn apply_skills(&self, visible: bool) -> Result<(), String> {
        crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
            .apply_feishu_skills(visible)
            .map_err(|e| format!("更新飞书技能失败: {e}"))
    }
}
const GATE: FeishuGate = FeishuGate;

/// 用户是否手动停用了飞书技能。
pub fn is_feishu_disabled() -> bool {
    GATE.is_disabled()
}

fn set_feishu_disabled_flag(disabled: bool) -> Result<(), String> {
    GATE.set_disabled_flag(disabled)
}

/// 飞书技能此刻该不该出现在 skills_dir:**未手动停用 且 已连接**。
/// 启动时(bundle)与命令里都用它判定。注:会 spawn lark-cli 查 auth status(未装则 false)。
pub fn feishu_skills_should_show() -> bool {
    !is_feishu_disabled() && is_user_ready()
}

/// 按当前"应否可见"状态写 / 删技能文件,并广播刷新在跑会话(当前对话即时生效)。
/// 前端在 **连接成功 / 断开 / 切开关** 后调,统一收口。
pub async fn feishu_apply_skills() -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(|| -> Result<bool, String> {
        let show = feishu_skills_should_show();
        GATE.apply_skills(show)?;
        Ok(show)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))??;
    // 技能写盘即可——连接成功弹窗已引导「新建对话」,新会话 spawn 时自然扫到飞书技能;
    // 不再原地广播刷新当前对话(故不依赖子模块 Op::RefreshSystemPrompt)。
    Ok(json!({ "visible": show }))
}

/// composer 飞书开关:`enabled` → 写停用标志 → 按规则增删技能 → 广播刷新。
///
/// 注:停用标志写盘此前用 `let _ =` 静默忽略失败,现统一为 `Result` 传播
/// (Wave 1 批准的契约面变更)。
pub async fn set_feishu_enabled(enabled: bool) -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        set_feishu_disabled_flag(!enabled)?;
        let show = feishu_skills_should_show();
        GATE.apply_skills(show)?;
        Ok(show)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))??;
    Ok(json!({ "ok": true, "visible": show }))
}

/// 给前端渲染开关态:`{connected, enabled(=未停用), visible(=connected&&enabled)}`。
pub async fn feishu_skills_state() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let disabled = is_feishu_disabled();
        let connected = is_user_ready();
        Ok::<Value, String>(json!({
            "connected": connected,
            "enabled": !disabled,
            "visible": connected && !disabled,
        }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}
