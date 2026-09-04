//! 会话模式策略：把 plain/code 的行为差异收敛为数据，共享链路不再 if 分流。
//! 方向对齐 .luzeyang/code-plain-decoupling/code-native-agent-会话能力档案设计.md（已归档）。
//! 能力面分化是**编译期常量**（模式定义的一部分）：能力档案（编译内嵌 JSON +
//! 统一解析器）已退役——底座能力不做用户级运行期开关，没有写入者的运行期
//! 配置只是常量的间接层。能力差量收敛为一张静态表 `MODE_TABLE`（取代最后的
//! match 臂）；技能线不做设计期差量（运行时按模式 scope 开关 + 组合目录治理，
//! 见 skill_materialization）。
//!
//! 新增模式清单：`SessionMode` 变体 + `SessionMode::ALL`（`declare_all_modes`
//! 宏展开，漏挂即编译失败）+ 本表一行 + `pack_default_policy()` 决策
//! （AllowAll/DenyAll 是安全姿态决策，见 core::session_mode）——表项漏填由
//! 穷尽性测试兜底（测试遍历的 ALL 已被编译期哨兵绑定到枚举变体）。

use crate::core::session_mode::SessionMode;
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

/// `load_skill` 工具名。不再由本策略恒返回：skill 按模式 scope 治理（组合目录）
/// 落地后，code 会话按「组合目录是否为空」动态决定隐藏（见
/// bridge::shape_disallowed_tools，由表字段 `skills_empty_hides_load_skill`
/// 驱动）——空 → 隐藏（避免"开关开着但没技能"的假状态），非空 → 放行。
/// 方向对齐 .luzeyang/code-plain-decoupling/skill-scope-governance-实施方案.md（已归档）。
pub(crate) const LOAD_SKILL: &str = "load_skill";

/// 单模式的能力差量（编译期常量，[`MODE_TABLE`] 的行）。语义全部是"该模式
/// 架构上有/无此能力"（模式身份），不是用户偏好——不出现在任何开关面。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ModeCapabilities {
    /// 该模式不提供的底座工具（disallowed_tools 通道）。code：产物卡工具在
    /// 代码车道没有 UI 消费者，调用也无处渲染；plain：无（plain 曾默认禁
    /// Git 家族，决策放开——底座能力不做用户级开关）。
    /// `load_skill` 不在此列：其隐藏与否由组合目录是否为空动态决定。
    pub unavailable_tools: &'static [&'static str],
    /// 该模式不提供的内置自动技能（设计期差量）：组合目录物化时按 scope 排除，
    /// composer 内置技能行也不显示。code：视觉设计属产物能力（直出网页/海报/
    /// 简历），代码车道无 UI 消费者，故排除。
    pub unavailable_builtin_skills: &'static [&'static str],
    /// 组合目录为空时是否隐藏 `load_skill`（空态保护，V-5 联动；判定在
    /// bridge 侧做——目录检查是磁盘 I/O，策略对象保持纯数据）。
    pub skills_empty_hides_load_skill: bool,
}

/// 模式能力差量静态表：每模式一行，新增模式漏填由穷尽性测试兜底。
const MODE_TABLE: &[(SessionMode, ModeCapabilities)] = &[
    (
        SessionMode::Plain,
        ModeCapabilities {
            unavailable_tools: &[],
            unavailable_builtin_skills: &[],
            skills_empty_hides_load_skill: false,
        },
    ),
    (
        SessionMode::Code,
        ModeCapabilities {
            unavailable_tools: &["mcp_pinvou3_present_artifact"],
            unavailable_builtin_skills: &["visual-design"],
            skills_empty_hides_load_skill: true,
        },
    ),
];

/// 查表（crate 内共用：SessionPolicy::capabilities 与按 scope 查表的小 helper）。
/// 表是编译期静态的，缺项只能来自新增模式漏填——穷尽性测试会先于运行失败。
// A missing table row can only come from a newly added SessionMode without its
// row; the exhaustiveness test blocks it in CI first. Reaching this branch at
// runtime is a programming error, so panic is acceptable without a silent fallback.
#[allow(clippy::expect_used)]
fn capabilities_for(mode: SessionMode) -> ModeCapabilities {
    MODE_TABLE
        .iter()
        .find(|(m, _)| *m == mode)
        .map(|(_, caps)| *caps)
        .expect("MODE_TABLE 必须覆盖全部 SessionMode（穷尽性测试守护）")
}

/// 按 scope（即模式）查 `unavailable_builtin_skills` 表字段。技能组合目录物化
/// 用它排除该模式不提供的内置自动技能（如 code 模式的视觉设计）。
pub(crate) fn unavailable_builtin_skills_for(scope: SessionMode) -> &'static [&'static str] {
    capabilities_for(scope).unavailable_builtin_skills
}

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

    /// Whether this product mode supports Pinvou's opt-in multi-agent mode.
    ///
    /// This policy owns only the plain/code product axis. The bridge combines
    /// it with the native/external-ACP runtime axis before exposing capability.
    pub fn supports_multi_agent_mode(&self) -> bool {
        matches!(self.mode, SessionMode::Plain | SessionMode::Code)
    }

    /// 该模式的能力差量（编译期静态表 [`MODE_TABLE`] 查取）。
    pub(crate) fn capabilities(&self) -> ModeCapabilities {
        capabilities_for(self.mode)
    }

    /// 该模式不提供的底座工具（编译期常量表字段，disallowed_tools 通道）。
    pub fn unavailable_tools(&self) -> &'static [&'static str] {
        self.capabilities().unavailable_tools
    }

    /// 该模式不提供的内置自动技能（编译期常量表字段，组合目录物化排除）。
    pub fn unavailable_builtin_skills(&self) -> &'static [&'static str] {
        self.capabilities().unavailable_builtin_skills
    }

    // ── 运行行为语义方法 ──────────────────────────────────────────────
    // 能力部分是编译期常量表（capabilities/MODE_TABLE）；运行行为
    // （prompt 分层、项目规则注入等本质是代码行为）收敛为本组语义方法。
    // **全仓唯一的模式分支点集中在策略对象内**，消费点调用语义方法而非裸模式
    // 判断——新增模式的运行行为只改这里。
    // 语义命名表达"为什么"（绑项目目录/用代码层指令），而非"是什么模式"。

    /// 该模式绑定真实项目目录（决定 code_session_project_rules 注入等）。
    pub fn binds_project(&self) -> bool {
        matches!(self.mode, SessionMode::Code)
    }

    /// 该模式使用代码层 instructions（编码执行循环 + 代码场景纪律，无产物卡语义）。
    pub fn uses_code_instructions(&self) -> bool {
        matches!(self.mode, SessionMode::Code)
    }

    /// Returns whether this mode exposes Browser MCP tools (`mcp_browser_*`) for the headed
    /// Work-mode browser. Bridge configuration-path selection and injection of the browser-
    /// unavailable system message must use the same decision so registration and capability
    /// declarations remain aligned. This is their sole branch point.
    pub fn exposes_browser_mcp(&self) -> bool {
        matches!(self.mode, SessionMode::Plain)
    }

    /// Plan 模式 per-turn reminder。两模式同文：R-1 已为 code 页接上方案审批卡，
    /// reminder 描述的卡片交互对两模式都成立，无需分化。
    pub fn plan_reminder(&self) -> Option<&'static str> {
        Some(PLAN_REMINDER)
    }

    /// 审批参数（auto_approve, approval_mode）：本期两模式同为「全自动 + Auto」，
    /// 与 D-2 前共享 op 的写死值逐字节一致（行为不变）。S-1 安全分化（
    /// docs/code-mode-解耦与权限持久化-改动说明.md 挂起项）落地时按模式差异化，
    /// 调用点已策略取数，无需再动共享链路。
    pub fn approval_params(&self) -> (bool, ApprovalMode) {
        (true, ApprovalMode::Auto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table-driven dual-mode assertion (merging the two former standalone
    /// tests): locks every capability bit per mode, so a regression in any
    /// row pinpoints exactly which field flipped. See the per-field comments.
    #[test]
    fn mode_policy_capability_matrix() {
        struct Row {
            mode: SessionMode,
            // load_skill is not in the unavailable list: whether it is hidden
            // is decided dynamically by whether the composed skills dir is empty
            // (bridge::shape_disallowed_tools, driven by the
            // skills_empty_hides_load_skill field below).
            unavailable_tools: &'static [&'static str],
            // Design-time delta: code does not ship the builtin auto skill
            // "visual-design" (an artifact capability), plain has none (composed
            // dir materialization excludes it by scope, and the composer builtin
            // skill row is not shown either).
            unavailable_builtin_skills: &'static [&'static str],
            hides_load_skill_when_empty: bool,
            binds_project: bool,
            uses_code_instructions: bool,
        }
        let rows = [
            Row {
                mode: SessionMode::Code,
                unavailable_tools: &["mcp_pinvou3_present_artifact"],
                unavailable_builtin_skills: &["visual-design"],
                hides_load_skill_when_empty: true,
                binds_project: true,
                uses_code_instructions: true,
            },
            // plain has no mode-unavailable tools (the Git family was decided
            // to be open; foundation capabilities carry no user-level switch).
            Row {
                mode: SessionMode::Plain,
                unavailable_tools: &[],
                unavailable_builtin_skills: &[],
                hides_load_skill_when_empty: false,
                binds_project: false,
                uses_code_instructions: false,
            },
        ];
        for row in rows {
            let policy = SessionPolicy::for_mode(row.mode);
            assert_eq!(policy.mode(), row.mode, "{:?} mode", row.mode);
            assert!(
                policy.supports_multi_agent_mode(),
                "{:?} supports_multi_agent_mode",
                row.mode
            );
            assert_eq!(
                policy.unavailable_tools(),
                row.unavailable_tools,
                "{:?} unavailable_tools",
                row.mode
            );
            assert_eq!(
                policy.unavailable_builtin_skills(),
                row.unavailable_builtin_skills,
                "{:?} unavailable_builtin_skills",
                row.mode
            );
            assert_eq!(
                policy.capabilities().skills_empty_hides_load_skill,
                row.hides_load_skill_when_empty,
                "{:?} skills_empty_hides_load_skill",
                row.mode
            );
            assert_eq!(
                policy.binds_project(),
                row.binds_project,
                "{:?} binds_project",
                row.mode
            );
            assert_eq!(
                policy.uses_code_instructions(),
                row.uses_code_instructions,
                "{:?} uses_code_instructions",
                row.mode
            );
        }
    }

    /// 穷尽性：每个已注册模式都必须有表项——新增模式漏填表 → 本测试失败，
    /// 而不是运行期静默落到某个模式的差量。
    #[test]
    fn mode_table_covers_every_session_mode() {
        assert_eq!(MODE_TABLE.len(), SessionMode::ALL.len());
        for mode in SessionMode::ALL {
            assert!(
                MODE_TABLE.iter().any(|(m, _)| m == mode),
                "MODE_TABLE 缺少 {mode:?} 的表项"
            );
        }
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
