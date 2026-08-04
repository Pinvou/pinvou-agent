//! Per-session runtime mode state machine.
//!
//! These methods drive the in-memory `mode_states` map (mode, pinvou_review,
//! pending Plan ticket + claim-in-flight, active skill binding, persona,
//! mounted collection). All state is deliberately in-memory only: mode /
//! plan_phase is runtime interaction state that should reset to Yolo + None on
//! restart, while skill bindings and model selections are persisted in their
//! own sidecars (see [`super::sidecars`]).

use anyhow::{bail, Result};

use crate::core::mode_state::{
    ActiveSkillBinding, ModeDefaultsView, ModeLane, SerializableMode, SessionModeState,
};

use super::injections::{PendingPlanClaim, PendingTurnInjections};
use super::SessionStore;

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

    fn is_code_session(&self, id: &str) -> bool {
        self.code_session_predicate
            .read()
            .as_ref()
            .is_some_and(|predicate| predicate(id))
    }

    /// 无条目时的默认 mode 解析：code 会话回落全局 `code_permission.last_mode`
    /// （None = 用户从未用过 code 模式 → Plan 只读首启）；plain 会话缺省 Yolo
    /// （work/design lane 的全局默认由前端在会话物化时应用，后端不区分这两个
    /// lane，见 `set_mode_default`）。
    fn resolved_default_mode(&self, id: &str) -> SerializableMode {
        if self.is_code_session(id) {
            self.code_permission
                .read()
                .last_mode
                .unwrap_or(SerializableMode::Plan)
        } else {
            SerializableMode::Yolo
        }
    }

    fn mode_state_entry<'m>(
        states: &'m mut HashMap<String, SessionModeState>,

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

    fn save_multi_agent_flags_locked(&self) -> Result<()> {
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

    fn finish_pending_plan_claim(&self, id: &str, plan_id: &str) {
        let mut states = self.mode_states.write();
        let Some(entry) = states.get_mut(id) else {
            return;
        };
        if entry.plan_claim_in_flight.as_deref() == Some(plan_id) {
            entry.plan_claim_in_flight = None;
        }
    }

    fn restore_pending_plan_claim(&self, id: &str, plan_id: &str) -> Result<()> {
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

    fn restore_pending_turn_injections(
        &self,

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
        self.save_skill_bindings();
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

    pub fn add_mounted_collection(
        &self,

    pub fn set_mounted_collection_enabled(
        &self,

    pub fn remove_mounted_collection(
        &self,

    pub fn remove_mounted_collection_from_all(
        &self,

    fn update_mounted_collections<F>(&self, id: &str, update: F) -> MountedCollectionsSnapshot
    where

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
        let states_file =
            crate::platform::paths::sessions_root().join("_session_mode_states.json");
        let modes = self.session_mode_states.read();
        if modes.is_empty() {
            let _ = std::fs::remove_file(&states_file);
            return;
        }
        let Ok(json) = serde_json::to_string_pretty(&*modes) else {
            eprintln!("[sessions] serialize _session_mode_states.json failed");
            return;
        };
        if let Err(error) =
            crate::platform::filesystem::atomic_write(&states_file, json.as_bytes())
        {
            eprintln!("[sessions] persist _session_mode_states.json failed: {error}");
        }
    }

    /// 启动时恢复所有会话的 per-session mode：合并进 `mode_states`，
    /// 重开某个会话即恢复它自己上次显式使用的 mode。
    /// 兼容：新文件不存在时回退读旧的 `_code_mode_states.json`（只含 code 会话
    /// 的时代产物），下次保存自然写到新文件，旧文件不删。
    pub fn load_session_mode_states(&self) {
        let states_file =
            crate::platform::paths::sessions_root().join("_session_mode_states.json");
        let legacy_file =
            crate::platform::paths::sessions_root().join("_code_mode_states.json");
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
