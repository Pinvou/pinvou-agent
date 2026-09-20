//! Tencent ima OpenAPI connector and model-facing tool.
//!
//! Credentials stay in the local credential store. The model can only choose
//! from an exact API-path allowlist; it cannot supply a host, URL, headers, or
//! credentials.

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};

use deepseek_tui::tools::spec::{ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec};

use crate::features::marketplace::skill_marketplace::SkillMarketplaceManager;
use crate::platform::credential_store::{
    CredentialReference, CredentialStore, SystemCredentialStore, redact_secret,
};

const CLIENT_ID_SECRET: &str = "client_id";
const API_KEY_SECRET: &str = "api_key";
const IMA_SKILL_ID: &str = "ima-skills";
const IMA_BASE_URL: &str = "https://ima.qq.com";
const IMA_SKILL_VERSION: &str = "1.1.8";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

const READ_API_PATHS: &[&str] = &[
    "openapi/check_skill_update",
    "openapi/note/v1/search_note",
    "openapi/note/v1/list_notebook",
    "openapi/note/v1/list_note",
    "openapi/note/v1/get_doc_content",
    "openapi/wiki/v1/search_knowledge_base",
    "openapi/wiki/v1/get_knowledge_base",
    "openapi/wiki/v1/get_knowledge_list",
    "openapi/wiki/v1/search_knowledge",
    "openapi/wiki/v1/get_addable_knowledge_base_list",
    "openapi/wiki/v1/get_media_info",
];

const WRITE_API_PATHS: &[&str] = &[
    "openapi/note/v1/import_doc",
    "openapi/note/v1/append_doc",
    "openapi/wiki/v1/import_urls",
    "openapi/wiki/v1/add_knowledge",
];

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

fn status_with_store<S: CredentialStore>(store: &S) -> Result<ImaConnectorStatus, String> {
    let credentials_present = credentials(store)?.is_some();
    let skill_installed = skill_installed();
    Ok(ImaConnectorStatus {
        connected: credentials_present && skill_installed,
        credentials_present,
        skill_installed,
    })
}

fn is_allowed_api_path(api_path: &str) -> bool {
    READ_API_PATHS.contains(&api_path) || WRITE_API_PATHS.contains(&api_path)
}

fn is_read_api_path(api_path: &str) -> bool {
    READ_API_PATHS.contains(&api_path)
}

async fn read_json_response(response: reqwest::Response) -> Result<Value, String> {
    let status = response.status();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("读取 IMA 响应失败: {e}"))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err("IMA 响应超过 1 MiB 安全上限。".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        return Err(format!("IMA 请求失败，HTTP 状态码 {status}。"));
    }
    serde_json::from_slice(&bytes).map_err(|_| "IMA 响应不是合法 JSON，请稍后重试。".to_string())
}

async fn request_ima(
    client_id: &str,
    api_key: &str,
    api_path: &str,
    body: &Value,
) -> Result<Value, String> {
    if !is_allowed_api_path(api_path) {
        return Err("不支持的 IMA API 路径。".to_string());
    }
    if !body.is_object() {
        return Err("IMA 请求 body 必须是 JSON object。".to_string());
    }

    // Process-wide shared client: building a Client per call re-creates the
    // TLS config/connection pool, wasting all keep-alive/h2 reuse against
    // the same host (ima.qq.com). The timeout moves to per-request
    // (reqwest::RequestBuilder::timeout), keeping the same 30s.
    // Two OnceLock caveats:
    // 1. reqwest enables system-proxy detection by default; the proxy
    // config is snapshotted at first build and never re-read for the
    // process lifetime — changing the system proxy mid-session needs an
    // app restart to take effect.
    // 2. A build failure (TLS/system config unavailable) is cached
    // process-wide as Err with no per-call retry (retrying an identical
    // failure is pointless; Client::default() panics on the same failure
    // and is not a usable fallback). Request-level errors (connection
    // refused/timeout) are unaffected by the cache and still propagate
    // per call.
    static CLIENT: std::sync::OnceLock<Result<reqwest::Client, String>> =
        std::sync::OnceLock::new();
    let client = CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .build()
                .map_err(|e| format!("创建 IMA 客户端失败: {e}"))
        })
        .as_ref()
        .map_err(Clone::clone)?;
    let response = client
        .post(format!("{IMA_BASE_URL}/{api_path}"))
        .timeout(std::time::Duration::from_secs(30))
        .header("ima-openapi-clientid", client_id)
        .header("ima-openapi-apikey", api_key)
        .header(
            "ima-openapi-ctx",
            format!("skill_version={IMA_SKILL_VERSION}"),
        )
        .json(body)
        .send()
        .await
        .map_err(|e| format!("连接 IMA 失败，请检查网络或代理: {e}"))?;
    read_json_response(response).await
}

async fn validate_credentials(client_id: &str, api_key: &str) -> Result<(), String> {
    if client_id.trim().is_empty() || api_key.trim().is_empty() {
        return Err("请填写 IMA Client ID 和 API Key。".to_string());
    }

    let parsed = request_ima(
        client_id.trim(),
        api_key.trim(),
        "openapi/check_skill_update",
        &json!({ "version": IMA_SKILL_VERSION }),
    )
    .await?;
    let code = parsed
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| "IMA 校验响应缺少 code 字段，请稍后重试。".to_string())?;
    if code == 0 {
        return Ok(());
    }
    let message = parsed
        .get("msg")
        .and_then(Value::as_str)
        .unwrap_or("IMA OpenAPI 鉴权失败，请确认 Client ID / API Key。");
    Err(redact_secret(message))
}

/// Model-facing IMA tool. It owns no credential or endpoint input surface.
pub struct ImaOpenApiTool;

impl ImaOpenApiTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ImaOpenApiTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolSpec for ImaOpenApiTool {
    fn name(&self) -> &str {
        "ima_openapi"
    }

    fn description(&self) -> &str {
        "调用腾讯 ima 官方 OpenAPI，操作用户已连接的 ima 笔记与知识库。\
         仅接受受控的 api_path 和 JSON body；凭据由 Pinvou 从本机系统凭据读取，\
         不要询问、传入或输出 Client ID、API Key、请求头。"
    }

    fn input_schema(&self) -> Value {
        let paths: Vec<&str> = READ_API_PATHS
            .iter()
            .chain(WRITE_API_PATHS.iter())
            .copied()
            .collect();
        json!({
            "type": "object",
            "properties": {
                "api_path": {
                    "type": "string",
                    "enum": paths,
                    "description": "要调用的 ima OpenAPI 路径"
                },
                "body": {
                    "type": "object",
                    "description": "发送给 ima OpenAPI 的 JSON 请求体"
                }
            },
            "required": ["api_path", "body"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::Network]
    }

    fn is_read_only_for(&self, input: &Value) -> bool {
        input
            .get("api_path")
            .and_then(Value::as_str)
            .is_some_and(is_read_api_path)
    }

    fn supports_parallel_for(&self, input: &Value) -> bool {
        self.is_read_only_for(input)
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let api_path = input
            .get("api_path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::missing_field("api_path"))?
            .trim()
            .to_string();
        if !is_allowed_api_path(&api_path) {
            return Err(ToolError::invalid_input("不支持的 IMA API 路径。"));
        }
        let body = input
            .get("body")
            .cloned()
            .ok_or_else(|| ToolError::missing_field("body"))?;
        if !body.is_object() {
            return Err(ToolError::invalid_input(
                "IMA 请求 body 必须是 JSON object。",
            ));
        }

        let credentials = tokio::task::spawn_blocking(|| {
            credentials(&SystemCredentialStore::new())
                .map_err(|e| redact_secret(&e))?
                .ok_or_else(|| {
                    "未找到 IMA 凭据。请先在 Pinvou 插件中心连接「腾讯 ima」。".to_string()
                })
        })
        .await
        .map_err(|e| ToolError::execution_failed(format!("读取 IMA 凭据失败: {e}")))?
        .map_err(ToolError::execution_failed)?;

        let response = request_ima(&credentials.0, &credentials.1, &api_path, &body)
            .await
            .map_err(|e| ToolError::execution_failed(redact_secret(&e)))?;
        let response = serde_json::to_string(&response)
            .map_err(|e| ToolError::execution_failed(format!("序列化 IMA 响应失败: {e}")))?;
        Ok(ToolResult::success(redact_known_credentials(
            response,
            &credentials.0,
            &credentials.1,
        )))
    }
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
        ima_connect_sync_with_store(client_id, api_key, &SystemCredentialStore::new())
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// Testable connect core (injected store, mirrors `status_with_store`): tests
/// pass a `MemoryCredentialStore` so no thread ever touches the real system
/// credential vault.
fn ima_connect_sync_with_store<S: CredentialStore>(
    client_id: String,
    api_key: String,
    store: &S,
) -> Result<Value, String> {
    let previous_client_id = store.get(&client_id_ref()).map_err(|e| e.user_message())?;
    let previous_api_key = store.get(&api_key_ref()).map_err(|e| e.user_message())?;

    let result = (|| -> Result<(), String> {
        // Deny-first (#517 review round 4): register the deny entry BEFORE
        // the credentials are touched or the skill content is replaced —
        // a refusal aborts with nothing changed at all. A connect
        // re-runs `install` over an existing skill, so an uninstall-based
        // rollback here would destroy the user's pre-existing copy.
        // 新装技能默认加入 DenyAll scope（当前 code）禁用集（外部能力显式
        // 开启）；在线会话组合目录
        // 由命令层（connectors::ima_connect）重写。
        // 注意引用 marketplace::skill_scope（持久化层）而非 assistant：避免
        // connectors → assistant 依赖环（架构守卫 rust_feature_cycles）。
        crate::features::marketplace::skill_scope::sync_deny_all_scopes_after_skill_install(
            IMA_SKILL_ID,
        )?;
        store
            .set(&client_id_ref(), client_id.trim())
            .map_err(|e| e.user_message())?;
        store
            .set(&api_key_ref(), api_key.trim())
            .map_err(|e| e.user_message())?;
        SkillMarketplaceManager::new().install(IMA_SKILL_ID)?;
        Ok(())
    })();

    if let Err(err) = result {
        // Credentials are restored to their previous values; the skill is
        // left exactly as found (deny-first above already refused before
        // anything was replaced).
        rollback_secret(store, &client_id_ref(), previous_client_id)?;
        rollback_secret(store, &api_key_ref(), previous_api_key)?;
        return Err(err);
    }

    Ok(json!({ "ok": true, "connected": true }))
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
        let client_result = store.delete(&client_id_ref());
        let api_key_result = store.delete(&api_key_ref());
        let _ = SkillMarketplaceManager::new().uninstall(IMA_SKILL_ID);
        // 已卸载技能从各 scope 禁用集清除残留；在线会话组合目录由命令层
        // （connectors::ima_logout）重写。引用 marketplace::scope 避免
        // connectors → assistant 依赖环。
        if let Err(error) =
            crate::features::marketplace::scope::remove_bundle_from_disabled_scopes(IMA_SKILL_ID)
        {
            eprintln!("[ima] scope cleanup for {IMA_SKILL_ID} skipped: {error}");
        }
        client_result.map_err(|e| e.user_message())?;
        api_key_result.map_err(|e| e.user_message())?;
        Ok::<Value, String>(json!({ "ok": true, "connected": false }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

fn redact_known_credentials(mut text: String, client_id: &str, api_key: &str) -> String {
    for secret in [client_id, api_key] {
        if secret.is_empty() {
            continue;
        }
        if let Ok(json_secret) = serde_json::to_string(secret) {
            text = text.replace(&json_secret, "\"[REDACTED]\"");
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::credential_store::MemoryCredentialStore;

    /// Temp-home harness (ENV_LOCK-serialized) for the connect regressions.
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-ima-connect-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        f();
        match prev {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_requires_both_credentials() {
        let store = MemoryCredentialStore::default();
        assert!(!status_with_store(&store).unwrap().credentials_present);
        store.set(&client_id_ref(), "client").unwrap();
        assert!(!status_with_store(&store).unwrap().credentials_present);
        store.set(&api_key_ref(), "api-key").unwrap();
        assert!(status_with_store(&store).unwrap().credentials_present);
    }

    /// Final semantics for a FRESH ima connect (#517 deny-first): the
    /// consent-gate registration is refused when the scope lock is
    /// unavailable, and the refusal must abort BEFORE the credentials are
    /// touched or any skill content lands.
    #[test]
    fn connect_refused_sync_aborts_before_anything_lands() {
        with_temp_home(|| {
            let store = MemoryCredentialStore::default();

            // Initialize the code scope (makes the DenyAll sync a required
            // write), then make the lock file unopenable so the sync refuses.
            crate::features::marketplace::save_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Code,
                &["seed-bundle".to_string()],
            )
            .expect("code scope must initialize while the lock works");
            let lock = crate::platform::paths::pinvou3_home().join("disabled_bundles.lock");
            std::fs::remove_file(&lock).unwrap();
            std::fs::create_dir_all(&lock).unwrap();

            let error =
                ima_connect_sync_with_store("client-v2".to_string(), "key-v2".to_string(), &store)
                    .unwrap_err();
            assert!(
                error.contains("disabled_bundles.lock"),
                "refusal must name the lock failure: {error}"
            );
            assert!(
                store.get(&client_id_ref()).unwrap().is_none(),
                "credentials must never be written on a refused connect"
            );
            assert!(
                store.get(&api_key_ref()).unwrap().is_none(),
                "credentials must never be written on a refused connect"
            );
            assert!(
                SkillMarketplaceManager::new()
                    .find_skill_dir(IMA_SKILL_ID)
                    .is_none(),
                "the skill must never land on a refused connect"
            );
        });
    }

    /// Review round 4 (#517) + the reinstall-semantics fix: a RE-connect of
    /// the already-installed ima skill is a known-bundle operation — the
    /// consent gate skips the write entirely (even with the scope lock
    /// unavailable), so the reconnect proceeds and the recorded consent
    /// state stays untouched. The old unconditional re-registration would
    /// have re-denied the enabled skill, and a post-gate failure then left
    /// it disabled with no recovery path.
    #[test]
    fn ima_reconnect_of_installed_skill_preserves_consent_state() {
        with_temp_home(|| {
            let store = MemoryCredentialStore::default();
            store.set(&client_id_ref(), "client-v1").unwrap();
            store.set(&api_key_ref(), "key-v1").unwrap();
            let skills = SkillMarketplaceManager::new();
            skills
                .install(IMA_SKILL_ID)
                .expect("skill install must succeed while the lock works");
            let skill_md = skills
                .find_skill_dir(IMA_SKILL_ID)
                .expect("precondition: ima skill installed")
                .join("SKILL.md");

            // Initialize the code scope (ima stays absent = enabled), then
            // break the lock to prove the reconnect needs no scope write.
            crate::features::marketplace::save_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Code,
                &["seed-bundle".to_string()],
            )
            .expect("code scope must initialize while the lock works");
            let lock = crate::platform::paths::pinvou3_home().join("disabled_bundles.lock");
            std::fs::remove_file(&lock).unwrap();
            std::fs::create_dir_all(&lock).unwrap();

            ima_connect_sync_with_store("client-v2".to_string(), "key-v2".to_string(), &store)
                .expect("a known bundle's reconnect must not need the consent-gate write");
            assert_eq!(
                store.get(&client_id_ref()).unwrap().as_deref(),
                Some("client-v2"),
                "the reconnect must update the credentials"
            );
            assert_eq!(
                store.get(&api_key_ref()).unwrap().as_deref(),
                Some("key-v2"),
                "the reconnect must update the credentials"
            );
            assert!(
                skill_md.is_file(),
                "the reconnected skill must still be on disk"
            );
            assert!(
                !crate::features::marketplace::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                )
                .iter()
                .any(|id| id == "ima"),
                "the reconnect must not re-deny the enabled skill"
            );
        });
    }

    #[test]
    fn api_path_allowlist_rejects_host_and_traversal_inputs() {
        assert!(is_allowed_api_path("openapi/note/v1/search_note"));
        assert!(is_allowed_api_path("openapi/wiki/v1/import_urls"));
        assert!(!is_allowed_api_path(
            "https://example.com/openapi/note/v1/search_note"
        ));
        assert!(!is_allowed_api_path("openapi/../admin"));
        assert!(!is_allowed_api_path("openapi/note/v1/unknown"));
    }

    #[test]
    fn read_only_classification_matches_operation() {
        let tool = ImaOpenApiTool::new();
        assert!(
            tool.is_read_only_for(&json!({"api_path": "openapi/note/v1/search_note", "body": {}}))
        );
        assert!(
            !tool.is_read_only_for(&json!({"api_path": "openapi/note/v1/append_doc", "body": {}}))
        );
    }

    #[test]
    fn tool_schema_exposes_only_allowlisted_path_and_body() {
        let tool = ImaOpenApiTool::new();
        let schema = tool.input_schema();
        assert_eq!(tool.name(), "ima_openapi");
        assert_eq!(schema["additionalProperties"], false);
        assert!(schema["properties"].get("base_url").is_none());
        assert!(schema["properties"].get("client_id").is_none());
        assert!(schema["properties"].get("api_key").is_none());
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
    fn tool_response_redacts_exact_credentials_without_corrupting_other_text() {
        let response = r#"{"client_id":"client-secret","api_key":"api-secret","data":"available"}"#
            .to_string();
        let redacted = redact_known_credentials(response, "client-secret", "api-secret");
        assert_eq!(
            redacted,
            r#"{"client_id":"[REDACTED]","api_key":"[REDACTED]","data":"available"}"#
        );
    }
}
