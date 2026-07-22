//! H3C 知道知识库接入 —— 连接生命周期 + 鉴权编排。
//!
//! 复用 IT 提供的 `zhidao` CLI。连接流程与 EIP 同源 SSO_NEXT。当前 zhidao CLI
//! 只提供 `login/save/load/exchange`,没有 poll 子命令;但底层凭证格式和 EIP CLI
//! 相同,因此 Pinvou 用 `zhidao login` 拿 SSO URL,再借 EIP CLI 的 `auth poll`
//! 收 token/AIT 到 zhidao 凭证目录,实现和 EIP 一样的扫码自动连接体验。

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use qrcode::{render::svg, QrCode};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

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

fn zhidao_home() -> PathBuf {
    crate::platform::paths::pinvou3_home().join("zhidao")
}

fn credentials_dir() -> PathBuf {
    let d = zhidao_home().join("credentials");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn device_id() -> String {
    let p = zhidao_home().join("device_id");
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
    let _ = std::fs::create_dir_all(zhidao_home());
    let _ = std::fs::write(&p, &id);
    id
}

fn zhidao_bin_path() -> Result<PathBuf, String> {
    // Windows ships a native reimplementation (zhidao-cli.exe, see resources/common/bundle/
    // zhidao/win-src); Unix runs the CLI binary directly so architecture errors
    // surface before the wrapper hides them as a generic connection failure.
    let name = if cfg!(windows) {
        "zhidao-cli.exe"
    } else if std::env::consts::ARCH == "aarch64" {
        "zhidao-cli-aarch64"
    } else {
        "zhidao-cli"
    };
    let p = crate::platform::paths::bundle_skills_dir()
        .join("zhidao")
        .join("bin")
        .join(name);
    if !p.is_file() {
        return Err(format!(
            "zhidao CLI 未找到: {}(需先把知道技能二进制打包进 bundle)",
            p.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        validate_linux_cli_arch(&p, "zhidao-cli")?;
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

fn base_cmd() -> Result<Command, String> {
    let mut c = Command::new(zhidao_bin_path()?);
    c.env("AGENT_DEVICE_ID", device_id());
    c.env("AGENT_CREDENTIALS_DIR", credentials_dir());
    c.env("AGENT_NON_INTERACTIVE", "1");
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

fn zhidao(args: &[&str]) -> Result<Command, String> {
    let mut c = base_cmd()?;
    c.args(args);
    Ok(c)
}

fn run(mut cmd: Command) -> Result<(bool, i32, String, String), String> {
    let out = cmd.output().map_err(|e| format!("启动 zhidao 失败: {e}"))?;
    Ok((
        out.status.success(),
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

fn parse_json(s: &str) -> Option<Value> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&s[start..=end]).ok()
}

fn is_authed() -> bool {
    matches!(run(match zhidao(&["load"]) { Ok(c) => c, Err(_) => return false }), Ok((true, _, so, _)) if so.contains("ZHIDAO_TOKEN"))
}

fn connected_flag() -> PathBuf {
    zhidao_home().join("connected.flag")
}

pub fn zhidao_skills_should_show() -> bool {
    connected_flag().is_file()
}

fn set_connected(v: bool) -> bool {
    let previous = zhidao_skills_should_show();
    let p = connected_flag();
    if previous != v {
        if v {
            let _ = std::fs::create_dir_all(zhidao_home());
            let _ = std::fs::write(&p, b"1");
        } else {
            let _ = std::fs::remove_file(&p);
        }
    }
    let skill_visible = crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
        .skills_dir
        .join("zhidao")
        .join("SKILL.md")
        .is_file();
    let deferred = (previous != v || skill_visible != v)
        && crate::features::connectors::visibility::request(
            crate::features::connectors::visibility::ConnectorKind::Zhidao,
            v,
        );
    if deferred {
        log::info!("[zhidao] skill visibility queued for the next turn connected={v}");
    }
    previous != v
}

#[derive(Default)]
pub struct ZhidaoConn {
    cancelled: AtomicBool,
}

fn emit(app: &AppHandle, event: &str, payload: Value) {
    let _ = app.emit(event, payload);
}

fn is_cancelled(app: &AppHandle) -> bool {
    app.state::<ZhidaoConn>().cancelled.load(Ordering::SeqCst)
}

#[tauri::command]
pub async fn zhidao_status() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let cmd = zhidao(&["load"])?;
        let (ok, _code, so, _se) = run(cmd)?;
        let connected = ok && so.contains("ZHIDAO_TOKEN");
        // 状态查询只更新期望可见性；活跃 turn 自然结束后再落技能文件。
        let _ = set_connected(connected);
        // 只回 connected:load 的 stdout 就是 ZHIDAO_TOKEN 本体,不能进 webview
        Ok::<Value, String>(json!({ "connected": connected }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[tauri::command]
pub async fn zhidao_connect_begin(app: AppHandle) -> Result<Value, String> {
    app.state::<ZhidaoConn>()
        .cancelled
        .store(false, Ordering::SeqCst);
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || run_login_flow(&app2));
    Ok(json!({ "started": true }))
}

fn run_login_flow(app: &AppHandle) {
    let login = match get_sso_login() {
        Ok(login) => login,
        Err(e) => {
            emit(app, "zhidao:error", json!({ "message": e }));
            return;
        }
    };
    let qr = make_qr(&login.url);
    emit(
        app,
        "zhidao:sso",
        json!({ "url": login.url, "qr_data_url": qr }),
    );

    let deadline = Instant::now() + Duration::from_secs(300);
    while Instant::now() < deadline {
        if is_cancelled(app) {
            return;
        }
        // 用 zhidao-cli 自己的 poll(复用 H3C session/poll 端点)收 token/AIT 到 zhidao 凭证目录。
        // 成功即由本 CLI 用同一套加密落盘,`load` 随后能读到 —— 不再依赖 eip-cli 的凭证格式。
        if let Ok(cmd) = zhidao(&["poll", "--session-id", &login.session_id, "--timeout", "5"]) {
            let _ = run(cmd);
        }
        if is_authed() {
            if set_connected(true) {
                log::info!("[zhidao] connected state changed; visibility will apply without evicting active turns");
            }
            emit(app, "zhidao:connected", json!({ "ok": true }));
            return;
        }
    }
    emit(
        app,
        "zhidao:error",
        json!({ "message": "知道 SSO 认证超时，请重新连接" }),
    );
}

struct SsoLogin {
    url: String,
    session_id: String,
}

fn get_sso_login() -> Result<SsoLogin, String> {
    let cmd = zhidao(&["login"])?;
    let (_ok, _code, so, se) = run(cmd)?;
    let p = parse_json(&so)
        .or_else(|| parse_json(&se))
        .unwrap_or(Value::Null);
    let url = ["ssoLoginUrl", "ssoUrl", "url"]
        .iter()
        .find_map(|k| p.get(*k).and_then(|v| v.as_str()))
        .or_else(|| {
            p.get("data").and_then(|d| {
                ["ssoLoginUrl", "ssoUrl", "url"]
                    .iter()
                    .find_map(|k| d.get(*k).and_then(|v| v.as_str()))
            })
        })
        .map(String::from)
        .ok_or_else(|| {
            let message = p
                .get("message")
                .and_then(|v| v.as_str())
                .or_else(|| p.get("msg").and_then(|v| v.as_str()))
                .unwrap_or("zhidao login 未返回 SSO 登录地址");
            format!("zhidao login 未返回 SSO 登录地址: {message}")
        })?;
    let session_id = ["sessionId", "session_id", "state"]
        .iter()
        .find_map(|k| p.get(*k).and_then(|v| v.as_str()))
        .or_else(|| {
            p.get("data").and_then(|d| {
                ["sessionId", "session_id", "state"]
                    .iter()
                    .find_map(|k| d.get(*k).and_then(|v| v.as_str()))
            })
        })
        .map(str::to_string)
        .or_else(|| {
            url.split("state=")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .map(str::to_string)
        })
        .ok_or_else(|| "zhidao login 未返回 sessionId/state,无法自动轮询".to_string())?;
    Ok(SsoLogin {
        url: url.to_string(),
        session_id,
    })
}

#[tauri::command]
pub async fn zhidao_cancel(app: AppHandle) -> Result<Value, String> {
    app.state::<ZhidaoConn>()
        .cancelled
        .store(true, Ordering::SeqCst);
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub async fn zhidao_logout() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let cmd = zhidao(&["clear"])?;
        let (ok, _code, so, se) = run(cmd)?;
        if ok {
            let _ = set_connected(false);
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
