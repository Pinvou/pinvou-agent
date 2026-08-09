//! 模型服务端点（URL / 协议）层面的共用判定与直连：连接测试
//! （app/commands/settings.rs）与运行状态探测（features/monitor）都直连
//! `{base}/models`，鉴权方式与探测地址必须同一口径；品悟（features/review）与
//! 记忆回顾（features/memory）选 Anthropic preset 时走 Messages 原生协议，
//! 鉴权与地址口径与上述探测一致。

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

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
    let host = strip_v1_suffix(base_url)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    if let Ok(resp) = client
        .get(format!("{host}/api/v0/models"))
        .send()
        .await
        .and_then(|r| r.error_for_status())
    {
        if let Ok(v) = resp.json::<serde_json::Value>().await {
            if let Some(models) = parse_lmstudio_v0_models(&v) {
                return Some(OpenAiModelsProbe { models });
            }
        }
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

/// 探测本地推理服务类型。只应在 loopback 端点（`base_url_uses_loopback`）上调用；
/// 判定顺序按特征端点互斥性排列：Ollama（`/api/tags`）→ LM Studio（`/api/v0/models`）
/// → vLLM（`/v1/models` 的 `owned_by`）→ 通用。探测失败（服务未启动/超时）返回
/// `Generic`，调用方保持既有 openai wire route，不因探测失败改变行为。
pub async fn probe_local_server_kind(base_url: &str) -> LocalServerKind {
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

/// 探测 vLLM：`/v1/models` 响应中任一模型 `owned_by == "vllm"`（vLLM 标准实现）。
/// 探测地址与 `probe_vllm_model_info` 同一口径：upstream 带 `/v1` 直接拼 `/models`，
/// 不带则补 `/v1/models`。
async fn probe_vllm_owned(base_url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    else {
        return false;
    };
    let url = if base_url.trim_end_matches('/').ends_with("/v1") {
        format!("{}/models", base_url.trim_end_matches('/'))
    } else {
        format!("{}/v1/models", base_url.trim_end_matches('/'))
    };
    let Ok(resp) = client.get(url).send().await else {
        return false;
    };
    if !resp.status().is_success() {
        return false;
    }
    let Ok(v) = resp.json::<serde_json::Value>().await else {
        return false;
    };
    v.get("data").and_then(Value::as_array).is_some_and(|items| {
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
}
