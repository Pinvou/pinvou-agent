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

/// In-process serialization for `disabled_bundles.json` read-modify-write.
static DISABLED_BUNDLES_FILE_LOCK: Mutex<()> = Mutex::new(());

/// Runs the critical section under both locks and never runs `f()` when the
/// cross-process lock cannot be established. The in-process mutex serializes
/// threads; the exclusive flock on `disabled_bundles.lock` serializes
/// **processes** (the desktop app and the headless CLI each hold their own
/// in-process mutex, and both write the whole file — without the cross-process
/// lock a GUI toggle is overwritten by a CLI write and vice versa, the same
/// shape as the #287 two-legacy-files race). The flock uses the same primitive
/// and the same fd-lock crate as remote_control's process lock, and fails
/// closed like it: an unavailable lock returns `Err` instead of running the
/// write unserialized, because silently proceeding would reintroduce exactly
/// the lost-update this lock exists to prevent. The critical section only
/// reads/writes a file-sized payload (sub-millisecond), so blocking is
/// preferable to retry loops.
fn with_disabled_bundles_lock<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    attempt_cross_process_lock(f).map_err(|(error, _unrun)| error)
}

/// Read variant of [`with_disabled_bundles_lock`]: same two locks, but an
/// unavailable cross-process lock degrades to in-process-only serialization
/// instead of failing the read. A read cannot corrupt the file, and gating
/// reads run on every prompt/tool listing — refusing them would break the
/// GUI on exactly the degraded machines the lock failure describes. Writers
/// must not use this wrapper: they refuse (see `with_disabled_bundles_lock`).
/// The read path's internal migration saves converge (both processes merge
/// the same legacy sources), so the degraded read remains benign.
fn with_disabled_bundles_lock_read<T>(f: impl FnOnce() -> T) -> T {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match attempt_cross_process_lock(f) {
        Ok(value) => value,
        Err((error, f)) => {
            eprintln!(
                "[marketplace] {error}; proceeding with in-process locking only \
                 (read-only path)"
            );
            // The attempt hands the closure back unrun, so the degraded
            // fallback executes it exactly once.
            f()
        }
    }
}

/// Lock-acquisition half shared by both wrappers; the caller must already
/// hold [`DISABLED_BUNDLES_FILE_LOCK`]. `f()` runs exactly once on `Ok` and
/// never on `Err` — on refusal the closure is handed back unrun so the
/// degraded read path can still execute it.
fn attempt_cross_process_lock<T, F: FnOnce() -> T>(f: F) -> Result<T, (String, F)> {
    let lock_path = paths::pinvou3_home().join("disabled_bundles.lock");
    // The first write into a fresh PINVOU3_HOME happens before any other
    // writer has created the directory: create the parent first, otherwise
    // opening the lock would deterministically fail (same acquire-time
    // create_dir_all as remote_control's process lock). The lock file holds
    // nothing sensitive, so private-permission hardening is not pursued.
    if let Some(parent) = lock_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return Err((
                format!(
                    "[marketplace] create {}: {error}; cross-process lock unavailable",
                    parent.display()
                ),
                f,
            ));
        }
    }
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(error) => {
            return Err((
                format!(
                    "[marketplace] open cross-process lock {}: {error}; cross-process \
                     lock unavailable",
                    lock_path.display()
                ),
                f,
            ));
        }
    };
    let mut rw = fd_lock::RwLock::new(file);
    // fd-lock 4's write() returns a borrowing guard (no closure form); the
    // named binding keeps the guard alive until `f()` has returned, and
    // dropping it releases the flock.
    let _flock_guard = loop {
        match rw.write() {
            Ok(guard) => break guard,
            Err(error) => {
                // A caught signal delivered while blocked in flock aborts the
                // wait with EINTR; retrying is the standard convention and
                // keeps a stray signal from failing the write.
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err((
                    format!(
                        "[marketplace] acquire cross-process lock {}: {error}; \
                         cross-process lock unavailable",
                        lock_path.display()
                    ),
                    f,
                ));
            }
        }
    };
    Ok(f())
}

/// 读完整文件（取两层锁）。可能触发「读到即迁移」的读路径必须走本入口与持锁写方
/// 串行（与旧两份文件的 #287 竞态范式一致）。
pub(crate) fn load_disabled_bundles_file() -> DisabledBundlesFile {
    with_disabled_bundles_lock_read(load_disabled_bundles_file_locked)
}

/// 已持锁读实现。首个版本：文件不存在时从两份旧文件迁移（幂等）；文件存在时按新
/// 格式解析，防御性剥除 `skill:` 前缀残留（新写路径不会再产生）。
fn load_disabled_bundles_file_locked() -> DisabledBundlesFile {
    let path = disabled_bundles_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            let file = migrate_from_legacy_files();
            if !file.scopes.is_empty() || file.initialized.iter().any(|k| !k.is_empty()) {
                save_disabled_bundles_file(&file);
            }
            return file;
        }
    };
    let mut file: DisabledBundlesFile = serde_json::from_str(&content).unwrap_or_default();
    if strip_skill_prefixes(&mut file) {
        save_disabled_bundles_file(&file);
    }
    file
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

/// Maps a user-supplied raw id to the package id the persisted list stores,
/// for headless callers (the CLI's toggle read-back verification). A raw
/// skill id is conditionally re-claimed to its owner package, so verifying
/// against the raw id yields false positives.
pub fn package_id_for(raw: &str) -> String {
    to_package_id(raw)
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

/// 写完整文件（原子替换，与旧文件同范式）。
fn save_disabled_bundles_file(file: &DisabledBundlesFile) {
    if let Ok(json) = serde_json::to_string(file) {
        if let Err(error) =
            deepseek_tui::utils::write_atomic(&disabled_bundles_path(), json.as_bytes())
        {
            eprintln!("[scope] write disabled_bundles.json failed: {error}");
        }
    }
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
///
/// Fails closed: when the cross-process lock cannot be established the write
/// is refused with `Err` instead of running unserialized (writers from the
/// GUI and the CLI processes would overwrite each other whole-file).
pub fn save_disabled_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    with_disabled_bundles_lock(|| {
        let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
        let mut file = load_disabled_bundles_file_locked();
        let key = scope.as_str().to_string();
        file.scopes.insert(key.clone(), normalized);
        file.initialized.insert(key);
        save_disabled_bundles_file(&file);
    })
}

/// Single-critical-section read-modify-write of one scope's disabled package
/// id list, serialized both in-process and across the GUI/CLI processes. A
/// per-scope load→save across two lock acquisitions loses concurrent writes
/// in the inter-lock window (same shape as M-6b: while the GUI toggles
/// exclusively, a whole CLI disable can be dropped); the CLI's enable/disable
/// and the lock-holding writers share this entry point. The closure receives
/// the effective list including the DenyAll fallback, matching
/// `load_disabled_bundles_for`. The closure runs under both locks and must
/// not re-enter this module's load/save helpers (the in-process mutex is not
/// reentrant — it would self-deadlock). Fails closed: when the cross-process
/// lock cannot be established the write is refused with `Err` instead of
/// running unserialized.
pub fn update_disabled_bundles_for(
    scope: ConnectorScope,
    update: impl FnOnce(&mut Vec<String>),
) -> Result<(), String> {
    with_disabled_bundles_lock(|| {
        let file = load_disabled_bundles_file_locked();
        let mut ids = resolve_scope_disabled_ids(&file, scope);
        update(&mut ids);
        let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
        let mut file = file;
        let key = scope.as_str().to_string();
        file.scopes.insert(key.clone(), normalized);
        file.initialized.insert(key);
        save_disabled_bundles_file(&file);
    })
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
///
/// Fails closed like the other writers: an unavailable cross-process lock
/// refuses the write with `Err`.
pub fn save_hidden_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    with_disabled_bundles_lock(|| {
        let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
        let mut file = load_disabled_bundles_file_locked();
        file.hidden_scopes
            .insert(scope.as_str().to_string(), normalized);
        save_disabled_bundles_file(&file);
    })
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
///
/// Fails closed like the other writers: an unavailable cross-process lock
/// refuses the write with `Err`. Propagating matters here — a skipped sync
/// would leave a freshly installed bundle enabled in DenyAll scopes, the
/// opposite of the user's standing default.
pub fn sync_deny_all_scopes_after_install(raw_id: &str) -> Result<(), String> {
    let package_id = to_package_id(raw_id);
    with_disabled_bundles_lock(|| {
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
            save_disabled_bundles_file(&file);
        }
    })
}

/// Removes the package id from every scope's disabled and hidden lists so no
/// stale entry survives an uninstall. Fails closed like the other writers:
/// an unavailable cross-process lock refuses the write with `Err`.
pub fn remove_bundle_from_disabled_scopes(raw_id: &str) -> Result<(), String> {
    let package_id = to_package_id(raw_id);
    with_disabled_bundles_lock(|| {
        let mut file = load_disabled_bundles_file_locked();
        let mut changed = false;
        for ids in file.scopes.values_mut() {
            let before = ids.len();
            ids.retain(|id| id != &package_id);
            changed |= ids.len() != before;
        }
        // The hidden sets are cleaned the same way: a stale hidden entry left
        // behind by an uninstall would wrongly hide a future reinstall of the
        // same id.
        for ids in file.hidden_scopes.values_mut() {
            let before = ids.len();
            ids.retain(|id| id != &package_id);
            changed |= ids.len() != before;
        }
        if changed {
            save_disabled_bundles_file(&file);
        }
    })
}

/// 项目级 skills 开关（默认关）。
pub fn project_skills_enabled() -> bool {
    load_disabled_bundles_file().project_skills_enabled
}

/// 写项目级 skills 开关。落盘后由调用方重写在线会话组合目录。
///
/// Fails closed like the other writers: an unavailable cross-process lock
/// refuses the write with `Err`.
pub fn set_project_skills_enabled(enabled: bool) -> Result<(), String> {
    with_disabled_bundles_lock(|| {
        let mut file = load_disabled_bundles_file_locked();
        if file.project_skills_enabled == enabled {
            return;
        }
        file.project_skills_enabled = enabled;
        save_disabled_bundles_file(&file);
    })
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
            let mut u = unavailable_bundles_for(ConnectorScope::Plain);
            u.sort();
            assert_eq!(u, vec!["pptx".to_string(), "weather".to_string()]);
        });
    }

    /// Writers go through the cross-process lock file: the GUI app and the
    /// headless CLI are two processes whose in-process mutexes cannot see each
    /// other, so the flock on `disabled_bundles.lock` (same primitive as
    /// remote_control's process lock) is the only serialization point they
    /// share. The lock file must be created with the first write.
    #[test]
    fn writers_hold_the_cross_process_lock_file() {
        with_temp_home(|| {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            assert!(
                paths::pinvou3_home().join("disabled_bundles.lock").exists(),
                "cross-process lock file must exist after the first write"
            );
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()],
                "the write must survive the double critical section"
            );
        });
    }

    /// Restores the previous `PINVOU3_HOME` value on drop — on normal return
    /// and on panic unwind — so a failing test cannot pollute later tests.
    /// `OsString` preserves non-Unicode values.
    struct HomeGuard(Option<std::ffi::OsString>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: constructed while ENV_LOCK is held; env writes are
            // serialized in-process.
            unsafe {
                match self.0.take() {
                    Some(value) => std::env::set_var("PINVOU3_HOME", value),
                    None => std::env::remove_var("PINVOU3_HOME"),
                }
            }
        }
    }

    /// Fail-closed: when the lock file cannot be opened the write is refused
    /// with `Err` and the critical section never runs — running it would
    /// reintroduce exactly the cross-process lost-update the lock exists to
    /// prevent. The read path degrades instead and still reflects the
    /// untouched state.
    #[test]
    fn writer_refuses_when_lock_file_cannot_be_opened() {
        with_temp_home(|| {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            let lock_path = paths::pinvou3_home().join("disabled_bundles.lock");
            std::fs::remove_file(&lock_path).unwrap();
            std::fs::create_dir(&lock_path).unwrap();
            let ran = std::cell::Cell::new(false);
            let result = update_disabled_bundles_for(ConnectorScope::Plain, |ids| {
                ran.set(true);
                ids.push("unserialized".to_string());
            });
            assert!(
                result.is_err(),
                "the write must be refused while the lock file cannot be opened"
            );
            assert!(
                !ran.get(),
                "the critical section must not run without the cross-process lock"
            );
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()],
                "the refused write must leave the persisted state untouched"
            );
        });
    }

    /// Fail-closed at the earliest stage: when the lock file's parent
    /// directory cannot be created (here: the home's parent is a regular
    /// file — the realistic fresh-home failure), the write is refused, the
    /// closure never runs, and no state file is created.
    #[test]
    fn writer_refuses_when_lock_directory_cannot_be_created() {
        let env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let base = std::env::temp_dir().join(format!(
            "pinvou3-scope-lockdir-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_file(&base);
        std::fs::write(&base, b"not a directory").unwrap();
        let prev = std::env::var_os("PINVOU3_HOME");
        // SAFETY: ENV_LOCK held; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", base.join("home")) };
        let _home = HomeGuard(prev);
        let ran = std::cell::Cell::new(false);
        let result = update_disabled_bundles_for(ConnectorScope::Plain, |ids| {
            ran.set(true);
            ids.push("unserialized".to_string());
        });
        assert!(
            result.is_err(),
            "the write must be refused while the lock directory cannot be created"
        );
        assert!(
            !ran.get(),
            "the critical section must not run without the cross-process lock"
        );
        assert!(
            !paths::pinvou3_home().join("disabled_bundles.json").exists(),
            "the refused write must not create the state file"
        );
        drop(_home);
        drop(env_lock);
        let _ = std::fs::remove_file(&base);
    }

    /// Single-critical-section RMW semantics: the closure receives the
    /// **effective list** (an uninitialized DenyAll scope expands to the
    /// currently claimed set ∪ built-in CLI packages, the same view
    /// `load_disabled_bundles_for` returns), and writing back marks the scope
    /// initialized — from then on the read path trusts the persisted list and
    /// the fallback no longer expands.
    #[test]
    fn update_disabled_bundles_for_writes_effective_list_and_initializes() {
        with_temp_home(|| {
            // Uninitialized Code (DenyAll): the read path and the closure
            // input must be the same effective list.
            let effective = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                !effective.is_empty(),
                "the DenyAll fallback must expand to the effective set"
            );
            update_disabled_bundles_for(ConnectorScope::Code, |ids| {
                assert_eq!(
                    *ids, effective,
                    "closure input must match the read path's effective list"
                );
                ids.clear();
                ids.push("kept-pkg".to_string());
            })
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["kept-pkg".to_string()],
                "the write-back must freeze the scope as initialized"
            );
            // RMW after initialization: the closure sees the persisted list,
            // not the fallback expansion (idempotent read-modify-write).
            update_disabled_bundles_for(ConnectorScope::Code, |ids| {
                assert_eq!(ids.as_slice(), ["kept-pkg".to_string()].as_slice());
            })
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["kept-pkg".to_string()]
            );
        });
    }

    /// The flock's actual exclusion: while another fd in this process holds
    /// the lock (flock conflicts per open file description, equivalent to
    /// another process), a writer must block until the lock is released. A
    /// "create the lock file but forget to flock" regression stays green under
    /// an existence assertion and only turns red here.
    #[test]
    fn cross_process_lock_blocks_a_concurrent_writer() {
        with_temp_home(|| {
            let lock_path = paths::pinvou3_home().join("disabled_bundles.lock");
            let stand_in = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)
                .unwrap();
            let mut rw = fd_lock::RwLock::new(stand_in);
            let guard = rw.write().expect("hold the stand-in process lock");
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            let writer = std::thread::spawn(move || {
                save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
                tx.send(()).expect("signal writer completion");
            });
            // A non-blocking write over a small file returns in well under a
            // millisecond; still incomplete after 500ms proves it is waiting on
            // the lock.
            assert!(
                rx.recv_timeout(std::time::Duration::from_millis(500))
                    .is_err(),
                "a writer must block while another process holds the flock"
            );
            drop(guard);
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .expect("the writer completes once the flock is released");
            writer.join().unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()],
                "the blocked write must land intact after the lock is released"
            );
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

    /// 读路径的「读到即迁移落盘」必须取 `DISABLED_BUNDLES_FILE_LOCK` 与持锁写方
    /// 串行：持锁期间并发 load（磁盘为旧连接器文件、必然触发迁移落盘）不得先行落盘。
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
            let reader = std::thread::spawn(load_disabled_bundles_for_plain_for_lock_test);
            std::thread::sleep(std::time::Duration::from_millis(200));
            // 持锁期间 bundles 文件尚未写入（迁移被串行化）。
            assert!(
                !disabled_bundles_path().exists(),
                "持锁期间读路径不得先行迁移落盘"
            );
            drop(guard);
            assert_eq!(reader.join().unwrap(), vec!["weather".to_string()]);
            let content = std::fs::read_to_string(disabled_bundles_path()).unwrap();
            assert!(
                content.contains("\"scopes\""),
                "释放锁后迁移完成: {content}"
            );
        });
    }

    fn load_disabled_bundles_for_plain_for_lock_test() -> Vec<String> {
        load_disabled_bundles_for(ConnectorScope::Plain)
    }
}
