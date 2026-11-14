//! 端点模型自动发现 + 上下文/输出长度探测的核心阶梯(用户点击型,一次性)。
//!
//! 与 [`crate::core::model_endpoint`] 的常驻探测(连接测试 / monitor 轮询)
//! 共用同一批请求装配与解析口径(`apply_bearer` / `apply_anthropic_auth_headers`
//! / `models_probe_url` / `parse_entry_output_limit` / `probe_ollama_models`
//! / `probe_lmstudio_models` / `probe_local_server_kind` 等),只新增各协议
//! 原生列表形态的解析;HTTP 逻辑不在这里重写。
//!
//! 纪律边界:
//! - 纯 core 层,不依赖 `crate::app`,也不读 keyring——鉴权由命令层解析后
//!   以 `api_key: Option<&str>` 传入;`None`/空串照常发请求(keyless 端点
//!   合法),远端 401/403 才报 `AuthRequired`。
//! - 结果结构(`EndpointModelsListing` / `DiscoveredModel`)只携带分类码、
//!   HTTP 状态、transport 类别与端点自报的模型事实,绝不携带原始 reqwest
//!   错误文本 / URL / key(与 monitor 探测的脱敏先例同一纪律)。
//! - Ollama 的 GGUF 训练窗口(`model_info.<arch>.context_length`)刻意不
//!   采纳(仓库不变量,见 [`crate::core::model_endpoint`
//!   ::parse_ollama_show_context] 文档);发现同样只消费 `/api/ps` 生效值
//!   与 `/api/show` 的 `num_ctx` 声明。

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::core::model_context::{resolved_context_window, resolved_output_limit};
use crate::core::model_endpoint::{
    LocalServerKind, OllamaShowProbe, apply_anthropic_auth_headers, apply_bearer,
    base_url_uses_local_or_private, models_probe_url, parse_entry_output_limit, parse_positive_u32,
    probe_lmstudio_models, probe_local_server_kind, probe_ollama_models, probe_ollama_show_context,
    shared_probe_client_with_timeout, with_opencode_session_header,
};
use crate::platform::prefs::ModelPreset;

/// 用户点击型探测的逐请求超时:8s,与连接测试(`test_model_connection` 的
/// 8s client)同一量级;客户端复用 [`shared_probe_client_with_timeout`]`(None)`
/// 的无超时连接池,超时逐请求挂(`.timeout()`),不污染常驻探测的 3s 语义。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

/// OpenAI 兼容列表的条数上限:超过截断并置 `truncated`(一次性把上万条
/// 模型灌给前端没有意义,前 2000 条足够人工挑选)。
const OPENAI_LISTING_CAP: usize = 2000;

/// Anthropic / Gemini 翻页页数上限:每页 `limit=1000` / `pageSize=1000`,
/// 3 页即 3000 条量级,再多视为截断而不是无限翻。
const PAGINATION_PAGE_CAP: usize = 3;

/// Ollama 逐模型 `/api/show` 补查上限:缺窗口事实(未加载)的条目超过该数
/// 时只查前 50 个并置 `truncated`,不无限扇出 GGUF 元数据重读。
const OLLAMA_SHOW_LOOKUP_CAP: usize = 50;

/// 发现阶梯按哪种协议的模型列表形态探测。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryProtocol {
    /// OpenAI 兼容 `{base}/models`(含本地 vLLM / Ollama / LM Studio 原生
    /// 短路与裸数组形态)。
    OpenAiCompatible,
    /// Anthropic Messages 同源的 `/v1/models` 列表。
    Anthropic,
    /// Gemini 原生 `/v1beta/models` 列表。
    Gemini,
}

/// 模型长度事实的来源:`Endpoint` = 端点在列表响应里自报;`Catalog` =
/// 端点未报时由本地目录补全。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFactSource {
    Endpoint,
    Catalog,
}

/// 发现列表里的一条模型事实。长度字段与来源成对出现:`Some(值)` + `Some`
/// 来源,或 `None` + `None`(两边都没有)。`loaded` 仅本地 Ollama / LM
/// Studio 的列表有意义(通用端点恒 `None`)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub context_window_tokens: Option<u32>,
    pub context_from: Option<ModelFactSource>,
    pub max_output_tokens: Option<u32>,
    pub output_from: Option<ModelFactSource>,
    /// 是否已加载到内存。`None` = 未知 / 不适用。
    pub loaded: Option<bool>,
    /// anthropic `display_name` / gemini `displayName`;兼容端点恒 `None`。
    pub title: Option<String>,
}

/// 发现结果状态。`AuthRequired` 覆盖 401 与 403(Groq 用 403 表鉴权失败);
/// `Unsupported` = 端点没有模型列表接口(404/405、非 JSON、HTML 盾页、
/// 形状不认识);其余失败归 `Error`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingStatus {
    Listed,
    AuthRequired,
    Unsupported,
    Error,
}

/// 一次端点模型发现的结果(全部字段可序列化,直接做 Tauri 返回值)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EndpointModelsListing {
    pub status: ListingStatus,
    pub http_status: Option<u16>,
    /// 传输失败类别:"timeout" | "dns" | "connect" | "network"(顺序
    /// is_timeout → is_dns → is_connect,与 monitor 的 transport_class 一致)。
    pub transport: Option<String>,
    /// 本地短路时探测出的 [`LocalServerKind`] 字符串;远端/非兼容协议为
    /// `None`。
    pub server_kind: Option<String>,
    pub models: Vec<DiscoveredModel>,
    /// 翻页 / 条数 / 补查上限截断。
    pub truncated: bool,
}

/// 按 [`ModelPreset`] 定发现协议:openai 系(含 responses / 兼容 / 各厂商
/// 预设与本地 vLLM)走 OpenAI 兼容;anthropic / gemini 走各自原生列表。
#[must_use]
pub fn protocol_for_preset(preset: ModelPreset) -> DiscoveryProtocol {
    match preset {
        ModelPreset::Anthropic => DiscoveryProtocol::Anthropic,
        ModelPreset::Gemini => DiscoveryProtocol::Gemini,
        _ => DiscoveryProtocol::OpenAiCompatible,
    }
}

/// 按 ACP wire 名定发现协议:`"anthropic"` → Anthropic;其余
/// (`"openai"` / `"kimi"` / `"responses"`)一律 OpenAI 兼容。
#[must_use]
pub fn protocol_for_wire(wire: &str) -> DiscoveryProtocol {
    match wire.trim().to_ascii_lowercase().as_str() {
        "anthropic" => DiscoveryProtocol::Anthropic,
        _ => DiscoveryProtocol::OpenAiCompatible,
    }
}

/// 端点模型发现入口(8s 逐请求超时)。
pub async fn probe_endpoint_models(
    base_url: &str,
    api_key: Option<&str>,
    protocol: DiscoveryProtocol,
) -> EndpointModelsListing {
    probe_endpoint_models_with_timeout(base_url, api_key, protocol, REQUEST_TIMEOUT).await
}

/// 带可注入超时的实现(公开入口固定 8s;测试用短超时驱动 timeout 分类,
/// 免得 mock 停听 socket 把套件拖到 8s)。内部函数,不是公开 API 面。
async fn probe_endpoint_models_with_timeout(
    base_url: &str,
    api_key: Option<&str>,
    protocol: DiscoveryProtocol,
    request_timeout: Duration,
) -> EndpointModelsListing {
    let mut server_kind: Option<String> = None;
    // 本地短路只属于 OpenAI 兼容阶梯:环回/私网地址先判服务类型,ollama /
    // lmstudio 走各自原生列表。Anthropic / Gemini 协议即使指向环回(本地
    // claude 代理、gemini 网关)也必须按其协议探测——本地判别探测的签名
    // 端点对它们没有意义,还会把代理误判成 Generic。
    let terminal = if protocol == DiscoveryProtocol::OpenAiCompatible
        && base_url_uses_local_or_private(base_url)
    {
        let kind = probe_local_server_kind(base_url, api_key).await;
        server_kind = Some(local_server_kind_str(kind).to_string());
        match kind {
            LocalServerKind::Ollama => match ollama_discovered(base_url, api_key).await {
                Some((models, truncated)) => Terminal::Listed(models, truncated),
                // 列表探测失败(/api/tags 空/失败):回落 OpenAI 兼容阶梯,
                // 让分类逻辑给出同一个口径的错误结果。
                None => ladder_openai(base_url, api_key, request_timeout).await,
            },
            LocalServerKind::LmStudio => match lmstudio_discovered(base_url).await {
                Some(models) => Terminal::Listed(models, false),
                None => ladder_openai(base_url, api_key, request_timeout).await,
            },
            _ => ladder_openai(base_url, api_key, request_timeout).await,
        }
    } else {
        match protocol {
            DiscoveryProtocol::OpenAiCompatible => {
                ladder_openai(base_url, api_key, request_timeout).await
            }
            DiscoveryProtocol::Anthropic => {
                ladder_anthropic(base_url, api_key, request_timeout).await
            }
            DiscoveryProtocol::Gemini => ladder_gemini(base_url, api_key, request_timeout).await,
        }
    };
    let (status, http_status, transport, raw_models, truncated) = match terminal {
        Terminal::Listed(models, truncated) => {
            (ListingStatus::Listed, None, None, models, truncated)
        }
        Terminal::Auth(http) => (
            ListingStatus::AuthRequired,
            Some(http),
            None,
            Vec::new(),
            false,
        ),
        Terminal::Unsupported(http) => (ListingStatus::Unsupported, http, None, Vec::new(), false),
        Terminal::Error { http, transport } => (
            ListingStatus::Error,
            http,
            transport.map(String::from),
            Vec::new(),
            false,
        ),
    };
    // 目录补全只在 Listed 时发生:endpoint 未报的字段用本地目录补 `Catalog`
    // 来源;endpoint 自报的值在 RawModel → DiscoveredModel 时已标 `Endpoint`,
    // 绝不被覆盖。非 Listed 时模型列表恒空(错误状态不带半截数据)。
    let models = match status {
        ListingStatus::Listed => raw_models.into_iter().map(fill_from_catalog).collect(),
        _ => Vec::new(),
    };
    EndpointModelsListing {
        status,
        http_status,
        transport,
        server_kind,
        models,
        truncated,
    }
}

/// [`LocalServerKind`] → 稳定的 server_kind 字符串(kind 键拼写与
/// `app/commands/monitor.rs::local_model_provider` 一致;Generic 不做端口
/// 推断,固定 "generic")。
fn local_server_kind_str(kind: LocalServerKind) -> &'static str {
    match kind {
        LocalServerKind::Vllm => "vllm",
        LocalServerKind::Tachyon => "tachyon",
        LocalServerKind::Ollama => "ollama",
        LocalServerKind::LmStudio => "lm_studio",
        LocalServerKind::Sglang => "sglang",
        LocalServerKind::LlamaCpp => "llamacpp",
        LocalServerKind::KoboldCpp => "koboldcpp",
        LocalServerKind::LmDeploy => "lmdeploy",
        LocalServerKind::DockerModelRunner => "dockermodelrunner",
        LocalServerKind::Generic => "generic",
    }
}

/// 端点自报的一条模型事实(目录补全前)。长度为 `None` 表示端点未报。
struct RawModel {
    id: String,
    context: Option<u32>,
    output: Option<u32>,
    loaded: Option<bool>,
    title: Option<String>,
}

impl RawModel {
    fn endpoint_sourced(self) -> DiscoveredModel {
        DiscoveredModel {
            id: self.id,
            context_window_tokens: self.context,
            context_from: self.context.map(|_| ModelFactSource::Endpoint),
            max_output_tokens: self.output,
            output_from: self.output.map(|_| ModelFactSource::Endpoint),
            loaded: self.loaded,
            title: self.title,
        }
    }
}

/// 目录补全(仅空字段;endpoint 自报值永不覆盖):先落 `Endpoint` 来源,
/// 再对未报字段查本地目录补 `Catalog`——context 走
/// [`resolved_context_window`],output 走 [`resolved_output_limit`]。
fn fill_from_catalog(raw: RawModel) -> DiscoveredModel {
    let mut model = raw.endpoint_sourced();
    if model.context_window_tokens.is_none() {
        if let Some(window) = resolved_context_window(&model.id) {
            model.context_window_tokens = Some(window);
            model.context_from = Some(ModelFactSource::Catalog);
        }
    }
    if model.max_output_tokens.is_none() {
        if let Some(output) = resolved_output_limit(&model.id) {
            model.max_output_tokens = Some(output);
            model.output_from = Some(ModelFactSource::Catalog);
        }
    }
    model
}

/// 单次请求的终止形态:2xx 且 JSON 可解析,或按错误阶梯分类的失败。
enum ProbeOutcome {
    Body(Value),
    /// 401 / 403(Groq 用 403 表鉴权失败)。
    Auth(u16),
    /// 404 / 405(没有列表接口)或 2xx 但响应不是 JSON / 是 HTML 盾页。
    Unsupported(u16),
    /// 其余 ≥400。
    HttpError(u16),
    /// 传输失败,类别顺序 is_timeout → is_dns → is_connect → network。
    Transport(&'static str),
}

/// 传输失败 → 稳定类别词。顺序照搬 monitor 的 transport_class:DNS 检查
/// 必须先于 connect(hyper-util 把解析器失败也归入 Connect kind)。
fn transport_class(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        "timeout"
    } else if err.is_dns() {
        "dns"
    } else if err.is_connect() {
        "connect"
    } else {
        "network"
    }
}

/// 发出一个已装配好的探测请求并按状态码 / 传输结果分类。成功路径要求
/// 2xx 且 JSON 可解析;错误路径只携带分类码,绝不携带响应体 / 错误文本。
async fn send_classified(request: reqwest::RequestBuilder, timeout: Duration) -> ProbeOutcome {
    let resp = match request.timeout(timeout).send().await {
        Ok(resp) => resp,
        Err(err) => return ProbeOutcome::Transport(transport_class(&err)),
    };
    let status = resp.status();
    match status.as_u16() {
        401 | 403 => return ProbeOutcome::Auth(status.as_u16()),
        404 | 405 => return ProbeOutcome::Unsupported(status.as_u16()),
        code if code >= 400 => return ProbeOutcome::HttpError(code),
        _ => {}
    }
    match resp.json::<Value>().await {
        Ok(v) => ProbeOutcome::Body(v),
        // 2xx 但非 JSON(Cloudflare HTML 盾页等):端点没有可解析的列表。
        Err(_) => ProbeOutcome::Unsupported(status.as_u16()),
    }
}

/// 阶梯的最终形态:成功给出端点自报条目(可为空)或按分类失败。
enum Terminal {
    Listed(Vec<RawModel>, bool),
    Auth(u16),
    Unsupported(Option<u16>),
    Error {
        http: Option<u16>,
        transport: Option<&'static str>,
    },
}

fn terminal_from_outcome(outcome: ProbeOutcome) -> Terminal {
    match outcome {
        ProbeOutcome::Body(_) => unreachable!("send_classified 的 Body 由各协议解析分支消费"),
        ProbeOutcome::Auth(http) => Terminal::Auth(http),
        ProbeOutcome::Unsupported(http) => Terminal::Unsupported(Some(http)),
        ProbeOutcome::HttpError(http) => Terminal::Error {
            http: Some(http),
            transport: None,
        },
        ProbeOutcome::Transport(class) => Terminal::Error {
            http: None,
            transport: Some(class),
        },
    }
}

/// OpenAI 兼容条目的上下文字段名归一化:`max_model_len`(vLLM / 标准形态)
/// | `context_length` | `context_window` | `max_context_length`。同一条目
/// 报了多个名字时取最紧(与输出侧的 only-tighten 口径一致),绝不取松。
fn parse_entry_context_window(item: &Value) -> Option<u32> {
    [
        "max_model_len",
        "context_length",
        "context_window",
        "max_context_length",
    ]
    .into_iter()
    .filter_map(|key| item.get(key).and_then(parse_positive_u32))
    .min()
}

/// OpenAI 兼容 `/models` 响应的发现版解析:接受 `{data:[...]}` **和裸数组**
/// (Together 形态)。返回 `None` = 形状不认识(→ Unsupported);`Some` =
/// 条目(可为空——空 data 数组是合法的 Listed)。条数超过
/// [`OPENAI_LISTING_CAP`] 时截断并置 truncated。输出侧复用
/// [`parse_entry_output_limit`](三形态取最紧);`loaded` / `title` 兼容端点
/// 不提供。
fn parse_openai_listing(v: &Value) -> Option<(Vec<RawModel>, bool)> {
    let entries = if let Some(array) = v.as_array() {
        array
    } else {
        v.get("data")?.as_array()?
    };
    let models = entries
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?.trim();
            (!id.is_empty()).then(|| RawModel {
                id: id.to_string(),
                context: parse_entry_context_window(item),
                output: parse_entry_output_limit(item),
                loaded: None,
                title: None,
            })
        })
        .collect::<Vec<_>>();
    let truncated = models.len() > OPENAI_LISTING_CAP;
    let mut models = models;
    models.truncate(OPENAI_LISTING_CAP);
    Some((models, truncated))
}

/// OpenAI 兼容阶梯:GET [`models_probe_url(base_url)`](不强行补 /v1——glm
/// `/paas/v4`、火山 `/api/v3`、gemini 兼容端点的 `/models` 都存在,补了
/// 反而 404)。本地与远端共用同一函数,错误分类口径一致。
async fn ladder_openai(base_url: &str, api_key: Option<&str>, timeout: Duration) -> Terminal {
    let Some(client) = shared_probe_client_with_timeout(None) else {
        return Terminal::Error {
            http: None,
            transport: None,
        };
    };
    // 网关探测带会话亲和头(与 probe_openai_models 同一装配);非网关是 no-op。
    let request = with_opencode_session_header(
        apply_bearer(client.get(models_probe_url(base_url)), api_key),
        base_url,
        "models-probe",
    );
    match send_classified(request, timeout).await {
        ProbeOutcome::Body(v) => match parse_openai_listing(&v) {
            Some((models, truncated)) => Terminal::Listed(models, truncated),
            None => Terminal::Unsupported(Some(200)),
        },
        outcome => terminal_from_outcome(outcome),
    }
}

/// Anthropic 模型列表地址:与 `anthropic_messages_url` 同一规整口径——
/// base 以 `/v1` 结尾直接拼 `/models`,否则补 `/v1/models`。
fn anthropic_models_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        format!("{trimmed}/models")
    } else {
        format!("{trimmed}/v1/models")
    }
}

/// Anthropic 列表页解析:`max_input_tokens` → context、`max_tokens` →
/// output、`display_name` → title;翻页游标 `has_more` + 末条 `id`
/// (作为下一请求的 `after_id`)。
fn parse_anthropic_page(v: &Value) -> Option<(Vec<RawModel>, bool, Option<String>)> {
    let entries = v.get("data")?.as_array()?;
    let models = entries
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?.trim();
            (!id.is_empty()).then(|| RawModel {
                id: id.to_string(),
                context: item.get("max_input_tokens").and_then(parse_positive_u32),
                output: item.get("max_tokens").and_then(parse_positive_u32),
                loaded: None,
                title: item
                    .get("display_name")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect::<Vec<_>>();
    let has_more = v.get("has_more").and_then(Value::as_bool).unwrap_or(false);
    let after_id = entries
        .last()
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Some((models, has_more, after_id))
}

/// Anthropic 阶梯:`GET {规整 base}/models?limit=1000`,鉴权头走
/// [`apply_anthropic_auth_headers`](`x-api-key` + `anthropic-version`,
/// 与 Messages 直连同一口径);翻页 `has_more` + `after_id`,最多
/// [`PAGINATION_PAGE_CAP`] 页,仍有下一页时置 truncated。
async fn ladder_anthropic(base_url: &str, api_key: Option<&str>, timeout: Duration) -> Terminal {
    let Some(client) = shared_probe_client_with_timeout(None) else {
        return Terminal::Error {
            http: None,
            transport: None,
        };
    };
    let base = anthropic_models_url(base_url);
    let key = api_key.unwrap_or("");
    let mut all = Vec::new();
    let mut after_id: Option<String> = None;
    for _page in 0..PAGINATION_PAGE_CAP {
        let mut url = format!("{base}?limit=1000");
        if let Some(id) = &after_id {
            url.push_str("&after_id=");
            url.push_str(id);
        }
        let request = apply_anthropic_auth_headers(client.get(url), key);
        match send_classified(request, timeout).await {
            ProbeOutcome::Body(v) => match parse_anthropic_page(&v) {
                Some((models, has_more, next_after)) => {
                    all.extend(models);
                    if !has_more {
                        return Terminal::Listed(all, false);
                    }
                    // has_more 但拿不到游标(末条缺 id):无法续页,按截断收尾。
                    match next_after {
                        Some(id) => after_id = Some(id),
                        None => return Terminal::Listed(all, true),
                    }
                }
                None => return Terminal::Unsupported(Some(200)),
            },
            outcome => return terminal_from_outcome(outcome),
        }
    }
    Terminal::Listed(all, true)
}

/// Gemini 原生 API 根:剥掉 OpenAI 兼容后缀 `/openai`,再剥掉版本段
/// `/v1beta`(用户常从 OpenAI 兼容地址 `/v1beta/openai` 直接切过来),
/// 得到挂 `/v1beta/models` 的原生根。
fn gemini_native_root(base_url: &str) -> String {
    let mut root = base_url.trim().trim_end_matches('/').to_string();
    for suffix in ["/openai", "/v1beta"] {
        if root.ends_with(suffix) {
            root.truncate(root.len() - suffix.len());
        }
    }
    root
}

/// Gemini 列表页解析:`supportedGenerationMethods` 含 `generateContent`
/// 的条目才保留(embedding / 纯补全类剔除);`name` 的 `models/` 前缀剥掉;
/// `inputTokenLimit` → context、`outputTokenLimit` → output、
/// `displayName` → title;翻页游标 `nextPageToken`。
fn parse_gemini_page(v: &Value) -> Option<(Vec<RawModel>, Option<String>)> {
    let entries = v.get("models")?.as_array()?;
    let models = entries
        .iter()
        .filter_map(|item| {
            let generates = item
                .get("supportedGenerationMethods")
                .and_then(Value::as_array)
                .is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method
                            .as_str()
                            .is_some_and(|s| s.eq_ignore_ascii_case("generateContent"))
                    })
                });
            if !generates {
                return None;
            }
            let name = item.get("name").and_then(Value::as_str)?.trim();
            let id = name.strip_prefix("models/").unwrap_or(name);
            (!id.is_empty()).then(|| RawModel {
                id: id.to_string(),
                context: item.get("inputTokenLimit").and_then(parse_positive_u32),
                output: item.get("outputTokenLimit").and_then(parse_positive_u32),
                loaded: None,
                title: item
                    .get("displayName")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect::<Vec<_>>();
    let next_page_token = v
        .get("nextPageToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_string);
    Some((models, next_page_token))
}

/// Gemini 阶梯:`GET {root}/v1beta/models?pageSize=1000`,鉴权
/// `x-goog-api-key`(空 key 不加头,keyless 照发);翻页 `nextPageToken`
/// → `pageToken`,最多 [`PAGINATION_PAGE_CAP`] 页,仍有下一页时置 truncated。
async fn ladder_gemini(base_url: &str, api_key: Option<&str>, timeout: Duration) -> Terminal {
    let Some(client) = shared_probe_client_with_timeout(None) else {
        return Terminal::Error {
            http: None,
            transport: None,
        };
    };
    let base = format!(
        "{}/v1beta/models?pageSize=1000",
        gemini_native_root(base_url)
    );
    let key = api_key.map(str::trim).filter(|key| !key.is_empty());
    let mut all = Vec::new();
    let mut url = base.clone();
    for _page in 0..PAGINATION_PAGE_CAP {
        let mut request = client.get(url.clone());
        if let Some(key) = key {
            request = request.header("x-goog-api-key", key);
        }
        match send_classified(request, timeout).await {
            ProbeOutcome::Body(v) => match parse_gemini_page(&v) {
                Some((models, next_page_token)) => {
                    all.extend(models);
                    match next_page_token {
                        Some(token) => {
                            url = format!("{base}&pageToken={token}");
                        }
                        None => return Terminal::Listed(all, false),
                    }
                }
                None => return Terminal::Unsupported(Some(200)),
            },
            outcome => return terminal_from_outcome(outcome),
        }
    }
    Terminal::Listed(all, true)
}

/// Ollama 分支:[`probe_ollama_models`] 的列表(`/api/ps` 已给已加载模型
/// 生效窗口)+ 缺窗口事实(未加载)条目逐模型 [`probe_ollama_show_context`]
/// 的 `num_ctx` 补查。补查条数以 [`OLLAMA_SHOW_LOOKUP_CAP`] 为上限,缺事实
/// 条目超过上限时置 truncated。GGUF 训练窗口(`model_info` 的
/// `<arch>.context_length`)刻意不采纳——`/api/show` 解析只认 `num_ctx`,
/// 见 [`parse_ollama_show_context`][crate::core::model_endpoint
/// ::parse_ollama_show_context] 文档。
async fn ollama_discovered(base_url: &str, api_key: Option<&str>) -> Option<(Vec<RawModel>, bool)> {
    let probe = probe_ollama_models(base_url, api_key).await?;
    let mut models = probe
        .models
        .into_iter()
        .map(|entry| RawModel {
            id: entry.id,
            context: entry.max_model_len,
            output: entry.max_output_tokens,
            loaded: entry.loaded,
            title: None,
        })
        .collect::<Vec<_>>();
    let missing = (0..models.len())
        .filter(|&index| models[index].context.is_none())
        .collect::<Vec<_>>();
    let truncated = missing.len() > OLLAMA_SHOW_LOOKUP_CAP;
    for &index in missing.iter().take(OLLAMA_SHOW_LOOKUP_CAP) {
        if let OllamaShowProbe::Declared(ctx) =
            probe_ollama_show_context(base_url, api_key, &models[index].id).await
        {
            models[index].context = Some(ctx);
        }
    }
    Some((models, truncated))
}

/// LM Studio 分支:[`probe_lmstudio_models`]——原生 `/api/v0/models`
/// (带 loaded 状态与 `loaded_context_length` 生效窗口)优先,旧版本回退
/// OpenAI 兼容 `/models`。能力上限 `max_context_length` 的拒绝口径由
/// v0 解析负责,这里原样消费。
async fn lmstudio_discovered(base_url: &str) -> Option<Vec<RawModel>> {
    let probe = probe_lmstudio_models(base_url).await?;
    Some(
        probe
            .models
            .into_iter()
            .map(|entry| RawModel {
                id: entry.id,
                context: entry.max_model_len,
                output: entry.max_output_tokens,
                loaded: entry.loaded,
                title: None,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model_endpoint::models_mock;

    const SHORT_TIMEOUT: Duration = Duration::from_millis(500);

    async fn probe(
        base_url: &str,
        api_key: Option<&str>,
        protocol: DiscoveryProtocol,
    ) -> EndpointModelsListing {
        probe_endpoint_models_with_timeout(base_url, api_key, protocol, SHORT_TIMEOUT).await
    }

    fn model_of<'a>(listing: &'a EndpointModelsListing, id: &str) -> &'a DiscoveredModel {
        listing
            .models
            .iter()
            .find(|model| model.id == id)
            .unwrap_or_else(|| panic!("model {id} missing from listing"))
    }

    fn assert_endpoint(model: &DiscoveredModel, context: Option<u32>, output: Option<u32>) {
        assert_eq!(model.context_window_tokens, context);
        assert_eq!(
            model.context_from,
            context.map(|_| ModelFactSource::Endpoint)
        );
        assert_eq!(model.max_output_tokens, output);
        assert_eq!(model.output_from, output.map(|_| ModelFactSource::Endpoint));
    }

    #[tokio::test]
    async fn openai_data_shape_parses_context_and_output_fields() {
        // 规格 §2.10-1:{data:[...]} 形态,max_model_len → context、
        // max_completion_tokens → output,来源 Endpoint;loopback mock 走本地
        // 短路(kind 判别 Generic)→ OpenAI 兼容阶梯。
        let mock = models_mock::spawn(&[(
            "/models",
            200,
            r#"{"object":"list","data":[
                {"id":"vllm-qwen","max_model_len":262144,"max_completion_tokens":8192},
                {"id":"bare-id"}
            ]}"#
            .into(),
        )]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.http_status, None);
        assert_eq!(listing.transport, None);
        assert_eq!(listing.server_kind.as_deref(), Some("generic"));
        assert!(!listing.truncated);
        assert_endpoint(model_of(&listing, "vllm-qwen"), Some(262_144), Some(8_192));
        assert_endpoint(model_of(&listing, "bare-id"), None, None);
        assert_eq!(model_of(&listing, "vllm-qwen").title, None);
        // 空列表(200 但 0 条)同样是 Listed + models 空。
        let mock = models_mock::spawn(&[("/models", 200, r#"{"data":[]}"#.into())]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert!(listing.models.is_empty());
        assert!(!listing.truncated);
    }

    #[tokio::test]
    async fn bare_array_shape_parses_like_data_wrapper() {
        // 规格 §2.10-2:Together 的裸数组形态;context 字段归一化
        // (context_length / context_window)与输出 max_output_tokens。
        let mock = models_mock::spawn(&[(
            "/models",
            200,
            r#"[
                {"id":"together-a","context_length":32768,"max_output_tokens":4096},
                {"id":"together-b","context_window":8192}
            ]"#
            .into(),
        )]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_endpoint(model_of(&listing, "together-a"), Some(32_768), Some(4_096));
        assert_endpoint(model_of(&listing, "together-b"), Some(8_192), None);
    }

    #[tokio::test]
    async fn openrouter_top_provider_output_limit_is_read() {
        // 规格 §2.10-3:OpenRouter 网关形态,top_provider.max_completion_tokens
        // 是输出上限;同条目的 context_length 归一化为 context。
        let mock = models_mock::spawn(&[(
            "/models",
            200,
            r#"{"data":[
                {"id":"or-model","context_length":1000000,
                 "top_provider":{"max_completion_tokens":131072}}
            ]}"#
            .into(),
        )]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_endpoint(
            model_of(&listing, "or-model"),
            Some(1_000_000),
            Some(131_072),
        );
    }

    #[tokio::test]
    async fn anthropic_pagination_headers_and_facts() {
        // 规格 §2.10-4:两页翻页(has_more/after_id)+ max_input_tokens /
        // max_tokens + display_name,并断言 anthropic-version 与 x-api-key 头。
        let mock = models_mock::spawn_seq(&[(
            "/v1/models",
            200,
            vec![
                r#"{"data":[
                        {"id":"claude-a","display_name":"Claude A",
                         "max_input_tokens":200000,"max_tokens":8192},
                        {"id":"claude-b","display_name":"Claude B",
                         "max_input_tokens":1000000,"max_tokens":32000}
                    ],"has_more":true}"#
                    .into(),
                r#"{"data":[
                        {"id":"claude-c","display_name":"Claude C",
                         "max_input_tokens":180000,"max_tokens":64000}
                    ],"has_more":false}"#
                    .into(),
            ],
        )]);
        let listing = probe(
            &mock.base_url,
            Some("sk-ant-test"),
            DiscoveryProtocol::Anthropic,
        )
        .await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.server_kind, None, "anthropic 协议不做本地短路判别");
        assert!(!listing.truncated);
        assert_eq!(mock.hits_for("/v1/models"), 2, "两页各打一次");
        assert_eq!(listing.models.len(), 3);
        let claude_a = model_of(&listing, "claude-a");
        assert_endpoint(claude_a, Some(200_000), Some(8_192));
        assert_eq!(claude_a.title.as_deref(), Some("Claude A"));
        assert_eq!(
            model_of(&listing, "claude-b").max_output_tokens,
            Some(32_000)
        );
        assert_eq!(
            model_of(&listing, "claude-c").context_window_tokens,
            Some(180_000)
        );
        // 官方端点走 x-api-key + anthropic-version(2023-06-01),不发 Bearer。
        assert_eq!(
            mock.header_for("/v1/models", "x-api-key").as_deref(),
            Some("sk-ant-test")
        );
        assert_eq!(
            mock.header_for("/v1/models", "anthropic-version")
                .as_deref(),
            Some("2023-06-01")
        );
        assert_eq!(mock.header_for("/v1/models", "authorization"), None);
        // 静态 has_more=true 的无限页在 3 页封顶:截断置位。
        let mock = models_mock::spawn(&[(
            "/v1/models",
            200,
            r#"{"data":[{"id":"claude-x","max_input_tokens":200000}],"has_more":true}"#.into(),
        )]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::Anthropic).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert!(listing.truncated);
        assert_eq!(mock.hits_for("/v1/models"), 3);
    }

    #[tokio::test]
    async fn gemini_pagination_filter_headers_and_openai_suffix_strip() {
        // 规格 §2.10-5:/openai 后缀剥离 + nextPageToken 翻页 +
        // generateContent 过滤 + x-goog-api-key 头。
        let mock = models_mock::spawn_seq(&[(
            "/v1beta/models",
            200,
            vec![
                r#"{"models":[
                        {"name":"models/gemini-pro","displayName":"Gemini Pro",
                         "inputTokenLimit":1000000,"outputTokenLimit":8192,
                         "supportedGenerationMethods":["generateContent","countTokens"]},
                        {"name":"models/gemini-embedding",
                         "supportedGenerationMethods":["embeddingContent"]}
                    ],"nextPageToken":"tok-2"}"#
                    .into(),
                r#"{"models":[
                        {"name":"models/gemini-flash","displayName":"Gemini Flash",
                         "inputTokenLimit":1000000,
                         "supportedGenerationMethods":["generateContent"]}
                    ]}"#
                .into(),
            ],
        )]);
        // 兼容地址以 /openai 结尾:剥掉(以及内层版本段)得原生根。
        let base = format!("{}/v1beta/openai", mock.base_url);
        let listing = probe(&base, Some("gkey-123"), DiscoveryProtocol::Gemini).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.server_kind, None);
        assert!(!listing.truncated);
        assert_eq!(mock.hits_for("/v1beta/models"), 2);
        assert_eq!(listing.models.len(), 2, "embedding 条目被过滤");
        let pro = model_of(&listing, "gemini-pro");
        assert_endpoint(pro, Some(1_000_000), Some(8_192));
        assert_eq!(pro.title.as_deref(), Some("Gemini Pro"));
        assert_eq!(
            model_of(&listing, "gemini-flash").context_window_tokens,
            Some(1_000_000)
        );
        assert_eq!(
            mock.header_for("/v1beta/models", "x-goog-api-key")
                .as_deref(),
            Some("gkey-123")
        );
        assert_eq!(mock.header_for("/v1beta/models", "authorization"), None);
    }

    #[tokio::test]
    async fn ollama_chain_ps_tags_and_show() {
        // 规格 §2.10-6:/api/ps 生效窗口 → /api/tags 下载列表 → 未加载条目
        // 逐模型 /api/show 的 num_ctx;GGUF 训练窗口(model_info 的
        // context_length)不采纳。
        let mock = models_mock::spawn_seq(&[
            (
                "/api/ps",
                200,
                vec![r#"{"models":[{"name":"loaded-1","context_length":8192}]}"#.into()],
            ),
            (
                "/api/tags",
                200,
                vec![
                    r#"{"models":[{"name":"loaded-1"},{"name":"declared-2"},{"name":"plain-3"}]}"#
                        .into(),
                ],
            ),
            (
                "/api/show",
                200,
                vec![
                    // declared-2:显式 num_ctx 16384 生效(不是 40960 训练上限)。
                    r#"{"parameters":"num_ctx 16384","model_info":{"qwen3.context_length":40960}}"#
                        .into(),
                    // plain-3:无 num_ctx,只有 GGUF 训练窗口 → 不采纳,保持未报。
                    r#"{"model_info":{"qwen3.context_length":40960}}"#.into(),
                ],
            ),
        ]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.server_kind.as_deref(), Some("ollama"));
        assert!(!listing.truncated);
        let loaded = model_of(&listing, "loaded-1");
        assert_endpoint(loaded, Some(8_192), None);
        assert_eq!(loaded.loaded, Some(true));
        let declared = model_of(&listing, "declared-2");
        assert_endpoint(declared, Some(16_384), None);
        assert_eq!(declared.loaded, Some(false));
        let plain = model_of(&listing, "plain-3");
        assert_eq!(plain.context_window_tokens, None, "GGUF 训练窗口不采纳");
        assert_eq!(plain.context_from, None);
        assert_eq!(plain.loaded, Some(false));
        assert_eq!(mock.hits_for("/api/show"), 2, "仅缺窗口事实的条目补查 show");
        assert_eq!(mock.hits_for("/api/ps"), 1);
        assert_eq!(mock.hits_for("/api/tags"), 2, "kind 判别与列表探测各一次");
    }

    #[tokio::test]
    async fn lmstudio_native_listing_then_openai_fallback() {
        // 规格 §2.10-7:/api/v0/models 原生列表(loaded_context_length 生效值;
        // 能力上限 max_context_length 拒绝采纳)+ 无 v0 时回退 OpenAI 兼容列表。
        let mock = models_mock::spawn(&[(
            "/api/v0/models",
            200,
            r#"{"data":[
                {"id":"lm-x","state":"loaded","max_context_length":131072,
                 "loaded_context_length":8192},
                {"id":"lm-y","state":"not-loaded","max_context_length":40960}
            ]}"#
            .into(),
        )]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.server_kind.as_deref(), Some("lm_studio"));
        let loaded = model_of(&listing, "lm-x");
        assert_endpoint(loaded, Some(8_192), None);
        assert_eq!(loaded.loaded, Some(true));
        let unloaded = model_of(&listing, "lm-y");
        assert_eq!(
            unloaded.context_window_tokens, None,
            "能力上限 max_context_length 不是生效窗口,不采纳"
        );
        assert_eq!(unloaded.loaded, Some(false));
        // 旧版本无 /api/v0/models:回退 /v1/models(用户 base 带 /v1 时
        // models_probe_url 拼 /v1/models),server_kind 退为 generic。
        let mock = models_mock::spawn(&[(
            "/v1/models",
            200,
            r#"{"data":[{"id":"legacy-z","max_model_len":4096}]}"#.into(),
        )]);
        let base = format!("{}/v1", mock.base_url);
        let listing = probe(&base, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.server_kind.as_deref(), Some("generic"));
        assert_endpoint(model_of(&listing, "legacy-z"), Some(4_096), None);
        assert_eq!(model_of(&listing, "legacy-z").loaded, None);
    }

    #[tokio::test]
    async fn error_classification_ladder() {
        // 规格 §2.10-8:401/403→AuthRequired;404/非 JSON→Unsupported;
        // 500→Error;超时→Error + transport=timeout。
        let cases = [
            (401u16, ListingStatus::AuthRequired),
            (403, ListingStatus::AuthRequired),
            (404, ListingStatus::Unsupported),
            (500, ListingStatus::Error),
        ];
        for (http, expected) in cases {
            let mock = models_mock::spawn(&[("/models", http, "{}".into())]);
            let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
            assert_eq!(listing.status, expected, "HTTP {http} 分类");
            assert_eq!(listing.http_status, Some(http));
            assert_eq!(listing.transport, None);
            assert!(listing.models.is_empty());
            assert!(!listing.truncated);
        }
        // 2xx 非 JSON(Cloudflare HTML 盾页)→ Unsupported。
        let mock = models_mock::spawn(&[(
            "/models",
            200,
            "<html><body>Just a moment...</body></html>".into(),
        )]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Unsupported);
        assert_eq!(listing.http_status, Some(200));
        // 超时:监听但不应答的 socket + 短超时 → Error + transport=timeout
        // (anthropic 协议不做本地短路,免掉 kind 判别的 3s 停听)。
        let stalled = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stalled listener");
        let listing = probe_endpoint_models_with_timeout(
            &format!("http://{}/", stalled.local_addr().unwrap()),
            None,
            DiscoveryProtocol::Anthropic,
            Duration::from_millis(300),
        )
        .await;
        assert_eq!(listing.status, ListingStatus::Error);
        assert_eq!(listing.http_status, None);
        assert_eq!(listing.transport.as_deref(), Some("timeout"));
        assert!(listing.models.is_empty());
    }

    #[tokio::test]
    async fn keyless_request_still_lists_on_200() {
        // 规格 §2.10-9:keyless(None / 空白 key)照常发请求,远端 200 →
        // Listed,且不带任何鉴权头。
        for key in [None, Some(""), Some("   ")] {
            let mock = models_mock::spawn(&[(
                "/models",
                200,
                r#"{"data":[{"id":"free-tier-model"}]}"#.into(),
            )]);
            let listing = probe(&mock.base_url, key, DiscoveryProtocol::OpenAiCompatible).await;
            assert_eq!(listing.status, ListingStatus::Listed);
            assert_eq!(listing.models.len(), 1);
            assert_eq!(model_of(&listing, "free-tier-model").id, "free-tier-model");
            assert_eq!(mock.header_for("/models", "authorization"), None);
        }
    }

    #[tokio::test]
    async fn catalog_completion_fills_missing_and_never_overwrites() {
        // 规格 §2.10-10:端点未报的字段由本地目录补 Catalog 来源
        // (kimi-k3 → 1M 窗口);endpoint 自报的值绝不覆盖。
        let mock = models_mock::spawn(&[(
            "/models",
            200,
            r#"{"data":[
                {"id":"kimi-k3"},
                {"id":"deepseek-flash","max_model_len":32768,"max_output_tokens":4096}
            ]}"#
            .into(),
        )]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        // kimi-k3:端点未报 context → 目录补 1M;未报 output → 目录没有 → 双 None。
        let kimi = model_of(&listing, "kimi-k3");
        assert_eq!(kimi.context_window_tokens, Some(1_048_576));
        assert_eq!(kimi.context_from, Some(ModelFactSource::Catalog));
        assert_eq!(kimi.max_output_tokens, None);
        assert_eq!(kimi.output_from, None);
        // deepseek-flash:端点报了 ctx/out → 保持 Endpoint 值,目录不覆盖。
        let deepseek = model_of(&listing, "deepseek-flash");
        assert_endpoint(deepseek, Some(32_768), Some(4_096));
    }

    #[tokio::test]
    async fn openai_listing_cap_truncates_at_2000() {
        // 规格 §2.10-11:条数上限 2000,超过截断置 truncated;恰好 2000 不截。
        let body = |count: usize| {
            let entries = (0..count)
                .map(|index| format!(r#"{{"id":"m{index}"}}"#))
                .collect::<Vec<_>>()
                .join(",");
            format!(r#"{{"data":[{entries}]}}"#)
        };
        let mock = models_mock::spawn(&[("/models", 200, body(2001))]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.models.len(), 2000);
        assert!(listing.truncated);
        assert_eq!(model_of(&listing, "m0").id, "m0");
        assert_eq!(model_of(&listing, "m1999").id, "m1999");
        let mock = models_mock::spawn(&[("/models", 200, body(2000))]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.models.len(), 2000);
        assert!(!listing.truncated);
    }

    #[tokio::test]
    async fn listing_serialization_redacts_url_and_key() {
        // 规格 §2.10-12:错误路径的序列化串不含 mock URL 与 key(脱敏纪律:
        // 结果结构只携带分类码,不携带 reqwest 文本 / URL / key)。
        let key = "sk-discovery-secret-777";
        let mock = models_mock::spawn(&[("/models", 500, "{}".into())]);
        let listing = probe(
            &mock.base_url,
            Some(key),
            DiscoveryProtocol::OpenAiCompatible,
        )
        .await;
        assert_eq!(listing.status, ListingStatus::Error);
        let json = serde_json::to_string(&listing).expect("listing serializes");
        assert!(!json.contains(key));
        assert!(!json.contains(&mock.base_url));
        assert!(!json.contains("127.0.0.1"));
        // 鉴权失败路径同样不携带 key。
        let mock = models_mock::spawn(&[("/models", 401, "{}".into())]);
        let listing = probe(
            &mock.base_url,
            Some(key),
            DiscoveryProtocol::OpenAiCompatible,
        )
        .await;
        let json = serde_json::to_string(&listing).expect("listing serializes");
        assert_eq!(listing.status, ListingStatus::AuthRequired);
        assert!(!json.contains(key));
        assert!(!json.contains(&mock.base_url));
    }

    /// 附加钉(超出 12 项清单):Ollama 缺窗口事实条目超过 50 时只补查前
    /// 50 个 show,并置 truncated——一次性发现不无限扇出 GGUF 重读。
    #[tokio::test]
    async fn ollama_show_lookups_cap_at_50_and_mark_truncated() {
        let names = (0..55)
            .map(|index| format!(r#"{{"name":"bulk-{index}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let mock = models_mock::spawn(&[
            ("/api/ps", 200, r#"{"models":[]}"#.into()),
            ("/api/tags", 200, format!(r#"{{"models":[{names}]}}"#)),
            // 无 num_ctx 的 show 应答:补查发生但拿不到事实,条目保持未报。
            ("/api/show", 200, r#"{"model_info":{}}"#.into()),
        ]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.models.len(), 55);
        assert!(listing.truncated, ">50 缺窗口条目必须置 truncated");
        assert_eq!(
            mock.hits_for("/api/show"),
            50,
            "逐模型补查封顶 50,不扇出到全部条目"
        );
    }

    /// 附加钉:协议映射(preset / wire)——openai 系全走兼容、anthropic /
    /// gemini 走原生;wire 名只有 "anthropic" 映射原生协议。
    #[test]
    fn protocol_mapping_pins() {
        for preset in [
            ModelPreset::LocalVllm,
            ModelPreset::Deepseek,
            ModelPreset::Kimi,
            ModelPreset::OpenaiCompatible,
            ModelPreset::Qwen,
            ModelPreset::Doubao,
            ModelPreset::Minimax,
            ModelPreset::Glm,
            ModelPreset::Mimo,
            ModelPreset::Openai,
            ModelPreset::OpenaiResponses,
            ModelPreset::Xai,
        ] {
            assert_eq!(
                protocol_for_preset(preset),
                DiscoveryProtocol::OpenAiCompatible,
                "{preset:?} 必须走 OpenAI 兼容"
            );
        }
        assert_eq!(
            protocol_for_preset(ModelPreset::Anthropic),
            DiscoveryProtocol::Anthropic
        );
        assert_eq!(
            protocol_for_preset(ModelPreset::Gemini),
            DiscoveryProtocol::Gemini
        );
        assert_eq!(protocol_for_wire("anthropic"), DiscoveryProtocol::Anthropic);
        // trim + 小写后的 "anthropic" 变体仍映射 Anthropic。
        assert_eq!(
            protocol_for_wire("  ANTHROPIC  "),
            DiscoveryProtocol::Anthropic
        );
        for wire in ["openai", "kimi", "responses", "responses-v2", ""] {
            assert_eq!(
                protocol_for_wire(wire),
                DiscoveryProtocol::OpenAiCompatible,
                "wire {wire:?} 必须走 OpenAI 兼容"
            );
        }
    }

    /// 附加钉:序列化形状——枚举 snake_case、字段名与规格 §2.1 一致,
    /// 直接做 Tauri 返回值时前端拿到的键不漂移。
    #[test]
    fn listing_serialization_shape_pins() {
        let listing = EndpointModelsListing {
            status: ListingStatus::Listed,
            http_status: None,
            transport: None,
            server_kind: Some("ollama".into()),
            models: vec![DiscoveredModel {
                id: "m".into(),
                context_window_tokens: Some(8192),
                context_from: Some(ModelFactSource::Endpoint),
                max_output_tokens: None,
                output_from: Some(ModelFactSource::Catalog),
                loaded: Some(false),
                title: None,
            }],
            truncated: true,
        };
        let json = serde_json::to_string(&listing).expect("serializes");
        assert_eq!(
            json,
            r#"{"status":"listed","http_status":null,"transport":null,"server_kind":"ollama","models":[{"id":"m","context_window_tokens":8192,"context_from":"endpoint","max_output_tokens":null,"output_from":"catalog","loaded":false,"title":null}],"truncated":true}"#
        );
    }
}
