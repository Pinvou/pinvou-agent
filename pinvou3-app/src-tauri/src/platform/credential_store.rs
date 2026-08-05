use codewhale_secrets::{DefaultKeyringStore, Secrets, SecretsError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

const MODEL_API_KEY_SERVICE: &str = "pinvou3-model-api-key";

/// 进程级凭据值缓存(键 = `(service, account)`,值 = 缓存的凭据,`None` 表示"已知不存在")。
///
/// **目的**:缓解 macOS ad-hoc 签名下 Keychain 反复弹窗(#175)。macOS Keychain 的 ACL
/// 以应用代码签名身份(designated requirement)识别可信应用;社区版 DMG 是 ad-hoc 签名
/// (`signingIdentity = "-"`),只有 cdhash、无稳定证书身份,无法建立持久 ACL —— 导致
/// "始终允许"无效、每次访问 keychain item 都重新判定为未授权应用并弹窗。`keyring` crate
/// 的 macOS 后端用经典 `SecKeychainAddGenericPassword` API,创建 item 时不设自定义 ACL,
/// 完全依赖默认访问控制,ad-hoc 下默认访问控制无法稳定放行。
///
/// 缓存让同一凭据在一次进程生命周期内只访问 Keychain 一次:首次 `get` 触发授权弹窗
/// (用户点"允许"后本次成功读取并缓存),之后命中缓存即不触碰 Keychain,应用使用期间不再
/// 反复弹窗。重启应用后首次访问仍会弹一次(详见 `docs/macos-keychain-弹窗说明.md`)。
///
/// **安全权衡**:缓存值为明文 secret,仅在进程内存(不落盘),与 `RuntimeModelCredential`
/// 在 bridge 层的内存缓存(见 `bridge.rs` 的 `api_key`)同级。Keychain 仍是单一真相源:
/// `set`/`delete` 同步更新缓存;环境变量路径不经过此缓存。仅 `Ok` 结果缓存(含 `Ok(None)`),
/// `Err` 不缓存,允许临时性 Keychain 故障恢复后自愈。
static VALUE_CACHE: OnceLock<Mutex<HashMap<(String, String), Option<String>>>> = OnceLock::new();

fn value_cache() -> &'static Mutex<HashMap<(String, String), Option<String>>> {
    VALUE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}
const SEARCH_API_KEY_SERVICE: &str = "pinvou3-search-api-key";
const MCP_SECRET_SERVICE: &str = "pinvou3-mcp-secret";
const IMA_SECRET_SERVICE: &str = "pinvou3-ima-secret";

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
    pub fn for_ima_secret(secret_name: &str) -> Self {
        Self {
            service: IMA_SECRET_SERVICE.to_string(),
            account: format!("ima:{secret_name}"),
            version: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum CredentialState {
    #[default]
    Missing,
    Configured,
    EnvOverride,
    NeedsMigration,
    Unavailable,
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
    ///
    /// 所有桌面平台策略一致:优先系统凭据存储(macOS Keychain / Windows Credential
    /// Manager / Linux Secret Service),只有 `probe()` 明确失败时才回退文件存储。
    ///
    /// macOS ad-hoc 构建的签名身份不稳定,可能让 Keychain 再次请求授权,但这不应成为
    /// 默认降级成明文存储的理由；稳定签名可改善授权体验,安全默认值仍应保持 Keychain。
    fn secrets_for(&self, service: &str) -> Arc<Secrets> {
        let started_at = Instant::now();
        log::info!(
            "[credential_store] secrets_for lock wait start service={}",
            service
        );
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
        log::info!(
            "[credential_store] secrets_for cache miss service={}",
            service
        );
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
        let cache_key = (reference.service.clone(), reference.account.clone());
        // 命中进程级缓存(含"已知不存在"的 None)即直接返回,不触碰 Keychain —— 这是
        // macOS ad-hoc 签名下避免反复弹窗的关键(见 VALUE_CACHE 注释)。
        if let Ok(cache) = value_cache().lock() {
            if let Some(cached) = cache.get(&cache_key) {
                log::info!(
                    "[credential_store] get cache hit service={} account={}",
                    reference.service,
                    reference.account
                );
                return Ok(cached.clone());
            }
        }
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
        // 仅缓存 Ok 结果(含 Ok(None));Err 不缓存,允许下次重试自愈。
        if let Ok(value) = &result {
            if let Ok(mut cache) = value_cache().lock() {
                cache.insert(cache_key, value.clone());
            }
        }
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
        let result = secrets
            .set(&reference.account, value)
            .map_err(secrets_error);
        log::info!(
            "[credential_store] set returned service={} account={} ok={} elapsed_ms={}",
            reference.service,
            reference.account,
            result.is_ok(),
            started_at.elapsed().as_millis()
        );
        // 写入成功后同步更新缓存:后续 get 命中即不再访问 Keychain。
        if result.is_ok() {
            if let Ok(mut cache) = value_cache().lock() {
                cache.insert(
                    (reference.service.clone(), reference.account.clone()),
                    Some(value.to_string()),
                );
            }
        }
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
        // 删除成功后缓存为 None,避免下次 get 再次访问 Keychain(命中"已知不存在")。
        if result.is_ok() {
            if let Ok(mut cache) = value_cache().lock() {
                cache.insert(
                    (reference.service.clone(), reference.account.clone()),
                    None,
                );
            }
        }
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
        if let Some(message) = self
            .fail
            .lock()
            .expect("memory credential fail lock")
            .clone()
        {
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
        if part
            .trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';')
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

    /// 测试间清空进程级凭据值缓存,避免 static 状态串扰。
    /// (生产代码不需要清空;缓存跨整个进程生命周期持久是设计意图。)
    fn reset_value_cache() {
        value_cache()
            .lock()
            .expect("value cache lock")
            .clear();
    }

    #[test]
    fn value_cache_roundtrip_set_get_delete_consistency() {
        reset_value_cache();
        let key = ("pinvou3-model-api-key".to_string(), "model:m1".to_string());

        // 初始:缓存未命中(value_cache.get 返回 None 表示"未缓存")。
        {
            let cache = value_cache().lock().unwrap();
            assert!(cache.get(&key).is_none(), "缓存应为空");
        }

        // 模拟 get 成功后缓存 Ok(None) —— "已知不存在"。
        {
            let mut cache = value_cache().lock().unwrap();
            cache.insert(key.clone(), None);
        }
        {
            let cache = value_cache().lock().unwrap();
            assert_eq!(cache.get(&key), Some(&None), "应缓存为已知不存在");
        }

        // 模拟 set 成功后更新为 Some(value)。
        {
            let mut cache = value_cache().lock().unwrap();
            cache.insert(key.clone(), Some("sk-secret-1234567890".to_string()));
        }
        {
            let cache = value_cache().lock().unwrap();
            assert_eq!(
                cache.get(&key),
                Some(&Some("sk-secret-1234567890".to_string())),
                "set 后缓存应反映新值"
            );
        }

        // 模拟 delete 成功后回退为 None。
        {
            let mut cache = value_cache().lock().unwrap();
            cache.insert(key.clone(), None);
        }
        {
            let cache = value_cache().lock().unwrap();
            assert_eq!(cache.get(&key), Some(&None), "delete 后应缓存为不存在");
        }

        reset_value_cache();
    }

    #[test]
    fn value_cache_distinguishes_services() {
        reset_value_cache();
        let model_key = (
            "pinvou3-model-api-key".to_string(),
            "model:m1".to_string(),
        );
        let search_key = (
            "pinvou3-search-api-key".to_string(),
            "search:metaso".to_string(),
        );
        {
            let mut cache = value_cache().lock().unwrap();
            cache.insert(model_key.clone(), Some("sk-model".to_string()));
            cache.insert(search_key.clone(), Some("mk-search".to_string()));
        }
        {
            let cache = value_cache().lock().unwrap();
            assert_eq!(
                cache.get(&model_key),
                Some(&Some("sk-model".to_string()))
            );
            assert_eq!(
                cache.get(&search_key),
                Some(&Some("mk-search".to_string()))
            );
        }
        reset_value_cache();
    }

    #[test]
    fn memory_store_roundtrip_and_delete() {
        let store = MemoryCredentialStore::default();
        let reference = CredentialReference::for_model("m1");
        assert_eq!(store.get(&reference).unwrap(), None);
        store.set(&reference, "sk-test-secret").unwrap();
        assert_eq!(
            store.get(&reference).unwrap().as_deref(),
            Some("sk-test-secret")
        );
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
    fn ima_reference_uses_separate_service() {
        let reference = CredentialReference::for_ima_secret("api_key");
        assert_eq!(reference.service, "pinvou3-ima-secret");
        assert_eq!(reference.account, "ima:api_key");
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
    fn credential_error_redacts_mcp_bearer_tokens() {
        let err =
            CredentialError::new("request failed Authorization Bearer qcc-secret-token-1234567890");
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
