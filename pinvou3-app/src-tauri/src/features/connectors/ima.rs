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
        // A new skill joins the DenyAll scopes' (currently code) deny sets by
        // default — external capabilities stay explicitly opted in; the
        // online-session composed catalogs are rewritten by the command layer
        // (connectors::ima_connect).
        // Note: reference marketplace::scope (the persistence layer), not
        // assistant: that would form a connectors → assistant dependency
        // cycle (architecture guard rust_feature_cycles).
        // The sync itself skips known bundles, so a reconnect of the
        // already-installed ima skill never re-runs the consent write and
        // never needs the lock (review round 4 regression).
        crate::features::marketplace::scope::sync_deny_all_scopes_after_install(IMA_SKILL_ID)?;
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
        // Round-37 C5 (review #455): a rollback failure must not replace the
        // primary error (which carries the consent-guidance copy and the
        // frontend marker) nor skip the second rollback — log each rollback
        // failure and continue, letting the primary err win.
        if let Err(e) = rollback_secret(store, &client_id_ref(), previous_client_id) {
            log::warn!("[ima] rolling back the client id secret failed: {e}");
        }
        if let Err(e) = rollback_secret(store, &api_key_ref(), previous_api_key) {
            log::warn!("[ima] rolling back the API key secret failed: {e}");
        }
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
        ima_logout_sync_with_store(&store)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

/// Testable logout core (injected store, mirrors `ima_connect_sync_with_store`):
/// tests pass a `MemoryCredentialStore` so no thread ever touches the real
/// system credential vault.
fn ima_logout_sync_with_store<S: CredentialStore>(store: &S) -> Result<Value, String> {
    let client_result = store.delete(&client_id_ref());
    let api_key_result = store.delete(&api_key_ref());
    // Ordering disclosure (round-12 review B2): the credentials are deleted
    // BEFORE the uninstall attempt — the same order main uses (a failed
    // uninstall leaves the credentials deleted there too); a failed uninstall
    // leaves an on-disk skill without credentials (fail-closed direction).
    // A successful uninstall of the actually-installed skill clears its
    // leftover entries from every scope; the online-session composed catalogs
    // are rewritten by the command layer (connectors::ima_logout). The strip
    // itself runs inside the manager's `uninstall_and_strip_scope`, under
    // the same per-id import lock as the teardown (round-12 review B2);
    // marketplace::scope no longer needs to be referenced from here (the
    // direct reference avoided a connectors -> assistant dependency cycle).
    // A failed uninstall must KEEP the deny entries (round-6 review): the
    // skill is still on disk, so clearing the entries here would silently
    // restore it to callable (fail-open). Keeping them fails closed (the
    // skill stays off) and converges on the next connect/uninstall.
    // A VACUOUS uninstall (nothing installed — no record, nothing on disk)
    // must keep them too (round-10 review): stripping on `Ok(false)` would
    // remove the deny-first entry a concurrent same-id connect may have just
    // registered (its install lands after the gate), re-enabling the skill as
    // it lands — the same guard as the tool/skill uninstall channels.
    match SkillMarketplaceManager::new().uninstall_and_strip_scope(IMA_SKILL_ID) {
        Ok(true) => {
            // The scope strip already ran inside the helper, under the same
            // per-id import lock as the teardown (round-12 review B2); a
            // refused strip was logged there and keeps the entries
            // (fail-closed).
        }
        Ok(false) => {
            log::warn!(
                "[ima] logout: {IMA_SKILL_ID} was not installed; \
                 keeping its DenyAll entries (nothing legitimate to clear)"
            );
        }
        Err(error) => {
            // Failed teardown logs and returns Ok (round-25 MAJOR 4's
            // retryable-Err contract was inverted by deny-first): the
            // credentials are already gone, so retrying the logout command
            // cannot succeed differently — recovery runs through the
            // marketplace skill-uninstall lane instead. Keeping the entries
            // is the fail-closed direction (installed + denied).
            log::warn!(
                "[ima] logout: uninstall of {IMA_SKILL_ID} failed ({error}); \
                 keeping its DenyAll entries so the on-disk skill stays off"
            );
        }
    }
    client_result.map_err(|e| e.user_message())?;
    api_key_result.map_err(|e| e.user_message())?;
    Ok(json!({ "ok": true, "connected": false }))
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
    /// Round-32 minor 10 (review #455): the shared consent-failure marker is
    /// the exact string the frontend matches (ToolStoreView
    /// `consentFailure`); rewording it without updating the frontend matcher
    /// would silently degrade the en/ja guidance to the generic copy. The
    /// ima-local alias of the marker was production-dead since deny-first
    /// (a refused gate aborts before anything lands, so ima connect surfaces
    /// the raw refusal instead of a wrapped marker copy) — removed in the
    /// round-15 pass; the contract pin lives on against the shared constant.
    #[test]
    fn consent_failure_marker_matches_the_frontend_contract() {
        assert_eq!(
            crate::features::marketplace::scope::CONSENT_SYNC_FAILURE_MARKER,
            "persisting their default-off consent state failed",
            "the marker value is the frontend contract (ToolStoreView consentFailure) — update both sides together"
        );
    }

    use super::*;
    use crate::platform::credential_store::MemoryCredentialStore;

    /// Temp-home harness for the connect regressions. Delegated to the
    /// shared RAII helper (round 9): a failing assertion unwinds past a
    /// straight-line env restore, which would leave PINVOU3_HOME pointed at
    /// a deleted temp dir and cascade unrelated failures.
    fn with_temp_home<F: FnOnce()>(f: F) {
        crate::platform::test_support::with_temp_home("pinvou3-ima-connect", f);
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

    /// Round-10 review: a VACUOUS logout (the skill was never installed —
    /// no record, nothing on disk) must keep the deny entries. Stripping on
    /// the vacuous `Ok(false)` would remove the deny-first entry a concurrent
    /// same-id connect may have just registered (its install lands after the
    /// gate) and re-enable the skill as it lands.
    #[test]
    fn ima_logout_vacuous_uninstall_keeps_deny_entries() {
        with_temp_home(|| {
            crate::features::marketplace::save_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Code,
                &[IMA_SKILL_ID.to_string()],
            )
            .expect("code scope must initialize while the lock works");

            let store = MemoryCredentialStore::default();
            ima_logout_sync_with_store(&store).expect("a vacuous logout must succeed");

            assert!(
                crate::features::marketplace::scope::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                )
                // "ima-skills" normalizes to its owner-package id "ima" on save.
                .contains(&"ima".to_string()),
                "a vacuous logout must not strip the deny-first entry"
            );
        });
    }

    /// Round-10 review: a logout whose uninstall FAILS (here: the bundle
    /// store is unreadable, which `SkillMarketplaceManager::uninstall`
    /// refuses fail-closed) must keep the deny entries — the on-disk skill
    /// (if any) would otherwise be silently re-enabled in initialized
    /// DenyAll scopes. Pins the keep-on-failure branch that round 8 added;
    /// the extraction of `ima_logout_sync_with_store` makes it reachable
    /// without touching the real system credential vault.
    #[test]
    fn ima_logout_failed_uninstall_keeps_deny_entries() {
        with_temp_home(|| {
            crate::features::marketplace::save_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Code,
                &[IMA_SKILL_ID.to_string()],
            )
            .expect("code scope must initialize while the lock works");
            // Make the bundle store unreadable: bundles.json becomes a
            // directory, so the uninstall's fail-closed read refuses.
            let bundles_json = crate::features::marketplace::store::BundleStore::new().file_path();
            if bundles_json.is_file() {
                std::fs::remove_file(&bundles_json).unwrap();
            }
            std::fs::create_dir_all(&bundles_json).unwrap();

            let store = MemoryCredentialStore::default();
            ima_logout_sync_with_store(&store)
                .expect("logout must still succeed when only the skill teardown fails");

            assert!(
                crate::features::marketplace::scope::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                )
                // "ima-skills" normalizes to its owner-package id "ima" on save.
                .contains(&"ima".to_string()),
                "a failed uninstall must keep the deny entries (fail-closed)"
            );
        });
    }

    /// Round-10 review: the deny-first gate must land the ima entry BEFORE
    /// the first credential write touches the vault. The probe store
    /// snapshots the Code deny list at the first `set` — it must already be
    /// the final post-gate state, and it must contain a fresh registration
    /// beyond the seed. Reordering the gate after the credential writes
    /// fails the first assert (the snapshot would miss the new entry).
    struct GateOrderProbe {
        inner: MemoryCredentialStore,
        deny_list_at_first_set: std::sync::Mutex<Option<Vec<String>>>,
    }

    impl CredentialStore for GateOrderProbe {
        fn get(
            &self,
            reference: &CredentialReference,
        ) -> Result<Option<String>, crate::platform::credential_store::CredentialError> {
            self.inner.get(reference)
        }

        fn set(
            &self,
            reference: &CredentialReference,
            value: &str,
        ) -> Result<(), crate::platform::credential_store::CredentialError> {
            let mut first = self.deny_list_at_first_set.lock().unwrap();
            if first.is_none() {
                *first = Some(
                    crate::features::marketplace::scope::load_disabled_bundles_for(
                        crate::features::marketplace::ConnectorScope::Code,
                    ),
                );
            }
            drop(first);
            self.inner.set(reference, value)
        }

        fn delete(
            &self,
            reference: &CredentialReference,
        ) -> Result<(), crate::platform::credential_store::CredentialError> {
            self.inner.delete(reference)
        }
    }

    #[test]
    fn connect_touches_credentials_only_after_the_deny_entry_lands() {
        with_temp_home(|| {
            crate::features::marketplace::save_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Code,
                &["seed-bundle".to_string()],
            )
            .expect("code scope must initialize while the lock works");

            let probe = GateOrderProbe {
                inner: MemoryCredentialStore::default(),
                deny_list_at_first_set: std::sync::Mutex::new(None),
            };
            ima_connect_sync_with_store("client".to_string(), "key".to_string(), &probe)
                .expect("a fresh connect must succeed while the lock works");

            let final_list = crate::features::marketplace::scope::load_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Code,
            );
            let snapshot = probe
                .deny_list_at_first_set
                .lock()
                .unwrap()
                .clone()
                .expect("the connect must write credentials through the probe");
            assert_eq!(
                snapshot, final_list,
                "the deny list at the first credential write must already be the final post-gate state"
            );
            assert!(
                snapshot.len() > 1,
                "a fresh connect must have registered a deny entry beyond the seed: {snapshot:?}"
            );
            assert_eq!(
                probe.get(&client_id_ref()).unwrap().as_deref(),
                Some("client"),
                "the connect must still write the credentials through the probe"
            );
        });
    }
}
