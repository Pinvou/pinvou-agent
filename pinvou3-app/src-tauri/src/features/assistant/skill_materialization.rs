//! 技能按 scope 治理：开关双 scope 持久化 + 按会话拼组合 skills_dir。
//!
//! 把「按会话的技能集」翻译成引擎现成的发现协议（`EngineConfig.skills_dir`）：
//! 底座每轮 prompt 对 `skills_dir` 重新发现渲染，因此组合目录的内容就是该会话
//! 下一轮 prompt 的 `## Skills` 块（空目录 → 整个块不渲染，见底座
//! `render_skills_block` 空 registry 返回 None）。
//!
//! 设计对照 `.luzeyang/code-plain-decoupling/skill-scope-governance-实施方案.md`
//! （已归档，按 §2.1/§2.2/§2.3/§2.4 实现）：
//!   - 开关落 `~/.pinvou3/disabled_skills.json`（`{scopes: {<mode>: [...]},
//!     "initialized": [...]}`，scope 键即模式名，与 `disabled_connectors.json`
//!     同构；旧裸数组/旧借道 `skill:` 条目/旧双 scope 对象迁移为 plain/新 map）；
//!   - code scope 未初始化时默认全禁已装技能（外部能力显式开启，封泄露面）；
//!   - 组合目录 = 来源（用户 skills 目录 + bundle/skills）中**启用**的技能，first-wins
//!     同名去重，另排除该 scope 被禁用连接器的 companion skills（保持现状联动语义）；
//!   - 物化时机三段式：spawn 全量拼、toggle/安装/卸载事件驱动增量重写、发送路径
//!     `exists()` 自愈；不做每轮 diff。
//!
//! 依赖方向：本模块是 `features/assistant` 的组件，只依赖 `platform` 与
//! `marketplace`（scope 枚举 / companion 查询），不得反向依赖引擎运行时。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::features::marketplace::skill_marketplace::SkillMarketplaceManager;
use crate::features::marketplace::{ConnectorScope, MarketplaceManager};
use crate::platform::paths;

// ---------------------------------------------------------------------------
// 开关双 scope 持久化（disabled_skills.json）
// ---------------------------------------------------------------------------
//
// 持久化层在 `features/marketplace/skill_scope.rs`（与连接器开关同领域、避免
// connectors → assistant 依赖环），这里 re-export 保持调用路径不变。
pub use crate::features::marketplace::skill_scope::*;

// ---------------------------------------------------------------------------
// 组合目录物化
// ---------------------------------------------------------------------------

/// 技能来源目录（除项目级），**first-wins 顺序（高优先级在前）**。与底座并集
/// 语义一致：同名技能按高优先级来源入组合目录、低优先级跳过（底座发现时同名
/// 不重复加入）。用户手放技能（`~/.pinvou3/user/skills/`）覆盖市场安装
/// （`bundle/skills/`），用户目录中的同名 Skill 优先。
fn skill_source_dirs() -> Vec<PathBuf> {
    vec![paths::user_skills_dir(), paths::bundle_skills_dir()]
}

/// 项目技能来源目录（workspace 内工具约定，按底座上游 #432 优先级降序，
/// `.pinvou/skills` 为 pinvou3 自有约定，插在 `.agents/skills` 之后）。
/// 仅当项目级 skills 开关开启且该 scope 的 `project_skills_opt_in` 表字段为
/// true（当前仅 code）时使用（§2.4：项目内文本是 prompt-injection 面，显式
/// 开启才扫描；fork #41 已砍断 workspace 并集发现，这里在 app 侧按同一来源
/// 顺序补上，经组合目录通道物化）。
fn project_skill_source_dirs(project_workspace: &Path) -> Vec<PathBuf> {
    [
        ".agents/skills",
        ".pinvou/skills",
        "skills",
        ".opencode/skills",
        ".claude/skills",
        ".cursor/skills",
        ".codewhale/skills",
    ]
    .iter()
    .map(|rel| project_workspace.join(rel))
    .collect()
}

/// 该 scope 的启用技能集合（目录名 → 来源路径），按来源优先级排序。
///
/// 排除两类：本 scope 禁用集中的技能（含未初始化 DenyAll 模式的默认全禁）+
/// 被禁用连接器声明的 companion skills（保持「关 MCP → 关联技能一并隐藏」的
/// 既有联动）。`project_workspace` 仅在该 scope 的 `project_skills_opt_in` 表
/// 字段为 true 且项目级 skills 开关开启时被扫描（排在用户/市场来源之前，
/// 项目本地覆盖语义与底座 workspace 目录优先一致）。
pub fn enabled_skills_for(
    scope: ConnectorScope,
    project_workspace: Option<&Path>,
) -> Vec<(String, PathBuf)> {
    let disabled = disabled_skill_names_for(scope);
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    // 项目技能优先（workspace 目录 > 全局来源，与底座 first-wins 一致）；
    // 项目门由模式能力表字段驱动（当前仅 code 为 true）。
    if crate::features::assistant::session_policy::project_skills_opt_in_for(scope)
        && project_skills_enabled()
    {
        if let Some(workspace) = project_workspace {
            for src in project_skill_source_dirs(workspace) {
                collect_source_skills(&src, &disabled, &mut seen, &mut out);
            }
        }
    }
    for src in skill_source_dirs() {
        collect_source_skills(&src, &disabled, &mut seen, &mut out);
    }
    out
}

/// 枚举单个来源目录的技能（first-wins 同名去重 + 禁用过滤）。
fn collect_source_skills(
    src: &Path,
    disabled: &HashSet<String>,
    seen: &mut HashSet<String>,
    out: &mut Vec<(String, PathBuf)>,
) {
    let Ok(rd) = std::fs::read_dir(src) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !seen.insert(name.clone()) {
            continue; // first-wins：同名技能不重复入目录
        }
        if disabled.contains(&name) {
            continue;
        }
        out.push((name, path));
    }
}

/// 禁用集（市场 id → 落盘目录名）+ 被禁用连接器的 companion skills → 目录名集合。
fn disabled_skill_names_for(scope: ConnectorScope) -> HashSet<String> {
    let ids = load_disabled_skills_for(scope);
    let mut names: HashSet<String> = SkillMarketplaceManager::new()
        .model_skill_names(&ids)
        .into_iter()
        .collect();
    // companion 联动：scope 中被禁用连接器声明的 companion skills 一并排除
    // （现状 refresh_disabled_skills 只查 plain scope；这里按 scope 各自联动）。
    let market = MarketplaceManager::new();
    for cid in crate::features::marketplace::load_disabled_connectors_for(scope) {
        for sid in market.companion_skills(&cid) {
            names.insert(sid);
        }
    }
    names
}

/// 递归拷贝技能目录（SKILL.md + 伴随文件）。跳过市场安装标记 `.installed-from`
/// （组合目录是内容镜像，无装卸语义）。返回复制失败的第一个错误。
fn copy_skill_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        // .installed-from 是市场落盘标记，不属技能运行内容
        if name == ".installed-from" {
            continue;
        }
        let target = dest.join(&name);
        if entry.file_type()?.is_dir() {
            copy_skill_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// 临时目录名（组合目录的同级兄弟，避免被 discover 当技能扫到）。
fn staging_dir(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_owned();
    s.push(".staging");
    PathBuf::from(s)
}

/// **spawn 全量拼**：按当前 scope 集合全量物化组合目录（staged + rename 原子替换，
/// 运行中引擎的并发发现不会读到半写目录）。兜底：app 重启、引擎回收后重建、
/// toggle 时引擎不在线。
pub fn materialize_session_skills(
    session_id: &str,
    scope: ConnectorScope,
    project_workspace: Option<&Path>,
) -> Result<(), String> {
    let target = paths::session_skills_dir(session_id);
    let enabled = enabled_skills_for(scope, project_workspace);
    let parent = target
        .parent()
        .ok_or_else(|| format!("组合目录无父级: {}", target.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("创建会话目录: {e}"))?;
    let staged = staging_dir(&target);
    let _ = std::fs::remove_dir_all(&staged);
    for (name, src) in &enabled {
        let dest = staged.join(name);
        copy_skill_dir(src, &dest).map_err(|e| {
            format!(
                "复制技能 '{name}' ({} → {}): {e}",
                src.display(),
                dest.display()
            )
        })?;
    }
    // 空集也保留空目录：`insert_configured_skills_dir` 只把**存在**的 skills_dir
    // 插入发现集，空目录 → registry 空 → `## Skills` 块不渲染（V-1 封口机制）。
    let _ = std::fs::create_dir_all(&staged);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&staged, &target)
        .map_err(|e| format!("落盘组合目录 {}: {e}", target.display()))?;
    Ok(())
}

/// **事件驱动增量重写**：只增删变化部分（diff），未变的技能目录保持不动——
/// 底座每轮重扫，下一轮 prompt 即生效；前缀缓存只在目录块真变时 miss 一轮。
/// 幂等：同一期望集合重复调用不产生任何变化。
pub fn rewrite_session_skills(
    session_id: &str,
    scope: ConnectorScope,
    project_workspace: Option<&Path>,
) {
    let target = paths::session_skills_dir(session_id);
    let enabled: HashMap<String, PathBuf> = enabled_skills_for(scope, project_workspace)
        .into_iter()
        .collect();
    let mut existing: HashSet<String> = HashSet::new();
    if let Ok(rd) = std::fs::read_dir(&target) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = entry.file_name().to_str().map(str::to_string) {
                    existing.insert(name);
                }
            }
        }
    }
    // 删除多余：组合目录里有、期望集合里没有
    for name in existing.difference(&enabled.keys().cloned().collect()) {
        let _ = std::fs::remove_dir_all(target.join(name));
    }
    // 拷贝新增：期望集合里有、组合目录里没有
    for (name, src) in &enabled {
        if !existing.contains(name) {
            let dest = target.join(name);
            if let Err(e) = copy_skill_dir(src, &dest) {
                eprintln!(
                    "[skill-scope] rewrite {session_id} add '{name}' ({}) failed: {e}",
                    src.display()
                );
            }
        }
    }
    // 空集保证目录存在（同 materialize 的空目录语义）
    if enabled.is_empty() {
        let _ = std::fs::create_dir_all(&target);
    }
}

/// **发送路径自愈**：组合目录缺失时按当前 scope 全量重建（微秒级 stat，防手动
/// 删除后静默丢失）。不做每轮全量比对。
pub fn ensure_session_skills(
    session_id: &str,
    scope: ConnectorScope,
    project_workspace: Option<&Path>,
) {
    if !paths::session_skills_dir(session_id).is_dir() {
        if let Err(e) = materialize_session_skills(session_id, scope, project_workspace) {
            eprintln!("[skill-scope] self-heal {session_id} failed: {e}");
        }
    }
}

/// 判定（策略用）：该会话组合目录是否为空（目录不存在或无数）。code 会话据此
/// 决定 `load_skill` 是否隐藏——空 → 隐藏，避免「开关开着但没技能」的假状态。
pub fn session_skills_is_empty(session_id: &str) -> bool {
    let dir = paths::session_skills_dir(session_id);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return true;
    };
    !rd.flatten().any(|e| e.path().is_dir())
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

    fn write_skill(root: &Path, name: &str, content: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\n---\n{content}"),
        )
        .unwrap();
        std::fs::write(dir.join("companion.txt"), "x").unwrap();
    }

    /// 写连接器 manifest（companion_skills 联动测试用）。
    fn write_tool_manifest(tool_id: &str, manifest: &str) {
        let dir = crate::platform::paths::bundle_mcp_servers_dir().join(tool_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    }

    // ---- 组合目录 ----

    /// 连接器禁用联动技能：禁用声明了 companion_skills 的连接器 → 该技能从组合
    /// 目录排除；开回来 → 恢复。守"公文 MCP 关掉 → government-writing 从
    /// `## Skills` 隐藏"这条链路（过滤职责现由组合目录计算承担）。
    #[test]
    fn companion_skill_excluded_when_connector_disabled() {
        with_temp_home(|| {
            write_tool_manifest(
                "gongwen",
                r#"{"id":"gongwen","name":"公文写作","description":"d","version":"1.0.0","icon":"file-text","category":"办公","mcp_tools":["mcp_gongwen_make_gongwen"],"command":"python","args":["server.py"],"companion_skills":["government-writing"]}"#,
            );
            let skill_dir = paths::bundle_skills_dir().join("government-writing");
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                "---\nname: government-writing\n---\n# GW\n",
            )
            .unwrap();

            // 禁用公文 MCP → 组合目录计算排除关联技能
            crate::features::marketplace::save_disabled_connectors(&["gongwen".to_string()]);
            let enabled = enabled_skills_for(ConnectorScope::Plain, None);
            assert!(
                !enabled.iter().any(|(n, _)| n == "government-writing"),
                "禁用公文 MCP 后关联技能应从组合目录排除"
            );

            // 开回来 → 恢复
            crate::features::marketplace::save_disabled_connectors(&[]);
            let enabled = enabled_skills_for(ConnectorScope::Plain, None);
            assert!(
                enabled.iter().any(|(n, _)| n == "government-writing"),
                "启用公文 MCP 后关联技能应恢复"
            );
        });
    }

    /// 独立安装的 marketplace skill 没有 companion MCP，但 composer 工具菜单也
    /// 允许直接开关它；技能开关走独立 disabled_skills.json 双 scope 持久化
    /// （不再借道连接器文件的 skill: 前缀）。
    #[test]
    fn disabling_direct_skill_id_hides_skill() {
        with_temp_home(|| {
            crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
                .install("visualizer")
                .unwrap();

            save_disabled_skills_for(ConnectorScope::Plain, &["visualizer".to_string()]);
            let enabled = enabled_skills_for(ConnectorScope::Plain, None);
            assert!(
                !enabled.iter().any(|(n, _)| n == "visualizer"),
                "禁用 skill id 后应从组合目录排除"
            );

            save_disabled_skills_for(ConnectorScope::Plain, &[]);
            let enabled = enabled_skills_for(ConnectorScope::Plain, None);
            assert!(
                enabled.iter().any(|(n, _)| n == "visualizer"),
                "启用 skill id 后应恢复"
            );
        });
    }

    /// connector id 和用户上传 skill id 同名时，关闭 connector 不应误停用该 skill
    /// （companion 排除按连接器 manifest 声明，与技能名无关）。
    #[test]
    fn disabling_connector_id_does_not_hide_same_named_user_skill() {
        with_temp_home(|| {
            write_tool_manifest(
                "weather",
                r#"{"id":"weather","name":"天气","description":"d","version":"1.0.0","icon":"cloud","category":"查询","mcp_tools":["mcp_weather_query"],"command":"python","args":["server.py"]}"#,
            );
            let skill_dir = paths::bundle_skills_dir().join("weather");
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                "---\nname: weather\ndescription: user weather skill\n---\n# Weather\n",
            )
            .unwrap();
            std::fs::write(skill_dir.join(".installed-from"), "upload:weather.zip").unwrap();

            crate::features::marketplace::save_disabled_connectors(&["weather".to_string()]);
            let enabled = enabled_skills_for(ConnectorScope::Plain, None);
            assert!(
                enabled.iter().any(|(n, _)| n == "weather"),
                "禁用同名 connector 不应误排除用户上传 skill"
            );

            // 技能自己的开关（独立文件）才会排除它
            save_disabled_skills_for(ConnectorScope::Plain, &["weather".to_string()]);
            let enabled = enabled_skills_for(ConnectorScope::Plain, None);
            assert!(
                !enabled.iter().any(|(n, _)| n == "weather"),
                "禁用 skill id 才应排除用户上传 skill"
            );
        });
    }

    fn seed_sources() {
        let bundle = paths::bundle_skills_dir();
        let user = paths::user_skills_dir();
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        write_skill(&bundle, "visualizer", "market version");
        write_skill(&bundle, "government-writing", "market version");
        write_skill(&bundle, "ima-skills", "market version");
        // 用户手放技能 + 同名覆盖
        write_skill(&user, "user-custom", "user version");
        write_skill(&user, "visualizer", "user version");
        // 手放技能不应带市场标记（默认启用、无开关）
        std::fs::write(
            bundle.join("government-writing").join(".installed-from"),
            "pinvou3-marketplace:government-writing",
        )
        .unwrap();
    }

    #[test]
    fn enabled_skills_respect_first_wins_and_scope_disabled() {
        with_temp_home(|| {
            seed_sources();
            // 默认（无禁用）：user 覆盖 bundle 同名 + 手放技能入集
            let enabled = enabled_skills_for(ConnectorScope::Plain, None);
            let names: HashSet<&str> = enabled.iter().map(|(n, _)| n.as_str()).collect();
            assert!(names.contains("visualizer"));
            assert!(names.contains("government-writing"));
            assert!(names.contains("user-custom"));
            assert_eq!(
                enabled.len(),
                4,
                "3 bundle + 1 user 手放(user 覆盖同名后仍 4 个)"
            );
            // user 版本优先：visualizer 来自 user/skills
            let (_, src) = enabled.iter().find(|(n, _)| n == "visualizer").unwrap();
            assert_eq!(src, &paths::user_skills_dir().join("visualizer"));

            // plain 关 visualizer → 组合集不含（market 版本也被 user 覆盖，整名排除）
            save_disabled_skills_for(ConnectorScope::Plain, &["visualizer".to_string()]);
            let enabled = enabled_skills_for(ConnectorScope::Plain, None);
            assert!(!enabled.iter().any(|(n, _)| n == "visualizer"));
            assert!(enabled.iter().any(|(n, _)| n == "government-writing"));
        });
    }

    #[test]
    fn materialize_then_rewrite_is_idempotent() {
        with_temp_home(|| {
            seed_sources();
            let sid = "session-test-1";
            materialize_session_skills(sid, ConnectorScope::Plain, None).unwrap();
            let dir = paths::session_skills_dir(sid);
            assert!(dir.join("visualizer").join("SKILL.md").is_file());
            assert!(dir.join("visualizer").join("companion.txt").is_file());
            assert!(
                !dir.join("government-writing")
                    .join(".installed-from")
                    .exists(),
                "市场标记不拷入组合目录"
            );

            // 幂等：同期望集合再次重写无变化
            rewrite_session_skills(sid, ConnectorScope::Plain, None);
            rewrite_session_skills(sid, ConnectorScope::Plain, None);
            let names: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(names.len(), 4, "增量重写幂等：目录数不变: {names:?}");

            // 增量：关一个 → 目录删除；再开 → 目录回来
            save_disabled_skills_for(ConnectorScope::Plain, &["visualizer".to_string()]);
            rewrite_session_skills(sid, ConnectorScope::Plain, None);
            assert!(!dir.join("visualizer").exists());
            save_disabled_skills_for(ConnectorScope::Plain, &[]);
            rewrite_session_skills(sid, ConnectorScope::Plain, None);
            assert!(dir.join("visualizer").exists());
        });
    }

    #[test]
    fn empty_scope_leaves_empty_dir_and_is_empty_detection() {
        with_temp_home(|| {
            // 干净的 home（无任何技能来源）→ 全禁场景等价：组合目录为空目录
            let sid = "session-test-2";
            materialize_session_skills(sid, ConnectorScope::Plain, None).unwrap();
            let dir = paths::session_skills_dir(sid);
            assert!(
                dir.is_dir(),
                "空集也应物化出空目录（insert_configured_skills_dir 前提）"
            );
            assert!(session_skills_is_empty(sid));
            assert!(
                std::fs::read_dir(&dir).unwrap().next().is_none(),
                "空集目录内无任何技能"
            );

            // 自愈：手动删目录 → ensure 重建
            std::fs::remove_dir_all(&dir).unwrap();
            ensure_session_skills(sid, ConnectorScope::Plain, None);
            assert!(dir.is_dir(), "自愈应重建组合目录");
            assert!(session_skills_is_empty(sid));
        });
    }

    #[test]
    fn project_skills_follow_gate_and_priority() {
        with_temp_home(|| {
            // bundle 基线：市场装 visualizer
            let bundle = paths::bundle_skills_dir();
            std::fs::create_dir_all(&bundle).unwrap();
            write_skill(&bundle, "visualizer", "market version");

            let project =
                std::env::temp_dir().join(format!("pinvou3-project-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&project);
            write_skill(
                &project.join(".claude").join("skills"),
                "project-skill",
                "p",
            );
            // 项目同名技能应覆盖市场版（workspace 目录优先语义）
            write_skill(
                &project.join(".agents").join("skills"),
                "visualizer",
                "project version",
            );
            // .pinvou/skills 优先级仅次于 .agents/skills：
            // 同名时覆盖 .claude（低优先级约定目录），被 .agents 覆盖。
            write_skill(
                &project.join(".pinvou").join("skills"),
                "pinvou-skill",
                "pinvou version",
            );
            write_skill(
                &project.join(".claude").join("skills"),
                "pinvou-skill",
                "claude version",
            );
            write_skill(
                &project.join(".pinvou").join("skills"),
                "visualizer",
                "pinvou version",
            );

            // 本测试聚焦项目技能的门控与优先级覆盖。code scope「未初始化默认全禁」
            // 语义会把已装技能也排除掉，与测试意图无关——先显式初始化 code scope
            // （空禁用集 = 全部启用），让项目技能覆盖链路可被断言。
            save_disabled_skills_for(ConnectorScope::Code, &[]);

            // 默认关：code 组合集不含项目技能
            let enabled = enabled_skills_for(ConnectorScope::Code, Some(&project));
            assert!(
                !enabled.iter().any(|(n, _)| n == "project-skill"),
                "项目技能默认关：不应入组合目录"
            );
            assert!(enabled.iter().any(|(n, _)| n == "visualizer"));
            let (_, src) = enabled.iter().find(|(n, _)| n == "visualizer").unwrap();
            assert_eq!(src, &bundle.join("visualizer"));

            // 开启后：项目技能入集 + 同名覆盖（.agents 优先级高于 .claude）
            set_project_skills_enabled(true);
            let enabled = enabled_skills_for(ConnectorScope::Code, Some(&project));
            assert!(enabled.iter().any(|(n, _)| n == "project-skill"));
            let (_, src) = enabled.iter().find(|(n, _)| n == "visualizer").unwrap();
            assert_eq!(
                src,
                &project.join(".agents").join("skills").join("visualizer"),
                "项目 .agents/skills 应覆盖市场同名技能"
            );
            // .pinvou/skills 仅次于 .agents/skills：覆盖 .claude 同名，
            // 但被 .agents 同名覆盖。
            let (_, src) = enabled.iter().find(|(n, _)| n == "pinvou-skill").unwrap();
            assert_eq!(
                src,
                &project.join(".pinvou").join("skills").join("pinvou-skill"),
                "项目 .pinvou/skills 应覆盖 .claude 同名技能"
            );
            let (_, src) = enabled.iter().find(|(n, _)| n == "visualizer").unwrap();
            assert_eq!(
                src,
                &project.join(".agents").join("skills").join("visualizer"),
                ".agents/skills 优先级应高于 .pinvou/skills（同名仍取 .agents）"
            );

            // plain scope 不受项目开关影响
            set_project_skills_enabled(false);
            let enabled = enabled_skills_for(ConnectorScope::Plain, Some(&project));
            assert!(!enabled.iter().any(|(n, _)| n == "project-skill"));
            let _ = std::fs::remove_dir_all(&project);
        });
    }

    #[test]
    fn session_skills_dir_uses_session_private_root() {
        // 会话技能物化(materialize_session_skills)落点即本目录(两个原测试断言同一等式)。
        let dir = paths::session_skills_dir("abc-123");
        assert_eq!(dir, paths::sessions_root().join("abc-123").join("skills"));
    }
}
