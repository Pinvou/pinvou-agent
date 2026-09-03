//! 模型服务端点（URL / 协议）层面的共用判定与直连：连接测试
//! （app/commands/settings.rs）与运行状态探测（features/monitor）都直连
//! `{base}/models`，鉴权方式与探测地址必须同一口径；品悟（features/review）与
//! 记忆回顾（features/memory）选 Anthropic preset 时走 Messages 原生协议，
//! 鉴权与地址口径与上述探测一致。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

/// 探测结果 TTL 缓存：同一 base_url 的本地服务类型在短时间内不会变化。
/// Probes are issued in parallel via `tokio::join!` (all requests share a 3s
/// timeout; a hung endpoint costs ~3s at worst). Repeated probes across
/// sessions/entry points (EnginePool spawn, connection test, frontend probe)
/// still amplify the cost; caching by base_url merges them.
const PROBE_CACHE_TTL: Duration = Duration::from_secs(60);

static PROBE_KIND_CACHE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, LocalServerKind)>>,
> = std::sync::OnceLock::new();

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

pub(crate) fn parse_models_response_list(v: serde_json::Value) -> Option<Vec<OpenAiModelInfo>> {
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

/// 通用 OpenAI 兼容 `/models` 探测。探测地址与云端 probe / 连接测试同一口径
/// （`models_probe_url`）：upstream 不带 `/v1` 也不补——glm `/paas/v4`、火山方舟
/// `/api/v3`、gemini `/v1beta/openai` 的 `/models` 端点均存在，补 `/v1` 会拼成
/// 不存在的地址永远 404。本地候选（vLLM/Ollama/LM Studio）由 discover 统一
/// 归一成 `/v1` 结尾后传入，行为不变。
pub async fn probe_openai_models(base_url: &str) -> Option<OpenAiModelsProbe> {
    let client = shared_probe_client()?;
    let url = models_probe_url(base_url);
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

/// Auth header for probe requests: a Bearer key from the same origin as the
/// real inference requests (authenticated endpoints such as local vLLM with
/// `--api-key` return 401 on `/v1/models`, so probing without credentials
/// misclassifies the endpoint as Generic). `None`/blank sends no auth header
/// (Ollama/LM Studio are auth-free by default and unaffected; services
/// without auth ignore the Bearer header).
fn apply_bearer(req: reqwest::RequestBuilder, bearer: Option<&str>) -> reqwest::RequestBuilder {
    match bearer.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => req.bearer_auth(key),
        None => req,
    }
}

/// Process-wide HTTP client shared by probes: feature-endpoint probes reuse
/// one connection pool instead of building a client per call (monitor's vLLM
/// served-name probe shares this pool via [`fetch_v1_models`]). Two
/// semantics aligned with the `features::monitor` probe singleton:
/// 1. The connection pool and proxy config are snapshotted at first build
///    and do not follow system proxy changes within the process;
/// 2. A build failure is cached process-wide as `None` with no per-call
///    retries, preserving the caller-side degradation semantics of
///    "probe failure → fall back to Generic/configured value"
///    (`Client::default()` panics on the same failure and cannot serve as
///    the fallback).
/// The per-request timeout stays at each probe's original 3 seconds;
/// request-level errors are unaffected and still handled by callers.
fn shared_probe_client() -> Option<&'static reqwest::Client> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .ok()
        })
        .as_ref()
}

/// 探测 Ollama：区分"已加载"（/api/ps）与"仅下载未加载"（/api/tags）。
/// 两个接口都是只读列表，不会触发加载；绝不能用推理请求探测。
/// See [`apply_bearer`] for `bearer` semantics.
pub async fn probe_ollama_models(
    base_url: &str,
    bearer: Option<&str>,
) -> Option<OpenAiModelsProbe> {
    let host = strip_v1_suffix(base_url)?;
    let client = shared_probe_client()?;
    // 已加载集合：失败按空集（全部未加载），不影响已下载列表。
    let loaded_names = match apply_bearer(client.get(format!("{host}/api/ps")), bearer)
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
    let resp = apply_bearer(client.get(format!("{host}/api/tags")), bearer)
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
    if let Some(probe) = probe_lmstudio_v0_only(base_url, None).await {
        return Some(probe);
    }
    probe_openai_models(base_url).await
}

/// Anthropic 官方端点判定：仅 api.anthropic.com 主机走 x-api-key 鉴权，其余一律 Bearer。
pub fn is_anthropic_api_url(url: &reqwest::Url) -> bool {
    url.host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("api.anthropic.com"))
}

/// 同上，接受 base_url 字符串；解析失败按非 Anthropic 处理（走 Bearer）。
pub fn is_anthropic_endpoint(base_url: &str) -> bool {
    reqwest::Url::parse(base_url.trim())
        .ok()
        .is_some_and(|url| is_anthropic_api_url(&url))
}

/// 模型列表探测地址：upstream 带 `/v1` 后缀时直接拼 `/models`；不带也拼 `/models`
/// 而非补一层 `/v1`——glm `/paas/v4`、火山方舟 `/api/v3`、gemini `/v1beta/openai`
/// 的 `/models` 端点均存在，补 `/v1` 会拼成不存在的地址永远 404。
pub fn models_probe_url(upstream: &str) -> String {
    format!("{}/models", upstream.trim_end_matches('/'))
}

/// 去掉 upstream 末尾的 `/v1`，取 API 根（Prometheus `/metrics`、Ollama `/api/tags`、
/// LM Studio `/api/v0/models` 等原生端点都不在 `/v1` 之下）。
pub fn strip_v1_suffix(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    Some(
        trimmed
            .strip_suffix("/v1")
            .map(String::from)
            .unwrap_or_else(|| trimmed.to_string()),
    )
}

/// 本地推理服务类型（决定思考控制走哪套 wire 协议）。
///
/// Ollama uses the `think` boolean switch (no effort tiers); vLLM and the
/// wire-identical SGLang / llama.cpp / KoboldCpp / LMDeploy / Docker Model
/// Runner support off/low/medium/high effort tiers via
/// `chat_template_kwargs.enable_thinking` + `reasoning_effort` (the bridge
/// maps them uniformly to the engine vllm provider); LM Studio and generic
/// OpenAI-compatible endpoints take the openai wire route, where the engine
/// does not yet inject thinking control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalServerKind {
    /// vLLM：底座经 `chat_template_kwargs.enable_thinking` + `reasoning_effort`
    /// 支持 off/low/medium/high 档位。
    Vllm,
    /// Ollama：底座经 `think` 布尔支持开关（off=think:false，其余 think:true）。
    Ollama,
    /// SGLang: signature endpoint `/get_server_info`; thinking-control wire
    /// is identical to vLLM.
    Sglang,
    /// llama.cpp (llama-server): signature endpoint `/props`;
    /// thinking-control wire is identical to vLLM.
    LlamaCpp,
    /// KoboldCpp: signature endpoint `/api/extra/version`; also compatible
    /// with llama.cpp's `/props`, so its identification priority ranks above
    /// LlamaCpp; thinking-control wire is identical to vLLM.
    KoboldCpp,
    /// LMDeploy: `owned_by == "lmdeploy"` in `/v1/models` (medium-confidence
    /// signature); thinking-control wire is identical to vLLM.
    LmDeploy,
    /// Docker Model Runner: `/models` JSON-array management API on port 12434
    /// only (port-gated); thinking-control wire is identical to vLLM.
    DockerModelRunner,
    /// LM Studio：底座 openai wire route 暂不注入思考控制（保持旧行为）。
    LmStudio,
    /// 其他通用 OpenAI 兼容服务（探测不到任何特征端点）。
    Generic,
}

/// 探测本地推理服务类型。只应在本地端点（`base_url_uses_local_or_private`）上
/// called; all candidate signature endpoints are probed in parallel
/// (`tokio::join!`, shared 3s timeout, a hung endpoint costs ~3s at worst),
/// and once all complete the result is picked by signature-exclusivity
/// priority: Ollama (`/api/tags`) > LM Studio (`/api/v0/models`) > KoboldCpp
/// (`/api/extra/version`) > LlamaCpp (`/props`) > Sglang
/// (`/get_server_info`) > LmDeploy (`owned_by` in `/v1/models`) > Vllm
/// (`owned_by`) > Generic. The only exception is DockerModelRunner
/// port-gate priority: DMR also exposes an Ollama-compatible `/api/tags`, so
/// on port 12434 only, the management API shape (JSON array) at the host
/// root `/models` is checked before the Ollama decision — a hit means
/// DockerModelRunner, a miss continues in the original order.
/// Probe failure
/// (service not started/timeout/auth 401) returns `Generic`; callers keep the
/// existing openai wire route and do not change behavior on probe failure.
///
/// `bearer` is a credential from the same origin as the endpoint's real
/// inference requests (see [`apply_bearer`]): probing an authenticated
/// endpoint (vLLM `--api-key`) without credentials always 401s into a
/// Generic misclassification, losing default-off thinking and the real
/// effort tiers. Pass `None` for endpoints without auth.
///
/// Results are cached by base_url for `PROBE_CACHE_TTL`: even with parallel
/// probes a hung endpoint still costs one ~3s,
/// repeated probes across sessions/entry points amplify the cost; a hit
/// within the TTL returns the cached value directly. The cache/in-flight key
/// contains only base_url, never credentials: a positive identification
/// (Ollama/vLLM/LM Studio) presupposes successful auth and is a statement
/// about the server type itself, independent of the caller's credential, so
/// merging by URL is safe; a failure result (Generic) is not written to the
/// long cache — the service may simply be down (or the key changed), so the
/// next call should re-probe immediately instead of staying pinned to the
/// wrong Generic route for 60s. Concurrent misses share one probe through
/// the in-flight registry (first caller executes, the rest wait for the
/// broadcast) instead of each paying a serial probe.
pub async fn probe_local_server_kind(base_url: &str, bearer: Option<&str>) -> LocalServerKind {
    let key = base_url.trim_end_matches('/').to_string();
    if let Some(kind) = probe_kind_cache_get(&key) {
        return kind;
    }
    let kind = probe_kind_inflight(&key, bearer).await;
    if kind != LocalServerKind::Generic {
        probe_kind_cache_put(&key, kind);
    }
    kind
}

/// Concurrent dedupe registry: key → completion signal (Weak; the first
/// caller holds the only strong reference). The first caller runs the probe,
/// sends the result on completion and deregisters; concurrent callers
/// subscribe and wait so the probe runs once. When the first caller is
/// cancelled (task abort / dropped by an outer select), its strong reference
/// drops together with the future and the registry does not extend that
/// lifetime: waiters observe the channel closing (`changed()` returning Err)
/// and degrade to probing themselves, while later callers whose upgrade
/// fails start a fresh probe — a result is always obtained and no permanently
/// poisoned registry entry can exist.
static PROBE_KIND_INFLIGHT: std::sync::OnceLock<
    std::sync::Mutex<
        std::collections::HashMap<
            String,
            std::sync::Weak<tokio::sync::watch::Sender<Option<LocalServerKind>>>,
        >,
    >,
> = std::sync::OnceLock::new();

/// In-flight registration guard: holds the first caller's only strong
/// reference to the sender. If the future is cancelled while awaiting the
/// probe, the completion block never runs; on Drop the guard additionally
/// clears the stale Weak in the registry that still points at itself
/// (whether the cleanup succeeds does not affect correctness — dropping the
/// sender itself closes the channel and wakes the waiters).
struct InflightRegistration {
    key: String,
    sender: Option<Arc<tokio::sync::watch::Sender<Option<LocalServerKind>>>>,
}

impl Drop for InflightRegistration {
    fn drop(&mut self) {
        let Some(sender) = self.sender.take() else {
            return;
        };
        if let Some(registry) = PROBE_KIND_INFLIGHT.get() {
            if let Ok(mut guard) = registry.lock() {
                // Only clean up the registration that still points at ourselves
                // (same pointer); do not remove a fresh probe started later by
                // another caller.
                if guard
                    .get(&self.key)
                    .is_some_and(|weak| weak.as_ptr() == Arc::as_ptr(&sender))
                {
                    guard.remove(&self.key);
                }
            }
        }
        // Drop the only strong reference → channel closes → waiters see
        // changed() Err and degrade to probing themselves.
    }
}

async fn probe_kind_inflight(base_url: &str, bearer: Option<&str>) -> LocalServerKind {
    /// 注册结果：要么成为首个执行者，要么订阅在途探测的完成信号。
    enum Inflight {
        First(Arc<tokio::sync::watch::Sender<Option<LocalServerKind>>>),
        Wait(tokio::sync::watch::Receiver<Option<LocalServerKind>>),
    }
    let registry = PROBE_KIND_INFLIGHT.get_or_init(Default::default);
    // 注册/订阅在同步块内完成，guard 不跨 await（Send 约束）。
    let entry = {
        let Ok(mut guard) = registry.lock() else {
            // 注册表锁不可用（中毒）：降级为无合并直探。
            return probe_local_server_kind_uncached(base_url, bearer).await;
        };
        // upgrade failure = stale Weak (leftover from a previously cancelled
        // probe): treat as no in-flight probe and overwrite with a new
        // registration.
        if let Some(rx) = guard.get(base_url).and_then(|weak| weak.upgrade()) {
            Inflight::Wait(rx.subscribe())
        } else {
            let (tx, _rx) = tokio::sync::watch::channel(None);
            let tx = Arc::new(tx);
            guard.insert(base_url.to_string(), Arc::downgrade(&tx));
            Inflight::First(tx)
        }
    };
    match entry {
        // First caller: run the probe, broadcast the result on completion and
        // deregister. If cancelled while awaiting, the guard drops the strong
        // reference and closes the channel (see InflightRegistration).
        Inflight::First(sender) => {
            let mut registration = InflightRegistration {
                key: base_url.to_string(),
                sender: Some(Arc::clone(&sender)),
            };
            let kind = probe_local_server_kind_uncached(base_url, bearer).await;
            // Normal completion: hand over the sender so the guard's Drop
            // does not clean up twice.
            registration.sender = None;
            let _ = sender.send(Some(kind));
            if let Ok(mut guard) = registry.lock() {
                if guard
                    .get(base_url)
                    .is_some_and(|weak| weak.as_ptr() == Arc::as_ptr(&sender))
                {
                    guard.remove(base_url);
                }
            }
            kind
        }
        // Concurrent caller: wait for the first caller to broadcast the
        // result. Sharing has a credential boundary: a positive
        // identification (Vllm/Ollama/LmStudio) presupposes that the probe
        // request authenticated successfully and is a statement about the
        // server type itself, independent of the caller's credential — reuse
        // it directly. Generic only means the feature endpoints were
        // unreachable within the first caller's credential context (e.g. an
        // authenticated vLLM 401s on missing/wrong credentials), which does
        // not hold for a waiter with different credentials — it must re-probe
        // directly with its own credentials to avoid the cross-talk of "a
        // credential-less First broadcasting Generic while a correctly
        // authenticated Waiter gets misclassified" (see the mixed-credential
        // concurrency regression test).
        Inflight::Wait(mut rx) => loop {
            // The result may already have been broadcast at subscribe time
            // (send happens before subscribe): check the current value first.
            // Copy before matching so the borrow guard's temporary does not
            // live across await (Send bound).
            let current = *rx.borrow_and_update();
            if let Some(kind) = current {
                return match kind {
                    LocalServerKind::Generic => {
                        probe_local_server_kind_uncached(base_url, bearer).await
                    }
                    positive => positive,
                };
            }
            if rx.changed().await.is_err() {
                // Broadcaster cancelled/dropped: channel closed (changed()
                // Err); degrade to a direct probe as the fallback.
                return probe_local_server_kind_uncached(base_url, bearer).await;
            }
        },
    }
}

fn probe_kind_cache_get(base_url: &str) -> Option<LocalServerKind> {
    let cache = PROBE_KIND_CACHE.get_or_init(Default::default);
    let guard = cache.lock().ok()?;
    let (inserted_at, kind) = guard.get(base_url)?;
    if inserted_at.elapsed() > PROBE_CACHE_TTL {
        return None;
    }
    Some(*kind)
}

fn probe_kind_cache_put(base_url: &str, kind: LocalServerKind) {
    let cache = PROBE_KIND_CACHE.get_or_init(Default::default);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(base_url.to_string(), (std::time::Instant::now(), kind));
    }
}

/// 仅测试用：清空探测缓存，避免 TTL 命中污染 mock 调用计数/跨用例状态。
#[cfg(test)]
pub(crate) fn clear_probe_kind_cache() {
    if let Some(cache) = PROBE_KIND_CACHE.get() {
        if let Ok(mut guard) = cache.lock() {
            guard.clear();
        }
    }
    if let Some(inflight) = PROBE_KIND_INFLIGHT.get() {
        if let Ok(mut guard) = inflight.lock() {
            guard.clear();
        }
    }
}

/// Hit results of each candidate probe, for [`select_local_server_kind`] to
/// pick by priority. Kept as a plain data structure so the port-gate-first
/// ordering decision does not depend on a real port-12434 binding and tests
/// can construct hit combinations directly.
struct ProbeCandidateHits {
    /// base_url's effective port is 12434 (Docker Model Runner port gate).
    docker_port_gated: bool,
    /// Host-root `/models` on port 12434 returns a JSON array (DMR
    /// management API shape).
    docker_mgmt_shape: bool,
    ollama: bool,
    lmstudio_v0: bool,
    koboldcpp: bool,
    llamacpp: bool,
    sglang: bool,
    /// `/v1/models` response body (one fetch shared by the LMDeploy and vLLM
    /// owned_by decisions).
    v1_models: Option<serde_json::Value>,
}

/// Priority selection over probe results (pure function, no requests).
/// Order: Docker Model Runner port gate first (DMR also exposes an
/// Ollama-compatible `/api/tags`; under the generic priority the Ollama
/// branch would hit first and DockerModelRunner would be unreachable, so on
/// port 12434 only, the management API shape is checked before the Ollama
/// decision — a miss continues in the original order) > Ollama > LM Studio >
/// KoboldCpp > LlamaCpp > Sglang > LmDeploy > Vllm > Generic. KoboldCpp is
/// also compatible with llama.cpp's `/props`, so it must come before
/// LlamaCpp; LMDeploy and vLLM share the owned_by signature, LMDeploy first.
fn select_local_server_kind(hits: ProbeCandidateHits) -> LocalServerKind {
    if hits.docker_port_gated && hits.docker_mgmt_shape {
        return LocalServerKind::DockerModelRunner;
    }
    if hits.ollama {
        return LocalServerKind::Ollama;
    }
    if hits.lmstudio_v0 {
        return LocalServerKind::LmStudio;
    }
    if hits.koboldcpp {
        return LocalServerKind::KoboldCpp;
    }
    if hits.llamacpp {
        return LocalServerKind::LlamaCpp;
    }
    if hits.sglang {
        return LocalServerKind::Sglang;
    }
    // LMDeploy: any owned_by == "lmdeploy" in /v1/models. Medium-confidence
    // signature: owned_by is a self-reported field of each implementation,
    // not an LMDeploy-only convention, so it serves only as a weak marker.
    if hits
        .v1_models
        .as_ref()
        .is_some_and(|v| v1_models_owned_by_matches(v, "lmdeploy"))
    {
        return LocalServerKind::LmDeploy;
    }
    // vLLM：/v1/models 响应中模型 `owned_by == "vllm"`（vLLM 标准实现字段）。
    if hits
        .v1_models
        .as_ref()
        .is_some_and(|v| v1_models_owned_by_matches(v, "vllm"))
    {
        return LocalServerKind::Vllm;
    }
    LocalServerKind::Generic
}

/// The actual probe without cache (a TTL cache hit returns directly; see
/// `probe_local_server_kind`). All candidate probes are issued in parallel
/// via `tokio::join!` (each shares `shared_probe_client`'s 3s timeout, so a
/// hung endpoint costs ~3s at worst instead of accumulating serially);
/// `/v1/models` is fetched only once, shared by the LMDeploy and vLLM
/// `owned_by` decisions. Once all complete, the result is picked by
/// signature-exclusivity priority.
/// See [`apply_bearer`] for `bearer` semantics.
async fn probe_local_server_kind_uncached(base_url: &str, bearer: Option<&str>) -> LocalServerKind {
    let docker_port_gated = is_docker_model_runner_port(base_url);
    let (ollama, lmstudio, koboldcpp, llamacpp, sglang, v1_models, docker_mgmt) = tokio::join!(
        probe_ollama_tags(base_url, bearer),
        probe_lmstudio_v0_only(base_url, bearer),
        probe_koboldcpp_version(base_url, bearer),
        probe_llamacpp_props(base_url, bearer),
        probe_sglang_server_info(base_url, bearer),
        fetch_v1_models(base_url, bearer),
        probe_docker_model_runner(base_url, bearer),
    );
    select_local_server_kind(ProbeCandidateHits {
        docker_port_gated,
        docker_mgmt_shape: docker_mgmt,
        ollama,
        lmstudio_v0: lmstudio.is_some(),
        koboldcpp,
        llamacpp,
        sglang,
        v1_models,
    })
}

/// 仅探测 LM Studio 独有原生端点 `/api/v0/models`（不回退 `/v1/models`，后者
/// 不具判别性）。响应形状不认识时返回 `None`，调用方继续探测下一个候选。
/// 既是 `probe_lmstudio_models` 的 v0 前置，也是本地服务判别探测的前置：
/// 判别场景必须用它而非 `probe_lmstudio_models`（后者回退 `/v1/models`，而
/// `/v1/models` 是通用端点，Ollama/通用服务也有，会把非 LM Studio 误判）。
/// See [`apply_bearer`] for `bearer` semantics.
async fn probe_lmstudio_v0_only(base_url: &str, bearer: Option<&str>) -> Option<OpenAiModelsProbe> {
    let host = strip_v1_suffix(base_url)?;
    let client = shared_probe_client()?;
    let resp = apply_bearer(client.get(format!("{host}/api/v0/models")), bearer)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .ok()?;
    let v = resp.json::<serde_json::Value>().await.ok()?;
    Some(OpenAiModelsProbe {
        models: parse_lmstudio_v0_models(&v)?,
    })
}

/// 抓取 OpenAI 兼容 `/v1/models` 响应体。探测地址口径与
/// `features::monitor::probe_vllm_model_info` 一致：upstream 带 `/v1` 直接拼
/// `/models`，不带则补 `/v1/models`。失败/非 2xx/解析失败返回 `None`，调用方
/// treated as probe failure. Shared with the kind-probe chain
/// (`probe_local_server_kind_uncached` fetches once for the LMDeploy/vLLM
/// owned_by decisions) and monitor's vLLM served-name
/// probe, so the `/v1/models` URL assembly stays consistent in both places.
/// See [`apply_bearer`] for `bearer` semantics: an authenticated vLLM
/// returns 401 on `/v1/models` without credentials.
pub(crate) async fn fetch_v1_models(
    base_url: &str,
    bearer: Option<&str>,
) -> Option<serde_json::Value> {
    let Some(client) = shared_probe_client() else {
        return None;
    };
    let url = if base_url.trim_end_matches('/').ends_with("/v1") {
        format!("{}/models", base_url.trim_end_matches('/'))
    } else {
        format!("{}/v1/models", base_url.trim_end_matches('/'))
    };
    let Ok(resp) = apply_bearer(client.get(url), bearer).send().await else {
        return None;
    };
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<serde_json::Value>().await.ok()
}

/// Lightweight Ollama probe for identification: checks only `/api/tags` (200
/// with a non-empty model list); the decision matches `probe_ollama_models`
/// but does not fetch `/api/ps` — kind probing does not need loaded state,
/// and model-list assembly remains exclusive to `probe_ollama_models`.
/// See [`apply_bearer`] for `bearer` semantics.
async fn probe_ollama_tags(base_url: &str, bearer: Option<&str>) -> bool {
    let Some(host) = strip_v1_suffix(base_url) else {
        return false;
    };
    let Some(client) = shared_probe_client() else {
        return false;
    };
    let Ok(resp) = apply_bearer(client.get(format!("{host}/api/tags")), bearer)
        .send()
        .await
        .and_then(|r| r.error_for_status())
    else {
        return false;
    };
    let Ok(v) = resp.json::<serde_json::Value>().await else {
        return false;
    };
    !parse_ollama_tag_names(v).is_empty()
}

/// Probe KoboldCpp: `/api/extra/version` returns 200 and the JSON `result`
/// string contains "koboldcpp" (case-insensitive).
/// See [`apply_bearer`] for `bearer` semantics.
async fn probe_koboldcpp_version(base_url: &str, bearer: Option<&str>) -> bool {
    let Some(host) = strip_v1_suffix(base_url) else {
        return false;
    };
    let Some(client) = shared_probe_client() else {
        return false;
    };
    let Ok(resp) = apply_bearer(client.get(format!("{host}/api/extra/version")), bearer)
        .send()
        .await
        .and_then(|r| r.error_for_status())
    else {
        return false;
    };
    let Ok(v) = resp.json::<serde_json::Value>().await else {
        return false;
    };
    v.get("result")
        .and_then(Value::as_str)
        .is_some_and(|s| s.to_ascii_lowercase().contains("koboldcpp"))
}

/// Probe llama.cpp: `/props` returns 200 and the JSON object contains both
/// `default_generation_settings` and `total_slots`. Note KoboldCpp is also
/// compatible with `/props`, so this probe's identification priority must
/// rank below KoboldCpp (see the selection order in
/// `probe_local_server_kind_uncached`).
/// See [`apply_bearer`] for `bearer` semantics.
async fn probe_llamacpp_props(base_url: &str, bearer: Option<&str>) -> bool {
    let Some(host) = strip_v1_suffix(base_url) else {
        return false;
    };
    let Some(client) = shared_probe_client() else {
        return false;
    };
    let Ok(resp) = apply_bearer(client.get(format!("{host}/props")), bearer)
        .send()
        .await
        .and_then(|r| r.error_for_status())
    else {
        return false;
    };
    let Ok(v) = resp.json::<serde_json::Value>().await else {
        return false;
    };
    v.as_object().is_some_and(|obj| {
        obj.contains_key("default_generation_settings") && obj.contains_key("total_slots")
    })
}

/// Probe SGLang: `/get_server_info` returns 200 and the JSON object has a
/// `version` string field (the response is a large serialized ServerArgs
/// JSON; parse loosely and only check that version exists).
/// See [`apply_bearer`] for `bearer` semantics.
async fn probe_sglang_server_info(base_url: &str, bearer: Option<&str>) -> bool {
    let Some(host) = strip_v1_suffix(base_url) else {
        return false;
    };
    let Some(client) = shared_probe_client() else {
        return false;
    };
    let Ok(resp) = apply_bearer(client.get(format!("{host}/get_server_info")), bearer)
        .send()
        .await
        .and_then(|r| r.error_for_status())
    else {
        return false;
    };
    let Ok(v) = resp.json::<serde_json::Value>().await else {
        return false;
    };
    v.get("version").and_then(Value::as_str).is_some()
}

/// Docker Model Runner port gate: its management API is fixed on port 12434;
/// no other port is probed (avoids an extra `/models` request against
/// arbitrary local endpoints).
fn is_docker_model_runner_port(base_url: &str) -> bool {
    reqwest::Url::parse(base_url.trim())
        .ok()
        .and_then(|url| url.port_or_known_default())
        == Some(12434)
}

/// Docker Model Runner management API root: the management endpoint lives at
/// the host root (`/models`), not under the OpenAI-compatible prefix. Strip
/// trailing prefixes in the order `/engines/v1` → `/engines` → `/v1`
/// (`/engines/v1` must come before `/engines`, otherwise `/engines` is left
/// behind), normalizing documented addresses such as
/// `http://host:12434/engines/v1`, `http://host:12434/v1`, and the bare host
/// to the host root.
fn strip_docker_model_runner_root(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    for suffix in ["/engines/v1", "/engines", "/v1"] {
        if let Some(root) = trimmed.strip_suffix(suffix) {
            return Some(root.to_string());
        }
    }
    Some(trimmed.to_string())
}

/// Probe Docker Model Runner: only when base_url's effective port is 12434
/// (port gate, see [`is_docker_model_runner_port`]), GET `/models` at the
/// host root (address normalization in [`strip_docker_model_runner_root`]);
/// a hit requires 200 with a JSON-array response — that is the Docker
/// management API shape, whereas the OpenAI-compatible shape is an object
/// with "data", which distinguishes them.
/// See [`apply_bearer`] for `bearer` semantics.
async fn probe_docker_model_runner(base_url: &str, bearer: Option<&str>) -> bool {
    if !is_docker_model_runner_port(base_url) {
        return false;
    }
    let Some(root) = strip_docker_model_runner_root(base_url) else {
        return false;
    };
    let Some(client) = shared_probe_client() else {
        return false;
    };
    let Ok(resp) = apply_bearer(client.get(format!("{root}/models")), bearer)
        .send()
        .await
        .and_then(|r| r.error_for_status())
    else {
        return false;
    };
    resp.json::<serde_json::Value>()
        .await
        .ok()
        .is_some_and(|v| v.is_array())
}

/// Whether any model's `owned_by` in the `/v1/models` response body equals
/// the expected value (case-insensitive). Lets vLLM (`"vllm"`) and LMDeploy
/// (`"lmdeploy"`) identification share the same fetch result.
fn v1_models_owned_by_matches(v: &serde_json::Value, expected: &str) -> bool {
    v.get("data")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("owned_by")
                    .and_then(Value::as_str)
                    .is_some_and(|owned| owned.eq_ignore_ascii_case(expected))
            })
        })
}

/// Messages API 版本头，与连接测试同一口径。
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Messages 协议请求地址：upstream 带 `/v1` 后缀直接拼 `/messages`，否则补
/// `/v1/messages`（官方 preset 上游为 `https://api.anthropic.com`，Messages
/// 端点在 `/v1/messages`；模型列表探测的 `models_probe_url` 不补 `/v1`，
/// 二者口径不同，不要混用）。
pub fn anthropic_messages_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        format!("{trimmed}/messages")
    } else {
        format!("{trimmed}/v1/messages")
    }
}

/// 从 Messages 响应提取文本：`content` 是 block 数组，拼接其中 `type == "text"`
/// 的块（thinking 等块跳过）。无文本块返回 `None`，调用方按解析失败报错。
pub fn anthropic_messages_text(v: &Value) -> Option<String> {
    let blocks = v.get("content")?.as_array()?;
    let text = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<String>();
    (!text.is_empty()).then_some(text)
}

/// Anthropic Messages 协议直连：x-api-key + anthropic-version 鉴权（官方端点不接受
/// Bearer），`system` 是独立字段而非 messages 首条。Messages API 没有
/// `response_format`，JSON 约束靠 prompt 措辞 + 调用方解析兜底（与既有 chat/completions
/// 路径的 fallback 解析同款）。api_key 为空时不带鉴权头（同连接测试口径）。
pub async fn post_anthropic_messages(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": [{ "role": "user", "content": user }],
        "temperature": 0,
    });
    let mut req = client
        .post(anthropic_messages_url(base_url))
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&body);
    if !api_key.trim().is_empty() {
        req = req.header("x-api-key", api_key.trim());
    }
    let resp = req
        .send()
        .await
        .context("post anthropic messages")?
        .error_for_status()
        .context("anthropic messages status")?;
    let value: Value = resp.json().await.context("parse anthropic messages json")?;
    anthropic_messages_text(&value).context("no text block in anthropic messages response")
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Anthropic 地址走 x-api-key + anthropic-version，非 Anthropic 地址走 Bearer。
    #[test]
    fn anthropic_auth_branch_matches_only_official_host() {
        assert!(is_anthropic_api_url(
            &reqwest::Url::parse("https://api.anthropic.com/models").unwrap()
        ));
        assert!(is_anthropic_api_url(
            &reqwest::Url::parse("https://api.anthropic.com/v1/models").unwrap()
        ));
        assert!(is_anthropic_api_url(
            &reqwest::Url::parse("https://API.ANTHROPIC.COM/models").unwrap()
        ));
        assert!(!is_anthropic_api_url(
            &reqwest::Url::parse("https://api.openai.com/v1/models").unwrap()
        ));
        assert!(!is_anthropic_api_url(
            &reqwest::Url::parse("https://anthropic.example.com/models").unwrap()
        ));
        assert!(!is_anthropic_api_url(
            &reqwest::Url::parse("http://127.0.0.1:8000/v1/models").unwrap()
        ));
    }

    /// Anthropic 官方端点判定：仅 api.anthropic.com 主机走 x-api-key 鉴权。
    #[test]
    fn is_anthropic_endpoint_matches_only_official_host() {
        assert!(is_anthropic_endpoint("https://api.anthropic.com"));
        assert!(is_anthropic_endpoint("https://api.anthropic.com/v1"));
        assert!(is_anthropic_endpoint("https://API.ANTHROPIC.COM"));
        assert!(!is_anthropic_endpoint("https://api.openai.com/v1"));
        assert!(!is_anthropic_endpoint("https://anthropic.example.com"));
        assert!(!is_anthropic_endpoint("http://127.0.0.1:8000/v1"));
        assert!(!is_anthropic_endpoint("not a url"));
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

    /// 模型探测地址：带 `/v1` 结尾保持既有行为；不带时不补 `/v1`，直接拼 `/models`
    /// （glm `/paas/v4`、火山方舟 `/api/v3`、gemini `/v1beta/openai` 的 `/models` 均存在）。
    #[test]
    fn models_probe_url_appends_models_without_extra_v1() {
        assert_eq!(
            models_probe_url("http://127.0.0.1:8000/v1"),
            "http://127.0.0.1:8000/v1/models"
        );
        assert_eq!(
            models_probe_url("http://127.0.0.1:8000/v1/"),
            "http://127.0.0.1:8000/v1/models"
        );
        assert_eq!(
            models_probe_url("https://open.bigmodel.cn/api/paas/v4"),
            "https://open.bigmodel.cn/api/paas/v4/models"
        );
        assert_eq!(
            models_probe_url("https://ark.cn-beijing.volces.com/api/v3"),
            "https://ark.cn-beijing.volces.com/api/v3/models"
        );
        assert_eq!(
            models_probe_url("https://generativelanguage.googleapis.com/v1beta/openai"),
            "https://generativelanguage.googleapis.com/v1beta/openai/models"
        );
        assert_eq!(
            models_probe_url("https://api.anthropic.com"),
            "https://api.anthropic.com/models"
        );
    }

    /// Messages 地址：`/v1` 结尾直接拼 `/messages`；裸上游补 `/v1/messages`。
    #[test]
    fn anthropic_messages_url_appends_v1_when_missing() {
        assert_eq!(
            anthropic_messages_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            anthropic_messages_url("https://api.anthropic.com/"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            anthropic_messages_url("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            anthropic_messages_url("https://api.anthropic.com/v1/"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    /// 响应文本提取：拼接 text 块、跳过非文本块；无文本块 / 坏形状返回 None。
    #[test]
    fn anthropic_messages_text_joins_text_blocks() {
        let v = serde_json::json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "..."},
                {"type": "text", "text": "{\"a\":"},
                {"type": "text", "text": "1}"}
            ]
        });
        assert_eq!(anthropic_messages_text(&v).as_deref(), Some("{\"a\":1}"));
        assert!(anthropic_messages_text(&serde_json::json!({"content": []})).is_none());
        assert!(
            anthropic_messages_text(
                &serde_json::json!({"content": [{"type": "thinking", "thinking": "..."}]})
            )
            .is_none()
        );
        assert!(anthropic_messages_text(&serde_json::json!({})).is_none());
    }

    // —— 本地服务类型探测（本地 HTTP mock，无外部依赖）——

    /// Probe tests share process-level global state (PROBE_KIND_CACHE /
    /// PROBE_KIND_INFLIGHT) and each resets it via clear_probe_kind_cache():
    /// in parallel they would tear down each other's in-flight registrations
    /// (the merged run double-probes and the abort test's registration misses
    /// its window). These tests must run serially.
    static PROBE_STATE_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// 极简本地 HTTP server：按请求路径前缀返回固定 JSON，未注册路径返回 404。
    /// 给 probe_local_server_kind / fetch_v1_models 提供真实 HTTP 往返，
    /// 覆盖探测命中与失败回落路径。
    struct MockProbeServer {
        url: String,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for MockProbeServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn spawn_probe_server(routes: Vec<(&'static str, &'static str)>) -> MockProbeServer {
        spawn_auth_probe_server(routes, None).await
    }

    /// Same as [`spawn_probe_server`], but when `required_bearer` is present
    /// every 200 route requires `Authorization: Bearer <required_bearer>`
    /// (401 otherwise), simulating a vLLM `--api-key` authenticated endpoint.
    async fn spawn_auth_probe_server(
        routes: Vec<(&'static str, &'static str)>,
        required_bearer: Option<&'static str>,
    ) -> MockProbeServer {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let required_bearer = required_bearer.map(str::to_string);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 4096];
                let Ok(n) = stream.read(&mut buf).await else {
                    continue;
                };
                if n == 0 {
                    continue;
                }
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/");
                // Header name parsed case-insensitively (hyper HTTP/1.1
                // serializes it as lowercase authorization).
                let authorization = req.lines().find_map(|l| {
                    let (name, value) = l.split_once(':')?;
                    if !name.trim().eq_ignore_ascii_case("authorization") {
                        return None;
                    }
                    value.trim().strip_prefix("Bearer ").map(str::trim)
                });
                let unauthorized = required_bearer
                    .as_deref()
                    .is_some_and(|expected| authorization != Some(expected));
                let (status, body) = match routes.iter().find(|(p, _)| path.starts_with(p)) {
                    Some((_, b)) if unauthorized => (401, r#"{"error":"unauthorized"}"#),
                    Some((_, b)) => (200, *b),
                    None => (404, r#"{"error":"not found"}"#),
                };
                let resp = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        MockProbeServer {
            url: format!("http://{addr}/v1"),
            task,
        }
    }

    /// Ollama signature endpoint hit: /api/tags returns a model list →
    /// identified as Ollama (kind probing only checks /api/tags and does not
    /// fetch /api/ps; loaded state belongs to probe_ollama_models).
    #[tokio::test]
    async fn probe_local_kind_detects_ollama_via_api_tags() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        let server = spawn_probe_server(vec![(
            "/api/tags",
            r#"{"models":[{"name":"qwen3:8b"},{"name":"deepseek-r1:14b"}]}"#,
        )])
        .await;
        assert_eq!(
            probe_local_server_kind(&server.url, None).await,
            LocalServerKind::Ollama
        );
    }

    /// LM Studio 原生端点命中：/api/tags 404 → /api/v0/models 返回 loaded 模型
    /// → 判定 LM Studio。
    #[tokio::test]
    async fn probe_local_kind_detects_lmstudio_via_v0_models() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        let server = spawn_probe_server(vec![(
            "/api/v0/models",
            r#"{"data":[{"id":"local-model","state":"loaded"}]}"#,
        )])
        .await;
        assert_eq!(
            probe_local_server_kind(&server.url, None).await,
            LocalServerKind::LmStudio
        );
    }

    /// vLLM 命中：前两个特征端点 404 → /v1/models 中 owned_by == "vllm" → 判定
    /// vLLM。同时覆盖 fetch_v1_models 对带 /v1 后缀 base_url 的 URL 拼接。
    #[tokio::test]
    async fn probe_local_kind_detects_vllm_via_owned_by() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        let server = spawn_probe_server(vec![(
            "/v1/models",
            r#"{"object":"list","data":[{"id":"qwen3.6-35b","owned_by":"vllm"}]}"#,
        )])
        .await;
        assert_eq!(
            probe_local_server_kind(&server.url, None).await,
            LocalServerKind::Vllm
        );
    }

    /// 全失败回落：所有特征端点 404 → Generic（探测失败不改变 wire route）。
    #[tokio::test]
    async fn probe_local_kind_falls_back_to_generic_when_all_endpoints_404() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        let server = spawn_probe_server(vec![]).await;
        assert_eq!(
            probe_local_server_kind(&server.url, None).await,
            LocalServerKind::Generic
        );
    }

    /// KoboldCpp signature hit: the result field of /api/extra/version
    /// contains "koboldcpp" (case-insensitive) → identified as KoboldCpp.
    #[tokio::test]
    async fn probe_local_kind_detects_koboldcpp_via_extra_version() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        let server = spawn_probe_server(vec![(
            "/api/extra/version",
            r#"{"result":"KoboldCpp 1.74","version":"1.74"}"#,
        )])
        .await;
        assert_eq!(
            probe_local_server_kind(&server.url, None).await,
            LocalServerKind::KoboldCpp
        );
    }

    /// llama.cpp signature hit: /props contains both
    /// default_generation_settings and total_slots → identified as LlamaCpp;
    /// missing either field is not a hit (loose parsing prevents
    /// misidentification).
    #[tokio::test]
    async fn probe_local_kind_detects_llamacpp_via_props() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        let server = spawn_probe_server(vec![(
            "/props",
            r#"{"default_generation_settings":{"n_ctx":4096},"total_slots":1}"#,
        )])
        .await;
        assert_eq!(
            probe_local_server_kind(&server.url, None).await,
            LocalServerKind::LlamaCpp
        );
        clear_probe_kind_cache();
        // Missing total_slots: incomplete shape, not LlamaCpp (all other endpoints 404 → Generic).
        let partial = spawn_probe_server(vec![(
            "/props",
            r#"{"default_generation_settings":{"n_ctx":4096}}"#,
        )])
        .await;
        assert_eq!(
            probe_local_server_kind(&partial.url, None).await,
            LocalServerKind::Generic
        );
    }

    /// Priority: KoboldCpp is also compatible with llama.cpp's /props, so
    /// when both signatures hit it must be identified as KoboldCpp
    /// (KoboldCpp ranks above LlamaCpp).
    #[tokio::test]
    async fn probe_local_kind_koboldcpp_wins_over_llamacpp_props() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        let server = spawn_probe_server(vec![
            (
                "/api/extra/version",
                r#"{"result":"koboldcpp-1.74","version":"1.74"}"#,
            ),
            (
                "/props",
                r#"{"default_generation_settings":{},"total_slots":1}"#,
            ),
        ])
        .await;
        assert_eq!(
            probe_local_server_kind(&server.url, None).await,
            LocalServerKind::KoboldCpp
        );
    }

    /// SGLang signature hit: /get_server_info is a large serialized
    /// ServerArgs JSON; loose parsing only checks that the version string
    /// field exists → identified as Sglang.
    #[tokio::test]
    async fn probe_local_kind_detects_sglang_via_server_info() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        let server = spawn_probe_server(vec![(
            "/get_server_info",
            r#"{"model_path":"qwen3","version":"0.4.9","max_total_num_tokens":32768,"internal_states":[{"memory_usage":0.5}]}"#,
        )])
        .await;
        assert_eq!(
            probe_local_server_kind(&server.url, None).await,
            LocalServerKind::Sglang
        );
    }

    /// LMDeploy hit: owned_by == "lmdeploy" in /v1/models (case-insensitive,
    /// medium-confidence signature). Shares the same /v1/models fetch with
    /// vLLM; LMDeploy takes priority.
    #[tokio::test]
    async fn probe_local_kind_detects_lmdeploy_via_owned_by() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        let server = spawn_probe_server(vec![(
            "/v1/models",
            r#"{"object":"list","data":[{"id":"internlm3-8b","owned_by":"LMDeploy"}]}"#,
        )])
        .await;
        assert_eq!(
            probe_local_server_kind(&server.url, None).await,
            LocalServerKind::LmDeploy
        );
    }

    /// Docker Model Runner port gate: /models returns a JSON array
    /// (management API shape) but the port is not 12434 → the endpoint is
    /// not probed and DockerModelRunner is not selected (falls to Generic).
    #[tokio::test]
    async fn probe_local_kind_docker_model_runner_gated_by_port_12434() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        // The mock binds a random port (never the 12434 gate value, so probing is skipped).
        let server = spawn_probe_server(vec![("/models", r#"[{"id":"qwen3"}]"#)]).await;
        assert_eq!(
            probe_local_server_kind(&server.url, None).await,
            LocalServerKind::Generic,
            "non-12434 ports must not attempt the Docker Model Runner management API"
        );
    }

    /// Port-gate pure function: only an effective port == 12434 allows
    /// probing; no explicit port falls back to the protocol default
    /// (http=80) and misses; invalid URLs miss.
    #[test]
    fn docker_model_runner_port_gate_requires_12434() {
        assert!(is_docker_model_runner_port("http://localhost:12434/v1"));
        assert!(is_docker_model_runner_port(
            "http://host.docker.internal:12434/engines/v1"
        ));
        assert!(!is_docker_model_runner_port("http://127.0.0.1:11434/v1"));
        assert!(!is_docker_model_runner_port("http://localhost/v1"));
        assert!(!is_docker_model_runner_port("not a url"));
    }

    /// DMR management API address normalization: the management endpoint
    /// lives at the host root `/models`; `/engines/v1`, `/engines`, `/v1`
    /// (in this order, so `/engines/v1` does not leave `/engines` behind)
    /// and the bare host all normalize to the host root.
    #[test]
    fn docker_model_runner_root_strips_openai_prefixes() {
        assert_eq!(
            strip_docker_model_runner_root("http://host.docker.internal:12434/engines/v1")
                .as_deref(),
            Some("http://host.docker.internal:12434")
        );
        assert_eq!(
            strip_docker_model_runner_root("http://localhost:12434/engines").as_deref(),
            Some("http://localhost:12434")
        );
        assert_eq!(
            strip_docker_model_runner_root("http://localhost:12434/v1").as_deref(),
            Some("http://localhost:12434")
        );
        assert_eq!(
            strip_docker_model_runner_root("http://localhost:12434/v1/").as_deref(),
            Some("http://localhost:12434")
        );
        assert_eq!(
            strip_docker_model_runner_root("http://localhost:12434").as_deref(),
            Some("http://localhost:12434")
        );
        assert_eq!(
            strip_docker_model_runner_root("http://localhost:12434/").as_deref(),
            Some("http://localhost:12434")
        );
    }

    /// Ordering decision (pure function, no real port-12434 binding needed):
    /// DMR exposes an Ollama-compatible /api/tags, so within the port gate a
    /// management API shape hit must beat Ollama and select
    /// DockerModelRunner; on a shape miss or a non-gated port, the original
    /// order applies and it falls to Ollama.
    #[test]
    fn select_local_server_kind_docker_model_runner_wins_over_ollama_when_gated() {
        let both_hit = || ProbeCandidateHits {
            docker_port_gated: true,
            docker_mgmt_shape: true,
            ollama: true,
            lmstudio_v0: false,
            koboldcpp: false,
            llamacpp: false,
            sglang: false,
            v1_models: None,
        };
        assert_eq!(
            select_local_server_kind(both_hit()),
            LocalServerKind::DockerModelRunner,
            "a management API shape hit on port 12434 should beat Ollama"
        );
        // Management API shape miss: continue in the original order; the Ollama signature wins.
        assert_eq!(
            select_local_server_kind(ProbeCandidateHits {
                docker_mgmt_shape: false,
                ..both_hit()
            }),
            LocalServerKind::Ollama
        );
        // Non-12434 port: the management shape is not trusted (the probe itself was skipped by the gate).
        assert_eq!(
            select_local_server_kind(ProbeCandidateHits {
                docker_port_gated: false,
                ..both_hit()
            }),
            LocalServerKind::Ollama
        );
    }

    /// Authenticated-endpoint probing (round-6 P1 regression): vLLM
    /// `--api-key` returns 401 on `/v1/models` without credentials. Probing
    /// with an inference-same-origin key → vllm identified correctly; no key
    /// → 401 falls to generic. Positive results are cached by base_url and
    /// credentials never enter the cache key (a positive identification
    /// presupposes successful auth, so the result is credential-independent;
    /// no secrets in cache or logs).
    #[tokio::test]
    async fn probe_local_kind_sends_bearer_for_authenticated_vllm() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        clear_probe_kind_cache();
        let server = spawn_auth_probe_server(
            vec![(
                "/v1/models",
                r#"{"object":"list","data":[{"id":"qwen3.6-35b","owned_by":"vllm"}]}"#,
            )],
            Some("sk-local-secret"),
        )
        .await;
        // With the correct key: vllm identified.
        assert_eq!(
            probe_local_server_kind(&server.url, Some("sk-local-secret")).await,
            LocalServerKind::Vllm
        );
        // No key (pre-fix behavior): 401 → generic; the authenticated endpoint
        // gets misclassified.
        clear_probe_kind_cache();
        assert_eq!(
            probe_local_server_kind(&server.url, None).await,
            LocalServerKind::Generic
        );
        // A wrong key also 401s → generic (probe-failure semantics, no false
        // identification).
        clear_probe_kind_cache();
        assert_eq!(
            probe_local_server_kind(&server.url, Some("sk-wrong-key")).await,
            LocalServerKind::Generic
        );
        // Credentials never enter the cache key: after a positive
        // identification, a key-less call for the same URL hits the TTL cache
        // and returns vllm directly without another request (a positive
        // identification presupposes successful auth, so the result is
        // credential-independent).
        clear_probe_kind_cache();
        assert_eq!(
            probe_local_server_kind(&server.url, Some("sk-local-secret")).await,
            LocalServerKind::Vllm
        );
        assert_eq!(
            probe_local_server_kind(&server.url, None).await,
            LocalServerKind::Vllm
        );
        clear_probe_kind_cache();
    }

    /// A blank bearer counts as credential-less (apply_bearer trims then
    /// filters): an auth-free service is unaffected by a blank key and the
    /// Ollama identification still works.
    #[tokio::test]
    async fn probe_local_kind_blank_bearer_is_treated_as_unauthenticated() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        clear_probe_kind_cache();
        // Auth-free Ollama: a blank key is equivalent to None; identification
        // works normally.
        let open_server =
            spawn_probe_server(vec![("/api/tags", r#"{"models":[{"name":"qwen3:8b"}]}"#)]).await;
        assert_eq!(
            probe_local_server_kind(&open_server.url, Some("   ")).await,
            LocalServerKind::Ollama
        );
        clear_probe_kind_cache();
    }

    /// fetch_v1_models 的 URL 拼接：不带 /v1 后缀的 base_url 补 /v1/models；
    /// 带 /v1（含尾斜杠）直接拼 /models。两种形态都应命中同一 mock 路由。
    #[tokio::test]
    async fn fetch_v1_models_joins_url_with_and_without_v1_suffix() {
        let server = spawn_probe_server(vec![("/v1/models", r#"{"data":[]}"#)]).await;
        let base = server.url.trim_end_matches("/v1").to_string();
        assert!(
            fetch_v1_models(&base, None).await.is_some(),
            "无 /v1 后缀应补 /v1/models"
        );
        assert!(
            fetch_v1_models(&server.url, None).await.is_some(),
            "带 /v1 后缀应拼 /models"
        );
        assert!(
            fetch_v1_models(&format!("{base}/v1/"), None)
                .await
                .is_some(),
            "带 /v1/ 尾斜杠同样命中"
        );
    }

    /// TTL 缓存：同一 base_url 的探测结果缓存 60s，第二次调用不再发请求。
    /// mock server 每次响应后关闭连接，若缓存失效第二次调用会因服务已关而
    /// 落到 Generic——缓存命中则保持第一次的 Ollama 判定。
    #[tokio::test]
    async fn probe_local_kind_caches_result_per_base_url() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        clear_probe_kind_cache();
        let server =
            spawn_probe_server(vec![("/api/tags", r#"{"models":[{"name":"qwen3:8b"}]}"#)]).await;
        let first = probe_local_server_kind(&server.url, None).await;
        assert_eq!(first, LocalServerKind::Ollama);
        // 第二次调用命中缓存，不再访问已关闭的 server。
        let second = probe_local_server_kind(&server.url, None).await;
        assert_eq!(second, LocalServerKind::Ollama);
        clear_probe_kind_cache();
    }

    /// Generic（探测失败）不写入长缓存：服务从 404（未就绪）变为 Ollama 后，
    /// 下一次调用应立即重探并拿到新结果，不被 60s TTL 钉死在 Generic。
    #[tokio::test]
    async fn probe_local_kind_does_not_cache_generic_result() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        clear_probe_kind_cache();
        // 空 mock：所有特征端点 404 → Generic。
        let server = spawn_probe_server(vec![]).await;
        let base = server.url.clone();
        assert_eq!(
            probe_local_server_kind(&base, None).await,
            LocalServerKind::Generic
        );
        // 换成响应 /api/tags 的 server（同端口不可行，用第二个 server 验证
        // Generic 结果未被缓存的方式：直接查缓存状态）。
        // 简化口径：探测结果为 Generic 时注册表与缓存都不应留有该 key。
        let cache_has_key = PROBE_KIND_CACHE
            .get()
            .and_then(|c| {
                c.lock()
                    .ok()
                    .map(|g| g.contains_key(base.trim_end_matches('/')))
            })
            .unwrap_or(false);
        assert!(!cache_has_key, "Generic 结果不应写入 TTL 缓存");
        clear_probe_kind_cache();
    }

    /// in-flight 合并：并发多次调用同一 base_url 共享一次探测。
    /// mock server 统计 /api/tags 命中次数——合并生效时无论并发多少调用，
    /// each signature endpoint is hit exactly once (candidate probes run in
    /// parallel with no short-circuit, so each endpoint is hit once per probe).
    #[tokio::test]
    async fn probe_local_kind_merges_concurrent_calls_into_one_probe() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        clear_probe_kind_cache();
        use std::sync::atomic::{AtomicUsize, Ordering};
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = r#"{"models":[{"name":"qwen3:8b"}]}"#;
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 4096];
                let Ok(n) = stream.read(&mut buf).await else {
                    continue;
                };
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/");
                if path.starts_with("/api/tags") {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                } else {
                    let resp = "HTTP/1.1 404 OK\r\nContent-Length: 23\r\n\
                                Connection: close\r\n\r\n{\"error\":\"not found\"}";
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
                let _ = stream.shutdown().await;
            }
        });
        let url = format!("http://{addr}/v1");
        // 并发 8 个调用（首中缓存为空，全部走 in-flight 路径）。
        let mut joins = Vec::new();
        for _ in 0..8 {
            let u = url.clone();
            joins.push(tokio::spawn(async move {
                probe_local_server_kind(&u, None).await
            }));
        }
        for j in joins {
            assert_eq!(j.await.unwrap(), LocalServerKind::Ollama);
        }
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "并发调用应合并为一次探测（/api/tags 只命中一次）"
        );
        task.abort();
        clear_probe_kind_cache();
    }

    /// Cancellation-safety regression: after the first in-flight probe is
    /// aborted, the registry must not keep a poisoned entry — the already
    /// subscribed waiter must degrade to a direct probe and return within a
    /// deadline, and the next caller must start a fresh probe; neither may
    /// hang forever (before the fix the waiter's changed() saw neither the
    /// send nor the channel closing).
    ///
    /// The mock server uses a watch gate: before the gate opens all requests
    /// hang (so the first probe parks on HTTP await, ready to be aborted);
    /// after it opens every endpoint returns 404 (direct/re-probes fall to
    /// Generic).
    #[tokio::test]
    async fn probe_kind_inflight_abort_first_caller_unblocks_waiter_and_next_caller() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        clear_probe_kind_cache();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (gate_tx, gate_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move {
            let gate_rx = gate_rx;
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut gate = gate_rx.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let Ok(n) = stream.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    // Gate closed: park the request, simulating a slow endpoint
                    // so the first probe stops on HTTP await.
                    if !*gate.borrow_and_update() {
                        let _ = gate.changed().await;
                    }
                    let body = r#"{"error":"not found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        let url = format!("http://{addr}/v1");
        let key = url.trim_end_matches('/').to_string();

        // 1. The first probe enters the in-flight registry (then parks on
        // HTTP await).
        let first_url = url.clone();
        let first = tokio::spawn(async move { probe_local_server_kind(&first_url, None).await });
        let registry = PROBE_KIND_INFLIGHT.get_or_init(Default::default);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !registry.lock().unwrap().contains_key(&key) {
            assert!(
                std::time::Instant::now() < deadline,
                "first probe should complete in-flight registration within the deadline"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // 2. A concurrent waiter subscribes to the in-flight probe.
        let waiter_url = key.clone();
        let waiter = tokio::spawn(async move { probe_local_server_kind(&waiter_url, None).await });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 3. Abort the first probe (simulating a cancelled spawned task); the
        //    awaited handle guarantees the future was dropped (the
        //    deregistration guard has run) before continuing.
        first.abort();
        assert!(
            first.await.is_err(),
            "aborted first-probe task should end as cancelled"
        );

        // 4. Open the mock gate: subsequent direct/re-probe requests 404
        // immediately.
        gate_tx.send(true).unwrap();

        // 5. The waiter must not hang forever: it observes the channel
        // closing, degrades to a direct probe, and returns Generic within the
        // deadline.
        let waiter_kind = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("after aborting the first probe, the subscribed waiter should degrade to a direct probe and return within the deadline")
            .unwrap();
        assert_eq!(waiter_kind, LocalServerKind::Generic);

        // 6. The next caller must not hit the poisoned entry: it starts a
        // fresh probe and returns within the deadline.
        let next_url = key.clone();
        let next_kind = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::spawn(async move { probe_local_server_kind(&next_url, None).await }),
        )
        .await
        .expect("after aborting the first probe, the next caller should finish within the deadline (no registry leftover)")
        .unwrap();
        assert_eq!(next_kind, LocalServerKind::Generic);

        // 7. The registry must not keep the key in the end.
        assert!(
            !registry.lock().unwrap().contains_key(&key),
            "registry must not keep a poisoned entry after abort"
        );
        task.abort();
        clear_probe_kind_cache();
    }

    /// Mixed-credential concurrency regression: after a credential-less First
    /// broadcasts Generic, a waiter holding the correct credentials that
    /// subscribed to the same in-flight probe must not accept it as-is
    /// (before the fix the broadcast value was reused and misclassified as
    /// Generic — the authenticated endpoint would have identified the correct
    /// key); it must re-probe directly with its own credentials.
    ///
    /// Mock server behavior: /api/* requests without the correct credentials
    /// hang first (keeping the First in-flight so the waiter can subscribe),
    /// then fall back as unauthorized once the gate opens; /api/tags with the
    /// correct credentials returns the Ollama model list immediately.
    #[tokio::test]
    async fn probe_kind_inflight_waiter_reprobes_generic_broadcast_from_other_credentials() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        clear_probe_kind_cache();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        const GOOD_KEY: &str = "sk-correct";
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (gate_tx, gate_rx) = tokio::sync::watch::channel(false);
        let good_key_for_task = GOOD_KEY.to_string();
        let task = tokio::spawn(async move {
            let good_key = good_key_for_task;
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut gate = gate_rx.clone();
                let good_key = good_key.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let Ok(n) = stream.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");
                    // Header name parsed case-insensitively (hyper HTTP/1.1
                    // serializes it as lowercase authorization).
                    let authorized = req.lines().find_map(|l| {
                        let (name, value) = l.split_once(':')?;
                        if !name.trim().eq_ignore_ascii_case("authorization") {
                            return None;
                        }
                        value.trim().strip_prefix("Bearer ").map(str::trim)
                    }) == Some(good_key.as_str());
                    if !authorized {
                        // Missing/wrong credentials: hang until the gate opens,
                        // keeping the First on the in-flight probe; after the
                        // gate opens still treat as unauthorized → the First
                        // walks every endpoint and lands on Generic.
                        if !*gate.borrow_and_update() {
                            let _ = gate.changed().await;
                        }
                    }
                    let authorized_tags_hit = authorized && path.starts_with("/api/tags");
                    let body = if authorized_tags_hit {
                        r#"{"models":[{"name":"qwen3:8b"}]}"#
                    } else {
                        r#"{"error":"not found"}"#
                    };
                    let status = if authorized_tags_hit {
                        "200 OK"
                    } else {
                        "404 Not Found"
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        let url = format!("http://{addr}/v1");
        let key = url.trim_end_matches('/').to_string();

        // 1. The credential-less caller becomes the First and parks on the
        // hanging HTTP request.
        let first_url = url.clone();
        let first = tokio::spawn(async move { probe_local_server_kind(&first_url, None).await });
        let registry = PROBE_KIND_INFLIGHT.get_or_init(Default::default);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !registry.lock().unwrap().contains_key(&key) {
            assert!(
                std::time::Instant::now() < deadline,
                "first probe should complete in-flight registration within the deadline"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // 2. The caller with correct credentials subscribes to the same
        // in-flight probe.
        let waiter_url = key.clone();
        let waiter_key = GOOD_KEY.to_string();
        let waiter =
            tokio::spawn(
                async move { probe_local_server_kind(&waiter_url, Some(&waiter_key)).await },
            );
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 3. Open the gate: the First's request un-hangs and fails on every
        // endpoint as unauthorized.
        gate_tx.send(true).unwrap();
        let first_kind = tokio::time::timeout(Duration::from_secs(5), first)
            .await
            .expect("First should finish within the deadline after the gate opens")
            .unwrap();
        assert_eq!(
            first_kind,
            LocalServerKind::Generic,
            "credential-less First should fail on every endpoint and fall back to Generic"
        );

        // 4. Regression point: the waiter must not swallow the First's Generic
        // as-is; it must re-probe with its own credentials.
        let waiter_kind = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("waiter should finish the re-probe within the deadline")
            .unwrap();
        assert_eq!(
            waiter_kind,
            LocalServerKind::Ollama,
            "waiter with correct credentials should re-probe its real type with its own credentials, not accept the Generic broadcast by the credential-less First"
        );

        task.abort();
        clear_probe_kind_cache();
    }
}
