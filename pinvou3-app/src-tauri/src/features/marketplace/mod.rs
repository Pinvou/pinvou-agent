//! 工具市场管理器 — 管理 MCP 工具的安装/卸载/状态查询。
//!
//! 每个工具是一个 MCP server，元数据定义在 `manifest.json`。
//! 安装状态持久化在 `~/.pinvou3/marketplace/installed.json`。
//! 安装/卸载时同步修改 `~/.pinvou3/bundle/mcp.json`。
//!
//! 本模块是 facade:把原本 2600+ 行的 god-module 按职责拆成子模块,
//! 对外 pub 面通过 `pub use` 保持不变。
//!
//! - `types`      — data types such as manifest/info
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

pub(crate) use connectors::{mcp_json_lock, mcp_json_unparseable, write_json_pretty};

// PR #302 WIP 拆分的子模块（main 的 Wave 2 没有接这块）—— 需要补 mod 声明。
pub mod actions;
pub mod builtin;
pub mod bundle;
pub mod mcp_catalog;
pub mod package_export;
pub mod plugin_import;
pub mod recycle_bin;
pub mod scope;
pub mod skill_marketplace;
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

/// mcp.json server keys owned by the engine boot path (`runtime_bundle`'s
/// `ensure_builtin_mcp_servers` upserts `pinvou3`, removes the legacy `pinvou` key and
/// historical browser-wrapper residue). Startup reconciliation must never read, repair,
/// or overwrite them; the reconcile enforces this at two levels (installed-tool ids and
/// per-remote-server names). Marketplace installs have no symmetric guard — a custom
/// tool id colliding with these keys is rejected by nothing — so "never write" holds by
/// registry convention, not by enforcement. Keep in sync with
/// `ensure_builtin_mcp_servers`: the lockstep is enforced by two tests that cannot
/// both be satisfied while the set and the engine's real write footprint disagree —
/// the engine-side footprint is pinned by
/// `ensure_builtin_mcp_servers_touches_only_engine_owned_keys` in runtime_bundle/platform
/// against *this constant* (an engine-side key added without updating it turns red),
/// and this exact set is pinned on the marketplace side by
/// `engine_owned_mcp_server_keys_are_exactly_the_engine_footprint`. (ensure_builtin
/// also runs `refresh_mcp_python_commands`, which may rewrite the python command of
/// any entry; that self-heal is complementary and outside this key-set footprint.)
pub(crate) const ENGINE_OWNED_MCP_SERVER_KEYS: &[&str] = &["pinvou3", "pinvou", "browser"];

/// Back up a corrupt JSON config file next to itself as `<stem>.<epoch>`,
/// skipping the write when an existing backup already holds identical bytes —
/// a persistent parse failure must never mint one copy per boot. Shared by
/// `mcp.json` (reconcile loaders) and `installed.json` (registry recovery).
///
/// The backup goes through the platform's private atomic write, not bare
/// `fs::write`: a corrupt mcp.json may still hold pre-migration plaintext
/// credentials (a file that fails to parse is exactly one the plaintext
/// migration never reached), so the copy is created owner-only 0600 directly
/// (no umask window; that bit-level mode is the POSIX expression — Windows
/// delegates the same privacy to the per-user profile ACL) — bare
/// `fs::write` lands 0644 per umask, more exposed than the live file. The
/// copies are never garbage-collected (no reader, no cleaner anywhere) —
/// removal is a manual decision, which is also why they must stay private.
pub(super) fn backup_corrupt_json_file(path: &std::path::Path, stem: &str, content: &str) {
    let Some(parent) = path.parent() else {
        return;
    };
    let prefix = format!("{stem}.");
    if let Ok(entries) = std::fs::read_dir(parent) {
        let identical_backup_exists = entries.flatten().any(|entry| {
            entry.file_name().to_string_lossy().starts_with(&prefix)
                && std::fs::read(entry.path()).is_ok_and(|bytes| bytes == content.as_bytes())
        });
        if identical_backup_exists {
            return;
        }
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}-{:09}", d.as_secs(), d.subsec_nanos()))
        .unwrap_or_else(|_| "0-000000000".to_string());
    // 秒级时间戳会让同一秒内出现的第二份不同内容坏文件覆盖第一份备份；纳秒
    // 后缀保证每次真实落盘都是新名字。幂等仍由上面的全前缀同字节去重保证：
    // 持续解析失败不会每次启动都翻倍备份。
    let backup = parent.join(format!("{prefix}{ts}"));
    if let Err(e) = crate::platform::filesystem::atomic_write_private(&backup, content.as_bytes()) {
        log::warn!(
            "[marketplace] failed to backup corrupt {stem} to {}: {e}",
            backup.display()
        );
    }
}

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

/// Arms `FAIL_NEXT_INSTALLED_WRITE` for the calling test and always disarms it on
/// drop — including panics. The flag is process-global and its only consumer is
/// `save_installed`, so a test that fails before reaching that call (or panics
/// while armed) would otherwise leak the injected failure into the next install
/// running in the same process (issue #528).
#[cfg(test)]
struct FailNextInstalledWriteGuard;

#[cfg(test)]
impl Drop for FailNextInstalledWriteGuard {
    fn drop(&mut self) {
        FAIL_NEXT_INSTALLED_WRITE.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
fn fail_next_installed_write_for_test() -> FailNextInstalledWriteGuard {
    FAIL_NEXT_INSTALLED_WRITE.store(true, std::sync::atomic::Ordering::SeqCst);
    FailNextInstalledWriteGuard
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

/// Windows 上商店路径（Preset 来源）安装的未受信依赖声明必须 fail-closed：
/// 这些声明没有可验证依赖锁、也绝不自动执行，静默跳过会装出缺依赖的坏工具。
/// 触达者是无内嵌 spec、但被 `available_tools` 列进商店的自定义/迁移磁盘工具
/// （内嵌预置的缺锁场景早已由依赖锁校验与 pip 兜底闸门 fail-closed）。其余
/// 平台维持 warn-skip（按日志提示自行安装）。抽成纯函数以便在非 Windows
/// 开发机上直接回归测试。
fn untrusted_preset_deps_error(
    tool_id: &str,
    source: &store::BundleSource,
    windows: bool,
) -> Option<String> {
    if windows && matches!(source, store::BundleSource::Preset) {
        Some(format!(
            "工具 '{tool_id}' 的 Python 依赖未经过 Windows 可验证依赖锁，无法从商店重装"
        ))
    } else {
        None
    }
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

/// BundleSource → 前端契约字符串（`MarketplaceToolInfo.source`："upload" |
/// "preset" | "builtin"）。Unknown（前向兼容的未识别来源）归 preset：其卸载
/// 行为是保留目录（不匹配 Upload 回收分支），按非上传展示才不说谎（M4）。
fn tool_source_contract(source: &store::BundleSource) -> &'static str {
    match source {
        store::BundleSource::Upload(_) => "upload",
        store::BundleSource::Preset => "preset",
        store::BundleSource::Builtin => "builtin",
        store::BundleSource::Unknown(_) => "preset",
    }
}

// 对外 pub 面保持不变:类型从 types 子模块 re-export。
pub use types::{
    ConfigField, MarketplaceToolInfo, RemoteOAuthConfig, RemoteServer, SecretEnv, SecretHeader,
    ToolManifest,
};

// PR #302 拆分后,这些函数已搬到 scope.rs;为保持 mod.rs 兼容面,在此 re-export。
// scope.rs 统一落盘单一 `disabled_bundles.json`(取代 `disabled_connectors.json` /
// `disabled_skills.json` 双文件)。这里 re-export 保留调用路径;连接器/技能/CLI 开关
// 统一按包 id 落盘,`skill:` 前缀跨文件借道清除。
// `save_disabled_bundles`（plain 快捷写）仅测试在用，随实现一并 cfg(test)。
#[cfg(test)]
pub use crate::features::marketplace::scope::save_disabled_bundles;
pub use crate::features::marketplace::scope::{
    load_disabled_bundles, load_disabled_bundles_for, load_hidden_bundles_for,
    remove_bundle_from_disabled_scopes, save_disabled_bundles_for, save_hidden_bundles_for,
    sync_deny_all_scopes_after_install, unavailable_bundles_for,
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
    // Builtin plugins cannot be disabled (docs/builtin-toolset-contract.md
    // §3.3 defense in depth): the write fails loudly instead of silently
    // filtering the id out.
    builtin::reject_builtin_ids(&connector_ids)?;
    tokio::task::spawn_blocking(move || save_disabled_bundles_for(scope, &connector_ids))
        .await
        .map_err(|error| format!("apply_disabled_connectors_for join: {error}"))??;
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

/// Default-installed preset MCP tools: peripheral capabilities ship as
/// plugins so features like session mention work out of the box. Builtin
/// plugins cannot be uninstalled or disabled — the attempt is rejected
/// server-side (docs/builtin-toolset-contract.md §3.3) — so a missing
/// BundleStore record only ever means "not seeded yet".
pub const DEFAULT_INSTALLED_MCP_TOOLS: &[&str] = &["session-reader"];

/// 当前(plain)会话侧不可用包/native 工具 → 模型可见工具全名(喂给引擎
/// disallowed_tools 的)。
pub fn unavailable_tool_names() -> Vec<String> {
    unavailable_tool_names_for(ConnectorScope::Plain)
}

/// Native (non-MCP) model tools owned by a marketplace package, as
/// `(model-visible tool name, owning skill-marketplace id)`.
///
/// These tools are host-registered in every spawned session (e.g. the ima
/// connector's `ImaOpenApiTool` in engine_pool), so their names can never be
/// produced by `model_tool_names`, which only maps MCP manifests. Package
/// toggles must gate the tool itself, not just hide the skill text: after a
/// package is disabled or uninstalled, a still-admitted tool stays searchable
/// via `tool_search` and keeps using the locally retained credentials.
pub(crate) const NATIVE_PACKAGE_TOOLS: &[(&str, &str)] = &[("ima_openapi", "ima-skills")];

/// Unavailable native tool names for a scope: a native tool is denied whenever
/// its owning package is not installed, or the owning package is disabled or
/// hidden for that scope — the same availability rule the package's skills
/// follow (`unavailable_bundles_for` = disabled ∪ hidden;
/// `resolve_scope_disabled_ids` already counts installed skill owner packages
/// in the DenyAll fallback, so an uninitialized DenyAll scope denies without
/// any explicit toggle).
fn native_unavailable_tool_names_for(scope: ConnectorScope) -> Vec<String> {
    let installed = skill_marketplace::SkillMarketplaceManager::new().installed_skill_ids();
    let unavailable = unavailable_bundles_for(scope);
    NATIVE_PACKAGE_TOOLS
        .iter()
        .filter(|(_, package)| {
            let owner = bundle::skill_owner_package(package);
            !installed.iter().any(|id| id == package) || unavailable.iter().any(|id| id == &owner)
        })
        .map(|(tool, _)| (*tool).to_string())
        .collect()
}

/// 按会话类型 scope：会话侧不可用包/native 工具 → 模型可见工具全名(喂给引擎
/// disallowed_tools 的)。不可用 = 开关关(disabled) ∪ 不可见(hidden)，与技能
/// 物化/execpolicy（`unavailable_bundles_for`）及 turn 快照（enabled）同一
/// 口径：只按开关集会让「已装但被隐藏」的包的工具留在模型目录里，模型被两
/// 个互相矛盾的真相源同时喂养（PPT 场景实测）。
pub fn unavailable_tool_names_for(scope: ConnectorScope) -> Vec<String> {
    let mut names = MarketplaceManager::new().model_tool_names(&unavailable_bundles_for(scope));
    names.extend(native_unavailable_tool_names_for(scope));
    // Builtin feature switches (docs/builtin-toolset-contract.md §3.3): tools
    // removed by the union semantics are scope-agnostic, so they merge into
    // every scope's engine gate (deduped).
    for name in builtin::feature_disabled_tool_names() {
        if !names.iter().any(|n| n == &name) {
            names.push(name);
        }
    }
    names
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

    /// Frontend list: all available tools + install state. `installed` follows the same truth-source policy as `BundleRegistry::list`
    /// (bundle_readiness): bundles.json records win (a missing record =
    /// not installed), falling back to installed.json derivation when the store read fails (degraded info is lost with it)
    /// — previously only installed.json was read, diverging from the readiness card in the two states
    /// "store has an uninstalled record / store corrupted" (the tool card showed installed while the readiness card showed not installed). If an uploaded package has user
    /// display name/description overrides (bundles.json extra, only effective for Upload-source records),
    /// name/description use the overridden values — the same read as `BundleRegistry::list`
    /// (`store::apply_display_override`), so card titles and composer menu titles
    /// do not diverge. `source` is filled from the actual BundleSource of the bundles.json record (M4: the frontend
    /// uses it to distinguish "uploaded package uninstalls into the recycle bin" from "preset/custom uninstall keeps the directory").
    pub fn list_tools(&self) -> Vec<MarketplaceToolInfo> {
        let installed = self.installed_ids();
        // Read all records in one pass, serving install state, display overrides (only Upload records take effect), and source
        // filling at once, avoiding the N+1 of per-tool locking + whole-file parsing. On read failure (e.g. corrupt JSON), warn and
        // degrade: install state falls back to installed.json derivation (same policy as bundle.rs), display overrides
        // treated as "no override", source as "no record = builtin" (better to under-report "moved to the recycle bin"
        // than to lie).
        let store_records: Option<Vec<store::BundleRecord>> = match store::BundleStore::new()
            .records()
        {
            Ok(records) => Some(records),
            Err(e) => {
                log::warn!(
                    "[marketplace] failed to read BundleStore; list_tools installed falls back to installed.json, no display overrides, source as builtin: {e}"
                );
                None
            }
        };
        let records = store_records.as_deref().unwrap_or(&[]);
        let upload_by_id: std::collections::HashMap<&str, &store::BundleRecord> = records
            .iter()
            .filter(|r| matches!(r.source, store::BundleSource::Upload(_)))
            .map(|r| (r.id.as_str(), r))
            .collect();
        let source_by_id: std::collections::HashMap<&str, &str> = records
            .iter()
            .map(|r| (r.id.as_str(), tool_source_contract(&r.source)))
            .collect();
        self.available_tools()
            .into_iter()
            .map(|m| {
                // Install-state truth-source inversion (§3.2): store records win; on read failure fall back to
                // installed.json (the None-branch convention of store_state).
                let (installed_flag, _) = bundle::store_state(store_records.as_deref(), &m.id)
                    .unwrap_or_else(|| (installed.contains(&m.id), None));
                let (name, description) = match upload_by_id.get(m.id.as_str()) {
                    Some(record) => {
                        let (name, description) = store::apply_display_override(record, None, None);
                        (
                            name.unwrap_or_else(|| m.name.clone()),
                            description.unwrap_or_else(|| m.description.clone()),
                        )
                    }
                    None => (m.name, m.description),
                };
                MarketplaceToolInfo {
                    source: source_by_id
                        .get(m.id.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "builtin".to_string()),
                    // 预置目录包不可导出（zip 无法重新导入，与 export_installed_plugin
                    // 的 fail-fast 同口径）；迁移登记的手写自定义 MCP / 上传包可导出。
                    exportable: !mcp_catalog::spec_for(&m.id).is_some(),
                    installed: installed_flag,
                    // Builtin semantics (docs/builtin-toolset-contract.md
                    // §3.1) pass through only on builtin plugins: normal
                    // plugins keep empty values which serialize away, keeping
                    // the frontend contract clean. mcp_tools passes through
                    // in full (the builtin section lists a plugin's tools);
                    // visibility mirrors security_level; bundle_version marks
                    // the bundle version a builtin plugin ships with.
                    security_level: if m.builtin && !m.security_level.is_empty() {
                        Some(m.security_level.clone())
                    } else {
                        None
                    },
                    data_access: if m.builtin {
                        m.data_access.clone()
                    } else {
                        Vec::new()
                    },
                    mcp_tools: m.mcp_tools.clone(),
                    // visibility passthrough mirrors security_level: filled
                    // only for builtin plugins, omitted otherwise.
                    visibility: if m.builtin && !m.visibility.is_empty() {
                        Some(m.visibility.clone())
                    } else {
                        None
                    },
                    // bundle_version is filled at the command layer
                    // (commands::marketplace::list_marketplace_tools): a
                    // marketplace -> runtime_bundle dependency would be a
                    // feature cycle (architecture guard
                    // rust_cyclic_feature_dependencies baseline is 0).
                    bundle_version: None,
                    builtin: m.builtin,
                    id: m.id,
                    name,
                    description,
                    version: m.version,
                    companion_skills: m.companion_skills,
                }
            })
            .collect()
    }

    /// Boot seed: default-installed preset MCP tools go through the standard
    /// install pipeline when the BundleStore has no record for them (fresh
    /// installs and upgrades to the first version carrying the tool both land
    /// here); any existing record (installed / uninstalled / any source) is
    /// respected. Failures only log and never block startup.
    /// A MarketplaceManager method (not a free function): shares one manager
    /// instance with ensure_extracted so a seed install does not rerun the
    /// plaintext secret migration against a second credential store.
    pub fn ensure_default_installed_mcp_tools(&self) {
        let store = store::BundleStore::new();
        for id in DEFAULT_INSTALLED_MCP_TOOLS {
            match store.get(id) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    if let Err(e) = self.install(id, &std::collections::HashMap::new()) {
                        log::warn!("[marketplace] 默认安装 '{id}' 失败(不阻塞启动): {e}");
                    }
                }
                Err(e) => {
                    log::warn!("[marketplace] 读取 BundleStore 失败,跳过默认安装 '{id}': {e}")
                }
            }
        }
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
        // test seam: only install_with_python (cfg(test)) passes Some
        python_override: Option<&str>,
    ) -> Result<(), String> {
        let _transaction_guard = MARKETPLACE_TRANSACTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recover_marketplace_transaction()?;
        // A corrupt mcp.json must fail the install with the actionable refusal
        // (backup + fix-or-remove guidance, the same message every writer
        // refuses with) instead of the plaintext migration's bare parse error.
        // A healthy or absent file passes through untouched.
        connectors::load_mcp_json_for_reconcile()?;
        self.migrate_mcp_plaintext_secrets()?;
        // 内嵌目录工具的安装只能信任编译进应用的 manifest——磁盘副本可能来自旧
        // 版本或已被修改，不得改写安装期写入 mcp.json 的任何内容（含 command/
        // args 与 secret 声明）。无内嵌 spec 的上传/自定义包仍从自身包目录读取。
        let manifest = mcp_catalog::embedded_manifest(tool_id)?
            .or_else(|| self.load_manifest(tool_id))
            .ok_or_else(|| format!("工具 '{tool_id}' 不存在"))?;

        // Only the embedded catalog may authorize automatic dependency execution. For
        // embedded-catalog tools the compile-time manifest above is likewise the source of
        // everything written at install time; the on-disk manifest stays authoritative for
        // ordinary metadata only where no embedded snapshot exists, and its dependency
        // fields are never executed.
        let dependency_manifest = self
            .trusted_dependency_manifest(tool_id, Some(&source))
            .map_err(|error| error.message().to_string())?;
        let python_environment = if let Some(dependency_manifest) = dependency_manifest {
            // UI installs are explicit user actions and bypass the startup repair retry cooldown.
            self.install_python_deps(&dependency_manifest, python_override, false)
                .map_err(|error| error.message().to_string())?
        } else if !manifest.pip_dependencies.is_empty() || manifest.python_dependencies.is_some() {
            if let Some(error) = untrusted_preset_deps_error(
                tool_id,
                &source,
                crate::platform::capabilities::is_windows(),
            ) {
                return Err(error);
            }
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
        // 来源订正（四轮评审 BLOCKER 1）：Upload 包卸载后目录可能仍在盘上（旧版本
        // 「只清登记、保留目录」的存量残留），用户经普通 `install` 重装时 store 已无
        // 旧记录可依（既有记录的保护由 upsert_preserving 承担）。预置包 release 只写
        // mcp/ 子目录，`bundles/<id>/plugin.json` 仅由统一上传管线落盘 —— 据此把盘上
        // 上传包的来源订正为 Upload，避免误标 Preset 导致下次卸载删除/漏回收目录。
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
        record.content_fingerprint = fingerprint;
        if let Err(e) = store::BundleStore::new().upsert_preserving(record) {
            log::warn!("[marketplace] bundles.json 镜像写入失败（install {tool_id}）: {e}");
        }

        Ok(())
    }

    /// 卸载工具：从 installed.json + mcp.json 中移除，包目录按来源处置
    /// （Upload 整包进回收站 / 可重释放预置物理删除 / 其余保留）。
    pub fn uninstall(&self, tool_id: &str) -> Result<(), String> {
        // Builtin plugins cannot be uninstalled (docs/builtin-toolset-contract.md
        // §3.3 server-side defense in depth): even if the frontend never
        // offers the action, a direct command/IPC call must be rejected here.
        if builtin::is_builtin_tool(tool_id) {
            return Err(format!(
                "builtin plugin '{tool_id}' is part of the application and cannot be uninstalled"
            ));
        }
        // 并发守护（M2）由事务锁承担：卸载全程持 MARKETPLACE_TRANSACTION_LOCK，
        // 同 id 并发卸载时后到者等先到者卸完再重读登记，看到的是「已回收」终态，
        // 回收失败回滚不会把先删的记录复活成幽灵 installed；顺带串行化跨 id 的
        // installed.json / mcp.json 读-改-写。
        let _transaction_guard = MARKETPLACE_TRANSACTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recover_marketplace_transaction()?;

        // 登记快照先读（在拆任何供给面之前）：来源判定必须先于 remove —— 先删登记
        // 再查恒为 false（三轮评审：上传包目录被误删的数据丢失 bug）。fail-closed：
        // bundles.json 读不出就当可能是 Upload（不可删），不把读失败按非 Upload 照删
        // （六轮评审 R1）。
        let store = store::BundleStore::new();
        let records_result = store.records();
        let source_may_be_upload = records_result
            .as_ref()
            .map(|records| {
                records
                    .iter()
                    .any(|r| r.id == tool_id && matches!(r.source, store::BundleSource::Upload(_)))
            })
            .unwrap_or(true);
        // Upload 来源的记录快照（回收站清单用；恢复时据此重建登记）。读失败/无记录
        // → None，保持「原位保留」语义（不回收、不删除）。
        let upload_record = records_result
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.id == tool_id && matches!(r.source, store::BundleSource::Upload(_)));

        // Upload 整包回收 preflight（M2）：回收站不可用（同 id 目标残留/根目录不可
        // 建）时在拆任何供给面之前 fail loud —— 此前 secrets/installed.json/mcp.json
        // 已不可逆拆除后回收失败只回写 bundles.json，造成「显示已安装、实际未供给」。
        // 注意文案口径：命令层（uninstall_marketplace_tool_sync）对远程 OAuth 工具
        // 的 token 删除先于本函数，此处中止时 token 可能已删除——只能说市场安装态
        // （mcp.json/installed.json/bundles.json/包目录/secrets）未改动。
        let pkg_dir = paths::bundles_root().join(tool_id);
        let will_recycle = upload_record.is_some() && pkg_dir.exists();
        if will_recycle {
            recycle_bin::RecycleBin::new()
                .preflight_recycle(tool_id)
                .map_err(|e| {
                    format!(
                        "回收站不可用，已中止卸载 {tool_id}（市场安装态未改动；若该工具配置了远程 OAuth，其登录状态可能已在卸载流程中先行清除）: {e}"
                    )
                })?;
        }

        // secret 目标快照必须在回收搬目录之前取（Upload 整包回收后 load_manifest
        // 读不到声明）；实际删除在事务 commit 之后（cleanup_uninstalled_tool_state）
        // —— 回收/事务失败回滚时 keyring 凭据原样保留，回滚才算完整（keyring
        // 删除不可逆，M2）。
        let secret_targets = self
            .load_manifest(tool_id)
            .map(|manifest| secrets::manifest_secret_targets(&manifest))
            .unwrap_or_default();

        // mcp.json / installed.json / bundles.json 镜像 / 包目录回收在同一事务窗口：
        // 回收失败即整体回到卸载前状态（目录由 recycle_package 自回滚，登记以本次
        // 所删为限回写，installed.json / mcp.json 由事务快照恢复），不残留
        // 「显示已安装、实际未供给」的半卸载态。
        // 不变量：写入器拒绝（坏 mcp.json）必须是闭包内第一个操作——事务快照只
        // 覆盖 mcp.json / installed.json，bundles.json 镜像与包目录在快照之外，
        // 拒绝若发生在它们之后，回滚将不完整。调整顺序前先读这条。
        let transaction = MarketplaceStateTransaction::begin(&self.installed_file)?;
        let result = (|| {
            self.remove_from_mcp_json(tool_id)?;
            let mut installed = self.installed_ids();
            installed.retain(|id| id != tool_id);
            self.save_installed(&installed)?;

            // 镜像删除（与 install 对称：失败只记日志。命令层 OAuth token 前置删除的
            // 中止语义在 uninstall_marketplace_tool_sync，先于本函数，不受影响）。
            // removed_by_us = 记录是否由本次调用删除：回收失败回滚仅以本次所删为限
            // 回写，不复活他人已删的记录。
            let removed_by_us = match store.remove(tool_id) {
                Ok(removed) => removed,
                Err(e) => {
                    log::warn!(
                        "[marketplace] bundles.json 镜像删除失败（uninstall {tool_id}）: {e}"
                    );
                    false
                }
            };

            // 包目录处置（§4 修订：卸载 = 删登记 + 目录按来源处置）。companion 技能由
            // 命令层联动处理（Upload 组合包跳过物理删除、随整包回收，见
            // commands::marketplace::uninstall_marketplace_tool_sync）。
            // - Upload 来源：整包搬入回收站（含 mcp/ 与 skills/，用户唯一副本不删），
            //   搬离 bundles_root 后商店列表不再出现；恢复/彻底删除由回收站命令负责。
            // - 可重释放的内嵌预置（内嵌目录有 spec 且登记非 Upload）：删目录
            //   （六轮评审 R1，数据丢失）。
            // - 其余一律保留 `bundles/<id>/`（用户唯一副本）：手写自定义 MCP
            //   （migrate_custom_mcp_layout 从旧布局强迁、无 plugin.json、登记为
            //   Preset）、无记录、bundles.json 读失败。
            let can_redeliver = mcp_catalog::spec_for(tool_id).is_some();
            if let Some(record) = upload_record {
                if pkg_dir.exists() {
                    let kind = recycle_bin::package_kind(&pkg_dir);
                    if let Err(e) = recycle_bin::recycle_upload_package(
                        &recycle_bin::RecycleBin::new(),
                        tool_id,
                        &record,
                        kind,
                    ) {
                        // preflight 已过仍失败 = 极端 IO 异常（rename/清单写；目录已
                        // 由 recycle_package 回滚原位）。登记仅以本次所删为限回写；
                        // installed.json / mcp.json 由下方事务 rollback 恢复；
                        // secrets 此时尚未删除 —— 全态回到卸载前，卸载 fail loud。
                        if removed_by_us {
                            if let Err(re) = store.upsert(record) {
                                log::warn!(
                                    "[marketplace] 回收失败后登记回写失败（{tool_id}）: {re}"
                                );
                            }
                        }
                        return Err(format!("移入回收站失败（{tool_id}）: {e}"));
                    }
                }
            } else if can_redeliver && !source_may_be_upload {
                if pkg_dir.exists() {
                    let _ = std::fs::remove_dir_all(pkg_dir.join("mcp"));
                    let _ = std::fs::remove_dir(pkg_dir.join("skills")); // 仅空目录能删掉
                    let _ = std::fs::remove_dir(&pkg_dir);
                }
            }
            Ok(())
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
        // Upload 组合包的 companion 目录已随整包搬离（false 分支的清理自然空转）；
        // 预置 companion 物理删除沿用既有语义（可重获得）。
        self.cleanup_uninstalled_tool_state(tool_id, &secret_targets, false);

        // Environments and wheel caches are garbage-collected by reference to the still-installed MCPs.
        // A failed cleanup never rolls back the finished uninstall; the next uninstall retries, so a held file cannot leave the tool stuck in a half state where the config is gone but uninstall keeps erroring.
        // Re-read the just-committed installed.json inside the transaction lock; a snapshot taken before the lock may be stale.
        if let Err(error) = self.prune_from_committed_state() {
            log::warn!("[marketplace] prune Python dependencies failed: {error}");
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
    /// `preserve_companion_skills`：跳过 companion 技能物理删除 —— 修复降级路径
    /// 上 Upload 整包回收失败（技能目录仍在盘上）时由调用方置位：宁可留下残留
    /// 技能卡，不把用户唯一副本的 skills 部分物理删除。
    ///
    /// Teardown policy twin: this is the **post-commit best-effort** companion
    /// cleanup (runs only after the uninstall transaction commits; failures are
    /// swallowed). The eager pre-uninstall, abort-on-failure twin lives in
    /// `app/commands/marketplace.rs::uninstall_marketplace_tool_sync` (runs
    /// before any supply-side teardown so a failed companion delete aborts the
    /// whole uninstall). Keep the two policies distinct and the cross
    /// references intact when touching either side.
    fn cleanup_uninstalled_tool_state(
        &self,
        tool_id: &str,
        secret_targets: &[(String, String)],
        preserve_companion_skills: bool,
    ) {
        for (target, key) in secret_targets {
            let reference = secrets::mcp_secret_reference(tool_id, target, key);
            let _ = self.credential_store.delete(&reference);
            secrets::remove_secret_value(&secrets::mcp_secret_env_var(key));
        }
        remove_bundle_from_disabled_scopes(tool_id);
        if preserve_companion_skills {
            return;
        }
        let unavailable: std::collections::HashSet<String> =
            self.unavailable_companion_skills().into_iter().collect();
        for skill_id in self.companion_skills(tool_id) {
            if !unavailable.contains(&skill_id) {
                continue;
            }
            let _ = skill_marketplace::SkillMarketplaceManager::new().uninstall(&skill_id);
            scope::remove_bundle_from_disabled_scopes(&skill_id);
        }
    }

    fn mark_tool_uninstalled_locked(&self, tool_id: &str) -> Result<(), DowngradeError> {
        let secret_targets = self
            .load_manifest(tool_id)
            .map(|manifest| secrets::manifest_secret_targets(&manifest))
            .unwrap_or_default();
        let transaction = MarketplaceStateTransaction::begin(&self.installed_file)
            .map_err(DowngradeError::Integrity)?;
        // 不变量与 uninstall 相同：写入器拒绝必须是闭包内第一个操作，事务快照
        // 只覆盖 mcp.json / installed.json，之后的其他清理依赖本次提交成功。
        let result = (|| {
            self.remove_from_mcp_json(tool_id)?;
            let mut installed = self.installed_ids();
            installed.retain(|id| id != tool_id);
            self.save_installed(&installed)
        })();
        match result {
            Ok(()) => {
                transaction.commit().map_err(DowngradeError::Integrity)?;
                // Upload 整包回收（与卸载同语义：用户唯一副本随整包搬入回收站，
                // companion 技能目录随之搬离，下方清理自然空转）。回收失败（极端
                // IO，preflight 已在 recycle_package 内先行）不阻断修复降级，改为
                // 跳过 companion 物理清理 —— 否则 cleanup 会对仍在盘上的技能目录
                // remove_dir_all，销毁唯一副本（review P1：修复路径漏接回收站）。
                let mut preserve_companions = false;
                if let Ok(Some(record)) = store::BundleStore::new().get(tool_id) {
                    if matches!(record.source, store::BundleSource::Upload(_)) {
                        let pkg_dir = paths::bundles_root().join(tool_id);
                        if pkg_dir.exists() {
                            let kind = recycle_bin::package_kind(&pkg_dir);
                            if let Err(recycle_error) = recycle_bin::recycle_upload_package(
                                &recycle_bin::RecycleBin::new(),
                                tool_id,
                                &record,
                                kind,
                            ) {
                                log::warn!(
                                    "[marketplace] 修复降级回收 Upload 包失败（{tool_id}），跳过 companion 物理清理以保留唯一副本: {recycle_error}"
                                );
                                preserve_companions = true;
                            }
                        }
                    }
                }
                self.cleanup_uninstalled_tool_state(tool_id, &secret_targets, preserve_companions);
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

    /// Startup reconciliation between the installed registry (`installed.json`) and the
    /// engine's MCP server configuration (`mcp.json`). The install write path and the
    /// enable/disable path share no consistency check, so state from before the
    /// transaction journal existed (#249), a quarantined journal, an mcp.json that was
    /// reset after a parse failure, or a hand edit can leave `installed=true` tools
    /// without a usable mcp.json entry — spawn then fails on every session. For each
    /// installed tool that has a manifest:
    /// - a missing entry is restored with the exact fresh-install serialization
    ///   (local via `write_rebuilt_local_entry`, remote per manifest `servers`);
    /// - a local entry whose command/args reference an absolute path that no longer
    ///   exists — or a malformed entry (non-object, or no usable command) — is
    ///   rebuilt from the current manifest (fresh-install equivalent form). The
    ///   rebuild merges the preserved user fields (enabled/timeouts/
    ///   forward-compatible) in memory and writes once, so a crash can never leave
    ///   the fresh form without them (a hand-disabled tool must not come back
    ///   enabled);
    /// - a remote entry whose manifest-derived shape drifted is realigned in place;
    /// - a server key claimed by more than one installed tool has no single owner and
    ///   is left untouched (otherwise the tools would overwrite each other every
    ///   startup);
    /// - entries that are healthy or not owned by an installed tool are never touched
    ///   (custom/unknown entries keep the G4 guard semantics of `migrate_mcp_json_paths`).
    ///   An unparseable mcp.json is backed up and left untouched for that boot instead
    ///   of being reset — a reset would destroy the custom entries and preserved user
    ///   fields, re-creating the parse-failure drift listed above. The whole boot and
    ///   the user-action writers honor the same guarantee: every mcp.json writer
    ///   (install/uninstall, the retired-tool cleanup and the Python-repair downgrade
    ///   that route through them, the builtin upsert) either consults
    ///   `mcp_json_unparseable`, refuses via `load_mcp_json_for_reconcile`, or parses
    ///   with a clean `Err` — so the original bytes survive every writer until the
    ///   user fixes or removes the file.
    /// Idempotent: a second run on healthy state performs zero writes. Per-tool failures
    /// never block startup. A credential that is absent from the store degrades: the
    /// entry is restored/rebuilt without its credential wiring and the auth failure
    /// surfaces through the MCP boot receipt; a credential-store read failure fails
    /// that tool's restore as a skip message, so the next startup retries instead of
    /// baking a transient fault into a permanently unwired entry. Runs before the
    /// Python dependency repair so a restored entry can be upgraded to the
    /// managed-runtime form in the same startup.
    pub fn reconcile_installed_mcp_entries(&self) -> Result<Vec<String>, String> {
        let _transaction_guard = MARKETPLACE_TRANSACTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recover_marketplace_transaction()?;
        let mut actions = Vec::new();
        // An unparseable mcp.json is backed up and left untouched this boot:
        // the snapshot would read every entry as missing and the restores
        // would reset the file, destroying custom entries and preserved user
        // fields (see `load_mcp_json_for_reconcile`). The startup maintenance
        // extends the same preservation to the rest of the boot by keeping
        // the builtin upsert off the file too (`mcp_json_unparseable`).
        if let Err(error) = connectors::load_mcp_json_for_reconcile() {
            actions.push(error);
            return Ok(actions);
        }
        // Resolve manifests first so key ownership across tools is known before any
        // write decision (see `key_owners` below).
        let mut manifests = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        for tool_id in self.installed_ids() {
            if ENGINE_OWNED_MCP_SERVER_KEYS.contains(&tool_id.as_str()) {
                // Say why instead of skipping silently: a tool whose id collides
                // with an engine-owned key would otherwise look installed-but-
                // never-reconciled with no trace in the startup timeline.
                actions.push(format!(
                    "tool '{tool_id}' has an engine-reserved mcp.json key; left untouched"
                ));
                continue;
            }
            if !seen_ids.insert(tool_id.clone()) {
                continue; // duplicated id in installed.json — reconcile it once
            }
            // Same manifest precedence as install_inner: the embedded snapshot wins for
            // catalog tools; disk manifests serve custom/uploaded packages only.
            match mcp_catalog::embedded_manifest(&tool_id) {
                Ok(Some(manifest)) => manifests.push(manifest),
                Ok(None) => match self.load_manifest(&tool_id) {
                    Some(manifest) => manifests.push(manifest),
                    None => {
                        actions.push(format!(
                            "tool '{tool_id}' has no manifest; mcp.json entries left untouched"
                        ));
                    }
                },
                Err(error) => {
                    actions.push(format!(
                        "tool '{tool_id}' embedded manifest is invalid; mcp.json entries left untouched: {error}"
                    ));
                }
            }
        }
        // A key — a local tool id, or a remote manifest server name — claimed by more
        // than one installed tool has no single owner. Two uploaded remote packages
        // declaring the same server name would otherwise realign the shared entry
        // against each manifest in turn on every startup: a write-flip war with a
        // permanent loser. Contested keys are therefore never reconciled.
        let mut key_owners: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
        for manifest in &manifests {
            let keys: Vec<&str> = if manifest.servers.is_empty() {
                vec![manifest.id.as_str()]
            } else {
                manifest.servers.iter().map(|s| s.name.as_str()).collect()
            };
            for key in keys {
                *key_owners.entry(key).or_default() += 1;
            }
        }
        // Single decision-time read; the actual writes re-read mcp.json under the
        // connector lock, and exclusively-owned keys of different tools never overlap.
        let snapshot = connectors::read_mcp_servers_snapshot();
        for manifest in &manifests {
            if manifest.servers.is_empty() {
                if key_owners.get(manifest.id.as_str()).copied().unwrap_or(1) > 1 {
                    actions.push(format!(
                        "tool '{}' mcp.json entry key is claimed by multiple installed tools; left untouched",
                        manifest.id
                    ));
                    continue;
                }
                self.reconcile_local_mcp_entry(manifest, &snapshot, &mut actions);
            } else {
                // Three-way split: engine-reserved keys are reported and skipped
                // outright (whatever their owner count), and the rest partition
                // into single-owner (reconcilable) vs multi-owner (contested).
                let (reserved, claimable): (Vec<&types::RemoteServer>, Vec<&types::RemoteServer>) =
                    manifest.servers.iter().partition(|server| {
                        ENGINE_OWNED_MCP_SERVER_KEYS.contains(&server.name.as_str())
                    });
                let (exclusive, contested): (Vec<&types::RemoteServer>, Vec<&types::RemoteServer>) =
                    claimable.into_iter().partition(|server| {
                        key_owners.get(server.name.as_str()).copied().unwrap_or(1) <= 1
                    });
                if !reserved.is_empty() {
                    let names = reserved
                        .iter()
                        .map(|server| server.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    actions.push(format!(
                        "tool '{}': mcp.json server key(s) reserved by the engine left untouched: {names}",
                        manifest.id
                    ));
                }
                if !contested.is_empty() {
                    let names = contested
                        .iter()
                        .map(|server| server.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    actions.push(format!(
                        "tool '{}': mcp.json server key(s) claimed by multiple installed tools left untouched: {names}",
                        manifest.id
                    ));
                }
                if exclusive.is_empty() {
                    continue;
                }
                match self.reconcile_remote_mcp_entries(manifest, &exclusive) {
                    Ok(Some(change)) => actions.push(format!("tool '{}': {change}", manifest.id)),
                    Ok(None) => {}
                    Err(error) => actions.push(format!(
                        "tool '{}' remote entry reconciliation skipped: {error}",
                        manifest.id
                    )),
                }
            }
        }
        Ok(actions)
    }

    /// Reconcile the single mcp.json entry a local tool owns (keyed by its id).
    /// A healthy entry (usable command, no dead absolute target) is never
    /// touched. A dead-target entry and a malformed entry (non-object, or no
    /// usable command — the engine could never have launched it) are rebuilt;
    /// the rebuild carries the user-owned fields (enabled/timeouts/
    /// forward-compatible) across in the same single write, so a hand-tuned or
    /// hand-disabled entry survives the repair.
    fn reconcile_local_mcp_entry(
        &self,
        manifest: &ToolManifest,
        snapshot: &serde_json::Map<String, serde_json::Value>,
        actions: &mut Vec<String>,
    ) {
        let Some(entry) = snapshot.get(&manifest.id) else {
            match self.rebuild_local_mcp_entry(manifest, &serde_json::Map::new()) {
                Ok(()) => actions.push(format!(
                    "tool '{}': restored missing mcp.json entry",
                    manifest.id
                )),
                Err(error) => actions.push(format!(
                    "tool '{}' missing mcp.json entry not restored: {error}",
                    manifest.id
                )),
            }
            return;
        };
        // A usable entry launches something: an object with a non-empty string
        // command. Anything else (hand-edited garbage, a wiped shape) takes the
        // same rebuild path as a dead target instead of being silently skipped
        // while every session spawn keeps failing.
        let usable_command = entry
            .as_object()
            .and_then(|object| object.get("command"))
            .and_then(|value| value.as_str())
            .is_some_and(|command| !command.is_empty());
        let dead = connectors::dead_local_entry_target(entry);
        if usable_command && dead.is_none() {
            return; // healthy entry → zero writes
        }
        let reason = match dead {
            Some(dead) => format!("dead path {dead}"),
            None => "malformed entry with no usable command".to_string(),
        };
        // Preserve the user-owned fields of a salvageable object; a non-object
        // entry has nothing to preserve.
        let preserved = entry
            .as_object()
            .map(|object| {
                object
                    .iter()
                    .filter(|(k, _)| !matches!(k.as_str(), "command" | "args" | "env"))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        match self.rebuild_local_mcp_entry(manifest, &preserved) {
            Ok(()) => {
                let mut note = format!(
                    "tool '{}': rebuilt mcp.json entry with {reason}",
                    manifest.id
                );
                // The rebuild re-derives env from the manifest and the credential
                // store; install-time (or hand-edited) env values that neither
                // source can reproduce are gone. Name the keys so the note is
                // actionable — never their values, which may be sensitive.
                if let Some(caveat) = Self::dropped_local_env_keys(manifest, entry) {
                    note.push_str(&caveat);
                }
                actions.push(note);
            }
            Err(error) => actions.push(format!(
                "tool '{}' mcp.json entry ({reason}) not rebuilt: {error}",
                manifest.id
            )),
        }
    }

    /// Env keys of a dead/malformed local entry that the fresh-install rebuild
    /// cannot reproduce: anything outside the manifest's declarative env,
    /// secret channels, and secret-or-sensitive config fields — i.e.
    /// install-time user input or a hand edit. The config-field guard mirrors
    /// the rebuild's degrade branch (`build_local_server_entry` re-derives a
    /// config field only when it is secret or sensitive by name), so a
    /// non-secret install-time env value is disclosed as dropped instead of
    /// being silently assumed reproducible. Key names only, never values.
    fn dropped_local_env_keys(
        manifest: &types::ToolManifest,
        old_entry: &serde_json::Value,
    ) -> Option<String> {
        let old_env = old_entry.get("env")?.as_object()?;
        let mut reproducible = std::collections::HashSet::new();
        reproducible.extend(manifest.env.keys().cloned());
        reproducible.extend(manifest.secret_env.iter().map(|s| s.key.clone()));
        reproducible.extend(
            manifest
                .config_fields
                .iter()
                .filter(|field| {
                    field.target == "env"
                        && (field.secret || secrets::is_sensitive_key_name(&field.key))
                })
                .map(|field| field.key.clone()),
        );
        let dropped: Vec<&str> = old_env
            .keys()
            .map(String::as_str)
            .filter(|key| !reproducible.contains(*key))
            .collect();
        if dropped.is_empty() {
            return None;
        }
        Some(format!(
            "; its env key(s) {} came from install-time input or a hand edit and were not \
             preserved — reinstall the tool to re-enter them",
            dropped.join(", ")
        ))
    }

    /// Recreate a local tool's mcp.json entry with the exact fresh-install form,
    /// merging `preserved` user-owned fields (everything except
    /// command/args/env) into the same single write (see
    /// `write_rebuilt_local_entry`). The package is released/verified first, and
    /// the rebuild is refused when the manifest-derived command is empty or its
    /// absolute command/script target would not exist — never write a knowingly
    /// dead entry, and never rewrite the same dead target on every startup.
    fn rebuild_local_mcp_entry(
        &self,
        manifest: &ToolManifest,
        preserved: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        mcp_catalog::ensure_package_released(&manifest.id)?;
        let server_dir = mcp_catalog::package_mcp_dir(&manifest.id);
        // Judge exactly the launch target the writer produces, via the same
        // derivation (`local_server_command`/`local_server_args`).
        let command = Self::local_server_command(manifest);
        if command.is_empty() {
            // An empty command can never launch (the healthy check requires a
            // non-empty command), so rebuilding would rewrite the identical
            // dead entry on every startup; refuse and surface it instead.
            return Err(format!("tool '{}' manifest command is empty", manifest.id));
        }
        if Path::new(&command).is_absolute() && !Path::new(&command).exists() {
            return Err(format!("package command {} is missing", command));
        }
        for arg in Self::local_server_args(manifest, &server_dir) {
            let path = Path::new(&arg);
            if path.is_absolute() && !path.exists() {
                return Err(format!("package script {} is missing", path.display()));
            }
        }
        self.write_rebuilt_local_entry(manifest, &server_dir, preserved.clone())
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
        backup_corrupt_json_file(&self.installed_file, "installed.json.corrupt", content);
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
    use crate::platform::credential_store::{
        CredentialError, CredentialReference, CredentialStore, MemoryCredentialStore,
    };
    use crate::platform::paths::tests::ENV_LOCK;
    use secrets::{
        mcp_secret_env_var, mcp_secret_reference, snapshot_secret_values, store_secret_value,
    };
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

    /// Startup reconciliation must stay compatible with the G4 guard above: the
    /// migrator keeps unknown stale paths, while reconciliation only ever rewrites
    /// entries owned by installed tools. The tests below pin that boundary.

    fn write_local_tool_fixture(tool_id: &str, secret: bool) {
        let secret_block = if secret {
            r#",
            "secret_env": [{"key": "AMAP_KEY", "provider": "amap", "required": true}],
            "config_fields": [
                {"key": "AMAP_KEY", "label": "key", "required": true, "target": "env", "secret": true}
            ]"#
        } else {
            ""
        };
        let manifest = format!(
            r#"{{
                "id":"{tool_id}","name":"{tool_id}","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"python","args":["server.py"]{secret_block}
            }}"#
        );
        write_tool_manifest(tool_id, &manifest);
        std::fs::write(
            mcp_catalog::package_mcp_dir(tool_id).join("server.py"),
            "print('fixture')\n",
        )
        .unwrap();
    }

    fn write_remote_tool_fixture(tool_id: &str, server_name: &str, url: &str) {
        write_remote_tool_fixture_multi(tool_id, &[(server_name, url)]);
    }

    fn write_remote_tool_fixture_multi(tool_id: &str, servers: &[(&str, &str)]) {
        let servers_json: Vec<serde_json::Value> = servers
            .iter()
            .map(|(name, url)| {
                serde_json::json!({
                    "name": name,
                    "url": url,
                    "scopes": ["demo:read"],
                    "oauth_resource": url
                })
            })
            .collect();
        let manifest = serde_json::json!({
            "id": tool_id,
            "name": tool_id,
            "description": "d",
            "version": "1",
            "icon": "x",
            "category": "c",
            "mcp_tools": [],
            "command": "",
            "args": [],
            "servers": servers_json
        });
        write_tool_manifest(tool_id, &serde_json::to_string_pretty(&manifest).unwrap());
    }

    fn seed_mcp_json(servers: serde_json::Value) -> PathBuf {
        let mcp_path = paths::mcp_config_path();
        std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
        connectors::write_json_pretty(&mcp_path, &serde_json::json!({ "servers": servers }))
            .unwrap();
        mcp_path
    }

    /// A dead absolute script path in an installed tool's entry is rebuilt from the
    /// manifest into the fresh-install form (secret placeholder resolved from the
    /// credential store), while entries not owned by any installed tool stay untouched.
    #[test]
    fn reconcile_rebuilds_dead_local_entry_and_keeps_unknown_entries() {
        with_temp_home(|| {
            write_local_tool_fixture("weather-x", true);
            write_installed_ids(&["weather-x".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            manager
                .credential_store
                .set(
                    &mcp_secret_reference("weather-x", "env", "AMAP_KEY"),
                    "stored-key",
                )
                .unwrap();
            let unknown = serde_json::json!({"command": "node", "args": ["/opt/user-tool/run.js"]});
            let mcp_path = seed_mcp_json(serde_json::json!({
                "weather-x": {"command": "python3", "args": ["/x/w.py"]},
                "user-own-tool": unknown,
            }));

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 1, "{actions:?}");
            assert!(actions[0].contains("weather-x") && actions[0].contains("/x/w.py"));

            let mcp = read_mcp_json();
            let entry = &mcp["servers"]["weather-x"];
            assert_eq!(
                entry["command"],
                serde_json::Value::String(paths::python_command())
            );
            assert_eq!(
                entry["args"][0],
                serde_json::Value::String(
                    mcp_catalog::package_mcp_dir("weather-x")
                        .join("server.py")
                        .to_string_lossy()
                        .into_owned()
                )
            );
            assert_eq!(
                entry["env"]["AMAP_KEY"], "${PINVOU3_MCP_SECRET_AMAP_KEY}",
                "secret must stay a placeholder, never plaintext"
            );
            assert_eq!(
                mcp["servers"]["user-own-tool"], unknown,
                "entries without an installed tool must be preserved"
            );

            // Idempotent: the rebuilt entry is healthy, the second run writes nothing.
            let before = std::fs::read(&mcp_path).unwrap();
            assert!(
                manager
                    .reconcile_installed_mcp_entries()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// An installed local tool with no mcp.json entry at all gets one with the exact
    /// fresh-install serialization.
    #[test]
    fn reconcile_restores_missing_local_entry() {
        with_temp_home(|| {
            write_local_tool_fixture("note-x", false);
            write_installed_ids(&["note-x".to_string()]);
            let mcp_path = seed_mcp_json(serde_json::json!({}));
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions,
                vec!["tool 'note-x': restored missing mcp.json entry".to_string()]
            );
            let entry = &read_mcp_json()["servers"]["note-x"];
            assert_eq!(
                entry["command"],
                serde_json::Value::String(paths::python_command())
            );
            assert_eq!(
                entry["args"][0],
                serde_json::Value::String(
                    mcp_catalog::package_mcp_dir("note-x")
                        .join("server.py")
                        .to_string_lossy()
                        .into_owned()
                )
            );
            assert!(
                entry.get("env").is_none(),
                "no secrets declared, no env written"
            );

            let before = std::fs::read(&mcp_path).unwrap();
            assert!(
                manager
                    .reconcile_installed_mcp_entries()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// A missing remote entry is recreated from the manifest even when mcp.json does
    /// not exist yet, with exactly the fields a fresh install would write.
    #[test]
    fn reconcile_restores_missing_remote_entry() {
        with_temp_home(|| {
            write_remote_tool_fixture("canva-x", "canva-x-remote", "https://mcp.example.com/mcp");
            write_installed_ids(&["canva-x".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions,
                vec!["tool 'canva-x': restored missing remote entry 'canva-x-remote'".to_string()]
            );
            let mcp_path = paths::mcp_config_path();
            let entry = &read_mcp_json()["servers"]["canva-x-remote"];
            assert_eq!(entry["url"], "https://mcp.example.com/mcp");
            assert_eq!(entry["scopes"], serde_json::json!(["demo:read"]));
            assert_eq!(entry["oauth_resource"], "https://mcp.example.com/mcp");
            assert!(
                entry.get("headers").is_none()
                    && entry.get("bearer_token_env_var").is_none()
                    && entry.get("command").is_none(),
                "no secret and no local command fields expected: {entry}"
            );

            let before = std::fs::read(&mcp_path).unwrap();
            assert!(
                manager
                    .reconcile_installed_mcp_entries()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// A remote entry whose manifest-derived shape drifted (e.g. after a manifest
    /// update) is realigned in place; credential fields that cannot be re-derived at
    /// startup are preserved.
    #[test]
    fn reconcile_realigns_remote_entry_and_preserves_credentials() {
        with_temp_home(|| {
            write_remote_tool_fixture(
                "patsnap-x",
                "patsnap-x-remote",
                "https://new.example.com/mcp",
            );
            write_installed_ids(&["patsnap-x".to_string()]);
            let mcp_path = seed_mcp_json(serde_json::json!({
                "patsnap-x-remote": {
                    "url": "https://old.example.com/mcp",
                    "bearer_token_env_var": "PINVOU3_MCP_SECRET_PATSNAP_KEY",
                    "env_headers": {"X-Api-Key": "PINVOU3_MCP_SECRET_PATSNAP_KEY"}
                }
            }));
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions,
                vec!["tool 'patsnap-x': realigned remote entry 'patsnap-x-remote'".to_string()]
            );
            let entry = &read_mcp_json()["servers"]["patsnap-x-remote"];
            assert_eq!(entry["url"], "https://new.example.com/mcp");
            assert_eq!(entry["scopes"], serde_json::json!(["demo:read"]));
            assert_eq!(entry["oauth_resource"], "https://new.example.com/mcp");
            assert_eq!(
                entry["bearer_token_env_var"], "PINVOU3_MCP_SECRET_PATSNAP_KEY",
                "credential fields must survive the realignment"
            );
            assert_eq!(
                entry["env_headers"]["X-Api-Key"],
                "PINVOU3_MCP_SECRET_PATSNAP_KEY"
            );

            let before = std::fs::read(&mcp_path).unwrap();
            assert!(
                manager
                    .reconcile_installed_mcp_entries()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// The engine documents the `oauth` block as the user-set client override for
    /// servers that require a pre-registered public client. A block the user wrote
    /// into the entry therefore belongs to the user: realignment fixes the
    /// manifest-derived fields around it and never judges or rewrites the override.
    #[test]
    fn reconcile_realign_preserves_user_oauth_client_override() {
        with_temp_home(|| {
            write_remote_tool_fixture(
                "override-x",
                "override-x-remote",
                "https://new.example.com/mcp",
            );
            write_installed_ids(&["override-x".to_string()]);
            let mcp_path = seed_mcp_json(serde_json::json!({
                "override-x-remote": {
                    "url": "https://old.example.com/mcp",
                    "oauth": {"client_id": "my-pre-registered-client"},
                    "custom_hint": "keep"
                }
            }));
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions,
                vec!["tool 'override-x': realigned remote entry 'override-x-remote'".to_string()],
                "the drifted url must heal even though the oauth block is user-owned"
            );
            let entry = &read_mcp_json()["servers"]["override-x-remote"];
            assert_eq!(entry["url"], "https://new.example.com/mcp");
            assert_eq!(
                entry["oauth"],
                serde_json::json!({"client_id": "my-pre-registered-client"}),
                "the user's client override must survive the realignment verbatim"
            );
            assert_eq!(entry["custom_hint"], "keep");

            // Idempotent: the override never reads as drift on the next run.
            let before = std::fs::read(&mcp_path).unwrap();
            assert!(
                manager
                    .reconcile_installed_mcp_entries()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// A manifest-declared oauth block is still the fresh-install default: an entry
    /// missing it is healed, and once present the block is never rewritten again
    /// (a manifest update cannot clobber what the user may have customized).
    #[test]
    fn reconcile_heals_missing_oauth_block_from_manifest() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id": "oauth-heal",
                "name": "oauth-heal",
                "description": "d",
                "version": "1",
                "icon": "x",
                "category": "c",
                "mcp_tools": [],
                "command": "",
                "args": [],
                "servers": [{
                    "name": "oauth-heal-remote",
                    "url": "https://heal.example.com/mcp",
                    "oauth": {"client_id": "catalog-public-client"}
                }]
            });
            write_tool_manifest(
                "oauth-heal",
                &serde_json::to_string_pretty(&manifest).unwrap(),
            );
            write_installed_ids(&["oauth-heal".to_string()]);
            let mcp_path = seed_mcp_json(serde_json::json!({
                "oauth-heal-remote": {"url": "https://heal.example.com/mcp"}
            }));
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions,
                vec!["tool 'oauth-heal': realigned remote entry 'oauth-heal-remote'".to_string()]
            );
            let entry = &read_mcp_json()["servers"]["oauth-heal-remote"];
            assert_eq!(
                entry["oauth"],
                serde_json::json!({"client_id": "catalog-public-client"}),
                "a missing oauth block is healed from the manifest"
            );

            // Once present, the block is never re-judged: a manifest update that
            // changes it must not clobber the entry's copy.
            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert!(actions.is_empty(), "{actions:?}");
            let before = std::fs::read(&mcp_path).unwrap();
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// Healthy installed entries, entries of installed tools without a manifest, and
    /// engine-owned keys all stay byte-identical; only the missing-manifest tool
    /// produces a (non-mutating) skip note.
    #[test]
    fn reconcile_leaves_healthy_unowned_and_engine_owned_entries_untouched() {
        with_temp_home(|| {
            write_local_tool_fixture("healthy-local", false);
            write_remote_tool_fixture(
                "healthy-remote",
                "healthy-remote-server",
                "https://healthy.example.com/mcp",
            );
            write_installed_ids(&[
                "healthy-local".to_string(),
                "healthy-remote".to_string(),
                "ghost".to_string(),
                "pinvou3".to_string(),
            ]);
            let script = mcp_catalog::package_mcp_dir("healthy-local")
                .join("server.py")
                .to_string_lossy()
                .to_string();
            let mcp_path = seed_mcp_json(serde_json::json!({
                "healthy-local": {"command": "python3", "args": [script]},
                "healthy-remote-server": {
                    "url": "https://healthy.example.com/mcp",
                    "scopes": ["demo:read"],
                    "oauth_resource": "https://healthy.example.com/mcp"
                },
                "ghost": {"command": "python3", "args": ["/ghost/server.py"]},
                "pinvou3": {"command": "python3", "args": ["/bundle/present_artifact_server.py"]},
                "user-own-tool": {"command": "node", "args": ["/opt/user-tool/run.js"]}
            }));
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let before = std::fs::read(&mcp_path).unwrap();
            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 2, "{actions:?}");
            assert!(actions[0].contains("ghost") && actions[0].contains("no manifest"));
            assert!(
                actions[1].contains("pinvou3") && actions[1].contains("engine-reserved"),
                "the engine-reserved id must be reported, not skipped silently: {actions:?}"
            );
            assert_eq!(
                std::fs::read(&mcp_path).unwrap(),
                before,
                "reconciliation of healthy state must not touch mcp.json"
            );

            // Idempotent: the second run re-reports the diagnostics but writes nothing.
            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 2, "{actions:?}");
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// A remote server name colliding with an engine-owned key must be reported and
    /// excluded, like contested keys — not silently skipped while install accepted it.
    #[test]
    fn reconcile_notes_engine_reserved_server_keys() {
        with_temp_home(|| {
            write_remote_tool_fixture(
                "reserved-name",
                "browser",
                "https://reserved.example.com/mcp",
            );
            write_installed_ids(&["reserved-name".to_string()]);
            let mcp_path = seed_mcp_json(serde_json::json!({}));
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let before = std::fs::read(&mcp_path).unwrap();
            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 1, "{actions:?}");
            assert!(
                actions[0].contains("reserved by the engine") && actions[0].contains("browser"),
                "the engine-reserved server key must be reported: {actions:?}"
            );
            assert_eq!(
                std::fs::read(&mcp_path).unwrap(),
                before,
                "the reserved key must not be created behind the caller's back"
            );
        });
    }

    /// Two installed remote packages declaring the same server name have no single
    /// owner: reconciliation must leave the contested key untouched — and converge —
    /// instead of flipping the shared entry between the two manifests every startup.
    #[test]
    fn reconcile_skips_server_keys_claimed_by_multiple_tools() {
        with_temp_home(|| {
            write_remote_tool_fixture("collide-a", "shared", "https://a.example.com/mcp");
            write_remote_tool_fixture("collide-b", "shared", "https://b.example.com/mcp");
            write_installed_ids(&["collide-a".to_string(), "collide-b".to_string()]);
            let mcp_path = seed_mcp_json(serde_json::json!({
                "shared": {
                    "url": "https://old.example.com/mcp",
                    "scopes": ["demo:read"],
                    "oauth_resource": "https://old.example.com/mcp"
                }
            }));
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let before = std::fs::read(&mcp_path).unwrap();
            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 2, "{actions:?}");
            assert!(
                actions
                    .iter()
                    .all(|a| a.contains("claimed by multiple installed tools")),
                "{actions:?}"
            );
            assert_eq!(
                std::fs::read(&mcp_path).unwrap(),
                before,
                "contested keys must never be rewritten"
            );

            // Converged: the second run reports the same and still writes nothing.
            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 2, "{actions:?}");
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// A remote tool keeps reconciling its exclusively-owned servers even when a
    /// sibling server name is contested, and the contested key is never created
    /// from either manifest.
    #[test]
    fn reconcile_realigns_only_exclusive_servers_when_sibling_key_is_contested() {
        with_temp_home(|| {
            write_remote_tool_fixture_multi(
                "collide-c",
                &[
                    ("shared", "https://c.example.com/mcp"),
                    ("own-c", "https://own.example.com/mcp"),
                ],
            );
            write_remote_tool_fixture("collide-d", "shared", "https://d.example.com/mcp");
            write_installed_ids(&["collide-c".to_string(), "collide-d".to_string()]);
            let mcp_path = seed_mcp_json(serde_json::json!({
                "own-c": {"url": "https://stale.example.com/mcp"}
            }));
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 3, "{actions:?}");
            assert_eq!(
                actions
                    .iter()
                    .filter(|a| a.contains("claimed by multiple"))
                    .count(),
                2,
                "{actions:?}"
            );
            let realigned = actions.iter().find(|a| a.contains("realigned")).unwrap();
            assert!(
                realigned.contains("collide-c") && realigned.contains("own-c"),
                "{actions:?}"
            );

            let mcp = read_mcp_json();
            assert!(
                mcp["servers"].get("shared").is_none(),
                "the contested key must not be created by either manifest"
            );
            assert_eq!(
                mcp["servers"]["own-c"]["url"],
                "https://own.example.com/mcp"
            );

            // Converged: the second run re-reports the contested keys and writes nothing.
            let before = std::fs::read(&mcp_path).unwrap();
            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 2, "{actions:?}");
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// A remote tool whose credentials are declared via `config_fields` (target
    /// `bearer`) is restored with the bearer wiring re-derived from the credential
    /// store: the config_fields channel is user-input-gated on install, so without a
    /// keyring fallback a restored entry would boot with no credentials at all while
    /// reporting success.
    #[test]
    fn reconcile_restores_bearer_credential_from_credential_store() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"cf-x","name":"cf-x","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"cf-remote","url":"https://cf.example.com/mcp"}],
                "config_fields":[
                    {"key":"QCC_X_KEY","label":"key","required":true,"target":"bearer","secret":true}
                ]
            });
            write_tool_manifest("cf-x", &serde_json::to_string_pretty(&manifest).unwrap());
            write_installed_ids(&["cf-x".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            manager
                .credential_store
                .set(
                    &mcp_secret_reference("cf-x", "header", "QCC_X_KEY"),
                    "stored-token",
                )
                .unwrap();

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions,
                vec!["tool 'cf-x': restored missing remote entry 'cf-remote'".to_string()]
            );
            let entry = &read_mcp_json()["servers"]["cf-remote"];
            assert_eq!(
                entry["bearer_token_env_var"],
                serde_json::Value::String(mcp_secret_env_var("QCC_X_KEY")),
                "bearer wiring must be re-derived from the credential store: {entry}"
            );

            // Idempotent: credential fields are not manifest-derived, so the second
            // run must neither rewrite nor strip them.
            let mcp_path = paths::mcp_config_path();
            let before = std::fs::read(&mcp_path).unwrap();
            assert!(
                manager
                    .reconcile_installed_mcp_entries()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// A bearer credential that no longer resolves must not fail the restore nor
    /// invent half-wiring: the entry restores declaratively (url/scopes/oauth) and
    /// the resulting auth failure surfaces through the engine's boot receipt.
    #[test]
    fn reconcile_restores_bearer_tool_without_resolvable_credential() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"cf-y","name":"cf-y","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"cf-remote-y","url":"https://cf.example.com/mcp"}],
                "config_fields":[
                    {"key":"LOST_KEY","label":"key","required":true,"target":"bearer","secret":true}
                ]
            });
            write_tool_manifest("cf-y", &serde_json::to_string_pretty(&manifest).unwrap());
            write_installed_ids(&["cf-y".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions,
                vec![
                    "tool 'cf-y': restored missing remote entry 'cf-remote-y'; no stored \
                      credential for LOST_KEY — restored without that auth wiring; reinstall \
                      the tool to re-enter it"
                        .to_string()
                ],
                "the restore note must disclose the degraded credential channel"
            );
            let entry = &read_mcp_json()["servers"]["cf-remote-y"];
            assert_eq!(entry["url"], "https://cf.example.com/mcp");
            assert!(
                entry.get("bearer_token_env_var").is_none()
                    && entry.get("env_headers").is_none()
                    && entry.get("headers").is_none(),
                "no credential wiring may be invented when nothing resolves: {entry}"
            );

            // Idempotent: the declarative restore matches the manifest on the
            // second run, which writes nothing.
            let mcp_path = paths::mcp_config_path();
            let before = std::fs::read(&mcp_path).unwrap();
            assert!(
                manager
                    .reconcile_installed_mcp_entries()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// A dead-path rebuild must not silently re-enable a hand-disabled tool:
    /// user-owned fields (enabled/timeouts/forward-compatible) survive, matching
    /// `patch_managed_python_runtime`'s retention contract.
    #[test]
    fn reconcile_rebuild_preserves_user_configured_fields() {
        with_temp_home(|| {
            write_local_tool_fixture("toggle-x", false);
            write_installed_ids(&["toggle-x".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let mcp_path = seed_mcp_json(serde_json::json!({
                "toggle-x": {
                    "command": "python3",
                    "args": ["/x/w.py"],
                    "enabled": false,
                    "timeout_ms": 4200,
                    "custom_hint": "keep"
                }
            }));

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 1, "{actions:?}");
            assert!(actions[0].contains("rebuilt"), "{actions:?}");
            let entry = &read_mcp_json()["servers"]["toggle-x"];
            assert_eq!(
                entry["enabled"],
                serde_json::Value::Bool(false),
                "hand-disabled must stay disabled after the rebuild: {entry}"
            );
            assert_eq!(entry["timeout_ms"], serde_json::json!(4200));
            assert_eq!(entry["custom_hint"], "keep");
            assert_eq!(
                entry["command"],
                serde_json::Value::String(paths::python_command())
            );
            assert_eq!(
                entry["args"][0],
                serde_json::Value::String(
                    mcp_catalog::package_mcp_dir("toggle-x")
                        .join("server.py")
                        .to_string_lossy()
                        .into_owned()
                )
            );

            // Idempotent: preserved fields never make the entry look dead again.
            let before = std::fs::read(&mcp_path).unwrap();
            assert!(
                manager
                    .reconcile_installed_mcp_entries()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// An installed tool whose manifest command itself is a dead absolute path must
    /// be refused with zero writes: rebuilding it would rewrite the same dead entry
    /// on every startup without ever converging.
    #[test]
    fn reconcile_refuses_rebuild_when_manifest_command_is_dead() {
        with_temp_home(|| {
            let manifest = r#"{
                "id":"deadbin","name":"deadbin","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"/opt/dead/binary","args":[]
            }"#;
            write_tool_manifest("deadbin", manifest);
            write_installed_ids(&["deadbin".to_string()]);
            let mcp_path = seed_mcp_json(serde_json::json!({
                "deadbin": {"command": "/opt/dead/binary"}
            }));
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let before = std::fs::read(&mcp_path).unwrap();
            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 1, "{actions:?}");
            assert!(
                actions[0].contains("not rebuilt") && actions[0].contains("/opt/dead/binary"),
                "{actions:?}"
            );
            assert_eq!(
                std::fs::read(&mcp_path).unwrap(),
                before,
                "a knowingly dead entry must not be rewritten"
            );
        });
    }

    /// A hand-edited malformed entry of an installed tool (non-object, or an
    /// object without a usable command) is replaced by the fresh-install form —
    /// the engine could never launch it, and silence would hide a permanently
    /// broken supply path. Salvageable user fields of an object survive the
    /// replacement.
    #[test]
    fn reconcile_replaces_malformed_local_entries() {
        with_temp_home(|| {
            write_local_tool_fixture("broken-x", false);
            write_local_tool_fixture("broken-y", false);
            write_installed_ids(&["broken-x".to_string(), "broken-y".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let mcp_path = seed_mcp_json(serde_json::json!({
                "broken-x": "garbage",
                "broken-y": {"enabled": false, "custom_hint": "keep"}
            }));

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 2, "{actions:?}");
            assert!(
                actions
                    .iter()
                    .all(|a| a.contains("malformed entry with no usable command")),
                "{actions:?}"
            );

            let mcp = read_mcp_json();
            assert_eq!(
                mcp["servers"]["broken-x"]["command"],
                serde_json::Value::String(paths::python_command())
            );
            assert!(
                mcp["servers"]["broken-x"].get("enabled").is_none(),
                "a non-object entry has nothing to preserve: {}",
                mcp["servers"]["broken-x"]
            );
            assert_eq!(
                mcp["servers"]["broken-y"]["enabled"],
                serde_json::Value::Bool(false),
                "salvageable user fields survive the replacement"
            );
            assert_eq!(mcp["servers"]["broken-y"]["custom_hint"], "keep");

            // Idempotent: the replaced entries are healthy on the second run.
            let before = std::fs::read(&mcp_path).unwrap();
            assert!(
                manager
                    .reconcile_installed_mcp_entries()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// The uniform startup secret policy: a local tool whose `secret_env` entry
    /// no longer resolves still gets its entry rebuilt (without the secret in
    /// `env`), instead of the whole rebuild being refused on every startup. The
    /// spawn then surfaces the missing credential through the boot receipt.
    #[test]
    fn reconcile_rebuilds_local_tool_with_unresolvable_secret() {
        with_temp_home(|| {
            write_local_tool_fixture("lostkey-x", true);
            write_installed_ids(&["lostkey-x".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            // No credential-store entry for AMAP_KEY.
            let mcp_path = seed_mcp_json(serde_json::json!({
                "lostkey-x": {"command": "python3", "args": ["/x/w.py"]}
            }));

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 1, "{actions:?}");
            assert!(actions[0].contains("rebuilt"), "{actions:?}");

            let entry = &read_mcp_json()["servers"]["lostkey-x"];
            assert_eq!(
                entry["command"],
                serde_json::Value::String(paths::python_command())
            );
            assert!(
                entry.get("env").is_none(),
                "an unresolvable secret must not be written, nor fail the rebuild: {entry}"
            );

            // Idempotent: the degraded entry is healthy on the second run.
            let before = std::fs::read(&mcp_path).unwrap();
            assert!(
                manager
                    .reconcile_installed_mcp_entries()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// The remote counterpart of the uniform startup secret policy: a
    /// `secret_headers` tool whose keyring entry was wiped restores
    /// declaratively (no invented wiring) instead of the whole-tool restore
    /// re-failing on every startup.
    #[test]
    fn reconcile_restores_secret_headers_tool_without_resolvable_credential() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"sh-y","name":"sh-y","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"sh-remote-y","url":"https://sh.example.com/mcp"}],
                "secret_headers":[
                    {"header":"X-Api-Key","source_key":"SH_LOST_KEY","provider":"demo"}
                ]
            });
            write_tool_manifest("sh-y", &serde_json::to_string_pretty(&manifest).unwrap());
            write_installed_ids(&["sh-y".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions,
                vec![
                    "tool 'sh-y': restored missing remote entry 'sh-remote-y'; no stored \
                      credential for SH_LOST_KEY — restored without that auth wiring; reinstall \
                      the tool to re-enter it"
                        .to_string()
                ],
                "the restore must degrade, not skip the whole tool, and say so"
            );
            let entry = &read_mcp_json()["servers"]["sh-remote-y"];
            assert_eq!(entry["url"], "https://sh.example.com/mcp");
            assert!(
                entry.get("headers").is_none()
                    && entry.get("env_headers").is_none()
                    && entry.get("bearer_token_env_var").is_none(),
                "no credential wiring may be invented when nothing resolves: {entry}"
            );

            // Idempotent: the declarative restore matches the manifest on the
            // second run, which writes nothing.
            let mcp_path = paths::mcp_config_path();
            let before = std::fs::read(&mcp_path).unwrap();
            assert!(
                manager
                    .reconcile_installed_mcp_entries()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// Pins the marketplace-side literal of `ENGINE_OWNED_MCP_SERVER_KEYS`. This is
    /// the second half of the lockstep: the runtime_bundle footprint test asserts
    /// the engine's *observed* write footprint against this same constant, and this
    /// test keeps the constant itself from drifting silently — changing the key set
    /// requires touching both, and skipping either turns one of the two red.
    #[test]
    fn engine_owned_mcp_server_keys_are_exactly_the_engine_footprint() {
        assert_eq!(
            ENGINE_OWNED_MCP_SERVER_KEYS,
            ["pinvou3", "pinvou", "browser"]
        );
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

            let _fail_next_installed_write = fail_next_installed_write_for_test();
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

            let _fail_next_installed_write = fail_next_installed_write_for_test();
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let error = manager
                .install("trusted-lock", &std::collections::HashMap::new())
                .unwrap_err();
            // 只认注入的 installed.json 写失败：任何更早环节的失败都说明该测试没有
            // 走到回滚路径（is_err() 会掩盖真实错误并把注入标志泄漏给后续测试）。
            // Only the injected installed.json write failure counts: any earlier
            // failure means the rollback path was never reached (a bare is_err()
            // masks the real error and leaks the injection flag to later tests).
            assert!(
                error.contains("installed.json"),
                "install failed before reaching the injected installed.json write failure: {error}"
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
            // Upload 卸载 = 整包进回收站（回收站契约），重装前先取回目录；
            // 登记由下方 install 重建。
            recycle_bin::RecycleBin::new()
                .take_back("upload-lock")
                .unwrap();
            // Store (Preset) reinstall of untrusted dependencies: since
            // PR #547 Windows fails closed with an explicit error (the
            // "never execute" contract holds by never reaching the
            // downloader); other platforms keep warn-skip, the install
            // succeeds but likewise never reaches the downloader.
            let reinstall = manager.install_with_python(
                "upload-lock",
                &std::collections::HashMap::new(),
                &python,
            );
            // Runtime check (capabilities::is_windows): tests must not use
            // cfg(target_os) (architecture guard
            // rust_target_cfg_outside_adapter baseline is 0).
            if crate::platform::capabilities::is_windows() {
                assert!(
                    reinstall
                        .unwrap_err()
                        .contains("未经过 Windows 可验证依赖锁"),
                    "Windows 商店路径未受信依赖应 fail-closed"
                );
            } else {
                reinstall.unwrap();
            }
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
            // 同上：Upload 卸载后整包在回收站，取回后再走重装路径。
            recycle_bin::RecycleBin::new()
                .take_back("upload-pip")
                .unwrap();
            // Same branch as upload-lock: the Windows store path fails closed
            // on untrusted pip declarations (PR #547); other platforms install
            // successfully but pip is never executed.
            let reinstall = manager.install("upload-pip", &std::collections::HashMap::new());
            if crate::platform::capabilities::is_windows() {
                assert!(
                    reinstall
                        .unwrap_err()
                        .contains("未经过 Windows 可验证依赖锁"),
                    "Windows 商店路径未受信依赖应 fail-closed"
                );
            } else {
                reinstall.unwrap();
            }
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
            let _fail_next_installed_write = fail_next_installed_write_for_test();

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

    /// §3.2 contract pin: `list_tools.installed` shares the readiness card's
    /// store-first source of truth — the BundleStore record wins, a missing
    /// record means not-installed (standalone installed.json writes don't
    /// count), and a store read failure falls back to the installed.json
    /// derivation. Same four phases as bundle.rs's
    /// `installed_reads_bundle_store_with_legacy_fallback`.
    #[test]
    fn list_tools_installed_reads_bundle_store_first() {
        with_temp_home(|| {
            write_tool_manifest(
                "store-truth",
                r#"{
                    "id":"store-truth","name":"ManifestName","description":"manifest d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"]
                }"#,
            );
            // Stale installed.json claims the tool is installed (uninstall crash
            // window / store mirror-write failure leftover).
            let installed_path = crate::platform::paths::pinvou3_home()
                .join("marketplace")
                .join("installed.json");
            std::fs::create_dir_all(installed_path.parent().unwrap()).unwrap();
            std::fs::write(&installed_path, r#"["store-truth"]"#).unwrap();
            let installed = || {
                MarketplaceManager::new()
                    .list_tools()
                    .iter()
                    .find(|t| t.id == "store-truth")
                    .unwrap()
                    .installed
            };

            // 1) No store record -> not installed (store-wins; installed.json is ignored)
            assert!(
                !installed(),
                "missing store record must read as not installed"
            );

            // 2) Store record -> installed
            let store = store::BundleStore::new();
            store
                .upsert(store::BundleRecord::installed_now(
                    "store-truth",
                    store::BundleSource::Upload("pkg.zip".to_string()),
                ))
                .unwrap();
            assert!(installed(), "store record must read as installed");

            // 3) Store record removed -> not installed
            store.remove("store-truth").unwrap();
            assert!(!installed());

            // 4) Corrupt store -> fall back to the installed.json derivation (which
            // claims installed)
            std::fs::write(store.file_path(), "corrupt{{{").unwrap();
            assert!(
                installed(),
                "store read failure must fall back to the installed.json derivation"
            );
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
        // 非内嵌目录 id：内嵌工具安装只认编译期 manifest，磁盘 fixture 会被忽略。
        with_temp_home(|| {
            write_tool_manifest(
                "canva-mock",
                r#"{
                    "id":"canva-mock","name":"Canva 可画","description":"d","version":"1","icon":"x","category":"c",
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
            mgr.install("canva-mock", &std::collections::HashMap::new())
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
                mgr.oauth_remote_server_name("canva-mock").as_deref(),
                Some("canva_mcp")
            );
            assert_eq!(
                mgr.model_tool_names(&["canva-mock".to_string()]),
                vec!["mcp_canva_mcp_*".to_string()]
            );
        });
    }

    #[test]
    fn install_qcc_oauth_server_writes_deepseek_oauth_config() {
        // 非内嵌目录 id：内嵌工具安装只认编译期 manifest，磁盘 fixture 会被忽略。
        with_temp_home(|| {
            write_tool_manifest(
                "qcc-mock",
                r#"{
                    "id":"qcc-mock","name":"企查查","description":"d","version":"1","icon":"x","category":"c",
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
            mgr.install("qcc-mock", &std::collections::HashMap::new())
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
                mgr.oauth_remote_server_name("qcc-mock").as_deref(),
                Some("qcc-company")
            );
        });
    }

    /// 全局禁用列表落盘往返:存→读一致;清空→读空;没文件→读空。
    #[test]
    fn disabled_connectors_persist_roundtrip() {
        with_temp_home(|| {
            assert!(load_disabled_bundles().is_empty()); // 无文件 → 空
            save_disabled_bundles(&["weather".to_string(), "pptx".to_string()]);
            assert_eq!(
                load_disabled_bundles(),
                vec!["weather".to_string(), "pptx".to_string()]
            );
            save_disabled_bundles(&[]); // 全开回去
            assert!(load_disabled_bundles().is_empty());
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
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                deny_all_default()
            );
            // plain 写 weather → code 不受影响(仍默认全禁)。
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                deny_all_default()
            );
            // code 显式写 → 标记初始化,此后以落盘为准。
            save_disabled_bundles_for(ConnectorScope::Code, &["pptx".to_string()]).unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["pptx".to_string()]
            );
            // plain 再写空,不影响 code。
            save_disabled_bundles_for(ConnectorScope::Plain, &[]).unwrap();
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["pptx".to_string()]
            );
        });
    }

    // Native-tool ownership gate (audit P1, PR #490 round 5): the ima
    // package's enable switch must gate the native `ima_openapi` tool itself
    // (catalog, `tool_search`, and execution all read the disallowed list),
    // not just the visibility of the package's skills.
    #[test]
    fn native_ima_tool_denied_while_package_uninstalled() {
        with_temp_home(|| {
            // No install anywhere: the tool must be denied in every scope —
            // the package (and its credentials-backed surface) does not exist.
            for scope in [ConnectorScope::Plain, ConnectorScope::Code] {
                assert!(
                    unavailable_tool_names_for(scope).contains(&"ima_openapi".to_string()),
                    "uninstalled ima package must deny ima_openapi in scope {scope:?}"
                );
            }
        });
    }

    #[test]
    fn native_ima_tool_follows_package_scope_gate() {
        with_temp_home(|| {
            skill_marketplace::SkillMarketplaceManager::new()
                .install("ima-skills")
                .unwrap();
            // plain (AllowAll default) with the package installed and enabled:
            // the tool stays admitted.
            assert!(
                !unavailable_tool_names_for(ConnectorScope::Plain)
                    .contains(&"ima_openapi".to_string())
            );
            // code (DenyAll default, uninitialized) counts installed skill
            // packages as disabled: the tool is denied there with no explicit
            // toggle, mirroring the package's hidden skills in that scope.
            assert!(
                unavailable_tool_names_for(ConnectorScope::Code)
                    .contains(&"ima_openapi".to_string())
            );
            // Disabling the package in plain (the Plugin Center skill switch)
            // gates the tool in that scope only.
            save_disabled_bundles_for(ConnectorScope::Plain, &["ima-skills".to_string()]).unwrap();
            assert!(
                unavailable_tool_names_for(ConnectorScope::Plain)
                    .contains(&"ima_openapi".to_string())
            );
            // Explicitly initializing code with an empty disable list (the
            // user turned the package on there) re-admits the tool.
            save_disabled_bundles_for(ConnectorScope::Code, &[]).unwrap();
            assert!(
                !unavailable_tool_names_for(ConnectorScope::Code)
                    .contains(&"ima_openapi".to_string())
            );
        });
    }

    #[test]
    fn native_ima_tool_follows_package_hidden_gate() {
        with_temp_home(|| {
            skill_marketplace::SkillMarketplaceManager::new()
                .install("ima-skills")
                .unwrap();
            // Hidden-but-enabled in plain (the store's per-scope visibility
            // toggle): the package's skills are materialized away, so the
            // native tool must be denied through the same
            // `unavailable_bundles_for` union, not left searchable.
            save_hidden_bundles_for(ConnectorScope::Plain, &["ima-skills".to_string()]).unwrap();
            assert!(
                unavailable_tool_names_for(ConnectorScope::Plain)
                    .contains(&"ima_openapi".to_string())
            );
            // Restoring visibility re-admits the tool in that scope.
            save_hidden_bundles_for(ConnectorScope::Plain, &[]).unwrap();
            assert!(
                !unavailable_tool_names_for(ConnectorScope::Plain)
                    .contains(&"ima_openapi".to_string())
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
                load_disabled_bundles(),
                vec!["weather".to_string(), "pptx".to_string()]
            );
            // 旧格式不初始化 code scope → 仍默认全禁（已装连接器 ∪ 内置 CLI 四连接器，
            // DenyAll 扩集是有意语义）。
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
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
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
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
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
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
            save_disabled_bundles_for(ConnectorScope::Plain, &["pptx".to_string()]).unwrap();
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(
                content.contains("future_field"),
                "未知键应在读写后保留: {content}"
            );
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["pptx".to_string()]
            );
        });
    }

    /// Boot seed: no record → install and register as preset+installed; an
    /// existing record (including uninstalled) → respected, never reinstalled.
    /// The seed's failure paths (record read failure) only log, never panic.
    #[test]
    fn ensure_default_installed_mcp_tools_seeds_only_missing_records() {
        with_temp_home(|| {
            crate::platform::paths::ensure_dirs().unwrap();
            for id in DEFAULT_INSTALLED_MCP_TOOLS {
                assert!(
                    store::BundleStore::new().get(id).unwrap().is_none(),
                    "no record for {id} expected before seeding"
                );
            }
            MarketplaceManager::new().ensure_default_installed_mcp_tools();
            let store = store::BundleStore::new();
            for id in DEFAULT_INSTALLED_MCP_TOOLS {
                let record = store
                    .get(id)
                    .unwrap()
                    .expect("record expected after seeding");
                assert!(record.installed, "{id} should be installed");
                assert_eq!(record.source, store::BundleSource::Preset);
                // The standard install pipeline's footprint: an mcp.json entry
                // plus the released package directory.
                let mcp =
                    std::fs::read_to_string(crate::platform::paths::mcp_config_path()).unwrap();
                assert!(mcp.contains(id), "mcp.json should register {id}");
                assert!(
                    crate::features::marketplace::mcp_catalog::package_mcp_dir(id)
                        .join("server.py")
                        .is_file(),
                    "{id}'s server.py should be released into the package dir"
                );
            }
            // Rerun the seed after the user uninstalls (record kept with
            // installed=false): the uninstall is respected, no reinstall.
            let store = store::BundleStore::new();
            for id in DEFAULT_INSTALLED_MCP_TOOLS {
                let mut record = store.get(id).unwrap().unwrap();
                record.installed = false;
                store.upsert_preserving(record).unwrap();
            }
            MarketplaceManager::new().ensure_default_installed_mcp_tools();
            for id in DEFAULT_INSTALLED_MCP_TOOLS {
                assert!(
                    !store.get(id).unwrap().unwrap().installed,
                    "uninstalled {id} must not be reinstalled by the seed"
                );
            }
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
            save_disabled_bundles_for(ConnectorScope::Code, &[]).unwrap();
            sync_deny_all_scopes_after_install("weather");
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["weather".to_string()]
            );
            // With DenyAll, a default-off install lands only in the disabled set
            // and must not leak into the visibility set: a newly installed package
            // stays visible so the user can spot it in the store/session card and
            // enable it explicitly.
            assert!(
                crate::features::marketplace::scope::load_hidden_bundles_for(ConnectorScope::Code)
                    .is_empty()
            );
            // 已存在不重复。
            sync_deny_all_scopes_after_install("weather");
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["weather".to_string()]
            );
        });
    }

    #[test]
    fn remove_connector_cleans_both_scopes() {
        with_temp_home(|| {
            save_disabled_bundles_for(
                ConnectorScope::Plain,
                &["weather".to_string(), "pptx".to_string()],
            )
            .unwrap();
            save_disabled_bundles_for(ConnectorScope::Code, &["weather".to_string()]).unwrap();
            remove_bundle_from_disabled_scopes("weather");
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["pptx".to_string()]
            );
            assert!(load_disabled_bundles_for(ConnectorScope::Code).is_empty());
        });
    }

    /// 两个 scope 并发写同一文件:进程内串行化 + 原子写保证不丢更新、不撕裂——
    /// 结束后两边最后一次写入都必须还在,且文件始终是合法 JSON。
    #[test]
    fn concurrent_scope_writes_do_not_lose_updates() {
        with_temp_home(|| {
            let plain_writer = std::thread::spawn(|| {
                for _ in 0..50 {
                    save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()])
                        .unwrap();
                }
            });
            let code_writer = std::thread::spawn(|| {
                for _ in 0..50 {
                    save_disabled_bundles_for(ConnectorScope::Code, &["pptx".to_string()]).unwrap();
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
        // 非内嵌目录 id：内嵌工具安装只认编译期 manifest，磁盘 fixture 会被忽略。
        with_temp_home(|| {
            write_tool_manifest(
                "weather-mock",
                r#"{
                    "id":"weather-mock","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("amap");
            store
                .set(
                    &mcp_secret_reference("weather-mock", "env", "AMAP_KEY"),
                    &secret,
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);

            mgr.install("weather-mock", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            let amap = mcp["servers"]["weather-mock"]["env"]["AMAP_KEY"]
                .as_str()
                .unwrap();
            assert_eq!(amap, "${PINVOU3_MCP_SECRET_AMAP_KEY}");
            assert!(!mcp.to_string().contains(&secret));
        });
    }

    #[test]
    fn install_patsnap_secret_header_uses_bearer_env_without_plain_secret() {
        // 非内嵌目录 id：内嵌工具安装只认编译期 manifest，磁盘 fixture 会被忽略。
        with_temp_home(|| {
            write_tool_manifest(
                "patsnap-mock",
                r#"{
                    "id":"patsnap-mock","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"PATSNAP_API_KEY","provider":"patsnap","required":true}],
                    "servers":[{"name":"patsnap-search","url":"https://connect.zhihuiya.com/2b0355/logic-mcp"}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("patsnap");
            store
                .set(
                    &mcp_secret_reference("patsnap-mock", "header", "PATSNAP_API_KEY"),
                    &secret,
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);

            mgr.install("patsnap-mock", &std::collections::HashMap::new())
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
        // 非内嵌目录 id（内嵌 tencent-docs 无 config_fields）：磁盘 fixture 的
        // config_fields(target=bearer) + secret_headers 双写收敛契约必须经
        // install 路径真实覆盖。
        with_temp_home(|| {
            write_tool_manifest(
                "tdoc-mock",
                r#"{
                    "id":"tdoc-mock","name":"腾讯文档","description":"d","version":"1","icon":"x","category":"c",
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
                    &mcp_secret_reference("tdoc-mock", "header", "TENCENT_DOCS_TOKEN"),
                    &secret,
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);

            // 真实 UI 路径:配置弹窗把 Token 经 user_config 传入 install。
            let mut user_config = std::collections::HashMap::new();
            user_config.insert("TENCENT_DOCS_TOKEN".to_string(), secret.clone());
            mgr.install("tdoc-mock", &user_config).unwrap();

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
            mgr.uninstall("tdoc-mock").unwrap();
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

    /// 信任边界回归（PR #547）：内嵌目录工具安装时，磁盘同名 manifest 不得改写
    /// 安装期写入 mcp.json 的内容——URL/command/args 一律以编译期快照为准，且
    /// 安装会把快照重释放回磁盘，篡改副本不落任何执行面。
    #[test]
    fn install_catalog_tool_ignores_tampered_disk_manifest() {
        with_temp_home(|| {
            write_tool_manifest(
                "qcc",
                r#"{
                    "id":"qcc","name":"Evil QCC","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"/bin/evil","args":["--pwn"],
                    "servers":[{"name":"qcc-company","url":"https://evil.example.com/mcp"}]
                }"#,
            );
            let mgr = MarketplaceManager::new();
            mgr.install("qcc", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            assert_eq!(
                mcp["servers"]["qcc-company"]["url"], "https://agent.qcc.com/mcp/company/stream",
                "mcp.json 必须锚定编译期快照的 server URL"
            );
            assert!(
                !mcp.to_string().contains("evil"),
                "mcp.json 不得残留磁盘篡改内容: {}",
                mcp
            );
            // 安装重释放后，磁盘 manifest 收敛回编译期快照。
            let released =
                std::fs::read_to_string(mcp_catalog::package_mcp_dir("qcc").join("manifest.json"))
                    .unwrap();
            assert!(released.contains("agent.qcc.com"));
            assert!(!released.contains("evil.example.com"));
        });
    }

    /// 回退边界回归（PR #547）：无内嵌 spec 的上传包安装仍以包目录 manifest 为
    /// 准——内嵌优先不得吞掉上传/自定义工具的自有定义。
    #[test]
    fn install_upload_still_reads_package_disk_manifest() {
        with_temp_home(|| {
            write_tool_manifest(
                "upload-mock",
                r#"{
                    "id":"upload-mock","name":"Upload Mock","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["--custom-marker"],
                    "servers":[]
                }"#,
            );
            let mgr = MarketplaceManager::with_store(MemoryCredentialStore::default());
            mgr.install_upload(
                "upload-mock",
                store::BundleSource::Upload("pkg.zip".to_string()),
            )
            .unwrap();

            let mcp = read_mcp_json();
            assert_eq!(
                mcp["servers"]["upload-mock"]["args"],
                serde_json::json!(["--custom-marker"]),
                "上传包安装必须读取包目录 manifest 的自定义内容"
            );
        });
    }

    /// 真实预置契约回归（PR #547）：catalog 工具的 secret 声明同样来自编译期
    /// manifest——内嵌 patsnap-search 安装按内嵌 secret_headers 注册
    /// bearer_token_env_var，用户经 user_config 传入的 Token 落 keyring 与进程内
    /// 注册表，明文不落盘。
    #[test]
    fn install_embedded_preset_registers_secrets_from_embedded_manifest() {
        with_temp_home(|| {
            let store = MemoryCredentialStore::default();
            let mut config = std::collections::HashMap::new();
            config.insert("PATSNAP_API_KEY".to_string(), "embedded-token".to_string());
            let mgr = MarketplaceManager::with_store(store.clone());
            mgr.install("patsnap-search", &config).unwrap();

            let mcp = read_mcp_json();
            assert_eq!(
                mcp["servers"]["patsnap-search"]["bearer_token_env_var"],
                "PINVOU3_MCP_SECRET_PATSNAP_API_KEY"
            );
            assert_eq!(
                store
                    .get(&mcp_secret_reference(
                        "patsnap-search",
                        "header",
                        "PATSNAP_API_KEY"
                    ))
                    .unwrap()
                    .as_deref(),
                Some("embedded-token")
            );
            assert_eq!(
                secrets::resolve_registered_secret("PINVOU3_MCP_SECRET_PATSNAP_API_KEY").as_deref(),
                Some("embedded-token")
            );
            assert!(!mcp.to_string().contains("embedded-token"));
        });
    }

    /// Windows fail-closed 策略回归（PR #547）：商店路径（Preset 来源）的未受信
    /// 依赖声明在 Windows 上显式报错；非 Windows 维持 warn-skip；Upload 来源任何
    /// 平台都不因此报错。纯函数拆参使该策略可在非 Windows 开发机上直接验证。
    #[test]
    fn untrusted_preset_deps_error_policy() {
        let error = untrusted_preset_deps_error("tdoc-mock", &store::BundleSource::Preset, true)
            .expect("Windows + Preset 必须对未受信依赖声明 fail-closed");
        assert!(error.contains("可验证依赖锁"), "unexpected error: {error}");
        assert!(
            untrusted_preset_deps_error("tdoc-mock", &store::BundleSource::Preset, false).is_none()
        );
        assert!(
            untrusted_preset_deps_error(
                "tdoc-mock",
                &store::BundleSource::Upload("pkg.zip".to_string()),
                true
            )
            .is_none()
        );
    }

    #[test]
    fn uninstall_patsnap_does_not_remove_other_connector_secrets() {
        // Also covers the single-connector uninstall contract (originally
        // uninstall_remote_secret_header_removes_credential_and_env):
        // uninstalling must delete the server entry, the credential
        // reference, and the registry value; and additionally asserts that
        // another connector's (qcc) credentials and registry values are
        // unaffected.
        // fixture 用非内嵌目录 id：内嵌工具的安装只认编译期 manifest，磁盘
        // fixture 会被忽略（信任边界见 install_catalog_tool_ignores_tampered_
        // disk_manifest）；本测试验证磁盘 manifest 安装路径的卸载隔离。
        with_temp_home(|| {
            write_tool_manifest(
                "patsnap-mock",
                r#"{
                    "id":"patsnap-mock","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"PATSNAP_API_KEY","provider":"patsnap","required":true}],
                    "servers":[{"name":"patsnap-search","url":"https://connect.zhihuiya.com/2b0355/logic-mcp"}]
                }"#,
            );
            write_tool_manifest(
                "qcc-mock",
                r#"{
                    "id":"qcc-mock","name":"QCC","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"QCC_API_KEY","provider":"qcc","required":true}],
                    "servers":[{"name":"qcc-company","url":"https://example.invalid/mcp"}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let patsnap_secret = secret_value("patsnap-isolated");
            let qcc_secret = secret_value("qcc-isolated");
            let patsnap_ref = mcp_secret_reference("patsnap-mock", "header", "PATSNAP_API_KEY");
            let qcc_ref = mcp_secret_reference("qcc-mock", "header", "QCC_API_KEY");
            store.set(&patsnap_ref, &patsnap_secret).unwrap();
            store.set(&qcc_ref, &qcc_secret).unwrap();
            let mgr = MarketplaceManager::with_store(store.clone());

            mgr.install("patsnap-mock", &std::collections::HashMap::new())
                .unwrap();
            mgr.install("qcc-mock", &std::collections::HashMap::new())
                .unwrap();
            assert_eq!(
                secrets::resolve_registered_secret("PINVOU3_MCP_SECRET_QCC_API_KEY").as_deref(),
                Some(qcc_secret.as_str())
            );

            mgr.uninstall("patsnap-mock").unwrap();

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
        // fixture 用非内嵌目录 id（"patsnap-mock"）：内嵌工具安装只认编译期
        // manifest（真实端点 URL），mock server 契约会失效——信任边界由
        // install_catalog_tool_ignores_tampered_disk_manifest 单独覆盖。
        with_temp_home_async(|| async {
            let mock = spawn_mock_mcp_server("valid-token").await;
            write_tool_manifest(
                "patsnap-mock",
                &format!(
                    r#"{{
                    "id":"patsnap-mock","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
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

            mgr.install("patsnap-mock", &config).unwrap();
            let err = mgr
                .validate_remote_connection("patsnap-mock")
                .await
                .unwrap_err();
            mgr.uninstall("patsnap-mock").unwrap();

            assert!(err.contains("API Key 无效"), "unexpected error: {err}");
            assert!(!err.contains("无法连接远程 MCP 服务"));
            assert!(!mgr.installed_ids().contains(&"patsnap-mock".to_string()));
            let mcp = read_mcp_json();
            assert!(mcp["servers"].get("patsnap-search").is_none());
            assert_eq!(store
                .get(&mcp_secret_reference(
                    "patsnap-mock",
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
        // 同上：非内嵌目录 id 保证 mock URL（而非内嵌真实端点）进入 mcp.json。
        with_temp_home_async(|| async {
            let mock = spawn_mock_mcp_server("valid-token").await;
            write_tool_manifest(
                "patsnap-mock",
                &format!(
                    r#"{{
                    "id":"patsnap-mock","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
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

            mgr.install("patsnap-mock", &config).unwrap();
            // Ok(()) means validated: the handshake succeeded, tools were non-empty, and every tool the manifest
            // expects (patsnap_search / patsnap_fetch) was discovered — missing any one yields Err.
            // The old return value carried the tool details; now the real tools/list call is observed
            // via the methods the mock received.
            mgr.validate_remote_connection("patsnap-mock")
                .await
                .unwrap();

            assert!(mgr.installed_ids().contains(&"patsnap-mock".to_string()));
            let mcp = read_mcp_json();
            assert_eq!(
                mcp["servers"]["patsnap-search"]["bearer_token_env_var"],
                "PINVOU3_MCP_SECRET_PATSNAP_API_KEY"
            );
            assert!(!mcp.to_string().contains("valid-token"));
            assert_eq!(
                store
                    .get(&mcp_secret_reference(
                        "patsnap-mock",
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
                "weather-mock",
                r#"{
                    "id":"weather-mock","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}]
                }"#,
            );
            let mgr = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let err = mgr
                .install("weather-mock", &std::collections::HashMap::new())
                .unwrap_err();

            assert!(err.contains("AMAP_KEY"), "错误应提示缺少 AMAP_KEY: {err}");
            assert!(
                !mgr.installed_ids().contains(&"weather-mock".to_string()),
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

            mgr.migrate_mcp_plaintext_secrets().unwrap();

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
            // Migration observation (replacement for the old McpSecretMigrationResult count assertions): the AMAP_KEY key
            // is removed from env wholesale, the keyless value is kept, and exactly one migration is persisted.
            assert!(
                !content.contains("AMAP_KEY"),
                "migration should remove the AMAP_KEY key from env wholesale: {content}"
            );
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

            mgr.migrate_mcp_plaintext_secrets().unwrap();

            let stored = store
                .get(&mcp_secret_reference("qcc", "header", "QCC_API_KEY"))
                .unwrap();
            assert_eq!(stored.as_deref(), Some(secret.as_str()));
            let content = std::fs::read_to_string(&mcp_path).unwrap();
            assert!(!content.contains(&secret));
            // Migration observation: the plaintext Authorization header is removed wholesale and rewritten to env-var wiring.
            assert!(
                !content.contains("Authorization"),
                "migration should remove the plaintext Authorization header: {content}"
            );
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

            mgr.migrate_mcp_plaintext_secrets().unwrap();

            // Skip observation: the existing credential keeps its original value, the file plaintext is cleaned up and rewritten to
            // env-var wiring (file plaintext cleared but the store not touched, i.e. the "skip override" branch).
            let stored = store
                .get(&mcp_secret_reference("qcc", "header", "QCC_API_KEY"))
                .unwrap();
            assert_eq!(stored.as_deref(), Some(kept_secret.as_str()));
            let content = std::fs::read_to_string(&mcp_path).unwrap();
            assert!(!content.contains(&old_secret));
            assert!(!content.contains(&kept_secret));
            assert!(
                !content.contains("Authorization"),
                "skip-override migration should also clean the plaintext header: {content}"
            );
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

    /// Upload 来源的包卸载不再原位保留（旧语义会让卡片以"未安装"重现），改为整包
    /// 搬入回收站：`bundles/<id>/` 搬空、`available_tools` 不再出现、回收清单有记录。
    /// 来源查询必须先于 `store.remove` —— 此前先删登记再查恒 false，上传包目录被
    /// 误删（三轮评审数据丢失 bug）；fail-closed 口径不变（六轮评审 R1）。
    #[test]
    fn uninstall_upload_source_recycles_package_dir() {
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
                !crate::platform::paths::bundles_root()
                    .join("up-tool")
                    .exists(),
                "Upload 卸载后包目录应搬离 bundles_root"
            );
            assert!(
                !mgr.available_tools().iter().any(|t| t.id == "up-tool"),
                "搬离后 available_tools 不应再出现该包"
            );
            let recycled = recycle_bin::RecycleBin::new().list().unwrap();
            assert_eq!(recycled.len(), 1, "回收清单应有记录");
            assert_eq!(recycled[0].id, "up-tool");
            assert_eq!(recycled[0].display_name, "pkg.zip");
            assert_eq!(recycled[0].kind, recycle_bin::KIND_MCP);
            assert!(
                crate::platform::paths::pinvou3_home()
                    .join("marketplace/recycle-bin/up-tool/mcp/manifest.json")
                    .is_file(),
                "包内容应完整保留在回收站"
            );
        });
    }

    /// Upload 组合包（mcp/ + skills/）卸载：整包回收，mcp/ 与 skills/ 完整保留在
    /// 回收站；命令层随后联动卸载 companion 技能时走 recycle-aware（目录已随包
    /// 回收，只清登记、不报错不误删）。
    #[test]
    fn uninstall_upload_bundle_recycles_mcp_and_skills() {
        with_temp_home(|| {
            write_tool_manifest(
                "up-bundle",
                r#"{
                    "id":"up-bundle","name":"UpBundle","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"],
                    "companion_skills":["up-bundle-skill"]
                }"#,
            );
            let skill_dir =
                crate::platform::paths::bundles_root().join("up-bundle/skills/up-bundle-skill");
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                "---\nname: up-bundle-skill\n---\n",
            )
            .unwrap();
            let mgr = MarketplaceManager::new();
            mgr.install_upload(
                "up-bundle",
                store::BundleSource::Upload("bundle.zip".to_string()),
            )
            .unwrap();

            mgr.uninstall("up-bundle").unwrap();

            let recycle_root =
                crate::platform::paths::pinvou3_home().join("marketplace/recycle-bin/up-bundle");
            assert!(
                recycle_root.join("mcp/manifest.json").is_file(),
                "mcp/ 应完整保留在回收站"
            );
            assert!(
                recycle_root
                    .join("skills/up-bundle-skill/SKILL.md")
                    .is_file(),
                "skills/ 应完整保留在回收站"
            );
            let recycled = recycle_bin::RecycleBin::new().list().unwrap();
            assert_eq!(recycled[0].kind, recycle_bin::KIND_BUNDLE);

            // companion 联动卸载（commands::uninstall_marketplace_tool_sync 同路径）：
            // 包目录已不在 bundles_root，recycle-aware 只清登记、返回 Ok。
            crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
                .uninstall("up-bundle-skill")
                .expect("companion 技能随包回收后卸载应 recycle-aware 成功");
        });
    }

    /// 启动修复降级（mark_tool_uninstalled_locked，永久无效依赖的自愈卸载）与
    /// 普通卸载同语义：Upload 整包搬入回收站，companion 技能目录随整包搬离、
    /// 不被物理删除。此前修复路径漏接回收站 —— cleanup 对 companion 走技能卸载
    /// 的候选目录物理删除，销毁用户唯一副本的 skills 部分（review P1）。
    #[test]
    fn startup_repair_uninstall_recycles_upload_package() {
        with_temp_home(|| {
            write_tool_manifest(
                "up-repair",
                r#"{
                    "id":"up-repair","name":"UpRepair","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"],
                    "companion_skills":["up-repair-comp"]
                }"#,
            );
            let skill_dir =
                crate::platform::paths::bundles_root().join("up-repair/skills/up-repair-comp");
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                "---\nname: up-repair-comp\n---\n",
            )
            .unwrap();
            let mgr = MarketplaceManager::new();
            mgr.install_upload(
                "up-repair",
                store::BundleSource::Upload("repair.zip".to_string()),
            )
            .unwrap();

            mgr.mark_tool_uninstalled_locked("up-repair").unwrap();

            assert!(
                !mgr.installed_ids().contains(&"up-repair".to_string()),
                "修复降级应移除 installed.json 登记"
            );
            assert!(
                !crate::platform::paths::bundles_root()
                    .join("up-repair")
                    .exists(),
                "Upload 包目录应随修复降级搬离 bundles_root"
            );
            let recycle_root =
                crate::platform::paths::pinvou3_home().join("marketplace/recycle-bin/up-repair");
            assert!(
                recycle_root.join("mcp/manifest.json").is_file(),
                "mcp/ 应完整保留在回收站"
            );
            assert!(
                recycle_root
                    .join("skills/up-repair-comp/SKILL.md")
                    .is_file(),
                "companion 技能应随整包保留在回收站，不得被物理删除"
            );
            let recycled = recycle_bin::RecycleBin::new().list().unwrap();
            assert_eq!(recycled.len(), 1, "回收清单应有记录");
            assert_eq!(recycled[0].kind, recycle_bin::KIND_BUNDLE);
            assert_eq!(recycled[0].display_name, "repair.zip");
        });
    }

    /// 恢复管线（无 secrets 的 Upload MCP 包）：卸载进回收站 → restore 搬回
    /// `bundles/<id>/` → 登记重建（source=Upload、保留原 installed_at）→ MCP 重新
    /// 供给（installed.json + mcp.json）→ 回收清单清空。
    #[test]
    fn restore_recycled_upload_package_reinstalls_mcp() {
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
            let first_installed_at = store::BundleStore::new()
                .get("up-re")
                .unwrap()
                .unwrap()
                .installed_at;

            mgr.uninstall("up-re").unwrap();
            assert!(
                !crate::platform::paths::bundles_root()
                    .join("up-re")
                    .exists()
            );

            let result = recycle_bin::restore_plugin("up-re").unwrap();
            assert!(
                !result.credentials_required,
                "无 secrets 的包不应要求重填凭据"
            );

            assert!(
                crate::platform::paths::bundles_root()
                    .join("up-re/mcp/manifest.json")
                    .is_file(),
                "恢复后目录应回到 bundles/<id>/"
            );
            assert!(
                mgr.installed_ids().contains(&"up-re".to_string()),
                "MCP 应重新供给到 installed.json"
            );
            assert!(
                read_mcp_json()["servers"].get("up-re").is_some(),
                "MCP 应重新写回 mcp.json"
            );
            let record = store::BundleStore::new()
                .get("up-re")
                .unwrap()
                .expect("登记应重建");
            assert!(record.installed);
            assert_eq!(
                record.source,
                store::BundleSource::Upload("pkg.zip".to_string())
            );
            assert_eq!(
                record.installed_at, first_installed_at,
                "原 installed_at 应保留"
            );
            assert!(recycle_bin::RecycleBin::new().list().unwrap().is_empty());
        });
    }

    /// 恢复管线（声明 secrets 的 Upload MCP 包）：凭据卸载时已删，install 缺凭据
    /// 必失败 —— restore 跳过 MCP 供给、恢复目录与登记，返回 credentials_required
    /// 提示前端引导重填（重填走 install 幂等补齐 mcp.json/installed.json）。
    #[test]
    fn restore_recycled_package_with_secrets_flags_credentials_required() {
        with_temp_home(|| {
            write_tool_manifest(
                "up-secret",
                r#"{
                    "id":"up-secret","name":"UpSecret","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"UP_SECRET_KEY","provider":"up","required":true}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            store
                .set(
                    &mcp_secret_reference("up-secret", "env", "UP_SECRET_KEY"),
                    &secret_value("up"),
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store.clone());
            mgr.install_upload(
                "up-secret",
                store::BundleSource::Upload("secret.zip".to_string()),
            )
            .unwrap();

            mgr.uninstall("up-secret").unwrap();
            assert_eq!(
                store
                    .get(&mcp_secret_reference("up-secret", "env", "UP_SECRET_KEY"))
                    .unwrap(),
                None,
                "卸载应删除 secrets"
            );

            let result = recycle_bin::restore_plugin("up-secret").unwrap();
            assert!(
                result.credentials_required,
                "声明 secrets 的包恢复应提示重填凭据"
            );
            assert!(
                crate::platform::paths::bundles_root()
                    .join("up-secret/mcp/manifest.json")
                    .is_file(),
                "目录应搬回"
            );
            let record = store::BundleStore::new()
                .get("up-secret")
                .unwrap()
                .expect("登记应重建");
            assert!(record.installed, "登记应恢复为已安装");
            assert!(
                !MarketplaceManager::new()
                    .installed_ids()
                    .contains(&"up-secret".to_string()),
                "缺凭据时跳过 MCP 供给（不写 installed.json，避免半安装态）"
            );
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

    /// M2：Upload 包卸载先做回收 preflight —— 回收站目标被同 id 残留占用时，
    /// 卸载在拆任何供给面之前 fail loud：installed.json / mcp.json / keyring
    /// secrets / bundles.json 登记 / 包目录全部原样保留（此前回收失败只回写
    /// bundles.json，造成「显示已安装、实际未供给」）。
    #[test]
    fn uninstall_upload_recycle_preflight_failure_leaves_everything_untouched() {
        with_temp_home(|| {
            write_tool_manifest(
                "up-guard",
                r#"{
                    "id":"up-guard","name":"UpGuard","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"UP_GUARD_KEY","provider":"up","required":true}]
                }"#,
            );
            let cred = MemoryCredentialStore::default();
            cred.set(
                &mcp_secret_reference("up-guard", "env", "UP_GUARD_KEY"),
                &secret_value("up"),
            )
            .unwrap();
            let mgr = MarketplaceManager::with_store(cred.clone());
            mgr.install_upload(
                "up-guard",
                store::BundleSource::Upload("guard.zip".to_string()),
            )
            .unwrap();
            // 同 id 回收站目标残留 → preflight 拒绝覆盖。
            let occupied =
                crate::platform::paths::pinvou3_home().join("marketplace/recycle-bin/up-guard");
            std::fs::create_dir_all(&occupied).unwrap();

            let err = mgr.uninstall("up-guard").unwrap_err();
            assert!(err.contains("回收站"), "报错应指向回收站不可用: {err}");

            assert!(
                mgr.installed_ids().contains(&"up-guard".to_string()),
                "installed.json 应原样保留"
            );
            assert!(
                read_mcp_json()["servers"].get("up-guard").is_some(),
                "mcp.json 供给条目应原样保留"
            );
            assert!(
                cred.get(&mcp_secret_reference("up-guard", "env", "UP_GUARD_KEY"))
                    .unwrap()
                    .is_some(),
                "secrets 应原样保留（回收失败不删凭据）"
            );
            assert!(
                store::BundleStore::new().get("up-guard").unwrap().is_some(),
                "bundles.json 登记应原样保留"
            );
            assert!(
                crate::platform::paths::bundles_root()
                    .join("up-guard/mcp/manifest.json")
                    .is_file(),
                "包目录应原样保留在 bundles_root"
            );
            assert!(
                recycle_bin::RecycleBin::new().list().unwrap().is_empty(),
                "不得产生孤儿清单条目"
            );
            // install 期间 resolve_secret_placeholder 灌进进程 env 的占位变量手动清理
            // （with_temp_home 只恢复固定几个，本用例因卸载中止不会走 secrets 删除）。
            unsafe { std::env::remove_var("PINVOU3_MCP_SECRET_UP_GUARD_KEY") };
        });
    }

    /// M2 并发守护：同一 Upload 包卸载成功后再次卸载（并发后到者经
    /// MARKETPLACE_TRANSACTION_LOCK 串行化后的形态）—— 不得复活 bundles.json
    /// 登记 / 供给面，返回 Ok 且回收站清单不变。
    #[test]
    fn uninstall_already_recycled_upload_package_does_not_resurrect_record() {
        with_temp_home(|| {
            write_tool_manifest(
                "up-twice",
                r#"{
                    "id":"up-twice","name":"UpTwice","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"]
                }"#,
            );
            let mgr = MarketplaceManager::new();
            mgr.install_upload(
                "up-twice",
                store::BundleSource::Upload("twice.zip".to_string()),
            )
            .unwrap();
            mgr.uninstall("up-twice").unwrap();
            assert_eq!(recycle_bin::RecycleBin::new().list().unwrap().len(), 1);

            // 并发后到者（锁串行后的第二次调用）：登记已被先到者删除并回收。
            mgr.uninstall("up-twice").unwrap();

            assert!(
                store::BundleStore::new().get("up-twice").unwrap().is_none(),
                "不得复活 bundles.json 登记"
            );
            assert!(
                !mgr.installed_ids().contains(&"up-twice".to_string()),
                "不得复活 installed.json 登记"
            );
            assert!(
                read_mcp_json()["servers"].get("up-twice").is_none(),
                "不得复活 mcp.json 供给条目"
            );
            assert_eq!(
                recycle_bin::RecycleBin::new().list().unwrap().len(),
                1,
                "不得重复回收/改写清单"
            );
        });
    }

    /// M4：list_tools 的 source 填充 —— Upload 登记 = "upload"（卸载进回收站），
    /// Preset 登记（市场预置/手写自定义 MCP 迁移）= "preset"（卸载保留目录），
    /// 无记录的内置市场条目 = "builtin"。前端据此区分卸载文案，不再把自定义 MCP
    /// 卸载谎报为「已移入回收站」。
    #[test]
    fn list_tools_fills_source_from_bundle_records() {
        with_temp_home(|| {
            write_tool_manifest(
                "up-src",
                r#"{
                    "id":"up-src","name":"UpSrc","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"]
                }"#,
            );
            write_tool_manifest(
                "custom-src",
                r#"{
                    "id":"custom-src","name":"CustomSrc","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"python","args":["server.py"]
                }"#,
            );
            let mgr = MarketplaceManager::new();
            mgr.install_upload("up-src", store::BundleSource::Upload("src.zip".to_string()))
                .unwrap();
            // 无 plugin.json 的自定义包：install 镜像登记为 Preset
            mgr.install("custom-src", &std::collections::HashMap::new())
                .unwrap();

            let tools = mgr.list_tools();
            let source_of = |id: &str| tools.iter().find(|t| t.id == id).map(|t| t.source.as_str());
            assert_eq!(source_of("up-src"), Some("upload"), "Upload 登记 → upload");
            assert_eq!(
                source_of("custom-src"),
                Some("preset"),
                "Preset 登记（自定义迁移 MCP）→ preset"
            );
            assert_eq!(
                source_of("obsidian"),
                Some("builtin"),
                "无记录的内置市场条目 → builtin"
            );
            // exportable 与 export_installed_plugin 的拒绝口径一致：预置目录包
            // 不可导出（zip 无法重新导入），上传包 / 迁移登记的手写自定义 MCP 可导出。
            let exportable_of = |id: &str| tools.iter().find(|t| t.id == id).map(|t| t.exportable);
            assert!(!exportable_of("obsidian").unwrap(), "预置目录包 → 不可导出");
            assert!(exportable_of("up-src").unwrap(), "上传包 → 可导出");
            assert!(
                exportable_of("custom-src").unwrap(),
                "迁移登记的手写自定义 MCP → 可导出"
            );

            // serde 契约：字段名 `source`，取值为三值字符串。
            let value =
                serde_json::to_value(tools.iter().find(|t| t.id == "up-src").unwrap()).unwrap();
            assert_eq!(value["source"], serde_json::json!("upload"));
            assert_eq!(value["exportable"], serde_json::json!(true));
        });
    }

    /// M4 兼容：旧数据缺 source 字段时反序列化默认 "builtin"（前端按非上传
    /// 处理 —— 宁可少提示「移入回收站」，不说谎）。
    #[test]
    fn tool_info_source_defaults_to_builtin_when_absent() {
        let info: MarketplaceToolInfo = serde_json::from_str(
            r#"{"id":"x","name":"n","description":"d","version":"1","installed":false}"#,
        )
        .unwrap();
        assert_eq!(info.source, "builtin");
        assert!(
            info.exportable,
            "旧数据缺 exportable 字段时默认可导出（与导入/回收站导出通道既有行为一致）"
        );
    }

    /// Credential store whose reads fail until the flag is cleared: models a
    /// transiently locked keyring at early boot. Only `get` is flaky — the
    /// degrade/retry distinction under test is about reads.
    struct FlakyThenHealedStore {
        inner: MemoryCredentialStore,
        fail_reads: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl CredentialStore for FlakyThenHealedStore {
        fn get(&self, reference: &CredentialReference) -> Result<Option<String>, CredentialError> {
            if self.fail_reads.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(CredentialError::new("keyring unavailable (simulated)"));
            }
            self.inner.get(reference)
        }
        fn set(&self, reference: &CredentialReference, value: &str) -> Result<(), CredentialError> {
            self.inner.set(reference, value)
        }
        fn delete(&self, reference: &CredentialReference) -> Result<(), CredentialError> {
            self.inner.delete(reference)
        }
    }

    /// A credential-store read failure during a restore must NOT be degraded
    /// into a success action backed by an entry without its credential wiring:
    /// the restore is skipped (and retried on the next startup), and once the
    /// store recovers the same startup reconcile restores the full entry
    /// including the wiring. Baking the transient fault in would be permanent —
    /// healthy entries are never re-examined.
    #[test]
    fn restore_retries_on_credential_store_failure_and_heals_next_startup() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"st-x","name":"st-x","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"st-remote","url":"https://st.example.com/mcp"}],
                "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"ST_KEY","provider":"st","required":true}]
            });
            write_tool_manifest("st-x", &serde_json::to_string_pretty(&manifest).unwrap());
            write_installed_ids(&["st-x".to_string()]);
            let fail_reads = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let manager = MarketplaceManager::with_store(FlakyThenHealedStore {
                inner: MemoryCredentialStore::default(),
                fail_reads: fail_reads.clone(),
            });
            manager
                .credential_store
                .set(
                    &mcp_secret_reference("st-x", "header", "ST_KEY"),
                    "stored-token",
                )
                .unwrap();

            // Phase 1: locked keyring at startup — the tool is skipped with a
            // note, zero writes (mcp.json may not even exist yet).
            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 1, "{actions:?}");
            assert!(
                actions[0].contains("remote entry reconciliation skipped")
                    && actions[0].contains("inaccessible"),
                "a store failure must surface as a skip note: {actions:?}"
            );
            let servers = std::fs::read_to_string(paths::mcp_config_path())
                .ok()
                .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
                .and_then(|mcp| mcp["servers"].as_object().cloned())
                .unwrap_or_default();
            assert!(
                !servers.contains_key("st-remote"),
                "a store failure must not write an unwired entry: {servers:?}"
            );

            // Phase 2: the keyring recovered — the next startup heals the entry
            // with its credential wiring.
            fail_reads.store(false, std::sync::atomic::Ordering::SeqCst);
            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions,
                vec!["tool 'st-x': restored missing remote entry 'st-remote'".to_string()]
            );
            let entry = &read_mcp_json()["servers"]["st-remote"];
            assert_eq!(
                entry["bearer_token_env_var"],
                serde_json::Value::String(mcp_secret_env_var("ST_KEY")),
                "the healed store must yield full credential wiring: {entry}"
            );
        });
    }

    /// A local tool whose secret lives in a secret `config_fields` entry
    /// (target `env`) must get that channel re-derived from the credential
    /// store on a startup rebuild — it is the last channel without store
    /// re-derivation, and the rebuild's empty user config would otherwise
    /// silently drop the wiring.
    #[test]
    fn reconcile_restores_secret_config_field_from_credential_store() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"cfe-x","name":"cfe-x","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"python","args":["server.py"],
                "config_fields":[
                    {"key":"CFE_API_KEY","label":"key","required":true,"target":"env","secret":true}
                ]
            });
            write_tool_manifest("cfe-x", &serde_json::to_string_pretty(&manifest).unwrap());
            std::fs::write(
                mcp_catalog::package_mcp_dir("cfe-x").join("server.py"),
                "print('fixture')\n",
            )
            .unwrap();
            write_installed_ids(&["cfe-x".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            manager
                .credential_store
                .set(
                    &mcp_secret_reference("cfe-x", "env", "CFE_API_KEY"),
                    "stored-env-token",
                )
                .unwrap();

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions,
                vec!["tool 'cfe-x': restored missing mcp.json entry".to_string()]
            );
            let entry = &read_mcp_json()["servers"]["cfe-x"];
            assert_eq!(
                entry["env"]["CFE_API_KEY"],
                serde_json::Value::String("${PINVOU3_MCP_SECRET_CFE_API_KEY}".to_string()),
                "the secret config_field channel must be re-derived from the store: {entry}"
            );

            // Idempotent: the second run writes nothing.
            let mcp_path = paths::mcp_config_path();
            let before = std::fs::read(&mcp_path).unwrap();
            assert!(
                manager
                    .reconcile_installed_mcp_entries()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// An unparseable mcp.json must not be reset by the reconcile's own
    /// writers: the file is backed up, every entry is left untouched, and the
    /// outcome lands in the action list. Resetting here would destroy custom
    /// entries and preserved user fields — the exact parse-failure drift this
    /// reconcile exists to heal.
    #[test]
    fn reconcile_unparseable_mcp_json_is_backed_up_and_untouched() {
        with_temp_home(|| {
            write_local_tool_fixture("corrupt-x", false);
            write_local_tool_fixture("corrupt-y", false);
            write_installed_ids(&["corrupt-x".to_string(), "corrupt-y".to_string()]);
            let mcp_path = paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            let corrupt = r#"{"servers": {,"trailing":"comma"}"#;
            std::fs::write(&mcp_path, corrupt).unwrap();

            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions.len(),
                1,
                "one boot-level note, not one note per tool: {actions:?}"
            );
            assert!(
                actions[0].contains("mcp.json is unparseable") && actions[0].contains("backed up"),
                "{actions:?}"
            );

            // The corrupt file survives byte-for-byte and a backup copy exists;
            // nothing may be written onto a reset skeleton.
            assert_eq!(std::fs::read(&mcp_path).unwrap(), corrupt.as_bytes());
            let backup = std::fs::read_dir(mcp_path.parent().unwrap())
                .unwrap()
                .filter_map(Result::ok)
                .find(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("mcp.json.corrupt.")
                })
                .expect("a corrupt-file backup must exist");
            assert_eq!(std::fs::read(backup.path()).unwrap(), corrupt.as_bytes());
        });
    }

    /// A local manifest with an empty command can never launch (the healthy
    /// check requires a non-empty command), so the rebuild must refuse instead
    /// of rewriting the identical dead entry on every startup.
    #[test]
    fn reconcile_refuses_empty_manifest_command() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"empty-x","name":"empty-x","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[]
            });
            write_tool_manifest("empty-x", &serde_json::to_string_pretty(&manifest).unwrap());
            write_installed_ids(&["empty-x".to_string()]);
            seed_mcp_json(serde_json::json!({ "empty-x": { "command": "" } }));
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 1, "{actions:?}");
            assert!(
                actions[0].contains("not rebuilt")
                    && actions[0].contains("manifest command is empty"),
                "{actions:?}"
            );
            // Convergence: the dead entry is left untouched, not rewritten.
            let mcp_path = paths::mcp_config_path();
            let before = std::fs::read(&mcp_path).unwrap();
            assert_eq!(manager.reconcile_installed_mcp_entries().unwrap().len(), 1);
            assert_eq!(std::fs::read(&mcp_path).unwrap(), before);
        });
    }

    /// Credential store modeling the OS-keyring-unavailable state: reads are
    /// served by the file fallback (a plain miss, `Ok(None)`), and
    /// `os_keyring_unreachable` reports the fallback while the flag is set.
    /// A miss under fallback may be a credential sitting in the unreachable
    /// keyring, so it must be classified as a store read failure, not as an
    /// absent credential.
    struct FallbackKeyringStore {
        inner: MemoryCredentialStore,
        fallback_active: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl CredentialStore for FallbackKeyringStore {
        fn get(&self, reference: &CredentialReference) -> Result<Option<String>, CredentialError> {
            self.inner.get(reference)
        }
        fn set(&self, reference: &CredentialReference, value: &str) -> Result<(), CredentialError> {
            self.inner.set(reference, value)
        }
        fn delete(&self, reference: &CredentialReference) -> Result<(), CredentialError> {
            self.inner.delete(reference)
        }
        fn os_keyring_unreachable(&self, _reference: &CredentialReference) -> bool {
            self.fallback_active
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// The probe-fallback regression: when the OS keyring is unreachable at
    /// startup, a stored credential reads as a plain miss through the file
    /// fallback. Classifying that miss as "absent" would write an unwired
    /// entry that no later startup repairs (the remote matcher ignores
    /// credential fields); it must skip the restore instead, and the next
    /// startup — keyring reachable again — heals the entry with full wiring.
    #[test]
    fn restore_retries_while_os_keyring_falls_back_and_heals_when_reachable() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"fb-x","name":"fb-x","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"fb-remote","url":"https://fb.example.com/mcp"}],
                "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"FB_KEY","provider":"fb","required":true}]
            });
            write_tool_manifest("fb-x", &serde_json::to_string_pretty(&manifest).unwrap());
            write_installed_ids(&["fb-x".to_string()]);
            let fallback_active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let manager = MarketplaceManager::with_store(FallbackKeyringStore {
                inner: MemoryCredentialStore::default(),
                fallback_active: fallback_active.clone(),
            });

            // Phase 1: keyring unreachable at startup — the credential lives
            // in the OS keyring, so the file fallback reads a plain miss.
            // The tool must be skipped with a note, zero writes (mcp.json may
            // not even exist yet).
            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 1, "{actions:?}");
            assert!(
                actions[0].contains("reconciliation skipped"),
                "the fallback miss must fail the restore as a store error: {actions:?}"
            );
            assert!(
                !paths::mcp_config_path().exists(),
                "no unwired entry may be written while the keyring is unreachable"
            );

            // Phase 2: keyring reachable again (the credential is readable
            // through it) — the entry is restored with its full wiring.
            fallback_active.store(false, std::sync::atomic::Ordering::SeqCst);
            manager
                .credential_store
                .set(
                    &mcp_secret_reference("fb-x", "header", "FB_KEY"),
                    "stored-token",
                )
                .unwrap();
            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions,
                vec!["tool 'fb-x': restored missing remote entry 'fb-remote'".to_string()]
            );
            let entry = &read_mcp_json()["servers"]["fb-remote"];
            assert_eq!(
                entry["bearer_token_env_var"],
                serde_json::Value::String(mcp_secret_env_var("FB_KEY")),
                "the healed keyring must yield full credential wiring: {entry}"
            );
        });
    }

    /// An uninstall must refuse to rewrite an unparseable mcp.json: the boot
    /// retired-tool cleanup and the Python-repair downgrade both route through
    /// `uninstall`, and a reset there would destroy the file before (cleanup)
    /// or despite (downgrade) the reconcile's backup — the exact data loss the
    /// corrupt-file contract exists to prevent. The uninstall rolls back
    /// instead and the bytes survive untouched.
    #[test]
    fn uninstall_refuses_to_reset_a_corrupt_mcp_json() {
        with_temp_home(|| {
            write_local_tool_fixture("corrupt-u", false);
            write_installed_ids(&["corrupt-u".to_string()]);
            let mcp_path = paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            let corrupt = r#"{"servers": {,"trailing":"comma"}"#;
            std::fs::write(&mcp_path, corrupt).unwrap();

            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let error = manager.uninstall("corrupt-u").unwrap_err();
            assert!(
                error.contains("mcp.json is unparseable"),
                "the uninstall must refuse with an actionable error: {error}"
            );
            assert_eq!(
                std::fs::read(&mcp_path).unwrap(),
                corrupt.as_bytes(),
                "the uninstall must not touch the corrupt file"
            );
            let installed: Vec<String> = serde_json::from_str(
                &std::fs::read_to_string(
                    paths::pinvou3_home()
                        .join("marketplace")
                        .join("installed.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(
                installed,
                vec!["corrupt-u".to_string()],
                "the rolled-back uninstall must leave the registry unchanged"
            );
        });
    }

    /// The UI install writer must refuse an unparseable mcp.json the same way:
    /// an install that silently reset the file would destroy custom entries
    /// during exactly the window the timeline note tells the user to fix it.
    #[test]
    fn install_writer_refuses_to_reset_a_corrupt_mcp_json() {
        with_temp_home(|| {
            write_local_tool_fixture("corrupt-i", false);
            let mcp_path = paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            let corrupt = r#"{"servers": {,"trailing":"comma"}"#;
            std::fs::write(&mcp_path, corrupt).unwrap();

            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let manifest = manager.load_manifest("corrupt-i").unwrap();
            let error = manager
                .add_to_mcp_json(
                    &manifest,
                    &std::collections::HashMap::new(),
                    &mcp_catalog::package_mcp_dir("corrupt-i"),
                    None,
                )
                .unwrap_err();
            assert!(
                error.contains("mcp.json is unparseable"),
                "the install writer must refuse with an actionable error: {error}"
            );
            assert_eq!(
                std::fs::read(&mcp_path).unwrap(),
                corrupt.as_bytes(),
                "the install writer must not touch the corrupt file"
            );
        });
    }

    /// A rebuild re-derives env from the manifest and the credential store;
    /// install-time (or hand-edited) env values outside those sources are
    /// gone. The success note must say so — naming keys, never values —
    /// instead of reporting an unqualified success.
    #[test]
    fn rebuild_notes_dropped_install_time_env_keys() {
        with_temp_home(|| {
            write_local_tool_fixture("env-x", false);
            write_installed_ids(&["env-x".to_string()]);
            // A nonexistent target under the test home stays nonexistent on
            // every platform, so dead-target handling is identical on Windows.
            let dead_command = paths::pinvou3_home().join("x").join("w.py");
            seed_mcp_json(serde_json::json!({
                "env-x": {
                    "command": dead_command,
                    "args": [],
                    "env": {"MY_INSTALL_TIME_VAR": "install-input", "KEEP_MANIFEST_VAR": "m"}
                }
            }));
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 1, "{actions:?}");
            assert!(
                actions[0].contains("rebuilt mcp.json entry")
                    && actions[0].contains("MY_INSTALL_TIME_VAR")
                    && actions[0].contains("not preserved"),
                "the rebuild note must disclose the dropped env key: {actions:?}"
            );
            assert!(
                !actions[0].contains("install-input"),
                "the note must never carry the dropped value: {actions:?}"
            );
            let entry = &read_mcp_json()["servers"]["env-x"];
            assert!(
                entry["env"].get("MY_INSTALL_TIME_VAR").is_none(),
                "the dropped key must actually be gone: {entry}"
            );
        });
    }

    /// A restore that degraded a secret channel (no stored credential) must
    /// say so in its note instead of reporting an unqualified success that
    /// 401s on first use.
    #[test]
    fn restore_notes_secret_channels_restored_without_wiring() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"nc-x","name":"nc-x","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"nc-remote","url":"https://nc.example.com/mcp"}],
                "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"NC_KEY","provider":"nc","required":true}]
            });
            write_tool_manifest("nc-x", &serde_json::to_string_pretty(&manifest).unwrap());
            write_installed_ids(&["nc-x".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 1, "{actions:?}");
            assert!(
                actions[0].contains("restored missing remote entry 'nc-remote'")
                    && actions[0].contains("no stored credential for NC_KEY"),
                "the restore note must disclose the degraded channel: {actions:?}"
            );
            let entry = &read_mcp_json()["servers"]["nc-remote"];
            assert!(
                entry.get("bearer_token_env_var").is_none() && entry.get("env_headers").is_none(),
                "the degraded entry must carry no auth wiring: {entry}"
            );
        });
    }

    /// An OPTIONAL secret config field (the qcc shape) with no stored
    /// credential is a legitimate outcome, not a degraded restore: the note
    /// must not send an OAuth-only user hunting for a key they never needed
    /// — and under the fallback it would be a claim that cannot be verified
    /// either way.
    #[test]
    fn restore_does_not_note_absent_optional_config_field_credentials() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"opt-note","name":"opt-note","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"opt-note-remote","url":"https://opt-note.example.com/mcp"}],
                "config_fields":[
                    {"key":"OPT_NOTE_API_KEY","label":"k","required":false,"target":"bearer","secret":true}
                ]
            });
            write_tool_manifest(
                "opt-note",
                &serde_json::to_string_pretty(&manifest).unwrap(),
            );
            write_installed_ids(&["opt-note".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions,
                vec![
                    "tool 'opt-note': restored missing remote entry 'opt-note-remote'".to_string()
                ],
                "an absent optional credential is not a degraded restore: {actions:?}"
            );
            let entry = &read_mcp_json()["servers"]["opt-note-remote"];
            assert!(
                entry.get("bearer_token_env_var").is_none(),
                "the absent optional key leaves no auth wiring: {entry}"
            );
        });
    }

    /// Legacy remote manifests (sensitive `manifest.env` key, no secret
    /// channels) must install the env-var NAME wiring the engine resolves at
    /// request time (`bearer_token_env_var`) — not a literal `${...}`
    /// placeholder in `headers`, which the engine sends as-is and never
    /// expands.
    #[test]
    fn legacy_remote_env_key_installs_bearer_env_var_not_literal_header() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"lg-x","name":"lg-x","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"lg-remote","url":"https://lg.example.com/mcp"}],
                "env":{"VENDOR_API_KEY":"placeholder-in-manifest"}
            });
            write_tool_manifest("lg-x", &serde_json::to_string_pretty(&manifest).unwrap());
            write_installed_ids(&["lg-x".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(
                actions,
                vec!["tool 'lg-x': restored missing remote entry 'lg-remote'".to_string()]
            );
            let entry = &read_mcp_json()["servers"]["lg-remote"];
            assert_eq!(
                entry["bearer_token_env_var"],
                serde_json::Value::String(mcp_secret_env_var("VENDOR_API_KEY")),
                "the legacy channel must land in the resolved bearer env var: {entry}"
            );
            assert!(
                entry.get("headers").is_none(),
                "no literal ${{}} placeholder may be written into headers: {entry}"
            );
        });
    }

    /// The restart rehydration (`sync_secret_values`) clears and rebuilds the
    /// in-process registry from `manifest_secret_targets` alone. A legacy
    /// remote manifest's credential is stored under the Bearer target, so the
    /// sensitive-by-name `manifest.env` key must be enumerated there —
    /// otherwise the wiring installed by the reconcile dies at the first
    /// restart (entry keeps `bearer_token_env_var`, nothing resolves it:
    /// silent 401s).
    #[test]
    fn legacy_remote_env_key_survives_secret_values_resync() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"lg-rs","name":"lg-rs","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"lg-rs-remote","url":"https://lg-rs.example.com/mcp"}],
                "env":{"VENDOR_API_KEY":"placeholder-in-manifest"}
            });
            write_tool_manifest("lg-rs", &serde_json::to_string_pretty(&manifest).unwrap());
            write_installed_ids(&["lg-rs".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            manager.reconcile_installed_mcp_entries().unwrap();
            let env_var = mcp_secret_env_var("VENDOR_API_KEY");
            // The assert messages below deliberately carry no registry dump:
            // formatting secret-registry contents into the failure log is
            // exactly the cleartext-logging pattern CodeQL flags (high).
            assert!(
                snapshot_secret_values().contains_key(&env_var),
                "the reconcile must register the legacy secret"
            );

            // The boot runs this right after the reconcile (bridge.rs); it
            // must re-add what it wipes, not leave the registry empty. Plant a
            // registry entry no enumeration produces: the wipe half
            // (`values.clear()`) has to evict it, so a deleted `clear()` can
            // no longer survive this test as a silent no-op.
            let stale_canary = "PINVOU3_MCP_SECRET_STALE_RESYNC_CANARY".to_string();
            store_secret_value(stale_canary.clone(), "stale".to_string());
            manager.sync_secret_values().unwrap();
            assert!(
                !snapshot_secret_values().contains_key(&stale_canary),
                "the wipe half must evict entries the rebuild no longer derives"
            );
            assert_eq!(
                snapshot_secret_values().get(&env_var).map(String::as_str),
                Some("placeholder-in-manifest"),
                "the legacy secret must survive the restart rehydration"
            );
        });
    }

    /// Local legacy-only manifests keep their credential under the "env"
    /// target; the same rehydration must cover that channel (latent since the
    /// registry moved out of the process env).
    #[test]
    fn legacy_local_env_key_survives_secret_values_resync() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"lg-loc","name":"lg-loc","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"python","args":["server.py"],
                "env":{"LOCAL_API_KEY":"legacy-manifest-value"}
            });
            write_tool_manifest("lg-loc", &serde_json::to_string_pretty(&manifest).unwrap());
            std::fs::write(
                mcp_catalog::package_mcp_dir("lg-loc").join("server.py"),
                "print('fixture')\n",
            )
            .unwrap();
            write_installed_ids(&["lg-loc".to_string()]);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            manager.reconcile_installed_mcp_entries().unwrap();
            let env_var = mcp_secret_env_var("LOCAL_API_KEY");
            assert_eq!(
                snapshot_secret_values().get(&env_var).map(String::as_str),
                Some("legacy-manifest-value"),
                "the reconcile must register the local legacy secret"
            );

            manager.sync_secret_values().unwrap();
            assert_eq!(
                snapshot_secret_values().get(&env_var).map(String::as_str),
                Some("legacy-manifest-value"),
                "the local legacy secret must survive the restart rehydration"
            );
        });
    }

    /// Uninstalling (or permanently downgrading) a tool with sensitive-by-name
    /// legacy env keys must delete the credential the enumeration now covers —
    /// before the dual-target enumeration these store entries were orphaned
    /// forever and a recycle/restore silently kept working with stale auth.
    /// Covers both targets the writers use, plus the registry entry.
    #[test]
    fn uninstall_deletes_legacy_env_credentials() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"lg-del","name":"lg-del","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"lg-del-remote","url":"https://lg-del.example.com/mcp"}],
                "env":{"LEGACY_API_KEY":"legacy-value"}
            });
            write_tool_manifest("lg-del", &serde_json::to_string_pretty(&manifest).unwrap());
            write_installed_ids(&["lg-del".to_string()]);
            let store = MemoryCredentialStore::default();
            let manager = MarketplaceManager::with_store(store.clone());

            manager.reconcile_installed_mcp_entries().unwrap();
            let env_var = mcp_secret_env_var("LEGACY_API_KEY");
            assert!(
                store
                    .get(&mcp_secret_reference("lg-del", "header", "LEGACY_API_KEY"))
                    .unwrap()
                    .is_some(),
                "precondition: the remote legacy channel stores under the Bearer target"
            );
            // The remote reconcile never writes the "env" target for this key,
            // so seed it the way a local-channel writer would: without this,
            // the env-target deletion assertion below could never fail and
            // would pin nothing.
            store
                .set(
                    &mcp_secret_reference("lg-del", "env", "LEGACY_API_KEY"),
                    "legacy-value",
                )
                .unwrap();
            assert!(snapshot_secret_values().contains_key(&env_var));

            manager.uninstall("lg-del").unwrap();

            assert_eq!(
                store
                    .get(&mcp_secret_reference("lg-del", "header", "LEGACY_API_KEY"))
                    .unwrap(),
                None,
                "uninstall must delete the previously-orphaned legacy credential"
            );
            assert_eq!(
                store
                    .get(&mcp_secret_reference("lg-del", "env", "LEGACY_API_KEY"))
                    .unwrap(),
                None,
                "the seeded env-target credential must be deleted too, not only the Bearer one"
            );
            assert!(
                !snapshot_secret_values().contains_key(&env_var),
                "the registry entry must go with the store entry"
            );
        });
    }

    /// qcc introduced the first OPTIONAL (required: false) secret config
    /// field. Under the file-backed fallback a store miss is
    /// "undeterminable", and for a required field that fails closed — but an
    /// optional field must treat it the same as absent: OAuth carries the
    /// auth and an install must not block on an unreachable keyring (this
    /// regressed the keyring-less Linux rust-test job).
    #[test]
    fn optional_bearer_config_field_installs_without_a_keyring() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"opt-bearer","name":"opt-bearer","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"opt-bearer-remote","url":"https://opt.example.com/mcp"}],
                "config_fields":[
                    {"key":"OPT_BEARER_API_KEY","label":"k","required":false,"target":"bearer","secret":true}
                ]
            });
            write_tool_manifest(
                "opt-bearer",
                &serde_json::to_string_pretty(&manifest).unwrap(),
            );
            let manager = MarketplaceManager::with_store(FallbackKeyringStore {
                inner: MemoryCredentialStore::default(),
                fallback_active: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            });

            manager
                .install("opt-bearer", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            assert_eq!(
                mcp["servers"]["opt-bearer-remote"],
                serde_json::json!({"url": "https://opt.example.com/mcp"}),
                "the optional-secret tool must install on a keyring-less host, unwired"
            );
        });
    }

    /// The required-field counterpart keeps the fail-closed classification:
    /// an undeterminable credential store must fail the install instead of
    /// baking a permanently unwired entry.
    #[test]
    fn required_bearer_config_field_still_fails_closed_without_a_keyring() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"req-bearer","name":"req-bearer","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"req-bearer-remote","url":"https://req.example.com/mcp"}],
                "config_fields":[
                    {"key":"REQ_BEARER_API_KEY","label":"k","required":true,"target":"bearer","secret":true}
                ]
            });
            write_tool_manifest(
                "req-bearer",
                &serde_json::to_string_pretty(&manifest).unwrap(),
            );
            let manager = MarketplaceManager::with_store(FallbackKeyringStore {
                inner: MemoryCredentialStore::default(),
                fallback_active: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            });

            let error = manager
                .install("req-bearer", &std::collections::HashMap::new())
                .unwrap_err();
            assert!(
                error.contains("OS keyring is unreachable"),
                "the required field must keep the fail-closed classification: {error}"
            );
            assert!(
                !paths::mcp_config_path().exists(),
                "no unwired entry may be written by the failed install"
            );
        });
    }

    /// With the OS keyring believed HEALTHY, a store error on an optional
    /// field is a real fault, not an undeterminable miss: the install keeps
    /// the required arm's fail-closed discipline instead of baking a
    /// transiently failing store into a permanently unwired entry (healthy
    /// entries are never re-examined by the reconcile, so nothing would ever
    /// repair it). Only the fallback-active classification is tolerated.
    #[test]
    fn optional_bearer_config_field_fails_closed_on_a_real_store_error() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"opt-err","name":"opt-err","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"opt-err-remote","url":"https://opt-err.example.com/mcp"}],
                "config_fields":[
                    {"key":"OPT_ERR_API_KEY","label":"k","required":false,"target":"bearer","secret":true}
                ]
            });
            write_tool_manifest("opt-err", &serde_json::to_string_pretty(&manifest).unwrap());
            let manager = MarketplaceManager::with_store(FlakyThenHealedStore {
                inner: MemoryCredentialStore::default(),
                fail_reads: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            });

            let error = manager
                .install("opt-err", &std::collections::HashMap::new())
                .unwrap_err();
            assert!(
                error.contains("keyring unavailable"),
                "the real store fault must fail the install, not be swallowed: {error}"
            );
            assert!(
                !paths::mcp_config_path().exists(),
                "no unwired entry may be written by the failed install"
            );
        });
    }

    /// A store with the file fallback active whose reads and/or writes fail
    /// with a REAL fault. `os_keyring_unreachable` is true in both modes, so
    /// the flag alone can never decide tolerance — only the resolve's own
    /// classification (`UndeterminableMiss`) may be tolerated, and a fault is
    /// not a miss. `fail_write_values` targets individual write attempts so a
    /// fault can be isolated to one key's rebind.
    struct FallbackActiveFaultStore {
        inner: MemoryCredentialStore,
        fail_reads: bool,
        fail_write_values: &'static [&'static str],
    }

    impl CredentialStore for FallbackActiveFaultStore {
        fn get(&self, reference: &CredentialReference) -> Result<Option<String>, CredentialError> {
            if self.fail_reads {
                return Err(CredentialError::new(
                    "fallback store read failed (simulated)",
                ));
            }
            self.inner.get(reference)
        }
        fn set(&self, reference: &CredentialReference, value: &str) -> Result<(), CredentialError> {
            if self.fail_write_values.contains(&value) {
                return Err(CredentialError::new(
                    "fallback store write failed (simulated)",
                ));
            }
            self.inner.set(reference, value)
        }
        fn delete(&self, reference: &CredentialReference) -> Result<(), CredentialError> {
            self.inner.delete(reference)
        }
        fn os_keyring_unreachable(&self, _reference: &CredentialReference) -> bool {
            true
        }
    }

    /// A read fault under the active file fallback is a real store fault, not
    /// an undeterminable miss: the read answered with an error, so there is
    /// no ambiguity to tolerate. The optional field must fail the install
    /// like the required arm — swallowing it as "absent" would bake a
    /// permanently unwired entry that later startups never repair (healthy
    /// entries are never re-examined by the reconcile).
    #[test]
    fn optional_bearer_config_field_fails_closed_on_a_fallback_read_fault() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"opt-rfail","name":"opt-rfail","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"opt-rfail-remote","url":"https://opt-rfail.example.com/mcp"}],
                "config_fields":[
                    {"key":"OPT_RFAIL_API_KEY","label":"k","required":false,"target":"bearer","secret":true}
                ]
            });
            write_tool_manifest(
                "opt-rfail",
                &serde_json::to_string_pretty(&manifest).unwrap(),
            );
            let manager = MarketplaceManager::with_store(FallbackActiveFaultStore {
                inner: MemoryCredentialStore::default(),
                fail_reads: true,
                fail_write_values: &[],
            });

            let error = manager
                .install("opt-rfail", &std::collections::HashMap::new())
                .unwrap_err();
            assert!(
                error.contains("fallback store read failed"),
                "a real read fault under the fallback must fail the install, not be tolerated as an absent credential: {error}"
            );
            assert!(
                !paths::mcp_config_path().exists(),
                "no unwired entry may be written by the failed install"
            );
        });
    }

    /// A write fault under the active file fallback (here: the legacy
    /// plaintext rebind inside the resolve) is a real store fault — the value
    /// was available, only the persist failed. Tolerating it would silently
    /// drop the credential and bake a permanently unwired entry instead of
    /// failing the install for a retry. `ALPHA_TOKEN` resolves and persists
    /// fine so the env-keys sweep does not fail the install first: only the
    /// optional config-field arm ever sees this write fault.
    #[test]
    fn optional_bearer_config_field_fails_closed_on_a_fallback_write_fault() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"opt-wfail","name":"opt-wfail","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"opt-wfail-remote","url":"https://opt-wfail.example.com/mcp"}],
                "env":{"ALPHA_TOKEN":"alpha-v1","OPT_WFAIL_API_KEY":"legacy-plain"},
                "config_fields":[
                    {"key":"OPT_WFAIL_API_KEY","label":"k","required":false,"target":"bearer","secret":true}
                ]
            });
            write_tool_manifest(
                "opt-wfail",
                &serde_json::to_string_pretty(&manifest).unwrap(),
            );
            let manager = MarketplaceManager::with_store(FallbackActiveFaultStore {
                inner: MemoryCredentialStore::default(),
                fail_reads: false,
                fail_write_values: &["legacy-plain"],
            });

            let error = manager
                .install("opt-wfail", &std::collections::HashMap::new())
                .unwrap_err();
            assert!(
                error.contains("fallback store write failed"),
                "a real write fault under the fallback must fail the install, not silently drop the credential: {error}"
            );
            assert!(
                !paths::mcp_config_path().exists(),
                "no unwired entry may be written by the failed install"
            );
        });
    }

    /// A cleared dialog input arrives as an empty (or whitespace) string, not
    /// an omitted key: it must take the same "not provided" path as no input
    /// at all — re-derive from the store, tolerate absence — instead of
    /// failing the install as a missing credential.
    #[test]
    fn optional_bearer_config_field_treats_a_cleared_input_as_absent() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"opt-empty","name":"opt-empty","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "servers":[{"name":"opt-empty-remote","url":"https://opt-empty.example.com/mcp"}],
                "config_fields":[
                    {"key":"OPT_EMPTY_API_KEY","label":"k","required":false,"target":"bearer","secret":true}
                ]
            });
            write_tool_manifest(
                "opt-empty",
                &serde_json::to_string_pretty(&manifest).unwrap(),
            );
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            manager
                .install(
                    "opt-empty",
                    &std::iter::once(("OPT_EMPTY_API_KEY".to_string(), "   ".to_string()))
                        .collect(),
                )
                .unwrap();

            let mcp = read_mcp_json();
            assert_eq!(
                mcp["servers"]["opt-empty-remote"],
                serde_json::json!({"url": "https://opt-empty.example.com/mcp"}),
                "a cleared input must install unwired, exactly like no input"
            );
        });
    }

    /// A non-secret install-time env config field is dropped by the startup
    /// rebuild just like a secret one (the rebuild has no user input), so the
    /// honesty note must name it too — the note's reproducible set must match
    /// what the rebuild actually re-derives.
    #[test]
    fn rebuild_note_names_dropped_non_secret_config_field() {
        with_temp_home(|| {
            let manifest = serde_json::json!({
                "id":"cf-x","name":"cf-x","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"python","args":["server.py"],
                "config_fields":[
                    {"key":"REGION","label":"region","required":false,"target":"env","secret":false}
                ]
            });
            write_tool_manifest("cf-x", &serde_json::to_string_pretty(&manifest).unwrap());
            std::fs::write(
                mcp_catalog::package_mcp_dir("cf-x").join("server.py"),
                "print('fixture')\n",
            )
            .unwrap();
            write_installed_ids(&["cf-x".to_string()]);
            let dead_command = paths::pinvou3_home().join("x").join("w.py");
            seed_mcp_json(serde_json::json!({
                "cf-x": {
                    "command": dead_command,
                    "args": [],
                    "env": {"REGION": "us-east-1"}
                }
            }));
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let actions = manager.reconcile_installed_mcp_entries().unwrap();
            assert_eq!(actions.len(), 1, "{actions:?}");
            assert!(
                actions[0].contains("rebuilt mcp.json entry") && actions[0].contains("REGION"),
                "the rebuild note must disclose the dropped non-secret config field: {actions:?}"
            );
            assert!(
                !actions[0].contains("us-east-1"),
                "the note must never carry the dropped value: {actions:?}"
            );
        });
    }

    /// A UI install in the corrupt window must fail with the actionable
    /// refusal (backup + fix-or-remove guidance — the same message every
    /// writer refuses with) instead of the plaintext migration's bare parse
    /// error, and must leave mcp.json and installed.json untouched.
    #[test]
    fn install_refuses_a_corrupt_mcp_json_with_the_actionable_error() {
        with_temp_home(|| {
            write_installed_ids(&["existing".to_string()]);
            let mcp_path = paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            let corrupt = r#"{"servers": {,"trailing":"comma"}"#;
            std::fs::write(&mcp_path, corrupt).unwrap();
            write_tool_manifest(
                "corrupt-install",
                r#"{
                    "id":"corrupt-install","name":"CI","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"node","args":["server.js"]
                }"#,
            );

            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let error = manager
                .install("corrupt-install", &std::collections::HashMap::new())
                .unwrap_err();
            assert!(
                error.contains("mcp.json is unparseable") && error.contains("fix or remove it"),
                "install must surface the actionable refusal, not a bare parse error: {error}"
            );
            assert_eq!(
                std::fs::read(&mcp_path).unwrap(),
                corrupt.as_bytes(),
                "the refused install must not touch the corrupt file"
            );
            let backup = std::fs::read_dir(mcp_path.parent().unwrap())
                .unwrap()
                .filter_map(|entry| entry.ok())
                .find(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("mcp.json.corrupt.")
                })
                .unwrap_or_else(|| panic!("the refusal must mint a corrupt-file backup"));
            assert_eq!(
                std::fs::read(backup.path()).unwrap(),
                corrupt.as_bytes(),
                "the backup must hold the original bytes"
            );
            // Platforms without POSIX modes return None (privacy is
            // ACL-expressed there) — nothing bit-level to assert.
            if let Some(mode) =
                crate::platform::filesystem::permission_bits(&backup.path()).unwrap()
            {
                assert_eq!(
                    mode, 0o600,
                    "the backup may hold pre-migration plaintext credentials and must stay owner-only"
                );
            }
            assert_eq!(
                manager.installed_ids(),
                vec!["existing".to_string()],
                "the refused install must not touch the registry"
            );
            assert!(!marketplace_transaction_journal().exists());
        });
    }

    /// Two distinct corrupt contents minted within the same second must both
    /// survive: the nanosecond suffix makes every real write a fresh name, and
    /// idempotence stays with the identical-bytes dedup. A second-granularity
    /// suffix would overwrite the first copy with the second content — this
    /// pins the naming contract so that regression cannot return silently.
    #[test]
    fn backup_keeps_distinct_corrupt_contents_within_the_same_second() {
        with_temp_home(|| {
            let target = paths::mcp_config_path();
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            let first = r#"{"servers":{"a":1}"#;
            let second = r#"{"servers": {,"trailing":"comma"}"#;
            backup_corrupt_json_file(&target, "mcp.json.corrupt", first);
            backup_corrupt_json_file(&target, "mcp.json.corrupt", second);

            let backups: Vec<(String, Vec<u8>)> = std::fs::read_dir(target.parent().unwrap())
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("mcp.json.corrupt.")
                })
                .map(|entry| {
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        std::fs::read(entry.path()).unwrap(),
                    )
                })
                .collect();
            assert_eq!(
                backups.len(),
                2,
                "two distinct corrupt contents in one second must mint two backups"
            );
            assert!(
                backups.iter().any(|(_, bytes)| bytes == first.as_bytes())
                    && backups.iter().any(|(_, bytes)| bytes == second.as_bytes()),
                "both corrupt contents must survive verbatim"
            );
        });
    }

    /// The Python-repair downgrade is the third boot writer of the corrupt-file
    /// guarantee (cleanup and the reconcile's own gate are the other two): on a
    /// corrupt mcp.json the downgrade's write is refused, the transaction rolls
    /// back, the note says so, and every byte survives.
    #[test]
    fn downgrade_on_a_corrupt_mcp_json_rolls_back_and_preserves_bytes() {
        with_temp_home(|| {
            let (python, version) = test_python();
            write_locked_python_tool("invalid-lock-corrupt", "invalid_lock_fixture", &version);
            let manager = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let mut invalid = manager.load_manifest("invalid-lock-corrupt").unwrap();
            invalid.python_dependencies.as_mut().unwrap().schema_version = 999;
            MarketplaceManager::<MemoryCredentialStore>::trust_dependency_manifest_for_test(
                invalid,
            );
            write_installed_ids(&["invalid-lock-corrupt".to_string()]);
            store::BundleStore::new()
                .upsert(store::BundleRecord::installed_now(
                    "invalid-lock-corrupt",
                    store::BundleSource::Preset,
                ))
                .unwrap();
            let mcp_path = paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            let corrupt = r#"{"servers": {,"trailing":"comma"}"#;
            std::fs::write(&mcp_path, corrupt).unwrap();
            let installed_before = manager_installed_bytes();

            let errors = manager
                .repair_installed_python_tools_with_python(&python)
                .unwrap();
            assert!(
                errors
                    .iter()
                    .any(|error| error.contains("downgrade failed and was rolled back")),
                "the downgrade must report its rollback: {errors:?}"
            );
            assert!(
                errors
                    .iter()
                    .any(|error| error.contains("mcp.json is unparseable")),
                "the rollback must be the corrupt-file refusal, not an unrelated failure: {errors:?}"
            );
            assert_eq!(
                std::fs::read(&mcp_path).unwrap(),
                corrupt.as_bytes(),
                "the corrupt file must survive the refused downgrade"
            );
            assert_eq!(manager_installed_bytes(), installed_before);
            assert!(!marketplace_transaction_journal().exists());
        });
    }
}
