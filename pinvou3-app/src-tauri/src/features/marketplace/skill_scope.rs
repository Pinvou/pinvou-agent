//! 技能开关按模式 scope 持久化（`~/.pinvou3/disabled_skills.json`）。
//!
//! 与连接器开关（`disabled_connectors.json`，本模块上级）同构：`{scopes:
//! {"<mode>": [...]}, "initialized": ["<mode>"], project_skills_enabled}`，scope
//! 键即 `SessionMode` 的 kebab-case 名。旧数据迁移（裸数组 → plain scope；旧版
//! 借道 `disabled_connectors.json` 的 `skill:<id>` 条目 → 提取进 plain 并清除
//! 连接器文件残留；旧双 scope 对象 `{plain, code, code_initialized}` → 新 map，
//! 读到即落盘）。组合目录物化消费本层（`features/assistant/skill_materialization.rs`）。
//!
//! 放在 marketplace 而不是 assistant：skill 开关是「技能市场」的持久化数据，
//! 与连接器开关同领域；connectors（ima 连接/退出）也会读写它，若放 assistant
//! 会形成 connectors → assistant 的依赖环（架构守卫 rust_feature_cycles）。

use std::path::PathBuf;
use std::sync::Mutex;

use crate::core::session_mode::{PackDefaultPolicy, SessionMode};
use crate::features::marketplace::skill_marketplace::SkillMarketplaceManager;
use crate::features::marketplace::ConnectorScope;
use crate::platform::paths;

/// 按模式 scope 键控的技能禁用列表。
///
/// 某 scope 遵循其模式的包默认策略（`SessionMode::pack_default_policy`）：
/// `initialized` 不含该 scope 时（用户从未改过这类会话的技能开关），DenyAll
/// 模式默认禁用**所有已安装技能**（外部能力显式开启）；一旦用户改过该 scope
/// 的开关（进入 `initialized`），就以落盘列表为准。与连接器
/// `DisabledConnectorsFile` 同构（§8.3 范本）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DisabledSkillsFile {
    /// scope（模式 kebab-case 名）→ 该 scope 被禁用的技能 id 列表。
    #[serde(default)]
    pub scopes: std::collections::BTreeMap<String, Vec<String>>,
    /// 已被用户显式初始化（改过开关）的 scope 集合。
    #[serde(default)]
    pub initialized: std::collections::BTreeSet<String>,
    /// 项目级 skills 是否对 code 会话开启（默认关：项目内文本是 prompt-injection
    /// 面，开启需用户显式确认并看到注入风险警告）。开启后，绑项目的 code 会话
    /// 组合目录额外包含项目 `.agents/skills` 等工具约定目录（§2.4 兜底路径：
    /// fork #41 已砍断 workspace 并集扫描，项目技能经同一物化通道拷入组合目录）。
    #[serde(default)]
    pub project_skills_enabled: bool,
    /// 未知键原样保留（前向兼容：新版写入的字段经旧版读写后不丢失）。
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

fn disabled_skills_path() -> PathBuf {
    paths::pinvou3_home().join("disabled_skills.json")
}

/// `disabled_skills.json` 读-改-写的进程内串行化：开关命令、安装/卸载同步可能
/// 并发触发同一份文件的读-改-写，串行化避免交错丢更新（与连接器文件同一范式）。
static DISABLED_SKILLS_FILE_LOCK: Mutex<()> = Mutex::new(());

/// 读完整文件。兼容三种旧数据，首次读到时迁移并落盘：
///  1. 裸数组 `["a","b"]`（方案 §2.1 的旧 disabled_skills.json 形态）→ plain scope；
///  2. 旧版借道 `disabled_connectors.json` 的 `skill:<id>` 条目（本分支历史实现）
///     → 提取进 plain scope，并从连接器文件清除 `skill:` 残留；
///  3. 旧双 scope 对象 `{plain, code, code_initialized, ...}` → scopes map +
///     initialized 集合（顶层带 `scopes` 键的即新格式，直接解析）。
/// 迁移失败按「全部启用（plain）+ DenyAll 模式默认全禁」安全兜底（全部落空 → 默认值）。
fn load_disabled_skills_file() -> DisabledSkillsFile {
    let path = disabled_skills_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            // 首次启动：从旧连接器文件的 skill: 条目迁移（一次）。
            let file = migrate_legacy_skill_ids();
            if file
                .scopes
                .get(SessionMode::Plain.as_str())
                .map(|ids| !ids.is_empty())
                .unwrap_or(false)
            {
                save_disabled_skills_file(&file);
            }
            return file;
        }
    };
    if let Ok(legacy) = serde_json::from_str::<Vec<String>>(&content) {
        // 与对象格式读路径一致：剥除 `skill:` 前缀（旧前端 bug 窗口期误写入的
        // 带前缀 id），否则 `model_skill_names` 按裸 id 映射不到目录名，
        // 该技能当次启动漏禁（下次读取自愈）。
        let plain = legacy
            .into_iter()
            .map(|id| id.strip_prefix("skill:").map(str::to_string).unwrap_or(id))
            .collect();
        let mut file = DisabledSkillsFile::default();
        file.scopes
            .insert(SessionMode::Plain.as_str().to_string(), plain);
        save_disabled_skills_file(&file);
        return file;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return DisabledSkillsFile::default();
    };
    if value.is_object() && value.get("scopes").is_none() {
        // 旧双 scope 对象 `{plain, code, code_initialized, project_skills_enabled}`
        // → 新 map；读到即落盘新格式。
        let mut file = DisabledSkillsFile::default();
        if let Some(obj) = value.as_object() {
            for mode in SessionMode::ALL {
                if let Some(ids) = obj
                    .get(mode.as_str())
                    .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
                {
                    file.scopes.insert(mode.as_str().to_string(), ids);
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
            file.project_skills_enabled = obj
                .get("project_skills_enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // 未知键保留（前向兼容）；已消费的旧键不带入。
            for (key, v) in obj {
                if !matches!(
                    key.as_str(),
                    "plain" | "code" | "code_initialized" | "project_skills_enabled"
                ) {
                    file.extra.insert(key.clone(), v.clone());
                }
            }
        }
        strip_skill_prefixes(&mut file);
        save_disabled_skills_file(&file);
        return file;
    }
    let mut file: DisabledSkillsFile = serde_json::from_value(value).unwrap_or_default();
    strip_skill_prefixes(&mut file);
    file
}

/// 防御：剥除所有 scope 禁用集里的 `skill:` 前缀（旧前端 bug 窗口期误写入的
/// 带前缀 id；物化/展示/UI 判定全部按裸 id 匹配，读者在此统一归一）。
fn strip_skill_prefixes(file: &mut DisabledSkillsFile) {
    for ids in file.scopes.values_mut() {
        for id in ids.iter_mut() {
            if let Some(stripped) = id.strip_prefix("skill:") {
                *id = stripped.to_string();
            }
        }
    }
}

/// 用迁移来的 plain 禁用集构造文件（旧数据的全局/进程级语义 → plain scope 透明迁移）。
fn file_with_plain_scope(plain: Vec<String>) -> DisabledSkillsFile {
    let mut file = DisabledSkillsFile::default();
    if !plain.is_empty() {
        file.scopes
            .insert(SessionMode::Plain.as_str().to_string(), plain);
    }
    file
}

/// 旧版技能开关借道 `disabled_connectors.json` 的 `skill:<id>` 条目（本分支
/// `refresh_disabled_skills` 的历史实现）。迁移：strip 前缀进 plain scope，并清除
/// 连接器文件里的 `skill:` 残留（避免两处真相）。旧语义是进程级全局禁用集，
/// 全局 → plain scope 透明迁移。
/// 连接器文件的三种形态都兼容：裸数组、旧双 scope 对象（顶层 plain/code）、
/// 新 scopes map（`scopes.<mode>`）。
fn migrate_legacy_skill_ids() -> DisabledSkillsFile {
    let conn_path = paths::pinvou3_home().join("disabled_connectors.json");
    let Ok(content) = std::fs::read_to_string(&conn_path) else {
        return DisabledSkillsFile::default();
    };
    let mut plain = Vec::new();
    let mut legacy_removed = false;
    // 兼容旧裸数组格式（连接器禁用 id 列表，可能含 skill: 前缀）
    if let Ok(list) = serde_json::from_str::<Vec<String>>(&content) {
        let kept: Vec<String> = list
            .into_iter()
            .filter(|id| {
                if let Some(skill_id) = id.strip_prefix("skill:") {
                    plain.push(skill_id.to_string());
                    legacy_removed = true;
                    false
                } else {
                    true
                }
            })
            .collect();
        if legacy_removed {
            if let Ok(json) = serde_json::to_string(&kept) {
                let _ = deepseek_tui::utils::write_atomic(&conn_path, json.as_bytes());
            }
        }
        return file_with_plain_scope(plain);
    }
    // 对象格式（旧双 scope 对象或新 scopes map）：同样剥离各 scope 数组里的
    // skill: 前缀条目。
    if let Ok(mut file) = serde_json::from_str::<serde_json::Value>(&content) {
        if let Some(obj) = file.as_object_mut() {
            if let Some(scopes) = obj.get_mut("scopes").and_then(|v| v.as_object_mut()) {
                for arr in scopes.values_mut() {
                    if let Some(arr) = arr.as_array_mut() {
                        legacy_removed |= extract_skill_prefixed(arr, &mut plain);
                    }
                }
            } else {
                for key in ["plain", "code"] {
                    if let Some(arr) = obj.get_mut(key).and_then(|v| v.as_array_mut()) {
                        legacy_removed |= extract_skill_prefixed(arr, &mut plain);
                    }
                }
            }
            if legacy_removed {
                let _ = deepseek_tui::utils::write_atomic(
                    &conn_path,
                    serde_json::to_string(obj).unwrap_or_default().as_bytes(),
                );
            }
        }
    }
    file_with_plain_scope(plain)
}

/// 从数组里提取 `skill:` 前缀条目（strip 后进 `plain`），原数组只留其余条目。
/// 返回是否有残留被清除。
fn extract_skill_prefixed(arr: &mut Vec<serde_json::Value>, plain: &mut Vec<String>) -> bool {
    let mut removed = false;
    let mut kept: Vec<serde_json::Value> = Vec::with_capacity(arr.len());
    for item in arr.drain(..) {
        if let Some(skill_id) = item.as_str().and_then(|s| s.strip_prefix("skill:")) {
            plain.push(skill_id.to_string());
            removed = true;
            continue;
        }
        kept.push(item);
    }
    *arr = kept;
    removed
}

/// 写完整文件。临时文件 + rename 原子替换（与 `save_disabled_connectors_file`
/// 同范式，走底座 `write_atomic` 含 Windows 替换重试）。
fn save_disabled_skills_file(file: &DisabledSkillsFile) {
    if let Ok(json) = serde_json::to_string(file) {
        if let Err(error) =
            deepseek_tui::utils::write_atomic(&disabled_skills_path(), json.as_bytes())
        {
            eprintln!("[skill-scope] write disabled_skills.json failed: {error}");
        }
    }
}

/// 读某 scope 被禁用的技能 id 列表（市场 id；读不到/空 → 空）。
///
/// 已初始化的 scope 以落盘列表为准；未初始化的 scope 按其模式的包默认策略
/// 兜底：DenyAll（如 code）返回全部已安装技能 id ——「默认全禁，外部能力
/// 显式开启」的安全默认（与连接器同语义）；AllowAll（如 plain）返回落盘列表
/// （缺省空 = 全开）。
pub fn load_disabled_skills_for(scope: ConnectorScope) -> Vec<String> {
    let file = load_disabled_skills_file();
    let key = scope.as_str();
    if file.initialized.contains(key) {
        return file.scopes.get(key).cloned().unwrap_or_default();
    }
    match scope.pack_default_policy() {
        // AllowAll 无「默认全禁」兜底：落盘列表即真相（旧格式迁移来的 plain
        // 列表即使未标记 initialized 也必须生效）。
        PackDefaultPolicy::AllowAll => file.scopes.get(key).cloned().unwrap_or_default(),
        PackDefaultPolicy::DenyAll => SkillMarketplaceManager::new().installed_skill_ids(),
    }
}

/// 写某 scope 被禁用的技能 id 列表（写入即标记该 scope 已初始化）。
/// 入参统一剥 `skill:` 命名空间前缀（前端行 id 带前缀，物化/匹配按裸 id；
/// 防御历史版本误写入的带前缀条目在下次保存时被清洗）。
pub fn save_disabled_skills_for(scope: ConnectorScope, ids: &[String]) {
    let _guard = DISABLED_SKILLS_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let normalized: Vec<String> = ids
        .iter()
        .map(|id| id.strip_prefix("skill:").unwrap_or(id).to_string())
        .collect();
    let mut file = load_disabled_skills_file();
    let key = scope.as_str().to_string();
    file.scopes.insert(key.clone(), normalized);
    file.initialized.insert(key);
    save_disabled_skills_file(&file);
}

/// 项目级 skills 开关（默认关，§2.4 注入风险警告）。
pub fn project_skills_enabled() -> bool {
    load_disabled_skills_file().project_skills_enabled
}

/// 写项目级 skills 开关。落盘后由调用方重写在线会话组合目录。
pub fn set_project_skills_enabled(enabled: bool) {
    let _guard = DISABLED_SKILLS_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_skills_file();
    if file.project_skills_enabled == enabled {
        return;
    }
    file.project_skills_enabled = enabled;
    save_disabled_skills_file(&file);
}

/// 读全局（plain）被禁用的技能 id 列表。兼容既有调用方。
pub fn load_disabled_skills() -> Vec<String> {
    load_disabled_skills_for(ConnectorScope::Plain)
}

/// 技能安装后同步所有 DenyAll 且已初始化的 scope：用户已改过这类会话技能开关
/// 时，新装的技能默认仍保持关闭（加入该 scope 禁用集）；未初始化时无需落盘
/// （load 会按「默认全禁已装技能」兜底）。AllowAll 模式无需同步（默认全开）。
/// 与连接器 `sync_deny_all_scopes_after_install` 同语义。
pub fn sync_deny_all_scopes_after_skill_install(skill_id: &str) {
    let _guard = DISABLED_SKILLS_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_skills_file();
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
        if !ids.iter().any(|id| id == skill_id) {
            ids.push(skill_id.to_string());
            changed = true;
        }
    }
    if changed {
        save_disabled_skills_file(&file);
    }
}

/// 技能卸载后同步所有 scope：已卸载的技能从各 scope 禁用集移除，避免残留 id
/// 指向不存在的技能。与连接器 `remove_connector_from_disabled_scopes` 同语义。
pub fn remove_skill_from_disabled_scopes(skill_id: &str) {
    let _guard = DISABLED_SKILLS_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_skills_file();
    let mut changed = false;
    for ids in file.scopes.values_mut() {
        let before = ids.len();
        ids.retain(|id| id != skill_id);
        changed |= ids.len() != before;
    }
    if changed {
        save_disabled_skills_file(&file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把 PINVOU3_HOME 指到干净临时目录跑闭包，跑完恢复并清理。
    /// 借 `platform::paths::tests::ENV_LOCK` 与其它 mutate PINVOU3_HOME 的测试串行。
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("pinvou3-skillscope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);
        f();
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_skills_roundtrip_per_scope() {
        with_temp_home(|| {
            assert!(load_disabled_skills_for(ConnectorScope::Plain).is_empty());
            save_disabled_skills_for(ConnectorScope::Plain, &["visualizer".to_string()]);
            save_disabled_skills_for(ConnectorScope::Code, &["ima-skills".to_string()]);
            assert_eq!(
                load_disabled_skills_for(ConnectorScope::Plain),
                vec!["visualizer".to_string()]
            );
            assert_eq!(
                load_disabled_skills_for(ConnectorScope::Code),
                vec!["ima-skills".to_string()]
            );
            // code 写入后已初始化：再次读仍是落盘列表
            assert_eq!(
                load_disabled_skills_for(ConnectorScope::Code),
                vec!["ima-skills".to_string()]
            );
        });
    }

    #[test]
    fn migrates_legacy_bare_array_to_plain() {
        with_temp_home(|| {
            std::fs::create_dir_all(paths::pinvou3_home()).unwrap();
            std::fs::write(
                disabled_skills_path(),
                r#"["visualizer","government-writing"]"#,
            )
            .unwrap();
            assert_eq!(
                load_disabled_skills_for(ConnectorScope::Plain),
                vec!["visualizer".to_string(), "government-writing".to_string()]
            );
            // 落盘已迁移为对象格式
            let content = std::fs::read_to_string(disabled_skills_path()).unwrap();
            assert!(
                content.contains("\"plain\""),
                "迁移后应为对象格式: {content}"
            );
        });
    }

    #[test]
    fn migrates_legacy_bare_array_strips_skill_prefix() {
        with_temp_home(|| {
            std::fs::create_dir_all(paths::pinvou3_home()).unwrap();
            // 旧前端 bug 窗口期误写入的带 `skill:` 前缀条目：迁移时必须归一为裸 id，
            // 否则按裸 id 匹配的 model_skill_names/组合目录物化会漏禁该技能。
            std::fs::write(
                disabled_skills_path(),
                r#"["skill:visualizer","government-writing"]"#,
            )
            .unwrap();
            assert_eq!(
                load_disabled_skills_for(ConnectorScope::Plain),
                vec!["visualizer".to_string(), "government-writing".to_string()],
                "裸数组迁移应剥除 skill: 前缀"
            );
            // 落盘后的对象格式同样无前缀
            let content = std::fs::read_to_string(disabled_skills_path()).unwrap();
            assert!(
                !content.contains("skill:visualizer"),
                "迁移落盘不应残留前缀: {content}"
            );
        });
    }

    /// 旧双 scope 对象 `{plain, code, code_initialized, project_skills_enabled}`
    /// 迁移为 scopes map：迁移前后行为一致，project_skills_enabled 全局字段保留。
    #[test]
    fn migrates_legacy_object_to_scopes_map() {
        with_temp_home(|| {
            std::fs::create_dir_all(paths::pinvou3_home()).unwrap();
            std::fs::write(
                disabled_skills_path(),
                r#"{"plain":["visualizer"],"code":["ima-skills"],"code_initialized":true,"project_skills_enabled":true}"#,
            )
            .unwrap();
            assert_eq!(
                load_disabled_skills_for(ConnectorScope::Plain),
                vec!["visualizer".to_string()]
            );
            assert_eq!(
                load_disabled_skills_for(ConnectorScope::Code),
                vec!["ima-skills".to_string()]
            );
            assert!(project_skills_enabled(), "项目技能开关应随迁移保留");
            // 读到即迁移：落盘已是新格式，旧键不残留
            let content = std::fs::read_to_string(disabled_skills_path()).unwrap();
            assert!(
                content.contains("\"scopes\""),
                "迁移后应为新格式: {content}"
            );
            assert!(
                !content.contains("code_initialized"),
                "旧键不应残留: {content}"
            );
        });
    }

    /// 旧对象 `code_initialized=false`：code 数组被忽略、按 DenyAll 默认全禁
    /// （与迁移前一致）；plain 列表无 initialized 标记也必须生效。
    #[test]
    fn legacy_object_uninitialized_code_keeps_deny_all_default() {
        with_temp_home(|| {
            for name in ["visualizer", "government-writing"] {
                let dir = paths::bundle_skills_dir().join(name);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
            }
            std::fs::write(
                disabled_skills_path(),
                r#"{"plain":["visualizer"],"code":[],"code_initialized":false}"#,
            )
            .unwrap();
            assert_eq!(
                load_disabled_skills_for(ConnectorScope::Plain),
                vec!["visualizer".to_string()]
            );
            let mut disabled = load_disabled_skills_for(ConnectorScope::Code);
            disabled.sort();
            assert_eq!(
                disabled,
                vec!["government-writing".to_string(), "visualizer".to_string()],
                "code 未初始化应按 DenyAll 默认全禁已装技能"
            );
        });
    }

    #[test]
    fn migrates_legacy_skill_prefix_entries_from_connectors_file() {
        with_temp_home(|| {
            std::fs::create_dir_all(paths::pinvou3_home()).unwrap();
            // 旧版借道 disabled_connectors.json 存 skill: 前缀条目
            std::fs::write(
                paths::pinvou3_home().join("disabled_connectors.json"),
                r#"{"plain":["gongwen","skill:visualizer"],"code":[],"code_initialized":false}"#,
            )
            .unwrap();
            assert_eq!(
                load_disabled_skills_for(ConnectorScope::Plain),
                vec!["visualizer".to_string()]
            );
            // 连接器文件里 skill: 残留已清除，gongwen 保留
            let conn: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(paths::pinvou3_home().join("disabled_connectors.json"))
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(conn["plain"].as_array().unwrap().len(), 1);
            assert_eq!(conn["plain"][0], "gongwen");
        });
    }

    #[test]
    fn code_scope_uninitialized_defaults_to_all_disabled() {
        with_temp_home(|| {
            // 每个 skill 需先建子目录再写 SKILL.md（裸 fs::write 不递归建目录，
            // 否则父目录缺失 → NotFound panic）。
            for name in ["visualizer", "government-writing"] {
                let dir = paths::bundle_skills_dir().join(name);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
            }
            // 未初始化：code 默认全禁已装技能
            let mut disabled = load_disabled_skills_for(ConnectorScope::Code);
            disabled.sort();
            assert_eq!(
                disabled,
                vec!["government-writing".to_string(), "visualizer".to_string()]
            );
            // 改过开关后以落盘为准
            save_disabled_skills_for(ConnectorScope::Code, &["visualizer".to_string()]);
            assert_eq!(
                load_disabled_skills_for(ConnectorScope::Code),
                vec!["visualizer".to_string()]
            );
        });
    }

    #[test]
    fn install_syncs_deny_all_scopes_only_when_initialized() {
        with_temp_home(|| {
            // 未初始化：安装不落盘（读取时按默认全禁兜底）
            sync_deny_all_scopes_after_skill_install("new-skill");
            assert!(!load_disabled_skills_file().initialized.contains("code"));

            // 初始化后：新装技能默认加入 code 禁用集
            save_disabled_skills_for(ConnectorScope::Code, &["a".to_string()]);
            sync_deny_all_scopes_after_skill_install("new-skill");
            assert_eq!(
                load_disabled_skills_for(ConnectorScope::Code),
                vec!["a".to_string(), "new-skill".to_string()]
            );
        });
    }

    #[test]
    fn uninstall_removes_skill_from_both_scopes() {
        with_temp_home(|| {
            save_disabled_skills_for(ConnectorScope::Plain, &["x".to_string(), "y".to_string()]);
            save_disabled_skills_for(ConnectorScope::Code, &["y".to_string()]);
            remove_skill_from_disabled_scopes("y");
            assert_eq!(
                load_disabled_skills_for(ConnectorScope::Plain),
                vec!["x".to_string()]
            );
            assert!(load_disabled_skills_for(ConnectorScope::Code).is_empty());
        });
    }

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
}
