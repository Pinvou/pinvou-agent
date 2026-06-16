//! GB10 设备 + vLLM 后端 + pinvou3-app 自身的健康/性能采样。
//!
//! 数据流：**按需采样**——前端在监控页面 mount 时启 1s interval 调
//! `get_monitor_snapshot`，离开页面就停。后端每次 command 直接跑一次
//! `sample_all`。设计目的：用户不在监控页面时**完全不跑 nvidia-smi**。
//! GPU util 峰值靠前端 5 个值滑窗 max（A+B）补足瞬时采样易错过推理峰的问题。
//!
//! 设计原则：**任何采样失败都 graceful degrade**——返回 None / OFFLINE，
//! 而不是 panic 或让上层崩。pinvou3 用户可能没装 nvidia-smi，可能没启 vLLM。

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::bridge::{ModelMonitorTarget, ModelMonitorTargetKind};

/// 单次完整采样结果。所有字段 `Option`——采集失败就为 None。
#[derive(Debug, Clone, Default, Serialize)]
pub struct MonitorSnapshot {
    pub generated_at_ms: u64, // unix epoch ms
    pub gpu: Option<GpuSnapshot>,
    pub ram: Option<RamSnapshot>,
    pub vllm: Option<VllmSnapshot>,
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
    pub target_kind: ModelMonitorTargetKind,
    /// vLLM `/v1/models` 返回的真实模型名。
    pub model: Option<String>,
    /// 用户 settings 中配置的模型名（与 `model` 可能不同）。
    pub configured_model: Option<String>,
    pub provider: Option<String>,
    pub upstream: String,
    pub diagnostic: Option<StatusDiagnostic>,
    pub metrics_applicable: bool,
    pub metric_diagnostics: Vec<StatusDiagnostic>,
    pub max_model_len: Option<u32>,
    pub num_requests_running: Option<f64>,
    pub num_requests_waiting: Option<f64>,
    /// 历史累计 prefix cache 命中率: hits_total / queries_total × 100。
    /// 反映"重复 prompt prefix 复用 KV 比例",直接关联首字延迟。
    /// 瞬时 kv_cache_usage_perc 单用户场景一直是 0-2%,意义不大,已替换。
    pub prefix_cache_hit_pct: Option<f64>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VllmStatus {
    Offline,
    Ready,
    Busy,
    Unknown,
    /// 配置的模型名与 vLLM 实际返回的模型名不一致。vLLM 服务在线但聊天会报 model_not_found。
    Mismatch,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusDiagnostic {
    pub code: &'static str,
    pub message: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AppSnapshot {
    pub pinvou3_version: &'static str,
    pub deepseek_tui_version: &'static str,
    pub session_uptime_secs: u64,
}

/// Monitor 状态——只持有 session 起始时间，sample 全部按需。
#[derive(Debug, Clone, Default)]
pub struct MonitorState {
    started_at: Option<Instant>,
}

impl MonitorState {
    pub fn new() -> Self {
        Self {
            started_at: Some(Instant::now()),
        }
    }

    pub fn session_uptime_secs(&self) -> u64 {
        self.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0)
    }
}

pub async fn sample_all(state: &MonitorState, model_target: ModelMonitorTarget) -> MonitorSnapshot {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    MonitorSnapshot {
        generated_at_ms: now_ms,
        gpu: gpu_snapshot(),
        ram: crate::os::ram_snapshot(),
        vllm: model_target_snapshot(model_target).await,
        app: AppSnapshot {
            pinvou3_version: env!("CARGO_PKG_VERSION"),
            deepseek_tui_version: env!("CARGO_PKG_VERSION"), // TODO: 从 deepseek-tui crate 取
            session_uptime_secs: state.session_uptime_secs(),
        },
    }
}

/// 调 `nvidia-smi` 查 GPU。本机没 GPU/没装 nvidia-smi → None。
/// 桌面环境启动时 PATH 可能不含 nvidia-smi，加常见绝对路径 fallback。
fn gpu_snapshot() -> Option<GpuSnapshot> {
    let args = [
        "--query-gpu=name,memory.used,memory.total,utilization.gpu,temperature.gpu,power.draw",
        "--format=csv,noheader,nounits",
    ];
    let out = crate::os::nvidia_smi_candidates().into_iter().find_map(|cmd| {
        std::process::Command::new(cmd)
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
    let target = ModelMonitorTarget {
        base_url: upstream.to_string(),
        configured_model,
        provider: "vllm".to_string(),
        kind: ModelMonitorTargetKind::Local,
        source: "discover".to_string(),
        api_key: None,
    };
    model_target_snapshot(target).await
}

pub async fn model_target_snapshot(target: ModelMonitorTarget) -> Option<VllmSnapshot> {
    match target.kind {
        ModelMonitorTargetKind::Invalid => Some(empty_snapshot(
            target,
            VllmStatus::Offline,
            Some(diagnostic(
                "invalid_config",
                "模型配置无效",
                Some("base_url 或模型名为空，或 base_url 不是有效 URL".to_string()),
            )),
            false,
            Vec::new(),
        )),
        ModelMonitorTargetKind::Remote => remote_model_snapshot(target).await,
        ModelMonitorTargetKind::Local => local_model_snapshot(target).await,
    }
}

async fn remote_model_snapshot(target: ModelMonitorTarget) -> Option<VllmSnapshot> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    let models_url = models_url(&target.base_url);
    let mut req = client.get(models_url);
    if let Some(key) = target.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
        req = req.bearer_auth(key);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            let code = if e.is_timeout() {
                "request_timeout"
            } else {
                "connection_failed"
            };
            let message = if e.is_timeout() {
                "远端模型请求超时"
            } else {
                "远端模型连接失败"
            };
            return Some(empty_snapshot(
                target,
                VllmStatus::Offline,
                Some(diagnostic(code, message, Some(e.to_string()))),
                false,
                Vec::new(),
            ));
        }
    };
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED
        || resp.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Some(empty_snapshot(
            target,
            VllmStatus::Offline,
            Some(diagnostic(
                "unauthorized",
                "远端模型鉴权失败",
                Some(format!("HTTP {}", resp.status())),
            )),
            false,
            Vec::new(),
        ));
    }
    if !resp.status().is_success() {
        return Some(empty_snapshot(
            target,
            VllmStatus::Unknown,
            Some(diagnostic(
                "unexpected_response",
                "远端模型响应异常，无法确认状态",
                Some(format!("HTTP {}", resp.status())),
            )),
            false,
            Vec::new(),
        ));
    }
    let json = match resp.json::<serde_json::Value>().await {
        Ok(v) => v,
        Err(e) => {
            return Some(empty_snapshot(
                target,
                VllmStatus::Unknown,
                Some(diagnostic(
                    "unexpected_response",
                    "远端模型响应不是有效的模型列表",
                    Some(e.to_string()),
                )),
                false,
                Vec::new(),
            ));
        }
    };
    let ids = parse_model_ids(&json);
    if ids.is_empty() {
        return Some(empty_snapshot(
            target,
            VllmStatus::Unknown,
            Some(diagnostic(
                "unexpected_response",
                "远端模型响应缺少模型列表",
                None,
            )),
            false,
            Vec::new(),
        ));
    }
    let actual = match target.configured_model.as_deref() {
        Some(configured) => ids
            .iter()
            .find(|id| id.trim() == configured.trim())
            .cloned()
            .or_else(|| ids.first().cloned()),
        None => ids.first().cloned(),
    };
    let mut status = VllmStatus::Ready;
    let mut diagnostic_value = None;
    if let (Some(configured), Some(actual)) = (target.configured_model.as_deref(), actual.as_deref()) {
        if configured.trim() != actual.trim() {
            status = VllmStatus::Mismatch;
            diagnostic_value = Some(diagnostic(
                "model_mismatch",
                "远端服务返回的模型与当前配置不一致",
                Some(format!("configured={configured}, actual={actual}")),
            ));
        }
    }
    Some(VllmSnapshot {
        status,
        target_kind: target.kind,
        model: actual,
        configured_model: target.configured_model,
        provider: Some(target.provider),
        upstream: target.base_url,
        diagnostic: diagnostic_value,
        metrics_applicable: false,
        metric_diagnostics: vec![diagnostic(
            "remote_metrics_not_applicable",
            "远端模型不提供本地运行指标",
            None,
        )],
        max_model_len: None,
        num_requests_running: None,
        num_requests_waiting: None,
        prefix_cache_hit_pct: None,
        ttft_sum_s: None,
        ttft_count: None,
        tpot_sum_s: None,
        tpot_count: None,
        generation_tokens_total: None,
        prompt_tokens_total: None,
    })
}

async fn local_model_snapshot(target: ModelMonitorTarget) -> Option<VllmSnapshot> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;

    // 1) /v1/models 健康
    // upstream 通常已带 `/v1` 后缀（DEEPSEEK_BASE_URL=http://...:8000/v1），
    // 所以直接拼 `/models`；不带的话补 `/v1/models`。
    let models_resp = client.get(models_url(&target.base_url)).send().await;
    let models_resp = match models_resp {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            return Some(empty_snapshot(
                target,
                VllmStatus::Offline,
                Some(diagnostic(
                    "unexpected_response",
                    "本地模型响应异常或不是模型服务",
                    Some(format!("HTTP {}", r.status())),
                )),
                true,
                Vec::new(),
            ));
        }
        Err(e) => {
            let code = if e.is_timeout() {
                "request_timeout"
            } else {
                "connection_failed"
            };
            let message = if e.is_timeout() {
                "本地模型请求超时"
            } else {
                "本地模型连接失败"
            };
            return Some(empty_snapshot(
                target,
                VllmStatus::Offline,
                Some(diagnostic(code, message, Some(e.to_string()))),
                true,
                Vec::new(),
            ));
        }
    };

    let (model_id, max_model_len) = match models_resp.json::<serde_json::Value>().await.ok() {
        Some(v) => parse_models_response(v).unwrap_or((None, None)),
        None => (None, None),
    };

    // 2) /metrics（用 host 根目录，不带 /v1）
    let metrics_url = strip_v1_suffix(&target.base_url).map(|h| format!("{h}/metrics"));
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
    let prefix_hit_pct = metrics_text.as_deref().and_then(|t| {
        let hits = parse_prom_metric(t, "vllm:prefix_cache_hits_total")?;
        let queries = parse_prom_metric(t, "vllm:prefix_cache_queries_total")?;
        if queries > 0.0 {
            Some(hits / queries * 100.0)
        } else {
            None
        }
    });

    let perf = metrics_text
        .as_deref()
        .map(parse_perf_metrics)
        .unwrap_or_default();

    let mut status = match (running, waiting) {
        (Some(r), _) if r > 0.0 => VllmStatus::Busy,
        (_, Some(w)) if w > 0.0 => VllmStatus::Busy,
        _ => VllmStatus::Ready,
    };
    let mut diagnostic_value = None;
    // 如果用户配置了模型名，但和 vLLM 实际返回的不一致，降级为 Mismatch。
    // 这样监控台不会显示绿色 READY，聊天 live dot 也会变红。
    if let Some(ref cfg) = target.configured_model {
        if let Some(ref actual) = model_id {
            if cfg.trim() != actual.trim() {
                status = VllmStatus::Mismatch;
                diagnostic_value = Some(diagnostic(
                    "model_mismatch",
                    "本地服务返回的模型与当前配置不一致",
                    Some(format!("configured={cfg}, actual={actual}")),
                ));
            }
        }
    }
    let mut metric_diagnostics = Vec::new();
    if metrics_text.is_none() {
        metric_diagnostics.push(diagnostic(
            "metrics_unavailable",
            "本地模型运行指标暂不可用",
            None,
        ));
    } else if running.is_none()
        || waiting.is_none()
        || max_model_len.is_none()
        || prefix_hit_pct.is_none()
    {
        metric_diagnostics.push(diagnostic(
            "metric_missing",
            "部分本地模型运行指标缺失",
            None,
        ));
    }

    Some(VllmSnapshot {
        status,
        target_kind: target.kind,
        model: model_id,
        configured_model: target.configured_model,
        provider: Some(target.provider),
        upstream: target.base_url,
        diagnostic: diagnostic_value,
        metrics_applicable: true,
        metric_diagnostics,
        max_model_len,
        num_requests_running: running,
        num_requests_waiting: waiting,
        prefix_cache_hit_pct: prefix_hit_pct,
        ttft_sum_s: perf.ttft_sum_s,
        ttft_count: perf.ttft_count,
        tpot_sum_s: perf.tpot_sum_s,
        tpot_count: perf.tpot_count,
        generation_tokens_total: perf.generation_tokens_total,
        prompt_tokens_total: perf.prompt_tokens_total,
    })
}

fn diagnostic(code: &'static str, message: &str, detail: Option<String>) -> StatusDiagnostic {
    StatusDiagnostic {
        code,
        message: message.to_string(),
        detail,
    }
}

fn empty_snapshot(
    target: ModelMonitorTarget,
    status: VllmStatus,
    diagnostic_value: Option<StatusDiagnostic>,
    metrics_applicable: bool,
    metric_diagnostics: Vec<StatusDiagnostic>,
) -> VllmSnapshot {
    VllmSnapshot {
        status,
        target_kind: target.kind,
        model: None,
        configured_model: target.configured_model,
        provider: Some(target.provider),
        upstream: target.base_url,
        diagnostic: diagnostic_value,
        metrics_applicable,
        metric_diagnostics,
        max_model_len: None,
        num_requests_running: None,
        num_requests_waiting: None,
        prefix_cache_hit_pct: None,
        ttft_sum_s: None,
        ttft_count: None,
        tpot_sum_s: None,
        tpot_count: None,
        generation_tokens_total: None,
        prompt_tokens_total: None,
    }
}

fn models_url(upstream: &str) -> String {
    if upstream.trim_end_matches('/').ends_with("/v1") {
        format!("{}/models", upstream.trim_end_matches('/'))
    } else {
        format!("{}/v1/models", upstream.trim_end_matches('/'))
    }
}

fn parse_model_ids(v: &serde_json::Value) -> Vec<String> {
    v.get("data")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(|v| v.as_str()))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
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

fn strip_v1_suffix(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    Some(
        trimmed
            .strip_suffix("/v1")
            .map(String::from)
            .unwrap_or_else(|| trimmed.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn sample_all_includes_os_ram_and_app_snapshot() {
        let state = MonitorState::new();
        let snapshot = sample_all(&state, test_target("http://127.0.0.1:1/v1", ModelMonitorTargetKind::Local)).await;

        assert_eq!(snapshot.app.pinvou3_version, env!("CARGO_PKG_VERSION"));
        assert!(snapshot.ram.is_some());
        let ram = snapshot.ram.unwrap();
        assert!(ram.total_kib > 0);
        assert!(ram.used_kib <= ram.total_kib);
    }

    fn test_target(base_url: &str, kind: ModelMonitorTargetKind) -> ModelMonitorTarget {
        ModelMonitorTarget {
            base_url: base_url.to_string(),
            configured_model: Some("qwen36_35b_256k".to_string()),
            provider: "vllm".to_string(),
            kind,
            source: "test".to_string(),
            api_key: None,
        }
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
    fn parse_model_ids_handles_openai_compatible_shape() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"object":"list","data":[{"id":"model-a"},{"id":"model-b"}]}"#,
        )
        .unwrap();
        assert_eq!(parse_model_ids(&json), vec!["model-a".to_string(), "model-b".to_string()]);
    }

    #[tokio::test]
    async fn invalid_target_returns_invalid_config_diagnostic() {
        let mut target = test_target("", ModelMonitorTargetKind::Invalid);
        target.configured_model = None;
        let snapshot = model_target_snapshot(target).await.unwrap();
        assert_eq!(snapshot.status, VllmStatus::Offline);
        assert_eq!(snapshot.target_kind, ModelMonitorTargetKind::Invalid);
        assert_eq!(snapshot.diagnostic.as_ref().map(|d| d.code), Some("invalid_config"));
        assert!(!snapshot.metrics_applicable);
    }

    #[tokio::test]
    async fn remote_target_marks_local_metrics_not_applicable() {
        let target = test_target("http://127.0.0.1:1/v1", ModelMonitorTargetKind::Remote);
        let snapshot = model_target_snapshot(target).await.unwrap();
        assert_eq!(snapshot.target_kind, ModelMonitorTargetKind::Remote);
        assert!(!snapshot.metrics_applicable);
        assert!(
            snapshot
                .metric_diagnostics
                .iter()
                .any(|d| d.code == "remote_metrics_not_applicable")
                || snapshot.diagnostic.is_some()
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
