//! 会话 checkpoint 命令：turn 边界快照的查询、差异预览与回滚。
//!
//! 快照本体在 `features/code_checkpoints`（影子 git 仓库，数据落账本根）；本文件
//! 只做传输边界：原生车道校验、两个根解析、忙碌门与错误文案。原生车道 = 工作模式
//! 会话 + 品悟原生 code 会话（`SessionKind::Chat` + 非 ACP）：ACP 会话与定时
//! 会话如实拒绝（设计 §11：ACP 不做；定时会话无每轮快照，与 fork 拒绝口径一致）。
//! aux 侧聊会话（`aux-`）无快照即无回退面（r5 评审起 chat() 侧跳过快照）；
//! r29 minor 3：破坏性命令（rewind/undo）显式拒绝 aux——raw IPC 直调不得经
//! truncate_to_user_turn 改写 aux transcript（文件头旧声明「仅历史查询照常
//! 放行」对破坏性腿失实，已按实际行为改写）。

use super::prelude::*;
use crate::features::code_checkpoints as checkpoints;
use crate::features::codex_acp::AcpPool;
use checkpoints::{CheckpointDiff, CheckpointKind, CheckpointMeta};

/// 原生车道判定：`SessionKind::Chat` 排除定时会话（ScheduledRun），`!is_acp`
/// 排除外部 agent 车道（调用方传 `AcpPool::is_acp`；测试注入替身谓词——AcpPool
/// 依赖 AppHandle，无法在单元测试构造）。工作模式与原生 code 会话同为原生车道
/// （后者是本能力的既有用户，门放宽后行为不变）。判定配方与 `list_sessions`
/// 的车道过滤同源。
fn is_native_checkpoint_lane(
    session_id: &str,
    store: &SessionStore,
    is_acp: impl Fn(&str) -> bool,
) -> bool {
    matches!(store.session_kind(session_id), Ok(SessionKind::Chat)) && !is_acp(session_id)
}

/// 解析原生车道会话的两个根：账本根（checkpoint 数据落点）+ 执行根（快照对象）。
/// 非原生车道（ACP/定时）、会话不存在、根不可用都如实报错，不静默降级。
/// `is_acp` 注入缝同 [`is_native_checkpoint_lane`]。
fn resolve_native_session_roots(
    session_id: &str,
    store: &SessionStore,
    is_acp: impl Fn(&str) -> bool,
) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    if !is_native_checkpoint_lane(session_id, store, is_acp) {
        return Err("仅原生会话支持检查点".to_string());
    }
    let roots = store
        .session_roots(session_id)
        .map_err(|error| format!("解析会话根失败: {error:#}"))?;
    Ok((roots.ledger, roots.execution))
}

/// r23 M1：replay 臂的破坏性栅栏输入。压缩不持 turn 预约——会让破坏性重放
/// （force-checkout + `clean -fd` + truncate）踩到在飞写入，与 rewind_to_turn
/// 的栅栏同类（r22 R2 / r21 M4）。
/// r24 minor 5：`pools` 让对端探针与执行根互斥惰性化——仅当 pending 存在且
/// 走到重放臂时才消费（`None` = 单元夹具无池：对端探针跳过、根互斥跳过，
/// 生产接线由 production-body pin 锚定，见 `send_preludes_wire_reconcile_gates_to_real_probes`）。
#[derive(Default, Clone, Copy)]
pub(super) struct ReconcileGates<'a> {
    pub compaction_in_flight: bool,
    pub pools: Option<(&'a EnginePool, &'a AcpPool)>,
}

/// r23 M1：同根对端忙碌探针——`reject_if_peer_busy` 的枚举内核、只回结论
/// （对账顺延路径不给发送路径塞拒绝文案；两者必须同向演化，改动枚举口径时
/// 同步检查）。`None` = 探针本身失败（fail-closed，按忙碌顺延）。
pub(super) async fn same_root_replay_busy(
    store: &SessionStore,
    pool: &EnginePool,
    acp_pool: &AcpPool,
    session_id: &str,
    execution: &std::path::Path,
) -> Option<bool> {
    let store_gate = store.clone();
    let session_id_gate = session_id.to_string();
    let execution_gate = execution.to_path_buf();
    let pool_gate = pool.clone();
    let acp_store = store.clone();
    let acp_session = session_id.to_string();
    let acp_execution = execution.to_path_buf();
    let agents_probe = acp_pool.agents().clone();
    let agents_peers = acp_pool.agents().clone();
    let (busy, acp_peers) = match tauri::async_runtime::spawn_blocking(move || {
        let busy = busy_peer_on_same_execution_root(
            &store_gate,
            &session_id_gate,
            &execution_gate,
            production_execution_root_resolver(agents_probe, store_gate.clone()),
            |id| pool_gate.is_turn_active(id) || pool_gate.is_scheduled_turn_running(id),
        )?;
        let peers = peers_on_same_execution_root(
            &acp_store,
            &acp_session,
            &acp_execution,
            production_execution_root_resolver(agents_peers, acp_store.clone()),
        )?;
        Ok::<_, String>((busy, peers))
    })
    .await
    {
        Ok(Ok(pair)) => pair,
        _ => return None,
    };
    if busy.is_some() {
        return Some(true);
    }
    let acp_peer_ids: Vec<String> = acp_peers
        .iter()
        .map(|(id, _)| id.as_str())
        .filter(|id| acp_pool.is_acp(id))
        .map(str::to_string)
        .collect();
    if acp_peer_ids.is_empty() {
        return Some(false);
    }
    let blocking = acp_pool.rebind_blocking_sessions(&acp_peer_ids).await;
    Some(match blocking {
        None => true,
        Some(blocking) => !blocking.is_empty(),
    })
}

/// r19 M6 崩溃对账：回退协议（apply → truncate → clear）中途进程死亡后暂记
/// 留在盘上——代码可能已恢复而 transcript 未截断（revision 仍等于暂记值）。
/// 不在此收口，下一轮快照会把混合树拍进影子库、revision 前进，pending 复用
/// 门与 undo 绑定双双永久失效（崩溃那次的 PreRestore——真前态的唯一副本——
/// 从此不可达）。
/// 规则（r23 M2 重写）：「截断已提交」的判定器是 sidecar 记录而非 revision——
/// `truncate_to_user_turn` 在持久化之前先写绑定本次 `pre_restore_id` 的
/// `RewoundTurnsRecord`（先备份后落盘），故「绑定记录 **且 revision 已前进**」
/// ⇔ 已提交（二者缺一不可：persist 分裂窗内记录已写而截断持久化失败，
/// revision 仍匹配 → 落入重放臂，幂等重放 + 重复记录无害，r25 MAJOR-3）；
/// 缺失 ⇔ 未提交（崩溃**或 live-abort**），进入重放臂——即使 revision 已被
/// 非协议写（headless 引擎持久化、压缩 persist、远程编辑）推进，截断以
/// keep_turns 的 turn 计数锚定、与 revision 无关。pending 在飞期间新回退
/// 被拒，`latest` 记录即唯一候选。重放臂是破坏性的（r23 M1）：压缩在飞或
/// 同根对端忙碌时顺延对账（阻断本轮快照，下一轮重试，混合态保持未固化）。
/// 返回 false = 本轮快照必须阻断（对账未完成时把混合状态固化进历史是更坏
/// 的失败模式）。
async fn reconcile_pending_rewind(
    store: &SessionStore,
    session_id: &str,
    ledger: &std::path::Path,
    execution: &std::path::Path,
    gates: ReconcileGates<'_>,
) -> bool {
    let pending = match checkpoints::live_pending_rewind(ledger) {
        Ok(Some(pending)) => pending,
        Ok(None) => return true,
        Err(error) => {
            log::error!(
                "[pinvou3][checkpoints] pending rewind read failed, blocking this turn's snapshot sid={session_id}: {error:#}"
            );
            return false;
        }
    };
    // 当前 revision 与会话加载同源（store.load）；turn 计数同源复用（r24 M1）。
    let (current_revision, current_turn_count) = match store.load(session_id) {
        Ok(session) => {
            let revision = match crate::features::sessions::transcript_revision(&session.messages) {
                Ok(revision) => revision,
                // r22 minor 2：revision 计算失败不得吞成默认值——空串恒不
                // 等于暂记 revision，会把「截断未提交（apply 可能没跑过）」
                // 误判成「截断已提交」并清掉暂记。与下方 load 失败同款：
                // 阻断本轮快照。
                Err(error) => {
                    log::error!(
                        "[pinvou3][checkpoints] transcript revision failed during rewind reconciliation, blocking this turn's snapshot sid={session_id}: {error:#}"
                    );
                    return false;
                }
            };
            (revision, checkpoints::count_user_turns(&session.messages))
        }
        Err(error) => {
            log::error!(
                "[pinvou3][checkpoints] session load failed during rewind reconciliation, blocking this turn's snapshot sid={session_id}: {error:#}"
            );
            return false;
        }
    };
    // r23 M2：sidecar 是真正的提交判定器（见函数头注释）。
    let committed = match store.latest_rewound_turns_record(session_id) {
        Ok(Some(record)) => {
            record.pre_restore_checkpoint_id.as_deref() == Some(pending.pre_restore_id.as_str())
        }
        Ok(None) => false,
        Err(error) => {
            log::error!(
                "[pinvou3][checkpoints] rewind sidecar read failed, blocking this turn's snapshot sid={session_id}: {error:#}"
            );
            return false;
        }
    };
    if current_revision != pending.transcript_revision && committed {
        // 已提交、仅清理失败的残留——清掉暂记即可。
        checkpoints::clear_pending_rewind(ledger);
        return true;
    }
    // r24 M1：keep_turns ≥ 当前总轮数 ⇒ 该回退已无截断对象（确定性失败，
    // sessions/rewind.rs 的轮数门必然再拒）——按空满足处理：清暂记、放行
    // 本轮快照，**绝不重放 apply**。否则重放臂先 force-checkout 旧边界树
    // （清掉更新的写入）再截断失败、暂记永存 = 每条消息无限破坏性重放
    // （CLI undo 跨进程收缩 transcript、带外编辑皆可达）。瞬态 IO 失败
    // 仍走下方阻断路径。
    if current_turn_count <= pending.keep_turns {
        log::warn!(
            "[pinvou3][checkpoints] pending rewind is vacuous (keep_turns={} >= current turns {current_turn_count}), clearing pending sid={session_id}",
            pending.keep_turns
        );
        checkpoints::clear_pending_rewind(ledger);
        return true;
    }
    // r35 MAJOR-2：pre_restore_id 必须真实存在于索引——否则 planted pending
    // 的伪造 id 保证 committed=false → 重放臂无备份破坏性恢复 + 截断。
    // 真实崩溃暂记的 PreRestore 在 prepare_restore 注册后才写入暂记，此刻
    // 必然仍在索引中；被 LRU 淘汰则**应阻断**而非重放（模块既有的
    // fail-closed 方向）。 NotFound 与真实 IO 错误同款区分。
    if checkpoints::find_checkpoint(ledger, &pending.pre_restore_id).is_err() {
        log::warn!(
            "[pinvou3][checkpoints] pending rewind pre_restore_id {} not found in index, blocking sid={session_id}",
            pending.pre_restore_id
        );
        return false;
    }
    // r23 M1：重放臂破坏性且调用点（快照前奏）不持预约、无任何栅栏——
    // live-abort（ACP 复查失败 / apply / join 失败）留下的 pending 若无门
    // 重放，等于让用户看着失败掉的回退在下一条消息发送时静默完成。
    // 压缩闩是会话内的即时读，先查。
    if gates.compaction_in_flight {
        log::warn!(
            "[pinvou3][checkpoints] pending rewind replay deferred (compaction_in_flight), blocking this turn's snapshot sid={session_id}"
        );
        return false;
    }
    // r24 M3 + r25 MAJOR-1：**先置执行根互斥标志、后探对端忙碌**——
    // `busy_peer_on_same_execution_root` 的契约要求 flag-then-check（探针通过
    // 后、置位前，同根原生对端的 reserve_turn 会被放行，重放的
    // checkout-index / clean 会清掉其在飞写入；手动路径均为先置位再探）。
    // 拿不到标志 = 另一回退在飞，与手动路径同款顺延。
    // r24 minor 5：对端探针惰性化——仅当 pending 存在且走到重放臂时才枚举。
    let _root_guard = match gates.pools {
        Some((pool, _)) => match pool.begin_execution_root_rewind(execution) {
            Ok(guard) => Some(guard),
            Err(error) => {
                log::warn!(
                    "[pinvou3][checkpoints] pending rewind replay deferred (execution-root rewind flag held), blocking this turn's snapshot sid={session_id}: {error:#}"
                );
                return false;
            }
        },
        None => None,
    };
    let peer_busy = match gates.pools {
        Some((pool, acp_pool)) => {
            same_root_replay_busy(store, pool, acp_pool, session_id, execution)
                .await
                .unwrap_or(true)
        }
        None => false,
    };
    if peer_busy {
        log::warn!(
            "[pinvou3][checkpoints] pending rewind replay deferred (peer_busy={peer_busy}), blocking this turn's snapshot sid={session_id}"
        );
        return false;
    }
    // 半程态：幂等重放 apply（覆盖「写暂记后、apply 前死亡」与「apply 后、
    // 截断前死亡」两种崩溃点），随后完成截断。
    let target = match checkpoints::find_checkpoint(ledger, &pending.checkpoint_id) {
        Ok(meta) => meta,
        // r25 minor 1：淘汰（目标不在索引）已由 live_pending_rewind 前置清理，
        // 这里残余的 Err 是索引/IO 读取失败——按瞬态处理阻断本轮快照，
        // 不得误判为淘汰而清掉暂记。
        Err(error) => {
            log::error!(
                "[pinvou3][checkpoints] pending rewind target lookup failed, blocking this turn's snapshot sid={session_id} target={}: {error:#}",
                pending.checkpoint_id
            );
            return false;
        }
    };
    let apply_ledger = ledger.to_path_buf();
    let apply_execution = execution.to_path_buf();
    // r38 M3：evict 在 apply **之前**——与手动回退路径的 restore-前驱逐同向。
    // 驱逐后即使引擎的 idle persist 在 apply/truncate 窗口内落地，
    // `persist_admitted_chat_display` 的 revision 守卫也会拒绝（引擎不存在 =
    // 无存活写入者，无复活窗口）。
    if let Some((pool, _)) = gates.pools {
        pool.evict_if_present(session_id).await;
    }
    let apply = tauri::async_runtime::spawn_blocking(move || {
        checkpoints::apply_restore(&apply_ledger, &apply_execution, &target)
    })
    .await;
    match apply {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            log::error!(
                "[pinvou3][checkpoints] rewind reconciliation apply failed, blocking this turn's snapshot sid={session_id}: {error:#}"
            );
            return false;
        }
        Err(error) => {
            log::error!(
                "[pinvou3][checkpoints] rewind reconciliation apply task failed, blocking this turn's snapshot sid={session_id}: {error}"
            );
            return false;
        }
    }
    let store_truncate = store.clone();
    let sid_truncate = session_id.to_string();
    let pre_restore_id = pending.pre_restore_id.clone();
    let keep_turns = pending.keep_turns;
    let truncate = tauri::async_runtime::spawn_blocking(move || {
        store_truncate.truncate_to_user_turn(&sid_truncate, keep_turns, Some(pre_restore_id))
    })
    .await;
    match truncate {
        Ok(Ok(_outcome)) => {
            // r26 M3：重放真正截断了 transcript——活引擎仍持有全量历史的内存
            // 副本，本轮 turn 结束的 persist 会把被截轮次写回磁盘（对话恢复、
            // 工作区仍回退 = r21-M1 判不可接受的分叉，且 undo 计数失配永久
            // 消失）。与手动回退成功路径同款驱逐活引擎，下一次使用从截断后的
            // 磁盘重建（SyncSession）。单元夹具无池（None）跳过，生产接线由
            // send-prelude pin 保证 gates.pools 恒为 Some。
            if let Some((pool, _)) = gates.pools {
                // r28 B1：evict_if_present——evict 的无条目臂会 invalidate 未
                // 提交预约（本条消息正是持预约进入快照前奏的），且重启恢复流
                // 的池是空的（引擎在内存、重启即清空）。存在性判定在 entries
                // 锁内（与 evict_locked 同一 chokepoint，无探针窗口）。
                pool.evict_if_present(session_id).await;
            }
        }
        Ok(Err(error)) => {
            log::error!(
                "[pinvou3][checkpoints] rewind reconciliation truncate failed, blocking this turn's snapshot sid={session_id}: {error:#}"
            );
            return false;
        }
        Err(error) => {
            log::error!(
                "[pinvou3][checkpoints] rewind reconciliation truncate task failed, blocking this turn's snapshot sid={session_id}: {error}"
            );
            return false;
        }
    }
    checkpoints::clear_pending_rewind(ledger);
    let ledger_invalidate = ledger.to_path_buf();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        invalidate_abandoned_turn_checkpoints(&ledger_invalidate, keep_turns);
    })
    .await;
    log::info!(
        "[pinvou3][checkpoints] crash-interrupted rewind auto-completed at turn snapshot sid={session_id} keep_turns={keep_turns}"
    );
    true
}

/// Turn 开始前的影子 git checkpoint 前奏，`chat` 与 `accept_plan`（均发送真实
/// 用户消息，`is_user_turn_prompt` 计数口径一致）共用。流程：崩溃对账 →
/// 执行根体积门 → turn 序号计数 → `create_checkpoint` → 失败/alias 冲突按级别
/// 记日志。失败/超预算不阻断 turn（设计 §5 降级语义）；对账未完成时例外——
/// 快照必须阻断（r19 M6），turn 本身照常发出。返回本轮成功登记的快照 id；
/// 发送失败时调用方按 id 精确作废「未成活」快照（评审 M5，见各自 Err 分支）。
pub(super) async fn create_turn_checkpoint(
    store: &SessionStore,
    session_id: &str,
    ledger: std::path::PathBuf,
    execution: std::path::PathBuf,
    label: String,
    caller: &str,
    gates: ReconcileGates<'_>,
) -> Option<String> {
    // r19 M6：快照前先对账中断的回退（收口混合态；未完成则跳过本轮快照）。
    // r23 M1：栅栏输入由命令层以真实 pool/ACP 探针预计算（ReconcileGates）。
    if !reconcile_pending_rewind(store, session_id, &ledger, &execution, gates).await {
        return None;
    }
    let store_count = store.clone();
    let sid_count = session_id.to_string();
    let snapshot = tauri::async_runtime::spawn_blocking(move || {
        // 执行根体积门（对齐底座 snapshot 机制）：超过 2GB 的目录不做快照——
        // 全量 add -A 的 IO 与影子仓库存储都不划算，该轮如实没有回退入口
        // （返回 None 表示主动跳过，与快照失败区分）。
        if !checkpoints::execution_root_within_snapshot_budget(&execution, &ledger) {
            return Ok(None);
        }
        // turn 序号用 is_user_turn_prompt 同口径计数（tool_result 不计入），
        // 计数失败（会话加载失败等）登记 None，前端按顺序兜底对齐。会话 JSON
        // 读取是阻塞 IO，与快照同驻 spawn_blocking。
        let turn_number = store_count
            .load(&sid_count)
            .map(|session| checkpoints::count_user_turns(&session.messages) + 1)
            .ok();
        checkpoints::create_checkpoint(
            &ledger,
            &execution,
            turn_number,
            checkpoints::CheckpointKind::Turn,
            &label,
        )
        .map(Some)
    })
    .await;
    match snapshot {
        Ok(Ok(Some(meta))) => Some(meta.id),
        Ok(Ok(None)) => {
            log::info!(
                "[pinvou3][{caller}] checkpoint skipped sid={session_id}: execution root over snapshot size budget or not fully readable (see earlier checkpoint estimate log)"
            );
            None
        }
        Ok(Err(error)) => {
            // git alias 冲突（大小写不敏感目录下 Makefile/makefile 共存）会让
            // 该会话每轮快照都失败、永久没有回退入口——error 级显式上报，
            // 不静默 warn（评审 M1）。
            if format!("{error:#}").contains("alias") {
                log::error!(
                    "[pinvou3][{caller}] checkpoint failed (git alias conflict, session has no rewind entries) sid={session_id}: {error:#}"
                );
            } else {
                log::warn!("[pinvou3][{caller}] checkpoint failed sid={session_id}: {error:#}")
            }
            None
        }
        Err(error) => {
            log::warn!("[pinvou3][{caller}] checkpoint task failed sid={session_id}: {error}");
            None
        }
    }
}

/// Invalidate "unsent" snapshots (any pre-send early exit; exact delete by id so
/// leftovers cannot steal the same-numbered turn's first-wins alignment anchor on
/// retry — review M5). Shared by `chat` send failure and each `accept_plan` early exit.
pub(super) async fn drop_unsent_turn_checkpoint(
    ledger: Option<std::path::PathBuf>,
    snapshot_id: Option<String>,
    session_id: &str,
    caller: &str,
) {
    if let (Some(ledger), Some(snapshot_id)) = (ledger, snapshot_id) {
        let sid = session_id.to_string();
        let caller = caller.to_string();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) =
                crate::features::code_checkpoints::drop_checkpoint(&ledger, &snapshot_id)
            {
                log::warn!(
                    "[pinvou3][{caller}] drop unsent-turn checkpoint failed sid={sid}: {error:#}"
                );
            }
        })
        .await;
    }
}

/// list_checkpoints 的响应体：列表 + 后端权威的用户 turn 计数（评审 M2 调和门）。
/// 前端用它校验「时间线序号 ↔ 后端 turn 号」映射是否成立（压缩会让后端
/// count 收缩而前端时间线保留全部气泡、前端-only 回显不计后端 turn），不一致
/// 时不渲染回退入口——把「错点回退却报成功」降级为诚实拒绝。durable 重载后
/// 时间线按收缩后的 messages 重建，两侧重新对齐、入口恢复。
// rename_all（评审 r6 M1）：本文件所有 JS 可见响应体都走 camelCase wire——
// 缺了它则 wire 携带 user_turns 而前端读 userTurns，调和门静默退化为不比对
// （r5 的实际事故，测试只注入参数、从未见过真实 wire 键）。wire 形状由
/// tests::js_facing_response_wire_shapes_are_camel_case 钉住（覆盖全部五个 JS 可见响应体）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointListForUi {
    pub checkpoints: Vec<CheckpointMeta>,
    pub user_turns: u32,
}

#[tauri::command]
pub async fn list_checkpoints(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
    store: State<'_, SessionStore>,
) -> Result<CheckpointListForUi, String> {
    // 列表只读账本根索引，不触碰执行根（执行根被删的历史会话仍可查看/清理认知）。
    if !is_native_checkpoint_lane(&session_id, &store, |id| acp_pool.is_acp(id)) {
        return Err("仅原生会话支持检查点".to_string());
    }
    let ledger = store
        .session_roots(&session_id)
        .map_err(|error| format!("解析会话根失败: {error:#}"))?
        .ledger;
    // r19 M8：用户 turn 计数从账本索引末条 Turn 快照派生（每轮发送前以
    // count+1 登记 → 末条 turn == 最后一个已开始轮的序号），消除每 turn 边沿
    // 2-3 次的全量 transcript 读盘+解析（大会话 0.5-2s 级后台 CPU、随长度
    // 线性、与流式渲染抢预算；main 上本列表只读 index.json）。已知 staleness
    // 窗口（消息唤醒轮后未再发送、发送失败且作废失败、压缩后未再发送）均
    // 表现为调和门诚实隐藏入口、下次发送自愈——与主车道失准的既有处理语义
    // 一致。rewind_to_turn 预检仍走全量精确计数：那是实际执行回退前的一次性
    // 校验，不在轮询热路径。
    tauri::async_runtime::spawn_blocking(move || {
        let entries = checkpoints::list_checkpoints(&ledger)
            .map_err(|error| format!("读取检查点列表失败: {error:#}"))?;
        let user_turns = entries
            .iter()
            .rev()
            .find(|entry| entry.kind == checkpoints::CheckpointKind::Turn)
            .and_then(|entry| entry.turn)
            .unwrap_or(0);
        Ok::<_, String>(CheckpointListForUi {
            checkpoints: entries,
            user_turns,
        })
    })
    .await
    .map_err(|error| format!("读取检查点列表任务失败: {error}"))?
}

#[tauri::command]
pub async fn checkpoint_diff(
    session_id: String,
    checkpoint_id: String,
    pool: State<'_, EnginePool>,
    acp_pool: State<'_, AcpPool>,
    store: State<'_, SessionStore>,
) -> Result<CheckpointDiff, String> {
    let (ledger, execution) =
        resolve_native_session_roots(&session_id, &store, |id| acp_pool.is_acp(id))?;
    // r33 minor 11：aux 会话拒绝——diff 的 ensure_repo 会在 aux 私有目录
    // materialize 影子仓库（一行拒绝，与破坏性命令族同口径）。
    if crate::features::sessions::is_aux_session_id(&session_id) {
        return Err("辅助会话不支持检查点差异预览".to_string());
    }
    // 软忙碌门：diff 会先 add -A 写影子 index，turn 进行中会与引擎写文件及
    // create_checkpoint 抢同一 index.lock（偶发失败或抓到写了一半的中间态），
    // 如实拒绝让前端稍后重试（确认弹窗只在空闲时可开，竞态窗口在点击之后）。
    if pool.is_turn_active(&session_id) {
        return Err("会话正在执行，请稍后再读取变更预览".to_string());
    }
    // r22 R2：压缩在飞时预览读到的是「压缩中」的中间态（forwarder 尚未落盘
    // 压缩转录），拒绝让前端稍后重试（评审标注 arguably；与上游闸同向）。
    if pool.is_compaction_in_flight(&session_id) {
        return Err("会话正在压缩对话，请稍候再读取变更预览".to_string());
    }
    // 跨会话软门：同执行根的其它会话在跑时，本 diff 的 add -A 会读到对方
    // 引擎写了一半的文件（影子仓库按会话分目录，peer 间不共享 index.lock——
    // 门的理由是共享执行根的中间态，不是锁竞争）。
    reject_if_peer_busy(
        &store,
        &pool,
        &acp_pool,
        &session_id,
        &execution,
        "请稍后再读取变更预览",
    )
    .await?;
    tauri::async_runtime::spawn_blocking(move || {
        // `false`: the webview renders only the `changes` list (RewindChip);
        // generating the unified diff here would cost a full `git diff` per
        // preview open for bytes nothing reads. The patch surface is the
        // CLI's, whose caller passes `true`.
        checkpoints::diff_checkpoint(&ledger, &execution, &checkpoint_id, false)
            .map_err(|error| format!("读取检查点差异失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("读取检查点差异任务失败: {error}"))?
}

/// `rewind_to_turn` 的编排结果（设计 §4：供前端刷新与提示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewindToTurnResult {
    /// 恢复代码时自动打的 PreRestore 回滚点（与 feature 层
    /// `checkpoints::restore_checkpoint` 返回值同语义，
    /// 供「已回退、可反悔」提示）；降级（仅回退对话）时为 None。注：undo 的绑定
    /// 载体是 sidecar 记录（`RewoundTurnsRecord.pre_restore_checkpoint_id`），本
    /// 返回值仅作展示，不参与反悔链路。
    pub restored_checkpoint: Option<CheckpointMeta>,
    /// 被截掉的用户 turn 数。
    pub rewound_turns: u32,
    /// true = 本次只截断了对话、代码未回退（调用方选择 conversation_only，通常
    /// 因目标 turn 的 checkpoint 不存在——LRU 淘汰/当时快照失败），前端文案必须明示。
    pub degraded: bool,
    /// true = 持久化的 system_prompt 含 compaction 摘要残留（截断不触碰
    /// system_prompt；前端据此提示「回退位置之前发生过上下文压缩」）。
    pub had_compaction: bool,
}

/// 回退计划：`conversation_only=true` 时恒为仅对话降级（绝不动代码）；否则定位
/// turn N+1 的 Turn 快照，entries 按创建顺序升序，`find` 命中即同 turn 先创建者；
/// 快照不存在时如实报错。
#[derive(Debug)]
struct RewindPlan {
    checkpoint: Option<CheckpointMeta>,
    degraded: bool,
}

fn resolve_rewind_plan(
    entries: Vec<CheckpointMeta>,
    keep_turns: u32,
    conversation_only: bool,
) -> Result<RewindPlan, String> {
    // 「回退到第 N 轮」= 恢复第 N+1 轮写入之前的快照；N=0 = 恢复第 1 轮快照。
    let target_turn = keep_turns + 1;
    let checkpoint = entries
        .into_iter()
        .find(|entry| entry.kind == CheckpointKind::Turn && entry.turn == Some(target_turn));
    // conversation_only 优先：调用方明确仅回退对话（前端「无可用快照」变体），
    // 即便快照存在也绝不动代码——用户确认弹窗上看到的是「代码保持不变」。
    if conversation_only {
        return Ok(RewindPlan {
            checkpoint: None,
            degraded: true,
        });
    }
    match checkpoint {
        Some(checkpoint) => Ok(RewindPlan {
            checkpoint: Some(checkpoint),
            degraded: false,
        }),
        None => Err(format!(
            "第 {target_turn} 轮的检查点不存在（可能已被淘汰或当时快照失败）；仅回退对话请使用 conversation_only"
        )),
    }
}

fn normalize_root(path: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// apply_restore 前的 ACP peer 最后一刻复检（评审 r7 M2 引入于 rewind 侧；
/// r11 M3 提取为共用函数，undo 侧同款调用——正门只拦「检查时已在途」的 ACP
/// 写入，预约→apply 之间放入的写入由本复检兜底，残余 check-then-act 窗口
/// （复检返回→apply 启动）在 §12 如实披露）。
async fn reject_if_acp_peer_busy_before_apply(
    store: &SessionStore,
    acp_pool: &AcpPool,
    session_id: &str,
    execution: &std::path::Path,
    action: &str,
) -> Result<(), String> {
    let recheck_store = store.clone();
    let recheck_session = session_id.to_string();
    let recheck_execution = execution.to_path_buf();
    let recheck_agents = acp_pool.agents().clone();
    let recheck_store2 = store.clone();
    let recheck_peers = tauri::async_runtime::spawn_blocking(move || {
        peers_on_same_execution_root(
            &recheck_store,
            &recheck_session,
            &recheck_execution,
            production_execution_root_resolver(recheck_agents, recheck_store2),
        )
    })
    .await
    .map_err(|error| format!("执行前 ACP 复检任务失败: {error}"))??;
    let recheck_ids: Vec<String> = recheck_peers
        .iter()
        .map(|(id, _)| id.as_str())
        .filter(|id| acp_pool.is_acp(id))
        .map(str::to_string)
        .collect();
    if !recheck_ids.is_empty() {
        match acp_pool.rebind_blocking_sessions(&recheck_ids).await {
            None => {
                return Err(format!(
                    "同目录 ACP 会话的忙碌状态暂时不可读（并发冷启动握手中），{action}"
                ));
            }
            Some(blocking) => {
                if let Some(blocked_id) = blocking.first() {
                    let label = recheck_peers
                        .iter()
                        .find(|(id, _)| id == blocked_id)
                        .map(|(_, label)| label.clone())
                        .unwrap_or_else(|| blocked_id.clone());
                    return Err(format!(
                        "会话「{label}」绑定同一项目目录且正在执行，{action}"
                    ));
                }
            }
        }
    }
    Ok(())
}
/// 跨会话忙碌门的 async 包装（diff / rewind / undo 三处共用的 State clone +
/// spawn_blocking + map_err 编排）：在阻塞线程上执行
/// [`busy_peer_on_same_execution_root`]，任一同执行根 peer 在跑时返回带上下文
/// 的错误（`action` 为各调用方的动作文案，如「请先停止该会话再回退」）。
///
/// ACP 腿（评审 M4）：ACP 会话的 turn 不经 EnginePool（外部 agent 进程），
/// 原生闭包对其恒 false——补一条 ACP 专用检查：同执行根的 ACP 会话中任一
/// 处于忙碌（busy/configuring/prompt_pending，与 rebind fence 共用
/// [`AcpPool::rebind_blocking_sessions`] 的谓词）即拒绝。回退的 force-checkout
/// + clean -fd 比 rebind 更具破坏性，二者必须同门。状态暂不可读（冷启动握手
/// 持锁，fence 返回 None）时如实拒绝，不静默放行。
async fn reject_if_peer_busy(
    store: &SessionStore,
    pool: &EnginePool,
    acp_pool: &AcpPool,
    session_id: &str,
    execution: &std::path::Path,
    action: &str,
) -> Result<(), String> {
    let store_gate = store.clone();
    let session_id_gate = session_id.to_string();
    let execution_gate = execution.to_path_buf();
    let pool_gate = pool.clone();
    let acp_store = store.clone();
    let acp_session = session_id.to_string();
    let acp_execution = execution.to_path_buf();
    // 生产根解析（评审 r6 M3）：先查 ACP 项目绑定（agents store 的
    // acp_project_workspace——ACP 会话 mode 恒 Plain，session_roots/code_
    // project_workspace 都解析不到其真实项目目录），再回退 session_roots。
    let agents_gate = acp_pool.agents().clone();
    let agents_peers = acp_pool.agents().clone();
    let (busy, acp_peers) = tauri::async_runtime::spawn_blocking(move || {
        let busy = busy_peer_on_same_execution_root(
            &store_gate,
            &session_id_gate,
            &execution_gate,
            production_execution_root_resolver(agents_gate, store_gate.clone()),
            |id| pool_gate.is_turn_active(id) || pool_gate.is_scheduled_turn_running(id),
        )?;
        let acp_peers_store = acp_store.clone();
        let peers = peers_on_same_execution_root(
            &acp_store,
            &acp_session,
            &acp_execution,
            production_execution_root_resolver(agents_peers, acp_peers_store),
        )?;
        Ok::<_, String>((busy, peers))
    })
    .await
    .map_err(|error| format!("跨会话忙碌检查任务失败: {error}"))??;
    if let Some(busy) = busy {
        return Err(format!(
            "会话「{busy}」绑定同一项目目录且正在执行，{action}"
        ));
    }
    let acp_peer_ids: Vec<String> = acp_peers
        .iter()
        .map(|(id, _)| id.as_str())
        .filter(|id| acp_pool.is_acp(id))
        .map(str::to_string)
        .collect();
    if !acp_peer_ids.is_empty() {
        match acp_pool.rebind_blocking_sessions(&acp_peer_ids).await {
            None => {
                return Err(format!(
                    "同目录 ACP 会话的忙碌状态暂时不可读（并发冷启动握手中），{action}"
                ));
            }
            Some(blocking) => {
                if let Some(blocked_id) = blocking.first() {
                    let label = acp_peers
                        .iter()
                        .find(|(id, _)| id == blocked_id)
                        .map(|(_, label)| label.clone())
                        .unwrap_or_else(|| blocked_id.clone());
                    return Err(format!(
                        "会话「{label}」绑定同一项目目录且正在执行，{action}"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// 同执行根 peer 枚举（不含自身与 aux、跳过根解析失败项），返回 (id, 展示名)。
/// 原生忙碌门与 ACP 忙碌门（评审 M4）共用：前者在
/// `busy_peer_on_same_execution_root` 内联判定，后者由调用方对 ACP 子集走
/// `AcpPool::rebind_blocking_sessions`。
///
/// 根解析（评审 r6 M3）：`resolve_execution` 闭包必须先查 ACP 项目绑定
/// （`SessionAgentStore::acp_project_workspace`——ACP 会话 mode 恒 Plain，
/// `code_project_workspace`/`session_roots` 对其都解析不到真实项目目录），
/// 再回退 `session_roots`。生产闭包见 `reject_if_peer_busy`；测试闭包只走
/// `session_roots`（原生语义）。
fn peers_on_same_execution_root(
    store: &SessionStore,
    session_id: &str,
    execution_root: &std::path::Path,
    resolve_execution: impl Fn(&str) -> Option<std::path::PathBuf>,
) -> Result<Vec<(String, String)>, String> {
    let target = normalize_root(execution_root);
    let sessions = store
        .list_sessions_cached()
        .map_err(|error| format!("枚举会话失败: {error:#}"))?;
    let mut peers = Vec::new();
    for metadata in sessions.iter() {
        if metadata.id == session_id {
            continue;
        }
        // Aux side-chat sessions have no writer surface by construction
        // (PR #433; ADR-0024 leaves them at most one scoped READ-only tool):
        // they can never write files, so the gate's premise — a peer engine
        // leaving half-written files in the shared execution root — is
        // structurally false for them. In production the exclusion is defense-in-depth:
        // `SessionStore::session_roots` resolves an aux id to its private
        // `sessions/aux-<id>/workspace` (no codex record, no workspace-binding
        // sidecar is ever written for aux), so an aux root can never equal
        // the parent's. The skip keeps the gate correct against any future
        // resolver change that would surface a shared root for aux records.
        if crate::features::sessions::is_aux_session_id(&metadata.id) {
            continue;
        }
        let Some(execution) = resolve_execution(&metadata.id) else {
            // 根解析失败的会话（如目录被删）无从比较执行根，跳过。
            continue;
        };
        if normalize_root(&execution) == target {
            let label = if metadata.title.is_empty() {
                metadata.id.clone()
            } else {
                metadata.title.clone()
            };
            peers.push((metadata.id.clone(), label));
        }
    }
    Ok(peers)
}

/// 生产根解析（评审 r7 NIT：此前两处闭包 + 测试内一处手写副本，形状漂移无
/// 锚）：ACP 项目绑定（agents store 的 `acp_project_workspace`——ACP 会话
/// mode 恒 Plain，`session_roots`/`code_project_workspace` 都解析不到其真实
/// 项目目录）优先，`session_roots` 兜底。调用点（正门两条腿、apply 前复检）
/// 与端到端测试共用本函数，测试因此测的就是生产形状本身。
fn production_execution_root_resolver(
    agents: crate::features::codex_acp::SessionAgentStore,
    store: SessionStore,
) -> impl Fn(&str) -> Option<std::path::PathBuf> {
    move |id| {
        agents
            .acp_project_workspace(id)
            .or_else(|| store.session_roots(id).ok().map(|roots| roots.execution))
    }
}

/// 跨会话忙碌门（设计 §6 防线二）：枚举全部会话（`list_sessions_cached`，含
/// scheduled 的 sched- 前缀会话——`store.list()` 会把它们滤掉，那是确定性
/// 漏判），找出 execution 根相同的其它会话，任一 `is_busy` 即返回其展示名
/// （标题，空则 id）。调用方的 is_busy 应覆盖 `is_turn_active` 与
/// `is_scheduled_turn_running`（scheduled 轮的 spawn→submit 窗口）。
///
/// 调用方必须先在 `begin_execution_root_rewind` 置位互斥 flag 再调本函数：
/// 在途 peer 由本函数复查拦下，新 peer 由 flag 门在预约侧拦下——**原生与
/// scheduled 车道**两侧在同一把 flag 锁上做 check-and-act，无竞态。**ACP 车
/// 道例外（评审 r7 M2）**：ACP prompt 的 admit 不经 EnginePool、不读该 flag，
/// 正门只能拦「检查时已在途」的 ACP 写入；预约→apply 之间放入的 ACP 写入由
/// rewind_to_turn 的 apply 前复检兜底，残余窗口（复检返回→apply 启动）在
/// 设计 §12 如实披露。
///
/// ACP 会话：本函数的 is_busy 闭包只覆盖 EnginePool（原生车道）；ACP turn 不
/// 经 EnginePool，其忙碌由 [`reject_if_peer_busy`] 的 ACP 腿
/// （`rebind_blocking_sessions` 同谓词）补齐——单测直接调本函数时不含 ACP 判定。
fn busy_peer_on_same_execution_root(
    store: &SessionStore,
    session_id: &str,
    execution_root: &std::path::Path,
    resolve_execution: impl Fn(&str) -> Option<std::path::PathBuf>,
    is_busy: impl Fn(&str) -> bool,
) -> Result<Option<String>, String> {
    for (id, label) in
        peers_on_same_execution_root(store, session_id, execution_root, resolve_execution)?
    {
        if is_busy(&id) {
            return Ok(Some(label));
        }
    }
    Ok(None)
}

/// 回退成功后作废被截对话分支的 Turn checkpoint（设计审阅 P0 修复，机制见
/// [`checkpoints::invalidate_turn_checkpoints_after`]）。
///
/// 时机：restore + 对话截断都成功之后。清理性质——失败只如实记日志，不让整个
/// 回退失败（恢复与截断已生效，作废只是防止旧分支快照遮蔽新分支）。
/// （独立的 restore_checkpoint IPC 命令已移除，此处仅 rewind/undo 编排调用。）
fn invalidate_abandoned_turn_checkpoints(ledger: &std::path::Path, keep_turns: u32) {
    match checkpoints::invalidate_turn_checkpoints_after(ledger, keep_turns) {
        Ok(_) => {}
        Err(error) => eprintln!(
            "[checkpoints] 作废被截分支 checkpoint 失败（回退已生效，仅清理未做）: {error:#}"
        ),
    }
}

/// 回退到第 `keep_turns` 轮末尾：恢复第 N+1 轮 checkpoint + 对话截断到第 N 轮
/// + 回收 engine（下次发送走既有 lazy respawn + `Op::SyncSession` 用截断后的
/// messages 重新注水）。编排顺序按设计 §4。
#[tauri::command]
pub async fn rewind_to_turn(
    session_id: String,
    keep_turns: u32,
    conversation_only: bool,
    pool: State<'_, EnginePool>,
    acp_pool: State<'_, AcpPool>,
    store: State<'_, SessionStore>,
) -> Result<RewindToTurnResult, String> {
    let (ledger, execution) =
        resolve_native_session_roots(&session_id, &store, |id| acp_pool.is_acp(id))?;
    // r29 minor 3：破坏性命令族显式排除 aux 会话——文件头「仅历史查询过门」
    // 的旧声明与实际不符；无快照的 aux 会话本无回退面，raw IPC 直调也不得
    // 经 truncate_to_user_turn 改写其 transcript。
    if crate::features::sessions::is_aux_session_id(&session_id) {
        return Err("辅助会话不支持检查点回退".to_string());
    }
    // 本会话忙碌门：同 restore_checkpoint，先快速检查给出友好文案，再原子占位；
    // 预约持有到编排结束（未提交，Drop 自动归还 slot）。
    if pool.is_turn_active(&session_id) {
        return Err("会话正在执行，请先停止当前任务再回退".to_string());
    }
    // r22 R2：压缩不持 turn 预约（engine_pool 文档明示破坏性栅栏必须显式
    // 询问；projects.rs 的 rebind 忙门同款）。compact_now 在飞时放行回退，
    // forwarder 随后的压缩转录 persist 会整体覆写 state.messages，把已回退
    // 的轮次复活、undo 又被 revision 失配永久关闭——正是 r21 M1 判不可接受
    // 的分叉类。
    if pool.is_compaction_in_flight(&session_id) {
        return Err("会话正在压缩对话，请稍候再回退".to_string());
    }
    let reservation = pool
        .reserve_turn(&session_id)
        .map_err(|error| format!("预约会话 turn 失败（会话忙碌？）: {error:#}"))?;
    // 执行根互斥：check-and-set 置位后同根会话的新 turn 预约（含 scheduled）
    // 全部被拒；另一个回退已在飞时如实拒绝（胜者优先，败者不持 Guard）。
    let _root_guard = pool.begin_execution_root_rewind(&execution)?;
    // r35 MAJOR-1：将引擎驱逐提前到 restore 之前——否则引擎在 restore/截断
    // 窗口内的空闲 persist（MCP 启动失败注释等 idle append）会把回退前
    // transcript 整体写回磁盘（对话复活、工作区仍回退 = 不可接受分叉）。
    // 驱逐后下次 get_or_spawn 未命中 → lazy respawn → 从截断后的磁盘注入。
    // 如果 restore/截断失败，引擎已不存在但下次使用时会从磁盘重建（安全）。
    pool.evict(&session_id).await;
    // 跨会话忙碌门：恢复单位是执行根，同根其它会话在跑时回退会撤销它正在写的
    // 文件，如实拒绝并告知哪个会话在忙。全量枚举（含 scheduled）是阻塞 IO，
    // 移出 async worker。
    reject_if_peer_busy(
        &store,
        &pool,
        &acp_pool,
        &session_id,
        &execution,
        "请先停止该会话再回退",
    )
    .await?;

    // 定位 checkpoint + 截断可行性预检。必须在 restore 之前：先恢复代码才发现
    // 对话无可截内容，会留下「代码已回退、对话未动」的不一致状态。
    let store_snapshot = store.inner().clone();
    let ledger_snapshot = ledger.clone();
    let session_id_snapshot = session_id.clone();
    let (entries, total_turns, transcript_revision) = tauri::async_runtime::spawn_blocking(move || {
        // 陈旧 Turn 快照和解（评审 finding）：历史回退的作废步骤失败或进程在
        // 截断落盘后、作废前崩溃时，被截分支的 Turn 快照会残留，并在 first-wins
        // 下抢占新分支同号 turn 的对齐锚（再次回退静默恢复出被弃分支的代码）。
        // sidecar 备份先于截断落盘，其记录是可靠凭据：`turn > kept_turns` 且
        // 创建于该次回退之前的快照必属被弃分支（新分支同号快照创建于回退后）。
        // best-effort：和解失败不阻塞回退，如实记日志（与即时作废同级语义）。
        // r26 M2：活暂记在飞时跳过本轮陈旧和解——sidecar 记录写于截断持久化
        // 之前，persist 分裂窗内其记录并非「截断已发生」的凭据；按记录失效会
        // 删掉暂记自己的目标快照，把错误信息承诺的引导重打（P1 复用）变成
        // checkpoint_missing，并把暂记楔死在 evicted 态（混合树固化）。暂记由
        // 对账先收口，和解留给收口后的下一次回退。
        match checkpoints::live_pending_rewind(&ledger_snapshot) {
            Ok(Some(_)) => {
                eprintln!(
                    "[checkpoints] live pending rewind present, skipping stale-Turn reconciliation this round"
                );
            }
            Err(error) => {
                eprintln!(
                    "[checkpoints] 读取回退暂记失败（仅和解未做，fail-closed）: {error:#}"
                );
            }
            Ok(None) => match store_snapshot.rewound_turns_records(&session_id_snapshot) {
            Ok(records) => {
                for record in records {
                    let cutoff = match chrono::DateTime::parse_from_rfc3339(&record.rewound_at) {
                        Ok(moment) => moment.timestamp(),
                        Err(error) => {
                            eprintln!(
                                "[checkpoints] 回退备份时间戳无法解析，跳过该条和解（{}）: {error:#}",
                                record.rewound_at
                            );
                            continue;
                        }
                    };
                    if let Err(error) = checkpoints::invalidate_stale_turn_checkpoints(
                        &ledger_snapshot,
                        record.kept_turns,
                        cutoff,
                    ) {
                        eprintln!("[checkpoints] 陈旧 checkpoint 和解失败（仅清理未做）: {error:#}");
                    }
                }
            }
                Err(error) => {
                    eprintln!("[checkpoints] 读取回退备份记录失败（仅和解未做）: {error:#}");
                }
            }
        }
        let entries = checkpoints::list_checkpoints(&ledger_snapshot)
            .map_err(|error| format!("读取检查点列表失败: {error:#}"))?;
        let session = store_snapshot
            .load(&session_id_snapshot)
            .map_err(|error| format!("读取会话失败: {error:#}"))?;
        let transcript_revision = crate::features::sessions::transcript_revision(&session.messages)
            .map_err(|error| format!("计算对话 revision 失败: {error:#}"))?;
        Ok::<_, String>((entries, checkpoints::count_user_turns(&session.messages), transcript_revision))
    })
    .await
    .map_err(|error| format!("回退预检任务失败: {error}"))??;
    if keep_turns >= total_turns {
        return Err(format!(
            "会话当前只有 {total_turns} 轮，无法回退到第 {keep_turns} 轮末尾"
        ));
    }
    let plan = resolve_rewind_plan(entries, keep_turns, conversation_only)?;

    // 1) 恢复代码（内部强制 PreRestore，失败则中止）；降级模式跳过本步。
    // 评审 r6 M2：恢复拆为 prepare（创建/复用 PreRestore，不动工作区）+ apply
    // （read-tree → purge → checkout-index → clean）两段，中间落 pending_rewind
    // 暂记——同目标（checkpoint id + keepTurns）的引导重试复用首个 PreRestore，
    // 不对混合/已回退状态新拍快照，undo 不会绑到错误状态。暂记复用前先核对
    // 「该 PreRestore 未被任何成功回退的备份记录绑定」，防成功后清理失败的
    // 陈旧暂记误复用。
    //
    // 评审 r7 M2：apply 前最后一刻复检 ACP peer。正门（reject_if_peer_busy）在
    // 预约时检查过，但 ACP prompt 的 admit 不经 EnginePool、不受执行根回退 flag
    // 拦截（围栏是单向的）——预约与 apply 之间仍可能放进同根 ACP 会话的写入。
    // 此处把窗口从「整个 apply（秒级）」压缩到「复检返回 → apply 启动」；残余
    // check-then-act 窗口如实记录（§12 与 busy_peer_on_same_execution_root 的
    // 注释已按实际保证范围改写）。
    let mut restored_checkpoint = None;
    if let Some(checkpoint) = plan.checkpoint {
        let ledger_restore = ledger.clone();
        let execution_restore = execution.clone();
        let revision_for_prepare = transcript_revision.clone();
        let (target_meta, undo) = tauri::async_runtime::spawn_blocking(move || {
            // r26 M1：与 CLI 侧（code.rs，r25 MAJOR-3）同款——不加「绑定记录
            // 否决」。matcher 的 revision 匹配本身就是提交判定器：completed
            // rewind 必然推进 revision、matcher 返回 None；能拿到 Some
            // （revision 仍匹配）的唯一绑定记录形态是 persist 分裂窗，此刻
            // 复用 P1（幂等重放）恰恰正确——否决会把 P1 孤儿化、undo 变
            // no-op，直接违背下方截断失败的重试承诺。
            let reuse = checkpoints::matching_pending_pre_restore(
                &ledger_restore,
                &checkpoint.id,
                keep_turns,
                &revision_for_prepare,
            )
            .map_err(|error| format!("读取回退暂记失败: {error:#}"))?;
            let (target_meta, undo) = checkpoints::prepare_restore(
                &ledger_restore,
                &execution_restore,
                &checkpoint.id,
                reuse.as_deref(),
            )
            .map_err(|error| format!("准备回滚检查点失败: {error:#}"))?;
            checkpoints::write_pending_rewind(
                &ledger_restore,
                &checkpoint.id,
                keep_turns,
                &undo.id,
                &revision_for_prepare,
            )
            .map_err(|error| format!("写入回退暂记失败: {error:#}"))?;
            Ok::<_, String>((target_meta, undo))
        })
        .await
        .map_err(|error| format!("准备回滚检查点任务失败: {error}"))??;
        // ACP peer 复检（r7 M2；r11 M3 提取为共用函数，undo 侧同款）。
        reject_if_acp_peer_busy_before_apply(
            &store.inner(),
            &acp_pool,
            &session_id,
            &execution,
            "请先停止该会话再回退",
        )
        .await?;
        let apply_ledger = ledger.clone();
        let apply_execution = execution.clone();
        let undo_id = undo.id.clone();
        tauri::async_runtime::spawn_blocking(move || {
            checkpoints::apply_restore(&apply_ledger, &apply_execution, &target_meta)
        })
        .await
        .map_err(|error| {
            // JoinError = 阻塞任务 panic/被取消：可能发生在 apply 中途，工作区
            // 可能已部分变更——按混合态披露，不假设「尚未启动」。
            format!(
                "回滚检查点任务异常终止: {error}。工作区可能处于混合状态（部分文件已恢复），请勿在此状态继续工作；直接重试回退会复用回滚点 {undo_id} 重新执行恢复（幂等）并完成后续截断"
            )
        })?
        .map_err(|error| {
            // 评审 M3：恢复是多步变更（read-tree → purge → checkout-index
            // → clean -fd），中途失败（ENOSPC/EACCES/AV 锁）时工作区已是
            // 目标态与回退前的混合体，且本分支尚未写 undo 记录（「撤销回
            // 退」不可用）——与截断失败分支对齐，如实披露混合态并引导
            // 重试：重试会复用回滚点 {} 重新执行恢复（幂等）并完成后续截断。
            format!(
                "回滚检查点失败: {error:#}。工作区当前可能处于混合状态（部分文件已恢复、部分仍为回退前），请勿在此状态继续工作；直接重试回退会复用回滚点 {} 重新执行恢复（幂等）并完成后续截断",
                undo.id
            )
        })?;
        restored_checkpoint = Some(undo);
    }
    // 2) 截断对话（被截段落先备份进 `_rewound_turns.json`，备份失败则中止；
    //    记录绑定步骤 1 的 PreRestore id，供 undo 精确配对）。阻塞 IO 移出
    //    async worker（会话 JSON 读 + sidecar 写 + 持久化）。
    //    注：pre_restore_id 的真实接线（restored_checkpoint.id → 记录）无单元
    //    测试锚定（State/EnginePool 无法单测构造，测试均为手工镜像编排），
    //    改动此处时需人工核对 undo 绑定链路。
    let store_truncate = store.inner().clone();
    let session_id_truncate = session_id.clone();
    let pre_restore_id = match restored_checkpoint.as_ref() {
        Some(checkpoint) => Some(checkpoint.id.clone()),
        // 评审 r7 minor 4：降级回退携带同边界失败全量回退的暂记 PreRestore——
        // 代码已在那次尝试中回退，undo 必须能恢复它（记录不得承诺「代码保持
        // 不变」而与实际不符）。无匹配暂记（真降级）保持 None 语义不变。
        None => checkpoints::pending_pre_restore_for_degraded(&ledger, &transcript_revision)
            .map_err(|error| format!("读取回退暂记失败: {error:#}"))?,
    };
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        store_truncate.truncate_to_user_turn(&session_id_truncate, keep_turns, pre_restore_id)
    })
    .await
    .map_err(|error| format!("截断对话任务失败: {error}"))?
    .map_err(|error| match &restored_checkpoint {
        // 代码已回退而对话未截断：备份未写成 → undo 记录不存在，「撤销回退」
        // 不可用；独立的 restore 入口已移除。可执行的自救只有重试回退（恢复
        // 幂等，重试会重做恢复与截断）；回滚点 id 仅作排查线索如实给出。
        Some(checkpoint) => format!(
            "代码已回退（原代码保存在回滚点 {}），但截断对话失败: {error:#}。请勿在此混合状态下继续工作；直接重试回退会复用该回滚点完成截断（不会对当前状态新拍快照）；应用内没有单独恢复代码的入口",
            checkpoint.id
        ),
        None => format!("截断对话失败: {error:#}"),
    })?;
    // 回退已走完（备份记录绑定 PreRestore）：清掉待完成暂记，同目标/其它
    // 目标的后续回退不再复用（评审 r6 M2 的暂记生命周期收口）。清理失败由
    // 匹配侧的「已绑定 + revision」双核对兜底。
    checkpoints::clear_pending_rewind(&ledger);
    // 2.5) 作废被截对话分支的 Turn checkpoint（P0 修复：turn 序号会被重新创作
    //    复用，留着旧分支快照会让 first-wins 对齐锚到被遗弃分支）。降级模式同样
    //    要作废——对话已截断，turn 复用问题与是否恢复代码无关。git 子进程 +
    //    gc 是阻塞 IO，移出 async worker。
    // 评审 r7 minor 3：作废是清理性质——JoinError（panic/取消）不得把已完整
    // 生效的回退报成失败（前端会误示错误且不刷新）。与内部非致命路径同级：
    // 如实记日志，副作用留给下一次回退的陈旧和解兜底。
    let ledger_invalidate = ledger.clone();
    if let Err(error) = tauri::async_runtime::spawn_blocking(move || {
        invalidate_abandoned_turn_checkpoints(&ledger_invalidate, keep_turns);
    })
    .await
    {
        eprintln!("[checkpoints] 作废被截分支检查点任务异常（回退已生效，仅清理未做）: {error}");
    }
    // 引擎已在 restore 前驱逐（r35 MAJOR-1），此处无需重复驱逐。
    drop(reservation);
    Ok(RewindToTurnResult {
        restored_checkpoint,
        rewound_turns: outcome.rewound_turns,
        degraded: plan.degraded,
        had_compaction: outcome.had_compaction,
    })
}

/// `rewind_undo_state` 的返回：可反悔所需的全部信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewindUndoInfo {
    /// 本次回退强制的 PreRestore checkpoint id（反悔 = 恢复到它，精确绑定自
    /// sidecar 记录，不用「最新 PreRestore」兜底——其它回退/反悔产生的回滚点
    /// 会被误配）。None = 本次回退是仅对话降级，代码未动，反悔只还原对话，
    /// 前端文案必须相应调整（不得再承诺恢复代码）。
    pub checkpoint_id: Option<String>,
    /// 当时回退保留的用户 turn 数。
    pub kept_turns: u32,
    /// 被截掉的用户 turn 数（供文案「将恢复 N 轮」）。
    pub rewound_turns: u32,
    /// 回退时间（RFC3339）。
    pub rewound_at: String,
}

/// 可反悔判定（undo 条件）：① sidecar 有回退备份记录（取最新）② 当前
/// `count_user_turns` == 记录的 kept_turns，且记录带 truncated_revision 时当前
/// revision 精确匹配（回退后没发过新轮次、尾部未被编辑；发了/改了就不再可
/// 反悔）③ 记录绑定的 PreRestore checkpoint 仍在 index 里（LRU 淘汰则代码侧
/// 不可反悔）；记录未绑定（仅对话降级）时本条件不涉及代码。①②不满足返回
/// None（如实不可反悔，不报错）。
fn resolve_rewind_undo_state(
    store: &SessionStore,
    ledger: &std::path::Path,
    session_id: &str,
) -> Result<Option<RewindUndoInfo>, String> {
    let Some(record) = store
        .latest_rewound_turns_record(session_id)
        .map_err(|error| format!("读取回退备份失败: {error:#}"))?
    else {
        return Ok(None);
    };
    let session = store
        .load(session_id)
        .map_err(|error| format!("读取会话失败: {error:#}"))?;
    if checkpoints::count_user_turns(&session.messages) != record.kept_turns {
        return Ok(None);
    }
    if !record.truncated_revision.is_empty() {
        let current_revision = crate::features::sessions::transcript_revision(&session.messages)
            .map_err(|error| format!("计算对话 revision 失败: {error:#}"))?;
        if current_revision != record.truncated_revision {
            return Ok(None);
        }
    }
    // 代码侧恢复目标：精确匹配记录绑定的 PreRestore（无归属的「最新一条」会
    // 在多次回退/反悔重试后错配到无关快照）。已绑定但被 LRU 淘汰则整体不可
    // 反悔（None，不渲染入口）——此时代码已回退而快照不可用，若只还原对话
    // 会制造代码/对话分叉，比如实拒绝更糟。
    let checkpoint_id = match &record.pre_restore_checkpoint_id {
        Some(bound_id) => {
            let entries = checkpoints::list_checkpoints(ledger)
                .map_err(|error| format!("读取检查点列表失败: {error:#}"))?;
            let still_there = entries
                .iter()
                .any(|entry| entry.kind == CheckpointKind::PreRestore && entry.id == *bound_id);
            if !still_there {
                return Ok(None);
            }
            Some(bound_id.clone())
        }
        None => None,
    };
    Ok(Some(RewindUndoInfo {
        checkpoint_id,
        kept_turns: record.kept_turns,
        rewound_turns: checkpoints::count_user_turns(&record.removed_messages),
        rewound_at: record.rewound_at,
    }))
}

/// 查询侧主体（同步，便于测试）：非原生车道（ACP/定时）如实返回 None（不报错）。
/// `is_acp` 为 ACP 谓词注入缝（生产传 `AcpPool::is_acp`，测试传替身——见
/// [`is_native_checkpoint_lane`]）。
fn rewind_undo_state_inner(
    store: &SessionStore,
    is_acp: impl Fn(&str) -> bool,
    session_id: &str,
) -> Result<Option<RewindUndoInfo>, String> {
    if !is_native_checkpoint_lane(session_id, store, is_acp) {
        return Ok(None);
    }
    let ledger = store
        .session_roots(session_id)
        .map_err(|error| format!("解析会话根失败: {error:#}"))?
        .ledger;
    resolve_rewind_undo_state(store, &ledger, session_id)
}

#[tauri::command]
pub async fn rewind_undo_state(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
    store: State<'_, SessionStore>,
) -> Result<Option<RewindUndoInfo>, String> {
    let store_inner = store.inner().clone();
    let acp_inner = acp_pool.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        rewind_undo_state_inner(&store_inner, |id| acp_inner.is_acp(id), &session_id)
    })
    .await
    .map_err(|error| format!("读取反悔状态任务失败: {error}"))?
}

/// `undo_last_rewind` 的返回。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoLastRewindResult {
    /// 恢复回 transcript 的消息条数。
    pub restored_messages: usize,
}

/// 反悔最近一次回退：恢复代码到回退时强制打的 PreRestore 快照 + 把被截对话
/// 追加回 transcript + 回收 engine（下次发送 lazy respawn + SyncSession 重注水）。
#[tauri::command]
pub async fn undo_last_rewind(
    session_id: String,
    pool: State<'_, EnginePool>,
    acp_pool: State<'_, AcpPool>,
    store: State<'_, SessionStore>,
) -> Result<UndoLastRewindResult, String> {
    let (ledger, execution) =
        resolve_native_session_roots(&session_id, &store, |id| acp_pool.is_acp(id))?;
    // r29 minor 3：与 rewind_to_turn 同款 aux 排除（破坏性命令族显式拒绝，
    // 文件头声明恢复真实）。
    if crate::features::sessions::is_aux_session_id(&session_id) {
        return Err("辅助会话不支持检查点反悔".to_string());
    }
    // 忙碌门与 rewind_to_turn 同款：本会话原子占位 + 同执行根跨会话拒绝。
    if pool.is_turn_active(&session_id) {
        return Err("会话正在执行，请先停止当前任务再反悔".to_string());
    }
    // r22 R2：与 rewind_to_turn 同款压缩栅栏——undo 的 truncate 同样会被
    // forwarder 的压缩 persist 覆写分叉。
    if pool.is_compaction_in_flight(&session_id) {
        return Err("会话正在压缩对话，请稍候再反悔".to_string());
    }
    let reservation = pool
        .reserve_turn(&session_id)
        .map_err(|error| format!("预约会话 turn 失败（会话忙碌？）: {error:#}"))?;
    // 执行根互斥：与 rewind_to_turn 同款（check-and-set 置位 + 在途 peer 复查）。
    let _root_guard = pool.begin_execution_root_rewind(&execution)?;
    reject_if_peer_busy(
        &store,
        &pool,
        &acp_pool,
        &session_id,
        &execution,
        "请先停止该会话再反悔",
    )
    .await?;

    // 预检先于 restore：可反悔条件全部满足才动代码，把「代码已反悔、对话未
    // 反悔」的窗口压到最小（restore_rewound_turns 落盘前还会在 mutation 锁内
    // 复核 turn 数 + revision 双条件）。
    let store_probe = store.inner().clone();
    let ledger_probe = ledger.clone();
    let session_id_probe = session_id.clone();
    let undo_info = tauri::async_runtime::spawn_blocking(move || {
        resolve_rewind_undo_state(&store_probe, &ledger_probe, &session_id_probe)
    })
    .await
    .map_err(|error| format!("反悔预检任务失败: {error}"))??
    .ok_or_else(|| "没有可反悔的回退（未回退过、回退后已产生新轮次/对话被编辑，或绑定的回滚点已被最近 20 个检查点淘汰）".to_string())?;

    // 1) 恢复代码到本次回退绑定的 PreRestore（仅对话降级的回退无绑定，跳过
    //    本步——degraded 语义承诺绝不动代码，也不做 ACP 复检）。对称语义：
    //    restore 内部会再打一次 PreRestore（内容为回退后的状态），「反悔的反悔」
    //    仍可恢复；该新回滚点不影响本 undo 的目标选择（绑定 id 已在 undo_info
    //    里，重试不漂移）。
    // r12 M1：与 rewind 侧同款拆开——prepare（打 PreRestore 快照，非破坏，秒级）
    //    → ACP 复检 → apply（read-tree → checkout → clean -fd，破坏）。r11 的
    //    摆放把复检放在整个组合之前，正门后放入的同根 ACP 写入会在 PreRestore
    //    快照的秒级窗口内被拍进快照并被 apply 摧毁。
    if let Some(checkpoint_id) = undo_info.checkpoint_id.clone() {
        let ledger_prepare = ledger.clone();
        let execution_prepare = execution.clone();
        let id_for_prepare = checkpoint_id.clone();
        let (target_meta, _new_pre_restore) = tauri::async_runtime::spawn_blocking(move || {
            checkpoints::prepare_restore(&ledger_prepare, &execution_prepare, &id_for_prepare, None)
        })
        .await
        .map_err(|error| format!("准备恢复回滚点任务失败: {error}"))?
        .map_err(|error| format!("准备恢复回滚点失败: {error:#}"))?;
        // apply 前 ACP 复检（r11 M3 引入；r12 M1 后移到 prepare 与 apply 之间，
        // 与 rewind 侧完全同位）。busy peer 用可操作文案（r12 M1：统一「稍后重试」
        // 对正在执行的 peer 只会一直失败）。
        reject_if_acp_peer_busy_before_apply(
            &store.inner(),
            &acp_pool,
            &session_id,
            &execution,
            "请先停止该会话再撤销",
        )
        .await?;
        let ledger_apply = ledger.clone();
        let execution_apply = execution.clone();
        tauri::async_runtime::spawn_blocking(move || {
            checkpoints::apply_restore(&ledger_apply, &execution_apply, &target_meta)
        })
        .await
        .map_err(|error| {
            format!("恢复回滚点任务失败: {error}。工作区可能处于混合状态（部分文件已恢复），请勿在此状态继续工作；直接重试撤销可重新执行恢复（幂等，目标回滚点不变）")
        })?
        .map_err(|error| {
            // 评审 r6 minor 5：与 rewind 侧同款的混合态披露——恢复中途失败时
            // 工作区可能部分恢复；重试安全（恢复幂等且绑定 id 固定在 undo_info
            // 里，不因重打 PreRestore 漂移）。
            format!(
                "恢复回滚点失败: {error:#}。工作区当前可能处于混合状态（部分文件已恢复），请勿在此状态继续工作；直接重试撤销可重新执行恢复（幂等，目标回滚点不变）"
            )
        })?;
    }
    // 2) 恢复对话：被截消息追加回 transcript 尾部并删 sidecar 记录（阻塞 IO
    //    移出 async worker）。此步失败 = 代码已反悔而对话未反悔——如实报错
    //    并留可诊断日志（含回滚点 id 供人工对齐）；记录未消费，重试仍会恢复
    //    到同一个绑定回滚点，不会因步骤 1 新打的 PreRestore 而漂移。
    let store_restore = store.inner().clone();
    let session_id_restore = session_id.clone();
    let restored_messages = tauri::async_runtime::spawn_blocking(move || {
        store_restore.restore_rewound_turns(&session_id_restore)
    })
    .await
    .map_err(|error| format!("恢复对话任务失败: {error}"))?
    .map_err(|error| {
        // 区分可重试与条件已破（评审 M7）：revision/turn 数不匹配（「不可反悔」）
        // 时重试必然再败，如实告知记录仍在、需人工处理，不把用户引向无效重试；
        // IO 类失败（记录未消费）才可安全重试。
        let detail = format!("{error:#}");
        let condition_broken = detail.contains("不可反悔");
        match (&undo_info.checkpoint_id, condition_broken) {
            (Some(checkpoint_id), true) => {
                eprintln!(
                    "[checkpoints] undo_last_rewind conversation restore failed (condition broken) for {session_id} after code restore to {checkpoint_id}: {detail}"
                );
                format!(
                    "代码已恢复到回滚点 {checkpoint_id}，但对话恢复失败且不可重试（{detail}）；被截对话仍保留在回退备份中，需人工处理"
                )
            }
            (Some(checkpoint_id), false) => {
                eprintln!(
                    "[checkpoints] undo_last_rewind conversation restore failed for {session_id} after code restore to {checkpoint_id}: {detail}"
                );
                format!(
                    "代码已恢复到回滚点 {checkpoint_id}，但对话恢复失败（可重试；记录未消费）: {detail}"
                )
            }
            (None, true) => format!("对话恢复失败且不可重试（{detail}）；被截对话仍保留在回退备份中，需人工处理"),
            (None, false) => format!("对话恢复失败: {detail}"),
        }
    })?;
    // 3) 回收 engine（同 rewind：下次发送 lazy respawn + SyncSession 重注水）。
    // undo 成功 = 会话状态已变，任何在途回退暂记一律作废（评审 r6 M2 暂记
    // 生命周期：rev 会话 revision 已不匹配兜底，这里显式清理收口）。
    checkpoints::clear_pending_rewind(&ledger);
    pool.evict(&session_id).await;
    drop(reservation);
    Ok(UndoLastRewindResult { restored_messages })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    // Message came from the commands prelude while save_session_messages was
    // alive; the prelude re-export died with it, so the test helpers import
    // it directly.
    use deepseek_tui::models::Message;
    // git 可用性探测与 feature 层测试共用（features/code_checkpoints 内唯一实现）。
    use checkpoints::git_available;

    fn production_body<'a>(src: &'a str, signature: &str) -> &'a str {
        let start = src.find(signature).expect("production fn must exist");
        let rest = &src[start + signature.len()..];
        // End at the next fn ITEM, not the next `pub ` token (engine_pool.rs
        // review #463 round-17 SF-3): a `pub(crate)` item would otherwise
        // extend the span past the function under test.
        let end = [
            "\npub ",
            "\npub(crate) ",
            "\npub(super) ",
            "\nfn ",
            "\nasync fn ",
            "\n///",
        ]
        .iter()
        .filter_map(|marker| rest.find(marker))
        .min()
        .map_or(src.len(), |offset| start + signature.len() + offset);
        &src[start..end]
    }

    /// r22 R2 pin（production_body 模式，engine_pool.rs 同款）：三个破坏性/
    /// 预览入口的命令体必须真的询问压缩闩——压缩不持 turn 预约，turn-active
    /// 探针对它全盲；删掉任意一处栅栏即红（接线 pin，非仅判定体 pin）。
    #[test]
    fn destructive_checkpoint_gates_ask_the_compaction_latch() {
        let src = include_str!("checkpoints.rs");
        // Cut at the test module so the probe's own strings can never satisfy
        // the contains-assertions (engine_pool.rs round-26 MAJOR-3 pattern).
        let src = &src[..src
            .find("#[cfg(test)]\nmod tests {")
            .expect("test module marker must exist")];
        for (signature, verb) in [
            ("pub async fn rewind_to_turn(", "回退"),
            ("pub async fn undo_last_rewind(", "反悔"),
            ("pub async fn checkpoint_diff(", "读取变更预览"),
        ] {
            let body = production_body(src, signature);
            assert!(
                body.contains("is_compaction_in_flight(&session_id)"),
                "{signature} 必须在压缩在飞时拒绝（r22 R2）"
            );
            assert!(
                body.contains(&format!("请稍候再{verb}")),
                "{signature} 必须携带 {verb} 的压缩文案"
            );
        }
        // r24 M1：对端探针失败必须 fail-closed 按忙碌顺延（翻转为 unwrap_or
        // (false) 会让探针故障期的重放畅通无阻）。
        assert!(
            src.contains(".unwrap_or(true)"),
            "same_root_replay_busy 的探针失败必须 fail-closed 顺延（r24 M1）"
        );
    }

    /// r25 MAJOR-1 pin：重放臂必须先置执行根互斥标志、后探对端忙碌
    /// （flag-then-check，busy_peer_on_same_execution_root 的契约；顺序颠倒
    /// 留下「探针通过 → 置位前」窗口，同根原生对端的 reserve_turn 会被放行、
    /// 其在飞写入被重放的 checkout-index/clean 清掉）。变异实证：交换两块
    /// 顺序或将 begin_execution_root_rewind 整块删除，必须在此变红。
    #[test]
    fn replay_arm_takes_the_root_flag_before_probing_peers() {
        let src = include_str!("checkpoints.rs");
        let src = &src[..src
            .find(
                "#[cfg(test)]
mod tests {",
            )
            .expect("test module marker must exist")];
        let fn_start = src
            .find("async fn reconcile_pending_rewind(")
            .expect("reconcile fn must exist");
        let body = &src[fn_start..];
        // r26 minor 11：span 收紧到 reconcile 函数体（截到下一个 fn 条目）——
        // 首次出现式搜索会被函数之前的注释或其它块静默满足。
        let body_end = ["\npub ", "\npub(super) ", "\nasync fn ", "\nfn "]
            .iter()
            .filter_map(|marker| body.find(marker))
            .min()
            .map_or(body.len(), |offset| offset);
        let body = &body[..body_end];
        let guard_at = body
            .find("begin_execution_root_rewind")
            .expect("replay arm must hold the execution-root flag (r24 M3)");
        let probe_at = body
            .find("same_root_replay_busy")
            .expect("replay arm must probe same-root peers (r23 M1)");
        assert!(
            guard_at < probe_at,
            "重放臂必须先置执行根互斥标志、后探对端忙碌（r25 MAJOR-1：flag-then-check）"
        );
        // r26 M3：重放截断后必须驱逐活引擎（否则引擎 persist 复活被截轮次）。
        assert!(
            body.contains("pool.evict_if_present(session_id)"),
            "重放截断后必须以 evict_if_present 驱逐活引擎（r26 M3 + r28 B1：普通 evict 的无条目臂会失效在途预约）"
        );
    }

    /// r28 M2(a) pin：rewind_to_turn 的复用判定不得查 sidecar 绑定记录——
    /// matcher 的 revision 匹配已是提交判定器（r25 MAJOR-3），绑定记录在
    /// revision 匹配时只能是 persist 分裂窗（复用恰恰正确）。复活绑定记录
    /// 否决（r26-M1 类）必须在此变红。
    #[test]
    fn rewind_reuse_does_not_consult_the_sidecar_record() {
        let src = include_str!("checkpoints.rs");
        let fn_start = src
            .find("pub async fn rewind_to_turn(")
            .expect("rewind_to_turn must exist");
        let rest = &src[fn_start..];
        let body_end = [
            "
pub ",
            "
pub(super) ",
            "
async fn ",
            "
fn ",
        ]
        .iter()
        .filter_map(|marker| rest.find(marker))
        .min()
        .map_or(rest.len(), |offset| offset);
        let body = &rest[..body_end];
        assert!(
            !body.contains("latest_rewound_turns_record"),
            "rewind_to_turn 的复用判定不得查 sidecar 绑定记录（r28 M2：否则 persist 分裂窗被否决、P1 孤儿化）"
        );
    }

    /// r34 M5 pin：破坏性命令族的 aux 显式拒绝——raw IPC 直调不得经
    /// truncate_to_user_turn 改写 aux transcript（文件头声明已按实际行为改写）。
    /// 删除任一拒绝必须变红（命令体无法单测直调，源码 pin 是本项目惯例）。
    #[test]
    fn destructive_commands_refuse_aux_sessions() {
        let src = include_str!("checkpoints.rs");
        for signature in [
            "pub async fn rewind_to_turn(",
            "pub async fn undo_last_rewind(",
        ] {
            let fn_start = src.find(signature).expect("production fn must exist");
            let rest = &src[fn_start..];
            let body_end = [
                "
pub ",
                "
pub(super) ",
                "
async fn ",
                "
fn ",
            ]
            .iter()
            .filter_map(|marker| rest.find(marker))
            .min()
            .map_or(rest.len(), |offset| offset);
            let body = &rest[..body_end];
            assert!(
                body.contains("is_aux_session_id(&session_id)"),
                "{signature} 必须显式拒绝 aux 会话（r34 M5）"
            );
            assert!(
                body.contains("辅助会话不支持检查点"),
                "{signature} 必须携带 aux 拒绝文案（r34 M5）"
            );
        }
    }

    /// r26 M2 pin：回退前奏必须先探活暂记、再做陈旧 Turn 和解
    /// （persist 分裂窗内 sidecar 记录不是「截断已发生」的凭据，按记录失效
    /// 会删掉暂记自己的目标快照、楔死错误信息承诺的引导重试）。
    #[test]
    fn rewind_prelude_probes_live_pending_before_the_stale_sweep() {
        let src = include_str!("checkpoints.rs");
        let fn_start = src
            .find("pub async fn rewind_to_turn(")
            .expect("rewind_to_turn must exist");
        let rest = &src[fn_start..];
        let body_end = [
            "
pub ",
            "
pub(super) ",
            "
async fn ",
            "
fn ",
        ]
        .iter()
        .filter_map(|marker| rest.find(marker))
        .min()
        .map_or(rest.len(), |offset| offset);
        let body = &rest[..body_end];
        let pending_at = body
            .find("live_pending_rewind(&ledger_snapshot)")
            .expect("the prelude must probe the live pending (r26 M2)");
        let sweep_at = body
            .find("rewound_turns_records(&session_id_snapshot)")
            .expect("the stale sweep must exist");
        assert!(
            pending_at < sweep_at,
            "回退前奏必须先探活暂记、再做陈旧和解（r26 M2）"
        );
    }

    /// r24 M5 pin（production 源码锚，chat.rs / interaction.rs）：两个发送
    /// 前奏的 ReconcileGates 块必须真实接线——压缩闩即时读 + pool/ACP 引用
    /// 传入 create_turn_checkpoint。r24 评审变异实证：整块换 default、硬
    /// 编码探针结果、翻转 unwrap_or 方向均全绿（"dead on the wire" 类，本
    /// PR 在 r6/r8/r9 各中过一次）——删除任何一条接线腿必须在此变红。
    #[test]
    fn send_preludes_wire_reconcile_gates_to_real_probes() {
        let chat = include_str!("chat.rs");
        assert!(
            chat.contains("let reconcile_gates = super::checkpoints::ReconcileGates {")
                && chat.contains("compaction_in_flight: pool.is_compaction_in_flight(&sid)")
                && chat.contains("pools: Some((&pool, &acp_pool))"),
            "chat() 发送前奏必须把真实压缩闩与 pool/ACP 引用接进 ReconcileGates（r24 M5）"
        );
        let interaction = include_str!("interaction.rs");
        assert!(
            interaction.contains("let reconcile_gates = super::checkpoints::ReconcileGates {")
                && interaction
                    .contains("compaction_in_flight: pool.is_compaction_in_flight(&session_id)")
                && interaction.contains("pools: Some((&pool, &acp_pool))"),
            "accept_plan() 发送前奏必须把真实压缩闩与 pool/ACP 引用接进 ReconcileGates（r24 M5）"
        );
    }

    /// wire 形状 pin（评审 r6 M1；r7 M3 扩展到全部 JS 可见响应体）：事故类是
    /// 「结构体缺 rename_all → wire 携带 snake_case 键而前端读 camelCase，
    /// 功能静默失效」——r5 的 list_checkpoints 实际发生过（userTurns 恒
    /// undefined，调和门从未生效）。逐结构断言 camelCase 键存在且 snake_case
    /// 键不存在：任何一个结构丢 rename_all 都必须让本 pin 变红。
    #[test]
    fn js_facing_response_wire_shapes_are_camel_case() {
        let sample_meta = meta("c1-1", Some(1), CheckpointKind::Turn);
        let list = serde_json::to_value(CheckpointListForUi {
            checkpoints: vec![sample_meta.clone()],
            user_turns: 3,
        })
        .expect("serialize CheckpointListForUi");
        assert!(
            list.get("userTurns").is_some(),
            "list wire must carry userTurns: {list}"
        );
        assert!(
            list.get("user_turns").is_none(),
            "list wire must not carry user_turns: {list}"
        );
        assert!(
            list["checkpoints"][0].get("createdAt").is_some(),
            "CheckpointMeta wire must carry createdAt: {}",
            list["checkpoints"][0]
        );
        assert!(
            list["checkpoints"][0].get("created_at").is_none(),
            "CheckpointMeta wire must not carry created_at"
        );

        let rewind = serde_json::to_value(RewindToTurnResult {
            restored_checkpoint: Some(sample_meta.clone()),
            rewound_turns: 2,
            degraded: false,
            had_compaction: true,
        })
        .expect("serialize RewindToTurnResult");
        for key in ["restoredCheckpoint", "rewoundTurns", "hadCompaction"] {
            assert!(
                rewind.get(key).is_some(),
                "RewindToTurnResult wire must carry {key}: {rewind}"
            );
        }
        assert!(
            rewind.get("rewound_turns").is_none(),
            "RewindToTurnResult must not carry snake_case"
        );

        let undo_info = serde_json::to_value(RewindUndoInfo {
            checkpoint_id: Some("pre-1".into()),
            kept_turns: 1,
            rewound_turns: 2,
            rewound_at: "2026-10-13T00:00:00Z".into(),
        })
        .expect("serialize RewindUndoInfo");
        for key in ["checkpointId", "keptTurns", "rewoundTurns", "rewoundAt"] {
            assert!(
                undo_info.get(key).is_some(),
                "RewindUndoInfo wire must carry {key}: {undo_info}"
            );
        }
        assert!(
            undo_info.get("kept_turns").is_none(),
            "RewindUndoInfo must not carry snake_case"
        );

        let undo = serde_json::to_value(UndoLastRewindResult {
            restored_messages: 5,
        })
        .expect("serialize UndoLastRewindResult");
        assert!(
            undo.get("restoredMessages").is_some(),
            "UndoLastRewindResult wire must carry restoredMessages: {undo}"
        );
        assert!(
            undo.get("restored_messages").is_none(),
            "UndoLastRewindResult must not carry snake_case"
        );
    }

    /// 测试根解析闭包：只走 session_roots（原生语义，评审 r6 M3 前的旧行为）。
    fn session_roots_resolver_for_tests<'a>(
        store: &'a SessionStore,
    ) -> impl Fn(&str) -> Option<std::path::PathBuf> + 'a {
        move |id| store.session_roots(id).ok().map(|roots| roots.execution)
    }

    /// ACP peer 根解析必须走 agents store 的项目绑定（评审 r6 M3 端到端 pin）：
    /// ACP 会话 mode 恒 Plain，session_roots 解析到会话私有目录——若忙碌门用
    /// 旧解析，同项目目录的 ACP peer 永远配不上根、门结构性失效。本测试用
    /// 生产形状的解析闭包（acp_project_workspace 优先）证明 bob 被列为 peer，
    /// 并用旧解析证明其确实看不见 bob（锚定修复的必要性）。
    #[test]
    fn acp_peer_on_project_root_is_visible_through_agent_binding() {
        let (store, _guard) = isolated_store("acp-peer-root");
        let root = std::env::temp_dir().join(format!(
            "pinvou3-peer-acp-root-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let alice = store
            .create_new("/model".into(), None, root.clone())
            .expect("create alice");
        let bob = store
            .create_new("/model".into(), None, root.join("unrelated"))
            .expect("create bob");
        let (alice_id, bob_id) = (alice.metadata.id.clone(), bob.metadata.id.clone());

        // bob 是绑定项目目录的 ACP 会话（mode=Plain + workspace_path=root）。
        let agents = crate::features::codex_acp::SessionAgentStore::in_memory_for_tests();
        agents
            .set_acp_workspace(
                &bob_id,
                crate::features::codex_acp::AgentBackend::CodexAcp,
                crate::features::codex_acp::CodexWorkspaceKind::Project,
                Some(root.clone()),
                // #484 multi-root keychain snapshot: single-root binding locks
                // the root itself as the full set.
                vec![root.clone()],
            )
            .expect("set acp workspace");

        // 生产形状解析：ACP 项目绑定优先 → bob 与 alice 同根，被列为 peer。
        // 直接复用 production_execution_root_resolver——端到端测试因此锚定的
        // 就是生产形状本身（评审 r7 NIT：不再维护测试侧手写镜像）。
        let production_peers = peers_on_same_execution_root(
            &store,
            &alice_id,
            &root,
            production_execution_root_resolver(agents, store.clone()),
        )
        .expect("peers");
        assert!(
            production_peers.iter().any(|(id, _)| id == &bob_id),
            "ACP peer bound to the same project dir must be enumerated via its agent binding"
        );

        // 旧解析（只走 session_roots）：bob 的私有目录 ≠ 项目根 → 看不见
        // （这正是 r6 M3 修复的盲区，保留对照断言防回退）。
        let legacy_peers = peers_on_same_execution_root(
            &store,
            &alice_id,
            &root,
            session_roots_resolver_for_tests(&store),
        )
        .expect("peers");
        assert!(
            !legacy_peers.iter().any(|(id, _)| id == &bob_id),
            "legacy session_roots resolution must not see the ACP peer (documents the fixed blind spot)"
        );
    }

    fn meta(id: &str, turn: Option<u32>, kind: CheckpointKind) -> CheckpointMeta {
        CheckpointMeta {
            id: id.into(),
            turn,
            kind,
            label: String::new(),
            commit: format!("commit-{id}"),
            created_at: 0,
        }
    }

    /// 「回退到第 N 轮」= 定位 turn N+1 的 Turn 快照；同 turn 取先创建者，
    /// PreRestore 快照即使 turn 对得上也不得命中。
    #[test]
    fn rewind_plan_locates_first_created_turn_checkpoint() {
        let entries = vec![
            meta("c1-1", Some(1), CheckpointKind::Turn),
            // 防御：PreRestore 不带 turn（实际数据 turn=None），即便带也不得命中。
            meta("c9-9", Some(2), CheckpointKind::PreRestore),
            meta("c2-2", Some(2), CheckpointKind::Turn),
            meta("c3-3", Some(2), CheckpointKind::Turn),
        ];
        // 回退到第 1 轮 = 恢复 turn 2 快照，取同 turn 先创建的 c2-2。
        let plan = resolve_rewind_plan(entries, 1, false).expect("plan");
        assert!(!plan.degraded);
        assert_eq!(plan.checkpoint.expect("checkpoint").id, "c2-2");

        // N=0 = 恢复 turn 1 快照。
        let plan = resolve_rewind_plan(
            vec![
                meta("c1-1", Some(1), CheckpointKind::Turn),
                meta("c2-2", Some(2), CheckpointKind::Turn),
            ],
            0,
            false,
        )
        .expect("plan");
        assert_eq!(plan.checkpoint.expect("checkpoint").id, "c1-1");
    }

    /// 目标 turn 快照不存在：conversation_only=true 降级为仅对话回退并标记
    /// degraded；=false 如实报错。
    #[test]
    fn rewind_plan_degrades_or_errors_when_checkpoint_missing() {
        let entries = || vec![meta("c1-1", Some(1), CheckpointKind::Turn)];
        // 回退到第 5 轮 = turn 6 快照不存在（LRU 淘汰/当时快照失败）。
        let error = resolve_rewind_plan(entries(), 5, false).expect_err("must error");
        assert!(error.contains("第 6 轮"), "错误需指明缺失的 turn: {error}");

        let plan = resolve_rewind_plan(entries(), 5, true).expect("degraded plan");
        assert!(plan.degraded);
        assert!(plan.checkpoint.is_none());
    }

    /// conversation_only 严格生效：即便目标 turn 快照存在，调用方选择仅回退
    /// 对话时也绝不动代码（用户确认弹窗上看到的是「代码保持不变」）。
    #[test]
    fn rewind_plan_conversation_only_never_restores_code() {
        let entries = vec![
            meta("c1-1", Some(1), CheckpointKind::Turn),
            meta("c2-2", Some(2), CheckpointKind::Turn),
        ];
        let plan = resolve_rewind_plan(entries, 1, true).expect("degraded plan");
        assert!(plan.degraded);
        assert!(plan.checkpoint.is_none());
    }

    fn isolated_store(label: &str) -> (SessionStore, std::sync::MutexGuard<'static, ()>) {
        let guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-rewind-gate-{label}-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        // SAFETY: 测试进程内由 ENV_LOCK 串行化所有 PINVOU3_HOME 改写（与
        // sessions/tests.rs 同款约定）。
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        let store = SessionStore::boot().expect("boot SessionStore");
        (store, guard)
    }

    /// 同长覆写已被影子 index 暂存的执行根文件时，若写入落在一个时间戳 tick
    /// 内且 ext4 把刚释放的 inode 立刻复用，stat 与 index 缓存项完全一致，
    /// 后续 `add -A` 会信任旧 blob（新旧快照树相同 → 提交复用，新内容丢失）。
    /// 与 features/code_checkpoints 的 TestDir::write #590 加固同款：先写
    /// 兄弟临时文件（创建时旧 inode 仍在，编号必不相同）再 rename 覆盖，
    /// 与时间戳粒度和 inode 复用行为无关。
    fn rewrite_staged_file(path: &std::path::Path, content: &str) {
        let mut staging = path.as_os_str().to_os_string();
        staging.push(format!(
            ".pinvou3-new-{}",
            crate::platform::paths::tests::unique_suffix()
        ));
        let staging = std::path::PathBuf::from(staging);
        std::fs::write(&staging, content).expect("write staging file");
        std::fs::rename(&staging, path).expect("rename staging file over target");
    }

    /// 定时会话 profile（车道判定的 ScheduledRun 腿测试用；workspace 由
    /// `create_scheduled_run` 从 task_id 派生，这里只填必填字段）。
    fn scheduled_profile_for_gate_test(
        task_id: &str,
    ) -> crate::features::sessions::ScheduledRunProfile {
        crate::features::sessions::ScheduledRunProfile {
            task_id: task_id.to_string(),
            model: "/scheduled-model".into(),
            model_id: None,
            workspace: std::env::temp_dir(),
            mode: crate::features::sessions::ScheduledRunMode::Plan,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
        }
    }

    /// 跨会话忙碌门：同执行根的其它会话忙碌 → 返回其标题；不忙碌、
    /// 不同根、自身忙碌都不拦截。非 code 会话（plain/scheduled）同根且在途
    /// 也拦截——门看的是「谁在往这个目录写文件」，不是会话类型。
    #[test]
    fn busy_gate_flags_only_busy_code_peers_on_same_root() {
        let (store, _g) = isolated_store("peer");
        let project = std::env::temp_dir().join(format!(
            "pinvou3-rewind-gate-proj-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&project).expect("project dir");
        let alice = store
            .create_new("/model".into(), None, project.clone())
            .expect("create alice");
        let bob = store
            .create_new("/model".into(), None, project.clone())
            .expect("create bob");
        let (alice_id, bob_id) = (alice.metadata.id.clone(), bob.metadata.id.clone());

        // 两个会话绑定同一项目目录（共享执行根）。
        let bound = project.clone();
        let (a, b) = (alice_id.clone(), bob_id.clone());
        store.set_execution_root_resolver(Arc::new(move |id: &str| {
            if id == a || id == b {
                Some(bound.clone())
            } else {
                None
            }
        }));

        // bob 忙碌 → 命中其标题。
        let busy_bob = bob_id.clone();
        let hit = busy_peer_on_same_execution_root(
            &store,
            &alice_id,
            &project,
            session_roots_resolver_for_tests(&store),
            |id| id == busy_bob,
        )
        .expect("gate");
        assert_eq!(hit, Some(bob.metadata.title.clone()));

        // 不忙碌 → 放行。
        let none = busy_peer_on_same_execution_root(
            &store,
            &alice_id,
            &project,
            session_roots_resolver_for_tests(&store),
            |_| false,
        )
        .expect("gate");
        assert_eq!(none, None);

        // 自身忙碌不算（门只看其它会话）。
        let busy_alice = alice_id.clone();
        let none = busy_peer_on_same_execution_root(
            &store,
            &alice_id,
            &project,
            session_roots_resolver_for_tests(&store),
            |id| id == busy_alice,
        )
        .expect("gate");
        assert_eq!(none, None);

        // 非 code 会话（plain）同根且在途同样拦截：它会往同一目录写文件。
        let only_alice = alice_id.clone();
        store.set_code_session_predicate(Arc::new(move |id: &str| id == only_alice));
        let busy_bob = bob_id.clone();
        let hit = busy_peer_on_same_execution_root(
            &store,
            &alice_id,
            &project,
            session_roots_resolver_for_tests(&store),
            |id| id == busy_bob,
        )
        .expect("gate");
        assert_eq!(hit, Some(bob.metadata.title.clone()));
    }

    /// An aux session (aux-) is not intercepted even when bound to the same
    /// execution root and streaming an answer (PR #433 round-20 Major-1):
    /// under the scoped-read-only semantics it cannot write files into the
    /// directory, so the busy gate's premise structurally does not hold for
    /// it. Note
    /// (round-22 correction): the production root resolver always returns
    /// None for aux ids (no codex record, and no workspace binding sidecar is
    /// written), so the aux root always resolves to the private
    /// `sessions/aux-<id>/workspace`, and the same-root-as-main scenario is
    /// unreachable in production — the resolver binding below is a
    /// hypothetical configuration that production could never produce for an
    /// aux; what this test pins is the defensive exclusion itself, not a
    /// mis-lock that ever really happened.
    #[test]
    fn busy_gate_ignores_busy_aux_peers_on_same_root() {
        let (store, _g) = isolated_store("aux-peer");
        let project = std::env::temp_dir().join(format!(
            "pinvou3-rewind-gate-aux-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&project).expect("project dir");
        let main = store
            .create_new("/model".into(), None, project.clone())
            .expect("create main");
        let main_id = main.metadata.id.clone();
        let aux = store
            .get_or_create_aux_session(&main_id)
            .expect("create aux");

        // Defensive configuration: the production root resolver always returns
        // None for aux ids (see the test doc comment); here the aux is
        // explicitly given the same root as the main session, purely to pin
        // the exclusion logic itself.
        let bound = project.clone();
        let (m, x) = (main_id.clone(), aux.id.clone());
        store.set_execution_root_resolver(Arc::new(move |id: &str| {
            if id == m || id == x {
                Some(bound.clone())
            } else {
                None
            }
        }));

        // The aux session is streaming an answer (busy evaluates true) -> not intercepted.
        let busy_aux = aux.id.clone();
        let none = busy_peer_on_same_execution_root(
            &store,
            &main_id,
            &project,
            session_roots_resolver_for_tests(&store),
            |id| id == busy_aux,
        )
        .expect("gate");
        assert_eq!(none, None, "busy aux peer must not block the main session");

        // Control: a busy ordinary session on the same root still blocks (the
        // aux exclusion did not weaken existing semantics).
        let bob = store
            .create_new("/model".into(), None, project.clone())
            .expect("create bob");
        let bound2 = project.clone();
        let (m2, b2) = (main_id.clone(), bob.metadata.id.clone());
        store.set_execution_root_resolver(Arc::new(move |id: &str| {
            if id == m2 || id == b2 {
                Some(bound2.clone())
            } else {
                None
            }
        }));
        let busy_bob = bob.metadata.id.clone();
        let hit = busy_peer_on_same_execution_root(
            &store,
            &main_id,
            &project,
            session_roots_resolver_for_tests(&store),
            |id| id == busy_bob,
        )
        .expect("gate");
        assert_eq!(hit, Some(bob.metadata.title.clone()));
    }

    /// 不同执行根的会话忙碌不影响本根回退。
    #[test]
    fn busy_gate_ignores_peers_on_other_roots() {
        let (store, _g) = isolated_store("other-root");
        let project = std::env::temp_dir().join(format!(
            "pinvou3-rewind-gate-p1-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let other = std::env::temp_dir().join(format!(
            "pinvou3-rewind-gate-p2-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&project).expect("project dir");
        std::fs::create_dir_all(&other).expect("other dir");
        let alice = store
            .create_new("/model".into(), None, project.clone())
            .expect("create alice");
        let carol = store
            .create_new("/model".into(), None, other.clone())
            .expect("create carol");
        let (alice_id, carol_id) = (alice.metadata.id.clone(), carol.metadata.id.clone());

        // alice 绑 project，carol 绑 other（不同执行根）。
        let (a, c) = (alice_id.clone(), carol_id.clone());
        let (p, o) = (project.clone(), other.clone());
        store.set_execution_root_resolver(Arc::new(move |id: &str| {
            if id == a {
                Some(p.clone())
            } else if id == c {
                Some(o.clone())
            } else {
                None
            }
        }));
        let code_ids = [alice_id.clone(), carol_id.clone()];
        store.set_code_session_predicate(Arc::new(move |id: &str| {
            code_ids.iter().any(|candidate| candidate == id)
        }));

        let busy_carol = carol_id.clone();
        let none = busy_peer_on_same_execution_root(
            &store,
            &alice_id,
            &project,
            session_roots_resolver_for_tests(&store),
            |id| id == busy_carol,
        )
        .expect("gate");
        assert_eq!(none, None, "不同执行根的忙碌会话不得拦截");
    }

    /// 编排层 P0 场景：旧分支 turn 1..3 的快照在回退到第 1 轮后被作废，
    /// index 不再含 turn > keep_turns 的 Turn 条目；重新创作打同号新快照后，
    /// first-wins 对齐命中的必须是新分支快照而不是被遗弃分支的旧快照。
    #[test]
    fn rewind_invalidate_unshadows_recreated_turn_branch() {
        if !git_available() {
            return;
        }
        let suffix = format!(
            "{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        );
        let ledger = std::env::temp_dir().join(format!("pinvou3-rewind-inval-ledger-{suffix}"));
        let exec = std::env::temp_dir().join(format!("pinvou3-rewind-inval-exec-{suffix}"));
        std::fs::create_dir_all(&ledger).expect("ledger dir");
        std::fs::create_dir_all(&exec).expect("exec dir");

        // 旧分支：turn 1/2/3 各打一个 Turn 快照（模拟会话进行到第 3 轮）。
        std::fs::write(exec.join("a.txt"), "0\n").expect("write");
        checkpoints::create_checkpoint(&ledger, &exec, Some(1), CheckpointKind::Turn, "t1")
            .expect("c1");
        rewrite_staged_file(&exec.join("a.txt"), "1\n");
        let old_t2 =
            checkpoints::create_checkpoint(&ledger, &exec, Some(2), CheckpointKind::Turn, "t2")
                .expect("c2");
        rewrite_staged_file(&exec.join("a.txt"), "2\n");
        checkpoints::create_checkpoint(&ledger, &exec, Some(3), CheckpointKind::Turn, "t3")
            .expect("c3");

        // 回退到第 1 轮（编排层在 restore + 截断成功后调用的正是本函数）。
        invalidate_abandoned_turn_checkpoints(&ledger, 1);
        let listed = checkpoints::list_checkpoints(&ledger).expect("list");
        assert!(
            listed
                .iter()
                .all(|entry| !(entry.kind == CheckpointKind::Turn
                    && entry.turn.is_some_and(|turn| turn > 1))),
            "回退后 index 不得残留 turn > keep_turns 的 Turn 条目: {listed:?}"
        );

        // 重新创作：新分支的 turn 2 打新快照。若旧 t2 未作废，find 会先命中它。
        std::fs::write(exec.join("a.txt"), "new-branch\n").expect("write");
        let new_t2 =
            checkpoints::create_checkpoint(&ledger, &exec, Some(2), CheckpointKind::Turn, "t2-new")
                .expect("c2 new");
        assert_ne!(old_t2.id, new_t2.id);
        let plan = resolve_rewind_plan(
            checkpoints::list_checkpoints(&ledger).expect("list"),
            1,
            false,
        )
        .expect("plan");
        assert_eq!(
            plan.checkpoint.expect("checkpoint").id,
            new_t2.id,
            "再次回退到第 1 轮必须命中新分支的 turn 2 快照"
        );

        let _ = std::fs::remove_dir_all(&ledger);
        let _ = std::fs::remove_dir_all(&exec);
    }

    // ── 回退反悔（rewind_undo_state / undo_last_rewind 编排件）──────────────

    // Message is imported once at the top of this test module; the
    // rewind-undo section only adds ContentBlock.
    use deepseek_tui::models::ContentBlock;

    fn user_msg(text: &str) -> Message {
        Message {
            role: "user".into(),
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        }
    }

    fn assistant_msg(text: &str) -> Message {
        Message {
            role: "assistant".into(),
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        }
    }

    /// 造一个带两轮对话、且已回退到第 1 轮的原生工作模式会话（不设 code
    /// predicate——门放宽后车道判定与产品模式轴无关，工作模式即原生车道）；
    /// 返回 (store, guard, 会话 id)。
    fn rewound_native_session(
        label: &str,
    ) -> (SessionStore, std::sync::MutexGuard<'static, ()>, String) {
        let (store, guard) = isolated_store(label);
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let id = session.metadata.id.clone();
        store
            .update_messages(
                &id,
                vec![
                    user_msg("第一轮"),
                    assistant_msg("答一"),
                    user_msg("第二轮"),
                    assistant_msg("答二"),
                ],
            )
            .expect("seed transcript");
        store
            .truncate_to_user_turn(&id, 1, None)
            .expect("rewind to turn 1");
        (store, guard, id)
    }

    /// undo 条件①③满足但回退后发过新轮次（条件②破）→ None。
    #[test]
    fn undo_state_none_after_new_turn_since_rewind() {
        let (store, _g, id) = rewound_native_session("undo-new-turn");
        store
            .update_messages(
                &id,
                vec![
                    user_msg("第一轮"),
                    assistant_msg("答一"),
                    user_msg("新分支"),
                ],
            )
            .expect("append new turn");
        // 条件①: sidecar 有记录；条件③ 与本断言无关（ledger 无 checkpoint 也会先被
        // 条件②挡住）——直接验证 inner 返回 None。
        assert_eq!(
            rewind_undo_state_inner(&store, |_| false, &id).expect("state"),
            None,
            "回退后发过新轮次必须不可反悔"
        );
    }

    /// undo 条件①破（无回退备份记录）→ None。
    #[test]
    fn undo_state_none_without_backup_record() {
        let (store, _g) = isolated_store("undo-no-record");
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let id = session.metadata.id.clone();
        store
            .update_messages(&id, vec![user_msg("第一轮"), assistant_msg("答一")])
            .expect("seed");
        assert_eq!(
            rewind_undo_state_inner(&store, |_| false, &id).expect("state"),
            None
        );
    }

    /// 非原生车道 → None（不报错）：ACP 会话（is_acp 命中）与定时会话
    /// （SessionKind::ScheduledRun）都不在检查点能力范围；工作模式会话
    /// （is_acp 未命中）同条件下不再被排除（门放宽的核心语义）。
    #[test]
    fn undo_state_none_for_non_native_lane_sessions() {
        let (store, _g) = isolated_store("undo-non-native-lane");
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let plain_id = session.metadata.id.clone();
        // ACP 谓词命中 → 非原生车道。
        assert_eq!(
            rewind_undo_state_inner(&store, |id| id == plain_id, &plain_id).expect("state"),
            None,
            "ACP 会话必须软拒绝（Ok(None)）"
        );
        // 定时会话（ScheduledRun）→ 非原生车道。
        let scheduled = store
            .create_scheduled_run(scheduled_profile_for_gate_test("task-undo-gate"))
            .expect("create scheduled run");
        assert_eq!(
            rewind_undo_state_inner(&store, |_| false, &scheduled.metadata.id).expect("state"),
            None,
            "定时会话必须软拒绝（Ok(None)）"
        );
        // 对照：同一 plain 会话在非 ACP 谓词下命中原生车道——不再是 None-by-lane
        // （此处无回退记录，故为条件①破的 None，但车道判定已放行）。
        let lane_hit = is_native_checkpoint_lane(&plain_id, &store, |_| false);
        assert!(lane_hit, "工作模式会话必须命中原生车道");
    }

    /// undo 条件③破（记录绑定的 PreRestore 已被 LRU 淘汰）→ None；
    /// 条件齐全 → Some 且字段正确。顺带覆盖 rewind→undo 编排往返：
    /// 代码与对话都回到 rewind 前。
    #[test]
    fn undo_state_conditions_and_rewind_undo_round_trip() {
        if !git_available() {
            return;
        }
        let (store, _g) = isolated_store("undo-round-trip");
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let id = session.metadata.id.clone();
        store
            .update_messages(
                &id,
                vec![
                    user_msg("第一轮"),
                    assistant_msg("答一"),
                    user_msg("第二轮"),
                    assistant_msg("答二"),
                ],
            )
            .expect("seed transcript");
        // 该会话两根相同（未绑项目）：checkpoint 账本落会话私有目录。
        let roots = store.session_roots(&id).expect("roots");
        let ledger = roots.ledger.clone();
        let exec = roots.execution.clone();
        std::fs::create_dir_all(&ledger).expect("ledger dir");
        std::fs::write(exec.join("code.txt"), "v2\n").expect("write v2");
        // 镜像 rewind_to_turn 步骤 1：恢复代码前强制的 PreRestore（内容 = rewind 前 v2）。
        let pre_restore = checkpoints::create_checkpoint(
            &ledger,
            &exec,
            None,
            CheckpointKind::PreRestore,
            "回滚点",
        )
        .expect("pre-restore checkpoint");
        // 镜像步骤 2：截断对话，记录绑定本次回退的 PreRestore。
        store
            .truncate_to_user_turn(&id, 1, Some(pre_restore.id.clone()))
            .expect("rewind to turn 1");
        // 模拟 rewind 的代码侧结果：执行根已是回退后状态 v1。
        rewrite_staged_file(&exec.join("code.txt"), "v1\n");

        // 条件齐全 → Some，字段正确（被截 1 轮，checkpoint_id 为记录绑定的 PreRestore）。
        let info = rewind_undo_state_inner(&store, |_| false, &id)
            .expect("state")
            .expect("undoable");
        assert_eq!(info.checkpoint_id.as_deref(), Some(pre_restore.id.as_str()));
        assert_eq!(info.kept_turns, 1);
        assert_eq!(info.rewound_turns, 1);
        assert!(!info.rewound_at.is_empty());

        // 编排往返（镜像 undo_last_rewind 的步骤 1/2；State/EnginePool 无法在
        // 单元测试构造，命令本体仅是这段顺序 + 忙碌门 + evict）：
        checkpoints::restore_checkpoint(&ledger, &exec, &pre_restore.id)
            .expect("restore code to pre-restore");
        let restored = store
            .restore_rewound_turns(&id)
            .expect("restore conversation");
        assert_eq!(restored, 2);
        // 代码与对话都回到 rewind 前。
        assert_eq!(
            std::fs::read_to_string(exec.join("code.txt")).expect("read"),
            "v2\n"
        );
        assert_eq!(
            store.load(&id).expect("load").messages,
            vec![
                user_msg("第一轮"),
                assistant_msg("答一"),
                user_msg("第二轮"),
                assistant_msg("答二"),
            ]
        );
        // 记录已消费 → 不再可反悔；对称语义：restore 又打了一条 PreRestore，
        // 「反悔的反悔」在代码侧仍可恢复（index 里 PreRestore 仍在）。
        assert_eq!(
            rewind_undo_state_inner(&store, |_| false, &id).expect("state"),
            None
        );
        assert!(
            checkpoints::list_checkpoints(&ledger)
                .expect("list")
                .iter()
                .any(|entry| entry.kind == CheckpointKind::PreRestore)
        );

        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// M1 回归：仅对话降级回退（无代码快照）的记录不绑定 PreRestore——undo 只
    /// 还原对话，绝不因 index 里存在其它回退留下的 PreRestore 而误动代码。
    #[test]
    fn degraded_rewind_undo_never_restores_code() {
        if !git_available() {
            return;
        }
        let (store, _g, id) = rewound_native_session("undo-degraded");
        let roots = store.session_roots(&id).expect("roots");
        let ledger = roots.ledger.clone();
        let exec = roots.execution.clone();
        std::fs::create_dir_all(&ledger).expect("ledger dir");
        std::fs::write(exec.join("code.txt"), "user-work\n").expect("write");
        // 一次更早的完整回退留下的 PreRestore（与本次降级回退无关）。
        checkpoints::create_checkpoint(
            &ledger,
            &exec,
            None,
            CheckpointKind::PreRestore,
            "无关回滚点",
        )
        .expect("unrelated pre-restore");
        std::fs::write(exec.join("code.txt"), "user-work-new\n").expect("write new");

        // 降级记录（rewound_native_session 以 None 绑定截断）：undo 状态可用但
        // checkpoint_id 必须是 None——不能错配到上面那条无关回滚点。
        let info = rewind_undo_state_inner(&store, |_| false, &id)
            .expect("state")
            .expect("degraded undo available");
        assert_eq!(info.checkpoint_id, None, "降级回退的 undo 不得恢复代码");

        // undo 只追加对话：代码文件保持用户新作的内容。
        let restored = store
            .restore_rewound_turns(&id)
            .expect("restore conversation");
        assert_eq!(restored, 2);
        assert_eq!(
            std::fs::read_to_string(exec.join("code.txt")).expect("read"),
            "user-work-new\n",
            "降级回退的 undo 不得触碰代码"
        );
        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// M2 回归：undo 步骤 1 会新打一条更晚的 PreRestore；undo 候选必须仍绑定
    /// 原记录里的回滚点，重试不得漂移到新快照（否则把代码恢复回已回退状态）。
    #[test]
    fn undo_target_stays_bound_after_later_pre_restore() {
        if !git_available() {
            return;
        }
        let (store, _g) = isolated_store("undo-retry");
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let id = session.metadata.id.clone();
        store
            .update_messages(
                &id,
                vec![
                    user_msg("第一轮"),
                    assistant_msg("答一"),
                    user_msg("第二轮"),
                    assistant_msg("答二"),
                ],
            )
            .expect("seed transcript");
        let roots = store.session_roots(&id).expect("roots");
        let ledger = roots.ledger.clone();
        let exec = roots.execution.clone();
        std::fs::create_dir_all(&ledger).expect("ledger dir");
        std::fs::write(exec.join("code.txt"), "v2\n").expect("write v2");
        let bound = checkpoints::create_checkpoint(
            &ledger,
            &exec,
            None,
            CheckpointKind::PreRestore,
            "回滚点",
        )
        .expect("bound pre-restore");
        store
            .truncate_to_user_turn(&id, 1, Some(bound.id.clone()))
            .expect("rewind to turn 1");
        // 模拟 undo 步骤 1 已成功、步骤 2 失败：restore 内部新打的更晚 PreRestore。
        rewrite_staged_file(&exec.join("code.txt"), "v1\n");
        let poisoned = checkpoints::create_checkpoint(
            &ledger,
            &exec,
            None,
            CheckpointKind::PreRestore,
            "undo 步骤 1 的副作用快照",
        )
        .expect("poisoning pre-restore");
        assert_ne!(bound.id, poisoned.id);

        // 重试 undo：候选仍是绑定的回滚点，不是更晚的那条。
        let info = rewind_undo_state_inner(&store, |_| false, &id)
            .expect("state")
            .expect("retry undoable");
        assert_eq!(
            info.checkpoint_id.as_deref(),
            Some(bound.id.as_str()),
            "undo 目标不得漂移到 undo 自己新打的 PreRestore"
        );
        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// m4 回归：回退后编辑了保留尾部的 assistant 回复（turn 数不变）→ revision
    /// 不再匹配截断时的记录，如实不可反悔（弱代理时代会错误放行）。
    #[test]
    fn undo_state_none_after_tail_edit_since_rewind() {
        let (store, _g, id) = rewound_native_session("undo-tail-edit");
        store
            .update_messages(
                &id,
                vec![user_msg("第一轮"), assistant_msg("答一（已编辑）")],
            )
            .expect("edit tail");
        assert_eq!(
            rewind_undo_state_inner(&store, |_| false, &id).expect("state"),
            None,
            "回退后尾部被编辑必须不可反悔"
        );
        // restore 路径在 mutation 锁内复核，同样拒绝。错误文案锚定「不可反悔」
        // 字样——undo_last_rewind 的失败分类依赖它区分「条件已破（不可重试）」
        // 与「IO 失败（可重试）」，改文案必须先改分类逻辑（评审 nit）。
        let error = store.restore_rewound_turns(&id).expect_err("must refuse");
        assert!(
            format!("{error:#}").contains("不可反悔"),
            "条件已破的错误必须含「不可反悔」字样（undo 分类契约）: {error:#}"
        );
    }

    /// undo 条件③单独为否：记录绑定了 PreRestore 但该条目已不在 index（LRU
    /// 淘汰）→ None。独立成测试：`isolated_store` 持有进程级 ENV_LOCK 直到
    /// guard drop，同一测试内二次调用会自死锁（std::sync::Mutex 不可重入）。
    #[test]
    fn undo_state_none_without_pre_restore_checkpoint() {
        let (store, _g) = isolated_store("undo-no-prerestore");
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let id = session.metadata.id.clone();
        store
            .update_messages(
                &id,
                vec![
                    user_msg("第一轮"),
                    assistant_msg("答一"),
                    user_msg("第二轮"),
                    assistant_msg("答二"),
                ],
            )
            .expect("seed transcript");
        // 绑定一个不存在的 checkpoint（等价于绑定后又被淘汰）。
        store
            .truncate_to_user_turn(&id, 1, Some("c999-1".into()))
            .expect("rewind to turn 1");
        let ledger = store.session_roots(&id).expect("roots").ledger;
        assert_eq!(
            resolve_rewind_undo_state(&store, &ledger, &id).expect("state"),
            None,
            "绑定的 PreRestore 不在 index 时必须不可反悔"
        );
    }

    /// 旧格式 sidecar 记录（修复前写入，无 preRestoreCheckpointId /
    /// truncatedRevision 字段）的 serde 兼容：#[serde(default)] 落 None/空串，
    /// undo 退回 turn 数弱代理、代码侧不恢复（checkpoint_id = None）。
    #[test]
    fn legacy_backup_record_without_binding_still_loads() {
        let (store, _g, id) = rewound_native_session("undo-legacy");
        // 用旧格式 JSON 覆盖 sidecar（模拟修复前的记录）。
        let session = store.load(&id).expect("load");
        let removed: Vec<Message> = vec![user_msg("第二轮"), assistant_msg("答二")];
        let legacy = serde_json::json!({
            id.clone(): [{
                "rewoundAt": "2026-08-21T10:00:00Z",
                "originalRevision": "legacy",
                "keptTurns": 1,
                "removedMessages": removed,
            }]
        });
        let sidecar = crate::platform::paths::sessions_root().join("_rewound_turns.json");
        std::fs::write(&sidecar, serde_json::to_vec_pretty(&legacy).expect("json"))
            .expect("write legacy sidecar");

        // 旧记录可读：无绑定 → 仅对话反悔；revision 校验跳过（空串哨兵）。
        let ledger = store.session_roots(&id).expect("roots").ledger;
        let info = resolve_rewind_undo_state(&store, &ledger, &id)
            .expect("state")
            .expect("legacy record undoable");
        assert_eq!(info.checkpoint_id, None, "旧记录无绑定，undo 不得恢复代码");
        let restored = store.restore_rewound_turns(&id).expect("legacy restore");
        assert_eq!(restored, 2);
        assert_eq!(
            store.load(&id).expect("load").messages.len(),
            session.messages.len() + 2
        );
    }

    // ── 工作模式快照/回退（门放宽回归：F1/F2/F4/F6/F8、B1-B4）────────────────

    /// 造一个工作模式会话（非 code、未绑项目、非定时）并返回其两根。
    fn work_mode_session(
        label: &str,
    ) -> (
        SessionStore,
        std::sync::MutexGuard<'static, ()>,
        String,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let (store, guard) = isolated_store(label);
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let id = session.metadata.id.clone();
        assert!(
            is_native_checkpoint_lane(&id, &store, |_| false),
            "工作模式会话必须命中原生车道"
        );
        let roots = store.session_roots(&id).expect("roots");
        assert_eq!(roots.execution, roots.ledger, "未绑定会话两根同为私有目录");
        // 私有 workspace 由引擎懒创建，测试先建目录（体积估算遇缺失目录按不在
        // 预算内处理，快照会被跳过）。
        std::fs::create_dir_all(&roots.execution).expect("create private workspace");
        (store, guard, id, roots.ledger, roots.execution)
    }

    /// F1/B4：工作模式会话经共享快照缝 `create_turn_checkpoint`（chat/accept_plan
    /// 同一编排）每轮产生 Turn 快照，turn 序号随会话消息计数递增；未绑定会话
    /// 快照落私有目录（ensure_repo 自排除 checkpoints 目录，feature 层由
    /// `ledger_inside_execution_root_is_excluded_from_snapshot_and_clean` 钉住）。
    /// 时序镜像生产：快照先于该轮消息落盘（turn 号 = 已有轮数 + 1）。
    #[tokio::test]
    async fn work_mode_turn_checkpoints_via_shared_seam() {
        if !git_available() {
            return;
        }
        let (store, _g, id, ledger, execution) = work_mode_session("work-snapshot");
        let snapshot1 = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第一轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 1 snapshot");
        store
            .update_messages(&id, vec![user_msg("第一轮"), assistant_msg("答一")])
            .expect("seed turn 1");
        let snapshot2 = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第二轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 2 snapshot");
        store
            .update_messages(&id, vec![user_msg("第二轮"), assistant_msg("答二")])
            .expect("seed turn 2");
        let turns: Vec<(Option<u32>, String)> = checkpoints::list_checkpoints(&ledger)
            .expect("list")
            .into_iter()
            .filter(|entry| entry.kind == CheckpointKind::Turn)
            .map(|entry| (entry.turn, entry.id))
            .collect();
        assert!(
            turns.contains(&(Some(1), snapshot1.clone())),
            "turn 1 快照缺失: {turns:?}"
        );
        assert!(
            turns.contains(&(Some(2), snapshot2.clone())),
            "turn 2 快照缺失: {turns:?}"
        );

        // F8：发送失败路径按 id 精确作废「未成活」快照（drop_unsent 共用编排）。
        let dropped_id = snapshot2.clone();
        drop_unsent_turn_checkpoint(Some(ledger.clone()), Some(snapshot2), &id, "chat").await;
        let remaining: Vec<String> = checkpoints::list_checkpoints(&ledger)
            .expect("list")
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert!(remaining.contains(&snapshot1), "其它快照不得被误删");
        assert!(
            !remaining.contains(&dropped_id),
            "未成活快照必须按 id 精确删除: {remaining:?}"
        );
        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// r21 M1（阻塞项回归钉）：压缩整体替换 transcript 后的空闲窗口——账本
    /// 末条 Turn 序号必须随收缩钩子作废而被拉回与新 transcript 同数（此处
    /// 保留 1），调和门据此隐藏错位入口；否则前端时间线（保留全部气泡的
    /// 10）== 账本序号 → 门放行 → 内部芯片静默错位回退。
    #[tokio::test]
    async fn compaction_shrink_keeps_ledger_count_transcript_honest() {
        if !git_available() {
            return;
        }
        let (store, _g, id, ledger, execution) = work_mode_session("compaction-window");
        let _s1 = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第一轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("t1");
        store
            .update_messages(&id, vec![user_msg("第一轮"), assistant_msg("答一")])
            .expect("seed 1");
        let _s2 = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第二轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("t2");
        store
            .update_messages(
                &id,
                vec![
                    user_msg("第一轮"),
                    assistant_msg("答一"),
                    user_msg("第二轮"),
                    assistant_msg("答二"),
                ],
            )
            .expect("seed 2");

        // 注册与 lib.rs 组合根同形的收缩钩子（测试内同步执行——失效只触碰
        // 账本索引，不重入 store，符合钩子纪律），随后模拟压缩整体替换。
        let hook_store = store.clone();
        let hook_ledger = ledger.clone();
        let hook_id = id.clone();
        store.register_messages_shrunk_hook(std::sync::Arc::new(
            move |session_id: &str, messages: &[deepseek_tui::models::Message]| {
                let kept = checkpoints::count_user_turns(messages);
                let _ = checkpoints::invalidate_turn_checkpoints_after(&hook_ledger, kept);
                let _ = (session_id, &hook_id, &hook_store);
            },
        ));
        let compacted = crate::features::sessions::ChatEngineState {
            messages: vec![user_msg("第一轮"), assistant_msg("答一")],
            system_prompt: None,
            model: "/model".into(),
            workspace: std::env::temp_dir(),
        };
        store
            .persist_chat_engine_state_with_commit(&id, &compacted, || true, |_| {})
            .expect("compaction persist");

        // 空闲窗口的调和门计数：索引末条 Turn 必须是收缩后的 1（而非 2）。
        let entries = checkpoints::list_checkpoints(&ledger).expect("list");
        let last_turn = entries
            .iter()
            .rev()
            .find(|entry| entry.kind == checkpoints::CheckpointKind::Turn)
            .and_then(|entry| entry.turn);
        assert_eq!(
            last_turn,
            Some(1),
            "压缩收缩后账本派生计数必须与新 transcript 同数（r21 M1），否则调和门放行错位回退"
        );
        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// r19 M6：回退在 apply 后、truncate 前崩溃（暂记落盘、代码已恢复、对话
    /// 未截断）——下一次 turn 快照前的崩溃对账必须自动完成协议：截断生效、
    /// 被截分支快照作废、暂记清除、本轮快照登记在截断后的树上（而不是把混合
    /// 树固化进历史、让 pending 复用门与 undo 绑定永久失效）。
    #[tokio::test]
    async fn crash_interrupted_rewind_auto_completes_before_next_snapshot() {
        if !git_available() {
            return;
        }
        let (store, _g, id, ledger, execution) = work_mode_session("crash-reconcile");
        std::fs::write(execution.join("code.txt"), "v1\n").expect("seed code");
        let first = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第一轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 1 snapshot");
        store
            .update_messages(&id, vec![user_msg("第一轮"), assistant_msg("答一")])
            .expect("seed turn 1");
        std::fs::write(execution.join("code.txt"), "v2\n").expect("advance code");
        let second = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第二轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 2 snapshot");
        store
            .update_messages(
                &id,
                vec![
                    user_msg("第一轮"),
                    assistant_msg("答一"),
                    user_msg("第二轮"),
                    assistant_msg("答二"),
                ],
            )
            .expect("seed turn 2");

        // 模拟崩溃点：协议走到 apply 完成、truncate 未启动（暂记留存）。
        let revision = {
            let session = store.load(&id).expect("load");
            crate::features::sessions::transcript_revision(&session.messages).expect("revision")
        };
        let target = checkpoints::find_checkpoint(&ledger, &first).expect("target");
        let (_t, undo) =
            checkpoints::prepare_restore(&ledger, &execution, &first, None).expect("prepare");
        checkpoints::write_pending_rewind(&ledger, &first, 1, &undo.id, &revision)
            .expect("pending");
        checkpoints::apply_restore(&ledger, &execution, &target).expect("apply");
        assert_eq!(
            std::fs::read_to_string(execution.join("code.txt")).unwrap(),
            "v1\n",
            "apply 已把代码恢复到第一轮"
        );
        assert_eq!(
            checkpoints::count_user_turns(&store.load(&id).expect("load").messages),
            2,
            "崩溃点：对话尚未截断（混合态）"
        );

        // 下一次 turn 快照：对账 seam 自动完成协议 + 照常登记本轮快照。
        let snapshot3 = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第三轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("reconcile 后本轮快照必须照常登记");
        let session = store.load(&id).expect("load after");
        assert_eq!(
            checkpoints::count_user_turns(&session.messages),
            1,
            "对账必须完成截断（保留第一轮）"
        );
        assert!(
            !ledger.join("pending_rewind.json").exists(),
            "协议完成后暂记必须清除"
        );
        assert_eq!(
            std::fs::read_to_string(execution.join("code.txt")).unwrap(),
            "v1\n",
            "幂等重放 apply 不得破坏已恢复状态"
        );
        let turn_entries: Vec<(Option<u32>, String)> = checkpoints::list_checkpoints(&ledger)
            .expect("list")
            .into_iter()
            .filter(|entry| entry.kind == CheckpointKind::Turn)
            .map(|entry| (entry.turn, entry.id))
            .collect();
        assert!(
            !turn_entries.iter().any(|(_, id)| id == &second),
            "被截分支的第二轮快照必须作废: {turn_entries:?}"
        );
        assert!(
            turn_entries.contains(&(Some(2), snapshot3.clone())),
            "新分支的同号快照必须登记: {turn_entries:?}"
        );
        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// r23 M1：live-abort（回退在 ACP 复查/apply/join 失败后放弃）与崩溃留下
    /// 同样的盘上状态——但 live-abort 时用户可能继续与同根对端/压缩并发。对账
    /// 的破坏性重放臂必须在压缩在飞或同根对端忙碌时顺延（阻断本轮快照、
    /// 混合态保持未固化、暂记保留），栅栏解除后下一次发送照常完成协议。
    #[tokio::test]
    async fn reconcile_replay_defers_while_compaction_or_peers_busy() {
        if !git_available() {
            return;
        }
        let (store, _g, id, ledger, execution) = work_mode_session("reconcile-defer");
        std::fs::write(execution.join("code.txt"), "v1\n").expect("seed code");
        let first = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第一轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 1 snapshot");
        store
            .update_messages(&id, vec![user_msg("第一轮"), assistant_msg("答一")])
            .expect("seed turn 1");
        std::fs::write(execution.join("code.txt"), "v2\n").expect("advance code");
        create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第二轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 2 snapshot");
        store
            .update_messages(
                &id,
                vec![
                    user_msg("第一轮"),
                    assistant_msg("答一"),
                    user_msg("第二轮"),
                    assistant_msg("答二"),
                ],
            )
            .expect("seed turn 2");

        // 模拟 live-abort 留下的状态：pending + 代码已恢复 + 对话未截断。
        let revision = {
            let session = store.load(&id).expect("load");
            crate::features::sessions::transcript_revision(&session.messages).expect("revision")
        };
        let target = checkpoints::find_checkpoint(&ledger, &first).expect("target");
        let (_t, undo) =
            checkpoints::prepare_restore(&ledger, &execution, &first, None).expect("prepare");
        checkpoints::write_pending_rewind(&ledger, &first, 1, &undo.id, &revision)
            .expect("pending");
        checkpoints::apply_restore(&ledger, &execution, &target).expect("apply");

        // r24 minor 5 后：对端探针与根互斥由 `pools` 引用惰性消费（单元夹具
        // 无池 = 跳过），其对端/互斥顺延腿由 production-body pin 锚定生产接线
        // （send_preludes_wire_reconcile_gates_to_real_probes +
        // replay_arm_takes_the_root_flag_before_probing_peers）；此处单测钉
        // 压缩闩腿——压缩在飞时重放必须顺延。
        let gates = ReconcileGates {
            compaction_in_flight: true,
            pools: None,
        };
        {
            let deferred = "deferred-compaction".to_string();
            let snapshot = create_turn_checkpoint(
                &store,
                &id,
                ledger.clone(),
                execution.clone(),
                deferred,
                "chat",
                gates,
            )
            .await;
            assert!(snapshot.is_none(), "压缩在飞时对账必须顺延（阻断本轮快照）");
            assert!(
                checkpoints::live_pending_rewind(&ledger)
                    .expect("pending read")
                    .is_some(),
                "顺延不得清掉暂记"
            );
            assert_eq!(
                checkpoints::count_user_turns(&store.load(&id).expect("load").messages),
                2,
                "顺延不得执行破坏性截断"
            );
        }
        // 栅栏解除：下一次发送照常完成协议（重放 + 截断 + 本轮快照）。
        let snapshot = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第三轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("栅栏解除后本轮快照必须照常登记");
        assert_eq!(
            checkpoints::count_user_turns(&store.load(&id).expect("load").messages),
            1,
            "重放必须完成截断（保留第一轮）"
        );
        assert!(!snapshot.is_empty(), "本轮快照登记");
        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// r27 M2 pin（persist-split 端到端）：引导重试的窗口——暂记 revision 仍
    /// 匹配、绑定记录已存在（truncate 写记录后、持久化前失败 + 引擎旧副本
    /// 重新落盘）。对账必须**复用 P1** 幂等重放（不得 fresh 一枚 PreRestore
    /// 快照已恢复树、孤儿化 P1），完成后暂记清除、transcript 收敛到 1 轮。
    /// 本测试钉住重放流程本身：从绑定暂记出发的幂等重放必须收敛（截断到
    /// keep_turns、暂记清除、无 fresh PreRestore）——r28 复核：否决复活由
    /// `rewind_reuse_does_not_consult_the_sidecar_record` 源码 pin 承接（重放
    /// 不经 matcher，本测试对否决不敏感）。
    #[tokio::test]
    async fn persist_split_replay_reuses_the_first_pre_restore() {
        if !git_available() {
            return;
        }
        let (store, _g, id, ledger, execution) = work_mode_session("persist-split");
        std::fs::write(
            execution.join("code.txt"),
            "v1
",
        )
        .expect("seed code");
        let first = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第一轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 1 snapshot");
        store
            .update_messages(&id, vec![user_msg("第一轮"), assistant_msg("答一")])
            .expect("seed turn 1");
        std::fs::write(
            execution.join("code.txt"),
            "v2
",
        )
        .expect("advance code");
        create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第二轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 2 snapshot");
        store
            .update_messages(
                &id,
                vec![
                    user_msg("第一轮"),
                    assistant_msg("答一"),
                    user_msg("第二轮"),
                    assistant_msg("答二"),
                ],
            )
            .expect("seed turn 2");

        // 协议走到「apply 完成 + 记录已写」后、截断持久化前失败；随后引擎旧
        // 副本把 transcript 原样重新落盘（revision 与暂记一致、记录已绑定）。
        let revision = {
            let session = store.load(&id).expect("load");
            crate::features::sessions::transcript_revision(&session.messages).expect("revision")
        };
        let target = checkpoints::find_checkpoint(&ledger, &first).expect("target");
        let (_t, undo) =
            checkpoints::prepare_restore(&ledger, &execution, &first, None).expect("prepare");
        checkpoints::write_pending_rewind(&ledger, &first, 1, &undo.id, &revision)
            .expect("pending");
        checkpoints::apply_restore(&ledger, &execution, &target).expect("apply");
        store
            .truncate_to_user_turn(&id, 1, Some(undo.id.clone()))
            .expect("truncate wrote the bound record");
        store
            .update_messages(
                &id,
                vec![
                    user_msg("第一轮"),
                    assistant_msg("答一"),
                    user_msg("第二轮"),
                    assistant_msg("答二"),
                ],
            )
            .expect("stale engine persist restores the full transcript");

        // 下一次发送：persist 分裂窗 → 复用 P1 幂等重放，不得 fresh PreRestore。
        let snapshot = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第三轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("persist 分裂窗重放后本轮快照必须照常登记");
        let rollbacks: Vec<String> = checkpoints::list_checkpoints(&ledger)
            .expect("list")
            .into_iter()
            .filter(|entry| entry.kind == CheckpointKind::PreRestore)
            .map(|entry| entry.id)
            .collect();
        assert_eq!(
            rollbacks,
            vec![undo.id.clone()],
            "persist 分裂窗重放必须复用 P1，不得 fresh PreRestore: {rollbacks:?}"
        );
        assert_eq!(
            checkpoints::count_user_turns(&store.load(&id).expect("load").messages),
            1,
            "重放必须完成截断"
        );
        assert!(
            !ledger.join("pending_rewind.json").exists(),
            "协议完成后暂记清除"
        );
        assert!(!snapshot.is_empty(), "本轮快照登记");
        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// r35 MAJOR-2 行为 pin：planted pending（伪造 pre_restore_id）必须被
    /// fail-closed 拒绝——不得触发重放臂的破坏性恢复 + 截断，工作区不动。
    #[tokio::test]
    async fn reconcile_rejects_planted_pre_restore_id() {
        if !git_available() {
            return;
        }
        let (store, _g, id, ledger, execution) = work_mode_session("planted-pending");
        std::fs::write(execution.join("code.txt"), "v1\n").expect("seed code");
        let first = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第一轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 1");
        store
            .update_messages(&id, vec![user_msg("第一轮"), assistant_msg("答一")])
            .expect("seed");
        // 写入伪造 pre_restore_id 的暂记——checkpoint_id 指向真实索引条目
        // （第一轮快照），但 pre_restore_id "c0-fake" 不在索引中。
        let revision = {
            let session = store.load(&id).expect("load");
            crate::features::sessions::transcript_revision(&session.messages).expect("revision")
        };
        checkpoints::write_pending_rewind(&ledger, &first, 0, "c0-fake", &revision).unwrap();
        // 对账：pre_restore_id 不在索引 → fail-closed 阻断（快照跳过），
        // 工作区不动、暂记不清。
        let snapshot = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第二轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await;
        assert!(
            snapshot.is_none(),
            "planted pending 必须 fail-closed 阻断快照"
        );
        assert!(
            checkpoints::live_pending_rewind(&ledger)
                .expect("pending read")
                .is_some(),
            "planted pending 不得被清除（fail-closed 阻断不清暂记）"
        );
        assert_eq!(
            std::fs::read_to_string(execution.join("code.txt")).unwrap(),
            "v1\n",
            "工作区不得被重放破坏"
        );
    }

    /// r24 M1：keep_turns ≥ 当前总轮数（CLI undo 跨进程收缩、带外编辑后）的
    /// 暂记是确定性不可满足——对账必须按空满足清掉暂记并放行本轮快照，
    /// **绝不重放 apply**（重放会先 force-checkout 旧边界树、再截断失败、
    /// 暂记永存 = 每条消息无限破坏性重放）。
    #[tokio::test]
    async fn reconcile_clears_vacuous_pending_without_replay() {
        if !git_available() {
            return;
        }
        let (store, _g, id, ledger, execution) = work_mode_session("reconcile-vacuous");
        std::fs::write(execution.join("code.txt"), "v1\n").expect("seed code");
        let first = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第一轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 1 snapshot");
        store
            .update_messages(&id, vec![user_msg("第一轮"), assistant_msg("答一")])
            .expect("seed turn 1");
        std::fs::write(execution.join("code.txt"), "v2\n").expect("advance code");
        create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第二轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 2 snapshot");

        // rewind₂ 语义：keep_turns=2 的暂记；随后 CLI undo 把 transcript 收缩
        // 到 1 轮（无 shrink hook 的跨进程路径）→ 总轮数 < keep_turns。
        let revision = {
            let session = store.load(&id).expect("load");
            crate::features::sessions::transcript_revision(&session.messages).expect("revision")
        };
        checkpoints::write_pending_rewind(&ledger, &first, 2, "pre-restore-x", &revision)
            .expect("pending");
        store
            .update_messages(&id, vec![user_msg("第一轮"), assistant_msg("答一")])
            .expect("cross-process undo shrinks the transcript");

        // 下一次发送：空满足 → 清暂记、放行快照、**代码不被 force-checkout**。
        let snapshot = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第三轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("空满足后本轮快照必须照常登记");
        assert!(
            !ledger.join("pending_rewind.json").exists(),
            "空满足必须清掉暂记"
        );
        assert_eq!(
            checkpoints::count_user_turns(&store.load(&id).expect("load").messages),
            1,
            "空满足不得改写 transcript"
        );
        assert!(!snapshot.is_empty(), "本轮快照登记");
        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// r23 M2：revision 前进 ≠ 截断已提交——非协议写（headless 引擎持久化、
    /// 压缩 persist、远程编辑）同样推进 revision。提交判定器是 sidecar 绑定
    /// 记录：截断失败（truncate 未跑、无绑定记录）后用户继续聊天，下一条
    /// 发送的对账必须**重放**协议，而不是误判已提交、清掉暂记，留下
    /// 「文件已回退 + 对话未回退 + undo 永久不可用」的静默分叉。
    #[tokio::test]
    async fn reconcile_replays_when_revision_advanced_without_commit() {
        if !git_available() {
            return;
        }
        let (store, _g, id, ledger, execution) = work_mode_session("reconcile-m2");
        std::fs::write(execution.join("code.txt"), "v1\n").expect("seed code");
        let first = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第一轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 1 snapshot");
        store
            .update_messages(&id, vec![user_msg("第一轮"), assistant_msg("答一")])
            .expect("seed turn 1");
        std::fs::write(execution.join("code.txt"), "v2\n").expect("advance code");
        create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第二轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 2 snapshot");

        // live-abort：apply 完成、truncate 未跑（sidecar 无绑定记录）。
        let revision = {
            let session = store.load(&id).expect("load");
            crate::features::sessions::transcript_revision(&session.messages).expect("revision")
        };
        let target = checkpoints::find_checkpoint(&ledger, &first).expect("target");
        let (_t, undo) =
            checkpoints::prepare_restore(&ledger, &execution, &first, None).expect("prepare");
        checkpoints::write_pending_rewind(&ledger, &first, 1, &undo.id, &revision)
            .expect("pending");
        checkpoints::apply_restore(&ledger, &execution, &target).expect("apply");

        // 用户继续聊天：非协议写推进 revision（截断从未发生、sidecar 为空）。
        store
            .update_messages(
                &id,
                vec![
                    user_msg("第一轮"),
                    assistant_msg("答一"),
                    user_msg("第二轮"),
                    assistant_msg("答二"),
                    user_msg("第三轮"),
                ],
            )
            .expect("keep chatting");

        // 下一次发送：对账必须重放协议（截断到第 1 轮），而非清暂记了事。
        let snapshot = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第四轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("重放后本轮快照必须照常登记");
        assert_eq!(
            checkpoints::count_user_turns(&store.load(&id).expect("load").messages),
            1,
            "无绑定记录 ⇒ 未提交 ⇒ 必须重放截断"
        );
        assert!(
            !ledger.join("pending_rewind.json").exists(),
            "协议完成后暂记清除"
        );
        assert_eq!(
            std::fs::read_to_string(execution.join("code.txt")).unwrap(),
            "v1\n",
            "重放的 apply 不得破坏已恢复状态"
        );
        assert!(!snapshot.is_empty(), "本轮快照登记");
        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// r23 M2 对照腿：截断已提交（sidecar 有绑定记录）但暂记未清（truncate
    /// 持久化之后、clear 之前崩溃）——对账必须只清暂记，不得重放破坏性恢复。
    #[tokio::test]
    async fn reconcile_clears_pending_without_replay_when_commit_recorded() {
        if !git_available() {
            return;
        }
        let (store, _g, id, ledger, execution) = work_mode_session("reconcile-committed");
        std::fs::write(execution.join("code.txt"), "v1\n").expect("seed code");
        let first = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第一轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 1 snapshot");
        store
            .update_messages(&id, vec![user_msg("第一轮"), assistant_msg("答一")])
            .expect("seed turn 1");
        std::fs::write(execution.join("code.txt"), "v2\n").expect("advance code");
        create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第二轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 2 snapshot");
        store
            .update_messages(
                &id,
                vec![
                    user_msg("第一轮"),
                    assistant_msg("答一"),
                    user_msg("第二轮"),
                    assistant_msg("答二"),
                ],
            )
            .expect("seed turn 2");

        // 走完整协议到「截断已持久化、暂记未清」的崩溃点。
        let revision = {
            let session = store.load(&id).expect("load");
            crate::features::sessions::transcript_revision(&session.messages).expect("revision")
        };
        let target = checkpoints::find_checkpoint(&ledger, &first).expect("target");
        let (_t, undo) =
            checkpoints::prepare_restore(&ledger, &execution, &first, None).expect("prepare");
        checkpoints::write_pending_rewind(&ledger, &first, 1, &undo.id, &revision)
            .expect("pending");
        checkpoints::apply_restore(&ledger, &execution, &target).expect("apply");
        store
            .truncate_to_user_turn(&id, 1, Some(undo.id.clone()))
            .expect("truncate commits, binding the sidecar record");
        assert_eq!(
            checkpoints::count_user_turns(&store.load(&id).expect("load").messages),
            1,
            "截断已提交"
        );

        // 下一次发送：committed 判定命中 → 只清暂记，不得再次恢复/截断。
        let snapshot = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第三轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("committed 分支本轮快照必须照常登记");
        assert_eq!(
            checkpoints::count_user_turns(&store.load(&id).expect("load").messages),
            1,
            "不得对已提交的截断重放（重复恢复/截断）"
        );
        assert!(!ledger.join("pending_rewind.json").exists(), "暂记必须清除");
        assert!(!snapshot.is_empty(), "本轮快照登记");
        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// F2：工作模式快照沿用 code_checkpoints 的内容策略——秘密文件与可再生
    /// 目录不进快照（diff 预览的可见面同理），普通源文件照常纳入。
    #[tokio::test]
    async fn work_mode_snapshot_excludes_secrets_and_regenerable_dirs() {
        if !git_available() {
            return;
        }
        let (store, _g, id, ledger, execution) = work_mode_session("work-exclusion");
        std::fs::create_dir_all(execution.join("src")).expect("src dir");
        std::fs::create_dir_all(execution.join("node_modules/pkg")).expect("dep dir");
        std::fs::write(execution.join("src/a.rs"), "a\n").expect("write");
        std::fs::write(execution.join(".env"), "SECRET=1\n").expect("write");
        std::fs::write(execution.join("node_modules/pkg/index.js"), "dep\n").expect("write");
        store
            .update_messages(&id, vec![user_msg("第一轮")])
            .expect("seed turn 1");
        let snapshot = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "第一轮".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("turn 1 snapshot");
        // turn 之后的改动构成 diff 预览（「回滚将撤销的变更」的可见面）。
        std::fs::write(execution.join("src/b.rs"), "b\n").expect("write");
        std::fs::write(execution.join(".env"), "SECRET=2\n").expect("write");
        std::fs::write(execution.join("node_modules/pkg/index.js"), "dep2\n").expect("write");
        let diff =
            checkpoints::diff_checkpoint(&ledger, &execution, &snapshot, false).expect("diff");
        let paths: Vec<&str> = diff.changes.iter().map(|c| c.path.as_str()).collect();
        assert!(
            paths.contains(&"src/b.rs"),
            "普通源文件变更必须可见: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| *p == ".env" || p.ends_with("/.env")),
            "秘密文件不得进快照: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.starts_with("node_modules/")),
            "可再生目录不得进快照: {paths:?}"
        );
        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// F3/F4/F4a：工作模式双层回退与仅对话回退——`conversation_only` 恒降级
    /// （快照存在也不动代码），截断后用户在执行根的手动改动保持原样；目标轮
    /// 快照缺失时同样可用；对照腿验证勾选「同时回退工作区文件」时定位 Turn
    /// 快照、恢复到该轮之前并自动留 PreRestore 回滚点（双层，F3）。
    #[test]
    fn work_mode_conversation_only_rewind_leaves_files_untouched() {
        if !git_available() {
            return;
        }
        let (store, _g, id, ledger, execution) = work_mode_session("work-conv-only");
        store
            .update_messages(
                &id,
                vec![
                    user_msg("第一轮"),
                    assistant_msg("答一"),
                    user_msg("第二轮"),
                    assistant_msg("答二"),
                ],
            )
            .expect("seed transcript");
        std::fs::create_dir_all(&ledger).expect("ledger dir");
        checkpoints::create_checkpoint(&ledger, &execution, Some(1), CheckpointKind::Turn, "t1")
            .expect("turn 1 snapshot");
        std::fs::write(execution.join("code.txt"), "turn-1-state\n").expect("write t1 state");
        checkpoints::create_checkpoint(&ledger, &execution, Some(2), CheckpointKind::Turn, "t2")
            .expect("turn 2 snapshot");
        std::fs::write(execution.join("code.txt"), "turn-2-state\n").expect("write t2 state");

        // F3 对照腿（勾选「同时回退工作区文件」）：定位 turn 2 快照、恢复文件到
        // 第 2 轮之前，restore 内部强制 PreRestore 回滚点（镜像 rewind_to_turn
        // 步骤 1/2 的编排：restore → truncate 绑定回滚点）。
        let entries = checkpoints::list_checkpoints(&ledger).expect("list");
        let plan = resolve_rewind_plan(entries, 1, false).expect("double-layer plan");
        assert!(!plan.degraded);
        let restore_target = plan.checkpoint.expect("turn 2 checkpoint");
        let undo = checkpoints::restore_checkpoint(&ledger, &execution, &restore_target.id)
            .expect("restore turn 2 checkpoint");
        store
            .truncate_to_user_turn(&id, 1, Some(undo.id.clone()))
            .expect("truncate to turn 1 with bound rollback point");
        assert_eq!(
            std::fs::read_to_string(execution.join("code.txt")).expect("read"),
            "turn-1-state\n",
            "双层回退必须把文件恢复到第 2 轮之前"
        );
        assert!(
            checkpoints::list_checkpoints(&ledger)
                .expect("list")
                .iter()
                .any(|entry| entry.kind == CheckpointKind::PreRestore && entry.id == undo.id),
            "双层回退必须自动留 PreRestore 回滚点（可反悔）"
        );

        // F4：conversation_only 恒降级（此后对话已在第 1 轮末，回退到第 0 轮）。
        // 工作模式常态：用户手动改动与 agent 改动交织——在 F3 的 restore 之后
        // 写入（restore 的 clean -fd 会移除目标快照之后的新文件，那正是双层
        // 回退的代价；本腿验证的是「仅回退对话」时文件保持原样）。
        std::fs::write(execution.join("notes.md"), "user manual edit\n").expect("write");
        let entries = checkpoints::list_checkpoints(&ledger).expect("list");
        let plan = resolve_rewind_plan(entries, 0, true).expect("degraded plan");
        assert!(plan.degraded);
        assert!(plan.checkpoint.is_none());
        // F4a：目标轮快照缺失（超预算/当时失败）同样可用。
        let plan = resolve_rewind_plan(Vec::new(), 1, true).expect("degraded plan");
        assert!(plan.degraded);
        // 镜像 rewind_to_turn 的 degraded 编排：跳过 restore，仅截断对话。
        store
            .truncate_to_user_turn(&id, 0, None)
            .expect("truncate to turn 0");
        assert_eq!(
            std::fs::read_to_string(execution.join("notes.md")).expect("read"),
            "user manual edit\n",
            "仅对话回退不得触碰文件"
        );
        assert!(
            store.load(&id).expect("load").messages.is_empty(),
            "对话应全部截断（回退到第 0 轮）"
        );
        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// F6：工作模式 Plan 会话的批准轮快照——accept_plan 与 chat 共用
    /// `create_turn_checkpoint`，其门（scheduled_profile.is_none）对 Plan 工作模式
    /// 会话放行，批准轮（is_user_turn_prompt 同口径计数的真实用户消息）照常登记。
    /// 时序镜像生产：快照先于该轮消息落盘。
    #[tokio::test]
    async fn work_mode_plan_approval_turn_snapshots() {
        if !git_available() {
            return;
        }
        let (store, _g, id, ledger, execution) = work_mode_session("work-plan");
        store
            .set_mode(&id, SerializableMode::Plan)
            .expect("plan mode");
        let plan_snapshot = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "给个方案".into(),
            "chat",
            ReconcileGates::default(),
        )
        .await
        .expect("plan turn snapshot");
        store
            .update_messages(&id, vec![user_msg("给个方案"), assistant_msg("方案…")])
            .expect("seed plan turn");
        // accept_plan 的门谓词：工作模式会话（非定时）放行。
        assert!(
            store.scheduled_profile(&id).is_none(),
            "accept_plan/chat 快照门对工作模式会话必须放行"
        );
        // 批准轮（「✅ 就这么干」是真实用户消息，同口径计数为第 2 轮）。
        let approval_snapshot = create_turn_checkpoint(
            &store,
            &id,
            ledger.clone(),
            execution.clone(),
            "✅ 就这么干".into(),
            "accept_plan",
            ReconcileGates::default(),
        )
        .await
        .expect("approval turn snapshot");
        store
            .update_messages(&id, vec![user_msg("✅ 就这么干")])
            .expect("seed approval turn");
        let turns: Vec<(Option<u32>, String)> = checkpoints::list_checkpoints(&ledger)
            .expect("list")
            .into_iter()
            .filter(|entry| entry.kind == CheckpointKind::Turn)
            .map(|entry| (entry.turn, entry.id))
            .collect();
        assert!(turns.contains(&(Some(1), plan_snapshot)));
        assert!(turns.contains(&(Some(2), approval_snapshot)));
        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// B1/B2：命令族车道拒绝——ACP 谓词命中或定时会话（ScheduledRun）时
    /// `resolve_native_session_roots` 以新文案拒绝；工作模式会话放行。
    /// `rewind_undo_state` 的软门（Ok(None)）见
    /// `undo_state_none_for_non_native_lane_sessions`。
    #[test]
    fn command_family_rejects_non_native_lane_sessions() {
        let (store, _g, id, ledger, execution) = work_mode_session("lane-reject");
        // B1：ACP 谓词命中（AcpPool::is_acp 的测试替身）。
        let error = resolve_native_session_roots(&id, &store, |probe| probe == id)
            .expect_err("ACP 会话必须拒绝");
        assert_eq!(error, "仅原生会话支持检查点");
        // B2：定时会话。
        let scheduled = store
            .create_scheduled_run(scheduled_profile_for_gate_test("task-lane"))
            .expect("scheduled run");
        let error = resolve_native_session_roots(&scheduled.metadata.id, &store, |_| false)
            .expect_err("定时会话必须拒绝");
        assert_eq!(error, "仅原生会话支持检查点");
        // 对照：工作模式会话放行，两根解析成功（未绑定 → 私有目录）。
        let (resolved_ledger, resolved_execution) =
            resolve_native_session_roots(&id, &store, |_| false).expect("roots");
        assert_eq!(resolved_ledger, ledger);
        assert_eq!(resolved_execution, execution);
        // B2 快照门谓词：定时会话 scheduled_profile 命中（chat.rs/accept_plan 的
        // !scheduled 门据此跳过快照），工作模式未命中。覆盖口径说明：chat/accept_plan
        // 命令本体依赖 State/EnginePool 无法单测，这里钉的是门读取的谓词本身
        // （与既有「手工镜像编排」测试同款取舍）；B2 的命令侧由上方
        // ScheduledRun 车道拒绝锚定。
        assert!(store.scheduled_profile(&scheduled.metadata.id).is_some());
        assert!(store.scheduled_profile(&id).is_none());
    }

    /// B3：非法 id（路径逃逸形状）被校验拦下、不 panic；合法形状但不存在的
    /// 会话 → 无状态可反悔（Ok(None)），同样不 panic。
    #[test]
    fn command_family_handles_invalid_and_missing_ids() {
        let (store, _g, _id, _ledger, _execution) = work_mode_session("invalid-id");
        let error = resolve_native_session_roots("../outside", &store, |_| false)
            .expect_err("非法 id 必须拒绝");
        assert!(
            error.contains("解析会话根失败"),
            "非法 id 必须被校验拦下: {error}"
        );
        let missing = format!(
            "missing-session-{}",
            crate::platform::paths::tests::unique_suffix()
        );
        let state = rewind_undo_state_inner(&store, |_| false, &missing).expect("不得 panic");
        assert_eq!(state, None, "不存在的会话没有可反悔状态");
    }
}
