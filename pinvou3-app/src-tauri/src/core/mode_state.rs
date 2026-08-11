//! 会话 mode 的序列化镜像(跨层协议类型)。
//!
//! `AppMode` 不是 Serialize,pinvou3 这层用一个序列化友好的镜像 enum,
//! 跟前端 / settings.json(code_permission.last_mode)流通用,发给 engine 时
//! `to_app_mode()` 转回。被 platform(prefs)与 features(sessions/commands)
//! 共享,故定义在 core,避免 platform → features 反向依赖。

use deepseek_tui::tui::app::AppMode;
use serde::{Deserialize, Serialize};

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

/// 三个工作区 lane（工作 work / 设计 design / 代码 code）的全局默认 mode 标识。
/// 草稿态显式切换只写本 lane 的全局默认（`set_mode_default`）；已生成会话的
/// 切换只写会话自己的 per-session 记录，不渗全局（复审拍板的三分 lane 语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeLane {
    Work,
    Design,
    Code,
}

impl ModeLane {
    /// 从命令字符串参数解析；非法值给出明确错误（防 IPC 直调写入未知 lane）。
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "work" => Ok(Self::Work),
            "design" => Ok(Self::Design),
            "code" => Ok(Self::Code),
            other => Err(format!(
                "unknown mode lane: {other:?}（期望 work/design/code）"
            )),
        }
    }
}

/// `get_mode_defaults` 的返回视图：三个 lane 的全局默认 mode（None = 该 lane
/// 从未显式选过；前端缺省解析 code→plan、work/design→yolo）。
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ModeDefaultsView {
    pub work: Option<SerializableMode>,
    pub design: Option<SerializableMode>,
    pub code: Option<SerializableMode>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_yolo() {
        let s = SessionModeState::default();
        assert_eq!(s.mode, SerializableMode::Yolo);
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
            pending_plan_id: Some("plan-1".to_string()),
            plan_claim_in_flight: None,
            pinvou_review_enabled: false,
            active_skill: None,
            active_persona: None,
            pending_persona_body: None,
            mounted_collection: None,
            mounted_collections: Vec::new(),
            mounted_collections_revision: 0,
            multi_agent: false,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"mode\":\"plan\""));
        assert!(json.contains("\"pending_plan_id\":\"plan-1\""));
        assert!(!json.contains("plan_claim_in_flight"));
    }

    #[test]
    fn pinvou_review_default_off() {
        let s = SessionModeState::default();
        assert!(!s.pinvou_review_enabled);
    }
}
