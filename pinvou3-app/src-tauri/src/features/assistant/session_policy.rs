//! 会话模式策略：把 plain/code 的行为差异收敛为数据，共享链路不再 if 分流。
//! 方向对齐 .luzeyang/code-plain-decoupling/code-native-agent-会话能力档案设计.md（已归档）。

use crate::core::session_mode::SessionMode;
use crate::features::marketplace::ConnectorScope;
use deepseek_tui::tui::approval::ApprovalMode;

/// Plan 模式 per-turn reminder:命令式、短、列禁令(Qwen3.6 友好)。写保护真防线是底座
/// 只读工具集 + ReadOnly sandbox,禁写条只是减少弱模型撞墙的引导(消融证非 load-bearing)。
///
/// 两模式同文:R-1 已为 code 页接上方案审批卡(plan_snapshot/plan_ready → accept_plan),
/// "方案卡片由系统自动展示"对 work/code 均为真实描述,无需按模式分化。
///
/// v0.9.5 起模型可见的进度工具只有 canonical `todo_write`(explanation/items 形式的
/// `update_plan` 与 `checklist_write` 均为隐藏 replay 别名,不进模型目录);决策卡由
/// engine 监听 todo_write 结果触发,方案步骤写进 todos.content,status 用 pending。
const PLAN_REMINDER: &str = "你现在在 Plan 模式(只读调研)。本 turn:\n\
     1. 想清楚后 → 调 `todo_write` 工具输出方案步骤(content 写清每一步,\
     status 用 pending;系统会在你调 todo_write 后自动展示方案卡片)。\n\
     2. **禁止**在 text 里描述方案/贴代码/写\"请点【就这么干】\"等按钮引导文字——\
     方案卡片由系统在你调 todo_write 后自动展示,你写引导是死锁。";

/// 原生代码会话没有产出物面板/成品卡语义(提示词也不再提及),隐藏 present_artifact。
const PRESENT_ARTIFACT: &str = "mcp_pinvou3_present_artifact";

/// `load_skill` 工具名。不再由本策略恒返回：skill 双 scope 治理（组合目录）落地后，
/// code 会话按「组合目录是否为空」动态决定隐藏（见 bridge::shape_disallowed_tools）——
/// 空 → 隐藏（避免"开关开着但没技能"的假状态），非空 → 放行。方向对齐
/// .luzeyang/code-plain-decoupling/skill-scope-governance-实施方案.md（已归档）。
pub(crate) const LOAD_SKILL: &str = "load_skill";

/// 单一会话模式的策略对象：共享链路（发送 op 构造、工具整形）按它取数，
/// 不再散 `is_code_session` 裸判断。reminder 同文（R-1 审批卡已落地）与审批
/// 参数（R-2）均已挂载；S-1 安全分化落地时改本对象取值即可。
#[derive(Debug, Clone, Copy)]
pub struct SessionPolicy {
    mode: SessionMode,
}

impl SessionPolicy {
    pub fn for_mode(mode: SessionMode) -> Self {
        Self { mode }
    }

    pub fn mode(&self) -> SessionMode {
        self.mode
    }

    /// Whether Pinvou may expose its opt-in multi-agent mode for this session.
    ///
    /// This product capability is Work-only for now. It must never be used to
    /// disable CodeWhale's ordinary `agent` or `workflow` tools in Code mode;
    /// those remain part of the base Code session behavior.
    pub fn multi_agent_mode_available(&self) -> bool {
        matches!(self.mode, SessionMode::Plain)
    }

    /// 连接器禁用集 scope：plain 用全局 scope，code 用 Code scope。
    pub fn connector_scope(&self) -> ConnectorScope {
        match self.mode {
            SessionMode::Plain => ConnectorScope::Plain,
            SessionMode::Code => ConnectorScope::Code,
        }
    }

    /// 该模式恒额外隐藏的工具（code：产物卡；load_skill 不在此列——其隐藏与否
    /// 由该会话组合目录是否为空动态决定，见 bridge::shape_disallowed_tools）。
    pub fn extra_hidden_tools(&self) -> &'static [&'static str] {
        match self.mode {
            SessionMode::Plain => &[],
            SessionMode::Code => &[PRESENT_ARTIFACT],
        }
    }

    /// Plan 模式 per-turn reminder。两模式同文：R-1 已为 code 页接上方案审批卡，
    /// reminder 描述的卡片交互对两模式都成立，无需分化。
    pub fn plan_reminder(&self) -> Option<&'static str> {
        Some(PLAN_REMINDER)
    }

    /// 审批参数（auto_approve, approval_mode）：本期两模式同为「全自动 + Auto」，
    /// 与 D-2 前共享 op 的写死值逐字节一致（行为不变）。S-1 安全分化（
    /// docs/code-plain-decoupling-改动说明.md 挂起项）落地时按模式差异化，
    /// 调用点已策略取数，无需再动共享链路。
    pub fn approval_params(&self) -> (bool, ApprovalMode) {
        (true, ApprovalMode::Auto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_policy_uses_code_scope_and_hides_artifact() {
        let policy = SessionPolicy::for_mode(SessionMode::Code);
        assert_eq!(policy.mode(), SessionMode::Code);
        assert!(
            !policy.multi_agent_mode_available(),
            "Pinvou 多智能体模式本期仅对 Work 开放"
        );
        assert_eq!(policy.connector_scope(), ConnectorScope::Code);
        // load_skill 不在恒隐藏列表：其隐藏与否由组合目录空否动态决定
        // （bridge::shape_disallowed_tools，V-5 联动）。
        assert_eq!(
            policy.extra_hidden_tools(),
            &["mcp_pinvou3_present_artifact"]
        );
    }

    #[test]
    fn plain_policy_uses_plain_scope_and_hides_nothing() {
        let policy = SessionPolicy::for_mode(SessionMode::Plain);
        assert_eq!(policy.mode(), SessionMode::Plain);
        assert!(policy.multi_agent_mode_available());
        assert_eq!(policy.connector_scope(), ConnectorScope::Plain);
        assert!(policy.extra_hidden_tools().is_empty());
    }

    /// 同文断言：R-1 审批卡落地后 reminder 对两模式都是真实描述，保持同文；
    /// 若未来真要按模式分化，须同步改这里。
    #[test]
    fn plan_reminder_is_same_text_for_both_modes_for_now() {
        let plain = SessionPolicy::for_mode(SessionMode::Plain).plan_reminder();
        let code = SessionPolicy::for_mode(SessionMode::Code).plan_reminder();
        assert_eq!(plain, Some(PLAN_REMINDER));
        assert_eq!(plain, code, "两模式 Plan reminder 必须同文(行为不变)");
    }

    /// R-2 行为不变断言：两模式审批参数均为「全自动 + Auto」，与 D-2 前共享 op
    /// 的写死值一致；S-1 分化时改这里并补差异断言。
    #[test]
    fn approval_params_are_full_auto_for_both_modes_for_now() {
        for mode in [SessionMode::Plain, SessionMode::Code] {
            let (auto_approve, approval_mode) = SessionPolicy::for_mode(mode).approval_params();
            assert!(auto_approve, "{mode:?} 本期必须全自动(行为不变)");
            assert_eq!(
                approval_mode,
                ApprovalMode::Auto,
                "{mode:?} 本期必须 Auto(行为不变)"
            );
        }
    }
}
