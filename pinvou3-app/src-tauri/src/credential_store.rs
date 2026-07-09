use codewhale_secrets::{DefaultKeyringStore, Secrets, SecretsError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const MODEL_API_KEY_SERVICE: &str = "pinvou3-model-api-key";
const SEARCH_API_KEY_SERVICE: &str = "pinvou3-search-api-key";
const MCP_SECRET_SERVICE: &str = "pinvou3-mcp-secret";
const LLMAPI_TOKEN_SERVICE: &str = "pinvou3-llmapi-token";
const LLMAPI_ADMIN_SERVICE: &str = "pinvou3-llmapi-admin";
const LLMAPI_USER_SESSION_SERVICE: &str = "pinvou3-llmapi-user-session";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialReference {
    pub service: String,
    pub account: String,
    pub version: u32,
}

impl CredentialReference {
    pub fn for_model(model_id: &str) -> Self {
        Self {
            service: MODEL_API_KEY_SERVICE.to_string(),
            account: format!("model:{model_id}"),
            version: 1,
        }
    }

    pub fn for_search_provider(provider: &str) -> Self {
        Self {
            service: SEARCH_API_KEY_SERVICE.to_string(),
            account: format!("search:{provider}"),
            version: 1,
        }
    }

    pub fn for_mcp_secret(tool_id: &str, target: &str, secret_name: &str) -> Self {
        Self {
            service: MCP_SECRET_SERVICE.to_string(),
            account: format!("mcp:{tool_id}:{target}:{secret_name}"),
            version: 1,
        }
    }

    pub fn for_llmapi_token(pinvou_user_id: &str, device_binding_id: &str) -> Self {
        Self {
            service: LLMAPI_TOKEN_SERVICE.to_string(),
            account: format!("llmapi:{pinvou_user_id}:{device_binding_id}"),
            version: 1,
        }
    }

    pub fn for_llmapi_admin() -> Self {
        Self {
            service: LLMAPI_ADMIN_SERVICE.to_string(),
            account: "newapi-admin".to_string(),
            version: 1,
        }
    }

    pub fn for_llmapi_user_session(pinvou_user_id: &str, device_binding_id: &str) -> Self {
        Self {
            service: LLMAPI_USER_SESSION_SERVICE.to_string(),
            account: format!("llmapi-session:{pinvou_user_id}:{device_binding_id}"),
            version: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    Missing,
    Configured,
    EnvOverride,
    NeedsMigration,
    Unavailable,
}

impl Default for CredentialState {
    fn default() -> Self {
        Self::Missing
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialEditAction {
    KeepExisting,
    Replace,
    Delete,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialMigrationResult {
    pub migrated_count: usize,
    pub skipped_count: usize,
    pub failed_model_ids: Vec<String>,
    pub failed_search_providers: Vec<String>,
    pub settings_sanitized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialError {
    message: String,
}

impl CredentialError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: redact_secret(&message.into()),
        }
    }

    pub fn user_message(&self) -> String {
        self.message.clone()
    }
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CredentialError {}

pub trait CredentialStore {
    fn get(&self, reference: &CredentialReference) -> Result<Option<String>, CredentialError>;
    fn set(&self, reference: &CredentialReference, value: &str) -> Result<(), CredentialError>;
    fn delete(&self, reference: &CredentialReference) -> Result<(), CredentialError>;
}

fn secrets_error(err: SecretsError) -> CredentialError {
    CredentialError::new(format!(
        "credential store access failed; please reconfigure API Key or repair system credential access: {err}"
    ))
}

/// 复用底座 codewhale-secrets,但**按 `reference.service` 选 keyring 命名空间**:
/// keyring 条目 = `(reference.service, reference.account)`,与历史命名空间
/// (`pinvou3-model-api-key` / `pinvou3-mcp-secret`)**保持一致** —— 升级不丢已存凭据。
/// OS keyring 优先,不可用(无 D-Bus / headless 服务器)自动回退 FileKeyringStore。
/// 每个 service 一个 `Secrets`,首次用到时惰性构造并缓存。
#[derive(Clone, Default)]
pub struct SystemCredentialStore {
    cache: Arc<Mutex<HashMap<String, Arc<Secrets>>>>,
}

impl SystemCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取(或惰性构造 + 缓存)某 keyring service 对应的 Secrets 后端。
    fn secrets_for(&self, service: &str) -> Arc<Secrets> {
        let started_at = Instant::now();
        log::info!("[credential_store] secrets_for lock wait start service={}", service);
        let mut cache = self.cache.lock().expect("credential store cache lock");
        log::info!(
            "[credential_store] secrets_for lock acquired service={} elapsed_ms={}",
            service,
            started_at.elapsed().as_millis()
        );
        if let Some(existing) = cache.get(service) {
            log::info!(
                "[credential_store] secrets_for cache hit service={} elapsed_ms={}",
                service,
                started_at.elapsed().as_millis()
            );
            return existing.clone();
        }
        log::info!("[credential_store] secrets_for cache miss service={}", service);
        let store = DefaultKeyringStore::new(service);
        log::info!("[credential_store] keyring probe start service={}", service);
        let secrets = match store.probe() {
            Ok(()) => {
                log::info!(
                    "[credential_store] keyring probe ok service={} elapsed_ms={}",
                    service,
                    started_at.elapsed().as_millis()
                );
                Secrets::new(Arc::new(store))
            }
            Err(err) => {
                log::warn!(
                    "[credential_store] keyring probe failed service={} elapsed_ms={} error={}",
                    service,
                    started_at.elapsed().as_millis(),
                    err
                );
                log::warn!("OS keyring 不可用({err}),改用文件回退凭证存储");
                Secrets::file_backed()
            }
        };
        let arc = Arc::new(secrets);
        cache.insert(service.to_string(), arc.clone());
        log::info!(
            "[credential_store] secrets_for cached service={} elapsed_ms={}",
            service,
            started_at.elapsed().as_millis()
        );
        arc
    }
}

impl std::fmt::Debug for SystemCredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemCredentialStore").finish()
    }
}

impl CredentialStore for SystemCredentialStore {
    fn get(&self, reference: &CredentialReference) -> Result<Option<String>, CredentialError> {
        let started_at = Instant::now();
        log::info!(
            "[credential_store] get start service={} account={}",
            reference.service,
            reference.account
        );
        let secrets = self.secrets_for(&reference.service);
        log::info!(
            "[credential_store] get backend ready service={} account={} elapsed_ms={}",
            reference.service,
            reference.account,
            started_at.elapsed().as_millis()
        );
        let result = secrets.get(&reference.account).map_err(secrets_error);
        log::info!(
            "[credential_store] get returned service={} account={} ok={} elapsed_ms={}",
            reference.service,
            reference.account,
            result.is_ok(),
            started_at.elapsed().as_millis()
        );
        result
    }

    fn set(&self, reference: &CredentialReference, value: &str) -> Result<(), CredentialError> {
        let started_at = Instant::now();
        log::info!(
            "[credential_store] set start service={} account={}",
            reference.service,
            reference.account
        );
        let secrets = self.secrets_for(&reference.service);
        log::info!(
            "[credential_store] set backend ready service={} account={} elapsed_ms={}",
            reference.service,
            reference.account,
            started_at.elapsed().as_millis()
        );
        let result = secrets.set(&reference.account, value).map_err(secrets_error);
        log::info!(
            "[credential_store] set returned service={} account={} ok={} elapsed_ms={}",
            reference.service,
            reference.account,
            result.is_ok(),
            started_at.elapsed().as_millis()
        );
        result
    }

    fn delete(&self, reference: &CredentialReference) -> Result<(), CredentialError> {
        let started_at = Instant::now();
        log::info!(
            "[credential_store] delete start service={} account={}",
            reference.service,
            reference.account
        );
        let secrets = self.secrets_for(&reference.service);
        let result = secrets.delete(&reference.account).map_err(secrets_error);
        log::info!(
            "[credential_store] delete returned service={} account={} ok={} elapsed_ms={}",
            reference.service,
            reference.account,
            result.is_ok(),
            started_at.elapsed().as_millis()
        );
        result
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemoryCredentialStore {
    values: Arc<Mutex<HashMap<(String, String), String>>>,
    fail: Arc<Mutex<Option<String>>>,
}

impl MemoryCredentialStore {
    pub fn fail_with(&self, message: impl Into<String>) {
        *self.fail.lock().expect("memory credential fail lock") = Some(message.into());
    }

    fn maybe_fail(&self) -> Result<(), CredentialError> {
        if let Some(message) = self.fail.lock().expect("memory credential fail lock").clone() {
            return Err(CredentialError::new(message));
        }
        Ok(())
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn get(&self, reference: &CredentialReference) -> Result<Option<String>, CredentialError> {
        self.maybe_fail()?;
        Ok(self
            .values
            .lock()
            .expect("memory credential values lock")
            .get(&(reference.service.clone(), reference.account.clone()))
            .cloned())
    }

    fn set(&self, reference: &CredentialReference, value: &str) -> Result<(), CredentialError> {
        self.maybe_fail()?;
        self.values
            .lock()
            .expect("memory credential values lock")
            .insert(
                (reference.service.clone(), reference.account.clone()),
                value.to_string(),
            );
        Ok(())
    }

    fn delete(&self, reference: &CredentialReference) -> Result<(), CredentialError> {
        self.maybe_fail()?;
        self.values
            .lock()
            .expect("memory credential values lock")
            .remove(&(reference.service.clone(), reference.account.clone()));
        Ok(())
    }
}

pub fn redact_secret(input: &str) -> String {
    let bearer_redacted = redact_bearer_tokens(input);
    bearer_redacted
        .split_whitespace()
        .map(|part| {
            if is_secret_like(part) {
                "[REDACTED]"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_bearer_tokens(input: &str) -> String {
    let mut output = Vec::new();
    let mut redact_next = false;
    for part in input.split_whitespace() {
        if redact_next {
            output.push("[REDACTED]");
            redact_next = false;
            continue;
        }
        output.push(part);
        if part.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';')
            .eq_ignore_ascii_case("bearer")
        {
            redact_next = true;
        }
    }
    output.join(" ")
}

pub fn is_secret_like(value: &str) -> bool {
    let trimmed = value.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';');
    if trimmed.len() < 8 {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("sk-")
        || lower.starts_with("ak-")
        || lower.starts_with("bce-v3/")
        || lower.starts_with("tvly-")
        || lower.starts_with("mgp")
        || (trimmed.len() >= 24
            && trimmed.chars().any(|c| c.is_ascii_digit())
            && trimmed.chars().any(|c| c.is_ascii_alphabetic()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip_and_delete() {
        let store = MemoryCredentialStore::default();
        let reference = CredentialReference::for_model("m1");
        assert_eq!(store.get(&reference).unwrap(), None);
        store.set(&reference, "sk-test-secret").unwrap();
        assert_eq!(store.get(&reference).unwrap().as_deref(), Some("sk-test-secret"));
        store.delete(&reference).unwrap();
        assert_eq!(store.get(&reference).unwrap(), None);
    }

    #[test]
    fn credential_error_redacts_secret_like_content() {
        let err = CredentialError::new("write failed for sk-test-secret-1234567890");
        assert!(!err.user_message().contains("sk-test-secret"));
        assert!(err.user_message().contains("[REDACTED]"));
    }

    #[test]
    fn mcp_reference_uses_separate_service() {
        let reference = CredentialReference::for_mcp_secret("iwencai", "env", "IWENCAI_API_KEY");
        assert_eq!(reference.service, "pinvou3-mcp-secret");
        assert_eq!(reference.account, "mcp:iwencai:env:IWENCAI_API_KEY");
        assert_eq!(reference.version, 1);
    }

    #[test]
    fn search_reference_uses_separate_service() {
        let reference = CredentialReference::for_search_provider("metaso");
        assert_eq!(reference.service, "pinvou3-search-api-key");
        assert_eq!(reference.account, "search:metaso");
        assert_eq!(reference.version, 1);
    }

    #[test]
    fn llmapi_references_use_dedicated_services() {
        let token = CredentialReference::for_llmapi_token("u_1", "dev_abc");
        assert_eq!(token.service, "pinvou3-llmapi-token");
        assert_eq!(token.account, "llmapi:u_1:dev_abc");
        assert_eq!(token.version, 1);

        let admin = CredentialReference::for_llmapi_admin();
        assert_eq!(admin.service, "pinvou3-llmapi-admin");
        assert_eq!(admin.account, "newapi-admin");
        assert_eq!(admin.version, 1);
    }

    #[test]
    fn credential_error_redacts_mcp_bearer_tokens() {
        let err = CredentialError::new("request failed Authorization Bearer qcc-secret-token-1234567890");
        let message = err.user_message();
        assert!(!message.contains("qcc-secret-token"));
        assert!(message.contains("[REDACTED]"));
    }

    #[test]
    fn failing_memory_store_returns_redacted_errors() {
        let store = MemoryCredentialStore::default();
        store.fail_with("cannot read sk-secret-value-123456789");
        let err = store
            .get(&CredentialReference::for_model("m1"))
            .expect_err("store should fail");
        assert!(!err.user_message().contains("sk-secret-value"));
    }
}
