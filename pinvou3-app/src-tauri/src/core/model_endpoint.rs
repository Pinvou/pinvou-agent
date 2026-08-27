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

/// 探测请求的鉴权头：与真实推理请求同源的 Bearer key（本地 vLLM `--api-key`
/// 等鉴权端点的 `/v1/models` 也会 401，不带凭据探测会把鉴权端点误判成
/// Generic）。`None`/空白不带鉴权头（默认无鉴权的 Ollama/LM Studio 不受影响，
/// 无鉴权服务会忽略 Bearer 头）。
fn apply_bearer(req: reqwest::RequestBuilder, bearer: Option<&str>) -> reqwest::RequestBuilder {
    match bearer.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => req.bearer_auth(key),
        None => req,
    }
}

/// 探测共用的进程级 HTTP 客户端：各特征端点探测复用同一连接池，不再每次调用
/// 新建客户端（monitor 的 vLLM served-name 探测经 [`fetch_v1_models`] 共享此池）。
/// 与 `features::monitor` 的探测单例同口径的两条语义：
/// 1. 连接池与代理配置在首次构建时快照，进程内不跟随系统代理变化；
/// 2. 构建失败按 `None` 进程级缓存、不做逐次重试，保持调用方
///    "探测失败→回落 Generic/配置值"的降级语义
///    （`Client::default()` 同类失败会 panic，不能作回退）。
/// 单请求超时仍是各探测原有的 3 秒；请求级错误不受影响，仍由调用方处理。
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
/// `bearer` 语义见 [`apply_bearer`]。
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
/// （服务未启动/超时/鉴权 401）返回 `Generic`，调用方保持既有 openai wire
/// route，不因探测失败改变行为。
///
/// `bearer` 是与该端点真实推理请求同源的凭据（见 [`apply_bearer`]）：鉴权端点
/// （vLLM `--api-key`）不带凭据探测必然 401 误判 Generic，丢失默认关思考与
/// 真实档位。无鉴权端点传 `None` 即可。
///
/// 结果按 base_url 缓存 `PROBE_CACHE_TTL`：探测最坏 ~12-15s 串行（挂起端点），
/// 多会话/多入口重复探测会放大开销；TTL 内命中直接返回缓存值。缓存/in-flight
/// 的 key 只含 base_url、绝不包含凭据：正识别（Ollama/vLLM/LM Studio）必然以
/// 鉴权成功为前提，结果与具体凭据无关，按 URL 合并即安全；失败结果（Generic）
/// 不写入长缓存——服务可能只是未启动（或换了 key），下次调用应立即重探，避免
/// 60s 内仍被钉死在 Generic 错路由。并发未命中经 in-flight 注册表共享同一次
/// 探测（首个调用执行、其余等待广播），不再各自串行重付。
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

/// 并发去重注册表：key → 完成信号（Weak，首个调用方持有唯一强引用）。
/// 首个调用方执行探测、完成后发送结果并注销；并发调用方订阅等待，探测只
/// 跑一次。首个调用方被取消（任务 abort / 外层 select 丢弃）时其强引用随
/// future 一起 drop，注册表不延残生命周期：等待方观察到通道关闭（`changed()`
/// 返回 Err）降级为自行直探，后续调用方 upgrade 失败后重新发起探测，总能
/// 得到结果，也不存在永久毒化的注册条目。
static PROBE_KIND_INFLIGHT: std::sync::OnceLock<
    std::sync::Mutex<
        std::collections::HashMap<
            String,
            std::sync::Weak<tokio::sync::watch::Sender<Option<LocalServerKind>>>,
        >,
    >,
> = std::sync::OnceLock::new();

/// in-flight 注册守卫：持有首个调用方 sender 的唯一强引用。future 在 await
/// 探测期间被取消时完成块不会运行，守卫在 Drop 时顺带清掉注册表中仍指向
/// 自己的陈旧 Weak（cleanup 成功与否都不影响正确性——sender 的 drop 本身
/// 就会关闭通道唤醒等待方）。
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
                // 只清理仍指向自己（同指针）的注册，不误删后续调用方重新
                // 发起的新探测。
                if guard
                    .get(&self.key)
                    .is_some_and(|weak| weak.as_ptr() == Arc::as_ptr(&sender))
                {
                    guard.remove(&self.key);
                }
            }
        }
        // drop 唯一强引用 → 通道关闭 → 等待方 changed() 得 Err 降级直探。
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
        // upgrade 失败 = 陈旧 Weak（此前被取消的探测残留）：按无在途处理，
        // 用新注册覆盖。
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
        // 首个调用方：执行探测，完成后广播结果并注销注册。await 期间被取消
        // 时由守卫 drop 强引用关闭通道（见 InflightRegistration）。
        Inflight::First(sender) => {
            let mut registration = InflightRegistration {
                key: base_url.to_string(),
                sender: Some(Arc::clone(&sender)),
            };
            let kind = probe_local_server_kind_uncached(base_url, bearer).await;
            // 正常完成：交出 sender，守卫 Drop 不再重复清理。
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
        // 并发调用方：等待首个调用方广播结果。共享结果有凭据边界：正识别
        // （Vllm/Ollama/LmStudio）以探测请求鉴权成功为前提，是关于服务端
        // 类型本身的结论，与调用方凭据无关，可直接复用；Generic 只说明
        // 首个调用方的凭据上下文内各特征端点均不可达（如鉴权 vLLM 对无/
        // 错凭据一律 401），对持不同凭据的等待方不成立——必须用自身凭据
        // 重新直探，避免「无凭据 First 广播 Generic、正确凭据 Waiter 被
        // 误分类」的串扰（见混合凭据并发回归测试）。
        Inflight::Wait(mut rx) => loop {
            // 订阅时结果可能已广播（send 先于 subscribe）：先查当前值。
            // 先拷贝再匹配，避免 borrow 守卫的临时值跨 await 违反 Send。
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
                // 广播方被取消/异常丢弃：通道关闭（changed() Err），降级直探兜底。
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

/// 无缓存的实际探测（TTL 缓存命中时直接返回，见 `probe_local_server_kind`）。
/// `bearer` 语义见 [`apply_bearer`]。
async fn probe_local_server_kind_uncached(base_url: &str, bearer: Option<&str>) -> LocalServerKind {
    // Ollama 特征端点 /api/tags 存在且模型列表非空（probe_ollama_models 内部要求）。
    if probe_ollama_models(base_url, bearer).await.is_some() {
        return LocalServerKind::Ollama;
    }
    // LM Studio 原生端点 /api/v0/models 存在且形状认识。注意不能复用
    // probe_lmstudio_models：它失败时回退 /v1/models，而 /v1/models 是通用端点
    // （Ollama/通用服务也有），会把非 LM Studio 误判成 LM Studio。
    if probe_lmstudio_v0_only(base_url, bearer).await.is_some() {
        return LocalServerKind::LmStudio;
    }
    // vLLM：/v1/models 响应中模型 `owned_by == "vllm"`（vLLM 标准实现字段）。
    if probe_vllm_owned(base_url, bearer).await {
        return LocalServerKind::Vllm;
    }
    LocalServerKind::Generic
}

/// 仅探测 LM Studio 独有原生端点 `/api/v0/models`（不回退 `/v1/models`，后者
/// 不具判别性）。响应形状不认识时返回 `None`，调用方继续探测下一个候选。
/// 既是 `probe_lmstudio_models` 的 v0 前置，也是本地服务判别探测的前置：
/// 判别场景必须用它而非 `probe_lmstudio_models`（后者回退 `/v1/models`，而
/// `/v1/models` 是通用端点，Ollama/通用服务也有，会把非 LM Studio 误判）。
/// `bearer` 语义见 [`apply_bearer`]。
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
/// 按探测失败处理。共享给 `probe_vllm_owned` 与 monitor 的 vLLM served-name
/// 探测，避免 `/v1/models` 的 URL 拼装口径在两处漂移。`bearer` 语义见
/// [`apply_bearer`]：鉴权 vLLM 的 `/v1/models` 不带凭据会 401。
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

/// 探测 vLLM：`/v1/models` 响应中任一模型 `owned_by == "vllm"`（vLLM 标准实现）。
async fn probe_vllm_owned(base_url: &str, bearer: Option<&str>) -> bool {
    let Some(v) = fetch_v1_models(base_url, bearer).await else {
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
        assert!(
            anthropic_messages_text(
                &serde_json::json!({"content": [{"type": "thinking", "thinking": "..."}]})
            )
            .is_none()
        );
        assert!(anthropic_messages_text(&serde_json::json!({})).is_none());
    }

    // —— 本地服务类型探测（本地 HTTP mock，无外部依赖）——

    /// 探测用例共享进程级全局状态（PROBE_KIND_CACHE / PROBE_KIND_INFLIGHT），
    /// 且各自 clear_probe_kind_cache() 重置：并行时会拆掉对方在途注册
    /// （合并测试出现双探、abort 测试注册限期轮空）。这些用例必须串行。
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

    /// 同 [`spawn_probe_server`]，但 `required_bearer` 存在时所有 200 路由都
    /// 要求 `Authorization: Bearer <required_bearer>`（否则 401），模拟
    /// vLLM `--api-key` 鉴权端点。
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
                // 头名大小写不敏感解析（hyper HTTP/1.1 序列化为小写 authorization）。
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

    /// Ollama 特征端点命中：/api/ps 404（容忍，按空集）→ /api/tags 返回模型列表
    /// → 判定 Ollama。
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

    /// 鉴权端点探测（六审 P1 回归）：vLLM `--api-key` 的 `/v1/models` 不带凭据
    /// 401。带与推理同源的 key 探测 → 正确识别 vllm；不带 key → 401 落
    /// generic。正识别结果按 base_url 缓存且凭据不进缓存 key（正识别必然以
    /// 鉴权成功为前提，结果与具体凭据无关；缓存/日志不落密钥）。
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
        // 带正确 key：识别 vllm。
        assert_eq!(
            probe_local_server_kind(&server.url, Some("sk-local-secret")).await,
            LocalServerKind::Vllm
        );
        // 无 key（修复前的行为）：401 → generic，鉴权端点被误判。
        clear_probe_kind_cache();
        assert_eq!(
            probe_local_server_kind(&server.url, None).await,
            LocalServerKind::Generic
        );
        // 错误 key 同样 401 → generic（探测失败语义，不误识别）。
        clear_probe_kind_cache();
        assert_eq!(
            probe_local_server_kind(&server.url, Some("sk-wrong-key")).await,
            LocalServerKind::Generic
        );
        // 凭据不进缓存 key：正识别后同 URL 的无 key 调用命中 TTL 缓存直接
        // 返回 vllm，不再发请求（正识别以鉴权成功为前提，结果与凭据无关）。
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

    /// 空白 bearer 视同无凭据（apply_bearer trim 后过滤）：无鉴权服务不受
    /// 空白 key 影响，正常识别 Ollama。
    #[tokio::test]
    async fn probe_local_kind_blank_bearer_is_treated_as_unauthenticated() {
        let _state = PROBE_STATE_TEST_MUTEX.lock().await;
        clear_probe_kind_cache();
        // 无鉴权 Ollama：空白 key 与 None 等价，正常识别。
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
    /// 特征端点只被打一次（Ollama 判定在第一个端点即短路返回）。
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

    /// 取消安全回归：首个 in-flight 探测被 abort 后注册表不能残留毒化条目——
    /// 已订阅的等待方须降级直探限期返回，后续调用方须重新发起探测，均不得
    /// 永久挂起（修复前等待方 changed() 既等不到 send 也等不到通道关闭）。
    ///
    /// mock server 用 watch 门闩控制：放行前挂起所有请求（让首探停在 HTTP
    /// await 上以便 abort），放行后对所有端点回 404（直探/重探落 Generic）。
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
                    // 门闩未放行：请求挂起，模拟慢端点让首探停在 HTTP await。
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

        // 1. 首个探测进入 in-flight 注册（注册后停在 HTTP await 上）。
        let first_url = url.clone();
        let first = tokio::spawn(async move { probe_local_server_kind(&first_url, None).await });
        let registry = PROBE_KIND_INFLIGHT.get_or_init(Default::default);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !registry.lock().unwrap().contains_key(&key) {
            assert!(
                std::time::Instant::now() < deadline,
                "首个探测应在限期内完成 in-flight 注册"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // 2. 并发等待方订阅在途探测。
        let waiter_url = key.clone();
        let waiter = tokio::spawn(async move { probe_local_server_kind(&waiter_url, None).await });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 3. abort 首个探测（模拟 spawn 任务被取消）；await 句柄确保 future
        //    已被丢弃（注销守卫已运行）再继续。
        first.abort();
        assert!(first.await.is_err(), "被 abort 的首探任务应以取消告终");

        // 4. 放行 mock：之后的直探/重探请求立即 404。
        gate_tx.send(true).unwrap();

        // 5. 等待方不得永久挂起：观察到通道关闭后降级直探，限期返回 Generic。
        let waiter_kind = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("abort 首探后，已订阅的等待方应在限期内降级直探返回")
            .unwrap();
        assert_eq!(waiter_kind, LocalServerKind::Generic);

        // 6. 后续调用方不得命中毒化条目：重新发起探测并限期返回。
        let next_url = key.clone();
        let next_kind = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::spawn(async move { probe_local_server_kind(&next_url, None).await }),
        )
        .await
        .expect("abort 首探后，后续调用方应在限期内完成（注册表无残留）")
        .unwrap();
        assert_eq!(next_kind, LocalServerKind::Generic);

        // 7. 注册表最终不残留该 key。
        assert!(
            !registry.lock().unwrap().contains_key(&key),
            "abort 后注册表不应残留毒化条目"
        );
        task.abort();
        clear_probe_kind_cache();
    }

    /// 混合凭据并发回归：无凭据的 First 广播 Generic 后，持正确凭据、订阅
    /// 同一在途探测的等待方不得原样接收（修复前直接复用广播值被误判
    /// Generic——鉴权端点对正确 key 本可识别），必须用自身凭据重新直探。
    ///
    /// mock server 行为：未持正确凭据的 /api/* 请求先挂起（保证 First 停留
    /// 在途、等待方先完成订阅），放行后按未授权回落；持正确凭据的
    /// /api/tags 立即返回 Ollama 模型列表。
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
                    // 头名大小写不敏感解析（hyper HTTP/1.1 序列化为小写 authorization）。
                    let authorized = req.lines().find_map(|l| {
                        let (name, value) = l.split_once(':')?;
                        if !name.trim().eq_ignore_ascii_case("authorization") {
                            return None;
                        }
                        value.trim().strip_prefix("Bearer ").map(str::trim)
                    }) == Some(good_key.as_str());
                    if !authorized {
                        // 无/错凭据：挂起等待放行，让 First 停留在在途探测上；
                        // 放行后仍按未授权处理 → First 走完全部端点落 Generic。
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

        // 1. 无凭据调用方成为 First 并停在挂起的 HTTP 请求上。
        let first_url = url.clone();
        let first = tokio::spawn(async move { probe_local_server_kind(&first_url, None).await });
        let registry = PROBE_KIND_INFLIGHT.get_or_init(Default::default);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !registry.lock().unwrap().contains_key(&key) {
            assert!(
                std::time::Instant::now() < deadline,
                "首个探测应在限期内完成 in-flight 注册"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // 2. 正确凭据调用方订阅同一在途探测。
        let waiter_url = key.clone();
        let waiter_key = GOOD_KEY.to_string();
        let waiter =
            tokio::spawn(
                async move { probe_local_server_kind(&waiter_url, Some(&waiter_key)).await },
            );
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 3. 放行：First 的请求解除挂起并以未授权身份全端点失败。
        gate_tx.send(true).unwrap();
        let first_kind = tokio::time::timeout(Duration::from_secs(5), first)
            .await
            .expect("First 应在放行后限期完成")
            .unwrap();
        assert_eq!(
            first_kind,
            LocalServerKind::Generic,
            "无凭据 First 应全端点失败回落 Generic"
        );

        // 4. 回归点：等待方不得照单全收 First 的 Generic，应用自身凭据重探。
        let waiter_kind = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("等待方应在限期内完成重探")
            .unwrap();
        assert_eq!(
            waiter_kind,
            LocalServerKind::Ollama,
            "持正确凭据的等待方应用自身凭据重探出真实类型，而不是接收无凭据 First 广播的 Generic"
        );

        task.abort();
        clear_probe_kind_cache();
    }
}
