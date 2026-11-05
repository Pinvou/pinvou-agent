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
//! 处理器里的 `clear_session`）。目录重绑的绑定提交点（projects.rs 的
//! `clear_approval_memory_for_rebound_sessions`）对每个被移动会话推进
//! **工作区代**（[`advance_workspace_generation`]）并做保留代清理
//! （[`clear_session_preserving_generation`]，评审 JensenChen28 5764675531
//! 第一项，机制自 round-2/3 形状按本线重落）：请求/决策写入携带发起时刻的
//! 代，与已提交代失配即在锁内拒绝——「换工作区即清」之外再加一道版本边界，
//! 迟交的旧目录决策写不进新目录的记忆。

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
    /// **发起实例**的工作区代：转发器在其 engine 实例 spawn 时快照（实例的
    /// 身份属于它的 spawn 时刻），经 [`record`] 在锁内盖章（评审 5764675531，
    /// 参考 round-4 P2 sub-gap a 的来源语义）。决策写入
    /// （[`remember_grant`] / [`remember_denial`]）携带它做原子失效比较：
    /// 落笔时代 ≠ 当前已提交代（重绑的绑定提交点已推进）即整条丢弃——迟交
    /// 的旧范围决策不得给新代盖章。只派生 `Serialize`：登记纯驻内存、无磁盘
    /// 存量；决策失败臂的恢复（[`restore_if_generation_is`]）原样带回登记代，
    /// 恢复后的卡再次决策仍按同一纪元校验。
    pub request_workspace_generation: Option<u64>,
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
    /// 会话 → 已提交工作区代（重绑的绑定提交点推进）。缺项 = 未重绑时代
    /// （None）；[`remember_grant`] / [`remember_denial`] 在同一锁临界区内
    /// 把写入携带的代与它比对，失配（含 None vs Some(1)）即拒绝不写。
    workspace_generations: HashMap<String, u64>,
}

static PENDING: Mutex<Option<ApprovalStore>> = Mutex::new(None);

fn with_store<T>(f: impl FnOnce(&mut ApprovalStore) -> T) -> T {
    let mut guard = PENDING.lock().unwrap_or_else(|poison| poison.into_inner());
    f(guard.get_or_insert_with(ApprovalStore::default))
}

/// 转发器弹卡时登记。`instance_generation` 是**发起实例**的工作区代——
/// 转发器（每个 engine 化身一个任务）在该实例 spawn 时从
/// [`workspace_generation`] 快照，不得传「收卡时刻的当前代」（评审
/// 5764675531，参考 round-4 P2 sub-gap a）：重绑期间越过入口忙闸启动的
/// turn 仍由旧 cwd 运行时执行，旧实例跨绑定提交点的代推进后弹出的卡若按
/// 当前代盖章，就把旧目录的请求记到新代名下，用户批准即写入新代记忆。
/// 同 approval_id 去重（原位替换）：engine 重放同一审批时不得重复出卡，
/// 但字段（如 intent_summary）以最新一次为准——工作区代同样取最新一次的
/// 实例代（同一实例重放 = 同一代，仍是新请求语义；engine 审批 id 按实例
/// 铸造，重建实例不会重放旧 id）。
pub fn record(session_id: &str, mut approval: PendingApproval, instance_generation: Option<u64>) {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        // 请求创建即盖章发起实例的 spawn 代：决策写入以它做原子失效比较。
        approval.request_workspace_generation = instance_generation;
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

/// 会话当前已提交的工作区代（只读）：[`capture_workspace_identity`] 在
/// `AppEngine::spawn_for_session` 固化 `engine_config.workspace` 的同一
/// 同步段（roots 解析后的无 await 区间）内用它做前后两次代读取，捕获
/// 一致的实例身份代，随 spawn 传给转发器供 [`record`] 盖章——不得在
/// 转发器 async body 首次执行时现读（评审 5766477612/5766212059/
/// 5766526920：该执行可能已越过重绑绑定提交点的代推进），也不得在
/// spawn 侧绕过 [`capture_workspace_identity`] 单次现读（评审
/// 5768924092/5768948485 P2-a 第二轮：单次读取与 roots 解析无共同锁，
/// 跨线程夹进 rebind 提交即撕裂快照）。从未重绑的会话返回 None——与新
/// 实例的决策写入比较语义一致（None == None）。
pub fn workspace_generation(session_id: &str) -> Option<u64> {
    with_store(|store| store.workspace_generations.get(session_id).copied())
}

/// 推进会话的已提交工作区代，返回推进后的值。调用点是重绑的绑定提交点
/// （projects.rs 的 `clear_approval_memory_for_rebound_sessions`，紧邻保留
/// 代清理）：绑定提交即身份切换，旧目录纪元自此关闭——携带旧代的请求卡
/// 与决策写入在锁内失配被拒。缺失条目视作 0 起步（首次重绑 → 1）。
pub fn advance_workspace_generation(session_id: &str) -> u64 {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        let next = store
            .workspace_generations
            .entry(session_id.to_string())
            .or_insert(0);
        *next = next.wrapping_add(1);
        *next
    })
}

/// 单轮身份捕获失败后的重试上限。4 次全部撕裂 → `Err`（fail-closed：拒绝
/// 本次 spawn），不无界自旋。
const CAPTURE_IDENTITY_ATTEMPTS: usize = 4;

/// Spawn 侧工作区身份的**一致性捕获**（检测-重试协议，评审 5768924092 /
/// 5768948485 P2-a 第二轮）。[`workspace_generation`] 与
/// `SessionStore::session_roots` 分属两把锁，且两把锁都不跨调用持有：
/// 单次「读代 → 解析 roots」之间没有共同临界区，无 await 也挡不住其他
/// 线程——重绑提交点（projects.rs 的
/// `clear_approval_memory_for_rebound_sessions`）在另一个线程**先落绑定
/// 写入（换根）再推进代**（swap→advance 次序是该函数注释的既有契约），
/// 夹进读取窗口即得到撕裂快照：目录 runtime 与代章不同纪元，卡带 B 代
/// 通过决策比较，A 目录的授权/拒绝写进 B。
///
/// 协议：`gen_a = workspace_generation()` → `roots = resolve()` →
/// `gen_b = workspace_generation()`；`gen_a != gen_b` 判定读取窗口内发生
/// 了 rebind advance，整轮作废重试。`expected` 是 engine 侧**先前**解析、
/// 已喂给 `engine_config.workspace` 的同一份 roots；`roots != expected`
/// 同按撕裂窗口处理——swap→advance 次序下 advance 落地前的「新根+旧代」
/// 配对代比较检不出来，只能靠 roots 一致性核对拒收（两个方向都拒：
/// 「旧根+新代」由代比较拒，「新根+旧代」由 roots 比较拒）。协议假设即
/// 上述 projects.rs 既有契约；残差如实披露：advance 尚未落地时本轮按
/// roots 漂移重试，全部尝试耗尽 → `Err`，由调用点拒绝本次 spawn（此时
/// `engine_config.workspace` 仍是旧根，放行即缺陷本身；fail-closed 重复
/// 询问优于跨纪元写记忆，且既有行为本就如此——advance 前的决策写入同样
/// 被锁内失配拒绝）。
///
/// 返回 `(roots, gen_b)`：roots 仅用于协议比对（与 `expected` 相等才被
/// 接受），调用点的实际运行目录仍取其先前构造的 workspace；`gen_b` 即
/// 实例身份代，随 spawn 传给转发器盖章（[`record`]）。
pub fn capture_workspace_identity<T: PartialEq>(
    session_id: &str,
    expected: &T,
    resolve: impl Fn() -> T,
) -> Result<(T, Option<u64>), String> {
    for _ in 0..CAPTURE_IDENTITY_ATTEMPTS {
        let generation_before = workspace_generation(session_id);
        let roots = resolve();
        let generation_after = workspace_generation(session_id);
        if generation_before != generation_after {
            // 代在读取窗口内被推进：rebind 的绑定提交点落进两次读取之间，
            // 整轮快照撕裂，作废重试。
            continue;
        }
        if roots != *expected {
            // 代未推进但 roots 相对 engine 侧已解析值漂移：swap 已落地、
            // advance 尚未落地的「新根+旧代」撕裂配对，同按撕裂窗口作废
            // 重试。
            continue;
        }
        return Ok((roots, generation_after));
    }
    Err(format!(
        "workspace rebind is in flight for session {session_id}: no \
         consistent workspace identity could be captured across \
         {CAPTURE_IDENTITY_ATTEMPTS} attempts; refusing this spawn \
         (fail-closed) — resend the request once the rebind settles"
    ))
}

/// 未加栅格的摘除助手（round-8 起 `decide_approval` 走 [`remove_fenced`]，
/// 本函数已无生产调用方——与 [`clear_pending`] 同类，保留为测试/探测胶水；
/// 新调用方必须走栅格变体，否则失败臂的恢复协议会失去代数前提）。返回摘下
/// 的条目（决策需要它的 approval_key / grouping_key）。
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

/// Fenced removal for the decide-approval failure arm: like [`remove`], but
/// also returns the store generation captured atomically under the same lock
/// as the removal (the removal itself bumps it). The command layer re-reads
/// [`generation`] before restoring a removed entry, so any registry mutation
/// that raced the await — the turn-terminal sweep, a session purge, a
/// concurrent decision or a newly raised card — cancels the restore instead
/// of recreating an entry the registry lifecycle already closed (reviewer
/// P2: restore only within the same still-live approval lifecycle).
pub fn remove_fenced(session_id: &str, approval_id: &str) -> Option<(PendingApproval, u64)> {
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
        Some((removed, store.generation))
    })
}

/// Fenced restore for the decide-approval failure arm: re-inserts the entry
/// removed by [`remove_fenced`] only while the registry generation still
/// equals the snapshot captured atomically with that removal — the
/// comparison and the insertion share one registry critical section.
/// `decide_approval` previously read [`generation`] and then called
/// [`record`] as two separate lock acquisitions, so a terminal sweep could
/// run between the comparison and the insertion and the stale restore
/// resurrected an entry whose engine-side lifecycle had already closed
/// (reviewer P2). Returns whether the restore happened; a `false` return
/// costs a re-trigger of the action, a wrongly restored entry would
/// recreate a dead decision surface.
pub fn restore_if_generation_is(
    session_id: &str,
    approval: PendingApproval,
    expected_generation: u64,
) -> bool {
    with_store(|store| {
        if store.generation != expected_generation {
            return false;
        }
        store.generation = store.generation.wrapping_add(1);
        let entries = store.by_session.entry(session_id.to_string()).or_default();
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| entry.approval_id == approval.approval_id)
        {
            *existing = approval;
            return true;
        }
        entries.push(approval);
        true
    })
}

/// Current store generation (see [`ApprovalStore::generation`]). Read-side
/// helper; production fencing goes through [`remove_fenced`]'s atomic
/// snapshot — today's direct callers are tests/probes. A global
/// counter by design: the restore fence is deliberately conservative — any
/// observable registry change during the await window skips the restore,
/// and a skipped restore only costs a re-trigger of the action, while a
/// wrongly restored entry recreates a decision surface whose engine-side
/// approval no longer exists.
pub fn generation() -> u64 {
    with_store(|store| store.generation)
}

/// turn 终态兜底清空该 session 的挂起卡（取消/中断时可能没有逐条决策）。
/// 授予/拒绝是会话级记忆，**不随 turn 终态消亡**——「本会话允许」必须跨
/// turn 存活（TUI 语义：只在换会话/换工作区时清），否则按钮实为「本 turn
/// 允许」。会话级记忆的清理点是 [`clear_session`]（会话删除）。
///
/// 生产终态路径应改用 [`clear_pending_returning_child_ids`]：detached
/// worker 的审批不能在这里被静默丢弃（丢弃 = 悬挂 worker），本函数保留给
/// 测试与未来非终态清理点。
pub fn clear_pending(session_id: &str) {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        store.by_session.remove(session_id);
    });
}

/// 终态清空的分裂变体：摘除该 session 的全部挂起卡，并返回其中
/// **detached worker 审批**的 approval_id。底座子代理审批 id 形如
/// `agent:<agent_id>:approval:<boot_id>:<n>`（
/// `SubAgentManager::register_child_approval`），它是子代理 oneshot 的键：
/// 静默丢弃等于把还在 `WaitingForUser` 等待的 worker 永久悬挂（detached
/// worker 刻意比父 turn 活得久）。调用方必须对返回的 id 逐个走 engine
/// deny（`route_child_approval_decision` 路由到子代理），把「父 turn 已
/// 结束」变成 worker 可见的确定性拒绝；引擎已被回收时 oneshot 随 receiver
/// drop 落 `Unavailable`，同样是确定性终态。父会话自己的审批 id（引擎
/// per-turn 审批随 turn 消亡）直接丢弃即可。
pub fn clear_pending_returning_child_ids(session_id: &str) -> Vec<String> {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        let entries = store.by_session.remove(session_id).unwrap_or_default();
        entries
            .into_iter()
            .filter(|entry| {
                deepseek_tui::tools::subagent::SubAgentManager::is_child_approval_id(
                    &entry.approval_id,
                )
            })
            .map(|entry| entry.approval_id)
            .collect()
    })
}

/// 会话删除/清理时摘除该 session 的全部份额：挂起卡、授予、拒绝、工作区代
/// （完整生命周期重置：删除后重建的同 id 会话重新从「无代」起步）。授予/
/// 拒绝按会话集合化存储，只摘自己的份额，不误撤销他人授予。身份切换清理
/// （重绑绑定提交点）**不得**走本函数——清掉代会让旧代决策重新相等（评审
/// 5764675531），那条路径用 [`clear_session_preserving_generation`]。
pub fn clear_session(session_id: &str) {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        store.by_session.remove(session_id);
        store.workspace_generations.remove(session_id);
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

/// 身份切换清理（重绑绑定提交点的
/// `clear_approval_memory_for_rebound_sessions`）：回收挂起卡、授予、拒绝的
/// 存储份额，但**保留工作区代**。这是与 [`clear_session`] 的唯一差别，也是
/// 评审 5764675531 第一项的修复点：若把代一并清掉，会话回落「无代」，绑定
/// 提交点推进前盖 None 代的旧决策重新与新状态相等——旧授予复活、迟交的
/// 旧范围写入又被接受。保留代让旧代永远 miss（重新请批便宜，陈旧的静默
/// 放行不可接受）；新目录下的决策按存活代照常落笔命中。会话删除路径
/// （lib.rs）仍走 [`clear_session`] 的完整重置。
pub fn clear_session_preserving_generation(session_id: &str) {
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

/// 进程级全量快照（测试与未来陈旧性探测用，无生产调用方）：真实的
/// 前端 remount 恢复路径是 `get_pending_approvals` → [`list`]（按会话
/// 过滤）。按 (session, created_at) 排序保证输出稳定。
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
/// `origin_generation` 是决策来源的请求工作区代（decide_approval 以登记的
/// `pending.request_workspace_generation` 落笔）：与当前已提交代在同一把
/// store 锁内比对，失配（含 None vs Some(1)）即整条拒绝、不写任何记忆
/// （评审 5764675531 第一项：重绑绑定提交点推进代后，旧成功臂恢复执行的
/// 迟交写入不得给新代盖章；比对与插入同临界区，无检查-写入窗口）。返回
/// 是否落笔；`false` 的代价是重新请批一次，静默写错代不可接受。
pub fn remember_grant(
    session_id: &str,
    grouping_key: &str,
    origin_generation: Option<u64>,
) -> bool {
    with_store(|store| {
        if store.workspace_generations.get(session_id).copied() != origin_generation {
            return false;
        }
        store.generation = store.generation.wrapping_add(1);
        store
            .grants
            .entry(grouping_key.to_string())
            .or_default()
            .insert(session_id.to_string());
        true
    })
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
/// 代校验与 [`remember_grant`] 同款：同一锁临界区内比对来源代，失配即拒绝
/// 不写（迟交的旧目录拒绝不得跨目录连坐新目录）。
pub fn remember_denial(
    session_id: &str,
    approval_key: &str,
    origin_generation: Option<u64>,
) -> bool {
    with_store(|store| {
        if store.workspace_generations.get(session_id).copied() != origin_generation {
            return false;
        }
        store.generation = store.generation.wrapping_add(1);
        store
            .denials
            .entry(approval_key.to_string())
            .or_default()
            .insert(session_id.to_string());
        true
    })
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

/// `chat:approval_required` 事件载荷的唯一构造点。字段与
/// [`PendingApproval`] 一一对应再加 `session_id`（serde camelCase 只用于
/// remount 读路径；这里的事件面沿用底座事件的 snake_case 键，前端两车道
/// 与 web 通告都按这套键消费）。此前载荷在转发器手构 `json!`，与本登记簿
/// 结构形成两处手工同步的形状——评审指出 deny/resolved/mode_changed 三个
/// 载荷都有键集钉而它没有（round-8 L3-F2），遂收口到同一模式：一个构造点
/// + 一个键集钉，字段漂移即红。
pub fn approval_required_payload(
    session_id: &str,
    approval: &PendingApproval,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "approval_id": approval.approval_id,
        "tool_name": approval.tool_name,
        "description": approval.description,
        "input": approval.input,
        "intent_summary": approval.intent_summary,
        "approval_key": approval.approval_key,
        "approval_grouping_key": approval.approval_grouping_key,
        "force_prompt": approval.force_prompt,
    })
}

/// `chat:guardrail_denied` 事件载荷的唯一构造点。`reason` 是稳定英文键
/// （[`ApprovalDenyReason::reason_key`]），前端 i18n 据此映射三语文案。
/// 刻意**不带 tool input 原文**——敏感参数不得进事件流（桌面 emit 与
/// remote 转发共用本载荷）；卡片路径的 input 只走 `chat:approval_required`
/// （用户已见面的场景）。
pub fn guardrail_denied_payload(
    session_id: &str,
    tool_name: &str,
    reason_key: &str,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "tool_name": tool_name,
        "reason": reason_key,
    })
}

/// 引擎审批 ack 的「未知终态」判定（评审 P2）。CodeWhale fb4f5e63f 起
/// approve/deny 句柄带 ack + settle 双窗确认契约（`EngineHandle::
/// approve_tool_call` 文档）：确定拒绝（stale id / 未能持久提交 / 未被
/// 引擎消费 / 决策通道关闭）与未知终态（双窗都超时，决策仍可能在引擎侧
/// 迟交并执行——durable commit 无截止时间）是两种不同的 `Err`。宿主区分
/// 这两类：确定拒绝的卡仍是活的未决请求（按栅栏恢复可重试），未知终态
/// 不得恢复活跃决策控件（迟交没有宿主对账事件，恢复即许诺一次可能已被
/// 消费的决策可以重来）。错误类型在底座 crate 内 pub(crate)，宿主无法按
/// 类型 downcast，只能按 Display 措辞判定；底座措辞漂移会让未知终态落回
/// 确定拒绝臂——恢复可重试的未决卡是旧（不诚实）行为（方向：多恢复、
/// 不误放行，双栅栏仍在）。宿主测试钉住「分类器 ↔ 措辞字面量」这一侧；
/// 底座 Display 本身的漂移检测需要底座导出标记常量/类型（CodeWhale#131
/// 后续项，round-8 L3-F1/L6-F2 如实收窄声明）。
pub(crate) fn approval_outcome_is_unknown(error: &anyhow::Error) -> bool {
    error
        .to_string()
        .contains("approval decision outcome is unknown")
}

/// `chat:approval_resolved` 事件载荷的唯一构造点（tri-mode M5 收敛面）：
/// 一处决策落定后广播终态，第二窗口 / 远端发起的决策也能把其它表面的卡
/// 就地落终态。前端只对未决卡生效（幂等）；负载不带 tool input（同
/// [`guardrail_denied_payload`] 的敏感面纪律）。
pub fn approval_resolved_payload(
    session_id: &str,
    approval_id: &str,
    decision: &str,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "approval_id": approval_id,
        "decision": decision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // 单元测试共用的 session id 前缀，clear_session 先行清理保证隔离。
    fn isolate(session: &str) -> String {
        clear_session(session);
        session.to_string()
    }

    /// 注册表与 `generation` 都是进程级全局（单例 `PENDING`）：并行测试线程的
    /// 任何登记变更都会推进 generation，令依赖「快照窗口内无并发变更」断言的
    /// 测试随机红（round-10 评审实证：forkguard 全量 84 条并发跑 2/4 红，命中
    /// `restore_if_generation_is` 与 `pending_card_stamps` 两条）。凡触碰注册
    /// 表的测试先取本锁串行化；纯载荷构造/源码钉测试不需要。
    fn test_registry_lock() -> std::sync::MutexGuard<'static, ()> {
        static REGISTRY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        REGISTRY_TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
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
            // record 盖章调用方传入的实例代；None 仅是构造占位。
            request_workspace_generation: None,
        }
    }

    /// M6b：硬拒通知载荷只带稳定键（session_id/tool_name/reason），刻意不带
    /// tool input 原文——敏感参数不得进事件流（桌面 emit 与 remote 转发共用
    /// 本载荷构造点）。字段集合扩大必须改这里。
    #[test]
    fn forkguard_guardrail_denied_payload_carries_stable_keys_only() {
        let payload = guardrail_denied_payload("sess-1", "write_file", "guardrail_ask_off");
        let obj = payload.as_object().expect("payload is a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["reason", "session_id", "tool_name"],
            "deny payload carries exactly the stable keys (no tool input): {obj:?}"
        );
        assert_eq!(obj["reason"], "guardrail_ask_off");
        let serialized = payload.to_string();
        assert!(
            !serialized.contains("input"),
            "deny payload must not carry tool input: {serialized}"
        );
    }

    /// `chat:approval_required` 载荷键集钉（round-8 L3-F2）：事件面 snake_case
    /// 键与登记簿结构一一对应；多一个键（泄漏）或少一个键（前端字段缺失）都红。
    #[test]
    fn forkguard_approval_required_payload_carries_the_registry_field_set() {
        let approval = PendingApproval {
            approval_id: "ap-1".to_string(),
            tool_name: "write_file".to_string(),
            description: "describe".to_string(),
            input: serde_json::json!({"path": "/tmp/x"}),
            intent_summary: Some("intent".to_string()),
            approval_key: "key".to_string(),
            approval_grouping_key: "group".to_string(),
            force_prompt: true,
            created_at_ms: 1,
            // 事件载荷走 json! 手工构造（不含代），代只在登记簿 list 面出现。
            request_workspace_generation: None,
        };
        let payload = approval_required_payload("sess-1", &approval);
        let obj = payload.as_object().expect("payload is a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "approval_grouping_key",
                "approval_id",
                "approval_key",
                "description",
                "force_prompt",
                "input",
                "intent_summary",
                "session_id",
                "tool_name",
            ],
            "approval_required payload carries exactly the registry field set: {obj:?}"
        );
        assert_eq!(obj["session_id"], "sess-1");
        assert_eq!(obj["approval_id"], "ap-1");
        assert_eq!(obj["force_prompt"], true);
    }

    /// M5 收敛面：决策终态广播载荷只带定位键 + 决策值（幂等去重靠
    /// approval_id），不带 tool input。
    #[test]
    fn forkguard_approval_resolved_payload_carries_keys_and_decision_only() {
        let payload = approval_resolved_payload("sess-1", "ap-1", "allow_for_session");
        let obj = payload.as_object().expect("payload is a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["approval_id", "decision", "session_id"],
            "resolved payload carries exactly the routing keys: {obj:?}"
        );
        assert_eq!(obj["decision"], "allow_for_session");
    }

    /// 未知终态判定的契约钉（评审 P2）：类型在底座 crate 内 pub(crate)，
    /// 宿主按 Display 措辞判定——底座 `ApprovalAckUnknownError` 的措辞
    /// （"approval decision outcome is unknown: …"）漂移会静默把未知终态
    /// 落回确定拒绝臂（恢复可重试的未决卡 = 旧的不诚实行为），这里把两类
    /// `Err` 的字面措辞钉住。确定拒绝措辞取自底座 `ack_answer` 的
    /// `NotApplied` 两臂与决策通道关闭；任何一臂措辞变化本测试即红。
    #[test]
    fn forkguard_approval_outcome_unknown_classifier_separates_the_engine_contracts() {
        // 未知终态：ack + settle 双窗超时（ApprovalAckUnknownError Display）。
        let unknown = anyhow::anyhow!(
            "approval decision outcome is unknown: the engine did not \
             acknowledge within 5s ack + 10s settle, and the decision may \
             still be committed and executed"
        );
        assert!(
            approval_outcome_is_unknown(&unknown),
            "the settle-expiry wording must classify as unknown"
        );
        // 确定拒绝三臂：stale id / 未被消费 / 通道关闭，均不得判成未知。
        let refused_stale = anyhow::anyhow!(
            "approval decision was not applied: the tool call id is stale \
             or its outcome could not be committed"
        );
        let refused_dropped =
            anyhow::anyhow!("approval decision was dropped without being consumed by the engine");
        let channel_closed = anyhow::anyhow!("channel closed");
        for error in [&refused_stale, &refused_dropped, &channel_closed] {
            assert!(
                !approval_outcome_is_unknown(error),
                "a determinate refusal must keep the retryable restore: {error}"
            );
        }
    }

    /// M6a：转发器调用点是纯函数真值表之外的实际接线——授予/拒绝实参对调
    /// （round-1 B1 的复发形态：跨会话放行）或 ask/force 对调都会静默翻转
    /// 处置，而真值表测试测不到调用点。钉住实参顺序与键维度：granted
    /// （grouping key，lossy）在前、denied（exact approval_key）在后，
    /// ask_on_guardrail 在 force_prompt 之前，且拒绝通知走唯一载荷构造点
    /// （不得退回内联 json! 重新引入 input）。
    #[test]
    fn forkguard_forwarder_approval_surface_call_site_pins_argument_order() {
        let src = include_str!("forwarder.rs");
        let production = src.split("#[cfg(test)]").next().expect("non-empty source");
        let call_start = production
            .find("approval_surface_decision(")
            .expect("the forwarder must route ApprovalRequired through the pure decision fn");
        let call = &production[call_start..];
        let call_end = call.find(");").expect("the decision call must terminate");
        let call = &call[..call_end];
        let granted = call
            .find("session_granted(")
            .expect("the grant cache read must feed the granted arm");
        let denied = call
            .find("session_denied(")
            .expect("the denial cache read must feed the denied arm");
        assert!(
            granted < denied,
            "positional argument order: granted (grouping key) must precede denied \
             (exact key) — a swap silently approves denied operations"
        );
        let granted_arm = &call[granted..];
        let denied_arm = &call[denied..];
        assert!(
            granted_arm.contains("&approval_grouping_key"),
            "the granted arm must read the lossy grouping key"
        );
        assert!(
            denied_arm.contains("&approval_key"),
            "the denied arm must read the exact approval key"
        );
        assert!(
            call.find("ask_on_guardrail").is_some(),
            "the ask-on-guardrail arming must be an input to the decision"
        );
        let ask = call.find("ask_on_guardrail").expect("checked above");
        let force = call
            .find("approval_force_prompt")
            .expect("the force flag must be an input to the decision");
        assert!(
            ask < force,
            "positional argument order: ask_on_guardrail must precede approval_force_prompt"
        );
        assert!(
            production.contains("guardrail_denied_payload("),
            "the deny announcement must go through the single payload constructor \
             (no inline json! that could reintroduce tool input)"
        );
    }

    #[test]
    fn forkguard_approval_surface_record_dedupes_by_approval_id() {
        let _registry = test_registry_lock();
        let session = isolate("approval-dedupe-test");
        record(&session, sample("ap-1", "k-1", "g-1"), None);
        record(&session, sample("ap-1", "k-1", "g-1"), None);
        record(&session, sample("ap-2", "k-2", "g-2"), None);
        let listed = list(&session);
        assert_eq!(listed.len(), 2, "同 approval_id 登记去重");
        assert_eq!(listed[0].approval_id, "ap-1", "原位替换不打乱顺序");

        // 原位替换：重复登记覆盖字段。
        let mut replaced = sample("ap-1", "k-1", "g-1");
        replaced.intent_summary = None;
        record(&session, replaced, None);
        assert!(list(&session)[0].intent_summary.is_none());
    }

    #[test]
    fn forkguard_approval_surface_remove_returns_entry_and_cleans_empty_session() {
        let _registry = test_registry_lock();
        let session = isolate("approval-remove-test");
        record(&session, sample("ap-1", "k-1", "g-1"), None);
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
    fn forkguard_remove_fenced_returns_atomic_generation_snapshot() {
        let _registry = test_registry_lock();
        let session = isolate("approval-remove-fenced-test");
        record(&session, sample("ap-f1", "k-f1", "g-f1"), None);
        let (removed, generation_after_remove) = remove_fenced(&session, "ap-f1")
            .expect("fenced removal must find the recorded approval");
        assert_eq!(removed.approval_id, "ap-f1");
        // The snapshot is captured under the same lock as the removal (the
        // removal itself bumps the generation), so absent further mutations
        // a fresh read equals it — that is the restore fence's "registry
        // unchanged during the await" precondition.
        assert_eq!(generation(), generation_after_remove);
        // Any later observable mutation advances the generation past the
        // snapshot, which is what cancels the decide-approval failure-arm
        // restore (terminal sweep, purge, concurrent decision, new card).
        record(&session, sample("ap-f2", "k-f2", "g-f2"), None);
        assert_ne!(
            generation(),
            generation_after_remove,
            "a registry mutation during the await window must cancel the fenced restore"
        );
        // remove_fenced shares remove's cleanup contract.
        assert!(
            remove_fenced(&session, "ap-f1").is_none(),
            "重复移除返回 None"
        );
    }

    #[test]
    fn forkguard_stale_restore_after_interleaved_mutation_is_refused() {
        let _registry = test_registry_lock();
        let session = isolate("approval-cas-restore-test");
        record(&session, sample("ap-r1", "k-r1", "g-r1"), None);
        let (pending, snapshot) = remove_fenced(&session, "ap-r1")
            .expect("fenced removal must find the recorded approval");
        // Happy path: no mutation since the removal — the compare-and-insert
        // succeeds and the entry is back in the registry.
        assert!(
            restore_if_generation_is(&session, pending.clone(), snapshot),
            "unchanged registry must restore the removed entry"
        );
        assert_eq!(list(&session).len(), 1, "restored entry is pending again");

        // Interleaving (review round 7 P2): a terminal sweep's mutation
        // lands between the remove_fenced snapshot and the restore. The
        // previous read-generation-then-record pair compared the generation
        // in one lock acquisition and inserted in a second, so the sweep
        // could advance it in between and the stale restore resurrected a
        // closed lifecycle's entry. The CAS rejects it under the same lock
        // as the insertion.
        //
        // Naming honesty (round-11 review mutation probe): this sequential
        // test cannot observe a lock split INSIDE restore_if_generation_is —
        // splitting the compare and the insert into two acquisitions yields
        // identical outcomes here, so atomicity itself is not what this test
        // pins. What it pins is the refuse-on-interleaved-mutation CAS
        // contract; the two-acquisition anti-pattern is pinned textually in
        // `decide_approval_side_effect_ordering_is_pinned`'s negative assert.
        let (pending, snapshot) = remove_fenced(&session, "ap-r1").expect("second fenced removal");
        record(&session, sample("ap-r2", "k-r2", "g-r2"), None);
        assert!(
            !restore_if_generation_is(&session, pending, snapshot),
            "a generation advanced after the snapshot must cancel the restore"
        );
        let live = list(&session);
        assert_eq!(live.len(), 1, "only the interleaved card stays");
        assert_eq!(live[0].approval_id, "ap-r2", "no stale resurrection");
    }

    #[test]
    fn forkguard_approval_surface_grant_and_denial_scoping() {
        let _registry = test_registry_lock();
        let session = isolate("approval-scope-test");
        let other = isolate("approval-scope-other");
        // 授予按 grouping_key：同组变体放行，异组不连坐。
        assert!(!session_granted(&session, "g-write"));
        assert!(remember_grant(&session, "g-write", None));
        assert!(session_granted(&session, "g-write"));
        assert!(
            !session_granted(&session, "g-rm"),
            "未授予的 grouping key 不放行"
        );

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
        assert!(remember_denial(&session, "k-rm-rf", None));
        assert!(session_denied(&session, "k-rm-rf"));
        assert!(
            !session_denied(&session, "k-rm"),
            "exact key 之外的调用不受拒绝连坐"
        );

        // 授予与拒绝互不串作用域。
        assert!(!session_denied(&session, "g-write"));
        assert!(!session_granted(&session, "k-rm-rf"));
    }

    #[test]
    fn forkguard_approval_surface_clear_pending_keeps_session_memory() {
        let _registry = test_registry_lock();
        let session = isolate("approval-clear-pending-test");
        record(&session, sample("ap-1", "k-1", "g-1"), None);
        remember_grant(&session, "g-1", None);
        remember_denial(&session, "k-1", None);

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

    /// turn 终态分裂清空：detached worker 的审批 id（底座
    /// `agent:<agent_id>:approval:<boot_id>:<n>` 形状）被摘出并返回给调用方
    /// 做确定性拒绝，父会话自己的审批 id 直接丢弃；两类条目都离开登记。
    /// 静默丢弃 worker 审批 = 把 `WaitingForUser` 的 worker 永久悬挂。
    #[test]
    fn forkguard_approval_surface_terminal_split_returns_child_ids() {
        let _registry = test_registry_lock();
        let session = isolate("approval-terminal-split-test");
        record(&session, sample("ap-parent-1", "k-1", "g-1"), None);
        let mut child = sample("agent:ag-1:approval:boot-1:1", "k-2", "g-2");
        child.intent_summary = None;
        record(&session, child, None);

        let child_ids = clear_pending_returning_child_ids(&session);
        assert_eq!(
            child_ids,
            vec!["agent:ag-1:approval:boot-1:1".to_string()],
            "只有子代理审批 id 被摘出供确定性拒绝"
        );
        assert!(list(&session).is_empty(), "两类条目都离开登记");
        assert!(
            clear_pending_returning_child_ids(&session).is_empty(),
            "重复清空返回空"
        );

        // 会话级授予/拒绝记忆不受终态清空影响（与 clear_pending 同语义）。
        remember_grant(&session, "g-1", None);
        remember_denial(&session, "k-1", None);
        clear_pending_returning_child_ids(&session);
        assert!(session_granted(&session, "g-1"), "授予跨 turn 存活");
        assert!(session_denied(&session, "k-1"), "拒绝跨 turn 存活");
        clear_session(&session);
    }

    /// 终态接线钉：`emit_chat_terminal` 必须用分裂变体摘登记，并把摘出的
    /// 子代理审批送确定性拒绝——退回静默 `clear_pending` 会把
    /// `WaitingForUser` 的 detached worker 永久悬挂（worker 审批比父 turn
    /// 活得久）。
    #[test]
    fn forkguard_emit_chat_terminal_denies_child_approvals_after_terminal() {
        let _registry = test_registry_lock();
        let src = include_str!("engine.rs");
        let production = src.split("#[cfg(test)]").next().expect("non-empty source");
        let sweep = production
            .find("clear_pending_returning_child_ids")
            .expect("the terminal sweep must split out detached-worker approval ids");
        assert!(
            production[sweep..].contains("deny_child_approvals_after_terminal"),
            "the split ids must be routed to the deterministic child denial"
        );
        assert!(
            production.contains("deny_tool_call"),
            "the denial task must go through the engine deny route (oneshot resolution)"
        );
        assert!(
            !production.contains("pending_approvals::clear_pending("),
            "the terminal sweep must not drop pending entries silently"
        );
    }

    #[test]
    fn forkguard_approval_surface_clear_session_wipes_memory() {
        let _registry = test_registry_lock();
        let session = isolate("approval-clear-test");
        let other = isolate("approval-clear-other");
        record(&session, sample("ap-1", "k-1", "g-1"), None);
        remember_grant(&session, "g-1", None);
        remember_grant(&other, "g-1", None);
        remember_denial(&session, "k-1", None);

        clear_session(&session);
        assert!(list(&session).is_empty(), "clear_session 清挂起");
        assert!(!session_denied(&session, "k-1"), "clear_session 清拒绝缓存");
        // g-1 同时被 other 会话授予：只摘除自己的份额，不误撤销他人授予。
        assert!(session_granted(&other, "g-1"), "他人授予不受本会话清理影响");
        assert!(!session_granted(&session, "g-1"), "本会话授予被摘除");
        clear_session(&other);
        assert!(
            !session_granted(&other, "g-1"),
            "最后一个授予会话清理后撤销"
        );
    }

    /// 核心时序（评审 JensenChen28 5764675531 第一项点名，机制自参考线
    /// round-3 P2 形状按本线重落）：决策写入携带**请求创建时**捕获的工作区
    /// 代，落笔时代 ≠ 当前已提交代即在 store 锁内被原子拒绝、整条丢弃。
    /// Source-reachable interleave：A 目录点击 → approve ack 已提交 → IPC
    /// future 就绪未恢复 → 重绑门空闲 → 绑定移到 B（绑定提交点推进代 + 保留
    /// 代清理）→ 旧成功臂恢复执行——此刻的落笔若不校验来源代，就会把 A 目录
    /// 的决策写进 B 的记忆。Mutation（negative-first）：移除 remember_* 的
    /// 比对臂即核心断言红；移除 advance（代不推进）即「迟交写入被拒」红。
    #[test]
    fn forkguard_late_old_scope_decision_is_rejected_after_rebind() {
        let _registry = test_registry_lock();
        let session = isolate("approval-req-gen-late");
        // 卡片在 A 目录（无代）弹出：实例 spawn 代快照为 None。
        record(&session, sample("ap-late", "k-late", "g-late"), None);
        let origin = list(&session)[0].request_workspace_generation;
        assert_eq!(origin, None, "A 时代的请求卡携带无代");

        // 重绑的绑定提交点：推进代 + 保留代清理（B 目录纪元开始）。
        assert_eq!(advance_workspace_generation(&session), 1);
        clear_session_preserving_generation(&session);

        // 旧成功回调迟交：携带请求时捕获的代，必须被整体拒绝——不得给
        // 新代（B）盖章。
        assert!(
            !remember_grant(&session, "g-late", origin),
            "旧范围迟交授予必须被拒（重新请批，不静默记到新代名下）"
        );
        assert!(
            !remember_denial(&session, "k-late", origin),
            "旧范围迟交拒绝必须被拒（不跨目录连坐新目录）"
        );
        // 新范围查询：B 的同族请求不得命中 A 时代的迟交决策。
        assert!(
            !session_granted(&session, "g-late"),
            "B 的同族请求不得命中旧范围的迟交授予"
        );
        assert!(
            !session_denied(&session, "k-late"),
            "B 的同族请求不得命中旧范围的迟交拒绝"
        );

        // 阴性对照：新实例（B，代 1 spawn）创建的请求照常落笔并命中——
        // record 盖的是调用方传入的实例代，新实例携带推进后的代。
        record(&session, sample("ap-b", "k-late", "g-late"), Some(1));
        let origin_b = list(&session)
            .iter()
            .find(|entry| entry.approval_id == "ap-b")
            .map(|entry| entry.request_workspace_generation)
            .flatten();
        assert_eq!(origin_b, Some(1), "record 盖章调用方传入的实例代");
        assert!(remember_grant(&session, "g-late", origin_b));
        assert!(session_granted(&session, "g-late"), "当前代决策照常命中");
        // 代再推进（下一次重绑）+ 保留代清理后，该决策同样失效——有效性
        // 始终绑定版本；且代必须单调（不得清后重臂，见 identity-switch 测试）。
        assert_eq!(advance_workspace_generation(&session), 2);
        clear_session_preserving_generation(&session);
        assert!(
            !session_granted(&session, "g-late"),
            "版本前进 + 份额回收后决策失效"
        );
        assert!(
            !remember_grant(&session, "g-late", Some(1)),
            "推进后旧代（1）的写入被拒：代单调，不得重臂"
        );
        clear_session(&session);
    }

    /// 挂起卡盖章**发起实例**的 spawn 代，而不是收卡时刻的当前代（评审
    /// 5764675531，参考 round-4 P2 sub-gap a）。重绑期间越过入口忙闸启动的
    /// turn 仍由旧 cwd 运行时执行：旧实例跨绑定提交点的代推进后弹出的卡若
    /// 按收卡时的当前代盖章，用户批准即把旧目录的决策写入新代记忆。登记代
    /// = 实例 spawn 代后，决策写入按登记代比较，旧实例的卡在新代下整体拒绝
    /// （fail-safe 重新请批），新实例（新 cwd）的卡照常落笔；决策失败臂的
    /// 恢复（remove_fenced → restore_if_generation_is）原样带回登记代，恢复
    /// 后的卡再次决策仍按实例代校验（post-partial-failure recovery 同源
    /// 覆盖）。Mutation（negative-first）：让 record 忽略 instance_generation、
    /// 改为盖章收卡时刻的当前代，即第一对与恢复对断言红（forwarder 侧的
    /// 快照接线另有 source pin）。
    #[test]
    fn forkguard_pending_card_stamps_emitting_instance_generation() {
        let _registry = test_registry_lock();
        let session = isolate("approval-inst-gen");
        // 旧实例（spawn 于 A 目录、无代）在绑定提交点的代推进之后才弹卡：
        // 登记携带实例的 spawn 代（None），不是收卡时的当前代。
        assert_eq!(advance_workspace_generation(&session), 1);
        record(&session, sample("ap-old", "k-old", "g-old"), None);
        let stamped = list(&session)[0].request_workspace_generation;
        assert_eq!(
            stamped, None,
            "旧实例的卡必须携带实例的 spawn 代（None），不得按收卡当前代盖章"
        );
        // 用户批准该卡：决策写入携带登记代，与当前代不等 → 原子拒绝，
        // 旧目录的决策不得记到新代名下。
        assert!(
            !remember_grant(&session, "g-old", stamped),
            "旧实例跨推进弹卡的批准必须被拒（重新请批，不静默记到新代名下）"
        );
        assert!(
            !remember_denial(&session, "k-old", stamped),
            "旧实例跨推进弹卡的拒绝同样被拒"
        );
        assert!(
            !session_granted(&session, "g-old") && !session_denied(&session, "k-old"),
            "被拒的旧范围写入不得留下任何记忆"
        );

        // post-partial-failure recovery：决策的 engine 调用失败臂把摘除的
        // 登记原样放回（remove_fenced → restore_if_generation_is）。恢复后
        // 的卡必须保留实例代，再决策仍按实例代校验。
        let (pending, snapshot) = remove_fenced(&session, "ap-old").expect("旧实例的卡仍在登记表");
        assert_eq!(
            pending.request_workspace_generation, None,
            "恢复携带必须原样带回实例代"
        );
        assert!(
            restore_if_generation_is(&session, pending, snapshot),
            "无并发变更时恢复照常落回"
        );
        let restored = list(&session)[0].request_workspace_generation;
        assert_eq!(restored, None, "恢复后的卡保留实例代");
        assert!(
            !remember_grant(&session, "g-old", restored),
            "恢复后的卡再次决策仍按实例代校验（旧实例代仍被拒）"
        );
        clear_session(&session);
    }

    /// 同 approval_id 重放（原位替换）按**最新一次**盖章实例代：engine 重放
    /// 同一审批按新请求语义以最新一次的实例代为准，不得保留旧值。同一实例
    /// 的重放 = 同一代；engine 审批 id 按实例铸造，重建实例不会重放旧 id，
    /// 新实例的重放携带自己的 spawn 代（与实例来源语义一致）。
    #[test]
    fn forkguard_record_replace_recaptures_request_generation() {
        let _registry = test_registry_lock();
        let session = isolate("approval-req-gen-recapture");
        record(&session, sample("ap-re", "k-re", "g-re"), None);
        assert_eq!(list(&session)[0].request_workspace_generation, None);
        advance_workspace_generation(&session);
        let mut replay = sample("ap-re", "k-re", "g-re");
        replay.intent_summary = None;
        record(&session, replay, Some(1));
        let entry = &list(&session)[0];
        assert_eq!(
            entry.request_workspace_generation,
            Some(1),
            "重放按最新一次的实例代盖章"
        );
        assert!(entry.intent_summary.is_none(), "其余字段仍以最新一次为准");
        clear_session(&session);
    }

    /// 冷启动身份快照 vs 绑定提交点推进的**真实交错**（评审 JensenChen28
    /// 5766477612、zhuowp 5766212059/5766526920，P2-b）：实例身份必须在
    /// **启动捕获**时定死（AppEngine spawn 链同步段、与 roots/workspace 固
    /// 定同点——engine/forwarder 两侧 source pin 钉住该接线），本测试驱动
    /// 「startup 捕获 → rebind 绑定提交点 `advance_workspace_generation` →
    /// forwarder 首次执行 mint 卡」：启动步走生产同一捕获函数
    /// `workspace_generation` 取值（不手工捏造旧代），断言卡携带**启动时**
    /// 捕获的身份代——缺陷形态（async body 首次执行现读）会把卡盖成推进
    /// 后的新代、通过决策比较、把 A 目录的授权/拒绝写进 B；随后断言旧代
    /// 决策写入被拒。阴性对照：推进后冷启动的 B 实例经同一捕获函数取得
    /// 新代，其卡照常落笔命中。Mutation（negative-first）：把捕获挪回
    /// forwarder async body 现读 → 两侧 source pin 红（本测试锁 store 端
    /// 语义）；record 改章收卡时刻当前代 → 第一对断言红。
    #[test]
    fn forkguard_coldstart_identity_survives_rebind_before_first_mint() {
        let _registry = test_registry_lock();
        let session = isolate("approval-coldstart-gen");
        // Startup（A 目录冷启动）：spawn 链同步段经生产捕获函数快照身份代
        // （从未重绑 → None）。
        let startup_captured = workspace_generation(&session);
        assert_eq!(startup_captured, None, "A 纪元冷启动实例身份代为 None");

        // 重绑绑定提交点：推进代（B 纪元开始）。
        assert_eq!(advance_workspace_generation(&session), 1);

        // Forwarder 首次执行 mint：卡登记携带 spawn 传入的启动身份代，不是
        // 收卡时刻的当前代——缺陷形态现读会带 Some(1)，A 的决策随之写入 B。
        record(&session, sample("ap-a", "k-a", "g-a"), startup_captured);
        let stamped = list(&session)[0].request_workspace_generation;
        assert_eq!(
            stamped, startup_captured,
            "跨推进首次 mint 的卡必须携带启动捕获的身份代"
        );
        assert_ne!(
            stamped,
            Some(1),
            "A 实例的卡不得带推进后的 B 代：带 B 代即通过决策比较写入 B"
        );

        // A 实例卡的用户决策：携带启动旧代 vs 已提交新代 → 原子拒绝，
        // 不给新代盖章、不落记忆。
        assert!(
            !remember_grant(&session, "g-a", stamped),
            "A 实例（启动代 None）跨推进的批准必须被拒（重新请批）"
        );
        assert!(
            !remember_denial(&session, "k-a", stamped),
            "A 实例跨推进的拒绝必须被拒（不跨目录连坐）"
        );
        assert!(
            !session_granted(&session, "g-a") && !session_denied(&session, "k-a"),
            "被拒的旧身份写入不得留下任何记忆"
        );

        // 阴性对照：重绑后新发送方在 B 目录冷启动，同一捕获函数现读得推进
        // 后的代；其卡照常落笔并命中（推进不误伤新纪元）。
        let startup_captured_b = workspace_generation(&session);
        assert_eq!(
            startup_captured_b,
            Some(1),
            "B 纪元冷启动实例经同一捕获函数取得推进后的代"
        );
        record(&session, sample("ap-b", "k-b", "g-b"), startup_captured_b);
        assert!(
            remember_grant(&session, "g-b", startup_captured_b),
            "B 实例（当前代）的批准照常落笔"
        );
        assert!(session_granted(&session, "g-b"), "当前代决策照常命中");
        clear_session(&session);
    }

    /// 捕获协议（评审 5768924092 / 5768948485 P2-a 第二轮）—— 无并发：
    /// 一次捕获即成功，返回值 = (roots == expected, 当前 gen_b)。engine
    /// 侧实际喂 config 的 workspace 不取返回值（与 expected 相等才被
    /// 接受），gen_b 随 spawn 传给转发器盖章。
    #[test]
    fn forkguard_capture_identity_happy_path_single_round() {
        let _registry = test_registry_lock();
        let session = isolate("capture-happy");
        let expected = "A-root".to_string();
        let resolves = std::cell::Cell::new(0usize);
        let (roots, generation) = capture_workspace_identity(&session, &expected, || {
            resolves.set(resolves.get() + 1);
            expected.clone()
        })
        .expect("无并发时一次捕获必须成功");
        assert_eq!(roots, expected, "接受的 roots 必须与 expected 一致");
        assert_eq!(generation, None, "从未重绑的会话身份代为 None");
        assert_eq!(resolves.get(), 1, "无并发时协议只走一轮");
        clear_session(&session);
    }

    /// 捕获协议 —— **窗口内 advance**（模拟另一线程的重绑提交点落在
    /// gen_a 与 gen_b 之间）：首轮代前后失配被检出、整轮作废；重试轮闭包
    /// 不再推进 → 捕获成功。返回的 gen 是推进后的新代（推进是既成事实，
    /// 协议只保证「代与 roots 同窗一致」），且重试轮数证明撕裂轮没有被
    /// 静默接受——变异：删掉 gen_a != gen_b 比较臂，本测试的轮数断言红
    /// （首轮撕裂快照会被直接放行）。
    #[test]
    fn forkguard_capture_identity_retries_advance_inside_window() {
        let _registry = test_registry_lock();
        let session = isolate("capture-window-advance");
        let expected = "A-root".to_string();
        let first_round = std::cell::Cell::new(true);
        let resolves = std::cell::Cell::new(0usize);
        let (roots, generation) = capture_workspace_identity(&session, &expected, || {
            resolves.set(resolves.get() + 1);
            // 首轮模拟另一线程的重绑提交点落进读取窗口：resolve 执行时
            // 推进代（gen_a 读旧值、gen_b 读新值 → 整轮撕裂）。
            if first_round.get() {
                first_round.set(false);
                advance_workspace_generation(&session);
            }
            expected.clone()
        })
        .expect("窗口内 advance 首轮检出后，重试轮必须成功");
        assert_eq!(roots, expected, "重试轮返回的 roots 与 expected 一致");
        assert_eq!(
            generation,
            Some(1),
            "返回代为推进后的新代（同窗一致的现值，不是撕裂轮的旧值）"
        );
        assert_eq!(
            resolves.get(),
            2,
            "撕裂轮必须整轮作废重试（一次检出 + 一次干净轮），不得静默接受首轮"
        );
        clear_session(&session);
    }

    /// 捕获协议 —— **持续 advance**（每一轮的重绑提交点都落进读取窗口，
    /// 现实对应重绑风暴期间发起的 spawn）：重试上限用尽 → Err，fail-closed
    /// 拒绝本次 spawn——报错措辞说明重绑进行中无法一致捕获工作区身份。
    /// 变异：删掉重试上限的 Err 臂（耗尽后照常返回），本测试红。
    #[test]
    fn forkguard_capture_identity_exhausts_under_persistent_advance() {
        let _registry = test_registry_lock();
        let session = isolate("capture-persistent-advance");
        let expected = "A-root".to_string();
        let error = capture_workspace_identity(&session, &expected, || {
            advance_workspace_generation(&session);
            expected.clone()
        })
        .expect_err("持续撕裂必须耗尽重试并 Err（fail-closed 拒绝 spawn）");
        assert!(
            error.contains("rebind is in flight"),
            "报错必须说明重绑进行中无法一致捕获身份：{error}"
        );
        clear_session(&session);
    }

    /// 捕获协议 —— **roots 漂移**（swap→advance 次序下 advance 落地前的
    /// 「新根+旧代」撕裂配对：代比较检不出来，只能靠 roots 相对 engine
    /// 侧已解析值的核对拒收）。漂移一轮后回到 expected → 检出 + 重试
    /// 成功；持续漂移（rebind 已完全落地：旧 expected 永不再现）→ 耗尽
    /// → Err。两个撕裂方向都被拒：本测试锁「新根+旧代」，窗口 advance
    /// 用例锁「旧根+新代」。变异：删掉 roots 比较臂，两条断言红。
    #[test]
    fn forkguard_capture_identity_rejects_roots_drift() {
        let _registry = test_registry_lock();
        let session = isolate("capture-roots-drift");
        let expected = "A-root".to_string();
        // 漂移一轮：resolve 返回换根后的新 roots（代未推进——advance 尚未
        // 落地的真实形态），次轮回到 expected → 重试成功。
        let first_round = std::cell::Cell::new(true);
        let resolves = std::cell::Cell::new(0usize);
        let (roots, generation) = capture_workspace_identity(&session, &expected, || {
            resolves.set(resolves.get() + 1);
            if first_round.get() {
                first_round.set(false);
                "B-root".to_string()
            } else {
                expected.clone()
            }
        })
        .expect("漂移轮检出后，回到 expected 的重试轮必须成功");
        assert_eq!(roots, expected);
        assert_eq!(generation, None, "漂移轮代未推进，接受轮读得的仍是原代");
        assert_eq!(
            resolves.get(),
            2,
            "roots 漂移必须按撕裂窗口整轮作废重试，不得静默接受首轮"
        );

        // 持续漂移：重绑完全落在捕获窗口之前，新根永远不等于 engine 侧
        // 旧 expected（此时 engine_config.workspace 仍是旧根，放行即
        // 「A 目录 runtime + B 纪元」本身）→ 耗尽 → Err。
        let error = capture_workspace_identity(&session, &expected, || "B-root".to_string())
            .expect_err("持续 roots 漂移必须耗尽重试并 Err");
        assert!(
            error.contains("rebind is in flight"),
            "耗尽报错与代撕裂同口径：{error}"
        );
        clear_session(&session);
    }

    /// 身份切换清理与删除清理的唯一差别（评审 5764675531 第一项）：
    /// `clear_session_preserving_generation` 回收挂起卡、授予、拒绝，但
    /// **保留工作区代**——若代被一并清掉，会话回落「无代」，绑定提交点推进
    /// 前盖 None 代的旧决策重新与新状态相等（旧授予复活、迟交的旧范围写入
    /// 又被接受）。保留代让旧代永远 miss，新目录下的决策照常落笔命中；
    /// 同会话他人授予不受影响；随后真正的 `clear_session`（会话删除）仍走
    /// 完整重置。Mutation（negative-first）：把绑定提交点清理换回
    /// `clear_session` 即最后一对断言红（projects.rs 的命令层测试从 rebind
    /// 入口复现同一形态）。
    #[test]
    fn forkguard_identity_switch_cleanup_preserves_workspace_generation() {
        let _registry = test_registry_lock();
        let session = isolate("approval-ws-gen-preserve");
        let other = isolate("approval-ws-gen-preserve-other");
        // A 目录（无代）的预重绑决策。
        assert!(remember_grant(&session, "g-keep", None));
        assert!(remember_denial(&session, "k-keep", None));
        assert!(remember_grant(&other, "g-keep", None));
        assert!(session_granted(&session, "g-keep"));

        // 绑定提交点推进代（B 目录），旧决策 miss（写侧防线：None 代迟交
        // 写入被拒，不给新代盖章）。
        assert_eq!(advance_workspace_generation(&session), 1);
        assert!(!remember_grant(&session, "g-keep", None));
        assert!(!remember_denial(&session, "k-keep", None));

        // 绑定提交点的身份切换清理：回收条目、保留代。
        clear_session_preserving_generation(&session);
        assert!(list(&session).is_empty(), "挂起卡仍被回收");
        assert!(
            !session_granted(&session, "g-keep"),
            "本会话旧授予已被清理摘除，不得复活"
        );
        assert!(
            !session_denied(&session, "k-keep"),
            "本会话旧拒绝已被清理摘除"
        );
        assert!(
            session_granted(&other, "g-keep"),
            "他人授予不受本会话清理影响"
        );
        // 代存活：None 代（A 时代）的迟交写入被拒，不给新代盖章。
        assert!(
            !remember_grant(&session, "g-keep", None),
            "迟交的旧范围授予必须被拒（代未回落到无代）"
        );
        assert!(!session_granted(&session, "g-keep"));
        // 新目录（代 1）下的决策照常落笔并命中。
        assert!(remember_grant(&session, "g-keep", Some(1)));
        assert!(session_granted(&session, "g-keep"));
        // 真正的会话删除仍是完整重置：删除后重建的同 id 会话从无代重新
        // 起步（删除语义不回归）。
        clear_session(&session);
        assert!(
            remember_grant(&session, "g-keep", None),
            "删除清理后同 id 会话从无代重新起步"
        );
        assert!(session_granted(&session, "g-keep"));
        clear_session(&session);
        clear_session(&other);
    }

    /// 处置真值表（定时任务无人值守路径不在此函数，另行短路）。
    /// 列：mode | ask | force | granted | denied → 处置。
    #[test]
    fn forkguard_approval_surface_decision_truth_table() {
        let _registry = test_registry_lock();
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
            (
                SerializableMode::Plan,
                false,
                false,
                true,
                false,
                Deny(PlanMode),
            ),
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
