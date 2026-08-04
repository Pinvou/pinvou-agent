//! 会话名册（Roster）装配。
//!
//! 名册由底座从会话工作区的 `.codewhale/agents/*.toml` 加载（见
//! `fleet::roster::FleetRoster::load`），本模块的职责就是把文件摆对：把
//! 用户专家池的自创卡装配成专家角色。角色文件只承载身份与人设，不定义
//! 工具权限；子智能体的运行权限继承父会话 Plan / Yolo。**不播种内置默认角色**（用户
//! 决策：委派本质是写提示词——有合适专家用 `profile` 指定，没有就让模型
//! 自拟任务说明裸派；旧版播种过的默认角色文件属"文件即真身"的用户可
//! 编辑遗留，不清理）。谁来把这些文件交给底座（构造 EngineConfig）是
//! assistant 侧的事，本模块不碰引擎配置。

fn roles_dir(workspace: &std::path::Path) -> std::path::PathBuf {
    workspace.join(deepseek_tui::fleet::profile::WORKSPACE_AGENT_PROFILE_DIR)
}

// ── 专家池入册 ────────────────────────────────────────────────────────────
//
// 专家池的**用户自创卡**入册为专家角色（数据通路版）：模型编排时可以把
// 子任务派给"某某专家"，被派中的子智能体拿到整张卡的人设正文。内置面具卡不
// 自动入册——那是聊天用的通用卡池，几十张全塞既撑名册又未必是用户想要的
// "我的专家"；选择性入册留给以后的界面。

/// 入册上限。名单会以每专家一行进入主 agent 的入口消息，塞太多既撑上下文
/// 又让派工混乱；完整人设只进被派中的子智能体提示
/// （底座 `spawn_profile_prompt_overlay`），主 agent 全程不付全文成本。
pub const EXPERT_ENROLL_LIMIT: usize = 8;

/// 专家角色 id：`exp-<slug>`。前缀自成命名空间（也与旧版默认角色隔开）；
/// slug 只留底座校验允许的 ASCII token 字符，其余折成 `-`。
fn expert_role_slug(card_id: &str) -> String {
    let slug: String = card_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "exp-persona".to_string()
    } else {
        format!("exp-{slug}")
    }
}

/// 选卡并分配去重后的角色 id。选择与 id 分配必须一次完成——入册文件和入口
/// 消息里的名单都用它，两边错位的话模型派的名字会查无此人。
fn enrolled_experts() -> Vec<(String, crate::features::personas::PersonaCard)> {
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for summary in crate::features::personas::all_summaries() {
        if summary.source != "user" {
            continue;
        }
        let Some(card) = crate::features::personas::get(&summary.id) else {
            continue;
        };
        let base = expert_role_slug(&card.id);
        let mut role_id = base.clone();
        let mut suffix = 1;
        while !used.insert(role_id.clone()) {
            suffix += 1;
            role_id = format!("{base}-{suffix}");
        }
        out.push((role_id, card));
        if out.len() >= EXPERT_ENROLL_LIMIT {
            break;
        }
    }
    out
}

/// 生成专家角色 TOML。经 `toml` 序列化而非手拼字符串：卡片正文是任意 markdown
/// （引号、三引号、反斜杠都可能出现），手拼等于给自己埋注入炸弹。
fn expert_profile_toml(
    role_id: &str,
    card: &crate::features::personas::PersonaCard,
) -> Option<String> {
    let mut root = toml::map::Map::new();
    root.insert("id".into(), toml::Value::String(role_id.to_string()));
    // **不写 base_role**：底座 role_name 会回落到成员 id（exp-*），随后
    // apply_spawn_profile 把它写进 assignment.role → worker ledger 的
    // spec.role 就是专家 id，面板据此解析出专家池头像与名字。运行语义
    // 不变：未知 role 名在 fleet_role_to_agent_type 回落 General；角色文件
    // 不写工具姿态，实际权限继续由父会话 Plan / Yolo 与底座运行时约束决定。
    root.insert(
        "display_name".into(),
        toml::Value::String(card.name.clone()),
    );
    root.insert(
        "description".into(),
        toml::Value::String(format!("专家：{}", card.description)),
    );
    let mut instructions = toml::map::Map::new();
    instructions.insert("text".into(), toml::Value::String(card.body.clone()));
    root.insert("instructions".into(), toml::Value::Table(instructions));
    toml::to_string_pretty(&toml::Value::Table(root)).ok()
}

/// 工作区是否曾装配过专家池投影。多智能体开关关闭时文件会保留，专家池
/// 后续增删改仍需刷新这些旧投影及其在跑引擎，避免已删除专家继续可用。
pub fn has_expert_role_projection(workspace: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(roles_dir(workspace)) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.starts_with("exp-") && name.ends_with(".toml")
    })
}

/// 把专家池入册进工作区名册。每次装配**全量重写** `exp-*.toml` 并清掉已删卡
/// 的残留：专家的真身是专家池里那张卡，角色文件只是投影——编辑应发生在卡上。
/// 非 `exp-` 文件一概不动（含旧版播种的默认角色：文件即真身，用户可编辑）。
/// 写盘/清理失败向上抛——静默吞掉的话开关显示已开启，派工却报 profile 不存在。
pub fn enroll_expert_roles(workspace: &std::path::Path) -> Result<(), String> {
    let dir = roles_dir(workspace);
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("创建角色目录失败 {}: {err}", dir.display()))?;
    let mut kept: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (role_id, card) in enrolled_experts() {
        let Some(body) = expert_profile_toml(&role_id, &card) else {
            continue;
        };
        let file_name = format!("{role_id}.toml");
        std::fs::write(dir.join(&file_name), body)
            .map_err(|err| format!("写入专家角色 {role_id} 失败: {err}"))?;
        kept.insert(file_name);
    }
    let entries = std::fs::read_dir(&dir)
        .map_err(|err| format!("读取角色目录失败 {}: {err}", dir.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("exp-") && name.ends_with(".toml") && !kept.contains(&name) {
            std::fs::remove_file(entry.path())
                .map_err(|err| format!("清理已删专家 {name} 失败: {err}"))?;
        }
    }
    Ok(())
}

/// 主 agent 可派角色的一行式名单（拼进每轮委派提醒）。
///
/// 底座不会把自定义名册成员列给主 agent（真机验证过：它只认内置别名），这份
/// 名单是模型知道"有哪些专家可派"的唯一途径；成本 = 每人一行。专家池为空时
/// 返回空表，提醒转而教模型自拟任务说明裸派。
pub fn available_role_lines() -> Vec<String> {
    enrolled_experts()
        .into_iter()
        .map(|(role_id, card)| {
            let mut description: String = card.description.chars().take(40).collect();
            if description.trim().is_empty() {
                description = card.name.clone();
            }
            format!("{role_id}：{description}（专家 · {}）", card.name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::paths;

    /// 借 paths 的进程级 env 锁，避免与其它写 PINVOU3_HOME 的测试并发打架。
    fn isolated_home() -> std::sync::MutexGuard<'static, ()> {
        let guard = paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-roster-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("PINVOU3_HOME", &tmp);
        guard
    }

    /// 专家池用户卡入册为继承父会话权限的专家角色；正文经 toml 序列化不被
    /// 特殊字符炸开；删卡后残留清掉；整册必须能被**底座名册真实解析**。
    #[test]
    fn user_personas_enroll_as_session_authority_expert_roles() {
        let _guard = isolated_home();
        let card = crate::features::personas::PersonaCard {
            id: "行业分析师".into(),
            dept: "research".into(),
            name: "行业分析师".into(),
            description: "竞品与市场格局分析".into(),
            emoji: "📊".into(),
            color: "#123456".into(),
            body: "你是行业分析师。\n引号\"三引号\"\"\"反斜杠\\结尾".into(),
            source: "user".into(),
            conversational_only: false,
        };
        let created = crate::features::personas::create_user_persona(card).expect("create persona");

        let workspace = paths::pinvou3_home().join("ws");
        enroll_expert_roles(&workspace).expect("enroll experts");

        let dir = roles_dir(&workspace);
        let expert_files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("exp-"))
            .collect();
        assert_eq!(
            expert_files.len(),
            1,
            "一张用户卡应入册一名专家: {expert_files:?}"
        );
        let expert_id = expert_files[0].trim_end_matches(".toml").to_string();
        let profile_body = std::fs::read_to_string(dir.join(&expert_files[0])).unwrap();
        assert!(
            !profile_body.contains("posture") && !profile_body.contains("[tools]"),
            "专家人设不得另设工具权限；应继承父会话 Plan/Yolo: {profile_body}"
        );
        assert!(has_expert_role_projection(&workspace));

        // 底座名册必须能真实解析这份 TOML——手拼字符串遇到卡片正文里的引号
        // 就会在这里现形。
        let roster = deepseek_tui::fleet::roster::FleetRoster::load(
            &codewhale_config::FleetConfigToml::default(),
            &workspace,
        );
        let member = roster
            .get(&expert_id)
            .unwrap_or_else(|| panic!("专家角色未通过底座校验: {expert_id}"));
        // 身份链契约：role_name 必须回落到成员 id，worker ledger 才会记录
        // exp-* 供面板解析专家头像；写死 base_role=general 会让整链断掉。
        assert_eq!(
            member.profile.role.name, expert_id,
            "专家 profile 的 role 名必须是成员 id（不得写 base_role）"
        );

        // 主 agent 的名单里要有这位专家（它靠这份名单知道能派谁）
        let lines = available_role_lines();
        assert!(
            lines.iter().any(|l| l.starts_with(expert_id.as_str())),
            "名单缺专家: {lines:?}"
        );
        assert!(
            lines.iter().all(|l| l.starts_with("exp-")),
            "名单只来自专家池，不得再有内置角色行: {lines:?}"
        );

        // 旧版播种过的默认角色文件属"文件即真身"的用户可编辑遗留，
        // 清理只针对 exp-*。
        std::fs::write(dir.join("scout.toml"), "id = \"scout\"\n").unwrap();
        // 删卡 → 重新装配 → 专家文件清掉，遗留角色文件不受影响
        crate::features::personas::delete_user_persona(&created.id).expect("delete");
        enroll_expert_roles(&workspace).expect("re-enroll after delete");
        let after: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("exp-"))
            .collect();
        assert!(after.is_empty(), "删卡后的专家文件应被清掉: {after:?}");
        assert!(!has_expert_role_projection(&workspace));
        assert!(dir.join("scout.toml").exists(), "遗留角色文件不受清理影响");
    }
}
