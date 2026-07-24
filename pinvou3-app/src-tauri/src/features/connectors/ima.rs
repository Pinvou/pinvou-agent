//! Tencent IMA OpenAPI Skill connector.
//!
//! IMA is intentionally not registered as an MCP server. The marketplace card
//! stores the user's OpenAPI credentials, validates them against ima.qq.com,
//! then installs the `ima-skills` SKILL.md package so the assistant can call
//! the bundled local Node helper from shell.

use serde::Serialize;
use serde_json::{json, Value};

use crate::features::marketplace::skill_marketplace::SkillMarketplaceManager;
use crate::platform::credential_store::{
    redact_secret, CredentialReference, CredentialStore, SystemCredentialStore,
};

const CLIENT_ID_SECRET: &str = "client_id";
const API_KEY_SECRET: &str = "api_key";
const IMA_SKILL_ID: &str = "ima-skills";
const IMA_BASE_URL: &str = "https://ima.qq.com";
const IMA_SKILL_VERSION: &str = "1.1.8";

#[derive(Debug, Clone, Serialize)]
struct ImaConnectorStatus {
    connected: bool,
    credentials_present: bool,
    skill_installed: bool,
}

fn client_id_ref() -> CredentialReference {
    CredentialReference::for_ima_secret(CLIENT_ID_SECRET)
}

fn api_key_ref() -> CredentialReference {
    CredentialReference::for_ima_secret(API_KEY_SECRET)
}

fn skill_installed() -> bool {
    SkillMarketplaceManager::new()
        .list_skills()
        .into_iter()
        .any(|skill| skill.id == IMA_SKILL_ID && skill.installed)
}

fn set_ima_env(client_id: &str, api_key: &str) {
    // The WorkBuddy IMA helper accepts both naming conventions. Export both so
    // the bundled skill and any user-authored snippets behave consistently.
    std::env::set_var("IMA_CLIENT_ID", client_id);
    std::env::set_var("IMA_API_KEY", api_key);
    std::env::set_var("IMA_OPENAPI_CLIENTID", client_id);
    std::env::set_var("IMA_OPENAPI_APIKEY", api_key);
}

fn clear_ima_env() {
    for key in [
        "IMA_CLIENT_ID",
        "IMA_API_KEY",
        "IMA_OPENAPI_CLIENTID",
        "IMA_OPENAPI_APIKEY",
    ] {
        std::env::remove_var(key);
    }
}

fn credentials<S: CredentialStore>(store: &S) -> Result<Option<(String, String)>, String> {
    let client_id = store.get(&client_id_ref()).map_err(|e| e.user_message())?;
    let api_key = store.get(&api_key_ref()).map_err(|e| e.user_message())?;
    Ok(match (client_id, api_key) {
        (Some(client_id), Some(api_key))
            if !client_id.trim().is_empty() && !api_key.trim().is_empty() =>
        {
            Some((client_id, api_key))
        }
        _ => None,
    })
}

pub fn sync_ima_env_from_credentials() {
    match credentials(&SystemCredentialStore::new()) {
        Ok(Some((client_id, api_key))) => set_ima_env(&client_id, &api_key),
        Ok(None) => clear_ima_env(),
        Err(err) => {
            eprintln!("[ima] credential sync skipped: {err}");
            clear_ima_env();
        }
    }
}

fn status_with_store<S: CredentialStore>(store: &S) -> Result<ImaConnectorStatus, String> {
    let credentials_present = match credentials(store)? {
        Some((client_id, api_key)) => {
            set_ima_env(&client_id, &api_key);
            true
        }
        None => {
            clear_ima_env();
            false
        }
    };
    let skill_installed = skill_installed();
    Ok(ImaConnectorStatus {
        connected: credentials_present && skill_installed,
        credentials_present,
        skill_installed,
    })
}

async fn validate_credentials(client_id: &str, api_key: &str) -> Result<(), String> {
    if client_id.trim().is_empty() || api_key.trim().is_empty() {
        return Err("请填写 IMA Client ID 和 API Key。".to_string());
    }

    let base_url = std::env::var("PINVOU3_IMA_BASE_URL").unwrap_or_else(|_| IMA_BASE_URL.into());
    let url = format!(
        "{}/openapi/check_skill_update",
        base_url.trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("创建 IMA 校验客户端失败: {e}"))?;
    let response = client
        .post(url)
        .header("ima-openapi-clientid", client_id)
        .header("ima-openapi-apikey", api_key)
        .header(
            "ima-openapi-ctx",
            format!("skill_version={IMA_SKILL_VERSION}"),
        )
        .json(&json!({ "version": IMA_SKILL_VERSION }))
        .send()
        .await
        .map_err(|e| format!("连接 IMA 失败，请检查网络或代理: {e}"))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| format!("读取 IMA 校验响应失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("IMA 校验失败，HTTP 状态码 {status}。"));
    }

    let parsed = serde_json::from_str::<Value>(&text)
        .map_err(|_| "IMA 校验响应不是合法 JSON，请稍后重试。".to_string())?;
    let code = parsed
        .get("code")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "IMA 校验响应缺少 code 字段，请稍后重试。".to_string())?;
    if code == 0 {
        return Ok(());
    }
    let message = parsed
        .get("msg")
        .and_then(|v| v.as_str())
        .unwrap_or("IMA OpenAPI 鉴权失败，请确认 Client ID / API Key。");
    Err(redact_secret(message))
}

pub async fn ima_status() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let status = status_with_store(&SystemCredentialStore::new())?;
        serde_json::to_value(status).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

pub async fn ima_connect(client_id: String, api_key: String) -> Result<Value, String> {
    validate_credentials(&client_id, &api_key).await?;

    tokio::task::spawn_blocking(move || {
        let store = SystemCredentialStore::new();
        let previous_client_id = store.get(&client_id_ref()).map_err(|e| e.user_message())?;
        let previous_api_key = store.get(&api_key_ref()).map_err(|e| e.user_message())?;

        let result = (|| -> Result<(), String> {
            store
                .set(&client_id_ref(), client_id.trim())
                .map_err(|e| e.user_message())?;
            store
                .set(&api_key_ref(), api_key.trim())
                .map_err(|e| e.user_message())?;
            set_ima_env(client_id.trim(), api_key.trim());
            SkillMarketplaceManager::new().install(IMA_SKILL_ID)?;
            crate::features::marketplace::skill_marketplace::refresh_disabled_skills();
            Ok(())
        })();

        if let Err(err) = result {
            rollback_secret(&store, &client_id_ref(), previous_client_id)?;
            rollback_secret(&store, &api_key_ref(), previous_api_key)?;
            sync_ima_env_from_credentials();
            return Err(err);
        }

        Ok::<Value, String>(json!({ "ok": true, "connected": true }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

fn rollback_secret<S: CredentialStore>(
    store: &S,
    reference: &CredentialReference,
    previous: Option<String>,
) -> Result<(), String> {
    match previous {
        Some(value) => store.set(reference, &value).map_err(|e| e.user_message()),
        None => store.delete(reference).map_err(|e| e.user_message()),
    }
}

pub async fn ima_logout() -> Result<Value, String> {
    tokio::task::spawn_blocking(|| {
        let store = SystemCredentialStore::new();
        let _ = store.delete(&client_id_ref());
        let _ = store.delete(&api_key_ref());
        clear_ima_env();
        let _ = SkillMarketplaceManager::new().uninstall(IMA_SKILL_ID);
        crate::features::marketplace::skill_marketplace::refresh_disabled_skills();
        Ok::<Value, String>(json!({ "ok": true, "connected": false }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::credential_store::MemoryCredentialStore;

    #[test]
    fn status_requires_both_credentials() {
        let store = MemoryCredentialStore::default();
        assert!(!status_with_store(&store).unwrap().credentials_present);
        store.set(&client_id_ref(), "client").unwrap();
        assert!(!status_with_store(&store).unwrap().credentials_present);
        store.set(&api_key_ref(), "api-key").unwrap();
        assert!(status_with_store(&store).unwrap().credentials_present);
    }

    #[test]
    fn sets_both_ima_env_naming_conventions() {
        let previous = [
            ("IMA_CLIENT_ID", std::env::var("IMA_CLIENT_ID").ok()),
            ("IMA_API_KEY", std::env::var("IMA_API_KEY").ok()),
            (
                "IMA_OPENAPI_CLIENTID",
                std::env::var("IMA_OPENAPI_CLIENTID").ok(),
            ),
            (
                "IMA_OPENAPI_APIKEY",
                std::env::var("IMA_OPENAPI_APIKEY").ok(),
            ),
        ];
        set_ima_env("client", "api");
        assert_eq!(std::env::var("IMA_CLIENT_ID").as_deref(), Ok("client"));
        assert_eq!(std::env::var("IMA_API_KEY").as_deref(), Ok("api"));
        assert_eq!(
            std::env::var("IMA_OPENAPI_CLIENTID").as_deref(),
            Ok("client")
        );
        assert_eq!(std::env::var("IMA_OPENAPI_APIKEY").as_deref(), Ok("api"));

        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn rollback_restores_previous_secret() {
        let store = MemoryCredentialStore::default();
        let reference = client_id_ref();
        store.set(&reference, "old").unwrap();
        rollback_secret(&store, &reference, Some("prev".into())).unwrap();
        assert_eq!(store.get(&reference).unwrap().as_deref(), Some("prev"));
        rollback_secret(&store, &reference, None).unwrap();
        assert_eq!(store.get(&reference).unwrap(), None);
    }

    #[test]
    fn validation_error_redacts_secret_like_message() {
        let redacted = redact_secret("apikey 鉴权失败 sk-secret-token-1234567890");
        assert!(!redacted.contains("sk-secret-token"));
    }

    #[test]
    fn config_keys_match_frontend_contract() {
        use std::collections::HashMap;

        let keys = HashMap::from([
            ("IMA_CLIENT_ID".to_string(), CLIENT_ID_SECRET.to_string()),
            ("IMA_API_KEY".to_string(), API_KEY_SECRET.to_string()),
        ]);
        assert_eq!(
            keys.get("IMA_CLIENT_ID").map(String::as_str),
            Some("client_id")
        );
        assert_eq!(keys.get("IMA_API_KEY").map(String::as_str), Some("api_key"));
    }
}
