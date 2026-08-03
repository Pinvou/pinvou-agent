//! 模型服务端点（URL / 协议）层面的共用直连：品悟（features/review）与记忆回顾
//! （features/memory）选 Anthropic preset 时走 Messages 原生协议，鉴权与地址
//! 口径必须和连接测试（app/commands/settings.rs）、运行状态探测
//! （features/monitor）一致。

use anyhow::{Context, Result};
use serde_json::Value;

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
