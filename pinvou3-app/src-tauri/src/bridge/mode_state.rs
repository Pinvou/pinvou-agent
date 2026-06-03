//! Per-session mode + plan_phase 状态。
//!
//! pinvou3 把底座 `AppMode::Plan / Yolo` 简化暴露给用户，加一个 `PlanPhase`
//! 子状态机跟踪 Plan 流程的生命周期。
//!
//! 决策来源：`docs/Plan-YOLO双模式-设计决策.md` 第 3 节状态机。

use deepseek_tui::tui::app::AppMode;
use serde::{Deserialize, Serialize};

/// Per-session 绑定的 skill。在 session 上点工作流卡片"启用"时,
/// `commands::start_skill_session` 先 create_new() 再把这个结构挂到 session
/// 的 mode_state 上,后续这个 session 内的 chat 自动 prepend 一次性 instruction。
///
/// 设计:in-memory only(不持久化到 SavedSession.json) — skill 绑定是运行时
/// 状态,重启 app 后 session 仍可继续对话但 chips 不再驱动是合理的(用户可以
/// 重新点卡片新建另一个绑定 session)。PhaseDef 上游只 derive Serialize,
/// 所以这里也只单向序列化给前端,不走 deserialize 路径。
#[derive(Debug, Clone, Serialize)]
pub struct ActiveSkillBinding {
    pub name: String,
    /// skill.body + phase 追踪规则文案,首条消息 prepend 后置空。
    #[serde(skip)]
    pub pending_instruction: Option<String>,
    /// 前端渲染 chips 用的 phases 列表(JSON 透传)。
    pub phases: Vec<deepseek_tui::skills::PhaseDef>,
}

/// Plan 流程的子阶段。`mode = Plan` 时 phase 在 Planning/Ready 间流转；
/// 用户 accept plan 后 `mode = Yolo, phase = Executing`；执行完毕 `phase = None`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanPhase {
    /// YOLO 默认态，无 plan 流程
    None,
    /// 用户已进 Plan，AI 在调研 / 用户在讨论
    Planning,
    /// AI 已出完整 plan，等用户决策（✅/✏️/🚪）
    Ready,
    /// 用户已接受 plan，AI 在 YOLO 模式按 plan 执行
    Executing,
}

impl Default for PlanPhase {
    fn default() -> Self {
        Self::None
    }
}

/// 单 session 的 mode 状态。前端通过 `get_mode_state` 拉取，
/// `set_plan_mode_next` / `accept_plan` 等命令修改。
///
/// **品悟 review 是与 Plan/YOLO 正交的独立开关**(`pinvou_review_enabled`):
/// - Plan + 开 = plan 出炉 EXIT GATE + 任务收口 final review
/// - Plan + 关 = 现状行为
/// - YOLO + 开 = 只触发 final review(YOLO 无 plan 期)
/// - YOLO + 关 = 现状行为
///
/// careful hook 跨所有组合默认开启(由 DeepSeek-TUI shell.rs 强制 BLOCKED Dangerous 实现,
/// 不依赖此开关)。设计依据:docs/Pinvou-品悟设计.md §5。
///
/// **active_skill** 是工作流 phase 可视化 MVP1 加的 per-session 绑定字段:
/// 用户在工作流页点"启用" → start_skill_session 命令 create_new + 写这里 →
/// 切到该 session 时 chips strip 自动显示绑定 skill 的 phases。
///
/// 上游 PhaseDef 只 derive Serialize,所以这里也只单向序列化给前端;
/// SessionModeState 不需要从前端 deserialize 回来(它通过 set_*_state 命令逐字段写)。
#[derive(Debug, Clone, Serialize)]
pub struct SessionModeState {
    /// 当前激活 mode。`build_send_message_op` 用这个值。
    pub mode: SerializableMode,
    pub plan_phase: PlanPhase,
    /// 品悟 review 质量护栏开关。默认 false(保持现状)。
    /// 开启后 accept_plan / exit_plan_to_yolo 触发 EXIT GATE。
    #[serde(default)]
    pub pinvou_review_enabled: bool,
    /// 该 session 绑定的工作流 skill。`None` = 普通对话。
    #[serde(default)]
    pub active_skill: Option<ActiveSkillBinding>,
    /// 该 session 当前加持的专家面具 id（卡片池选中的 persona）。`None` = 未加持。
    /// 仅存 id，完整卡片由 `personas::get(id)` 解析；前端挂件按 id 在已拉取的池里查显示字段。
    /// 同 active_skill：in-memory only，重启 app 后丢失（可重新点卡加持）。
    #[serde(default)]
    pub active_persona: Option<String>,
    /// Side B: 加持后**一次性**注入的完整人设正文（agency-agents-zh body）。
    /// 加持时写入，该 session 下一条 chat 消费后置空（仿 active_skill.pending_instruction）。
    /// 之后每 turn 只靠 `equip_anchor` 轻锚点维持身份，不再重灌 body。
    #[serde(skip)]
    pub pending_persona_body: Option<String>,
}

impl Default for SessionModeState {
    fn default() -> Self {
        Self {
            mode: SerializableMode::Yolo,
            plan_phase: PlanPhase::None,
            pinvou_review_enabled: false,
            active_skill: None,
            active_persona: None,
            pending_persona_body: None,
        }
    }
}

/// `AppMode` 不是 Serialize，pinvou3 这层用一个序列化友好的镜像 enum，
/// 跟前端 / settings.json 流通用，发给 engine 时 `to_app_mode()` 转回。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SerializableMode {
    Plan,
    Yolo,
}

impl SerializableMode {
    pub fn to_app_mode(self) -> AppMode {
        match self {
            Self::Plan => AppMode::Plan,
            Self::Yolo => AppMode::Yolo,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_yolo_none() {
        let s = SessionModeState::default();
        assert_eq!(s.mode, SerializableMode::Yolo);
        assert_eq!(s.plan_phase, PlanPhase::None);
    }

    #[test]
    fn mode_round_trips_to_app_mode() {
        assert!(matches!(
            SerializableMode::Plan.to_app_mode(),
            AppMode::Plan
        ));
        assert!(matches!(
            SerializableMode::Yolo.to_app_mode(),
            AppMode::Yolo
        ));
    }

    #[test]
    fn serializes_to_snake_case() {
        let s = SessionModeState {
            mode: SerializableMode::Plan,
            plan_phase: PlanPhase::Planning,
            pinvou_review_enabled: false,
            active_skill: None,
            active_persona: None,
            pending_persona_body: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"mode\":\"plan\""));
        assert!(json.contains("\"plan_phase\":\"planning\""));
    }

    #[test]
    fn pinvou_review_default_off() {
        let s = SessionModeState::default();
        assert!(!s.pinvou_review_enabled);
    }
}
