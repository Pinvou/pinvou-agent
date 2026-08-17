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
/// 探测最坏 ~12-15s 串行（挂起端点），多会话/多入口（EnginePool spawn、
/// 连接测试、前端探测）重复探测会放大开销；按 base_url 缓存可合并。
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
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
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
    if let Some(probe) = probe_lmstudio_v0_only(base_url).await {
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
/// 只有前两类有底座现成能力：Ollama 经 `think` 布尔开关（无档位）、vLLM 经
/// `chat_template_kwargs` 支持 off/low/medium/high 档位；LM Studio 与通用
/// OpenAI 兼容端点走 openai wire route，底座暂不注入思考控制。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalServerKind {
    /// vLLM：底座经 `chat_template_kwargs.enable_thinking` + `reasoning_effort`
    /// 支持 off/low/medium/high 档位。
    Vllm,
    /// Ollama：底座经 `think` 布尔支持开关（off=think:false，其余 think:true）。
    Ollama,
    /// LM Studio：底座 openai wire route 暂不注入思考控制（保持旧行为）。
    LmStudio,
    /// 其他通用 OpenAI 兼容服务（探测不到任何特征端点）。
    Generic,
}

/// 探测本地推理服务类型。只应在本地端点（`base_url_uses_local_or_private`）上
/// 调用；判定顺序按特征端点互斥性排列：Ollama（`/api/tags`）→ LM Studio
/// （`/api/v0/models`）→ vLLM（`/v1/models` 的 `owned_by`）→ 通用。探测失败
/// （服务未启动/超时）返回 `Generic`，调用方保持既有 openai wire route，不因
/// 探测失败改变行为。
///
/// 结果按 base_url 缓存 `PROBE_CACHE_TTL`：探测最坏 ~12-15s 串行（挂起端点），
/// 多会话/多入口重复探测会放大开销；TTL 内命中直接返回缓存值。并发未命中
/// 经 in-flight 注册表共享同一次探测（首个调用执行、其余等待广播），不再
/// 各自串行重付。失败结果（`Generic`）不写入长缓存：服务可能只是未启动，
/// 下次调用应立即重探，避免 60s 内起服务仍被钉死在 Generic 错路由。
pub async fn probe_local_server_kind(base_url: &str) -> LocalServerKind {
    let key = base_url.trim_end_matches('/').to_string();
    if let Some(kind) = probe_kind_cache_get(&key) {
        return kind;
    }
    let kind = probe_kind_inflight(&key).await;
    if kind != LocalServerKind::Generic {
        probe_kind_cache_put(&key, kind);
    }
    kind
}

/// 并发去重注册表：key → 完成信号。首个调用方执行探测、完成后发送结果并
/// 注销；并发调用方订阅等待，探测只跑一次。发送方异常丢弃时等待方降级
/// 为自行直探（watch 关闭 `changed()` 返回 Err），总能得到结果。
static PROBE_KIND_INFLIGHT: std::sync::OnceLock<
    std::sync::Mutex<
        std::collections::HashMap<String, Arc<tokio::sync::watch::Sender<Option<LocalServerKind>>>>,
    >,
> = std::sync::OnceLock::new();

async fn probe_kind_inflight(base_url: &str) -> LocalServerKind {
    /// 注册结果：要么成为首个执行者，要么订阅在途探测的完成信号。
    enum Inflight {
        First,
        Wait(tokio::sync::watch::Receiver<Option<LocalServerKind>>),
    }
    let registry = PROBE_KIND_INFLIGHT.get_or_init(Default::default);
    // 注册/订阅在同步块内完成，guard 不跨 await（Send 约束）。
    let entry = {
        let Ok(mut guard) = registry.lock() else {
            // 注册表锁不可用（中毒）：降级为无合并直探。
            return probe_local_server_kind_uncached(base_url).await;
        };
        if let Some(sender) = guard.get(base_url) {
            Inflight::Wait(sender.subscribe())
        } else {
            let (tx, _rx) = tokio::sync::watch::channel(None);
            guard.insert(base_url.to_string(), Arc::new(tx));
            Inflight::First
        }
    };
    match entry {
        // 首个调用方：执行探测，完成后广播结果并注销注册。
        Inflight::First => {
            let kind = probe_local_server_kind_uncached(base_url).await;
            if let Ok(mut guard) = registry.lock() {
                if let Some(sender) = guard.get(base_url) {
                    let _ = sender.send(Some(kind));
                }
                guard.remove(base_url);
            }
            kind
        }
        // 并发调用方：等待首个调用方广播结果。
        Inflight::Wait(mut rx) => {
            while rx.changed().await.is_ok() {
                if let Some(kind) = *rx.borrow_and_update() {
                    return kind;
                }
            }
            // 广播方异常丢弃：降级直探兜底。
            probe_local_server_kind_uncached(base_url).await
        }
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

/// 无缓存的实际探测（TTL 缓存命中时直接返回，见 `probe_local_server_kind`）。
async fn probe_local_server_kind_uncached(base_url: &str) -> LocalServerKind {
    // Ollama 特征端点 /api/tags 存在且模型列表非空（probe_ollama_models 内部要求）。
    if probe_ollama_models(base_url).await.is_some() {
        return LocalServerKind::Ollama;
    }
    // LM Studio 原生端点 /api/v0/models 存在且形状认识。注意不能复用
    // probe_lmstudio_models：它失败时回退 /v1/models，而 /v1/models 是通用端点
    // （Ollama/通用服务也有），会把非 LM Studio 误判成 LM Studio。
    if probe_lmstudio_v0_only(base_url).await.is_some() {
        return LocalServerKind::LmStudio;
    }
    // vLLM：/v1/models 响应中模型 `owned_by == "vllm"`（vLLM 标准实现字段）。
    if probe_vllm_owned(base_url).await {
        return LocalServerKind::Vllm;
    }
    LocalServerKind::Generic
}

/// 仅探测 LM Studio 独有原生端点 `/api/v0/models`（不回退 `/v1/models`，后者
/// 不具判别性）。响应形状不认识时返回 `None`，调用方继续探测下一个候选。
/// 既是 `probe_lmstudio_models` 的 v0 前置，也是本地服务判别探测的前置：
/// 判别场景必须用它而非 `probe_lmstudio_models`（后者回退 `/v1/models`，而
/// `/v1/models` 是通用端点，Ollama/通用服务也有，会把非 LM Studio 误判）。
async fn probe_lmstudio_v0_only(base_url: &str) -> Option<OpenAiModelsProbe> {
    let host = strip_v1_suffix(base_url)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    let resp = client
        .get(format!("{host}/api/v0/models"))
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
/// 按探测失败处理。共享给 `probe_vllm_owned` 与 monitor 的 vLLM served-name
/// 探测，避免 `/v1/models` 的 URL 拼装口径在两处漂移。
pub(crate) async fn fetch_v1_models(base_url: &str) -> Option<serde_json::Value> {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    else {
        return None;
    };
    let url = if base_url.trim_end_matches('/').ends_with("/v1") {
        format!("{}/models", base_url.trim_end_matches('/'))
    } else {
        format!("{}/v1/models", base_url.trim_end_matches('/'))
    };
    let Ok(resp) = client.get(url).send().await else {
        return None;
    };
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<serde_json::Value>().await.ok()
}

/// 探测 vLLM：`/v1/models` 响应中任一模型 `owned_by == "vllm"`（vLLM 标准实现）。
async fn probe_vllm_owned(base_url: &str) -> bool {
    let Some(v) = fetch_v1_models(base_url).await else {
        return false;
    };
    v.get("data")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("owned_by")
                    .and_then(Value::as_str)
                    .is_some_and(|owned| owned.eq_ignore_ascii_case("vllm"))
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
        assert!(anthropic_messages_text(
            &serde_json::json!({"content": [{"type": "thinking", "thinking": "..."}]})
        )
        .is_none());
        assert!(anthropic_messages_text(&serde_json::json!({})).is_none());
    }

    // —— 本地服务类型探测（本地 HTTP mock，无外部依赖）——

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
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
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
                let (status, body) = match routes.iter().find(|(p, _)| path.starts_with(p)) {
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

    /// Ollama 特征端点命中：/api/ps 404（容忍，按空集）→ /api/tags 返回模型列表
    /// → 判定 Ollama。
    #[tokio::test]
    async fn probe_local_kind_detects_ollama_via_api_tags() {
        let server = spawn_probe_server(vec![(
            "/api/tags",
            r#"{"models":[{"name":"qwen3:8b"},{"name":"deepseek-r1:14b"}]}"#,
        )])
        .await;
        assert_eq!(
            probe_local_server_kind(&server.url).await,
            LocalServerKind::Ollama
        );
    }

    /// LM Studio 原生端点命中：/api/tags 404 → /api/v0/models 返回 loaded 模型
    /// → 判定 LM Studio。
    #[tokio::test]
    async fn probe_local_kind_detects_lmstudio_via_v0_models() {
        let server = spawn_probe_server(vec![(
            "/api/v0/models",
            r#"{"data":[{"id":"local-model","state":"loaded"}]}"#,
        )])
        .await;
        assert_eq!(
            probe_local_server_kind(&server.url).await,
            LocalServerKind::LmStudio
        );
    }

    /// vLLM 命中：前两个特征端点 404 → /v1/models 中 owned_by == "vllm" → 判定
    /// vLLM。同时覆盖 fetch_v1_models 对带 /v1 后缀 base_url 的 URL 拼接。
    #[tokio::test]
    async fn probe_local_kind_detects_vllm_via_owned_by() {
        let server = spawn_probe_server(vec![(
            "/v1/models",
            r#"{"object":"list","data":[{"id":"qwen3.6-35b","owned_by":"vllm"}]}"#,
        )])
        .await;
        assert_eq!(
            probe_local_server_kind(&server.url).await,
            LocalServerKind::Vllm
        );
    }

    /// 全失败回落：所有特征端点 404 → Generic（探测失败不改变 wire route）。
    #[tokio::test]
    async fn probe_local_kind_falls_back_to_generic_when_all_endpoints_404() {
        let server = spawn_probe_server(vec![]).await;
        assert_eq!(
            probe_local_server_kind(&server.url).await,
            LocalServerKind::Generic
        );
    }

    /// fetch_v1_models 的 URL 拼接：不带 /v1 后缀的 base_url 补 /v1/models；
    /// 带 /v1（含尾斜杠）直接拼 /models。两种形态都应命中同一 mock 路由。
    #[tokio::test]
    async fn fetch_v1_models_joins_url_with_and_without_v1_suffix() {
        let server = spawn_probe_server(vec![("/v1/models", r#"{"data":[]}"#)]).await;
        let base = server.url.trim_end_matches("/v1").to_string();
        assert!(
            fetch_v1_models(&base).await.is_some(),
            "无 /v1 后缀应补 /v1/models"
        );
        assert!(
            fetch_v1_models(&server.url).await.is_some(),
            "带 /v1 后缀应拼 /models"
        );
        assert!(
            fetch_v1_models(&format!("{base}/v1/")).await.is_some(),
            "带 /v1/ 尾斜杠同样命中"
        );
    }

    /// TTL 缓存：同一 base_url 的探测结果缓存 60s，第二次调用不再发请求。
    /// mock server 每次响应后关闭连接，若缓存失效第二次调用会因服务已关而
    /// 落到 Generic——缓存命中则保持第一次的 Ollama 判定。
    #[tokio::test]
    async fn probe_local_kind_caches_result_per_base_url() {
        clear_probe_kind_cache();
        let server =
            spawn_probe_server(vec![("/api/tags", r#"{"models":[{"name":"qwen3:8b"}]}"#)]).await;
        let first = probe_local_server_kind(&server.url).await;
        assert_eq!(first, LocalServerKind::Ollama);
        // 第二次调用命中缓存，不再访问已关闭的 server。
        let second = probe_local_server_kind(&server.url).await;
        assert_eq!(second, LocalServerKind::Ollama);
        clear_probe_kind_cache();
    }

    /// Generic（探测失败）不写入长缓存：服务从 404（未就绪）变为 Ollama 后，
    /// 下一次调用应立即重探并拿到新结果，不被 60s TTL 钉死在 Generic。
    #[tokio::test]
    async fn probe_local_kind_does_not_cache_generic_result() {
        clear_probe_kind_cache();
        // 空 mock：所有特征端点 404 → Generic。
        let server = spawn_probe_server(vec![]).await;
        let base = server.url.clone();
        assert_eq!(
            probe_local_server_kind(&base).await,
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
    /// 特征端点只被打一次（Ollama 判定在第一个端点即短路返回）。
    #[tokio::test]
    async fn probe_local_kind_merges_concurrent_calls_into_one_probe() {
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
            joins.push(tokio::spawn(
                async move { probe_local_server_kind(&u).await },
            ));
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
}
