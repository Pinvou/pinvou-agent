//! 多智能体（会话内主动委派，ADR-0006）的 Tauri 命令与每轮 turn 装配。
//!
//! 这里只保留基于 CodeWhale 通用能力的两类东西：
//! 子智能体执行记录的只读投影命令（行内专家卡 + 只读面板的数据源），以及
//! 发送链的每轮装配（`prepare_delegation_turn`，会话交互中产生用户 turn 的
//! 发送链统一调用）。蜂群契约本体已上移为 spawn 级系统指令
//! （`features::assistant::swarm`，经 `EngineConfig.instructions` 注入一次），
//! 每轮动态内容只剩专家候选行，随快照一起交给 Engine route。
//! 旧的独立发起命令与 wf- 会话形态已整体退役（开关在 interaction.rs）。

use super::prelude::*;

use crate::features::assistant::expert_roster::ExpertRosterSnapshot;
use crate::features::multiagent;

/// 一次普通多智能体 turn 的模型内容与专家配置必须共用同一个快照。
/// `expert_snapshot = None` 表示普通/不可用会话：内容逐字保持不变，Engine route
/// 也不得注入专家。`expert_candidates` 是与同一快照同源的每轮候选行（已匹配、
/// 不会为凑数而为空），由 bridge 放进 `<system-reminder>` 信封；蜂群契约本体
/// 不在这里——它在 spawn 级 instructions（`swarm::SWARM_CONTRACT`）。
pub(crate) struct PreparedDelegationTurn {
    pub content: String,
    pub expert_snapshot: Option<std::sync::Arc<ExpertRosterSnapshot>>,
    pub expert_candidates: Vec<String>,
}

/// 候选匹配的输入文本（用户消息原文或计划原文），与 `content`（实际发送的
/// 组装稿）刻意区分成不同类型：两个字符串参数按位置传反仍能编译，而类型
/// 区分让“匹配看原文、发送看组装稿”的次序错误直接变成编译错误。
pub(crate) struct MatchSource<'a>(pub &'a str);

pub(crate) fn prepare_delegation_turn(
    pool: &EnginePool,
    session_id: &str,
    enabled: bool,
    content: String,
    match_source: MatchSource<'_>,
) -> PreparedDelegationTurn {
    prepare_delegation_turn_impl(
        enabled,
        pool.swarm_mode_available(session_id),
        content,
        match_source.0,
    )
}

/// [`prepare_delegation_turn`] 的可测主体（EnginePool 依赖 AppHandle，单测无法
/// 实例化；availability 拆成布尔入参）。
///
/// 开关关闭或蜂群对当前会话不可用（如定时任务）时：内容逐字透传，不带快照
/// （engine 侧的 hard-error 不变式要求多智能体 turn 必带快照，反之普通 turn
/// 必不带）。开启时捕获一次快照，同一份快照同时产出候选行与引擎 fleet 配置，
/// 避免"候选里有、派工时没有"的错位；用户内容保持原文——契约在系统指令里，
/// 不再逐轮改写消息。
///
/// `match_source` 是候选匹配的输入，与 `content`（实际发送内容）分离：匹配只看
/// 用户原文/计划原文，不看组装后的注入文本（persona 正文、KB 引导、附件引用），
/// 否则注入文本里的领域词会虚假抬升无关专家卡的得分。
fn prepare_delegation_turn_impl(
    enabled: bool,
    available: bool,
    content: String,
    match_source: &str,
) -> PreparedDelegationTurn {
    if !enabled || !available {
        return PreparedDelegationTurn {
            content,
            expert_snapshot: None,
            expert_candidates: Vec::new(),
        };
    }
    let snapshot = ExpertRosterSnapshot::capture();
    let expert_candidates = snapshot.available_role_lines(match_source);
    PreparedDelegationTurn {
        content,
        expert_snapshot: Some(snapshot),
        expert_candidates,
    }
}

/// 多智能体会话私有的 CodeWhale delegated-agent 状态根。
fn subagent_state_root(session_id: &str, pool: &EnginePool) -> Result<std::path::PathBuf, String> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("非法会话 id: {session_id}"));
    }
    if !pool.multi_agent_mode_available(session_id) {
        return Err("当前会话不支持子智能体执行记录".to_string());
    }
    pool.session_state_root(session_id)
}

/// 列出一次会话派发过的子智能体（读工作区对话记录的表头）。
///
/// 行内专家卡与只读面板的数据源。记录由底座落盘（`.codewhale/state/
/// subagent-transcripts/` + worker ledger），会话结束、进程重启后依然可查。
#[tauri::command]
pub async fn list_subagent_transcripts(
    run_id: String,
    pool: State<'_, EnginePool>,
) -> Result<Vec<multiagent::transcripts::SubagentTranscriptSummary>, String> {
    let state_root = subagent_state_root(&run_id, &pool)?;
    // 传引擎纪元而非"引擎是否存在"：重启后父会话重建引擎时，上一进程的
    // 僵尸 worker（落盘仍是 running）必须继续判 interrupted，见
    // transcripts::projected_worker_status。
    let engine_epoch_ms = pool.engine_epoch_ms(&run_id).await;
    // 文件 I/O 移出异步运行线程（复核 P2）：清单每 2s 被轮询一次。
    tokio::task::spawn_blocking(move || multiagent::transcripts::list(&state_root, engine_epoch_ms))
        .await
        .map_err(|join| format!("读取子智能体清单失败: {join}"))?
}

/// 增量读取某个子智能体的对话记录（只读面板点开时调用）。客户端把上次
/// 返回的 offset/revision 原样带回；游标失效时后端返回 reset chunk。
#[tauri::command]
pub async fn read_subagent_transcript(
    run_id: String,
    agent_id: String,
    offset: Option<u64>,
    revision: Option<String>,
    pool: State<'_, EnginePool>,
) -> Result<multiagent::transcripts::SubagentTranscriptChunk, String> {
    let state_root = subagent_state_root(&run_id, &pool)?;
    tokio::task::spawn_blocking(move || {
        multiagent::transcripts::read_chunk(&state_root, &agent_id, offset, revision.as_deref())
    })
    .await
    .map_err(|join| format!("读取子智能体记录失败: {join}"))?
}

#[cfg(test)]
mod tests {
    use super::prepare_delegation_turn_impl;
    use crate::features::assistant::expert_roster::tests::PersonaHomeGuard;
    use crate::platform::paths::tests::ENV_LOCK;

    /// "审查 React 前端代码" 历来命中内置前端专家（旧提醒测试同款任务），
    /// 用它验证候选行确实来自快照匹配器而非恒空。
    const MATCHING_TASK: &str = "审查 React 前端代码";
    /// 无语义的 ASCII 串：匹配器的 n-gram 词不会出现在任何内置卡摘要里，
    /// 隔离后的卡池（仅内嵌 268 卡，无用户卡）对该任务候选为空。
    const NO_MATCH_TASK: &str = "zzqqxx wubbo jubbo";

    /// 开关开启且蜂群可用：内容逐字保持原文（不再拼接任何提醒），快照与
    /// 候选行同源存在，且候选确实来自匹配器。
    #[test]
    fn prepare_keeps_content_raw_and_pairs_snapshot_with_candidates() {
        let _env = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PersonaHomeGuard::setup("commands-prepared-turn");
        let prepared =
            prepare_delegation_turn_impl(true, true, MATCHING_TASK.to_string(), MATCHING_TASK);
        assert_eq!(prepared.content, MATCHING_TASK, "内容必须逐字保持原文");
        let snapshot = prepared
            .expert_snapshot
            .expect("enabled turn must carry the snapshot");
        assert!(
            !prepared.expert_candidates.is_empty(),
            "匹配任务必须产出候选行"
        );
        // 候选行与快照同源：同一次 capture 的匹配器对同一任务产出一致。
        assert_eq!(
            prepared.expert_candidates,
            snapshot.available_role_lines(MATCHING_TASK),
            "候选行必须来自同一个快照"
        );
        for line in &prepared.expert_candidates {
            assert!(line.starts_with("- `"), "候选行必须是既定行格式：{line}");
        }
    }

    /// 开关关闭或会话不可用（定时任务等）：内容逐字透传，无快照无候选——
    /// engine 侧普通 turn 也不得携带专家快照（hard-error 不变式的另一半）。
    #[test]
    fn prepare_disabled_or_unavailable_passes_content_verbatim() {
        for (enabled, available) in [(false, true), (true, false), (false, false)] {
            let prepared = prepare_delegation_turn_impl(
                enabled,
                available,
                format!("{MATCHING_TASK}（不应被改写）"),
                MATCHING_TASK,
            );
            assert_eq!(
                prepared.content,
                format!("{MATCHING_TASK}（不应被改写）"),
                "enabled={enabled} available={available} 时内容必须逐字透传"
            );
            assert!(
                prepared.expert_snapshot.is_none(),
                "enabled={enabled} available={available} 不得带快照"
            );
            assert!(
                prepared.expert_candidates.is_empty(),
                "enabled={enabled} available={available} 不得带候选"
            );
        }
    }

    /// 无匹配任务：候选为空（bridge 层兜底一句名册提示），但快照仍在——引擎
    /// fleet 配置与候选行共用同一快照，不能因为本轮没有候选就不带快照。
    #[test]
    fn prepare_without_matching_experts_keeps_snapshot_with_empty_candidates() {
        let _env = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PersonaHomeGuard::setup("commands-prepared-turn-empty");
        let prepared =
            prepare_delegation_turn_impl(true, true, NO_MATCH_TASK.to_string(), NO_MATCH_TASK);
        assert!(
            prepared.expert_snapshot.is_some(),
            "快照必须始终随 turn 携带"
        );
        assert!(
            prepared.expert_candidates.is_empty(),
            "无匹配时候选必须为空: {:?}",
            prepared.expert_candidates
        );
        assert_eq!(prepared.content, NO_MATCH_TASK);
    }

    /// 候选匹配只看 `match_source`（用户/计划原文），不看 `content`（组装后
    /// 实际发送的文本）：persona 正文、KB 引导等注入文本里的领域词不得虚假
    /// 抬升无关专家卡。反向亦然：原文命中时，噪声 content 不影响候选。
    #[test]
    fn prepare_matches_on_match_source_not_assembled_content() {
        let _env = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PersonaHomeGuard::setup("commands-prepared-turn-source");
        let injected =
            format!("系统指引：请始终遵循 React 前端工程规范。\n\n---\n\n{NO_MATCH_TASK}");
        let prepared = prepare_delegation_turn_impl(true, true, injected, NO_MATCH_TASK);
        assert!(
            prepared.expert_candidates.is_empty(),
            "注入文本里的领域词不得凭空产出候选: {:?}",
            prepared.expert_candidates
        );
        let noisy_content = format!("{MATCHING_TASK}\n\nzzqqxx wubbo jubbo");
        let prepared = prepare_delegation_turn_impl(true, true, noisy_content, MATCHING_TASK);
        assert!(
            !prepared.expert_candidates.is_empty(),
            "匹配源命中时必须产出候选（content 噪声不影响）"
        );
    }

    /// 类型系统只保证 `prepare_delegation_turn` 的参数次序（`String` 与
    /// `MatchSource` 类型不同），保证不了调用点把「哪根串」交给 `MatchSource`
    /// ——两个实参都是 String，`MatchSource(&full)` 也能编译。这里把两个生产
    /// 调用点的语义选择钉在源码上（随 006682ea 移除的 node 侧正则钉的 Rust
    /// 替身，rust-test 对任何 Rust 改动必跑）。
    #[test]
    fn match_source_call_sites_pass_the_unassembled_text() {
        let chat = include_str!("chat.rs");
        assert!(
            chat.contains("super::multiagent::MatchSource(&raw_message)"),
            "chat 发送链必须以用户原文 raw_message 作为候选匹配源"
        );
        let interaction = include_str!("interaction.rs");
        assert!(
            interaction.contains("super::multiagent::MatchSource(&plan_markdown)"),
            "accept_plan 必须以计划原文 plan_markdown 作为候选匹配源"
        );
    }
}
