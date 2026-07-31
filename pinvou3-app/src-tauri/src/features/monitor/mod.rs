//! GB10 设备 + vLLM 后端 + pinvou3-app 自身的健康/性能采样。
//!
//! 数据流：**按需采样**——前端在监控页面 mount 时启 1s interval 调
//! `get_monitor_snapshot`，离开页面就停。后端每次 command 直接跑一次
//! `sample_all`。设计目的：用户不在监控页面时**完全不跑 GPU 探测**
//! (nvidia-smi / Windows 性能计数器 / macOS ioreg)。
//! GPU util 峰值靠前端 5 个值滑窗 max（A+B）补足瞬时采样易错过推理峰的问题。
//!
//! 设计原则：**任何采样失败都 graceful degrade**——返回 None / OFFLINE，
//! 而不是 panic 或让上层崩。pinvou3 用户可能没装 nvidia-smi，可能没启 vLLM。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::Serialize;

use crate::platform::credential_store::{CredentialStore, SystemCredentialStore};
use crate::platform::prefs::{ModelPreset, SavedModel, UserPrefs};

mod platform;

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

#[derive(Debug, Clone, Serialize)]
pub struct MonitorDiagnostic {
    pub code: String,
    pub message: String,
}

/// 当前模型运行态 + 本地 vLLM 队列指标。字段名暂保留 vllm 兼容前端。
#[derive(Debug, Clone, Serialize)]
pub struct VllmSnapshot {
    pub status: VllmStatus,
    /// 当前 active model id / preset，用于诊断热切换是否跟随用户选择。
    pub model_id: Option<String>,
    pub provider: String,
    /// vLLM `/v1/models` 返回的真实模型名。
    pub model: Option<String>,
    /// 用户 settings 中配置的模型名（与 `model` 可能不同）。
    pub configured_model: Option<String>,
    pub upstream: String,
    /// 后端类型(前端监控卡显示标签 + 决定 vLLM 指标是否适用 + 小窗口告警是否触发):
    /// `local` 本地推理引擎(环回/私有 IP,自托管 vLLM,有 Prometheus 指标)/
    /// `remote` 云端 API(公网,无 /metrics)/ `invalid` 配置异常(base_url 解析失败)。
    pub target_kind: String,
    /// vLLM Prometheus 指标是否适用(= `target_kind == "local"`);云端 API 无 /metrics。
    pub metrics_applicable: bool,
    /// `verified` / `unverified` / `missing_api_key` / `auth_failed` / `offline` / `mismatch`。
    pub health_status: String,
    pub diagnostic: Option<MonitorDiagnostic>,
    pub metric_diagnostics: Vec<MonitorDiagnostic>,
    pub max_model_len: Option<u32>,
    pub num_requests_running: Option<f64>,
    pub num_requests_waiting: Option<f64>,
    /// 历史累计 prefix cache 命中率: hits_total / queries_total × 100。
    /// 反映"重复 prompt prefix 复用 KV 比例",直接关联首字延迟。
    /// 瞬时 kv_cache_usage_perc 单用户场景一直是 0-2%,意义不大,已替换。
    pub prefix_cache_hit_pct: Option<f64>,
    /// prefix cache 原始计数器（hits_total / queries_total）。前端「清除统计」
    /// 用基准点对各累计 counter 做减法重算,命中率必须拿到原始分子/分母,
    /// 只给百分比无法做区间重算,故一并暴露。
    pub prefix_cache_hits: Option<f64>,
    pub prefix_cache_queries: Option<f64>,
    /// TTFT 直方图累计值（vllm:time_to_first_token_seconds_sum/_count）。
    /// 累积平均 = sum/count。counter 跟随 vLLM 进程生命周期，
    /// 换模型 = 重启进程 = 自动归零，因此天然按模型分段。
    pub ttft_sum_s: Option<f64>,
    pub ttft_count: Option<f64>,
    /// TPOT 直方图累计值。⚠️ 真实指标名带 request_ 前缀
    /// （vllm:request_time_per_output_token_seconds_*），2026-06-10 实测锁名。
    pub tpot_sum_s: Option<f64>,
    pub tpot_count: Option<f64>,
    pub generation_tokens_total: Option<f64>,
    pub prompt_tokens_total: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VllmStatus {
    Offline,
    Ready,
    Busy,
    /// 配置的模型名与 vLLM 实际返回的模型名不一致。vLLM 服务在线但聊天会报 model_not_found。
    Mismatch,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AppSnapshot {
    pub pinvou3_version: &'static str,
    pub deepseek_tui_version: &'static str,
    pub session_uptime_secs: u64,
}

/// app 侧自测指标的对外快照（单调累计，进程生命周期）。字段与 vLLM `/metrics`
/// 同"sum+count"形状，好让监控页「按住清除」的区间重算逻辑原样复用。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SelfPerfSnapshot {
    /// 首字延迟：Σ(TTFT) / 次数，仅纯文本轮（无工具调用）计入。
    pub ttft_sum_s: f64,
    pub ttft_count: u64,
    /// 生成速度：tps_tokens / tps_time_s = tok/s。同样仅纯文本轮计入
    /// （工具轮墙钟含工具执行耗时，计进去会把速度拉低失真，D2 决定跳过）。
    pub tps_tokens: u64,
    pub tps_time_s: f64,
    /// 累计 tokens：**全部轮**（含工具轮）真实 usage 之和。
    pub gen_tokens_total: u64,
    pub prompt_tokens_total: u64,
    /// KV 命中率（token 口径）：cache_hit /(hit+miss)×100。来自 usage 的
    /// prompt_cache_hit/miss_tokens——DeepSeek/部分云端会返回，返回不了的后端保持 0。
    pub cache_hit_tokens: u64,
    pub cache_miss_tokens: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SelfMetricsDebugSnapshot {
    pub inflight_count: usize,
    pub warmed_sessions_count: usize,
    pub last_event: Option<String>,
}

#[derive(Debug, Default)]
struct SelfPerfInner {
    ttft_sum_s: f64,
    ttft_count: u64,
    tps_tokens: u64,
    tps_time_s: f64,
    gen_tokens_total: u64,
    prompt_tokens_total: u64,
    cache_hit_tokens: u64,
    cache_miss_tokens: u64,
}

/// 单个 session 在途轮次的计时状态（TTFT 需要"起始"与"首 token"两个时点）。
#[derive(Debug)]
struct TurnTiming {
    start: Instant,
    first: Option<Instant>,
    had_tool: bool,
    output_chars: u64,
}

/// app 侧自测推理指标累加器。流式转发通路（engine.rs forwarder）在
/// TurnStarted / 首个 MessageDelta / ToolCallStarted / TurnComplete 四处打点写入，
/// `sample_all` 读出。`inflight` 按 `session_id` 键控——多 session 并发各测各的，
/// 不串台；`perf` 是全局单调累计，各轮把自己的增量加进去。
#[derive(Debug, Default)]
pub struct SelfMetrics {
    perf: Mutex<SelfPerfInner>,
    inflight: Mutex<HashMap<String, TurnTiming>>,
    /// 已完成过至少一轮的 session。每 session 首个完成轮 = 带底座 cache warmup 的**冷轮**
    /// (warmup 同步跑完整段冷 prefill,TurnStarted→首token 窗口吃满冷启),TTFT/TPS 不代表
    /// 稳态,故跳过(tokens 照记)。warmup 恰好只在 session 首轮跑,此集合精确识别那一轮。
    warmed_sessions: Mutex<HashSet<String>>,
    last_event: Mutex<Option<String>>,
}

impl SelfMetrics {
    /// TurnStarted：打点本轮起始。覆盖任何残留（上一轮异常未收尾）。
    pub fn on_turn_started(&self, session_id: &str) {
        self.inflight.lock().insert(
            session_id.to_string(),
            TurnTiming {
                start: Instant::now(),
                first: None,
                had_tool: false,
                output_chars: 0,
            },
        );
        self.set_last_event(format!("turn_started session={session_id}"));
    }

    /// 首个 MessageDelta：记首 token 时点（仅首次）。TTFT 的停表点 + 生成时长起点。
    #[cfg(test)]
    pub fn on_first_delta(&self, session_id: &str) {
        self.on_message_delta(session_id, 0);
    }

    pub fn on_message_delta(&self, session_id: &str, char_count: usize) {
        if let Some(t) = self.inflight.lock().get_mut(session_id) {
            if t.first.is_none() {
                t.first = Some(Instant::now());
                self.set_last_event(format!("first_delta session={session_id}"));
            }
            t.output_chars = t.output_chars.saturating_add(char_count as u64);
        }
    }

    /// 本轮出现过工具调用 → 标记。收尾时据此跳过 TTFT/TPS（D2）。
    pub fn on_tool(&self, session_id: &str) {
        if let Some(t) = self.inflight.lock().get_mut(session_id) {
            t.had_tool = true;
            self.set_last_event(format!("tool session={session_id}"));
        }
    }

    /// TurnComplete：用精确 usage 累加。tokens/KV 永远记；TTFT/TPS 仅跳过工具轮。
    /// 部分远端模型会省略 output_tokens，TPS 会回落到流式文本字符数估算。
    pub fn on_turn_complete(
        &self,
        session_id: &str,
        input_tokens: u32,
        output_tokens: u32,
        cache_hit: Option<u32>,
        cache_miss: Option<u32>,
    ) {
        let timing = self.inflight.lock().remove(session_id);
        let is_first_turn = self.warmed_sessions.lock().insert(session_id.to_string());
        let mut p = self.perf.lock();
        p.gen_tokens_total += output_tokens as u64;
        p.prompt_tokens_total += input_tokens as u64;
        if let Some(h) = cache_hit {
            p.cache_hit_tokens += h as u64;
        }
        if let Some(m) = cache_miss {
            p.cache_miss_tokens += m as u64;
        }
        let mut recorded_perf = false;
        let had_timing = timing.is_some();
        if let Some(t) = timing {
            if !t.had_tool {
                if let Some(first) = t.first {
                    p.ttft_sum_s += first.duration_since(t.start).as_secs_f64();
                    p.ttft_count += 1;
                    let gen_s = first.elapsed().as_secs_f64();
                    let tps_units = if output_tokens > 0 {
                        output_tokens as u64
                    } else {
                        // Some remote providers stream text but omit final token usage.
                        // Use a conservative character-based fallback so speed still
                        // reflects completed pure text turns instead of staying blank.
                        (t.output_chars / 2).max(1)
                    };
                    if gen_s > 0.0 && tps_units > 0 {
                        p.tps_time_s += gen_s;
                        p.tps_tokens += tps_units;
                        recorded_perf = true;
                    }
                }
            }
        }
        self.set_last_event(format!(
            "turn_complete session={session_id} input={input_tokens} output={output_tokens} first_turn={is_first_turn} had_timing={had_timing} recorded_perf={recorded_perf}"
        ));
    }

    pub fn snapshot(&self) -> SelfPerfSnapshot {
        let p = self.perf.lock();
        SelfPerfSnapshot {
            ttft_sum_s: p.ttft_sum_s,
            ttft_count: p.ttft_count,
            tps_tokens: p.tps_tokens,
            tps_time_s: p.tps_time_s,
            gen_tokens_total: p.gen_tokens_total,
            prompt_tokens_total: p.prompt_tokens_total,
            cache_hit_tokens: p.cache_hit_tokens,
            cache_miss_tokens: p.cache_miss_tokens,
        }
    }

    pub fn debug_snapshot(&self) -> SelfMetricsDebugSnapshot {
        SelfMetricsDebugSnapshot {
            inflight_count: self.inflight.lock().len(),
            warmed_sessions_count: self.warmed_sessions.lock().len(),
            last_event: self.last_event.lock().clone(),
        }
    }

    fn set_last_event(&self, value: String) {
        *self.last_event.lock() = Some(value);
    }
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
    static GPU_CACHE: OnceLock<Mutex<GpuSnapshotCache>> = OnceLock::new();
    let cache = GPU_CACHE.get_or_init(|| Mutex::new(GpuSnapshotCache::default()));
    let mut guard = cache.lock();
    if guard
        .sampled_at
        .is_some_and(|sampled_at| sampled_at.elapsed() < Duration::from_secs(3))
    {
        return guard.value.clone();
    }
    let value = nvidia_gpu_snapshot().or_else(platform::gpu_snapshot);
    guard.sampled_at = Some(Instant::now());
    if let Some(snapshot) = value {
        guard.value = Some(snapshot.clone());
        Some(snapshot)
    } else {
        // Windows performance counters occasionally time out or return no samples.
        // Keep the last good local compute snapshot so the UI does not flicker
        // between valid data and "unavailable" during normal polling.
        guard.value.clone()
    }
}

#[derive(Default)]
struct GpuSnapshotCache {
    sampled_at: Option<Instant>,
    value: Option<GpuSnapshot>,
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
            crate::platform::process::HiddenCommand::new(candidate)
                .args(args)
                .output()
                .ok()
                .filter(|o| o.status.success())
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

pub async fn active_model_snapshot() -> Option<VllmSnapshot> {
    let prefs = UserPrefs::load();
    let model = prefs.active_model().cloned();
    let env_base = std::env::var("DEEPSEEK_BASE_URL").ok();
    let env_model = std::env::var("DEEPSEEK_MODEL").ok();
    let upstream = env_base
        .or_else(|| model.as_ref().map(|m| m.base_url.clone()))
        .unwrap_or_else(|| "http://127.0.0.1:8000/v1".to_string());
    let preset = model
        .as_ref()
        .map(|m| m.preset)
        .unwrap_or(ModelPreset::LocalVllm);
    let configured_model = env_model.or_else(|| {
        model
            .as_ref()
            .and_then(|m| (m.preset != ModelPreset::LocalVllm).then(|| m.model.clone()))
    });
    let api_key = model.as_ref().and_then(model_api_key);
    let model_id = model.as_ref().map(|m| m.id.clone());
    let provider = preset.as_str().to_string();
    snapshot_for_model_config(
        &upstream,
        configured_model,
        preset,
        model_id,
        provider,
        api_key.as_deref(),
    )
    .await
}

/// 兼容旧调用。优先用于本地 vLLM 探测；active-model 面板走 `active_model_snapshot()`。
pub async fn vllm_snapshot(
    upstream: &str,
    configured_model: Option<String>,
) -> Option<VllmSnapshot> {
    snapshot_for_model_config(
        upstream,
        configured_model,
        ModelPreset::LocalVllm,
        None,
        "local_vllm".to_string(),
        None,
    )
    .await
}

fn model_api_key(model: &SavedModel) -> Option<String> {
    if let Ok(v) = std::env::var("DEEPSEEK_API_KEY") {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(reference) = &model.credential_ref {
        let store = SystemCredentialStore::new();
        match store.get(reference) {
            Ok(Some(key)) if !key.trim().is_empty() => return Some(key),
            Ok(_) => {}
            Err(err) => eprintln!(
                "[monitor] credential read failed for model {}: {}",
                model.id,
                err.user_message()
            ),
        }
    }
    let trimmed = model.api_key.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// 当前模型健康探测 + 本地 vLLM Prometheus metrics 解析。
async fn snapshot_for_model_config(
    upstream: &str,
    configured_model: Option<String>,
    preset: ModelPreset,
    model_id: Option<String>,
    provider: String,
    api_key: Option<&str>,
) -> Option<VllmSnapshot> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    let target_kind = if preset == ModelPreset::LocalVllm {
        "local"
    } else {
        vllm_target_kind(upstream)
    };
    let metrics_applicable = target_kind == "local";

    // 1) /v1/models 健康
    // upstream 通常已带 `/v1` 后缀（DEEPSEEK_BASE_URL=http://...:8000/v1），
    // 所以直接拼 `/models`；不带的话补 `/v1/models`。
    let models_url = if upstream.trim_end_matches('/').ends_with("/v1") {
        format!("{}/models", upstream.trim_end_matches('/'))
    } else {
        format!("{}/v1/models", upstream.trim_end_matches('/'))
    };
    let mut request = client.get(models_url);
    if let Some(key) = api_key.map(str::trim).filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let should_probe_models =
        target_kind == "local" || api_key.map(str::trim).is_some_and(|key| !key.is_empty());
    let models_resp = if should_probe_models {
        Some(request.send().await)
    } else {
        None
    };
    let models_resp = match models_resp {
        Some(Ok(r)) if r.status().is_success() => Some(r),
        Some(Ok(r))
            if r.status() == reqwest::StatusCode::UNAUTHORIZED
                || r.status() == reqwest::StatusCode::FORBIDDEN =>
        {
            return Some(base_model_snapshot(
                VllmStatus::Offline,
                model_id,
                provider,
                configured_model,
                upstream,
                target_kind,
                metrics_applicable,
                "auth_failed",
                Some(MonitorDiagnostic {
                    code: "auth_failed".to_string(),
                    message: format!("模型接口鉴权失败 (HTTP {})", r.status().as_u16()),
                }),
            ));
        }
        Some(Ok(r)) => {
            if target_kind == "local" {
                return Some(base_model_snapshot(
                    VllmStatus::Offline,
                    model_id,
                    provider,
                    configured_model,
                    upstream,
                    target_kind,
                    metrics_applicable,
                    "offline",
                    Some(MonitorDiagnostic {
                        code: "models_http_error".to_string(),
                        message: format!("/v1/models 返回 HTTP {}", r.status().as_u16()),
                    }),
                ));
            }
            None
        }
        Some(Err(err)) => {
            if target_kind == "local" {
                return Some(base_model_snapshot(
                    VllmStatus::Offline,
                    model_id,
                    provider,
                    configured_model,
                    upstream,
                    target_kind,
                    metrics_applicable,
                    "offline",
                    Some(MonitorDiagnostic {
                        code: "models_unreachable".to_string(),
                        message: format!("/v1/models 不可达: {err}"),
                    }),
                ));
            }
            None
        }
        None if target_kind == "local" => {
            return Some(VllmSnapshot {
                status: VllmStatus::Offline,
                model_id,
                provider,
                model: None,
                configured_model,
                upstream: upstream.to_string(),
                target_kind: target_kind.to_string(),
                metrics_applicable,
                health_status: "offline".to_string(),
                diagnostic: Some(MonitorDiagnostic {
                    code: "models_unverified".to_string(),
                    message: "未探测 /v1/models".to_string(),
                }),
                metric_diagnostics: Vec::new(),
                max_model_len: None,
                num_requests_running: None,
                num_requests_waiting: None,
                prefix_cache_hit_pct: None,
                prefix_cache_hits: None,
                prefix_cache_queries: None,
                ttft_sum_s: None,
                ttft_count: None,
                tpot_sum_s: None,
                tpot_count: None,
                generation_tokens_total: None,
                prompt_tokens_total: None,
            });
        }
        None => None,
    };

    let (served_model, max_model_len) = match models_resp {
        Some(r) => match r.json::<serde_json::Value>().await.ok() {
            Some(v) => parse_models_response(v).unwrap_or((None, None)),
            None => (None, None),
        },
        None => (None, None),
    };

    // 2) /metrics（用 host 根目录，不带 /v1）
    let metrics_url = metrics_applicable
        .then(|| strip_v1_suffix(upstream).map(|h| format!("{h}/metrics")))
        .flatten();
    let metrics_resp = match metrics_url {
        Some(u) => client.get(&u).send().await.ok(),
        None => None,
    };
    let metrics_text = match metrics_resp {
        Some(r) if r.status().is_success() => r.text().await.ok(),
        _ => None,
    };
    let mut metric_diagnostics = if metrics_applicable && metrics_text.is_none() {
        vec![MonitorDiagnostic {
            code: "metrics_unavailable".to_string(),
            message: "本地 /metrics 不可用或未返回 Prometheus 指标".to_string(),
        }]
    } else {
        Vec::new()
    };
    let max_model_len = max_model_len.or_else(|| {
        let inferred = infer_context_window(
            preset,
            configured_model.as_deref().or(served_model.as_deref()),
        );
        if inferred.is_some() {
            metric_diagnostics.push(MonitorDiagnostic {
                code: "context_window_inferred".to_string(),
                message: "上下文长度由模型名/供应商预设推断，远端模型接口未直接提供".to_string(),
            });
        }
        inferred
    });

    let running = metrics_text
        .as_deref()
        .and_then(|t| parse_prom_metric(t, "vllm:num_requests_running"));
    let waiting = metrics_text
        .as_deref()
        .and_then(|t| parse_prom_metric(t, "vllm:num_requests_waiting"));
    // 历史累计 prefix cache 命中率: hits/queries × 100。两个都是 vLLM Prometheus
    // counter (单调递增,vLLM 进程生命周期内累积)。queries=0 时返回 None 显示 "—"
    // 而非 NaN。
    let prefix_cache_hits = metrics_text
        .as_deref()
        .and_then(|t| parse_prom_metric(t, "vllm:prefix_cache_hits_total"));
    let prefix_cache_queries = metrics_text
        .as_deref()
        .and_then(|t| parse_prom_metric(t, "vllm:prefix_cache_queries_total"));
    let prefix_hit_pct = match (prefix_cache_hits, prefix_cache_queries) {
        (Some(h), Some(q)) if q > 0.0 => Some(h / q * 100.0),
        _ => None,
    };

    let perf = metrics_text
        .as_deref()
        .map(parse_perf_metrics)
        .unwrap_or_default();

    let mut status = match (running, waiting) {
        (Some(r), _) if r > 0.0 => VllmStatus::Busy,
        (_, Some(w)) if w > 0.0 => VllmStatus::Busy,
        _ => VllmStatus::Ready,
    };
    // 如果用户配置了模型名，但和 vLLM 实际返回的不一致，降级为 Mismatch。
    // 这样监控台不会显示绿色 READY，聊天 live dot 也会变红。
    if metrics_applicable {
        if let Some(ref cfg) = configured_model {
            if let Some(ref actual) = served_model {
                if cfg.trim() != actual.trim() {
                    status = VllmStatus::Mismatch;
                }
            }
        }
    }
    let health_status = match status {
        VllmStatus::Mismatch => "mismatch",
        VllmStatus::Offline => "offline",
        _ if target_kind == "remote"
            && api_key
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .is_none() =>
        {
            "missing_api_key"
        }
        _ if target_kind == "remote" && served_model.is_none() => "unverified",
        _ => "verified",
    };
    let diagnostic = match health_status {
        "missing_api_key" => Some(MonitorDiagnostic {
            code: "missing_api_key".to_string(),
            message: "远端模型未配置 API Key，跳过在线探测".to_string(),
        }),
        "unverified" => Some(MonitorDiagnostic {
            code: "remote_unverified".to_string(),
            message: "远端模型未返回可用模型列表，保留当前配置展示".to_string(),
        }),
        "mismatch" => Some(MonitorDiagnostic {
            code: "model_mismatch".to_string(),
            message: "配置模型名与本地服务返回模型名不一致".to_string(),
        }),
        _ => None,
    };

    Some(VllmSnapshot {
        status,
        model_id,
        provider,
        model: if target_kind == "remote" {
            configured_model.clone().or(served_model)
        } else {
            served_model
        },
        configured_model,
        upstream: upstream.to_string(),
        target_kind: target_kind.to_string(),
        metrics_applicable,
        health_status: health_status.to_string(),
        diagnostic,
        metric_diagnostics,
        max_model_len,
        num_requests_running: running,
        num_requests_waiting: waiting,
        prefix_cache_hit_pct: prefix_hit_pct,
        prefix_cache_hits,
        prefix_cache_queries,
        ttft_sum_s: perf.ttft_sum_s,
        ttft_count: perf.ttft_count,
        tpot_sum_s: perf.tpot_sum_s,
        tpot_count: perf.tpot_count,
        generation_tokens_total: perf.generation_tokens_total,
        prompt_tokens_total: perf.prompt_tokens_total,
    })
}

fn base_model_snapshot(
    status: VllmStatus,
    model_id: Option<String>,
    provider: String,
    configured_model: Option<String>,
    upstream: &str,
    target_kind: &str,
    metrics_applicable: bool,
    health_status: &str,
    diagnostic: Option<MonitorDiagnostic>,
) -> VllmSnapshot {
    VllmSnapshot {
        status,
        model_id,
        provider,
        model: configured_model.clone(),
        configured_model,
        upstream: upstream.to_string(),
        target_kind: target_kind.to_string(),
        metrics_applicable,
        health_status: health_status.to_string(),
        diagnostic,
        metric_diagnostics: Vec::new(),
        max_model_len: None,
        num_requests_running: None,
        num_requests_waiting: None,
        prefix_cache_hit_pct: None,
        prefix_cache_hits: None,
        prefix_cache_queries: None,
        ttft_sum_s: None,
        ttft_count: None,
        tpot_sum_s: None,
        tpot_count: None,
        generation_tokens_total: None,
        prompt_tokens_total: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiModelInfo {
    pub id: String,
    pub max_model_len: Option<u32>,
    /// 是否已加载到内存。`None` = 未知（通用 OpenAI 兼容端点不区分）。
    /// Ollama（/api/ps vs /api/tags）与 LM Studio（/api/v0/models 的 state）
    /// 的列表接口返回全部已下载模型，二者都是 JIT 加载——任何推理请求引用
    /// 模型名就会静默载入内存。探测必须把这个状态传给前端，避免把未加载的
    /// 大模型当作"就绪"自动填充。
    pub loaded: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiModelsProbe {
    pub models: Vec<OpenAiModelInfo>,
}

fn parse_models_response(v: serde_json::Value) -> Option<(Option<String>, Option<u32>)> {
    let first = parse_models_response_list(v)?.into_iter().next()?;
    Some((Some(first.id), first.max_model_len))
}

fn parse_models_response_list(v: serde_json::Value) -> Option<Vec<OpenAiModelInfo>> {
    let data = v.get("data")?.as_array()?;
    let models = data
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(|v| v.as_str())?.trim();
            if id.is_empty() {
                return None;
            }
            let max_model_len = item
                .get("max_model_len")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32);
            Some(OpenAiModelInfo {
                id: id.to_string(),
                max_model_len,
                loaded: None,
            })
        })
        .collect::<Vec<_>>();
    (!models.is_empty()).then_some(models)
}

pub async fn probe_openai_models(base_url: &str) -> Option<OpenAiModelsProbe> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    let url = if base_url.trim_end_matches('/').ends_with("/v1") {
        format!("{}/models", base_url.trim_end_matches('/'))
    } else {
        format!("{}/v1/models", base_url.trim_end_matches('/'))
    };
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v = resp.json::<serde_json::Value>().await.ok()?;
    Some(OpenAiModelsProbe {
        models: parse_models_response_list(v)?,
    })
}

/// Ollama `/api/ps` 返回的已加载模型名集合。解析失败按空集处理
/// （宁可全部标未加载，也不错标已加载）。
fn parse_ollama_ps_names(v: serde_json::Value) -> std::collections::HashSet<String> {
    v.get("models")
        .and_then(|m| m.as_array())
        .map(|models| {
            models
                .iter()
                .filter_map(|item| {
                    item.get("name")
                        .or_else(|| item.get("model"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                })
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Ollama `/api/tags` 返回的已下载模型名列表（保持顺序、去重）。
fn parse_ollama_tag_names(v: serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(models) = v.get("models").and_then(|m| m.as_array()) {
        for item in models {
            let Some(name) = item.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let name = name.trim();
            if !name.is_empty() && !out.iter().any(|existing| existing == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// LM Studio 原生 REST `/api/v0/models`：每项带 `state`（loaded / not-loaded）。
/// 返回 `None` 表示响应形状不认识，调用方回退 OpenAI 兼容探测。
fn parse_lmstudio_v0_models(v: &serde_json::Value) -> Option<Vec<OpenAiModelInfo>> {
    let data = v.get("data")?.as_array()?;
    let models = data
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(|v| v.as_str())?.trim();
            if id.is_empty() {
                return None;
            }
            let loaded = item
                .get("state")
                .and_then(|v| v.as_str())
                .map(|state| state == "loaded");
            Some(OpenAiModelInfo {
                id: id.to_string(),
                max_model_len: None,
                loaded,
            })
        })
        .collect::<Vec<_>>();
    (!models.is_empty()).then_some(models)
}

/// 探测 Ollama：区分"已加载"（/api/ps）与"仅下载未加载"（/api/tags）。
/// 两个接口都是只读列表，不会触发加载；绝不能用推理请求探测。
pub async fn probe_ollama_models(base_url: &str) -> Option<OpenAiModelsProbe> {
    let host = strip_v1_suffix(base_url)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    // 已加载集合：失败按空集（全部未加载），不影响已下载列表。
    let loaded_names = match client
        .get(format!("{host}/api/ps"))
        .send()
        .await
        .and_then(|r| r.error_for_status())
    {
        Ok(resp) => resp
            .json::<serde_json::Value>()
            .await
            .map(parse_ollama_ps_names)
            .unwrap_or_default(),
        Err(_) => Default::default(),
    };
    // 已下载列表：/api/tags 是必需项，失败则整个候选离线。
    let resp = client
        .get(format!("{host}/api/tags"))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .ok()?;
    let tags = parse_ollama_tag_names(resp.json::<serde_json::Value>().await.ok()?);
    (!tags.is_empty()).then_some(OpenAiModelsProbe {
        models: tags
            .into_iter()
            .map(|name| {
                let loaded = loaded_names.contains(&name);
                OpenAiModelInfo {
                    id: name,
                    max_model_len: None,
                    loaded: Some(loaded),
                }
            })
            .collect(),
    })
}

/// 探测 LM Studio：优先原生 `/api/v0/models`（带 loaded 状态），
/// 旧版本没有该接口时回退 `/v1/models`（loaded 未知）。
pub async fn probe_lmstudio_models(base_url: &str) -> Option<OpenAiModelsProbe> {
    let host = strip_v1_suffix(base_url)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    if let Ok(resp) = client
        .get(format!("{host}/api/v0/models"))
        .send()
        .await
        .and_then(|r| r.error_for_status())
    {
        if let Ok(v) = resp.json::<serde_json::Value>().await {
            if let Some(models) = parse_lmstudio_v0_models(&v) {
                return Some(OpenAiModelsProbe { models });
            }
        }
    }
    probe_openai_models(base_url).await
}

fn infer_context_window(preset: ModelPreset, model: Option<&str>) -> Option<u32> {
    // 模型名事实与 Engine route_limits 共用同一入口，避免页面显示 1M、实际仍按
    // 128K 压缩。这里只保留底座与补充表都无法识别时的供应商预设兜底。
    if let Some(window) = model.and_then(crate::core::model_context::resolved_context_window) {
        return Some(window);
    }
    match preset {
        ModelPreset::LocalVllm => Some(262_144),
        ModelPreset::Deepseek => Some(131_072),
        ModelPreset::Kimi => Some(262_144),
        ModelPreset::OpenaiCompatible => Some(131_072),
        ModelPreset::Qwen => Some(131_072),
        ModelPreset::Doubao => Some(262_144),
        ModelPreset::Minimax => Some(204_800),
        ModelPreset::Glm => Some(131_072),
        ModelPreset::Mimo => Some(1_000_000),
    }
}

/// 推理性能相关的 6 个累计指标，统一解析、统一缺省 None。
#[derive(Debug, Default)]
struct PerfMetrics {
    ttft_sum_s: Option<f64>,
    ttft_count: Option<f64>,
    tpot_sum_s: Option<f64>,
    tpot_count: Option<f64>,
    generation_tokens_total: Option<f64>,
    prompt_tokens_total: Option<f64>,
}

fn parse_perf_metrics(text: &str) -> PerfMetrics {
    PerfMetrics {
        ttft_sum_s: parse_prom_metric(text, "vllm:time_to_first_token_seconds_sum"),
        ttft_count: parse_prom_metric(text, "vllm:time_to_first_token_seconds_count"),
        tpot_sum_s: parse_prom_metric(text, "vllm:request_time_per_output_token_seconds_sum"),
        tpot_count: parse_prom_metric(text, "vllm:request_time_per_output_token_seconds_count"),
        generation_tokens_total: parse_prom_metric(text, "vllm:generation_tokens_total"),
        prompt_tokens_total: parse_prom_metric(text, "vllm:prompt_tokens_total"),
    }
}

/// 从 Prometheus 文本里抽某个指标的第一个数值，例如：
/// `vllm:num_requests_running{engine="0",model_name="/model"} 0.0` → 0.0
fn parse_prom_metric(text: &str, name: &str) -> Option<f64> {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if !line.starts_with(name) {
            continue;
        }
        // 跳过指标名称 + 可选 `{labels}`，找最后一个空格后的数字
        let after_name = &line[name.len()..];
        let value_part = if after_name.starts_with('{') {
            let close = after_name.find('}')?;
            after_name[close + 1..].trim()
        } else {
            after_name.trim()
        };
        let token = value_part.split_whitespace().next()?;
        if let Ok(v) = token.parse::<f64>() {
            return Some(v);
        }
    }
    None
}

/// 按 base_url 主机段判后端类型:环回/私有 IP 段 = 本地推理引擎(`local`,自托管 vLLM,
/// 有 Prometheus 指标);公网域名/IP = 云端 API(`remote`);空/解析失败 = 配置异常(`invalid`)。
/// 前端监控卡的「本地模型/远端模型/配置异常」标签 + 指标适用性 + 小窗口告警都据此。
fn vllm_target_kind(upstream: &str) -> &'static str {
    let s = upstream.trim();
    if s.is_empty() {
        return "invalid";
    }
    let Some(rest) = s
        .strip_prefix("http://")
        .or_else(|| s.strip_prefix("https://"))
    else {
        return "invalid";
    };
    let Some(host_port) = rest.split('/').next() else {
        return "invalid";
    };
    // 去端口 + ipv6 括号
    let host = host_port.rsplit_once(':').map_or(host_port, |(h, _)| h);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return "invalid";
    }
    if host == "localhost" || host == "::1" {
        return "local";
    }
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        let o = ip.octets();
        let private = o[0] == 127
            || o[0] == 10
            || (o[0] == 172 && (16..=31).contains(&o[1]))
            || (o[0] == 192 && o[1] == 168);
        return if private { "local" } else { "remote" };
    }
    // 域名或公网 IPv6 → 云端 API
    "remote"
}

fn strip_v1_suffix(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    Some(
        trimmed
            .strip_suffix("/v1")
            .map(String::from)
            .unwrap_or_else(|| trimmed.to_string()),
    )
}

/// 轻量探测本地 vLLM:一次 `/v1/models` 拿两样——实际 served 模型名 + `max_model_len`
/// (上下文窗口)。名字用于发请求(免写死名字与 `--served-model-name` 不一致的
/// model_not_found);窗口用于填 `active_route_limits.context_tokens`,让压缩阈值按真实
/// 窗口推导(见 docs/context-compaction-设计.md)。探测失败(vLLM 没起/超时)返回
/// `(None, None)`,调用方 fallback 配置值 + 名字 hint 老路。
pub async fn probe_vllm_model_info(base_url: &str) -> (Option<String>, Option<u32>) {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    else {
        return (None, None);
    };
    let url = if base_url.trim_end_matches('/').ends_with("/v1") {
        format!("{}/models", base_url.trim_end_matches('/'))
    } else {
        format!("{}/v1/models", base_url.trim_end_matches('/'))
    };
    let Ok(resp) = client.get(url).send().await else {
        return (None, None);
    };
    if !resp.status().is_success() {
        return (None, None);
    }
    let Ok(v) = resp.json::<serde_json::Value>().await else {
        return (None, None);
    };
    parse_models_response(v).unwrap_or((None, None))
}

/// 当前 monitor/探测应使用的 vLLM base_url。
/// 优先级：环境变量 `DEEPSEEK_BASE_URL` > settings.json `custom_base_url` > 默认值。
/// 与 Engine 使用的逻辑保持一致（见 `bridge::Pinvou3Bridge::base_url`）。
pub fn vllm_base_url() -> String {
    if let Ok(v) = std::env::var("DEEPSEEK_BASE_URL") {
        return v;
    }
    let prefs = crate::platform::prefs::UserPrefs::load();
    prefs
        .active_model()
        .map(|m| m.base_url.clone())
        .unwrap_or_else(|| "http://127.0.0.1:8000/v1".to_string())
}

/// 用户配置的模型名（用于 monitor 显示"配置目标"）。
/// 优先级：环境变量 `DEEPSEEK_MODEL` > settings.json `custom_model_name` > None。
pub fn vllm_configured_model() -> Option<String> {
    if let Ok(v) = std::env::var("DEEPSEEK_MODEL") {
        return Some(v);
    }
    let prefs = crate::platform::prefs::UserPrefs::load();
    match prefs.active_model() {
        // 本地 vLLM 动态跟随实际 served name(见 EnginePool::fresh_bridge_for),
        // 不声明固定配置目标 → 监控不做 mismatch 误报,只显示 vLLM 实际名字。
        Some(m) if m.preset == crate::platform::prefs::ModelPreset::LocalVllm => None,
        Some(m) => Some(m.model.clone()),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_metrics_first_turn_per_session_records_ttft_tps() {
        // 监控面板需要首轮纯文本恢复后立刻显示 TTFT/TPS；仅工具轮跳过。
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_first_delta("s1"); // 幂等
        m.on_turn_complete("s1", 100, 50, Some(90), Some(10));
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 1);
        assert_eq!(s.tps_tokens, 50);
        assert_eq!(s.gen_tokens_total, 50); // tokens 照记
        assert_eq!(s.prompt_tokens_total, 100);
        assert_eq!(s.cache_hit_tokens, 90); // cache 照记
        assert_eq!(s.cache_miss_tokens, 10);
    }

    #[test]
    fn self_metrics_second_pure_turn_records_ttft_tps() {
        let m = SelfMetrics::default();
        // 首轮也记
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_turn_complete("s1", 10, 5, None, None);
        // 二轮继续记
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_turn_complete("s1", 100, 50, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 2);
        assert_eq!(s.tps_tokens, 55);
        assert!(s.tps_time_s > 0.0);
        assert_eq!(s.gen_tokens_total, 55); // 两轮 tokens 都在
    }

    #[test]
    fn self_metrics_tool_turn_skips_ttft_tps_but_keeps_tokens() {
        let m = SelfMetrics::default();
        // 先跑两轮纯文本(都计 TTFT)
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_turn_complete("s1", 1, 1, None, None);
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_turn_complete("s1", 1, 1, None, None);
        // 工具轮:tokens 记,TTFT/TPS 跳(D2)
        m.on_turn_started("s1");
        m.on_tool("s1");
        m.on_first_delta("s1");
        m.on_turn_complete("s1", 200, 80, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 2); // 工具轮没加,仍是前两轮
        assert_eq!(s.gen_tokens_total, 82); // 1+1+80 全记
    }

    #[test]
    fn self_metrics_delta_without_start_still_counts_tokens() {
        // forwarder 中途起来、没接到 TurnStarted 的轮:TTFT 记不了,但 tokens 不丢。
        let m = SelfMetrics::default();
        m.on_first_delta("s1"); // 无 inflight,no-op
        m.on_turn_complete("s1", 10, 5, None, None);
        let s = m.snapshot();
        assert_eq!(s.gen_tokens_total, 5);
        assert_eq!(s.ttft_count, 0);
    }

    #[test]
    fn self_metrics_first_turn_tracked_per_session() {
        // 各 session 独立记录,不串台。
        let m = SelfMetrics::default();
        m.on_turn_started("a");
        m.on_turn_started("b");
        m.on_first_delta("a");
        m.on_first_delta("b");
        m.on_turn_complete("a", 1, 1, None, None);
        m.on_turn_complete("b", 1, 1, None, None);
        let s1 = m.snapshot();
        assert_eq!(s1.ttft_count, 2);
        assert_eq!(s1.gen_tokens_total, 2);
        // a 二轮继续记,b 仍只跑过首轮
        m.on_turn_started("a");
        m.on_first_delta("a");
        m.on_turn_complete("a", 1, 1, None, None);
        let s2 = m.snapshot();
        assert_eq!(s2.ttft_count, 3);
    }

    #[test]
    fn prom_metric_extracts_value_with_labels() {
        let text = "# HELP foo\n\
                    vllm:num_requests_running{engine=\"0\",model_name=\"/model\"} 0.0\n";
        assert_eq!(
            parse_prom_metric(text, "vllm:num_requests_running"),
            Some(0.0)
        );
    }

    #[test]
    fn prom_metric_handles_nonzero() {
        let text = "vllm:num_requests_running{engine=\"0\"} 42.5";
        assert_eq!(
            parse_prom_metric(text, "vllm:num_requests_running"),
            Some(42.5)
        );
    }

    #[test]
    fn prom_metric_returns_none_for_missing() {
        let text = "some_other_metric 1.0";
        assert!(parse_prom_metric(text, "vllm:num_requests_running").is_none());
    }

    #[test]
    fn strip_v1_suffix_removes_trailing_v1() {
        assert_eq!(
            strip_v1_suffix("http://host:8000/v1").as_deref(),
            Some("http://host:8000")
        );
        assert_eq!(
            strip_v1_suffix("http://host:8000/v1/").as_deref(),
            Some("http://host:8000")
        );
        assert_eq!(
            strip_v1_suffix("http://host:8000").as_deref(),
            Some("http://host:8000")
        );
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
    fn vllm_target_kind_classifies_by_host() {
        // 本地推理引擎:环回 + 私有 IP 段
        assert_eq!(vllm_target_kind("http://10.0.0.113:8000/v1"), "local");
        assert_eq!(vllm_target_kind("http://127.0.0.1:8000/v1"), "local");
        assert_eq!(vllm_target_kind("http://localhost:8000/v1"), "local");
        assert_eq!(vllm_target_kind("http://192.168.1.5:8000/v1"), "local");
        assert_eq!(vllm_target_kind("http://172.16.0.9:8000/v1"), "local");
        // 云端 API:公网域名 / 公网 IP
        assert_eq!(vllm_target_kind("https://api.deepseek.com/v1"), "remote");
        assert_eq!(vllm_target_kind("http://8.8.8.8:8000/v1"), "remote");
        assert_eq!(vllm_target_kind("http://172.32.0.1:8000/v1"), "remote"); // 172.32 不在私有段
                                                                             // 配置异常:空 / 非 URL
        assert_eq!(vllm_target_kind(""), "invalid");
        assert_eq!(vllm_target_kind("not-a-url"), "invalid");
    }

    #[test]
    fn parse_models_response_handles_vllm_shape() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"object":"list","data":[{"id":"/model","object":"model","max_model_len":65536}]}"#,
        )
        .unwrap();
        let (id, max) = parse_models_response(json).unwrap();
        assert_eq!(id.as_deref(), Some("/model"));
        assert_eq!(max, Some(65536));
    }

    #[test]
    fn parse_models_response_list_keeps_all_model_ids() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"object":"list","data":[{"id":"qwen2.5-coder:32b"},{"id":"deepseek-r1:14b","max_model_len":32768}]}"#,
        )
        .unwrap();
        let models = parse_models_response_list(json).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "qwen2.5-coder:32b");
        assert_eq!(models[0].max_model_len, None);
        assert_eq!(models[1].id, "deepseek-r1:14b");
        assert_eq!(models[1].max_model_len, Some(32768));
    }

    #[test]
    fn ollama_ps_names_collects_loaded_models() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"models":[{"name":"qwen3:8b","model":"qwen3:8b","size_vram":5000000000},{"model":"deepseek-r1:14b"}]}"#,
        )
        .unwrap();
        let names = parse_ollama_ps_names(json);
        assert!(names.contains("qwen3:8b"));
        assert!(names.contains("deepseek-r1:14b")); // 缺 name 时回退 model 字段
        assert!(!names.contains("llama3.2:3b"));
        // 坏形状按空集（宁全标未加载，不错标已加载）
        assert!(parse_ollama_ps_names(serde_json::json!({})).is_empty());
    }

    #[test]
    fn ollama_tag_names_dedupes_and_keeps_order() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"models":[{"name":"qwen3:8b"},{"name":"deepseek-r1:14b"},{"name":"qwen3:8b"},{"name":" "}]}"#,
        )
        .unwrap();
        assert_eq!(
            parse_ollama_tag_names(json),
            vec!["qwen3:8b".to_string(), "deepseek-r1:14b".to_string()]
        );
    }

    #[test]
    fn lmstudio_v0_models_parse_loaded_state() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"object":"list","data":[
                {"id":"qwen3-8b","state":"loaded"},
                {"id":"deepseek-r1-14b","state":"not-loaded"},
                {"id":"legacy-model"}
            ]}"#,
        )
        .unwrap();
        let models = parse_lmstudio_v0_models(&json).unwrap();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].loaded, Some(true));
        assert_eq!(models[1].loaded, Some(false));
        // 缺 state 字段 = 未知；空列表 / 坏形状返回 None，调用方回退 OpenAI 兼容探测。
        assert_eq!(models[2].loaded, None);
        assert!(parse_lmstudio_v0_models(&serde_json::json!({"data":[]})).is_none());
        assert!(parse_lmstudio_v0_models(&serde_json::json!({})).is_none());
    }

    /// 真机冒烟(#[ignore]):对活着的本地 vLLM 跑 `probe_vllm_model_info`,确认拿到
    /// **served name + max_model_len**——客户 bug 的核心修复点(丢 --served-model-name
    /// 后,名字虽无 _Nk 后缀,但 /v1/models 仍带 max_model_len,探测据此绕过名字依赖)。
    /// 跑法:
    ///   PINVOU3_LIVE_VLLM=http://127.0.0.1:8000/v1 cargo test --manifest-path \
    ///     pinvou3-app/src-tauri/Cargo.toml --lib -- --ignored --nocapture live_probe
    #[tokio::test]
    #[ignore]
    async fn live_probe_returns_window() {
        let base = std::env::var("PINVOU3_LIVE_VLLM")
            .unwrap_or_else(|_| "http://127.0.0.1:8000/v1".to_string());
        let (name, window) = probe_vllm_model_info(&base).await;
        eprintln!("live probe @ {base}: name={name:?} max_model_len={window:?}");
        let window = window.expect("真机 vLLM 必须探测到 max_model_len(客户 bug 的核心修复)");
        assert!(
            window >= 100_000,
            "窗口应为真实 max_model_len(期望 262144),实得 {window}"
        );
        // 端到端佐证:探测窗口喂进 derive 公式应得按窗口缩放的 T(非写死 190K)。
        // 复算 derive_compaction_threshold(bridge 私有,此处内联同公式):
        //   E = W − O − 1024;T = (E−S)/1.5 − 22000, clamp[4096, 0.75W]。O=24576(默认预留)。
        let e = (window as usize)
            .saturating_sub(24_576)
            .saturating_sub(1_024);
        let t = (e.saturating_sub(4_000).saturating_mul(2) / 3)
            .saturating_sub(22_000)
            .clamp(4_096, window as usize * 3 / 4);
        eprintln!("derived token_threshold for W={window}: T={t}  E={e}");
        assert!(
            t < e,
            "推导 T({t}) 必须低于紧急线 E({e})——nice 主路径先于 emergency(不倒置);\
             按真实窗口缩放,而非写死单值"
        );
    }

    /// 2026-06-10 本机 vLLM nightly(NVFP4) /metrics 实抓片段。
    /// 注意 TPOT 直方图真实名带 request_ 前缀。
    const REAL_METRICS_FIXTURE: &str = "\
# HELP vllm:prompt_tokens_total Number of prefill tokens processed.\n\
# TYPE vllm:prompt_tokens_total counter\n\
vllm:prompt_tokens_total{engine=\"0\",model_name=\"qwen36_35b_256k\"} 4.1367205e+07\n\
# HELP vllm:generation_tokens_total Number of generation tokens processed.\n\
# TYPE vllm:generation_tokens_total counter\n\
vllm:generation_tokens_total{engine=\"0\",model_name=\"qwen36_35b_256k\"} 295648.0\n\
vllm:time_to_first_token_seconds_bucket{engine=\"0\",le=\"0.001\",model_name=\"qwen36_35b_256k\"} 0.0\n\
vllm:time_to_first_token_seconds_created{engine=\"0\",model_name=\"qwen36_35b_256k\"} 1.7654321e+09\n\
vllm:time_to_first_token_seconds_count{engine=\"0\",model_name=\"qwen36_35b_256k\"} 498.0\n\
vllm:time_to_first_token_seconds_sum{engine=\"0\",model_name=\"qwen36_35b_256k\"} 1049.8486831188202\n\
vllm:request_time_per_output_token_seconds_count{engine=\"0\",model_name=\"qwen36_35b_256k\"} 495.0\n\
vllm:request_time_per_output_token_seconds_sum{engine=\"0\",model_name=\"qwen36_35b_256k\"} 6.363213540238716\n";

    #[test]
    fn perf_metrics_parse_from_real_fixture() {
        let m = parse_perf_metrics(REAL_METRICS_FIXTURE);
        assert_eq!(m.ttft_sum_s, Some(1049.8486831188202));
        assert_eq!(m.ttft_count, Some(498.0));
        assert_eq!(m.tpot_sum_s, Some(6.363213540238716));
        assert_eq!(m.tpot_count, Some(495.0));
        assert_eq!(m.generation_tokens_total, Some(295648.0));
        // 科学计数法 counter 也要能解析
        assert_eq!(m.prompt_tokens_total, Some(4.1367205e+07));
    }

    #[test]
    fn perf_metrics_all_none_when_metrics_absent() {
        let m = parse_perf_metrics("some_other_metric 1.0\n");
        assert!(m.ttft_sum_s.is_none());
        assert!(m.ttft_count.is_none());
        assert!(m.tpot_sum_s.is_none());
        assert!(m.tpot_count.is_none());
        assert!(m.generation_tokens_total.is_none());
        assert!(m.prompt_tokens_total.is_none());
    }

    /// 运行状态上下文长度推断：覆盖设置页全部云端模型（2026-07 逐厂商核实，
    /// 依据为仓库 catalog + 底座启发式 + 各厂商官方文档，见 pinvou_known_context_window 注释）。
    #[test]
    fn infer_context_window_cloud_models() {
        let cases: &[(ModelPreset, &str, u32)] = &[
            // DeepSeek：v4 全系 1M（原 bug：预设固定 128K）
            (ModelPreset::Deepseek, "deepseek-v4-pro", 1_000_000),
            (ModelPreset::Deepseek, "deepseek-v4-flash", 1_000_000),
            // Kimi：k3 是 1M，k2.x / for-coding 系 256K
            (ModelPreset::Kimi, "kimi-k3", 1_048_576),
            (ModelPreset::Kimi, "kimi-k2.7-code", 262_144),
            (ModelPreset::Kimi, "kimi-k2.7-code-highspeed", 262_144),
            (ModelPreset::Kimi, "kimi-k2.6", 262_144),
            // Kimi Coding Plan 走 openai_compatible 预设
            (ModelPreset::OpenaiCompatible, "kimi-for-coding", 262_144),
            (
                ModelPreset::OpenaiCompatible,
                "kimi-for-coding-highspeed",
                262_144,
            ),
            (ModelPreset::OpenaiCompatible, "k3-256k", 256_000),
            (ModelPreset::OpenaiCompatible, "k3", 1_048_576),
            // GLM：5.2 是 1M，5.1/5-turbo 是 202,752，4.7 官方 200K
            (ModelPreset::Glm, "glm-5.2", 1_000_000),
            (ModelPreset::Glm, "glm-5.1", 202_752),
            (ModelPreset::Glm, "glm-5-turbo", 202_752),
            (ModelPreset::Glm, "glm-4.7", 204_800),
            // MiniMax：M3 是 1M，M2.x 全系 204,800
            (ModelPreset::Minimax, "MiniMax-M3", 1_000_000),
            (ModelPreset::Minimax, "MiniMax-M2.7", 204_800),
            (ModelPreset::Minimax, "MiniMax-M2.7-highspeed", 204_800),
            (ModelPreset::Minimax, "MiniMax-M2.5", 204_800),
            (ModelPreset::Minimax, "MiniMax-M2.5-highspeed", 204_800),
            // MiMo：v2.5 全系 1M
            (ModelPreset::Mimo, "mimo-v2.5-pro", 1_000_000),
            (ModelPreset::Mimo, "mimo-v2.5", 1_000_000),
            // Qwen：3.7 全系 / 3.6-flash 均 1M
            (ModelPreset::Qwen, "qwen3.7-plus", 1_000_000),
            (ModelPreset::Qwen, "qwen3.7-max", 1_000_000),
            (ModelPreset::Qwen, "qwen3.7-flash", 1_000_000),
            (ModelPreset::Qwen, "qwen3.6-flash", 1_000_000),
            // 豆包：evolving 已升 1M，2.x 全系 256K
            (ModelPreset::Doubao, "doubao-seed-evolving", 1_048_576),
            (ModelPreset::Doubao, "doubao-seed-2.1-pro", 262_144),
            (ModelPreset::Doubao, "doubao-seed-2.1-turbo", 262_144),
            (ModelPreset::Doubao, "doubao-seed-2.0-pro", 262_144),
            (ModelPreset::Doubao, "doubao-seed-2.0-lite", 262_144),
            // OpenAI 兼容示例：gpt-5.6 全系 1.05M
            (ModelPreset::OpenaiCompatible, "gpt-5.6-terra", 1_050_000),
            (ModelPreset::OpenaiCompatible, "gpt-5.6-luna", 1_050_000),
            (ModelPreset::OpenaiCompatible, "gpt-5.6-sol", 1_050_000),
        ];
        for (preset, model, expected) in cases {
            assert_eq!(
                infer_context_window(*preset, Some(model)),
                Some(*expected),
                "{model} 上下文窗口推断错误"
            );
        }
    }

    /// 推断优先级：显式 Nk 后缀 > pinvou 补充表/底座 > 预设兜底。
    #[test]
    fn infer_context_window_fallback_order() {
        // 显式后缀优先于一切（含底座 catalog 里的同名模型）
        assert_eq!(
            infer_context_window(ModelPreset::Deepseek, Some("deepseek-v4-flash-128k")),
            Some(128_000)
        );
        // 底座与补充表都不认识的自定义模型名 → 预设兜底
        assert_eq!(
            infer_context_window(ModelPreset::Deepseek, Some("my-custom-finetune")),
            Some(131_072)
        );
        assert_eq!(
            infer_context_window(ModelPreset::Minimax, Some("minimax-future-x")),
            Some(204_800)
        );
        assert_eq!(
            infer_context_window(ModelPreset::Mimo, Some("mimo-future-x")),
            Some(1_000_000)
        );
        // 无模型名 → 预设兜底
        assert_eq!(infer_context_window(ModelPreset::Kimi, None), Some(262_144));
    }
}
