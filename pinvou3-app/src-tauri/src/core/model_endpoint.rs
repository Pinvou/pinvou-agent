//! 模型服务端点（URL / 协议）层面的共用判定与直连：连接测试
//! （app/commands/settings.rs）与运行状态探测（features/monitor）都直连
//! `{base}/models`，鉴权方式与探测地址必须同一口径；品悟（features/review）与
//! 记忆回顾（features/memory）选 Anthropic preset 时走 Messages 原生协议，
//! 鉴权与地址口径与上述探测一致。

// R12 去重批次：`reaper`（空闲回收脚手架）与 `test_support`（跨特性测试脚
// 手架）已分别收敛至 `core/reaper.rs` 与 `platform/test_support.rs`，本文件仅保留
// 模型端点共用判定。
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
    /// Entry self-reported per-turn output limit (tokens). `None` = the
    /// endpoint does not declare one (most local engines omit it from
    /// `/v1/models`; Ollama/LM Studio listings do too). Best-effort parse of
    /// `max_output_tokens` / `max_completion_tokens` /
    /// `top_provider.max_completion_tokens` (OpenRouter gateway shape;
    /// unlimited is null, treated as undeclared); when an entry reports
    /// several shapes the tightest value wins. Only used to min-tighten
    /// route declarations, never to raise any limit.
    pub max_output_tokens: Option<u32>,
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

/// u64 JSON number → positive u32. Self-reported values outside the u32
/// range are always treated as undeclared: `as u32` would truncate 2^32 to
/// 0 and 2^32+5 to 5, letting downstream code mistake a corrupted value for
/// a real limit; non-positive values are equally invalid.
fn parse_positive_u32(v: &serde_json::Value) -> Option<u32> {
    u32::try_from(v.as_u64()?).ok().filter(|n| *n > 0)
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
            let max_model_len = item.get("max_model_len").and_then(parse_positive_u32);
            Some(OpenAiModelInfo {
                id: id.to_string(),
                max_model_len,
                max_output_tokens: parse_entry_output_limit(item),
                loaded: None,
            })
        })
        .collect::<Vec<_>>();
    (!models.is_empty()).then_some(models)
}

/// Best-effort extraction of the entry's self-reported per-turn output
/// limit from a `/v1/models` entry. Covers three known shapes: direct
/// `max_output_tokens` / `max_completion_tokens`, and the OpenRouter
/// gateway's `top_provider.max_completion_tokens` (unlimited is null;
/// `as_u64()` missing means undeclared). Every value must be positive,
/// and when several shapes coexist the tightest one wins: preferring a
/// shape by priority could adopt a cap larger than another cap the same
/// entry reported, breaking the only-tighten contract. Local engines
/// usually omit the field → None, and callers must not fabricate a
/// limit from it.
fn parse_entry_output_limit(item: &serde_json::Value) -> Option<u32> {
    let direct = ["max_output_tokens", "max_completion_tokens"]
        .into_iter()
        .filter_map(|key| item.get(key).and_then(parse_positive_u32));
    let top_provider = item
        .get("top_provider")
        .and_then(|provider| provider.get("max_completion_tokens"))
        .and_then(parse_positive_u32);
    direct.chain(top_provider).min()
}

/// 通用 OpenAI 兼容 `/models` 探测。探测地址与云端 probe / 连接测试同一口径
/// （`models_probe_url`）：upstream 不带 `/v1` 也不补——glm `/paas/v4`、火山方舟
/// `/api/v3`、gemini `/v1beta/openai` 的 `/models` 端点均存在，补 `/v1` 会拼成
/// 不存在的地址永远 404。本地候选（vLLM/Ollama/LM Studio）由 discover 统一
/// 归一成 `/v1` 结尾后传入，行为不变。
pub async fn probe_openai_models(base_url: &str) -> Option<OpenAiModelsProbe> {
    let client = shared_probe_client()?;
    let url = models_probe_url(base_url);
    // Same consistency hardening as fetch_v1_models: gateway probes carry the
    // session-affinity header (feature-label key); no-op off-gateway. All
    // current callers are loopback-only, so this is latent-gap hardening.
    let resp = with_opencode_session_header(client.get(url), base_url, "models-probe")
        .send()
        .await
        .ok()?;
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
fn parse_ollama_ps_names(v: &serde_json::Value) -> std::collections::HashSet<String> {
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

/// Ollama `/api/ps` 返回的已加载模型 → 实际生效上下文。`context_length` 是
/// 服务端 `num_ctx`（Modelfile 参数 / `OLLAMA_CONTEXT_LENGTH` / 服务端默认值）
/// 叠加后的最终生效值，即部署真相本身；模型未加载时该接口不包含它。
/// 同一条目的 `name` 与 `model` 两个键都登记（显式 digest 拉取时二者不同）。
/// 非正值 / 越界值按未声明处理（与 `parse_positive_u32` 同口径）。
fn parse_ollama_ps_contexts(v: &serde_json::Value) -> std::collections::HashMap<String, u32> {
    let mut out = std::collections::HashMap::new();
    if let Some(models) = v.get("models").and_then(|m| m.as_array()) {
        for item in models {
            let Some(ctx) = item.get("context_length").and_then(parse_positive_u32) else {
                continue;
            };
            for key in ["name", "model"] {
                if let Some(name) = item.get(key).and_then(|v| v.as_str()).map(str::trim) {
                    if !name.is_empty() {
                        out.insert(name.to_string(), ctx);
                    }
                }
            }
        }
    }
    out
}

/// Ollama 裸名的规范形：模型段（最后一个 `/` 之后）不含 tag（`:`）时为
/// `{name}:latest`，否则原样返回（registry 形如 `host:port/ns/model` 的
/// 主机端口不算 tag）。ps 键匹配（[`ollama_ps_context_lookup`]）与采纳 /
/// 展示两侧的闸门（monitor 的 `adopts_probed_facts`、
/// `native_display_window_adoptable`）必须共用同一口径：同一个裸名在引擎
/// 预算与监控展示两侧都要解析到同一条目。
pub(crate) fn ollama_canonical_name(name: &str) -> String {
    let name_segment = name.rsplit('/').next().unwrap_or(name);
    if name_segment.contains(':') {
        name.to_string()
    } else {
        format!("{name}:latest")
    }
}

/// `/api/ps` 表按配置名取生效上下文，容忍省略 tag 的裸名：Ollama 把裸名
/// 规范化为 `name:latest`（`ollama run llama3` 在 `/api/ps` 里报
/// `llama3:latest`），手敲配置名省略 tag 时按规范化形式补查一次——否则
/// 全局 `OLLAMA_CONTEXT_LENGTH` 部署的真实窗口永远只挂在规范化键下，裸名
/// 路由的首载自愈永远落空（2026-09-30 报告的手打名形态）。tag 判定收敛在
/// [`ollama_canonical_name`]。带 tag 的名字只做精确查找（其规范形即自身）
/// ：tag 不同即不同模型，绝不跨条目借用。
pub(crate) fn ollama_ps_context_lookup(
    contexts: &std::collections::HashMap<String, u32>,
    model: &str,
) -> Option<u32> {
    if let Some(ctx) = contexts.get(model) {
        return Some(*ctx);
    }
    let canonical = ollama_canonical_name(model);
    if canonical != model {
        if let Some(ctx) = contexts.get(&canonical) {
            return Some(*ctx);
        }
    }
    None
}

/// Ollama `/api/show` 单模型上下文事实：仅采信 `parameters` 中的 `num_ctx N`
/// —— Modelfile 显式声明的运行配置，模型加载后即生效值。
///
/// `model_info` 的 `<arch>.context_length` 是 GGUF 训练上下文（能力上限），
/// **不是**运行配置：未声明 `num_ctx` 时 Ollama 按服务端默认值（常见 4096，
/// `OLLAMA_CONTEXT_LENGTH`）运行，该默认值对未加载模型不可探测。采信训练
/// 上限会在常见首启路径高报窗口，放宽 compaction/输入预算后由上游静默截断
/// （低报只是早压缩，高报会让上游截断——保守方向必须拒绝），因此无声明时
/// 返回 None，调用方保留各自的保守兜底；窗口事实在引擎生成时一次性采纳，
/// 模型首次加载后的纠正依赖下次生成（`/api/ps` 生效值）。
///
/// 第一条 `num_ctx` 行即定论（ollama 由 map 生成 `parameters`，一个键一行；
/// 后续行不可能推翻它）：值合法返回 `Some`，畸形（非正 / 越界 / 非数字）
/// 按"声明存在但不可证明"返回 None——后续合法行不追认。
fn parse_ollama_show_context(v: serde_json::Value) -> Option<u32> {
    let params = v.get("parameters").and_then(|v| v.as_str())?;
    for line in params.lines() {
        let mut parts = line.split_whitespace();
        if parts.next() == Some("num_ctx") {
            return parts
                .next()
                .and_then(|n| n.parse::<u64>().ok())
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0);
        }
    }
    None
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
            // The only served-window fact in the native v0 listing is
            // `loaded_context_length` on a loaded entry (real payloads carry
            // it next to the cap, and the two diverge when the model was
            // loaded with a reduced context override — lmstudio-bug-tracker
            // #726: 131072 vs 12918). `max_context_length` is the model's
            // *capability* ("maximum context length supported by the model",
            // lmstudio.ai REST docs; embeddings entries carry it too), the
            // same class refused for Ollama's GGUF trained cap: adopting it
            // would over-report the window whenever the load override is
            // below the cap and let upstream silently truncate. Unloaded and
            // legacy entries therefore stay undeclared.
            let max_model_len = if loaded == Some(true) {
                item.get("loaded_context_length")
                    .and_then(parse_positive_u32)
            } else {
                None
            };
            Some(OpenAiModelInfo {
                id: id.to_string(),
                max_model_len,
                max_output_tokens: None,
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

/// Process-wide HTTP clients shared by probes: probes reuse one connection
/// pool instead of building a client per call. Two semantics aligned with the
/// `features::monitor` probe singleton:
/// 1. The connection pool and proxy config are snapshotted at first build
///    and do not follow system proxy changes within the process;
/// 2. A build failure is cached process-wide as `None` with no per-call
///    retries, preserving the caller-side degradation semantics of
///    "probe failure → fall back to Generic/configured value"
///    (`Client::default()` panics on the same failure and cannot serve as
///    the fallback).
///
/// `timeout` pins the client-level default request timeout. The crate uses
/// exactly two forms and each form has its own singleton (first build wins
/// per form):
/// - `Some(Duration::from_secs(3))` — this module's probes keep their
///   original client-level 3s timeout;
/// - `None` — `features::monitor`'s probes, whose 3s timeout moves to
///   per-request (its 1 Hz polling shares the no-timeout pool).
/// Request-level errors are unaffected and still handled by callers.
pub(crate) fn shared_probe_client_with_timeout(
    timeout: Option<Duration>,
) -> Option<&'static reqwest::Client> {
    static WITH_TIMEOUT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    static WITHOUT_TIMEOUT: std::sync::OnceLock<Option<reqwest::Client>> =
        std::sync::OnceLock::new();
    let build = || {
        let mut builder = reqwest::Client::builder();
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }
        builder.build().ok()
    };
    match timeout {
        // 每种形态各自一个单例：core（客户端级 3s）与 monitor（无客户端级
        // 超时、逐请求 3s）不会互相污染对方的超时语义。
        Some(_) => WITH_TIMEOUT.get_or_init(build).as_ref(),
        None => WITHOUT_TIMEOUT.get_or_init(build).as_ref(),
    }
}

/// 本 crate 自有探测（连接测试 / 本地服务类型 / served-name / 模型列表）
/// 共享的连接池：客户端级默认超时保持原有的 3 秒。
fn shared_probe_client() -> Option<&'static reqwest::Client> {
    shared_probe_client_with_timeout(Some(Duration::from_secs(3)))
}

/// 探测 Ollama：区分"已加载"（/api/ps）与"仅下载未加载"（/api/tags）。
/// 两个接口都是只读列表，不会触发加载；绝不能用推理请求探测。
/// `/api/ps` 的 `context_length`（已加载模型的生效上下文）同步填入对应条目的
/// `max_model_len`；未加载模型不补查 `/api/show`（列表探测保持零推理元数据
/// 之外的开销，逐模型事实由 `fetch_ollama_model_context` 按需查询）。
/// See [`apply_bearer`] for `bearer` semantics.
pub async fn probe_ollama_models(
    base_url: &str,
    bearer: Option<&str>,
) -> Option<OpenAiModelsProbe> {
    let host = strip_v1_suffix(base_url)?;
    let client = shared_probe_client()?;
    // 已加载集合与生效上下文表：失败按空（全部未加载、无窗口事实），不影响已下载列表。
    let (loaded_names, ps_contexts) =
        match apply_bearer(client.get(format!("{host}/api/ps")), bearer)
            .send()
            .await
            .and_then(|r| r.error_for_status())
        {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(v) => (parse_ollama_ps_names(&v), parse_ollama_ps_contexts(&v)),
                Err(_) => Default::default(),
            },
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
                    max_model_len: ps_contexts.get(&name).copied(),
                    id: name,
                    max_output_tokens: None,
                    loaded: Some(loaded),
                }
            })
            .collect(),
    })
}

/// 拉取 Ollama `/api/ps` 的已加载模型生效上下文表。返回值三态：
/// `Some(map)` —— 端点以 Ollama 形状应答（`models` 数组存在，可为空）；
/// `None` —— 请求失败 / 非成功状态 / 响应不是 Ollama 形状（非 Ollama 端点）。
/// 调用方以 `Some` 判定"这是 Ollama 形状的原生接口"，再决定是否补查
/// `/api/show`。See [`apply_bearer`] for `bearer` semantics.
pub async fn fetch_ollama_contexts(
    base_url: &str,
    bearer: Option<&str>,
) -> Option<std::collections::HashMap<String, u32>> {
    let host = strip_v1_suffix(base_url)?;
    let client = shared_probe_client()?;
    let resp = apply_bearer(client.get(format!("{host}/api/ps")), bearer)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v = resp.json::<serde_json::Value>().await.ok()?;
    let ollama_shaped = v.get("models").and_then(|m| m.as_array()).is_some();
    ollama_shaped.then(|| parse_ollama_ps_contexts(&v))
}

/// `/api/show` 的探测结果三态：`Declared` —— `parameters` 带可采纳的
/// `num_ctx`；`NoDeclaration` —— 2xx 且 JSON 合形但无 `num_ctx`，或 404
/// （清单里没有这个名字：未下载 / 名称不符——与"未声明"一样是稳定的按名
/// 事实，可负缓存，否则 monitor 1 Hz 轮询会对同一个不存在的名字每秒重发
/// POST）；`Unreachable` —— 传输失败 / 其余非成功状态 / 响应不合形（瞬态，
/// 不缓存，下轮重询）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OllamaShowProbe {
    Declared(u32),
    NoDeclaration,
    Unreachable,
}

/// Ollama `/api/show` 单模型窗口探测（只读元数据查询，不触发模型加载）。
/// 只读事实口径见 [`parse_ollama_show_context`]：仅 Modelfile `num_ctx`
/// 声明，不采信 GGUF 训练上限。
pub(crate) async fn probe_ollama_show_context(
    base_url: &str,
    bearer: Option<&str>,
    model: &str,
) -> OllamaShowProbe {
    let probe = async {
        let host = strip_v1_suffix(base_url)?;
        let client = shared_probe_client()?;
        let resp = apply_bearer(client.post(format!("{host}/api/show")), bearer)
            .json(&serde_json::json!({ "model": model }))
            .send()
            .await
            .ok()?;
        // 404 是稳定的按名事实（清单里没有这个名字），按"未声明"负缓存；
        // 其余非 2xx 视为瞬态。
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Some(None);
        }
        if !resp.status().is_success() {
            return None;
        }
        let v = resp.json::<serde_json::Value>().await.ok()?;
        Some(parse_ollama_show_context(v))
    };
    match probe.await {
        Some(Some(ctx)) => OllamaShowProbe::Declared(ctx),
        Some(None) => OllamaShowProbe::NoDeclaration,
        None => OllamaShowProbe::Unreachable,
    }
}

/// `/api/show` 查询缓存（60s TTL，按 upstream + 模型名）：`/api/show` 每次调用
/// 都重读 GGUF 元数据，monitor 1 Hz 轮询与引擎 spawn / 单发旁路调用共享本缓存，
/// 只有 `/api/ps` 生效值每次现查（一个小本地 GET，跟随加载状态与服务端配置）。
/// 命中与未命中 alike 缓存同一 TTL：无 `num_ctx` 的 show 应答（已下载未加载
/// 模型的默认形态）与声明一样稳定，不缓存则每秒重读 GGUF 直到首次加载。
/// 只缓存合形应答（[`OllamaShowProbe::Declared`] / [`OllamaShowProbe::
/// NoDeclaration`]）；[`OllamaShowProbe::Unreachable`] 是瞬态，不缓存——
/// "服务端正忙"不得在 TTL 内被钉成"无窗口"，代价是 show 持续 5xx/挂起期间
/// 每次轮询都会重发一次 show（一次 GGUF 重读），直到服务端恢复。声明中途
/// 新增 60s 内可见。
///
/// 缓存键刻意只含 URL 不含凭证（与 [`PROBE_KIND_CACHE`] 同一理由）：
/// `num_ctx` 是同一服务端的部署事实，与调用方凭证无关；凭证错误的调用得到
/// 401 → `Unreachable` → 不入缓存，因此跨凭证共享键不会投毒。键取
/// `trim_end_matches('/')` 后的 upstream（与 [`probe_local_server_kind`]
/// 同一口径）：同一 base_url 的带斜杠 / 不带斜杠拼写在请求层等价，不该
/// 在缓存里裂成两个条目。
static OLLAMA_SHOW_CACHE: std::sync::OnceLock<
    std::sync::Mutex<
        std::collections::HashMap<(String, String), (std::time::Instant, Option<u32>)>,
    >,
> = std::sync::OnceLock::new();

const OLLAMA_SHOW_CACHE_TTL: Duration = Duration::from_secs(60);

/// 缓存化的 `/api/show` 查询：瞬态失败不缓存（也不计入 TTL 钉死），
/// 合形的命中 / 未命中共享 60s TTL。
pub(crate) async fn cached_ollama_show_context(
    upstream: &str,
    api_key: Option<&str>,
    name: &str,
) -> Option<u32> {
    let upstream = upstream.trim_end_matches('/');
    let cache = OLLAMA_SHOW_CACHE.get_or_init(Default::default);
    {
        let guard = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, ctx)) = guard.get(&(upstream.to_string(), name.to_string())) {
            if at.elapsed() < OLLAMA_SHOW_CACHE_TTL {
                return *ctx;
            }
        }
    }
    let ctx = match probe_ollama_show_context(upstream, api_key, name).await {
        // 合形应答（命中 / 未命中 alike）缓存 60s；瞬态失败不缓存。
        probe @ (OllamaShowProbe::Declared(_) | OllamaShowProbe::NoDeclaration) => {
            let ctx = match probe {
                OllamaShowProbe::Declared(ctx) => Some(ctx),
                _ => None,
            };
            cache.lock().unwrap_or_else(|p| p.into_inner()).insert(
                (upstream.to_string(), name.to_string()),
                (std::time::Instant::now(), ctx),
            );
            ctx
        }
        OllamaShowProbe::Unreachable => None,
    };
    ctx
}

/// 仅测试用：清空 show 缓存，避免 TTL 命中污染 mock 调用计数 / 跨用例状态
/// （与 [`clear_probe_kind_cache`] 同位）。
#[cfg(test)]
pub(crate) fn clear_ollama_show_cache() {
    if let Some(cache) = OLLAMA_SHOW_CACHE.get() {
        if let Ok(mut guard) = cache.lock() {
            guard.clear();
        }
    }
}

/// 仅测试用：把 show 缓存中的现有条目回拨到 TTL 之外，让"到期后必须真正
/// 重查"可以在不 sleep 的情况下断言（删除 `elapsed < TTL` 检查 = 缓存
/// 永不过期、改过的 `num_ctx` 到重启前都不再重读——该回归只能由本助手
/// 揭红）。
#[cfg(test)]
pub(crate) fn age_ollama_show_cache_beyond_ttl() {
    let Some(cache) = OLLAMA_SHOW_CACHE.get() else {
        return;
    };
    let Ok(mut guard) = cache.lock() else {
        return;
    };
    let past = std::time::Instant::now()
        .checked_sub(OLLAMA_SHOW_CACHE_TTL + Duration::from_secs(1))
        .expect("monotonic clock far enough past boot to backdate a 60s TTL");
    for entry in guard.values_mut() {
        entry.0 = past;
    }
}

/// 单模型上下文事实，按可信度排序：`/api/ps` 的生效值（模型已加载时即部署
/// 真相，[`ollama_ps_context_lookup`] 容忍裸名省略 tag）→ `/api/show` 的
/// Modelfile `num_ctx` 显式声明（60s 缓存，monitor 与引擎共享；GGUF 重读是
/// 缓存的全部理由）。全部失败返回 None，调用方保留既有兜底。Ollama 的
/// OpenAI 兼容 `/v1/models` 从不携带窗口事实，这是唯一的事实来源；缺失时
/// foundation 对 unknown ollama 模型按 8192 兜底窗口推导预算（压缩阈值打到
/// 4096 地板、压缩后输入预算只剩 1024，见 2026-09-30 用户报告）。
pub(crate) async fn fetch_ollama_model_context(
    base_url: &str,
    bearer: Option<&str>,
    model: &str,
) -> Option<u32> {
    if let Some(ctx) = fetch_ollama_contexts(base_url, bearer)
        .await
        .and_then(|contexts| ollama_ps_context_lookup(&contexts, model))
    {
        return Some(ctx);
    }
    cached_ollama_show_context(base_url, bearer, model).await
}

/// LM Studio 已加载条目的 served window（`/api/v0/models` 的
/// `loaded_context_length`，lmstudio-bug-tracker #726：与能力上限
/// `max_context_length` 在缩窗加载时真实分歧——131072 vs 12918）。未加载 /
/// 列表中无此名（精确匹配）返回 None；能力上限拒绝口径见
/// [`parse_lmstudio_v0_models`]。
pub async fn fetch_lmstudio_served_context(
    base_url: &str,
    bearer: Option<&str>,
    model: &str,
) -> Option<u32> {
    // 与判别探测共用同一 v0 取数路径（同一 URL / bearer / 合形口径），
    // 避免两处实现漂移后"判别是 LM Studio、取数却不认"的分歧。
    probe_lmstudio_v0_only(base_url, bearer)
        .await?
        .models
        .into_iter()
        .find(|entry| entry.id == model)?
        .max_model_len
}

/// 按 server kind 取原生 served window 的统一分发（引擎两处调用——spawn
/// 采纳与复用重查——共用）：外层 `Some` = 该 kind 有原生 API 可问
/// （Ollama / LM Studio），内层 = 它是否给出了事实；`None` = 该 kind 没有
/// 原生 API（vLLM 等的窗口事实在 listing 里，原生跟进只会是每轮一次注定
/// 404 的 `/api/ps`）。monitor 的展示跟进保留自己的分发：从未判别过的
/// monitor-only 目标要求 `/api/ps` 应答非 Ollama 形状即止（不付 show
/// 兜底 POST），与引擎的"已确认 Ollama kind"语义不同，见
/// `features::monitor::model_probe::local_native_display_window`。
pub(crate) async fn fetch_native_served_context(
    kind: Option<LocalServerKind>,
    base_url: &str,
    bearer: Option<&str>,
    model: &str,
) -> Option<Option<u32>> {
    match kind {
        Some(LocalServerKind::Ollama) => {
            Some(fetch_ollama_model_context(base_url, bearer, model).await)
        }
        Some(LocalServerKind::LmStudio) => {
            Some(fetch_lmstudio_served_context(base_url, bearer, model).await)
        }
        _ => None,
    }
}

/// 已缓存的本地服务判别（只读窥视，不发请求）：TTL 内的正向判别结果；
/// 从未探测过（或上次探测失败——Generic 不入长缓存）返回 None。monitor
/// 的原生窗口展示用它决定是否值得跟进：引擎已判别过的非 Ollama / 非
/// LM Studio 端点直接跳过，免掉每轮一次注定 404 的 `/api/ps`。
pub(crate) fn cached_local_server_kind(base_url: &str) -> Option<LocalServerKind> {
    probe_kind_cache_get(base_url.trim_end_matches('/'))
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

/// Whether to treat this base_url as a "local inference service": loopback
/// (localhost / 127.0.0.0/8 / ::1), RFC1918 private ranges (10/8, 172.16/12,
/// 192.168/16), or Docker-specific hostnames (host.docker.internal, etc.).
/// These endpoints usually run on the user's own machine/intranet; probing
/// them is cheap so real thinking tiers can be offered (defaulting to the
/// lowest thinking tier — see `request_reasoning_effort`); public
/// OpenAI-compatible endpoints are excluded (keep the default high).
/// Difference from `base_url_uses_loopback`: the latter is only for the
/// "allow unauthenticated" decision (api_key required), while this decision
/// covers probing and thinking control (LAN vLLM/Ollama also defaults to
/// the lowest thinking tier). Lives in core so the engine bridge, the pool,
/// and the monitor's native-window display gate share one classification
/// (a public base_url saved under a local preset must not receive native
/// probes on either path).
pub(crate) fn base_url_uses_local_or_private(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .is_some_and(|host| {
            let host = host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim_end_matches('.');
            if host.eq_ignore_ascii_case("localhost") {
                return true;
            }
            // Docker Desktop host alias: the common way to reach the host from
            // inside a container.
            if host.eq_ignore_ascii_case("host.docker.internal")
                || host.eq_ignore_ascii_case("host.lima.internal")
                || host.eq_ignore_ascii_case("host.orbstack.internal")
                || host.ends_with(".docker.internal")
            {
                return true;
            }
            let Ok(address) = host.parse::<std::net::IpAddr>() else {
                return false;
            };
            if address.is_loopback() {
                return true;
            }
            // RFC1918 private ranges (10/8, 172.16/12, 192.168/16): std's
            // `Ipv4Addr::is_private` has exactly equivalent semantics, so
            // reuse it directly.
            match address {
                std::net::IpAddr::V4(v4) => v4.is_private(),
                std::net::IpAddr::V6(_) => false,
            }
        })
}

/// OpenCode gateway endpoint detection (opencode.ai/zen/...).
///
/// The Go gateway (`/zen/go/v1`) has enforced the `x-opencode-session`
/// affinity header with HTTP 400 `MissingSessionID` since 2026-09; plain Zen
/// (`/zen/v1`) ignores the header today. Matching the whole `/zen` prefix is
/// deliberate future-proofing so custom endpoints keep working if Zen turns
/// the header on too. The foundation's builtin injection keys only on the
/// OpencodeGo/OpencodeZen provider identities, so requests that reach the
/// gateway through a custom OpenAI-compatible endpoint (provider `openai`)
/// must get the header from the app layer.
pub fn is_opencode_gateway_base_url(base_url: &str) -> bool {
    reqwest::Url::parse(base_url.trim())
        .ok()
        .is_some_and(|url| {
            let host = url
                .host_str()
                .map(|host| host.trim_end_matches('.').to_ascii_lowercase());
            let host_matches =
                host.is_some_and(|host| host == "opencode.ai" || host.ends_with(".opencode.ai"));
            let path = url.path();
            host_matches && (path == "/zen" || path.starts_with("/zen/"))
        })
}

/// Stable `x-opencode-session` value per conversation key.
///
/// OpenCode's documented contract is one stable ID per conversation. The
/// foundation's builtin injection is process-global; this app keys IDs by
/// conversation instead: engine spawns key on the session id (stable across
/// respawns because `EnginePool::prepare_runtime_model` reuses the same
/// session key), auxiliary gateway callers key on the session id when they
/// hold a session-bound bridge and on their feature label otherwise. IDs are
/// derived deterministically — UUID v5 over the conversation key under a
/// fixed application namespace — so one conversation keeps one ID across app
/// restarts and across the engine/auxiliary lanes.
pub fn opencode_session_id_for(conversation_key: &str) -> String {
    uuid::Uuid::new_v5(&OPENCODE_SESSION_NAMESPACE, conversation_key.as_bytes()).to_string()
}

/// Fixed v5 namespace for OpenCode session-affinity IDs: random bytes
/// generated once offline, carrying no secret or identity.
const OPENCODE_SESSION_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_bytes(*b"\xd0\x7d\xd0\x5c\x48\xc1\x4e\x62\x8a\x4b\x6e\x54\xc0\x84\x12\xe2");

/// Attach `x-opencode-session` to an auxiliary reqwest request when the
/// endpoint is an OpenCode gateway; no-op otherwise.
///
/// The engine chat path carries the header through the foundation config
/// (`Pinvou3Bridge::build_dt_config`). Hand-rolled auxiliary clients
/// (memory review, voice postprocess, model review, connection test,
/// image-capability probe, model probe) bypass the foundation config and
/// must attach the header themselves or the Go gateway rejects them with
/// 400 `MissingSessionID`.
pub fn with_opencode_session_header(
    req: reqwest::RequestBuilder,
    base_url: &str,
    conversation_key: &str,
) -> reqwest::RequestBuilder {
    if is_opencode_gateway_base_url(base_url) {
        req.header(
            "x-opencode-session",
            opencode_session_id_for(conversation_key),
        )
    } else {
        req
    }
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
/// Generic misclassification, losing the local default thinking tier and
/// the real effort tiers. Pass `None` for endpoints without auth.
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
/// hung endpoint costs ~3s at worst instead of accumulating serially;
/// `fetch_v1_models`'s 404/405 root fallback adds at most one more request
/// window — a hung primary times out once and is not retried);
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

/// Fetches an OpenAI-compatible model-list response body. URL convention:
/// a configured root already ending in `/v1` appends `/models` directly
/// (same convention as `features::monitor`'s `models_probe_url`); the bare
/// host form (common for local vLLM) gets `/v1/models` appended first. For
/// non-`/v1` version roots (glm `/api/paas/v4`, Volcengine Ark `/api/v3`,
/// etc. — their model list lives at `{base}/models`, and appending `/v1`
/// would 404), retry `{base}/models` once only when the primary candidate
/// clearly reports "path not found" (404/405); auth failures (401/403) are
/// not helped by switching paths, and timeouts/connection refusals mean the
/// host is unreachable — neither is retried, conservatively falling back to
/// no facts. Failure / non-2xx / parse failure returns `None`, and the
/// caller treated as probe failure. Shared with the kind-probe chain
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
    let trimmed = base_url.trim_end_matches('/');
    let (primary, fallback) = if trimmed.ends_with("/v1") {
        (format!("{trimmed}/models"), None)
    } else {
        (
            format!("{trimmed}/v1/models"),
            Some(format!("{trimmed}/models")),
        )
    };
    // The gateway probes carry the same session-affinity header (feature-label
    // key: the probe has no conversation); no-op off-gateway.
    let attach =
        |req: reqwest::RequestBuilder| with_opencode_session_header(req, base_url, "models-probe");
    let resp = attach(apply_bearer(client.get(primary), bearer))
        .send()
        .await
        .ok()?;
    // Retry once only when the primary candidate clearly reports "path not
    // found" (404/405) and a fallback candidate exists; auth failures
    // (401/403) are not helped by switching paths, and timeouts/connection
    // refusals mean the host is unreachable — neither is retried,
    // conservatively falling back to no facts.
    let resp = match (resp.status().as_u16(), fallback) {
        (200..=299, _) => resp,
        (404 | 405, Some(url)) => {
            let resp = attach(apply_bearer(client.get(url), bearer))
                .send()
                .await
                .ok()?;
            if !resp.status().is_success() {
                return None;
            }
            resp
        }
        _ => return None,
    };
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

/// Extracts the top-level `stop_reason` from a Messages response:
/// `max_tokens` means the output was truncated (the counterpart of OpenAI
/// chat/completions' `finish_reason == "length"`); other values include
/// `end_turn` / `stop_sequence`. A missing field (some gateways strip it)
/// yields `None`, and callers treat that as "unknown, never truncated".
pub fn anthropic_messages_stop_reason(v: &Value) -> Option<String> {
    v.get("stop_reason")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Parsed result of a Messages-protocol direct call: the text plus the
/// top-level `stop_reason`, so callers (voice long-draft editing) can block
/// a truncated half-edited draft before it is written back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicCompletion {
    pub text: String,
    pub stop_reason: Option<String>,
}

/// Characters of an error response body kept in the surfaced message before
/// the ellipsis: large enough to carry a gateway's field-level reason (the
/// Kimi Coding Plan 400 named the exact field and allowed value), small
/// enough for a one-line toast.
const ERROR_BODY_SNIPPET_CHARS: usize = 400;

/// Wire-read cap for an error body: the endpoint is user-configurable and
/// the body it returns with an error is endpoint-controlled, so the read
/// itself — not just the surfaced snippet — must be bounded; a broken or
/// hostile 4xx/5xx body can be arbitrarily large or stream forever. Four
/// bytes per char covers the snippet's worst-case UTF-8 width, and +4
/// guarantees at least one byte past the snippet cap's chars whenever the
/// body continues, so "there is more" is decidable without a further read.
const ERROR_BODY_READ_CAP_BYTES: usize = ERROR_BODY_SNIPPET_CHARS * 4 + 4;

/// Error raised by [`error_for_status_with_body`]: the HTTP status plus a
/// trimmed snippet of the body the endpoint returned with it. `Display`
/// carries the full diagnostic message (label, status, snippet); downcast to
/// this type when a call site needs the bare status without the body — e.g.
/// the voice lane reduces frontend-facing diagnostics to error class and
/// status and must not persist endpoint-controlled text.
#[derive(Debug)]
pub struct StatusWithBodyError {
    label: String,
    status: reqwest::StatusCode,
    /// First [`ERROR_BODY_SNIPPET_CHARS`] chars of the error body; `None`
    /// when the body was blank or unreadable.
    body_snippet: Option<String>,
}

impl StatusWithBodyError {
    pub(crate) fn new(
        label: &str,
        status: reqwest::StatusCode,
        body_snippet: Option<String>,
    ) -> Self {
        Self {
            label: label.to_string(),
            status,
            body_snippet,
        }
    }

    /// The HTTP status, for callers that classify errors without the body
    /// (the voice lane's frontend-diagnosis redaction).
    pub(crate) fn status(&self) -> reqwest::StatusCode {
        self.status
    }
}

impl std::fmt::Display for StatusWithBodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.body_snippet {
            Some(snippet) => write!(f, "{}: HTTP {}: {}", self.label, self.status, snippet),
            None => write!(f, "{}: HTTP {}", self.label, self.status),
        }
    }
}

impl std::error::Error for StatusWithBodyError {}

/// `error_for_status` that keeps the server's reason. A bare
/// "HTTP status client error (400 Bad Request)" hides which field the
/// endpoint rejected — the Kimi Coding Plan temperature 400 named the exact
/// field and allowed value in its body ("invalid temperature: only 1 is
/// allowed for this model"), and the one-shot aux callers discarded it, so a
/// one-line toast gave nothing to act on. Read a bounded prefix of the error
/// body and surface a trimmed snippet via [`StatusWithBodyError`]; success
/// responses pass through untouched.
pub async fn error_for_status_with_body(
    resp: reqwest::Response,
    label: &str,
) -> Result<reqwest::Response> {
    let status = resp.status();
    if !status.is_client_error() && !status.is_server_error() {
        return Ok(resp);
    }
    let (body, body_continues) = read_error_body_bounded(resp).await;
    let mut snippet: String = body.chars().take(ERROR_BODY_SNIPPET_CHARS).collect();
    if body_continues || body.chars().skip(ERROR_BODY_SNIPPET_CHARS).next().is_some() {
        snippet.push('…');
    }
    let snippet = if snippet.trim().is_empty() {
        None
    } else {
        Some(snippet)
    };
    Err(StatusWithBodyError::new(label, status, snippet).into())
}

/// Bounded error-body read: stop at [`ERROR_BODY_READ_CAP_BYTES`] bytes or
/// end of stream, whichever comes first, and drop the response — the rest of
/// the body is abandoned and the connection closed, never drained. Decoding
/// is lossy UTF-8, the tolerance the wire text read it replaces had for
/// off-charset bytes. The bool reports that the cap was reached, i.e. the
/// body certainly carries more than [`ERROR_BODY_SNIPPET_CHARS`] chars.
async fn read_error_body_bounded(mut resp: reqwest::Response) -> (String, bool) {
    let mut buf: Vec<u8> = Vec::new();
    while buf.len() < ERROR_BODY_READ_CAP_BYTES {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let take = chunk.len().min(ERROR_BODY_READ_CAP_BYTES - buf.len());
                buf.extend_from_slice(&chunk[..take]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    let hit_cap = buf.len() >= ERROR_BODY_READ_CAP_BYTES;
    (String::from_utf8_lossy(&buf).into_owned(), hit_cap)
}

/// Anthropic Messages 协议直连：x-api-key + anthropic-version 鉴权（官方端点不接受
/// Bearer），`system` 是独立字段而非 messages 首条。Messages API 没有
/// `response_format`，JSON 约束靠 prompt 措辞 + 调用方解析兜底（与既有 chat/completions
/// 路径的 fallback 解析同款）。api_key 为空时不带鉴权头（同连接测试口径）。
/// `conversation_key` keys the OpenCode gateway session-affinity header: the
/// gateway also serves `/v1/messages` (docs/go Endpoints table), so a
/// preset-Anthropic caller pointed at a gateway base_url must carry it too;
/// off-gateway this is a no-op.
pub async fn post_anthropic_messages(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
    conversation_key: &str,
) -> Result<AnthropicCompletion> {
    let req = anthropic_messages_request(
        client,
        base_url,
        api_key,
        model,
        system,
        user,
        max_tokens,
        conversation_key,
    );
    let resp = req.send().await.context("post anthropic messages")?;
    let resp = error_for_status_with_body(resp, "anthropic messages status").await?;
    let value: Value = resp.json().await.context("parse anthropic messages json")?;
    let text =
        anthropic_messages_text(&value).context("no text block in anthropic messages response")?;
    Ok(AnthropicCompletion {
        text,
        stop_reason: anthropic_messages_stop_reason(&value),
    })
}

/// Request builder behind [`post_anthropic_messages`], extracted so tests can
/// assert the header set (gateway vs off-gateway) without a live server.
fn anthropic_messages_request(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
    conversation_key: &str,
) -> reqwest::RequestBuilder {
    // No "temperature": the aux bodies match the main-session engine wire,
    // which sends none (foundation tests pin temperature absence). The
    // hard-coded 0 400s on gateways that pin sampling server-side — the Kimi
    // Coding Plan Messages endpoint sits on the same backend as its
    // chat/completions one, which rejected temperature=0 with "invalid
    // temperature: only 1 is allowed for this model" (live-probed 2026-09-30).
    let body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": [{ "role": "user", "content": user }],
    });
    let mut req = with_opencode_session_header(
        client.post(anthropic_messages_url(base_url)),
        base_url,
        conversation_key,
    )
    .header("anthropic-version", ANTHROPIC_VERSION)
    .json(&body);
    if !api_key.trim().is_empty() {
        req = req.header("x-api-key", api_key.trim());
    }
    req
}

/// Test-only: minimal model-list mock server. Returns a configurable status
/// code and body per path, recording hit counts and Authorization headers —
/// shared by the `fetch_v1_models` fallback-convention tests and the
/// engine_pool spawn adoption wiring tests, asserting "which path was hit,
/// how many times, and whether credentials were attached".
#[cfg(test)]
pub(crate) mod models_mock {
    use std::collections::HashMap;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    pub(crate) struct ModelsMock {
        pub base_url: String,
        hits: Arc<Mutex<HashMap<String, usize>>>,
        auth: Arc<Mutex<HashMap<String, String>>>,
    }

    impl ModelsMock {
        pub(crate) fn hits_for(&self, path: &str) -> usize {
            self.hits
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(path)
                .copied()
                .unwrap_or(0)
        }

        /// Authorization header carried by the last request to this path
        /// (None when absent).
        pub(crate) fn auth_for(&self, path: &str) -> Option<String> {
            self.auth
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(path)
                .cloned()
        }
    }

    pub(crate) fn spawn(routes: &[(&str, u16, String)]) -> ModelsMock {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock listener");
        let addr = listener.local_addr().expect("mock listener addr");
        let routes: Vec<(String, u16, String)> = routes
            .iter()
            .map(|(path, status, body)| ((*path).to_string(), *status, body.clone()))
            .collect();
        let hits: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        let hits_thread = hits.clone();
        let auth: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
        let auth_thread = auth.clone();
        std::thread::spawn(move || {
            // Serve at most 32 connections: the core tests each send 1-2
            // requests and the finalize wiring tests also go through the kind
            // probe (7 parallel candidates), so leave headroom; this also
            // bounds thread leaks.
            for stream in listener.incoming().take(32) {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let Ok(n) = std::io::Read::read(&mut stream, &mut chunk) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&buf);
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .split('?')
                    .next()
                    .unwrap_or("")
                    .to_string();
                *hits_thread
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .entry(path.clone())
                    .or_insert(0) += 1;
                let authorization = request
                    .lines()
                    .skip(1)
                    .take_while(|line| !line.is_empty())
                    .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                    .map(|line| {
                        line.split_once(':')
                            .unwrap_or((":", ""))
                            .1
                            .trim()
                            .to_string()
                    });
                if let Some(value) = authorization {
                    auth_thread
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert(path.clone(), value);
                }
                let matched = routes.iter().find(|(route, _, _)| path == *route);
                let (status, body) = matched
                    .map(|(_, status, body)| (*status, body.clone()))
                    .unwrap_or((404, "{}".to_string()));
                let reason = match status {
                    200 => "OK",
                    401 => "Unauthorized",
                    404 => "Not Found",
                    _ => "OK",
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
            }
        });
        ModelsMock {
            base_url: format!("http://{addr}"),
            hits,
            auth,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::platform::paths::tests::ENV_LOCK;

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

    /// The three known shapes of a `/v1/models` entry's self-reported output
    /// limit + rejection of invalid values. Local engines (vLLM/Ollama)
    /// usually omit the field → None; it must not be fabricated.
    #[test]
    fn parse_models_response_list_extracts_output_limits() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"object":"list","data":[
                {"id":"direct-max-output","max_output_tokens":65536},
                {"id":"direct-max-completion","max_completion_tokens":8192},
                {"id":"openrouter-shape","top_provider":{"max_completion_tokens":131072,"context_length":1000000}},
                {"id":"unlimited-null","top_provider":{"max_completion_tokens":null}},
                {"id":"zero-invalid","max_output_tokens":0},
                {"id":"plain-vllm","max_model_len":262144}
            ]}"#,
        )
        .unwrap();
        let models = parse_models_response_list(json).unwrap();
        let output_of = |id: &str| {
            models
                .iter()
                .find(|m| m.id == id)
                .unwrap()
                .max_output_tokens
        };
        assert_eq!(output_of("direct-max-output"), Some(65536));
        assert_eq!(output_of("direct-max-completion"), Some(8192));
        assert_eq!(output_of("openrouter-shape"), Some(131072));
        // unlimited (null) and 0 are both treated as undeclared
        assert_eq!(output_of("unlimited-null"), None);
        assert_eq!(output_of("zero-invalid"), None);
        // max_model_len is a context window, not an output limit
        assert_eq!(output_of("plain-vllm"), None);
    }

    /// A single entry may report several output-cap shapes at once (a
    /// gateway mirroring both the OpenAI and OpenRouter fields). The
    /// adopted limit is the minimum of every valid value, so it never
    /// exceeds any cap the endpoint declared; invalid shapes are ignored
    /// and must not shadow the remaining valid one.
    #[test]
    fn parse_models_response_list_conflicting_output_caps_takes_the_tightest() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"object":"list","data":[
                {"id":"all-three","max_output_tokens":65536,"max_completion_tokens":4096,
                 "top_provider":{"max_completion_tokens":8192}},
                {"id":"direct-and-null-provider","max_output_tokens":16384,
                 "top_provider":{"max_completion_tokens":null}},
                {"id":"invalid-plus-valid","max_output_tokens":0,"max_completion_tokens":2048}
            ]}"#,
        )
        .unwrap();
        let models = parse_models_response_list(json).unwrap();
        let output_of = |id: &str| {
            models
                .iter()
                .find(|m| m.id == id)
                .unwrap()
                .max_output_tokens
        };
        assert_eq!(output_of("all-three"), Some(4096));
        assert_eq!(output_of("direct-and-null-provider"), Some(16384));
        assert_eq!(output_of("invalid-plus-valid"), Some(2048));
    }

    /// Parse robustness: self-reported values beyond u32 (`as u32` would
    /// truncate them to 0 or a small value, mistaking corrupted values for
    /// real limits), float/negative/string forms are all treated as
    /// undeclared; max_model_len 0 and out-of-range values are likewise
    /// rejected.
    #[test]
    fn parse_models_response_list_rejects_out_of_range_and_malformed_values() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"object":"list","data":[
                {"id":"u64-overflow","max_output_tokens":4294967296},
                {"id":"u64-overflow-plus","max_completion_tokens":4294967301},
                {"id":"top-provider-overflow","top_provider":{"max_completion_tokens":4294967296}},
                {"id":"float-form","max_output_tokens":65536.0},
                {"id":"negative","max_output_tokens":-1},
                {"id":"string-form","max_output_tokens":"65536"},
                {"id":"zero-context","max_model_len":0},
                {"id":"overflow-context","max_model_len":4294967296}
            ]}"#,
        )
        .unwrap();
        let models = parse_models_response_list(json).unwrap();
        for id in [
            "u64-overflow",
            "u64-overflow-plus",
            "top-provider-overflow",
            "float-form",
            "negative",
            "string-form",
        ] {
            let model = models.iter().find(|m| m.id == id).unwrap();
            assert_eq!(
                model.max_output_tokens, None,
                "{id} must be treated as undeclared"
            );
        }
        let zero_context = models.iter().find(|m| m.id == "zero-context").unwrap();
        assert_eq!(zero_context.max_model_len, None, "a 0 window is not a fact");
        let overflow_context = models.iter().find(|m| m.id == "overflow-context").unwrap();
        assert_eq!(
            overflow_context.max_model_len, None,
            "a window beyond u32 must not be truncated into a fake value"
        );
    }

    #[test]
    fn ollama_ps_names_collects_loaded_models() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"models":[{"name":"qwen3:8b","model":"qwen3:8b","size_vram":5000000000},{"model":"deepseek-r1:14b"}]}"#,
        )
        .unwrap();
        let names = parse_ollama_ps_names(&json);
        assert!(names.contains("qwen3:8b"));
        assert!(names.contains("deepseek-r1:14b")); // 缺 name 时回退 model 字段
        assert!(!names.contains("llama3.2:3b"));
        // 坏形状按空集（宁全标未加载，不错标已加载）
        assert!(parse_ollama_ps_names(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn ollama_ps_contexts_maps_effective_context_per_loaded_model() {
        // Real /api/ps shape (ollama ≥0.6, api/types.go ProcessModelResponse):
        // context_length is the effective runtime context after num_ctx /
        // OLLAMA_CONTEXT_LENGTH / server default composition.
        let json: serde_json::Value = serde_json::from_str(
            r#"{"models":[
                {"name":"qwen3:32b","model":"qwen3:32b","size_vram":1,"context_length":131072},
                {"name":"mproj:7b","model":"mproj:7b-instruct","context_length":8192},
                {"name":"legacy","context_length":0},
                {"name":"huge","context_length":4294967296},
                {"name":"no-field"}
            ]}"#,
        )
        .unwrap();
        let contexts = parse_ollama_ps_contexts(&json);
        assert_eq!(contexts.get("qwen3:32b"), Some(&131_072));
        // Both name and model keys are registered when they differ.
        assert_eq!(contexts.get("mproj:7b"), Some(&8192));
        assert_eq!(contexts.get("mproj:7b-instruct"), Some(&8192));
        // 0 / out-of-u32-range / missing are undeclared, never fabricated.
        assert!(!contexts.contains_key("legacy"));
        assert!(!contexts.contains_key("huge"));
        assert!(!contexts.contains_key("no-field"));
        // Bad shape → empty map (callers fall through to /api/show).
        assert!(parse_ollama_ps_contexts(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn ollama_show_context_is_num_ctx_declaration_only() {
        // A Modelfile num_ctx declaration is the effective value once the
        // model loads — it outranks the GGUF trained context.
        let declared: serde_json::Value = serde_json::from_str(
            r#"{"parameters":"stop <|im_end|>\nnum_ctx 131072\ntop_p 0.9",
                "model_info":{"general.architecture":"qwen3","qwen3.context_length":40960}}"#,
        )
        .unwrap();
        assert_eq!(parse_ollama_show_context(declared), Some(131_072));
        // Without a num_ctx declaration the response yields no fact at all:
        // `model_info`'s `<arch>.context_length` is the GGUF *trained* cap
        // (a capability ceiling), while an unloaded model actually serves at
        // the server default (OLLAMA_CONTEXT_LENGTH, 4096-class) — a value no
        // API exposes. Adopting the cap would loosen compaction budgets
        // beyond what the server accepts, so it is refused; callers keep
        // their conservative fallback. Arch-matched, unrelated, and
        // minimal-key variants are all refused alike.
        let trained: serde_json::Value = serde_json::from_str(
            r#"{"model_info":{"general.architecture":"llama",
                "llama.context_length":131072,"llama2.context_length":4096}}"#,
        )
        .unwrap();
        assert_eq!(parse_ollama_show_context(trained), None);
        let ambiguous: serde_json::Value = serde_json::from_str(
            r#"{"model_info":{"a.context_length":8192,"b.context_length":4096}}"#,
        )
        .unwrap();
        assert_eq!(parse_ollama_show_context(ambiguous), None);
        // A malformed num_ctx declaration is not a provable runtime config —
        // undeclared, not "fall back to the trained cap". The FIRST num_ctx
        // line decides (ollama generates `parameters` from a map, one line
        // per key): a later valid line does not rehabilitate it, and an
        // out-of-range one is not traded down for a later in-range one.
        let bad_decl: serde_json::Value = serde_json::from_str(
            r#"{"parameters":"num_ctx not-a-number",
                "model_info":{"qwen3.context_length":40960}}"#,
        )
        .unwrap();
        assert_eq!(parse_ollama_show_context(bad_decl), None);
        let bad_then_good: serde_json::Value =
            serde_json::from_str(r#"{"parameters":"num_ctx not-a-number\nnum_ctx 4096"}"#).unwrap();
        assert_eq!(parse_ollama_show_context(bad_then_good), None);
        let overflow_then_good: serde_json::Value =
            serde_json::from_str(r#"{"parameters":"num_ctx 99999999999\nnum_ctx 4096"}"#).unwrap();
        assert_eq!(parse_ollama_show_context(overflow_then_good), None);
        // Two valid lines: the first one decides (a mutation taking the
        // last / max valid line must turn this red).
        let good_then_good: serde_json::Value =
            serde_json::from_str(r#"{"parameters":"num_ctx 4096\nnum_ctx 131072"}"#).unwrap();
        assert_eq!(parse_ollama_show_context(good_then_good), Some(4_096));
        // Absent / invalid shapes → None.
        assert_eq!(parse_ollama_show_context(serde_json::json!({})), None);
        assert_eq!(
            parse_ollama_show_context(serde_json::json!({"model_info":{}})),
            None
        );
        assert_eq!(
            parse_ollama_show_context(serde_json::json!({"model_info":{"x.context_length":0}})),
            None
        );
    }

    #[tokio::test]
    async fn fetch_ollama_model_context_prefers_ps_then_show() {
        // Loaded model: the /api/ps effective value wins and /api/show is not
        // consulted.
        let mock = models_mock::spawn(&[
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"my-model","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        assert_eq!(
            fetch_ollama_model_context(&mock.base_url, None, "my-model").await,
            Some(131_072)
        );
        assert_eq!(mock.hits_for("/api/show"), 0);
        // Not loaded (or pre-0.6 server without the field): the /api/show
        // fact is queried — a Modelfile num_ctx declaration here.
        let mock = models_mock::spawn(&[
            ("/api/ps", 200, r#"{"models":[]}"#.into()),
            (
                "/api/show",
                200,
                r#"{"parameters":"num_ctx 32768","model_info":{"general.architecture":"qwen3","qwen3.context_length":40960}}"#.into(),
            ),
        ]);
        assert_eq!(
            fetch_ollama_model_context(&mock.base_url, None, "my-model").await,
            Some(32_768)
        );
        // Not loaded and no num_ctx declaration (the model_info entry is
        // only the trained cap) → None (callers keep their existing
        // conservative fallbacks).
        let mock = models_mock::spawn(&[
            ("/api/ps", 200, r#"{"models":[]}"#.into()),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        assert_eq!(
            fetch_ollama_model_context(&mock.base_url, None, "gone").await,
            None
        );
        // The bearer credential rides the native probes the same as
        // real inference (authenticated gateways 401 without it).
        let mock = models_mock::spawn(&[
            ("/api/ps", 200, r#"{"models":[]}"#.into()),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let _ = fetch_ollama_model_context(&mock.base_url, Some("secret"), "m").await;
        assert_eq!(mock.auth_for("/api/ps").as_deref(), Some("Bearer secret"));
        assert_eq!(mock.auth_for("/api/show").as_deref(), Some("Bearer secret"));
    }

    #[tokio::test]
    async fn probe_ollama_models_fills_window_for_loaded_entries() {
        // The same /api/ps response that yields the loaded set also carries
        // the effective context; loaded entries get their own window, the
        // downloaded-only ones stay undeclared (no per-model /api/show
        // fan-out in the list probe).
        let mock = models_mock::spawn(&[
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"loaded-131k","context_length":131072}]}"#.into(),
            ),
            (
                "/api/tags",
                200,
                r#"{"models":[{"name":"loaded-131k"},{"name":"jit-only"}]}"#.into(),
            ),
        ]);
        let probe = probe_ollama_models(&mock.base_url, None).await.unwrap();
        let window_of = |id: &str| {
            probe
                .models
                .iter()
                .find(|m| m.id == id)
                .unwrap()
                .max_model_len
        };
        assert_eq!(window_of("loaded-131k"), Some(131_072));
        assert_eq!(window_of("jit-only"), None);
        assert_eq!(
            mock.hits_for("/api/show"),
            0,
            "the list probe reuses its own /api/ps response — no per-model /api/show fan-out"
        );
    }

    /// The `/api/show` cache only caches well-formed responses: transport
    /// failures / non-success statuses are transient and must not pin "no
    /// window" into the 60s TTL (each call re-tries; 404 excepted — a
    /// stable per-name miss, see the next test), while a well-formed miss
    /// is as stable as a declaration and caches for the TTL. Fresh
    /// names + fresh ports per segment keep the shared static cache from
    /// colliding with parallel tests; the clears themselves run under the
    /// crate ENV_LOCK so a concurrent cache-state test (same lock) can
    /// never wipe an entry between this test's count-pinned calls.
    #[tokio::test]
    async fn cached_ollama_show_context_caches_only_well_formed() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _ = clear_ollama_show_cache();
        // Server error → Unreachable → not cached.
        let mock = models_mock::spawn(&[("/api/show", 500, "{}".into())]);
        assert_eq!(
            cached_ollama_show_context(&mock.base_url, None, "unreachable-x").await,
            None
        );
        assert_eq!(
            cached_ollama_show_context(&mock.base_url, None, "unreachable-x").await,
            None
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            2,
            "an unreachable show must not be cached — every call re-tries"
        );
        // Well-formed miss → cached for the TTL (one hit across two calls).
        let mock = models_mock::spawn(&[("/api/show", 200, r#"{"model_info":{}}"#.into())]);
        assert_eq!(
            cached_ollama_show_context(&mock.base_url, None, "well-formed-miss-y").await,
            None
        );
        assert_eq!(
            cached_ollama_show_context(&mock.base_url, None, "well-formed-miss-y").await,
            None
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            1,
            "well-formed misses cache for the TTL"
        );
        let _ = clear_ollama_show_cache();
    }

    /// A 404 `/api/show` is a stable per-name fact (the manifest does not
    /// list this name): cached like a well-formed miss, so a permanently
    /// wrong name does not re-POST once per monitor poll; other non-2xx
    /// stay transient (the 500 segment above). Runs under ENV_LOCK for the
    /// same cross-test reason as the previous test.
    #[tokio::test]
    async fn show_404_is_a_stable_per_name_miss_cached_for_ttl() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _ = clear_ollama_show_cache();
        let mock = models_mock::spawn(&[(
            "/api/show",
            404,
            r#"{"error":"model 'x' not found"}"#.into(),
        )]);
        assert_eq!(
            cached_ollama_show_context(&mock.base_url, None, "absent-model-z").await,
            None
        );
        assert_eq!(
            cached_ollama_show_context(&mock.base_url, None, "absent-model-z").await,
            None
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            1,
            "a 404 is stable per name — served from the TTL cache, not re-POSTed per poll"
        );
        let _ = clear_ollama_show_cache();
    }

    /// TTL 到期必须真正重查：删掉 `elapsed < TTL` 检查（缓存永不过期，
    /// 服务端改过的 `num_ctx` 到重启前都不再重读）时本测试必须变红。
    /// Runs under ENV_LOCK for the same cross-test reason as the count
    /// pins above; aging replaces a sleep so the assertion is exact.
    #[tokio::test]
    async fn cached_ollama_show_context_re_fetches_after_ttl() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _ = clear_ollama_show_cache();
        let mock =
            models_mock::spawn(&[("/api/show", 200, r#"{"parameters":"num_ctx 8192"}"#.into())]);
        assert_eq!(
            cached_ollama_show_context(&mock.base_url, None, "ttl-expiry-x").await,
            Some(8_192)
        );
        age_ollama_show_cache_beyond_ttl();
        assert_eq!(
            cached_ollama_show_context(&mock.base_url, None, "ttl-expiry-x").await,
            Some(8_192),
            "the value must come back fresh, not from the aged entry"
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            2,
            "an expired entry must re-fetch, not serve stale forever"
        );
        let _ = clear_ollama_show_cache();
    }

    /// 缓存键刻意不含凭证（同一服务端的部署事实；错误凭证得到 401 →
    /// `Unreachable` → 不入缓存，跨凭证共享键不会投毒）：换凭证 / 去掉
    /// 凭证都不得裂出第二个缓存条目。
    #[tokio::test]
    async fn cached_ollama_show_context_shares_key_across_credentials() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _ = clear_ollama_show_cache();
        let mock =
            models_mock::spawn(&[("/api/show", 200, r#"{"parameters":"num_ctx 4096"}"#.into())]);
        assert_eq!(
            cached_ollama_show_context(&mock.base_url, Some("k1"), "cred-share-x").await,
            Some(4_096)
        );
        assert_eq!(
            cached_ollama_show_context(&mock.base_url, None, "cred-share-x").await,
            Some(4_096),
            "the credential-free entry must serve the credential-less caller too"
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            1,
            "the cache key must not fork per credential"
        );
        let _ = clear_ollama_show_cache();
    }

    /// 裸名与规范化名同时在 `/api/ps` 表中（服务器同时加载了两个 ref）时
    /// 必须精确命中配置名自己的条目——`:latest` 补查只是裸名的兜底，不是
    /// 优先路径。
    #[test]
    fn ollama_ps_lookup_prefers_exact_over_canonical() {
        let mut contexts = std::collections::HashMap::new();
        contexts.insert("llama3".to_string(), 4_096);
        contexts.insert("llama3:latest".to_string(), 131_072);
        assert_eq!(
            ollama_ps_context_lookup(&contexts, "llama3"),
            Some(4_096),
            "the exact configured-name entry wins over the canonical fallback"
        );
        assert_eq!(
            ollama_ps_context_lookup(&contexts, "llama3:latest"),
            Some(131_072)
        );
    }

    /// Ollama canonicalizes a bare model name to `name:latest` in `/api/ps`
    /// (`ollama run llama3` reports `llama3:latest`), so a hand-typed
    /// tagless configured name must still find its own entry — the exact
    /// shape where a global `OLLAMA_CONTEXT_LENGTH` (the 2026-09-30
    /// report's fatality) only ever shows up under the canonical key and
    /// the first-load self-heal would otherwise never land. A tagged name
    /// matches only itself (a different tag is a different model), and a
    /// registry-path name canonicalizes its model segment only.
    #[tokio::test]
    async fn ollama_ps_lookup_tolerates_tagless_names() {
        // Tagless configured name → canonical ps entry, no show fallback.
        let mock = models_mock::spawn(&[
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"llama3:latest","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        assert_eq!(
            fetch_ollama_model_context(&mock.base_url, None, "llama3").await,
            Some(131_072),
            "the canonical entry answers the tagless configured name"
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            0,
            "the ps fact wins — no show fallback needed"
        );
        // A tagged configured name matches only itself: `llama3:8b` is not
        // `llama3:latest`.
        let mock = models_mock::spawn(&[
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"llama3:latest","context_length":131072}]}"#.into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        assert_eq!(
            fetch_ollama_model_context(&mock.base_url, None, "llama3:8b").await,
            None,
            "a different tag is a different model — no cross-entry borrow"
        );
        // Registry-style path: the model segment (after the last `/`) is
        // what gets the `:latest` canonicalization, not the whole string.
        let mock = models_mock::spawn(&[
            (
                "/api/ps",
                200,
                r#"{"models":[{"name":"example.com/library/qwen3:latest","context_length":40960}]}"#
                    .into(),
            ),
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        assert_eq!(
            fetch_ollama_model_context(&mock.base_url, None, "example.com/library/qwen3").await,
            Some(40_960),
            "registry-path names canonicalize the model segment only"
        );
    }

    /// The show-cache key normalizes the upstream spelling
    /// (`trim_end_matches('/')`, same as the kind cache): both spellings of
    /// one base_url must share a single TTL entry instead of forking the
    /// GGUF re-reads.
    #[tokio::test]
    async fn show_cache_key_ignores_trailing_slash() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _ = clear_ollama_show_cache();
        let mock =
            models_mock::spawn(&[("/api/show", 200, r#"{"parameters":"num_ctx 8192"}"#.into())]);
        let slashed = format!("{}/", mock.base_url);
        assert_eq!(
            cached_ollama_show_context(&slashed, None, "slash-key-a").await,
            Some(8_192)
        );
        assert_eq!(
            cached_ollama_show_context(&mock.base_url, None, "slash-key-a").await,
            Some(8_192),
            "the bare spelling hits the entry the slashed spelling cached"
        );
        assert_eq!(
            mock.hits_for("/api/show"),
            1,
            "both spellings of one base_url share one TTL entry"
        );
        let _ = clear_ollama_show_cache();
    }

    /// LM Studio served window (the `loaded_context_length` of the loaded
    /// entry, exact-id match): the real #726 divergence shape adopts the
    /// served 12918, not the 131072 capability cap; a non-loaded entry does
    /// not lend its `loaded_context_length`, and an absent id is no fact.
    #[tokio::test]
    async fn fetch_lmstudio_served_context_adopts_loaded_entry() {
        let mock = models_mock::spawn(&[(
            "/api/v0/models",
            200,
            r#"{"data":[
                {"id":"loaded-model","state":"loaded","max_context_length":131072,"loaded_context_length":12918},
                {"id":"stale","state":"not-loaded","max_context_length":131072,"loaded_context_length":40960},
                {"id":"plain","state":"loaded"}
            ]}"#
                .into(),
        )]);
        assert_eq!(
            fetch_lmstudio_served_context(&mock.base_url, None, "loaded-model").await,
            Some(12_918),
            "the served window (12918), not the capability cap (131072)"
        );
        assert_eq!(
            fetch_lmstudio_served_context(&mock.base_url, None, "stale").await,
            None,
            "a non-loaded entry must not lend its loaded_context_length"
        );
        assert_eq!(
            fetch_lmstudio_served_context(&mock.base_url, None, "absent").await,
            None,
            "an id the server does not list is no fact"
        );
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
        // Real v0 shape (lmstudio.ai REST docs + lmstudio-bug-tracker #726):
        // every model entry carries `max_context_length` — the model's
        // *capability* ("maximum context length supported by the model"),
        // embeddings included — while only a loaded entry carries the
        // served window `loaded_context_length`, and the two diverge when
        // the model was loaded with a reduced context override.
        let json: serde_json::Value = serde_json::from_str(
            r#"{"object":"list","data":[
                {"id":"qwen3-8b","state":"loaded","max_context_length":131072,"loaded_context_length":40960},
                {"id":"gemma3-4b","type":"vlm","state":"loaded","max_context_length":131072,"loaded_context_length":12918},
                {"id":"deepseek-r1-14b","state":"not-loaded","max_context_length":65536},
                {"id":"stale-window","state":"not-loaded","max_context_length":131072,"loaded_context_length":40960},
                {"id":"legacy-model"},
                {"id":"nomic-embed","type":"embeddings","state":"loaded","max_context_length":2048},
                {"id":"no-loaded-field","state":"loaded","max_context_length":8192}
            ]}"#,
        )
        .unwrap();
        let models = parse_lmstudio_v0_models(&json).unwrap();
        assert_eq!(models.len(), 7);
        assert_eq!(models[0].loaded, Some(true));
        assert_eq!(models[0].max_model_len, Some(40_960));
        // The cap must never win over the served window: a 131072-capable
        // model loaded at 12918 reports 12918 (bug #726's real shape).
        assert_eq!(models[1].loaded, Some(true));
        assert_eq!(models[1].max_model_len, Some(12_918));
        // Unloaded: only the capability cap is available — undeclared.
        assert_eq!(models[2].loaded, Some(false));
        assert_eq!(models[2].max_model_len, None);
        // An unloaded entry leaking a stale `loaded_context_length` (schema
        // drift is exactly what #726 showed) must stay undeclared: the
        // state=="loaded" gate is load-bearing.
        assert_eq!(models[3].loaded, Some(false));
        assert_eq!(models[3].max_model_len, None);
        // 缺 state 字段 = 未知；窗口同样未声明。
        assert_eq!(models[4].loaded, None);
        assert_eq!(models[4].max_model_len, None);
        // Embedding entries carry the capability cap too → still undeclared.
        assert_eq!(models[5].max_model_len, None);
        // A loaded entry without `loaded_context_length` (older server):
        // the cap is not a fallback — undeclared.
        assert_eq!(models[6].max_model_len, None);
        // 空列表 / 坏形状返回 None，调用方回退 OpenAI 兼容探测。
        assert!(parse_lmstudio_v0_models(&serde_json::json!({"data":[]})).is_none());
        assert!(parse_lmstudio_v0_models(&serde_json::json!({})).is_none());
    }

    /// `max_context_length` is LM Studio's capability ceiling, not a served
    /// window (see `parse_lmstudio_v0_models`): the generic `/v1/models`
    /// parser must not adopt it as `max_model_len` — that field feeds route
    /// limits and saved declarations as a deployment fact, a trust class the
    /// cap does not belong to. Only a true served-window key qualifies.
    #[test]
    fn parse_models_response_list_refuses_lmstudio_cap_alias() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"object":"list","data":[
                {"id":"cap-alias","max_context_length":131072},
                {"id":"served-key","max_model_len":4096,"max_context_length":131072}
            ]}"#,
        )
        .unwrap();
        let models = parse_models_response_list(json).unwrap();
        let window_of = |id: &str| models.iter().find(|m| m.id == id).unwrap().max_model_len;
        assert_eq!(
            window_of("cap-alias"),
            None,
            "a capability ceiling must not become a route-limit fact"
        );
        assert_eq!(window_of("served-key"), Some(4_096));
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
    /// its window). These tests must run serially. `pub(crate)` so the
    /// engine-pool wiring tests that drive a real classification battery and
    /// clear the state afterwards take the same serializer — their clear
    /// lands mid-poll of the in-flight tests otherwise.
    pub(crate) static PROBE_STATE_TEST_MUTEX: tokio::sync::Mutex<()> =
        tokio::sync::Mutex::const_new(());

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

    /// TTL cache: probe results are cached per base_url for 60s. The mock counts
    /// /api/tags hits: with the cache active, two calls issue exactly one probe
    /// (the old "server closed" premise was false — the mock keeps accepting, so
    /// the test passed even without the cache; hence the hit-count assertion).
    #[tokio::test]
    async fn probe_local_kind_caches_result_per_base_url() {
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
                }
                let _ = stream.shutdown().await;
            }
        });
        let url = format!("http://{addr}/v1");
        let first = probe_local_server_kind(&url, None).await;
        assert_eq!(first, LocalServerKind::Ollama);
        // Second call hits the cache: same result, and no second probe request.
        let second = probe_local_server_kind(&url, None).await;
        assert_eq!(second, LocalServerKind::Ollama);
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "with the cache active, /api/tags must be probed exactly once"
        );
        task.abort();
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

    #[tokio::test]
    async fn fetch_v1_models_prefers_v1_root_and_does_not_fall_back_on_success() {
        let mock = models_mock::spawn(&[
            ("/v1/models", 200, r#"{"data":[{"id":"served-a"}]}"#.into()),
            ("/models", 200, r#"{"data":[{"id":"root-b"}]}"#.into()),
        ]);
        let value = fetch_v1_models(&mock.base_url, None)
            .await
            .expect("bare-host form hits the primary /v1/models candidate");
        assert_eq!(parse_models_response_list(value).unwrap()[0].id, "served-a");
        assert_eq!(mock.hits_for("/v1/models"), 1);
        assert_eq!(
            mock.hits_for("/models"),
            0,
            "a successful primary must not hit the fallback path"
        );
    }

    #[tokio::test]
    async fn fetch_v1_models_falls_back_to_root_models_on_404() {
        // Non-v1 version roots such as glm /api/paas/v4 or Ark /api/v3:
        // appending /v1 always 404s and the model list lives at
        // {base}/models — the single fallback must hit.
        let mock = models_mock::spawn(&[
            ("/v1/models", 404, "{}".into()),
            ("/models", 200, r#"{"data":[{"id":"glm-4.7"}]}"#.into()),
        ]);
        let value = fetch_v1_models(&mock.base_url, Some("route-key"))
            .await
            .expect("after 404 the fallback to {base}/models must happen");
        assert_eq!(parse_models_response_list(value).unwrap()[0].id, "glm-4.7");
        assert_eq!(mock.hits_for("/v1/models"), 1);
        assert_eq!(mock.hits_for("/models"), 1);
        assert_eq!(
            mock.auth_for("/models").as_deref(),
            Some("Bearer route-key"),
            "the fallback request must carry the same-origin credentials, never silently degrade to anonymous"
        );
    }

    #[tokio::test]
    async fn fetch_v1_models_auth_failure_does_not_retry_alt_path() {
        let mock = models_mock::spawn(&[
            ("/v1/models", 401, "{}".into()),
            ("/models", 200, r#"{"data":[{"id":"x"}]}"#.into()),
        ]);
        assert!(fetch_v1_models(&mock.base_url, None).await.is_none());
        assert_eq!(
            mock.hits_for("/models"),
            0,
            "401 is an auth problem; switching paths cannot help, no retry"
        );
    }

    #[tokio::test]
    async fn fetch_v1_models_v1_shaped_base_has_no_alt_path() {
        // A configured root ending in /v1: the primary candidate is
        // {base}/models == the mock's "/v1/models"; on 404 no second path
        // may ever appear (no fallback beyond /v1/models).
        let mock = models_mock::spawn(&[("/v1/models", 404, "{}".into())]);
        let base = format!("{}/v1", mock.base_url);
        assert!(fetch_v1_models(&base, None).await.is_none());
        assert_eq!(mock.hits_for("/v1/models"), 1);
        assert_eq!(
            mock.hits_for("/models"),
            0,
            "a /v1-shaped configured root has a single candidate, no fallback path"
        );
    }

    /// Messages direct-call parsing: the text and the top-level stop_reason
    /// are surfaced together. `max_tokens` drives the voice long-draft
    /// "truncated, refuse writeback" decision; a missing field (some
    /// gateways strip it) must parse as None rather than failing the whole
    /// response.
    #[tokio::test]
    async fn post_anthropic_messages_surfaces_stop_reason() {
        let client = reqwest::Client::new();
        // 多 text 块拼接 + stop_reason == "max_tokens"（截断）。
        let truncated = models_mock::spawn(&[(
            "/v1/messages",
            200,
            r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-x","content":[{"type":"text","text":"{\"a\":"},{"type":"text","text":"1}"}],"stop_reason":"max_tokens"}"#.into(),
        )]);
        let completion = post_anthropic_messages(
            &client,
            &truncated.base_url,
            "key",
            "claude-x",
            "sys",
            "user",
            64,
            "messages-test",
        )
        .await
        .expect("mock messages response should parse");
        assert_eq!(completion.text, "{\"a\":1}");
        assert_eq!(
            completion.stop_reason.as_deref(),
            Some("max_tokens"),
            "stop_reason=max_tokens must be surfaced so callers can reject truncated output"
        );

        // 正常结束：stop_reason == "end_turn" 也原样带出，调用方与 max_tokens 区分。
        let complete = models_mock::spawn(&[(
            "/v1/messages",
            200,
            r#"{"id":"msg_2","type":"message","role":"assistant","content":[{"type":"text","text":"done"}],"stop_reason":"end_turn"}"#.into(),
        )]);
        let completion = post_anthropic_messages(
            &client,
            &complete.base_url,
            "key",
            "claude-x",
            "sys",
            "user",
            64,
            "messages-test",
        )
        .await
        .expect("mock messages response should parse");
        assert_eq!(completion.text, "done");
        assert_eq!(completion.stop_reason.as_deref(), Some("end_turn"));

        // 旧网关裁掉 stop_reason 字段：解析不受影响，stop_reason 为 None。
        let missing = models_mock::spawn(&[(
            "/v1/messages",
            200,
            r#"{"id":"msg_3","type":"message","role":"assistant","content":[{"type":"text","text":"legacy"}]}"#.into(),
        )]);
        let completion = post_anthropic_messages(
            &client,
            &missing.base_url,
            "key",
            "claude-x",
            "sys",
            "user",
            64,
            "messages-test",
        )
        .await
        .expect("a response without stop_reason must still parse");
        assert_eq!(completion.text, "legacy");
        assert!(
            completion.stop_reason.is_none(),
            "absent stop_reason stays None (unknown, never treated as truncated)"
        );
    }

    /// The Messages-protocol direct call carries the OpenCode gateway
    /// session-affinity header when preset=Anthropic callers point at a
    /// gateway base_url (the gateway serves /v1/messages), and stays clean
    /// off-gateway.
    #[test]
    fn anthropic_messages_request_carries_gateway_header() {
        let client = reqwest::Client::new();
        let request = anthropic_messages_request(
            &client,
            "https://opencode.ai/zen/go/v1",
            "key",
            "claude-x",
            "sys",
            "user",
            64,
            "memory-review",
        )
        .build()
        .expect("request builds");
        assert_eq!(
            request.headers().get("x-opencode-session"),
            Some(
                &opencode_session_id_for("memory-review")
                    .parse()
                    .expect("valid header value")
            ),
            "gateway /v1/messages request must carry the conversation-keyed header"
        );
        assert_eq!(
            request.url().path(),
            "/zen/go/v1/messages",
            "the gateway URL must keep the documented /messages suffix"
        );

        let request = anthropic_messages_request(
            &client,
            "https://api.anthropic.com/v1",
            "key",
            "claude-x",
            "sys",
            "user",
            64,
            "memory-review",
        )
        .build()
        .expect("request builds");
        assert!(
            request.headers().get("x-opencode-session").is_none(),
            "off-gateway /v1/messages request must stay clean"
        );
    }

    #[test]
    fn anthropic_messages_body_omits_temperature() {
        let client = reqwest::Client::new();
        let request = anthropic_messages_request(
            &client,
            "https://api.anthropic.com/v1",
            "key",
            "claude-x",
            "sys",
            "user",
            64,
            "memory-review",
        )
        .build()
        .expect("request builds");
        let bytes = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .expect("json body is buffered");
        let body: Value = serde_json::from_slice(bytes).expect("body is json");
        assert!(
            body.get("temperature").is_none(),
            "aux bodies must mirror the engine wire (no temperature): a hard-coded \
             0 400s on sampling-pinned gateways (Kimi Coding Plan, 2026-09-30)"
        );
        assert_eq!(body["model"], "claude-x");
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["system"], "sys");
    }

    /// Pin the call site, not just the builder: the pin above goes red when
    /// `anthropic_messages_request` itself changes, but a re-inlined body
    /// inside `post_anthropic_messages` would bypass it silently. The
    /// request region must build through the shared builder and must not
    /// carry a sampling parameter.
    #[test]
    fn post_anthropic_messages_call_site_builds_body_through_builder() {
        let source = include_str!("model_endpoint.rs");
        let start = source
            .find("pub async fn post_anthropic_messages")
            .expect("post_anthropic_messages definition present");
        let region = &source[start
            ..source[start..]
                .find("fn anthropic_messages_request")
                .expect("anthropic_messages_request definition present")
                + start];
        assert!(
            region.contains("anthropic_messages_request("),
            "post_anthropic_messages must build its body via \
             anthropic_messages_request"
        );
        assert!(
            !region.contains("\"temperature\""),
            "the Messages request region must not re-inline a temperature field"
        );
    }

    #[tokio::test]
    async fn status_error_surfaces_response_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = r#"{"error":{"message":"invalid temperature: only 1 is allowed for this model","type":"invalid_request_error"}}"#;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/chat/completions"))
            .json(&serde_json::json!({"model": "k3"}))
            .send()
            .await
            .expect("request sends");
        let err = error_for_status_with_body(resp, "chat/completions status")
            .await
            .expect_err("400 must error");
        let msg = format!("{err:#}");
        assert!(
            msg.starts_with("chat/completions status: HTTP 400 Bad Request: "),
            "unexpected error shape: {msg}"
        );
        assert!(
            msg.contains("invalid temperature: only 1 is allowed"),
            "error must carry the server's reason: {msg}"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn status_success_passes_response_through() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = r#"{"choices":[{"message":{"content":"ok"}}]}"#;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/chat/completions"))
            .json(&serde_json::json!({"model": "k3"}))
            .send()
            .await
            .expect("request sends");
        let resp = error_for_status_with_body(resp, "chat/completions status")
            .await
            .expect("2xx passes through");
        let value: Value = resp.json().await.expect("body still readable");
        assert_eq!(value["choices"][0]["message"]["content"], "ok");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn status_error_without_body_reports_status_only() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let resp = "HTTP/1.1 500 Internal Server Error\r\n\
                        Content-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/chat/completions"))
            .json(&serde_json::json!({"model": "k3"}))
            .send()
            .await
            .expect("request sends");
        let err = error_for_status_with_body(resp, "anthropic messages status")
            .await
            .expect_err("500 must error");
        let msg = format!("{err:#}");
        assert_eq!(
            msg,
            "anthropic messages status: HTTP 500 Internal Server Error"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn status_error_snippet_is_capped_with_ellipsis() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        // The tail marker sits past the snippet cap, so surfacing it (or
        // dropping the ellipsis) means the cap or its branch regressed.
        let payload = format!("{}TAIL_MARKER", "x".repeat(ERROR_BODY_SNIPPET_CHARS + 100));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/chat/completions"))
            .json(&serde_json::json!({"model": "k3"}))
            .send()
            .await
            .expect("request sends");
        let err = error_for_status_with_body(resp, "chat/completions status")
            .await
            .expect_err("400 must error");
        let msg = err.to_string();
        let snippet = msg
            .strip_prefix("chat/completions status: HTTP 400 Bad Request: ")
            .expect("message keeps the label/status prefix");
        assert!(
            snippet.ends_with('…'),
            "capped snippet must carry the ellipsis: {snippet}"
        );
        assert_eq!(
            snippet.chars().count(),
            // Literal, not ERROR_BODY_SNIPPET_CHARS + 1: this pin exists to
            // make the snippet cap's value a reviewed change, so the test
            // must not inherit whatever the constant currently says.
            401,
            "snippet = 400 body chars + ellipsis"
        );
        assert!(
            !msg.contains("TAIL_MARKER"),
            "body past the cap must not leak into the message: {msg}"
        );
        server.await.unwrap();
    }

    /// The wire read itself must be bounded, not just the surfaced snippet:
    /// the endpoint is user-configurable and the error body is
    /// endpoint-controlled, so a broken or hostile 4xx/5xx may declare a
    /// huge body. The server writes far more than the snippet cap needs; the
    /// helper must stop reading early enough that the server's writes fail
    /// behind the closed socket. An unbounded read would drain the whole
    /// body, the write loop would run to completion, and the final assert
    /// would go red.
    #[tokio::test]
    async fn status_error_read_stops_well_before_oversized_body_ends() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        const TOTAL_BYTES: usize = 64 * 1024 * 1024;
        const CHUNK_BYTES: usize = 64 * 1024;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let head = format!(
                "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\n\
                 Content-Length: {TOTAL_BYTES}\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let chunk = vec![b'a'; CHUNK_BYTES];
            let mut client_went_away = false;
            for _ in 0..TOTAL_BYTES / CHUNK_BYTES {
                if stream.write_all(&chunk).await.is_err() {
                    client_went_away = true;
                    break;
                }
            }
            client_went_away
        });
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/chat/completions"))
            .json(&serde_json::json!({"model": "k3"}))
            .send()
            .await
            .expect("request sends");
        let err = tokio::time::timeout(
            Duration::from_secs(30),
            error_for_status_with_body(resp, "chat/completions status"),
        )
        .await
        .expect("helper must not wait out the oversized body")
        .expect_err("500 must error");
        let snippet = err
            .to_string()
            .strip_prefix("chat/completions status: HTTP 500 Internal Server Error: ")
            .expect("message keeps the label/status prefix")
            .to_string();
        assert_eq!(snippet.chars().count(), 401, "snippet stays at the cap");
        let client_went_away = tokio::time::timeout(Duration::from_secs(30), server)
            .await
            .expect("server finishes instead of blocking on a full body")
            .expect("server task joins");
        assert!(
            client_went_away,
            "client must stop reading well before the declared body ends"
        );
    }

    /// Same bound for the undeclared-length case: a chunked error body that
    /// never ends must not hang the helper or keep the read going past the
    /// snippet cap. The server keeps chunking until its writes fail, so a
    /// client that drains the body would hang until the helper's timeout
    /// below and the test would go red there.
    #[tokio::test]
    async fn status_error_read_is_bounded_on_endless_chunked_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let head = "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\n\
                        Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes()).await;
            let chunk_body = vec![b'b'; 64 * 1024];
            let mut frame = format!("{:x}\r\n", chunk_body.len()).into_bytes();
            frame.extend_from_slice(&chunk_body);
            frame.extend_from_slice(b"\r\n");
            loop {
                if stream.write_all(&frame).await.is_err() {
                    break;
                }
            }
        });
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/chat/completions"))
            .json(&serde_json::json!({"model": "k3"}))
            .send()
            .await
            .expect("request sends");
        let err = tokio::time::timeout(
            Duration::from_secs(30),
            error_for_status_with_body(resp, "chat/completions status"),
        )
        .await
        .expect("helper must not wait out the endless body")
        .expect_err("500 must error");
        let msg = err.to_string();
        assert!(
            msg.ends_with('…'),
            "capped snippet must carry the ellipsis: {msg}"
        );
        server.await.unwrap();
    }
}
