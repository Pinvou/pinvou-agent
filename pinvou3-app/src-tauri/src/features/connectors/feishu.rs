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

use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use crate::features::connectors::connector_cli::{self as cc, CliCtx, ConnectorConn};
use crate::features::connectors::skill_gate::ConnectorGate;

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

/// Minimum acceptable lark-cli version for the skill pack's command surface.
/// The skill pack documents the lock baseline (1.0.95, see NOTICE.md); older
/// installs are missing dozens of taught shortcuts (e.g. `+get`/`+table-copy`),
/// so they count as not installed and trigger the locked-version replacement —
/// same policy as wecom/tmeet, which carry per-connector minimums and lark
/// historically did not (installed 1.0.65 silently degraded to generic help).
const LARK_MIN_VERSION: (u64, u64, u64) = (1, 0, 95);

/// 解析 `lark-cli --version` 输出为三段语义版本。输出形如
/// `lark-cli version 1.0.65`(程序名与版本之间夹着字面量 `version`),故按
/// 「首个可解析为 semver3 的空白分隔 token」取值,顺序保证版本号先于任何
/// 构建元数据出现。
fn parse_lark_version(s: &str) -> Option<(u64, u64, u64)> {
    s.split_whitespace().find_map(cc::parse_semver3)
}

/// Installed lark-cli version, if the `--version` probe runs and parses.
fn lark_cli_version() -> Option<(u64, u64, u64)> {
    let Ok((ok, so, se)) = cc::run(lark(&["--version"])) else {
        return None;
    };
    if !ok {
        return None;
    }
    parse_lark_version(&so).or_else(|| parse_lark_version(&se))
}

/// Whether lark-cli is installed at or above [`LARK_MIN_VERSION`]; older
/// installs count as not installed and trigger the online replacement.
/// Failure states (spawn error, non-zero probe) fold into "unavailable",
/// mirroring `lark_cli_probe`'s disconnect-path folding.
fn lark_cli_present() -> bool {
    lark_cli_version()
        .map(|v| v >= LARK_MIN_VERSION)
        .unwrap_or(false)
}

/// `--version` three-state probe, mirroring DingTalk's `dws_cli_probe`:
/// `Ok(true)` installed and usable; `Ok(false)` installed but the version probe exited
/// non-zero; `Err(ProbeError)` classified as Spawn/Timeout/Other. The disconnect path
/// must receive the [`cc::logout_probe_verdict`] verdict before it may degrade to
/// not-installed.
fn lark_cli_probe() -> Result<bool, cc::ProbeError> {
    cc::run_probe(lark(&["--version"])).map(|(ok, _, _)| ok)
}

/// `auth status` 里用户身份是否 ready(已授权)。
pub(crate) fn is_user_ready() -> bool {
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

/// 引导:首次使用、或已装 CLI 低于 [`LARK_MIN_VERSION`] 时,下载并校验锁定
/// 版本的 lark-cli;满足最低版本的已装 CLI 秒返回。托管目录内的旧版 CLI 在
/// lock 哈希不匹配时被整体替换(wecom 同款)。
pub async fn feishu_ensure_cli() -> Result<Value, String> {
    cc::ensure_cli_with(
        "feishu",
        lark_cli_present,
        "飞书 CLI 安装完成但无法执行，请重试",
        || crate::features::connectors::native_installer::ensure_native_cli("lark-cli"),
    )
    .await
}

/// 查询当前飞书连接状态:`lark-cli auth status --json`。
/// 返回 lark-cli 的原始 JSON(含 appId / identities.user.status 等);未配置 app
/// 或未登录则 connected=false。未装 CLI 时返回结构化 `installed:false`
/// (与 wecom/dingtalk/tmeet 一致),不向消费方抛 Err。
/// (Only called internally by the command layer's `bundle_readiness` CLI dispatch; there is no standalone Tauri command anymore.)
pub async fn feishu_status() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        // 没装就别 spawn auth status —— 未装用户每次白等子进程且拿到的是 Err,
        // 统一返回结构化未装态(其余 CLI 连接器同款短路)。installed 只看
        // 「是否装了任意版本」:过旧版本同样报 installed:true,升级引导由
        // ensure_cli 的最低版本门负责(wecom 同款分工)。
        if lark_cli_version().is_none() {
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
/// 进度全程走事件:`feishu:qr` / `feishu:connected` / `feishu:error`。
/// 立即返回 `{started:true}`;前端 listen 事件驱动 UI。
pub async fn feishu_connect_begin(app: AppHandle) -> Result<Value, String> {
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

/// 编排:段① 注册 app → 段② 授权用户。任一段出错 / 取消即停,错误经事件上报。
fn run_connect_flow(app: &AppHandle, generation: u64) {
    let conn = app.state::<ConnectorConn>();
    match phase_register(app, generation) {
        Ok(true) => {}
        Ok(false) => return, // 取消,静默
        Err(e) => {
            // The card renders a localized category message only; the raw cause
            // is logged here (stdout in dev runs, the app log in packaged builds).
            log::warn!("[feishu] register phase failed: {e}");
            // reset() clears the cancelled flag, so the flag alone cannot stop a
            // late emit in the cancel-then-reconnect window; a cancelled or
            // superseded round stays silent instead of polluting the new card.
            if conn.is_cancelled(ID) || conn.flow_stale(ID, generation) {
                return; // superseded by a newer round: stay silent
            }
            cc::emit(
                app,
                "feishu:error",
                json!({ "phase": "register", "code": "registration_failed", "message": e }),
            );
            return;
        }
    }
    if let Err(e) = phase_authorize(app, generation) {
        log::warn!("[feishu] authorize phase failed: {e}");
        if conn.is_cancelled(ID) || conn.flow_stale(ID, generation) {
            return; // superseded by a newer round: stay silent
        }
        cc::emit(
            app,
            "feishu:error",
            json!({ "phase": "authorize", "code": "auth_failed", "message": e }),
        );
    }
}

/// 段①:`config init --new` 长驻 → 抓 URL 出二维码 → 等用户扫码完成(进程退出)。
/// 返回 Ok(true)=注册成功;Ok(false)=被取消;Err=失败。
fn phase_register(app: &AppHandle, generation: u64) -> Result<bool, String> {
    let mut cmd = lark(&["config", "init", "--new"]);
    // 独立进程组:npm shim(shell→node)派生的孙进程与 shim 同组,退出收割的
    // kill_pid_tree 按负 pid 组杀整棵树,单杀 shim pid 会把 node 孤儿化。
    crate::platform::process::std_process_group_leader(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("config init --new 启动失败: {e}(需要先完成飞书 CLI 在线安装)"))?;
    let conn = app.state::<ConnectorConn>();
    let pid = child.id();
    conn.set_pid(ID, Some(pid));

    // 排空 stdout+stderr,每行送回 (首个飞书 URL, 脱敏安全行)。主线程的 tx 丢掉,
    // 这样两个管道都 EOF 后 rx 自动断开,不会永久阻塞。安全行进失败原因缓冲:
    // 卡片只显示本地化类目文案,捕获的原因行走 log 诊断轨迹。
    let (tx, rx) = std::sync::mpsc::channel::<(Option<String>, Option<String>)>();
    if let Some(o) = child.stdout.take() {
        cc::drain_for_url(FEISHU_CTX, o, tx.clone());
    }
    if let Some(e) = child.stderr.take() {
        cc::drain_for_url(FEISHU_CTX, e, tx.clone());
    }
    drop(tx);

    let mut auth_lines = std::collections::VecDeque::with_capacity(32);
    let deadline = Instant::now() + Duration::from_secs(40);
    let url = loop {
        // 40s 总预算;无 URL 行只记入原因缓冲并继续等。管道 EOF(子进程没打
        // URL 就退出)会让 recv_timeout 立即返回 Disconnected,走进下面的失败臂。
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((Some(u), safe)) => {
                cc::remember_auth_line(&mut auth_lines, safe);
                break u;
            }
            Ok((None, safe)) => cc::remember_auth_line(&mut auth_lines, safe),
            Err(_) => {
                let _ = child.kill();
                cc::reap_after_kill(&mut child);
                conn.clear_pid_if(ID, pid);
                // Cancel tree-kills the child → pipe EOF lands here: the user stopped
                // on purpose, so finish silently instead of misreporting a register failure.
                if conn.is_cancelled(ID) {
                    return Ok(false);
                }
                return Err(cc::auth_failure_reason(
                    &auth_lines,
                    "注册:40s 内未拿到二维码链接(检查网络 / 代理)",
                ));
            }
        }
    };
    let qr = cc::make_qr(&url);
    // A cancelled round must not re-open the scan modal the user dismissed,
    // and after a reconnect a superseded round's QR would paint a code that
    // can never be exchanged onto the new round's card.
    if !(conn.is_cancelled(ID) || conn.flow_stale(ID, generation)) {
        cc::emit(
            app,
            "feishu:qr",
            json!({ "phase": "register", "url": url, "qr_data_url": qr }),
        );
    }

    // 等进程退出(用户扫码完成);期间轮询取消标志。URL 打出后 CLI 才打印的
    // 失败原因行也持续收进缓冲,退出臂才有得拼。
    loop {
        if conn.is_cancelled(ID) {
            let _ = child.kill();
            cc::reap_after_kill(&mut child);
            conn.clear_pid_if(ID, pid);
            return Ok(false);
        }
        while let Ok((_, safe)) = rx.try_recv() {
            cc::remember_auth_line(&mut auth_lines, safe);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                conn.clear_pid_if(ID, pid);
                if conn.is_cancelled(ID) {
                    // Cancel race: a kill-induced failed exit is handled as a cancel, silent.
                    log::info!("[feishu] register cancelled; child exit={status}");
                    return Ok(false);
                }
                if !status.success() {
                    return Err(cc::auth_failure_reason(
                        &auth_lines,
                        "注册应用未完成(可能已取消或超时)",
                    ));
                }
                return Ok(true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(400)),
            Err(e) => {
                conn.clear_pid_if(ID, pid);
                return Err(format!("config init 等待失败: {e}"));
            }
        }
    }
}

/// 段②:`auth login --no-wait --json --recommend` 拿 URL+device_code → 二维码 →
/// 轮询 `auth login --device-code`(兼容它阻塞或立即返回)直到 user:ready / 超时。
fn phase_authorize(app: &AppHandle, generation: u64) -> Result<(), String> {
    let (_ok, so, se) = cc::run(lark(&[
        "auth",
        "login",
        "--no-wait",
        "--json",
        "--recommend",
    ]))?;
    let conn = app.state::<ConnectorConn>();
    // This probe child is untracked (cancel cannot tree-kill it), so it can
    // outlive a cancel by its whole run budget. Either outcome after a cancel
    // or a supersede must stay silent: a late error would resurrect the closed
    // card, and a late QR would show a device code that is never exchanged.
    if conn.is_cancelled(ID) || conn.flow_stale(ID, generation) {
        return Ok(());
    }
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
    // The early guard above ran before parsing and QR rendering; re-check at
    // the emit itself so a cancel or supersede landing in between cannot paint
    // a dead device code onto a newer round's card (same emit-site wrap as the
    // register QR emit and the dingtalk/tmeet QR emits).
    if !(conn.is_cancelled(ID) || conn.flow_stale(ID, generation)) {
        cc::emit(
            app,
            "feishu:qr",
            json!({ "phase": "authorize", "url": url, "qr_data_url": qr }),
        );
    }

    let start = Instant::now();
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
        // The probe can block for a long time and span a cancel: finish
        // silently, no connected onto a closed card.
        if conn.is_cancelled(ID) {
            return Ok(());
        }
        let ready = is_user_ready();
        // A cancel landing inside the readiness probe must not emit onto the
        // just-closed card either.
        if conn.is_cancelled(ID) {
            return Ok(());
        }
        if ready {
            cc::bundle_store_on_connected(ID);
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

/// Disconnect Feishu: `lark-cli auth logout` (clears tokens). The probe verdict is
/// unified with DingTalk/tmeet via [`cc::logout_probe_verdict`]: a genuinely not-installed
/// CLI degrades to `installed:false` and clears the bundle store; when the credential
/// state is unconfirmed the error is propagated as-is — never falsely report
/// "disconnected".
pub async fn feishu_logout() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let not_installed = || {
            cc::bundle_store_on_disconnected(ID);
            Ok::<Value, String>(json!({ "ok": true, "installed": false }))
        };
        match cc::logout_probe_verdict("飞书", lark_cli_probe()) {
            cc::LogoutProbeVerdict::Installed => {}
            cc::LogoutProbeVerdict::NotInstalled => return not_installed(),
            cc::LogoutProbeVerdict::Unconfirmed(message) => return Err(message),
        }
        let (ok, so, se) = cc::run(lark(&["auth", "logout"]))?;
        if !ok {
            return Err("飞书 CLI 退出登录失败，请重试".to_string());
        }
        cc::bundle_store_on_disconnected(ID);
        Ok::<Value, String>(json!({ "ok": true, "installed": true, "stdout": so, "stderr": se }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ─────────────────────── 飞书技能门控(§八.4 + composer 开关)───────────────────────
//
// 飞书技能可见性 = `skills_dir` 里 lark 目录在不在(引擎 SkillRegistry 扫目录)。
// 规则:**已连接(user:ready) 且 未手动停用** 才写技能;否则删掉(省 token / 关闭)。
// 手动停用标志:`~/.pinvou3/feishu_disabled` 文件存在 = 停用。与连接状态正交。

/// 按 visible 写 / 删飞书技能文件(调 [`Pinvou3Bundle::apply_feishu_skills`])。
pub(crate) fn apply_bundle_skills(visible: bool) -> std::io::Result<()> {
    crate::features::runtime_bundle::platform::Pinvou3Bundle::paths().apply_feishu_skills(visible)
}

/// 飞书门控表项:停用标志 + 就绪探测 + 技能落盘;
/// apply/skills_state 等命令公共体见 [`ConnectorGate`]。
pub(crate) static FEISHU_GATE: ConnectorGate = ConnectorGate {
    id: "feishu",
    // 原飞书版 apply_skills 独有的两条注释(其余三连接器写「见 feishu 同名注释」):
    // 技能写盘即可——连接成功弹窗已引导「新建对话」,新会话 spawn 时自然扫到飞书技能;
    // 不再原地广播刷新当前对话(故不依赖子模块 Op::RefreshSystemPrompt)。
    disabled_filename: "feishu_disabled",
    display_name: "飞书",
    ready_probe: is_user_ready,
    apply_bundle_skills: apply_bundle_skills,
};

/// 按当前"应否可见"状态写 / 删技能文件,并广播刷新在跑会话(当前对话即时生效)。
/// 前端在 **连接成功 / 断开 / 切开关** 后调,统一收口。
pub async fn feishu_apply_skills() -> Result<Value, String> {
    FEISHU_GATE.apply_skills_command().await
}

/// 给前端渲染开关态:`{connected, enabled(=未停用), visible(=connected&&enabled)}`。
pub async fn feishu_skills_state() -> Result<Value, String> {
    FEISHU_GATE.skills_state_command().await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--version` 输出 → 三段版本号。lark-cli 的输出在程序名与版本号之间夹
    /// 字面量 `version`(`lark-cli version 1.0.65`),按「首个可解析 token」取值;
    /// 版本号之后的构建信息尾巴不参与解析。
    #[test]
    fn parses_lark_versions() {
        assert_eq!(
            parse_lark_version("lark-cli version 1.0.65"),
            Some((1, 0, 65))
        );
        assert_eq!(
            parse_lark_version("lark-cli version 1.0.95 (build 2026-09-30T00:00:00Z abc1234)"),
            Some((1, 0, 95)),
        );
        // 两段式按共享口径补 0(不因假想的「1.1」误判未装触发降级重装)。
        assert_eq!(parse_lark_version("lark-cli version 1.1"), Some((1, 1, 0)));
        assert_eq!(parse_lark_version("hello"), None);
        // 纯噪声(无版本段)不解析出误值。
        assert_eq!(parse_lark_version("error: something"), None);
    }

    /// 最低版本门必须与平台 lock 表钉住的 lark-cli 版本一致:lock 升级而门
    /// 不升,技能包教的命令面就会在旧 CLI 上静默缺失;门高于 lock 则会触发
    /// 安装/替换循环。
    #[test]
    fn lark_min_version_matches_platform_lock() {
        let lock = crate::platform::connector_lock::lock_json();
        assert!(!lock.is_empty(), "platform lock json must be embedded");
        let parsed: Value = serde_json::from_str(lock).expect("lock json parses");
        let entries = parsed
            .pointer("/artifacts")
            .and_then(|v| v.as_array())
            .expect("lock json carries an artifacts list");
        let lark = entries
            .iter()
            .find(|e| e.get("name").and_then(|v| v.as_str()) == Some("lark-cli"))
            .expect("lock must pin lark-cli");
        let locked = lark
            .get("version")
            .and_then(|v| v.as_str())
            .expect("lark-cli lock entry carries a version");
        assert_eq!(
            cc::parse_semver3(locked),
            Some(LARK_MIN_VERSION),
            "LARK_MIN_VERSION must equal the locked lark-cli version ({locked})"
        );
    }
}
