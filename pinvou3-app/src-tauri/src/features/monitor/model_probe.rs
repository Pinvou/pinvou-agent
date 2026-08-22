//! 模型探针：当前模型健康探测 + 本地 vLLM Prometheus metrics 解析。
//!
//! 职责边界——本模块只管「向 upstream 探一次模型健康 + 拉本地 vLLM `/metrics`」，
//! 不涉及系统自指标采集（CPU/GPU/内存见 [`super::self_metrics`]）。
//! 入口：[`active_model_snapshot`] / [`vllm_snapshot`] / [`snapshot_for_model_config`]。
//! 对外类型：[`VllmSnapshot`] / [`VllmStatus`] / [`MonitorDiagnostic`]。

use std::time::Duration;

use serde::Serialize;

use crate::core::model_endpoint::{is_anthropic_endpoint, models_probe_url, strip_v1_suffix};
use crate::platform::credential_store::{CredentialStore, SystemCredentialStore};
use crate::platform::prefs::{ModelPreset, SavedModel, UserPrefs};

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

#[derive(Debug, Clone, Serialize)]
pub struct MonitorDiagnostic {
    pub code: String,
    pub message: String,
}

impl VllmSnapshot {
    /// 构造一份带核心标识字段的快照,所有 vLLM `/metrics` 派生字段(`num_requests_*` /
    /// `prefix_cache_*` / `ttft_*` / `tpot_*` / `*_tokens_total` / `metric_diagnostics` /
    /// `max_model_len`)缺省 None/空。调用方(健康探测的各早退分支 + happy-path 起点)
    /// 共享同一组「指标缺失」默认值,happy-path 再逐项覆盖真实解析值。
    ///
    /// 这是原来散落在 `snapshot_for_model_config` 中的「离线 / 早退」构造块与
    /// `base_model_snapshot` helper 的收敛入口——行为保持:字段值与原三处构造完全一致。
    fn with_base(
        status: VllmStatus,
        model_id: Option<String>,
        provider: String,
        model: Option<String>,
        configured_model: Option<String>,
        upstream: &str,
        target_kind: &str,
        metrics_applicable: bool,
        health_status: &str,
        diagnostic: Option<MonitorDiagnostic>,
    ) -> Self {
        VllmSnapshot {
            status,
            model_id,
            provider,
            model,
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

    // 1) /models 健康
    let models_url = models_probe_url(upstream);
    let mut request = client.get(models_url);
    if let Some(key) = api_key.map(str::trim).filter(|key| !key.is_empty()) {
        if is_anthropic_endpoint(upstream) {
            request = request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01");
        } else {
            request = request.bearer_auth(key);
        }
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
            return Some(VllmSnapshot::with_base(
                VllmStatus::Offline,
                model_id,
                provider,
                configured_model.clone(),
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
                return Some(VllmSnapshot::with_base(
                    VllmStatus::Offline,
                    model_id,
                    provider,
                    configured_model.clone(),
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
                return Some(VllmSnapshot::with_base(
                    VllmStatus::Offline,
                    model_id,
                    provider,
                    configured_model.clone(),
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
            return Some(VllmSnapshot::with_base(
                VllmStatus::Offline,
                model_id,
                provider,
                None,
                configured_model,
                upstream,
                target_kind,
                metrics_applicable,
                "offline",
                Some(MonitorDiagnostic {
                    code: "models_unverified".to_string(),
                    message: "未探测 /v1/models".to_string(),
                }),
            ));
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

    // happy-path:从带默认值的 base 起步,再逐项覆盖真实解析值。
    let mut snapshot = VllmSnapshot::with_base(
        status,
        model_id,
        provider,
        if target_kind == "remote" {
            configured_model.clone().or(served_model)
        } else {
            served_model
        },
        configured_model,
        upstream,
        target_kind,
        metrics_applicable,
        health_status,
        diagnostic,
    );
    snapshot.metric_diagnostics = metric_diagnostics;
    snapshot.max_model_len = max_model_len;
    snapshot.num_requests_running = running;
    snapshot.num_requests_waiting = waiting;
    snapshot.prefix_cache_hit_pct = prefix_hit_pct;
    snapshot.prefix_cache_hits = prefix_cache_hits;
    snapshot.prefix_cache_queries = prefix_cache_queries;
    snapshot.ttft_sum_s = perf.ttft_sum_s;
    snapshot.ttft_count = perf.ttft_count;
    snapshot.tpot_sum_s = perf.tpot_sum_s;
    snapshot.tpot_count = perf.tpot_count;
    snapshot.generation_tokens_total = perf.generation_tokens_total;
    snapshot.prompt_tokens_total = perf.prompt_tokens_total;
    Some(snapshot)
}

fn parse_models_response(v: serde_json::Value) -> Option<(Option<String>, Option<u32>)> {
    let first = crate::core::model_endpoint::parse_models_response_list(v)?
        .into_iter()
        .next()?;
    Some((Some(first.id), first.max_model_len))
}

fn infer_context_window(preset: ModelPreset, model: Option<&str>) -> Option<u32> {
    // 模型名事实与 Engine route_limits 共用同一入口，避免页面显示 1M、实际仍按
    // 128K 压缩。底座与补充表都无法识别时按供应商预设兜底（表在 prefs::model_preset）。
    if let Some(window) = model.and_then(crate::core::model_context::resolved_context_window) {
        return Some(window);
    }
    preset.context_window_fallback(model)
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

/// 轻量探测本地 vLLM:一次 `/v1/models` 拿两样——实际 served 模型名 + `max_model_len`
/// (上下文窗口)。名字用于发请求(免写死名字与 `--served-model-name` 不一致的
/// model_not_found);窗口用于填 `active_route_limits.context_tokens`,让压缩阈值按真实
/// 窗口推导(见 docs/context-compaction-设计.md)。探测失败(vLLM 没起/超时)返回
/// `(None, None)`,调用方 fallback 配置值 + 名字 hint 老路。
pub async fn probe_vllm_model_info(base_url: &str) -> (Option<String>, Option<u32>) {
    // HTTP 层与 URL 拼装复用 core 的共享探测（避免 /v1/models 口径漂移）。
    match crate::core::model_endpoint::fetch_v1_models(base_url).await {
        Some(v) => parse_models_response(v).unwrap_or((None, None)),
        None => (None, None),
    }
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
    use crate::platform::prefs::ModelPreset;

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
            // Kimi：直连平台 kimi-k3 是 1M；Coding Plan 裸 k3 默认按 256K 安全值
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
            (ModelPreset::OpenaiCompatible, "k3", 262_144),
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
            // 底座 catalog 已知（haiku 200K）与 PINVOU_OVERRIDES 覆盖（opus-5 1M）
            // 的 Anthropic 模型走 resolved_context_window，preset 兜底见 prefs 测试。
            (ModelPreset::Anthropic, "claude-haiku-4-5", 200_000),
            (ModelPreset::Anthropic, "claude-opus-5", 1_000_000),
        ];
        for (preset, model, expected) in cases {
            assert_eq!(
                infer_context_window(*preset, Some(model)),
                Some(*expected),
                "{model} 上下文窗口推断错误"
            );
        }
    }

    /// 推断优先级：显式 Nk 后缀 > pinvou 补充表/底座 > 预设兜底
    /// （兜底表逐厂商覆盖见 prefs::model_preset 的 context_window_fallback 测试）。
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
        assert_eq!(infer_context_window(ModelPreset::Kimi, None), Some(262_144));
    }
}
