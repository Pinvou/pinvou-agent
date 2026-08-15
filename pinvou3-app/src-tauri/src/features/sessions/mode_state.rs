//! Per-session runtime mode state machine + 类型定义。
//!
//! `SessionModeState` / `ActiveSkillBinding` / `SerializableMode` 的**类型定义**
//! 原先在 `core/mode_state.rs`，但它们是 session 域聚合（persona/review/workflow/
//! knowledge 四特性字段），违反 `core/README.md` 的"feature 内部类型不入 core"准则。
//! Wave 3 将类型定义迁回此文件（行为 impl 一直在此），并由 `features::sessions`
//! 正式 re-export，消费方一律从 `crate::features::sessions::{...}` 导入，
//! 不再经过 `core` 垫片（避免形成 `core → features` 反向依赖）。
//!
//! These methods drive the in-memory `mode_states` map (mode, pinvou_review,
//! pending Plan ticket + claim-in-flight, active skill binding, persona,
//! mounted collection). All state is deliberately in-memory only: mode /
//! plan_phase is runtime interaction state that should reset to Yolo + None on
//! restart, while skill bindings and model selections are persisted in their
//! own sidecars (see [`super::sidecars`]).

use serde::{Deserialize, Serialize};

use anyhow::{bail, Context, Result};

use crate::core::mode_state::{ModeDefaultsView, ModeLane, SerializableMode};

use super::injections::{PendingPlanClaim, PendingTurnInjections};
use super::SessionStore;
use crate::platform::prefs::{CodePermissionPrefs, UserPrefs};
use std::collections::HashMap;
use std::io::ErrorKind;

// ── 类型定义（session 域聚合，由 sessions 正式 re-export） ──

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
#[serde(rename_all = "camelCase")]
pub struct MountedCollection {
    pub collection_id: i64,
    pub enabled: bool,
}

/// Revisioned snapshot of every mounted collection for one session. The
/// revision bumps on any mutation so the Tauri boundary can publish a single
/// revisioned event that lets concurrent clients reconcile out-of-order
/// updates.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
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

#[cfg(test)]
mod type_tests {
    use super::*;
    use deepseek_tui::tui::app::AppMode;

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

// ── SessionStore 行为 impl ──

impl SessionStore {
    pub fn mode_state(&self, id: &str) -> SessionModeState {
        self.mode_states
            .read()
            .get(id)
            .cloned()
            .unwrap_or_else(|| SessionModeState {
                mode: self.resolved_default_mode(id),
                ..SessionModeState::default()
            })
    }

    pub(crate) fn is_code_session(&self, id: &str) -> bool {
        self.code_session_predicate
            .read()
            .as_ref()
            .is_some_and(|predicate| predicate(id))
    }

    /// 无条目时的默认 mode 解析：code 会话回落全局 `code_permission.last_mode`
    /// （None = 用户从未用过 code 模式 → Plan 只读首启）；plain 会话缺省 Yolo
    /// （work/design lane 的全局默认由前端在会话物化时应用，后端不区分这两个
    /// lane，见 `set_mode_default`）。
    pub(crate) fn resolved_default_mode(&self, id: &str) -> SerializableMode {
        if self.is_code_session(id) {
            self.code_permission
                .read()
                .last_mode
                .unwrap_or(SerializableMode::Plan)
        } else {
            SerializableMode::Yolo
        }
    }

    pub(crate) fn mode_state_entry<'m>(
        states: &'m mut HashMap<String, SessionModeState>,
        id: &str,
        default_mode: SerializableMode,
    ) -> &'m mut SessionModeState {
        states
            .entry(id.to_string())
            .or_insert_with(|| SessionModeState {
                mode: default_mode,
                ..SessionModeState::default()
            })
    }

    /// 设置 mode。砍 PlanPhase 后是 Plan/Yolo 唯一 setter(流转命令都调它),
    /// 只改 mode,保留 pinvou_review_enabled 等其他字段。
    ///
    /// per-session 持久化（三分 lane 语义）：任何会话都写
    /// `_session_mode_states.json`（重开恢复它自己上次的 mode）；**不再**更新
    /// 全局 lane 默认——全局默认只由草稿态显式切换经 `set_mode_default` 写入。
    /// ACP 会话不经此命令（有自己的权限模式）。落盘失败只记日志不打断交互
    /// ——内存切换已生效，与 save_skill_bindings 同级容错。
    pub fn set_mode(&self, id: &str, mode: SerializableMode) -> Result<()> {
        {
            let mut m = self.mode_states.write();
            let entry = m.entry(id.to_string()).or_default();
            entry.mode = mode;
            entry.pending_plan_id = None;
            entry.plan_claim_in_flight = None;
        }
        self.session_mode_states
            .write()
            .insert(id.to_string(), mode);
        self.save_session_mode_states();
        Ok(())
    }

    pub fn set_multi_agent(&self, id: &str, enabled: bool) -> Result<()> {
        let _io = self.multi_agent_flags_io.lock();
        let previous = {
            let mut m = self.mode_states.write();
            let entry = m.entry(id.to_string()).or_default();
            let previous = entry.multi_agent;
            entry.multi_agent = enabled;
            previous
        };
        if let Err(error) = self.save_multi_agent_flags_locked() {
            let mut m = self.mode_states.write();
            if let Some(entry) = m.get_mut(id) {
                entry.multi_agent = previous;
            }
            return Err(error).context("persist multi-agent flag");
        }
        Ok(())
    }

    pub fn multi_agent_session_ids(&self) -> Vec<String> {
        let m = self.mode_states.read();
        let mut ids: Vec<String> = m
            .iter()
            .filter(|(_, state)| state.multi_agent)
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    pub fn save_multi_agent_flags(&self) -> Result<()> {
        let _io = self.multi_agent_flags_io.lock();
        self.save_multi_agent_flags_locked()
    }

    pub(crate) fn save_multi_agent_flags_locked(&self) -> Result<()> {
        let file = crate::platform::paths::sessions_root().join("_multi_agent.json");
        let ids = self.multi_agent_session_ids();
        if ids.is_empty() {
            return match std::fs::remove_file(&file) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error).context("remove _multi_agent.json"),
            };
        }
        let json = serde_json::to_string_pretty(&ids).context("serialize multi-agent flags")?;
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, json).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &file)
            .with_context(|| format!("commit {} -> {}", tmp.display(), file.display()))
    }

    pub fn load_multi_agent_flags(&self) {
        let file = crate::platform::paths::sessions_root().join("_multi_agent.json");
        let Ok(content) = std::fs::read_to_string(&file) else {
            return;
        };
        let ids: Vec<String> = match serde_json::from_str(&content) {
            Ok(ids) => ids,
            Err(e) => {
                eprintln!("[sessions] load_multi_agent_flags failed: {e}");
                return;
            }
        };
        let sessions_dir = self.manager.sessions_dir().to_path_buf();
        let mut ghosts = false;
        {
            let mut m = self.mode_states.write();
            for id in ids {
                if sessions_dir.join(format!("{id}.json")).is_file() {
                    m.entry(id).or_default().multi_agent = true;
                } else {
                    ghosts = true;
                }
            }
        }
        if ghosts {
            if let Err(error) = self.save_multi_agent_flags() {
                eprintln!(
                    "[sessions] rewrite _multi_agent.json after ghost cleanup failed: {error:#}"
                );
            }
        }
    }

    pub(crate) fn register_pending_plan(
        &self,
        id: &str,
        plan_id: String,
    ) -> Option<SessionModeState> {
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let entry = Self::mode_state_entry(&mut states, id, default_mode);
        if entry.mode != SerializableMode::Plan || entry.plan_claim_in_flight.is_some() {
            return None;
        }
        entry.pending_plan_id = Some(plan_id);
        Some(entry.clone())
    }

    pub(crate) fn claim_pending_plan(&self, id: &str, plan_id: &str) -> Result<PendingPlanClaim> {
        let accepted_state = {
            let default_mode = self.resolved_default_mode(id);
            let mut states = self.mode_states.write();
            let entry = Self::mode_state_entry(&mut states, id, default_mode);
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
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let entry = Self::mode_state_entry(&mut states, id, default_mode);
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
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let entry = Self::mode_state_entry(&mut states, id, default_mode);
        if entry.mode != SerializableMode::Plan
            || entry.pending_plan_id.as_deref() != Some(plan_id)
            || entry.plan_claim_in_flight.is_some()
        {
            bail!("plan_not_active");
        }
        entry.pending_plan_id = None;
        Ok(entry.clone())
    }

    pub fn set_pinvou_review(&self, id: &str, enabled: bool) {
        let default_mode = self.resolved_default_mode(id);
        let mut m = self.mode_states.write();
        let entry = Self::mode_state_entry(&mut m, id, default_mode);
        entry.pinvou_review_enabled = enabled;
    }

    pub fn reset_mode_state(&self, id: &str) {
        self.mode_states.write().remove(id);
        if self.session_mode_states.write().remove(id).is_some() {
            self.save_session_mode_states();
        }
    }

    pub fn bind_skill(&self, id: &str, binding: ActiveSkillBinding) {
        let default_mode = self.resolved_default_mode(id);
        let mut m = self.mode_states.write();
        let entry = Self::mode_state_entry(&mut m, id, default_mode);
        entry.active_skill = Some(binding);
    }

    pub fn active_skill(&self, id: &str) -> Option<ActiveSkillBinding> {
        self.mode_states.read().get(id)?.active_skill.clone()
    }

    pub fn take_pending_skill_instruction(&self, id: &str) -> Option<String> {
        let mut m = self.mode_states.write();
        let entry = m.get_mut(id)?;
        let skill = entry.active_skill.as_mut()?;
        skill.pending_instruction.take()
    }

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

    pub fn set_active_persona(&self, id: &str, persona_id: Option<String>) {
        let default_mode = self.resolved_default_mode(id);
        Self::mode_state_entry(&mut self.mode_states.write(), id, default_mode).active_persona =
            persona_id;
    }

    pub fn active_persona_id(&self, id: &str) -> Option<String> {
        self.mode_states.read().get(id)?.active_persona.clone()
    }

    pub fn set_pending_persona_body(&self, id: &str, body: Option<String>) {
        let default_mode = self.resolved_default_mode(id);
        Self::mode_state_entry(&mut self.mode_states.write(), id, default_mode)
            .pending_persona_body = body;
    }

    pub fn take_pending_persona_body(&self, id: &str) -> Option<String> {
        self.mode_states
            .write()
            .get_mut(id)?
            .pending_persona_body
            .take()
    }

    pub fn unbind_skill(&self, id: &str) {
        if let Some(entry) = self.mode_states.write().get_mut(id) {
            entry.active_skill = None;
        }
        super::sidecars::save_skill_bindings(&self.mode_states);
    }

    pub fn find_session_with_skill(&self, skill_name: &str) -> Option<String> {
        self.mode_states
            .read()
            .iter()
            .find(|(_, state)| {
                state.active_skill.as_ref().map(|s| s.name.as_str()) == Some(skill_name)
            })
            .map(|(id, _)| id.clone())
    }

    pub fn set_mounted_collection(&self, id: &str, collection_id: Option<i64>) {
        let mounted = collection_id
            .filter(|collection_id| *collection_id > 0)
            .map(|collection_id| MountedCollection {
                collection_id,
                enabled: true,
            })
            .into_iter()
            .collect();
        self.set_mounted_collections(id, mounted);
    }

    pub fn set_mounted_collections(
        &self,
        id: &str,
        collections: Vec<MountedCollection>,
    ) -> MountedCollectionsSnapshot {
        self.update_mounted_collections(id, |_| collections)
    }

    pub fn add_mounted_collection(
        &self,
        id: &str,
        collection_id: i64,
    ) -> MountedCollectionsSnapshot {
        self.update_mounted_collections(id, |mut collections| {
            if let Some(collection) = collections
                .iter_mut()
                .find(|collection| collection.collection_id == collection_id)
            {
                collection.enabled = true;
            } else {
                collections.push(MountedCollection {
                    collection_id,
                    enabled: true,
                });
            }
            collections
        })
    }

    pub fn set_mounted_collection_enabled(
        &self,
        id: &str,
        collection_id: i64,
        enabled: bool,
    ) -> MountedCollectionsSnapshot {
        self.update_mounted_collections(id, |mut collections| {
            if let Some(collection) = collections
                .iter_mut()
                .find(|collection| collection.collection_id == collection_id)
            {
                collection.enabled = enabled;
            }
            collections
        })
    }

    pub fn remove_mounted_collection(
        &self,
        id: &str,
        collection_id: i64,
    ) -> MountedCollectionsSnapshot {
        self.update_mounted_collections(id, |mut collections| {
            collections.retain(|collection| collection.collection_id != collection_id);
            collections
        })
    }

    pub fn remove_mounted_collection_from_all(
        &self,
        collection_id: i64,
    ) -> Vec<(String, MountedCollectionsSnapshot)> {
        let mut states = self.mode_states.write();
        let mut changed = Vec::new();
        for (session_id, state) in states.iter_mut() {
            let mut collections = if state.mounted_collections.is_empty() {
                state
                    .mounted_collection
                    .map(|mounted_id| MountedCollection {
                        collection_id: mounted_id,
                        enabled: true,
                    })
                    .into_iter()
                    .collect::<Vec<_>>()
            } else {
                state.mounted_collections.clone()
            };
            let previous_len = collections.len();
            collections.retain(|collection| collection.collection_id != collection_id);
            if collections.len() == previous_len {
                continue;
            }

            state.mounted_collection = collections
                .iter()
                .find(|collection| collection.enabled)
                .map(|collection| collection.collection_id);
            state.mounted_collections = collections.clone();
            state.mounted_collections_revision =
                state.mounted_collections_revision.wrapping_add(1).max(1);
            changed.push((
                session_id.clone(),
                MountedCollectionsSnapshot {
                    revision: state.mounted_collections_revision,
                    collections,
                },
            ));
        }
        changed.sort_by(|left, right| left.0.cmp(&right.0));
        changed
    }

    pub(crate) fn update_mounted_collections<F>(
        &self,
        id: &str,
        update: F,
    ) -> MountedCollectionsSnapshot
    where
        F: FnOnce(Vec<MountedCollection>) -> Vec<MountedCollection>,
    {
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let state = Self::mode_state_entry(&mut states, id, default_mode);
        let current = if state.mounted_collections.is_empty() {
            state
                .mounted_collection
                .map(|collection_id| MountedCollection {
                    collection_id,
                    enabled: true,
                })
                .into_iter()
                .collect()
        } else {
            state.mounted_collections.clone()
        };
        let mut normalized = Vec::new();
        for collection in update(current) {
            if collection.collection_id <= 0
                || normalized.iter().any(|mounted: &MountedCollection| {
                    mounted.collection_id == collection.collection_id
                })
            {
                continue;
            }
            normalized.push(collection);
        }
        let legacy = normalized
            .iter()
            .find(|collection| collection.enabled)
            .map(|collection| collection.collection_id);
        state.mounted_collection = legacy;
        state.mounted_collections = normalized.clone();
        state.mounted_collections_revision =
            state.mounted_collections_revision.wrapping_add(1).max(1);
        MountedCollectionsSnapshot {
            revision: state.mounted_collections_revision,
            collections: normalized,
        }
    }

    pub fn mounted_collections(&self, id: &str) -> Vec<MountedCollection> {
        self.mounted_collections_snapshot(id).collections
    }

    pub fn mounted_collections_snapshot(&self, id: &str) -> MountedCollectionsSnapshot {
        let states = self.mode_states.read();
        let Some(state) = states.get(id) else {
            return MountedCollectionsSnapshot {
                revision: 0,
                collections: Vec::new(),
            };
        };
        let collections = if !state.mounted_collections.is_empty() {
            state.mounted_collections.clone()
        } else {
            state
                .mounted_collection
                .map(|collection_id| MountedCollection {
                    collection_id,
                    enabled: true,
                })
                .into_iter()
                .collect()
        };
        MountedCollectionsSnapshot {
            revision: state.mounted_collections_revision,
            collections,
        }
    }

    pub fn mounted_collection_ids(&self, id: &str) -> Vec<i64> {
        self.mounted_collections(id)
            .into_iter()
            .filter(|collection| collection.enabled)
            .map(|collection| collection.collection_id)
            .collect()
    }

    pub fn mounted_collection(&self, id: &str) -> Option<i64> {
        self.mounted_collection_ids(id).into_iter().next()
    }

    // ===================== per-session mode 持久化（所有会话） =====================

    /// 持久化所有会话的 per-session mode 到 `_session_mode_states.json`
    /// （仿 `_skill_bindings.json`；三分 lane 语义后 plain 会话也持久化）。
    /// 空表时删文件，与 save_skill_bindings 同款语义。
    ///
    /// 原子写 + 失败可见：直接 `std::fs::write` 在进程中断时可能留下截断文件，
    /// 而 `load_session_mode_states` 对损坏文件是静默跳过——一次中断写入会让所有
    /// per-session mode 记录永久丢失，表现为「显式切过 mode，重启后回 Plan」。
    pub fn save_session_mode_states(&self) {
        let states_file = crate::platform::paths::sessions_root().join("_session_mode_states.json");
        let modes = self.session_mode_states.read();
        if modes.is_empty() {
            let _ = std::fs::remove_file(&states_file);
            return;
        }
        let Ok(json) = serde_json::to_string_pretty(&*modes) else {
            eprintln!("[sessions] serialize _session_mode_states.json failed");
            return;
        };
        if let Err(error) = crate::platform::filesystem::atomic_write(&states_file, json.as_bytes())
        {
            eprintln!("[sessions] persist _session_mode_states.json failed: {error}");
        }
    }

    /// 启动时恢复所有会话的 per-session mode：合并进 `mode_states`，
    /// 重开某个会话即恢复它自己上次显式使用的 mode。
    /// 兼容：新文件不存在时回退读旧的 `_code_mode_states.json`（只含 code 会话
    /// 的时代产物），下次保存自然写到新文件，旧文件不删。
    pub fn load_session_mode_states(&self) {
        let states_file = crate::platform::paths::sessions_root().join("_session_mode_states.json");
        let legacy_file = crate::platform::paths::sessions_root().join("_code_mode_states.json");
        let source = if states_file.exists() {
            states_file
        } else if legacy_file.exists() {
            legacy_file
        } else {
            return;
        };
        let content = match std::fs::read_to_string(&source) {
            Ok(c) => c,
            Err(_) => return,
        };
        let modes: std::collections::HashMap<String, SerializableMode> =
            match serde_json::from_str(&content) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("[sessions] load_session_mode_states failed: {e}");
                    return;
                }
            };
        {
            let mut persisted = self.session_mode_states.write();
            *persisted = modes.clone();
        }
        let mut states = self.mode_states.write();
        for (id, mode) in modes {
            states.entry(id).or_default().mode = mode;
        }
    }

    pub fn code_permission_prefs(&self) -> CodePermissionPrefs {
        *self.code_permission.read()
    }

    /// 三个 lane 的全局默认 mode 视图（内存镜像；work/design 磁盘真相在
    /// settings.json `mode_defaults`，code 在 `code_permission.last_mode`）。
    pub fn mode_defaults(&self) -> ModeDefaultsView {
        let defaults = self.mode_defaults.read();
        ModeDefaultsView {
            work: defaults.work,
            design: defaults.design,
            code: self.code_permission.read().last_mode,
        }
    }

    /// 草稿态显式切换写入对应 lane 的全局默认 mode（三分 lane 语义：已生成
    /// 会话的切换不碰这里）。先更新内存镜像（本次运行立即生效），再字段级
    /// 事务写 settings.json；写盘失败只记日志（与 set_mode 的容错语义一致）。
    pub fn set_mode_default(&self, lane: ModeLane, mode: SerializableMode) {
        match lane {
            ModeLane::Code => {
                self.code_permission.write().last_mode = Some(mode);
            }
            ModeLane::Work => {
                self.mode_defaults.write().work = Some(mode);
            }
            ModeLane::Design => {
                self.mode_defaults.write().design = Some(mode);
            }
        }
        if let Err(error) = UserPrefs::update_transaction(|prefs| {
            match lane {
                ModeLane::Code => prefs.code_permission.last_mode = Some(mode),
                ModeLane::Work => prefs.mode_defaults.work = Some(mode),
                ModeLane::Design => prefs.mode_defaults.design = Some(mode),
            }
            Ok(())
        }) {
            eprintln!("[sessions] persist mode default for {lane:?} failed: {error}");
        }
    }

    /// accept 方案（`claim_pending_plan` 切 Yolo）确认提交后，把任务级切换纳入
    /// per-session 持久化：写 `_session_mode_states.json`（重开/切走切回恢复
    /// Yolo）。**不**更新任何全局 lane 默认（三分 lane 语义：已生成会话的切换
    /// 只写会话自己的记录）。
    ///
    /// 只在 `PendingPlanClaim::commit`（engine 提交已确认）调用：任务真正开始
    /// 执行时才记忆，提交失败回滚（`restore_pending_plan_claim`）不碰磁盘，
    /// 内存回 Plan 与磁盘保持一致。
    pub(crate) fn persist_accepted_yolo_mode(&self, id: &str) {
        self.session_mode_states
            .write()
            .insert(id.to_string(), SerializableMode::Yolo);
        self.save_session_mode_states();
    }

    pub fn confirm_code_yolo(&self) -> Result<CodePermissionPrefs, String> {
        UserPrefs::update_transaction(|prefs| {
            prefs.code_permission.yolo_confirmed = true;
            Ok(())
        })?;
        self.code_permission.write().yolo_confirmed = true;
        Ok(self.code_permission_prefs())
    }
}
