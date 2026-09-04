//! 会话 mode 的跨层协议类型。
//!
//! pinvou3 把底座 `AppMode::Plan / Yolo` 二态暴露给用户。Plan 流程的交接
//! (出方案→accept→执行) 复用底座原生闭环,app 不再自建 phase 状态机。
//!
//! 这里只保留跨层流通的协议类型(`SerializableMode`/`ModeLane`/
//! `ModeDefaultsView`);session 域聚合(`SessionModeState`/`ActiveSkillBinding`/
//! `MountedCollection*`)由 `features::sessions` 拥有并 re-export(见
//! `features/sessions/mode_state.rs`),避免 core 沉淀 feature 内部状态。
use deepseek_tui::tui::app::AppMode;
use serde::{Deserialize, Serialize};

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

/// Identifies the workspace lane (work / code) a global default mode belongs to.
/// The design lane has been merged into the work lane; a legacy
/// `mode_defaults.design` value in settings.json is folded into the work
/// mirror at startup load (see `features/sessions/store.rs`).
/// An explicit switch in the draft state writes only this lane's global
/// default (`set_mode_default`); a switch in an already-materialized session
/// writes only that session's per-session record and never leaks into
/// globals (the two-lane semantics settled in review).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeLane {
    Work,
    Code,
}

impl ModeLane {
    /// 从命令字符串参数解析；非法值给出明确错误（防 IPC 直调写入未知 lane）。
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "work" => Ok(Self::Work),
            "code" => Ok(Self::Code),
            other => Err(format!("unknown mode lane: {other:?} (expected work/code)")),
        }
    }
}

/// Return view of `get_mode_defaults`: the global default mode per lane
/// (None = the lane was never explicitly chosen; the frontend resolves
/// defaults as code→plan, work→yolo).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ModeDefaultsView {
    pub work: Option<SerializableMode>,
    pub code: Option<SerializableMode>,
}
