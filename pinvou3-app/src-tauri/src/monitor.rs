//! GB10 设备 + vLLM 后端 + pinvou3-app 自身的健康/性能采样。
//!
//! 数据流：后台 task 每 5s 跑一次 `sample_all`（async），结果写入
//! `Arc<RwLock<MonitorSnapshot>>` 缓存。Tauri command `get_monitor_snapshot()`
//! 直接读缓存——避免每次前端 invoke 都跑 `nvidia-smi` 或 HTTP 请求。
//!
//! 设计原则：**任何采样失败都 graceful degrade**——返回 None / OFFLINE，
//! 而不是 panic 或让上层崩。pinvou3 用户可能没装 nvidia-smi，可能没启 vLLM。

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::RwLock;

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
    pub model: Option<String>,
    pub upstream: String,
    pub max_model_len: Option<u32>,
    pub num_requests_running: Option<f64>,
    pub num_requests_waiting: Option<f64>,
    pub kv_cache_usage_pct: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VllmStatus {
    Offline,
    Ready,
    Busy,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AppSnapshot {
    pub pinvou3_version: &'static str,
    pub deepseek_tui_version: &'static str,
    pub session_uptime_secs: u64,
}

/// 持有缓存的 Monitor 状态，可 clone（内部 Arc）。
#[derive(Debug, Clone, Default)]
pub struct MonitorState {
    inner: Arc<RwLock<MonitorSnapshot>>,
    started_at: Option<Instant>,
}

impl MonitorState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(MonitorSnapshot::default())),
            started_at: Some(Instant::now()),
        }
    }

    pub async fn snapshot(&self) -> MonitorSnapshot {
        self.inner.read().await.clone()
    }

    pub async fn replace(&self, snap: MonitorSnapshot) {
        *self.inner.write().await = snap;
    }

    pub fn session_uptime_secs(&self) -> u64 {
        self.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0)
    }
}

/// 后台任务：每 `interval` 跑一次 `sample_all`，结果写入 state。
/// 通常在 Tauri setup() 里 spawn 一次，进程整个生命周期都活着。
pub fn spawn_sampler(state: MonitorState, interval: Duration) {
    tauri::async_runtime::spawn(async move {
        let upstream = vllm_base_url();
        loop {
            let snap = sample_all(&state, &upstream).await;
            state.replace(snap).await;
            tokio::time::sleep(interval).await;
        }
    });
}

async fn sample_all(state: &MonitorState, vllm_upstream: &str) -> MonitorSnapshot {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    MonitorSnapshot {
        generated_at_ms: now_ms,
        gpu: gpu_snapshot(),
        ram: ram_snapshot(),
        vllm: vllm_snapshot(vllm_upstream).await,
        app: AppSnapshot {
            pinvou3_version: env!("CARGO_PKG_VERSION"),
            deepseek_tui_version: env!("CARGO_PKG_VERSION"), // TODO: 从 deepseek-tui crate 取
            session_uptime_secs: state.session_uptime_secs(),
        },
    }
}

/// 调 `nvidia-smi` 查 GPU。本机没 GPU/没装 nvidia-smi → None。
fn gpu_snapshot() -> Option<GpuSnapshot> {
    let out = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.used,memory.total,utilization.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = std::str::from_utf8(&out.stdout).ok()?.lines().next()?;
    let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
    if parts.len() < 4 {
        return None;
    }
    Some(GpuSnapshot {
        name: parts[0].to_string(),
        vram_used_mib: parts[1].parse().ok()?,
        vram_total_mib: parts[2].parse().ok()?,
        utilization_pct: parts[3].parse().ok()?,
    })
}

/// 读 `/proc/meminfo`。Linux 专有，其他 OS → None。
fn ram_snapshot() -> Option<RamSnapshot> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = None;
    let mut available = None;
    let mut swap_total = None;
    let mut swap_free = None;
    for line in text.lines() {
        let (key, val) = line.split_once(':')?;
        let kib: u64 = val.trim().trim_end_matches(" kB").parse().ok().unwrap_or(0);
        match key {
            "MemTotal" => total = Some(kib),
            "MemAvailable" => available = Some(kib),
            "SwapTotal" => swap_total = Some(kib),
            "SwapFree" => swap_free = Some(kib),
            _ => {}
        }
    }
    let total = total?;
    let available = available?;
    Some(RamSnapshot {
        total_kib: total,
        used_kib: total.saturating_sub(available),
        swap_total_kib: swap_total.unwrap_or(0),
        swap_used_kib: swap_total
            .unwrap_or(0)
            .saturating_sub(swap_free.unwrap_or(0)),
    })
}

/// 健康探测 + Prometheus metrics 解析。
/// `/v1/models` 返不到 200 → OFFLINE；返 200 但 metrics 拿不到 → READY 无指标。
async fn vllm_snapshot(upstream: &str) -> Option<VllmSnapshot> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;

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
                upstream: upstream.to_string(),
                max_model_len: None,
                num_requests_running: None,
                num_requests_waiting: None,
                kv_cache_usage_pct: None,
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
    let kv = metrics_text
        .as_deref()
        .and_then(|t| parse_prom_metric(t, "vllm:kv_cache_usage_perc"));

    let status = match (running, waiting) {
        (Some(r), _) if r > 0.0 => VllmStatus::Busy,
        (_, Some(w)) if w > 0.0 => VllmStatus::Busy,
        _ => VllmStatus::Ready,
    };

    Some(VllmSnapshot {
        status,
        model: model_id,
        upstream: upstream.to_string(),
        max_model_len,
        num_requests_running: running,
        num_requests_waiting: waiting,
        kv_cache_usage_pct: kv.map(|v| v * 100.0),
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

fn vllm_base_url() -> String {
    std::env::var("DEEPSEEK_BASE_URL")
        .unwrap_or_else(|_| "http://10.214.74.113:8000/v1".to_string())
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

    #[test]
    fn ram_snapshot_succeeds_on_linux() {
        // 跑测试的环境（GB10/笔记本都是 Linux）一定有 /proc/meminfo
        let s = ram_snapshot().expect("/proc/meminfo should be readable");
        assert!(s.total_kib > 0);
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
}
