//! llama-server 子进程生命周期：端口选择、健康探测、stderr 诊断、停止与崩溃自愈。
//!
//! 进程句柄由 watcher 任务持有（`kill_on_drop(true)`），static 只存 pid；
//! 停止时经 `platform::os::kill_pid_tree` 整树终结（Windows taskkill /F /T）。

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::Emitter;

use super::download::{self, LlamaModelSpec};
use super::{EngineDevice, llama_engine_dir};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum EnginePhase {
    #[default]
    Idle,
    Starting,
    Running,
    Stopped,
}

impl EnginePhase {
    pub(crate) fn name(self) -> &'static str {
        match self {
            EnginePhase::Idle => "idle",
            EnginePhase::Starting => "starting",
            EnginePhase::Running => "running",
            EnginePhase::Stopped => "stopped",
        }
    }
}

/// 运行期可变状态（进程句柄不落 static，由 watcher 任务持有）。
#[derive(Default)]
pub(crate) struct EngineRuntime {
    pub phase: EnginePhase,
    pub port: Option<u16>,
    pub pid: Option<u32>,
    pub device: Option<EngineDevice>,
    pub active_model: Option<String>,
    pub stderr_tail: VecDeque<String>,
    pub last_error: Option<String>,
    crash_reboot_count: u32,
    last_crash_at: Option<Instant>,
}

/// 状态快照（status() 用，锁外构建）。
#[derive(Debug, Clone)]
pub(crate) struct EngineRuntimeSnapshot {
    pub phase: &'static str,
    pub port: Option<u16>,
    pub pid: Option<u32>,
    pub device: Option<EngineDevice>,
    pub active_model: Option<String>,
    pub last_error: Option<String>,
    pub stderr_tail: Vec<String>,
}

static RUNTIME: OnceLock<Mutex<EngineRuntime>> = OnceLock::new();
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// 首次加载模型（CPU 慢）与自动重启后的就绪等待上限。
const HEALTH_TIMEOUT: Duration = Duration::from_secs(120);
const CRASH_REBOOT_WINDOW: Duration = Duration::from_secs(60);
/// 窗口内允许的自愈次数（超过则停等用户手动）。
const MAX_CRASH_REBOOTS: u32 = 2;
const STDERR_TAIL_CAP: usize = 20;
const STDERR_LINE_CAP: usize = 2000;

fn runtime() -> &'static Mutex<EngineRuntime> {
    RUNTIME.get_or_init(|| Mutex::new(EngineRuntime::default()))
}

fn lock_runtime() -> std::sync::MutexGuard<'static, EngineRuntime> {
    runtime().lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn runtime_snapshot() -> EngineRuntimeSnapshot {
    let guard = lock_runtime();
    EngineRuntimeSnapshot {
        phase: guard.phase.name(),
        port: guard.port,
        pid: guard.pid,
        device: guard.device,
        active_model: guard.active_model.clone(),
        last_error: guard.last_error.clone(),
        stderr_tail: guard.stderr_tail.iter().cloned().collect(),
    }
}

/// 引擎运行中返回 OpenAI 兼容端点；否则 None（bridge.rs 接线点）。
pub(crate) fn running_endpoint() -> Option<String> {
    let guard = lock_runtime();
    if guard.phase == EnginePhase::Running {
        Some(format!("http://127.0.0.1:{}/v1", guard.port?))
    } else {
        None
    }
}

/// 找空闲端口：bind 127.0.0.1:0 取端口后释放（毫秒级竞态窗口，v1 接受）。
pub(crate) fn pick_free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("无法绑定本地端口: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("读取端口失败: {e}"))?
        .port();
    drop(listener);
    Ok(port)
}

/// 构造 llama-server 启动参数（纯函数，便于单测）。
/// 单模型模式忽略请求体 `model` 字段，故不设 `--alias`。
pub(crate) fn build_args(
    bin: &Path,
    spec: &LlamaModelSpec,
    port: u16,
    device: EngineDevice,
) -> Vec<OsString> {
    let ngl = match device {
        EngineDevice::Gpu => "99",
        EngineDevice::Cpu => "0",
    };
    vec![
        bin.as_os_str().to_owned(),
        "--model".into(),
        download::model_gguf_path(spec).into_os_string(),
        "--mmproj".into(),
        download::mmproj_path(spec).into_os_string(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string().into(),
        "--ctx-size".into(),
        "8192".into(),
        "-ngl".into(),
        ngl.into(),
        "--no-webui".into(),
    ]
}

/// 启动引擎（幂等守卫：Running/Starting 时拒绝重复启动）。
pub(crate) async fn start(
    app: &tauri::AppHandle,
    model_id: &str,
    device: EngineDevice,
) -> Result<(), String> {
    let spec = download::model_spec(model_id)?;
    let bin = download::engine_binary_path();
    if !bin.is_file() {
        return Err("引擎未安装，请先在设置中下载引擎".to_string());
    }
    if !download::model_files_verified(spec) {
        return Err(format!("模型 {model_id} 未就绪，请先在设置中下载模型"));
    }
    let port = pick_free_port()?;
    {
        let mut guard = lock_runtime();
        if matches!(guard.phase, EnginePhase::Starting | EnginePhase::Running) {
            return Err("引擎已在运行或启动中".to_string());
        }
        guard.phase = EnginePhase::Starting;
        guard.port = Some(port);
        guard.device = Some(device);
        guard.active_model = Some(model_id.to_string());
        guard.last_error = None;
        guard.crash_reboot_count = 0;
        guard.last_crash_at = None;
        guard.stderr_tail.clear();
    }
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    emit_state(app, "starting", None);

    std::fs::create_dir_all(llama_engine_dir())
        .map_err(|e| format!("创建引擎目录失败: {e}"))?;
    let mut child = match spawn_server(&bin, &build_args(&bin, spec, port, device)).await {
        Ok(child) => child,
        Err(error) => {
            let mut guard = lock_runtime();
            guard.phase = EnginePhase::Idle;
            guard.pid = None;
            guard.last_error = Some(error.clone());
            drop(guard);
            emit_state(app, "stopped", Some(error.clone()));
            return Err(error);
        }
    };
    {
        let mut guard = lock_runtime();
        guard.pid = child.id().filter(|id| *id > 0);
    }

    let app = app.clone();
    tokio::spawn(async move {
        let stderr = child.stderr.take();
        let stderr_task = tokio::spawn(async move {
            if let Some(stderr) = stderr {
                drain_stderr(stderr).await;
            }
        });

        match wait_until_healthy_or_exit(&mut child, port).await {
            HealthOutcome::Healthy => {
                mark_running();
                emit_state(&app, "running", None);
                watch_running(app, child, stderr_task, port).await;
            }
            HealthOutcome::Exited(status) => {
                let _ = stderr_task.await;
                let reason = if STOP_REQUESTED.swap(false, Ordering::SeqCst) {
                    "已停止".to_string()
                } else {
                    diagnose_exit(status, "启动失败")
                };
                transition_stopped(&app, reason);
            }
            HealthOutcome::Timeout(error) => {
                if STOP_REQUESTED.swap(false, Ordering::SeqCst) {
                    transition_stopped(&app, "已停止".to_string());
                    return;
                }
                let _ = child.kill().await;
                let _ = child.wait().await;
                let _ = stderr_task.await;
                let reason = if error.is_empty() {
                    format!("等待服务就绪超时（{}s）", HEALTH_TIMEOUT.as_secs())
                } else {
                    format!(
                        "等待服务就绪超时（{}s）：{error}",
                        HEALTH_TIMEOUT.as_secs()
                    )
                };
                transition_stopped(&app, reason);
            }
        }
    });
    Ok(())
}

/// 停止引擎：置位停止标志 + 整树终结（watcher 收口为 Stopped）。
pub(crate) fn stop() {
    STOP_REQUESTED.store(true, Ordering::SeqCst);
    let pid = lock_runtime().pid;
    if let Some(pid) = pid {
        crate::platform::os::kill_pid_tree(pid);
    }
}

enum HealthOutcome {
    Healthy,
    Exited(Option<std::process::ExitStatus>),
    Timeout(String),
}

async fn wait_until_healthy_or_exit(
    child: &mut tokio::process::Child,
    port: u16,
) -> HealthOutcome {
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    let mut last_error = String::new();
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return HealthOutcome::Exited(Some(status));
        }
        match check_health(port).await {
            Ok(()) => return HealthOutcome::Healthy,
            Err(error) => last_error = error,
        }
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
    }
    HealthOutcome::Timeout(last_error)
}

/// 运行中监视：崩溃自愈（60s 窗口内 <MAX_CRASH_REBOOTS 次则自动重启）。
async fn watch_running(
    app: tauri::AppHandle,
    mut child: tokio::process::Child,
    mut stderr_task: tokio::task::JoinHandle<()>,
    port: u16,
) {
    loop {
        let status = child.wait().await.ok();
        let _ = stderr_task.await;
        if STOP_REQUESTED.swap(false, Ordering::SeqCst) {
            transition_stopped(&app, "已停止".to_string());
            return;
        }
        let reason = diagnose_exit(status, "引擎进程异常退出");
        let should_reboot = {
            let mut guard = lock_runtime();
            let now = Instant::now();
            if guard
                .last_crash_at
                .map(|t| now.duration_since(t) > CRASH_REBOOT_WINDOW)
                .unwrap_or(true)
            {
                guard.crash_reboot_count = 0;
            }
            guard.crash_reboot_count += 1;
            guard.last_crash_at = Some(now);
            guard.crash_reboot_count < MAX_CRASH_REBOOTS
        };
        if !should_reboot {
            transition_stopped(&app, format!("引擎连续崩溃已停止：{reason}"));
            return;
        }

        // 自动重启（复用同一模型与设备）。
        {
            let mut guard = lock_runtime();
            guard.phase = EnginePhase::Starting;
            guard.pid = None;
            guard.last_error = Some(format!("引擎异常退出，正在自动重启：{reason}"));
        }
        emit_state(&app, "starting", None);
        tokio::time::sleep(Duration::from_secs(2)).await;

        let (model_id, device) = {
            let guard = lock_runtime();
            (guard.active_model.clone(), guard.device)
        };
        let Some(device) = device else {
            transition_stopped(&app, "自动重启失败：运行配置缺失".to_string());
            return;
        };
        let Some(model_id) = model_id else {
            transition_stopped(&app, "自动重启失败：模型不可用".to_string());
            return;
        };
        let Ok(spec) = download::model_spec(&model_id) else {
            transition_stopped(&app, "自动重启失败：模型不可用".to_string());
            return;
        };
        let bin = download::engine_binary_path();
        let new_port = pick_free_port().unwrap_or(port);
        let Ok(mut new_child) = spawn_server(&bin, &build_args(&bin, spec, new_port, device)).await
        else {
            transition_stopped(&app, "自动重启失败：无法启动引擎进程".to_string());
            return;
        };
        {
            let mut guard = lock_runtime();
            guard.port = Some(new_port);
            guard.pid = new_child.id().filter(|id| *id > 0);
        }
        let stderr = new_child.stderr.take();
        stderr_task = tokio::spawn(async move {
            if let Some(stderr) = stderr {
                drain_stderr(stderr).await;
            }
        });
        match wait_until_healthy_or_exit(&mut new_child, new_port).await {
            HealthOutcome::Healthy => {
                mark_running();
                emit_state(&app, "running", None);
            }
            HealthOutcome::Exited(status) => {
                let _ = stderr_task.await;
                let reason = diagnose_exit(status, "自动重启后启动失败");
                transition_stopped(&app, reason);
                return;
            }
            HealthOutcome::Timeout(error) => {
                let _ = new_child.kill().await;
                let _ = new_child.wait().await;
                let _ = stderr_task.await;
                let reason = if error.is_empty() {
                    "自动重启后就绪超时".to_string()
                } else {
                    format!("自动重启后就绪超时：{error}")
                };
                transition_stopped(&app, reason);
                return;
            }
        }
        child = new_child;
    }
}

fn mark_running() {
    let mut guard = lock_runtime();
    guard.phase = EnginePhase::Running;
    guard.last_error = None;
}

fn transition_stopped(app: &tauri::AppHandle, reason: String) {
    {
        let mut guard = lock_runtime();
        guard.phase = EnginePhase::Stopped;
        guard.pid = None;
        guard.last_error = Some(reason.clone());
    }
    emit_state(app, "stopped", Some(reason));
}

fn emit_state(app: &tauri::AppHandle, phase: &'static str, error: Option<String>) {
    let snapshot = runtime_snapshot();
    let _ = app.emit(
        "llama-engine:state",
        serde_json::json!({
            "phase": phase,
            "port": snapshot.port,
            "pid": snapshot.pid,
            "device": snapshot.device,
            "model": snapshot.active_model,
            "error": error.or(snapshot.last_error),
        }),
    );
}

async fn spawn_server(
    bin: &Path,
    args: &[OsString],
) -> Result<tokio::process::Child, String> {
    let mut command = crate::platform::process::HiddenTokioCommand::new(bin);
    command.args(&args[1..]);
    // 钉死工作目录，防 llama.cpp 往源码树写日志（voice_asr 同款教训）。
    command.current_dir(llama_engine_dir());
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::piped());
    command.kill_on_drop(true);
    command.spawn().map_err(|e| format!("启动 llama-server 失败: {e}"))
}

/// 常驻排空 stderr（防管道写满阻塞子进程），保留尾部供诊断。
async fn drain_stderr(stderr: tokio::process::ChildStderr) {
    let mut reader = tokio::io::BufReader::new(stderr);
    let mut line = String::new();
    loop {
        line.clear();
        match tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let mut text = line.trim_end().to_string();
                if text.len() > STDERR_LINE_CAP {
                    text.truncate(STDERR_LINE_CAP);
                }
                let mut guard = lock_runtime();
                if guard.stderr_tail.len() >= STDERR_TAIL_CAP {
                    guard.stderr_tail.pop_front();
                }
                guard.stderr_tail.push_back(text);
            }
        }
    }
}

async fn check_health(port: u16) -> Result<(), String> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    let client = CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .build()
                .map_err(|e| format!("HTTP client 构建失败: {e}"))
        })
        .as_ref()
        .map_err(|e| e.clone())?;
    let response = client
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .await
        .map_err(|e| format!("健康检查请求失败: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("健康检查 HTTP {}", response.status()));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("健康检查响应解析失败: {e}"))?;
    let status = body
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    if status == "ok" {
        Ok(())
    } else {
        Err(format!("服务未就绪（{status}）"))
    }
}

fn diagnose_exit(status: Option<std::process::ExitStatus>, prefix: &str) -> String {
    let code = status
        .and_then(|s| s.code())
        .map(|c| c.to_string())
        .unwrap_or_else(|| "未知".to_string());
    let tail = stderr_tail_text();
    if tail.is_empty() {
        format!("{prefix}（退出码 {code}）")
    } else {
        format!("{prefix}（退出码 {code}）\n{tail}")
    }
}

fn stderr_tail_text() -> String {
    let guard = lock_runtime();
    guard
        .stderr_tail
        .iter()
        .rev()
        .take(6)
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::llama_engine::download::MODEL_Q3_K_S;

    #[test]
    fn pick_free_port_returns_bindable_port() {
        let port = pick_free_port().expect("must pick a port");
        let listener = std::net::TcpListener::bind(("127.0.0.1", port));
        assert!(listener.is_ok(), "picked port {port} must be bindable");
    }

    #[test]
    fn build_args_includes_required_flags() {
        let bin = Path::new("llama-server");
        let port = 4242;
        for (device, expected_ngl) in [
            (EngineDevice::Gpu, "99"),
            (EngineDevice::Cpu, "0"),
        ] {
            let args = build_args(bin, &MODEL_Q3_K_S, port, device);
            let text: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
            assert_eq!(text[0], "llama-server");
            assert!(
                text.windows(2).any(|w| w[0] == "--model"
                    && w[1].ends_with(MODEL_Q3_K_S.gguf.filename)),
                "must pass the gguf model path; got {text:?}"
            );
            assert!(text.iter().any(|a| a == "--mmproj"));
            assert!(text.iter().any(|a| a == "127.0.0.1"));
            assert!(text.iter().any(|a| a == port.to_string().as_str()));
            assert!(text.iter().any(|a| a == "8192"));
            assert!(text.windows(2).any(|w| w[0] == "-ngl" && w[1] == expected_ngl));
            assert!(text.iter().any(|a| a == "--no-webui"));
        }
    }

    #[test]
    fn device_parse_accepts_cpu_gpu_case_insensitive() {
        assert_eq!(EngineDevice::parse("cpu").unwrap(), EngineDevice::Cpu);
        assert_eq!(EngineDevice::parse("GPU").unwrap(), EngineDevice::Gpu);
        assert!(EngineDevice::parse("tpu").is_err());
    }

    #[test]
    fn phase_names_match_frontend_contract() {
        assert_eq!(EnginePhase::Idle.name(), "idle");
        assert_eq!(EnginePhase::Starting.name(), "starting");
        assert_eq!(EnginePhase::Running.name(), "running");
        assert_eq!(EnginePhase::Stopped.name(), "stopped");
    }
}
