//! 包 id × SessionMode 的单一禁用集（`~/.pinvou3/disabled_bundles.json`）。
//!
//! 这是「工具市场统一治理」scope 收敛（todo A 节）的单一真相源：取代原先
//! `disabled_connectors.json`（连接器 id）与 `disabled_skills.json`（技能 id）两份
//! 文件。开关粒度收敛为**包 id**（= `bundle.rs` 里 `BundleInfo.id`，即 MCP 工具 id /
//! 技能 id / CLI 连接器 id），一个包 = 一个开关，包内技能（companion skills）可见性
//! 唯一跟随所属包（§5.2 不变量）。
//!
//! 落盘格式与 #287 泛化后的两份旧文件同构：`{scopes: {"<mode>": [...]},
//! "initialized": ["<mode>"], project_skills_enabled}`，scope 键即 `SessionMode` 的
//! kebab-case 名。首个版本读取时把两份旧文件迁移到本文件（读到即迁移）：
//! 旧连接器 id 原样进包 id（连接器 id 即包 id）；旧技能 id 经 `bundle::skill_owner_package`
//! 映射到所属包（companion → MCP/CLI 包，独立技能 → 自身）；`skill:` 前缀跨文件借道
//! 残留统一剥除并清出连接器文件。迁移幂等，失败回退默认值（安全兜底）。
//!
//! 依赖方向：本模块与 `bundle` / `skill_marketplace` 同属 marketplace 领域，只依赖
//! `platform::paths` 与 marketplace 内既有类型，不反向依赖 assistant 运行时。

use std::path::PathBuf;
use std::sync::Mutex;

use crate::core::session_mode::{PackDefaultPolicy, SessionMode};
use crate::features::marketplace::bundle::{builtin_cli_bundle_ids, skill_owner_package};
use crate::features::marketplace::skill_marketplace::SkillMarketplaceManager;
use crate::features::marketplace::{ConnectorScope, MarketplaceManager};
use crate::platform::paths;

/// 按模式 scope 键控的禁用**包**列表（包 id 集合）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DisabledBundlesFile {
    /// scope（模式 kebab-case 名）→ 该 scope 被禁用（开关关）的包 id 列表。
    #[serde(default)]
    pub scopes: std::collections::BTreeMap<String, Vec<String>>,
    /// scope → 该 scope 被「不可见」（可见性过滤，从 composer 列表消失）的包 id 列表。
    /// 与 `scopes`（开关）正交：开关控制 on/off，可见性控制是否出现在列表。
    #[serde(default)]
    pub hidden_scopes: std::collections::BTreeMap<String, Vec<String>>,
    /// 已被用户显式初始化（改过开关）的 scope 集合。
    #[serde(default)]
    pub initialized: std::collections::BTreeSet<String>,
    /// Whether project-level skills are enabled (default off; effective only
    /// when the session is bound to a project/work directory — not code-only;
    /// see skill_materialization.rs). Moved here with the skills side.
    #[serde(default)]
    pub project_skills_enabled: bool,
    /// 未知键原样保留（前向兼容）。
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

fn disabled_bundles_path() -> PathBuf {
    paths::pinvou3_home().join("disabled_bundles.json")
}

/// In-process serialization for the `disabled_bundles.json` read-modify-write.
///
/// #515: an in-process mutex alone cannot stop cross-process races — the GUI
/// and headless hosts can share one `~/.pinvou3` home and both install/toggle
/// packs, so two concurrent load→save sections silently drop each other's
/// writes (a lost update; the lost side is the user's explicit off, which is
/// fail-open on the DenyAll gate). Every write critical section must go
/// through `with_scope_file_lock`: take this mutex first, then the OS-level
/// file lock (flock / LockFileEx via `fd-lock`, the same primitive and crate
/// as the remote-control process-ownership lock). The file lock is owned per
/// open file description and does not serialize threads within one process,
/// so the in-process mutex stays.
static DISABLED_BUNDLES_FILE_LOCK: Mutex<()> = Mutex::new(());

/// Cross-process lock file path (same directory as the data file; holds no
/// user data).
fn disabled_bundles_lock_path() -> PathBuf {
    paths::pinvou3_home().join("disabled_bundles.lock")
}

/// Opens (creating if missing) the cross-process lock file. Shared by the
/// blocking write path and the try-lock read path.
fn open_scope_lock_file() -> Result<std::fs::File, String> {
    let lock_path = disabled_bundles_lock_path();
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .map_err(|error| format!("open {}: {error}", lock_path.display()))
}

/// Runs a write critical section `f` while holding the combined in-process
/// mutex + OS file lock that serializes every `disabled_bundles.json`
/// load→save against the peer process (GUI / headless sharing the home)
/// (#515). The `try_` prefix means fallible, not non-blocking.
///
/// Returns `Err` when cross-process serialization cannot be established (the
/// lock file cannot be opened or locked, e.g. a filesystem without lock
/// support). Callers must then refuse the read-modify-write instead of running
/// it unsynchronized — an unlocked RMW is exactly the cross-process lost
/// update this module guards against.
///
/// Blocking has no timeout (fd-lock v4 has no timeout API): the OS releases
/// the lock when the peer process exits or crashes (flock / LockFileEx die
/// with the fd), but a frozen peer (SIGSTOP / debugger) makes this process
/// wait indefinitely. The critical section is a local JSON read-modify-write
/// (the widest variant, the connector-switch sync, additionally enumerates
/// installed ids), so that fail-stop hang (frozen peer only) is accepted over
/// a fail-open lost update.
///
/// Lock order is uniform module-wide: in-process mutex → OS file lock, and
/// the only other lock reachable inside a critical section is the bundle
/// store's own mutex (via `resolve_scope_disabled_ids` → installed-ids
/// enumeration). That ordering is never reversed — no store method enters
/// this module's critical sections — so no deadlock class exists.
fn with_scope_file_lock<F, R>(f: F) -> Result<R, String>
where
    F: FnOnce() -> R,
{
    let _process_guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let file = open_scope_lock_file()?;
    let mut lock = fd_lock::RwLock::new(file);
    // The in-process mutex is already held while the OS lock is taken, and
    // the store mutex (see the lock-order note above) is never held by
    // another thread waiting on this one, so deadlock is impossible.
    let _os_guard = lock
        .write()
        .map_err(|error| format!("lock {}: {error}", disabled_bundles_lock_path().display()))?;
    Ok(f())
}

/// Loads the file for policy reads. Takes the in-process mutex, then *tries*
/// the OS lock without blocking: when it is free, the read runs fully locked
/// so a read-time repair (legacy migration, `skill:` strip) persists
/// serialized with writers; when a peer holds the lock, the read degrades to
/// a bounded, never-persisting unlocked snapshot — `write_atomic` replaces
/// the file atomically, so the snapshot is always a complete (possibly
/// just-superseded) state, and the next uncontended read converges the file.
///
/// The bounded degrade is what keeps the engine-side hot readers (per-turn
/// inventory reminders, deny rulesets, engine spawn config) safe to call
/// directly: they never couple to a peer's critical section, so unlike the
/// write path no operation needs to be kept off the Tokio executor for them.
/// Contention is a normal, silent degradation; only an unexpected error is
/// logged.
pub(crate) fn load_disabled_bundles_file() -> DisabledBundlesFile {
    let _process_guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match open_scope_lock_file() {
        Ok(file) => match fd_lock::RwLock::new(file).try_write() {
            Ok(_guard) => load_disabled_bundles_file_locked(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                read_disabled_bundles_file(false)
            }
            Err(error) => {
                eprintln!(
                    "[scope] cross-process lock probe failed ({error}); unlocked read without persist"
                );
                read_disabled_bundles_file(false)
            }
        },
        Err(error) => {
            eprintln!(
                "[scope] cross-process lock unavailable ({error}); unlocked read without persist"
            );
            read_disabled_bundles_file(false)
        }
    }
}

/// Read implementation shared by the locked and degraded paths. First
/// version: a missing file migrates the two legacy files (idempotent); an
/// existing file is parsed with a defensive `skill:` prefix strip (new write
/// paths no longer produce it). Read-time repairs persist only when
/// `persist_repairs` is set — i.e. only under the full lock, so the on-disk
/// file converges to the new format without any unsynchronized write.
fn read_disabled_bundles_file(persist_repairs: bool) -> DisabledBundlesFile {
    match std::fs::read_to_string(&disabled_bundles_path()) {
        Err(_) => {
            let file = migrate_from_legacy_files();
            if persist_repairs
                && (!file.scopes.is_empty() || file.initialized.iter().any(|k| !k.is_empty()))
            {
                if let Err(error) = save_disabled_bundles_file(&file) {
                    eprintln!("[scope] read-repair persist failed: {error}");
                }
            }
            file
        }
        Ok(content) => {
            let mut file: DisabledBundlesFile = serde_json::from_str(&content).unwrap_or_default();
            if strip_skill_prefixes(&mut file) && persist_repairs {
                if let Err(error) = save_disabled_bundles_file(&file) {
                    eprintln!("[scope] read-repair persist failed: {error}");
                }
            }
            file
        }
    }
}

/// Read under the full write lock: repairs persist (serialized with every
/// other lock holder). Write critical sections load through this.
fn load_disabled_bundles_file_locked() -> DisabledBundlesFile {
    read_disabled_bundles_file(true)
}

/// 防御：剥除所有 scope 禁用集与不可见集里的 `skill:` 前缀（旧前端 bug 窗口期
/// 误写入的带前缀 id；本文件按裸包 id 匹配，读者在此统一归一）。返回是否剥出过前缀。
fn strip_skill_prefixes(file: &mut DisabledBundlesFile) -> bool {
    let mut stripped = false;
    for ids in file
        .scopes
        .values_mut()
        .chain(file.hidden_scopes.values_mut())
    {
        for id in ids.iter_mut() {
            if let Some(s) = id.strip_prefix("skill:") {
                *id = s.to_string();
                stripped = true;
            }
        }
    }
    stripped
}

/// 原始条目 → 包 id。连接器/CLI id 原样保留（`skill_owner_package` 对它们恒等）；
/// `skill:` 前缀剥除后按技能名映射到所属包（companion → MCP/CLI 包，独立技能 → 自身）。
fn to_package_id(raw: &str) -> String {
    let stripped = raw.strip_prefix("skill:").unwrap_or(raw);
    skill_owner_package(stripped)
}

/// 读时归一：存储条目按**当前**认领状态重映射为包 id 并去重（保序）。
/// 认领（`skill_owner_package`）随安装态时变：条目可能在 companion MCP 未装时
/// 按独立技能 id 落库，MCP 后装则认领翻转到包 id——只在写时归一会让用户的
/// 「关/隐藏」在认领翻转后静默失效（F4）；读时归一让门控跟随技能本体。
fn normalize_stored_pkg_ids(ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(ids.len());
    for id in ids {
        let pkg = to_package_id(id);
        if !out.iter().any(|x| x == &pkg) {
            out.push(pkg);
        }
    }
    out
}

/// 首启迁移：读旧 `disabled_connectors.json` + `disabled_skills.json`（各兼容三种
/// 旧形态），把条目映射为包 id 后按 scope 取并集，`project_skills_enabled` 取自技能
/// 文件。迁移不删旧文件（本版本内保留为惰性历史，只读新文件；下个版本周期随
/// 旧布局退役一并清理，见 todo C 节）。
fn migrate_from_legacy_files() -> DisabledBundlesFile {
    let mut file = DisabledBundlesFile::default();
    merge_connector_scopes_into(&mut file);
    merge_skill_scopes_into(&mut file);
    file
}

/// 把旧 `disabled_connectors.json` 的各 scope 条目映射为包 id 并并进 `file`。
fn merge_connector_scopes_into(file: &mut DisabledBundlesFile) {
    let path = paths::pinvou3_home().join("disabled_connectors.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    // 裸数组 → plain scope
    if let Ok(list) = serde_json::from_str::<Vec<String>>(&content) {
        let ids: Vec<String> = list.iter().map(|id| to_package_id(id)).collect();
        if !ids.is_empty() {
            file.scopes
                .insert(SessionMode::Plain.as_str().to_string(), ids);
        }
        return;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };
    let Some(obj) = value.as_object() else {
        return;
    };
    if let Some(scopes) = obj.get("scopes").and_then(|v| v.as_object()) {
        for (key, arr) in scopes {
            if let Some(arr) = arr.as_array() {
                let ids: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(to_package_id))
                    .collect();
                if !ids.is_empty() {
                    file.scopes.insert(key.clone(), ids);
                }
            }
        }
        if let Some(initialized) = obj.get("initialized").and_then(|v| v.as_array()) {
            for key in initialized.iter().filter_map(|v| v.as_str()) {
                file.initialized.insert(key.to_string());
            }
        }
    } else {
        // 旧双 scope 对象 {plain, code, code_initialized}
        for key in ["plain", "code"] {
            if let Some(arr) = obj.get(key).and_then(|v| v.as_array()) {
                let ids: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(to_package_id))
                    .collect();
                if !ids.is_empty() {
                    file.scopes.insert(key.to_string(), ids);
                }
            }
        }
        if obj
            .get("code_initialized")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            file.initialized
                .insert(SessionMode::Code.as_str().to_string());
        }
    }
}

/// 把旧 `disabled_skills.json` 的各 scope 条目映射为包 id 并并进 `file`（取并集），
/// 并继承 `project_skills_enabled`。
fn merge_skill_scopes_into(file: &mut DisabledBundlesFile) {
    let path = paths::pinvou3_home().join("disabled_skills.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    // 裸数组 → plain scope
    if let Ok(list) = serde_json::from_str::<Vec<String>>(&content) {
        let ids: Vec<String> = list.iter().map(|id| to_package_id(id)).collect();
        if !ids.is_empty() {
            merge_ids_into_scope(file, SessionMode::Plain.as_str(), ids);
        }
        return;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };
    let Some(obj) = value.as_object() else {
        return;
    };
    if let Some(scopes) = obj.get("scopes").and_then(|v| v.as_object()) {
        for (key, arr) in scopes {
            if let Some(arr) = arr.as_array() {
                let ids: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(to_package_id))
                    .collect();
                merge_ids_into_scope(file, key, ids);
            }
        }
        if let Some(initialized) = obj.get("initialized").and_then(|v| v.as_array()) {
            for key in initialized.iter().filter_map(|v| v.as_str()) {
                file.initialized.insert(key.to_string());
            }
        }
    } else {
        for key in ["plain", "code"] {
            if let Some(arr) = obj.get(key).and_then(|v| v.as_array()) {
                let ids: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(to_package_id))
                    .collect();
                merge_ids_into_scope(file, key, ids);
            }
        }
        if obj
            .get("code_initialized")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            file.initialized
                .insert(SessionMode::Code.as_str().to_string());
        }
    }
    if let Some(enabled) = obj.get("project_skills_enabled").and_then(|v| v.as_bool()) {
        file.project_skills_enabled = enabled;
    }
}

/// 并集合并到某 scope（去重、保序）。
fn merge_ids_into_scope(file: &mut DisabledBundlesFile, key: &str, ids: Vec<String>) {
    if ids.is_empty() {
        return;
    }
    let entry = file.scopes.entry(key.to_string()).or_default();
    for id in ids {
        if !entry.iter().any(|e| e == &id) {
            entry.push(id);
        }
    }
}

/// 写完整文件（原子替换，与旧文件同范式）。The write is part of the locked
/// critical section, so a failure is returned and the caller's critical
/// section reports `Err` — an `Ok` must mean the caller's change landed,
/// otherwise the DenyAll consent gate could still fail open (silently
/// dropping the write) on a full disk or an unwritable home.
fn save_disabled_bundles_file(file: &DisabledBundlesFile) -> Result<(), String> {
    let json = serde_json::to_string(file)
        .map_err(|error| format!("serialize disabled bundles: {error}"))?;
    deepseek_tui::utils::write_atomic(&disabled_bundles_path(), json.as_bytes())
        .map_err(|error| format!("write {}: {error}", disabled_bundles_path().display()))
}

/// 读某 scope 被禁用的**包 id** 列表（读不到/空 → 空）。
///
/// 已初始化的 scope 以落盘列表为准；未初始化的 scope 按其模式的包默认策略兜底：
/// DenyAll（如 code）返回全部已安装包 id ∪ 全部内置 CLI 包 id ——「默认全关，外部
/// 能力显式开启」；AllowAll（如 plain）返回落盘列表（缺省空 = 全开）。CLI 包未连接时
/// 纳入无害（配套技能不在盘上，排除为空操作），且「后才连接」也自动默认关。
pub fn load_disabled_bundles_for(scope: ConnectorScope) -> Vec<String> {
    let file = load_disabled_bundles_file();
    resolve_scope_disabled_ids(&file, scope)
}

/// 已加载文件 → 某 scope 的有效禁用包 id 列表（含 DenyAll 默认兜底）。供
/// `load_disabled_bundles_for` 与持锁写方（单临界区 RMW）共用，口径一致。
fn resolve_scope_disabled_ids(file: &DisabledBundlesFile, scope: ConnectorScope) -> Vec<String> {
    let key = scope.as_str();
    if file.initialized.contains(key) {
        return normalize_stored_pkg_ids(&file.scopes.get(key).cloned().unwrap_or_default());
    }
    match scope.pack_default_policy() {
        PackDefaultPolicy::AllowAll => {
            normalize_stored_pkg_ids(&file.scopes.get(key).cloned().unwrap_or_default())
        }
        PackDefaultPolicy::DenyAll => {
            // 现算分支：已按当前认领推导包 id，无需再归一。
            let mut ids: Vec<String> = MarketplaceManager::new().installed_ids();
            ids.extend(builtin_cli_bundle_ids().map(str::to_string));
            for skill_id in SkillMarketplaceManager::new().installed_skill_ids() {
                let pkg = skill_owner_package(&skill_id);
                if !ids.iter().any(|id| id == &pkg) {
                    ids.push(pkg);
                }
            }
            ids
        }
    }
}

/// 写某 scope 被禁用的包 id 列表（写入即标记该 scope 已初始化）。入参统一归一为包
/// id（剥 `skill:` 前缀 + companion 映射），防御历史版本误写入的带前缀条目。
/// Cross-process lock unavailable → `Err`: the write is refused, never
/// performed unsynchronized (#515).
pub fn save_disabled_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    with_scope_file_lock(|| {
        let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
        let mut file = load_disabled_bundles_file_locked();
        let key = scope.as_str().to_string();
        file.scopes.insert(key.clone(), normalized);
        file.initialized.insert(key);
        save_disabled_bundles_file(&file)
            .map_err(|error| format!("save disabled bundles ({}): {error}", scope.as_str()))
    })?
}

/// 读某 scope 被「不可见」（可见性过滤）的包 id 列表。缺省空 = 全可见。
/// 与 `load_disabled_bundles_for`（开关）正交：可见性只决定是否出现在 composer 列表，
/// 不决定 on/off。
pub fn load_hidden_bundles_for(scope: ConnectorScope) -> Vec<String> {
    let file = load_disabled_bundles_file();
    normalize_stored_pkg_ids(
        &file
            .hidden_scopes
            .get(scope.as_str())
            .cloned()
            .unwrap_or_default(),
    )
}

/// 写某 scope 被「不可见」的包 id 列表（不参与 DenyAll 默认，显式写入才隐藏）。
/// Cross-process lock unavailable → `Err`: the write is refused, never
/// performed unsynchronized (#515).
pub fn save_hidden_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    with_scope_file_lock(|| {
        let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
        let mut file = load_disabled_bundles_file_locked();
        file.hidden_scopes
            .insert(scope.as_str().to_string(), normalized);
        save_disabled_bundles_file(&file)
            .map_err(|error| format!("save hidden bundles ({}): {error}", scope.as_str()))
    })?
}

/// 该 scope 对底座「不可用」的包 id 并集 = 开关关（disabled）+ 不可见（hidden）。
/// 物化/工具白名单按此并集排除，两套门控对模型都是「调不到」。
pub fn unavailable_bundles_for(scope: ConnectorScope) -> Vec<String> {
    let mut ids = load_disabled_bundles_for(scope);
    for id in load_hidden_bundles_for(scope) {
        if !ids.iter().any(|x| x == &id) {
            ids.push(id);
        }
    }
    ids
}

/// 读全局（plain）被禁用的包 id 列表。兼容既有调用方。
pub fn load_disabled_bundles() -> Vec<String> {
    load_disabled_bundles_for(ConnectorScope::Plain)
}

/// 写全局（plain）被禁用的包 id 列表。兼容既有调用方。
pub fn save_disabled_bundles(ids: &[String]) -> Result<(), String> {
    save_disabled_bundles_for(ConnectorScope::Plain, ids)
}

/// 包安装/连接后同步所有 DenyAll 且已初始化的 scope：用户已改过这类会话开关时，
/// 新装的包默认仍保持关闭（加入该 scope 禁用集）；未初始化时无需处理（load 会按
/// 「默认全禁已装包」兜底）。AllowAll 模式无需同步（默认全开）。连接器与技能安装
/// 共用本入口：入参可为连接器 id / 技能 id / 包 id，统一归一为包 id。
/// This is the safety-default write for the DenyAll consent gate: refused with
/// `Err` when the cross-process lock is unavailable, never unsynchronized.
pub fn sync_deny_all_scopes_after_install(raw_id: &str) -> Result<(), String> {
    let package_id = to_package_id(raw_id);
    with_scope_file_lock(|| {
        let mut file = load_disabled_bundles_file_locked();
        let mut changed = false;
        for mode in SessionMode::ALL {
            if mode.pack_default_policy() != PackDefaultPolicy::DenyAll {
                continue;
            }
            let key = mode.as_str();
            if !file.initialized.contains(key) {
                continue;
            }
            let ids = file.scopes.entry(key.to_string()).or_default();
            if !ids.iter().any(|id| id == &package_id) {
                ids.push(package_id.clone());
                changed = true;
            }
        }
        if changed {
            save_disabled_bundles_file(&file).map_err(|error| {
                format!("sync DenyAll scopes after install ({package_id}): {error}")
            })?;
        }
        Ok(())
    })?
}

pub fn remove_bundle_from_disabled_scopes(raw_id: &str) -> Result<(), String> {
    let package_id = to_package_id(raw_id);
    with_scope_file_lock(|| {
        let mut file = load_disabled_bundles_file_locked();
        let mut changed = false;
        for ids in file.scopes.values_mut() {
            let before = ids.len();
            ids.retain(|id| id != &package_id);
            changed |= ids.len() != before;
        }
        // 可见性集同样清理：卸载后残留 hidden 会误隐藏未来同名重装。
        for ids in file.hidden_scopes.values_mut() {
            let before = ids.len();
            ids.retain(|id| id != &package_id);
            changed |= ids.len() != before;
        }
        if changed {
            save_disabled_bundles_file(&file)
                .map_err(|error| format!("remove {package_id} from disabled scopes: {error}"))?;
        }
        Ok(())
    })?
}

/// 项目级 skills 开关（默认关）。
pub fn project_skills_enabled() -> bool {
    load_disabled_bundles_file().project_skills_enabled
}

/// 写项目级 skills 开关。落盘后由调用方重写在线会话组合目录。
/// Cross-process lock unavailable → `Err`: the write is refused, never
/// performed unsynchronized (#515).
pub fn set_project_skills_enabled(enabled: bool) -> Result<(), String> {
    with_scope_file_lock(|| {
        let mut file = load_disabled_bundles_file_locked();
        if file.project_skills_enabled == enabled {
            return Ok(());
        }
        file.project_skills_enabled = enabled;
        save_disabled_bundles_file(&file)
            .map_err(|error| format!("set project skills enabled: {error}"))
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把 PINVOU3_HOME 指到干净临时目录跑闭包，借 ENV_LOCK 与其它 mutate 测试串行。
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("pinvou3-scope-{}", std::process::id()));
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
    fn bundles_roundtrip_per_scope() {
        with_temp_home(|| {
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            save_disabled_bundles_for(ConnectorScope::Code, &["feishu".to_string()]).unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["feishu".to_string()]
            );
        });
    }

    /// 开关（disabled）与可见性（hidden）两套集合正交，互不污染。
    #[test]
    fn hidden_bundles_are_orthogonal_to_disabled() {
        with_temp_home(|| {
            assert!(load_hidden_bundles_for(ConnectorScope::Plain).is_empty());
            save_hidden_bundles_for(ConnectorScope::Plain, &["combo-demo".to_string()]).unwrap();
            // hidden 不影响 disabled
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert_eq!(
                load_hidden_bundles_for(ConnectorScope::Plain),
                vec!["combo-demo".to_string()]
            );
            // 并集：不可用集包含 hidden
            assert!(
                unavailable_bundles_for(ConnectorScope::Plain).contains(&"combo-demo".to_string())
            );
        });
    }

    /// Unavailable = disabled + hidden, deduped; visibility writes must not
    /// pollute the disabled set.
    #[test]
    fn unavailable_is_union_deduped() {
        with_temp_home(|| {
            // Initially both sets are empty.
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert!(load_hidden_bundles_for(ConnectorScope::Plain).is_empty());

            // Disable weather, hide weather + pptx (weather appears in both sets).
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            save_hidden_bundles_for(
                ConnectorScope::Plain,
                &["weather".to_string(), "pptx".to_string()],
            )
            .unwrap();

            // Visibility writes do not pollute the disabled set.
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert_eq!(
                load_hidden_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string(), "pptx".to_string()]
            );

            // Union dedup: weather appears exactly once.
            let mut u = unavailable_bundles_for(ConnectorScope::Plain);
            u.sort();
            assert_eq!(u, vec!["pptx".to_string(), "weather".to_string()]);
        });
    }

    /// 卸载/断开后清理残留：同时清 disabled 与 hidden 两套集合。
    #[test]
    fn remove_bundle_clears_both_sets() {
        with_temp_home(|| {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            save_hidden_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            remove_bundle_from_disabled_scopes("weather").unwrap();
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert!(load_hidden_bundles_for(ConnectorScope::Plain).is_empty());
        });
    }

    /// 保存路径统一归一为包 id：剥 `skill:` 前缀 + companion 映射到所属包。
    #[test]
    fn save_normalizes_to_package_id() {
        with_temp_home(|| {
            // gongwen 未装：companion 技能保留独立纯技能包形态 → 归一为自身 id
            // （与 list_bundles 的 V5 认领展示一致，开关不回弹）。
            save_disabled_bundles_for(
                ConnectorScope::Plain,
                &["skill:government-writing".to_string()],
            )
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["government-writing".to_string()]
            );

            // 登记 gongwen 安装态后：companion 技能归属到包 → 归一为 gongwen。
            crate::features::marketplace::store::BundleStore::new()
                .upsert(
                    crate::features::marketplace::store::BundleRecord::installed_now(
                        "gongwen".to_string(),
                        crate::features::marketplace::store::BundleSource::Preset,
                    ),
                )
                .unwrap();
            save_disabled_bundles_for(
                ConnectorScope::Plain,
                &[
                    "skill:visualizer".to_string(),
                    "government-writing".to_string(),
                ],
            )
            .unwrap();
            // government-writing 是内嵌 gongwen manifest 的 companion 技能 → gongwen 包。
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["visualizer".to_string(), "gongwen".to_string()]
            );
        });
    }

    /// F4 回归：条目在 companion MCP 未装时按独立技能 id 落库；MCP 后装 →
    /// 认领翻转到包 id。读时归一（`normalize_stored_pkg_ids`）须让「关/隐藏」
    /// 跟随技能本体，否则用户的禁用/隐藏态在认领翻转后静默失效。
    #[test]
    fn load_normalizes_stale_skill_id_after_claim_flip() {
        with_temp_home(|| {
            save_disabled_bundles_for(ConnectorScope::Plain, &["government-writing".to_string()])
                .unwrap();
            save_hidden_bundles_for(ConnectorScope::Plain, &["government-writing".to_string()])
                .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["government-writing".to_string()]
            );
            assert_eq!(
                load_hidden_bundles_for(ConnectorScope::Plain),
                vec!["government-writing".to_string()]
            );

            // 后装 gongwen → 认领翻转 government-writing → gongwen
            crate::features::marketplace::store::BundleStore::new()
                .upsert(
                    crate::features::marketplace::store::BundleRecord::installed_now(
                        "gongwen".to_string(),
                        crate::features::marketplace::store::BundleSource::Preset,
                    ),
                )
                .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["gongwen".to_string()],
                "认领翻转后读时归一应把禁用条目重映射到包 id"
            );
            assert_eq!(
                load_hidden_bundles_for(ConnectorScope::Plain),
                vec!["gongwen".to_string()],
                "认领翻转后读时归一应把隐藏条目重映射到包 id"
            );
        });
    }

    /// 项目级 skills 开关往返。
    #[test]
    fn project_skills_roundtrip() {
        with_temp_home(|| {
            assert!(!project_skills_enabled(), "项目技能默认关");
            set_project_skills_enabled(true).unwrap();
            assert!(project_skills_enabled());
            set_project_skills_enabled(false).unwrap();
            assert!(!project_skills_enabled());
        });
    }

    /// The read path's read-then-migrate-persist must serialize with lock
    /// holders: while the test thread holds `DISABLED_BUNDLES_FILE_LOCK`, a
    /// concurrent load (the legacy connectors file on disk forces the
    /// migration persist) must not land on disk first. The worker signals
    /// readiness before calling load, and the assertion is a bounded poll
    /// window: if serialization were broken, the migration write would land
    /// inside the window and be caught (no fixed-sleep timing luck).
    #[test]
    fn read_path_migration_serializes_with_file_lock() {
        with_temp_home(|| {
            let legacy = r#"["weather"]"#;
            let conn = paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(&conn, legacy).unwrap();
            let guard = DISABLED_BUNDLES_FILE_LOCK
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                ready_tx
                    .send(())
                    .expect("reader start signal before taking the lock");
                load_disabled_bundles_for_plain_for_lock_test()
            });
            ready_rx
                .recv()
                .expect("reader thread should signal it started");
            // The lock is held here, so a working lock keeps the migration
            // write pending; broken serialization makes it land immediately.
            assert_data_file_absent_within();
            drop(guard);
            assert_eq!(reader.join().unwrap(), vec!["weather".to_string()]);
            let content = std::fs::read_to_string(disabled_bundles_path()).unwrap();
            assert!(
                content.contains("\"scopes\""),
                "migration should land after the lock is released: {content}"
            );
        });
    }

    fn load_disabled_bundles_for_plain_for_lock_test() -> Vec<String> {
        load_disabled_bundles_for(ConnectorScope::Plain)
    }

    /// Blocks until some thread holds the in-process scope mutex, i.e. the
    /// worker has provably reached the critical section and its OS-lock
    /// attempt has started. Deterministic handshake for the cross-process
    /// lock regressions (no sleep-before-assert races).
    fn wait_until_scope_mutex_held() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match DISABLED_BUNDLES_FILE_LOCK.try_lock() {
                // Not held by the worker yet — retry shortly.
                Ok(guard) => drop(guard),
                // Held by the worker: with the foreign lock held, it is now
                // either blocked on the OS lock or its acquisition failed.
                Err(std::sync::TryLockError::WouldBlock) => return,
                Err(std::sync::TryLockError::Poisoned(p)) => {
                    drop(p.into_inner());
                    return;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker never reached the scope lock acquisition point"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Asserts the data file does not appear within the poll window. Must run
    /// after the worker provably reached the acquisition point (handshake
    /// above or a start signal): a broken lock then makes the write land
    /// within milliseconds and fails the assertion deterministically, while a
    /// working lock simply runs out the window.
    fn assert_data_file_absent_within() {
        let window = std::time::Duration::from_secs(2);
        let deadline = std::time::Instant::now() + window;
        while std::time::Instant::now() < deadline {
            assert!(
                !disabled_bundles_path().exists(),
                "data file written while the lock was still held — serialization is broken"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Harness for the cross-process WRITE regression: holds the lock with a
    /// foreign fd, runs `worker` on a fresh thread, deterministically proves
    /// the worker cannot write the data file while the foreign lock is held,
    /// releases the lock, and joins. Returns the worker's result.
    ///
    /// The foreign fd stands in for the peer process: flock ownership is per
    /// open file description, so a second fd inside this process contends
    /// exactly like another process. The write guard borrows the local
    /// `RwLock`, so both stay plain locals of this function.
    fn assert_blocked_until_foreign_lock_release<R, F>(worker: F) -> R
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(disabled_bundles_lock_path())
            .expect("test should be able to open the lock file");
        let mut foreign = fd_lock::RwLock::new(file);
        let foreign_guard = foreign
            .write()
            .expect("test should be able to take the foreign cross-process write lock");
        let handle = std::thread::spawn(worker);
        wait_until_scope_mutex_held();
        assert_data_file_absent_within();
        drop(foreign_guard);
        handle
            .join()
            .expect("worker should finish once the foreign lock is released")
    }

    /// #515 cross-process contention on the READ path: while a peer holds the
    /// OS lock, a load must degrade promptly to the unlocked, never-persisting
    /// view (bounded — hot readers never couple to a peer's critical section)
    /// instead of blocking or writing unsynchronized. A later uncontended read
    /// converges the on-disk format.
    #[test]
    fn cross_process_lock_contention_degrades_read_without_persist() {
        with_temp_home(|| {
            let legacy = r#"["weather"]"#;
            let conn = paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(&conn, legacy).unwrap();

            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(disabled_bundles_lock_path())
                .expect("test should be able to open the lock file");
            let mut foreign = fd_lock::RwLock::new(file);
            let foreign_guard = foreign
                .write()
                .expect("test should be able to take the foreign cross-process write lock");

            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                let got = load_disabled_bundles_for_plain_for_lock_test();
                done_tx.send(got).expect("reader should send its result");
            });
            // A contended read must return promptly. recv_timeout doubles as
            // the regression assertion: a blocking read hangs here and fails
            // the test with a bounded, diagnosable timeout.
            let got = done_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("contended read must degrade instead of blocking on the peer's lock");
            assert_eq!(got, vec!["weather".to_string()]);
            assert!(
                !disabled_bundles_path().exists(),
                "a contended read must not persist the migration"
            );

            drop(foreign_guard);
            reader.join().expect("reader thread should finish");

            // A later uncontended read runs fully locked and converges the
            // file to the new format.
            let got = load_disabled_bundles_for_plain_for_lock_test();
            assert_eq!(got, vec!["weather".to_string()]);
            let content = std::fs::read_to_string(disabled_bundles_path()).unwrap();
            assert!(
                content.contains("\"scopes\""),
                "migration should land on the next uncontended read: {content}"
            );
        });
    }

    /// #515 symmetric case for the WRITE path: while a foreign fd holds the
    /// lock, the save must not land first; after the release the write lands
    /// with its full content.
    #[test]
    fn cross_process_lock_blocks_save_write_until_release() {
        with_temp_home(|| {
            assert_blocked_until_foreign_lock_release(|| {
                save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()])
                    .expect("write should succeed once the foreign lock is released");
            });
            let content = std::fs::read_to_string(disabled_bundles_path()).unwrap();
            assert!(
                content.contains("\"weather\""),
                "write should land after the lock is released: {content}"
            );
        });
    }

    /// #515 hard-fail: when the lock file cannot be opened (a directory at the
    /// lock path), every write entry point is refused with `Err` — never run
    /// unsynchronized — and the data file stays untouched.
    #[test]
    fn writes_refused_when_lock_file_unavailable() {
        with_temp_home(|| {
            std::fs::create_dir_all(disabled_bundles_lock_path()).unwrap();
            assert!(
                save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).is_err()
            );
            assert!(save_hidden_bundles_for(ConnectorScope::Plain, &["w".to_string()]).is_err());
            assert!(set_project_skills_enabled(true).is_err());
            assert!(
                sync_deny_all_scopes_after_install("weather").is_err(),
                "the DenyAll consent-gate sync must refuse too"
            );
            assert!(
                sync_disabled_bundles_for_connector_switch("weather", false).is_err(),
                "the connector-switch disable sync must refuse too"
            );
            assert!(
                remove_bundle_from_disabled_scopes("weather").is_err(),
                "the uninstall/restore cleanup must refuse too"
            );
            assert!(!disabled_bundles_path().exists());
        });
    }

    /// The atomic write itself is inside the locked critical section, so a
    /// failed write (here: a directory at the data path defeats write_atomic's
    /// rename) must surface as `Err` — an `Ok` that silently dropped the
    /// caller's change would re-open the fail-open hole on the DenyAll gate.
    /// Entry points whose RMW always lands a change (the save, the project
    /// toggle) are injectable this way; a conditional writer like the DenyAll
    /// sync is not (with the data path unreadable it has no initialized scope
    /// to modify, so it legitimately no-ops).
    #[test]
    fn write_failure_surfaces_as_err() {
        with_temp_home(|| {
            std::fs::create_dir_all(disabled_bundles_path()).unwrap();
            assert!(
                save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).is_err()
            );
            assert!(
                set_project_skills_enabled(true).is_err(),
                "the project-skills toggle must fail loudly when its write fails"
            );
        });
    }

    /// Reads degrade to an unlocked, never-persisting read when the lock is
    /// unavailable: existing data still loads, and the migration path computes
    /// in memory without writing the data file.
    #[test]
    fn read_degrades_without_persist_when_lock_unavailable() {
        with_temp_home(|| {
            std::fs::create_dir_all(disabled_bundles_lock_path()).unwrap();
            // Seed a data file directly (the save path is refused without the lock).
            let mut file = DisabledBundlesFile::default();
            file.scopes
                .insert("plain".to_string(), vec!["weather".to_string()]);
            std::fs::write(
                disabled_bundles_path(),
                serde_json::to_string(&file).unwrap(),
            )
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );

            // Migration path (no data file, legacy connectors file present):
            // loads the migrated view without persisting it.
            std::fs::remove_file(disabled_bundles_path()).unwrap();
            let conn = paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(&conn, r#"["weather"]"#).unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert!(
                !disabled_bundles_path().exists(),
                "degraded read must not persist"
            );
        });
    }
}
