use serde_json::{json, Value};
use std::time::Duration;

const WENDAO_ENDPOINT: &str = "https://externalcallback.ctrip.com/skills/api/crew/qclaw/searchInfo";

#[tauri::command]
pub async fn query_ctrip_wendao(token: String, query: String) -> Result<String, String> {
    let token = token.trim();
    let query = query.trim();
    if token.is_empty() {
        return Err("缺少携程问道 API Token".to_string());
    }
    if query.is_empty() {
        return Err("缺少携程问道查询内容".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|error| format!("创建携程问道请求客户端失败: {error}"))?;

    let response = client
        .post(WENDAO_ENDPOINT)
        .json(&json!({
            "inputs": {
                "token": token,
                "query": query,
            }
        }))
        .send()
        .await
        .map_err(|error| format!("携程问道请求失败: {error}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("携程问道请求失败: HTTP {status}"));
    }

    let data = response
        .json::<Value>()
        .await
        .map_err(|error| format!("解析携程问道响应失败: {error}"))?;

    normalize_result(&data).ok_or_else(|| "携程问道没有返回可展示的 result".to_string())
}

fn normalize_result(data: &Value) -> Option<String> {
    match data.get("result").unwrap_or(data) {
        Value::String(value) => non_empty(value),
        Value::Object(map) => map
            .get("content")
            .and_then(Value::as_str)
            .and_then(non_empty)
            .or_else(|| map.get("text").and_then(Value::as_str).and_then(non_empty))
            .or_else(|| non_empty(&Value::Object(map.clone()).to_string())),
        other => non_empty(&other.to_string()),
    }
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
