//! 会话名册（Roster）装配。
//!
//! 名册由底座从会话工作区的 `.codewhale/agents/*.toml` 加载（见
//! `FleetRoster::load`），本模块的职责就是把文件摆对：把
//! 专家池中可执行的内置卡与用户自创卡装配成专家角色。角色文件只承载身份与人设，不定义
//! 工具权限；子智能体的运行权限继承父会话 Plan / Yolo。完整人设只在某个 `profile` 真正
//! 被派中后进入该子智能体提示词；主 agent 每轮只看到按当前任务匹配出的轻量候选名单。
//! 谁来把这些文件交给底座（构造 EngineConfig）是 assistant 侧的事，本模块不碰引擎配置。

fn roles_dir(workspace: &std::path::Path) -> std::path::PathBuf {
    workspace.join(deepseek_tui::WORKSPACE_AGENT_PROFILE_DIR)
}

// ── 专家池入册 ────────────────────────────────────────────────────────────
//
// 专家池的可执行卡注册为角色：底座名册持有完整 profile，主 agent 的每轮提醒则只列
// 最相关的短摘要。这样内置专家真正参与委派，同时不会把约两百张卡的正文灌入父上下文。

/// 每轮提供给主 agent 的候选上限。完整人设只进被派中的子智能体提示
/// （底座 `spawn_profile_prompt_overlay`），主 agent 全程不付全文成本。
pub const EXPERT_CANDIDATE_LIMIT: usize = 20;

const EXPERT_SUMMARY_CHAR_LIMIT: usize = 36;

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
fn delegatable_summaries() -> Vec<(String, crate::features::personas::PersonaSummary)> {
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for summary in crate::features::personas::executable_summaries() {
        let base = expert_role_slug(&summary.id);
        let mut role_id = base.clone();
        let mut suffix = 1;
        while !used.insert(role_id.clone()) {
            suffix += 1;
            role_id = format!("{base}-{suffix}");
        }
        out.push((role_id, summary));
    }
    out
}

fn delegatable_experts() -> Vec<(String, crate::features::personas::PersonaCard)> {
    delegatable_summaries()
        .into_iter()
        .filter_map(|(role_id, summary)| {
            crate::features::personas::get(&summary.id).map(|card| (role_id, card))
        })
        .collect()
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
    )
}

fn is_stop_term(term: &str) -> bool {
    matches!(
        term,
        "一个"
            | "一下"
            | "这个"
            | "那个"
            | "这些"
            | "那些"
            | "可以"
            | "需要"
            | "进行"
            | "当前"
            | "目前"
            | "是否"
            | "什么"
            | "怎么"
            | "如何"
            | "帮我"
            | "请问"
            | "任务"
            | "问题"
            | "专家"
            | "the"
            | "and"
            | "for"
            | "with"
            | "this"
            | "that"
            | "from"
            | "into"
            | "please"
            | "can"
            | "you"
            | "your"
            | "our"
            | "are"
            | "is"
            | "to"
            | "of"
            | "in"
            | "on"
    )
}

/// 不引入分词/向量依赖的本地匹配词提取：英文保留技术 token，连续中文生成 2~4 字片段。
/// 候选只比较卡片轻摘要，不扫描约 1.2MB 的完整人设正文，因此每轮成本稳定且不泄露正文。
fn query_terms(query: &str) -> Vec<String> {
    let mut terms = std::collections::HashSet::new();
    let mut ascii = String::new();
    let mut cjk = Vec::new();

    let flush_ascii = |value: &mut String, terms: &mut std::collections::HashSet<String>| {
        let token = value
            .trim_matches(|ch: char| matches!(ch, '.' | '-' | '_' | '+' | '#'))
            .to_ascii_lowercase();
        if token.len() >= 2 && !is_stop_term(&token) {
            terms.insert(token);
        }
        value.clear();
    };
    let flush_cjk = |value: &mut Vec<char>, terms: &mut std::collections::HashSet<String>| {
        for width in 2..=4.min(value.len()) {
            for start in 0..=value.len() - width {
                let token = value[start..start + width].iter().collect::<String>();
                if !is_stop_term(&token) {
                    terms.insert(token);
                }
            }
        }
        value.clear();
    };

    for ch in query.chars() {
        if is_cjk(ch) {
            flush_ascii(&mut ascii, &mut terms);
            cjk.push(ch);
        } else if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '+' | '#') {
            flush_cjk(&mut cjk, &mut terms);
            ascii.push(ch);
        } else {
            flush_ascii(&mut ascii, &mut terms);
            flush_cjk(&mut cjk, &mut terms);
        }
    }
    flush_ascii(&mut ascii, &mut terms);
    flush_cjk(&mut cjk, &mut terms);

    let mut terms = terms.into_iter().collect::<Vec<_>>();
    terms.sort_by(|a, b| {
        b.chars()
            .count()
            .cmp(&a.chars().count())
            .then_with(|| a.cmp(b))
    });
    terms.truncate(256);
    terms
}

fn term_score(field: &str, term: &str, weight: u32) -> u32 {
    if field.contains(term) {
        let technical_multiplier = if term.is_ascii() && term.len() >= 3 {
            3
        } else {
            1
        };
        weight * term.chars().count().min(8) as u32 * technical_multiplier
    } else {
        0
    }
}

fn expert_match_score(
    card: &crate::features::personas::PersonaSummary,
    query: &str,
    compact_query: &str,
    terms: &[String],
) -> u32 {
    let name = card.name.to_lowercase();
    let compact_name = name
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>();
    let id = card.id.to_lowercase();
    let dept = card.dept.to_lowercase();
    let description = card.description.to_lowercase();

    let mut score = 0;
    if compact_name.chars().count() >= 2 && compact_query.contains(&compact_name) {
        score += 10_000;
    }
    if id.len() >= 2 && query.contains(&id) {
        score += 8_000;
    }
    for term in terms {
        score += term_score(&name, term, 24);
        score += term_score(&id, term, 16);
        score += term_score(&dept, term, 10);
        score += term_score(&description, term, 6);
    }
    score
}

fn matched_experts(task: &str) -> Vec<(String, crate::features::personas::PersonaSummary)> {
    let query = task.to_lowercase();
    let compact_query = query
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>();
    let terms = query_terms(&query);
    let mut candidates = delegatable_summaries()
        .into_iter()
        .filter_map(|(role_id, card)| {
            let score = expert_match_score(&card, &query, &compact_query, &terms);
            // 用户自创卡代表用户主动维护的角色，始终进入候选竞争；内置卡只有与本轮
            // 任务存在文本相关性时才占上下文。
            if card.source == "user" || score > 0 {
                Some((role_id, card, score))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        let a_user = a.1.source == "user";
        let b_user = b.1.source == "user";
        b_user
            .cmp(&a_user)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.1.name.cmp(&b.1.name))
            .then_with(|| a.0.cmp(&b.0))
    });
    candidates
        .into_iter()
        .take(EXPERT_CANDIDATE_LIMIT)
        .map(|(role_id, card, _)| (role_id, card))
        .collect()
}

fn short_single_line(value: &str, limit: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut short = normalized.chars().take(limit).collect::<String>();
    if normalized.chars().count() > limit {
        short.push('…');
    }
    short.replace('`', "")
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

/// 把专家池入册进工作区名册。每次装配全量对账 `exp-*.toml`，内容变化时才写盘，并清掉
/// 已删卡的残留：专家的真身是专家池里那张卡，角色文件只是投影——编辑应发生在卡上。
/// 非 `exp-` 文件一概不动（含旧版播种的默认角色：文件即真身，用户可编辑）。
/// 写盘/清理失败向上抛——静默吞掉的话开关显示已开启，派工却报 profile 不存在。
pub fn enroll_expert_roles(workspace: &std::path::Path) -> Result<(), String> {
    let dir = roles_dir(workspace);
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("创建角色目录失败 {}: {err}", dir.display()))?;
    let mut kept: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (role_id, card) in delegatable_experts() {
        let Some(body) = expert_profile_toml(&role_id, &card) else {
            continue;
        };
        let file_name = format!("{role_id}.toml");
        let path = dir.join(&file_name);
        let unchanged = std::fs::read(&path)
            .map(|existing| existing == body.as_bytes())
            .unwrap_or(false);
        if !unchanged {
            std::fs::write(&path, body)
                .map_err(|err| format!("写入专家角色 {role_id} 失败: {err}"))?;
        }
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

/// 主 agent 本轮可派角色的一行式候选名单（拼进每轮委派提醒）。
///
/// 底座不会把自定义名册成员列给主 agent（真机验证过：它只认内置别名），这份
/// 名单是模型知道"有哪些专家可派"的唯一途径。每轮只列匹配出的最多 20 位候选；
/// profile 全文已在底座名册内，但只有实际被派中时才注入对应子智能体。
pub fn available_role_lines(task: &str) -> Vec<String> {
    matched_experts(task)
        .into_iter()
        .map(|(role_id, card)| {
            let name = short_single_line(&card.name, EXPERT_SUMMARY_CHAR_LIMIT);
            let mut description = short_single_line(&card.description, EXPERT_SUMMARY_CHAR_LIMIT);
            if description.trim().is_empty() {
                description = name.clone();
            }
            format!("- `{role_id}`：{name}｜{description}")
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

    /// 内置卡与用户卡都入册为继承父会话权限的专家角色；用户卡在本轮候选中优先，
    /// 正文经 toml 序列化不被特殊字符炸开；删卡后残留清掉；整册必须能被底座真实解析。
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
        let expert_id = expert_role_slug(&created.id);
        let expert_files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("exp-"))
            .collect();
        assert!(
            expert_files.len() > 100,
            "内置可执行专家与用户卡都应入册: {}",
            expert_files.len()
        );
        let user_profile = dir.join(format!("{expert_id}.toml"));
        assert!(user_profile.exists(), "用户专家投影缺失: {expert_id}");
        assert!(
            dir.join("exp-engineering-frontend-developer.toml").exists(),
            "内置前端专家应可供委派"
        );
        assert!(
            !dir.join("exp-pinvou-card-creator.toml").exists(),
            "纯对话卡牌制造器不得成为执行型子智能体"
        );
        let profile_body = std::fs::read_to_string(&user_profile).unwrap();
        assert!(
            !profile_body.contains("posture") && !profile_body.contains("[tools]"),
            "专家人设不得另设工具权限；应继承父会话 Plan/Yolo: {profile_body}"
        );
        assert!(has_expert_role_projection(&workspace));

        // 底座名册必须能真实解析这份 TOML——手拼字符串遇到卡片正文里的引号
        // 就会在这里现形。
        let roster = deepseek_tui::FleetRoster::load(
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

        // 主 agent 每轮只看到短候选：用户卡优先，相关内置卡补齐，完整正文不泄露。
        let lines = available_role_lines("请用 React 检查前端，并分析竞品与市场格局");
        assert!(
            lines.first().is_some_and(|line| line.contains(&expert_id)),
            "用户自创专家应优先进入候选: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("exp-engineering-frontend-developer")),
            "任务相关的内置前端专家应进入候选: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains(expert_id.as_str())),
            "名单缺专家: {lines:?}"
        );
        assert!(
            lines.iter().all(|l| l.starts_with("- `exp-")),
            "名单只来自专家池，不得再有内置角色行: {lines:?}"
        );
        assert!(lines.len() <= EXPERT_CANDIDATE_LIMIT);
        assert!(
            lines.iter().all(|line| !line.contains("三引号")),
            "主 agent 候选不得携带专家完整正文: {lines:?}"
        );

        // 旧版播种过的默认角色文件属"文件即真身"的用户可编辑遗留，
        // 清理只针对 exp-*。
        std::fs::write(dir.join("scout.toml"), "id = \"scout\"\n").unwrap();
        // 删卡 → 重新装配 → 专家文件清掉，遗留角色文件不受影响
        crate::features::personas::delete_user_persona(&created.id).expect("delete");
        enroll_expert_roles(&workspace).expect("re-enroll after delete");
        assert!(!user_profile.exists(), "删卡后的用户专家投影应被清掉");
        assert!(
            dir.join("exp-engineering-frontend-developer.toml").exists(),
            "删除用户卡不得清掉内置专家投影"
        );
        assert!(has_expert_role_projection(&workspace));
        assert!(dir.join("scout.toml").exists(), "遗留角色文件不受清理影响");
    }

    #[test]
    fn task_matching_is_local_bounded_and_excludes_conversational_cards() {
        let lines = available_role_lines(
            "开发 设计 产品 营销 数据 运营 安全 测试 管理 分析 用户 内容 工程 React",
        );
        assert_eq!(
            lines.len(),
            EXPERT_CANDIDATE_LIMIT,
            "宽任务应截断到固定候选上限"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("exp-engineering-frontend-developer")),
            "React 关键词应命中内置前端专家: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .all(|line| !line.contains("exp-pinvou-card-creator")),
            "纯对话元卡不得出现在候选中"
        );
    }
}
