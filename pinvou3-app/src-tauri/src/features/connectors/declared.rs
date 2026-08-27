//! 声明式 CLI 连接器包（Upload）的通用编排器（§14.3 授权契约，阶段 3a）。
//!
//! 契约（`auth.steps` 非空的包）：CLI 约定子命令与 JSON 输出——
//! - `<bin> auth begin --step <step_id> --json` → `{"qr_url"|"browser_url","ticket"}`
//! - `<bin> auth poll --ticket <t> --json` → `{"state":"pending|done|error","message"?}`
//! - `<bin> auth logout`
//! 不满足契约（无 steps）= manual：CLI 自行交互授权，`connect_begin` 返回
//! `{started:false, mode:"manual"}`，前端提示用户自行在终端完成授权。
//!
//! 编排核心 [`run_declared_connect_flow`] 与进程/事件底座解耦（run/emit 注入），
//! 测试不起真实子进程即可锁定契约流程；AppHandle 出口在 [`declared_connect_begin`]。

use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::features::connectors::connector_cli::{self as cc, ConnectorConn};
use crate::features::marketplace::plugin_import::{CliAuthStep, CliAuthStepKind, CliConnectorDecl};

/// 编排核心向外发射的事件（与 cc::emit_* 统一契约一一对应；注入形式便于测试，
/// qr_data_url 的渲染在 AppHandle 出口完成）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeclaredEvent {
    Qr {
        step: String,
        url: String,
        browser_auth: bool,
    },
    Phase {
        step: String,
        state: String,
    },
    Connected,
    Error {
        step: String,
        message: String,
    },
}

/// 编排核心参数（全部注入：子进程执行、事件出口、槽位、节奏）。
pub(crate) struct DeclaredConnectParams<'a> {
    pub id: &'a str,
    pub steps: &'a [CliAuthStep],
    pub conn: &'a ConnectorConn,
    /// 执行 `<bin> <args>`，返回 (success, stdout, stderr)。
    pub run: &'a dyn Fn(&[&str]) -> Result<(bool, String, String), String>,
    pub emit: &'a dyn Fn(DeclaredEvent),
    /// poll 间隔与总超时（生产 2s / 300s；测试注入毫秒级）。
    pub poll_interval: Duration,
    pub poll_timeout: Duration,
}

/// 契约驱动连接流程：按 auth.steps 顺序执行「begin 拿 URL → 展示 → poll 等完成」。
/// 取消静默返回；任一步失败 emit Error 并中止（与内置连接器同语义）。
pub(crate) fn run_declared_connect_flow(p: &DeclaredConnectParams) {
    for step in p.steps {
        if p.conn.is_cancelled(p.id) {
            return; // 取消：静默
        }
        // 1) begin：拿授权 URL + ticket
        let begin = (p.run)(&["auth", "begin", "--step", &step.id, "--json"]);
        let (url, ticket) = match begin {
            Ok((true, so, se)) => {
                let parsed = cc::parse_json(&so).or_else(|| cc::parse_json(&se));
                let url = parsed
                    .as_ref()
                    .and_then(|v| {
                        v.get("qr_url")
                            .or_else(|| v.get("browser_url"))
                            .and_then(|v| v.as_str())
                    })
                    .map(str::to_string);
                let ticket = parsed
                    .as_ref()
                    .and_then(|v| v.get("ticket"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                match (url, ticket) {
                    (Some(url), Some(ticket)) => (url, ticket),
                    _ => {
                        (p.emit)(DeclaredEvent::Error {
                            step: step.id.clone(),
                            message: format!(
                                "auth begin 未按契约返回 qr_url/browser_url + ticket（步骤 '{}'）",
                                step.id
                            ),
                        });
                        return;
                    }
                }
            }
            Ok((false, _, se)) => {
                (p.emit)(DeclaredEvent::Error {
                    step: step.id.clone(),
                    message: format!("auth begin 失败（步骤 '{}'）: {se}", step.id),
                });
                return;
            }
            Err(e) => {
                (p.emit)(DeclaredEvent::Error {
                    step: step.id.clone(),
                    message: format!("auth begin 启动失败（步骤 '{}'）: {e}", step.id),
                });
                return;
            }
        };
        (p.emit)(DeclaredEvent::Qr {
            step: step.id.clone(),
            url,
            browser_auth: step.kind == CliAuthStepKind::Browser,
        });

        // 2) poll：等本步完成
        let start = std::time::Instant::now();
        loop {
            if p.conn.is_cancelled(p.id) {
                return; // 取消：静默
            }
            if start.elapsed() > p.poll_timeout {
                (p.emit)(DeclaredEvent::Error {
                    step: step.id.clone(),
                    message: format!("授权超时（步骤 '{}' 未完成）", step.id),
                });
                return;
            }
            match (p.run)(&["auth", "poll", "--ticket", &ticket, "--json"]) {
                Ok((_, so, se)) => {
                    let parsed = cc::parse_json(&so).or_else(|| cc::parse_json(&se));
                    let state = parsed
                        .as_ref()
                        .and_then(|v| v.get("state"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("pending")
                        .to_string();
                    match state.as_str() {
                        "done" => {
                            (p.emit)(DeclaredEvent::Phase {
                                step: step.id.clone(),
                                state: "done".to_string(),
                            });
                            break;
                        }
                        "error" => {
                            let message = parsed
                                .as_ref()
                                .and_then(|v| v.get("message"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("授权失败")
                                .to_string();
                            (p.emit)(DeclaredEvent::Error {
                                step: step.id.clone(),
                                message,
                            });
                            return;
                        }
                        _ => std::thread::sleep(p.poll_interval),
                    }
                }
                Err(e) => {
                    (p.emit)(DeclaredEvent::Error {
                        step: step.id.clone(),
                        message: format!("auth poll 启动失败: {e}"),
                    });
                    return;
                }
            }
        }
    }
    // 全部步完成：登记（source 保留 Upload，清 degraded）+ 发射 connected
    cc::bundle_store_on_connected_upload(p.id);
    (p.emit)(DeclaredEvent::Connected);
}

/// 声明式包 `<bin> <args>` 子进程构造（抑黑窗，与内置连接器同一平台入口）。
fn declared_cli_cmd(bin: &str, args: &[&str]) -> std::process::Command {
    let mut c = crate::platform::os::connector_cli_command(bin, bin);
    c.args(args);
    c
}

/// 开始连接（通用命令分派入口）：manual（无 steps）返回结构化告知；
/// 否则立即返回 `{started:true}`，进度经统一事件 `connector:event` 上报。
pub async fn declared_connect_begin(
    app: &AppHandle,
    id: &str,
    decl: &CliConnectorDecl,
) -> Result<Value, String> {
    let steps = decl
        .auth
        .as_ref()
        .map(|a| a.steps.clone())
        .unwrap_or_default();
    if steps.is_empty() {
        // manual：CLI 自行交互授权，宿主无编排（提示前端引导用户去终端）
        return Ok(json!({ "started": false, "mode": "manual" }));
    }
    app.state::<ConnectorConn>().reset(id);
    let app2 = app.clone();
    let id_owned = id.to_string();
    let bin = decl.bin.clone();
    tokio::task::spawn_blocking(move || {
        let conn = app2.state::<ConnectorConn>();
        let run = |args: &[&str]| cc::run(declared_cli_cmd(&bin, args));
        let emit = |event: DeclaredEvent| match event {
            DeclaredEvent::Qr {
                step,
                url,
                browser_auth,
            } => {
                let qr = cc::make_qr(&url);
                cc::emit_qr(
                    &app2,
                    &id_owned,
                    &step,
                    &url,
                    &qr,
                    browser_auth.then_some(true),
                    None,
                );
            }
            DeclaredEvent::Phase { step, state } => cc::emit_phase(&app2, &id_owned, &step, &state),
            DeclaredEvent::Connected => cc::emit_connected(&app2, &id_owned, false),
            DeclaredEvent::Error { step, message } => {
                cc::emit_error(&app2, &id_owned, &step, &message);
            }
        };
        run_declared_connect_flow(&DeclaredConnectParams {
            id: &id_owned,
            steps: &steps,
            conn: &conn,
            run: &run,
            emit: &emit,
            poll_interval: Duration::from_secs(2),
            poll_timeout: Duration::from_secs(300),
        });
    });
    Ok(json!({ "started": true }))
}

/// 声明式包 CLI 就位/修复（通用 `ensure_cli` 分派）：按声明重新下载校验
/// （同版本同哈希已在盘则秒返回）。tmeet 式 npm 路径不适用于声明式包
/// （下载 pin 是契约的一部分）。
pub async fn declared_ensure_cli(id: &str, decl: &CliConnectorDecl) -> Result<Value, String> {
    let decl = decl.clone();
    let id = id.to_string();
    tokio::task::spawn_blocking(move || {
        let platform_key = crate::platform::paths::connector_platform_dir(
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
        .ok_or("当前平台不支持声明式 CLI 连接器")?;
        let artifact = decl
            .platforms
            .as_ref()
            .and_then(|p| p.get(platform_key))
            .ok_or_else(|| {
                format!("CLI 连接器 platforms 缺当前平台 '{platform_key}' 的下载声明")
            })?;
        let source = crate::platform::connector_installer::DeclaredCliArtifact {
            bin: decl.bin.clone(),
            version: decl
                .version
                .clone()
                .ok_or_else(|| format!("CLI 连接器 '{id}' 声明缺 version"))?,
            url: artifact.url.clone(),
            archive_sha256: artifact.archive_sha256.clone(),
            binary_sha256: artifact.binary_sha256.clone(),
            license: decl.license.clone(),
        };
        let written = crate::platform::connector_installer::ensure_declared_native_cli(&source)?;
        Ok::<Value, String>(json!({ "ok": true, "already": !written }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 声明式包连接状态：二进制不在位 → `installed:false`；auth.steps 非空时先走
/// 契约 `auth status --json`（容忍 `state: done/authorized/ready` 或
/// `connected: true`）；CLI 无 status 能力（退出非零/无 JSON）或 manual →
/// 回退「二进制在位 + 登记态（非 degraded）」（注释即契约：M2 readiness 同口径）。
pub async fn declared_status(id: &str, decl: &CliConnectorDecl) -> Result<Value, String> {
    let decl = decl.clone();
    let id = id.to_string();
    tokio::task::spawn_blocking(move || {
        let version = decl.version.clone().unwrap_or_default();
        let exe = crate::platform::paths::assets_cli_dir(&decl.bin, &version)
            .join(crate::platform::connector_lock::executable_name(&decl.bin));
        if !exe.is_file() {
            return Ok::<Value, String>(json!({
                "ok": false, "connected": false, "installed": false
            }));
        }
        let degraded = crate::features::marketplace::store::BundleStore::new()
            .get(&id)
            .ok()
            .flatten()
            .and_then(|r| r.degraded)
            .is_some();
        let mut connected = !degraded;
        if decl.auth.as_ref().is_some_and(|a| !a.steps.is_empty()) {
            if let Ok((true, so, se)) =
                cc::run(declared_cli_cmd(&decl.bin, &["auth", "status", "--json"]))
            {
                let parsed = cc::parse_json(&so).or_else(|| cc::parse_json(&se));
                if let Some(v) = parsed {
                    connected = v
                        .get("connected")
                        .and_then(|c| c.as_bool())
                        .unwrap_or_else(|| {
                            matches!(
                                v.get("state").and_then(|s| s.as_str()),
                                Some("done" | "authorized" | "ready")
                            )
                        });
                }
                // 无 JSON → 保持回退值（CLI 无 status 能力）
            }
        }
        Ok::<Value, String>(json!({
            "ok": true, "connected": connected, "installed": true
        }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 声明式包断开：优先走契约 `auth logout`；CLI 无 logout 能力（启动失败/非零
/// 退出）时降级为仅清本地登记（记日志，不算断开失败）。
pub async fn declared_logout(id: &str, decl: &CliConnectorDecl) -> Result<Value, String> {
    let decl = decl.clone();
    let id = id.to_string();
    tokio::task::spawn_blocking(move || {
        match cc::run(declared_cli_cmd(&decl.bin, &["auth", "logout"])) {
            Ok((true, _, _)) => {}
            Ok((false, _, se)) => {
                log::warn!(
                    "[connectors] 声明式包 '{id}' auth logout 失败，降级为仅清本地登记: {se}"
                );
            }
            Err(e) => {
                log::warn!(
                    "[connectors] 声明式包 '{id}' 无 auth logout 能力，降级为仅清本地登记: {e}"
                );
            }
        }
        if let Err(e) = crate::features::marketplace::store::BundleStore::new()
            .mark_degraded(&id, "已断开授权：重新连接即可恢复")
        {
            log::warn!("[connectors] bundles.json 镜像写入失败（disconnect {id}）: {e}");
        }
        Ok::<Value, String>(json!({ "ok": true }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 声明式包技能门控说明：Upload 包技能是包内容（安装时落盘），**不做**
/// 内置连接器式「连接态门控写删」（其技能是内嵌资源可重放的缓存，语义不同）。
/// `apply_skills` 仅回报当前在盘状态，不写删内容。
pub async fn declared_apply_skills(id: &str) -> Result<Value, String> {
    let id = id.to_string();
    tokio::task::spawn_blocking(move || {
        let visible = crate::platform::paths::bundles_root()
            .join(&id)
            .join("skills")
            .is_dir();
        Ok::<Value, String>(json!({ "visible": visible }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 声明式包开关：无内置式停用标志文件，直接桥接统一 scope 禁用集
/// （execpolicy CLI 硬拦截与技能物化排除按包 id 生效）。
pub async fn declared_set_enabled(id: &str, enabled: bool) -> Result<Value, String> {
    let id = id.to_string();
    tokio::task::spawn_blocking(move || {
        crate::features::marketplace::sync_disabled_bundles_for_connector_switch(&id, enabled);
        Ok::<Value, String>(json!({ "ok": true, "visible": enabled }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// 声明式包开关态：`{connected(=二进制在位且非 degraded), enabled(恒 true——
/// 无独立停用态), visible(=技能目录在盘)}`。
pub async fn declared_skills_state(id: &str, decl: &CliConnectorDecl) -> Result<Value, String> {
    let decl = decl.clone();
    let id = id.to_string();
    tokio::task::spawn_blocking(move || {
        let version = decl.version.clone().unwrap_or_default();
        let exe = crate::platform::paths::assets_cli_dir(&decl.bin, &version)
            .join(crate::platform::connector_lock::executable_name(&decl.bin));
        let degraded = crate::features::marketplace::store::BundleStore::new()
            .get(&id)
            .ok()
            .flatten()
            .and_then(|r| r.degraded)
            .is_some();
        let visible = crate::platform::paths::bundles_root()
            .join(&id)
            .join("skills")
            .is_dir();
        Ok::<Value, String>(json!({
            "connected": exe.is_file() && !degraded,
            "enabled": true,
            "visible": visible,
        }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    fn step(id: &str, kind: CliAuthStepKind) -> CliAuthStep {
        CliAuthStep {
            id: id.to_string(),
            kind,
            label: id.to_string(),
        }
    }

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-declared-test-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);
        f();
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 契约驱动全流程：两步链（qr + browser）的 CLI 调用参数与统一事件序列锁定；
    /// 连接成功后 degraded 清除（source 保留 Upload）。
    #[test]
    fn declared_flow_two_steps_event_sequence() {
        with_temp_home(|| {
            // 预置 degraded 登记（连接成功应清除）
            let store = crate::features::marketplace::store::BundleStore::new();
            let mut record = crate::features::marketplace::store::BundleRecord::installed_now(
                "up-test",
                crate::features::marketplace::store::BundleSource::Upload("up.zip".to_string()),
            );
            record.degraded = Some("CLI 下载失败".to_string());
            store.upsert(record).unwrap();

            let conn = ConnectorConn::default();
            let events = StdMutex::new(Vec::new());
            let calls = StdMutex::new(Vec::new());
            let poll_count = StdMutex::new(0u32);
            let run = |args: &[&str]| {
                calls.lock().unwrap().push(args.join(" "));
                match args {
                    ["auth", "begin", "--step", step, "--json"] => Ok((
                        true,
                        format!(
                            r#"{{"qr_url":"https://example.com/auth?step={step}","ticket":"t-{step}"}}"#
                        ),
                        String::new(),
                    )),
                    ["auth", "poll", "--ticket", _, "--json"] => {
                        let mut c = poll_count.lock().unwrap();
                        *c += 1;
                        if *c >= 2 {
                            Ok((true, r#"{"state":"done"}"#.to_string(), String::new()))
                        } else {
                            Ok((true, r#"{"state":"pending"}"#.to_string(), String::new()))
                        }
                    }
                    _ => Ok((false, String::new(), "unexpected".to_string())),
                }
            };
            let emit = |ev: DeclaredEvent| events.lock().unwrap().push(ev);
            let steps = [
                step("register", CliAuthStepKind::Qr),
                step("authorize", CliAuthStepKind::Browser),
            ];
            run_declared_connect_flow(&DeclaredConnectParams {
                id: "up-test",
                steps: &steps,
                conn: &conn,
                run: &run,
                emit: &emit,
                poll_interval: Duration::from_millis(1),
                poll_timeout: Duration::from_secs(5),
            });

            assert_eq!(
                *events.lock().unwrap(),
                vec![
                    DeclaredEvent::Qr {
                        step: "register".to_string(),
                        url: "https://example.com/auth?step=register".to_string(),
                        browser_auth: false,
                    },
                    DeclaredEvent::Phase {
                        step: "register".to_string(),
                        state: "done".to_string(),
                    },
                    DeclaredEvent::Qr {
                        step: "authorize".to_string(),
                        url: "https://example.com/auth?step=authorize".to_string(),
                        browser_auth: true,
                    },
                    DeclaredEvent::Phase {
                        step: "authorize".to_string(),
                        state: "done".to_string(),
                    },
                    DeclaredEvent::Connected,
                ],
                "两步链事件序列"
            );
            let calls = calls.lock().unwrap();
            assert!(calls.contains(&"auth begin --step register --json".to_string()));
            assert!(calls.contains(&"auth begin --step authorize --json".to_string()));
            assert!(calls.contains(&"auth poll --ticket t-register --json".to_string()));
            assert!(calls.contains(&"auth poll --ticket t-authorize --json".to_string()));

            let record = store.get("up-test").unwrap().unwrap();
            assert!(record.degraded.is_none(), "连接成功应清 degraded");
            assert!(matches!(
                record.source,
                crate::features::marketplace::store::BundleSource::Upload(_)
            ));
        });
    }

    /// poll 报 error → Error 事件中止，不发 connected、不动登记。
    #[test]
    fn declared_flow_poll_error_aborts() {
        with_temp_home(|| {
            let conn = ConnectorConn::default();
            let events = StdMutex::new(Vec::new());
            let run = |args: &[&str]| match args {
                ["auth", "begin", ..] => Ok((
                    true,
                    r#"{"browser_url":"https://example.com/a","ticket":"t1"}"#.to_string(),
                    String::new(),
                )),
                ["auth", "poll", ..] => Ok((
                    true,
                    r#"{"state":"error","message":"user denied"}"#.to_string(),
                    String::new(),
                )),
                _ => Ok((false, String::new(), String::new())),
            };
            let emit = |ev: DeclaredEvent| events.lock().unwrap().push(ev);
            let steps = [step("authorize", CliAuthStepKind::Browser)];
            run_declared_connect_flow(&DeclaredConnectParams {
                id: "up-test",
                steps: &steps,
                conn: &conn,
                run: &run,
                emit: &emit,
                poll_interval: Duration::from_millis(1),
                poll_timeout: Duration::from_secs(5),
            });
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 2);
            assert!(matches!(
                &events[0],
                DeclaredEvent::Qr {
                    browser_auth: true,
                    ..
                }
            ));
            assert_eq!(
                events[1],
                DeclaredEvent::Error {
                    step: "authorize".to_string(),
                    message: "user denied".to_string(),
                }
            );
        });
    }

    /// 取消即静默（零事件）；begin 返回非契约 JSON → Error 事件。
    #[test]
    fn declared_flow_cancel_and_bad_begin() {
        let conn = ConnectorConn::default();
        let events = StdMutex::new(Vec::new());
        let run = |_: &[&str]| Ok((true, r#"{"unexpected":true}"#.to_string(), String::new()));
        let emit = |ev: DeclaredEvent| events.lock().unwrap().push(ev);
        let steps = [step("authorize", CliAuthStepKind::Qr)];
        conn.cancel("up-test");
        run_declared_connect_flow(&DeclaredConnectParams {
            id: "up-test",
            steps: &steps,
            conn: &conn,
            run: &run,
            emit: &emit,
            poll_interval: Duration::from_millis(1),
            poll_timeout: Duration::from_secs(5),
        });
        assert!(events.lock().unwrap().is_empty(), "已取消应零事件");

        let conn2 = ConnectorConn::default();
        run_declared_connect_flow(&DeclaredConnectParams {
            id: "up-test",
            steps: &steps,
            conn: &conn2,
            run: &run,
            emit: &emit,
            poll_interval: Duration::from_millis(1),
            poll_timeout: Duration::from_secs(5),
        });
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], DeclaredEvent::Error { message, .. } if message.contains("未按契约返回")),
            "非契约 begin 应 Error，实际: {events:?}"
        );
    }

    /// logout 降级：CLI 不存在（无 logout 能力）时仍 Ok 且本地登记 degraded。
    #[test]
    fn declared_logout_degrades_to_local_cleanup() {
        with_temp_home(|| {
            let store = crate::features::marketplace::store::BundleStore::new();
            store
                .upsert(
                    crate::features::marketplace::store::BundleRecord::installed_now(
                        "up-test",
                        crate::features::marketplace::store::BundleSource::Upload(
                            "up.zip".to_string(),
                        ),
                    ),
                )
                .unwrap();
            let decl = CliConnectorDecl {
                id: "up-test".to_string(),
                bin: "definitely-not-exists-bin-xyz".to_string(),
                version: Some("1.0.0".to_string()),
                platforms: None,
                skills_dir: None,
                auth: None,
                license: None,
            };
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let result = rt
                .block_on(declared_logout("up-test", &decl))
                .expect("logout 失败降级为本地清理，不应 Err");
            assert_eq!(result["ok"], true);
            let record = store.get("up-test").unwrap().unwrap();
            assert!(record.degraded.is_some(), "断开后应记 degraded");
        });
    }
}
