//! 包 id × SessionMode 的单一禁用集（`~/.pinvou3/disabled_bundles.json`）。
//!
//! 这是「工具市场统一治理」scope 收敛（todo A 节）的单一真相源：取代原先
//! `disabled_connectors.json`（连接器 id）与 `disabled_skills.json`（技能 id）两份
//! 文件。开关粒度收敛为**包 id**（= `bundle.rs` 里 `BundleInfo.id`，即 MCP 工具 id /
//! 技能 id / CLI 连接器 id），一个包 = 一个开关，包内技能（companion skills）可见性
//! 唯一跟随所属包（§5.2 不变量）。
//!
//! 落盘格式与 #287 泛化后的两份旧文件同构：`{scopes: {"<mode>": [...]},
//! "initialized": ["<mode>"], project_skills_enabled, plain_defaults_migrated}`，
//! scope 键即 `SessionMode` 的 kebab-case 名。`plain_defaults_migrated` 是 plain
//! scope 默认策略迁移标记：旧版文件（false）= 写于 plain 仍 AllowAll 的时代，
//! 读到即迁移把 plain 初始化为落盘列表后落盘置位。首个版本读取时把两份旧文件迁移到本文件（读到即迁移）：
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
    /// 项目级 skills 是否对 code 会话开启（默认关）。随技能侧迁入本文件。
    #[serde(default)]
    pub project_skills_enabled: bool,
    /// plain scope 默认策略迁移标记：false（旧版文件无此字段）= 文件写于 plain
    /// 仍 AllowAll 的时代，读时迁移会把 plain 初始化为落盘列表（锁定当时实际的
    /// 开/关状态）后置 true——存量用户升级后开关状态不变；全新装机首读置 true
    /// 不初始化 plain，且**同样落盘冻结判定**（首启自写 settings.json/sessions
    /// 会污染升级信号，见读路径注释），plain 未初始化时按 DenyAll 兜底（默认全关）。
    #[serde(default)]
    pub plain_defaults_migrated: bool,
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
///
/// plain 默认策略迁移（工具开关全量收敛 DenyAll）：旧版文件（无
/// `plain_defaults_migrated` 字段）或旧版双文件时代（legacy 文件存在）= 升级
/// 装机，把 plain 初始化为落盘列表——其有效状态即旧 AllowAll 语义下的真实开关
/// 状态（缺省空 = 全开），升级后用户无感；全新装机只置标记不初始化，plain 未
/// 初始化按 DenyAll 兜底（默认全关）。
///
/// 「全新装机」的判定不能只看本文件与两份 legacy 文件：统一文件自 v0.8.6
/// 起就存在、且只在有内容可写时才落盘——老装机 + 从未动过开关的用户可能
/// 三者皆无。因此升级信号放宽为「任何既有 pinvou3_home 痕迹」：
/// marketplace/installed.json、settings.json 或已有会话目录，任一存在即视
/// 为升级装机并保留旧 AllowAll 语义（评审 #445 P1-2）。全空家目录才算全新。
///
/// 该宽口径信号会被应用自身的首启行为污染（bridge boot 的 ensure_dirs 自写
/// sessions/default/artifacts/、缺省补写默认 settings.json），因此首读被
/// 上提至 Tauri setup 钩子顶部（lib.rs `disabled_bundles_migration` 标记，
/// 早于一切首启自写痕迹），且「升级 vs 全新」的判定在**首次读取时无条件
/// 落盘**（置 `plain_defaults_migrated` 标记）冻结——否则全新装机会被
/// 首启痕迹误判为升级装机而翻回全开（评审 #455 阻塞项）。
fn load_disabled_bundles_file_locked() -> DisabledBundlesFile {
    let path = disabled_bundles_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            let home = paths::pinvou3_home();
            let legacy_existed = home.join("disabled_connectors.json").exists()
                || home.join("disabled_skills.json").exists();
            // 宽口径升级信号:三份开关相关文件皆无,但家目录有其他状态
            // (安装记录/设置/会话) ⇒ 老装机,plain 保持旧 AllowAll 语义。
            let upgraded_install = legacy_existed
                || home.join("marketplace").join("installed.json").is_file()
                || paths::settings_path().is_file()
                || paths::sessions_root()
                    .read_dir()
                    .map(|mut entries| entries.next().is_some())
                    .unwrap_or(false);
            let mut file = migrate_from_legacy_files();
            if upgraded_install {
                // 升级：初始化 plain（scopes 缺省空 = 旧语义全开），锁定升级前状态。
                file.initialized
                    .insert(SessionMode::Plain.as_str().to_string());
            }
            file.plain_defaults_migrated = true;
            // 无条件落盘冻结判定：全新装机不持久化标记时，首启自写的
            // settings.json/sessions/default 会污染宽口径升级信号，次读即被
            // 误判为升级装机而翻回全开（评审 #455 阻塞项）。
            save_disabled_bundles_file(&file);
            return file;
        }
    };
    let mut file: DisabledBundlesFile = match serde_json::from_str(&content) {
        Ok(file) => file,
        Err(error) => {
            // 损坏文件不静默覆盖:先留 .corrupt.<ts> 隔离副本(installed.json
            // 同款),再把降级态**覆盖落盘**——恢复须一次性完成,否则损坏文件
            // 留在盘上、每次读都重新隔离,副本无界累积(评审 #455 阻塞项)。
            // 恢复必须 fail-closed：按空状态降级会把带迁移标记的新格式文件
            // （用户可能显式关过包）恢复成全开；安全收敛特性宁可恢复全关——
            // 只置迁移标记（冻结为全新装机判定）、不初始化任何 scope，未初始化
            // scope 按 DenyAll 兜底（评审 #455）。
            quarantine_corrupt_disabled_bundles(&content, &error.to_string());
            let recovered = DisabledBundlesFile {
                plain_defaults_migrated: true,
                ..DisabledBundlesFile::default()
            };
            save_disabled_bundles_file(&recovered);
            recovered
        }
    };
    if !file.plain_defaults_migrated {
        file.initialized
            .insert(SessionMode::Plain.as_str().to_string());
        file.plain_defaults_migrated = true;
        save_disabled_bundles_file(&file);
    }
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

/// 写完整文件（原子替换，与旧文件同范式）。先建父目录：迁移冻结/损坏恢复的
/// 首次落盘可能早于任何建目录的启动步骤，`write_atomic` 不代建父目录，写失败
/// 会让「全新 vs 升级」判定静默解冻、次读 fail-open（评审 #455）。
fn save_disabled_bundles_file(file: &DisabledBundlesFile) {
    let path = disabled_bundles_path();
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("[scope] create parent dir for disabled_bundles.json failed: {error}");
            return;
        }
    }
    if let Ok(json) = serde_json::to_string(file) {
        if let Err(error) = deepseek_tui::utils::write_atomic(&path, json.as_bytes()) {
            eprintln!("[scope] write disabled_bundles.json failed: {error}");
        }
    }
}

/// 读某 scope 被禁用的**包 id** 列表（读不到/空 → 空）。
///
/// 已初始化的 scope 以落盘列表为准；未初始化的 scope 按 DenyAll 兜底（全部
/// 已安装包 id ∪ 全部内置 CLI 包 id）——「默认全关，外部能力显式开启」。
/// 全部模式均 DenyAll；plain 的存量装机由 `load_disabled_bundles_file_locked`
/// 的读时迁移初始化（锁定升级前开关状态），不走此兜底。CLI 包未连接时纳入
/// 无害（配套技能不在盘上，排除为空操作），且「后才连接」也自动默认关。
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
pub fn save_disabled_bundles_for(scope: ConnectorScope, ids: &[String]) {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
    let mut file = load_disabled_bundles_file_locked();
    let key = scope.as_str().to_string();
    file.scopes.insert(key.clone(), normalized);
    file.initialized.insert(key);
    save_disabled_bundles_file(&file);
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
pub fn save_hidden_bundles_for(scope: ConnectorScope, ids: &[String]) {
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let normalized: Vec<String> = ids.iter().map(|id| to_package_id(id)).collect();
    let mut file = load_disabled_bundles_file_locked();
    file.hidden_scopes
        .insert(scope.as_str().to_string(), normalized);
    save_disabled_bundles_file(&file);
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
pub fn save_disabled_bundles(ids: &[String]) {
    save_disabled_bundles_for(ConnectorScope::Plain, ids);
}

/// 包安装/连接后同步所有已初始化的 scope：用户已改过开关时，新装的包默认仍
/// 保持关闭（加入该 scope 禁用集）；未初始化时无需处理（load 会按「默认全禁
/// 已装包」兜底）。全部模式均 DenyAll（plain 由读时迁移初始化后同样进同步）。
/// 连接器与技能安装共用本入口：入参可为连接器 id / 技能 id / 包 id，统一归一
/// 为包 id。
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
        save_disabled_bundles_file(&file);
    }
}

/// 包卸载/断开后同步所有 scope：从各 scope 禁用集与可见性集移除该包 id，避免残留
/// 指向不存在的包。连接器与技能卸载共用本入口：入参可为连接器 id / 技能 id / 包 id，
/// 统一归一为包 id。
/// composer 连接器开关 ↔ 统一禁用集桥接（二轮评审：CLI 三数据源无桥接）。
/// 连接器停用标志（`<connector>_disabled` 文件）只删技能目录，而 execpolicy CLI
/// 硬拦截与技能物化排除读 `disabled_bundles.json`——开关关掉连接器时必须同步把
/// 包 id 写入所有 scope 的禁用集，开回时必须把它从有效禁用集移除，两条门控才一致。
pub fn sync_disabled_bundles_for_connector_switch(connector_id: &str, enabled: bool) {
    if enabled {
        enable_bundle_in_deny_all_scopes(connector_id);
        return;
    }
    // 单临界区 RMW（四轮评审 M-6b）：逐 scope 独立 load→save 两次加锁会在跨临界区
    // 窗口丢并发写（lost-update），与文件内其它写方同范式——持锁读 → 改 → 一次落盘。
    let _guard = DISABLED_BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_bundles_file_locked();
    let mut changed = false;
    for mode in SessionMode::ALL {
        let mut ids = resolve_scope_disabled_ids(&file, *mode);
        if ids.iter().any(|id| id == connector_id) {
            continue;
        }
        // id 不在有效禁用集（未初始化 scope 按现算扩集兜底，未装的预置包不在
        // 扩集里）：落盘初始化会把当前扩集固化为用户状态，未来新增的内置包将
        // 默认开——与「用户没碰过的包默认关」语义相悖，不动（评审 #455）。
        if !file.initialized.contains(mode.as_str()) {
            continue;
        }
        ids.push(connector_id.to_string());
        let key = mode.as_str().to_string();
        file.scopes.insert(key.clone(), ids);
        file.initialized.insert(key);
        changed = true;
    }
    if changed {
        save_disabled_bundles_file(&file);
    }
}

/// 连接器开回（enabled=true）方向的桥接：必须让包 id 退出**有效**禁用集。
/// 只编辑落盘列表对未初始化 scope 是静默 no-op——其门控来自 DenyAll 现算扩集
/// （无视落盘列表），UI 显示「已启用」而 CLI 硬拦截/技能排除依旧（评审 #455
/// 阻塞项）。因此对每个「包 id 在现算扩集里且未初始化」的 scope，以
/// 扩集减该 id 初始化（用户刚显式开启 = 显式 opt-in）；已初始化的 scope
/// 保持原有从落盘列表移除的行为；同时清理可见性集残留。
fn enable_bundle_in_deny_all_scopes(raw_id: &str) {
    // 与旧 enable 路径（save/remove 等写方）同口径：入参统一归一为包 id
    // （剥 `skill:` 前缀 + companion 映射），防御非内置 id 调用方。
    let connector_id = to_package_id(raw_id);
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
        if file.initialized.contains(key) {
            continue;
        }
        let mut ids = resolve_scope_disabled_ids(&file, *mode);
        let before = ids.len();
        ids.retain(|id| id != &connector_id);
        if ids.len() == before {
            continue;
        }
        file.scopes.insert(key.to_string(), ids);
        file.initialized.insert(key.to_string());
        changed = true;
    }
    // 已初始化 scope 与可见性集：从落盘列表移除（与卸载清理同路径）。
    for ids in file.scopes.values_mut() {
        let before = ids.len();
        ids.retain(|id| id != &connector_id);
        changed |= ids.len() != before;
    }
    for ids in file.hidden_scopes.values_mut() {
        let before = ids.len();
        ids.retain(|id| id != &connector_id);
        changed |= ids.len() != before;
    }
    if changed {
        save_disabled_bundles_file(&file);
    }
}

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
        save_disabled_bundles_file(&file);
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
    save_disabled_bundles_file(&file);
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
            // 全模式 DenyAll 后 fresh home 未初始化 scope 默认全关（含内置 CLI
            // 包）；本测试聚焦 per-scope 读写 roundtrip，先显式初始化 plain 为空集。
            save_disabled_bundles_for(ConnectorScope::Plain, &[]);
            assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
            save_disabled_bundles_for(ConnectorScope::Code, &["feishu".to_string()]);
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
            // 显式初始化 plain 为空集（DenyAll 收敛后 fresh home 未初始化默认
            // 全关，hidden 正交性断言需要空 disabled 基线）。
            save_disabled_bundles_for(ConnectorScope::Plain, &[]);
            save_hidden_bundles_for(ConnectorScope::Plain, &["combo-demo".to_string()]);
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

    /// 并集 = 开关关 + 不可见，去重。
    #[test]
    fn unavailable_is_union_deduped() {
        with_temp_home(|| {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
            save_hidden_bundles_for(
                ConnectorScope::Plain,
                &["weather".to_string(), "pptx".to_string()],
            );
            let mut u = unavailable_bundles_for(ConnectorScope::Plain);
            u.sort();
            assert_eq!(u, vec!["pptx".to_string(), "weather".to_string()]);
        });
    }

    /// 卸载/断开后清理残留：同时清 disabled 与 hidden 两套集合。
    #[test]
    fn remove_bundle_clears_both_sets() {
        with_temp_home(|| {
            save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
            save_hidden_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
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
            );
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
            );
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
            save_disabled_bundles_for(ConnectorScope::Plain, &["government-writing".to_string()]);
            save_hidden_bundles_for(ConnectorScope::Plain, &["government-writing".to_string()]);
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

/// 把损坏的 disabled_bundles.json 原始字节隔离成 `.corrupt.<ts>` 副本
/// （与 installed.json 共用 `quarantine_corrupt_state_file`），随后由读路径
/// fail-closed 降级自愈。
fn quarantine_corrupt_disabled_bundles(content: &str, error: &str) {
    let path = disabled_bundles_path();
    super::quarantine_corrupt_state_file(&path, content);
    eprintln!(
        "[marketplace] disabled_bundles.json was corrupt ({error}); quarantined and reset to fail-closed defaults"
    );
}
