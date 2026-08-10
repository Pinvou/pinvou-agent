//! Per-session mode 状态。
//!
//! pinvou3 把底座 `AppMode::Plan / Yolo` 二态暴露给用户。Plan 流程的交接
//! (出方案→accept→执行) 复用底座原生闭环,app 不再自建 phase 状态机。
//!
use deepseek_tui::tui::app::AppMode;
use serde::{Deserialize, Serialize};

/// Per-session 绑定的 skill。在 session 上点工作流卡片"启用"时,
/// `commands::start_skill_session` 先查找已有绑定同名 skill 的 session，
/// 找到则切回去（恢复工作流），找不到才 create_new()。
///
/// 持久化：`SessionStore::save_skill_bindings()` 把所有绑定写到
/// `~/.pinvou3/sessions/_skill_bindings.json`，启动时 `load_skill_bindings()` 恢复。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveSkillBinding {
    pub name: String,
    #[serde(skip)]
    pub pending_instruction: Option<String>,
    /// 前端渲染 chips 用的 phases 列表(JSON 透传)。
    #[serde(default)]
    /// (底座 v0.8.57 删除 PhaseDef;字段保留作前端/持久化兼容,恒为空)
    pub phases: Vec<serde_json::Value>,
    /// 该 session 绑定的工作流项目目录（所有工作流 session 都填充）。
    /// 当前是 `{workspace}/ppt-<ts>-<scenario>/`(历史前缀)，含 `_state/workflow_progress.json`。
    /// 持久化在 `_skill_bindings.json` 里跟随 binding 一起恢复，重启 app 后 harness
    /// 能继续找到对应项目。
    #[serde(default)]
    pub project_dir: Option<String>,
}

/// 会话挂载的单个本地知识集。`enabled=false` 保留挂载关系，但不参与检索。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MountedCollection {
    pub collection_id: i64,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MountedCollectionsSnapshot {
    pub revision: u64,
    pub collections: Vec<MountedCollection>,
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
/// careful hook 跨所有组合默认开启(由 CodeWhale shell.rs 强制 BLOCKED Dangerous 实现,
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
    /// Side B 卡片池:当前加持的专家卡 id(远端 persona 体系)。
    #[serde(default)]
    pub active_persona: Option<String>,
    /// Side B:待一次性注入的人设 body。
    #[serde(default, skip)]
    pub pending_persona_body: Option<String>,
    /// 当前激活 mode。`build_send_message_op` 用这个值。
    pub mode: SerializableMode,
    /// 当前可操作方案的服务端 ticket。新 plan_ready 会替换旧 ticket；
    /// accept/discard 必须 compare-and-consume，防多端旧卡重复执行。
    #[serde(default)]
    pub pending_plan_id: Option<String>,
    /// accept 已原子 claim、但 Engine mailbox 尚未确认的 ticket。
    /// 仅用于进程内失败回滚，不暴露给前端。
    #[serde(skip)]
    pub(crate) plan_claim_in_flight: Option<String>,
    /// 品悟 review 质量护栏开关。默认 false(保持现状)。
    /// 开启后 accept_plan / exit_plan_to_yolo 触发 EXIT GATE。
    #[serde(default)]
    pub pinvou_review_enabled: bool,
    /// 该 session 绑定的工作流 skill。`None` = 普通对话。
    #[serde(default)]
    pub active_skill: Option<ActiveSkillBinding>,
    /// 该 session 挂载的本地知识集 id(会话级粘连)。`None` = 未挂载。
    /// 挂上后每条 user 消息发送前,用消息文本对该集 `kb_retrieve`,把命中片段
    /// 当附件一样注入(见 `commands::chat`)。与 `active_persona` 一样仅驻内存,
    /// 不落盘——重启 app 后回到未挂载。
    #[serde(default)]
    pub mounted_collection: Option<i64>,
    /// 多知识库挂载事实源。旧单库字段保留给旧前端/远程端兼容读取。
    #[serde(default)]
    pub mounted_collections: Vec<MountedCollection>,
    /// 仅驻内存的并发版本号；通过专用 snapshot 命令对外提供，不混入 mode_state 协议。
    #[serde(skip)]
    pub mounted_collections_revision: u64,
    /// 多智能体模式开关（ADR-0006）：模型列表下方的会话级开关。开启后本会话
    /// 装配专家名册，并在每轮注入主动委派指令；关闭停止注入并回收引擎，取消
    /// 仍在后台运行的子智能体（工具面不随开关变化，与主线一致）。会话级记忆，经
    /// `sessions/_multi_agent.json` sidecar 持久化，重启不丢。
    #[serde(default)]
    pub multi_agent: bool,
}

impl Default for SessionModeState {
    fn default() -> Self {
        Self {
            mode: SerializableMode::Yolo,
            pending_plan_id: None,
            plan_claim_in_flight: None,
            pinvou_review_enabled: false,
            active_skill: None,
            active_persona: None,
            pending_persona_body: None,
            mounted_collection: None,
            mounted_collections: Vec::new(),
            mounted_collections_revision: 0,
            multi_agent: false,
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
