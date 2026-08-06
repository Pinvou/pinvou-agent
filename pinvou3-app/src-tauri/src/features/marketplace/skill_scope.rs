//! 技能开关双 scope 持久化（`~/.pinvou3/disabled_skills.json`）。
//!
//! 与连接器开关（`disabled_connectors.json`，本模块上级）同构：`{plain, code,
//! code_initialized, project_skills_enabled}`，旧数据迁移（裸数组 → plain；
//! 旧版借道 `disabled_connectors.json` 的 `skill:<id>` 条目 → 提取进 plain 并
//! 清除连接器文件残留）。组合目录物化消费本层（
//! `features/assistant/skill_materialization.rs`）。
//!
//! 放在 marketplace 而不是 assistant：skill 开关是「技能市场」的持久化数据，
//! 与连接器开关同领域；connectors（ima 连接/退出）也会读写它，若放 assistant
//! 会形成 connectors → assistant 的依赖环（架构守卫 rust_feature_cycles）。

use std::path::PathBuf;
use std::sync::Mutex;

use crate::features::marketplace::skill_marketplace::SkillMarketplaceManager;
use crate::features::marketplace::ConnectorScope;
use crate::platform::paths;

/// 两个 scope 的技能禁用列表。`plain` = 普通会话，`code` = 原生代码会话。
///
/// `code` scope 遵循「默认全关」安全默认：文件里还没有 code 记录时（首次读取），
/// 代码会话默认禁用**所有已安装技能**（外部能力显式开启）；一旦用户改过 code
/// 开关（`code_initialized=true`），就以落盘列表为准。与连接器
/// `DisabledConnectorsFile` 同构（§8.3 范本）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DisabledSkillsFile {
    #[serde(default)]
    pub plain: Vec<String>,
    #[serde(default)]
    pub code: Vec<String>,
    /// code scope 是否已被用户显式初始化过（改过开关）。false = 未初始化，
    /// 按「默认全禁已装技能」处理。
    #[serde(default)]
    pub code_initialized: bool,
    /// 项目级 skills 是否对 code 会话开启（默认关：项目内文本是 prompt-injection
    /// 面，开启需用户显式确认并看到注入风险警告）。开启后，绑项目的 code 会话
    /// 组合目录额外包含项目 `.agents/skills` 等工具约定目录（§2.4 兜底路径：
    /// fork #41 已砍断 workspace 并集扫描，项目技能经同一物化通道拷入组合目录）。
    #[serde(default)]
    pub project_skills_enabled: bool,
}

fn disabled_skills_path() -> PathBuf {
    paths::pinvou3_home().join("disabled_skills.json")
}

/// `disabled_skills.json` 读-改-写的进程内串行化：开关命令、安装/卸载同步可能
/// 并发触发同一份文件的读-改-写，串行化避免交错丢更新（与连接器文件同一范式）。
static DISABLED_SKILLS_FILE_LOCK: Mutex<()> = Mutex::new(());

/// 读完整文件。兼容两种旧数据，首次读到时迁移并落盘：
///  1. 裸数组 `["a","b"]`（方案 §2.1 的旧 disabled_skills.json 形态）→ plain scope；
///  2. 旧版借道 `disabled_connectors.json` 的 `skill:<id>` 条目（本分支历史实现）
///     → 提取进 plain scope，并从连接器文件清除 `skill:` 残留。
/// 迁移失败按「全部启用（plain）+ code 默认全禁」安全兜底（全部落空 → 默认值）。
fn load_disabled_skills_file() -> DisabledSkillsFile {
    let path = disabled_skills_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            // 首次启动：从旧连接器文件的 skill: 条目迁移（一次）。
            let file = migrate_legacy_skill_ids();
            if !file.plain.is_empty() {
                save_disabled_skills_file(&file);
            }
            return file;
        }
    };
    if let Ok(legacy) = serde_json::from_str::<Vec<String>>(&content) {
        let file = DisabledSkillsFile {
            plain: legacy,
            code: Vec::new(),
            code_initialized: false,
            project_skills_enabled: false,
        };
        save_disabled_skills_file(&file);
        return file;
    }
    serde_json::from_str(&content).unwrap_or_default()
}

/// 旧版技能开关借道 `disabled_connectors.json` 的 `skill:<id>` 条目（本分支
/// `refresh_disabled_skills` 的历史实现）。迁移：strip 前缀进 plain scope，并清除
/// 连接器文件里的 `skill:` 残留（避免两处真相）。旧语义是进程级全局禁用集，
/// 全局 → plain scope 透明迁移。
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
        return DisabledSkillsFile {
            plain,
            code: Vec::new(),
            code_initialized: false,
            project_skills_enabled: false,
        };
    }
    // 新版对象格式：同样剥离两个 scope 数组里的 skill: 前缀条目
    if let Ok(mut file) = serde_json::from_str::<serde_json::Value>(&content) {
        if let Some(obj) = file.as_object_mut() {
            for key in ["plain", "code"] {
                if let Some(arr) = obj.get_mut(key).and_then(|v| v.as_array_mut()) {
                    let mut kept: Vec<serde_json::Value> = Vec::new();
                    for item in arr.drain(..) {
                        if let Some(s) = item.as_str() {
                            if let Some(skill_id) = s.strip_prefix("skill:") {
                                plain.push(skill_id.to_string());
                                legacy_removed = true;
                                continue;
                            }
                        }
                        kept.push(item);
                    }
                    *obj.get_mut(key).unwrap() = serde_json::Value::Array(kept);
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
    DisabledSkillsFile {
        plain,
        code: Vec::new(),
        code_initialized: false,
        project_skills_enabled: false,
    }
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
/// `code` scope 未初始化时（用户从未改过代码会话技能开关）返回全部已安装技能 id，
/// 即「代码会话默认全禁，外部能力显式开启」的安全默认（与连接器 §8.3 同语义）。
pub fn load_disabled_skills_for(scope: ConnectorScope) -> Vec<String> {
    let file = load_disabled_skills_file();
    match scope {
        ConnectorScope::Plain => file.plain,
        ConnectorScope::Code => {
            if file.code_initialized {
                file.code
            } else {
                SkillMarketplaceManager::new().installed_skill_ids()
            }
        }
    }
}

/// 写某 scope 被禁用的技能 id 列表。code scope 写入时置 `code_initialized=true`。
pub fn save_disabled_skills_for(scope: ConnectorScope, ids: &[String]) {
    let _guard = DISABLED_SKILLS_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_skills_file();
    match scope {
        ConnectorScope::Plain => file.plain = ids.to_vec(),
        ConnectorScope::Code => {
            file.code = ids.to_vec();
            file.code_initialized = true;
        }
    }
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

/// 技能安装后同步 code scope：若用户已初始化过代码会话技能开关（改过），新装的
/// 技能默认仍保持关闭（加入 code 禁用集）；未初始化时无需落盘（load 会按
/// 「默认全禁已装技能」兜底）。与连接器 `sync_code_scope_after_install` 同语义。
pub fn sync_code_scope_after_skill_install(skill_id: &str) {
    let _guard = DISABLED_SKILLS_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_skills_file();
    if !file.code_initialized {
        return;
    }
    if !file.code.iter().any(|id| id == skill_id) {
        file.code.push(skill_id.to_string());
        save_disabled_skills_file(&file);
    }
}

/// 技能卸载后同步两个 scope：已卸载的技能从 plain/code 禁用集移除，避免残留 id
/// 指向不存在的技能。与连接器 `remove_connector_from_disabled_scopes` 同语义。
pub fn remove_skill_from_disabled_scopes(skill_id: &str) {
    let _guard = DISABLED_SKILLS_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = load_disabled_skills_file();
    let before = (file.plain.len(), file.code.len());
    file.plain.retain(|id| id != skill_id);
    file.code.retain(|id| id != skill_id);
    if file.plain.len() != before.0 || file.code.len() != before.1 {
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
            std::fs::create_dir_all(paths::bundle_skills_dir()).unwrap();
            std::fs::write(
                paths::bundle_skills_dir()
                    .join("visualizer")
                    .join("SKILL.md"),
                "---\nname: visualizer\n---\n",
            )
            .unwrap();
            std::fs::write(
                paths::bundle_skills_dir()
                    .join("government-writing")
                    .join("SKILL.md"),
                "---\nname: government-writing\n---\n",
            )
            .unwrap();
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
    fn install_syncs_code_scope_only_when_initialized() {
        with_temp_home(|| {
            // 未初始化：安装不落盘（读取时按默认全禁兜底）
            sync_code_scope_after_skill_install("new-skill");
            assert!(!load_disabled_skills_file().code_initialized);

            // 初始化后：新装技能默认加入 code 禁用集
            save_disabled_skills_for(ConnectorScope::Code, &["a".to_string()]);
            sync_code_scope_after_skill_install("new-skill");
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
