//! Per-session mode + plan_phase 状态。
//!
//! pinvou3 把底座 `AppMode::Plan / Yolo` 简化暴露给用户，加一个 `PlanPhase`
//! 子状态机跟踪 Plan 流程的生命周期。
//!
//! 决策来源：`docs/Plan-YOLO双模式-设计决策.md` 第 3 节状态机。

use deepseek_tui::tui::app::AppMode;
use serde::{Deserialize, Serialize};

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
/// careful hook 跨所有 mode 组合默认开启(由 DeepSeek-TUI shell.rs 强制
/// BLOCKED Dangerous 实现)。
///
/// 不需要从前端 deserialize 回来(它通过 set_*_state 命令逐字段写)。
#[derive(Debug, Clone, Serialize)]
pub struct SessionModeState {
    /// 当前激活 mode。`build_send_message_op` 用这个值。
    pub mode: SerializableMode,
    pub plan_phase: PlanPhase,
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
            active_persona: None,
            pending_persona_body: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"mode\":\"plan\""));
        assert!(json.contains("\"plan_phase\":\"planning\""));
    }
}
