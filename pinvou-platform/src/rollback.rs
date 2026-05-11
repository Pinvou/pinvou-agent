//! RollbackManager — 处理 slash 命令与状态机回退。
//!
//! 支持命令：
//! - `/back`        回到上一个 Done milestone
//! - `/skip`        跳过当前 milestone
//! - `/redo`        重做当前 milestone（清自身 context）
//! - `/replan`      重拆整体计划（state 改 ReplanMode，由调用方触发新一轮 LLM 调用）
//! - `/use <id>`    切换 agent（仅在 Q&A 模式或未拆解时合法）

use crate::workflow::{ConversationState, GlobalMode, MilestoneStatus};

/// 解析后的 slash 命令
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Back,
    Skip,
    Redo,
    Replan,
    Use(String),
}

/// 命令执行结果
#[derive(Debug, Clone)]
pub struct RollbackOutcome {
    /// 用户可见的反馈
    pub message: String,
    /// 是否成功改变了状态
    pub state_changed: bool,
    /// 调用方是否应该触发一次重新拆解（仅 Replan 会置 true）
    pub trigger_replan: bool,
    /// 切换 agent 的目标 id（仅 Use 会有）
    pub switch_agent: Option<String>,
}

impl RollbackOutcome {
    fn noop(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            state_changed: false,
            trigger_replan: false,
            switch_agent: None,
        }
    }

    fn changed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            state_changed: true,
            trigger_replan: false,
            switch_agent: None,
        }
    }
}

/// 检测输入是否为 slash 命令
pub fn is_slash_command(input: &str) -> bool {
    input.trim_start().starts_with('/')
}

/// 解析 slash 命令；不是命令或不识别的命令返回 None
pub fn parse_command(input: &str) -> Option<SlashCommand> {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix('/')?;
    let mut parts = rest.split_whitespace();
    let head = parts.next()?;
    match head {
        "back" => Some(SlashCommand::Back),
        "skip" => Some(SlashCommand::Skip),
        "redo" => Some(SlashCommand::Redo),
        "replan" => Some(SlashCommand::Replan),
        "use" => {
            let target = parts.next()?.to_string();
            Some(SlashCommand::Use(target))
        }
        _ => None,
    }
}

/// 在 state 上执行命令
pub fn execute(cmd: SlashCommand, state: &mut ConversationState) -> RollbackOutcome {
    match cmd {
        SlashCommand::Back => execute_back(state),
        SlashCommand::Skip => execute_skip(state),
        SlashCommand::Redo => execute_redo(state),
        SlashCommand::Replan => execute_replan(state),
        SlashCommand::Use(target) => execute_use(state, &target),
    }
}

fn execute_back(state: &mut ConversationState) -> RollbackOutcome {
    if state.global_mode == GlobalMode::QnA {
        return RollbackOutcome::noop("Q&A 模式无 milestone，/back 无效");
    }
    // 找最后一个 Done 的 milestone
    let prev_done = state
        .milestones
        .iter()
        .rev()
        .find(|(_, s)| *s == MilestoneStatus::Done)
        .map(|(m, _)| m.id.clone());

    let Some(target_id) = prev_done else {
        return RollbackOutcome::noop("没有可回退的步骤");
    };

    if state.rewind_to(&target_id) {
        state.global_mode = GlobalMode::Executing;
        RollbackOutcome::changed(format!("已回退到「{target_id}」"))
    } else {
        RollbackOutcome::noop("回退失败")
    }
}

fn execute_skip(state: &mut ConversationState) -> RollbackOutcome {
    if state.global_mode == GlobalMode::QnA {
        return RollbackOutcome::noop("Q&A 模式无 milestone，/skip 无效");
    }
    match state.skip_active() {
        Some(id) => RollbackOutcome::changed(format!("已跳过「{id}」")),
        None => RollbackOutcome::noop("没有可跳过的当前步骤"),
    }
}

fn execute_redo(state: &mut ConversationState) -> RollbackOutcome {
    if state.global_mode == GlobalMode::QnA {
        return RollbackOutcome::noop("Q&A 模式无 milestone，/redo 无效");
    }
    match state.redo_active() {
        Some(id) => RollbackOutcome::changed(format!("已重置当前步骤「{id}」")),
        None => RollbackOutcome::noop("没有可重做的当前步骤"),
    }
}

fn execute_replan(state: &mut ConversationState) -> RollbackOutcome {
    state.global_mode = GlobalMode::Replan;
    state.plan_initialized = false;
    // milestones 留待 CombinedPlanner 重新填充
    state.milestones.clear();
    state.question_counts.clear();
    // context 保留（让新计划自行决定哪些 required）
    RollbackOutcome {
        message: "已请求重新拆解，请发一条消息触发新计划".to_string(),
        state_changed: true,
        trigger_replan: true,
        switch_agent: None,
    }
}

fn execute_use(state: &mut ConversationState, target: &str) -> RollbackOutcome {
    // /use 仅在 QnA 或未初始化时合法（避免会话中段乱切）
    let can_switch = matches!(state.global_mode, GlobalMode::QnA)
        || !state.plan_initialized;
    if !can_switch {
        return RollbackOutcome::noop(
            "只有 Q&A 模式或会话开始时才能 /use 切换 agent；如需切换请先 /replan",
        );
    }
    state.set_agent(target);
    RollbackOutcome {
        message: format!("已切换到 agent「{target}」"),
        state_changed: true,
        trigger_replan: false,
        switch_agent: Some(target.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::Milestone;

    fn sample_milestones() -> Vec<Milestone> {
        vec![
            Milestone {
                id: "a".into(),
                label: "A".into(),
                ..Default::default()
            },
            Milestone {
                id: "b".into(),
                label: "B".into(),
                ..Default::default()
            },
            Milestone {
                id: "c".into(),
                label: "C".into(),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn detects_slash_prefix() {
        assert!(is_slash_command("/back"));
        assert!(is_slash_command("  /skip"));
        assert!(!is_slash_command("hello"));
        assert!(!is_slash_command(" hi /not-command"));
    }

    #[test]
    fn parses_known_commands() {
        assert_eq!(parse_command("/back"), Some(SlashCommand::Back));
        assert_eq!(parse_command("/skip"), Some(SlashCommand::Skip));
        assert_eq!(parse_command("/redo"), Some(SlashCommand::Redo));
        assert_eq!(parse_command("/replan"), Some(SlashCommand::Replan));
        assert_eq!(
            parse_command("/use planning"),
            Some(SlashCommand::Use("planning".into()))
        );
    }

    #[test]
    fn parses_unknown_returns_none() {
        assert_eq!(parse_command("/wat"), None);
        assert_eq!(parse_command("not a command"), None);
        assert_eq!(parse_command("/"), None);
    }

    #[test]
    fn use_without_target_returns_none() {
        assert_eq!(parse_command("/use"), None);
    }

    #[test]
    fn back_rewinds_to_last_done() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        state.mark_done("a"); // b is now active
        state.mark_done("b"); // c is now active

        let out = execute(SlashCommand::Back, &mut state);
        assert!(out.state_changed);
        assert!(out.message.contains("回退"));
        // 回到 b：a 还是 Done，b Active，c Pending
        assert_eq!(state.milestones[0].1, MilestoneStatus::Done);
        assert_eq!(state.milestones[1].1, MilestoneStatus::Active);
        assert_eq!(state.milestones[2].1, MilestoneStatus::Pending);
    }

    #[test]
    fn back_with_no_done_milestones_is_noop() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        let out = execute(SlashCommand::Back, &mut state);
        assert!(!out.state_changed);
    }

    #[test]
    fn skip_advances_active() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        let out = execute(SlashCommand::Skip, &mut state);
        assert!(out.state_changed);
        assert_eq!(state.milestones[0].1, MilestoneStatus::Skipped);
        assert_eq!(state.milestones[1].1, MilestoneStatus::Active);
    }

    #[test]
    fn redo_clears_own_context_and_keeps_active() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        state.set_context_with_origin("k", "v", "a");
        state.increment_question_count("a");

        let out = execute(SlashCommand::Redo, &mut state);
        assert!(out.state_changed);
        assert!(!state.context.contains_key("k"));
        assert_eq!(state.question_count("a"), 0);
        assert_eq!(state.milestones[0].1, MilestoneStatus::Active);
    }

    #[test]
    fn replan_sets_replan_mode_and_clears_milestones() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        state.set_context_with_origin("k", "v", "a");
        state.mark_done("a");

        let out = execute(SlashCommand::Replan, &mut state);
        assert!(out.state_changed);
        assert!(out.trigger_replan);
        assert_eq!(state.global_mode, GlobalMode::Replan);
        assert!(state.milestones.is_empty());
        assert!(!state.plan_initialized);
        // context 保留
        assert!(state.context.contains_key("k"));
    }

    #[test]
    fn use_in_qa_mode_switches_agent() {
        let mut state = ConversationState::new_qa("qa");
        let out = execute(SlashCommand::Use("planning".into()), &mut state);
        assert!(out.state_changed);
        assert_eq!(out.switch_agent.as_deref(), Some("planning"));
        assert_eq!(state.agent_id.as_deref(), Some("planning"));
    }

    #[test]
    fn use_during_execution_is_blocked() {
        let mut state = ConversationState::new("test".into(), sample_milestones());
        state.plan_initialized = true;

        let out = execute(SlashCommand::Use("planning".into()), &mut state);
        assert!(!out.state_changed);
        assert!(out.switch_agent.is_none());
        assert!(out.message.contains("/replan"));
    }

    #[test]
    fn qa_mode_ignores_milestone_commands() {
        let mut state = ConversationState::new_qa("qa");
        for cmd in [SlashCommand::Back, SlashCommand::Skip, SlashCommand::Redo] {
            let out = execute(cmd, &mut state);
            assert!(!out.state_changed);
        }
    }
}
