use codewhale_secrets::{DefaultKeyringStore, Secrets, SecretsError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(test)]
use std::thread;
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
/// **并发正确性**:Keychain 读取可能因授权弹窗阻塞数秒至数分钟。若 `get` 在"读缓存未命中
/// → 访问 Keychain → 回写缓存"期间释放锁,并发场景会出两类问题:① 多线程同时未命中同一
/// key → 都访问 Keychain → 都弹窗(重复弹窗);② 线程 A 读到旧值阻塞中、线程 B `set` 新值
/// 已更新缓存,A 读完后用旧值覆盖缓存 → 此后进程持续返回陈旧密钥(凭据正确性 bug)。为此
/// `KEY_LOCKS` 为每个 `(service, account)` 维护一把 `Arc<Mutex<()>>`,`get`/`set`/`delete`
/// 在"访问 Keychain + 读写值缓存"整段持有对应 key 的锁,串行化同一凭据的所有操作;不同 key
/// 互不阻塞。锁内部从不嵌套 `value_cache`/`KEY_LOCKS` 之外的锁,无死锁风险。
///
/// **安全权衡**:缓存值为明文 secret,仅在进程内存(不落盘),与本 crate 内其他明文 secret
/// 在内存中的驻留(如 bridge 注入给引擎的 api_key、marketplace 重灌进进程 env 的 mcp
/// secret)同等敏感等级,均仅驻留进程内存、不落盘。Keychain 仍是单一真相源:`set`/`delete`
/// 同步更新缓存;环境变量路径不经过此缓存。仅 `Ok` 结果缓存(含 `Ok(None)`),`Err` 不缓存,
/// 允许临时性 Keychain 故障(或用户在授权弹窗上点"拒绝")后下次重试自愈。
///
/// **陈旧边界**:`Ok(None)`("已知不存在")会缓存整个进程生命周期;若运行期间用户经"钥匙串
/// 访问"App 或其它进程新增了该 item,本应用在重启前仍读到 `None`(设置页误显示"未配置")。
/// 这是进程级内存缓存的固有边界,正常改 keychain 的路径走应用 UI(经 `set`/`delete` 同步
/// 更新缓存),故影响有限。
static VALUE_CACHE: OnceLock<Mutex<HashMap<(String, String), Option<String>>>> = OnceLock::new();

fn value_cache() -> &'static Mutex<HashMap<(String, String), Option<String>>> {
    VALUE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 每个 `(service, account)` 一把锁,串行化同一凭据的"访问 Keychain + 读写值缓存",
/// 消除并发下的重复弹窗与陈旧覆盖(见 `VALUE_CACHE` 的"并发正确性"小节)。不同 key 互不阻塞。
static KEY_LOCKS: OnceLock<Mutex<HashMap<(String, String), Arc<Mutex<()>>>>> = OnceLock::new();

fn key_locks() -> &'static Mutex<HashMap<(String, String), Arc<Mutex<()>>>> {
    KEY_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 取(或惰性创建)某 `(service, account)` 对应的 per-key 锁。锁句柄为 `Arc`,克隆后
/// 在释放 `key_locks` 自身锁的前提下被调用方持有,确保"取锁"不与"持锁"嵌套、无死锁。
fn key_lock_for(service: &str, account: &str) -> Arc<Mutex<()>> {
    let key = (service.to_string(), account.to_string());
    let mut locks = key_locks().lock().expect("key locks poisoned");
    locks
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}
const SEARCH_API_KEY_SERVICE: &str = "pinvou3-search-api-key";
const MCP_SECRET_SERVICE: &str = "pinvou3-mcp-secret";
const IMA_SECRET_SERVICE: &str = "pinvou3-ima-secret";
const ACP_PROVIDER_KEY_SERVICE: &str = "pinvou3-acp-provider-key";
const REMOTE_KNOWLEDGE_TOKEN_SERVICE: &str = "pinvou3-remote-knowledge-token";
const SHARED_KNOWLEDGE_BACKUP_SERVICE: &str = "pinvou3-shared-knowledge-backup";

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

    /// ACP Provider（第三方中转）的 API key 引用。`agent` 使用 AgentBackend 的
    /// `agent_id()` 值（"codex"/"claude"/"kimi"）。
    pub fn for_acp_provider(agent: &str, provider_id: &str) -> Self {
        Self {
            service: ACP_PROVIDER_KEY_SERVICE.to_string(),
            account: format!("{agent}:{provider_id}"),
            version: 1,
        }
    }

    pub fn for_remote_knowledge(server_id: &str) -> Self {
        Self {
            service: REMOTE_KNOWLEDGE_TOKEN_SERVICE.to_string(),
            account: format!("server:{server_id}"),
            version: 1,
        }
    }

    pub fn for_remote_knowledge_join(request_id: &str) -> Self {
        Self {
            service: REMOTE_KNOWLEDGE_TOKEN_SERVICE.to_string(),
            account: format!("join:{request_id}"),
            version: 1,
        }
    }

    pub fn for_shared_knowledge_backup() -> Self {
        Self {
            service: SHARED_KNOWLEDGE_BACKUP_SERVICE.to_string(),
            account: "local-restore-identity".to_string(),
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
        // 快路径:命中进程级缓存(含"已知不存在"的 None)即直接返回,不触碰 Keychain —— 这是
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
        // 未命中:取 per-key 锁,在持锁临界区内"再次检查缓存(double-check)→ 访问 Keychain
        // → 回写缓存",串行化同一凭据的所有并发 get/set/delete(见 VALUE_CACHE 并发小节)。
        let lock = key_lock_for(&reference.service, &reference.account);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        // double-check:另一线程可能刚把结果写入缓存,命中即避免重复访问 Keychain/重复弹窗。
        if let Ok(cache) = value_cache().lock() {
            if let Some(cached) = cache.get(&cache_key) {
                log::info!(
                    "[credential_store] get cache hit (double-check) service={} account={}",
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
        // 持 per-key 锁覆盖"访问 Keychain + 回写值缓存"整段,与 get/delete 采用同一锁范围,
        // 串行化同一凭据的所有并发写。若仅在缓存回写时加锁而后端写入在锁外,并发 set 的
        // 后端完成顺序与缓存回写顺序可能错位,导致缓存值与 Keychain 不一致(lost-update)。
        // 失败时不更新缓存,保留既有自愈语义(见 VALUE_CACHE 并发正确性小节)。
        let lock = key_lock_for(&reference.service, &reference.account);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
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
        // 写入成功后同步更新缓存(仍在同一 per-key 临界区内),后续 get 命中即不再访问 Keychain。
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
        // 持 per-key 锁覆盖"访问 Keychain + 回写值缓存"整段,锁范围与 get/set 一致
        // (见 VALUE_CACHE 并发正确性小节)。失败时不更新缓存。
        let lock = key_lock_for(&reference.service, &reference.account);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let secrets = self.secrets_for(&reference.service);
        let result = secrets.delete(&reference.account).map_err(secrets_error);
        log::info!(
            "[credential_store] delete returned service={} account={} ok={} elapsed_ms={}",
            reference.service,
            reference.account,
            result.is_ok(),
            started_at.elapsed().as_millis()
        );
        // 删除成功后缓存为 None(仍在同一临界区内),避免下次 get 再次访问 Keychain(命中"已知不存在")。
        if result.is_ok() {
            if let Ok(mut cache) = value_cache().lock() {
                cache.insert((reference.service.clone(), reference.account.clone()), None);
            }
        }
        result
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub struct MemoryCredentialStore {
    values: Arc<Mutex<HashMap<(String, String), String>>>,
    fail: Arc<Mutex<Option<String>>>,
}

#[cfg(test)]
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

#[cfg(test)]
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

    /// 内存 `KeyringStore` 兼 spy:记录 `get` 调用次数,用于断言"命中缓存后不再触底层"。
    /// 仅用于测试;不触碰真实 Keychain。
    struct FakeKeyringStore {
        values: Mutex<HashMap<String, String>>,
        get_count: AtomicU64,
    }

    impl FakeKeyringStore {
        fn new() -> Self {
            Self {
                values: Mutex::new(HashMap::new()),
                get_count: AtomicU64::new(0),
            }
        }

        fn get_count(&self) -> u64 {
            self.get_count.load(Ordering::Relaxed)
        }

        fn seed(&self, key: &str, value: &str) {
            self.values
                .lock()
                .expect("fake store values lock")
                .insert(key.to_string(), value.to_string());
        }
    }

    impl codewhale_secrets::KeyringStore for FakeKeyringStore {
        fn get(&self, key: &str) -> Result<Option<String>, SecretsError> {
            self.get_count.fetch_add(1, Ordering::Relaxed);
            Ok(self
                .values
                .lock()
                .expect("fake store values lock")
                .get(key)
                .cloned())
        }

        fn set(&self, key: &str, value: &str) -> Result<(), SecretsError> {
            self.values
                .lock()
                .expect("fake store values lock")
                .insert(key.to_string(), value.to_string());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecretsError> {
            self.values
                .lock()
                .expect("fake store values lock")
                .remove(key);
            Ok(())
        }

        fn backend_name(&self) -> &'static str {
            "in-memory (test)"
        }
    }

    /// 把一个 fake `Secrets` 注入 store 的 service 缓存,使 `secrets_for` 命中它而不
    /// 触碰真实 Keychain。借此可断言"缓存命中后不再访问底层后端"。
    fn inject_fake_secrets(store: &SystemCredentialStore, service: &str, fake: Arc<Secrets>) {
        store
            .cache
            .lock()
            .expect("credential store cache lock")
            .insert(service.to_string(), fake);
    }

    /// 核心契约:首次 `get` 访问底层后端并缓存结果;第二次 `get` 命中缓存,
    /// **不再触碰底层后端**(这正是缓解反复弹窗的关键)。
    #[test]
    fn system_store_second_get_hits_cache_without_touching_backend() {
        let backend = Arc::new(FakeKeyringStore::new());
        backend.seed("model:cache-hit-probe", "sk-cached-1234567890");
        let store = SystemCredentialStore::new();
        inject_fake_secrets(
            &store,
            MODEL_API_KEY_SERVICE,
            Arc::new(Secrets::new(backend.clone())),
        );

        let reference = CredentialReference::for_model("cache-hit-probe");
        // 首次:未命中缓存,触达底层后端。
        assert_eq!(
            store.get(&reference).unwrap().as_deref(),
            Some("sk-cached-1234567890")
        );
        assert_eq!(backend.get_count(), 1, "首次 get 应访问后端一次");
        // 第二次:命中进程级缓存,不再访问后端。
        assert_eq!(
            store.get(&reference).unwrap().as_deref(),
            Some("sk-cached-1234567890")
        );
        assert_eq!(
            backend.get_count(),
            1,
            "第二次 get 命中缓存,后端访问次数不应增加"
        );
    }

    /// `set` 成功后同步更新缓存,使得后续 `get` 命中缓存而不再访问后端。
    #[test]
    fn system_store_set_updates_cache_so_subsequent_get_skips_backend() {
        let backend = Arc::new(FakeKeyringStore::new());
        let store = SystemCredentialStore::new();
        inject_fake_secrets(
            &store,
            MODEL_API_KEY_SERVICE,
            Arc::new(Secrets::new(backend.clone())),
        );

        let reference = CredentialReference::for_model("set-sync-probe");
        store.set(&reference, "sk-new-9999999999").unwrap();
        assert_eq!(backend.get_count(), 0, "set 不应触发后端 get");
        // set 已更新缓存,后续 get 命中缓存,不触后端。
        assert_eq!(
            store.get(&reference).unwrap().as_deref(),
            Some("sk-new-9999999999")
        );
        assert_eq!(backend.get_count(), 0, "set 后 get 应命中缓存,不访问后端");
    }

    /// `delete` 成功后缓存为"已知不存在"(None),后续 `get` 命中缓存返回 None,
    /// 不再访问后端(命中"已知不存在"分支)。
    #[test]
    fn system_store_delete_marks_cache_known_absent() {
        let backend = Arc::new(FakeKeyringStore::new());
        backend.seed("model:delete-probe", "sk-will-be-deleted-12345");
        let store = SystemCredentialStore::new();
        inject_fake_secrets(
            &store,
            MODEL_API_KEY_SERVICE,
            Arc::new(Secrets::new(backend.clone())),
        );

        let reference = CredentialReference::for_model("delete-probe");
        // 先填充缓存(首次 get 触达后端)。
        assert_eq!(
            store.get(&reference).unwrap().as_deref(),
            Some("sk-will-be-deleted-12345")
        );
        let count_before_delete = backend.get_count();
        // delete 后缓存更新为 None。
        store.delete(&reference).unwrap();
        // 后续 get 命中"已知不存在",返回 None 且不触后端。
        assert_eq!(store.get(&reference).unwrap(), None);
        assert_eq!(
            backend.get_count(),
            count_before_delete,
            "delete 后 get 应命中缓存,不访问后端"
        );
    }

    /// 缓存按 `(service, account)` 隔离:一个凭据的缓存不影响另一凭据的后端访问。
    #[test]
    fn system_store_cache_isolates_services() {
        let model_backend = Arc::new(FakeKeyringStore::new());
        model_backend.seed("model:isolate-probe", "sk-model-isolated-123");
        let search_backend = Arc::new(FakeKeyringStore::new());
        search_backend.seed("search:isolate-probe", "mk-search-isolated-123");

        let store = SystemCredentialStore::new();
        inject_fake_secrets(
            &store,
            MODEL_API_KEY_SERVICE,
            Arc::new(Secrets::new(model_backend.clone())),
        );
        inject_fake_secrets(
            &store,
            SEARCH_API_KEY_SERVICE,
            Arc::new(Secrets::new(search_backend.clone())),
        );

        let model_ref = CredentialReference::for_model("isolate-probe");
        let search_ref = CredentialReference::for_search_provider("isolate-probe");

        // 各自首次 get 触达各自后端。
        assert_eq!(
            store.get(&model_ref).unwrap().as_deref(),
            Some("sk-model-isolated-123")
        );
        assert_eq!(
            store.get(&search_ref).unwrap().as_deref(),
            Some("mk-search-isolated-123")
        );
        assert_eq!(model_backend.get_count(), 1);
        assert_eq!(search_backend.get_count(), 1);

        // 再次 get 均命中各自缓存,两个后端访问次数都不增加。
        store.get(&model_ref).unwrap();
        store.get(&search_ref).unwrap();
        assert_eq!(model_backend.get_count(), 1, "model 后端不应被重复访问");
        assert_eq!(search_backend.get_count(), 1, "search 后端不应被重复访问");
    }

    /// 并发契约:多个线程同时对同一凭据 `get`(缓存未命中),只应访问后端**一次**。
    /// per-key 锁串行化 + 持锁后 double-check 保证后续线程命中缓存,不再触碰后端(不再弹窗)。
    /// 此为缓解反复弹窗的核心契约;移除 per-key 锁或 double-check 均会使 `get_count` > 1。
    #[test]
    fn system_store_concurrent_gets_access_backend_once() {
        let backend = Arc::new(FakeKeyringStore::new());
        backend.seed("model:concurrent-get-probe", "sk-concurrent-1234567890");
        let store = Arc::new(SystemCredentialStore::new());
        inject_fake_secrets(
            &store,
            MODEL_API_KEY_SERVICE,
            Arc::new(Secrets::new(backend.clone())),
        );

        let reference = Arc::new(CredentialReference::for_model("concurrent-get-probe"));
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let store = store.clone();
                let reference = reference.clone();
                thread::spawn(move || {
                    store
                        .get(&reference)
                        .unwrap()
                        .as_deref()
                        .map(str::to_string)
                })
            })
            .collect();
        for handle in handles {
            assert_eq!(
                handle.join().unwrap().as_deref(),
                Some("sk-concurrent-1234567890"),
                "所有并发 get 应返回同一正确值"
            );
        }
        assert_eq!(
            backend.get_count(),
            1,
            "16 个并发 get 应只访问后端一次(per-key 锁 + double-check)"
        );
    }

    /// 并发契约:多个线程并发 `set` 同一凭据后,缓存值须与后端最终值一致。
    /// per-key 锁串行化"后端写入 + 缓存回写",消除 lost-update 导致的缓存陈旧。
    #[test]
    fn system_store_concurrent_sets_keep_cache_consistent_with_backend() {
        let backend = Arc::new(FakeKeyringStore::new());
        let store = Arc::new(SystemCredentialStore::new());
        inject_fake_secrets(
            &store,
            MODEL_API_KEY_SERVICE,
            Arc::new(Secrets::new(backend.clone())),
        );

        let reference = Arc::new(CredentialReference::for_model("concurrent-set-probe"));
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let store = store.clone();
                let reference = reference.clone();
                thread::spawn(move || {
                    store
                        .set(&reference, &format!("sk-value-{i}-1234567890"))
                        .unwrap()
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        // 并发写结束后,缓存值(get 命中缓存)须与后端最终值一致。
        let backend_value = backend
            .values
            .lock()
            .expect("fake store values lock")
            .get("model:concurrent-set-probe")
            .cloned();
        let cache_value = store.get(&reference).unwrap();
        assert_eq!(
            cache_value, backend_value,
            "并发 set 后缓存值须与后端最终值一致"
        );
    }

    /// 并发契约:`set` 与 `delete` 并发后,缓存的存在性须与后端一致。
    /// per-key 锁串行化两种操作,避免"后端已删除但缓存仍有旧值"的不一致。
    #[test]
    fn system_store_concurrent_set_and_delete_keep_cache_consistent() {
        let backend = Arc::new(FakeKeyringStore::new());
        backend.seed("model:concurrent-sd-probe", "sk-initial-1234567890");
        let store = Arc::new(SystemCredentialStore::new());
        inject_fake_secrets(
            &store,
            MODEL_API_KEY_SERVICE,
            Arc::new(Secrets::new(backend.clone())),
        );

        let reference = Arc::new(CredentialReference::for_model("concurrent-sd-probe"));
        // 预热缓存,使后续 set/delete 走"缓存已存在"路径。
        store.get(&reference).unwrap();

        let mut handles = Vec::new();
        for _ in 0..4 {
            let store = store.clone();
            let reference = reference.clone();
            handles.push(thread::spawn(move || {
                store.set(&reference, "sk-set-1234567890").unwrap()
            }));
        }
        for _ in 0..4 {
            let store = store.clone();
            let reference = reference.clone();
            handles.push(thread::spawn(move || store.delete(&reference).unwrap()));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let backend_has = backend
            .values
            .lock()
            .expect("fake store values lock")
            .contains_key("model:concurrent-sd-probe");
        let cache_has = store.get(&reference).unwrap().is_some();
        assert_eq!(
            cache_has, backend_has,
            "并发 set/delete 后缓存存在性须与后端一致"
        );
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
