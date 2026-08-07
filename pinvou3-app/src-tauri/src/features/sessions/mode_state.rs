//! Per-session runtime mode state machine.
//!
//! These methods drive the in-memory `mode_states` map (mode, pinvou_review,
//! pending Plan ticket + claim-in-flight, active skill binding, persona,
//! mounted collection). All state is deliberately in-memory only: mode /
//! plan_phase is runtime interaction state that should reset to Yolo + None on
//! restart, while skill bindings and model selections are persisted in their
//! own sidecars (see [`super::sidecars`]).

use anyhow::{bail, Result};

use crate::core::mode_state::{ActiveSkillBinding, SerializableMode, SessionModeState};

use super::injections::{PendingPlanClaim, PendingTurnInjections};
use super::SessionStore;

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
