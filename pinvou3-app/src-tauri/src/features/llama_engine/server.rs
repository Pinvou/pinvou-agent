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
/// 发送门（chat.rs）等待窗口与本值对齐，不得另设更短的超时。
pub(crate) const HEALTH_TIMEOUT: Duration = Duration::from_secs(120);
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
    // 线程数钉物理核数：llama.cpp 默认按逻辑核调度，超线程在纯 CPU 推理上
    // 通常零/负收益；GPU 档该值只影响少量 CPU 侧算子，无副作用。
    let threads = crate::platform::os::physical_core_count();
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
        // 高分辨率图像降采样上限(实测 4K 图视觉编码:默认 409s → 1024 上限 42s,
        // 总耗时 427s → 58s,7.4 倍)。1024 正是 Qwen-VL grounding 的建议下限,
        // 只压大图、不影响小图分辨率与准确性。核显/慢设备上必须,独显上无副作用。
        "--image-max-tokens".into(),
        "1024".into(),
        "-t".into(),
        threads.to_string().into(),
        // batch/ubatch 提到 1024：1024 视觉 token 不被默认 ubatch 512 切成
        // 两批，避免跨批 prefill 的额外开销与注意力截断。
        "--batch-size".into(),
        "1024".into(),
        "--ubatch-size".into(),
        "1024".into(),
        // FlashAttention：图像 token 多时显著降低 KV 访存与显存占用。
        "--flash-attn".into(),
        // KV cache q8_0 量化：视觉任务 KV 常驻占用大，q8_0 精度损失可忽略、
        // 内存近减半（8GB 内存机器跑 2B 档的保命项）。
        "--cache-type-k".into(),
        "q8_0".into(),
        "--cache-type-v".into(),
        "q8_0".into(),
        // mlock：模型常驻物理内存，防系统换页导致解码中途卡顿；
        // 权限/内存不足时 llama.cpp 仅告警继续运行，无失败风险。
        "--mlock".into(),
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
                if STOP_REQUESTED.swap(false, Ordering::SeqCst) {
                    // spawn/加载期间用户点了停止：此时 pid 尚未写入 guard、
                    // 停止标志无人消费会残留——必须在这里终结进程并落 Stopped，
                    // 否则引擎带病进入 Running、标志留到下次退出被误消费。
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    let _ = stderr_task.await;
                    transition_stopped(&app, "已停止".to_string());
                    return;
                }
                mark_running();
                emit_state(&app, "running", None);
                notify_sessions_changed(&app);
                spawn_warmup(port);
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

/// 幂等启动：Running/Starting 时直接 Ok(false)（并发防护，绝不把
/// "引擎已在运行或启动中" 当失败上报）；真正发起启动返回 Ok(true)。
/// 自动启动（发送门 / launch 后台）与手动启动共用，避免并发双启动。
pub(crate) async fn start_if_needed(
    app: &tauri::AppHandle,
    model_id: &str,
    device: EngineDevice,
) -> Result<bool, String> {
    {
        let guard = lock_runtime();
        if matches!(guard.phase, EnginePhase::Starting | EnginePhase::Running) {
            return Ok(false);
        }
    }
    match start(app, model_id, device).await {
        Ok(()) => Ok(true),
        // 锁外检查与 start 内部守卫之间的竞态窗口：已有人启动，归一为 Ok(false)。
        Err(error) if error.contains("引擎已在运行或启动中") => Ok(false),
        Err(error) => Err(error),
    }
}

/// 轮询等待引擎进入 Running（自动启动后、发送路由与 spawn 之前调用）。
/// 期间若转入 Stopped 且带错误 → Err(该错误)；超时 → Err(超时文案)。
/// 轮询用 tokio::time::sleep，不阻塞运行时。
pub(crate) async fn wait_until_running(timeout: Duration) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let snapshot = runtime_snapshot();
        if snapshot.phase == "running" {
            return Ok(());
        }
        if snapshot.phase == "stopped" {
            if let Some(error) = snapshot.last_error.filter(|e| !e.is_empty()) {
                return Err(error);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("等待本地引擎就绪超时（{}s）", timeout.as_secs()));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// 测试钩子：强制置 Running（bridge.rs 规则 0 单测用；改全局 RUNTIME，
/// 使用方需保证测试串行）。
#[cfg(test)]
pub(crate) fn force_running_for_test(port: u16) {
    let mut guard = lock_runtime();
    guard.phase = EnginePhase::Running;
    guard.port = Some(port);
}

/// 测试钩子：复位 RUNTIME 到默认（配合 force_running_for_test 的收尾，
/// 避免污染后续测试的引擎运行态）。
#[cfg(test)]
pub(crate) fn reset_runtime_for_test() {
    let mut guard = lock_runtime();
    *guard = EngineRuntime::default();
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

        // 重启窗口内的 stop()：pid 已清空、新进程未 spawn，kill 无从下手。
        // 这里补查停止标志，避免停止请求在自愈流程中被吞掉。
        if STOP_REQUESTED.swap(false, Ordering::SeqCst) {
            transition_stopped(&app, "已停止".to_string());
            return;
        }

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
        // 自愈优先复用旧端口：端点 URL 不变，已快照本地端点的会话无需重建；
        // 仅当旧端口被占时才退避到随机端口（此时靠会话失效钩子 bump revision）。
        let new_port = if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            port
        } else {
            pick_free_port().unwrap_or(port)
        };
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
                if STOP_REQUESTED.swap(false, Ordering::SeqCst) {
                    // 就绪期间收到了停止请求（重启窗口边界）：杀掉刚就绪的进程。
                    let _ = new_child.kill().await;
                    let _ = new_child.wait().await;
                    let _ = stderr_task.await;
                    transition_stopped(&app, "已停止".to_string());
                    return;
                }
                mark_running();
                emit_state(&app, "running", None);
                notify_sessions_changed(&app);
                spawn_warmup(new_port);
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
    // 手动停止/崩溃终停同样使会话端点快照失效（回落 vision_model_id 规则）。
    notify_sessions_changed(app);
}

// ---------------- 会话失效钩子 ----------------
// 引擎运行态翻转（进入 Running / 落 Stopped）时由宿主（lib.rs）注入的回调
// bump 会话模型 revision，强制 EngineConfig 重快照——本地端点只在会话
// spawn 时读取（vision_endpoint 快照语义）。llama_engine 不反向依赖
// assistant::EnginePool，故用注入钩子而不是直接调用。

type SessionInvalidationHook = Box<dyn Fn(&tauri::AppHandle) + Send + Sync>;
static SESSION_INVALIDATION_HOOK: OnceLock<SessionInvalidationHook> = OnceLock::new();

/// 注册会话失效钩子（lib.rs setup 时调用一次；重复注册后者被忽略）。
pub fn set_session_invalidation_hook(hook: SessionInvalidationHook) {
    let _ = SESSION_INVALIDATION_HOOK.set(hook);
}

fn notify_sessions_changed(app: &tauri::AppHandle) {
    if let Some(hook) = SESSION_INVALIDATION_HOOK.get() {
        hook(app);
    }
}

// ---------------- warmup ----------------

/// 内置 64×64 测试图（纯色 PNG）：warmup 与微基准探测共用，
/// 够走通视觉编码全链路又足够小。
const WARMUP_IMAGE_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAATklEQVR42u3PQQkAAAgEsAtoNDtrBN/CYAWW6nktAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgKXBZ60cbTWfPOjAAAAAElFTkSuQmCC";

/// 引擎进入 Running 后后台预热：发一次最小请求（内置小图 + max_tokens 16），
/// 把 mmproj/视觉编码器初始化从首个真实请求里挪掉，消除冷启动体感。
/// 失败静默（预热是纯优化，绝不影响引擎可用性）。
fn spawn_warmup(port: u16) {
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
        {
            Ok(client) => client,
            Err(_) => return,
        };
        let payload = serde_json::json!({
            // 单模型模式忽略 model 字段。
            "model": "warmup",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Describe this image briefly." },
                    { "type": "image_url", "image_url": {
                        "url": format!("data:image/png;base64,{WARMUP_IMAGE_BASE64}")
                    } }
                ]
            }],
            "max_tokens": 16,
        });
        let _ = client
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .json(&payload)
            .send()
            .await;
    });
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
            // PR3 启动参数调优：物理核线程数 / batch 1024 / flash-attn /
            // KV q8_0 / mlock（缺一项即回归，逐项断言）。
            assert!(
                text.windows(2)
                    .any(|w| w[0] == "-t" && w[1].parse::<usize>().is_ok()),
                "must pass physical core count via -t; got {text:?}"
            );
            assert!(text.windows(2).any(|w| w[0] == "--batch-size" && w[1] == "1024"));
            assert!(text.windows(2).any(|w| w[0] == "--ubatch-size" && w[1] == "1024"));
            assert!(text.iter().any(|a| a == "--flash-attn"));
            assert!(text.windows(2).any(|w| w[0] == "--cache-type-k" && w[1] == "q8_0"));
            assert!(text.windows(2).any(|w| w[0] == "--cache-type-v" && w[1] == "q8_0"));
            assert!(text.iter().any(|a| a == "--mlock"));
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
