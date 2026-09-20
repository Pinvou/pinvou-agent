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

/// `disabled_bundles.json` 读-改-写的进程内串行化。
static DISABLED_BUNDLES_FILE_LOCK: Mutex<()> = Mutex::new(());

/// 读完整文件（取文件锁）。可能触发「读到即迁移」的读路径必须走本入口与持锁写方
/// 串行（与旧两份文件的 #287 竞态范式一致）。
pub(crate) fn load_disabled_bundles_file() -> DisabledBundlesFile {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    load_disabled_bundles_file_locked()
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
/// 写失败原样上抛（用户治理状态不得静默丢写）。
pub fn save_disabled_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
    let mut file = load_disabled_bundles_file_locked();
    let key = scope.as_str().to_string();
    file.scopes.insert(key.clone(), normalized);
    file.initialized.insert(key);
    save_disabled_bundles_file(&file)
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
/// 写失败原样上抛（用户治理状态不得静默丢写）。
pub fn save_hidden_bundles_for(scope: ConnectorScope, ids: &[String]) -> Result<(), String> {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
    let mut file = load_disabled_bundles_file_locked();
    file.hidden_scopes
        .insert(scope.as_str().to_string(), normalized);
    save_disabled_bundles_file(&file)
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

/// 写全局（plain）被禁用的包 id 列表。兼容既有调用方；启动期 best-effort，
/// 写失败降级为日志（调用方无法处理治理写失败）。
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
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
}

/// Sync every scope after a bundle uninstall/disconnect: drop the id from each
/// scope's disabled and visibility sets so no stale entry keeps pointing at a
/// missing package. Shared entry point for connector, skill, and package
/// teardown: the argument may be a connector id / skill id / package id and is
/// normalized to the package id.
pub fn remove_bundle_from_disabled_scopes(raw_id: &str) {
    let package_id = to_package_id(raw_id);
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
}

/// 项目级 skills 开关（默认关）。
pub fn project_skills_enabled() -> bool {
    load_disabled_bundles_file().project_skills_enabled
}

/// 写项目级 skills 开关。落盘后由调用方重写在线会话组合目录。
pub fn set_project_skills_enabled(enabled: bool) {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_bundles_file_locked();
    if file.project_skills_enabled == enabled {
        return;
    }
    file.project_skills_enabled = enabled;
    if let Err(error) = save_disabled_bundles_file(&file) {
        eprintln!("[scope] write disabled_bundles.json failed: {error}");
    }
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

    /// Unavailable = disabled + hidden, deduped; visibility writes must not
    /// pollute the disabled set (the two sets stay orthogonal).
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
            remove_bundle_from_disabled_scopes("weather");
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
