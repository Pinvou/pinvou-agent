//! 会话 mode 的跨层协议类型。
//!
//! pinvou3 UI 以 `Plan / Auto / Yolo` 三态呈现（tri-mode 安全模型）；
//! CodeWhale 的底座类型是 `AppMode::Plan / Agent`，序列化层把兼容名 Yolo
//! 映射到 Agent。审批姿态不进 mode：Auto = Suggest + 不自动批准（守卫命中
//! 按开关弹卡或拒绝），YOLO = Bypass + 自动批准 + `ask_on_guardrail`（命中
//! 底线守卫按开关弹卡或硬拒）。Plan 流程的交接(出方案→accept→回到进入
//! Plan 前的 mode)复用底座原生闭环,app 不自建 phase 状态机。
//!
//! 这里只保留跨层流通的协议类型(`SerializableMode`/`ModeLane`/
//! `ModeDefaultsView`);session 域聚合(`SessionModeState`/`ActiveSkillBinding`/
//! `MountedCollection*`)由 `features::sessions` 拥有并 re-export(见
//! `features/sessions/mode_state.rs`),避免 core 沉淀 feature 内部状态。
use deepseek_tui::AppMode;
use serde::{Deserialize, Serialize};

/// `AppMode` 不是 Serialize，pinvou3 这层用一个序列化友好的镜像 enum，
/// 跟前端 / settings.json 流通用，发给 engine 时 `to_app_mode()` 转回。
///
/// 历史：曾经只有 `Plan / Yolo` 两态（工作车道默认 Yolo，即全程自动批准）。
/// `Auto` 是后加的中间档（对齐 Claude Code `acceptEdits`/`default` 的主流
/// 形态）：普通可提示工具逐次请批，工作区内文件写走底座 carve-out 免弹卡。
/// 旧侧车文件里的 `"plan"/"yolo"` 照常反序列化。兼容面纪律（两条车道同规矩）：
/// settings.json 的 lane 默认永不写 `"auto"` 字面量——显式 Auto 选择走
/// `*_auto` 伴随旗（见 `platform::prefs`）；per-session 的
/// `_session_mode_states.json` 同样只保留 pre-tri-mode 词表——
/// `apply_session_mode_mutation_locked` 把 Auto 折叠成 `"plan"` 落共享表，
/// 显式选择以 `true` 旗记入 `_session_auto_modes.json` 伴随表。共享
/// PINVOU3_HOME 的 pre-tri-mode 二进制因此可以整表 strict 反序列化成功：
/// 降级不再隔离整表、不再在下次写入时把其他会话的选择从空表重建
/// （评审 P2 数据兼容要求），Auto 会话在其视图里保守降级为 Plan。
/// 语义与降级恢复边界见 `docs/safety-modes.md` §3.1/§6。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SerializableMode {
    Plan,
    Auto,
    Yolo,
}

impl SerializableMode {
    pub fn to_app_mode(self) -> AppMode {
        match self {
            Self::Plan => AppMode::Plan,
            Self::Auto | Self::Yolo => AppMode::Agent,
        }
    }

    /// Tri-mode 审批姿态映射 (auto_approve, approval_mode)——发给 engine 的
    /// `Op::SendMessage` 逐轮字段。
    ///
    /// - Plan `(false, Suggest)`：只读由底座 Plan 归一兜底（禁 shell、只读
    ///   sandbox），姿态值与底座 `base_policy_for_mode` 一致。
    /// - Auto `(false, Suggest)`：普通可提示工具逐次审批卡；工作区内文件写
    ///   走底座 carve-out 免弹卡（#5185），只读命令走 readonly 分类免弹卡。
    ///   对齐 Claude Code `default`/`acceptEdits` 的主流形态。
    /// - Yolo `(true, Bypass)`：自动批准（历史 Yolo 行为）。底线守卫命中
    ///   弹卡还是硬拒由 `ask_on_guardrail` 决定（engine 侧武装映射）。
    pub fn approval_params(self) -> (bool, deepseek_tui::ApprovalMode) {
        use deepseek_tui::ApprovalMode;
        match self {
            Self::Plan | Self::Auto => (false, ApprovalMode::Suggest),
            Self::Yolo => (true, ApprovalMode::Bypass),
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

/// Return view of `get_mode_defaults`: the global default mode per lane.
/// None = the lane was never explicitly chosen; resolution then falls to
/// the session kind (code/bound → Plan, unbound plain → Auto — tri-mode).
/// An explicit Auto choice is surfaced as `Some(Auto)` here: the disk
/// representation lives in dedicated `*_auto` companion flags, so the
/// legacy settings.json vocabulary stays parseable by pre-tri-mode binaries.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ModeDefaultsView {
    pub work: Option<SerializableMode>,
    pub code: Option<SerializableMode>,
}
