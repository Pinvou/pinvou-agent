//! Per-session runtime mode state machine + 类型定义。
//!
//! `SessionModeState` / `ActiveSkillBinding` / `SerializableMode` 的**类型定义**
//! 原先在 `core/mode_state.rs`，但它们是 session 域聚合（persona/review/workflow/
//! knowledge 四特性字段），违反 `core/README.md` 的"feature 内部类型不入 core"准则。
//! Wave 3 将类型定义迁回此文件（行为 impl 一直在此），`core/mode_state.rs` 退化为
//! 重新导出垫片以保持外部 import 兼容。
//!
//! These methods drive the in-memory `mode_states` map (mode, pinvou_review,
//! pending Plan ticket + claim-in-flight, active skill binding, persona,
//! mounted collection). All state is deliberately in-memory only: mode /
//! plan_phase is runtime interaction state that should reset to Yolo + None on
//! restart, while skill bindings and model selections are persisted in their
//! own sidecars (see [`super::sidecars`]).

use deepseek_tui::tui::app::AppMode;
use serde::{Deserialize, Serialize};

use anyhow::{bail, Result};

use super::injections::{PendingPlanClaim, PendingTurnInjections};
use super::SessionStore;

// ── 类型定义（自 core/mode_state.rs 迁入） ──

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

/// A knowledge collection mounted into a session with an enabled flag, so the
/// UI can toggle a mount on/off without losing its position in the ordered
/// list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MountedCollection {
    pub collection_id: i64,
    pub enabled: bool,
}

/// Revisioned snapshot of every mounted collection for one session. The
/// revision bumps on any mutation so the Tauri boundary can publish a single
/// revisioned event that lets concurrent clients reconcile out-of-order
/// updates.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
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
    /// 该 session 挂载的本地知识集列表(多知识库,会话级粘连)。空 = 未挂载。
    /// `mounted_collection` 退化为该列表首个 enabled 项的兼容镜像,供旧协议读取。
    #[serde(default)]
    pub mounted_collections: Vec<MountedCollection>,
    /// 多知识库挂载的单调递增版本号,变更时 bump,供前端 reconcile 并发更新。
    #[serde(default)]
    pub mounted_collections_revision: u64,
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
mod type_tests {
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

// ── SessionStore 行为 impl ──

impl SessionStore {
    // ===================== Mode 状态机 =====================

    /// 取当前 session 的 mode 状态。未存在时返回 default（Yolo + None）。
    pub fn mode_state(&self, id: &str) -> SessionModeState {
        self.mode_states.read().get(id).cloned().unwrap_or_default()
    }

    /// 设置 mode。砍 PlanPhase 后是 Plan/Yolo 唯一 setter(流转命令都调它),
    /// 只改 mode,保留 pinvou_review_enabled 等其他字段。
    pub fn set_mode(&self, id: &str, mode: SerializableMode) -> Result<()> {
        let mut m = self.mode_states.write();
        let entry = m.entry(id.to_string()).or_default();
        entry.mode = mode;
        entry.pending_plan_id = None;
        entry.plan_claim_in_flight = None;
        Ok(())
    }

    /// Register the newest actionable plan only while the session is still in
    /// Plan mode. A newer TurnComplete supersedes the previous ticket.
    pub(crate) fn register_pending_plan(
        &self,
        id: &str,
        plan_id: String,
    ) -> Option<SessionModeState> {
        let mut states = self.mode_states.write();
        let entry = states.entry(id.to_string()).or_default();
        if entry.mode != SerializableMode::Plan || entry.plan_claim_in_flight.is_some() {
            return None;
        }
        entry.pending_plan_id = Some(plan_id);
        Some(entry.clone())
    }

    /// Atomically compare-and-consume a Plan ticket and switch to Yolo. The
    /// returned guard restores the ticket if Engine submission does not commit.
    pub(crate) fn claim_pending_plan(&self, id: &str, plan_id: &str) -> Result<PendingPlanClaim> {
        let accepted_state = {
            let mut states = self.mode_states.write();
            let entry = states.entry(id.to_string()).or_default();
            if entry.mode != SerializableMode::Plan
                || entry.pending_plan_id.as_deref() != Some(plan_id)
                || entry.plan_claim_in_flight.is_some()
            {
                bail!("plan_not_active");
            }
            entry.mode = SerializableMode::Yolo;
            entry.pending_plan_id = None;
            entry.plan_claim_in_flight = Some(plan_id.to_string());
            entry.clone()
        };
        Ok(PendingPlanClaim {
            store: self.clone(),
            session_id: id.to_string(),
            plan_id: plan_id.to_string(),
            accepted_state,
            settled: false,
        })
    }

    pub(crate) fn finish_pending_plan_claim(&self, id: &str, plan_id: &str) {
        let mut states = self.mode_states.write();
        let Some(entry) = states.get_mut(id) else {
            return;
        };
        if entry.plan_claim_in_flight.as_deref() == Some(plan_id) {
            entry.plan_claim_in_flight = None;
        }
    }

    pub(crate) fn restore_pending_plan_claim(&self, id: &str, plan_id: &str) -> Result<()> {
        let mut states = self.mode_states.write();
        let entry = states.entry(id.to_string()).or_default();
        if entry.mode != SerializableMode::Yolo
            || entry.pending_plan_id.is_some()
            || entry.plan_claim_in_flight.as_deref() != Some(plan_id)
        {
            bail!("restore plan claim conflict");
        }
        entry.mode = SerializableMode::Plan;
        entry.pending_plan_id = Some(plan_id.to_string());
        entry.plan_claim_in_flight = None;
        Ok(())
    }

    pub(crate) fn discard_pending_plan(&self, id: &str, plan_id: &str) -> Result<SessionModeState> {
        let mut states = self.mode_states.write();
        let entry = states.entry(id.to_string()).or_default();
        if entry.mode != SerializableMode::Plan
            || entry.pending_plan_id.as_deref() != Some(plan_id)
            || entry.plan_claim_in_flight.is_some()
        {
            bail!("plan_not_active");
        }
        entry.pending_plan_id = None;
        Ok(entry.clone())
    }

    /// 设置品悟 review 开关（用户在 UI 顶部 toggle 切换）。
    /// 与 Plan/YOLO 切换正交：品悟 toggle 不动 mode/phase。
    pub fn set_pinvou_review(&self, id: &str, enabled: bool) {
        let mut m = self.mode_states.write();
        let entry = m.entry(id.to_string()).or_default();
        entry.pinvou_review_enabled = enabled;
    }

    /// 重置到默认（Yolo + None）。delete_session 时调用。
    pub fn reset_mode_state(&self, id: &str) {
        self.mode_states.write().remove(id);
    }

    // ===================== 工作流 skill 绑定 (per-session) =====================

    /// 把一个 skill 绑定到指定 session。`start_skill_session` 在 create_new
    /// 之后立刻调,挂 pending_instruction 让该 session 第一条 chat 自动 prepend。
    pub fn bind_skill(&self, id: &str, binding: ActiveSkillBinding) {
        let mut m = self.mode_states.write();
        let entry = m.entry(id.to_string()).or_default();
        entry.active_skill = Some(binding);
    }

    /// 取该 session 当前绑定的 skill 信息(给前端渲染 chips strip)。
    /// 注意:返回的 binding 里 pending_instruction 是 None(serde skip + 一次性消费)。
    pub fn active_skill(&self, id: &str) -> Option<ActiveSkillBinding> {
        self.mode_states.read().get(id)?.active_skill.clone()
    }

    /// 一次性消费 session 绑定 skill 的 pending instruction。
    /// commands::chat 在发用户消息前调,prepend 到 message content 后置空,
    /// 后续 turn 不再重复(LLM 已经看到过,靠 session 上下文保持)。
    pub fn take_pending_skill_instruction(&self, id: &str) -> Option<String> {
        let mut m = self.mode_states.write();
        let entry = m.get_mut(id)?;
        let skill = entry.active_skill.as_mut()?;
        skill.pending_instruction.take()
    }

    /// 解除 session 的 skill 绑定(用户点 chips 区 ✕ 时调用)。
    /// 不删 session 本身,只清掉绑定 — chips strip 在前端会因此隐藏。
    pub fn unbind_skill(&self, id: &str) {
        if let Some(entry) = self.mode_states.write().get_mut(id) {
            entry.active_skill = None;
        }
        self.save_skill_bindings();
    }

    /// 查找已有绑定指定 skill 的 session ID（用于恢复工作流）。
    pub fn find_session_with_skill(&self, skill_name: &str) -> Option<String> {
        self.mode_states
            .read()
            .iter()
            .find(|(_, state)| {
                state.active_skill.as_ref().map(|s| s.name.as_str()) == Some(skill_name)
            })
            .map(|(id, _)| id.clone())
    }

    /// 持久化所有 skill binding 到磁盘。
    pub fn save_skill_bindings(&self) {
        super::sidecars::save_skill_bindings(&self.mode_states);
    }

    /// 从磁盘恢复 skill bindings（启动时调用）。
    pub fn load_skill_bindings(&self) {
        super::sidecars::load_skill_bindings(&self.mode_states);
    }

    // ── Side B 卡片池(persona,远端体系) ──

    pub fn set_active_persona(&self, id: &str, persona_id: Option<String>) {
        self.mode_states
            .write()
            .entry(id.to_string())
            .or_default()
            .active_persona = persona_id;
    }
    pub fn active_persona_id(&self, id: &str) -> Option<String> {
        self.mode_states.read().get(id)?.active_persona.clone()
    }
    pub fn set_pending_persona_body(&self, id: &str, body: Option<String>) {
        self.mode_states
            .write()
            .entry(id.to_string())
            .or_default()
            .pending_persona_body = body;
    }
    pub fn take_pending_persona_body(&self, id: &str) -> Option<String> {
        self.mode_states
            .write()
            .get_mut(id)?
            .pending_persona_body
            .take()
    }

    /// Atomically checkout every one-shot prompt injection for a turn. The
    /// returned guard restores them on any pre-submission error or cancelled
    /// future; callers commit it only after EngineHandle accepts the operation.
    pub(crate) fn take_pending_turn_injections(&self, id: &str) -> PendingTurnInjections {
        let (skill, persona) = {
            let mut states = self.mode_states.write();
            match states.get_mut(id) {
                Some(state) => {
                    let skill = state.active_skill.as_mut().and_then(|binding| {
                        binding
                            .pending_instruction
                            .take()
                            .map(|instruction| (binding.name.clone(), instruction))
                    });
                    let persona = state
                        .pending_persona_body
                        .take()
                        .map(|body| (state.active_persona.clone(), body));
                    (skill, persona)
                }
                None => (None, None),
            }
        };
        PendingTurnInjections {
            store: self.clone(),
            session_id: id.to_string(),
            skill,
            persona,
            committed: false,
        }
    }

    pub(crate) fn restore_pending_turn_injections(
        &self,
        id: &str,
        skill: Option<(String, String)>,
        persona: Option<(Option<String>, String)>,
    ) {
        if skill.is_none() && persona.is_none() {
            return;
        }
        let mut states = self.mode_states.write();
        let Some(state) = states.get_mut(id) else {
            return;
        };
        if let Some((skill_name, instruction)) = skill {
            if let Some(binding) = state.active_skill.as_mut() {
                if binding.name == skill_name && binding.pending_instruction.is_none() {
                    binding.pending_instruction = Some(instruction);
                }
            }
        }
        if let Some((persona_id, body)) = persona {
            if state.active_persona == persona_id && state.pending_persona_body.is_none() {
                state.pending_persona_body = Some(body);
            }
        }
    }
}
