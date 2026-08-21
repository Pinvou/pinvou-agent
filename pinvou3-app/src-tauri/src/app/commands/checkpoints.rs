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
    store: State<'_, SessionStore>,
) -> Result<CheckpointDiff, String> {
    let (ledger, execution) = resolve_code_session_roots(&session_id, &store)?;
    tauri::async_runtime::spawn_blocking(move || {
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
    // 文件，如实拒绝并告知哪个会话在忙。
    if let Some(busy) = busy_peer_on_same_execution_root(&store, &session_id, &execution, |id| {
        pool.is_turn_active(id)
    })? {
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
    // 2) 截断对话（被截段落先备份进 `_rewound_turns.json`，备份失败则中止）。
    let outcome = store
        .truncate_to_user_turn(&session_id, keep_turns)
        .map_err(|error| format!("截断对话失败: {error:#}"))?;
    // 2.5) 作废被截对话分支的 Turn checkpoint（P0 修复：turn 序号会被重新创作
    //    复用，留着旧分支快照会让 first-wins 对齐锚到被遗弃分支）。降级模式同样
    //    要作废——对话已截断，turn 复用问题与是否恢复代码无关。
    invalidate_abandoned_turn_checkpoints(&ledger, keep_turns);
    // 3) 回收 engine 实例（复用删除会话的回收路径：cancel + Shutdown + abort
    //    forwarder）；下次发送时 get_or_spawn 未命中 → lazy respawn → 用截断后
    //    的磁盘历史 SyncSession 注水（engine_pool.rs 既有链路，设计 §4.2）。
    pool.evict(&session_id).await;
    drop(reservation);
    Ok(RewindToTurnResult {
        restored_checkpoint,
        rewound_turns: outcome.rewound_turns,
        degraded: plan.degraded,
    })
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
        std::env::set_var("PINVOU3_HOME", &dir);
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
}
