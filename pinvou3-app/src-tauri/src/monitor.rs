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
    /// vLLM `/v1/models` 返回的真实模型名。
    pub model: Option<String>,
    /// 用户 settings 中配置的模型名（与 `model` 可能不同）。
    pub configured_model: Option<String>,
    pub upstream: String,
    pub max_model_len: Option<u32>,
    pub num_requests_running: Option<f64>,
    pub num_requests_waiting: Option<f64>,
    /// 历史累计 prefix cache 命中率: hits_total / queries_total × 100。
    /// 反映"重复 prompt prefix 复用 KV 比例",直接关联首字延迟。
    /// 瞬时 kv_cache_usage_perc 单用户场景一直是 0-2%,意义不大,已替换。
    pub prefix_cache_hit_pct: Option<f64>,
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

pub async fn sample_all(state: &MonitorState, vllm_upstream: &str, configured_model: Option<String>) -> MonitorSnapshot {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    MonitorSnapshot {
        generated_at_ms: now_ms,
        gpu: gpu_snapshot(),
        ram: ram_snapshot(),
        vllm: vllm_snapshot(vllm_upstream, configured_model).await,
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
    // 先试 PATH 查找，再试常见绝对路径
    let out = std::process::Command::new("nvidia-smi")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .or_else(|| {
            std::process::Command::new("/usr/bin/nvidia-smi")
                .args(args)
                .output()
                .ok()
                .filter(|o| o.status.success())
        })
        .or_else(|| {
            std::process::Command::new("/usr/local/bin/nvidia-smi")
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
pub async fn vllm_snapshot(upstream: &str, configured_model: Option<String>) -> Option<VllmSnapshot> {
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
                configured_model,
                upstream: upstream.to_string(),
                max_model_len: None,
                num_requests_running: None,
                num_requests_waiting: None,
                prefix_cache_hit_pct: None,
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
    let prefix_hit_pct = metrics_text.as_deref().and_then(|t| {
        let hits = parse_prom_metric(t, "vllm:prefix_cache_hits_total")?;
        let queries = parse_prom_metric(t, "vllm:prefix_cache_queries_total")?;
        if queries > 0.0 {
            Some(hits / queries * 100.0)
        } else {
            None
        }
    });

    let status = match (running, waiting) {
        (Some(r), _) if r > 0.0 => VllmStatus::Busy,
        (_, Some(w)) if w > 0.0 => VllmStatus::Busy,
        _ => VllmStatus::Ready,
    };

    Some(VllmSnapshot {
        status,
        model: model_id,
        configured_model,
        upstream: upstream.to_string(),
        max_model_len,
        num_requests_running: running,
        num_requests_waiting: waiting,
        prefix_cache_hit_pct: prefix_hit_pct,
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

/// 当前 monitor/探测应使用的 vLLM base_url。
/// 优先级：环境变量 `DEEPSEEK_BASE_URL` > settings.json `custom_base_url` > 默认值。
/// 与 Engine 使用的逻辑保持一致（见 `bridge::Pinvou3Bridge::base_url`）。
pub fn vllm_base_url() -> String {
    if let Ok(v) = std::env::var("DEEPSEEK_BASE_URL") {
        return v;
    }
    let prefs = crate::bridge::prefs::UserPrefs::load();
    prefs
        .advanced
        .custom_base_url
        .unwrap_or_else(|| "http://127.0.0.1:8000/v1".to_string())
}

/// 用户配置的模型名（用于 monitor 显示"配置目标"）。
/// 优先级：环境变量 `DEEPSEEK_MODEL` > settings.json `custom_model_name` > None。
pub fn vllm_configured_model() -> Option<String> {
    if let Ok(v) = std::env::var("DEEPSEEK_MODEL") {
        return Some(v);
    }
    let prefs = crate::bridge::prefs::UserPrefs::load();
    prefs.advanced.custom_model_name
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
