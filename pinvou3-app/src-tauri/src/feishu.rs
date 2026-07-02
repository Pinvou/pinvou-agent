//! 飞书(Lark)CLI 接入 —— 启动引导 + 鉴权编排。
//!
//! 路线:`lark-cli`(飞书官方 CLI)+ 官方域技能(bundle 在
//! `~/.pinvou3/bundle/skills/lark-*`,见 `bridge::bundle`)。模型通过 shell 跑
//! `lark-cli <域> ...`,技能渐进披露教它用法。
//!
//! 凭证模型(用户零找 key):pinvou3 作为产品方**一次性**用自己的飞书 app
//! (app-id/secret)`config init`,用户只走浏览器 OAuth(`auth login` device flow)。
//! 见《飞书接入-CLI技能-开发方案 v3》四节。
//!
//! ⚠️ C 端注意(方案 4.4):app secret 当前从环境变量读、随 `config init` 落到
//! 本机 lark-cli 配置。生产应迁到安全配置 / 后端代理(secret 不落客户端)。
//! 本阶段(P0.5 实测)先用环境变量插入,代码与生产一致,只换凭证来源。

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::engine_pool::EnginePool;

/// 构造一个子进程命令,**Windows 下抑制黑窗**(CREATE_NO_WINDOW)。
/// Windows 上 npm 全局 shim(`lark-cli` / `npx`)是 `.cmd`,`std::process::Command`
/// 不会自动补 `.cmd`,故经 `cmd /C` 走 PATH 解析;Unix 直接调。
/// Windows 上把逻辑名解析成 npm 全局 shim 的 `.cmd`。
/// **关键**:直接调 `.cmd`(不经 `cmd /C`)——Rust 1.77+ 会对 `.cmd` 参数做正确转义,
/// 否则授权 URL 里的 `&` 会被 `cmd /C` 当成命令分隔符,`auth qrcode` 直接裂开。
#[cfg(windows)]
fn win_shim(program: &str) -> std::ffi::OsString {
    match program {
        "lark-cli" => {
            if let Ok(appdata) = std::env::var("APPDATA") {
                let p = std::path::Path::new(&appdata)
                    .join("npm")
                    .join("lark-cli.cmd");
                if p.is_file() {
                    return p.into_os_string();
                }
            }
            "lark-cli.cmd".into() // 回退:靠 PATH 找
        }
        "npx" => "npx.cmd".into(),
        other => other.into(),
    }
}

#[cfg(windows)]
fn base_cmd(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut c = Command::new(win_shim(program));
    c.creation_flags(CREATE_NO_WINDOW);
    // lark-cli 自带的绕代理开关:飞书是国内站,被 Clash 等代理走国外节点会 EOF。
    // 设了让 lark-cli 直连飞书。(若将来有"必须走代理才能到飞书"的用户,改成可配置。)
    c.env("LARK_CLI_NO_PROXY", "1");
    c
}

#[cfg(not(windows))]
fn base_cmd(program: &str) -> Command {
    let mut c = Command::new(program);
    c.env("LARK_CLI_NO_PROXY", "1");
    c
}

/// `lark-cli <args>`(抑黑窗)。
fn lark(args: &[&str]) -> Command {
    let mut c = base_cmd("lark-cli");
    c.args(args);
    c
}

/// 跑一个命令、收集 (success, stdout, stderr)。在 spawn_blocking 里调。
fn run(mut cmd: Command) -> Result<(bool, String, String), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("启动失败: {e}(需要 Node + lark-cli)"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// 跑命令并带**超时**(防 npx 卡在网络/代理/无 TTY 提示上无限转)。
/// stdout/stderr 丢弃(install 输出不解析),避免管道写满导致的死锁。
fn run_with_timeout(mut cmd: Command, secs: u64) -> Result<bool, String> {
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动失败: {e}(需要 Node)"))?;
    let start = Instant::now();
    loop {
        match child.try_wait().map_err(|e| format!("wait: {e}"))? {
            Some(status) => return Ok(status.success()),
            None => {
                if start.elapsed() > Duration::from_secs(secs) {
                    let _ = child.kill();
                    return Err(format!(
                        "lark-cli 安装超时({secs}s):可能是网络/代理(Clash)拦截,\
                         或需手动跑 `npx -y @larksuite/cli@latest install`"
                    ));
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

/// lark-cli 是否已在 PATH(快速,~秒级)。
fn lark_cli_present() -> bool {
    matches!(run(lark(&["--version"])), Ok((true, _, _)))
}

/// 把授权 URL 生成二维码,返回 data URL(供前端 `<img>` 显示)。
///
/// **本地生成**(qrcode crate),不再 shell 调 `lark-cli auth qrcode`。
/// 真因:部分用户 `npm i` 装到的是**老版本 lark-cli**,它没有 `auth qrcode`
/// 子命令 → 那条路径必然失败、二维码弹不出来。本地生成只依赖 URL 本身,
/// 与 CLI 版本彻底解耦,任何环境都能出码。
/// 产物是 SVG(矢量、清晰、体积小),编码成 data URL。失败返回 None(前端回退开浏览器)。
fn make_qr(url: &str) -> Option<String> {
    use qrcode::render::svg;
    use qrcode::{EcLevel, QrCode};

    // 授权 URL 较长,EcLevel::M 在容错与密度间平衡。
    let code = QrCode::with_error_correction_level(url.as_bytes(), EcLevel::M).ok()?;
    let svg_xml = code
        .render::<svg::Color>()
        .min_dimensions(220, 220)
        .quiet_zone(true)
        .dark_color(svg::Color("#0f172a"))
        .light_color(svg::Color("#ffffff"))
        .build();
    Some(format!("data:image/svg+xml;base64,{}", b64(svg_xml.as_bytes())))
}

/// 标准 base64 编码(避免引新依赖)。
fn b64(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(A[((n >> 18) & 63) as usize] as char);
        out.push(A[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { A[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// 从一段输出里抓第一个 JSON 对象(lark-cli --json 有时夹带 spinner/提示行)。
fn parse_json(s: &str) -> Option<Value> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&s[start..=end]).ok()
}

// ──────────────────────── 连接编排:状态 + 辅助 ────────────────────────

/// 连接编排的共享状态:当前长驻子进程 PID(供取消时 tree-kill)+ 取消标志。
/// 在 `lib.rs` 用 `.manage(FeishuConn::default())` 注册。
#[derive(Default)]
pub struct FeishuConn {
    pid: Mutex<Option<u32>>,
    cancelled: AtomicBool,
}

fn emit(app: &AppHandle, event: &str, payload: Value) {
    let _ = app.emit(event, payload);
}

fn is_cancelled(app: &AppHandle) -> bool {
    app.state::<FeishuConn>().cancelled.load(Ordering::SeqCst)
}

fn set_pid(app: &AppHandle, pid: Option<u32>) {
    if let Ok(mut g) = app.state::<FeishuConn>().pid.lock() {
        *g = pid;
    }
}

/// 从一行输出里抓飞书的配置/授权 URL(`https://...feishu.../...` 或 larksuite)。
fn extract_feishu_url(line: &str) -> Option<String> {
    let i = line.find("https://")?;
    let url: String = line[i..].chars().take_while(|c| !c.is_whitespace()).collect();
    if url.contains("feishu") || url.contains("larksuite") {
        Some(url)
    } else {
        None
    }
}

/// 后台线程:逐行排空一个管道(防写满阻塞),抓到首个飞书 URL 经 channel 送回。
fn drain_for_url<R: std::io::Read + Send + 'static>(
    r: R,
    tx: mpsc::Sender<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines().flatten() {
            if let Some(u) = extract_feishu_url(&line) {
                let _ = tx.send(u);
            }
        }
    })
}

/// tree-kill 一个 PID,连其子进程(.cmd 拉起的 node)一起。
fn kill_pid_tree(pid: u32) {
    let pid_s = pid.to_string();
    #[cfg(windows)]
    {
        let _ = base_cmd("taskkill")
            .args(["/F", "/T", "/PID", &pid_s])
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = base_cmd("kill").args(["-9", &pid_s]).output();
    }
}

/// `auth status` 里用户身份是否 ready(已授权)。
fn is_user_ready() -> bool {
    if let Ok((_, so, se)) = run(lark(&["auth", "status", "--json"])) {
        let p = parse_json(&so).or_else(|| parse_json(&se));
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

/// 引导:确保 lark-cli 装好(全局 shim 在 PATH 上),幂等。
/// `npx -y @larksuite/cli@latest install` —— 已装则跳过。需要 Node。
#[tauri::command]
pub async fn feishu_ensure_cli() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let t = std::time::Instant::now();
        // 已装则秒返回 —— 不跑慢吞吞、可能卡死的 npx install。
        let present = lark_cli_present();
        eprintln!("[feishu] ensure_cli: lark_cli_present={present} in {}ms", t.elapsed().as_millis());
        if present {
            return Ok::<Value, String>(json!({ "ok": true, "already": true }));
        }
        // 未装才装,带 180s 超时防卡死(网络/代理)。
        let mut c = base_cmd("npx");
        c.args(["-y", "@larksuite/cli@latest", "install"]);
        let ok = run_with_timeout(c, 180)?;
        Ok::<Value, String>(json!({ "ok": ok, "already": false }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 查询当前飞书连接状态:`lark-cli auth status --json`。
/// 返回 lark-cli 的原始 JSON(含 appId / identities.user.status 等);未配置 app
/// 或未登录则 connected=false。
#[tauri::command]
pub async fn feishu_status() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let (ok, so, se) = run(lark(&["auth", "status", "--json"]))?;
        let parsed = parse_json(&so).or_else(|| parse_json(&se));
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
        Ok::<Value, String>(json!({
            "ok": ok,
            "connected": connected,
            "configured": configured,
            "raw": parsed.unwrap_or(Value::Null),
            "stderr": se,
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
#[tauri::command]
pub async fn feishu_connect_begin(app: AppHandle) -> Result<Value, String> {
    app.state::<FeishuConn>()
        .cancelled
        .store(false, Ordering::SeqCst);
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || run_connect_flow(&app2));
    Ok(json!({ "started": true }))
}

/// 编排:段① 注册 app → 段② 授权用户。任一段出错/取消即停,错误经事件上报。
fn run_connect_flow(app: &AppHandle) {
    match phase_register(app) {
        Ok(true) => {}
        Ok(false) => return, // 取消,静默
        Err(e) => {
            emit(app, "feishu:error", json!({ "phase": "register", "message": e }));
            return;
        }
    }
    if let Err(e) = phase_authorize(app) {
        emit(app, "feishu:error", json!({ "phase": "authorize", "message": e }));
    }
}

/// 段①:`config init --new` 长驻 → 抓 URL 出二维码 → 等用户扫码完成(进程退出)。
/// 返回 Ok(true)=注册成功;Ok(false)=被取消;Err=失败。
fn phase_register(app: &AppHandle) -> Result<bool, String> {
    let mut cmd = base_cmd("lark-cli");
    cmd.args(["config", "init", "--new"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("config init --new 启动失败: {e}(需要 Node + lark-cli)"))?;
    set_pid(app, Some(child.id()));

    // 排空 stdout+stderr,抓首个飞书 URL(channel 送回)。主线程的 tx 丢掉,
    // 这样两个管道都 EOF 后 rx 自动断开,不会永久阻塞。
    let (tx, rx) = mpsc::channel::<String>();
    if let Some(o) = child.stdout.take() {
        drain_for_url(o, tx.clone());
    }
    if let Some(e) = child.stderr.take() {
        drain_for_url(e, tx.clone());
    }
    drop(tx);

    let url = match rx.recv_timeout(Duration::from_secs(40)) {
        Ok(u) => u,
        Err(_) => {
            let _ = child.kill();
            set_pid(app, None);
            return Err("注册:40s 内未拿到二维码链接(检查网络 / 代理)".into());
        }
    };
    let qr = make_qr(&url);
    emit(
        app,
        "feishu:qr",
        json!({ "phase": "register", "url": url, "qr_data_url": qr }),
    );

    // 等进程退出(用户扫码完成);期间轮询取消标志。
    loop {
        if is_cancelled(app) {
            let _ = child.kill();
            set_pid(app, None);
            return Ok(false);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                set_pid(app, None);
                if !status.success() {
                    return Err("注册应用未完成(可能已取消或超时)".into());
                }
                emit(app, "feishu:phase", json!({ "phase": "registered" }));
                return Ok(true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(400)),
            Err(e) => {
                set_pid(app, None);
                return Err(format!("config init 等待失败: {e}"));
            }
        }
    }
}

/// 段②:`auth login --no-wait --json --recommend` 拿 URL+device_code → 二维码 →
/// 轮询 `auth login --device-code`(兼容它阻塞或立即返回)直到 user:ready / 超时。
fn phase_authorize(app: &AppHandle) -> Result<(), String> {
    let (_ok, so, se) = run(lark(&["auth", "login", "--no-wait", "--json", "--recommend"]))?;
    let p = parse_json(&so).or_else(|| parse_json(&se)).unwrap_or(Value::Null);
    let url = ["verification_uri_complete", "verification_url", "verificationUrl", "url"]
        .iter()
        .find_map(|k| p.get(*k).and_then(|v| v.as_str()))
        .map(String::from)
        .ok_or("auth login 未返回授权链接")?;
    let device_code = ["device_code", "deviceCode"]
        .iter()
        .find_map(|k| p.get(*k).and_then(|v| v.as_str()))
        .map(String::from)
        .ok_or("auth login 未返回 device_code")?;
    let qr = make_qr(&url);
    emit(
        app,
        "feishu:qr",
        json!({ "phase": "authorize", "url": url, "qr_data_url": qr }),
    );

    let start = Instant::now();
    loop {
        if is_cancelled(app) {
            return Ok(()); // 取消:静默(run_connect_flow 不再 emit)
        }
        if start.elapsed() > Duration::from_secs(300) {
            return Err("授权超时(5 分钟内未完成扫码)".into());
        }
        std::thread::sleep(Duration::from_secs(3));
        // 这步可能阻塞到完成、也可能立即返回 pending —— 两种都兼容,靠 auth status 判 ready。
        let _ = run(lark(&["auth", "login", "--device-code", &device_code, "--json"]));
        if is_user_ready() {
            emit(app, "feishu:connected", json!({ "ok": true }));
            return Ok(());
        }
    }
}

/// 取消连接:置取消标志 + tree-kill 当前长驻子进程(关二维码弹窗 / 超时时调)。
#[tauri::command]
pub async fn feishu_cancel(app: AppHandle) -> Result<Value, String> {
    let pid = {
        let st = app.state::<FeishuConn>();
        st.cancelled.store(true, Ordering::SeqCst);
        st.pid.lock().ok().and_then(|g| *g)
    };
    if let Some(pid) = pid {
        let _ = tokio::task::spawn_blocking(move || kill_pid_tree(pid)).await;
    }
    Ok(json!({ "ok": true }))
}

/// 断开飞书:`lark-cli auth logout`(清 token)。
#[tauri::command]
pub async fn feishu_logout() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let (ok, so, se) = run(lark(&["auth", "logout"]))?;
        Ok::<Value, String>(json!({ "ok": ok, "stdout": so, "stderr": se }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ─────────────────────── 飞书技能门控(§八.4 + composer 开关)───────────────────────
//
// 飞书技能可见性 = `skills_dir` 里 9 个 lark 目录在不在(引擎 SkillRegistry 扫目录)。
// 规则:**已连接(user:ready) 且 未手动停用** 才写技能;否则删掉(省 token / 关闭)。
// 手动停用标志:`~/.pinvou3/feishu_disabled` 文件存在 = 停用。与连接状态正交。

fn feishu_disabled_path() -> std::path::PathBuf {
    crate::bridge::paths::pinvou3_home().join("feishu_disabled")
}

/// 用户是否手动停用了飞书技能。
pub fn is_feishu_disabled() -> bool {
    feishu_disabled_path().exists()
}

fn set_feishu_disabled_flag(disabled: bool) {
    let p = feishu_disabled_path();
    if disabled {
        let _ = std::fs::write(&p, b"1");
    } else {
        let _ = std::fs::remove_file(&p);
    }
}

/// 飞书技能此刻该不该出现在 skills_dir:**未手动停用 且 已连接**。
/// 启动时(bundle)与命令里都用它判定。注:会 spawn lark-cli 查 auth status(未装则 false)。
pub fn feishu_skills_should_show() -> bool {
    !is_feishu_disabled() && is_user_ready()
}

/// 按当前"应否可见"状态写/删技能文件,并广播刷新在跑会话(当前对话即时生效)。
/// 前端在 **连接成功 / 断开 / 切开关** 后调,统一收口。
#[tauri::command]
pub async fn feishu_apply_skills() -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(|| {
        let show = feishu_skills_should_show();
        let _ = crate::bridge::bundle::Pinvou3Bundle::paths().apply_feishu_skills(show);
        show
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?;
    // 技能写盘即可——连接成功弹窗已引导「新建对话」,新会话 spawn 时自然扫到飞书技能;
    // 不再原地广播刷新当前对话(故不依赖子模块 Op::RefreshSystemPrompt)。
    Ok(json!({ "visible": show }))
}

/// composer 飞书开关:`enabled` → 写停用标志 → 按规则增删技能 → 广播刷新。
#[tauri::command]
pub async fn set_feishu_enabled(enabled: bool) -> Result<Value, String> {
    let show = tokio::task::spawn_blocking(move || {
        set_feishu_disabled_flag(!enabled);
        let show = feishu_skills_should_show();
        let _ = crate::bridge::bundle::Pinvou3Bundle::paths().apply_feishu_skills(show);
        show
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?;
    Ok(json!({ "ok": true, "visible": show }))
}

/// 给前端渲染开关态:`{connected, enabled(=未停用), visible(=connected&&enabled)}`。
#[tauri::command]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 本地生成二维码:任意 URL 都能出码(不依赖 lark-cli 版本),
    /// 且是可直接塞进 <img> 的 SVG data URL。
    #[test]
    fn make_qr_local_produces_svg_data_url() {
        let url = "https://accounts.feishu.cn/oauth/authorize?client_id=cli_abc&device_code=xyz&scope=a%20b";
        let qr = make_qr(url).expect("本地二维码生成不应失败");
        assert!(qr.starts_with("data:image/svg+xml;base64,"), "应是 SVG data URL");
        // 解码回 SVG,确认是真实矢量二维码(含 <svg> 与绘制的模块 path/rect)。
        let b64 = qr.trim_start_matches("data:image/svg+xml;base64,");
        let svg = String::from_utf8(b64_decode(b64)).unwrap();
        assert!(svg.contains("<svg"), "应含 <svg> 根节点");
        assert!(svg.contains("#0f172a"), "应含深色模块颜色");
    }

    // 测试辅助:标准 base64 解码(生产 b64 的逆运算,仅测试用)。
    fn b64_decode(s: &str) -> Vec<u8> {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let val = |c: u8| A.iter().position(|&x| x == c).unwrap() as u32;
        let mut out = Vec::new();
        let clean: Vec<u8> = s.bytes().filter(|&c| c != b'=').collect();
        for chunk in clean.chunks(4) {
            let mut n = 0u32;
            for (i, &c) in chunk.iter().enumerate() {
                n |= val(c) << (18 - 6 * i);
            }
            out.push((n >> 16) as u8);
            if chunk.len() > 2 {
                out.push((n >> 8) as u8);
            }
            if chunk.len() > 3 {
                out.push(n as u8);
            }
        }
        out
    }
}
