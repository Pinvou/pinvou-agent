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

/// 两个工作区 lane（工作 work / 代码 code）的全局默认 mode 标识。
/// 设计（design）lane 已并入 work lane；旧 settings.json 的 `mode_defaults.design`
/// 值在启动加载时折叠进 work 镜像（见 `features/sessions/store.rs`）。
/// 草稿态显式切换只写本 lane 的全局默认（`set_mode_default`）；已生成会话的
/// 切换只写会话自己的 per-session 记录，不渗全局（复审拍板的两分 lane 语义）。
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
            other => Err(format!("unknown mode lane: {other:?}（期望 work/code）")),
        }
    }
}

/// `get_mode_defaults` 的返回视图：两个 lane 的全局默认 mode（None = 该 lane
/// 从未显式选过；前端缺省解析 code→plan、work→yolo）。
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ModeDefaultsView {
    pub work: Option<SerializableMode>,
    pub code: Option<SerializableMode>,
}
