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
use super::{llama_engine_dir, EngineDevice};

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

/// start() 的结构化错误：把「已在运行/启动中」的幂等守卫冲突与其他启动
/// 失败区分开，start_if_needed 按类型归一（不做中文字符串匹配控制流）。
#[derive(Debug)]
pub(crate) enum StartError {
    /// 引擎已在运行或启动中（幂等守卫拒绝重复启动，非真失败）。
    AlreadyRunning,
    /// 其他启动失败（值为用户可见文案）。
    Failed(String),
}

impl StartError {
    /// 转为用户可见文案（tauri 命令边界用；AlreadyRunning 的文案保持不变）。
    pub(crate) fn into_message(self) -> String {
        match self {
            StartError::AlreadyRunning => "引擎已在运行或启动中".to_string(),
            StartError::Failed(message) => message,
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
    /// 单次启动会话内的累计自愈次数（不随崩溃窗口清零，start() 重置）：
    /// 兜底低速崩溃（间隔恰好大于 CRASH_REBOOT_WINDOW 时窗口计数每次被
    /// 清零，无累计上限可无限自愈）。
    crash_reboot_total: u32,
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
/// 300s 的依据：并发下载显著拖慢 mmap 分页——实测下载 4B 模型
/// 期间 2B 模型冷加载从 ~12s 恶化到 ~50s（约 4 倍），120s 窗口
/// 在机械盘/更大并发下会被打穿，引擎被误杀、UI 只剩"启动中→
/// 超时"。300s 覆盖 6 倍恶化余量。
pub(crate) const HEALTH_TIMEOUT: Duration = Duration::from_secs(300);
const CRASH_REBOOT_WINDOW: Duration = Duration::from_secs(60);
/// 窗口内允许的自愈次数（超过则停等用户手动）。
const MAX_CRASH_REBOOTS: u32 = 2;
/// 单次启动会话内的累计自愈上限：窗口计数只拦密集崩溃，低速崩溃（间隔
/// 恰好大于 CRASH_REBOOT_WINDOW）每次都会清零窗口计数、可无限自愈；
/// 累计超限即放弃自愈转 Stopped（带 stderr 尾），等用户手动处理。
const MAX_CRASH_REBOOTS_TOTAL: u32 = 5;
/// stop() 遇到 pid 未写入的启动/spawn 窗口时，后台轮询等 pid 出现的上限。
const STOP_ORPHAN_PID_WAIT: Duration = Duration::from_secs(5);
/// 上述轮询的间隔。
const STOP_ORPHAN_POLL_INTERVAL: Duration = Duration::from_millis(50);
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
    let mut args = vec![
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
        // 新版 llama.cpp 的 -fa/--flash-attn 必须带值(on|off|auto)，裸 flag
        // 会打印 usage 并以退出码 1 拒绝启动。
        "--flash-attn".into(),
        "on".into(),
        // KV cache q8_0 量化：视觉任务 KV 常驻占用大，q8_0 精度损失可忽略、
        // 内存近减半（8GB 内存机器跑 2B 档的保命项）。
        "--cache-type-k".into(),
        "q8_0".into(),
        "--cache-type-v".into(),
        "q8_0".into(),
        // 注意：不传 --mlock。b10362 起 --mlock（及其替代 --load-mode mlock）
        // 与 -ngl 0（纯 CPU 加载）组合会在模型 mmap 阶段触发
        // llama-mmap.cpp GGML_ASSERT(addr) 崩溃（真机二分定位：两参单独
        // 均正常，组合必崩），属引擎侧回归；mlock 只是常驻内存优化，移除
        // 代价可接受。
        "-ngl".into(),
        ngl.into(),
        "--no-webui".into(),
    ];
    // CPU 档禁 mmproj GPU 卸载：llama.cpp 默认把视觉编码器卸载到 GPU
    // （--mmproj-offload enabled），但弱核显实测 CPU 编码反而快 ~27%
    // （UHD 750 Vulkan 1027 token 46.5s vs CPU 33.8s，两轮一致）；独显
    // 保持默认卸载，编码算力是真优势。
    if device == EngineDevice::Cpu {
        args.push("--no-mmproj-offload".into());
    }
    args
}

/// 启动引擎（幂等守卫：Running/Starting 时返回 StartError::AlreadyRunning
/// 拒绝重复启动，由调用方按类型归一）。
pub(crate) async fn start(
    app: &tauri::AppHandle,
    model_id: &str,
    device: EngineDevice,
) -> Result<(), StartError> {
    let spec = download::model_spec(model_id).map_err(StartError::Failed)?;
    let bin = download::engine_binary_path();
    if !bin.is_file() {
        return Err(StartError::Failed(
            "引擎未安装，请先在设置中下载引擎".to_string(),
        ));
    }
    if !download::model_files_verified(spec) {
        return Err(StartError::Failed(format!(
            "模型 {model_id} 未就绪，请先在设置中下载模型"
        )));
    }
    let port = pick_free_port().map_err(StartError::Failed)?;
    {
        let mut guard = lock_runtime();
        if matches!(guard.phase, EnginePhase::Starting | EnginePhase::Running) {
            return Err(StartError::AlreadyRunning);
        }
        guard.phase = EnginePhase::Starting;
        guard.port = Some(port);
        guard.device = Some(device);
        guard.active_model = Some(model_id.to_string());
        guard.last_error = None;
        guard.crash_reboot_count = 0;
        guard.crash_reboot_total = 0;
        guard.last_crash_at = None;
        guard.stderr_tail.clear();
        // 清停止标志必须与置 Starting 同临界区：若在锁外清空，stop() 会在
        // 「已置 Starting、标志未清」的窗口内置位并读到 Starting（走孤儿
        // pid 等待路径），随后被这次清空覆盖，停止请求整个丢失。
        STOP_REQUESTED.store(false, Ordering::SeqCst);
    }
    emit_state(app, "starting", None);

    std::fs::create_dir_all(llama_engine_dir())
        .map_err(|e| StartError::Failed(format!("创建引擎目录失败: {e}")))?;
    let mut child = match spawn_server(&bin, &build_args(&bin, spec, port, device)).await {
        Ok(child) => child,
        Err(error) => {
            let mut guard = lock_runtime();
            guard.phase = EnginePhase::Idle;
            guard.pid = None;
            guard.last_error = Some(error.clone());
            drop(guard);
            emit_state(app, "stopped", Some(error.clone()));
            return Err(StartError::Failed(error));
        }
    };
    {
        let mut guard = lock_runtime();
        guard.pid = child.id().filter(|id| *id > 0);
    }
    // spawn 窗口竞态补查：phase=Starting 到 pid 写入之间收到的 stop()
    // 读不到 pid、kill 无从下手；写入 pid 后立即复查停止标志，命中即
    // 自杀该子进程，防 llama-server（多 GB 内存）成孤儿。
    if STOP_REQUESTED.swap(false, Ordering::SeqCst) {
        if let Err(error) = kill_child(&mut child, "启动窗口停止").await {
            log::warn!("[llama-engine] {error}");
            transition_stopped(app, error);
            return Ok(());
        }
        let _ = child.wait().await;
        transition_stopped(app, "已停止".to_string());
        return Ok(());
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
                    if let Err(error) = kill_child(&mut child, "停止引擎").await {
                        // kill 失败不能报"已停止"：记警告并把失败原因落进状态。
                        log::warn!("[llama-engine] {error}");
                        let _ = stderr_task.await;
                        transition_stopped(&app, error);
                        return;
                    }
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
                if let Err(kill_error) = kill_child(&mut child, "启动超时收口").await {
                    log::warn!("[llama-engine] {kill_error}");
                }
                let _ = child.wait().await;
                let _ = stderr_task.await;
                let reason = if error.is_empty() {
                    format!("等待服务就绪超时（{}s）", HEALTH_TIMEOUT.as_secs())
                } else {
                    format!("等待服务就绪超时（{}s）：{error}", HEALTH_TIMEOUT.as_secs())
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
    let (pid, phase) = {
        let guard = lock_runtime();
        (guard.pid, guard.phase)
    };
    if let Some(pid) = pid {
        // kill_pid_tree 无返回值（签名归 platform/os 所有，此处不改）：
        // kill 失败时 watcher 不见退出、phase 不落 Stopped，UI 如实保持
        // 运行中；再次 stop() 可重试。
        crate::platform::os::kill_pid_tree(pid);
        return;
    }
    if phase == EnginePhase::Starting {
        // 启动/自愈 spawn 窗口：pid 尚未写入，直接 kill 无从下手，llama-server
        // （多 GB 内存）会成孤儿。后台有界轮询等 pid 出现再补杀；spawn 侧
        // 写入 pid 后也会复查 STOP_REQUESTED 立即自杀（start()/watch_running
        // 内的补查），双保险。
        std::thread::spawn(|| {
            if let Some(pid) = wait_orphan_pid(STOP_ORPHAN_PID_WAIT) {
                crate::platform::os::kill_pid_tree(pid);
            } else if STOP_REQUESTED.load(Ordering::SeqCst)
                && lock_runtime().phase == EnginePhase::Starting
            {
                log::warn!(
                    "[llama-engine] stop() 等待引擎 pid 超时（{}s），启动窗口内的子进程可能未被终结",
                    STOP_ORPHAN_PID_WAIT.as_secs()
                );
            }
        });
    }
}

/// stop() 时 pid 尚未写入（启动/自愈 spawn 窗口）的兜底：轮询等 pid 出现，
/// 上限 wait。提前放弃的情形：停止标志已被消费（spawn 侧自查已自杀）、
/// 相位离开 Starting（启动失败等已被其他路径收口）——均无杀的对象。
fn wait_orphan_pid(wait: Duration) -> Option<u32> {
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        if !STOP_REQUESTED.load(Ordering::SeqCst) {
            return None;
        }
        let (pid, phase) = {
            let guard = lock_runtime();
            (guard.pid, guard.phase)
        };
        if pid.is_some() {
            return pid;
        }
        if phase != EnginePhase::Starting {
            return None;
        }
        std::thread::sleep(STOP_ORPHAN_POLL_INTERVAL);
    }
    None
}

/// 停止/收口路径杀子进程：kill 失败返回带上下文的错误（调用方记警告并
/// 如实落进状态，不把已知失败的停止报成"已停止"）。
async fn kill_child(child: &mut tokio::process::Child, context: &str) -> Result<(), String> {
    child
        .kill()
        .await
        .map_err(|e| format!("{context}：结束引擎进程失败: {e}"))
}

/// 幂等启动：Running/Starting 时直接 Ok(false)（并发防护，绝不把
/// StartError::AlreadyRunning 当失败上报）；真正发起启动返回 Ok(true)。
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
        Err(StartError::AlreadyRunning) => Ok(false),
        Err(error) => Err(error.into_message()),
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
/// 调用方须在 locked_runtime_for_test() 锁下使用）。
#[cfg(test)]
pub(crate) fn force_running_for_test(port: u16) {
    let mut guard = lock_runtime();
    guard.phase = EnginePhase::Running;
    guard.port = Some(port);
}

/// 测试钩子：复位 RUNTIME 到默认（配合 force_running_for_test 的收尾，
/// 避免污染后续测试的引擎运行态）。同样须在 locked_runtime_for_test()
/// 锁下使用。
#[cfg(test)]
pub(crate) fn reset_runtime_for_test() {
    let mut guard = lock_runtime();
    *guard = EngineRuntime::default();
}

// 全局 RUNTIME/STOP_REQUESTED 是进程级共享状态：写它的测试（经
// force_running_for_test/reset_runtime_for_test 或本文件直接读写）与经
// bridge.rs resolve_vision_model_config（规则 0/3 → vision_endpoint →
// RUNTIME）间接读它的测试必须串行，否则默认并行 cargo test 下写测试的
// 「运行中」窗口会把读测试的断言翻掉（RAII reset 只能保证收尾，挡不住
// 并发读取）。锁源统一在本模块、crate 级共享（bridge.rs 测试复用同一
// 把），口径与 ENV_LOCK 一致：毒化经 into_inner 恢复，不绕过互斥。
#[cfg(test)]
pub(crate) static RUNTIME_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 获取 RUNTIME_TEST_LOCK（毒化视为可恢复）。任何直接调
/// force_running_for_test/reset_runtime_for_test、直接读写
/// RUNTIME/STOP_REQUESTED、或断言依赖引擎未运行态的测试，入口处先拿锁。
#[cfg(test)]
pub(crate) fn locked_runtime_for_test() -> std::sync::MutexGuard<'static, ()> {
    RUNTIME_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

enum HealthOutcome {
    Healthy,
    Exited(Option<std::process::ExitStatus>),
    Timeout(String),
}

async fn wait_until_healthy_or_exit(child: &mut tokio::process::Child, port: u16) -> HealthOutcome {
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

/// 运行中监视：崩溃自愈（60s 窗口内 <MAX_CRASH_REBOOTS 次且累计
/// <=MAX_CRASH_REBOOTS_TOTAL 次则自动重启，双上限任一超限即停）。
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
        let verdict = {
            let mut guard = lock_runtime();
            record_crash(&mut guard, Instant::now())
        };
        match verdict {
            CrashVerdict::Reboot => {}
            CrashVerdict::WindowExceeded => {
                transition_stopped(&app, format!("引擎连续崩溃已停止：{reason}"));
                return;
            }
            CrashVerdict::TotalExceeded => {
                transition_stopped(
                    &app,
                    format!(
                        "引擎反复崩溃已达自愈上限（{MAX_CRASH_REBOOTS_TOTAL} 次），已停止：{reason}"
                    ),
                );
                return;
            }
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
        // spawn 窗口竞态补查（同 start()）：自愈重启 spawn 期间收到的
        // stop() 读不到 pid；写入后立即复查停止标志，命中即终结刚 spawn
        // 的进程，防孤儿。
        if STOP_REQUESTED.swap(false, Ordering::SeqCst) {
            if let Err(error) = kill_child(&mut new_child, "重启窗口停止").await {
                log::warn!("[llama-engine] {error}");
                transition_stopped(&app, error);
                return;
            }
            let _ = new_child.wait().await;
            transition_stopped(&app, "已停止".to_string());
            return;
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
                    if let Err(error) = kill_child(&mut new_child, "停止引擎").await {
                        // kill 失败不能报"已停止"：记警告并把失败原因落进状态。
                        log::warn!("[llama-engine] {error}");
                        let _ = stderr_task.await;
                        transition_stopped(&app, error);
                        return;
                    }
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
                // 与初次启动分支同口径：就绪等待期间收到 stop() 时进程退出属
                // 预期停止，报「已停止」并消费标志，不走崩溃诊断文案。
                let reason = if STOP_REQUESTED.swap(false, Ordering::SeqCst) {
                    "已停止".to_string()
                } else {
                    diagnose_exit(status, "自动重启后启动失败")
                };
                transition_stopped(&app, reason);
                return;
            }
            HealthOutcome::Timeout(error) => {
                if let Err(kill_error) = kill_child(&mut new_child, "重启就绪超时收口").await
                {
                    log::warn!("[llama-engine] {kill_error}");
                }
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

/// record_crash 的判定结果（区分窗口上限与累计上限，停止文案如实区分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashVerdict {
    /// 允许自动重启。
    Reboot,
    /// CRASH_REBOOT_WINDOW 窗口内密集崩溃达 MAX_CRASH_REBOOTS。
    WindowExceeded,
    /// 累计自愈达 MAX_CRASH_REBOOTS_TOTAL（低速崩溃无限自愈的兜底）。
    TotalExceeded,
}

/// 崩溃计数与自愈判定（watch_running 锁内调用）：窗口内计数保留原逻辑——
/// 距上次崩溃超过 CRASH_REBOOT_WINDOW 则清零重计；累计计数（start() 才
/// 重置）另加 MAX_CRASH_REBOOTS_TOTAL 上限，兜底间隔恰好大于窗口的低速
/// 崩溃（窗口计数每次被清零，无累计上限可无限自愈）。
fn record_crash(guard: &mut EngineRuntime, now: Instant) -> CrashVerdict {
    if guard
        .last_crash_at
        .map(|t| now.duration_since(t) > CRASH_REBOOT_WINDOW)
        .unwrap_or(true)
    {
        guard.crash_reboot_count = 0;
    }
    guard.crash_reboot_count += 1;
    guard.crash_reboot_total += 1;
    guard.last_crash_at = Some(now);
    if guard.crash_reboot_total > MAX_CRASH_REBOOTS_TOTAL {
        CrashVerdict::TotalExceeded
    } else if guard.crash_reboot_count >= MAX_CRASH_REBOOTS {
        CrashVerdict::WindowExceeded
    } else {
        CrashVerdict::Reboot
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

async fn spawn_server(bin: &Path, args: &[OsString]) -> Result<tokio::process::Child, String> {
    let mut command = crate::platform::process::HiddenTokioCommand::new(bin);
    command.args(&args[1..]);
    // 钉死工作目录，防 llama.cpp 往源码树写日志（voice_asr 同款教训）。
    command.current_dir(llama_engine_dir());
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::piped());
    command.kill_on_drop(true);
    // Unix 上设为独立进程组组长：kill_pid_tree 的组杀(kill -9 -pgid)才能连
    // llama-server 派生的子进程一起终结，否则组杀恒 ESRCH、静默退化成单 pid。
    // Windows no-op（taskkill /T 已按树杀）。
    crate::platform::process::tokio_process_group_leader(&mut command);
    command
        .spawn()
        .map_err(|e| format!("启动 llama-server 失败: {e}"))
}

/// stderr 单行截断：按 UTF-8 字符边界截断——`String::truncate` 若切在多
/// 字节字符中间会 panic，杀死 drain 任务后管道写满会堵死引擎进程。
/// 复用 `platform::strings::truncate_utf8`（字节上限 + char 边界向下取整，
/// 语义与本地实现一致）。
fn truncate_at_line_cap(text: &mut String) {
    if text.len() > STDERR_LINE_CAP {
        let clipped = crate::platform::strings::truncate_utf8(text, STDERR_LINE_CAP).len();
        text.truncate(clipped);
    }
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
                truncate_at_line_cap(&mut text);
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
                // 整体超时：下载并发等 IO 打满时,server 可能接受连接但
                // 响应被拖住——没有总超时的请求会永远卡在一次轮询里,
                // 健康门的 120s 截止与退出分支全部失效(UI 永远"启动中")。
                .timeout(Duration::from_secs(3))
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
    use crate::features::llama_engine::download::MODEL_Q4_K_M;

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
        for (device, expected_ngl) in [(EngineDevice::Gpu, "99"), (EngineDevice::Cpu, "0")] {
            let args = build_args(bin, &MODEL_Q4_K_M, port, device);
            let text: Vec<String> = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert_eq!(text[0], "llama-server");
            assert!(
                text.windows(2)
                    .any(|w| w[0] == "--model" && w[1].ends_with(MODEL_Q4_K_M.gguf.filename)),
                "must pass the gguf model path; got {text:?}"
            );
            assert!(text.iter().any(|a| a == "--mmproj"));
            assert!(text.iter().any(|a| a == "127.0.0.1"));
            assert!(text.iter().any(|a| a == port.to_string().as_str()));
            assert!(text.iter().any(|a| a == "8192"));
            assert!(text
                .windows(2)
                .any(|w| w[0] == "-ngl" && w[1] == expected_ngl));
            assert!(text.iter().any(|a| a == "--no-webui"));
            // PR3 启动参数调优：物理核线程数 / batch 1024 / flash-attn /
            // KV q8_0（缺一项即回归，逐项断言）。
            assert!(
                text.windows(2)
                    .any(|w| w[0] == "-t" && w[1].parse::<usize>().is_ok()),
                "must pass physical core count via -t; got {text:?}"
            );
            assert!(text
                .windows(2)
                .any(|w| w[0] == "--batch-size" && w[1] == "1024"));
            assert!(text
                .windows(2)
                .any(|w| w[0] == "--ubatch-size" && w[1] == "1024"));
            assert!(
                text.windows(2)
                    .any(|w| w[0] == "--flash-attn" && w[1] == "on"),
                "flash-attn 必须带值(on|off|auto),裸 flag 会被新版引擎拒绝; got {text:?}"
            );
            assert!(text
                .windows(2)
                .any(|w| w[0] == "--cache-type-k" && w[1] == "q8_0"));
            assert!(text
                .windows(2)
                .any(|w| w[0] == "--cache-type-v" && w[1] == "q8_0"));
            // --mlock 与 -ngl 0 组合在 b10362 上 mmap 崩溃,必须不传。
            assert!(
                !text.iter().any(|a| a == "--mlock"),
                "--mlock 与 -ngl 0 组合会触发引擎 mmap 断言,不得再传; got {text:?}"
            );
            // mmproj GPU 卸载按设备条件化:CPU 档必须禁(弱核显实测 CPU 编码
            // 快 ~27%),GPU 档必须保持默认(不传该 flag)。
            let has_no_offload = text.iter().any(|a| a == "--no-mmproj-offload");
            assert_eq!(
                has_no_offload,
                device == EngineDevice::Cpu,
                "--no-mmproj-offload 应仅在 CPU 档出现; device={device:?}; got {text:?}"
            );
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

    #[test]
    fn truncate_at_line_cap_respects_utf8_boundaries() {
        // 中文（3 字节/字）：2000 落在第 667 字中间（1998..2001），截断
        // 必须退回字符边界 1998，而不是 panic 杀死 drain 任务。
        let mut text = "汉".repeat(700);
        assert_eq!(text.len(), 2100);
        truncate_at_line_cap(&mut text);
        assert_eq!(text.len(), 1998);
        assert!(text.is_char_boundary(text.len()));

        // 中文 + emoji（4 字节/字）混排：截断点落在 emoji 中间也不 panic。
        let mut mixed = "汉🦀".repeat(400); // 每单元 7 字节，共 2800
        truncate_at_line_cap(&mut mixed);
        assert!(mixed.len() <= STDERR_LINE_CAP);
        assert!(mixed.is_char_boundary(mixed.len()));

        // 未超上限：原样保留。
        let mut short = "短行".to_string();
        truncate_at_line_cap(&mut short);
        assert_eq!(short, "短行");
    }

    #[test]
    fn start_error_into_message_keeps_user_facing_text() {
        // 用户可见文案不变（前端/既有测试按该文案断言），结构仅供
        // start_if_needed 做控制流区分。
        assert_eq!(
            StartError::AlreadyRunning.into_message(),
            "引擎已在运行或启动中"
        );
        assert_eq!(
            StartError::Failed("其他失败".to_string()).into_message(),
            "其他失败"
        );
    }

    #[test]
    fn record_crash_keeps_window_limit() {
        // 窗口内密集崩溃：第 MAX_CRASH_REBOOTS 次即 WindowExceeded
        // （既有 60s 窗口语义不回退）。
        let mut runtime = EngineRuntime::default();
        let now = Instant::now();
        assert_eq!(record_crash(&mut runtime, now), CrashVerdict::Reboot);
        assert_eq!(
            record_crash(&mut runtime, now + Duration::from_secs(1)),
            CrashVerdict::WindowExceeded
        );
    }

    #[test]
    fn record_crash_caps_slow_loop_reboots() {
        // 低速崩溃（间隔 > CRASH_REBOOT_WINDOW）：窗口计数每次被清零，
        // 累计上限必须兜底——第 MAX_CRASH_REBOOTS_TOTAL+1 次判 TotalExceeded，
        // 不再无限自愈。
        let mut runtime = EngineRuntime::default();
        let start_at = Instant::now();
        let step = CRASH_REBOOT_WINDOW + Duration::from_secs(1);
        for i in 0..MAX_CRASH_REBOOTS_TOTAL {
            assert_eq!(
                record_crash(&mut runtime, start_at + step * i),
                CrashVerdict::Reboot,
                "第 {} 次低速崩溃应仍自愈",
                i + 1
            );
        }
        assert_eq!(
            record_crash(&mut runtime, start_at + step * MAX_CRASH_REBOOTS_TOTAL),
            CrashVerdict::TotalExceeded
        );
    }

    #[test]
    fn wait_orphan_pid_covers_spawn_window() {
        // 直接读写全局 RUNTIME/STOP_REQUESTED，须与 bridge.rs 等同样碰
        // RUNTIME 的测试串行（锁源见 RUNTIME_TEST_LOCK）；结束后复位。
        let _runtime_lock = locked_runtime_for_test();
        reset_runtime_for_test();
        STOP_REQUESTED.store(false, Ordering::SeqCst);

        // 场景 1：stop() 时 pid 未写入（spawn 窗口）、pid 晚到 → 轮询等到并返回。
        lock_runtime().phase = EnginePhase::Starting;
        STOP_REQUESTED.store(true, Ordering::SeqCst);
        let delayed = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            lock_runtime().pid = Some(424_242);
        });
        assert_eq!(wait_orphan_pid(Duration::from_secs(2)), Some(424_242));
        delayed.join().expect("delayed pid writer must finish");

        // 场景 2：停止标志已被 spawn 侧自查消费 → 立即放弃（不再补杀）。
        {
            let mut guard = lock_runtime();
            guard.phase = EnginePhase::Starting;
            guard.pid = None;
        }
        STOP_REQUESTED.store(false, Ordering::SeqCst);
        assert_eq!(wait_orphan_pid(Duration::from_secs(2)), None);

        // 场景 3：相位已离开 Starting（启动失败等已被收口）→ 立即放弃。
        STOP_REQUESTED.store(true, Ordering::SeqCst);
        lock_runtime().phase = EnginePhase::Idle;
        assert_eq!(wait_orphan_pid(Duration::from_secs(2)), None);

        STOP_REQUESTED.store(false, Ordering::SeqCst);
        reset_runtime_for_test();
    }
}
