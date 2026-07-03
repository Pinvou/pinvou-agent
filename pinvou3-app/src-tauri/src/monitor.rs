//! GB10 设备 + vLLM 后端 + pinvou3-app 自身的健康/性能采样。
//!
//! 数据流：**按需采样**——前端在监控页面 mount 时启 1s interval 调
//! `get_monitor_snapshot`，离开页面就停。后端每次 command 直接跑一次
//! `sample_all`。设计目的：用户不在监控页面时**完全不跑 nvidia-smi**。
//! GPU util 峰值靠前端 5 个值滑窗 max（A+B）补足瞬时采样易错过推理峰的问题。
//!
//! 设计原则：**任何采样失败都 graceful degrade**——返回 None / OFFLINE，
//! 而不是 panic 或让上层崩。pinvou3 用户可能没装 nvidia-smi，可能没启 vLLM。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::Serialize;

/// 单次完整采样结果。所有字段 `Option`——采集失败就为 None。
#[derive(Debug, Clone, Default, Serialize)]
pub struct MonitorSnapshot {
    pub generated_at_ms: u64, // unix epoch ms
    pub gpu: Option<GpuSnapshot>,
    pub ram: Option<RamSnapshot>,
    pub vllm: Option<VllmSnapshot>,
    /// app 侧自测推理指标(TTFT / 生成速度 / 累计 tokens / KV)。与 vLLM `/metrics`
    /// 无关,任何后端(本地 vLLM / LM Studio / Ollama / 云端 API)都有值——因为是在
    /// 流式转发通路上就地测的。前端一律用这块显示这四项(vllm 块只剩队列/窗口/健康)。
    pub self_perf: SelfPerfSnapshot,
    pub app: AppSnapshot,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuSnapshot {
    pub name: String,
    pub vram_used_mib: u64,
    pub vram_total_mib: u64,
    pub utilization_pct: u32,
    /// GB10 等 unified-memory 设备 VRAM 字段是 [N/A]，UI 切到温度+功耗显示。
    pub temperature_c: Option<u32>,
    pub power_w: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RamSnapshot {
    pub total_kib: u64,
    pub used_kib: u64, // total - available
    pub swap_total_kib: u64,
    pub swap_used_kib: u64,
}

/// vLLM 健康 + 队列指标。`status` 永远有值（OFFLINE / READY / BUSY）。
#[derive(Debug, Clone, Serialize)]
pub struct VllmSnapshot {
    pub status: VllmStatus,
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
            },
        );
    }

    /// 首个 MessageDelta：记首 token 时点（仅首次）。TTFT 的停表点 + 生成时长起点。
    pub fn on_first_delta(&self, session_id: &str) {
        if let Some(t) = self.inflight.lock().get_mut(session_id) {
            if t.first.is_none() {
                t.first = Some(Instant::now());
            }
        }
    }

    /// 本轮出现过工具调用 → 标记。收尾时据此跳过 TTFT/TPS（D2）。
    pub fn on_tool(&self, session_id: &str) {
        if let Some(t) = self.inflight.lock().get_mut(session_id) {
            t.had_tool = true;
        }
    }

    /// TurnComplete：用精确 usage 累加。tokens/KV 永远记；TTFT/TPS 仅**纯文本 & 非首轮**记
    /// (工具轮墙钟含工具耗时 → D2 跳过；每 session 首轮含 cache warmup 冷启 → A 跳过)。
    /// 调用方已过滤 `output_tokens == 0` 的非 LLM/错误轮。
    pub fn on_turn_complete(
        &self,
        session_id: &str,
        input_tokens: u32,
        output_tokens: u32,
        cache_hit: Option<u32>,
        cache_miss: Option<u32>,
    ) {
        let timing = self.inflight.lock().remove(session_id);
        // HashSet::insert 返回 true = 之前不在集合里 = 这是该 session 首个完成轮(冷/warmup)。
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
        if let Some(t) = timing {
            if !t.had_tool && !is_first_turn {
                if let Some(first) = t.first {
                    p.ttft_sum_s += first.duration_since(t.start).as_secs_f64();
                    p.ttft_count += 1;
                    let gen_s = first.elapsed().as_secs_f64();
                    if gen_s > 0.0 && output_tokens > 0 {
                        p.tps_time_s += gen_s;
                        p.tps_tokens += output_tokens as u64;
                    }
                }
            }
        }
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

pub async fn sample_all(state: &MonitorState, vllm_upstream: &str, configured_model: Option<String>) -> MonitorSnapshot {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    MonitorSnapshot {
        generated_at_ms: now_ms,
        gpu: gpu_snapshot(),
        ram: crate::os::ram_snapshot(),
        vllm: vllm_snapshot(vllm_upstream, configured_model).await,
        self_perf: state.self_metrics.snapshot(),
        app: AppSnapshot {
            pinvou3_version: env!("CARGO_PKG_VERSION"),
            deepseek_tui_version: env!("CARGO_PKG_VERSION"), // TODO: 从 deepseek-tui crate 取
            session_uptime_secs: state.session_uptime_secs(),
        },
    }
}

/// Return GPU telemetry when the current platform provides NVIDIA probe candidates.
fn gpu_snapshot() -> Option<GpuSnapshot> {
    let args = [
        "--query-gpu=name,memory.used,memory.total,utilization.gpu,temperature.gpu,power.draw",
        "--format=csv,noheader,nounits",
    ];
    // Try platform-provided probe candidates in order.
    let out = crate::os::nvidia_smi_candidates()
        .into_iter()
        .find_map(|candidate| {
            crate::process::HiddenCommand::new(candidate)
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
        temperature_c: parts[4].parse().ok(),
        power_w: parts[5].parse().ok(),
    })
}

/// 健康探测 + Prometheus metrics 解析。
/// `/v1/models` 返不到 200 → OFFLINE；返 200 但 metrics 拿不到 → READY 无指标。
pub async fn vllm_snapshot(upstream: &str, configured_model: Option<String>) -> Option<VllmSnapshot> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    let target_kind = vllm_target_kind(upstream);

    // 1) /v1/models 健康
    // upstream 通常已带 `/v1` 后缀（DEEPSEEK_BASE_URL=http://...:8000/v1），
    // 所以直接拼 `/models`；不带的话补 `/v1/models`。
    let models_url = if upstream.trim_end_matches('/').ends_with("/v1") {
        format!("{}/models", upstream.trim_end_matches('/'))
    } else {
        format!("{}/v1/models", upstream.trim_end_matches('/'))
    };
    let models_resp = client.get(models_url).send().await;
    let models_resp = match models_resp {
        Ok(r) if r.status().is_success() => r,
        _ => {
            return Some(VllmSnapshot {
                status: VllmStatus::Offline,
                model: None,
                configured_model,
                upstream: upstream.to_string(),
                target_kind: target_kind.to_string(),
                metrics_applicable: target_kind == "local",
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
    };

    let (model_id, max_model_len) = match models_resp.json::<serde_json::Value>().await.ok() {
        Some(v) => parse_models_response(v).unwrap_or((None, None)),
        None => (None, None),
    };

    // 2) /metrics（用 host 根目录，不带 /v1）
    let metrics_url = strip_v1_suffix(upstream).map(|h| format!("{h}/metrics"));
    let metrics_text = match metrics_url {
        Some(u) => client.get(&u).send().await.ok(),
        None => None,
    };
    let metrics_text = match metrics_text {
        Some(r) if r.status().is_success() => r.text().await.ok(),
        _ => None,
    };

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
    if let Some(ref cfg) = configured_model {
        if let Some(ref actual) = model_id {
            if cfg.trim() != actual.trim() {
                status = VllmStatus::Mismatch;
            }
        }
    }

    Some(VllmSnapshot {
        status,
        model: model_id,
        configured_model,
        upstream: upstream.to_string(),
        target_kind: target_kind.to_string(),
        metrics_applicable: target_kind == "local",
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

fn parse_models_response(v: serde_json::Value) -> Option<(Option<String>, Option<u32>)> {
    let first = v.get("data")?.as_array()?.first()?;
    let id = first
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let max = first
        .get("max_model_len")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    Some((id, max))
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
    let Some(rest) = s.strip_prefix("http://").or_else(|| s.strip_prefix("https://")) else {
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
    let prefs = crate::bridge::prefs::UserPrefs::load();
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
    let prefs = crate::bridge::prefs::UserPrefs::load();
    match prefs.active_model() {
        // 本地 vLLM 动态跟随实际 served name(见 EnginePool::fresh_bridge_for),
        // 不声明固定配置目标 → 监控不做 mismatch 误报,只显示 vLLM 实际名字。
        Some(m) if m.preset == crate::bridge::prefs::ModelPreset::LocalVllm => None,
        Some(m) => Some(m.model.clone()),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_metrics_first_turn_per_session_skips_ttft_tps() {
        // 每 session 首轮 = 冷/warmup 轮:跳 TTFT/TPS,tokens/cache 照记(A)。
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_first_delta("s1"); // 幂等
        m.on_turn_complete("s1", 100, 50, Some(90), Some(10));
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 0); // 首轮跳过 TTFT
        assert_eq!(s.tps_tokens, 0); // 首轮跳过 TPS
        assert_eq!(s.gen_tokens_total, 50); // tokens 照记
        assert_eq!(s.prompt_tokens_total, 100);
        assert_eq!(s.cache_hit_tokens, 90); // cache 照记
        assert_eq!(s.cache_miss_tokens, 10);
    }

    #[test]
    fn self_metrics_second_pure_turn_records_ttft_tps() {
        let m = SelfMetrics::default();
        // 首轮(冷)跳
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_turn_complete("s1", 10, 5, None, None);
        // 二轮(暖)记
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_turn_complete("s1", 100, 50, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 1); // 仅二轮
        assert_eq!(s.tps_tokens, 50);
        assert!(s.tps_time_s > 0.0);
        assert_eq!(s.gen_tokens_total, 55); // 两轮 tokens 都在
    }

    #[test]
    fn self_metrics_tool_turn_skips_ttft_tps_but_keeps_tokens() {
        let m = SelfMetrics::default();
        // 先暖首轮 + 一轮纯文本(计 1 次 TTFT)
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
        assert_eq!(s.ttft_count, 1); // 工具轮没加,仍是第二轮那 1 次
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
        // 各 session 首轮独立跳过,不串台。
        let m = SelfMetrics::default();
        m.on_turn_started("a");
        m.on_turn_started("b");
        m.on_first_delta("a");
        m.on_first_delta("b");
        m.on_turn_complete("a", 1, 1, None, None); // a 首轮跳
        m.on_turn_complete("b", 1, 1, None, None); // b 首轮跳
        let s1 = m.snapshot();
        assert_eq!(s1.ttft_count, 0);
        assert_eq!(s1.gen_tokens_total, 2);
        // a 二轮记,b 仍只跑过首轮
        m.on_turn_started("a");
        m.on_first_delta("a");
        m.on_turn_complete("a", 1, 1, None, None);
        let s2 = m.snapshot();
        assert_eq!(s2.ttft_count, 1); // 仅 a 的二轮
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

    #[test]
    fn ram_snapshot_succeeds_on_supported_platform() {
        let s = crate::os::ram_snapshot().expect("RAM snapshot should be readable");
        assert!(s.total_kib > 0);
    }

    #[test]
    fn vllm_target_kind_classifies_by_host() {
        // 本地推理引擎:环回 + 私有 IP 段(含用户的 10.214.74.113 局域网自托管 vLLM)
        assert_eq!(vllm_target_kind("http://10.214.74.113:8000/v1"), "local");
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

    /// 真机冒烟(#[ignore]):对活着的本地 vLLM 跑 `probe_vllm_model_info`,确认拿到
    /// **served name + max_model_len**——客户 bug 的核心修复点(丢 --served-model-name
    /// 后,名字虽无 _Nk 后缀,但 /v1/models 仍带 max_model_len,探测据此绕过名字依赖)。
    /// 跑法:
    ///   PINVOU3_LIVE_VLLM=http://10.214.74.113:8000/v1 cargo test --manifest-path \
    ///     pinvou3-app/src-tauri/Cargo.toml --lib -- --ignored --nocapture live_probe
    #[tokio::test]
    #[ignore]
    async fn live_probe_returns_window() {
        let base = std::env::var("PINVOU3_LIVE_VLLM")
            .unwrap_or_else(|_| "http://10.214.74.113:8000/v1".to_string());
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
        let e = (window as usize).saturating_sub(24_576).saturating_sub(1_024);
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
}
