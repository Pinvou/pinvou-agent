//! 模型探针：当前模型健康探测 + 本地 vLLM Prometheus metrics 解析。
//!
//! 职责边界——本模块只管「向 upstream 探一次模型健康 + 拉本地 vLLM `/metrics`」，
//! 不涉及系统自指标采集（CPU/GPU/内存见 [`super::self_metrics`]）。
//! Entrypoints: [`active_model_snapshot`] / [`snapshot_for_model_config`].
//! 对外类型：[`VllmSnapshot`] / [`VllmStatus`] / [`HealthStatus`] / [`MonitorDiagnostic`]。

use std::time::Duration;

use serde::Serialize;

use crate::core::model_endpoint::{is_anthropic_endpoint, models_probe_url, strip_v1_suffix};
use crate::platform::credential_store::{CredentialStore, SystemCredentialStore};
use crate::platform::prefs::{ModelPreset, SavedModel, UserPrefs};

/// Upper bound on a buffered `/metrics` body. The poll runs at ~1 Hz and
/// reqwest buffers the whole response in `text()`, so a misbehaving local
/// endpoint must not balloon memory; a real vLLM export is tens to a few
/// hundred KiB. Applies to the declared `Content-Length` only — chunked
/// responses are bounded by the 3 s request timeout instead.
const MAX_METRICS_BODY_BYTES: u64 = 16 * 1024 * 1024;

/// 当前模型运行态 + 本地 vLLM 指标。字段名暂保留 vllm 兼容前端。
#[derive(Debug, Clone, Default, Serialize)]
pub struct VllmSnapshot {
    pub status: VllmStatus,
    /// vLLM `/v1/models` 返回的真实模型名。
    pub model: Option<String>,
    /// 用户 settings 中配置的模型名（与 `model` 可能不同）。
    pub configured_model: Option<String>,
    /// 后端类型(前端监控卡显示标签 + 决定 vLLM 指标是否适用 + 小窗口告警是否触发):
    /// `local` 本地推理引擎(环回/私有 IP,自托管 vLLM,有 Prometheus 指标)/
    /// `remote` 云端 API(公网,无 /metrics)/ `invalid` 配置异常(base_url 解析失败)。
    pub target_kind: String,
    /// vLLM Prometheus 指标是否适用(= `target_kind == "local"`);云端 API 无 /metrics。
    pub metrics_applicable: bool,
    /// 健康判定的权威通道。枚举经 serde 渲染成既有的 snake_case wire 字符串
    /// (前端/CLI 已消费的取值逐字不变);用枚举而非 `String`,是让 producer
    /// 与消费方在字面量上不可能漂移(round-2 审:一个拼写 typo 就能让诊断
    /// 静默消失且零测试红)。
    pub health_status: HealthStatus,
    /// `health_status` 的精化诊断,恒有不变式
    /// `diagnostic.is_some() == (health_status == HealthStatus::Offline)`:
    /// `offline` 精化为 `models_http_error`(带 `http_status`)或
    /// `models_unreachable`(带 `transport`/`endpoint`);其余健康态自身即
    /// 结论,恒为 `None`。条目只携带机器可判定的稳定字段、不含人读 prose
    /// (UI 文案按 `code` 走 `shared/i18n.js` 映射,见 `MonitorDiagnostic`)。
    pub diagnostic: Option<MonitorDiagnostic>,
    /// Non-fatal notices with stable codes, independent of the health verdict:
    /// `metrics_unavailable` when a local `/metrics` is unreachable or its
    /// body parses to no vLLM series (an all-`None` metric set is never left
    /// silently readable as "no traffic"); `context_window_inferred` when the
    /// display window came from model-name / preset inference — this one
    /// fires for remote targets too (`metrics_applicable == false`), so never
    /// gate rendering of this vec on metrics applicability.
    pub notices: Vec<MonitorDiagnostic>,
    pub max_model_len: Option<u32>,
    /// vLLM queue-depth gauges (the Busy source for `status`; the CLI human
    /// line renders them as `queue=R/W`). Summed across every series of the
    /// metric, so under data-parallel (`engine="0"/"1"`) this is the fleet
    /// total in flight, not engine 0's queue.
    pub num_requests_running: Option<f64>,
    pub num_requests_waiting: Option<f64>,
    /// prefix cache 原始计数器（hits_total / queries_total）。前端「清除统计」
    /// 用基准点对各累计 counter 做减法重算命中率,必须拿到原始分子/分母,
    /// 只给百分比无法做区间重算。
    pub prefix_cache_hits: Option<f64>,
    pub prefix_cache_queries: Option<f64>,
    /// TTFT 直方图累计值（vllm:time_to_first_token_seconds_sum/_count）。
    /// 累积平均 = sum/count。counter 跟随 vLLM 进程生命周期，
    /// 换模型 = 重启进程 = 自动归零，因此天然按模型分段。
    pub ttft_sum_s: Option<f64>,
    pub ttft_count: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VllmStatus {
    #[default]
    Offline,
    Ready,
    Busy,
    /// 配置的模型名与 vLLM 实际返回的模型名不一致。vLLM 服务在线但聊天会报 model_not_found。
    Mismatch,
}

/// 健康判定。serde 渲染(`rename_all = "snake_case"`)与既有 wire 字符串逐字一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Verified,
    AuthFailed,
    #[default]
    Offline,
    MissingApiKey,
    Unverified,
    Mismatch,
}

impl HealthStatus {
    /// 与 serde 渲染逐字一致的人读标签(CLI 人读行用);由 CLI 的
    /// `health_label_matches_the_serde_rendering_of_every_variant` 及显式
    /// 字面量断言钉死,防 `rename_all` 变更静默改 wire。
    pub fn as_str(&self) -> &'static str {
        match self {
            HealthStatus::Verified => "verified",
            HealthStatus::AuthFailed => "auth_failed",
            HealthStatus::Offline => "offline",
            HealthStatus::MissingApiKey => "missing_api_key",
            HealthStatus::Unverified => "unverified",
            HealthStatus::Mismatch => "mismatch",
        }
    }
}

/// 结构化诊断条目:只有机器可判定的稳定事实。`code` 是稳定分类,人读文案
/// 必须由 UI 按 `code` 走 `shared/i18n.js` 映射,快照不携带 prose——
/// reqwest 的 `Display` 会内嵌完整请求 URL(用户 base_url 可能带 userinfo
/// 或 token 形查询串),所以诊断一律从解析后的字段构造,不经错误文本。
#[derive(Debug, Clone, Serialize)]
pub struct MonitorDiagnostic {
    pub code: String,
    /// HTTP 错误时的状态码。
    pub http_status: Option<u16>,
    /// 传输错误时的分类,词表与 `core::model_endpoint::model_probe_transport_code`
    /// 一致(`timeout` / `dns_failed` / `connection_refused` / `network_error`),但改用
    /// reqwest 的结构化 flag 判定(真实 reqwest 错误的 `Display` 不含
    /// source 链,那边的字符串臂对它们是死代码)。
    pub transport: Option<&'static str>,
    /// 目标端点的 `scheme://host:port` 安全子集:userinfo 在 reqwest 构建
    /// 请求时已被剥除,查询串在此按构造丢弃。
    pub endpoint: Option<String>,
}

impl VllmSnapshot {
    /// 后端在线判定(live dot / CLI `monitor status` 的 `vllm_online` 共用):
    /// 健康态 `verified` 且进程 Ready/Busy。原先该谓词在 GUI command 与
    /// CLI status_payload 各自手写一遍字面量匹配,单点收口避免字符串漂移。
    pub fn is_online(&self) -> bool {
        self.health_status == HealthStatus::Verified
            && matches!(self.status, VllmStatus::Ready | VllmStatus::Busy)
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
    // The context window the user explicitly declares in the model form must
    // join the monitor / live-dot display scale; otherwise after saving 1M the
    // chat page (chat:usage denominator) and the monitor page / progress-bar
    // denominator each tell their own story.
    let configured_context = model.as_ref().and_then(|m| m.context_window_tokens);
    snapshot_for_model_config(
        &upstream,
        configured_model,
        preset,
        api_key.as_deref(),
        configured_context,
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
///
/// Process-wide shared client: both the monitor page's 1 Hz polling and the
/// chat page's status dot go through here; a fresh Client per call would
/// rebuild TLS/connection pools every second with zero reuse. Two
/// OnceLock caveats (shared with `core::model_endpoint`'s probe pool, whose
/// singleton this delegates to with `None` = no client-level timeout — the
/// 3s probe timeout stays per-request below, keeping the original
/// semantics):
/// 1. reqwest enables system-proxy detection by default; the proxy config
/// is snapshotted at first build and never re-read for the process
/// lifetime — changing the system proxy mid-session needs an app
/// restart to take effect;
/// 2. A build failure (TLS/system config unavailable) is cached
/// process-wide as `None` with no per-call retry, preserving the
/// caller's "probe failed → fall back to configured values" downgrade —
/// Client::default() panics on the same failure and is not a usable
/// fallback. Request-level errors are unaffected and remain per-call,
/// handled by the caller.
fn shared_probe_client() -> Option<&'static reqwest::Client> {
    crate::core::model_endpoint::shared_probe_client_with_timeout(None)
}

async fn snapshot_for_model_config(
    upstream: &str,
    configured_model: Option<String>,
    preset: ModelPreset,
    api_key: Option<&str>,
    configured_context: Option<u32>,
) -> Option<VllmSnapshot> {
    let client = shared_probe_client()?;
    let target_kind = if preset == ModelPreset::LocalVllm {
        "local"
    } else {
        vllm_target_kind(upstream)
    };
    let metrics_applicable = target_kind == "local";

    // 1) /models 健康
    let models_url = models_probe_url(upstream);
    let mut request = client.get(models_url).timeout(Duration::from_secs(3));
    if let Some(key) = api_key.map(str::trim).filter(|key| !key.is_empty()) {
        if is_anthropic_endpoint(upstream) {
            request = crate::core::model_endpoint::apply_anthropic_auth_headers(request, key);
        } else {
            request = request.bearer_auth(key);
        }
    }
    let request =
        crate::core::model_endpoint::with_opencode_session_header(request, upstream, "model-probe");
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
            // auth_failed 即结论本身,无需精化诊断。
            return Some(VllmSnapshot {
                status: VllmStatus::Offline,
                model: configured_model.clone(),
                configured_model,
                target_kind: target_kind.to_string(),
                metrics_applicable,
                health_status: HealthStatus::AuthFailed,
                ..VllmSnapshot::default()
            });
        }
        Some(Ok(r)) => {
            if target_kind == "local" {
                return Some(VllmSnapshot {
                    status: VllmStatus::Offline,
                    model: configured_model.clone(),
                    configured_model,
                    target_kind: target_kind.to_string(),
                    metrics_applicable,
                    health_status: HealthStatus::Offline,
                    diagnostic: Some(MonitorDiagnostic {
                        code: "models_http_error".to_string(),
                        http_status: Some(r.status().as_u16()),
                        transport: None,
                        endpoint: endpoint_root(Some(r.url())),
                    }),
                    ..VllmSnapshot::default()
                });
            }
            None
        }
        Some(Err(err)) => {
            if target_kind == "local" {
                // 结构化 detail 取代 reqwest 的 `Display`:Display 内嵌完整
                // 请求 URL(用户 base_url 可能带 userinfo 或 token 形查询串,
                // 通用脱敏对无数字的查询值会漏),所以诊断只从解析后的字段
                // 构造——`endpoint` 取 `err.url()` 的 scheme/host/port 子集
                // (userinfo 在请求构建时已被剥除,查询串按构造丢弃),错误
                // 文本一律不进快照。
                return Some(VllmSnapshot {
                    status: VllmStatus::Offline,
                    model: configured_model.clone(),
                    configured_model,
                    target_kind: target_kind.to_string(),
                    metrics_applicable,
                    health_status: HealthStatus::Offline,
                    diagnostic: Some(MonitorDiagnostic {
                        code: "models_unreachable".to_string(),
                        http_status: None,
                        transport: Some(transport_class(&err)),
                        endpoint: endpoint_root(err.url()),
                    }),
                    ..VllmSnapshot::default()
                });
            }
            None
        }
        // `None` only happens for remote targets with no API key (the probe
        // is skipped for them); local targets always probe above, so no
        // local arm is needed here.
        None => None,
    };

    let (served_model, mut max_model_len, listed_entries) = match models_resp {
        Some(r) => match r.json::<serde_json::Value>().await.ok() {
            Some(v) => {
                // Roster size for the native follow-up's attribution gate
                // (a configured=None snapshot may only follow a
                // single-model listing — see
                // `local_native_display_window`).
                let listed = v.get("data").and_then(|d| d.as_array()).map(|a| a.len());
                let (served, len) =
                    parse_models_response(v, configured_model.as_deref()).unwrap_or((None, None));
                (served, len, listed)
            }
            None => (None, None, None),
        },
        None => (None, None, None),
    };
    // Ollama's and LM Studio's OpenAI-compatible `/v1/models` never carry a
    // window fact. For a local target whose listed window is missing, follow
    // up on the native API (`local_native_display_window`: kind- and
    // locality-gated, `/api/ps` fresh per poll, `/api/show` 60s-cached in
    // core, LM Studio `/api/v0/models` per poll). This keeps the monitor
    // denominator on the same deployment truth the engine route limits use
    // (`core::model_context::resolve_context_window` single scale) instead
    // of the preset fallback diverging from the engine's probed value.
    if target_kind == "local" && max_model_len.is_none() {
        max_model_len = local_native_display_window(
            upstream,
            api_key,
            configured_model.as_deref(),
            served_model.as_deref(),
            listed_entries,
        )
        .await;
    }

    // 2) /metrics（用 host 根目录，不带 /v1）
    // 鉴权与上方 /v1/models 探测同模式：本地引擎以 `--api-key` 启动时
    // /metrics 同样要求 bearer（Tachyon 驭光即如此），裸 GET 会 401 落进
    // metrics_unavailable 诊断。
    let metrics_url = metrics_applicable
        .then(|| strip_v1_suffix(upstream).map(|h| format!("{h}/metrics")))
        .flatten();
    let metrics_resp = match metrics_url {
        Some(u) => {
            let mut request = client.get(&u).timeout(Duration::from_secs(3));
            if let Some(key) = api_key.map(str::trim).filter(|key| !key.is_empty()) {
                request = request.bearer_auth(key);
            }
            request.send().await.ok()
        }
        None => None,
    };
    let metrics_text = match metrics_resp {
        Some(r) if r.status().is_success() => match r.content_length() {
            Some(n) if n > MAX_METRICS_BODY_BYTES => None,
            _ => r.text().await.ok(),
        },
        _ => None,
    };
    let running = metrics_text
        .as_deref()
        .and_then(|t| parse_prom_metric(t, "vllm:num_requests_running"));
    let waiting = metrics_text
        .as_deref()
        .and_then(|t| parse_prom_metric(t, "vllm:num_requests_waiting"));
    // prefix cache 原始计数器: hits/queries 都是 vLLM Prometheus counter (单调递增,
    // vLLM 进程生命周期内累积)。前端用基准点做减法重算区间命中率。
    let prefix_cache_hits = metrics_text
        .as_deref()
        .and_then(|t| parse_prom_metric(t, "vllm:prefix_cache_hits_total"));
    let prefix_cache_queries = metrics_text
        .as_deref()
        .and_then(|t| parse_prom_metric(t, "vllm:prefix_cache_queries_total"));
    let perf = metrics_text
        .as_deref()
        .map(parse_perf_metrics)
        .unwrap_or_default();

    // An unreachable /metrics and a /metrics that answers 200 with a body no
    // vLLM series parses leave every metric field None alike; flag both so an
    // all-None snapshot is never silently read as "no traffic".
    let mut notices = Vec::new();
    if metrics_applicable
        && running.is_none()
        && waiting.is_none()
        && prefix_cache_hits.is_none()
        && prefix_cache_queries.is_none()
        && perf == PerfMetrics::default()
    {
        notices.push(MonitorDiagnostic {
            code: "metrics_unavailable".to_string(),
            http_status: None,
            transport: None,
            endpoint: None,
        });
    }
    // The display window goes through the unified precedence (see
    // display_context_window): an explicit user declaration wins first, a local
    // deployment's probe value min-clamps; a cloud probe value (usually a list
    // entry from a proxy/gateway, unrelated to or stale for the configured
    // model) must not override the user declaration, otherwise saving 1M gets
    // the progress bar knocked back to 131K. Probe and catalog/preset fallbacks
    // only fill in when no declaration exists.
    let inferred = infer_context_window(
        preset,
        configured_model.as_deref().or(served_model.as_deref()),
    );
    let (max_model_len, window_from_inference) =
        display_context_window(configured_context, target_kind, max_model_len, inferred);
    if window_from_inference {
        notices.push(MonitorDiagnostic {
            code: "context_window_inferred".to_string(),
            http_status: None,
            transport: None,
            endpoint: None,
        });
    }

    let mut status = match (running, waiting) {
        (Some(r), _) if r > 0.0 => VllmStatus::Busy,
        (_, Some(w)) if w > 0.0 => VllmStatus::Busy,
        _ => VllmStatus::Ready,
    };
    // 如果用户配置了模型名，但和 vLLM 实际返回的不一致，降级为 Mismatch。
    // 这样监控台不会显示绿色 READY，聊天 live dot 也会变红。
    if metrics_applicable {
        status = mismatch_if_served_differs(
            status,
            configured_model.as_deref(),
            served_model.as_deref(),
        );
    }
    let health_status = match status {
        VllmStatus::Mismatch => HealthStatus::Mismatch,
        // No `Offline` arm: `status` is only ever Ready/Busy here (built from
        // the queue gauges, then possibly downgraded to Mismatch); every
        // Offline snapshot exits through the early returns above, which carry
        // the offline refinement diagnostic themselves.
        _ if target_kind == "remote"
            && api_key
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .is_none() =>
        {
            HealthStatus::MissingApiKey
        }
        _ if target_kind == "remote" && served_model.is_none() => HealthStatus::Unverified,
        _ => HealthStatus::Verified,
    };

    // happy-path:全字段逐项构造(不用 `..default()`),结构体加字段时这里
    // 与三个早退构造一起编译失败,强制有意识地补齐。
    Some(VllmSnapshot {
        status,
        model: if target_kind == "remote" {
            configured_model.clone().or(served_model)
        } else {
            served_model
        },
        configured_model,
        target_kind: target_kind.to_string(),
        metrics_applicable,
        health_status,
        diagnostic: None,
        notices,
        max_model_len,
        num_requests_running: running,
        num_requests_waiting: waiting,
        prefix_cache_hits,
        prefix_cache_queries,
        ttft_sum_s: perf.ttft_sum_s,
        ttft_count: perf.ttft_count,
    })
}

/// `scheme://host:port` 子集——URL 中可安全进入快照的部分:reqwest 在构建
/// 请求时已剥除 userinfo,查询串在此按构造丢弃,所以 base_url 携带凭据
/// (userinfo 或 token 形查询串)都不可能泄漏进诊断。
fn endpoint_root(url: Option<&reqwest::Url>) -> Option<String> {
    let url = url?;
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    })
}

/// 传输错误分类,取自 reqwest 的结构化 flag(而非 `Display` 文本——真实
/// reqwest 错误的 Display 不含 source 链,`model_probe_transport_code` 的
/// 字符串臂对它们是死代码)。词表与该 helper 保持一致。
fn transport_class(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        "timeout"
    } else if err.is_dns() {
        // Must precede `is_connect()`: hyper-util files resolver failures
        // under its Connect kind too, so the DNS check only classifies if it
        // runs first.
        "dns_failed"
    } else if err.is_connect() {
        "connection_refused"
    } else {
        "network_error"
    }
}

fn parse_models_response(
    v: serde_json::Value,
    configured: Option<&str>,
) -> Option<(Option<String>, Option<u32>)> {
    let entries = crate::core::model_endpoint::parse_models_response_list(v)?;
    // Cloud /models often lists every model at once: prefer the configured
    // model's own entry (exact hit, ASCII case-insensitive fallback; the
    // configured name is trimmed first, matching the double-sided trim of the
    // Mismatch check). The first-entry fallback only serves the served-name
    // display — its max_model_len belongs to another model and must not be lent
    // to the configured model (the same "a window is never borrowed from another
    // model" principle as `resolve_served_model_from_entries`). The two paths
    // falling back differently is intentional: the inference path keeps the
    // configured name on a miss so model_not_found surfaces explicitly; the
    // display path falling back to the first entry's name is diagnostic info
    // only.
    let configured = configured.map(str::trim).filter(|name| !name.is_empty());
    let matched = configured.and_then(|name| {
        entries
            .iter()
            .find(|entry| entry.id == name)
            .or_else(|| {
                entries
                    .iter()
                    .find(|entry| entry.id.eq_ignore_ascii_case(name))
            })
            // Ollama lists the canonical `name:latest`; a hand-typed
            // tagless configured name must still find its own entry (the
            // same fold as `ollama_ps_context_lookup`) — without it served
            // degrades to the first entry and the attribution gate below
            // refuses the native follow-up in every roster shape.
            .or_else(|| {
                let canonical = crate::core::model_endpoint::ollama_canonical_name(name);
                entries.iter().find(|entry| entry.id == canonical)
            })
    });
    let entry = matched.or_else(|| entries.first())?;
    let window = if matched.is_some() || configured.is_none() {
        entry.max_model_len
    } else {
        None
    };
    Some((Some(entry.id.clone()), window))
}

/// Window-attribution gate for the native display follow-up: the queried
/// entry must belong to the configured model. With a configured name the
/// served entry must be that model (trimmed, ASCII case-insensitive — the
/// same fold as `parse_models_response`; the lookups use `served`, the
/// server-canonical id, so this adopts the model's own window, never
/// another entry's); without one (local_vllm snapshots carry no configured
/// name) the served entry is the display subject — see
/// `local_native_display_window` for the single-model roster restriction
/// on that arm. A partial roster (configured name absent from the list)
/// must not borrow the first entry's window — the same "a window is never
/// borrowed from another model" principle as `parse_models_response` /
/// `resolve_served_model_from_entries`. Note the engine route's adoption
/// gate (`adopts_probed_facts`) matches exactly instead of
/// case-insensitively: a case-mismatched route is already a Mismatch
/// display state, and there the display may adopt a window the engine
/// route keeps its fallback for — accepted, since loosening the engine
/// gate would change pre-existing vLLM adoption semantics. Both gates
/// fold Ollama's `name:latest` canonicalization ([`ollama_canonical_name`]
/// — a hand-typed tagless configured name must resolve to its own entry
/// on both sides), so the remaining asymmetry is the ASCII-case fold: on a
/// list whose configured name matches only case-insensitively this gate
/// passes while the engine's exact-plus-canonical gate never adopts that
/// shape, and the keyed native maps (queried by the server-canonical
/// served id) can hand back a ps fact the engine route does not adopt.
fn native_display_window_adoptable(configured: Option<&str>, served: Option<&str>) -> bool {
    let Some(served) = served else {
        return false;
    };
    match configured.map(str::trim).filter(|name| !name.is_empty()) {
        Some(configured) => {
            served.eq_ignore_ascii_case(configured)
                || served.eq_ignore_ascii_case(&crate::core::model_endpoint::ollama_canonical_name(
                    configured,
                ))
        }
        None => true,
    }
}

/// Native window fact for a local target whose `/v1/models` listing carried
/// none. Gates, in order:
/// 1. attribution (`native_display_window_adoptable`) — a window is never
///    borrowed across model names;
/// 2. locality — the same `base_url_uses_local_or_private` classification
///    the engine route uses, so a public base_url saved under a local
///    preset gets no native probes on either path;
/// 3. roster — a configured=None snapshot attributes the listing to its
///    first entry, so only an unambiguous single-model roster may follow it
///    (a multi-model Ollama behind a local_vllm preset would otherwise
///    display the first-downloaded model's window);
/// 4. kind — an already-cached positive classification decides the native
///    API: Ollama → `/api/ps` effective context (fresh; the deployment
///    truth for loaded models) → `/api/show` (`num_ctx` declaration, 60s
///    cached in core and shared with the engine path; a show response
///    without one yields no fact — the GGUF trained cap is a capability
///    ceiling, not the served window), LM Studio → `/api/v0/models` served
///    window. An uncached kind (never probed — e.g. a monitor-only target
///    the engine never classified) still tries the Ollama-shaped `/api/ps`
///    probe, and a non-Ollama shape ends the lookup; a known other kind
///    skips the fetch entirely — no per-poll doomed `/api/ps` for servers
///    the engine already identified. (vLLM listings carry `max_model_len`
///    so there is nothing to follow up; llama.cpp-class listings usually
///    don't — the engine also skips native for those kinds, so the display
///    here shows the same preset fallback the engine's budget uses instead
///    of a diverging native fact.) That skip is TTL-scoped like every
///    cached kind: 60s after the last engine-side classification the
///    monitor resumes the probe; a monitor-only target the engine never
///    classified keeps paying one small doomed 404 per poll for as long
///    as it stays unclassified (the monitor deliberately only reads the
///    cache), bounded by the shared 3s probe client. One honest
///    exception: while `/api/show` itself
///    persistently errors (5xx / hang), those responses are `Unreachable`
///    and deliberately uncached — a busy server must not be pinned to "no
///    window" — so the lookup re-POSTs show once per poll until the server
///    recovers, and each POST is a server-side GGUF re-read. Bounded at 1
///    small request/s with the shared 3s timeout; the frontend in-flight
///    guard keeps slow polls from piling up.
async fn local_native_display_window(
    upstream: &str,
    api_key: Option<&str>,
    configured: Option<&str>,
    served: Option<&str>,
    listed_entries: Option<usize>,
) -> Option<u32> {
    use crate::core::model_endpoint::LocalServerKind;
    if !native_display_window_adoptable(configured, served) {
        return None;
    }
    if !crate::core::model_endpoint::base_url_uses_local_or_private(upstream) {
        return None;
    }
    if configured
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .is_none()
        && listed_entries != Some(1)
    {
        return None;
    }
    let name = served?;
    match crate::core::model_endpoint::cached_local_server_kind(upstream) {
        Some(LocalServerKind::LmStudio) => {
            crate::core::model_endpoint::fetch_lmstudio_served_context(upstream, api_key, name)
                .await
        }
        // Ollama, or never classified (monitor-only target): try the
        // Ollama-shaped ps probe. The lookup is the same tag-tolerant one
        // the engine route uses, and the attribution gate above folds the
        // same canonical form, so a tagless configured name reaches its
        // own canonical `/api/ps` entry exactly as the engine budget does;
        // unlike `fetch_ollama_model_context`, a non-Ollama-shaped ps
        // response ends the lookup here (a never-classified target must not
        // pay the show fallback POST).
        None | Some(LocalServerKind::Ollama) => {
            let contexts =
                crate::core::model_endpoint::fetch_ollama_contexts(upstream, api_key).await?;
            if let Some(ctx) =
                crate::core::model_endpoint::ollama_ps_context_lookup(&contexts, name)
            {
                return Some(ctx);
            }
            crate::core::model_endpoint::cached_ollama_show_context(upstream, api_key, name).await
        }
        // A known other kind: the engine's own adoption also skips native
        // for it, so the display must not dial a doomed per-poll request.
        // vLLM listings carry `max_model_len` (nothing to follow up);
        // llama.cpp-class listings usually don't, and the display then
        // shows the same preset fallback the engine's budget uses.
        Some(_) => None,
    }
}

/// Display-side context window (the monitor card + the progress-bar denominator
/// of `get_backend_status`). The precedence itself lives in
/// `core::model_context::resolve_context_window`, shared with the host
/// `bridge::route_limits_for_model` (which decides inference and compaction
/// thresholds); this wrapper only owns the "is the probe trustworthy" gate: a
/// local deployment's (loopback/private IP, locally introspectable) probe value
/// is handed to the unified scale for min-clamping; a cloud probe value comes
/// from gateway/proxy list entries and is not deployment ground truth, so when
/// the user already declared a window it does not participate at all (the
/// declaration wins and there is no clamping basis) and only fills the display
/// when no declaration exists.
fn display_context_window(
    configured: Option<u32>,
    target_kind: &str,
    probed: Option<u32>,
    inferred: Option<u32>,
) -> (Option<u32>, bool) {
    let probed = match (configured, target_kind) {
        (Some(_), "local") | (None, _) => probed,
        (Some(_), _) => None,
    };
    crate::core::model_context::resolve_context_window(configured, probed, inferred)
}

/// Downgrade to `Mismatch` when the configured model name differs from the
/// actual served name (only local-deployment callers enable it, see
/// `snapshot_for_model_config`). Extracted as a pure function so unit tests can
/// pin the comparison semantics: exact comparison after trimming both sides
/// (case-sensitive, the same scale as the inference path
/// `resolve_served_model_from_entries`' exact match — when a gateway normalizes
/// ids to lowercase while the configured name has uppercase, the Engine sending
/// the configured original name gets model_not_found, so the monitor red dot is
/// a real signal, not a false positive).
fn mismatch_if_served_differs(
    status: VllmStatus,
    configured: Option<&str>,
    served: Option<&str>,
) -> VllmStatus {
    match (configured, served) {
        (Some(cfg), Some(actual)) if cfg.trim() != actual.trim() => VllmStatus::Mismatch,
        _ => status,
    }
}

fn infer_context_window(preset: ModelPreset, model: Option<&str>) -> Option<u32> {
    // Model-name facts resolve through the core::model_context single entry, the
    // same source as the host route_limits inference fallback, so the page never
    // displays 1M while compaction still runs on 128K. When neither the base nor
    // the supplemental table recognizes the name, fall back to the vendor preset
    // (table in prefs::model_preset).
    if let Some(window) = model.and_then(crate::core::model_context::resolved_context_window) {
        return Some(window);
    }
    preset.context_window_fallback(model)
}

/// 推理性能相关的累计指标（TTFT 直方图），统一解析、统一缺省 None。
#[derive(Debug, Default, PartialEq)]
struct PerfMetrics {
    ttft_sum_s: Option<f64>,
    ttft_count: Option<f64>,
}

fn parse_perf_metrics(text: &str) -> PerfMetrics {
    PerfMetrics {
        ttft_sum_s: parse_prom_metric(text, "vllm:time_to_first_token_seconds_sum"),
        ttft_count: parse_prom_metric(text, "vllm:time_to_first_token_seconds_count"),
    }
}

/// Sum a metric across every one of its series in the Prometheus text, e.g.
/// `vllm:num_requests_running{engine="0"} 3` plus `{engine="1"} 2` → `5.0`.
/// vLLM with `data_parallel_size > 1` exports one series per `engine` label,
/// and every metric consumed here is additive (queue gauges, monotonic
/// counters, histogram `_sum`/`_count`), so the engine-wide total is the
/// honest published number — returning the first series would silently
/// report engine 0 only. `None` when no finite series exists.
///
/// Per-line robustness: comment lines are skipped; the metric name must end
/// at a boundary (space, tab, `{`, or end of line), so same-prefix longer
/// names (`_created` siblings, `vllm:num_requests_waiting_by_reason`) never
/// match; a truncated labels block or a non-numeric sample skips only that
/// line; `NaN` / `±Inf` samples (which `f64::from_str` accepts) are skipped
/// rather than summed.
fn parse_prom_metric(text: &str, name: &str) -> Option<f64> {
    let mut total: Option<f64> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || !line.starts_with(name) {
            continue;
        }
        let after_name = &line[name.len()..];
        match after_name.chars().next() {
            None | Some('{') | Some(' ') | Some('\t') => {}
            _ => continue,
        }
        let value_part = if let Some(rest) = after_name.strip_prefix('{') {
            match closing_label_brace(rest) {
                Some(close) => rest[close + 1..].trim(),
                // Truncated labels block: this line only.
                None => continue,
            }
        } else {
            after_name.trim()
        };
        let Some(token) = value_part.split_whitespace().next() else {
            continue;
        };
        if let Ok(v) = token.parse::<f64>() {
            if v.is_finite() {
                *total.get_or_insert(0.0) += v;
            }
        }
    }
    total
}

/// Offset of the labels-block closing `}` in `rest` (the text after the
/// opening `{`), honoring quoted label values — a `}` inside quotes is part
/// of the value (vLLM's `--served-model-name` is arbitrary text), not the
/// block end. `None` means the block never closes.
fn closing_label_brace(rest: &str) -> Option<usize> {
    let mut in_quotes = false;
    let mut escaped = false;
    for (i, ch) in rest.char_indices() {
        if escaped {
            escaped = false;
        } else {
            match ch {
                '\\' if in_quotes => escaped = true,
                '"' => in_quotes = !in_quotes,
                '}' if !in_quotes => return Some(i),
                _ => {}
            }
        }
    }
    None
}

/// Classify the backend kind by the base_url host: loopback/private IP
/// ranges (including docker host aliases) = local inference engine
/// (`local`, self-hosted vLLM with Prometheus metrics); public domain/IP =
/// cloud API (`remote`); empty/unparseable = invalid config (`invalid`).
/// Drives the monitor card's local/remote/invalid label, metric
/// applicability, and the native-window alert. The local/private decision
/// shares core's `base_url_uses_local_or_private` with engine routing (one
/// source of truth; the `local_native_display_window` gate uses it too) —
/// no more separate string parsing: `host.docker.internal`-style aliases
/// and trailing-dot local hosts classify as local instead of cloud.
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
    if crate::core::model_endpoint::base_url_uses_local_or_private(s) {
        return "local";
    }
    // 域名或公网 IPv6 → 云端 API
    "remote"
}

/// Final link of the "probe → user picks → use the pick" chain: decides the
/// model name a local OpenAI-compatible engine actually sends. LM Studio /
/// Ollama return **every downloaded model** in `/v1/models` (a multi-entry
/// list whose first entry is unrelated to the user's pick in Pinvou), so:
/// - configured name present in the list → keep it verbatim (an unloaded
///   model is JIT-loaded by the engine, which is the user's explicit choice;
///   overwriting the configured name with the first entry is exactly the
///   reported "conversation names A, engine loads B" break);
/// - configured name absent and the server exposes exactly one model →
///   follow that name (the pre-existing fix for real vLLM renamed via
///   `--served-model-name`, preserved unchanged);
/// - otherwise (probe failure / multi-entry list without the configured
///   name) → keep the configured name so inference surfaces `model_not_found`
///   explicitly, never silently switching to any other listed model.
/// Returns `(model name to send, that model's context window, that model's
/// self-reported output limit)`; when the configured name is kept the window
/// and output limit are `None` (the model is not in the list, so another
/// model's facts must not drive compaction thresholds or output caps).
fn resolve_served_model_from_entries(
    configured: &str,
    entries: &[crate::core::model_endpoint::OpenAiModelInfo],
) -> (String, Option<u32>, Option<u32>) {
    if let Some(matched) = entries.iter().find(|model| model.id == configured) {
        return (
            configured.to_string(),
            matched.max_model_len,
            matched.max_output_tokens,
        );
    }
    if let [single] = entries {
        return (
            single.id.clone(),
            single.max_model_len,
            single.max_output_tokens,
        );
    }
    (configured.to_string(), None, None)
}

/// Fetches `/v1/models` and decides the actual model name via
/// `resolve_served_model_from_entries` (private helper). On probe failure
/// returns the configured name with `None` window/limit. Pass `bearer` from
/// the same origin as real inference: authenticated engines (`--api-key`)
/// 401 on `/v1/models` without credentials.
pub async fn resolve_served_model(
    base_url: &str,
    bearer: Option<&str>,
    configured: &str,
) -> (String, Option<u32>, Option<u32>) {
    match crate::core::model_endpoint::fetch_v1_models(base_url, bearer)
        .await
        .and_then(crate::core::model_endpoint::parse_models_response_list)
    {
        Some(entries) => resolve_served_model_from_entries(configured, &entries),
        None => (configured.to_string(), None, None),
    }
}

/// Whether a probed entry's facts (context window / self-reported output
/// limit) may be adopted by this route. The facts must belong to the model
/// name actually sent to the endpoint: routes that follow the served name
/// (vLLM, whose name is usually corrected to the entry itself) may always
/// adopt; routes that do not rename adopt only when the configured name
/// exactly hits the list, or its Ollama canonical form does
/// (`ollama_canonical_name`: a hand-typed tagless name on a
/// single-entry Ollama roster *is* the listed model under Ollama's
/// `name:latest` canonicalization, not a borrowed one — without the fold
/// the whole native adoption is skipped there and the 8192-fallback
/// collapse survives the engine's lifetime; the fold matches only
/// `served == configured + ":latest"`, so a genuinely unrelated single
/// entry still refuses). The ps lookup this gate feeds folds the same way
/// (`ollama_ps_context_lookup`), so an adopted gate always resolves a fact.
/// In the remaining single-entry "borrowed name" scenario the returned
/// served name is unrelated to the configured one and its facts belong to
/// another model, so they must not tighten this route's window/output
/// caps.
///
/// Known exception (intentional trade-off): with vLLM +
/// `pins_scheduled_model` the served-name correction is suppressed and the
/// configured name goes live verbatim, but `follows_served_name` is still
/// true by route type — the single-entry facts are then adopted. A lenient
/// single-model server is genuinely serving that entry (facts correct); a
/// strict one 404s on the configured name (facts have no effect), so no
/// extra condition complexity is added for that corner.
pub fn adopts_probed_facts(follows_served_name: bool, configured: &str, served: &str) -> bool {
    follows_served_name
        || served == configured
        || served == crate::core::model_endpoint::ollama_canonical_name(configured)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::paths::tests::ENV_LOCK;
    use crate::platform::prefs::ModelPreset;

    #[test]
    fn vllm_target_kind_classifies_by_host() {
        // 本地推理引擎:环回 + 私有 IP 段
        assert_eq!(vllm_target_kind("http://10.0.0.113:8000/v1"), "local");
        assert_eq!(vllm_target_kind("http://127.0.0.1:8000/v1"), "local");
        assert_eq!(vllm_target_kind("http://localhost:8000/v1"), "local");
        assert_eq!(vllm_target_kind("http://192.168.1.5:8000/v1"), "local");
        assert_eq!(vllm_target_kind("http://172.16.0.9:8000/v1"), "local");
        // docker host alias (host.docker.internal): shares the core
        // classifier with engine routing, so it classifies local like the
        // engine does
        assert_eq!(
            vllm_target_kind("http://host.docker.internal:8000/v1"),
            "local"
        );
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
        let (id, max) = parse_models_response(json, None).unwrap();
        assert_eq!(id.as_deref(), Some("/model"));
        assert_eq!(max, Some(65536));
    }

    fn models_list_json(entries: &[(&str, Option<u32>)]) -> serde_json::Value {
        let data: Vec<String> = entries
            .iter()
            .map(|(id, len)| match len {
                Some(len) => format!(r#"{{"id":"{id}","max_model_len":{len}}}"#),
                None => format!(r#"{{"id":"{id}"}}"#),
            })
            .collect();
        serde_json::from_str(&format!(
            r#"{{"object":"list","data":[{}]}}"#,
            data.join(",")
        ))
        .unwrap()
    }

    /// Cloud /models often lists every model at once: max_model_len must come
    /// from the configured model's own entry, never borrowed from the first
    /// entry (often another model, or a gateway default of 131072).
    #[test]
    fn parse_models_response_matches_configured_model_entry() {
        let json = models_list_json(&[
            ("glm-5.2", Some(1000_000)),
            ("glm-5.3-flash", Some(131_072)),
        ]);
        let (id, max) = parse_models_response(json, Some("glm-5.3-flash")).unwrap();
        assert_eq!(id.as_deref(), Some("glm-5.3-flash"));
        assert_eq!(max, Some(131_072));
    }

    /// Configured name not in the list (gateway returns a partial roster): the
    /// first-entry fallback only serves the served-name display; its
    /// max_model_len belongs to another model and must not be lent to the
    /// configured model (the inference path resolve_served_model_from_entries
    /// follows the same "window never borrowed" principle).
    #[test]
    fn parse_models_response_first_entry_fallback_lends_name_not_window() {
        let json = models_list_json(&[("a", Some(4096)), ("b", Some(8192))]);
        let (id, max) = parse_models_response(json, Some("gone")).unwrap();
        assert_eq!(id.as_deref(), Some("a"));
        assert_eq!(max, None);
    }

    /// When the case-insensitive fallback hits, the window likewise belongs to
    /// the configured model (gateways normalizing ids to lowercase are a common
    /// shape, and the window does belong to the same model).
    #[test]
    fn parse_models_response_matches_case_insensitively_and_keeps_window() {
        let json = models_list_json(&[("other", Some(4096)), ("glm-5.3-flash", Some(131_072))]);
        let (id, max) = parse_models_response(json, Some("GLM-5.3-Flash")).unwrap();
        assert_eq!(id.as_deref(), Some("glm-5.3-flash"));
        assert_eq!(max, Some(131_072));
    }

    /// The configured name is trimmed before matching (the same scale as the
    /// double-sided trim of the Mismatch check), so a name with leading or
    /// trailing whitespace can finally match.
    #[test]
    fn parse_models_response_trims_configured_name() {
        let json = models_list_json(&[("glm-5.3-flash", Some(131_072))]);
        let (id, max) = parse_models_response(json, Some("  glm-5.3-flash\t")).unwrap();
        assert_eq!(id.as_deref(), Some("glm-5.3-flash"));
        assert_eq!(max, Some(131_072));
    }

    /// Matched entry but the server carries no max_model_len: the window stays
    /// None and is left to the declaration/inference fallbacks — never
    /// fabricated (the same principle as resolve_served_model_from_entries).
    #[test]
    fn parse_models_response_matched_entry_without_window_propagates_none() {
        let json = models_list_json(&[("user-picked", None)]);
        let (id, max) = parse_models_response(json, Some("user-picked")).unwrap();
        assert_eq!(id.as_deref(), Some("user-picked"));
        assert_eq!(max, None);
    }

    /// Ollama display follow-up attribution gate: the queried entry must be
    /// the configured model's own (trimmed, ASCII case-insensitive — the
    /// same scale as parse_models_response); with no configured name the
    /// served entry is the display subject; a partial roster never borrows
    /// the first entry's window; no served name → nothing to query.
    #[test]
    fn native_display_window_gate_never_borrows_across_names() {
        assert!(native_display_window_adoptable(
            Some("ollama-model"),
            Some("ollama-model")
        ));
        assert!(native_display_window_adoptable(
            Some("Ollama-Model"),
            Some("ollama-model")
        ));
        assert!(native_display_window_adoptable(
            Some("  ollama-model\t"),
            Some("ollama-model")
        ));
        assert!(native_display_window_adoptable(None, Some("served")));
        assert!(
            !native_display_window_adoptable(Some("configured"), Some("first-entry")),
            "a partial roster must not borrow the first entry's window"
        );
        assert!(!native_display_window_adoptable(Some("m"), None));
        assert!(!native_display_window_adoptable(None, None));
    }

    /// /metrics 拉取携带与 /v1/models 同源 bearer（round-1 评审 minor 补钉）：以
    /// `--api-key` 启动的本地引擎（如驭光 Tachyon）对裸 GET 会 401，修复前落
    /// metrics_unavailable 误诊。mock 逐路径记录 Authorization，两个方向都钉：
    /// 有 key 时两路径同带 `Bearer secret`；keyless 时不带任何 Authorization。
    #[tokio::test]
    async fn metrics_fetch_carries_the_same_origin_bearer() {
        use crate::core::model_endpoint::models_mock;
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let metrics_body = "vllm:num_requests_running 1\nvllm:num_requests_waiting 0\n".to_string();
        // The snapshot's health request goes through `models_probe_url`,
        // which appends `/models` directly on a bare-host base (no `/v1`).
        let mock = models_mock::spawn(&[
            (
                "/models",
                200,
                r#"{"object":"list","data":[{"id":"qwen3.8","max_model_len":131072}]}"#.into(),
            ),
            ("/metrics", 200, metrics_body.clone()),
        ]);
        let snap = snapshot_for_model_config(
            &mock.base_url,
            Some("qwen3.8".to_string()),
            ModelPreset::LocalVllm,
            Some("secret"),
            None,
        )
        .await
        .expect("snapshot with key");
        assert_eq!(
            mock.auth_for("/models").as_deref(),
            Some("Bearer secret"),
            "the health probe keeps the existing bearer semantics"
        );
        assert_eq!(
            mock.auth_for("/metrics").as_deref(),
            Some("Bearer secret"),
            "/metrics must carry the same-origin bearer (--api-key servers 401 a bare GET)"
        );
        assert_eq!(
            snap.status,
            VllmStatus::Busy,
            "metrics body parses once authorized (running=1 surfaces as Busy)"
        );

        let mock = models_mock::spawn(&[
            (
                "/models",
                200,
                r#"{"object":"list","data":[{"id":"qwen3.8","max_model_len":131072}]}"#.into(),
            ),
            ("/metrics", 200, metrics_body),
        ]);
        let snap = snapshot_for_model_config(
            &mock.base_url,
            Some("qwen3.8".to_string()),
            ModelPreset::LocalVllm,
            None,
            None,
        )
        .await
        .expect("snapshot keyless");
        assert_eq!(
            mock.auth_for("/metrics"),
            None,
            "keyless servers must not receive an Authorization header"
        );
        assert_eq!(snap.status, VllmStatus::Busy);
    }

    /// The happy-path snapshot surfaces the parsed queue gauges under the
    /// verified health with no diagnostic and no notice. Red arms: swapping
    /// the two queue assignments, hardwiring either gauge to `None`, or
    /// inventing a diagnostic/notice on the verified path must all go red
    /// here.
    #[tokio::test]
    async fn snapshot_happy_path_surfaces_queue_gauges_without_diagnostics() {
        use crate::core::model_endpoint::models_mock;
        let _lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mock = models_mock::spawn(&[
            (
                "/models",
                200,
                r#"{"data":[{"id":"happy-model","max_model_len":131072}]}"#.into(),
            ),
            (
                "/metrics",
                200,
                "vllm:num_requests_running 2\n\
                 vllm:num_requests_waiting 1\n\
                 vllm:prefix_cache_hits_total 30\n\
                 vllm:prefix_cache_queries_total 40\n"
                    .into(),
            ),
        ]);
        let snap = snapshot_for_model_config(
            &mock.base_url,
            Some("happy-model".to_string()),
            ModelPreset::LocalVllm,
            None,
            None,
        )
        .await
        .expect("a local target yields a snapshot");
        assert_eq!(snap.status, VllmStatus::Busy, "running=2 is Busy");
        assert_eq!(snap.num_requests_running, Some(2.0));
        assert_eq!(snap.num_requests_waiting, Some(1.0));
        assert_eq!(snap.health_status, HealthStatus::Verified);
        assert!(snap.diagnostic.is_none(), "verified carries no diagnostic");
        assert!(
            snap.notices.is_empty(),
            "a listed window over a served /metrics must carry no notices: {:?}",
            snap.notices
        );
    }

    /// The snapshot wiring sums multi-engine `/metrics` series: a
    /// `data_parallel_size > 1` export must publish the engine-wide queue
    /// depth and cache counters, not engine 0's series — and the Busy state
    /// follows the fleet total.
    #[tokio::test]
    async fn snapshot_sums_multi_engine_metrics() {
        use crate::core::model_endpoint::models_mock;
        let _lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mock = models_mock::spawn(&[
            ("/models", 200, r#"{"data":[{"id":"dp-model"}]}"#.into()),
            (
                "/metrics",
                200,
                "vllm:num_requests_running{engine=\"0\"} 0\n\
                 vllm:num_requests_running{engine=\"1\"} 2\n\
                 vllm:num_requests_waiting{engine=\"0\"} 1\n\
                 vllm:num_requests_waiting{engine=\"1\"} 1\n\
                 vllm:prefix_cache_hits_total{engine=\"0\"} 10\n\
                 vllm:prefix_cache_hits_total{engine=\"1\"} 20\n\
                 vllm:prefix_cache_queries_total{engine=\"0\"} 40\n\
                 vllm:prefix_cache_queries_total{engine=\"1\"} 60\n"
                    .into(),
            ),
        ]);
        let snap = snapshot_for_model_config(
            &mock.base_url,
            Some("dp-model".to_string()),
            ModelPreset::LocalVllm,
            None,
            None,
        )
        .await
        .expect("snapshot");
        assert_eq!(
            snap.status,
            VllmStatus::Busy,
            "engine 0 idle + engine 1 busy is Busy for the fleet"
        );
        assert_eq!(snap.num_requests_running, Some(2.0));
        assert_eq!(snap.num_requests_waiting, Some(2.0));
        assert_eq!(snap.prefix_cache_hits, Some(30.0));
        assert_eq!(snap.prefix_cache_queries, Some(100.0));
    }

    /// A /metrics that answers HTTP 200 with a body no vLLM series parses
    /// must flag itself: the resulting all-None snapshot must not be silently
    /// readable as "no traffic". (Transport failure and non-200 are pinned by
    /// the per-branch test below.)
    #[tokio::test]
    async fn snapshot_flags_a_metrics_body_with_no_parseable_series() {
        use crate::core::model_endpoint::models_mock;
        let _lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mock = models_mock::spawn(&[
            ("/models", 200, r#"{"data":[{"id":"m"}]}"#.into()),
            ("/metrics", 200, "<html>not prometheus</html>".into()),
        ]);
        let snap = snapshot_for_model_config(
            &mock.base_url,
            Some("m".to_string()),
            ModelPreset::LocalVllm,
            None,
            None,
        )
        .await
        .expect("snapshot");
        assert_eq!(snap.status, VllmStatus::Ready);
        assert!(snap.num_requests_running.is_none());
        let codes: Vec<_> = snap.notices.iter().map(|d| d.code.as_str()).collect();
        assert!(
            codes.contains(&"metrics_unavailable"),
            "an HTTP 200 body with no parseable series must flag itself: {codes:?}"
        );
    }

    /// A hostname that cannot resolve classifies as `dns_failed`, not
    /// `connection_refused`: hyper-util wraps resolver failures in its
    /// Connect kind, so `is_connect()` alone mislabels them (round-2
    /// adjudication finding). `.invalid` is reserved by RFC 6761 to never
    /// resolve.
    #[tokio::test]
    async fn snapshot_classifies_dns_failures_as_dns_failed() {
        let _lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let snap = snapshot_for_model_config(
            "http://pinvou-monitor-probe.invalid:1",
            Some("m".to_string()),
            ModelPreset::LocalVllm,
            None,
            None,
        )
        .await
        .expect("a local-preset target yields a snapshot even when unresolvable");
        let diagnostic = snap.diagnostic.expect("offline carries a diagnostic");
        assert_eq!(diagnostic.code, "models_unreachable");
        assert_eq!(diagnostic.transport, Some("dns_failed"));
        assert_eq!(
            diagnostic.endpoint.as_deref(),
            Some("http://pinvou-monitor-probe.invalid:1")
        );
    }

    /// Every branch carries the right structured diagnostic. The offline
    /// refinement is built from parsed fields, never from reqwest's
    /// `Display` (which embeds the full request URL): a token-shaped query
    /// on the base_url must stay out of the snapshot entirely, so replacing
    /// the endpoint construction with the raw error Display must go red
    /// here. The other health verdicts (`auth_failed` / `missing_api_key` /
    /// `unverified` / `mismatch`) are self-describing and carry
    /// `diagnostic: None` — their producer strings are enum variants now,
    /// so a typo is a compile error rather than a silent drift (round-2
    /// audit: the stringly-typed producer could drop the diagnostic with
    /// zero red tests).
    #[tokio::test]
    async fn snapshot_carries_structured_diagnostics_per_health_branch() {
        use crate::core::model_endpoint::models_mock;
        let _lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        // Transport error → models_unreachable with the transport class and
        // the scheme/host/port endpoint; the query never leaks.
        let refused = snapshot_for_model_config(
            "http://127.0.0.1:1?token=secret5",
            Some("m".to_string()),
            ModelPreset::LocalVllm,
            None,
            None,
        )
        .await
        .expect("a local target yields a snapshot even when unreachable");
        assert_eq!(refused.status, VllmStatus::Offline);
        assert_eq!(refused.health_status, HealthStatus::Offline);
        let unreachable = refused.diagnostic.expect("offline carries a diagnostic");
        assert_eq!(unreachable.code, "models_unreachable");
        assert_eq!(
            unreachable.transport,
            Some("connection_refused"),
            "a refused loopback connect classifies as connection_refused"
        );
        assert!(
            unreachable.http_status.is_none(),
            "a transport failure carries no HTTP status"
        );
        assert_eq!(
            unreachable.endpoint.as_deref(),
            Some("http://127.0.0.1:1"),
            "the endpoint keeps scheme/host/port for diagnosis"
        );
        let rendered = format!("{unreachable:?}");
        assert!(
            !rendered.contains("secret5") && !rendered.contains("token="),
            "the diagnostic must not echo the request URL or its query: {rendered}"
        );

        // Non-success HTTP (not 401/403) → models_http_error with the status.
        let mock = models_mock::spawn(&[("/models", 500, "boom".into())]);
        let http_err = snapshot_for_model_config(
            &mock.base_url,
            Some("m".to_string()),
            ModelPreset::LocalVllm,
            None,
            None,
        )
        .await
        .expect("snapshot");
        assert_eq!(http_err.health_status, HealthStatus::Offline);
        let http_diag = http_err
            .diagnostic
            .expect("http error carries a diagnostic");
        assert_eq!(http_diag.code, "models_http_error");
        assert_eq!(http_diag.http_status, Some(500));
        assert!(http_diag.transport.is_none());

        // 401 with a key → auth_failed; the verdict is the answer, so no
        // refinement diagnostic rides along.
        let mock = models_mock::spawn(&[("/models", 401, "{}".into())]);
        let auth = snapshot_for_model_config(
            &mock.base_url,
            Some("m".to_string()),
            ModelPreset::LocalVllm,
            Some("secret"),
            None,
        )
        .await
        .expect("snapshot");
        assert_eq!(auth.health_status, HealthStatus::AuthFailed);
        assert!(
            auth.diagnostic.is_none(),
            "auth_failed is self-describing and carries no refinement: {:?}",
            auth.diagnostic
        );

        // /metrics missing → metrics_unavailable (a local target classifies
        // any loopback base, so this fires for a Deepseek-preset model too).
        let mock = models_mock::spawn(&[(
            "/models",
            200,
            r#"{"data":[{"id":"windowless-model","max_model_len":8192}]}"#.into(),
        )]);
        let no_metrics = snapshot_for_model_config(
            &mock.base_url,
            Some("windowless-model".to_string()),
            ModelPreset::LocalVllm,
            None,
            None,
        )
        .await
        .expect("snapshot");
        assert_eq!(
            no_metrics
                .notices
                .iter()
                .map(|d| d.code.as_str())
                .collect::<Vec<_>>(),
            ["metrics_unavailable"]
        );

        // Mismatch → the verdict itself; no refinement diagnostic.
        let mock = models_mock::spawn(&[
            ("/models", 200, r#"{"data":[{"id":"served-other"}]}"#.into()),
            ("/metrics", 200, "".into()),
        ]);
        let mismatch = snapshot_for_model_config(
            &mock.base_url,
            Some("configured-name".to_string()),
            ModelPreset::LocalVllm,
            None,
            None,
        )
        .await
        .expect("snapshot");
        assert_eq!(mismatch.health_status, HealthStatus::Mismatch);
        assert!(
            mismatch.diagnostic.is_none(),
            "mismatch is self-describing and carries no refinement"
        );
    }

    /// The inferred-window notice fires for local targets too (a listing
    /// without a window whose native follow-up fails, with a recognizable
    /// model name) — the message must stay endpoint-neutral. Pins that
    /// stripping the push or the code must go red.
    #[tokio::test]
    async fn snapshot_reports_context_window_inferred_on_local_targets() {
        use crate::core::model_endpoint::models_mock;
        let _lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mock = models_mock::spawn(&[
            (
                "/models",
                200,
                r#"{"data":[{"id":"deepseek-v4-pro"}]}"#.into(),
            ),
            // Non-Ollama local: the native follow-up stops with the 404.
            ("/api/ps", 404, "{}".into()),
            ("/metrics", 200, "".into()),
        ]);
        let snap = snapshot_for_model_config(
            &mock.base_url,
            Some("deepseek-v4-pro".to_string()),
            ModelPreset::Deepseek,
            None,
            None,
        )
        .await
        .expect("snapshot");
        assert_eq!(
            snap.max_model_len,
            Some(1_000_000),
            "the inferred v4 window must reach the display scale"
        );
        // The mock's empty 200 /metrics body parses to no series, so the
        // metrics notice rides alongside; the inferred-window notice is the
        // second entry (index pinned — a copy of the first notice's code would
        // pass a `contains` check).
        let codes: Vec<_> = snap.notices.iter().map(|d| d.code.as_str()).collect();
        assert_eq!(codes, ["metrics_unavailable", "context_window_inferred"]);
        let inferred = &snap.notices[1];
        assert!(
            inferred.http_status.is_none() && inferred.transport.is_none(),
            "an inferred-window notice is a plain code, no transport/http detail: {inferred:?}"
        );
    }

    /// The display follow-up on a local Ollama endpoint: /api/ps effective
    /// value wins (fresh, loaded ground truth); an Ollama-shaped ps with no
    /// entry for the model falls through to /api/show (positively cached for
    /// the TTL, and misses — no `num_ctx` — cached alike); a non-Ollama-shaped
    /// ps (404 — vLLM/LM Studio/generic locals) ends the lookup with no
    /// native follow-up at all. Each segment uses its own model name so the
    /// shared static cache cannot collide even if the OS reuses a mock port;
    /// the hit-count pins across two calls run under the crate ENV_LOCK so
    /// a concurrent cache-clearing core test (same lock) can never wipe an
    /// entry in between.
    #[tokio::test]
    async fn local_native_display_window_ps_then_show_then_stop() {
        use crate::core::model_endpoint::models_mock;
        let _lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // Loaded: ps wins, show unqueried.
        let mock = models_mock::spawn(&[
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"loaded-a","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        assert_eq!(
            local_native_display_window(
                &mock.base_url,
                None,
                Some("loaded-a"),
                Some("loaded-a"),
                None
            )
            .await,
            Some(131_072)
        );
        assert_eq!(mock.hits_for("/api/show"), 0);
        // Ollama-shaped but model not loaded: show fallback (cached).
        let mock = models_mock::spawn(&[
            ("/api/ps", 200, r#"{"models":[]}"#.into()),
            (
                "/api/show",
                200,
                r#"{"parameters":"num_ctx 32768","model_info":{"qwen3.context_length":40960}}"#
                    .into(),
            ),
        ]);
        assert_eq!(
            local_native_display_window(
                &mock.base_url,
                None,
                Some("declared-b"),
                Some("declared-b"),
                None
            )
            .await,
            Some(32_768)
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            1,
            "first poll pays one /api/show"
        );
        assert_eq!(
            local_native_display_window(
                &mock.base_url,
                None,
                Some("declared-b"),
                Some("declared-b"),
                None
            )
            .await,
            Some(32_768)
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            1,
            "the 1 Hz poll must be served from the 60s show cache"
        );
        // Not loaded and no num_ctx declaration: the show response only has
        // the GGUF trained cap (a capability ceiling, not the served
        // window) — the display stays without a native fact instead of
        // adopting it, and the miss is cached for the TTL: the 1 Hz poll
        // must not re-read GGUF metadata every second until the first load.
        let mock = models_mock::spawn(&[
            ("/api/ps", 200, r#"{"models":[]}"#.into()),
            (
                "/api/show",
                200,
                r#"{"model_info":{"qwen3.context_length":131072}}"#.into(),
            ),
        ]);
        assert_eq!(
            local_native_display_window(
                &mock.base_url,
                None,
                Some("cap-only-c"),
                Some("cap-only-c"),
                None
            )
            .await,
            None,
            "the trained cap must not become the display window"
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            1,
            "first poll pays one /api/show for the miss"
        );
        assert_eq!(
            local_native_display_window(
                &mock.base_url,
                None,
                Some("cap-only-c"),
                Some("cap-only-c"),
                None
            )
            .await,
            None
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            1,
            "misses are cached for the TTL too — no per-poll /api/show repeat"
        );
        // Non-Ollama local (ps 404): lookup stops, no show request.
        let mock = models_mock::spawn(&[("/api/ps", 404, "{}".into())]);
        assert_eq!(
            local_native_display_window(
                &mock.base_url,
                None,
                Some("stopped-d"),
                Some("stopped-d"),
                None
            )
            .await,
            None
        );
        assert_eq!(mock.hits_for("/api/show"), 0);
        // Partial roster: the gate declines before any request.
        let mock = models_mock::spawn(&[(
            "/api/ps",
            200,
            r#"{"models":[{"name":"roster-e","context_length":8}]}"#.into(),
        )]);
        assert_eq!(
            local_native_display_window(
                &mock.base_url,
                None,
                Some("configured"),
                Some("first-entry"),
                None
            )
            .await,
            None
        );
        assert_eq!(mock.hits_for("/api/ps"), 0);
    }

    /// New native-display gates (round-3 review): a public base_url saved
    /// under a local preset gets no native probes (same locality
    /// classification as the engine route); a configured=None snapshot may
    /// only follow a single-model roster (multi-model rosters would
    /// attribute the first entry's window to an unrelated pick); a known
    /// non-Ollama/LM Studio kind (positive kind cache) skips the native
    /// fetch entirely.
    #[tokio::test]
    async fn local_native_display_window_gates_locality_roster_and_kind() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        use crate::core::model_endpoint::models_mock;
        // Public base_url under a local preset (target_kind forced "local"
        // by the caller): the locality gate ends the lookup before any
        // request. Pinned without touching the network: the mock is
        // loopback-bound, but `::ffff:127.0.0.1` classifies as a non-local
        // IPv6 literal (the conservative arm of `base_url_uses_local_or_
        // private`) while still routing to the mock — a hit on the mock
        // proves the gate ran, so deleting the gate turns both assertions
        // red (the ps fact would come back and the hit counter moves).
        let mock = models_mock::spawn(&[(
            "/api/ps",
            200,
            r#"{"models":[{"name":"a","context_length":8}]}"#.into(),
        )]);
        let port = mock
            .base_url
            .rsplit(':')
            .next()
            .expect("mock base_url carries a port");
        let v4mapped = format!("http://[::ffff:127.0.0.1]:{port}");
        assert_eq!(
            local_native_display_window(&v4mapped, None, Some("a"), Some("a"), Some(1)).await,
            None
        );
        assert_eq!(
            mock.hits_for("/api/ps"),
            0,
            "the locality gate must end the lookup before any request"
        );
        // configured=None with a multi-model roster: first-entry attribution
        // would borrow an unrelated model's window — declined before any
        // request.
        let mock = models_mock::spawn(&[(
            "/api/ps",
            200,
            r#"{"models":[{"name":"first","context_length":8}]}"#.into(),
        )]);
        assert_eq!(
            local_native_display_window(
                &mock.base_url,
                None,
                None,
                Some("first"),
                Some(3) // three listed entries → ambiguous subject
            )
            .await,
            None
        );
        assert_eq!(mock.hits_for("/api/ps"), 0);
        // configured=None with a single-model roster: unambiguous — adopt.
        let mock = models_mock::spawn(&[
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"only","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        assert_eq!(
            local_native_display_window(&mock.base_url, None, None, Some("only"), Some(1)).await,
            Some(131_072)
        );
        // A cached non-Ollama/LM Studio kind (vLLM) skips the native fetch:
        // pre-seed the kind cache by probing the mock as vLLM, then assert
        // the display pays no /api/ps.
        let mock = models_mock::spawn(&[
            (
                "/v1/models",
                200,
                r#"{"data":[{"id":"m","owned_by":"vllm","max_model_len":4096}]}"#.into(),
            ),
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"m","context_length":8}]}"#.into(),
            ),
        ]);
        crate::core::model_endpoint::probe_local_server_kind(&mock.base_url, None).await;
        assert_eq!(
            local_native_display_window(&mock.base_url, None, Some("m"), Some("m"), Some(1)).await,
            None,
            "a known vLLM kind takes its window from the listing, not a native follow-up"
        );
        assert_eq!(mock.hits_for("/api/ps"), 0);
        // The vLLM classification above seeded the shared kind cache with
        // this mock's live port; clear it under the ENV_LOCK serializer (the
        // same lock engine-side cache tests hold) so a later test whose mock
        // reuses the port within the 60s kind TTL cannot inherit the stale
        // classification and skip its own probe.
        crate::core::model_endpoint::clear_probe_kind_cache();
    }

    /// A cached LM Studio kind routes the display follow-up to the v0
    /// served-window API, not the Ollama ps probe: dropping the dispatch
    /// arm would send LM Studio targets to a doomed 404 `/api/ps` and the
    /// served window (#726's 12918-vs-131072 shape) would never display.
    #[tokio::test]
    async fn local_native_display_window_uses_lmstudio_served_api() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        use crate::core::model_endpoint::models_mock;
        let mock = models_mock::spawn(&[
            (
                "/api/v0/models",
                200,
                r#"{"data":[{"id":"m","state":"loaded","max_context_length":131072,"loaded_context_length":12918}]}"#
                    .into(),
            ),
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"m","context_length":8}]}"#.into(),
            ),
        ]);
        // Seed the kind cache through the real classification (the v0
        // signature is LM Studio's; the battery never dials /api/ps).
        crate::core::model_endpoint::probe_local_server_kind(&mock.base_url, None).await;
        assert_eq!(
            local_native_display_window(&mock.base_url, None, Some("m"), Some("m"), Some(1)).await,
            Some(12_918),
            "the served window, fetched from the v0 API the kind selects"
        );
        assert_eq!(
            mock.hits_for("/api/ps"),
            0,
            "the LmStudio arm must not fall through to the Ollama ps probe"
        );
        // Same cross-test hygiene as the vLLM-kind gate test: the LM Studio
        // classification seeded the shared kind cache with this mock's live
        // port — clear it under ENV_LOCK so a port-reusing later test cannot
        // inherit the stale kind.
        crate::core::model_endpoint::clear_probe_kind_cache();
    }

    /// A hand-typed tagless configured name against a single-model Ollama
    /// roster: served is the canonical `llama3:latest`, so the attribution
    /// gate must fold the same canonical form the engine's adoption gate
    /// does — without the fold the display falls back to preset inference
    /// while the engine adopts the ps truth (display/engine divergence in
    /// exactly the motivating hand-typed-name shape).
    #[tokio::test]
    async fn local_native_display_window_follows_tagless_single_entry() {
        use crate::core::model_endpoint::models_mock;
        let mock = models_mock::spawn(&[
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"llama3:latest","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        assert_eq!(
            local_native_display_window(
                &mock.base_url,
                None,
                Some("llama3"),
                Some("llama3:latest"),
                Some(1)
            )
            .await,
            Some(131_072),
            "the tagless configured name must follow its own canonical ps entry"
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            0,
            "the ps fact wins; the show fallback stays unqueried"
        );
    }

    /// The same fold at the listing-match level: a tagless configured name
    /// must find its canonical entry wherever it sits in the roster —
    /// served degrading to the first (unrelated) entry used to refuse the
    /// whole follow-up even with a tag-tolerant attribution gate.
    #[test]
    fn parse_models_response_folds_ollama_tagless_names() {
        let (served, window) = parse_models_response(
            serde_json::json!({
                "data": [{"id": "deepseek-r1:14b"}, {"id": "llama3:latest"}]
            }),
            Some("llama3"),
        )
        .expect("a well-formed listing parses");
        assert_eq!(
            served.as_deref(),
            Some("llama3:latest"),
            "the tagless configured name must match its canonical entry, not the first entry"
        );
        assert_eq!(window, None, "Ollama listings carry no window fact");
    }

    /// The snapshot wiring end to end: a local (LocalVllm-preset) target
    /// whose `/v1/models` listing carries no window must reach the native
    /// follow-up through the real `snapshot_for_model_config` glue — the
    /// `listed_entries` extraction, the served-vs-configured threading, and
    /// the `target_kind == "local" && max_model_len.is_none()` trigger —
    /// and land the `/api/ps` fact on the snapshot's display window. A
    /// listing that already carries the window (a real vLLM) must never dial
    /// the native API. Every gate test above calls
    /// `local_native_display_window` directly; this one pins its call site.
    #[tokio::test]
    async fn snapshot_wiring_drives_native_display_window() {
        use crate::core::model_endpoint::models_mock;
        // The snapshot's listing leg requests `{base}/models`
        // (`models_probe_url` deliberately never appends `/v1`).
        let mock = models_mock::spawn(&[
            (
                "/models",
                200,
                r#"{"data":[{"id":"wiring-e2e-model"}]}"#.into(),
            ),
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"wiring-e2e-model","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
            ("/metrics", 200, "".into()),
        ]);
        let snapshot = snapshot_for_model_config(
            &mock.base_url,
            Some("wiring-e2e-model".into()),
            ModelPreset::LocalVllm,
            None,
            None,
        )
        .await
        .expect("a local target yields a snapshot");
        assert_eq!(
            mock.hits_for("/api/ps"),
            1,
            "the follow-up must run exactly once per snapshot"
        );
        assert_eq!(
            snapshot.max_model_len,
            Some(131_072),
            "the native fact must reach the display window through the snapshot wiring"
        );
        // A listing that already carries the window ends the lookup before
        // the native follow-up (a real vLLM deployment must not be probed
        // with the Ollama API).
        let listed = models_mock::spawn(&[
            (
                "/models",
                200,
                r#"{"data":[{"id":"wiring-e2e-listed","max_model_len":262144}]}"#.into(),
            ),
            ("/metrics", 200, "".into()),
        ]);
        let snapshot = snapshot_for_model_config(
            &listed.base_url,
            Some("wiring-e2e-listed".into()),
            ModelPreset::LocalVllm,
            None,
            None,
        )
        .await
        .expect("a local target yields a snapshot");
        assert_eq!(snapshot.max_model_len, Some(262_144));
        assert_eq!(
            listed.hits_for("/api/ps"),
            0,
            "a listed window must skip the native follow-up entirely"
        );
    }

    /// A user-declared window (cloud; probe value 131072 from a gateway list):
    /// the declaration must win as-is — regression pin for the reported bug
    /// (after saving 1048576 the progress bar still showed 131.1K).
    #[test]
    fn display_window_remote_configured_declaration_beats_probe() {
        let (window, inferred) =
            display_context_window(Some(1_048_576), "remote", Some(131_072), Some(1_000_000));
        assert_eq!(window, Some(1_048_576));
        assert!(!inferred);
    }

    /// A local deployment's probe is ground truth: the same min-clamp as the
    /// host route_limits.
    #[test]
    fn display_window_local_min_clamps_declaration_with_probe() {
        let (window, inferred) =
            display_context_window(Some(1_048_576), "local", Some(131_072), None);
        assert_eq!(window, Some(131_072));
        assert!(!inferred);
        // Reverse: declared 32K, machine at 128K → clamped to the declaration
        // (consistent with route_limits).
        let (window, _) = display_context_window(Some(32_768), "local", Some(131_072), None);
        assert_eq!(window, Some(32_768));
    }

    /// Local declaration but probe absent (/models parse failure etc.): the
    /// declaration applies as-is — matching the host route_limits (Some, None)
    /// arm; an absent probe produces no clamping.
    #[test]
    fn display_window_local_declared_without_probe_uses_declaration() {
        let (window, inferred) =
            display_context_window(Some(1_048_576), "local", None, Some(131_072));
        assert_eq!(window, Some(1_048_576));
        assert!(!inferred);
    }

    /// Abnormal target_kind (URL parse failure): a declaration still displays
    /// as declared, and the probe value does not override it.
    #[test]
    fn display_window_declared_on_nonlocal_target_ignores_probe() {
        let (window, inferred) =
            display_context_window(Some(262_144), "invalid", Some(131_072), None);
        assert_eq!(window, Some(262_144));
        assert!(!inferred);
    }

    /// Without a declaration the existing scale holds: the probe wins first and
    /// inference only fills in when the probe is absent; the diagnostic flag is
    /// set only when inference is truly adopted.
    #[test]
    fn display_window_without_declaration_keeps_probe_then_infer() {
        let (window, inferred) =
            display_context_window(None, "remote", Some(262_144), Some(131_072));
        assert_eq!(window, Some(262_144));
        assert!(!inferred);
        let (window, inferred) = display_context_window(None, "remote", None, Some(1_000_000));
        assert_eq!(window, Some(1_000_000));
        assert!(inferred);
        let (window, inferred) = display_context_window(None, "remote", None, None);
        assert_eq!(window, None);
        assert!(!inferred);
    }

    /// Mismatch detection semantics (extracted pure function): double-sided
    /// trim, exact comparison (case-sensitive), a missing side never downgrades,
    /// and a non-Ready original status can only be downgraded, never upgraded.
    #[test]
    fn mismatch_detection_trims_and_is_case_sensitive() {
        use VllmStatus::{Busy, Ready};
        assert_eq!(
            mismatch_if_served_differs(Ready, Some(" qwen "), Some("qwen")),
            Ready
        );
        // Case difference → Mismatch: the Engine sending the configured original
        // name would get model_not_found, so the red dot is a real signal (the
        // same scale as resolve_served_model_from_entries' exact match).
        assert_eq!(
            mismatch_if_served_differs(Ready, Some("Qwen"), Some("qwen")),
            VllmStatus::Mismatch
        );
        assert_eq!(
            mismatch_if_served_differs(Busy, Some("a"), Some("b")),
            VllmStatus::Mismatch
        );
        assert_eq!(mismatch_if_served_differs(Ready, None, Some("b")), Ready);
        assert_eq!(mismatch_if_served_differs(Ready, Some("a"), None), Ready);
        assert_eq!(mismatch_if_served_differs(Ready, None, None), Ready);
    }

    fn served_entry(
        id: &str,
        max_model_len: Option<u32>,
    ) -> crate::core::model_endpoint::OpenAiModelInfo {
        served_entry_with_output(id, max_model_len, None)
    }

    fn served_entry_with_output(
        id: &str,
        max_model_len: Option<u32>,
        max_output_tokens: Option<u32>,
    ) -> crate::core::model_endpoint::OpenAiModelInfo {
        crate::core::model_endpoint::OpenAiModelInfo {
            id: id.to_string(),
            max_model_len,
            max_output_tokens,
            loaded: None,
        }
    }

    /// LM Studio shape: `/v1/models` lists every downloaded model (multi-entry).
    /// A user-picked model found in the list must be used verbatim with its own
    /// window, never displaced by the first entry (root cause of the reported
    /// "conversation names A, engine loads B" break).
    #[test]
    fn served_model_keeps_configured_name_found_in_multi_model_list() {
        let entries = vec![
            served_entry("first-downloaded", Some(4096)),
            served_entry("user-picked", Some(131_072)),
        ];
        let (name, window, output) = resolve_served_model_from_entries("user-picked", &entries);
        assert_eq!(name, "user-picked");
        assert_eq!(window, Some(131_072));
        assert_eq!(output, None);
    }

    /// Real vLLM scenario: after a `--served-model-name` change the list holds
    /// a single unfamiliar name → follow it (pre-existing fix, preserved).
    #[test]
    fn served_model_follows_single_unknown_name() {
        let entries = vec![served_entry("served-name", Some(65536))];
        let (name, window, output) = resolve_served_model_from_entries("qwen36_35b_256k", &entries);
        assert_eq!(name, "served-name");
        assert_eq!(window, Some(65536));
        assert_eq!(output, None);
    }

    /// Multi-entry list without the configured name (stale/hand-edited config):
    /// keep the configured name so inference surfaces `model_not_found`
    /// explicitly, never silently switching to any listed model; the window
    /// must not be borrowed from another model either (compaction thresholds
    /// would derive from the wrong window).
    #[test]
    fn served_model_keeps_configured_name_when_absent_from_multi_model_list() {
        let entries = vec![served_entry("a", Some(4096)), served_entry("b", Some(8192))];
        let (name, window, output) = resolve_served_model_from_entries("gone", &entries);
        assert_eq!(name, "gone");
        assert_eq!(window, None);
        assert_eq!(output, None);
    }

    /// Single entry that equals the configured name: takes the "found in list"
    /// branch, keeping the name and its window (same outcome as the
    /// single-model follow branch — pinned so reordering branches cannot
    /// silently change where the window comes from).
    #[test]
    fn served_model_single_entry_equal_to_configured_keeps_name_and_window() {
        let entries = vec![served_entry("qwen36_35b_256k", Some(262_144))];
        let (name, window, output) = resolve_served_model_from_entries("qwen36_35b_256k", &entries);
        assert_eq!(name, "qwen36_35b_256k");
        assert_eq!(window, Some(262_144));
        assert_eq!(output, None);
    }

    /// Matched entry without `max_model_len` (server does not expose it): the
    /// name still resolves and the window stays `None`, falling back to the
    /// declared/inferred value — never fabricate a window.
    #[test]
    fn served_model_matched_entry_without_window_propagates_none() {
        let entries = vec![
            served_entry("first-downloaded", None),
            served_entry("user-picked", None),
        ];
        let (name, window, output) = resolve_served_model_from_entries("user-picked", &entries);
        assert_eq!(name, "user-picked");
        assert_eq!(window, None);
        assert_eq!(output, None);
    }

    /// The entry's self-reported output limit rides the same matched-entry
    /// rule as the window: only the configured model's own limit is used,
    /// never another listed model's.
    #[test]
    fn served_model_output_limit_follows_matched_entry() {
        let entries = vec![
            served_entry_with_output("first-downloaded", Some(4096), Some(8192)),
            served_entry_with_output("user-picked", Some(262_144), Some(65_536)),
        ];
        let (name, _, output) = resolve_served_model_from_entries("user-picked", &entries);
        assert_eq!(name, "user-picked");
        assert_eq!(output, Some(65_536));
        // No match → never borrow another model's output limit
        let (_, _, borrowed) = resolve_served_model_from_entries("gone", &entries);
        assert_eq!(borrowed, None);
    }

    /// Probed facts belong only to the model name actually requested:
    /// routes that follow the served name (vLLM, whose name is corrected to
    /// the entry itself) always adopt; routes that do not rename adopt only
    /// on an exact configured-name match, and single-entry "borrowed name"
    /// facts must not be misattributed.
    #[test]
    fn probed_facts_adoptable_only_on_exact_match_unless_route_follows_served_name() {
        // vLLM: after correction the facts share the same origin as the
        // final request name; always adopt (both inputs, before and after
        // correction).
        assert!(adopts_probed_facts(true, "qwen36_35b_256k", "served-name"));
        assert!(adopts_probed_facts(
            true,
            "qwen36_35b_256k",
            "qwen36_35b_256k"
        ));
        // Non-vLLM: exact match adopts; a single-entry borrowed name does not.
        assert!(adopts_probed_facts(false, "user-picked", "user-picked"));
        assert!(!adopts_probed_facts(
            false,
            "user-picked",
            "first-downloaded"
        ));
        // Case-differing names are NOT an exact match: the gate must stay
        // case-sensitive even though the display matching folds ASCII case
        // (loosening this silently changes which routes adopt probed /
        // native windows — pin the divergence).
        assert!(!adopts_probed_facts(false, "user-picked", "User-Picked"));
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

    /// Prometheus exports `NaN` / `+Inf` for degenerate series and Rust's
    /// f64 FromStr accepts both spellings; they are not measurements. The
    /// parser must skip them and keep scanning — a later finite series of
    /// the same name wins, an all-degenerate family yields None.
    #[test]
    fn prom_metric_skips_non_finite_values() {
        let text = "vllm:prefix_cache_hits_total NaN\n\
                    vllm:prefix_cache_hits_total{engine=\"0\"} 5.0\n";
        assert_eq!(
            parse_prom_metric(text, "vllm:prefix_cache_hits_total"),
            Some(5.0)
        );
        let text = "vllm:prefix_cache_hits_total{engine=\"0\"} +Inf\n";
        assert_eq!(
            parse_prom_metric(text, "vllm:prefix_cache_hits_total"),
            None
        );
    }

    /// With `data_parallel_size > 1` vLLM exports one series per `engine`
    /// label; every metric consumed here is additive, so the parser must sum
    /// the series — returning the first would publish engine 0 only. Also
    /// pins that a `_created` sibling never joins a sum and that a non-finite
    /// series is skipped, not summed and not fatal.
    #[test]
    fn prom_metric_sums_series_across_engine_labels() {
        let text = "vllm:num_requests_running{engine=\"0\",model_name=\"m\"} 3\n\
                    vllm:num_requests_running{engine=\"1\",model_name=\"m\"} 2\n\
                    vllm:num_requests_waiting{engine=\"0\"} 0\n\
                    vllm:num_requests_waiting{engine=\"1\"} 4\n\
                    vllm:prefix_cache_hits_total_created 1.7e9\n\
                    vllm:prefix_cache_hits_total{engine=\"0\"} 30\n\
                    vllm:prefix_cache_hits_total{engine=\"1\"} 10\n\
                    vllm:prefix_cache_queries_total{engine=\"0\"} NaN\n\
                    vllm:prefix_cache_queries_total{engine=\"1\"} 7\n";
        assert_eq!(
            parse_prom_metric(text, "vllm:num_requests_running"),
            Some(5.0)
        );
        assert_eq!(
            parse_prom_metric(text, "vllm:num_requests_waiting"),
            Some(4.0)
        );
        assert_eq!(
            parse_prom_metric(text, "vllm:prefix_cache_hits_total"),
            Some(40.0),
            "the _created sibling must never join the sum"
        );
        assert_eq!(
            parse_prom_metric(text, "vllm:prefix_cache_queries_total"),
            Some(7.0),
            "a non-finite series is skipped, not summed"
        );
    }

    /// A labels block that never closes (truncated body) must cost only its
    /// own line, and a `}` inside a quoted label value must not be mistaken
    /// for the block end.
    #[test]
    fn prom_metric_skips_malformed_lines_without_aborting_the_scan() {
        let truncated = "vllm:num_requests_running{engine=\"0\" 3\n\
             vllm:num_requests_running{engine=\"1\"} 2\n";
        assert_eq!(
            parse_prom_metric(truncated, "vllm:num_requests_running"),
            Some(2.0),
            "one malformed line must not void the later series"
        );
        let quoted = "vllm:num_requests_running{model_name=\"a}b\"} 4\n\
                      vllm:num_requests_running{model_name=\"c\"} 1\n";
        assert_eq!(
            parse_prom_metric(quoted, "vllm:num_requests_running"),
            Some(5.0),
            "a quoted '}}' is part of the label value, not the block end"
        );
    }

    /// 名称边界:同前缀的更长指标绝不能冒充本指标的读数。vLLM 真实导出里
    /// 存在 `vllm:num_requests_waiting_by_reason`(gauge,≥v0.20)与每个
    /// counter 的 `<name>_created` 时间戳 series;带标签与不带标签两种形状
    /// 都钉。删掉边界检查、或把取值从首个 token 换成末个 token,这里都必须
    /// 变红(时间戳 token 会被当成计数器值)。
    #[test]
    fn prom_metric_name_boundary_rejects_prefix_extensions() {
        let text = "vllm:num_requests_waiting_by_reason{engine=\"0\",reason=\"capacity\"} 7.0\n\
                    vllm:num_requests_waiting{engine=\"0\"} 3.0\n\
                    vllm:prefix_cache_hits_total_created{engine=\"0\"} 1.7654321e+09\n\
                    vllm:prefix_cache_hits_total{engine=\"0\"} 30.0\n\
                    vllm:prefix_cache_queries_total_created 1.7654321e+09\n\
                    vllm:prefix_cache_queries_total 40.0\n";
        assert_eq!(
            parse_prom_metric(text, "vllm:num_requests_waiting"),
            Some(3.0)
        );
        assert_eq!(
            parse_prom_metric(text, "vllm:prefix_cache_hits_total"),
            Some(30.0)
        );
        assert_eq!(
            parse_prom_metric(text, "vllm:prefix_cache_queries_total"),
            Some(40.0)
        );
    }

    /// 2026-06-10 本机 vLLM nightly(NVFP4) /metrics 实抓片段（TTFT 相关行）。
    const REAL_METRICS_FIXTURE: &str = "\
# HELP vllm:prompt_tokens_total Number of prefill tokens processed.\n\
# TYPE vllm:prompt_tokens_total counter\n\
vllm:prompt_tokens_total{engine=\"0\",model_name=\"qwen36_35b_256k\"} 4.1367205e+07\n\
vllm:time_to_first_token_seconds_bucket{engine=\"0\",le=\"0.001\",model_name=\"qwen36_35b_256k\"} 0.0\n\
vllm:time_to_first_token_seconds_created{engine=\"0\",model_name=\"qwen36_35b_256k\"} 1.7654321e+09\n\
vllm:time_to_first_token_seconds_count{engine=\"0\",model_name=\"qwen36_35b_256k\"} 498.0\n\
vllm:time_to_first_token_seconds_sum{engine=\"0\",model_name=\"qwen36_35b_256k\"} 1049.8486831188202\n";

    #[test]
    fn perf_metrics_parse_from_real_fixture() {
        let m = parse_perf_metrics(REAL_METRICS_FIXTURE);
        assert_eq!(m.ttft_sum_s, Some(1049.8486831188202));
        assert_eq!(m.ttft_count, Some(498.0));
    }

    #[test]
    fn perf_metrics_all_none_when_metrics_absent() {
        let m = parse_perf_metrics("some_other_metric 1.0\n");
        assert!(m.ttft_sum_s.is_none());
        assert!(m.ttft_count.is_none());
    }

    /// 运行状态上下文长度推断：覆盖设置页全部云端模型（2026-07 逐厂商核实，
    /// 依据为仓库 catalog + 底座启发式 + 各厂商官方文档，见 pinvou_known_context_window 注释）。
    /// Exemptions: aggregator org-prefixed ids and the per-deployment context
    /// figures behind self-hosted Coding Plan gateways are not mirrored one by
    /// one (see the aggregator/Coding Plan group comments in model-catalog.js);
    /// such ids not covered by the base chain fall to the preset fallback and
    /// are pinned at the fallback values below.
    #[test]
    fn infer_context_window_cloud_models() {
        let cases: &[(ModelPreset, &str, u32)] = &[
            // DeepSeek: every v4 model is 1M (original bug: preset fixed 128K);
            // deepseek-flash (V4.1-Flash) is officially 1M, corrected via the
            // core::model_context override table (the base applies the legacy
            // 128K heuristic to deepseek names without "v4", checked 2026-09-11)
            (ModelPreset::Deepseek, "deepseek-v4-pro", 1_000_000),
            (ModelPreset::Deepseek, "deepseek-v4-flash", 1_000_000),
            (ModelPreset::Deepseek, "deepseek-flash", 1_000_000),
            // Kimi：直连平台 kimi-k3 是 1M；Coding Plan 裸 k3 默认按 256K 安全值
            (ModelPreset::Kimi, "kimi-k3", 1_048_576),
            (ModelPreset::Kimi, "kimi-k2.7-code", 262_144),
            (ModelPreset::Kimi, "kimi-k2.7-code-highspeed", 262_144),
            (ModelPreset::Kimi, "kimi-k2.6", 262_144),
            // The Kimi Coding Plan rides the openai_compatible preset. Bare
            // kimi-for-coding serves K2.8 Preview since 2026-09 (officially
            // 1M on every plan tier), corrected by the PINVOU_OVERRIDES entry;
            // the highspeed variant stays on K2.7 Code HighSpeed's 256K and
            // must keep outranking it in the override table.
            (ModelPreset::OpenaiCompatible, "kimi-for-coding", 1_048_576),
            (
                ModelPreset::OpenaiCompatible,
                "kimi-for-coding-highspeed",
                262_144,
            ),
            // Since foundation #19 the catalog authoritatively covers k3-256k
            // (binary 256K = 262,144).
            (ModelPreset::OpenaiCompatible, "k3-256k", 262_144),
            (ModelPreset::OpenaiCompatible, "k3", 262_144),
            // GLM: 5.2 / 5.3 are 1M, 5.1/5-turbo are 202,752, 4.7 is officially 200K
            (ModelPreset::Glm, "glm-5.3-flashx", 1_000_000),
            (ModelPreset::Glm, "glm-5.2", 1_000_000),
            (ModelPreset::Glm, "glm-5.3", 1_000_000),
            (ModelPreset::Glm, "glm-5.1", 202_752),
            (ModelPreset::Glm, "glm-5-turbo", 202_752),
            (ModelPreset::Glm, "glm-4.7", 204_800),
            // MiniMax: M3 is 1M, the whole M2.x family is 204,800 (carried
            // over from the 2026-09-11 verification; current official pages
            // no longer publish per-model context figures for M2.x)
            (ModelPreset::Minimax, "MiniMax-M3", 1_000_000),
            // M3.1 Flash Preview (2026-09-26) is 1M; the exact "MiniMax-M3"
            // spelling cannot suffix-match the m3.1 wire id.
            (
                ModelPreset::Minimax,
                "MiniMax-M3.1-Flash-Preview",
                1_000_000,
            ),
            (ModelPreset::Minimax, "MiniMax-M2.7", 204_800),
            (ModelPreset::Minimax, "MiniMax-M2.7-highspeed", 204_800),
            (ModelPreset::Minimax, "MiniMax-M2.5", 204_800),
            (ModelPreset::Minimax, "MiniMax-M2.5-highspeed", 204_800),
            // MiMo: the v2.5 family is 1M; v2.6 (2026-09-22 default) is 1M
            // as well, carried by the core::model_context supplemental table
            // (the base has no v2.6 rows)
            (ModelPreset::Mimo, "mimo-v2.5-pro", 1_000_000),
            (ModelPreset::Mimo, "mimo-v2.5", 1_000_000),
            (ModelPreset::Mimo, "mimo-v2.6-pro", 1_000_000),
            (ModelPreset::Mimo, "mimo-v2.6-flash", 1_000_000),
            (ModelPreset::Mimo, "mimo-v2.6-pro-ultraspeed", 1_000_000),
            // Qwen：3.7 全系 / 3.6-flash 均 1M
            (ModelPreset::Qwen, "qwen3.7-plus", 1_000_000),
            (ModelPreset::Qwen, "qwen3.7-max", 1_000_000),
            (ModelPreset::Qwen, "qwen3.7-flash", 1_000_000),
            (ModelPreset::Qwen, "qwen3.6-flash", 1_000_000),
            // New Token Plan rows (auto / the 0813 snapshot / v4.1-flash) and
            // legacy/unmatched ids (qwen3-coder-plus / qwen3-coder-next left
            // the Qwen Coding Plan group on 2026-10-13; the only remaining
            // bare glm-5 row is the Alibaba Coding Plan group, and no
            // GLM-preset group lists it): ids not covered by the base chain
            // fall to the Qwen preset fallback (131,072). The bare deepseek
            // rows ride the base v4 heuristic; the vendor-prefixed
            // deepseek/deepseek-flash spelling lacks the "v4" substring, so
            // it is anchored in core::model_context instead.
            (ModelPreset::Qwen, "auto", 131_072),
            (ModelPreset::Qwen, "deepseek-v4-pro-0813", 1_000_000),
            (ModelPreset::Qwen, "deepseek-v4.1-flash", 1_000_000),
            // The 2026-10-13 Token Plan additions: the current primary
            // deepseek spellings ride the same base v4 heuristic (the -0813
            // alias above stays pinned), the vendor-prefixed
            // deepseek/deepseek-flash is anchored in core::model_context
            // (no "v4" substring, no vendor-prefix tolerance), and
            // mimo-v2.6-flash rides the core::model_context supplemental
            // table — pinned here so the settings-page rows stay covered by
            // this charter.
            (ModelPreset::Qwen, "deepseek-v4-pro-202606", 1_000_000),
            (ModelPreset::Qwen, "deepseek-v4-flash-202605", 1_000_000),
            (ModelPreset::Qwen, "deepseek/deepseek-flash", 1_000_000),
            (ModelPreset::Qwen, "mimo-v2.6-flash", 1_000_000),
            (ModelPreset::Qwen, "qwen3.6-plus", 131_072),
            (ModelPreset::Qwen, "qwen3-coder-plus", 131_072),
            (ModelPreset::Qwen, "qwen3-coder-next", 131_072),
            (ModelPreset::Qwen, "glm-5", 131_072),
            // The 2026-10-13 Alibaba Coding Plan addition rides the
            // PINVOU_KNOWN row (officially 1M; listed here so the
            // settings-page row is covered by this charter too, same as the
            // opus-5-5 precedent below).
            (ModelPreset::Qwen, "qwen3.5-plus", 1_000_000),
            // Doubao: evolving is already 1M; the 2-1 -260628 generation and
            // the 2-0 snapshots are officially 256k, while the -260915
            // snapshots moved to 1024k (volcengine 1330310, re-checked
            // 2026-09-28), carried by the core::model_context supplemental
            // table — the base has no doubao rows, and without the
            // supplemental table the engine side falls to 128K, diverging
            // from the monitor page
            (ModelPreset::Doubao, "doubao-seed-evolving", 1_048_576),
            (ModelPreset::Doubao, "doubao-seed-2-1-pro-260628", 262_144),
            (ModelPreset::Doubao, "doubao-seed-2-1-turbo-260628", 262_144),
            (ModelPreset::Doubao, "doubao-seed-2-1-pro-260915", 1_048_576),
            (
                ModelPreset::Doubao,
                "doubao-seed-2-1-lite-260915",
                1_048_576,
            ),
            (
                ModelPreset::Doubao,
                "doubao-seed-2-0-code-preview-260215",
                262_144,
            ),
            (ModelPreset::Doubao, "doubao-seed-2-0-pro-260215", 262_144),
            (ModelPreset::Doubao, "doubao-seed-2-0-lite-260428", 262_144),
            // Ark Coding Plan dot spellings (same underlying models, plan
            // model list) ride their own supplemental rows.
            (
                ModelPreset::OpenaiCompatible,
                "doubao-seed-2.1-pro",
                1_048_576,
            ),
            (
                ModelPreset::OpenaiCompatible,
                "doubao-seed-2.0-mini",
                262_144,
            ),
            (
                ModelPreset::OpenaiCompatible,
                "kimi-k2.8-preview",
                1_048_576,
            ),
            // Remaining Ark Coding Plan rows: the lite dot spelling rides its
            // supplemental row; glm-5.3-flash / deepseek-v4.1-flash go through
            // the base exact row and the v4 heuristic; ark-code-latest is the
            // Auto shell model with no official per-model context figure, so
            // both sides keep the conservative fallback (the disclosed gateway
            // pattern: engine 128K / monitor page 131,072).
            (
                ModelPreset::OpenaiCompatible,
                "doubao-seed-2.1-lite",
                1_048_576,
            ),
            (ModelPreset::OpenaiCompatible, "glm-5.3-flash", 1_000_000),
            (
                ModelPreset::OpenaiCompatible,
                "deepseek-v4.1-flash",
                1_000_000,
            ),
            (ModelPreset::OpenaiCompatible, "ark-code-latest", 131_072),
            // OpenAI 兼容示例：gpt-5.6 全系 1.05M
            (ModelPreset::OpenaiCompatible, "gpt-5.6-terra", 1_050_000),
            (ModelPreset::OpenaiCompatible, "gpt-5.6-luna", 1_050_000),
            (ModelPreset::OpenaiCompatible, "gpt-5.6-sol", 1_050_000),
            // xAI: the base known table lists grok-4.6 / grok-4.5 at 500K (checked 2026-09-11)
            (ModelPreset::Xai, "grok-4.6", 500_000),
            // grok-4.7 (September 2026 default) is 500K per the release
            // notes; the base has no row yet, filled by the
            // core::model_context supplemental table.
            (ModelPreset::Xai, "grok-4.7", 500_000),
            // The base known table still records grok-4.20-0309-* as 2M; the
            // core::model_context override table corrects it first to the 1M
            // re-checked from docs.x.ai on 2026-09-11 (matching the catalog desc).
            (ModelPreset::Xai, "grok-4.20-0309-reasoning", 1_000_000),
            (ModelPreset::Xai, "grok-4.20-0309-non-reasoning", 1_000_000),
            // The pre-retirement legacy spelling kept by the ACP preset: the
            // base has no row and relies on the core::model_context override
            // table correcting it to 1M, otherwise the engine falls to 128K
            // and diverges from the monitor page's prefs substring fallback.
            (ModelPreset::Xai, "grok-4.20-reasoning", 1_000_000),
            // The base known table only has bare "grok-build" → 512K, which
            // misses the -0.1 wire id; the core::model_context override table
            // corrects it to the official docs.x.ai 256K (matching the catalog desc).
            (ModelPreset::Xai, "grok-build-0.1", 256_000),
            // The base chain has no rows for gpt-6 / gemini-3.8; the
            // core::model_context override table fills them in per the official
            // figures (the engine-side resolved was None → 128K, diverging from
            // the monitor page fallback).
            (ModelPreset::Openai, "gpt-6-astra", 1_050_000),
            // The 2026-09-28 listing rows: same anchor logic as astra (no
            // base gpt-6 row); official 1,050,000 per their model pages.
            (ModelPreset::Openai, "gpt-6-sol", 1_050_000),
            (ModelPreset::Openai, "gpt-6-luna", 1_050_000),
            (ModelPreset::Gemini, "gemini-3.8-flash", 1_048_576),
            // The 2026-10-13 GA flagship joins the catalog: the base gemini
            // exact rows stop at 3.7/3.1 (no gemini-4 row), so the
            // core::model_context override table fills it in — official
            // unified 1M context (ai.google.dev gemini-4-argon model page,
            // checked 2026-10-14); listed here so the settings-page row is
            // covered by this charter too.
            (ModelPreset::Gemini, "gemini-4-argon", 1_048_576),
            // Anthropic models covered by the base catalog (haiku 200K) and the
            // PINVOU_OVERRIDES entries (opus-5 / fable-5-1 both 1M) go through
            // resolved_context_window; preset fallbacks are covered by the prefs
            // tests. The base known table does not list fable-5-1 exactly (only
            // fable-5), so without the override it would fall to the claude
            // wildcard 200K.
            (ModelPreset::Anthropic, "claude-haiku-4-5", 200_000),
            (ModelPreset::Anthropic, "claude-opus-5", 1_000_000),
            (ModelPreset::Anthropic, "claude-fable-5-1", 1_000_000),
            // 2026-09-22 default recommendation: rides the claude-opus-5
            // override row via suffix tolerance (model_context tests pin it;
            // listed here so the settings-page row is covered by this
            // charter too).
            (ModelPreset::Anthropic, "claude-opus-5-5", 1_000_000),
            // The 2026-10-13 settings rows: both officially 1M (the whole
            // 5.5 generation, per the models-overview comparison table) via
            // their PINVOU_OVERRIDES anchors; listed here so the
            // settings-page rows are covered by this charter too.
            (ModelPreset::Anthropic, "claude-sonnet-5-5", 1_000_000),
            (ModelPreset::Anthropic, "claude-haiku-5-5", 1_000_000),
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

    /// Every preset's default model must resolve a context window through the
    /// shared resolution entry point (resolved_context_window): the engine-side
    /// `effective_context_window` falls straight to 128K when resolved is None,
    /// while the monitor page's `infer_context_window` still has the prefs
    /// vendor fallback, so the two sides diverge (gpt-6-astra /
    /// gemini-3.8-flash once derived compaction thresholds from 128K while the
    /// page displayed 1M for exactly this reason). This test turns "changing a
    /// default must pass the resolution chain" into an explicit gate; the
    /// expected values are the vendor official figures and, together with
    /// `default_model_matches_vendor_docs_2026_10_13` and the frontend
    /// MODEL_PRESET_DEFS lock tests, form a three-layer default-value defense.
    #[test]
    fn preset_default_models_resolve_engine_context_window() {
        let cases: &[(ModelPreset, u32)] = &[
            // The `_256k` suffix hint resolves to 256,000 via N×1000 (same
            // source as the monitor page; the prefs LocalVllm fallback of
            // 262,144 only applies when resolved is None, which this default
            // never reaches).
            (ModelPreset::LocalVllm, 256_000),
            (ModelPreset::Deepseek, 1_000_000),
            (ModelPreset::Kimi, 1_048_576),
            (ModelPreset::Qwen, 1_000_000),
            (ModelPreset::Doubao, 1_048_576),
            (ModelPreset::Minimax, 1_000_000),
            (ModelPreset::Glm, 1_000_000),
            (ModelPreset::Mimo, 1_000_000),
            (ModelPreset::Openai, 1_050_000),
            (ModelPreset::Anthropic, 1_000_000),
            (ModelPreset::Gemini, 1_048_576),
            (ModelPreset::Xai, 500_000),
            // The openai_compatible Rust default gpt-5.6-terra only serves as
            // the legacy migration fallback (deliberately left empty on the
            // frontend), but as a default_model() output it must likewise pass
            // the shared resolution entry (base gpt-5.6 exact list → 1.05M).
            (ModelPreset::OpenaiCompatible, 1_050_000),
            // Since 2026-10-13 the Openai preset default is gpt-6.1-sol —
            // the (Openai, 1_050_000) row above covers it via the shared
            // core::model_context override anchor. The openai_responses
            // custom-endpoint fallback prefills the same id and must resolve
            // through the same entry.
            (ModelPreset::OpenaiResponses, 1_050_000),
        ];
        for (preset, expected) in cases {
            let model = preset.default_model();
            assert_eq!(
                crate::core::model_context::resolved_context_window(model),
                Some(*expected),
                "{preset:?} default model {model} must resolve an official context window via the shared resolution entry (the engine side has no prefs fallback)"
            );
        }
    }
}
