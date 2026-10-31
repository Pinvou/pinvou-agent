//! 进程内交互审批面（tri-mode safety）的登记与决策缓存（按 session）。
//!
//! 底座 engine 在 `ask_on_guardrail` 武装（或 Auto 姿态）下对可提示工具发
//! `Event::ApprovalRequired`。转发器不再无条件自动批准，而是经
//! [`approval_surface_decision`] 定序处置：会话拒绝缓存 → Plan 防御硬拒 →
//! 会话授予缓存 → 三档 mode 状态。弹卡时在本模块登记，供 `get_pending_approvals`
//! 命令查询，让前端 remount 后恢复审批卡（同 `pending_user_input` 的还原思路）。
//!
//! 授予/拒绝缓存镜像 TUI `resolve_approval_request_disposition` 的作用域
//! 语义（crates/tui/src/core/authority.rs）：**拒绝按 exact approval_key**
//! （#1617，换一个参数就要重新请批），**授予按 lossy grouping_key**
//! （v0.8.37，「本次会话允许」覆盖后续旗标变体）。force_prompt 的 hold 永不
//! 被缓存放行——只有人当面点过的这张卡能过。全部驻内存：进程重启后 engine
//! lazy spawn，缓存随进程消亡，无需持久化。挂起卡在 turn 终态经
//! `clear_pending` 兜底清空（取消/中断时可能没有逐条决策）；授予/拒绝是
//! 会话级记忆，**跨 turn 存活**（与 TUI per-session 审批缓存一致：TUI 只在
//! 换会话/换工作区时清），宿主侧的对应清理点是会话删除（lib.rs 会话清理
//! 处理器里的 `clear_session`）。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::core::mode_state::SerializableMode;

/// 一条挂起的审批卡。字段与底座 `Event::ApprovalRequired` 一一对应。
/// serde camelCase 对齐前端 get_pending_* 的读取风格。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApproval {
    /// engine 审批 id（`Event::ApprovalRequired.id`），decide_approval 回传用。
    pub approval_id: String,
    pub tool_name: String,
    pub description: String,
    /// 工具参数，审批展示用。只透传不再加工。
    pub input: serde_json::Value,
    /// 模型对写类工具意图的解释（#2381），None = 只读工具无摘要。
    pub intent_summary: Option<String>,
    /// exact-args 指纹：denial 的作用域（#1617）。
    pub approval_key: String,
    /// lossy / arity-aware 指纹：session-grant 的作用域（v0.8.37）。
    pub approval_grouping_key: String,
    /// 底座 hold 强制弹卡（CW-7495）：即使有会话授予也必须当面请批。
    pub force_prompt: bool,
    pub created_at_ms: u64,
}

/// 毫秒级 UNIX 时间戳；时钟回拨只在极端情况下让排序抖动，不值得引入依赖。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Default)]
struct ApprovalStore {
    by_session: HashMap<String, Vec<PendingApproval>>,
    /// grouping_key → 授予它的 session 集合。集合化了「同 key 多会话各自授予」，
    /// `clear_session` 只摘除自己的份额，不误撤销他人授予。
    grants: HashMap<String, HashSet<String>>,
    /// approval_key → 拒绝它的 session 集合（与 grants 同构，作用域不同：
    /// 这里是 exact key，那里是 grouping key）。
    denials: HashMap<String, HashSet<String>>,
    /// 每次变更自增。未来用于「读完列表后注册表是否已变」的陈旧性探测，
    /// 现在只保证任何可观察变更都会推进它。
    generation: u64,
}

static PENDING: Mutex<Option<ApprovalStore>> = Mutex::new(None);

fn with_store<T>(f: impl FnOnce(&mut ApprovalStore) -> T) -> T {
    let mut guard = PENDING.lock().unwrap_or_else(|poison| poison.into_inner());
    f(guard.get_or_insert_with(ApprovalStore::default))
}

/// 转发器弹卡时登记。同 approval_id 去重（原位替换）：engine 重放同一审批
/// 时不得重复出卡，但字段（如 intent_summary）以最新一次为准。
pub fn record(session_id: &str, approval: PendingApproval) {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        let entries = store.by_session.entry(session_id.to_string()).or_default();
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| entry.approval_id == approval.approval_id)
        {
            *existing = approval;
            return;
        }
        entries.push(approval);
    });
}

/// `decide_approval` 收口：任何决策（allow / allow_for_session / deny）都先
/// 摘除登记，返回摘下的条目（决策需要它的 approval_key / grouping_key）。
pub fn remove(session_id: &str, approval_id: &str) -> Option<PendingApproval> {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        let entries = store.by_session.get_mut(session_id)?;
        let position = entries
            .iter()
            .position(|entry| entry.approval_id == approval_id)?;
        let removed = entries.remove(position);
        if entries.is_empty() {
            store.by_session.remove(session_id);
        }
        Some(removed)
    })
}

/// turn 终态兜底清空该 session 的挂起卡（取消/中断时可能没有逐条决策）。
/// 授予/拒绝是会话级记忆，**不随 turn 终态消亡**——「本会话允许」必须跨
/// turn 存活（TUI 语义：只在换会话/换工作区时清），否则按钮实为「本 turn
/// 允许」。会话级记忆的清理点是 [`clear_session`]（会话删除）。
pub fn clear_pending(session_id: &str) {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        store.by_session.remove(session_id);
    });
}

/// 会话删除/清理时摘除该 session 的全部份额：挂起卡、授予、拒绝。授予/
/// 拒绝按会话集合化存储，只摘自己的份额，不误撤销他人授予。
pub fn clear_session(session_id: &str) {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        store.by_session.remove(session_id);
        store.grants.retain(|_, sessions| {
            sessions.remove(session_id);
            !sessions.is_empty()
        });
        store.denials.retain(|_, sessions| {
            sessions.remove(session_id);
            !sessions.is_empty()
        });
    });
}

/// 前端 remount 恢复审批卡用；按 (session, created_at) 排序保证输出稳定。
pub fn list_all_sessions() -> Vec<(String, PendingApproval)> {
    let mut all = with_store(|store| {
        store
            .by_session
            .iter()
            .flat_map(|(session_id, entries)| {
                entries
                    .iter()
                    .map(|entry| (session_id.clone(), entry.clone()))
            })
            .collect::<Vec<_>>()
    });
    all.sort_by(|(left_session, left), (right_session, right)| {
        left_session
            .cmp(right_session)
            .then(left.created_at_ms.cmp(&right.created_at_ms))
            .then(left.approval_id.cmp(&right.approval_id))
    });
    all
}

pub fn list(session_id: &str) -> Vec<PendingApproval> {
    with_store(|store| {
        store
            .by_session
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    })
}

/// 该会话此前是否已授予这个 lossy 操作（grouping_key 命中即放行）。读路径
/// 必须带 session 维度：授予是**会话级**记忆，A 会话的授予不得放行 B 会话
/// 的同族调用（跨会话放行 = 一次点击授权整个进程，违背「本会话允许」语义）。
pub fn session_granted(session_id: &str, grouping_key: &str) -> bool {
    with_store(|store| {
        store
            .grants
            .get(grouping_key)
            .is_some_and(|sessions| sessions.contains(session_id))
    })
}

/// 「本次会话允许」：授予记在 grouping_key 上（覆盖后续旗标变体，v0.8.37）。
pub fn remember_grant(session_id: &str, grouping_key: &str) {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        store
            .grants
            .entry(grouping_key.to_string())
            .or_default()
            .insert(session_id.to_string());
    });
}

/// 该会话此前是否已拒绝这个确切操作（exact approval_key 命中即拒）。
/// 同 [`session_granted`]：拒绝也是会话级记忆，不跨会话连坐。
pub fn session_denied(session_id: &str, approval_key: &str) -> bool {
    with_store(|store| {
        store
            .denials
            .get(approval_key)
            .is_some_and(|sessions| sessions.contains(session_id))
    })
}

/// 「拒绝」记在 exact key 上（#1617：换参数要重新请批，不得连坐）。
pub fn remember_denial(session_id: &str, approval_key: &str) {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        store
            .denials
            .entry(approval_key.to_string())
            .or_default()
            .insert(session_id.to_string());
    });
}

// ===================== 纯决策函数（tri-mode 审批面定序） =====================

/// 硬拒原因。`reason_key` 是 `chat:guardrail_denied` 事件里的稳定英文键，
/// 前端/i18n 据此映射文案；人读的措辞不进协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDenyReason {
    /// 会话此前对该确切操作（exact approval_key）选择了拒绝。
    SessionDenied,
    /// Plan 姿态下底座应自行拦截工具执行；收到审批事件属异常，防御性硬拒。
    PlanMode,
    /// Yolo 全自动 + ask-on-guardrail 关闭：守卫命中按姿态硬拒。
    GuardrailAskOff,
}

impl ApprovalDenyReason {
    pub fn reason_key(self) -> &'static str {
        match self {
            Self::SessionDenied => "session_denied",
            Self::PlanMode => "plan_mode",
            Self::GuardrailAskOff => "guardrail_ask_off",
        }
    }
}

/// 审批面的处置结果。定时任务无人值守路径不走这里（在转发器里先行短路）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalSurfaceDecision {
    /// 静默批准（此前授予命中）：不发事件——ToolCallStarted 已展示调用。
    ApproveSilently,
    /// 静默硬拒 + `chat:guardrail_denied` 通知，不出卡。
    Deny(ApprovalDenyReason),
    /// 登记挂起 + 弹 `chat:approval_required` 卡。
    ShowCard,
}

/// 分支顺序以 TUI `resolve_approval_request_disposition`（authority.rs）为
/// 基础，Plan 防御臂有意更严：**会话拒绝 > Plan 硬拒 > 会话授予 >
/// force_prompt 弹卡 > 姿态判定**。定时任务无人值守由调用方先行短路，不进
/// 本函数。无人值守以外的定时/多智能体会话与普通对话同序。
///
/// - 拒绝缓存命中：无论什么姿态都硬拒（TUI legacy 顺序：session denial 最优先）。
/// - Plan：底座 Plan 本应挡下工具执行，审批事件到达即异常，防御性硬拒——
///   **先于授予缓存**。TUI legacy 顺序授予在前，但 TUI 的 Plan 流从不产生
///   审批事件；宿主侧 turn 中途可能切到 Plan（防御语义），此刻旧授予若仍
///   静默放行，「Plan=只读」就被缓存击穿。硬拒方向 fail-safe 且零成本。
/// - 授予缓存命中且非 force_prompt：静默批准（force_prompt 的 hold 只有
///   当面点的这张卡能过，缓存不得放行）。
/// - force_prompt：永远弹卡（武装宿主保证有人应答）。注：无人武装宿主的
///   Yolo+toggle 关下，底座各 face 在引擎门内即拒、不发 force 事件（见
///   #131 布线），故此臂实际只在武装宿主可达，与底座
///   `AutoDenyFullAccessPolicyHold` 不构成可达分歧。
/// - Yolo + toggle 关：守卫命中按姿态硬拒；Yolo + toggle 开、Auto：弹卡
///   （Auto 姿态下每个普通可提示工具都逐次请批，toggle 只管守卫命中）。
#[must_use]
pub fn approval_surface_decision(
    session_mode: SerializableMode,
    ask_on_guardrail: bool,
    force_prompt: bool,
    session_granted: bool,
    session_denied: bool,
) -> ApprovalSurfaceDecision {
    if session_denied {
        return ApprovalSurfaceDecision::Deny(ApprovalDenyReason::SessionDenied);
    }
    if session_mode == SerializableMode::Plan {
        return ApprovalSurfaceDecision::Deny(ApprovalDenyReason::PlanMode);
    }
    if session_granted && !force_prompt {
        return ApprovalSurfaceDecision::ApproveSilently;
    }
    if force_prompt {
        return ApprovalSurfaceDecision::ShowCard;
    }
    match session_mode {
        SerializableMode::Yolo if !ask_on_guardrail => {
            ApprovalSurfaceDecision::Deny(ApprovalDenyReason::GuardrailAskOff)
        }
        _ => ApprovalSurfaceDecision::ShowCard,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 单元测试共用的 session id 前缀，clear_session 先行清理保证隔离。
    fn isolate(session: &str) -> String {
        clear_session(session);
        session.to_string()
    }

    fn sample(approval_id: &str, approval_key: &str, grouping_key: &str) -> PendingApproval {
        PendingApproval {
            approval_id: approval_id.to_string(),
            tool_name: "write_file".to_string(),
            description: "write a file".to_string(),
            input: serde_json::json!({ "path": "a.txt" }),
            intent_summary: Some("save notes".to_string()),
            approval_key: approval_key.to_string(),
            approval_grouping_key: grouping_key.to_string(),
            force_prompt: false,
            created_at_ms: now_ms(),
        }
    }

    #[test]
    fn forkguard_approval_surface_record_dedupes_by_approval_id() {
        let session = isolate("approval-dedupe-test");
        record(&session, sample("ap-1", "k-1", "g-1"));
        record(&session, sample("ap-1", "k-1", "g-1"));
        record(&session, sample("ap-2", "k-2", "g-2"));
        let listed = list(&session);
        assert_eq!(listed.len(), 2, "同 approval_id 登记去重");
        assert_eq!(listed[0].approval_id, "ap-1", "原位替换不打乱顺序");

        // 原位替换：重复登记覆盖字段。
        let mut replaced = sample("ap-1", "k-1", "g-1");
        replaced.intent_summary = None;
        record(&session, replaced);
        assert!(list(&session)[0].intent_summary.is_none());
    }

    #[test]
    fn forkguard_approval_surface_remove_returns_entry_and_cleans_empty_session() {
        let session = isolate("approval-remove-test");
        record(&session, sample("ap-1", "k-1", "g-1"));
        let removed = remove(&session, "ap-1");
        assert_eq!(
            removed.as_ref().map(|entry| entry.approval_id.as_str()),
            Some("ap-1")
        );
        assert!(remove(&session, "ap-1").is_none(), "重复移除返回 None");
        assert!(list(&session).is_empty());
        assert!(
            !list_all_sessions().iter().any(|(sid, _)| sid == &session),
            "空 session 条目从全局索引摘除"
        );
    }

    #[test]
    fn forkguard_approval_surface_grant_and_denial_scoping() {
        let session = isolate("approval-scope-test");
        let other = isolate("approval-scope-other");
        // 授予按 grouping_key：同组变体放行，异组不连坐。
        assert!(!session_granted(&session, "g-write"));
        remember_grant(&session, "g-write");
        assert!(session_granted(&session, "g-write"));
        assert!(!session_granted(&session, "g-rm"), "未授予的 grouping key 不放行");

        // 会话维度：本会话的授予/拒绝不得跨会话生效（「本会话允许」≠ 进程级放行）。
        assert!(
            !session_granted(&other, "g-write"),
            "A 会话的授予不得放行 B 会话"
        );
        assert!(
            !session_denied(&other, "k-rm-rf"),
            "A 会话的拒绝不得连坐 B 会话"
        );

        // 拒绝按 exact approval_key：精确参数硬拒，其它参数重新请批。
        assert!(!session_denied(&session, "k-rm-rf"));
        remember_denial(&session, "k-rm-rf");
        assert!(session_denied(&session, "k-rm-rf"));
        assert!(!session_denied(&session, "k-rm"), "exact key 之外的调用不受拒绝连坐");

        // 授予与拒绝互不串作用域。
        assert!(!session_denied(&session, "g-write"));
        assert!(!session_granted(&session, "k-rm-rf"));
    }

    #[test]
    fn forkguard_approval_surface_clear_pending_keeps_session_memory() {
        let session = isolate("approval-clear-pending-test");
        record(&session, sample("ap-1", "k-1", "g-1"));
        remember_grant(&session, "g-1");
        remember_denial(&session, "k-1");

        // turn 终态只清挂起卡；授予/拒绝是会话级记忆，跨 turn 存活。
        clear_pending(&session);
        assert!(list(&session).is_empty(), "clear_pending 清挂起卡");
        assert!(session_granted(&session, "g-1"), "授予跨 turn 存活");
        assert!(session_denied(&session, "k-1"), "拒绝跨 turn 存活");

        // 会话删除才清会话级记忆。
        clear_session(&session);
        assert!(!session_granted(&session, "g-1"), "会话删除清授予");
        assert!(!session_denied(&session, "k-1"), "会话删除清拒绝");
    }

    #[test]
    fn forkguard_approval_surface_clear_session_wipes_memory() {
        let session = isolate("approval-clear-test");
        let other = isolate("approval-clear-other");
        record(&session, sample("ap-1", "k-1", "g-1"));
        remember_grant(&session, "g-1");
        remember_grant(&other, "g-1");
        remember_denial(&session, "k-1");

        clear_session(&session);
        assert!(list(&session).is_empty(), "clear_session 清挂起");
        assert!(!session_denied(&session, "k-1"), "clear_session 清拒绝缓存");
        // g-1 同时被 other 会话授予：只摘除自己的份额，不误撤销他人授予。
        assert!(session_granted(&other, "g-1"), "他人授予不受本会话清理影响");
        assert!(!session_granted(&session, "g-1"), "本会话授予被摘除");
        clear_session(&other);
        assert!(!session_granted(&other, "g-1"), "最后一个授予会话清理后撤销");
    }

    /// 处置真值表（定时任务无人值守路径不在此函数，另行短路）。
    /// 列：mode | ask | force | granted | denied → 处置。
    #[test]
    fn forkguard_approval_surface_decision_truth_table() {
        use ApprovalDenyReason::*;
        use ApprovalSurfaceDecision::*;
        let table: &[(
            SerializableMode,
            bool,
            bool,
            bool,
            bool,
            ApprovalSurfaceDecision,
        )] = &[
            // 拒绝缓存最优先，压过授予与一切姿态。
            (
                SerializableMode::Auto,
                true,
                false,
                true,
                true,
                Deny(SessionDenied),
            ),
            (
                SerializableMode::Yolo,
                true,
                true,
                false,
                true,
                Deny(SessionDenied),
            ),
            // 授予 + 非 force：静默批准（Plan 例外见下两行）。
            (
                SerializableMode::Auto,
                true,
                false,
                true,
                false,
                ApproveSilently,
            ),
            (
                SerializableMode::Yolo,
                false,
                false,
                true,
                false,
                ApproveSilently,
            ),
            // Plan 防御臂先于授予缓存（有意比 TUI legacy 顺序更严）：
            // turn 中途切 Plan 后，旧授予不得静默放行。
            (
                SerializableMode::Plan,
                true,
                false,
                true,
                false,
                Deny(PlanMode),
            ),
            (SerializableMode::Plan, false, false, true, false, Deny(PlanMode)),
            // force_prompt 的 hold 不吃缓存。
            (SerializableMode::Auto, true, true, true, false, ShowCard),
            // Plan 防御性硬拒（底座本应拦截工具执行），force_prompt 也不例外。
            (
                SerializableMode::Plan,
                true,
                false,
                false,
                false,
                Deny(PlanMode),
            ),
            (
                SerializableMode::Plan,
                false,
                true,
                false,
                false,
                Deny(PlanMode),
            ),
            // force_prompt 永远弹卡，Yolo toggle 关也不硬拒。
            (SerializableMode::Yolo, false, true, false, false, ShowCard),
            // Yolo + toggle 关 + 非 force：守卫命中按姿态硬拒。
            (
                SerializableMode::Yolo,
                false,
                false,
                false,
                false,
                Deny(GuardrailAskOff),
            ),
            // Yolo + toggle 开：弹卡。
            (SerializableMode::Yolo, true, false, false, false, ShowCard),
            // Auto：普通可提示工具逐次请批，toggle 只管守卫命中，一律弹卡。
            (SerializableMode::Auto, true, false, false, false, ShowCard),
            (SerializableMode::Auto, false, false, false, false, ShowCard),
            (SerializableMode::Auto, false, true, false, false, ShowCard),
        ];
        for (mode, ask, force, granted, denied, expected) in table {
            assert_eq!(
                approval_surface_decision(*mode, *ask, *force, *granted, *denied),
                *expected,
                "mode={mode:?} ask_on_guardrail={ask} force_prompt={force} granted={granted} denied={denied}"
            );
        }
    }
}
