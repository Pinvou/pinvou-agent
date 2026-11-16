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
//! - 三个协议阶梯走禁用重定向的专用客户端([`discovery_client`]):一次性
//!   点击探测没有跟随 302 的业务理由,而 reqwest 跨源重定向只剥
//!   `Authorization`/`Cookie` 族,`x-api-key` / `x-goog-api-key` 会原样
//!   带给重定向目标——把用户 key 泄给用户从未点名的主机(仓库先例:CLI
//!   `probe_client` 的 `Policy::none()` 同一理由)。本地短路里的 kind 判别
//!   与 Ollama / LM Studio 原生探测继续复用 [`crate::core::model_endpoint`
//!   ::shared_probe_client](既有函数;Bearer 在跨源重定向时会被 reqwest
//!   剥除,不新增暴露面)。
//! - Ollama 的 GGUF 训练窗口(`model_info.<arch>.context_length`)刻意不
//!   采纳(仓库不变量,见 [`crate::core::model_endpoint::
//!   parse_ollama_show_context`]);发现同样只消费 `/api/ps` 生效值
//!   与 `/api/show` 的 `num_ctx` 声明。

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::core::model_context::{resolved_context_window, resolved_output_limit};
use crate::core::model_endpoint::{
    LocalServerKind, apply_anthropic_auth_headers, apply_bearer, base_url_uses_local_or_private,
    cached_ollama_show_context, models_probe_url, parse_entry_output_limit, parse_positive_u32,
    probe_lmstudio_models, probe_local_server_kind, probe_ollama_models, raw_error_looks_like_tls,
    strip_v1_suffix, with_opencode_session_header,
};
use crate::platform::prefs::ModelPreset;

/// 用户点击型探测的逐请求超时:8s,与连接测试(`test_model_connection` 的
/// 8s client)同一量级。三个协议阶梯的每个 HTTP 请求各自挂 `.timeout()`;
/// 本地短路分支(kind 判别 / Ollama / LM Studio 原生探测)复用常驻探测的
/// 共享客户端,保持其客户端级 3s 语义——Ollama 逐模型 `/api/show` 补查
/// 另有整体墙钟预算([`OLLAMA_SHOW_BUDGET`]),不会按 50 × 8s 放大。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

/// OpenAI 兼容列表的条数上限:超过截断并置 `truncated`(一次性把上万条
/// 模型灌给前端没有意义,前 2000 条足够人工挑选)。
const OPENAI_LISTING_CAP: usize = 2000;

/// Anthropic / Gemini 原生列表的同一总量上限(OpenAI 阶梯已有
/// [`OPENAI_LISTING_CAP`],原生翻页 3 页 × 1000 = 3000 条量级,同样不该
/// 无封顶地灌给前端)。语义与 OpenAI 阶梯一致:恰好 2000 不截,超过才
/// 截断置 `truncated`。
const NATIVE_LISTING_CAP: usize = 2000;

/// Anthropic / Gemini 翻页页数上限:每页 `limit=1000` / `pageSize=1000`,
/// 3 页即 3000 条量级,再多视为截断而不是无限翻。
const PAGINATION_PAGE_CAP: usize = 3;

/// Ollama 逐模型 `/api/show` 补查上限:缺窗口事实(未加载)的条目超过该数
/// 时只查前 50 个并置 `truncated`,不无限扇出 GGUF 元数据重读。
const OLLAMA_SHOW_LOOKUP_CAP: usize = 50;

/// Ollama 逐模型 `/api/show` 补查的整体墙钟预算:每个 show 走共享探测
/// 客户端的 3s 超时,50 个串行最坏 ≈150s——一次性发现不应让用户干等
/// 这么久。超预算即停,余下未补查条目置 `truncated`(与条数上限同一
/// 语义:列表是完整的,窗口事实不全)。补查本身走 60s TTL 的共享缓存
/// ([`cached_ollama_show_context`]),与 monitor 轮询共享 GGUF 元数据
/// 读取,不重复打服务端。
const OLLAMA_SHOW_BUDGET: Duration = Duration::from_secs(20);

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
    /// 传输失败类别:"timeout" | "dns" | "connect" | "tls" | "network"
    /// (顺序 is_timeout → is_dns → is_connect → 裸文本 TLS 启发式,与
    /// monitor 的 transport_class / 连接测试的 transport_code_for 同源)。
    /// 小写 snake_case,前端 connectionMessages 按词消费。
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
                // 列表探测失败(/api/tags 空/失败):先复核 tags——kind 缓存
                // 刚确认过的 Ollama 若瞬态失败,按真实失败收口(见
                // [`ollama_native_failure`]),否则回落 OpenAI 兼容阶梯
                // (含裸 base 的 /v1 回退),保持「不再是 Ollama / 空列表」
                // 的既有语义。
                None => match ollama_native_failure(base_url, api_key).await {
                    Some(terminal) => terminal,
                    None => ladder_openai_local(base_url, api_key, request_timeout).await,
                },
            },
            LocalServerKind::LmStudio => match lmstudio_discovered(base_url).await {
                Some(models) => Terminal::Listed(models, false),
                None => ladder_openai_local(base_url, api_key, request_timeout).await,
            },
            _ => ladder_openai_local(base_url, api_key, request_timeout).await,
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
    /// 2xx body 中途断流:带状态码的传输失败(分类 `network`,可重试)。
    BrokenBody(u16),
    /// 传输失败,类别顺序 is_timeout → is_dns → is_connect → network。
    Transport(&'static str),
}

/// 传输失败 → 稳定类别词。顺序照搬 monitor 的 transport_class:DNS 检查
/// 必须先于 connect(hyper-util 把解析器失败也归入 Connect kind)。TLS 用
/// 与连接测试 `transport_code_for` 同源的裸文本启发式
/// ([`raw_error_looks_like_tls`])补 "tls" 类别——结构化标志 Answer 之后的
/// 兜底文本臂,证书校验失败不再误落 network。
fn transport_class(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        "timeout"
    } else if err.is_dns() {
        "dns"
    } else if err.is_connect() {
        "connect"
    } else if raw_error_looks_like_tls(&err.to_string()) {
        "tls"
    } else {
        "network"
    }
}

/// 发出一个已装配好的探测请求并按状态码 / 传输结果分类。成功路径要求
/// 2xx 且 JSON 可解析(限长读取,见 [`LISTING_BODY_CAP_BYTES`]);错误路径
/// 只携带分类码,绝不携带响应体 / 错误文本。
async fn send_classified(request: reqwest::RequestBuilder, timeout: Duration) -> ProbeOutcome {
    let mut resp = match request.timeout(timeout).send().await {
        Ok(resp) => resp,
        Err(err) => return ProbeOutcome::Transport(transport_class(&err)),
    };
    let status = resp.status();
    match status.as_u16() {
        401 | 403 => return ProbeOutcome::Auth(status.as_u16()),
        404 | 405 => return ProbeOutcome::Unsupported(status.as_u16()),
        // 3xx:客户端禁用重定向(Policy::none 下响应原样返回)。显式归类为
        // error——落进 body 读取的旧实现把 3xx 读成 Unsupported,JSON 响应体
        // 的 3xx 甚至会被当列表列出。
        code if (300..400).contains(&code) => return ProbeOutcome::HttpError(code),
        code if code >= 400 => return ProbeOutcome::HttpError(code),
        _ => {}
    }
    match read_json_body_bounded(&mut resp).await {
        Ok(v) => ProbeOutcome::Body(v),
        // 2xx 但非 JSON / 截断(Cloudflare HTML 盾页、超大恶意列表等):
        // 端点没有可解析的列表。
        Err(BodyReadFail::Unusable) => ProbeOutcome::Unsupported(status.as_u16()),
        Err(BodyReadFail::MidStreamTimeout) => ProbeOutcome::Transport("timeout"),
        // 2xx body 中途断流(代理/VPN 掐断等):传输层失败,重试可能成功,
        // 与「形状不认识」严格区分——误报 Unsupported 会把可重试错误钉成
        // 「端点不支持发现」。
        Err(BodyReadFail::MidStream) => ProbeOutcome::BrokenBody(status.as_u16()),
    }
}

/// 2xx 列表 body 的限长:2000 条量级的列表(含 OpenRouter 扩展字段)远小于
/// 该值;恶意端点想用超大响应打爆内存会被截断,截断后的 JSON 解析失败按
/// 「形状不认识」归类。读取与 `error_for_status_with_body` 同款限长 chunk
/// 循环:到上限或流结束即停,响应其余部分随 drop 丢弃、绝不排干。
const LISTING_BODY_CAP_BYTES: usize = 8 * 1024 * 1024;

/// 2xx body 读取失败的三种形态:不可解析(收口 Unsupported)、中途非超时
/// 断流(收口 BrokenBody → Error/network)与中途超时(收口 Transport
/// timeout——同样可重试,但前端按超时给提示,而不是泛化的 http_error)。
/// 截断(到 cap 仍未读完)由「cap 处停读 → 解析失败」自然落进 `Unusable`。
enum BodyReadFail {
    Unusable,
    MidStream,
    MidStreamTimeout,
}

async fn read_json_body_bounded(resp: &mut reqwest::Response) -> Result<Value, BodyReadFail> {
    let mut buf: Vec<u8> = Vec::new();
    while buf.len() < LISTING_BODY_CAP_BYTES {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let take = chunk.len().min(LISTING_BODY_CAP_BYTES - buf.len());
                buf.extend_from_slice(&chunk[..take]);
            }
            Ok(None) => break,
            // 中途断流:body 不可信,但这是传输层事实,不是「形状不认识」。
            // 超时单独拆出:逐请求超时打到 body 读取段时,类别仍是 timeout
            // (可重试、前端给重试提示),折叠进兜底 network 会丢掉这一语义。
            Err(e) if e.is_timeout() => return Err(BodyReadFail::MidStreamTimeout),
            Err(_) => return Err(BodyReadFail::MidStream),
        }
    }
    serde_json::from_slice(&buf).map_err(|_| BodyReadFail::Unusable)
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
        ProbeOutcome::BrokenBody(http) => Terminal::Error {
            http: Some(http),
            transport: Some("network"),
        },
        ProbeOutcome::Transport(class) => Terminal::Error {
            http: None,
            transport: Some(class),
        },
    }
}

/// OpenAI 兼容条目的上下文字段名归一化:`max_model_len`(vLLM / 标准形态)
/// | `context_length` | `context_window`。同一条目报了多个名字时取最紧
/// (与输出侧的 only-tighten 口径一致),绝不取松。**刻意不含
/// `max_context_length`**:那是 LM Studio 的能力上限(capability cap)而非
/// 服务窗口——远程兼容端点不会命中本地短路,这个字段一旦被采纳就会经
/// 空表单预填 / 批量添加落成部署事实;仓库不变量
/// `parse_models_response_list_refuses_lmstudio_cap_alias` 对连接测试路径
/// 钉了同一拒绝,发现路径口径一致(下方同口径拒绝测试钉住)。
fn parse_entry_context_window(item: &Value) -> Option<u32> {
    ["max_model_len", "context_length", "context_window"]
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

/// 发现阶梯专用客户端(一次性点击探测):唯一与共享探测客户端的语义差异
/// 是**禁用重定向**。reqwest 默认跟随 ≤10 跳,且跨源重定向只剥
/// `Authorization`/`Cookie`/`Proxy-Authorization` 族——`x-api-key`(anthropic
/// 阶梯)与 `x-goog-api-key`(gemini 阶梯)是自定义头,会原样发给重定向
/// 目标,把用户 key 泄给用户从未点名的主机。禁用后 3xx 不跟随、按
/// [`ProbeOutcome::HttpError`] 归入 error,与 CLI 一致——不再有携带 key 的
/// 第二跳。
fn discovery_client() -> Option<reqwest::Client> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .ok()
        })
        .clone()
}

/// OpenAI 兼容阶梯:GET [`models_probe_url(base_url)`](不强行补 /v1——glm
/// `/paas/v4`、火山 `/api/v3`、gemini 兼容端点的 `/models` 都存在,补了
/// 反而 404)。本地与远端共用同一函数,错误分类口径一致。
async fn ladder_openai(base_url: &str, api_key: Option<&str>, timeout: Duration) -> Terminal {
    ladder_openai_url(&models_probe_url(base_url), base_url, api_key, timeout).await
}

/// 指定完整列表地址的 OpenAI 兼容阶梯(base_url 仅用于网关亲和头判定)。
async fn ladder_openai_url(
    url: &str,
    base_url: &str,
    api_key: Option<&str>,
    timeout: Duration,
) -> Terminal {
    let Some(client) = discovery_client() else {
        return Terminal::Error {
            http: None,
            transport: None,
        };
    };
    // 网关探测带会话亲和头(与 probe_openai_models 同一装配);非网关是 no-op。
    let request = with_opencode_session_header(
        apply_bearer(client.get(url), api_key),
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

/// 本地短路的 OpenAI 兼容阶梯 + 裸 base 的 `/v1` 回退:健康的本地 vLLM /
/// Ollama 常以裸地址(`http://127.0.0.1:8000`)配置,`{base}/models` 404
/// 而 kind 判别探测(fetch_v1_models)确认服务在 `{base}/v1/models`——
/// 仅本地路径在 Unsupported(404/405) 且 base 不带 `/v1` 时补一发
/// `{base}/v1/models`(与 fetch_v1_models 的 URL 装配同一口径)。远端行为
/// 不变:远端版本根各异(glm `/paas/v4` 等),盲补 /v1 会把「路径不存在」
/// 的真实事实洗成另一条 404。
async fn ladder_openai_local(base_url: &str, api_key: Option<&str>, timeout: Duration) -> Terminal {
    let terminal = ladder_openai(base_url, api_key, timeout).await;
    let trimmed = base_url.trim_end_matches('/');
    match terminal {
        Terminal::Unsupported(Some(404 | 405)) if !trimmed.ends_with("/v1") => {
            ladder_openai_url(&format!("{trimmed}/v1/models"), base_url, api_key, timeout).await
        }
        terminal => terminal,
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

/// 原生形态单条目的公共解析结果(字段名知识只此一份:发现翻页与连接测试
/// 富化共用 [`anthropic_entry_facts`] / [`gemini_entry_facts`],字段改名
/// 不再需要跨层同步)。
pub(crate) struct NativeEntryFacts {
    pub id: String,
    pub context: Option<u32>,
    pub output: Option<u32>,
    pub title: Option<String>,
}

/// Anthropic 单条目:`max_input_tokens` → context、`max_tokens` → output、
/// `display_name` → title;条目缺 `id` / `id` 空白 → `None`。非正值 / 越界
/// 长度按 [`parse_positive_u32`] 口径视为未声明。
pub(crate) fn anthropic_entry_facts(item: &Value) -> Option<NativeEntryFacts> {
    let id = item.get("id").and_then(Value::as_str)?.trim();
    (!id.is_empty()).then(|| NativeEntryFacts {
        id: id.to_string(),
        context: item.get("max_input_tokens").and_then(parse_positive_u32),
        output: item.get("max_tokens").and_then(parse_positive_u32),
        title: item
            .get("display_name")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// Gemini 单条目:`inputTokenLimit` → context、`outputTokenLimit` → output、
/// `displayName` → title;`name` 的 `models/` 前缀剥掉;缺 `name` / 空白 →
/// `None`。注意:条目级过滤(`supportedGenerationMethods` 含
/// `generateContent`)只属于列表页解析([`parse_gemini_page`]),连接测试
/// 富化不过滤——配置了哪个条目就匹配哪个,与 monitor「匹配已配置条目」
/// 同一纪律。
pub(crate) fn gemini_entry_facts(item: &Value) -> Option<NativeEntryFacts> {
    let name = item.get("name").and_then(Value::as_str)?.trim();
    let id = name.strip_prefix("models/").unwrap_or(name);
    (!id.is_empty()).then(|| NativeEntryFacts {
        id: id.to_string(),
        context: item.get("inputTokenLimit").and_then(parse_positive_u32),
        output: item.get("outputTokenLimit").and_then(parse_positive_u32),
        title: item
            .get("displayName")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// Anthropic 列表页解析:`max_input_tokens` → context、`max_tokens` →
/// output、`display_name` → title;翻页游标 `has_more` + 末条 `id`
/// (作为下一请求的 `after_id`)。
fn parse_anthropic_page(v: &Value) -> Option<(Vec<RawModel>, bool, Option<String>)> {
    let entries = v.get("data")?.as_array()?;
    let models = entries
        .iter()
        .filter_map(|item| {
            anthropic_entry_facts(item).map(|facts| RawModel {
                id: facts.id,
                context: facts.context,
                output: facts.output,
                loaded: None,
                title: facts.title,
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

/// 原生阶梯(anthropic / gemini)的收尾:与 OpenAI 阶梯同一总量语义——
/// 恰好 [`NATIVE_LISTING_CAP`] 不截,超过才截断并置 `truncated`(翻页
/// 页数上限只控制请求轮数,条数上限控制灌给前端的总量,两道闸缺一不可)。
fn finish_native_listing(mut all: Vec<RawModel>, truncated: bool) -> Terminal {
    let truncated = truncated || all.len() > NATIVE_LISTING_CAP;
    all.truncate(NATIVE_LISTING_CAP);
    Terminal::Listed(all, truncated)
}

/// Anthropic 阶梯:`GET {规整 base}/models?limit=1000`,鉴权头走
/// [`apply_anthropic_auth_headers`](`x-api-key` + `anthropic-version`,
/// 与 Messages 直连同一口径);翻页 `has_more` + `after_id`,最多
/// [`PAGINATION_PAGE_CAP`] 页,仍有下一页时置 truncated。游标经
/// reqwest `.query()` 编码后拼入——端点自报的 id 不做字符串拼接。
async fn ladder_anthropic(base_url: &str, api_key: Option<&str>, timeout: Duration) -> Terminal {
    let Some(client) = discovery_client() else {
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
        let mut request = client.get(&base).query(&[("limit", "1000")]);
        if let Some(id) = &after_id {
            request = request.query(&[("after_id", id)]);
        }
        // 网关探测带会话亲和头(与 OpenAI 阶梯同一装配);非网关是 no-op。
        let request = with_opencode_session_header(
            apply_anthropic_auth_headers(request, key),
            base_url,
            "models-probe",
        );
        match send_classified(request, timeout).await {
            ProbeOutcome::Body(v) => match parse_anthropic_page(&v) {
                Some((models, has_more, next_after)) => {
                    all.extend(models);
                    if !has_more {
                        return finish_native_listing(all, false);
                    }
                    // has_more 但拿不到游标(末条缺 id):无法续页,按截断收尾。
                    match next_after {
                        Some(id) => after_id = Some(id),
                        None => return finish_native_listing(all, true),
                    }
                }
                None => return Terminal::Unsupported(Some(200)),
            },
            outcome => return terminal_from_outcome(outcome),
        }
    }
    finish_native_listing(all, true)
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
            gemini_entry_facts(item).map(|facts| RawModel {
                id: facts.id,
                context: facts.context,
                output: facts.output,
                loaded: None,
                title: facts.title,
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
/// → `pageToken`,最多 [`PAGINATION_PAGE_CAP`] 页,仍有下一页时置
/// truncated。游标经 reqwest `.query()` 编码后拼入。
async fn ladder_gemini(base_url: &str, api_key: Option<&str>, timeout: Duration) -> Terminal {
    let Some(client) = discovery_client() else {
        return Terminal::Error {
            http: None,
            transport: None,
        };
    };
    let base = format!("{}/v1beta/models", gemini_native_root(base_url));
    let key = api_key.map(str::trim).filter(|key| !key.is_empty());
    let mut all = Vec::new();
    let mut page_token: Option<String> = None;
    for _page in 0..PAGINATION_PAGE_CAP {
        let mut request = client.get(&base).query(&[("pageSize", "1000")]);
        if let Some(key) = key {
            request = request.header("x-goog-api-key", key);
        }
        if let Some(token) = &page_token {
            request = request.query(&[("pageToken", token)]);
        }
        // 网关探测带会话亲和头(与 OpenAI 阶梯同一装配);非网关是 no-op。
        let request = with_opencode_session_header(request, base_url, "models-probe");
        match send_classified(request, timeout).await {
            ProbeOutcome::Body(v) => match parse_gemini_page(&v) {
                Some((models, next_page_token)) => {
                    all.extend(models);
                    match next_page_token {
                        Some(token) => page_token = Some(token),
                        None => return finish_native_listing(all, false),
                    }
                }
                None => return Terminal::Unsupported(Some(200)),
            },
            outcome => return terminal_from_outcome(outcome),
        }
    }
    finish_native_listing(all, true)
}

/// Ollama 分支:[`probe_ollama_models`] 的列表(`/api/ps` 已给已加载模型
/// 生效窗口)+ 缺窗口事实(未加载)条目逐模型 `/api/show` 的 `num_ctx`
/// 补查。补查条数以 [`OLLAMA_SHOW_LOOKUP_CAP`] 为上限、整体受
/// [`OLLAMA_SHOW_BUDGET`] 墙钟预算约束(超预算即停,余量置 truncated,
/// 不让一次点击串行等待 ≈50 × 3s);补查走 60s TTL 共享缓存
/// ([`cached_ollama_show_context`]),与 monitor 轮询共享 GGUF 元数据读取。
/// GGUF 训练窗口(`model_info` 的 `<arch>.context_length`)刻意不采纳——
/// `/api/show` 解析只认 `num_ctx`,见 [`parse_ollama_show_context`]
/// [crate::core::model_endpoint::parse_ollama_show_context] 文档。
/// kind 缓存确认 Ollama 后的瞬态失败复核(minor-7):[`ollama_discovered`]
/// 返回 `None` 时(列表探测失败)调用——重新打一发 `/api/tags`:
/// - 5xx / 传输失败 / 401·403 → `Some(按真实分类的 Terminal)`,绝不回落
///   OpenAI 兼容阶梯把可重试的「服务端正忙」钉成 Unsupported(404);
/// - tags 合形应答(200,含空列表)或 404/405 → `None`,回落阶梯保持既有
///   语义(空列表不是失败;404 = kind 缓存过期、服务已不是 Ollama)。
async fn ollama_native_failure(base_url: &str, api_key: Option<&str>) -> Option<Terminal> {
    let Some(client) = discovery_client() else {
        return None;
    };
    let host = strip_v1_suffix(base_url)?;
    let request = apply_bearer(client.get(format!("{host}/api/tags")), api_key);
    match send_classified(request, REQUEST_TIMEOUT).await {
        ProbeOutcome::Body(_) | ProbeOutcome::Unsupported(_) => None,
        outcome => Some(terminal_from_outcome(outcome)),
    }
}

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
    let mut truncated = missing.len() > OLLAMA_SHOW_LOOKUP_CAP;
    let started = std::time::Instant::now();
    for &index in missing.iter().take(OLLAMA_SHOW_LOOKUP_CAP) {
        if started.elapsed() >= OLLAMA_SHOW_BUDGET {
            truncated = true;
            break;
        }
        // None = 无声明 / 瞬态失败(均不入缓存或按语义返回 None),条目保持未报。
        if let Some(ctx) = cached_ollama_show_context(base_url, api_key, &models[index].id).await {
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
    use crate::core::model_endpoint::{clear_ollama_show_cache, models_mock};

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

    /// 自审补钉(round-1 fix wave):`max_context_length` 是 LM Studio 能力
    /// 上限而非服务窗口——兼容条目只报它时上下文保持未声明,绝不经空表单
    /// 预填 / 批量添加落成部署事实(连接测试路径的
    /// `parse_models_response_list_refuses_lmstudio_cap_alias` 同口径)。
    #[tokio::test]
    async fn compat_lane_refuses_lmstudio_capability_cap() {
        let mock = models_mock::spawn(&[(
            "/models",
            200,
            r#"{"data":[
                {"id":"cap-only","max_context_length":131072},
                {"id":"mixed","max_model_len":8192,"max_context_length":131072}
            ]}"#
            .into(),
        )]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        let cap_only = model_of(&listing, "cap-only");
        assert_eq!(cap_only.context_window_tokens, None);
        assert_eq!(cap_only.context_from, None);
        // 生效窗口字段照常采纳,且不被同条目的能力上限覆盖(8192,非 131072)。
        assert_eq!(
            model_of(&listing, "mixed").context_window_tokens,
            Some(8_192)
        );
    }

    /// Gemini 翻页同样受页数上限约束:静态 `nextPageToken` 的无限页在第
    /// 3 页封顶并置 truncated(anthropic 侧已有同形钉,此处补 gemini 侧)。
    #[tokio::test]
    async fn gemini_pagination_caps_at_three_pages() {
        let mock = models_mock::spawn(&[(
            "/v1beta/models",
            200,
            r#"{"models":[
                {"name":"models/gemini-loop","supportedGenerationMethods":["generateContent"]}
            ],"nextPageToken":"tok"}"#
                .into(),
        )]);
        let listing = probe(&mock.base_url, Some("gkey"), DiscoveryProtocol::Gemini).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert!(listing.truncated, "静态翻页在第 3 页封顶必须置 truncated");
        assert_eq!(mock.hits_for("/v1beta/models"), 3);
    }

    /// 限长常量钉:8 MiB 是「截断 → 解析失败 → Unusable」分类的前提;大小
    /// 只应显式改动(下方 oversized 用例的 fixture 在任何不小于其长度的
    /// cap 下都不变红,常量钉守住「静默放大」)。
    #[test]
    fn listing_body_cap_is_pinned() {
        assert_eq!(LISTING_BODY_CAP_BYTES, 8 * 1024 * 1024);
    }

    #[tokio::test]
    async fn oversized_listing_body_is_truncated_to_unsupported() {
        // 自审补钉:2xx body 限长读取(LISTING_BODY_CAP_BYTES)——恶意超大
        // 列表在截断处 JSON 解析失败,按「形状不认识」归 Unsupported(200),
        // 绝不无界吃内存,也绝不把半截数据当 Listed。
        let oversized = format!("{{\"data\":[{}", "1,".repeat(5 * 1024 * 1024));
        assert!(oversized.len() > 8 * 1024 * 1024);
        let mock = models_mock::spawn(&[("/models", 200, oversized)]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Unsupported);
        assert_eq!(listing.http_status, Some(200));
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
        // show 补查走 60s 共享缓存;测试间端口可能复用,先清缓存防串扰。
        clear_ollama_show_cache();
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

    /// minor-6 钉:裸 base 的本地 vLLM——kind 判别确认 `/v1/models` 可服务
    /// (fetch_v1_models),但 OpenAI 兼容阶梯先打 `{base}/models` 404。
    /// 仅本地路径在 Unsupported(404/405) 且 base 不带 /v1 时补一发
    /// `{base}/v1/models`,否则健康 vLLM 被钉成 Unsupported(404)。远端
    /// 不走本地短路,行为不变。
    #[tokio::test]
    async fn bare_local_base_retries_v1_models_after_404() {
        let mock = models_mock::spawn(&[
            (
                "/v1/models",
                200,
                r#"{"data":[{"id":"vllm-bare","max_model_len":16384}]}"#.into(),
            ),
            ("/models", 404, "{}".into()),
        ]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.server_kind.as_deref(), Some("generic"));
        assert_endpoint(model_of(&listing, "vllm-bare"), Some(16_384), None);
        // kind 判别的 fetch_v1_models(裸 base 主候选)+ 阶梯回退各一次;
        // /models 只被阶梯首发命中一次,且 404/405 才回退(只补一发)。
        assert_eq!(mock.hits_for("/models"), 1);
        assert_eq!(mock.hits_for("/v1/models"), 2);
        // 带 /v1 结尾的 base 不回退(主候选 404 就是终局);两次命中 =
        // kind 判别的 fetch_v1_models + 阶梯首发,绝无第三发回退。
        let mock = models_mock::spawn(&[("/v1/models", 404, "{}".into())]);
        let base = format!("{}/v1", mock.base_url);
        let listing = probe(&base, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Unsupported);
        assert_eq!(listing.http_status, Some(404));
        assert_eq!(mock.hits_for("/v1/models"), 2, "/v1 形 base 无回退候选");
    }

    /// minor-7 钉:kind 判别(/api/tags 200)确认 Ollama 后,列表探测与
    /// 失败复核连吃 5xx——按真实失败收口 Error{http:500}(server_kind 仍
    /// 是 ollama),绝不回落 OpenAI 兼容阶梯钉成 Unsupported(404)。删掉
    /// [`ollama_native_failure`] 接线时本测试变红:回落阶梯在裸 base 上
    /// 拿到 /models 与 /v1/models 双 404,终局 Unsupported(404)。
    #[tokio::test]
    async fn confirmed_ollama_native_failure_reports_real_error() {
        // show 补查走 60s 共享缓存;测试间端口可能复用,先清缓存防串扰。
        clear_ollama_show_cache();
        let mock = models_mock::spawn_seq_status(&[(
            "/api/tags",
            vec![
                (200, r#"{"models":[{"name":"qwen3:8b"}]}"#.to_string()),
                (500, r#"{"error":"busy"}"#.to_string()),
            ],
        )]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::OpenAiCompatible).await;
        assert_eq!(listing.status, ListingStatus::Error);
        assert_eq!(listing.http_status, Some(500));
        assert_eq!(listing.transport, None);
        assert_eq!(
            listing.server_kind.as_deref(),
            Some("ollama"),
            "kind 已确认,失败分类不得丢掉 server_kind"
        );
        assert_eq!(
            mock.hits_for("/api/tags"),
            3,
            "kind 判别 + 列表探测 + 失败复核各一次"
        );
        // OpenAI 阶梯绝不回落:裸 base 的 kind 判别自身会打 /v1/models 与
        // /models(fetch_v1_models 主候选 + 404 回退)各一次——那是判别
        // 探测,不是阶梯。阶梯若回落,/v1/models 会再 +1(裸 base 的
        // /v1 回退)。删掉失败复核接线时这里翻成 2。
        assert_eq!(mock.hits_for("/v1/models"), 1);
        assert_eq!(mock.hits_for("/models"), 1);
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

    /// M1 钉(自审补钉):禁用重定向的专用客户端下 302 不跟随,按
    /// [`ProbeOutcome::HttpError`] 归 error。删掉显式 3xx 分支时本测试变红
    /// (302 落进 body 读取 → `Unsupported`);同时钉住 G3:第二跳从不发生
    /// (Location 目标 0 命中),携带 key 的请求不会重发给从未点名的主机。
    #[tokio::test]
    async fn redirect_is_not_followed_and_classified_as_error() {
        let mock = models_mock::spawn_redirect("/v1/models", "/models");
        let listing = probe(
            &mock.base_url,
            Some("sk-ant-redirect-secret"),
            DiscoveryProtocol::Anthropic,
        )
        .await;
        assert_eq!(listing.status, ListingStatus::Error);
        assert_eq!(listing.http_status, Some(302));
        assert_eq!(listing.transport, None);
        assert!(listing.models.is_empty());
        assert_eq!(mock.hits_for("/v1/models"), 1, "首跳发生一次");
        assert_eq!(
            mock.hits_for("/models"),
            0,
            "禁用重定向:Location 目标(第二跳)从不被请求,key 不重发"
        );
    }

    /// minor-1 钉:2xx 头已到、body 中途停住(声明超长 content-length 后只
    /// 写几个字节并保持连接)——逐请求超时打到 body 读取段,分类必须是
    /// timeout(可重试)而非兜底 network。把 chunk 错误整体折叠进 MidStream
    /// 的旧实现让前端拿到 network/http_error 提示,丢失重试语义。
    #[tokio::test]
    async fn mid_body_stall_classifies_as_timeout() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stalled listener");
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept probe connection");
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            // 半截 200 应答:content-length 声明远大于实际写出的字节,写完
            // 即挂住(不关流)——客户端的 chunk() 只能等到超时。
            let _ = std::io::Write::write_all(
                &mut stream,
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 512\r\nconnection: close\r\n\r\n",
            );
            let _ = std::io::Write::write_all(&mut stream, br#"{"data":[{"id":"partial""#);
            // 挂到客户端超时之后;限时后随 drop 收尾,不泄漏线程。
            std::thread::sleep(Duration::from_secs(2));
        });
        // anthropic 协议不做本地短路,阶梯直接打裸监听 socket。
        let listing = probe_endpoint_models_with_timeout(
            &format!("http://{addr}/"),
            None,
            DiscoveryProtocol::Anthropic,
            Duration::from_millis(300),
        )
        .await;
        assert_eq!(listing.status, ListingStatus::Error);
        assert_eq!(listing.http_status, None, "超时不是 HTTP 状态失败");
        assert_eq!(listing.transport.as_deref(), Some("timeout"));
        assert!(listing.models.is_empty());
        server.join().expect("stall thread joins");
    }

    /// G1 钉:2xx body 中途断流(声明长度没写满即断开)→
    /// `BrokenBody` → Error + transport=network + http_status=200。
    /// 把 MidStream 折叠进 Unusable 的旧实现把可重试的传输失败钉成
    /// Unsupported,本测试随之变红(status 翻成 unsupported)。
    #[tokio::test]
    async fn truncated_body_is_error_network_with_http_status() {
        let mock = models_mock::spawn_truncated(
            "/v1/models",
            200,
            r#"{"data":[{"id":"claude-trunc""#,
            512,
        );
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::Anthropic).await;
        assert_eq!(listing.status, ListingStatus::Error);
        assert_eq!(listing.transport.as_deref(), Some("network"));
        assert_eq!(listing.http_status, Some(200));
        assert!(listing.models.is_empty());
        assert!(!listing.truncated);
    }

    /// minor-3 钉:TLS 裸文本启发式与连接测试(`transport_code_for`)共用
    /// 一份关键词表——证书校验 / 握手失败在发现阶梯补 "tls" 类别而不是落
    /// 兜底 network;普通连接失败不误报 tls。reqwest 错误无法脱离真实
    /// TLS 握手构造,类别臂的集成覆盖由关键词启发式的单测 + 顺序
    /// (timeout/dns/connect 先行)守口。
    #[test]
    fn raw_error_looks_like_tls_pins_keyword_heuristic() {
        assert!(raw_error_looks_like_tls(
            "error sending request: certificate verify failed"
        ));
        assert!(raw_error_looks_like_tls("tls handshake failure"));
        assert!(raw_error_looks_like_tls(
            "SSL routines::wrong version number"
        ));
        assert!(!raw_error_looks_like_tls("connection refused"));
        assert!(!raw_error_looks_like_tls("operation was canceled"));
        assert!(!raw_error_looks_like_tls(""));
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

    /// minor-2 钉:原生翻页阶梯(anthropic)与 OpenAI 阶梯同一总量上限——
    /// 3 页 × 900 = 2700 条在 2000 处截断并置 truncated;恰好 2000(2 页 ×
    /// 1000)不截。删掉 [`finish_native_listing`] 的截断时第一段变红
    /// (len 翻成 2700)。
    #[tokio::test]
    async fn anthropic_native_listing_cap_truncates_at_2000() {
        let page_body = |start: usize, count: usize, has_more: bool| {
            let entries = (start..start + count)
                .map(|index| format!(r#"{{"id":"m{index}"}}"#))
                .collect::<Vec<_>>()
                .join(",");
            format!(r#"{{"data":[{entries}],"has_more":{has_more}}}"#)
        };
        let mock = models_mock::spawn_seq(&[(
            "/v1/models",
            200,
            vec![
                page_body(0, 900, true),
                page_body(900, 900, true),
                page_body(1800, 900, false),
            ],
        )]);
        let listing = probe(
            &mock.base_url,
            Some("sk-ant-cap"),
            DiscoveryProtocol::Anthropic,
        )
        .await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.models.len(), 2000, "3×900 必须在 2000 处截断");
        assert!(listing.truncated);
        assert_eq!(model_of(&listing, "m0").id, "m0");
        assert_eq!(model_of(&listing, "m1999").id, "m1999");
        // 边界:恰好 2000 不截(与 OpenAI 阶梯 openai_listing_cap 同语义)。
        let mock = models_mock::spawn_seq(&[(
            "/v1/models",
            200,
            vec![page_body(0, 1000, true), page_body(1000, 1000, false)],
        )]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::Anthropic).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.models.len(), 2000);
        assert!(!listing.truncated, "恰好 2000 不置 truncated");
    }

    /// minor-2 钉(gemini 侧):同上一条的上限与边界语义。
    #[tokio::test]
    async fn gemini_native_listing_cap_truncates_at_2000() {
        let page_body = |start: usize, count: usize, token: Option<&str>| {
            let entries = (start..start + count)
                .map(|index| {
                    format!(
                        r#"{{"name":"models/g{index}","supportedGenerationMethods":["generateContent"]}}"#
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            match token {
                Some(token) => format!(r#"{{"models":[{entries}],"nextPageToken":"{token}"}}"#),
                None => format!(r#"{{"models":[{entries}]}}"#),
            }
        };
        let mock = models_mock::spawn_seq(&[(
            "/v1beta/models",
            200,
            vec![
                page_body(0, 900, Some("t1")),
                page_body(900, 900, Some("t2")),
                page_body(1800, 900, None),
            ],
        )]);
        let listing = probe(&mock.base_url, Some("gkey"), DiscoveryProtocol::Gemini).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.models.len(), 2000, "3×900 必须在 2000 处截断");
        assert!(listing.truncated);
        // 边界:恰好 2000 不截。
        let mock = models_mock::spawn_seq(&[(
            "/v1beta/models",
            200,
            vec![page_body(0, 1000, Some("t1")), page_body(1000, 1000, None)],
        )]);
        let listing = probe(&mock.base_url, None, DiscoveryProtocol::Gemini).await;
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.models.len(), 2000);
        assert!(!listing.truncated, "恰好 2000 不置 truncated");
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
        // show 补查走 60s 共享缓存;测试间端口可能复用,先清缓存防串扰。
        clear_ollama_show_cache();
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
