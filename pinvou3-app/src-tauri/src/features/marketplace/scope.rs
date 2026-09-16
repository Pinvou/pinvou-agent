//! 包 id × SessionMode 的单一禁用集（`~/.pinvou3/disabled_bundles.json`）。
//!
//! 这是「工具市场统一治理」scope 收敛的单一真相源：取代原先
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

/// `disabled_bundles.json` 读-改-写的**进程内**串行化。
///
/// #515：仅进程内互斥挡不住跨进程竞态——GUI 与 headless 宿主可共享同一
/// `~/.pinvou3` home 且都会安装/开关包，两个进程各自的 load→save 可互相覆盖
/// （lost update；方向是丢用户显式关掉的包，对 DenyAll 门即 fail-open）。
/// 所有读写临界区必须经 `with_scope_file_lock`：先取本 Mutex，再取 OS 级文件锁
/// （flock / LockFileEx，`fd-lock` 与 remote_control 进程归属锁同一原语，crate
/// 已在依赖图内）。文件锁按 open file description 归属，同进程线程间不互斥，
/// 故进程内 Mutex 与 OS 锁并用。
static DISABLED_BUNDLES_FILE_LOCK: Mutex<()> = Mutex::new(());

/// 跨进程锁文件路径（与数据文件同目录；锁文件不含用户数据）。
fn disabled_bundles_lock_path() -> PathBuf {
    paths::pinvou3_home().join("disabled_bundles.lock")
}

/// 取「进程内 Mutex + OS 文件锁」后执行闭包：跨进程把守 load→save 整段临界区
/// （#515）。OS 锁阻塞等待另一进程（GUI / headless 共享 home）释放；获取失败
/// （文件系统不支持锁等）退化为仅进程内锁（= 修复前行为）并告警，不阻断写入。
///
/// 阻塞无超时（fd-lock v4 已无 timeout API）：对方进程崩溃/退出时 OS 会释放锁
/// （flock / LockFileEx 随 fd 关闭），但对方被冻结（SIGSTOP / 调试器）时本进程
/// 将无限等待——临界区为本地 JSON 读-改-写，窗口毫秒级，接受这一 tradeoff
/// （fail-open 的 lost update 换成 bounded 不了的 fail-stop 挂起，仅对端冻结时）。
fn with_scope_file_lock<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let _process_guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let lock_path = disabled_bundles_lock_path();
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut lock = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
    {
        Ok(file) => fd_lock::RwLock::new(file),
        Err(error) => {
            eprintln!(
                "[scope] open {} failed: {error}; continue without cross-process lock",
                lock_path.display()
            );
            return f();
        }
    };
    // 持 OS 锁期间进程内 Mutex 必已持有，且全模块上锁顺序一致（Mutex → flock）、
    // 持锁期间不等待其它锁，跨进程无死锁。
    let _os_guard = match lock.write() {
        Ok(guard) => Some(guard),
        Err(error) => {
            eprintln!(
                "[scope] acquire {} failed: {error}; continue without cross-process lock",
                lock_path.display()
            );
            None
        }
    };
    f()
}

/// 读完整文件（进程内锁 + 跨进程文件锁）。可能触发「读到即迁移」的读路径必须走
/// 本入口与持锁写方串行（与旧两份文件的 #287 竞态范式一致；#515 扩展到跨进程）。
pub(crate) fn load_disabled_bundles_file() -> DisabledBundlesFile {
    with_scope_file_lock(load_disabled_bundles_file_locked)
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
                if let Err(error) = save_disabled_bundles_file(&file) {
                    eprintln!("[scope] write disabled_bundles.json failed: {error}");
                }
            }
            return file;
        }
    };
    let mut file: DisabledBundlesFile = serde_json::from_str(&content).unwrap_or_default();
    if strip_skill_prefixes(&mut file) {
        if let Err(error) = save_disabled_bundles_file(&file) {
            eprintln!("[scope] write disabled_bundles.json failed: {error}");
        }
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

/// First-boot migration: reads the legacy `disabled_connectors.json` and
/// `disabled_skills.json` (each tolerant of three legacy shapes), maps the
/// entries to bundle ids, and merges them into the new file — connector
/// scopes overwrite while skill scopes union, with `project_skills_enabled`
/// taken from the skill file. The legacy files are not deleted (kept as
/// read-only history for this release cycle; retired alongside the legacy
/// layout in a later cycle).
fn migrate_from_legacy_files() -> DisabledBundlesFile {
    let mut file = DisabledBundlesFile::default();
    merge_connector_scopes_into(&mut file);
    merge_skill_scopes_into(&mut file);
    file
}

/// 把旧 `disabled_connectors.json` 的各 scope 条目映射为包 id 并并进 `file`
/// （scope 条目按旧文件**覆盖写**）。
fn merge_connector_scopes_into(file: &mut DisabledBundlesFile) {
    merge_legacy_scope_file_into(
        file,
        &paths::pinvou3_home().join("disabled_connectors.json"),
        |file, key, ids| {
            file.scopes.insert(key.to_string(), ids);
        },
        false,
    );
}

/// 把旧 `disabled_skills.json` 的各 scope 条目映射为包 id 并并进 `file`（取并集），
/// 并继承 `project_skills_enabled`。
fn merge_skill_scopes_into(file: &mut DisabledBundlesFile) {
    merge_legacy_scope_file_into(
        file,
        &paths::pinvou3_home().join("disabled_skills.json"),
        |file, key, ids| merge_ids_into_scope(file, key, ids),
        true,
    );
}

/// 旧 scope 文件（`disabled_connectors.json` / `disabled_skills.json`）的共用解析
/// 骨架：裸数组 → plain scope、新版 `{scopes, initialized}` 对象、旧双 scope 对象
/// `{plain, code, code_initialized}` 三种形态，条目经 `to_package_id` 归一为包 id，
/// 并迁移 `initialized` / `code_initialized`。
///
/// `merge_ids` 决定 scope 条目的落库语义（连接器文件 = 覆盖写，技能文件 = 并集
/// 合并）；`inherit_project_flag` 为真时继承 `project_skills_enabled`（仅技能文件）。
fn merge_legacy_scope_file_into(
    file: &mut DisabledBundlesFile,
    path: &std::path::Path,
    merge_ids: impl Fn(&mut DisabledBundlesFile, &str, Vec<String>),
    inherit_project_flag: bool,
) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    // 裸数组 → plain scope
    if let Ok(list) = serde_json::from_str::<Vec<String>>(&content) {
        let ids: Vec<String> = list.iter().map(|id| to_package_id(id)).collect();
        if !ids.is_empty() {
            merge_ids(file, SessionMode::Plain.as_str(), ids);
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
                    merge_ids(file, key, ids);
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
                    merge_ids(file, key, ids);
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
    if inherit_project_flag {
        if let Some(enabled) = obj.get("project_skills_enabled").and_then(|v| v.as_bool()) {
            file.project_skills_enabled = enabled;
        }
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

/// 写完整文件（原子替换，与旧文件同范式）。写失败上抛：开关/可见性是用户治理
/// 状态，「静默丢写」会让调用方在半应用状态上继续走（前端按成功提示）。内部
/// best-effort 调用方（读路径迁移、卸载清理、默认策略同步）自行降级为日志。
fn save_disabled_bundles_file(file: &DisabledBundlesFile) -> Result<(), String> {
    let json = serde_json::to_string(file)
        .map_err(|error| format!("serialize disabled_bundles.json failed: {error}"))?;
    deepseek_tui::utils::write_atomic(&disabled_bundles_path(), json.as_bytes())
        .map_err(|error| format!("write disabled_bundles.json failed: {error}"))
}

/// 读某 scope 被禁用的**包 id** 列表（读不到/空 → 空）。
///
/// 已初始化的 scope 以落盘列表为准；未初始化的 scope 按其模式的包默认策略兜底：
/// DenyAll（如 code）返回全部已安装包 id ∪ 全部内置 CLI 包 id ——「默认全关，外部
/// 能力显式开启」；AllowAll（如 plain）返回落盘列表（缺省空 = 全开）。CLI 包未连接时
/// 纳入无害（配套技能不在盘上，排除为空操作），且「后才连接」也自动默认关。
///
/// When skill enumeration fails (permissions / transient IO, #531) the freshly
/// computed default degrades toward over-denial: owner packages of all preset
/// skills and of all store-known upload records are blanket-unioned into the
/// deny set (a superset of anything the failed enumeration could have named).
/// Only the computed default of an uninitialized scope is affected; initialized
/// scopes keep their persisted list.
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
            let skill_market = SkillMarketplaceManager::new();
            let (skill_ids, skill_scan_degraded) = skill_market.installed_skill_ids_strict();
            if skill_scan_degraded {
                // Enumeration degraded (#531): a failed probe can masquerade an
                // installed skill as absent, so the default deny set must not
                // shrink because of it. Blanket-union the owner packages of
                // every preset skill (compile-time manifests) and of every
                // known upload record — an uninitialized DenyAll scope would
                // rather have the user enable a package explicitly than hand
                // the consent gate a default silently narrowed by an
                // enumeration failure. Owner mapping is the same
                // `skill_owner_package` path as the normal loop below.
                // Accepted residual: with an unreadable bundle store the upload
                // population itself is unknowable (the lenient upload read
                // yields nothing to union); with an unreadable packages root,
                // straggler-copy-only skills of neither kind can be seen.
                eprintln!(
                    "[scope] DenyAll default deny list degraded (skill enumeration failed); biasing to over-deny"
                );
                let mut blanket: Vec<String> =
                    SkillMarketplaceManager::preset_skill_ids().collect();
                blanket.extend(skill_market.uploaded_skill_ids());
                for skill_id in blanket {
                    let pkg = skill_owner_package(&skill_id);
                    if !ids.iter().any(|id| id == &pkg) {
                        ids.push(pkg);
                    }
                }
            }
            for skill_id in skill_ids {
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
/// 写失败原样上抛（用户治理状态不得静默丢写）。
pub fn save_disabled_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    with_scope_file_lock(|| {
        let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
        let mut file = load_disabled_bundles_file_locked();
        let key = scope.as_str().to_string();
        file.scopes.insert(key.clone(), normalized);
        file.initialized.insert(key);
        save_disabled_bundles_file(&file)
    })
}

/// 读某 scope 被「不可见」（可见性过滤）的包 id 列表。缺省空 = 全可见。
/// 与 `load_disabled_bundles_for`（开关）正交：可见性只决定是否出现在 composer 列表，
/// 不决定 on/off。
pub fn load_hidden_bundles_for(scope: ConnectorScope) -> Vec<String> {
    let file = load_disabled_bundles_file();
    resolve_scope_hidden_ids(&file, scope)
}

/// 已加载文件 → 某 scope 的有效不可见包 id 列表（读时归一，与开关集同口径；
/// 无默认兜底，显式写入才隐藏）。
/// 供 `load_hidden_bundles_for` 与 `unavailable_bundles_for` 的单快照合并读共用。
fn resolve_scope_hidden_ids(file: &DisabledBundlesFile, scope: ConnectorScope) -> Vec<String> {
    normalize_stored_pkg_ids(
        &file
            .hidden_scopes
            .get(scope.as_str())
            .cloned()
            .unwrap_or_default(),
    )
}

/// 写某 scope 被「不可见」的包 id 列表（不参与 DenyAll 默认，显式写入才隐藏）。
/// 写失败原样上抛（用户治理状态不得静默丢写）。
pub fn save_hidden_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    with_scope_file_lock(|| {
        let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
        let mut file = load_disabled_bundles_file_locked();
        file.hidden_scopes
            .insert(scope.as_str().to_string(), normalized);
        save_disabled_bundles_file(&file)
    })
}

/// 该 scope 对底座「不可用」的包 id 并集 = 开关关（disabled）+ 不可见（hidden）。
/// 物化/工具白名单按此并集排除，两套门控对模型都是「调不到」。单次持锁读出
/// 两套集合（同一文件快照）：每次并集解析只取一次锁、只解析一次文件，也不会
/// 混读两个时刻的 disabled/hidden（同一次刷新内多次调用之间的跨调用快照窗口
/// 仍在，由各调用方自行取舍）。
pub fn unavailable_bundles_for(scope: ConnectorScope) -> Vec<String> {
    let file = load_disabled_bundles_file();
    let mut ids = resolve_scope_disabled_ids(&file, scope);
    for id in resolve_scope_hidden_ids(&file, scope) {
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

/// 写全局（plain）被禁用的包 id 列表。测试专用（生产写一律走
/// [`save_disabled_bundles_for`] 显式给 scope）。启动期 best-effort，
/// 写失败降级为日志（调用方无法处理治理写失败）。
#[cfg(test)]
pub fn save_disabled_bundles(ids: &[String]) {
    if let Err(error) = save_disabled_bundles_for(ConnectorScope::Plain, ids) {
        eprintln!("[scope] write disabled_bundles.json failed: {error}");
    }
}

/// 包安装/连接后同步所有 DenyAll 且已初始化的 scope：用户已改过这类会话开关时，
/// 新装的包默认仍保持关闭（加入该 scope 禁用集）；未初始化时无需处理（load 会按
/// 「默认全禁已装包」兜底）。AllowAll 模式无需同步（默认全开）。连接器与技能安装
/// 共用本入口：入参可为连接器 id / 技能 id / 包 id，统一归一为包 id。
pub fn sync_deny_all_scopes_after_install(raw_id: &str) {
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
            if let Err(error) = save_disabled_bundles_file(&file) {
                eprintln!("[scope] write disabled_bundles.json failed: {error}");
            }
        }
    });
}

/// Sync every scope after a bundle uninstall/disconnect: drop the id from each
/// scope's disabled and visibility sets so no stale entry keeps pointing at a
/// missing package. Shared entry point for connector, skill, and package
/// teardown: the argument may be a connector id / skill id / package id and is
/// normalized to the package id.
pub fn remove_bundle_from_disabled_scopes(raw_id: &str) {
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
            if let Err(error) = save_disabled_bundles_file(&file) {
                eprintln!("[scope] write disabled_bundles.json failed: {error}");
            }
        }
    })
}

/// 项目级 skills 开关（默认关）。
pub fn project_skills_enabled() -> bool {
    load_disabled_bundles_file().project_skills_enabled
}

/// 写项目级 skills 开关。落盘后由调用方重写在线会话组合目录。
pub fn set_project_skills_enabled(enabled: bool) {
    with_scope_file_lock(|| {
        let mut file = load_disabled_bundles_file_locked();
        if file.project_skills_enabled == enabled {
            return;
        }
        file.project_skills_enabled = enabled;
        if let Err(error) = save_disabled_bundles_file(&file) {
            eprintln!("[scope] write disabled_bundles.json failed: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::test_support::{make_dir_unreadable_for_test, with_temp_home};

    #[test]
    fn bundles_roundtrip_per_scope() {
        with_temp_home("pinvou3-scope", || {
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

    /// Unavailable = disabled + hidden, deduped; visibility writes must not
    /// pollute the disabled set (the two sets stay orthogonal).
    #[test]
    fn unavailable_is_union_deduped() {
        with_temp_home("pinvou3-scope", || {
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

    /// DenyAll（未初始化）scope 的不可用并集 = 开关集默认兜底（已装包 ∪ 内置 CLI
    /// 包）∪ 显式 hidden：hidden 不参与默认策略，但并集仍须包含它，且与默认集
    /// 相交时按并集去重（feishu 既是内置 CLI 包又是显式 hidden，只出现一次）。
    /// 合并读与两套集合各自的读入口同口径。
    #[test]
    fn unavailable_includes_hidden_in_uninitialized_deny_all_scope() {
        with_temp_home("pinvou3-scope", || {
            save_hidden_bundles_for(
                ConnectorScope::Code,
                &["weather".to_string(), "feishu".to_string()],
            )
            .unwrap();

            // Code 未初始化：开关集走 DenyAll 兜底；hidden 只追加上显式条目。
            let mut expected = load_disabled_bundles_for(ConnectorScope::Code);
            if !expected.iter().any(|id| id == "weather") {
                expected.push("weather".to_string());
            }
            assert_eq!(unavailable_bundles_for(ConnectorScope::Code), expected);
        });
    }

    /// 卸载/断开后清理残留：同时清 disabled 与 hidden 两套集合。
    #[test]
    fn remove_bundle_clears_both_sets() {
        with_temp_home("pinvou3-scope", || {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            save_hidden_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            remove_bundle_from_disabled_scopes("weather");
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert!(load_hidden_bundles_for(ConnectorScope::Plain).is_empty());
        });
    }

    /// 一次助手调用覆盖**所有** scope 的 disabled + hidden 两套集合:退役工具清理等
    /// 调用方依赖「单次调用 = 全清理面」,无需逐 scope 手工 load/retain/save
    /// (#522:逐 scope 两段式各自取锁,会在 load 与 save 之间丢并发更新)。
    #[test]
    fn remove_bundle_clears_every_scope_and_hidden_set() {
        with_temp_home("pinvou3-scope", || {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]).unwrap();
            save_disabled_bundles_for(
                ConnectorScope::Code,
                &["weather".to_string(), "pptx".to_string()],
            )
            .unwrap();
            save_hidden_bundles_for(ConnectorScope::Code, &["weather".to_string()]).unwrap();

            remove_bundle_from_disabled_scopes("weather");

            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["pptx".to_string()],
                "同 scope 其它包的条目必须原样保留"
            );
            assert!(load_hidden_bundles_for(ConnectorScope::Code).is_empty());
            // helper 不得动 scope 初始化登记:退役清理依赖该契约保留用户的
            // 初始化状态(上面三次 save 已把 plain/code 标记为 initialized)。
            let file = load_disabled_bundles_file();
            assert!(
                file.initialized.contains("plain") && file.initialized.contains("code"),
                "helper 必须保留 scope 初始化登记: {:?}",
                file.initialized
            );
        });
    }

    /// 保存路径统一归一为包 id：剥 `skill:` 前缀 + companion 映射到所属包。
    #[test]
    fn save_normalizes_to_package_id() {
        with_temp_home("pinvou3-scope", || {
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
        with_temp_home("pinvou3-scope", || {
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
        with_temp_home("pinvou3-scope", || {
            assert!(!project_skills_enabled(), "项目技能默认关");
            set_project_skills_enabled(true);
            assert!(project_skills_enabled());
            set_project_skills_enabled(false);
            assert!(!project_skills_enabled());
        });
    }

    /// 读路径的「读到即迁移落盘」必须取 `DISABLED_BUNDLES_FILE_LOCK` 与持锁写方
    /// 串行：持锁期间并发 load（磁盘为旧连接器文件、必然触发迁移落盘）不得先行落盘。
    #[test]
    fn read_path_migration_serializes_with_file_lock() {
        with_temp_home("pinvou3-scope", || {
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

    /// Physically install the preset skill government-writing (its owner is
    /// claimed as gongwen: once the gongwen record is registered as installed,
    /// `skill_owner_package` follows the package claim).
    fn install_preset_skill_under_claimed_owner() -> PathBuf {
        let skill_dir = paths::bundles_root()
            .join("gongwen")
            .join("skills")
            .join("government-writing");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# government-writing").unwrap();
        crate::features::marketplace::store::BundleStore::new()
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "gongwen".to_string(),
                    crate::features::marketplace::store::BundleSource::Preset,
                ),
            )
            .unwrap();
        skill_dir
    }

    /// Physically install an upload skill `<name>` (bundle store record with an
    /// Upload source + `bundles/<name>/skills/<name>/SKILL.md`). Upload owner
    /// packages self-map, so `<name>` itself is the package id in the deny set.
    fn install_upload_skill(name: &str) -> PathBuf {
        let skill_dir = paths::bundles_root().join(name).join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), format!("# {name}")).unwrap();
        crate::features::marketplace::store::BundleStore::new()
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    name.to_string(),
                    crate::features::marketplace::store::BundleSource::Upload(name.to_string()),
                ),
            )
            .unwrap();
        skill_dir
    }

    /// Uninitialized DenyAll (code) default deny set = installed connector
    /// packages ∪ builtin CLI packages ∪ installed skill owner packages; under
    /// a clean scan it must equal the expected list exactly (no degraded
    /// blanket may leak in — equivalently asserts "the normal path never
    /// misreports degradation").
    #[test]
    fn denyall_default_clean_scan_exact_list() {
        with_temp_home("pinvou3-scope-denyall", || {
            // Empty install state: only builtin CLI package ids (manifest order).
            let builtin: Vec<String> = builtin_cli_bundle_ids().map(str::to_string).collect();
            assert_eq!(load_disabled_bundles_for(ConnectorScope::Code), builtin);

            // Physically install one preset skill → its owner package joins the
            // default deny set.
            install_preset_skill_under_claimed_owner();
            let mut expected = builtin;
            expected.push("gongwen".to_string());
            assert_eq!(load_disabled_bundles_for(ConnectorScope::Code), expected);
        });
    }

    /// #531: with an unscannable skill directory (permissions) the enumeration
    /// degrades and the DenyAll default biases toward over-denial — an
    /// installed skill's owner package must not drop out of the default deny
    /// set, and not-installed preset owners join as conservative fallback (the
    /// user can enable them explicitly). Lenient display paths are unaffected.
    #[test]
    fn denyall_default_degraded_scan_biases_to_overdeny() {
        with_temp_home("pinvou3-scope-denyall-degraded", || {
            let skill_dir = install_preset_skill_under_claimed_owner();

            // Clean baseline: the claimed owner package is denied by default,
            // the not-installed preset owner (pptx) is not.
            let clean = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(clean.contains(&"gongwen".to_string()));
            assert!(!clean.contains(&"pptx".to_string()));

            // Make the skill dir unreadable → the SKILL.md probe hits EACCES:
            // the lenient path would read "not installed" (fail-open), the
            // strict enumeration reports degradation and biases to over-deny.
            // Skip the assertions when the platform/environment cannot simulate
            // unreadable dirs (Windows, root).
            let Some(_unreadable) = make_dir_unreadable_for_test(&skill_dir) else {
                return;
            };
            let degraded = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                degraded.contains(&"gongwen".to_string()),
                "degraded enumeration must keep the installed skill's owner package denied: {degraded:?}"
            );
            assert!(
                degraded.contains(&"pptx".to_string()),
                "degradation must bias to over-deny (not-installed preset owner joins): {degraded:?}"
            );
        });
    }

    /// #531 for the upload half: an installed upload skill with an unscannable
    /// directory must not drop out of the default deny set either. The probe
    /// failure degrades the enumeration, the degraded blanket union carries the
    /// upload's own owner package (store readable), and the preset owners (pptx)
    /// join as conservative fallback.
    #[test]
    fn denyall_default_degraded_upload_scan_biases_to_overdeny() {
        with_temp_home("pinvou3-scope-upload-degraded", || {
            let skill_dir = install_upload_skill("my-weather");

            // Clean baseline: the upload's self-mapped owner package is denied.
            let clean = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                clean.contains(&"my-weather".to_string()),
                "installed upload owner package must be denied by default: {clean:?}"
            );

            let Some(_unreadable) = make_dir_unreadable_for_test(&skill_dir) else {
                return;
            };
            let degraded = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                degraded.contains(&"my-weather".to_string()),
                "degraded enumeration must keep the installed upload owner package denied: {degraded:?}"
            );
            assert!(
                degraded.contains(&"pptx".to_string()),
                "degradation must bias to over-deny (not-installed preset owner joins): {degraded:?}"
            );
        });
    }

    /// A corrupt bundle store (fail-loud read) makes the upload population
    /// unknowable: the enumeration must degrade into the preset-owner blanket
    /// union (pptx joins) instead of silently passing as an empty install set.
    #[test]
    fn denyall_default_corrupt_store_biases_to_overdeny() {
        with_temp_home("pinvou3-scope-store-corrupt", || {
            install_upload_skill("my-weather");
            let store_file = paths::pinvou3_home()
                .join("marketplace")
                .join("bundles.json");
            std::fs::write(&store_file, "{not json").unwrap();

            let degraded = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                degraded.contains(&"pptx".to_string()),
                "corrupt store must degrade into the preset-owner blanket union: {degraded:?}"
            );
        });
    }

    /// An unscannable packages root blinds both the claimed-dir context and the
    /// straggler scan: the enumeration must degrade, and the preset-owner
    /// blanket union must keep the installed preset's owner (gongwen) denied
    /// alongside the not-installed preset owners (pptx).
    #[test]
    fn denyall_default_packages_root_failure_biases_to_overdeny() {
        with_temp_home("pinvou3-scope-root-degraded", || {
            install_preset_skill_under_claimed_owner();
            let bundles_root = paths::bundles_root();

            let Some(_unreadable) = make_dir_unreadable_for_test(&bundles_root) else {
                return;
            };
            let degraded = load_disabled_bundles_for(ConnectorScope::Code);
            assert!(
                degraded.contains(&"gongwen".to_string()),
                "root-scan failure must keep the installed preset owner denied: {degraded:?}"
            );
            assert!(
                degraded.contains(&"pptx".to_string()),
                "root-scan failure must degrade into the blanket union: {degraded:?}"
            );
        });
    }

    /// A stray regular file in the packages root must not read as degradation:
    /// ENOTDIR on a joined candidate structurally answers "not there" (same
    /// family as NotFound), so the clean exact list holds without the degraded
    /// blanket.
    #[test]
    fn denyall_default_tolerates_stray_file_without_degradation() {
        with_temp_home("pinvou3-scope-stray-file", || {
            install_preset_skill_under_claimed_owner();
            let stray = paths::bundles_root().join("stray-file.txt");
            std::fs::write(&stray, "not a package").unwrap();

            let mut expected: Vec<String> = builtin_cli_bundle_ids().map(str::to_string).collect();
            expected.push("gongwen".to_string());
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                expected,
                "stray file must not flip the default into degraded over-deny"
            );
        });
    }

    /// #531 boundary: an initialized scope sticks to its persisted list; skill
    /// enumeration degradation must not affect it (the over-denial fallback
    /// only applies to the freshly computed default of an uninitialized scope).
    #[test]
    fn initialized_scope_ignores_degraded_skill_scan() {
        with_temp_home("pinvou3-scope-initialized", || {
            save_disabled_bundles_for(ConnectorScope::Code, &["weather".to_string()]).unwrap();
            let skill_dir = install_preset_skill_under_claimed_owner();
            let Some(_unreadable) = make_dir_unreadable_for_test(&skill_dir) else {
                return;
            };
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["weather".to_string()],
                "an initialized scope sticks to its persisted list, unaffected by degradation"
            );
        });
    }

    /// #515：跨进程文件锁守门——另一进程（用同进程内独立 fd 模拟：flock 按 open
    /// file description 归属，不同 fd 持锁同样互斥）已持有锁文件时，本进程读路径的
    /// 迁移落盘必须等其释放后才发生。
    #[test]
    fn cross_process_lock_blocks_migration_write_until_release() {
        with_temp_home("pinvou3-scope-cross-process", || {
            let legacy = r#"["weather"]"#;
            let conn = paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(&conn, legacy).unwrap();

            // 「另一进程」持有的锁：独立 open 的锁文件 + fd_lock 写锁。
            let foreign_file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(disabled_bundles_lock_path())
                .unwrap();
            let mut foreign_lock = fd_lock::RwLock::new(foreign_file);
            let foreign_guard = foreign_lock
                .write()
                .expect("测试内应能取得跨进程锁文件写锁");

            let reader = std::thread::spawn(load_disabled_bundles_for_plain_for_lock_test);
            std::thread::sleep(std::time::Duration::from_millis(200));
            // 持锁期间 bundles 文件尚未写入（迁移被跨进程锁串行化）。sleep 只给
            // reader 到达持锁点的调度窗口：未到点时此断言真空通过（弱化强度），
            // 但锁失效时迁移必然在窗口内落盘使断言失败——不会假阴性/flaky fail。
            assert!(
                !disabled_bundles_path().exists(),
                "跨进程锁持有期间读路径不得先行迁移落盘"
            );
            drop(foreign_guard);
            assert_eq!(reader.join().unwrap(), vec!["weather".to_string()]);
            let content = std::fs::read_to_string(disabled_bundles_path()).unwrap();
            assert!(
                content.contains("\"scopes\""),
                "释放锁后迁移完成: {content}"
            );
        });
    }

    /// #515 对称用例：读/迁移路径之外，写路径（`save_disabled_bundles_for`）同样
    /// 经 `with_scope_file_lock`——foreign 持锁期间写方不得先行落盘，释放后写入
    /// 内容完整。释放后的断言无竞态（join 确定性收束）。
    #[test]
    fn cross_process_lock_blocks_save_write_until_release() {
        with_temp_home("pinvou3-scope-cross-process-save", || {
            // 「另一进程」持有的锁：独立 open 的锁文件 + fd_lock 写锁。
            let foreign_file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(disabled_bundles_lock_path())
                .unwrap();
            let mut foreign_lock = fd_lock::RwLock::new(foreign_file);
            let foreign_guard = foreign_lock
                .write()
                .expect("测试内应能取得跨进程锁文件写锁");

            let writer = std::thread::spawn(|| {
                save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
            });
            std::thread::sleep(std::time::Duration::from_millis(200));
            // 持锁期间写方被跨进程锁串行化，数据文件不得先行落盘。
            assert!(
                !disabled_bundles_path().exists(),
                "跨进程锁持有期间写路径不得先行落盘"
            );
            drop(foreign_guard);
            writer.join().unwrap();
            let content = std::fs::read_to_string(disabled_bundles_path()).unwrap();
            assert!(
                content.contains("\"weather\""),
                "释放锁后写入完成: {content}"
            );
        });
    }
}
