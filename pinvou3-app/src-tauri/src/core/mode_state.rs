//! 会话 mode 的跨层协议类型。
//!
//! The pinvou3 UI presents three states, `Plan / Auto / Yolo` (tri-mode safety model);
//! CodeWhale's base type is `AppMode::Plan / Agent`, and the serialization layer maps the compatibility
//! name Yolo onto Agent. The approval stance is not part of the mode: Auto = Suggest + no auto-approval
//! (guardrail hits raise a card or deny per the toggle), YOLO = Bypass + auto-approval + `ask_on_guardrail`
//! (baseline guardrail hits raise a card or hard-deny per the toggle). The Plan flow's handoff
//! (propose → accept → back to the pre-Plan mode) reuses the base's native closed loop; the app does not build its own phase state machine.
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
/// History: there used to be only two states, `Plan / Yolo` (the work lane defaulted to Yolo, i.e. full
/// auto-approval). `Auto` is the later-added middle tier (aligned with the mainstream shape of Claude Code
/// `acceptEdits`/`default`): ordinary promptable tools ask each time, and in-workspace file writes go through
/// the base's carve-out without a card. `"plan"/"yolo"` in older sidecar files still deserialize as before.
/// Compatibility discipline (same rule for both lanes): the settings.json lane default never writes the literal
/// `"auto"` — an explicit Auto choice goes through the `*_auto` companion flag (see `platform::prefs`); the
/// per-session `_session_mode_states.json` likewise keeps only the pre-tri-mode vocabulary —
/// `apply_session_mode_mutation_locked` folds Auto into `"plan"` in the shared table,
/// and the explicit choice is recorded as a `true` flag in the `_session_auto_modes.json` companion table. A
/// pre-tri-mode binary sharing PINVOU3_HOME can therefore strict-deserialize the whole table successfully:
/// downgrade no longer quarantines the whole table or rebuilds other sessions' choices from an empty table on
/// the next write (review P2 data-compatibility requirement); Auto sessions conservatively downgrade to Plan in their view.
/// Semantics and the downgrade recovery boundary: see `docs/safety-modes.md` §3.1/§6.
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

    /// Tri-mode approval-stance mapping (auto_approve, approval_mode) — the per-turn fields sent to the engine
    /// in `Op::SendMessage`.
    ///
    /// - Plan `(false, Suggest)`: read-only is normalized by the base Plan fallback (shell disabled, read-only
    ///   sandbox); the stance value matches the base `base_policy_for_mode`.
    /// - Auto `(false, Suggest)`: ordinary promptable tools get an approval card each time; in-workspace file
    ///   writes go through the base carve-out without a card (#5185), and read-only commands via the readonly classification without a card.
    ///   Aligned with the mainstream shape of Claude Code `default`/`acceptEdits`.
    /// - Yolo `(true, Bypass)`: auto-approval (the historical Yolo behavior). Whether a baseline guardrail hit
    ///   raises a card or hard-denies is decided by `ask_on_guardrail` (the engine-side arming mapping).
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
