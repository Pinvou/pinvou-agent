//! 多智能体（会话内主动委派，ADR-0006）的 Tauri 命令与每轮提醒文案。
//!
//! 与 `workflows.rs`（MVP1 skill 工作流 + 既有 Python 调度器）**无数据交集**，
//! 见 `docs/adr/0004-多智能体编排建在底座而非既有调度器.md`。这里只剩两类东西：
//! 子智能体执行记录的只读投影命令（行内专家卡 + 只读面板的数据源），以及
//! 开关开启时每轮拼进用户消息前的委派提醒（`delegation_reminder`，chat 发送
//! 链注入）。旧的独立发起命令与 wf- 会话形态已整体退役（开关在 interaction.rs）。

use super::prelude::*;

use crate::features::multiagent;

/// 多智能体模式的每轮委派提醒（拼在用户消息之前，chat 发送链注入）。
///
/// **每轮都要重申**：实测长上下文里模型对开头一次性教学的遵循率衰减（skill
/// phase marker 同款教训），信号放在距用户消息最近的位置。
///
/// 内容契约（单测钉死）：
/// - 只教裸 `agent` 集群（单任务一个、多阶段一群、父模型亲自协调汇总），
///   简单任务不委派。**不提 `workflow`**：底座把 read_only 子任务钳成四个
///   本地文件工具、结构化阶段默认不传递上游结果（`depends_on_results` 留空）
///   两处基线行为未修，正是真机"调研断网、汇总烧穿预算"事故的根因——在
///   底座修复前不向模型开放或推荐该路径；
/// - 名册只来自专家池（用户决策：不内置兜底角色——委派本质是写提示词）：
///   有合适专家用 `profile` 字段指定（`role` 会被当成内置类型别名截走，
///   命中不了名册人设）；没有就不带 `profile` 裸派，把角色定位与要求写进
///   任务说明；
/// - 名单必须随消息带上：底座不会把自定义名册列给主 agent（真机验证过它
///   只认内置别名），不带的话专家角色等于隐身；专家池为空时名单省略。
/// - 资源护栏：主会话是总协调者，普通委派使用 `max_depth=0` 成为叶子；只有
///   任务本身足够复杂时，第一层调用省略深度参数以继承会话上限并允许再拆
///   一层。第二层不得继续派生；全树最多 4 个同时执行、8 个排队 + 执行。
/// - Git 与子智能体工作区策略沿用普通对话语义，由父模型按任务自主决定；App
///   不把每个会话强制 git 化，也不封禁底座已有的 worktree 能力。
pub(crate) fn delegation_reminder() -> String {
    // 名册只来自专家池（用户决策：不内置兜底角色）。空名册不渲染空列表，
    // 转而教模型自拟任务说明裸派。
    let roles = multiagent::roster::available_role_lines();
    let roster_block = if roles.is_empty() {
        "（当前专家池为空，可在「专家池」页添加）".to_string()
    } else {
        format!("，可派 profile：\n{}", roles.join("\n"))
    };
    format!(
        "本会话已开启多智能体模式：请按任务形态**主动委派**，工具面与普通\
         对话完全一致（联网检索、读取网页等照常）：\n\
         1. 单个有边界的独立任务：用 `agent` 工具派一个子智能体去办，任务\
         说明写完整，交付物说清楚；\n\
         2. 多阶段任务（并行调研再汇总等）：并行的部分各派一个子智能体\
         （`agent` 后台并行），用 agents 协调工具等待并收取结果，由你亲自\
         汇总；需要接力时把上游结果放进下一个子智能体的任务说明里；\n\
         3. 资源边界：你是总协调者。普通委派调用 `agent` 时设 `max_depth=0`，\
         让直属子智能体成为叶子；只有任务本身足够复杂、确实需要它再拆分时，\
         第一层调用才省略 `max_depth`，并在任务说明中明确可按需再派一层。\
         第二层子智能体不得继续派生；不要传任何正数深度覆盖值。全树同时\
         执行最多 4 个，排队与执行合计最多 8 个；不要递归裂变；\n\
         4. Git 与工作区策略由你按任务自主完成：只读或不会互相覆盖的任务可用\
         默认共享工作区；同一 Git 仓库内有多个并行写入者、确需隔离时可传\
         `workspace_policy=worktree`。采用 worktree 前自行确认 Git 可用并准备好\
         目标仓库与有效基线；尚未拉取的仓库可先 clone，需要新建仓库时仅在\
         执行权限和用户任务允许的目标项目目录内初始化；不得把 `.codewhale/`\
         等运行时状态纳入版本控制。Git 不可用或准备失败时，说明原因并改用\
         安全的共享/串行方案，不要让整批委派失败；\n\
         5. 很简单的事：不必委派，自己直接做；\n\
         6. 承担者：专家池有合适人选就用 `profile` 字段指定{roster_block}；\
         没有合适人选就不带 `profile` 直接派，此时给子智能体起个一目了然\
         的名字，写在任务说明**第一行**的「」里（如「调研专家-AI新闻」，\
         界面用它显示身份），再写角色定位、能力边界与要求——委派本质就是\
         写好提示词；不要用 `role` 字段选专家（那是底座内置类型别名，命中\
         不了专家名册）；\n\
         7. 只读会话（Plan 档）下只派调研、审查类子智能体，不做写入；执行\
         会话（Yolo 档）可派执行型子智能体产出交付物；\n\
         8. 每个子任务的说明末尾写上：若因权限、环境或信息不可得而无法完成，\
         最终回复必须以 `[BLOCKED]` 开头并说明原因，不得把受阻说明伪装成完成。"
    )
}

/// 会话的子智能体工作区（底座 transcripts / worker ledger 都落在这里）。
fn subagent_workspace(session_id: &str) -> Result<std::path::PathBuf, String> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("非法会话 id: {session_id}"));
    }
    Ok(crate::platform::paths::session_workspace_dir(session_id))
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
    let workspace = subagent_workspace(&run_id)?;
    // 传引擎纪元而非"引擎是否存在"：重启后父会话重建引擎时，上一进程的
    // 僵尸 worker（落盘仍是 running）必须继续判 interrupted，见
    // transcripts::projected_worker_status。
    let engine_epoch_ms = pool.engine_epoch_ms(&run_id).await;
    // 文件 I/O 移出异步运行线程（复核 P2）：清单每 2s 被轮询一次。
    tokio::task::spawn_blocking(move || multiagent::transcripts::list(&workspace, engine_epoch_ms))
        .await
        .map_err(|join| format!("读取子智能体清单失败: {join}"))?
}

/// 读某个子智能体的完整对话记录（只读面板点开时调用）。
#[tauri::command]
pub async fn read_subagent_transcript(
    run_id: String,
    agent_id: String,
) -> Result<Vec<serde_json::Value>, String> {
    let workspace = subagent_workspace(&run_id)?;
    tokio::task::spawn_blocking(move || multiagent::transcripts::read(&workspace, &agent_id))
        .await
        .map_err(|join| format!("读取子智能体记录失败: {join}"))?
}

#[cfg(test)]
mod tests {
    use super::delegation_reminder;

    /// 每轮提醒教的是**主动委派**（ADR-0006）：只教裸 `agent` 集群（单任务
    /// 一个、多阶段一群、父模型亲自协调汇总），简单任务不委派。
    #[test]
    fn delegation_reminder_teaches_delegation() {
        let msg = delegation_reminder();
        assert!(msg.contains("主动委派"), "必须点名主动委派的行事方式");
        assert!(
            msg.contains("`agent` 工具"),
            "默认委派路径是裸 agent，必须点名"
        );
        assert!(
            msg.contains("由你亲自汇总"),
            "多阶段任务由父模型协调收束，结果经父上下文接力"
        );
        assert!(
            msg.contains("`max_depth=0`")
                && msg.contains("第一层调用才省略 `max_depth`")
                && msg.contains("第二层子智能体不得继续派生")
                && msg.contains("不要传任何正数深度覆盖值")
                && msg.contains("同时执行最多 4 个")
                && msg.contains("最多 8 个"),
            "多智能体必须明确两层委派和并发/准入资源边界"
        );
        assert!(
            msg.contains("Git 与工作区策略由你按任务自主完成")
                && msg.contains("`workspace_policy=worktree`")
                && msg.contains("默认共享工作区"),
            "Git 与 shared/worktree 策略必须交由模型按任务自主选择"
        );
        assert!(
            msg.contains("Git 不可用或准备失败") && msg.contains("共享/串行方案"),
            "worktree 前置条件不满足时必须教模型说明并安全降级"
        );
        assert!(
            msg.contains("不得把 `.codewhale/`") && msg.contains("运行时状态"),
            "模型自主维护 Git 时不得提交底座运行时状态"
        );
        assert!(
            !msg.contains("不要`workspace_policy=worktree`")
                && !msg.contains("不要传 `workspace_policy=worktree`")
                && !msg.contains("一律用默认共享工作区"),
            "不得再一刀切禁用底座 worktree 能力"
        );
        assert!(
            msg.contains("不必委派，自己直接做"),
            "简单任务允许不委派——不再强制编排"
        );
        assert!(
            msg.contains("`profile`") && msg.contains("不要用 `role`"),
            "承担者必须教 profile：role 会被底座当内置类型别名截走，命中不了名册"
        );
        assert!(
            msg.contains("Plan 档") && msg.contains("Yolo 档"),
            "只读/执行两档语义要教给模型"
        );
    }

    /// 名册只来自专家池（用户决策）：不得再出现内置角色行；无论名册是否
    /// 为空，都必须教"自拟任务说明裸派"这条路，且裸派要起「」名——底座
    /// role 字段只收 ASCII token，中文名只能走文本约定，界面据此显示身份。
    #[test]
    fn delegation_reminder_relies_on_expert_pool_only() {
        let msg = delegation_reminder();
        assert!(
            msg.contains("写好提示词"),
            "必须教模型自拟任务说明（无合适专家时裸派）"
        );
        assert!(
            msg.contains("「」") && msg.contains("起个一目了然"),
            "必须教模型给子智能体起名（任务说明第一行「」约定）：{msg}"
        );
        assert!(
            !msg.contains("scout：") && !msg.contains("builder：") && !msg.contains("manager："),
            "不得再有内置角色行：{msg}"
        );
    }

    /// workflow 路径在底座修复只读钳制与阶段结果传递前不得出现在提醒里；
    /// 也不得再教手写 script / plan 协议的任何碎片（真机事故的根因）。
    #[test]
    fn delegation_reminder_never_mentions_the_workflow_path() {
        let msg = delegation_reminder();
        assert!(
            !msg.contains("workflow"),
            "底座 read_only 工具钳制与阶段结果不传递未修，不得推荐 workflow：{msg}"
        );
        assert!(!msg.contains("task("), "不得教模型手写 task() 脚本：{msg}");
        assert!(
            !msg.contains("token_budget") && !msg.contains("phases"),
            "plan 协议字段不得出现：{msg}"
        );
        assert!(
            msg.contains("与普通对话完全一致"),
            "必须写明工具面继承普通对话（联网可用）"
        );
        assert!(
            msg.contains("[BLOCKED]"),
            "必须教模型给子任务立受阻返回约定，界面靠它区分真完成与受阻"
        );
    }

    /// 续行符丢失会把源码缩进嵌进消息正文——模型会照着奇怪的空白理解任务。
    /// （回归：此前正是因为字符串断行丢了 `\`，提示语里混进大段缩进。）
    #[test]
    fn delegation_reminder_contains_no_stray_indentation() {
        let msg = delegation_reminder();
        assert!(!msg.contains("  "), "提示语混入了源码缩进空格:\n{msg}");
    }
}
