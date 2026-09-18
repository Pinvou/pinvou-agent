//! Pinvou 专家池到 CodeWhale Fleet 配置的纯内存投影。
//!
//! 专家卡是唯一持久化真身；本模块把同一次卡池读取转换为不可变快照，同时
//! 产出底座 `[fleet.profiles]` 与主 agent 的轻量候选索引。快照不写用户项目、
//! 不写 CodeWhale 个人目录，也不再给每个会话复制一整套 TOML。
//!
//! 每轮候选由本地轻量关键词匹配（含泛化词抑制与相关性门槛）从轻摘要中挑选，
//! 仅作提醒提示；候选之外的专家可经底座 roster 通道发现（列表按编号排序、
//! 至多 48 条，截断由响应如实标注），不依赖这里的截断。

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use codewhale_config::{FleetConfigToml, FleetProfile, FleetRole, FleetSlot};

/// 一轮多智能体交互共享的专家快照。
///
/// `fleet_config` 决定实际可派 profile，`candidates` 决定提醒里展示哪些 id；二者
/// 在构造时由同一批 [`crate::features::personas::PersonaCard`] 生成，避免专家 CRUD
/// 恰好发生在两次读取之间造成“提醒里有、派工时没有”或人设版本错位。
#[derive(Debug)]
pub struct ExpertRosterSnapshot {
    fleet_config: FleetConfigToml,
    candidates: Vec<(String, crate::features::personas::PersonaSummary)>,
}

static SNAPSHOT_CACHE: OnceLock<RwLock<Option<(u64, Arc<ExpertRosterSnapshot>)>>> = OnceLock::new();

fn snapshot_cache() -> &'static RwLock<Option<(u64, Arc<ExpertRosterSnapshot>)>> {
    SNAPSHOT_CACHE.get_or_init(|| RwLock::new(None))
}

impl ExpertRosterSnapshot {
    /// 从当前全局专家池获取一次完整、内部一致的快照。
    #[must_use]
    pub fn capture() -> Arc<Self> {
        loop {
            let before = crate::features::personas::executable_revision();
            // The snapshot cache only speeds things up; a panic while holding
            // the lock must not take down sessions: follow the repo-wide lock
            // poisoning recovery convention.
            if let Some(snapshot) = snapshot_cache()
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
                .filter(|(revision, _)| *revision == before)
                .map(|(_, snapshot)| Arc::clone(snapshot))
            {
                return snapshot;
            }

            let cards = crate::features::personas::executable_cards();
            let after = crate::features::personas::executable_revision();
            if before != after {
                continue;
            }
            let candidate = Arc::new(Self::from_cards(cards));
            // Same as above: on write-lock poisoning recover the guard and
            // continue; the cache content is replaced wholesale, so there is
            // no partial-write risk.
            let mut cache = snapshot_cache()
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((revision, snapshot)) = cache.as_ref() {
                if *revision == after {
                    return Arc::clone(snapshot);
                }
            }
            // reload_user may have published another version while we waited
            // for the cache lock. Never install the now-stale candidate.
            if crate::features::personas::executable_revision() != after {
                drop(cache);
                continue;
            }
            *cache = Some((after, Arc::clone(&candidate)));
            return candidate;
        }
    }

    fn from_cards(cards: Vec<crate::features::personas::PersonaCard>) -> Self {
        let mut used = HashSet::new();
        let mut profiles = BTreeMap::new();
        let mut candidates = Vec::with_capacity(cards.len());
        for card in cards {
            let summary = card.summary();
            let base = expert_role_slug(&card.id);
            let mut role_id = base.clone();
            let mut suffix = 1;
            while !used.insert(role_id.clone()) {
                suffix += 1;
                role_id = format!("{base}-{suffix}");
            }
            let profile = FleetProfile {
                slot: FleetSlot::Custom(role_id.clone()),
                role: FleetRole {
                    // 保留 exp-* 作为实际 role 名：worker ledger 与前端据此解析
                    // 专家身份；写成 general 会切断身份链。
                    name: role_id.clone(),
                    description: Some(format!("专家：{}", card.description)),
                    instructions: Some(card.body),
                },
                ..FleetProfile::default()
            };
            profiles.insert(role_id.clone(), profile);
            candidates.push((role_id, summary));
        }
        Self {
            fleet_config: FleetConfigToml {
                profiles,
                ..FleetConfigToml::default()
            },
            candidates,
        }
    }

    #[must_use]
    pub fn fleet_config(&self) -> &FleetConfigToml {
        &self.fleet_config
    }

    /// 为当前任务生成与本快照严格同源的轻量候选行。
    ///
    /// 行序按匹配分数降序（用户卡仅在分数并列时优先），只含短摘要、仅作提醒
    /// 提示；候选之外的专家可经底座 roster 通道发现（roster 无分页，单次列表
    /// 至多 48 条，截断尾部靠 `profile_query` 关键词过滤继续发现）。
    #[must_use]
    pub fn available_role_lines(&self, task: &str) -> Vec<String> {
        matched_experts(&self.candidates, task)
            .into_iter()
            .map(|(role_id, card)| {
                // 先剥信封标签再截断：截断会把标签腰斩成底座匹配不到完整标签的
                // 残片，剥除必须发生在 `short_single_line` 之前。
                let name = short_single_line(
                    &strip_reminder_tag_literals(&card.name),
                    EXPERT_SUMMARY_CHAR_LIMIT,
                );
                let mut description = short_single_line(
                    &strip_reminder_tag_literals(&card.description),
                    EXPERT_SUMMARY_CHAR_LIMIT,
                );
                if description.trim().is_empty() {
                    description = name.clone();
                }
                format!("- `{role_id}`：{name}｜{description}")
            })
            .collect()
    }
}

/// 候选行整体进 `<system-reminder>` 信封（bridge 组装，底座按首个闭合标签
/// 截取信封），而名字/描述来自用户自建卡。剥掉信封标签字面量，防止卡片文案
/// 提前闭合信封、把后续轮内容挤出 working_set 豁免路径。调用方必须先剥后截。
fn strip_reminder_tag_literals(value: &str) -> String {
    value
        .replace("<system-reminder>", "")
        .replace("</system-reminder>", "")
}

// ── 专家池入册 ────────────────────────────────────────────────────────────
//
// 专家池的可执行卡注册为角色：底座名册持有完整 profile，主 agent 的每轮提醒则只列
// 最相关的短摘要。这样内置专家真正参与委派，同时不会把约两百张卡的正文灌入父上下文。
//
// 每轮匹配是本地轻量关键词启发式（泛化词抑制 + 相关性门槛 + 候选截断），结果只作
// 提醒提示；候选之外的专家可经底座 roster 通道发现（列表按编号排序、至多 48 条，
// 截断由响应如实标注），不依赖这里的截断结果。

/// 每轮提供给主 agent 的候选上限。完整人设只进被派中的子智能体提示
/// （底座 `spawn_profile_prompt_overlay`），主 agent 全程不付全文成本。
/// 候选越少越依赖匹配的区分度（见 [`generic_terms`] 的泛化词抑制）。
pub const EXPERT_CANDIDATE_LIMIT: usize = 8;

const EXPERT_SUMMARY_CHAR_LIMIT: usize = 36;
/// role_id 总长上限（`exp-` 前缀 + slug）：底座 spawn 选择器限 128 字符，
/// 留足冲突后缀余量后取 120，见 [`expert_role_slug`]。
const EXPERT_ROLE_ID_CHAR_LIMIT: usize = 120;
/// 专家匹配只需任务主题与末尾约束；限制用于防止超长粘贴在本地 n-gram
/// 提取阶段产生与输入长度线性增长的大量临时字符串。
const EXPERT_QUERY_CHAR_LIMIT: usize = 4096;
const EXPERT_QUERY_HEAD_CHARS: usize = 3072;
const EXPERT_QUERY_TAIL_CHARS: usize = EXPERT_QUERY_CHAR_LIMIT - EXPERT_QUERY_HEAD_CHARS;

/// 专家角色 id：`exp-<slug>`。前缀自成命名空间（也与旧版默认角色隔开）；
/// slug 只留底座校验允许的 ASCII token 字符，其余折成 `-`。
///
/// 底座 spawn 选择器限 128 字符（`validate_roster_selector`）：slug 超长时
/// 截断，保证任何卡的 role_id（含冲突去重后缀）都不会落进「可列出、
/// 永不可派」的死区。
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
    // `exp-` 前缀 4 字符 + 冲突后缀至多 `-NNN` 4 字符，120 + 8 仍在 128 以内。
    let slug = slug
        .chars()
        .take(EXPERT_ROLE_ID_CHAR_LIMIT - 4)
        .collect::<String>();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "exp-persona".to_string()
    } else {
        format!("exp-{slug}")
    }
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

fn bounded_match_query(query: &str) -> std::borrow::Cow<'_, str> {
    if query.chars().nth(EXPERT_QUERY_CHAR_LIMIT).is_none() {
        return std::borrow::Cow::Borrowed(query);
    }
    let head = query
        .chars()
        .take(EXPERT_QUERY_HEAD_CHARS)
        .collect::<String>();
    let mut tail = query
        .chars()
        .rev()
        .take(EXPERT_QUERY_TAIL_CHARS)
        .collect::<Vec<_>>();
    tail.reverse();
    std::borrow::Cow::Owned(format!("{head}\n{}", tail.into_iter().collect::<String>()))
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

/// 泛化词抑制（文档频率过滤）：像“优化”“性能”这类短泛词几乎能在大多数卡的
/// 轻摘要里命中，对排序毫无区分度，只会把大量无关卡顶进候选。打分前先统计每个
/// 查询词命中的卡片数（与打分同源的 name/id/dept/description 子串匹配），命中数
/// 超过 `max(8, 卡池/20)` 的词本轮对所有卡贡献 0 分，等价于从查询中剔除该词。
fn generic_terms(
    roster: &[(String, crate::features::personas::PersonaSummary)],
    terms: &[String],
) -> HashSet<String> {
    let max_df = std::cmp::max(8, roster.len() / 20);
    let mut generic = HashSet::new();
    if terms.is_empty() {
        return generic;
    }
    // 与打分一样按小写比较；每张卡只小写化一次，避免逐词重复分配临时串。
    let fields = roster
        .iter()
        .map(|(_, card)| {
            (
                card.name.to_lowercase(),
                card.id.to_lowercase(),
                card.dept.to_lowercase(),
                card.description.to_lowercase(),
            )
        })
        .collect::<Vec<_>>();
    for term in terms {
        let mut hits = 0usize;
        for (name, id, dept, description) in &fields {
            let matched = name.contains(term.as_str())
                || id.contains(term.as_str())
                || dept.contains(term.as_str())
                || description.contains(term.as_str());
            if matched {
                hits += 1;
                // 已确认超限即可提前退出，不必数完整个卡池。
                if hits > max_df {
                    break;
                }
            }
        }
        if hits > max_df {
            generic.insert(term.clone());
        }
    }
    generic
}

fn expert_match_score(
    card: &crate::features::personas::PersonaSummary,
    query: &str,
    compact_query: &str,
    terms: &[String],
    generic: &HashSet<String>,
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
        // 被泛化词抑制命中的词对所有卡一律 0 分，保持相对排序不受泛词干扰。
        if generic.contains(term) {
            continue;
        }
        score += term_score(&name, term, 24);
        score += term_score(&id, term, 16);
        score += term_score(&dept, term, 10);
        score += term_score(&description, term, 6);
    }
    score
}

fn matched_experts(
    roster: &[(String, crate::features::personas::PersonaSummary)],
    task: &str,
) -> Vec<(String, crate::features::personas::PersonaSummary)> {
    // 头部通常承载任务目标，尾部通常承载补充约束；同时保留二者比只截头更稳。
    let query = bounded_match_query(task).to_lowercase();
    let compact_query = query
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>();
    let terms = query_terms(&query);
    // 先做泛化词抑制再打分：被抑制的词对所有卡贡献 0 分，避免“优化”“性能”
    // 这类泛词把无关卡批量顶进候选。
    let generic = generic_terms(roster, &terms);
    let mut candidates = roster
        .iter()
        .cloned()
        .filter_map(|(role_id, card)| {
            let score = expert_match_score(&card, &query, &compact_query, &terms, &generic);
            // 用户自创卡不再凭身份无条件占位：与内置卡一样必须与本轮任务存在
            // 文本相关性（分数>0）才进入候选，仅在分数并列时作为同分平局优先。
            if score > 0 {
                Some((role_id, card, score))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        let a_user = a.1.source == "user";
        let b_user = b.1.source == "user";
        b.2.cmp(&a.2)
            .then_with(|| b_user.cmp(&a_user))
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

/// 清理旧版本写入 Pinvou 会话账本的专家 TOML 投影。
///
/// 迁移边界刻意很窄：`ledger` 必须是 `sessions_root/<id>/workspace` 这一层的
/// App-owned 目录；目录链中存在 symlink / Windows reparse point，或规范化后越出
/// `sessions_root` 时一律拒绝。只删除 `.codewhale/agents/exp-*.toml` 普通文件，
/// 保留非 `exp-` 文件，也绝不沿 execution root 查找。
pub fn cleanup_legacy_expert_projection(
    ledger: &Path,
    sessions_root: &Path,
) -> Result<usize, String> {
    let session_dir = ledger
        .parent()
        .ok_or_else(|| format!("旧专家投影账本没有会话父目录: {}", ledger.display()))?;
    if ledger.file_name().and_then(|name| name.to_str()) != Some("workspace")
        || session_dir.parent() != Some(sessions_root)
    {
        return Err(format!(
            "拒绝在非 Pinvou 会话账本清理旧专家投影: {}",
            ledger.display()
        ));
    }

    let dir = ledger.join(deepseek_tui::WORKSPACE_AGENT_PROFILE_DIR);
    match std::fs::symlink_metadata(&dir) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(format!("检查旧专家投影目录失败 {}: {error}", dir.display()));
        }
    }

    // 不允许从 App-owned 会话根经链接或 junction 跳到外部。sessions_root 本身
    // 可以由 PINVOU3_HOME 重定位甚至链接，因此把它 canonicalize 为信任锚点；
    // 从具体 session 目录开始的每一层则必须是实体目录。
    for component in [
        session_dir,
        ledger,
        dir.parent().ok_or_else(|| {
            format!(
                "legacy expert projection dir has no parent: {}",
                dir.display()
            )
        })?,
        &dir,
    ] {
        let metadata = std::fs::symlink_metadata(component).map_err(|error| {
            format!("检查旧专家投影目录链失败 {}: {error}", component.display())
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "拒绝经链接或非目录清理旧专家投影: {}",
                component.display()
            ));
        }
    }
    let canonical_sessions = std::fs::canonicalize(sessions_root)
        .map_err(|error| format!("规范化会话根失败 {}: {error}", sessions_root.display()))?;
    let canonical_session = std::fs::canonicalize(session_dir)
        .map_err(|error| format!("规范化会话目录失败 {}: {error}", session_dir.display()))?;
    let canonical_ledger = std::fs::canonicalize(ledger)
        .map_err(|error| format!("规范化会话账本失败 {}: {error}", ledger.display()))?;
    let canonical_dir = std::fs::canonicalize(&dir)
        .map_err(|error| format!("规范化旧专家投影目录失败 {}: {error}", dir.display()))?;
    let session_name = session_dir
        .file_name()
        .ok_or_else(|| format!("会话目录没有名称: {}", session_dir.display()))?;
    let expected_session = canonical_sessions.join(session_name);
    let expected_ledger = expected_session.join("workspace");
    let expected_dir = expected_ledger.join(deepseek_tui::WORKSPACE_AGENT_PROFILE_DIR);
    if canonical_session != expected_session
        || canonical_ledger != expected_ledger
        || canonical_dir != expected_dir
    {
        return Err(format!(
            "拒绝清理规范化后越出会话根的旧专家投影: {}",
            dir.display()
        ));
    }

    let entries = std::fs::read_dir(&dir)
        .map_err(|error| format!("读取旧专家投影目录失败 {}: {error}", dir.display()))?;
    let mut removed = 0;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("读取旧专家投影条目失败 {}: {error}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("exp-") && name.ends_with(".toml") {
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| format!("检查旧专家投影 {name} 失败: {error}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(format!("拒绝删除链接或非普通文件的旧专家投影: {name}"));
            }
            let canonical_path = std::fs::canonicalize(&path)
                .map_err(|error| format!("规范化旧专家投影 {name} 失败: {error}"))?;
            if canonical_path != canonical_dir.join(entry.file_name()) {
                return Err(format!("拒绝删除越出名册目录的旧专家投影: {name}"));
            }
            std::fs::remove_file(entry.path())
                .map_err(|error| format!("清理旧专家投影 {name} 失败: {error}"))?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// `pub(crate)` 仅供其他模块的 `#[cfg(test)]` 复用隔离守卫（同 crate 测试
/// 基建，参照 `platform::paths::tests::ENV_LOCK` 的共享先例）。
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::features::personas::PersonaCard;

    fn card(id: &str, name: &str, source: &str, body: &str) -> PersonaCard {
        PersonaCard {
            id: id.into(),
            dept: "engineering".into(),
            name: name.into(),
            description: "React 与前端工程".into(),
            emoji: "🧰".into(),
            color: "#123456".into(),
            body: body.into(),
            source: source.into(),
            conversational_only: false,
        }
    }

    /// 把 PINVOU3_HOME 隔离到临时目录的 RAII 守卫：构造时切换并 reload_user，
    /// 析构时恢复原值、刷新卡池并清理临时目录。调用方必须先持有
    /// `crate::platform::paths::tests::ENV_LOCK`，保证环境变量写入串行化；
    /// 这样 capture() 只看到内嵌 268 卡，不受开发机用户卡或并行测试影响。
    pub(crate) struct PersonaHomeGuard {
        previous: Option<std::ffi::OsString>,
        home: std::path::PathBuf,
    }

    impl PersonaHomeGuard {
        pub(crate) fn setup(tag: &str) -> Self {
            let previous = std::env::var_os("PINVOU3_HOME");
            let home = std::env::temp_dir().join(format!(
                "pinvou3-roster-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&home);
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            unsafe { std::env::set_var("PINVOU3_HOME", &home) };
            crate::features::personas::reload_user();
            Self { previous, home }
        }
    }

    impl Drop for PersonaHomeGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
                Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
                // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
                None => unsafe { std::env::remove_var("PINVOU3_HOME") },
            }
            crate::features::personas::reload_user();
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    #[test]
    fn one_snapshot_drives_profiles_and_candidate_ids() {
        let snapshot = ExpertRosterSnapshot::from_cards(vec![
            card(
                "engineering-frontend",
                "前端专家",
                "builtin",
                "PROFILE_SENTINEL",
            ),
            card("user-reviewer", "自建评审", "user", "USER_SENTINEL"),
        ]);
        let config = snapshot.fleet_config();
        let user_id = "exp-user-reviewer";
        let member = config.profiles.get(user_id).expect("user profile");
        assert_eq!(member.role.name, user_id);
        assert_eq!(member.role.instructions.as_deref(), Some("USER_SENTINEL"));

        let lines = snapshot.available_role_lines("请审查 React 前端");
        // 用户卡不再无条件置顶：与任务相关的用户卡仍进候选，但排序只看分数。
        assert!(
            lines.iter().any(|line| line.contains(user_id)),
            "与任务相关的用户卡应进入候选: {lines:?}"
        );
        for line in &lines {
            let id = line
                .strip_prefix("- `")
                .and_then(|rest| rest.split('`').next())
                .expect("candidate id");
            assert!(
                config.profiles.contains_key(id),
                "候选必须存在于同轮名册: {id}"
            );
            assert!(!line.contains("SENTINEL"), "候选不得泄露完整人设");
        }
    }

    #[test]
    fn capture_reuses_revision_and_persona_crud_invalidates_snapshot() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PersonaHomeGuard::setup("cache");

        let first = ExpertRosterSnapshot::capture();
        let same_revision = ExpertRosterSnapshot::capture();
        assert!(
            Arc::ptr_eq(&first, &same_revision),
            "同一 Persona 版本应只 Arc-clone 已构造快照"
        );

        let created = crate::features::personas::create_user_persona(card(
            "ignored-on-create",
            "缓存失效专家",
            "user",
            "CACHE_SENTINEL_V1",
        ))
        .expect("create cached expert");
        let after_create = ExpertRosterSnapshot::capture();
        assert!(!Arc::ptr_eq(&first, &after_create));
        let role_id = expert_role_slug(&created.id);
        assert_eq!(
            after_create
                .fleet_config()
                .profiles
                .get(&role_id)
                .and_then(|profile| profile.role.instructions.as_deref()),
            Some("CACHE_SENTINEL_V1")
        );

        crate::features::personas::update_user_persona(card(
            &created.id,
            "缓存失效专家",
            "user",
            "CACHE_SENTINEL_V2",
        ))
        .expect("update cached expert");
        let after_update = ExpertRosterSnapshot::capture();
        assert!(!Arc::ptr_eq(&after_create, &after_update));
        assert_eq!(
            after_update
                .fleet_config()
                .profiles
                .get(&role_id)
                .and_then(|profile| profile.role.instructions.as_deref()),
            Some("CACHE_SENTINEL_V2")
        );

        crate::features::personas::delete_user_persona_with(&created.id, || ())
            .expect("delete cached expert");
        let after_delete = ExpertRosterSnapshot::capture();
        assert!(!after_delete.fleet_config().profiles.contains_key(&role_id));
    }

    #[test]
    fn legacy_cleanup_is_narrow_idempotent_and_preserves_other_profiles() {
        let sessions_root = std::env::temp_dir().join(format!(
            "pinvou3-roster-cleanup-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let root = sessions_root.join("safe-session").join("workspace");
        let dir = root.join(deepseek_tui::WORKSPACE_AGENT_PROFILE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("exp-old.toml"), "id = 'exp-old'").unwrap();
        std::fs::write(dir.join("scout.toml"), "id = 'scout'").unwrap();

        assert_eq!(
            cleanup_legacy_expert_projection(&root, &sessions_root).unwrap(),
            1
        );
        assert_eq!(
            cleanup_legacy_expert_projection(&root, &sessions_root).unwrap(),
            0
        );
        assert!(!dir.join("exp-old.toml").exists());
        assert!(dir.join("scout.toml").exists());
        let _ = std::fs::remove_dir_all(sessions_root);
    }

    #[test]
    fn legacy_cleanup_rejects_ledger_outside_direct_session_child() {
        let base = std::env::temp_dir().join(format!(
            "pinvou3-roster-escape-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let sessions_root = base.join("sessions");
        let outside = base.join("outside").join("workspace");
        let dir = outside.join(deepseek_tui::WORKSPACE_AGENT_PROFILE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let profile = dir.join("exp-keep.toml");
        std::fs::write(&profile, "id = 'exp-keep'").unwrap();

        assert!(cleanup_legacy_expert_projection(&outside, &sessions_root).is_err());
        assert!(profile.exists(), "越界账本中的文件不得被删除");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn legacy_cleanup_rejects_symlinked_agents_directory() {
        let base = std::env::temp_dir().join(format!(
            "pinvou3-roster-link-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let sessions_root = base.join("sessions");
        let ledger = sessions_root.join("safe-session").join("workspace");
        let outside = base.join("outside-agents");
        std::fs::create_dir_all(ledger.join(".codewhale")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let profile = outside.join("exp-keep.toml");
        std::fs::write(&profile, "id = 'exp-keep'").unwrap();
        let link = ledger.join(deepseek_tui::WORKSPACE_AGENT_PROFILE_DIR);
        if !crate::platform::filesystem::tests::try_link_dir(&outside, &link) {
            let _ = std::fs::remove_dir_all(base);
            return;
        }

        assert!(cleanup_legacy_expert_projection(&ledger, &sessions_root).is_err());
        assert!(profile.exists(), "链接目标中的文件不得被删除");
        crate::platform::filesystem::tests::remove_dir_link(&link);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn task_matching_is_local_bounded_and_excludes_conversational_cards() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PersonaHomeGuard::setup("matching");
        let lines = ExpertRosterSnapshot::capture().available_role_lines(
            "开发 设计 产品 营销 数据 运营 安全 测试 管理 分析 用户 内容 工程 React",
        );
        // 全行业泛词经泛化词抑制后不再批量命中无关卡，候选远小于上限；
        // 有区分度的 React 仍应命中内置前端专家。
        assert!(
            lines.len() < EXPERT_CANDIDATE_LIMIT,
            "全泛化词任务抑制后不应填满候选上限: {lines:?}"
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

    #[test]
    fn generic_terms_are_suppressed_and_distinctive_terms_still_match() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PersonaHomeGuard::setup("generic-suppression");
        let snapshot = ExpertRosterSnapshot::capture();
        // “优化”这类泛化词在大卡池里大量命中、无区分度；抑制后纯泛化任务
        // （如“帮我优化一下”）不应再产生一堆无关候选。
        let generic_only = snapshot.available_role_lines("帮我优化一下");
        assert!(
            generic_only.is_empty(),
            "纯泛化词任务不应产生候选: {generic_only:?}"
        );
        // 同一句式加上有区分度的词后，正确专家仍应浮出。
        let distinctive = snapshot.available_role_lines("帮我优化一下 React 项目");
        assert!(
            distinctive
                .iter()
                .any(|line| line.contains("exp-engineering-frontend-developer")),
            "有区分度的词仍应命中内置前端专家: {distinctive:?}"
        );
    }

    #[test]
    fn user_cards_respect_threshold_and_win_only_score_ties() {
        // 同名同摘要的孪生卡分数必然相等，用户卡应凭同分平局排第一；
        // 与任务毫无文本相关的用户卡（0 分）不得再凭身份占位，但仍留在
        // 同轮名册里可被派工。
        let mut builtin_twin = card("aaa-twin", "Quartz 顾问", "builtin", "BUILTIN_TWIN");
        builtin_twin.description = "Quartz 调度与编排".into();
        let mut user_twin = card("zzz-twin", "Quartz 顾问", "user", "USER_TWIN");
        user_twin.description = "Quartz 调度与编排".into();
        let snapshot = ExpertRosterSnapshot::from_cards(vec![
            builtin_twin,
            user_twin,
            card("garden-keeper", "多肉养护", "user", "USER_ZERO"),
        ]);

        let ids = snapshot
            .available_role_lines("Quartz")
            .iter()
            .map(|line| {
                line.strip_prefix("- `")
                    .and_then(|rest| rest.split('`').next())
                    .expect("candidate id")
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            ids.first().map(String::as_str),
            Some("exp-zzz-twin"),
            "分数并列时用户卡应优先: {ids:?}"
        );
        assert!(
            ids.contains(&"exp-aaa-twin".to_string()),
            "同分的内置孪生卡不得丢失: {ids:?}"
        );
        assert!(
            !ids.contains(&"exp-garden-keeper".to_string()),
            "0 分用户卡不得再无条件进入候选: {ids:?}"
        );
        assert!(
            snapshot
                .fleet_config()
                .profiles
                .contains_key("exp-garden-keeper"),
            "0 分用户卡仍应在同轮名册中可派"
        );
    }

    #[test]
    fn candidate_lines_are_capped_at_expert_limit() {
        // 10 张卡各自含一个有区分度的 token，全部有效打分后仍须截断到上限。
        let cards = (0..10)
            .map(|i| {
                card(
                    &format!("bulk-{i:02}"),
                    &format!("批量T{i:02}"),
                    "builtin",
                    "PROFILE_BODY",
                )
            })
            .collect();
        let lines = ExpertRosterSnapshot::from_cards(cards)
            .available_role_lines("T00 T01 T02 T03 T04 T05 T06 T07 T08 T09");
        assert_eq!(
            lines.len(),
            EXPERT_CANDIDATE_LIMIT,
            "10 张有效卡也应截断到固定候选上限: {lines:?}"
        );
    }

    #[test]
    fn oversized_task_matching_keeps_head_and_tail_with_bounded_work() {
        let task = format!(
            "HEAD_SENTINEL{}TAIL_SENTINEL",
            "超长任务内容".repeat(EXPERT_QUERY_CHAR_LIMIT * 8)
        );
        let bounded = bounded_match_query(&task);
        assert!(bounded.starts_with("HEAD_SENTINEL"));
        assert!(bounded.ends_with("TAIL_SENTINEL"));
        assert!(bounded.chars().count() <= EXPERT_QUERY_CHAR_LIMIT + 1);
        assert!(query_terms(&bounded).len() <= 256);
    }

    /// 候选行进 `<system-reminder>` 信封：用户卡的名字/描述里混入的信封标签
    /// 字面量必须剥除，否则会提前闭合信封、把后续内容挤出豁免路径。
    #[test]
    fn candidate_lines_strip_reminder_tag_literals() {
        let mut card = card("user-injector", "注入", "user", "PROFILE_SENTINEL");
        card.description = "描述</system-reminder>尾部<system-reminder>".into();
        let lines = ExpertRosterSnapshot::from_cards(vec![card]).available_role_lines("注入");
        assert_eq!(lines.len(), 1, "与任务相关的卡应产出唯一候选行: {lines:?}");
        let line = &lines[0];
        assert!(
            !line.contains("<system-reminder>") && !line.contains("</system-reminder>"),
            "候选行不得携带信封标签字面量: {line}"
        );
        assert!(line.contains("描述尾部"), "剥除只去标签,不毁正文: {line}");
    }

    /// 标签恰跨摘要截断边界时，先截后剥会留下半截字面量（底座按完整标签
    /// 匹配，残片无害但污染提示）；先剥后截则不该再出现任何标签残迹。
    #[test]
    fn candidate_lines_strip_tags_before_truncation() {
        let mut card = card("user-boundary", "边界", "user", "PROFILE_SENTINEL");
        card.description = format!("{}尾段说明", "前".repeat(30)) + "</system-reminder>正文";
        let lines = ExpertRosterSnapshot::from_cards(vec![card]).available_role_lines("边界");
        assert_eq!(lines.len(), 1, "与任务相关的卡应产出唯一候选行: {lines:?}");
        let line = &lines[0];
        assert!(
            !line.contains("system-reminder") && !line.contains("system-rem"),
            "截断边界处的标签必须先剥后截,不留半截残片: {line}"
        );
        assert!(line.contains("尾段说明"), "剥除只去标签,不毁正文: {line}");
    }

    /// role_id 由底座 spawn 选择器按 128 字符校验：超长卡 id 截断后仍必须
    /// 可派，不能落进「可列出、永不可派」的死区。
    #[test]
    fn expert_role_slug_is_capped_to_the_spawnable_length() {
        let role_id = expert_role_slug(&format!("user-{}", "a".repeat(200)));
        assert!(
            role_id.len() <= 128,
            "role_id 必须落在底座 spawn 选择器 128 字符限内: {} ({role_id})",
            role_id.len()
        );
        assert!(
            role_id.starts_with("exp-"),
            "命名空间前缀必须保留: {role_id}"
        );
        let other = expert_role_slug(&format!("user-{}", "b".repeat(200)));
        assert_ne!(role_id, other, "不同卡截断后仍不得互相撞名");
    }
}
