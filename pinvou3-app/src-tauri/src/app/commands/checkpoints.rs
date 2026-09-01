//! 代码会话 checkpoint 命令：turn 边界快照的查询、差异预览与回滚。
//!
//! 快照本体在 `features/code_checkpoints`（影子 git 仓库，数据落账本根）；本文件
//! 只做传输边界：原生代码会话校验、两个根解析、忙碌门与错误文案。仅品悟原生
//! code 会话可用（`SessionStore::is_code_session` 命中），ACP 会话与其余会话
//! 类型如实拒绝（设计 §11：ACP 不做）。

use super::prelude::*;
use crate::features::code_checkpoints as checkpoints;
use checkpoints::{CheckpointDiff, CheckpointKind, CheckpointMeta};

/// 解析原生代码会话的两个根：账本根（checkpoint 数据落点）+ 执行根（快照对象）。
/// 非原生代码会话、会话不存在、根不可用都如实报错，不静默降级。
fn resolve_code_session_roots(
    session_id: &str,
    store: &SessionStore,
) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    if !store.is_code_session(session_id) {
        return Err("仅原生代码会话支持检查点".to_string());
    }
    let roots = store
        .session_roots(session_id)
        .map_err(|error| format!("解析会话根失败: {error:#}"))?;
    Ok((roots.ledger, roots.execution))
}

#[tauri::command]
pub async fn list_checkpoints(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Vec<CheckpointMeta>, String> {
    // 列表只读账本根索引，不触碰执行根（执行根被删的历史会话仍可查看/清理认知）。
    if !store.is_code_session(&session_id) {
        return Err("仅原生代码会话支持检查点".to_string());
    }
    let ledger = store
        .session_roots(&session_id)
        .map_err(|error| format!("解析会话根失败: {error:#}"))?
        .ledger;
    tauri::async_runtime::spawn_blocking(move || {
        checkpoints::list_checkpoints(&ledger)
            .map_err(|error| format!("读取检查点列表失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("读取检查点列表任务失败: {error}"))?
}

#[tauri::command]
pub async fn checkpoint_diff(
    session_id: String,
    checkpoint_id: String,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<CheckpointDiff, String> {
    let (ledger, execution) = resolve_code_session_roots(&session_id, &store)?;
    // 软忙碌门：diff 会先 add -A 写影子 index，turn 进行中会与引擎写文件及
    // create_checkpoint 抢同一 index.lock（偶发失败或抓到写了一半的中间态），
    // 如实拒绝让前端稍后重试（确认弹窗只在空闲时可开，竞态窗口在点击之后）。
    if pool.is_turn_active(&session_id) {
        return Err("会话正在执行，请稍后再读取变更预览".to_string());
    }
    // 跨会话软门：同执行根的其它原生 code 会话在跑时同样抢影子 index.lock
    // （它的 create_checkpoint/引擎写文件 vs 本 diff 的 add -A 与迁移 purge）。
    let store_gate = store.inner().clone();
    let session_id_gate = session_id.clone();
    let pool_gate = pool.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(busy) =
            busy_peer_on_same_execution_root(&store_gate, &session_id_gate, &execution, |id| {
                pool_gate.is_turn_active(id)
            })?
        {
            return Err(format!("会话「{busy}」绑定同一项目目录且正在执行，请稍后再读取变更预览"));
        }
        checkpoints::diff_checkpoint(&ledger, &execution, &checkpoint_id)
            .map_err(|error| format!("读取检查点差异失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("读取检查点差异任务失败: {error}"))?
}

#[tauri::command]
pub async fn restore_checkpoint(
    session_id: String,
    checkpoint_id: String,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<CheckpointMeta, String> {
    let (ledger, execution) = resolve_code_session_roots(&session_id, &store)?;
    // 忙碌门：turn 进行中回滚会与引擎写文件竞争，如实拒绝（前端同时禁用入口）。
    // 先快速检查给出友好文案，再经 turn 预约机制原子占位消除竞态；预约持有到
    // 恢复完成（未提交，Drop 自动归还 slot），防止恢复期间新 turn 并发写文件。
    if pool.is_turn_active(&session_id) {
        return Err("会话正在执行，请先停止当前任务再回滚".to_string());
    }
    let reservation = pool
        .reserve_turn(&session_id)
        .map_err(|error| format!("预约会话 turn 失败（会话忙碌？）: {error:#}"))?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        checkpoints::restore_checkpoint(&ledger, &execution, &checkpoint_id)
            .map_err(|error| format!("回滚检查点失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("回滚检查点任务失败: {error}"))?;
    drop(reservation);
    result
}

/// `rewind_to_turn` 的编排结果（设计 §4：供前端刷新与提示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewindToTurnResult {
    /// 恢复代码时自动打的 PreRestore 回滚点（与 `restore_checkpoint` 返回值同语义，
    /// 供「已回退、可反悔」提示）；降级（仅回退对话）时为 None。
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

/// 跨会话忙碌门（设计 §6 防线二）：枚举会话，找出 execution 根相同的其它原生
/// 代码会话，任一 `is_busy` 即返回其展示名（标题，空则 id）。
///
/// 性能取舍：`store.list()` 与历史面板同价（读 ≤50 条会话元数据）；根解析只
/// 对 code session 做——`is_code_session` 是内存谓词零 IO，plain 会话不进
/// `session_roots`。
fn busy_peer_on_same_execution_root(
    store: &SessionStore,
    session_id: &str,
    execution_root: &std::path::Path,
    is_busy: impl Fn(&str) -> bool,
) -> Result<Option<String>, String> {
    let target = normalize_root(execution_root);
    let sessions = store
        .list()
        .map_err(|error| format!("枚举会话失败: {error:#}"))?;
    for metadata in sessions {
        if metadata.id == session_id || !store.is_code_session(&metadata.id) {
            continue;
        }
        let roots = match store.session_roots(&metadata.id) {
            Ok(roots) => roots,
            // 根解析失败的会话（如定时会话残留）无从比较执行根，跳过。
            Err(_) => continue,
        };
        if normalize_root(&roots.execution) == target && is_busy(&metadata.id) {
            let label = if metadata.title.is_empty() {
                metadata.id
            } else {
                metadata.title
            };
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
/// `restore_checkpoint` 命令不调本函数：它不动对话，turn 编号继续有效。
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
    store: State<'_, SessionStore>,
) -> Result<RewindToTurnResult, String> {
    let (ledger, execution) = resolve_code_session_roots(&session_id, &store)?;
    // 本会话忙碌门：同 restore_checkpoint，先快速检查给出友好文案，再原子占位；
    // 预约持有到编排结束（未提交，Drop 自动归还 slot）。
    if pool.is_turn_active(&session_id) {
        return Err("会话正在执行，请先停止当前任务再回退".to_string());
    }
    let reservation = pool
        .reserve_turn(&session_id)
        .map_err(|error| format!("预约会话 turn 失败（会话忙碌？）: {error:#}"))?;
    // 跨会话忙碌门：恢复单位是执行根，同根其它会话在跑时回退会撤销它正在写的
    // 文件，如实拒绝并告知哪个会话在忙。store.list() 全量读会话元数据是阻塞
    // IO，移出 async worker。
    let store_gate = store.inner().clone();
    let session_id_gate = session_id.clone();
    let execution_gate = execution.clone();
    let pool_gate = pool.inner().clone();
    if let Some(busy) = tauri::async_runtime::spawn_blocking(move || {
        busy_peer_on_same_execution_root(&store_gate, &session_id_gate, &execution_gate, |id| {
            pool_gate.is_turn_active(id)
        })
    })
    .await
    .map_err(|error| format!("跨会话忙碌检查任务失败: {error}"))??
    {
        return Err(format!("会话「{busy}」绑定同一项目目录且正在执行，请先停止该会话再回退"));
    }

    // 定位 checkpoint + 截断可行性预检。必须在 restore 之前：先恢复代码才发现
    // 对话无可截内容，会留下「代码已回退、对话未动」的不一致状态。
    let store_snapshot = store.inner().clone();
    let ledger_snapshot = ledger.clone();
    let session_id_snapshot = session_id.clone();
    let (entries, total_turns) = tauri::async_runtime::spawn_blocking(move || {
        let entries = checkpoints::list_checkpoints(&ledger_snapshot)
            .map_err(|error| format!("读取检查点列表失败: {error:#}"))?;
        let session = store_snapshot
            .load(&session_id_snapshot)
            .map_err(|error| format!("读取会话失败: {error:#}"))?;
        Ok::<_, String>((
            entries,
            checkpoints::count_user_turns(&session.messages),
        ))
    })
    .await
    .map_err(|error| format!("回退预检任务失败: {error}"))??;
    if keep_turns >= total_turns {
        return Err(format!("会话当前只有 {total_turns} 轮，无法回退到第 {keep_turns} 轮末尾"));
    }
    let plan = resolve_rewind_plan(entries, keep_turns, conversation_only)?;

    // 1) 恢复代码（内部强制 PreRestore，失败则中止）；降级模式跳过本步。
    let mut restored_checkpoint = None;
    if let Some(checkpoint) = plan.checkpoint {
        let ledger_restore = ledger.clone();
        let execution_restore = execution.clone();
        let undo = tauri::async_runtime::spawn_blocking(move || {
            checkpoints::restore_checkpoint(&ledger_restore, &execution_restore, &checkpoint.id)
                .map_err(|error| format!("回滚检查点失败: {error:#}"))
        })
        .await
        .map_err(|error| format!("回滚检查点任务失败: {error}"))??;
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
    let pre_restore_id = restored_checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.id.clone());
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        store_truncate.truncate_to_user_turn(&session_id_truncate, keep_turns, pre_restore_id)
    })
    .await
    .map_err(|error| format!("截断对话任务失败: {error}"))?
    .map_err(|error| match &restored_checkpoint {
        // 代码已回退而对话未截断：如实告知代码侧已变更并给出回滚点 id，
        // 用户可用「撤销回退」（备份已写时）或该回滚点恢复代码。
        Some(checkpoint) => format!(
            "代码已回退（回滚点 {}），但截断对话失败: {error:#}",
            checkpoint.id
        ),
        None => format!("截断对话失败: {error:#}"),
    })?;
    // 2.5) 作废被截对话分支的 Turn checkpoint（P0 修复：turn 序号会被重新创作
    //    复用，留着旧分支快照会让 first-wins 对齐锚到被遗弃分支）。降级模式同样
    //    要作废——对话已截断，turn 复用问题与是否恢复代码无关。git 子进程 +
    //    gc 是阻塞 IO，移出 async worker。
    let ledger_invalidate = ledger.clone();
    tauri::async_runtime::spawn_blocking(move || {
        invalidate_abandoned_turn_checkpoints(&ledger_invalidate, keep_turns);
    })
    .await
    .map_err(|error| format!("作废被截分支检查点任务失败: {error}"))?;
    // 3) 回收 engine 实例（复用删除会话的回收路径：cancel + Shutdown + abort
    //    forwarder）；下次发送时 get_or_spawn 未命中 → lazy respawn → 用截断后
    //    的磁盘历史 SyncSession 注水（engine_pool.rs 既有链路，设计 §4.2）。
    pool.evict(&session_id).await;
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

/// 查询侧主体（同步，便于测试）：非原生代码会话如实返回 None（不报错）。
fn rewind_undo_state_inner(
    store: &SessionStore,
    session_id: &str,
) -> Result<Option<RewindUndoInfo>, String> {
    if !store.is_code_session(session_id) {
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
    store: State<'_, SessionStore>,
) -> Result<Option<RewindUndoInfo>, String> {
    let store_inner = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || rewind_undo_state_inner(&store_inner, &session_id))
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
    store: State<'_, SessionStore>,
) -> Result<UndoLastRewindResult, String> {
    let (ledger, execution) = resolve_code_session_roots(&session_id, &store)?;
    // 忙碌门与 rewind_to_turn 同款：本会话原子占位 + 同执行根跨会话拒绝。
    if pool.is_turn_active(&session_id) {
        return Err("会话正在执行，请先停止当前任务再反悔".to_string());
    }
    let reservation = pool
        .reserve_turn(&session_id)
        .map_err(|error| format!("预约会话 turn 失败（会话忙碌？）: {error:#}"))?;
    let store_gate = store.inner().clone();
    let session_id_gate = session_id.clone();
    let execution_gate = execution.clone();
    let pool_gate = pool.inner().clone();
    if let Some(busy) = tauri::async_runtime::spawn_blocking(move || {
        busy_peer_on_same_execution_root(&store_gate, &session_id_gate, &execution_gate, |id| {
            pool_gate.is_turn_active(id)
        })
    })
    .await
    .map_err(|error| format!("跨会话忙碌检查任务失败: {error}"))??
    {
        return Err(format!("会话「{busy}」绑定同一项目目录且正在执行，请先停止该会话再反悔"));
    }

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
    .ok_or_else(|| "没有可反悔的回退（未回退过，或回退后已产生新轮次）".to_string())?;

    // 1) 恢复代码到本次回退绑定的 PreRestore（仅对话降级的回退无绑定，跳过
    //    本步——degraded 语义承诺绝不动代码）。对称语义：restore 内部会再打
    //    一次 PreRestore（内容为回退后的状态），「反悔的反悔」仍可恢复；该新
    //    回滚点不影响本 undo 的目标选择（绑定 id 已在 undo_info 里，重试不漂移）。
    if let Some(checkpoint_id) = undo_info.checkpoint_id.clone() {
        let ledger_restore = ledger.clone();
        let execution_restore = execution.clone();
        tauri::async_runtime::spawn_blocking(move || {
            checkpoints::restore_checkpoint(&ledger_restore, &execution_restore, &checkpoint_id)
                .map_err(|error| format!("恢复回滚点失败: {error:#}"))
        })
        .await
        .map_err(|error| format!("恢复回滚点任务失败: {error}"))??;
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
        match &undo_info.checkpoint_id {
            Some(checkpoint_id) => {
                eprintln!(
                    "[checkpoints] undo_last_rewind conversation restore failed for {session_id} after code restore to {checkpoint_id}: {error:#}"
                );
                format!("代码已恢复到回滚点，但对话恢复失败: {error:#}")
            }
            None => format!("对话恢复失败: {error:#}"),
        }
    })?;
    // 3) 回收 engine（同 rewind：下次发送 lazy respawn + SyncSession 重注水）。
    pool.evict(&session_id).await;
    drop(reservation);
    Ok(UndoLastRewindResult { restored_messages })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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

    /// 跨会话忙碌门：同执行根的其它原生 code 会话忙碌 → 返回其标题；不忙碌、
    /// 不同根、非 code 会话、自身忙碌都不拦截。
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

        // 仅 alice/bob 是原生 code 会话时：bob 忙碌 → 命中其标题。
        let code_ids = vec![alice_id.clone(), bob_id.clone()];
        store.set_code_session_predicate(Arc::new(move |id: &str| {
            code_ids.iter().any(|candidate| candidate == id)
        }));
        let busy_bob = bob_id.clone();
        let hit = busy_peer_on_same_execution_root(&store, &alice_id, &project, |id| id == busy_bob)
            .expect("gate");
        assert_eq!(hit, Some(bob.metadata.title.clone()));

        // 不忙碌 → 放行。
        let none = busy_peer_on_same_execution_root(&store, &alice_id, &project, |_| false)
            .expect("gate");
        assert_eq!(none, None);

        // 自身忙碌不算（门只看其它会话）。
        let busy_alice = alice_id.clone();
        let none = busy_peer_on_same_execution_root(&store, &alice_id, &project, |id| {
            id == busy_alice
        })
        .expect("gate");
        assert_eq!(none, None);

        // 非 code 会话即使同根且忙碌也不拦（ACP/plain 不归本门管）。
        let only_alice = alice_id.clone();
        store.set_code_session_predicate(Arc::new(move |id: &str| id == only_alice));
        let busy_bob = bob_id.clone();
        let none = busy_peer_on_same_execution_root(&store, &alice_id, &project, |id| id == busy_bob)
            .expect("gate");
        assert_eq!(none, None);
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
        let code_ids = vec![alice_id.clone(), carol_id.clone()];
        store.set_code_session_predicate(Arc::new(move |id: &str| {
            code_ids.iter().any(|candidate| candidate == id)
        }));

        let busy_carol = carol_id.clone();
        let none = busy_peer_on_same_execution_root(&store, &alice_id, &project, |id| {
            id == busy_carol
        })
        .expect("gate");
        assert_eq!(none, None, "不同执行根的忙碌会话不得拦截");
    }

    fn git_available() -> bool {
        crate::platform::process::HiddenCommand::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
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
        std::fs::write(exec.join("a.txt"), "1\n").expect("write");
        let old_t2 = checkpoints::create_checkpoint(&ledger, &exec, Some(2), CheckpointKind::Turn, "t2")
            .expect("c2");
        std::fs::write(exec.join("a.txt"), "2\n").expect("write");
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
        let new_t2 = checkpoints::create_checkpoint(&ledger, &exec, Some(2), CheckpointKind::Turn, "t2-new")
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

    /// 造一个带两轮对话、且已回退到第 1 轮的 code 会话；返回 (store, guard, 会话 id)。
    fn rewound_code_session(label: &str) -> (SessionStore, std::sync::MutexGuard<'static, ()>, String) {
        let (store, guard) = isolated_store(label);
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let id = session.metadata.id.clone();
        let code_id = id.clone();
        store.set_code_session_predicate(Arc::new(move |candidate: &str| candidate == code_id));
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
        store.truncate_to_user_turn(&id, 1, None).expect("rewind to turn 1");
        (store, guard, id)
    }

    /// undo 条件①③满足但回退后发过新轮次（条件②破）→ None。
    #[test]
    fn undo_state_none_after_new_turn_since_rewind() {
        let (store, _g, id) = rewound_code_session("undo-new-turn");
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
            rewind_undo_state_inner(&store, &id).expect("state"),
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
        let code_id = id.clone();
        store.set_code_session_predicate(Arc::new(move |candidate: &str| candidate == code_id));
        store
            .update_messages(&id, vec![user_msg("第一轮"), assistant_msg("答一")])
            .expect("seed");
        assert_eq!(rewind_undo_state_inner(&store, &id).expect("state"), None);
    }

    /// 非原生代码会话 → None（不报错）。
    #[test]
    fn undo_state_none_for_non_code_session() {
        let (store, _g) = isolated_store("undo-non-code");
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        // 谓词不含该会话（plain 会话）。
        store.set_code_session_predicate(Arc::new(|_: &str| false));
        assert_eq!(
            rewind_undo_state_inner(&store, &session.metadata.id).expect("state"),
            None
        );
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
        let code_id = id.clone();
        store.set_code_session_predicate(Arc::new(move |candidate: &str| candidate == code_id));
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
        std::fs::write(exec.join("code.txt"), "v1\n").expect("write v1");

        // 条件齐全 → Some，字段正确（被截 1 轮，checkpoint_id 为记录绑定的 PreRestore）。
        let info = rewind_undo_state_inner(&store, &id)
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
        let restored = store.restore_rewound_turns(&id).expect("restore conversation");
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
        assert_eq!(rewind_undo_state_inner(&store, &id).expect("state"), None);
        assert!(checkpoints::list_checkpoints(&ledger)
            .expect("list")
            .iter()
            .any(|entry| entry.kind == CheckpointKind::PreRestore));

        let _ = std::fs::remove_dir_all(&ledger);
    }

    /// M1 回归：仅对话降级回退（无代码快照）的记录不绑定 PreRestore——undo 只
    /// 还原对话，绝不因 index 里存在其它回退留下的 PreRestore 而误动代码。
    #[test]
    fn degraded_rewind_undo_never_restores_code() {
        if !git_available() {
            return;
        }
        let (store, _g, id) = rewound_code_session("undo-degraded");
        let roots = store.session_roots(&id).expect("roots");
        let ledger = roots.ledger.clone();
        let exec = roots.execution.clone();
        std::fs::create_dir_all(&ledger).expect("ledger dir");
        std::fs::write(exec.join("code.txt"), "user-work\n").expect("write");
        // 一次更早的完整回退留下的 PreRestore（与本次降级回退无关）。
        checkpoints::create_checkpoint(&ledger, &exec, None, CheckpointKind::PreRestore, "无关回滚点")
            .expect("unrelated pre-restore");
        std::fs::write(exec.join("code.txt"), "user-work-new\n").expect("write new");

        // 降级记录（rewound_code_session 以 None 绑定截断）：undo 状态可用但
        // checkpoint_id 必须是 None——不能错配到上面那条无关回滚点。
        let info = rewind_undo_state_inner(&store, &id)
            .expect("state")
            .expect("degraded undo available");
        assert_eq!(info.checkpoint_id, None, "降级回退的 undo 不得恢复代码");

        // undo 只追加对话：代码文件保持用户新作的内容。
        let restored = store.restore_rewound_turns(&id).expect("restore conversation");
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
        let code_id = id.clone();
        store.set_code_session_predicate(Arc::new(move |candidate: &str| candidate == code_id));
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
        std::fs::write(exec.join("code.txt"), "v1\n").expect("write v1");
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
        let info = rewind_undo_state_inner(&store, &id)
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
        let (store, _g, id) = rewound_code_session("undo-tail-edit");
        store
            .update_messages(
                &id,
                vec![user_msg("第一轮"), assistant_msg("答一（已编辑）")],
            )
            .expect("edit tail");
        assert_eq!(
            rewind_undo_state_inner(&store, &id).expect("state"),
            None,
            "回退后尾部被编辑必须不可反悔"
        );
        // restore 路径在 mutation 锁内复核，同样拒绝。
        assert!(store.restore_rewound_turns(&id).is_err());
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
        let code_id = id.clone();
        store.set_code_session_predicate(Arc::new(move |candidate: &str| candidate == code_id));
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
        let (store, _g, id) = rewound_code_session("undo-legacy");
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
}
