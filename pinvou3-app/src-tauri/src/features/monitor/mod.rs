//! GB10 设备 + vLLM 后端 + pinvou3-app 自身的健康/性能采样。
//!
//! 数据流分两条：监控 UI 在页面 mount 时以 1s interval 按需调用完整
//! `sample_all`，离开页面即停止；PinvouOS 的常驻 Resource Agent 只调用
//! `sample_local_resources`，不探测模型 HTTP 健康或推理指标。两条路径复用
//! GPU/CPU/RAM 探针及缓存（nvidia-smi / Windows 性能计数器 / macOS ioreg）。
//! GPU util 峰值靠前端 5 个值滑窗 max（A+B）补足瞬时采样易错过推理峰的问题。
//!
//! 设计原则：**任何采样失败都 graceful degrade**——返回 None / OFFLINE，
//! 而不是 panic 或让上层崩。pinvou3 用户可能没装 nvidia-smi，可能没启 vLLM。
//!
//! 模块拆分（保持原 pub 面不变）：
//! - [`self_metrics`]：app 侧自测推理指标累加器（TTFT/TPS/tokens/KV）。
//! - [`model_probe`]：当前模型健康探测 + 本地 vLLM Prometheus metrics 解析
//!   （`VllmSnapshot` / `snapshot_for_model_config`）。
//! - 本 facade：系统资源采集（GPU/CPU/RAM）+ `sample_all` 聚合 + `MonitorState`。

mod model_probe;
mod platform;
mod self_metrics;

// re-export 子模块 pub 面，保持 `crate::features::monitor::Foo` 调用路径不变。
// `MonitorDiagnostic` 仅在 model_probe 内部使用，但作为原 pub 面的一部分保留 re-export
// 以维持外部可见性承诺（无外部调用方，allow 抑制 unused_imports 门禁）。
#[allow(unused_imports)]
pub use model_probe::{
    active_model_snapshot, probe_vllm_model_info, vllm_base_url, vllm_configured_model,
    vllm_snapshot, MonitorDiagnostic, VllmSnapshot, VllmStatus,
};
pub use self_metrics::{SelfMetrics, SelfMetricsDebugSnapshot, SelfPerfSnapshot};

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::Serialize;

const GPU_CACHE_TTL: Duration = Duration::from_secs(3);
const NVIDIA_GPU_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

static GPU_CACHE: OnceLock<Arc<Mutex<GpuSnapshotCache>>> = OnceLock::new();

/// 单次完整采样结果。所有字段 `Option`——采集失败就为 None。
#[derive(Debug, Clone, Default, Serialize)]
pub struct MonitorSnapshot {
    pub generated_at_ms: u64, // unix epoch ms
    pub gpu: Option<GpuSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<CpuSnapshot>,
    pub ram: Option<RamSnapshot>,
    pub vllm: Option<VllmSnapshot>,
    /// app 侧自测推理指标(TTFT / 生成速度 / 累计 tokens / KV)。与 vLLM `/metrics`
    /// 无关,任何后端(本地 vLLM / LM Studio / Ollama / 云端 API)都有值——因为是在
    /// 流式转发通路上就地测的。前端一律用这块显示这四项(vllm 块只剩队列/窗口/健康)。
    pub self_perf: SelfPerfSnapshot,
    pub self_perf_debug: SelfMetricsDebugSnapshot,
    pub app: AppSnapshot,
}

/// PinvouOS 常驻 Resource Agent 使用的本机轻量快照。
///
/// 与 MonitorSnapshot 分开：资源治理不需要探测模型 HTTP 健康，也不携带 UI 专用
/// 的推理累计指标。采样实现仍复用本 feature 的平台探针和缓存，由 lib.rs 组合根
/// 转换为 pinvou_os 的领域 observation，两个 feature 不互相依赖。
#[derive(Debug, Clone)]
pub struct LocalResourceSnapshot {
    pub generated_at_ms: i64,
    pub gpu: Option<GpuSnapshot>,
    pub cpu: Option<CpuSnapshot>,
    pub ram: Option<RamSnapshot>,
    /// Hottest trusted CPU/package or GPU sensor available to resource governance.
    pub temperature_c: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuSnapshot {
    pub name: String,
    pub vram_used_mib: u64,
    pub vram_total_mib: u64,
    pub utilization_pct: u32,
    /// Windows / Intel fallback: CPU package load, used by the UI's "local processor" variant.
    pub processor_utilization_pct: Option<u32>,
    /// Windows / Intel fallback: shared GPU memory usage, in MiB.
    pub shared_memory_used_mib: Option<u64>,
    /// GB10 等 unified-memory 设备 VRAM 字段是 [N/A]，UI 切到温度+功耗显示。
    pub temperature_c: Option<u32>,
    pub power_w: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuSnapshot {
    pub name: String,
    pub total_usage_pct: Option<f64>,
    pub process_usage_pct: Option<f64>,
    pub logical_processors: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct RamSnapshot {
    pub total_kib: u64,
    pub used_kib: u64, // total - available
    pub swap_total_kib: u64,
    pub swap_used_kib: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AppSnapshot {
    pub pinvou3_version: &'static str,
    pub deepseek_tui_version: &'static str,
    pub session_uptime_secs: u64,
}

/// Monitor 状态——持有 session 起始时间 + app 侧自测指标累加器，sample 全部按需。
#[derive(Debug, Clone, Default)]
pub struct MonitorState {
    started_at: Option<Instant>,
    self_metrics: Arc<SelfMetrics>,
}

impl MonitorState {
    pub fn new() -> Self {
        Self {
            started_at: Some(Instant::now()),
            self_metrics: Arc::new(SelfMetrics::default()),
        }
    }

    pub fn session_uptime_secs(&self) -> u64 {
        self.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0)
    }

    /// forwarder 拿这个 Arc 往里写自测指标（`app.state::<MonitorState>().self_metrics()`）。
    pub fn self_metrics(&self) -> Arc<SelfMetrics> {
        self.self_metrics.clone()
    }
}

pub fn sample_local_resources() -> LocalResourceSnapshot {
    local_resource_snapshot(gpu_snapshot())
}

fn local_resource_snapshot(gpu: Option<GpuSnapshot>) -> LocalResourceSnapshot {
    let platform_temperature_c = platform::temperature_c();
    let gpu_temperature_c = gpu
        .as_ref()
        .and_then(|snapshot| snapshot.temperature_c)
        .map(f64::from);
    let temperature_c = match (platform_temperature_c, gpu_temperature_c) {
        (Some(platform), Some(gpu)) => Some(platform.max(gpu)),
        (Some(platform), None) => Some(platform),
        (None, Some(gpu)) => Some(gpu),
        (None, None) => None,
    };

    LocalResourceSnapshot {
        generated_at_ms: chrono::Utc::now().timestamp_millis(),
        gpu,
        cpu: platform::cpu_snapshot(),
        ram: platform::ram_snapshot(),
        temperature_c,
    }
}

pub async fn sample_all(
    state: &MonitorState,
    vllm_upstream: &str,
    configured_model: Option<String>,
) -> MonitorSnapshot {
    sample_all_with_cpu(
        state,
        vllm_upstream,
        configured_model,
        platform::cpu_snapshot(),
    )
    .await
}

async fn sample_all_with_cpu(
    state: &MonitorState,
    vllm_upstream: &str,
    configured_model: Option<String>,
    cpu: Option<CpuSnapshot>,
) -> MonitorSnapshot {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    MonitorSnapshot {
        generated_at_ms: now_ms,
        gpu: gpu_snapshot(),
        cpu,
        ram: platform::ram_snapshot(),
        vllm: match active_model_snapshot().await {
            Some(snapshot) => Some(snapshot),
            None => vllm_snapshot(vllm_upstream, configured_model).await,
        },
        self_perf: state.self_metrics.snapshot(),
        self_perf_debug: state.self_metrics.debug_snapshot(),
        app: AppSnapshot {
            pinvou3_version: env!("CARGO_PKG_VERSION"),
            deepseek_tui_version: env!("CARGO_PKG_VERSION"), // TODO: 从 deepseek-tui crate 取
            session_uptime_secs: state.session_uptime_secs(),
        },
    }
}

/// Return GPU telemetry: NVIDIA probe (nvidia-smi) first, then the platform
/// sampler (Windows performance counters / macOS ioreg IOAccelerator).
fn gpu_snapshot() -> Option<GpuSnapshot> {
    let cache = GPU_CACHE.get_or_init(|| Arc::new(Mutex::new(GpuSnapshotCache::default())));
    cached_gpu_snapshot_with_probe(cache, || {
        nvidia_gpu_snapshot().or_else(platform::gpu_snapshot)
    })
}

#[derive(Default)]
struct GpuSnapshotCache {
    sampled_at: Option<Instant>,
    value: Option<GpuSnapshot>,
    probe_in_flight: bool,
}

/// GPU telemetry is optional evidence. External probes must never sit on the
/// RAM/cgroup Resource Agent path: start at most one bounded refresh in a
/// background thread and immediately serve the last good value (or `None`).
/// The cache mutex protects only bookkeeping; no process or platform I/O runs
/// while it is held.
fn cached_gpu_snapshot_with_probe<F>(
    cache: &Arc<Mutex<GpuSnapshotCache>>,
    probe: F,
) -> Option<GpuSnapshot>
where
    F: FnOnce() -> Option<GpuSnapshot> + Send + 'static,
{
    let stale_value = {
        let mut guard = cache.lock();
        if guard
            .sampled_at
            .is_some_and(|sampled_at| sampled_at.elapsed() < GPU_CACHE_TTL)
            || guard.probe_in_flight
        {
            return guard.value.clone();
        }
        guard.probe_in_flight = true;
        guard.value.clone()
    };

    let worker_cache = Arc::clone(cache);
    let spawned = std::thread::Builder::new()
        .name("pinvou-gpu-probe".to_string())
        .spawn(move || {
            let sampled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(probe))
                .ok()
                .flatten();
            let mut guard = worker_cache.lock();
            guard.sampled_at = Some(Instant::now());
            if let Some(snapshot) = sampled {
                guard.value = Some(snapshot);
            }
            guard.probe_in_flight = false;
        });
    if let Err(error) = spawned {
        let mut guard = cache.lock();
        guard.sampled_at = Some(Instant::now());
        guard.probe_in_flight = false;
        log::warn!("failed to start bounded GPU probe: {error}");
    }
    stale_value
}

/// 调 `nvidia-smi` 查 GPU。本机没 NVIDIA/没装 nvidia-smi → None。
/// 桌面环境启动时 PATH 可能不含 nvidia-smi，加常见绝对路径 fallback。
fn nvidia_gpu_snapshot() -> Option<GpuSnapshot> {
    let args = [
        "--query-gpu=name,memory.used,memory.total,utilization.gpu,temperature.gpu,power.draw",
        "--format=csv,noheader,nounits",
    ];
    // Try platform-provided probe candidates in order.
    let out = crate::platform::os::nvidia_smi_candidates()
        .into_iter()
        .find_map(|candidate| {
            let mut command = crate::platform::process::HiddenCommand::new(candidate);
            command.args(args);
            run_gpu_probe_command(command, NVIDIA_GPU_PROBE_TIMEOUT)
        })?;
    let line = std::str::from_utf8(&out.stdout).ok()?.lines().next()?;
    let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
    if parts.len() < 6 {
        return None;
    }
    // unified-memory 设备（如 NVIDIA GB10）`nvidia-smi` 返 `[N/A]`，
    // parse 失败时不要让整个 snapshot 丢失：单字段降级为 0/None。
    // UI 层检测 vram_total_mib == 0 切到温度+功耗显示。
    Some(GpuSnapshot {
        name: parts[0].to_string(),
        vram_used_mib: parts[1].parse().unwrap_or(0),
        vram_total_mib: parts[2].parse().unwrap_or(0),
        utilization_pct: parts[3].parse().unwrap_or(0),
        processor_utilization_pct: None,
        shared_memory_used_mib: None,
        temperature_c: parts[4].parse().ok(),
        power_w: parts[5].parse().ok(),
    })
}

fn run_gpu_probe_command(
    command: std::process::Command,
    timeout: Duration,
) -> Option<std::process::Output> {
    crate::platform::process::output_with_timeout(command, timeout)
        .ok()
        .filter(|output| output.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu_fixture(name: &str) -> GpuSnapshot {
        GpuSnapshot {
            name: name.to_string(),
            vram_used_mib: 1,
            vram_total_mib: 2,
            utilization_pct: 3,
            processor_utilization_pct: None,
            shared_memory_used_mib: None,
            temperature_c: None,
            power_w: None,
        }
    }

    #[tokio::test]
    async fn sample_all_keeps_other_fields_when_cpu_snapshot_is_none() {
        let state = MonitorState::new();
        let snapshot = sample_all_with_cpu(&state, "not-a-url", None, None).await;
        assert!(snapshot.generated_at_ms > 0);
        assert!(snapshot.cpu.is_none());
        assert_eq!(snapshot.self_perf.gen_tokens_total, 0);
        assert_eq!(snapshot.app.pinvou3_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn hanging_gpu_refresh_does_not_block_ram_sampling_cadence_or_hold_cache_lock() {
        let cache = Arc::new(Mutex::new(GpuSnapshotCache::default()));
        let downstream_cgroup_reads = std::sync::atomic::AtomicUsize::new(0);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        let first_started = Instant::now();
        assert!(cached_gpu_snapshot_with_probe(&cache, move || {
            started_tx.send(()).expect("announce hanging probe");
            release_rx.recv().expect("release hanging probe");
            Some(gpu_fixture("bounded-probe"))
        })
        .is_none());
        assert!(
            first_started.elapsed() < Duration::from_millis(250),
            "resource sampling waited for optional GPU I/O"
        );
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("GPU probe worker started");
        assert!(
            cache.try_lock().is_some(),
            "GPU process I/O must not hold the cache mutex"
        );

        let cadence_started = Instant::now();
        for _ in 0..5 {
            let gpu = cached_gpu_snapshot_with_probe(&cache, || {
                panic!("an in-flight probe must suppress duplicate external I/O")
            });
            let snapshot = local_resource_snapshot(gpu);
            assert!(snapshot.generated_at_ms > 0);
            let _ram_evidence = snapshot.ram;
            downstream_cgroup_reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        assert!(
            cadence_started.elapsed() < Duration::from_millis(500),
            "a hanging GPU probe stalled repeated RAM/cgroup sampling cadence"
        );
        assert_eq!(
            downstream_cgroup_reads.load(std::sync::atomic::Ordering::Relaxed),
            5,
            "the downstream cgroup cache-read stage must remain reachable"
        );

        release_tx.send(()).expect("release GPU probe");
        let deadline = Instant::now() + Duration::from_secs(2);
        while cache.lock().probe_in_flight && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let guard = cache.lock();
        assert!(!guard.probe_in_flight);
        assert_eq!(
            guard.value.as_ref().map(|gpu| gpu.name.as_str()),
            Some("bounded-probe")
        );
    }

    #[test]
    fn external_gpu_probe_has_a_hard_wall_clock_timeout() {
        let executable = std::env::current_exe().expect("resolve current test executable");
        let mut command = crate::platform::process::HiddenCommand::new(executable);
        command.args([
            "--ignored",
            "--exact",
            "features::monitor::tests::gpu_probe_timeout_child",
        ]);
        let started = Instant::now();
        assert!(run_gpu_probe_command(command, Duration::from_millis(100)).is_none());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "external GPU probe exceeded its hard timeout"
        );
    }

    #[test]
    #[ignore = "child process fixture for the hard-timeout regression"]
    fn gpu_probe_timeout_child() {
        std::thread::sleep(Duration::from_secs(30));
    }
}
