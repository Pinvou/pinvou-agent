//! 记忆整理优化（organize）：全量扫描六类记忆存储 → LLM 产出 delete / update /
//! merge 动作 → 逐条清洗校验后应用，并把每次整理报告写入
//! `organize_history.json`（有界数组，保留最近 20 条）。
//!
//! 与 `llm_review` 的按轮复盘不同：整理是用户主动触发的全量 pass，目标是合并
//! 重复、改写过时表述、删除低价值条目，而不记录任何新信息；profile（用户身份
//! 字段）不在整理范围。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;
use std::time::Duration as StdDuration;

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use reqwest::Client;
use serde::Serialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::platform::prefs::ModelPreset;

use super::io;
use super::llm_review::{
    append_memory_review_diagnostic, apply_memory_review_reasoning_controls,
    memory_output_language_directive,
};
use super::types::{
    MemoryProfile, MemoryReviewModel, MemoryTextPatch, NeverMemoryItem, PendingMemoryItem,
    PreferenceFile, TimedMemoryItem, WorkContextFile, normalize_preference_topic,
    normalize_timed_memory_topic, normalize_work_context_topic,
};
use super::util::{
    clean_candidate_sentence, clean_id, clean_text, looks_recent_work_status, looks_sensitive,
    looks_sensitive_or_task_like, looks_task_like, write_json_atomic,
};

const LLM_ORGANIZE_TIMEOUT: StdDuration = StdDuration::from_secs(75);
const ORGANIZE_HISTORY_MAX_REPORTS: usize = 20;

/// 进程级单飞守卫：手动按钮与定时任务可能并发触发两次整理，io 层虽然并发
/// 安全，但两个 pass 会基于各自（最长 75 秒前的）快照交叉应用破坏性动作，
/// 这里直接拒绝后到者。pub(super) 供并发拒绝测试预占锁。
pub(super) static ORGANIZE_IN_FLIGHT: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

pub(super) const MEMORY_ORGANIZE_PROMPT: &str = r#"你是 pinvou 的后台记忆整理器。你只做一件事：对照已有的全部记忆存储，输出整理优化动作（合并重复、改写过时表述、删除低价值条目）。不要回答用户问题，不要解释你的判断，不要记录任何新信息。

你必须只输出 JSON，不要解释。格式：
{
  "actions": [
    {
      "op": "delete | update | merge",
      "kind": "preference | work_context | current_focus | recent_activity | pending",
      "ids": ["待操作条目的 id，必须来自输入"],
      "topic": "可选：整理后的 topic",
      "content": "update / merge 必填：整理后的完整记忆内容；delete 可省略",
      "reason": "一句话说明"
    }
  ]
}

你会收到一个 JSON 对象：
- profile：用户资料，仅供了解上下文，不允许输出针对它的动作。
- preferences：已生效的长期偏好。
- work_context：已生效的用户工作背景。
- current_focus：当前关注（含已过期归档）。
- recent_activity：近期动态（含已过期归档）。
- pending：待用户确认的候选记忆。
- never_memory：用户不希望再提示的记忆，仅供了解边界。

判断原则：
1. 目标是整理优化：合并重复与同主题条目（merge）；改写过时、含糊或命令口吻的表述为简洁的第三人称事实陈述（update）；删除过时、被覆盖、互相矛盾（保留较新信息）、低价值或与用户无关的条目（delete）。
2. 不要为了整理而整理：内容仍然准确且简洁时保持原样（skip = 不输出该条目的动作）。
3. content 必须是清洗后的事实摘要，不要照抄整句，不要带“请记住/以后你要”等命令口吻；不包含密码、手机号、证件号、token、API key、详细地址等敏感信息。
4. pending 只允许 delete（删除重复、过期或已被正式记忆覆盖的候选），不允许把 pending 升级为正式记忆，也不允许对它 update 或 merge。
5. 禁止输出 kind=profile 的动作：用户身份字段不在整理范围。
6. current_focus / recent_activity 的更新不修改 ttl_days，保持原有过期设置。
7. ids 必须引用输入中存在的 id；merge 至少给 2 个 id（第一个是保留的主条目），update 恰好给 1 个 id。
8. 没有可整理的内容时输出 {"actions":[]}。
"#;

/// 一次记忆整理的报告。`scanned` 固定含六类存储的条目数；`deleted` /
/// `updated` / `merged` 按 kind 计数且互不重叠：`merged` 只记 merge 中被
/// 合并移除的源条目，`deleted` 只记 delete 动作移除的条目，三者求和即实际
/// 改动的条目数。
/// Deserialize 供 `organize_history.json` 的有界历史回读使用。
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct MemoryOrganizeReport {
    pub started_at: String,
    pub finished_at: String,
    pub model: String,
    pub scanned: BTreeMap<String, u32>,
    pub deleted: BTreeMap<String, u32>,
    pub updated: BTreeMap<String, u32>,
    pub merged: BTreeMap<String, u32>,
    pub skipped_sensitive: u32,
    pub no_change: bool,
    pub warnings: Vec<String>,
}

/// 整理入口：机械预清理 → 全量快照 → LLM 整理动作 → 清洗校验 → 应用 → 落历史。
/// `cancel` 由定时任务入口传入（手动按钮没有可取消的宿主，传 `None`）：入口
/// 与“LLM 返回后、动作应用前”各检查一次，保证已取消的运行不留下已应用的
/// 破坏性动作。
pub async fn organize_memory_with_llm(
    bridge: &(impl MemoryReviewModel + ?Sized),
    cancel: Option<&CancellationToken>,
) -> Result<MemoryOrganizeReport> {
    if !io::memory_enabled() {
        append_memory_review_diagnostic(
            "organize",
            "skipped",
            json!({ "reason": "memory_disabled" }),
        );
        return Err(anyhow!("memory disabled"));
    }
    if cancel.is_some_and(|cancel| cancel.is_cancelled()) {
        append_memory_review_diagnostic("organize", "skipped", json!({ "reason": "canceled" }));
        return Err(anyhow!("memory organize canceled"));
    }
    let guard = ORGANIZE_IN_FLIGHT.get_or_init(|| tokio::sync::Mutex::new(()));
    let Ok(_in_flight) = guard.try_lock() else {
        append_memory_review_diagnostic(
            "organize",
            "skipped",
            json!({ "reason": "already_in_progress" }),
        );
        return Err(anyhow!("memory organize already in progress"));
    };
    let started_at = Utc::now().to_rfc3339();
    append_memory_review_diagnostic(
        "organize",
        "triggered",
        json!({
            "provider": bridge.memory_provider(),
            "model": bridge.memory_model(),
        }),
    );
    // 机械预清理：过期归档。各 pub 入口各自短暂持有写锁，幂等可重入。
    io::refresh_recent_work_expiry().context("refresh recent work expiry")?;
    io::refresh_timed_memory_expiry().context("refresh timed memory expiry")?;
    let snapshot = OrganizeSnapshot::load().context("load memory stores for organize")?;
    let scanned = snapshot.scanned_counts();
    let mut warnings = Vec::new();

    if snapshot.stores_empty() {
        let mut report = MemoryOrganizeReport {
            finished_at: Utc::now().to_rfc3339(),
            model: bridge.memory_model(),
            no_change: true,
            scanned,
            deleted: BTreeMap::new(),
            updated: BTreeMap::new(),
            merged: BTreeMap::new(),
            skipped_sensitive: 0,
            warnings,
            started_at,
        };
        finish_organize_report(&mut report);
        return Ok(report);
    }

    let raw_actions = match request_llm_organize_actions(bridge, &snapshot).await {
        Ok(actions) => actions,
        Err(error) => {
            append_memory_review_diagnostic(
                "organize",
                "failed",
                json!({ "error": clean_text(&format!("{error:#}"), 500) }),
            );
            return Err(error);
        }
    };
    // 取消边界：LLM 返回后、任何 delete/update/merge 应用前。LLM 等待期的取消
    // 由 executor 侧 select 丢弃本 future 兜住；这里兜住 select 无法打断的
    // 同步段——保证“已取消”的 run 不会留下已应用的动作。
    if cancel.is_some_and(|cancel| cancel.is_cancelled()) {
        append_memory_review_diagnostic(
            "organize",
            "skipped",
            json!({ "reason": "canceled_before_apply" }),
        );
        return Err(anyhow!("memory organize canceled before apply"));
    }

    let mut skipped_sensitive = 0u32;
    let validated: Vec<OrganizeAction> = raw_actions
        .into_iter()
        .filter_map(|raw| {
            validate_organize_action(raw, &snapshot, &mut skipped_sensitive, &mut warnings)
        })
        .collect();
    let (deleted, updated, merged, mut apply_warnings) = apply_organize_actions(&validated);
    compact_timed_stores(&mut apply_warnings);
    warnings.append(&mut apply_warnings);

    let no_change = deleted.values().sum::<u32>()
        + updated.values().sum::<u32>()
        + merged.values().sum::<u32>()
        == 0;
    let mut report = MemoryOrganizeReport {
        finished_at: Utc::now().to_rfc3339(),
        model: bridge.memory_model(),
        no_change,
        scanned,
        deleted,
        updated,
        merged,
        skipped_sensitive,
        warnings,
        started_at,
    };
    finish_organize_report(&mut report);
    Ok(report)
}

/// 报告收尾：先落历史（失败转 warning），再写完成诊断。
fn finish_organize_report(report: &mut MemoryOrganizeReport) {
    if let Err(error) = persist_organize_report(report) {
        let detail = format!("persist organize history: {error}");
        eprintln!("[memory] {detail}");
        report.warnings.push(detail);
    }
    append_memory_review_diagnostic(
        "organize",
        "completed",
        json!({
            "no_change": report.no_change,
            "deleted_total": report.deleted.values().sum::<u32>(),
            "updated_total": report.updated.values().sum::<u32>(),
            "merged_total": report.merged.values().sum::<u32>(),
            "skipped_sensitive": report.skipped_sensitive,
            "warning_count": report.warnings.len(),
        }),
    );
}

/// 最近的整理报告，新的在前；文件缺失或损坏时返回空。
pub fn load_organize_history() -> Vec<MemoryOrganizeReport> {
    let Ok(raw) = std::fs::read_to_string(io::organize_history_path()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<MemoryOrganizeReport>>(&raw).unwrap_or_default()
}

fn persist_organize_report(report: &MemoryOrganizeReport) -> std::io::Result<()> {
    let _guard = io::write_lock().lock();
    let mut history = load_organize_history();
    history.insert(0, report.clone());
    history.truncate(ORGANIZE_HISTORY_MAX_REPORTS);
    write_json_atomic(&io::organize_history_path(), &history)
}

/// 六类存储的一次性快照：LLM 输入、id 存在性校验与 scanned 计数都基于它。
pub(super) struct OrganizeSnapshot {
    profile: MemoryProfile,
    preferences: Vec<PreferenceFile>,
    work_context: Vec<WorkContextFile>,
    current_focus: Vec<TimedMemoryItem>,
    recent_activity: Vec<TimedMemoryItem>,
    pending: Vec<PendingMemoryItem>,
    never: Vec<NeverMemoryItem>,
}

impl OrganizeSnapshot {
    pub(super) fn load() -> std::io::Result<Self> {
        Ok(Self {
            profile: io::load_profile()?,
            preferences: io::load_preferences()?,
            work_context: io::load_work_context()?,
            current_focus: io::load_current_focus()?,
            recent_activity: io::load_recent_activity()?,
            // 只装载未决议候选：ignored/confirmed 已有用户决定，无整理价值，
            // 也不应随整理请求再次外发给模型（与按轮复盘的 pending 口径一致）。
            pending: io::load_pending_memory()?
                .into_iter()
                .filter(|item| item.status == super::types::PENDING_STATUS_PENDING)
                .collect(),
            never: io::load_never_memory()?,
        })
    }

    /// profile 之外的五类可整理存储是否全为空（全空则无需调 LLM）。
    fn stores_empty(&self) -> bool {
        self.preferences.is_empty()
            && self.work_context.is_empty()
            && self.current_focus.is_empty()
            && self.recent_activity.is_empty()
            && self.pending.is_empty()
    }

    /// profile 是否有任何内容（scanned["profile"] 只区分 0/1）。
    fn profile_has_content(&self) -> bool {
        let profile = &self.profile;
        !profile.identity.call_name.is_empty()
            || !profile.identity.assistant_alias.is_empty()
            || !profile.conventions.language.is_empty()
            || !profile.conventions.doc_standard.is_empty()
            || !profile.conventions.number_usage.is_empty()
            || !profile.conventions.style_notes.is_empty()
    }

    fn scanned_counts(&self) -> BTreeMap<String, u32> {
        let mut scanned = BTreeMap::new();
        scanned.insert("profile", u32::from(self.profile_has_content()));
        scanned.insert("preference", self.preferences.len() as u32);
        scanned.insert("work_context", self.work_context.len() as u32);
        scanned.insert("current_focus", self.current_focus.len() as u32);
        scanned.insert("recent_activity", self.recent_activity.len() as u32);
        scanned.insert("pending", self.pending.len() as u32);
        scanned
            .into_iter()
            .map(|(kind, count)| (kind.to_string(), count))
            .collect()
    }

    fn ids_for(&self, kind: &str) -> BTreeSet<String> {
        match kind {
            "preference" => self
                .preferences
                .iter()
                .map(|item| item.id.clone())
                .collect(),
            "work_context" => self
                .work_context
                .iter()
                .map(|item| item.id.clone())
                .collect(),
            "current_focus" => self
                .current_focus
                .iter()
                .map(|item| item.id.clone())
                .collect(),
            "recent_activity" => self
                .recent_activity
                .iter()
                .map(|item| item.id.clone())
                .collect(),
            "pending" => self.pending.iter().map(|item| item.id.clone()).collect(),
            _ => BTreeSet::new(),
        }
    }

    fn user_content(&self) -> String {
        json!({
            "profile": &self.profile,
            "preferences": &self.preferences,
            "work_context": &self.work_context,
            "current_focus": &self.current_focus,
            "recent_activity": &self.recent_activity,
            "pending": &self.pending,
            "never_memory": &self.never,
        })
        .to_string()
    }
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct LlmOrganizeActions {
    #[serde(default)]
    actions: Vec<LlmOrganizeAction>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct LlmOrganizeAction {
    #[serde(default)]
    pub(super) op: String,
    #[serde(default)]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) ids: Vec<String>,
    #[serde(default)]
    pub(super) topic: String,
    #[serde(default)]
    pub(super) content: String,
    #[serde(default)]
    pub(super) reason: String,
}

/// 校验后待执行的动作：ids 已确认存在于快照，content 已清洗并通过质量过滤。
#[derive(Debug, Clone)]
pub(super) struct OrganizeAction {
    op: String,
    kind: String,
    ids: Vec<String>,
    topic: Option<String>,
    content: String,
}

/// 校验单条 LLM 整理动作：kind 白名单、id 存在性、op-arity、content 清洗与
/// 分 kind 质量过滤。被丢弃时记 warning（敏感内容计 `skipped_sensitive`）。
pub(super) fn validate_organize_action(
    raw: LlmOrganizeAction,
    snapshot: &OrganizeSnapshot,
    skipped_sensitive: &mut u32,
    warnings: &mut Vec<String>,
) -> Option<OrganizeAction> {
    let mut drop_action = |reason: String| {
        warnings.push(reason);
        None
    };
    let op = clean_text(&raw.op, 16);
    if !matches!(op.as_str(), "delete" | "update" | "merge") {
        return drop_action(format!("organize: drop unknown op {:?}", op));
    }
    let kind = clean_text(&raw.kind, 24);
    // profile 与未知 kind 都不在整理范围（profile 动作按提示词约定直接丢弃）。
    if !matches!(
        kind.as_str(),
        "preference" | "work_context" | "current_focus" | "recent_activity" | "pending"
    ) {
        return drop_action(format!("organize: drop out-of-scope kind {kind:?}"));
    }
    if op != "delete" && kind == "pending" {
        return drop_action("organize: drop pending update/merge (delete only)".to_string());
    }
    let known = snapshot.ids_for(&kind);
    let mut ids = Vec::new();
    for id in raw.ids {
        let id = clean_id(&id);
        if id.is_empty() || ids.contains(&id) {
            continue;
        }
        if known.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return drop_action(format!("organize: drop {op} {kind} without known ids"));
    }
    if op == "merge" && ids.len() < 2 {
        return drop_action(format!("organize: drop merge {kind} with fewer than 2 ids"));
    }
    if op == "update" && ids.len() != 1 {
        return drop_action(format!("organize: drop update {kind} without exactly 1 id"));
    }
    let mut content = clean_text(&raw.content, 220);
    if op != "delete" {
        if content.is_empty() {
            return drop_action(format!("organize: drop {op} {kind} with empty content"));
        }
        if looks_sensitive(&content) {
            *skipped_sensitive += 1;
            return None;
        }
        content = clean_candidate_sentence(&content, 180);
        // 与 sanitize_llm_memory_item 同款的分 kind 质量过滤。
        match kind.as_str() {
            "preference" => {
                if looks_sensitive_or_task_like(&content) || content.chars().count() < 6 {
                    return drop_action(
                        "organize: drop preference content that is task-like or too short"
                            .to_string(),
                    );
                }
            }
            "work_context" => {
                if content.chars().count() < 8 {
                    return drop_action(
                        "organize: drop work_context content that is too short".to_string(),
                    );
                }
            }
            _ => {
                // current_focus / recent_activity：一次性任务口吻且不是进展/交付
                // 状态的表述不适合作为记忆内容。
                if looks_task_like(&content) && !looks_recent_work_status(&content) {
                    return drop_action(
                        "organize: drop timed content that looks like a one-off task".to_string(),
                    );
                }
            }
        }
        if content.is_empty() {
            return drop_action(format!(
                "organize: drop {op} {kind} with empty cleaned content"
            ));
        }
    }
    let raw_topic = clean_text(&raw.topic, 40);
    let topic = (!raw_topic.is_empty()).then(|| match kind.as_str() {
        "preference" => normalize_preference_topic(&raw_topic),
        "work_context" => normalize_work_context_topic(&raw_topic),
        "current_focus" | "recent_activity" => normalize_timed_memory_topic(&kind, &raw_topic),
        // pending 仅 delete，topic 不会被使用。
        _ => raw_topic,
    });
    let _reason = clean_text(&raw.reason, 120);
    Some(OrganizeAction {
        op,
        kind,
        ids,
        topic,
        content,
    })
}

/// 逐条应用已校验动作。每条动作走 io 既有加锁入口（各自持有写锁），单条失败
/// 记 warning 继续，不中断整批。
fn apply_organize_actions(
    actions: &[OrganizeAction],
) -> (
    BTreeMap<String, u32>,
    BTreeMap<String, u32>,
    BTreeMap<String, u32>,
    Vec<String>,
) {
    let mut deleted = BTreeMap::new();
    let mut updated = BTreeMap::new();
    let mut merged = BTreeMap::new();
    let mut warnings = Vec::new();
    for action in actions {
        match action.op.as_str() {
            "delete" => {
                for id in &action.ids {
                    match delete_organize_item(&action.kind, id) {
                        Ok(true) => *deleted.entry(action.kind.clone()).or_default() += 1,
                        Ok(false) => {
                            warnings.push(format!(
                                "organize: delete {} {id} did not match any item",
                                action.kind
                            ));
                        }
                        Err(error) => {
                            warnings
                                .push(format!("organize: delete {} {id}: {error}", action.kind));
                        }
                    }
                }
            }
            "update" => {
                let id = &action.ids[0];
                match update_organize_item(&action.kind, id, action.topic.clone(), &action.content)
                {
                    Ok(true) => *updated.entry(action.kind.clone()).or_default() += 1,
                    Ok(false) => {
                        warnings.push(format!(
                            "organize: update {} {id} did not match any item",
                            action.kind
                        ));
                    }
                    Err(error) => {
                        warnings.push(format!("organize: update {} {id}: {error}", action.kind));
                    }
                }
            }
            "merge" => {
                // 保留第一个 id 作为主条目：更新为合并内容，其余删除。
                let (keep, rest) = match action.ids.split_first() {
                    Some(split) => split,
                    None => continue,
                };
                let mut keep_updated = false;
                match update_organize_item(
                    &action.kind,
                    keep,
                    action.topic.clone(),
                    &action.content,
                ) {
                    Ok(true) => {
                        keep_updated = true;
                        *updated.entry(action.kind.clone()).or_default() += 1;
                    }
                    Ok(false) => {
                        warnings.push(format!(
                            "organize: merge {} {keep} did not match any item; merge skipped",
                            action.kind
                        ));
                    }
                    Err(error) => {
                        warnings.push(format!(
                            "organize: merge {} {keep}: {error}; merge skipped",
                            action.kind
                        ));
                    }
                }
                if !keep_updated {
                    // 合并内容尚未落库，此时删除其余源条目会造成不可逆的
                    // 信息丢失；整条动作按失败处理，源条目原样保留。
                    continue;
                }
                for id in rest {
                    match delete_organize_item(&action.kind, id) {
                        // 被合并移除的条目只计 merged，不与 deleted 重复计数：
                        // 三个口径互不重叠，求和即实际改动的条目数。
                        Ok(true) => *merged.entry(action.kind.clone()).or_default() += 1,
                        Ok(false) => {
                            warnings.push(format!(
                                "organize: merge {} {id} did not match any item",
                                action.kind
                            ));
                        }
                        Err(error) => {
                            warnings
                                .push(format!("organize: delete {} {id}: {error}", action.kind));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (deleted, updated, merged, warnings)
}

fn delete_organize_item(kind: &str, id: &str) -> std::io::Result<bool> {
    match kind {
        "preference" => io::delete_preference(id),
        "work_context" => io::delete_work_context(id),
        // pending 删除等价于用户忽略：置 ignored 而非物理移除，保留审计痕迹。
        "pending" => io::ignore_pending_memory(id).map(|event| event.is_some()),
        _ => io::delete_timed_memory(kind, id),
    }
}

fn update_organize_item(
    kind: &str,
    id: &str,
    topic: Option<String>,
    content: &str,
) -> Result<bool> {
    // ttl 不在整理范围内：current_focus / recent_activity 保持原 ttl。
    let patch = MemoryTextPatch {
        topic,
        text: Some(content.to_string()),
        ttl_days: None,
    };
    match kind {
        "preference" => io::update_preference(id, patch)
            .map(|mutation| mutation.is_some())
            .context("update preference"),
        "work_context" => io::update_work_context(id, patch)
            .map(|mutation| mutation.is_some())
            .context("update work context"),
        _ => io::update_timed_memory(kind, id, patch)
            .map(|item| item.is_some())
            .context("update timed memory"),
    }
}

/// 应用完成后对两类 timed store 再做一次规范化 / 去重 / 容量压实，清掉
/// update/merge 留下的重复或超限条目。压实走 io 层加锁入口：load 与整文件
/// 回写必须同锁，否则快照间隙里按轮复盘刚写入的条目会被旧列表覆盖丢失。
/// 失败记 warning，不影响主流程。
fn compact_timed_stores(warnings: &mut Vec<String>) {
    for kind in ["current_focus", "recent_activity"] {
        if let Err(error) = io::compact_timed_memory_store(kind) {
            warnings.push(format!("organize: compact {kind}: {error}"));
        }
    }
}

async fn request_llm_organize_actions(
    bridge: &(impl MemoryReviewModel + ?Sized),
    snapshot: &OrganizeSnapshot,
) -> Result<Vec<LlmOrganizeAction>> {
    let client = Client::builder()
        .timeout(LLM_ORGANIZE_TIMEOUT)
        .build()
        .context("build memory organize client")?;
    let base_url = bridge.memory_base_url();
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let provider = bridge.memory_provider();
    let preset = bridge.memory_model_preset();
    let model_name = if provider == "vllm" {
        // 与按轮复盘同款：served-name 探测使用同源推理密钥。
        crate::features::monitor::probe_vllm_model_info(
            &base_url,
            Some(bridge.memory_api_key().as_str()),
        )
        .await
        .0
        .unwrap_or_else(|| bridge.memory_model())
    } else {
        bridge.memory_model()
    };
    let user_content = snapshot.user_content();
    let prompt = match memory_output_language_directive(&bridge.memory_locale_tag()) {
        Some(suffix) => format!("{MEMORY_ORGANIZE_PROMPT}{suffix}"),
        None => MEMORY_ORGANIZE_PROMPT.to_string(),
    };
    // Anthropic 官方端点走 Messages 协议直连（同 llm_review 的按轮复盘）。
    if preset == ModelPreset::Anthropic {
        let content = crate::core::model_endpoint::post_anthropic_messages(
            &client,
            &base_url,
            &bridge.memory_api_key(),
            &model_name,
            &prompt,
            &user_content,
            1500,
        )
        .await?;
        return parse_llm_organize_actions(&content);
    }
    let mut body = json!({
        "model": model_name,
        "messages": [
            { "role": "system", "content": prompt },
            { "role": "user", "content": user_content }
        ],
        "temperature": 0,
        "max_tokens": 1500,
        "stream": false,
        "response_format": { "type": "json_object" }
    });
    apply_memory_review_reasoning_controls(&mut body, preset, &provider, &base_url, &model_name);
    let resp = client
        .post(url)
        .bearer_auth(bridge.memory_api_key())
        .json(&body)
        .send()
        .await
        .context("post memory organize chat/completions")?
        .error_for_status()
        .context("memory organize chat/completions status")?;
    let value: Value = resp
        .json()
        .await
        .context("parse memory organize response json")?;
    let content = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    parse_llm_organize_actions(&content)
}

/// 宽松解析：直接 JSON 失败时回退提取首尾大括号之间的对象（同 parse_llm_memory_review）。
fn parse_llm_organize_actions(content: &str) -> Result<Vec<LlmOrganizeAction>> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str::<LlmOrganizeActions>(trimmed) {
        Ok(actions) => Ok(actions.actions),
        Err(first_err) => {
            let Some(json_text) = extract_json_object(trimmed) else {
                return Err(first_err).context("parse memory organize json");
            };
            serde_json::from_str::<LlmOrganizeActions>(json_text)
                .context("parse extracted memory organize json")
                .map(|actions| actions.actions)
        }
    }
}

fn extract_json_object(value: &str) -> Option<&str> {
    let start = value.find('{')?;
    let end = value.rfind('}')?;
    (start <= end).then(|| &value[start..=end])
}
