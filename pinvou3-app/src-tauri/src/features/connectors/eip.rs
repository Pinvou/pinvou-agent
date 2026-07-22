//! H3C EIP 员工门户接入 —— 连接生命周期 + 鉴权编排。
//!
//! 路线:A(CLI 连接器)。复用 iClaw 的 `eip-cli` 预编译二进制(多平台:
//! `eip-cli` / `eip-cli-aarch64` / `eip-cli.exe`),内置打包到 `~/.pinvou3/bundle/skills/eip/bin/`。
//! 业务命令(考勤/假期/待办…)由模型经 shell 跑 `eip <域> ...`,技能文档教用法
//! (同飞书 lark-cli)。本模块只负责**连接态**:状态查询、SSO 登录(轮询自动收)、
//! 取消、登出。
//!
//! 鉴权(用户零找 key):CLI 自管凭证——首次跑业务命令未认证时输出 SSO URL,
//! 用户浏览器登录后 `auth poll` 自动收凭证;token 过期 CLI 自动用 AIT 刷新。
//! 凭证由 CLI 用 `AGENT_DEVICE_ID` 派生密钥加密存到 `AGENT_CREDENTIALS_DIR`。
//! 见《EIP员工门户接入-开发方案》§3。
//!
//! ⚠️ 二进制是 H3C IT 内部产物,当前先用 iClaw 这份打包;合规事后补内部知会。

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use qrcode::{render::svg, QrCode};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

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
        out.push(if chunk.len() > 1 {
            A[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// 把 SSO URL 生成二维码 data URL。用 SVG 避免依赖外部 CLI 的 qrcode 子命令。
fn make_qr(url: &str) -> Option<String> {
    let code = QrCode::new(url.as_bytes()).ok()?;
    let image = code
        .render::<svg::Color>()
        .min_dimensions(256, 256)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    Some(format!(
        "data:image/svg+xml;base64,{}",
        b64(image.as_bytes())
    ))
}

// ──────────────────────────── 路径 / 凭证 ────────────────────────────

/// `~/.pinvou3/eip/` —— EIP 的 device-id 与凭证根目录(与 bundle 里的二进制分开)。
fn eip_home() -> PathBuf {
    crate::platform::paths::pinvou3_home().join("eip")
}

/// 凭证目录,传给 CLI 的 `AGENT_CREDENTIALS_DIR`。CLI 在此加密存 token/ait。
fn credentials_dir() -> PathBuf {
    let d = eip_home().join("credentials");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// 稳定的 `AGENT_DEVICE_ID`(CLI 用它派生凭证加密密钥)。**一次生成、持久化**——
/// 换了 device-id 会解不开旧凭证(方案 §7 风险)。首次用 sha256(时间+pid) 取 16 字节 hex。
fn device_id() -> String {
    let p = eip_home().join("device_id");
    if let Ok(s) = std::fs::read_to_string(&p) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    use sha2::{Digest, Sha256};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut h = Sha256::new();
    h.update(format!("{}-{}", nanos, std::process::id()).as_bytes());
    let id: String = h
        .finalize()
        .iter()
        .take(16)
        .map(|b| format!("{:02x}", b))
        .collect();
    let _ = std::fs::create_dir_all(eip_home());
    let _ = std::fs::write(&p, &id);
    id
}

/// 定位 EIP CLI 二进制(按平台选 `.exe` / ELF)。Linux 下确保有执行权限。
fn eip_bin_path() -> Result<PathBuf, String> {
    let name = if cfg!(windows) {
        "eip-cli.exe"
    } else if std::env::consts::ARCH == "aarch64" {
        "eip-cli-aarch64"
    } else {
        "eip-cli"
    };
    let p = crate::platform::paths::bundle_skills_dir()
        .join("eip")
        .join("bin")
        .join(name);
    if !p.is_file() {
        #[cfg(unix)]
        if std::env::consts::ARCH == "aarch64"
            && crate::platform::paths::bundle_skills_dir()
                .join("eip")
                .join("bin")
                .join("eip-cli")
                .is_file()
        {
            return Err(format!(
                "eip-cli Linux ARM64 binary missing: expected {}. Bundle contains eip-cli, but this Linux device is aarch64; please package a matching aarch64 binary as eip-cli-aarch64.",
                p.display()
            ));
        }
        #[cfg(unix)]
        if crate::platform::paths::bundle_skills_dir()
            .join("eip")
            .join("bin")
            .join("eip-cli.exe")
            .is_file()
        {
            return Err(format!(
                "eip-cli Linux binary missing: expected {} for {}. Bundle only contains the Windows .exe; H3C EIP is unavailable on this Linux device until a matching Linux binary is packaged.",
                p.display(),
                std::env::consts::ARCH
            ));
        }
        return Err(format!(
            "eip-cli 未找到: {}(需先把 EIP 技能二进制打包进 bundle)",
            p.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        validate_linux_cli_arch(&p, "eip-cli")?;
        if let Ok(meta) = std::fs::metadata(&p) {
            let mode = meta.permissions().mode();
            if mode & 0o111 == 0 {
                let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
    Ok(p)
}

#[cfg(unix)]
fn validate_linux_cli_arch(path: &std::path::Path, label: &str) -> Result<(), String> {
    let Ok(bytes) = std::fs::read(path) else {
        return Ok(());
    };
    if bytes.len() < 20 || &bytes[0..4] != b"\x7FELF" || bytes[5] != 1 {
        return Ok(());
    }
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    let actual = match machine {
        62 => "x86_64",
        183 => "aarch64",
        3 => "x86",
        40 => "arm",
        _ => "unknown",
    };
    let expected = std::env::consts::ARCH;
    let compatible = matches!(
        (expected, actual),
        ("x86_64", "x86_64") | ("aarch64", "aarch64") | ("arm", "arm") | ("x86", "x86")
    );
    if compatible {
        Ok(())
    } else {
        Err(format!(
            "{label} architecture mismatch: packaged binary is {actual}, but this Linux device is {expected}. Please package a matching {expected} Linux binary at {}.",
            path.display()
        ))
    }
}

// ──────────────────────────── 子进程封装 ────────────────────────────

/// 构造 `eip-cli` 命令:注入 AGENT_* 环境,Windows 抑黑窗。
fn base_cmd() -> Result<Command, String> {
    let bin = eip_bin_path()?;
    let mut c = Command::new(&bin);
    c.env("AGENT_DEVICE_ID", device_id());
    c.env("AGENT_CREDENTIALS_DIR", credentials_dir());
    // 非交互:CLI 不弹本地交互提示,未认证时直接输出 SSO URL 让我们编排。
    c.env("AGENT_NON_INTERACTIVE", "1");
    // EIP/SSO 是内网服务,不能继承 Clash/VPN 代理环境；否则可能打到外部出口后被网关拒绝。
    for k in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        c.env_remove(k);
    }
    c.env("NO_PROXY", "*");
    c.env("no_proxy", "*");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(c)
}

/// `eip-cli <args>`(已注入环境)。
fn eip(args: &[&str]) -> Result<Command, String> {
    let mut c = base_cmd()?;
    c.args(args);
    Ok(c)
}

/// 跑命令,收 (success, exit_code, stdout, stderr)。`exit_code==3` = 需登录。
fn run(mut cmd: Command) -> Result<(bool, i32, String, String), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("启动 eip-cli 失败: {e}"))?;
    Ok((
        out.status.success(),
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// 从一段输出里抓第一个 JSON 对象(CLI 可能夹带提示行)。
fn parse_json(s: &str) -> Option<Value> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&s[start..=end]).ok()
}

/// `auth status` 是否已有有效 token(已认证)。
fn is_authed() -> bool {
    if let Ok(cmd) = eip(&["auth", "status", "--output", "json"]) {
        if let Ok((_ok, _code, so, se)) = run(cmd) {
            let p = parse_json(&so).or_else(|| parse_json(&se));
            return p
                .as_ref()
                .and_then(|v| v.get("hasToken"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        }
    }
    false
}

// ──────────────────────────── 连接标记 / 技能门控 ────────────────────────────

/// 连接态标记文件:连上写、登出删。用作**技能门控信号**——非 spawn `auth status`,
/// 故启动门控判定零子进程,且非 EIP 用户(无凭证/从不连接)天然为 false。
fn connected_flag() -> PathBuf {
    eip_home().join("connected.flag")
}

/// EIP 技能此刻该不该对模型可见:本机存在连接标记(已连接过且未登出)。
/// `bundle.rs::ensure_extracted` 启动时用它决定放 / 删 `eip/SKILL.md`。
pub fn eip_skills_should_show() -> bool {
    connected_flag().is_file()
}

/// 置连接标记并请求技能可见性变更。空闲时立即写/删 `eip/SKILL.md`；
/// 有活跃 turn 时延迟到边界应用，保证配置只影响下一个 turn。
fn set_connected(v: bool) -> bool {
    let previous = eip_skills_should_show();
    let p = connected_flag();
    if previous != v {
        if v {
            let _ = std::fs::create_dir_all(eip_home());
            let _ = std::fs::write(&p, b"1");
        } else {
            let _ = std::fs::remove_file(&p);
        }
    }
    let skill_visible = crate::platform::bundle::Pinvou3Bundle::paths()
        .skills_dir
        .join("eip")
        .join("SKILL.md")
        .is_file();
    let deferred = (previous != v || skill_visible != v)
        && crate::platform::connector_visibility::request(
            crate::platform::connector_visibility::ConnectorKind::Eip,
            v,
        );
    if deferred {
        log::info!("[eip] skill visibility queued for the next turn connected={v}");
    }
    previous != v
}

// ──────────────────────────── 连接编排状态 ────────────────────────────

/// 连接编排共享状态:登录轮询的取消标志。`lib.rs` 用 `.manage(EipConn::default())` 注册。
#[derive(Default)]
pub struct EipConn {
    cancelled: AtomicBool,
}

fn emit(app: &AppHandle, event: &str, payload: Value) {
    let _ = app.emit(event, payload);
}

fn is_cancelled(app: &AppHandle) -> bool {
    app.state::<EipConn>().cancelled.load(Ordering::SeqCst)
}

// ───────────────────────────── Tauri commands ─────────────────────────────

/// 查询 EIP 连接状态:`auth status --output json`。connected = 有有效 token。
/// 纯查询：不写连接标记、不改技能目录、不回收 Engine。
#[tauri::command]
pub async fn eip_status() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let cmd = eip(&["auth", "status", "--output", "json"])?;
        let (_ok, _code, so, se) = run(cmd)?;
        let p = parse_json(&so).or_else(|| parse_json(&se));
        let connected = p
            .as_ref()
            .and_then(|v| v.get("hasToken"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // Reconcile durable visibility without rebuilding or interrupting an
        // active Engine. connector_visibility applies it at the turn boundary.
        let _ = set_connected(connected);
        // 只回 connected:auth status 的 raw/stderr 可能含 token,不进 webview
        Ok::<Value, String>(json!({ "connected": connected }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 开始连接 EIP(SSO 轮询自动收):
/// ① `auth login --no-poll --output json` 拿 SSO URL → emit `eip:sso`(前端引导浏览器登录)。
/// ② 循环 `auth poll` 直到认证成功 / 超时(5min)/ 取消 → emit `eip:connected` / `eip:error`。
/// 立即返回 `{started:true}`,前端 listen 事件驱动 UI。
#[tauri::command]
pub async fn eip_connect_begin(app: AppHandle) -> Result<Value, String> {
    app.state::<EipConn>()
        .cancelled
        .store(false, Ordering::SeqCst);
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || run_login_flow(&app2));
    Ok(json!({ "started": true }))
}

fn run_login_flow(app: &AppHandle) {
    let url = match get_sso_url() {
        Ok(u) => u,
        Err(e) => {
            emit(app, "eip:error", json!({ "message": e }));
            return;
        }
    };
    let qr = make_qr(&url);
    emit(app, "eip:sso", json!({ "url": url, "qr_data_url": qr }));

    let start = Instant::now();
    loop {
        if is_cancelled(app) {
            return; // 取消:静默
        }
        if start.elapsed() > Duration::from_secs(300) {
            emit(
                app,
                "eip:error",
                json!({ "message": "登录超时(5 分钟内未完成)" }),
            );
            return;
        }
        std::thread::sleep(Duration::from_secs(3));
        // auth poll 一轮(自带 --timeout);成功与否统一靠 auth status 判定。
        if let Ok(cmd) = eip(&["auth", "poll", "--output", "json"]) {
            let _ = run(cmd);
        }
        if is_authed() {
            if set_connected(true) {
                log::info!("[eip] connected state changed; visibility will apply without evicting active turns");
            }
            emit(app, "eip:connected", json!({ "ok": true }));
            return;
        }
    }
}

/// `auth login --no-poll --output json` → 取 SSO 登录地址(只出 URL 不阻塞)。
fn get_sso_url() -> Result<String, String> {
    let cmd = eip(&["auth", "login", "--no-poll", "--output", "json"])?;
    let (_ok, _code, so, se) = run(cmd)?;
    let p = parse_json(&so)
        .or_else(|| parse_json(&se))
        .unwrap_or(Value::Null);
    ["ssoLoginUrl", "ssoUrl", "url"]
        .iter()
        .find_map(|k| p.get(*k).and_then(|v| v.as_str()))
        .map(String::from)
        .ok_or_else(|| {
            let message = p
                .get("message")
                .and_then(|v| v.as_str())
                .or_else(|| p.get("hint").and_then(|v| v.as_str()))
                .unwrap_or("auth login 未返回 SSO 登录地址");
            format!("auth login 未返回 SSO 登录地址: {message}")
        })
}

/// 取消连接:置取消标志(登录轮询下一拍自停)。
#[tauri::command]
pub async fn eip_cancel(app: AppHandle) -> Result<Value, String> {
    app.state::<EipConn>()
        .cancelled
        .store(true, Ordering::SeqCst);
    Ok(json!({ "ok": true }))
}

/// 断开 EIP:`auth logout`(清凭证)。
#[tauri::command]
pub async fn eip_logout() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let cmd = eip(&["auth", "logout"])?;
        let (ok, _code, so, se) = run(cmd)?;
        if ok {
            let _ = set_connected(false); // 下一个 turn 生效，不驱逐活跃 Engine
        }
        Ok::<Value, String>(json!({ "ok": ok, "stdout": so, "stderr": se }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::make_qr;

    #[test]
    fn make_qr_returns_svg_data_url() {
        let qr = make_qr("https://sso.h3c.com/login?sessionId=test-session").unwrap();
        assert!(qr.starts_with("data:image/svg+xml;base64,"));
        assert!(qr.len() > 512);
    }
}
