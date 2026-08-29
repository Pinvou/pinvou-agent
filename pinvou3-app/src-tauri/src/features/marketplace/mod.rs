//! 工具市场管理器 — 管理 MCP 工具的安装/卸载/状态查询。
//!
//! 每个工具是一个 MCP server，元数据定义在 `manifest.json`。
//! 安装状态持久化在 `~/.pinvou3/marketplace/installed.json`。
//! 安装/卸载时同步修改 `~/.pinvou3/bundle/mcp.json`。
//!
//! 本模块是 facade:把原本 2600+ 行的 god-module 按职责拆成子模块,
//! 对外 pub 面通过 `pub use` 保持不变。
//!
//! - `types`      — manifest/info/迁移结果等数据类型
//! - `secrets`    — 密钥/凭证助手 + MarketplaceManager 的 secret 读写方法
//! - `validation` — 远程 MCP 连接校验
//! - `migration`  — mcp.json 旧版明文密钥迁移
//! - `connectors` — connector 注册/注销(含拆分后的 add_to_mcp_json remote/local 分支)

mod connectors;
mod migration;
mod python_dependencies;
mod secrets;
mod types;
mod validation;

// PR #302 WIP 拆分的子模块（main 的 Wave 2 没有接这块）—— 需要补 mod 声明。
pub mod actions;
pub mod bundle;
pub mod mcp_catalog;
pub mod plugin_import;
pub mod scope;
pub mod skill_marketplace;
pub mod skill_scope;
pub mod store;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::platform::credential_store::{CredentialStore, SystemCredentialStore};
use crate::platform::paths;

/// installed.json, mcp.json, and managed Python environment liveness share one transaction
/// domain. Install, uninstall, and startup repair must take this lock before reading committed
/// state so cleanup never acts on a stale snapshot.
static MARKETPLACE_TRANSACTION_LOCK: Mutex<()> = Mutex::new(());

/// A freshly written journal on Windows can be briefly held by antivirus or indexer
/// processes, so the commit removal retries before failing — escalating a
/// self-healing transient hold into Integrity (which blocks every assistant startup) would be disproportionate (review 2026-08-28).
const JOURNAL_REMOVE_ATTEMPTS: usize = 3;
const JOURNAL_REMOVE_RETRY_DELAY: Duration = Duration::from_millis(120);

#[cfg(test)]
static FAIL_NEXT_INSTALLED_WRITE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
static FAIL_NEXT_JOURNAL_REMOVAL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn fail_next_journal_removal_for_test() {
    FAIL_NEXT_JOURNAL_REMOVAL.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
static FAIL_NEXT_MANAGED_DEPENDENCY_INSTALL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
pub(crate) struct ManagedDependencyInstallFailureGuard;

#[cfg(test)]
impl Drop for ManagedDependencyInstallFailureGuard {
    fn drop(&mut self) {
        FAIL_NEXT_MANAGED_DEPENDENCY_INSTALL.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
pub(crate) fn fail_next_managed_dependency_install_for_test() -> ManagedDependencyInstallFailureGuard
{
    FAIL_NEXT_MANAGED_DEPENDENCY_INSTALL.store(true, std::sync::atomic::Ordering::SeqCst);
    ManagedDependencyInstallFailureGuard
}

#[cfg(test)]
pub(crate) fn managed_dependency_install_failure_pending_for_test() -> bool {
    FAIL_NEXT_MANAGED_DEPENDENCY_INSTALL.load(std::sync::atomic::Ordering::SeqCst)
}

#[cfg(test)]
static TEST_TRUSTED_DEPENDENCY_MANIFESTS: std::sync::LazyLock<
    Mutex<std::collections::HashMap<String, ToolManifest>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

#[cfg(test)]
#[derive(Clone)]
struct InstallPause {
    entered: std::sync::Arc<std::sync::Barrier>,
    release: std::sync::Arc<std::sync::Barrier>,
}

#[cfg(test)]
static INSTALL_PAUSE: Mutex<Option<InstallPause>> = Mutex::new(None);

#[cfg(test)]
fn pause_install_after_environment_for_test() {
    let pause = INSTALL_PAUSE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(pause) = pause {
        pause.entered.wait();
        pause.release.wait();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MarketplaceStateSnapshot {
    installed: Option<Vec<u8>>,
    mcp: Option<Vec<u8>>,
}

fn marketplace_transaction_journal() -> PathBuf {
    paths::pinvou3_home()
        .join("marketplace")
        .join("state-transaction.json")
}

fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Failed to read {}: {error}", path.display())),
    }
}

fn write_atomic_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
    }
    deepseek_tui::utils::write_atomic(path, bytes)
        .map_err(|error| format!("Failed to write {}: {error}", path.display()))
}

fn restore_optional_file(path: &Path, content: &Option<Vec<u8>>) -> Result<(), String> {
    if let Some(bytes) = content {
        write_atomic_file(path, bytes)
    } else {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Failed to remove {}: {error}", path.display())),
        }
    }
}

fn restore_marketplace_snapshot(snapshot: &MarketplaceStateSnapshot) -> Result<(), String> {
    restore_optional_file(&paths::mcp_config_path(), &snapshot.mcp)?;
    restore_optional_file(
        &paths::pinvou3_home()
            .join("marketplace")
            .join("installed.json"),
        &snapshot.installed,
    )
}

fn remove_file_with_retry(path: &Path) -> Result<(), String> {
    let mut last_error: Option<std::io::Error> = None;
    for _attempt in 0..JOURNAL_REMOVE_ATTEMPTS {
        #[cfg(test)]
        if _attempt == 0
            && FAIL_NEXT_JOURNAL_REMOVAL.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            last_error = Some(std::io::Error::other(
                "test-injected journal removal failure",
            ));
            std::thread::sleep(JOURNAL_REMOVE_RETRY_DELAY);
            continue;
        }
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                std::thread::sleep(JOURNAL_REMOVE_RETRY_DELAY);
            }
        }
    }
    Err(format!(
        "Failed to remove {} after {JOURNAL_REMOVE_ATTEMPTS} attempts: {}",
        path.display(),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unknown error".to_string())
    ))
}

/// The journal only lives for milliseconds and both state files are atomic writes.
/// A corrupt or unrecoverable journal is quarantined instead of propagated: the retained on-disk state is at most half a transaction and the tool can
/// self-heal through a UI reinstall; blocking every assistant engine startup over it is disproportionate (review 2026-08-28).
fn quarantine_marketplace_journal(journal: &Path, reason: &str) -> Result<(), String> {
    let quarantine = journal.with_file_name(format!(
        "state-transaction.corrupt-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::rename(journal, &quarantine).map_err(|error| {
        format!("Failed to quarantine Marketplace transaction journal ({reason}): {error}")
    })?;
    log::warn!(
        "[marketplace] quarantined marketplace transaction journal ({reason}) to {}",
        quarantine.display()
    );
    Ok(())
}

fn recover_marketplace_transaction() -> Result<(), String> {
    let journal = marketplace_transaction_journal();
    let Some(bytes) = read_optional_file(&journal)? else {
        return Ok(());
    };
    let snapshot = match serde_json::from_slice::<MarketplaceStateSnapshot>(&bytes) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            quarantine_marketplace_journal(&journal, &format!("invalid journal: {error}"))?;
            return Ok(());
        }
    };
    if let Err(error) = restore_marketplace_snapshot(&snapshot) {
        quarantine_marketplace_journal(&journal, &format!("restore failed: {error}"))?;
        return Ok(());
    }
    remove_file_with_retry(&journal)
}

struct MarketplaceStateTransaction {
    snapshot: MarketplaceStateSnapshot,
    finished: bool,
}

#[derive(Debug)]
enum ManagedDependencyError {
    /// Network, interpreter, download, extraction, and probe failures may recover on restart.
    Transient(String),
    /// A trusted embedded declaration is structurally unusable on this application build.
    Permanent(String),
    /// Provenance could not be established, so dependency declarations must not execute.
    Untrusted(String),
    /// Transaction journal/rollback/commit failure leaves persistent-state integrity uncertain.
    Integrity(String),
}

impl ManagedDependencyError {
    fn message(&self) -> &str {
        match self {
            Self::Transient(message)
            | Self::Permanent(message)
            | Self::Untrusted(message)
            | Self::Integrity(message) => message,
        }
    }
}

#[derive(Debug)]
enum DowngradeError {
    /// The downgrade failed, but the transaction restored the exact committed state.
    StatePreserved(String),
    /// Journal, commit, or rollback failure means persistent-state integrity is uncertain.
    Integrity(String),
}

impl MarketplaceStateTransaction {
    fn begin(installed_file: &Path) -> Result<Self, String> {
        recover_marketplace_transaction()?;
        let snapshot = MarketplaceStateSnapshot {
            installed: read_optional_file(installed_file)?,
            mcp: read_optional_file(&paths::mcp_config_path())?,
        };
        let journal = serde_json::to_vec(&snapshot)
            .map_err(|error| format!("Failed to serialize Marketplace transaction: {error}"))?;
        write_atomic_file(&marketplace_transaction_journal(), &journal)?;
        Ok(Self {
            snapshot,
            finished: false,
        })
    }

    fn commit(mut self) -> Result<(), String> {
        remove_file_with_retry(&marketplace_transaction_journal())?;
        self.finished = true;
        Ok(())
    }

    fn rollback(mut self) -> Result<(), String> {
        restore_marketplace_snapshot(&self.snapshot)?;
        remove_file_with_retry(&marketplace_transaction_journal())?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for MarketplaceStateTransaction {
    fn drop(&mut self) {
        if !self.finished {
            let _ = restore_marketplace_snapshot(&self.snapshot);
        }
    }
}

// 对外 pub 面保持不变:类型从 types 子模块 re-export。
pub use types::{
    ConfigField, MarketplaceToolInfo, MarketplaceToolValidation, McpSecretMigrationResult,
    RemoteOAuthConfig, RemoteServer, SecretEnv, SecretHeader, ToolManifest,
};

// PR #302 拆分后,这些函数已搬到 scope.rs;为保持 mod.rs 兼容面,在此 re-export。
// scope.rs 统一落盘单一 `disabled_bundles.json`(取代 `disabled_connectors.json` /
// `disabled_skills.json` 双文件)。这里 re-export 保留调用路径;连接器/技能/CLI 开关
// 统一按包 id 落盘,`skill:` 前缀跨文件借道清除。
pub use crate::features::marketplace::scope::{
    load_disabled_bundles, load_disabled_bundles_for, load_hidden_bundles_for,
    remove_bundle_from_disabled_scopes, save_disabled_bundles, save_disabled_bundles_for,
    save_hidden_bundles_for, sync_deny_all_scopes_after_install,
    sync_disabled_bundles_for_connector_switch, unavailable_bundles_for,
};
// 兼容旧名（原「连接器开关」调用方）：语义已收敛为包 id，旧名仅作别名过渡。
pub use crate::features::marketplace::scope::{
    load_disabled_bundles as load_disabled_connectors,
    load_disabled_bundles_for as load_disabled_connectors_for,
    remove_bundle_from_disabled_scopes as remove_connector_from_disabled_scopes,
    save_disabled_bundles as save_disabled_connectors,
    save_disabled_bundles_for as save_disabled_connectors_for,
};

/// 按会话类型 scope 持久化连接器禁用列表并刷新技能目录。
///
/// Refreshing live engines (组合目录重写 + 工具热刷) is an application
/// orchestration concern and is deliberately left to the caller, keeping
/// marketplace independent from the assistant runtime. 连接器禁用影响
/// companion skills 的可见性:组合目录计算时按 scope 排除被禁用连接器的
/// companion skills(`skill_materialization::disabled_skill_names_for`),
/// 因此落盘后需由调用方重写在线会话的组合目录。
pub async fn apply_disabled_connectors_for(
    scope: ConnectorScope,
    connector_ids: Vec<String>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        save_disabled_connectors_for(scope, &connector_ids);
    })
    .await
    .map_err(|error| format!("apply_disabled_connectors_for join: {error}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------

use crate::core::session_mode::SessionMode;

/// 连接器禁用集 scope：按会话模式键控的命名空间（键即模式 kebab-case 名）。
/// 历史上是独立的二元枚举；泛化后降为 `SessionMode` 的别名——serde 同为
/// kebab-case，落盘键与前端协议（`"plain"/"code"`）不变，下游引用零改动。
pub type ConnectorScope = SessionMode;

/// After a restart, rehydrate the secrets of **all installed tools** from the
/// keyring into the in-process registry (the foundation resolver reads them on
/// demand when expanding `${...}` placeholders in MCP subprocess env). No
/// longer hardcoded to the three built-ins — custom/uploaded tools with
/// secrets work after restart too.
pub fn sync_mcp_secret_values() -> Result<(), String> {
    MarketplaceManager::new().sync_secret_values()
}

/// Register the foundation's MCP secret resolver at boot: when the foundation
/// expands mcp.json `${...}` placeholders / resolves `env_headers` /
/// `bearer_token_env_var`, it consults the in-process registry (keyring as the
/// persistent layer) instead, so the process env no longer carries runtime
/// secret writes. The OnceLock takes effect the first time; later Errs are
/// ignored,
/// same contract as `install_prompt_overrides`; must be called before any
/// engine spawn.
pub fn install_mcp_secret_resolver() {
    let _ = deepseek_tui::mcp::install_mcp_secret_resolver(Box::new(|name| {
        secrets::resolve_registered_secret(name)
    }));
}

/// 当前(plain)被禁用连接器 → 模型可见工具全名(喂给引擎 disallowed_tools 的)。
pub fn disabled_tool_names() -> Vec<String> {
    disabled_tool_names_for(ConnectorScope::Plain)
}

/// 按会话类型 scope:被禁用连接器 → 模型可见工具全名(喂给引擎 disallowed_tools 的)。
pub fn disabled_tool_names_for(scope: ConnectorScope) -> Vec<String> {
    MarketplaceManager::new().model_tool_names(&load_disabled_connectors_for(scope))
}

/// 存量 mcp.json 条目的路径迁移：指向旧布局（`bundle/mcp-servers/<id>/`）的
/// command/args 重写为新包目录（`bundles/<id>/mcp/`）。幂等；只改本 app 写的
/// 文件（mcp.json）。按 mcp.json 的 server 键（= 工具 id）逐条重写，覆盖内嵌预设
/// 与自定义/手放 MCP（不再只遍历内嵌清单）。返回是否有改动（供启动标记观测）。
pub fn migrate_mcp_json_paths() -> Result<bool, String> {
    // 与其他 mcp.json 写方（add/remove_to_mcp_json）同一把进程内锁串行化（M-8）。
    let _guard = connectors::mcp_json_lock();
    let mcp_path = paths::mcp_config_path();
    if !mcp_path.is_file() {
        return Ok(false);
    }
    let content =
        std::fs::read_to_string(&mcp_path).map_err(|e| format!("读取 mcp.json 失败: {e}"))?;
    let mut mcp: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("解析 mcp.json 失败: {e}"))?;
    let mut changed = false;
    let Some(servers) = mcp.get_mut("servers").and_then(|s| s.as_object_mut()) else {
        return Ok(false);
    };
    let ids: Vec<String> = servers.keys().cloned().collect();
    for id in ids {
        let old_dir = paths::bundle_mcp_servers_dir().join(&id);
        let new_dir = mcp_catalog::package_mcp_dir(&id);
        // 新目录不存在（自定义 MCP 搬迁 kept/重释放被跳过）时不重写：否则
        // mcp.json 指向不存在的新路径，工具静默死掉且读路径无旧布局回退（G4）。
        // 搬迁/重释放成功后的下一轮启动再重写（本函数幂等）。
        if !new_dir.is_dir() {
            continue;
        }
        // 兼容两种分隔符的历史写法（Windows 原生 `\` 与 `/`）
        let old_spellings = [
            old_dir.to_string_lossy().replace('\\', "/"),
            old_dir.to_string_lossy().replace('/', "\\"),
        ];
        let new_text = new_dir.to_string_lossy().to_string();
        let Some(entry) = servers.get_mut(&id) else {
            continue;
        };
        for key in ["command", "args"] {
            if let Some(value) = entry.get_mut(key) {
                if rewrite_path_strings(value, &old_spellings, &new_text) {
                    changed = true;
                }
            }
        }
    }
    if changed {
        connectors::write_json_pretty(&mcp_path, &mcp)?;
    }
    Ok(changed)
}

/// 强制迁移自定义 MCP（不在内嵌 `mcp_catalog` 里的）到新布局：把
/// `bundle/mcp-servers/<id>/` 整个搬到 `bundles/<id>/mcp/`。幂等（目标已存在则
/// 跳过）；内嵌预置由 `ensure_package_released` 从快照重释放，不在此搬。返回
/// (moved, kept)。
pub fn migrate_custom_mcp_layout() -> std::io::Result<(usize, usize)> {
    let old_root = paths::bundle_mcp_servers_dir();
    let Ok(rd) = std::fs::read_dir(&old_root) else {
        return Ok((0, 0));
    };
    let mut moved = 0;
    let mut kept = 0;
    for entry in rd.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue; // 跳过文件（如 present_artifact_server.py）
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if mcp_catalog::spec_for(&id).is_some() {
            continue; // 内嵌预置由快照重释放
        }
        let dst = mcp_catalog::package_mcp_dir(&id);
        if dst.is_dir() {
            kept += 1;
            continue; // 目标已存在，幂等跳过
        }
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::rename(entry.path(), &dst) {
            Ok(_) => {
                log::info!("[marketplace] 迁移自定义 MCP {id} → {}", dst.display());
                moved += 1;
            }
            Err(e) => {
                log::warn!("[marketplace] 迁移自定义 MCP {id} 失败，保留旧位置: {e}");
                kept += 1;
            }
        }
    }
    Ok((moved, kept))
}

/// 递归重写字符串/字符串数组里的旧路径前缀。返回是否有改动。
fn rewrite_path_strings(value: &mut serde_json::Value, old: &[String], new: &str) -> bool {
    match value {
        serde_json::Value::String(s) => {
            let mut out = s.clone();
            for spelling in old {
                out = out.replace(spelling.as_str(), new);
            }
            if out != *s {
                *s = out;
                true
            } else {
                false
            }
        }
        serde_json::Value::Array(items) => {
            let mut changed = false;
            for item in items.iter_mut() {
                if rewrite_path_strings(item, old, new) {
                    changed = true;
                }
            }
            changed
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// MarketplaceManager — facade:构造 + 核心 manifest/installed 读写 + 装卸编排。
// secret/migration/validation/connectors 的方法分别在各子模块的 impl 块里定义。
// ---------------------------------------------------------------------------

pub struct MarketplaceManager<S: CredentialStore = SystemCredentialStore> {
    /// bundle 解包后的 MCP servers 目录 (~/.pinvou3/bundle/mcp-servers/)
    pub(super) servers_dir: PathBuf,
    /// 已安装工具列表文件 (~/.pinvou3/marketplace/installed.json)
    pub(super) installed_file: PathBuf,
    pub(super) credential_store: S,
}

impl Default for MarketplaceManager<SystemCredentialStore> {
    fn default() -> Self {
        Self::new()
    }
}

impl MarketplaceManager<SystemCredentialStore> {
    pub fn new() -> Self {
        Self::with_store(SystemCredentialStore::new())
    }
}

impl<S: CredentialStore> MarketplaceManager<S> {
    pub fn with_store(credential_store: S) -> Self {
        let servers_dir = paths::bundle_mcp_servers_dir();
        let installed_file = paths::pinvou3_home()
            .join("marketplace")
            .join("installed.json");
        Self {
            servers_dir,
            installed_file,
            credential_store,
        }
    }

    /// 扫描所有含 manifest.json 的工具包:内嵌预设(mcp_catalog)+ 上传落盘
    /// (`bundles/<id>/mcp/manifest.json`)。旧布局 `bundle/mcp-servers/` 已退役,
    /// 不再回退读取。
    pub fn available_tools(&self) -> Vec<ToolManifest> {
        let mut by_id: std::collections::HashMap<String, ToolManifest> =
            std::collections::HashMap::new();
        for spec in mcp_catalog::MCP_PACKAGES {
            match serde_json::from_str::<ToolManifest>(spec.manifest_json) {
                Ok(manifest) => {
                    by_id.insert(spec.id.to_string(), manifest);
                }
                Err(e) => eprintln!("[marketplace] 内嵌 manifest 解析失败（{}）: {e}", spec.id),
            }
        }
        if let Ok(rd) = std::fs::read_dir(paths::bundles_root()) {
            for entry in rd.flatten() {
                let manifest_path = entry.path().join("mcp").join("manifest.json");
                if !manifest_path.is_file() {
                    continue;
                }
                match std::fs::read_to_string(&manifest_path) {
                    Ok(content) => match serde_json::from_str::<ToolManifest>(&content) {
                        Ok(manifest) => {
                            by_id.insert(manifest.id.clone(), manifest);
                        }
                        Err(e) => eprintln!("[marketplace] parse error: {e}"),
                    },
                    Err(e) => eprintln!("[marketplace] read error: {e}"),
                }
            }
        }
        // HashMap 迭代顺序不确定 → 按 id 排序返回，保证前端列表/开关菜单每次刷新顺序稳定
        // （否则每次 toggle 触发刷新时工具行会随机换位）。
        let mut tools: Vec<ToolManifest> = by_id.into_values().collect();
        tools.sort_by(|a, b| a.id.cmp(&b.id));
        tools
    }

    /// 已安装的工具 ID 列表
    pub fn installed_ids(&self) -> Vec<String> {
        let content = match std::fs::read_to_string(&self.installed_file) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        match serde_json::from_str::<Vec<String>>(&content) {
            Ok(ids) => ids,
            Err(e) => {
                eprintln!(
                    "[marketplace] installed.json is invalid: {e}; backing up and rebuilding from mcp.json"
                );
                self.backup_corrupt_installed(&content);
                let recovered = self.recover_installed_ids_from_mcp();
                if let Err(write_err) = self.save_installed(&recovered) {
                    eprintln!("[marketplace] failed to rewrite installed.json: {write_err}");
                }
                recovered
            }
        }
    }

    /// 前端列表：所有可用工具 + 安装状态。上传包若有用户自定义展示名/说明覆盖
    /// （bundles.json extra，仅 Upload 来源生效），name/description 用覆盖值——
    /// 与 `BundleRegistry::list`（bundle_readiness）同一口径，避免卡片与
    /// composer 菜单两处标题不一致。
    pub fn list_tools(&self) -> Vec<MarketplaceToolInfo> {
        let installed = self.installed_ids();
        // 一次读全量记录（含 Upload 门禁），避免逐工具取锁+整文件解析的 N+1；
        // 读失败（如损坏 JSON）warn 后降级为「无覆盖」，口径同 bundle.rs 的
        // store 读回退。
        let upload_records: Vec<store::BundleRecord> = match store::BundleStore::new().records() {
            Ok(records) => records
                .into_iter()
                .filter(|r| matches!(r.source, store::BundleSource::Upload(_)))
                .collect(),
            Err(e) => {
                log::warn!("[marketplace] BundleStore 读取失败，list_tools 不应用展示覆盖: {e}");
                Vec::new()
            }
        };
        let upload_by_id: std::collections::HashMap<&str, &store::BundleRecord> =
            upload_records.iter().map(|r| (r.id.as_str(), r)).collect();
        self.available_tools()
            .into_iter()
            .map(|m| {
                let (name, description) = match upload_by_id.get(m.id.as_str()) {
                    Some(record) => {
                        let name = store::display_override(record, store::EXTRA_DISPLAY_NAME)
                            .unwrap_or(m.name.clone());
                        let description =
                            store::display_override(record, store::EXTRA_DISPLAY_DESCRIPTION)
                                .unwrap_or(m.description.clone());
                        (name, description)
                    }
                    None => (m.name, m.description),
                };
                MarketplaceToolInfo {
                    installed: installed.contains(&m.id),
                    id: m.id,
                    name,
                    description,
                    version: m.version,
                    icon: m.icon,
                    category: m.category,
                    companion_skills: m.companion_skills,
                }
            })
            .collect()
    }

    /// 安装工具：写 installed.json + 更新 mcp.json
    /// `user_config` 是前端传入的用户配置（如 API Key），对应 config_fields
    pub fn install(
        &self,
        tool_id: &str,
        user_config: &std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        self.install_inner(tool_id, user_config, store::BundleSource::Preset, None)
    }

    /// 上传/导入包的 MCP 供给（plugin_import 统一上传路径）：与 `install` 同一管线，
    /// 但不自动执行 pip install —— 上传 manifest 声明的 `pip_dependencies` 未经白名单
    /// 与用户确认（供应链安全），非空时只在日志明确提示需用户自行安装。
    /// `source` 即上传来源（`Upload(zip 展示名)`）：镜像写必须直接带正确来源，
    /// 不能依赖调用方事后补写（补写仅 log::warn，失败会让 source 停在 Preset，
    /// 下次卸载误删用户唯一副本 —— 四轮评审 BLOCKER 1）。
    pub(crate) fn install_upload(
        &self,
        tool_id: &str,
        source: store::BundleSource,
    ) -> Result<(), String> {
        self.install_inner(tool_id, &std::collections::HashMap::new(), source, None)
    }

    fn install_inner(
        &self,
        tool_id: &str,
        user_config: &std::collections::HashMap<String, String>,
        source: store::BundleSource,
        python_override: Option<&str>,
    ) -> Result<(), String> {
        let _transaction_guard = MARKETPLACE_TRANSACTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recover_marketplace_transaction()?;
        self.migrate_mcp_plaintext_secrets()?;
        let manifest = self
            .load_manifest(tool_id)
            .ok_or_else(|| format!("工具 '{tool_id}' 不存在"))?;

        // Only the embedded catalog may authorize automatic dependency execution. The connector
        // manifest loaded from disk remains the source for ordinary metadata and user config,
        // but its dependency fields are never executed.
        let dependency_manifest = self
            .trusted_dependency_manifest(tool_id, Some(&source))
            .map_err(|error| error.message().to_string())?;
        let python_environment = if let Some(dependency_manifest) = dependency_manifest {
            // UI installs are explicit user actions and bypass the startup repair retry cooldown.
            self.install_python_deps(&dependency_manifest, python_override, false)
                .map_err(|error| error.message().to_string())?
        } else if !manifest.pip_dependencies.is_empty() || manifest.python_dependencies.is_some() {
            log::warn!("[marketplace] skipped untrusted dependency declarations for '{tool_id}'");
            None
        } else {
            None
        };
        #[cfg(test)]
        pause_install_after_environment_for_test();

        // 按需释放包资源到 bundles/<id>/mcp/（§4：安装时释放，非启动全量）。
        // 自定义工具（无内嵌 spec）已由强制迁移 `migrate_custom_mcp_layout` 搬到
        // 新布局；这里统一按新包目录写 mcp.json，不再回退旧布局。
        let fingerprint = mcp_catalog::release_package(tool_id)?;
        let server_dir = mcp_catalog::package_mcp_dir(tool_id);

        // mcp.json and installed.json form one recoverable transaction. A crash between their
        // atomic writes is rolled back on the next marketplace operation or application start.
        let transaction = MarketplaceStateTransaction::begin(&self.installed_file)?;
        let result = (|| {
            self.add_to_mcp_json(
                &manifest,
                user_config,
                &server_dir,
                python_environment.as_ref(),
            )?;
            let mut installed = self.installed_ids();
            if !installed.contains(&tool_id.to_string()) {
                installed.push(tool_id.to_string());
            }
            self.save_installed(&installed)
        })();
        match result {
            Ok(()) => transaction.commit()?,
            Err(error) => {
                transaction.rollback().map_err(|rollback| {
                    format!("{error}; marketplace state rollback failed: {rollback}")
                })?;
                drop(python_environment);
                // Do not prune here: the restored mcp.json may still reference the
                // pre-install environment, whose key is absent from the catalog
                // preserve-set after a lock upgrade. Startup repair re-runs the prune
                // with the repair-error guard once the environment is healthy again.
                return Err(error);
            }
        }

        // 镜像写入统一真相源 bundles.json（Phase 2 过渡期：installed.json / mcp.json
        // 仍是权威，bundles.json 只镜像安装态；镜像写失败不翻盘主操作，fail loud 到日志）。
        // 来源订正（四轮评审 BLOCKER 1）：卸载 Upload 包只清登记、保留目录（用户唯一
        // 副本），卡片重现后用户经普通 `install` 重装时 store 已无旧记录可依（既有
        // 记录的保护由 upsert_preserving 承担）。预置包 release 只写 mcp/ 子目录，
        // `bundles/<id>/plugin.json` 仅由统一上传管线落盘 —— 据此把盘上上传包的
        // 来源订正为 Upload，避免误标 Preset 导致下次卸载删除目录。
        let source = if matches!(source, store::BundleSource::Preset)
            && paths::bundles_root()
                .join(tool_id)
                .join("plugin.json")
                .is_file()
        {
            let display =
                std::fs::read_to_string(paths::bundles_root().join(tool_id).join("plugin.json"))
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .and_then(|v| {
                        v.get("name")
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string())
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| tool_id.to_string());
            store::BundleSource::Upload(display)
        } else {
            source
        };
        let mut record = store::BundleRecord::installed_now(tool_id, source);
        record.credential_keys = bundle::tool_credentials(&manifest)
            .into_iter()
            .map(|c| c.key)
            .collect();
        record.content_fingerprint = fingerprint;
        if let Err(e) = store::BundleStore::new().upsert_preserving(record) {
            log::warn!("[marketplace] bundles.json 镜像写入失败（install {tool_id}）: {e}");
        }

        Ok(())
    }

    /// 卸载工具：从 installed.json + mcp.json 中移除
    pub fn uninstall(&self, tool_id: &str) -> Result<(), String> {
        let _transaction_guard = MARKETPLACE_TRANSACTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recover_marketplace_transaction()?;
        let secret_targets = self
            .load_manifest(tool_id)
            .map(|manifest| secrets::manifest_secret_targets(&manifest))
            .unwrap_or_default();
        // 删该工具在 keyring 的 secret(防孤儿;此时 manifest 未删、仍可读声明)。
        // 删不掉不阻断卸载；若用户重新安装并重新填 key，会写入新的系统凭据。
        if let Some(manifest) = self.load_manifest(tool_id) {
            for (target, key) in secrets::manifest_secret_targets(&manifest) {
                let reference = secrets::mcp_secret_reference(tool_id, &target, &key);
                let _ = self.credential_store.delete(&reference);
                secrets::remove_secret_value(&secrets::mcp_secret_env_var(&key));
            }
        }
        let transaction = MarketplaceStateTransaction::begin(&self.installed_file)?;
        let result = (|| {
            self.remove_from_mcp_json(tool_id)?;
            let mut installed = self.installed_ids();
            installed.retain(|id| id != tool_id);
            self.save_installed(&installed)
        })();
        match result {
            Ok(()) => transaction.commit()?,
            Err(error) => {
                transaction.rollback().map_err(|rollback| {
                    format!("{error}; marketplace state rollback failed: {rollback}")
                })?;
                return Err(error);
            }
        }

        // Only clean credentials, scope state, and companion skills after the persistent
        // registration transaction commits. A failed transaction therefore leaves the old tool
        // fully usable instead of producing a half-uninstalled state.
        self.cleanup_uninstalled_tool_state(tool_id, &secret_targets);

        // Environments and wheel caches are garbage-collected by reference to the still-installed MCPs.
        // A failed cleanup never rolls back the finished uninstall; the next uninstall retries, so a held file cannot leave the tool stuck in a half state where the config is gone but uninstall keeps erroring.
        // Re-read the just-committed installed.json inside the transaction lock; a snapshot taken before the lock may be stale.
        if let Err(error) = self.prune_from_committed_state() {
            log::warn!("[marketplace] prune Python dependencies failed: {error}");
        }

        // 镜像删除（与 install 对称：失败只记日志。命令层 OAuth token 前置删除的
        // 中止语义在 uninstall_marketplace_tool_sync，先于本函数，不受影响）。
        // 来源查询必须先于 remove —— 先删登记再查恒为 false（三轮评审：上传包目录
        // 被误删的数据丢失 bug）。fail-closed：bundles.json 读不出就当可能是
        // Upload（不可删），不把读失败按非 Upload 照删（六轮评审 R1）。
        let source_may_be_upload = store::BundleStore::new()
            .records()
            .map(|records| {
                records
                    .iter()
                    .any(|r| r.id == tool_id && matches!(r.source, store::BundleSource::Upload(_)))
            })
            .unwrap_or(true);
        if let Err(e) = store::BundleStore::new().remove(tool_id) {
            log::warn!("[marketplace] bundles.json 镜像删除失败（uninstall {tool_id}）: {e}");
        }

        // 包目录资源清理（§4：卸载 = 删登记 + 删目录）。companion 技能由命令层
        // 联动删除（其 uninstall 会清理腾空的 skills/ 与包目录）；manifest 卸载后
        // 仍可读——load_manifest 回退内嵌 catalog。
        // 只有「可重释放」的包才删目录（六轮评审 R1，数据丢失）：内嵌目录有对应
        // spec（启动/重装可按快照重释放）且登记非 Upload 才删。其余一律保留
        // `bundles/<id>/`（用户唯一副本）：Upload 来源（二轮评审）、手写自定义
        // MCP（migrate_custom_mcp_layout 从旧布局强迁、无 plugin.json、登记为
        // Preset）、无记录。
        let can_redeliver = mcp_catalog::spec_for(tool_id).is_some();
        if can_redeliver && !source_may_be_upload {
            let pkg_dir = paths::bundles_root().join(tool_id);
            if pkg_dir.exists() {
                let _ = std::fs::remove_dir_all(pkg_dir.join("mcp"));
                let _ = std::fs::remove_dir(pkg_dir.join("skills")); // 仅空目录能删掉
                let _ = std::fs::remove_dir(&pkg_dir);
            }
        }

        Ok(())
    }

    fn active_python_locks_from_committed_state(
        &self,
    ) -> Result<Vec<python_dependencies::PythonDependencyLock>, String> {
        let mut locks = Vec::new();
        for installed_id in self.installed_ids() {
            let manifest = self
                .trusted_dependency_manifest(&installed_id, None)
                .map_err(|error| error.message().to_string())?;
            if let Some(lock) = manifest.and_then(|manifest| manifest.python_dependencies) {
                locks.push(lock);
            }
        }
        Ok(locks)
    }

    fn prune_from_committed_state(&self) -> Result<(), String> {
        python_dependencies::prune_unused(&self.active_python_locks_from_committed_state()?)
    }

    fn install_python_deps(
        &self,
        manifest: &ToolManifest,
        python_override: Option<&str>,
        respect_retry_cooldown: bool,
    ) -> Result<Option<python_dependencies::InstalledPythonEnvironment>, ManagedDependencyError>
    {
        #[cfg(test)]
        if FAIL_NEXT_MANAGED_DEPENDENCY_INSTALL.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(ManagedDependencyError::Transient(
                "test-injected transient managed dependency install failure".to_string(),
            ));
        }
        if let Some(lock) = &manifest.python_dependencies {
            python_dependencies::validate_lock(lock).map_err(ManagedDependencyError::Permanent)?;
            let python_command = match python_override {
                Some(command) => command.to_string(),
                None => {
                    paths::managed_python_command().map_err(ManagedDependencyError::Transient)?
                }
            };
            if let Some(environment) =
                python_dependencies::ensure_installed(lock, &python_command, respect_retry_cooldown)
                    .map_err(ManagedDependencyError::Transient)?
            {
                return Ok(Some(environment));
            }
            if crate::platform::capabilities::is_windows() {
                return Err(ManagedDependencyError::Permanent(format!(
                    "tool '{}' has no Python dependency lock for the current Windows platform",
                    manifest.id
                )));
            }
            // No target for this platform: dependencies load through the legacy pip
            // fallback below. Keep the wheel path's retry discipline so an offline
            // machine does not replay pip timeouts on every startup.
            if respect_retry_cooldown {
                if let Some(remaining) = python_dependencies::pip_fallback_cooldown_remaining(lock)
                {
                    return Err(ManagedDependencyError::Transient(format!(
                        "legacy pip dependency repair for '{}' is in the automatic retry cooldown (about {remaining} s remaining)",
                        manifest.id
                    )));
                }
            }
        }

        let pip_result = self.pip_install_deps(manifest);
        if let Some(lock) = &manifest.python_dependencies {
            match &pip_result {
                Ok(()) => python_dependencies::clear_pip_fallback_cooldown(lock),
                Err(_) => python_dependencies::record_pip_fallback_cooldown(lock),
            }
        }
        pip_result.map_err(ManagedDependencyError::Transient)?;
        Ok(None)
    }

    /// Domain cleanup after a tool registration is removed. Plain uninstall and failed startup
    /// repair must share one semantic, otherwise companion skills keep surfacing in session prompts after the MCP is already unusable.
    fn cleanup_uninstalled_tool_state(&self, tool_id: &str, secret_targets: &[(String, String)]) {
        for (target, key) in secret_targets {
            let reference = secrets::mcp_secret_reference(tool_id, target, key);
            let _ = self.credential_store.delete(&reference);
            secrets::remove_secret_value(&secrets::mcp_secret_env_var(key));
        }
        remove_connector_from_disabled_scopes(tool_id);
        let unavailable: std::collections::HashSet<String> =
            self.unavailable_companion_skills().into_iter().collect();
        for skill_id in self.companion_skills(tool_id) {
            if !unavailable.contains(&skill_id) {
                continue;
            }
            let _ = skill_marketplace::SkillMarketplaceManager::new().uninstall(&skill_id);
            skill_scope::remove_skill_from_disabled_scopes(&skill_id);
        }
    }

    fn mark_tool_uninstalled_locked(&self, tool_id: &str) -> Result<(), DowngradeError> {
        let secret_targets = self
            .load_manifest(tool_id)
            .map(|manifest| secrets::manifest_secret_targets(&manifest))
            .unwrap_or_default();
        let transaction = MarketplaceStateTransaction::begin(&self.installed_file)
            .map_err(DowngradeError::Integrity)?;
        let result = (|| {
            self.remove_from_mcp_json(tool_id)?;
            let mut installed = self.installed_ids();
            installed.retain(|id| id != tool_id);
            self.save_installed(&installed)
        })();
        match result {
            Ok(()) => {
                transaction.commit().map_err(DowngradeError::Integrity)?;
                self.cleanup_uninstalled_tool_state(tool_id, &secret_targets);
                if let Err(error) = store::BundleStore::new().remove(tool_id) {
                    log::warn!(
                        "[marketplace] bundles.json mirror cleanup failed during Python repair ({tool_id}): {error}"
                    );
                }
                Ok(())
            }
            Err(error) => match transaction.rollback() {
                Ok(()) => Err(DowngradeError::StatePreserved(error)),
                Err(rollback) => Err(DowngradeError::Integrity(format!(
                    "{error}; marketplace state rollback failed: {rollback}"
                ))),
            },
        }
    }

    fn downgrade_permanently_invalid_dependency(
        &self,
        tool_id: &str,
        message: &str,
        repair_errors: &mut Vec<String>,
    ) -> Result<(), String> {
        let diagnostic = format!(
            "tool '{tool_id}' has a permanently invalid embedded dependency declaration: {message}"
        );
        match self.mark_tool_uninstalled_locked(tool_id) {
            Ok(()) => repair_errors.push(format!("{diagnostic}; registration was downgraded")),
            Err(DowngradeError::StatePreserved(state_error)) => repair_errors.push(format!(
                "{diagnostic}; downgrade failed and was rolled back: {state_error}"
            )),
            Err(DowngradeError::Integrity(integrity_error)) => {
                return Err(format!(
                    "{diagnostic}; downgrade integrity failure: {integrity_error}"
                ));
            }
        }
        Ok(())
    }

    /// Repair legacy Python registrations before the engine reads mcp.json.
    /// Transient failures preserve the committed registration for automatic startup retry. Only a
    /// permanently invalid trusted lock is downgraded, and every state transition is transactional.
    pub fn repair_installed_python_tools(&self) -> Result<Vec<String>, String> {
        let _transaction_guard = MARKETPLACE_TRANSACTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recover_marketplace_transaction()?;
        self.repair_installed_python_tools_locked(None)
    }

    fn repair_installed_python_tools_locked(
        &self,
        python_override: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let installed = self.installed_ids();
        let mut repair_errors = Vec::new();
        for tool_id in installed {
            let manifest = match self.trusted_dependency_manifest(&tool_id, None) {
                Ok(Some(manifest)) => manifest,
                Ok(None) => continue,
                Err(ManagedDependencyError::Permanent(message)) => {
                    self.downgrade_permanently_invalid_dependency(
                        &tool_id,
                        &message,
                        &mut repair_errors,
                    )?;
                    continue;
                }
                Err(error) => {
                    repair_errors.push(format!(
                        "tool '{tool_id}' dependency repair skipped: {}",
                        error.message()
                    ));
                    continue;
                }
            };
            if manifest.python_dependencies.is_none() || !manifest.servers.is_empty() {
                continue;
            }

            let repair: Result<(), ManagedDependencyError> = (|| {
                let environment = self.install_python_deps(&manifest, python_override, true)?;
                let Some(environment) = environment else {
                    return Ok(());
                };
                mcp_catalog::ensure_package_released(&tool_id)
                    .map_err(ManagedDependencyError::Transient)?;
                let server_dir = mcp_catalog::package_mcp_dir(&tool_id);
                let transaction = MarketplaceStateTransaction::begin(&self.installed_file)
                    .map_err(ManagedDependencyError::Integrity)?;
                let result =
                    self.patch_managed_python_runtime(&manifest, &server_dir, &environment);
                match result {
                    Ok(()) => transaction
                        .commit()
                        .map_err(ManagedDependencyError::Integrity),
                    Err(error) => {
                        transaction.rollback().map_err(|rollback| {
                            ManagedDependencyError::Integrity(format!(
                                "{error}; marketplace state rollback failed: {rollback}"
                            ))
                        })?;
                        Err(ManagedDependencyError::Transient(error))
                    }
                }
            })();

            if let Err(error) = repair {
                match error {
                    ManagedDependencyError::Transient(message)
                    | ManagedDependencyError::Untrusted(message) => {
                        repair_errors.push(format!(
                            "tool '{tool_id}' dependency repair will retry: {message}"
                        ));
                    }
                    ManagedDependencyError::Permanent(message) => {
                        self.downgrade_permanently_invalid_dependency(
                            &tool_id,
                            &message,
                            &mut repair_errors,
                        )?;
                    }
                    ManagedDependencyError::Integrity(message) => {
                        return Err(format!(
                            "tool '{tool_id}' dependency repair integrity failure: {message}"
                        ));
                    }
                }
            }
        }
        // Once repair/downgrade persistent state is committed, physically removing unreferenced caches is retryable maintenance.
        // Orphan processes, antivirus, or indexers can briefly hold .pyd files on Windows; that must never block assistant startup.
        // Skip cleanup while transient failures exist: this round's active lock set may have shifted with a catalog upgrade while existing
        // mcp.json entries still target the old environment — pruning then would turn working tools permanently broken (review 2026-08-28).
        if repair_errors.is_empty() {
            if let Err(error) = self.prune_from_committed_state() {
                log::warn!("[marketplace] prune Python dependencies after repair failed: {error}");
            }
        } else {
            log::warn!(
                "[marketplace] {} python tool(s) await dependency repair retry; environment prune deferred to a clean startup",
                repair_errors.len()
            );
        }
        Ok(repair_errors)
    }

    #[cfg(test)]
    pub(crate) fn repair_installed_python_tools_with_python(
        &self,
        python_command: &str,
    ) -> Result<Vec<String>, String> {
        let _transaction_guard = MARKETPLACE_TRANSACTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recover_marketplace_transaction()?;
        self.repair_installed_python_tools_locked(Some(python_command))
    }

    #[cfg(test)]
    fn install_with_python(
        &self,
        tool_id: &str,
        user_config: &std::collections::HashMap<String, String>,
        python_command: &str,
    ) -> Result<(), String> {
        self.install_inner(
            tool_id,
            user_config,
            store::BundleSource::Preset,
            Some(python_command),
        )
    }

    /// manifest 声明的配套技能 id(装该 MCP 时一并装、卸时一并删)。
    /// uninstall 不删 manifest 文件,故卸载后仍可读到。
    pub fn companion_skills(&self, tool_id: &str) -> Vec<String> {
        self.load_manifest(tool_id)
            .map(|m| m.companion_skills)
            .unwrap_or_default()
    }

    /// Companion skills declared only by uninstalled connectors, i.e. with no live
    /// claimant left. Cleanup uses this to remove leftover directories; a skill still claimed by any installed connector must be kept.
    pub fn unavailable_companion_skills(&self) -> Vec<String> {
        let installed: std::collections::HashSet<String> =
            self.installed_ids().into_iter().collect();
        let mut declared = std::collections::HashSet::new();
        let mut active = std::collections::HashSet::new();
        for manifest in self.available_tools() {
            for skill_id in manifest.companion_skills {
                declared.insert(skill_id.clone());
                if installed.contains(&manifest.id) {
                    active.insert(skill_id);
                }
            }
        }
        declared.difference(&active).cloned().collect()
    }

    pub fn oauth_remote_server_name(&self, tool_id: &str) -> Option<String> {
        self.load_manifest(tool_id)?
            .servers
            .into_iter()
            .find(|server| server.requires_oauth())
            .map(|server| server.name)
    }

    /// 根据已安装工具生成 instructions 路由规则段 + 工具表条目
    pub fn build_instructions_fragment(&self) -> String {
        let installed = self.installed_ids();
        if installed.is_empty() {
            return String::new();
        }

        let mut tool_table_lines = Vec::new();
        let mut routing_lines = Vec::new();

        for tool_id in &installed {
            if let Some(manifest) = self.load_manifest(tool_id) {
                for entry in &manifest.tool_table_entries {
                    tool_table_lines.push(entry.clone());
                }
                for rule in &manifest.routing_rules {
                    routing_lines.push(format!("- {rule}"));
                }
            }
        }

        let mut fragment = String::new();

        // 工具表条目
        if !tool_table_lines.is_empty() {
            for line in &tool_table_lines {
                fragment.push_str(line);
                fragment.push('\n');
            }
        }

        // 路由规则
        if !routing_lines.is_empty() {
            fragment.push_str(
                "\n### 工具路由(优先用专用工具,不要用 `Web(action=\"search\")` 替代)\n\n",
            );
            for line in &routing_lines {
                fragment.push_str(line);
                fragment.push('\n');
            }
        }

        fragment
    }

    // --- internal ---

    pub(crate) fn load_manifest(&self, tool_id: &str) -> Option<ToolManifest> {
        // 新布局优先（上传包/解压包落盘 `bundles/<id>/mcp/manifest.json`）；
        // 内嵌预设回退到编译期 catalog。旧布局 `bundle/mcp-servers/` 已退役，不再回退读取。
        let new_path = mcp_catalog::package_mcp_dir(tool_id).join("manifest.json");
        if let Ok(content) = std::fs::read_to_string(&new_path) {
            if let Ok(manifest) = serde_json::from_str(&content) {
                return Some(manifest);
            }
        }
        let spec = mcp_catalog::spec_for(tool_id)?;
        serde_json::from_str(spec.manifest_json).ok()
    }

    /// Resolve the only manifest allowed to authorize automatic dependency execution.
    /// Disk manifests remain valid connector metadata, but never establish dependency trust.
    fn embedded_dependency_manifest(
        &self,
        tool_id: &str,
    ) -> Result<Option<ToolManifest>, ManagedDependencyError> {
        if let Some(spec) = mcp_catalog::spec_for(tool_id) {
            return serde_json::from_str(spec.manifest_json)
                .map(Some)
                .map_err(|error| {
                    ManagedDependencyError::Permanent(format!(
                        "embedded dependency manifest for '{tool_id}' is invalid: {error}"
                    ))
                });
        }
        #[cfg(test)]
        if let Some(manifest) = TEST_TRUSTED_DEPENDENCY_MANIFESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(tool_id)
            .cloned()
        {
            return Ok(Some(manifest));
        }
        Ok(None)
    }

    /// Initial preset installation is anchored directly by the embedded catalog because its
    /// mirror record does not exist yet. Every later installed-state operation additionally
    /// requires an intact Preset mirror and fails closed for Upload or unknown provenance.
    fn trusted_dependency_manifest(
        &self,
        tool_id: &str,
        install_source: Option<&store::BundleSource>,
    ) -> Result<Option<ToolManifest>, ManagedDependencyError> {
        let Some(manifest) = self.embedded_dependency_manifest(tool_id)? else {
            return Ok(None);
        };
        if manifest.pip_dependencies.is_empty() && manifest.python_dependencies.is_none() {
            return Ok(Some(manifest));
        }

        if install_source.is_some_and(|source| !matches!(source, store::BundleSource::Preset)) {
            return Ok(None);
        }

        let record = store::BundleStore::new().get(tool_id).map_err(|error| {
            ManagedDependencyError::Untrusted(format!(
                "dependency provenance for '{tool_id}' is unavailable: {error}"
            ))
        })?;
        match record {
            Some(record)
                if record.installed && matches!(record.source, store::BundleSource::Preset) =>
            {
                Ok(Some(manifest))
            }
            Some(record) => Err(ManagedDependencyError::Untrusted(format!(
                "dependency provenance for '{tool_id}' is not a live preset ({})",
                record.source
            ))),
            None if install_source.is_some()
                && !self.installed_ids().iter().any(|id| id == tool_id) =>
            {
                Ok(Some(manifest))
            }
            None => Err(ManagedDependencyError::Untrusted(format!(
                "dependency provenance for installed preset '{tool_id}' is missing"
            ))),
        }
    }

    #[cfg(test)]
    fn trust_dependency_manifest_for_test(manifest: ToolManifest) {
        TEST_TRUSTED_DEPENDENCY_MANIFESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(manifest.id.clone(), manifest);
    }

    /// pinvou3 工具开关:把"连接器 id 列表"映射成"模型可见工具全名"
    /// (`mcp_{server}_{tool}`,小写 —— 引擎 `command_denies_tool` 按小写精确匹配)。
    /// 关一个连接器要把它名下所有工具都列出来。
    pub fn model_tool_names(&self, connector_ids: &[String]) -> Vec<String> {
        let mut names = Vec::new();
        for cid in connector_ids {
            if let Some(m) = self.load_manifest(cid) {
                for server in &m.servers {
                    names.push(format!("mcp_{}_*", server.name).to_ascii_lowercase());
                }
                for t in &m.mcp_tools {
                    // manifest 的 mcp_tools 不统一:部分已是全名(mcp_xxx_yyy),部分是裸工具名。
                    // 已带 `mcp_` 前缀的原样用,否则补 `mcp_{id}_` —— 与引擎 mcp_{server}_{tool} 对齐。
                    let name = if t.starts_with("mcp_") {
                        t.clone()
                    } else {
                        format!("mcp_{}_{}", m.id, t)
                    };
                    names.push(name.to_ascii_lowercase());
                }
            }
        }
        names
    }

    fn save_installed(&self, ids: &[String]) -> Result<(), String> {
        #[cfg(test)]
        if FAIL_NEXT_INSTALLED_WRITE.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err("test injection: installed.json write failed".to_string());
        }
        // installed_file 由数据根 join 而来，必有父级；仍以错误返回兜底。
        // installed_file is joined from the data root, so a parent always
        // exists; still return an error as the fallback.
        let dir = self.installed_file.parent().ok_or_else(|| {
            format!(
                "installed.json has no parent directory: {}",
                self.installed_file.display()
            )
        })?;
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
        let json = serde_json::to_string_pretty(ids).map_err(|e| e.to_string())?;
        write_atomic_file(&self.installed_file, json.as_bytes())
    }

    fn backup_corrupt_installed(&self, content: &str) {
        let Some(parent) = self.installed_file.parent() else {
            return;
        };
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let backup = parent.join(format!("installed.json.corrupt.{ts}"));
        if let Err(e) = std::fs::write(&backup, content) {
            eprintln!(
                "[marketplace] failed to backup corrupt installed.json to {}: {e}",
                backup.display()
            );
        }
    }

    fn recover_installed_ids_from_mcp(&self) -> Vec<String> {
        let content = match std::fs::read_to_string(paths::mcp_config_path()) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let Ok(mcp) = serde_json::from_str::<serde_json::Value>(&content) else {
            return Vec::new();
        };
        let Some(servers) = mcp.get("servers").and_then(|s| s.as_object()) else {
            return Vec::new();
        };
        let mut recovered = Vec::new();
        for manifest in self.available_tools() {
            let registered = if manifest.servers.is_empty() {
                servers.contains_key(&manifest.id)
            } else {
                manifest
                    .servers
                    .iter()
                    .any(|server| servers.contains_key(&server.name))
            };
            if registered && !recovered.contains(&manifest.id) {
                recovered.push(manifest.id);
            }
        }
        recovered
    }
}

#[cfg(test)]
// Tests borrow platform::paths::tests::ENV_LOCK (std Mutex) to serialize global env access;
// cargo test runs test threads in parallel, but env-writing tests are mutually serialized, and
// the lock is held across await only inside a current_thread runtime with no reentrant path,
// so it cannot deadlock.
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::platform::credential_store::{CredentialStore, MemoryCredentialStore};
    use crate::platform::paths::tests::ENV_LOCK;
    use secrets::mcp_secret_reference;
    use sha2::{Digest, Sha256};
    use std::future::Future;
    use std::io::{Cursor, Write as _};
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    /// 把 PINVOU3_HOME 指到一个干净临时目录跑闭包,跑完恢复并清理。
    /// 借 paths 的 ENV_LOCK 跟其它 mutate PINVOU3_HOME 的测试串行,避免互相覆盖。
    /// The in-process secret registry is snapshotted-cleared-restored the
    /// same way (secrets no longer land in the process env; the isolation
    /// point moves from env to the registry).
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // The test process installs the foundation resolver too (OnceLock is
        // idempotent, same contract as the production boot): otherwise the
        // foundation falls back to the process env when resolving
        // placeholders, and the test outcome would depend on whether the
        // bridge boot tests have already run (order coupling).
        super::install_mcp_secret_resolver();
        let prev = std::env::var("PINVOU3_HOME").ok();
        let prev_secrets = secrets::snapshot_secret_values();
        secrets::clear_secret_values_for_test();
        let dir = std::env::temp_dir().join(format!("pinvou3-mkt-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        f();
        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        secrets::restore_secret_values(prev_secrets);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G4 回归：mcp.json 路径重写以新目录存在为前提——自定义 MCP 搬迁 kept
    /// （新目录不存在）时保留旧路径，不得把条目改指到不存在的目录（否则工具
    /// 静默死掉且读路径无旧布局回退）；新目录出现后下一轮再重写（幂等）。
    #[test]
    fn migrate_mcp_json_paths_skips_missing_target_dir() {
        with_temp_home(|| {
            let id = "custom-weather";
            let old_dir = paths::bundle_mcp_servers_dir().join(id);
            std::fs::create_dir_all(&old_dir).unwrap();
            let mcp_path = paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            let old_text = old_dir.to_string_lossy().replace('\\', "/");
            std::fs::write(
                &mcp_path,
                format!(
                    r#"{{"servers":{{"{id}":{{"command":"{old_text}/run.cmd","args":["{old_text}/server.py"]}}}}}}"#
                ),
            )
            .unwrap();

            // 新目录不存在：不重写、旧路径保留
            assert!(!migrate_mcp_json_paths().unwrap());
            let content = std::fs::read_to_string(&mcp_path).unwrap();
            assert!(content.contains(&old_text), "新目录缺失时旧路径应保留");

            // 新目录出现（搬迁成功）：重写指向新目录
            let new_dir = mcp_catalog::package_mcp_dir(id);
            std::fs::create_dir_all(&new_dir).unwrap();
            assert!(migrate_mcp_json_paths().unwrap());
            let mcp: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&mcp_path).unwrap()).unwrap();
            let command = mcp["servers"][id]["command"].as_str().unwrap();
            assert!(
                command.starts_with(&*new_dir.to_string_lossy()),
                "重写后应指向新包目录，实际: {command}"
            );
        });
    }

    async fn with_temp_home_async<F, Fut>(f: F)
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ()>,
    {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // Same as with_temp_home: install the foundation resolver to avoid
        // test order coupling.
        super::install_mcp_secret_resolver();
        let prev = std::env::var("PINVOU3_HOME").ok();
        let prev_secrets = secrets::snapshot_secret_values();
        secrets::clear_secret_values_for_test();
        let dir =
            std::env::temp_dir().join(format!("pinvou3-mkt-test-async-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        f().await;
        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        secrets::restore_secret_values(prev_secrets);
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct MockMcpServer {
        url: String,
        seen_methods: Arc<StdMutex<Vec<String>>>,
        task: JoinHandle<()>,
    }

    impl Drop for MockMcpServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn spawn_mock_mcp_server(valid_key: &'static str) -> MockMcpServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen_methods = Arc::new(StdMutex::new(Vec::new()));
        let seen_for_task = Arc::clone(&seen_methods);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let seen = Arc::clone(&seen_for_task);
                tokio::spawn(async move {
                    let _ = handle_mock_mcp_request(stream, valid_key, seen).await;
                });
            }
        });
        MockMcpServer {
            url: format!("http://{addr}/mcp"),
            seen_methods,
            task,
        }
    }

    async fn handle_mock_mcp_request(
        mut stream: tokio::net::TcpStream,
        valid_key: &str,
        seen_methods: Arc<StdMutex<Vec<String>>>,
    ) -> std::io::Result<()> {
        let mut buffer = Vec::new();
        let header_end = loop {
            let mut chunk = [0u8; 1024];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Ok(());
            }
            buffer.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find_header_end(&buffer) {
                break pos;
            }
        };
        let headers = String::from_utf8_lossy(&buffer[..header_end]).to_string();
        let first = headers.lines().next().unwrap_or_default();
        let mut parts = first.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let _path = parts.next().unwrap_or_default();

        if method == "GET" {
            return write_http_response(&mut stream, 404, "text/plain", "not found").await;
        }

        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.trim().eq_ignore_ascii_case("content-length") {
                    value.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let body_start = header_end + 4;
        while buffer.len() < body_start + content_length {
            let mut chunk = [0u8; 1024];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..n]);
        }
        let body = &buffer[body_start..buffer.len().min(body_start + content_length)];
        let expected_authorization = format!("Bearer {valid_key}");
        let authorized = headers.lines().any(|line| {
            line.split_once(':').is_some_and(|(name, value)| {
                name.trim().eq_ignore_ascii_case("authorization")
                    && value.trim() == expected_authorization
            })
        });
        if !authorized {
            return write_http_response(&mut stream, 401, "text/plain", "unauthorized").await;
        }

        let request: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
        let rpc_method = request
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        seen_methods.lock().unwrap().push(rpc_method.clone());

        if rpc_method == "notifications/initialized" {
            return write_http_response(&mut stream, 202, "application/json", "").await;
        }

        let id = request.get("id").cloned().unwrap_or(serde_json::json!(1));
        let result = match rpc_method.as_str() {
            "initialize" => serde_json::json!({
            "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock-patsnap", "version": "1.0.0"}
            }),
            "tools/list" => serde_json::json!({
                "tools": [
                    {"name": "patsnap_search", "description": "search", "inputSchema": {"type": "object"}},
                    {"name": "patsnap_fetch", "description": "fetch", "inputSchema": {"type": "object"}}
                ]
            }),
            "resources/list" => serde_json::json!({"resources": []}),
            "resources/templates/list" => serde_json::json!({"resourceTemplates": []}),
            "prompts/list" => serde_json::json!({"prompts": []}),
            _ => serde_json::json!({}),
        };
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        });
        write_http_response(&mut stream, 200, "application/json", &response.to_string()).await
    }

    fn find_header_end(buffer: &[u8]) -> Option<usize> {
        buffer.windows(4).position(|w| w == b"\r\n\r\n")
    }

    async fn write_http_response(
        stream: &mut tokio::net::TcpStream,
        status: u16,
        content_type: &str,
        body: &str,
    ) -> std::io::Result<()> {
        let reason = match status {
            200 => "OK",
            202 => "Accepted",
            401 => "Unauthorized",
            404 => "Not Found",
            _ => "Error",
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await
    }

    /// 按新包布局写测试 manifest（`bundles/<id>/mcp/manifest.json`）——
    /// `load_manifest` 只读新布局（+ 内嵌 catalog），旧布局 `bundle/mcp-servers/` 已退役。
    fn write_tool_manifest(tool_id: &str, manifest: &str) {
        let dir = mcp_catalog::package_mcp_dir(tool_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    }

    fn test_python() -> (String, String) {
        // An explicit PINVOU3_TEST_PYTHON wins. Without it, start from the
        // platform adapter's preferred interpreter (posix: python3→python on
        // PATH; windows: PINVOU3_PYTHON/bundled python), then fall back to the
        // other common name — a dev machine may have either one installed, so
        // the default environment no longer requires setting
        // PINVOU3_TEST_PYTHON.
        let explicit = std::env::var("PINVOU3_TEST_PYTHON").ok();
        let mut candidates: Vec<String> = Vec::new();
        match &explicit {
            Some(python) => candidates.push(python.clone()),
            None => {
                let primary = crate::platform::os::python_command();
                let alternate = if primary == "python3" {
                    "python"
                } else {
                    "python3"
                };
                candidates.push(primary);
                candidates.push(alternate.to_string());
            }
        }
        for python in &candidates {
            let output = std::process::Command::new(python)
                .args([
                    "-I",
                    "-S",
                    "-c",
                    "import sys; print(f'{sys.version_info[0]}.{sys.version_info[1]}')",
                ])
                .output();
            let Ok(output) = output else { continue };
            if output.status.success() {
                let version = String::from_utf8(output.stdout).unwrap().trim().to_string();
                return (python.clone(), version);
            }
        }
        // An explicit but unusable setting is a different failure from a fully
        // failed default probe: the former should be fixed (or dropped), so
        // telling the user to "set it explicitly" again would mislead.
        if let Some(python) = explicit {
            panic!(
                "PINVOU3_TEST_PYTHON={python} is set but not usable; fix it or unset it to auto-probe"
            );
        }
        panic!(
            "no usable Python interpreter (tried {candidates:?}); set PINVOU3_TEST_PYTHON explicitly"
        );
    }

    struct LockedPythonToolFixture {
        wheel_bytes: Vec<u8>,
        sha256: String,
        cache_path: PathBuf,
    }

    impl LockedPythonToolFixture {
        fn seed_validated_cache(&self) {
            assert_eq!(
                crate::platform::encoding::hex_lower(&Sha256::digest(&self.wheel_bytes)),
                self.sha256
            );
            std::fs::create_dir_all(self.cache_path.parent().unwrap()).unwrap();
            std::fs::write(&self.cache_path, &self.wheel_bytes).unwrap();
        }
    }

    fn write_locked_python_tool(
        tool_id: &str,
        module: &str,
        python_version: &str,
    ) -> LockedPythonToolFixture {
        write_locked_python_tool_with_trust(tool_id, module, python_version, true)
    }

    fn write_locked_python_tool_with_trust(
        tool_id: &str,
        module: &str,
        python_version: &str,
        trusted: bool,
    ) -> LockedPythonToolFixture {
        let filename = format!("{module}-1.0.0-py3-none-any.whl");
        let mut wheel = Cursor::new(Vec::new());
        {
            let mut archive = zip::ZipWriter::new(&mut wheel);
            archive
                .start_file(
                    format!("{module}/__init__.py"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            archive.write_all(b"VALUE = 'managed'\n").unwrap();
            archive.finish().unwrap();
        }
        let wheel = wheel.into_inner();
        let sha256 = crate::platform::encoding::hex_lower(&Sha256::digest(&wheel));
        let cache = crate::platform::paths::pinvou3_home()
            .join("cache")
            .join("python-wheels");
        let fixture = LockedPythonToolFixture {
            cache_path: cache.join(format!("{sha256}.whl")),
            wheel_bytes: wheel,
            sha256: sha256.clone(),
        };
        fixture.seed_validated_cache();

        let platform = crate::platform::paths::connector_platform_dir(
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
        .unwrap();
        let manifest = serde_json::json!({
            "id": tool_id,
            "name": tool_id,
            "description": "fixture",
            "version": "1.0.0",
            "icon": "fixture",
            "category": "fixture",
            "mcp_tools": [format!("mcp_{tool_id}_run")],
            "command": "python",
            "args": ["server.py"],
            "python_dependencies": {
                "schema_version": 1,
                "targets": [{
                    "platform": platform,
                    "python": python_version,
                    "imports": [module],
                    "wheels": [{
                        "name": module,
                        "version": "1.0.0",
                        "filename": filename,
                        "url": format!("https://files.pythonhosted.org/packages/{filename}"),
                        "sha256": sha256
                    }]
                }]
            }
        });
        let manifest = serde_json::to_string_pretty(&manifest).unwrap();
        write_tool_manifest(tool_id, &manifest);
        if trusted {
            MarketplaceManager::<MemoryCredentialStore>::trust_dependency_manifest_for_test(
                serde_json::from_str(&manifest).unwrap(),
            );
        }
        let server_dir = mcp_catalog::package_mcp_dir(tool_id);
        std::fs::write(
            server_dir.join("server.py"),
            format!("from {module} import VALUE\nprint(VALUE)\n"),
        )
        .unwrap();
        let runner = crate::platform::paths::bundle_mcp_python_runner();
        std::fs::create_dir_all(runner.parent().unwrap()).unwrap();
        std::fs::write(
            runner,
            include_str!(
                "../../../resources/common/bundle/mcp-servers/python_dependency_runner.py"
            ),
        )
        .unwrap();
        fixture
    }

    fn write_pip_python_tool(tool_id: &str, trusted: bool) {
        let manifest = serde_json::json!({
            "id": tool_id,
            "name": tool_id,
            "description": "fixture",
            "version": "1.0.0",
            "icon": "fixture",
            "category": "fixture",
            "mcp_tools": [format!("mcp_{tool_id}_run")],
            "command": "python",
            "args": ["server.py"],
            "pip_dependencies": ["pinvou3-test-only-dependency"]
        });
        let manifest = serde_json::to_string_pretty(&manifest).unwrap();
        write_tool_manifest(tool_id, &manifest);
        std::fs::write(
            mcp_catalog::package_mcp_dir(tool_id).join("server.py"),
            "print('pip fixture')\n",
        )
        .unwrap();
        if trusted {
            MarketplaceManager::<MemoryCredentialStore>::trust_dependency_manifest_for_test(
                serde_json::from_str(&manifest).unwrap(),
            );
        }
    }

    fn write_legacy_python_state(tool_id: &str) {
        write_installed_ids(&[tool_id.to_string()]);
        store::BundleStore::new()
            .upsert(store::BundleRecord::installed_now(
                tool_id,
                store::BundleSource::Preset,
            ))
            .unwrap();
        let server = mcp_catalog::package_mcp_dir(tool_id).join("server.py");
        let mut servers = serde_json::Map::new();
        servers.insert(
            tool_id.to_string(),
            serde_json::json!({ "command": "python", "args": [server] }),
        );
        connectors::write_json_pretty(
            &crate::platform::paths::mcp_config_path(),
            &serde_json::json!({ "servers": servers }),
        )
        .unwrap();
    }

    fn manager_installed_bytes() -> Vec<u8> {
        std::fs::read(
            crate::platform::paths::pinvou3_home()
                .join("marketplace")
                .join("installed.json"),
        )
        .unwrap()
    }

    #[test]
    fn installed_write_failure_rolls_back_mcp_and_survives_reopen() {
        with_temp_home(|| {
            write_installed_ids(&["existing".to_string()]);
            let old_mcp = serde_json::json!({
                "servers": { "existing": { "command": "node", "args": ["existing.js"] } }
            });
            connectors::write_json_pretty(&crate::platform::paths::mcp_config_path(), &old_mcp)
                .unwrap();
            write_tool_manifest(
                "half-install",
                r#"{
                    "id":"half-install","name":"Half","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"node","args":["server.js"]
                }"#,
            );

            FAIL_NEXT_INSTALLED_WRITE.store(true, std::sync::atomic::Ordering::SeqCst);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let error = manager
                .install("half-install", &std::collections::HashMap::new())
                .unwrap_err();
            assert!(error.contains("installed.json"));

            let reopened = MarketplaceManager::with_store(MemoryCredentialStore::default());
            assert_eq!(reopened.installed_ids(), vec!["existing".to_string()]);
            assert_eq!(read_mcp_json(), old_mcp);
            assert!(!marketplace_transaction_journal().exists());
        });
    }

    /// A failed install rolls the state back to an mcp.json entry that may reference a
    /// pre-upgrade environment no longer present in the catalog preserve-set. Pruning
    /// on the rollback path would delete that environment and strand the tool until
    /// the next successful startup repair, so pruning must stay deferred to repair.
    #[test]
    fn install_rollback_defers_prune_of_referenced_environment() {
        with_temp_home(|| {
            let (python, version) = test_python();
            write_locked_python_tool("trusted-lock", "trusted_lock_fixture", &version);
            store::BundleStore::new()
                .upsert(store::BundleRecord::installed_now(
                    "trusted-lock",
                    store::BundleSource::Preset,
                ))
                .unwrap();
            write_installed_ids(&["trusted-lock".to_string()]);
            let old_environment = crate::platform::paths::pinvou3_home()
                .join("marketplace")
                .join("python-envs")
                .join("f".repeat(64));
            std::fs::create_dir_all(&old_environment).unwrap();
            let server = mcp_catalog::package_mcp_dir("trusted-lock").join("server.py");
            connectors::write_json_pretty(
                &crate::platform::paths::mcp_config_path(),
                &serde_json::json!({
                    "servers": {
                        "trusted-lock": {
                            "command": python,
                            "args": [
                                "-I", "-S", "-B",
                                crate::platform::paths::bundle_mcp_python_runner(),
                                old_environment.join("site-packages"),
                                server,
                            ],
                        }
                    }
                }),
            )
            .unwrap();

            FAIL_NEXT_INSTALLED_WRITE.store(true, std::sync::atomic::Ordering::SeqCst);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            assert!(
                manager
                    .install("trusted-lock", &std::collections::HashMap::new())
                    .is_err()
            );
            assert!(
                old_environment.is_dir(),
                "the rolled-back mcp.json still references this environment"
            );
            assert_eq!(
                read_mcp_json()["servers"]["trusted-lock"]["args"][3],
                crate::platform::paths::bundle_mcp_python_runner()
                    .to_string_lossy()
                    .as_ref()
            );
        });
    }

    #[test]
    fn startup_recovers_interrupted_cross_file_transaction() {
        with_temp_home(|| {
            write_installed_ids(&["old-tool".to_string()]);
            let old_mcp = serde_json::json!({
                "servers": { "old-tool": { "command": "node", "args": ["old.js"] } }
            });
            connectors::write_json_pretty(&crate::platform::paths::mcp_config_path(), &old_mcp)
                .unwrap();
            let snapshot = MarketplaceStateSnapshot {
                installed: read_optional_file(
                    &crate::platform::paths::pinvou3_home()
                        .join("marketplace")
                        .join("installed.json"),
                )
                .unwrap(),
                mcp: read_optional_file(&crate::platform::paths::mcp_config_path()).unwrap(),
            };
            write_atomic_file(
                &marketplace_transaction_journal(),
                &serde_json::to_vec(&snapshot).unwrap(),
            )
            .unwrap();

            write_installed_ids(&["new-tool".to_string()]);
            connectors::write_json_pretty(
                &crate::platform::paths::mcp_config_path(),
                &serde_json::json!({
                    "servers": { "new-tool": { "command": "node", "args": ["new.js"] } }
                }),
            )
            .unwrap();

            let reopened = MarketplaceManager::with_store(MemoryCredentialStore::default());
            assert!(reopened.repair_installed_python_tools().unwrap().is_empty());
            assert_eq!(reopened.installed_ids(), vec!["old-tool".to_string()]);
            assert_eq!(read_mcp_json(), old_mcp);
            assert!(!marketplace_transaction_journal().exists());
        });
    }

    #[test]
    fn startup_repair_does_not_fail_when_unused_environment_cleanup_is_blocked() {
        with_temp_home(|| {
            let unused_environment = crate::platform::paths::pinvou3_home()
                .join("marketplace")
                .join("python-envs")
                .join("f".repeat(64));
            let other_unused_environment =
                unused_environment.parent().unwrap().join("e".repeat(64));
            std::fs::create_dir_all(&unused_environment).unwrap();
            std::fs::create_dir_all(&other_unused_environment).unwrap();
            python_dependencies::fail_next_prune_removal_for_test(unused_environment.clone());

            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            assert!(manager.repair_installed_python_tools().unwrap().is_empty());
            assert!(unused_environment.is_dir());
            assert!(!other_unused_environment.exists());

            assert!(manager.repair_installed_python_tools().unwrap().is_empty());
            assert!(!unused_environment.exists());
        });
    }

    #[test]
    fn legacy_python_install_is_repaired_before_engine_use() {
        with_temp_home(|| {
            let (python, version) = test_python();
            write_locked_python_tool("legacy-doc", "legacy_fixture", &version);
            write_legacy_python_state("legacy-doc");
            let mut configured = read_mcp_json();
            let configured_entry = configured["servers"]["legacy-doc"].as_object_mut().unwrap();
            configured_entry.insert(
                "env".to_string(),
                serde_json::json!({"API_KEY": "${PINVOU3_MCP_API_KEY}"}),
            );
            configured_entry.insert("enabled".to_string(), serde_json::json!(false));
            configured_entry.insert("timeout_ms".to_string(), serde_json::json!(12345));
            configured_entry.insert(
                "future_field".to_string(),
                serde_json::json!({"nested": [1, 2, 3]}),
            );
            connectors::write_json_pretty(&paths::mcp_config_path(), &configured).unwrap();

            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            assert!(
                manager
                    .repair_installed_python_tools_with_python(&python)
                    .unwrap()
                    .is_empty()
            );

            let mcp = read_mcp_json();
            let entry = &mcp["servers"]["legacy-doc"];
            let args = entry["args"].as_array().unwrap();
            assert_eq!(entry["command"], python);
            assert_eq!(args[0], "-I");
            assert_eq!(args[1], "-S");
            assert_eq!(args[2], "-B");
            assert_eq!(
                Path::new(args[3].as_str().unwrap()),
                crate::platform::paths::bundle_mcp_python_runner()
            );
            assert!(Path::new(args[4].as_str().unwrap()).is_dir());
            let output = std::process::Command::new(entry["command"].as_str().unwrap())
                .args(args.iter().map(|value| value.as_str().unwrap()))
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "managed");
            assert_eq!(entry["env"], configured["servers"]["legacy-doc"]["env"]);
            assert_eq!(entry["enabled"], false);
            assert_eq!(entry["timeout_ms"], 12345);
            assert_eq!(
                entry["future_field"],
                configured["servers"]["legacy-doc"]["future_field"]
            );

            let repaired_entry = entry.clone();
            assert!(
                manager
                    .repair_installed_python_tools_with_python(&python)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                read_mcp_json()["servers"]["legacy-doc"],
                repaired_entry,
                "repeated repair must be idempotent"
            );
        });
    }

    #[test]
    fn transient_legacy_python_repair_failure_preserves_state_for_startup_retry() {
        with_temp_home(|| {
            let (python, version) = test_python();
            let fixture = write_locked_python_tool("legacy-retry", "retry_fixture", &version);
            write_legacy_python_state("legacy-retry");
            let installed_before = manager_installed_bytes();
            let mcp_before = std::fs::read(paths::mcp_config_path()).unwrap();
            // 无主孤儿环境：活跃锁集合不含它，只有干净启动的 prune 才允许清掉。
            let stale_env = python_dependencies::environments_root_for_test().join("a".repeat(64));
            std::fs::create_dir_all(&stale_env).unwrap();
            python_dependencies::fail_next_download_for_test();

            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let errors = manager
                .repair_installed_python_tools_with_python(&python)
                .unwrap();
            assert_eq!(errors.len(), 1);
            assert!(errors[0].contains("will retry"));
            assert!(
                manager
                    .installed_ids()
                    .contains(&"legacy-retry".to_string())
            );
            assert_eq!(std::fs::read(paths::mcp_config_path()).unwrap(), mcp_before);
            assert_eq!(manager_installed_bytes(), installed_before);
            assert!(!marketplace_transaction_journal().exists());
            assert!(
                fixture.cache_path.is_file(),
                "live lock must retain its cache"
            );
            assert!(stale_env.is_dir(), "transient failure must defer env prune");
            let cooldown_path = python_dependencies::repair_cooldown_path_for_test();
            assert!(
                cooldown_path.is_file(),
                "failed download must arm the cooldown"
            );

            // 冷却期内不再触达下载器，只上报延期；下个冷却窗口外的启动重试。
            python_dependencies::fail_next_download_for_test();
            let deferred = manager
                .repair_installed_python_tools_with_python(&python)
                .unwrap();
            assert_eq!(deferred.len(), 1);
            assert!(deferred[0].contains("cooldown"));
            assert!(
                python_dependencies::take_pending_download_failure_for_test(),
                "deferred repair must not reach the downloader"
            );
            assert!(stale_env.is_dir());

            // 冷却过期（回拨时间戳）→ 重试成功，冷却标记清空，prune 恢复执行。
            let entries: std::collections::HashMap<String, u64> =
                serde_json::from_str(&std::fs::read_to_string(&cooldown_path).unwrap()).unwrap();
            assert!(!entries.is_empty());
            std::fs::write(
                &cooldown_path,
                serde_json::to_vec(
                    &entries
                        .into_iter()
                        .map(|(key, _)| (key, 0_u64))
                        .collect::<std::collections::HashMap<String, u64>>(),
                )
                .unwrap(),
            )
            .unwrap();
            assert!(
                manager
                    .repair_installed_python_tools_with_python(&python)
                    .unwrap()
                    .is_empty()
            );
            assert!(
                !cooldown_path.exists(),
                "successful repair must clear the cooldown"
            );
            assert!(!stale_env.exists(), "clean repair must run the env prune");
            assert_eq!(read_mcp_json()["servers"]["legacy-retry"]["args"][0], "-I");
        });
    }

    /// 损坏的 state-transaction.json 不得让启动修复失败（会把整个引擎池拖成
    /// 不可用且每次启动复现）：必须被隔离，on-disk 状态保持原样，后续事务照常。
    #[test]
    fn corrupt_transaction_journal_is_quarantined_instead_of_blocking_startup() {
        with_temp_home(|| {
            let journal = marketplace_transaction_journal();
            std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
            std::fs::write(&journal, b"{not valid json").unwrap();

            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let errors = manager.repair_installed_python_tools().unwrap();
            assert!(errors.is_empty());
            assert!(!journal.exists(), "quarantined journal must leave its path");
            let quarantined: Vec<String> = std::fs::read_dir(journal.parent().unwrap())
                .unwrap()
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .filter(|name| name.starts_with("state-transaction.corrupt-"))
                .collect();
            assert_eq!(
                quarantined.len(),
                1,
                "corrupt journal must be quarantined, not blocking startup"
            );
        });
    }

    /// commit 对 journal 的删除遇到瞬时占用（Windows 杀毒/索引器）必须重试成功，
    /// 而不是把一次可自愈的占用升级成 Integrity 失败。
    #[test]
    fn transaction_commit_retries_transient_journal_removal_failure() {
        with_temp_home(|| {
            fail_next_journal_removal_for_test();
            let installed_file = paths::pinvou3_home()
                .join("marketplace")
                .join("installed.json");
            let transaction = MarketplaceStateTransaction::begin(&installed_file).unwrap();
            transaction.commit().expect("retry must absorb one failure");
            assert!(!marketplace_transaction_journal().exists());
        });
    }

    #[test]
    fn untrusted_upload_dependencies_never_execute_during_install_repair_or_reinstall() {
        with_temp_home(|| {
            let (python, version) = test_python();
            write_locked_python_tool_with_trust(
                "upload-lock",
                "upload_lock_fixture",
                &version,
                false,
            );
            python_dependencies::fail_next_download_for_test();
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            manager
                .install_upload(
                    "upload-lock",
                    store::BundleSource::Upload("upload-lock.zip".to_string()),
                )
                .unwrap();
            assert!(
                manager
                    .repair_installed_python_tools_with_python(&python)
                    .unwrap()
                    .is_empty()
            );
            manager.uninstall("upload-lock").unwrap();
            manager
                .install_with_python("upload-lock", &std::collections::HashMap::new(), &python)
                .unwrap();
            assert!(
                python_dependencies::take_pending_download_failure_for_test(),
                "untrusted wheel lock must never reach the downloader"
            );

            write_pip_python_tool("upload-pip", false);
            connectors::set_next_pip_install_result_for_test(1);
            manager
                .install_upload(
                    "upload-pip",
                    store::BundleSource::Upload("upload-pip.zip".to_string()),
                )
                .unwrap();
            assert!(
                manager
                    .repair_installed_python_tools_with_python(&python)
                    .unwrap()
                    .is_empty()
            );
            manager.uninstall("upload-pip").unwrap();
            manager
                .install("upload-pip", &std::collections::HashMap::new())
                .unwrap();
            assert_eq!(
                connectors::take_pending_pip_install_result_for_test(),
                1,
                "untrusted pip declaration must never reach pip"
            );
        });
    }

    /// The legacy pip fallback (lock without a target for this platform) must
    /// honor the same automatic-retry cooldown as the wheel path: an offline
    /// machine must not replay pip timeouts on every startup, while explicit
    /// installs stay unrestricted and clear the marker on success.
    #[test]
    fn pip_fallback_repair_honors_retry_cooldown() {
        if crate::platform::capabilities::is_windows() {
            // Windows never takes the pip fallback: a lock without a Windows
            // target is a Permanent error covered by the other repair tests.
            return;
        }
        with_temp_home(|| {
            let (python, version) = test_python();
            let manifest = serde_json::json!({
                "id": "cooled-pip",
                "name": "cooled-pip",
                "description": "fixture",
                "version": "1.0.0",
                "icon": "fixture",
                "category": "fixture",
                "mcp_tools": ["mcp_cooled_pip_run"],
                "command": "python",
                "args": ["server.py"],
                "pip_dependencies": ["pinvou3-test-only-dependency"],
                "python_dependencies": {
                    "schema_version": 1,
                    "targets": [{
                        "platform": "windows-x64",
                        "python": version,
                        "imports": ["cooled_pip_fixture"],
                        "wheels": [{
                            "name": "cooled-pip-fixture",
                            "version": "1.0.0",
                            "filename": "cooled_pip_fixture-1.0.0-py3-none-any.whl",
                            "url": "https://files.pythonhosted.org/packages/cooled_pip_fixture-1.0.0-py3-none-any.whl",
                            "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
                        }]
                    }]
                }
            });
            let manifest = serde_json::to_string_pretty(&manifest).unwrap();
            write_tool_manifest("cooled-pip", &manifest);
            MarketplaceManager::<MemoryCredentialStore>::trust_dependency_manifest_for_test(
                serde_json::from_str(&manifest).unwrap(),
            );
            std::fs::write(
                mcp_catalog::package_mcp_dir("cooled-pip").join("server.py"),
                "print('pip fallback fixture')\n",
            )
            .unwrap();
            write_legacy_python_state("cooled-pip");

            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            connectors::set_next_pip_install_result_for_test(1);
            let errors = manager
                .repair_installed_python_tools_with_python(&python)
                .unwrap();
            assert_eq!(errors.len(), 1);
            assert!(errors[0].contains("will retry"));

            connectors::set_next_pip_install_result_for_test(2);
            let errors = manager
                .repair_installed_python_tools_with_python(&python)
                .unwrap();
            assert_eq!(errors.len(), 1);
            assert!(
                errors[0].contains("cooldown"),
                "deferred repair must report the retry cooldown: {}",
                errors[0]
            );
            assert_eq!(
                connectors::take_pending_pip_install_result_for_test(),
                2,
                "cooled-down repair must not reach pip"
            );

            connectors::set_next_pip_install_result_for_test(2);
            manager
                .install_with_python("cooled-pip", &std::collections::HashMap::new(), &python)
                .unwrap();
            assert_eq!(
                connectors::take_pending_pip_install_result_for_test(),
                0,
                "explicit install must bypass the cooldown and reach pip"
            );
            assert!(!python_dependencies::repair_cooldown_path_for_test().exists());
        });
    }

    #[test]
    fn trusted_catalog_dependencies_execute_for_initial_install() {
        with_temp_home(|| {
            let (python, version) = test_python();
            write_locked_python_tool("trusted-lock", "trusted_lock_fixture", &version);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            manager
                .install_with_python("trusted-lock", &std::collections::HashMap::new(), &python)
                .unwrap();
            assert_eq!(read_mcp_json()["servers"]["trusted-lock"]["args"][0], "-I");

            write_pip_python_tool("trusted-pip", true);
            connectors::set_next_pip_install_result_for_test(2);
            manager
                .install("trusted-pip", &std::collections::HashMap::new())
                .unwrap();
            assert_eq!(connectors::take_pending_pip_install_result_for_test(), 0);
            assert!(read_mcp_json()["servers"].get("trusted-pip").is_some());
        });
    }

    #[test]
    fn upload_source_and_corrupt_mirror_block_trusted_dependency_execution() {
        with_temp_home(|| {
            let (python, version) = test_python();
            write_locked_python_tool("trusted-upload-lock", "trusted_upload_fixture", &version);
            python_dependencies::fail_next_download_for_test();
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            manager
                .install_upload(
                    "trusted-upload-lock",
                    store::BundleSource::Upload("trusted-upload-lock.zip".to_string()),
                )
                .unwrap();
            let errors = manager
                .repair_installed_python_tools_with_python(&python)
                .unwrap();
            assert_eq!(errors.len(), 1);
            assert!(errors[0].contains("provenance"));
            assert!(python_dependencies::take_pending_download_failure_for_test());

            write_pip_python_tool("trusted-upload-pip", true);
            connectors::set_next_pip_install_result_for_test(1);
            manager
                .install_upload(
                    "trusted-upload-pip",
                    store::BundleSource::Upload("trusted-upload-pip.zip".to_string()),
                )
                .unwrap();
            assert_eq!(connectors::take_pending_pip_install_result_for_test(), 1);

            write_pip_python_tool("trusted-corrupt-pip", true);
            std::fs::write(store::BundleStore::new().file_path(), b"{not-json").unwrap();
            connectors::set_next_pip_install_result_for_test(1);
            let error = manager
                .install("trusted-corrupt-pip", &std::collections::HashMap::new())
                .unwrap_err();
            assert!(error.contains("provenance"));
            assert_eq!(connectors::take_pending_pip_install_result_for_test(), 1);
        });
    }

    #[test]
    fn startup_repair_fails_closed_when_preset_mirror_is_missing() {
        with_temp_home(|| {
            let (python, version) = test_python();
            write_locked_python_tool("missing-mirror", "missing_mirror_fixture", &version);
            write_legacy_python_state("missing-mirror");
            store::BundleStore::new().remove("missing-mirror").unwrap();
            let installed_before = manager_installed_bytes();
            let mcp_before = std::fs::read(paths::mcp_config_path()).unwrap();
            python_dependencies::fail_next_download_for_test();

            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let errors = manager
                .repair_installed_python_tools_with_python(&python)
                .unwrap();
            assert_eq!(errors.len(), 1);
            assert!(errors[0].contains("provenance"));
            assert!(python_dependencies::take_pending_download_failure_for_test());
            assert_eq!(manager_installed_bytes(), installed_before);
            assert_eq!(std::fs::read(paths::mcp_config_path()).unwrap(), mcp_before);
            assert!(!marketplace_transaction_journal().exists());
        });
    }

    #[test]
    fn startup_repair_fails_closed_when_preset_mirror_is_corrupt() {
        with_temp_home(|| {
            let (python, version) = test_python();
            write_locked_python_tool("corrupt-mirror", "corrupt_mirror_fixture", &version);
            write_legacy_python_state("corrupt-mirror");
            std::fs::write(store::BundleStore::new().file_path(), b"{not-json").unwrap();
            let installed_before = manager_installed_bytes();
            let mcp_before = std::fs::read(paths::mcp_config_path()).unwrap();
            python_dependencies::fail_next_download_for_test();

            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let errors = manager
                .repair_installed_python_tools_with_python(&python)
                .unwrap();
            assert_eq!(errors.len(), 1);
            assert!(errors[0].contains("provenance"));
            assert!(python_dependencies::take_pending_download_failure_for_test());
            assert_eq!(manager_installed_bytes(), installed_before);
            assert_eq!(std::fs::read(paths::mcp_config_path()).unwrap(), mcp_before);
            assert!(!marketplace_transaction_journal().exists());
        });
    }

    #[test]
    fn permanent_repair_downgrade_write_failure_rolls_back_and_continues() {
        with_temp_home(|| {
            let (python, version) = test_python();
            write_locked_python_tool("invalid-lock", "invalid_lock_fixture", &version);
            write_locked_python_tool("repair-next", "repair_next_fixture", &version);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let mut invalid = manager.load_manifest("invalid-lock").unwrap();
            invalid.python_dependencies.as_mut().unwrap().schema_version = 999;
            MarketplaceManager::<MemoryCredentialStore>::trust_dependency_manifest_for_test(
                invalid,
            );
            write_installed_ids(&["invalid-lock".to_string(), "repair-next".to_string()]);
            for tool_id in ["invalid-lock", "repair-next"] {
                store::BundleStore::new()
                    .upsert(store::BundleRecord::installed_now(
                        tool_id,
                        store::BundleSource::Preset,
                    ))
                    .unwrap();
            }
            let invalid_entry = serde_json::json!({
                "command": "python",
                "args": [mcp_catalog::package_mcp_dir("invalid-lock").join("server.py")],
                "env": {"KEEP": "exact"}
            });
            connectors::write_json_pretty(
                &paths::mcp_config_path(),
                &serde_json::json!({
                    "servers": {
                        "invalid-lock": invalid_entry.clone(),
                        "repair-next": {
                            "command": "python",
                            "args": [mcp_catalog::package_mcp_dir("repair-next").join("server.py")]
                        }
                    }
                }),
            )
            .unwrap();
            let installed_before = manager_installed_bytes();
            FAIL_NEXT_INSTALLED_WRITE.store(true, std::sync::atomic::Ordering::SeqCst);

            let errors = manager
                .repair_installed_python_tools_with_python(&python)
                .unwrap();
            assert!(
                errors
                    .iter()
                    .any(|error| error.contains("downgrade failed and was rolled back"))
            );
            assert_eq!(manager_installed_bytes(), installed_before);
            let mcp = read_mcp_json();
            assert_eq!(mcp["servers"]["invalid-lock"], invalid_entry);
            assert_eq!(mcp["servers"]["repair-next"]["args"][0], "-I");
            assert!(!marketplace_transaction_journal().exists());
        });
    }

    #[test]
    fn shared_companion_remains_available_while_any_connector_is_installed() {
        with_temp_home(|| {
            for tool_id in ["connector-a", "connector-b"] {
                write_tool_manifest(
                    tool_id,
                    &serde_json::json!({
                        "id": tool_id,
                        "name": tool_id,
                        "description": "fixture",
                        "version": "1",
                        "icon": "x",
                        "category": "c",
                        "mcp_tools": [],
                        "command": "node",
                        "args": ["server.js"],
                        "companion_skills": ["shared-skill"]
                    })
                    .to_string(),
                );
            }
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            write_installed_ids(&["connector-a".to_string()]);
            assert!(
                !manager
                    .unavailable_companion_skills()
                    .contains(&"shared-skill".to_string())
            );

            write_installed_ids(&["connector-b".to_string()]);
            assert!(
                !manager
                    .unavailable_companion_skills()
                    .contains(&"shared-skill".to_string())
            );

            write_installed_ids(&[]);
            assert!(
                manager
                    .unavailable_companion_skills()
                    .contains(&"shared-skill".to_string())
            );
        });
    }

    #[test]
    fn concurrent_uninstall_reloads_liveness_after_install_commit() {
        with_temp_home(|| {
            let (python, version) = test_python();
            write_locked_python_tool("old-doc", "old_fixture", &version);
            write_locked_python_tool("new-doc", "new_fixture", &version);
            MarketplaceManager::with_store(MemoryCredentialStore::default())
                .install_with_python("old-doc", &std::collections::HashMap::new(), &python)
                .unwrap();
            let old_mcp = read_mcp_json();
            let old_environment =
                PathBuf::from(old_mcp["servers"]["old-doc"]["args"][4].as_str().unwrap());
            assert!(old_environment.is_dir());

            let entered = Arc::new(std::sync::Barrier::new(2));
            let release = Arc::new(std::sync::Barrier::new(2));
            *INSTALL_PAUSE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(InstallPause {
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            });

            let install_python = python.clone();
            let install = std::thread::spawn(move || {
                MarketplaceManager::with_store(MemoryCredentialStore::default())
                    .install_with_python(
                        "new-doc",
                        &std::collections::HashMap::new(),
                        &install_python,
                    )
            });
            entered.wait();
            let uninstall = std::thread::spawn(|| {
                MarketplaceManager::with_store(MemoryCredentialStore::default())
                    .uninstall("old-doc")
            });
            release.wait();
            install.join().unwrap().unwrap();
            uninstall.join().unwrap().unwrap();
            *INSTALL_PAUSE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;

            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            assert_eq!(manager.installed_ids(), vec!["new-doc".to_string()]);
            let mcp = read_mcp_json();
            assert!(mcp["servers"].get("old-doc").is_none());
            let new_server = &mcp["servers"]["new-doc"];
            assert!(new_server.is_object());
            let args = new_server["args"].as_array().unwrap();
            assert_eq!(
                Path::new(args[3].as_str().unwrap()),
                crate::platform::paths::bundle_mcp_python_runner()
            );
            let new_environment = Path::new(args[4].as_str().unwrap());
            assert!(
                !old_environment.exists(),
                "uninstall should collect the old unreferenced environment"
            );
            assert!(
                new_environment.is_dir(),
                "the concurrently installed environment must remain live"
            );
        });
    }

    fn read_mcp_json() -> serde_json::Value {
        let content = std::fs::read_to_string(crate::platform::paths::mcp_config_path()).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    fn secret_value(name: &str) -> String {
        format!("test-secret-{name}-value-123456")
    }

    /// 连接器 → 模型可见工具全名:裸名补 `mcp_{id}_` 前缀,已带 `mcp_` 的原样(不双前缀),
    /// 一律小写;不存在的连接器跳过。这正是当初打印映射时抓到"双前缀 bug"的那段逻辑。
    #[test]
    fn installed_ids_recovers_corrupt_file_from_mcp_json() {
        with_temp_home(|| {
            write_tool_manifest(
                "weather",
                r#"{
                    "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["get_weather"],"command":"python","args":["server.py"]
                }"#,
            );
            let mcp_path = crate::platform::paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            std::fs::write(
                &mcp_path,
                r#"{"servers":{"weather":{"command":"python3","args":["server.py"]}}}"#,
            )
            .unwrap();
            let installed_path = crate::platform::paths::pinvou3_home()
                .join("marketplace")
                .join("installed.json");
            std::fs::create_dir_all(installed_path.parent().unwrap()).unwrap();
            std::fs::write(&installed_path, "[\"weather\"").unwrap();

            let ids = MarketplaceManager::new().installed_ids();

            assert_eq!(ids, vec!["weather".to_string()]);
            let repaired = std::fs::read_to_string(&installed_path).unwrap();
            assert_eq!(
                serde_json::from_str::<Vec<String>>(&repaired).unwrap(),
                vec!["weather".to_string()]
            );
            let backups: Vec<_> = std::fs::read_dir(installed_path.parent().unwrap())
                .unwrap()
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("installed.json.corrupt.")
                })
                .collect();
            assert_eq!(backups.len(), 1);
        });
    }

    /// list_tools（composer 工具菜单数据源）必须与 BundleRegistry::list
    /// （bundle_readiness）同口径应用上传包的展示名/说明覆盖——两处标题
    /// 不一致就是这条路径漏了覆盖（评审发现的测试空缺）。
    #[test]
    fn list_tools_applies_upload_display_override() {
        with_temp_home(|| {
            write_tool_manifest(
                "up-disp",
                r#"{
                    "id":"up-disp","name":"ManifestName","description":"manifest d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"]
                }"#,
            );
            let store = store::BundleStore::new();
            store
                .upsert(store::BundleRecord::installed_now(
                    "up-disp",
                    store::BundleSource::Upload("pkg.zip".to_string()),
                ))
                .unwrap();
            store
                .set_display_meta("up-disp", Some("我的工具"), Some("自定义说明"))
                .unwrap();

            let tools = MarketplaceManager::new().list_tools();
            let t = tools.iter().find(|t| t.id == "up-disp").unwrap();
            assert_eq!(t.name, "我的工具", "extra 覆盖应优先于 manifest name");
            assert_eq!(t.description, "自定义说明");

            // 清空覆盖 → 回退 manifest 值
            store
                .set_display_meta("up-disp", Some(""), Some(""))
                .unwrap();
            let tools = MarketplaceManager::new().list_tools();
            let t = tools.iter().find(|t| t.id == "up-disp").unwrap();
            assert_eq!(t.name, "ManifestName");
            assert_eq!(t.description, "manifest d");
        });
    }

    /// bundles.json 损坏时 list_tools 降级为「无覆盖」（warn + manifest 原值），
    /// 不得 panic 或丢工具——与 bundle.rs 的 store 读回退同口径。
    #[test]
    fn list_tools_degrades_to_manifest_values_when_store_corrupt() {
        with_temp_home(|| {
            write_tool_manifest(
                "up-corrupt",
                r#"{
                    "id":"up-corrupt","name":"ManifestName","description":"manifest d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"]
                }"#,
            );
            let store = store::BundleStore::new();
            store
                .upsert(store::BundleRecord::installed_now(
                    "up-corrupt",
                    store::BundleSource::Upload("pkg.zip".to_string()),
                ))
                .unwrap();
            store
                .set_display_meta("up-corrupt", Some("我的工具"), None)
                .unwrap();
            // 直接腐坏 bundles.json（绕过 store 的原子写）
            std::fs::write(store.file_path(), "{not json").unwrap();

            let tools = MarketplaceManager::new().list_tools();
            let t = tools.iter().find(|t| t.id == "up-corrupt").unwrap();
            assert_eq!(t.name, "ManifestName", "store 读失败应降级为 manifest 值");
        });
    }

    #[test]
    fn model_tool_names_prefix_dedup_and_lowercase() {
        with_temp_home(|| {
            write_tool_manifest(
                "demo",
                r#"{
                "id":"demo","name":"Demo","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":["bare_tool","mcp_demo_already","UPPER_Tool"],
                "command":"python","args":[]
            }"#,
            );

            let mgr = MarketplaceManager::new();
            let names = mgr.model_tool_names(&["demo".to_string()]);
            assert_eq!(
                names,
                vec![
                    "mcp_demo_bare_tool".to_string(),  // 裸名 → 补前缀
                    "mcp_demo_already".to_string(), // 已带 mcp_ → 原样,不变成 mcp_demo_mcp_demo_already
                    "mcp_demo_upper_tool".to_string(), // 小写化
                ]
            );
            // 没装/不存在的连接器 → 跳过(不报错,空)
            assert!(mgr.model_tool_names(&["nope".to_string()]).is_empty());
        });
    }

    /// 远程 server 连接器可能没有静态 mcp_tools 列表(qcc 即如此)。禁用时必须按
    /// server 名生成前缀规则,否则底座仍会暴露该连接器动态发现出来的全部工具。
    #[test]
    fn model_tool_names_generates_prefix_rules_for_remote_servers() {
        with_temp_home(|| {
            let dir = crate::platform::paths::bundle_mcp_servers_dir().join("qcc");
            std::fs::create_dir_all(&dir).unwrap();
            let manifest = r#"{
                "id":"qcc","name":"企查查","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],
                "command":"python","args":[],
                "servers":[
                    {
                        "name":"qcc-company",
                        "url":"https://agent.qcc.com/mcp/company/stream",
                        "scopes":["mcp:tools"],
                        "oauth_resource":"https://agent.qcc.com/mcp/company/stream"
                    }
                ]
            }"#;
            std::fs::write(dir.join("manifest.json"), manifest).unwrap();

            let mgr = MarketplaceManager::new();
            // 对齐真实 qcc manifest 的 qcc-company OAuth 远程 server。
            assert_eq!(
                mgr.model_tool_names(&["qcc".to_string()]),
                vec!["mcp_qcc-company_*".to_string()]
            );
        });
    }

    #[test]
    fn canva_oauth_server_writes_config_and_model_prefix() {
        with_temp_home(|| {
            write_tool_manifest(
                "canva-mcp",
                r#"{
                    "id":"canva-mcp","name":"Canva 可画","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "servers":[
                        {
                            "name":"canva_mcp",
                            "url":"https://mcp.canva.cn/mcp",
                            "scopes":[
                                "profile:read",
                                "design:meta:read",
                                "design:content:write",
                                "design:content:read",
                                "folder:read",
                                "folder:write",
                                "brandtemplate:content:read",
                                "brandtemplate:meta:read",
                                "brandtemplate:content:write",
                                "comment:write",
                                "comment:read",
                                "asset:read",
                                "asset:write",
                                "brandkit:read",
                                "help:answers:read",
                                "help:answers:write"
                            ],
                            "oauth_resource":"https://mcp.canva.cn/mcp"
                        }
                    ]
                }"#,
            );

            let mgr = MarketplaceManager::new();
            mgr.install("canva-mcp", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            let server = &mcp["servers"]["canva_mcp"];
            assert_eq!(server["url"], "https://mcp.canva.cn/mcp");
            assert_eq!(server["oauth_resource"], "https://mcp.canva.cn/mcp");
            assert!(
                server["scopes"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("profile:read"))
            );
            assert!(
                server["scopes"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("design:content:write"))
            );
            assert!(server.get("headers").is_none());
            assert!(server.get("env_headers").is_none());
            assert!(server.get("bearer_token_env_var").is_none());
            assert_eq!(
                mgr.oauth_remote_server_name("canva-mcp").as_deref(),
                Some("canva_mcp")
            );
            assert_eq!(
                mgr.model_tool_names(&["canva-mcp".to_string()]),
                vec!["mcp_canva_mcp_*".to_string()]
            );
        });
    }

    #[test]
    fn install_qcc_oauth_server_writes_deepseek_oauth_config() {
        with_temp_home(|| {
            write_tool_manifest(
                "qcc",
                r#"{
                    "id":"qcc","name":"企查查","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "config_fields":[],
                    "servers":[
                        {
                            "name":"qcc-company",
                            "url":"https://agent.qcc.com/mcp/company/stream",
                            "scopes":["mcp:tools"],
                            "oauth_resource":"https://agent.qcc.com/mcp/company/stream"
                        }
                    ]
                }"#,
            );

            let mgr = MarketplaceManager::new();
            mgr.install("qcc", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            let server = &mcp["servers"]["qcc-company"];
            assert_eq!(server["url"], "https://agent.qcc.com/mcp/company/stream");
            assert_eq!(server["scopes"], serde_json::json!(["mcp:tools"]));
            assert_eq!(
                server["oauth_resource"],
                "https://agent.qcc.com/mcp/company/stream"
            );
            assert!(server.get("headers").is_none());
            assert!(server.get("bearer_token_env_var").is_none());
            assert_eq!(
                mgr.oauth_remote_server_name("qcc").as_deref(),
                Some("qcc-company")
            );
        });
    }

    /// 全局禁用列表落盘往返:存→读一致;清空→读空;没文件→读空。
    #[test]
    fn disabled_connectors_persist_roundtrip() {
        with_temp_home(|| {
            assert!(load_disabled_connectors().is_empty()); // 无文件 → 空
            save_disabled_connectors(&["weather".to_string(), "pptx".to_string()]);
            assert_eq!(
                load_disabled_connectors(),
                vec!["weather".to_string(), "pptx".to_string()]
            );
            save_disabled_connectors(&[]); // 全开回去
            assert!(load_disabled_connectors().is_empty());
        });
    }

    fn write_installed_ids(ids: &[String]) {
        let path = crate::platform::paths::pinvou3_home()
            .join("marketplace")
            .join("installed.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string(ids).unwrap()).unwrap();
    }

    #[test]
    fn disabled_connectors_scope_isolation() {
        with_temp_home(|| {
            // 模拟已装 2 个连接器。
            write_installed_ids(&["weather".to_string(), "pptx".to_string()]);
            // 未初始化:code 默认全禁——已装连接器 ∪ 内置 CLI 四连接器 ∪ 已装技能包
            // （scope.rs DenyAll 扩集是有意语义）;plain 仍按空处理。
            let deny_all_default = || {
                vec![
                    "weather".to_string(),
                    "pptx".to_string(),
                    "feishu".to_string(),
                    "wecom".to_string(),
                    "dingtalk".to_string(),
                    "tmeet".to_string(),
                ]
            };
            assert!(load_disabled_connectors_for(ConnectorScope::Plain).is_empty());
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Code),
                deny_all_default()
            );
            // plain 写 weather → code 不受影响(仍默认全禁)。
            save_disabled_connectors_for(ConnectorScope::Plain, &["weather".to_string()]);
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Code),
                deny_all_default()
            );
            // code 显式写 → 标记初始化,此后以落盘为准。
            save_disabled_connectors_for(ConnectorScope::Code, &["pptx".to_string()]);
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Code),
                vec!["pptx".to_string()]
            );
            // plain 再写空,不影响 code。
            save_disabled_connectors_for(ConnectorScope::Plain, &[]);
            assert!(load_disabled_connectors_for(ConnectorScope::Plain).is_empty());
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Code),
                vec!["pptx".to_string()]
            );
        });
    }

    #[test]
    fn disabled_connectors_legacy_array_migrates_to_plain() {
        with_temp_home(|| {
            write_installed_ids(&["weather".to_string(), "pptx".to_string()]);
            let path = crate::platform::paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, r#"["weather","pptx"]"#).unwrap();
            assert_eq!(
                load_disabled_connectors(),
                vec!["weather".to_string(), "pptx".to_string()]
            );
            // 旧格式不初始化 code scope → 仍默认全禁（已装连接器 ∪ 内置 CLI 四连接器，
            // DenyAll 扩集是有意语义）。
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Code),
                vec![
                    "weather".to_string(),
                    "pptx".to_string(),
                    "feishu".to_string(),
                    "wecom".to_string(),
                    "dingtalk".to_string(),
                    "tmeet".to_string(),
                ]
            );
            // 读到即迁移：迁移结果落盘到单一真相源 `disabled_bundles.json`（旧文件不回写）。
            let file = crate::features::marketplace::scope::load_disabled_bundles_file();
            assert_eq!(
                file.scopes.get("plain"),
                Some(&vec!["weather".to_string(), "pptx".to_string()]),
                "迁移后应写入 disabled_bundles.json 的 plain scope: {file:?}"
            );
        });
    }

    /// 旧双 scope 对象 `{plain, code, code_initialized}` 迁移为 scopes map:
    /// 迁移前后行为一致(code_initialized=true → 以落盘为准;false → 默认全禁)。
    #[test]
    fn disabled_connectors_legacy_object_migrates_to_scopes_map() {
        with_temp_home(|| {
            write_installed_ids(&["weather".to_string(), "pptx".to_string()]);
            let path = crate::platform::paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                r#"{"plain":["weather"],"code":["pptx"],"code_initialized":true}"#,
            )
            .unwrap();
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Code),
                vec!["pptx".to_string()]
            );
            let file = crate::features::marketplace::scope::load_disabled_bundles_file();
            assert!(file.initialized.contains("code"));
            // 落盘已是单一真相源 `disabled_bundles.json` 的新格式（旧文件不回写）。
            assert_eq!(
                file.scopes.get("plain"),
                Some(&vec!["weather".to_string()]),
                "plain scope 应迁入 disabled_bundles.json: {file:?}"
            );
            assert_eq!(
                file.scopes.get("code"),
                Some(&vec!["pptx".to_string()]),
                "code scope 应迁入 disabled_bundles.json: {file:?}"
            );
        });
    }

    /// 旧对象 `code_initialized=false` 时,code 数组被忽略、按 DenyAll 默认全禁
    /// (与迁移前逐字节一致);plain 列表即使无 initialized 标记也必须生效
    /// (AllowAll 无兜底,落盘即真相)。
    #[test]
    fn legacy_object_uninitialized_code_keeps_deny_all_default() {
        with_temp_home(|| {
            write_installed_ids(&["weather".to_string(), "pptx".to_string()]);
            let path = crate::platform::paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                r#"{"plain":["weather"],"code":[],"code_initialized":false}"#,
            )
            .unwrap();
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Code),
                vec![
                    "weather".to_string(),
                    "pptx".to_string(),
                    "feishu".to_string(),
                    "wecom".to_string(),
                    "dingtalk".to_string(),
                    "tmeet".to_string(),
                ],
                "code 未初始化应按 DenyAll 默认全禁（已装连接器 ∪ 内置 CLI 四连接器）"
            );
        });
    }

    /// 新格式文件里的未知键经读-改-写后保留(前向兼容:新版字段不被旧版丢弃)。
    #[test]
    fn unknown_keys_survive_roundtrip() {
        with_temp_home(|| {
            let path = crate::platform::paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                r#"{"scopes":{"plain":["weather"]},"initialized":["plain"],"future_field":{"v":1}}"#,
            )
            .unwrap();
            save_disabled_connectors_for(ConnectorScope::Plain, &["pptx".to_string()]);
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(
                content.contains("future_field"),
                "未知键应在读写后保留: {content}"
            );
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Plain),
                vec!["pptx".to_string()]
            );
        });
    }

    #[test]
    fn sync_deny_all_scopes_after_install_keeps_new_connector_disabled_by_default() {
        with_temp_home(|| {
            write_installed_ids(&["pptx".to_string()]);
            // 未初始化 → 不落盘,文件保持无/空。
            sync_deny_all_scopes_after_install("weather");
            assert!(
                crate::features::marketplace::scope::load_disabled_bundles_file()
                    .scopes
                    .get("code")
                    .map(|ids| ids.is_empty())
                    .unwrap_or(true)
            );
            // 初始化 code 后(显式开掉 pptx),新装 weather → 自动进 code 禁用集。
            save_disabled_connectors_for(ConnectorScope::Code, &[]);
            sync_deny_all_scopes_after_install("weather");
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Code),
                vec!["weather".to_string()]
            );
            // 已存在不重复。
            sync_deny_all_scopes_after_install("weather");
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Code),
                vec!["weather".to_string()]
            );
        });
    }

    #[test]
    fn remove_connector_cleans_both_scopes() {
        with_temp_home(|| {
            save_disabled_connectors_for(
                ConnectorScope::Plain,
                &["weather".to_string(), "pptx".to_string()],
            );
            save_disabled_connectors_for(ConnectorScope::Code, &["weather".to_string()]);
            remove_connector_from_disabled_scopes("weather");
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Plain),
                vec!["pptx".to_string()]
            );
            assert!(load_disabled_connectors_for(ConnectorScope::Code).is_empty());
        });
    }

    /// 两个 scope 并发写同一文件:进程内串行化 + 原子写保证不丢更新、不撕裂——
    /// 结束后两边最后一次写入都必须还在,且文件始终是合法 JSON。
    #[test]
    fn concurrent_scope_writes_do_not_lose_updates() {
        with_temp_home(|| {
            let plain_writer = std::thread::spawn(|| {
                for _ in 0..50 {
                    save_disabled_connectors_for(ConnectorScope::Plain, &["weather".to_string()]);
                }
            });
            let code_writer = std::thread::spawn(|| {
                for _ in 0..50 {
                    save_disabled_connectors_for(ConnectorScope::Code, &["pptx".to_string()]);
                }
            });
            plain_writer.join().unwrap();
            code_writer.join().unwrap();

            let file = crate::features::marketplace::scope::load_disabled_bundles_file();
            assert_eq!(file.scopes.get("plain"), Some(&vec!["weather".to_string()]));
            assert!(file.initialized.contains("code"));
            assert_eq!(file.scopes.get("code"), Some(&vec!["pptx".to_string()]));
        });
    }

    /// 连接器禁用联动技能、独立 skill 开关、同名不误伤三个场景的组合目录断言
    /// 已随 `enabled_skills_for` 移入 `assistant::skill_materialization` 的测试
    /// （marketplace → assistant 会构成 feature 依赖环，架构守卫拒绝）。
    /// 覆盖位置：`skill_materialization.rs` tests 中
    /// `companion_skill_excluded_when_connector_disabled` /
    /// `enabled_skills_respect_first_wins_and_scope_disabled` /
    /// `disabling_connector_id_does_not_hide_same_named_user_skill`。

    #[test]
    fn secret_manifest_parses_declarations_without_plain_secret_values() {
        let weather: ToolManifest = serde_json::from_str(
            r#"{
                "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}]
            }"#,
        )
        .unwrap();
        assert!(weather.env.is_empty());
        assert_eq!(weather.secret_env[0].key, "AMAP_KEY");

        let qcc: ToolManifest = serde_json::from_str(
            r#"{
                "id":"qcc","name":"QCC","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"QCC_API_KEY","provider":"qcc","required":true}],
                "servers":[{"name":"qcc-company","url":"https://example.invalid/mcp"}]
            }"#,
        )
        .unwrap();
        assert!(qcc.env.is_empty());
        assert_eq!(qcc.secret_headers[0].source_key, "QCC_API_KEY");
    }

    #[test]
    fn install_local_secret_env_writes_placeholder_without_plain_secret() {
        with_temp_home(|| {
            write_tool_manifest(
                "weather",
                r#"{
                    "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("amap");
            store
                .set(&mcp_secret_reference("weather", "env", "AMAP_KEY"), &secret)
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);

            mgr.install("weather", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            let amap = mcp["servers"]["weather"]["env"]["AMAP_KEY"]
                .as_str()
                .unwrap();
            assert_eq!(amap, "${PINVOU3_MCP_SECRET_AMAP_KEY}");
            assert!(!mcp.to_string().contains(&secret));
        });
    }

    #[test]
    fn install_patsnap_secret_header_uses_bearer_env_without_plain_secret() {
        with_temp_home(|| {
            write_tool_manifest(
                "patsnap-search",
                r#"{
                    "id":"patsnap-search","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"PATSNAP_API_KEY","provider":"patsnap","required":true}],
                    "servers":[{"name":"patsnap-search","url":"https://connect.zhihuiya.com/2b0355/logic-mcp"}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("patsnap");
            store
                .set(
                    &mcp_secret_reference("patsnap-search", "header", "PATSNAP_API_KEY"),
                    &secret,
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);

            mgr.install("patsnap-search", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            let url = mcp["servers"]["patsnap-search"]["url"].as_str().unwrap();
            assert_eq!(url, "https://connect.zhihuiya.com/2b0355/logic-mcp");
            assert!(mcp["servers"]["patsnap-search"].get("headers").is_none());
            assert_eq!(
                mcp["servers"]["patsnap-search"]["bearer_token_env_var"],
                "PINVOU3_MCP_SECRET_PATSNAP_API_KEY"
            );
            assert!(!mcp.to_string().contains(&secret));
        });
    }

    /// 腾讯文档官方端点要求原始 Token(无 Bearer 前缀),四个远程 server 共用同一
    /// secret_headers 声明:每个 server 都落 `env_headers.Authorization` 指向同一
    /// 环境变量,不写 bearer_token_env_var(那会强制加 Bearer 前缀),也不落明文。
    /// fixture 含 config_fields(target=bearer)与实发 manifest 的历史形状一致:
    /// UI 首装会经 user_config 传入 Token,该通道不得与 secret_headers 双写
    /// Authorization(否则 bearer_token_env_var 与 env_headers 并存,自相矛盾)。
    #[test]
    fn install_tencent_docs_raw_authorization_env_headers_on_all_servers() {
        with_temp_home(|| {
            write_tool_manifest(
                "tencent-docs",
                r#"{
                    "id":"tencent-docs","name":"腾讯文档","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"","source_key":"TENCENT_DOCS_TOKEN","provider":"tencent-docs","required":true}],
                    "config_fields":[{"key":"TENCENT_DOCS_TOKEN","label":"腾讯文档 Token","required":true,"target":"bearer","secret":true}],
                    "servers":[
                        {"name":"tencent-docs","url":"https://docs.qq.com/openapi/mcp"},
                        {"name":"tdoc-slide","url":"https://docs.qq.com/api/v6/slide/mcp"},
                        {"name":"tdoc-doc","url":"https://docs.qq.com/api/v6/doc/mcp"},
                        {"name":"tdoc-sheet","url":"https://docs.qq.com/api/v6/sheet/mcp"}
                    ]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("tdoc");
            store
                .set(
                    &mcp_secret_reference("tencent-docs", "header", "TENCENT_DOCS_TOKEN"),
                    &secret,
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);

            // 真实 UI 路径:配置弹窗把 Token 经 user_config 传入 install。
            let mut user_config = std::collections::HashMap::new();
            user_config.insert("TENCENT_DOCS_TOKEN".to_string(), secret.clone());
            mgr.install("tencent-docs", &user_config).unwrap();

            let mcp = read_mcp_json();
            for server in ["tencent-docs", "tdoc-slide", "tdoc-doc", "tdoc-sheet"] {
                let entry = &mcp["servers"][server];
                assert!(
                    entry.get("url").is_some(),
                    "server {server} 应写入 mcp.json"
                );
                assert!(
                    entry.get("bearer_token_env_var").is_none(),
                    "server {server} 不应走 Bearer 前缀注入"
                );
                assert_eq!(
                    entry["env_headers"]["Authorization"], "PINVOU3_MCP_SECRET_TENCENT_DOCS_TOKEN",
                    "server {server} 应以原始值 env_headers 注入 Authorization"
                );
            }
            assert!(mcp["servers"]["tencent-docs"].get("headers").is_none());
            assert!(!mcp.to_string().contains(&secret));

            // 卸载:四个 server 一并从 mcp.json 移除,凭据删除。
            mgr.uninstall("tencent-docs").unwrap();
            let mcp = read_mcp_json();
            for server in ["tencent-docs", "tdoc-slide", "tdoc-doc", "tdoc-sheet"] {
                assert!(mcp["servers"].get(server).is_none());
            }
        });
    }

    /// config_fields(target=bearer)与 secret_headers(scheme=Bearer)语义一致的
    /// 成对声明(如 patsnap-search)不受同 key 跳过影响:两通道收敛到同一
    /// bearer_token_env_var,不落 env_headers。
    #[test]
    fn install_bearer_config_field_kept_when_secret_header_scheme_matches() {
        with_temp_home(|| {
            write_tool_manifest(
                "pair-bearer",
                r#"{
                    "id":"pair-bearer","name":"Pair","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"PAIR_API_KEY","provider":"pair","required":true}],
                    "config_fields":[{"key":"PAIR_API_KEY","label":"Key","required":true,"target":"bearer","secret":true}],
                    "servers":[{"name":"pair","url":"https://pair.example.com/mcp"}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("pair");
            store
                .set(
                    &mcp_secret_reference("pair-bearer", "header", "PAIR_API_KEY"),
                    &secret,
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);

            let mut user_config = std::collections::HashMap::new();
            user_config.insert("PAIR_API_KEY".to_string(), secret.clone());
            mgr.install("pair-bearer", &user_config).unwrap();

            let entry = &read_mcp_json()["servers"]["pair"];
            assert_eq!(
                entry["bearer_token_env_var"], "PINVOU3_MCP_SECRET_PAIR_API_KEY",
                "成对 Bearer 声明应收敛到 bearer_token_env_var"
            );
            assert!(
                entry.get("env_headers").is_none(),
                "成对 Bearer 声明不应落 env_headers"
            );
            assert!(!entry.to_string().contains(&secret));
        });
    }

    #[test]
    fn sync_secret_values_restores_header_secret() {
        with_temp_home(|| {
            write_tool_manifest(
                "patsnap-search",
                r#"{
                    "id":"patsnap-search","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"PATSNAP_API_KEY","provider":"patsnap","required":true}],
                    "servers":[{"name":"patsnap-search","url":"https://connect.zhihuiya.com/2b0355/logic-mcp"}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("patsnap-sync");
            store
                .set(
                    &mcp_secret_reference("patsnap-search", "header", "PATSNAP_API_KEY"),
                    &secret,
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);
            mgr.save_installed(&["patsnap-search".to_string()]).unwrap();

            mgr.sync_secret_values().unwrap();

            // The secret lands in the in-process registry (the foundation
            // resolver reads it on demand)…
            assert_eq!(
                secrets::resolve_registered_secret("PINVOU3_MCP_SECRET_PATSNAP_API_KEY").as_deref(),
                Some(secret.as_str())
            );
            // …and the process env is no longer written (edition 2024 runtime
            // env writes are eliminated).
            assert!(
                std::env::var("PINVOU3_MCP_SECRET_PATSNAP_API_KEY").is_err(),
                "the secret must not be written to the process environment"
            );
        });
    }

    #[test]
    fn uninstall_patsnap_does_not_remove_other_connector_secrets() {
        // Also covers the single-connector uninstall contract (originally
        // uninstall_remote_secret_header_removes_credential_and_env):
        // uninstalling must delete the server entry, the credential
        // reference, and the registry value; and additionally asserts that
        // another connector's (qcc) credentials and registry values are
        // unaffected.
        with_temp_home(|| {
            write_tool_manifest(
                "patsnap-search",
                r#"{
                    "id":"patsnap-search","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"PATSNAP_API_KEY","provider":"patsnap","required":true}],
                    "servers":[{"name":"patsnap-search","url":"https://connect.zhihuiya.com/2b0355/logic-mcp"}]
                }"#,
            );
            write_tool_manifest(
                "qcc",
                r#"{
                    "id":"qcc","name":"QCC","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"QCC_API_KEY","provider":"qcc","required":true}],
                    "servers":[{"name":"qcc-company","url":"https://example.invalid/mcp"}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let patsnap_secret = secret_value("patsnap-isolated");
            let qcc_secret = secret_value("qcc-isolated");
            let patsnap_ref = mcp_secret_reference("patsnap-search", "header", "PATSNAP_API_KEY");
            let qcc_ref = mcp_secret_reference("qcc", "header", "QCC_API_KEY");
            store.set(&patsnap_ref, &patsnap_secret).unwrap();
            store.set(&qcc_ref, &qcc_secret).unwrap();
            let mgr = MarketplaceManager::with_store(store.clone());

            mgr.install("patsnap-search", &std::collections::HashMap::new())
                .unwrap();
            mgr.install("qcc", &std::collections::HashMap::new())
                .unwrap();
            assert_eq!(
                secrets::resolve_registered_secret("PINVOU3_MCP_SECRET_QCC_API_KEY").as_deref(),
                Some(qcc_secret.as_str())
            );

            mgr.uninstall("patsnap-search").unwrap();

            let mcp = read_mcp_json();
            assert!(mcp["servers"].get("patsnap-search").is_none());
            assert!(mcp["servers"].get("qcc-company").is_some());
            assert_eq!(store.get(&patsnap_ref).unwrap(), None);
            assert!(
                secrets::resolve_registered_secret("PINVOU3_MCP_SECRET_PATSNAP_API_KEY").is_none()
            );
            assert_eq!(store.get(&qcc_ref).unwrap(), Some(qcc_secret.clone()));
            assert_eq!(
                secrets::resolve_registered_secret("PINVOU3_MCP_SECRET_QCC_API_KEY").as_deref(),
                Some(qcc_secret.as_str())
            );
        });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn validate_patsnap_with_invalid_token_can_be_rolled_back() {
        with_temp_home_async(|| async {
            let mock = spawn_mock_mcp_server("valid-token").await;
            write_tool_manifest(
                "patsnap-search",
                &format!(
                    r#"{{
                    "id":"patsnap-search","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["patsnap_search","patsnap_fetch"],"command":"","args":[],
                    "validate_on_install":true,
                    "secret_headers":[{{"header":"Authorization","scheme":"Bearer","source_key":"PATSNAP_API_KEY","provider":"patsnap","required":true}}],
                    "servers":[{{"name":"patsnap-search","url":"{}"}}]
                }}"#,
                    mock.url
                ),
            );
            let store = MemoryCredentialStore::default();
            let mgr = MarketplaceManager::with_store(store.clone());
            let mut config = std::collections::HashMap::new();
            config.insert("PATSNAP_API_KEY".to_string(), "wrong-token".to_string());

            mgr.install("patsnap-search", &config).unwrap();
            let err = mgr
                .validate_remote_connection("patsnap-search")
                .await
                .unwrap_err();
            mgr.uninstall("patsnap-search").unwrap();

            assert!(err.contains("API Key 无效"), "unexpected error: {err}");
            assert!(!err.contains("无法连接远程 MCP 服务"));
            assert!(!mgr.installed_ids().contains(&"patsnap-search".to_string()));
            let mcp = read_mcp_json();
            assert!(mcp["servers"].get("patsnap-search").is_none());
            assert_eq!(store
                .get(&mcp_secret_reference(
                    "patsnap-search",
                    "header",
                    "PATSNAP_API_KEY"
                ))
                .unwrap(), None);
            assert!(secrets::resolve_registered_secret("PINVOU3_MCP_SECRET_PATSNAP_API_KEY").is_none());
            assert!(!err.contains("wrong-token"));
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn validate_patsnap_with_valid_token_discovers_expected_tools() {
        with_temp_home_async(|| async {
            let mock = spawn_mock_mcp_server("valid-token").await;
            write_tool_manifest(
                "patsnap-search",
                &format!(
                    r#"{{
                    "id":"patsnap-search","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["patsnap_search","patsnap_fetch"],"command":"","args":[],
                    "validate_on_install":true,
                    "secret_headers":[{{"header":"Authorization","scheme":"Bearer","source_key":"PATSNAP_API_KEY","provider":"patsnap","required":true}}],
                    "servers":[{{"name":"patsnap-search","url":"{}"}}]
                }}"#,
                    mock.url
                ),
            );
            let store = MemoryCredentialStore::default();
            let mgr = MarketplaceManager::with_store(store.clone());
            let mut config = std::collections::HashMap::new();
            config.insert("PATSNAP_API_KEY".to_string(), "valid-token".to_string());

            mgr.install("patsnap-search", &config).unwrap();
            let validation = mgr
                .validate_remote_connection("patsnap-search")
                .await
                .unwrap();

            assert!(mgr.installed_ids().contains(&"patsnap-search".to_string()));
            let mcp = read_mcp_json();
            assert_eq!(
                mcp["servers"]["patsnap-search"]["bearer_token_env_var"],
                "PINVOU3_MCP_SECRET_PATSNAP_API_KEY"
            );
            assert!(!mcp.to_string().contains("valid-token"));
            assert_eq!(
                store
                    .get(&mcp_secret_reference(
                        "patsnap-search",
                        "header",
                        "PATSNAP_API_KEY"
                    ))
                    .unwrap()
                    .as_deref(),
                Some("valid-token")
            );
            assert_eq!(
                secrets::resolve_registered_secret("PINVOU3_MCP_SECRET_PATSNAP_API_KEY").as_deref(),
                Some("valid-token")
            );
            assert!(validation.tools.contains(&"patsnap_search".to_string()));
            assert!(validation.tools.contains(&"patsnap_fetch".to_string()));
            let seen = mock.seen_methods.lock().unwrap().clone();
            assert!(seen.contains(&"initialize".to_string()));
            assert!(seen.contains(&"tools/list".to_string()));
        })
        .await;
    }

    #[test]
    fn weather_missing_user_key_fails_without_fallback() {
        // 同时覆盖半安装回归(原 install_failure_does_not_leave_half_installed_state):
        // 缺密钥导致失败时 installed.json 不该记录该工具(顺序=先写 mcp 成功、再
        // save_installed);且 mcp.json 不得残留。
        with_temp_home(|| {
            write_tool_manifest(
                "weather",
                r#"{
                    "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}]
                }"#,
            );
            let mgr = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let err = mgr
                .install("weather", &std::collections::HashMap::new())
                .unwrap_err();

            assert!(err.contains("AMAP_KEY"), "错误应提示缺少 AMAP_KEY: {err}");
            assert!(
                !mgr.installed_ids().contains(&"weather".to_string()),
                "缺用户 key 时不应写 installed.json"
            );
            assert!(
                !crate::bridge::paths::mcp_config_path().is_file(),
                "缺用户 key 时不应写入 mcp.json"
            );
        });
    }

    #[test]
    fn migrate_legacy_manifest_env_moves_secret_to_store_and_removes_plaintext() {
        with_temp_home(|| {
            let secret = secret_value("legacy-amap");
            // 旧布局明文 manifest 迁移：`migrate_mcp_plaintext_secrets` 只扫旧布局
            // `bundle/mcp-servers/<id>/`（迁移对象本来就是旧版残留），此处直接写旧布局。
            let dir = crate::platform::paths::bundle_mcp_servers_dir().join("weather");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("manifest.json"),
                format!(
                    r#"{{
                        "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                        "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                        "env":{{"AMAP_KEY":"{secret}","SAFE_VALUE":"kept"}}
                    }}"#
                ),
            )
            .unwrap();
            let store = MemoryCredentialStore::default();
            let mgr = MarketplaceManager::with_store(store.clone());

            let result = mgr.migrate_mcp_plaintext_secrets().unwrap();

            assert_eq!(result.migrated_count, 1);
            let stored = store
                .get(&mcp_secret_reference("weather", "env", "AMAP_KEY"))
                .unwrap();
            assert_eq!(stored.as_deref(), Some(secret.as_str()));
            let content = std::fs::read_to_string(
                crate::platform::paths::bundle_mcp_servers_dir()
                    .join("weather")
                    .join("manifest.json"),
            )
            .unwrap();
            assert!(!content.contains(&secret));
            assert!(content.contains("SAFE_VALUE"));
        });
    }

    #[test]
    fn migrate_legacy_qcc_bearer_header_uses_env_and_stores_secret() {
        with_temp_home(|| {
            let secret = secret_value("legacy-qcc");
            let mcp_path = crate::platform::paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            std::fs::write(
                &mcp_path,
                format!(
                    r#"{{
                        "servers": {{
                            "qcc-company": {{
                                "url": "https://example.invalid/mcp",
                                "headers": {{"Authorization": "Bearer {secret}"}}
                            }}
                        }}
                    }}"#
                ),
            )
            .unwrap();
            let store = MemoryCredentialStore::default();
            let mgr = MarketplaceManager::with_store(store.clone());

            let result = mgr.migrate_mcp_plaintext_secrets().unwrap();

            assert_eq!(result.migrated_count, 1);
            let stored = store
                .get(&mcp_secret_reference("qcc", "header", "QCC_API_KEY"))
                .unwrap();
            assert_eq!(stored.as_deref(), Some(secret.as_str()));
            let content = std::fs::read_to_string(&mcp_path).unwrap();
            assert!(!content.contains(&secret));
            assert!(
                content.contains("\"bearer_token_env_var\": \"PINVOU3_MCP_SECRET_QCC_API_KEY\"")
            );
        });
    }

    #[test]
    fn migration_does_not_overwrite_existing_credential_but_cleans_file() {
        with_temp_home(|| {
            let old_secret = secret_value("old-qcc");
            let kept_secret = secret_value("kept-qcc");
            let mcp_path = crate::platform::paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            std::fs::write(
                &mcp_path,
                format!(
                    r#"{{
                        "servers": {{
                            "qcc-company": {{
                                "url": "https://example.invalid/mcp",
                                "headers": {{"Authorization": "Bearer {old_secret}"}}
                            }}
                        }}
                    }}"#
                ),
            )
            .unwrap();
            let store = MemoryCredentialStore::default();
            store
                .set(
                    &mcp_secret_reference("qcc", "header", "QCC_API_KEY"),
                    &kept_secret,
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store.clone());

            let result = mgr.migrate_mcp_plaintext_secrets().unwrap();

            assert_eq!(result.skipped_count, 1);
            let stored = store
                .get(&mcp_secret_reference("qcc", "header", "QCC_API_KEY"))
                .unwrap();
            assert_eq!(stored.as_deref(), Some(kept_secret.as_str()));
            let content = std::fs::read_to_string(&mcp_path).unwrap();
            assert!(!content.contains(&old_secret));
            assert!(!content.contains(&kept_secret));
            assert!(
                content.contains("\"bearer_token_env_var\": \"PINVOU3_MCP_SECRET_QCC_API_KEY\"")
            );
        });
    }

    #[test]
    fn install_missing_required_secret_returns_recoverable_redacted_error() {
        with_temp_home(|| {
            write_tool_manifest(
                "iwencai-custom",
                r#"{
                    "id":"iwencai-custom","name":"Iwencai","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_iwencai_query"],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"IWENCAI_TEST_KEY","provider":"iwencai-test","required":true}]
                }"#,
            );
            let mgr = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let err = mgr
                .install("iwencai-custom", &std::collections::HashMap::new())
                .unwrap_err();

            assert!(err.contains("IWENCAI_TEST_KEY"));
            assert!(!err.contains("test-secret"));
        });
    }

    /// 回归（三轮评审）：Upload 来源的包卸载必须保留 `bundles/<id>/` 目录（用户唯一
    /// 副本）。来源查询必须先于 `store.remove` —— 此前先删登记再查来源恒 false，
    /// 上传包目录被误删（数据丢失）。
    #[test]
    fn uninstall_upload_source_keeps_package_dir() {
        with_temp_home(|| {
            write_tool_manifest(
                "up-tool",
                r#"{
                    "id":"up-tool","name":"Up","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"]
                }"#,
            );
            let mgr = MarketplaceManager::new();
            mgr.install("up-tool", &std::collections::HashMap::new())
                .unwrap();
            // install 镜像写的来源是 Preset；覆盖为 Upload，模拟统一上传管线的登记。
            store::BundleStore::new()
                .upsert(store::BundleRecord::installed_now(
                    "up-tool",
                    store::BundleSource::Upload("pkg.zip".to_string()),
                ))
                .unwrap();

            mgr.uninstall("up-tool").unwrap();

            assert!(
                !mgr.installed_ids().contains(&"up-tool".to_string()),
                "卸载应移除 installed.json 登记"
            );
            assert!(
                crate::platform::paths::bundles_root()
                    .join("up-tool")
                    .join("mcp")
                    .join("manifest.json")
                    .is_file(),
                "Upload 来源卸载后包目录应保留"
            );
        });
    }

    /// 回归（四轮评审 BLOCKER 1，完整数据丢失链）：上传 MCP 包 → 卸载（Upload
    /// 保护生效、目录保留）→ 卡片重现 → 用户经普通 install 重装 → 再卸载，
    /// 目录仍必须保留。修复分两层：install_upload 镜像写直接带 Upload 来源；
    /// 卸载清登记后重装无旧记录可依，install 按盘上 plugin.json 标记（仅统一
    /// 上传管线落盘，预置 release 只写 mcp/ 子目录）订正为 Upload。此前任一层
    /// 失守都会把 source 停在 Preset，再卸载时整目录被删（用户上传内容无其他
    /// 副本）。
    #[test]
    fn reinstall_upload_package_then_uninstall_still_keeps_dir() {
        with_temp_home(|| {
            write_tool_manifest(
                "up-re",
                r#"{
                    "id":"up-re","name":"Up","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"]
                }"#,
            );
            // 统一上传管线总会落盘 plugin.json（原声明或派生副本）
            std::fs::write(
                crate::platform::paths::bundles_root()
                    .join("up-re")
                    .join("plugin.json"),
                r#"{"manifest_version":1,"id":"up-re","name":"Up"}"#,
            )
            .unwrap();
            let mgr = MarketplaceManager::new();
            mgr.install_upload("up-re", store::BundleSource::Upload("pkg.zip".to_string()))
                .unwrap();
            assert_eq!(
                store::BundleStore::new()
                    .get("up-re")
                    .unwrap()
                    .unwrap()
                    .source,
                store::BundleSource::Upload("pkg.zip".to_string()),
                "上传供给的镜像写应直接带 Upload 来源"
            );

            // 卸载：Upload 保护生效，目录保留、登记清除
            mgr.uninstall("up-re").unwrap();
            let manifest_path = crate::platform::paths::bundles_root()
                .join("up-re")
                .join("mcp")
                .join("manifest.json");
            assert!(manifest_path.is_file(), "Upload 卸载后目录应保留");

            // 卡片重现 → 用户重装走普通 install（store 已无旧记录可保留）
            mgr.install("up-re", &std::collections::HashMap::new())
                .unwrap();
            assert!(
                matches!(
                    store::BundleStore::new()
                        .get("up-re")
                        .unwrap()
                        .unwrap()
                        .source,
                    store::BundleSource::Upload(_)
                ),
                "重装应按盘上 plugin.json 标记订正为 Upload"
            );

            mgr.uninstall("up-re").unwrap();
            assert!(manifest_path.is_file(), "Upload 包重装后再卸载仍应保留目录");
        });
    }

    /// 上传/导入路径禁用 pip 自动安装（供应链安全）：`install_upload` 遇非空
    /// `pip_dependencies` 不执行 pip install（若执行，不存在的包名会让安装失败），
    /// 只提示用户自行安装，供给照常完成。
    #[test]
    fn install_upload_skips_pip_auto_install() {
        with_temp_home(|| {
            write_tool_manifest(
                "up-pip",
                r#"{
                    "id":"up-pip","name":"Up","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"],
                    "pip_dependencies":["pinvou3-nonexistent-pkg-xyz"]
                }"#,
            );
            let mgr = MarketplaceManager::new();

            mgr.install_upload(
                "up-pip",
                store::BundleSource::Upload("up-pip.zip".to_string()),
            )
            .unwrap();

            let mcp = read_mcp_json();
            assert!(
                mcp["servers"].get("up-pip").is_some(),
                "跳过 pip 后供给应照常写入 mcp.json"
            );
            assert!(mgr.installed_ids().contains(&"up-pip".to_string()));
        });
    }

    /// 回归（六轮评审 R1）：手写自定义 MCP（migrate_custom_mcp_layout 从旧布局
    /// 强迁、无 plugin.json、install 镜像登记为 Preset）卸载必须保留
    /// `bundles/<id>/` 目录 —— 内嵌目录无对应 spec，删除即丢失用户唯一副本。
    #[test]
    fn uninstall_custom_migrated_mcp_keeps_package_dir() {
        with_temp_home(|| {
            write_tool_manifest(
                "custom-migrated",
                r#"{
                    "id":"custom-migrated","name":"Custom","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"]
                }"#,
            );
            let mgr = MarketplaceManager::new();
            mgr.install("custom-migrated", &std::collections::HashMap::new())
                .unwrap();
            assert_eq!(
                store::BundleStore::new()
                    .get("custom-migrated")
                    .unwrap()
                    .unwrap()
                    .source,
                store::BundleSource::Preset,
                "无 plugin.json 的自定义包镜像来源应为 Preset"
            );

            mgr.uninstall("custom-migrated").unwrap();

            assert!(
                !mgr.installed_ids().contains(&"custom-migrated".to_string()),
                "卸载应移除 installed.json 登记"
            );
            assert!(
                crate::platform::paths::bundles_root()
                    .join("custom-migrated")
                    .join("mcp")
                    .join("manifest.json")
                    .is_file(),
                "自定义迁移 MCP 卸载后包目录应保留"
            );
        });
    }

    /// 回归（六轮评审 R1）：bundles.json 里无记录（旧版本安装/登记丢失）的自定
    /// 义包同样不可删目录 —— 无记录不等于可重释放。
    #[test]
    fn uninstall_custom_mcp_without_record_keeps_package_dir() {
        with_temp_home(|| {
            write_tool_manifest(
                "custom-norec",
                r#"{
                    "id":"custom-norec","name":"Custom","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"]
                }"#,
            );
            let mgr = MarketplaceManager::new();
            mgr.install("custom-norec", &std::collections::HashMap::new())
                .unwrap();
            // 模拟登记丢失：卸载前删掉 bundles.json 记录。
            store::BundleStore::new().remove("custom-norec").unwrap();

            mgr.uninstall("custom-norec").unwrap();

            assert!(
                crate::platform::paths::bundles_root()
                    .join("custom-norec")
                    .join("mcp")
                    .join("manifest.json")
                    .is_file(),
                "无记录的自定义包卸载后目录应保留"
            );
        });
    }

    /// 回归（六轮评审 R1）：内嵌预置包（内嵌目录有 spec，启动/重装可重释放）
    /// 卸载仍删目录 —— 新判定只收窄删除面，不放松预置包清理。
    #[test]
    fn uninstall_embedded_preset_removes_package_dir() {
        with_temp_home(|| {
            let mgr = MarketplaceManager::new();
            mgr.install("obsidian", &std::collections::HashMap::new())
                .unwrap();
            let pkg_dir = crate::platform::paths::bundles_root().join("obsidian");
            assert!(
                pkg_dir.join("mcp").join("manifest.json").is_file(),
                "内嵌预置安装后应释放 mcp/ 目录"
            );

            mgr.uninstall("obsidian").unwrap();

            assert!(!pkg_dir.exists(), "内嵌预置卸载后包目录应删除");
        });
    }

    /// 回归（六轮评审 R1 fail-closed）：bundles.json 损坏读不出时，卸载不得按
    /// 「非 Upload」照删目录 —— 读失败视为可能 Upload，保留目录。
    #[test]
    fn uninstall_with_corrupted_bundles_json_keeps_package_dir() {
        with_temp_home(|| {
            let mgr = MarketplaceManager::new();
            mgr.install("obsidian", &std::collections::HashMap::new())
                .unwrap();
            let pkg_dir = crate::platform::paths::bundles_root().join("obsidian");
            assert!(pkg_dir.join("mcp").join("manifest.json").is_file());
            // 损坏 bundles.json（load_locked 解析失败 fail loud）。
            std::fs::write(
                crate::platform::paths::pinvou3_home()
                    .join("marketplace")
                    .join("bundles.json"),
                b"{ not json",
            )
            .unwrap();

            mgr.uninstall("obsidian").unwrap();

            assert!(
                pkg_dir.join("mcp").join("manifest.json").is_file(),
                "bundles.json 读失败时卸载应保留目录（fail-closed）"
            );
        });
    }
}
