//! 多对话管理 wrapper。
//!
//! 复用 deepseek-tui 上游 [`SessionManager`]（已支持 `new(custom_dir)`），
//! 把 sessions 目录定向到 `~/.pinvou3/sessions/`（隔离 `~/.deepseek/`）。
//!
//! 暴露给 pinvou3-app Tauri commands 的能力：
//! - `list` —— 列出所有会话元数据（前端历史面板）
//! - `create_new` —— 新建空会话（首次未发送消息前）
//! - `load` —— 读完整对话（切换 session 时给 engine 通过 `Op::SyncSession` 注入）
//! - `save` —— 持久化（每轮 turn 完成 auto-save）
//! - `delete` —— 删除会话 + artifacts 目录
//! - `set_title` —— 重命名
//! - `active_id` / `set_active` —— 跟踪当前 active session（chat command 用）
//!
//! **Arc + RwLock 包装**：所有字段都是 `Arc`，整个 `SessionStore` 可以
//! 廉价 Clone 进 Tauri State + 多个 task 共享。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use deepseek_tui::artifacts::{ArtifactKind, ArtifactRecord};
use deepseek_tui::models::Message;
use deepseek_tui::session_manager::{
    create_saved_session_with_id_and_mode, SavedSession, SessionManager, SessionMetadata,
};
use parking_lot::RwLock;

use super::mode_state::{ActiveSkillBinding, SerializableMode, SessionModeState};
use super::paths;

/// pinvou3 session 存储：包 SessionManager + active id 跟踪 + per-session mode 状态。
///
/// mode 状态故意 in-memory only（不持久化到 SavedSession.json）：
/// - mode/plan_phase 是运行时交互状态，重启 app 后默认回到 Yolo+None 合理
/// - 持久化反而会让用户切了 session 后突然看到「这个 session 还停在 Plan 模式」困惑
///
/// `auto_continue_count`：M2 弱模型加固——Executing 态 LLM 调一次工具就停时,
/// bridge 自动 send "继续"消息驱动 agent loop。每个用户主动消息重置为 0,
#[derive(Clone)]
pub struct SessionStore {
    manager: Arc<SessionManager>,
    active: Arc<RwLock<Option<String>>>,
    mode_states: Arc<RwLock<HashMap<String, SessionModeState>>>,
    /// per-session 模型绑定:session_id → SavedModel.id。某 session 显式选过模型
    /// 才有条目;没选的回退全局 active_model_id。落盘到 `_session_models.json`
    /// (仿 skill_bindings),底座 SavedSession 不能加字段故独立存。
    session_models: Arc<RwLock<HashMap<String, String>>>,
    /// 历史对话置顶表:session_id -> pinned_at。独立落盘到 `_pinned_sessions.json`,
    /// 不改 SavedSession 结构。
    pinned_sessions: Arc<RwLock<HashMap<String, String>>>,
    /// 从左侧任务列表收起的会话:session_id -> hidden_at。独立落盘到
    /// `_hidden_sessions.json`,不改 SavedSession 结构。
    hidden_sessions: Arc<RwLock<HashMap<String, String>>>,
}

impl SessionStore {
    /// 用 `~/.pinvou3/sessions/` 初始化。如果目录不存在会自动创建。
    pub fn boot() -> Result<Self> {
        let dir = paths::sessions_root();
        let manager = SessionManager::new(dir.clone())
            .with_context(|| format!("SessionManager::new({}) failed", dir.display()))?;
        Ok(Self {
            manager: Arc::new(manager),
            active: Arc::new(RwLock::new(None)),
            mode_states: Arc::new(RwLock::new(HashMap::new())),
            session_models: Arc::new(RwLock::new(HashMap::new())),
            pinned_sessions: Arc::new(RwLock::new(HashMap::new())),
            hidden_sessions: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// 列出所有 session 元数据，按 updated_at 倒序（最新在前）。
    pub fn list(&self) -> Result<Vec<SessionMetadata>> {
        let mut out = self
            .manager
            .list_sessions()
            .context("list_sessions failed")?;
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(out)
    }

    /// 加载完整 session（包含所有 messages）。
    pub fn load(&self, id: &str) -> Result<SavedSession> {
        self.manager
            .load_session(id)
            .with_context(|| format!("load_session({id})"))
    }

    /// 落盘整个 session（atomic write 由上游处理）。
    pub fn save(&self, session: &SavedSession) -> Result<PathBuf> {
        self.manager
            .save_session(session)
            .context("save_session failed")
    }

    /// 删除 session（含 artifacts 子目录）。
    pub fn delete(&self, id: &str) -> Result<()> {
        match self.manager.delete_session(id) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let session_dir = paths::sessions_root().join(id.trim());
                if let Err(dir_err) = std::fs::remove_dir_all(&session_dir) {
                    if dir_err.kind() != std::io::ErrorKind::NotFound {
                        return Err(dir_err).with_context(|| {
                            format!("remove stale session dir({})", session_dir.display())
                        });
                    }
                }
            }
            Err(err) => return Err(err).with_context(|| format!("delete_session({id})")),
        }
        // 如果删的是 active session，清理 active 标记
        let mut active = self.active.write();
        if active.as_deref() == Some(id) {
            *active = None;
        }
        drop(active);
        self.mode_states.write().remove(id);
        let removed_session_model = {
            let mut session_models = self.session_models.write();
            session_models.remove(id).is_some()
        };
        if removed_session_model {
            self.save_session_models();
        }
        let removed_pin = {
            let mut pinned_sessions = self.pinned_sessions.write();
            pinned_sessions.remove(id).is_some()
        };
        if removed_pin {
            self.save_pinned_sessions();
        }
        let removed_hidden = {
            let mut hidden_sessions = self.hidden_sessions.write();
            hidden_sessions.remove(id).is_some()
        };
        if removed_hidden {
            self.save_hidden_sessions();
        }
        Ok(())
    }

    /// 重命名：load → 改 metadata.title → save。
    pub fn set_title(&self, id: &str, title: String) -> Result<()> {
        let mut session = self.load(id)?;
        session.metadata.title = title;
        self.save(&session)?;
        Ok(())
    }

    /// 新建空 session（无 messages）。返回 SavedSession 让调用方
    /// 立刻 `Op::SyncSession` 同步给 engine，并 set_active(id)。
    /// 上游空消息时 title 默认 "New Session"，pinvou3 覆写成中文。
    pub fn create_new(
        &self,
        model: String,
        model_id: Option<String>,
        workspace: PathBuf,
    ) -> Result<SavedSession> {
        let id = generate_session_id();
        let mut session = create_saved_session_with_id_and_mode(
            id.clone(),
            &[],
            &model,
            &workspace,
            0,
            None,
            None,
        );
        session.metadata.title = "新对话".to_string();
        self.save(&session)?;
        // per-session 模型:新建会话继承全局默认(active)模型 id,落盘记住。
        if let Some(mid) = model_id {
            self.set_session_model_id(&id, Some(mid));
        }
        Ok(session)
    }

    /// 替换 session 的 messages 数组并刷新 updated_at / message_count。
    /// 前端每轮 TurnComplete 后调用，把 messages 数组同步给后端持久化。
    /// total_tokens 暂时不维护（前端没拿到 usage 数据），保持原值。
    pub fn update_messages(&self, id: &str, messages: Vec<Message>) -> Result<()> {
        let mut session = self.load(id)?;
        if looks_like_truncating_overwrite(&session.messages, &messages) {
            anyhow::bail!(
                "refusing to overwrite {} existing messages with {} unrelated messages",
                session.messages.len(),
                messages.len()
            );
        }
        session.metadata.message_count = messages.len();
        session.metadata.updated_at = Utc::now();
        session.messages = messages;
        self.save(&session)?;
        Ok(())
    }

    /// 替换 session 的产物列表。前端跟踪 write_file / append_file 工具调用积累的 paths,
    /// 每轮 TurnComplete 一起落盘。重启 / 切换 session 后能从 SavedSession.artifacts
    /// 恢复列表(让用户感知产物跟 session 是一对一的)。
    pub fn update_artifacts(&self, id: &str, paths: Vec<String>) -> Result<()> {
        let mut session = self.load(id)?;
        let session_id = session.metadata.id.clone();
        let now = Utc::now();
        session.artifacts = paths
            .into_iter()
            .enumerate()
            .map(|(idx, p)| {
                let path = PathBuf::from(&p);
                let byte_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                ArtifactRecord {
                    id: format!("p3art_{session_id}_{idx}"),
                    kind: ArtifactKind::ToolOutput,
                    session_id: session_id.clone(),
                    tool_call_id: format!("p3_{idx}"),
                    tool_name: "write_file".into(),
                    created_at: now,
                    byte_size,
                    preview: String::new(),
                    storage_path: path,
                }
            })
            .collect();
        session.metadata.updated_at = now;
        self.save(&session)?;
        Ok(())
    }

    pub fn active_id(&self) -> Option<String> {
        self.active.read().clone()
    }

    pub fn set_active(&self, id: Option<String>) {
        *self.active.write() = id;
    }

    // ===================== Mode 状态机 =====================

    /// 取当前 session 的 mode 状态。未存在时返回 default（Yolo + None）。
    pub fn mode_state(&self, id: &str) -> SessionModeState {
        self.mode_states.read().get(id).cloned().unwrap_or_default()
    }

    /// 设置 mode。砍 PlanPhase 后是 Plan/Yolo 唯一 setter(流转命令都调它),
    /// 只改 mode,保留 pinvou_review_enabled 等其他字段。
    pub fn set_mode(&self, id: &str, mode: SerializableMode) {
        let mut m = self.mode_states.write();
        let entry = m.entry(id.to_string()).or_default();
        entry.mode = mode;
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
    // ── Side B 卡片池(persona,远端体系) ──
    pub fn set_active_persona(&self, id: &str, persona_id: Option<String>) {
        self.mode_states.write().entry(id.to_string()).or_default().active_persona = persona_id;
    }
    pub fn active_persona_id(&self, id: &str) -> Option<String> {
        self.mode_states.read().get(id)?.active_persona.clone()
    }
    pub fn set_pending_persona_body(&self, id: &str, body: Option<String>) {
        self.mode_states.write().entry(id.to_string()).or_default().pending_persona_body = body;
    }
    pub fn take_pending_persona_body(&self, id: &str) -> Option<String> {
        self.mode_states.write().get_mut(id)?.pending_persona_body.take()
    }

    // ── 知识库挂载(会话级粘连,仿 persona,仅驻内存) ──
    pub fn set_mounted_collection(&self, id: &str, collection_id: Option<i64>) {
        self.mode_states.write().entry(id.to_string()).or_default().mounted_collection = collection_id;
    }
    pub fn mounted_collection(&self, id: &str) -> Option<i64> {
        self.mode_states.read().get(id)?.mounted_collection
    }

    pub fn unbind_skill(&self, id: &str) {
        if let Some(entry) = self.mode_states.write().get_mut(id) {
            entry.active_skill = None;
        }
        self.save_skill_bindings();
    }

    /// 查找已有绑定指定 skill 的 session ID（用于恢复工作流）。
    pub fn find_session_with_skill(&self, skill_name: &str) -> Option<String> {
        self.mode_states.read()
            .iter()
            .find(|(_, state)| {
                state.active_skill.as_ref().map(|s| s.name.as_str()) == Some(skill_name)
            })
            .map(|(id, _)| id.clone())
    }

    /// 持久化所有 skill binding 到磁盘。
    pub fn save_skill_bindings(&self) {
        let bindings_file = super::paths::sessions_root().join("_skill_bindings.json");
        let m = self.mode_states.read();
        let bindings: std::collections::HashMap<String, &super::mode_state::ActiveSkillBinding> = m.iter()
            .filter_map(|(id, state)| {
                state.active_skill.as_ref().map(|s| (id.clone(), s))
            })
            .collect();
        if bindings.is_empty() {
            let _ = std::fs::remove_file(&bindings_file);
            return;
        }
        if let Ok(json) = serde_json::to_string_pretty(&bindings) {
            let _ = std::fs::write(bindings_file, json);
        }
    }

    /// 从磁盘恢复 skill bindings（启动时调用）。
    pub fn load_skill_bindings(&self) {
        let bindings_file = super::paths::sessions_root().join("_skill_bindings.json");
        if !bindings_file.exists() { return; }
        let content = match std::fs::read_to_string(&bindings_file) {
            Ok(c) => c,
            Err(_) => return,
        };
        let bindings: std::collections::HashMap<String, super::mode_state::ActiveSkillBinding> =
            match serde_json::from_str(&content) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("[sessions] load_skill_bindings failed: {e}");
                    return;
                }
            };
        let mut m = self.mode_states.write();
        for (id, binding) in bindings {
            let entry = m.entry(id).or_default();
            entry.active_skill = Some(binding);
        }
    }

    // ===================== per-session 模型绑定 =====================

    /// 取该 session 显式选定的模型 id(没选过返回 None,调用方回退全局 active)。
    pub fn session_model_id(&self, id: &str) -> Option<String> {
        self.session_models.read().get(id).cloned()
    }

    /// 设/清该 session 的模型 id 并落盘。`None` = 清除(回退全局默认)。
    pub fn set_session_model_id(&self, id: &str, model_id: Option<String>) {
        {
            let mut m = self.session_models.write();
            match model_id {
                Some(mid) => {
                    m.insert(id.to_string(), mid);
                }
                None => {
                    m.remove(id);
                }
            }
        }
        self.save_session_models();
    }

    /// 持久化 per-session 模型绑定到 `~/.pinvou3/sessions/_session_models.json`。
    pub fn save_session_models(&self) {
        let file = super::paths::sessions_root().join("_session_models.json");
        let m = self.session_models.read();
        if m.is_empty() {
            let _ = std::fs::remove_file(&file);
            return;
        }
        if let Ok(json) = serde_json::to_string_pretty(&*m) {
            let _ = std::fs::write(file, json);
        }
    }

    /// 启动时从磁盘恢复 per-session 模型绑定。
    pub fn load_session_models(&self) {
        let file = super::paths::sessions_root().join("_session_models.json");
        if !file.exists() {
            return;
        }
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => return,
        };
        match serde_json::from_str::<HashMap<String, String>>(&content) {
            Ok(map) => {
                *self.session_models.write() = map;
            }
            Err(e) => eprintln!("[sessions] load_session_models failed: {e}"),
        }
    }

    // ===================== 历史对话置顶 =====================

    pub fn is_pinned(&self, id: &str) -> bool {
        self.pinned_sessions.read().contains_key(id)
    }

    pub fn pinned_at(&self, id: &str) -> Option<String> {
        self.pinned_sessions.read().get(id).cloned()
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) {
        {
            let mut pins = self.pinned_sessions.write();
            if pinned {
                pins.insert(id.to_string(), Utc::now().to_rfc3339());
            } else {
                pins.remove(id);
            }
        }
        self.save_pinned_sessions();
    }

    pub fn save_pinned_sessions(&self) {
        let file = super::paths::sessions_root().join("_pinned_sessions.json");
        let pins = self.pinned_sessions.read();
        if pins.is_empty() {
            let _ = std::fs::remove_file(&file);
            return;
        }
        let mut out: Vec<_> = pins
            .iter()
            .map(|(id, pinned_at)| {
                serde_json::json!({
                    "id": id,
                    "pinned_at": pinned_at,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            a.get("id")
                .and_then(|v| v.as_str())
                .cmp(&b.get("id").and_then(|v| v.as_str()))
        });
        if let Ok(json) = serde_json::to_string_pretty(&out) {
            let _ = std::fs::write(file, json);
        }
    }

    pub fn load_pinned_sessions(&self) {
        let file = super::paths::sessions_root().join("_pinned_sessions.json");
        if !file.exists() {
            return;
        }
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => return,
        };
        match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(serde_json::Value::Array(items)) => {
                let mut pins = HashMap::new();
                for item in items {
                    match item {
                        serde_json::Value::String(id) => {
                            pins.insert(id, Utc::now().to_rfc3339());
                        }
                        serde_json::Value::Object(mut obj) => {
                            let id = obj.remove("id").and_then(|v| v.as_str().map(str::to_string));
                            let pinned_at = obj
                                .remove("pinned_at")
                                .and_then(|v| v.as_str().map(str::to_string))
                                .unwrap_or_else(|| Utc::now().to_rfc3339());
                            if let Some(id) = id {
                                pins.insert(id, pinned_at);
                            }
                        }
                        _ => {}
                    }
                }
                *self.pinned_sessions.write() = pins;
            }
            Ok(_) => eprintln!("[sessions] load_pinned_sessions failed: invalid shape"),
            Err(e) => eprintln!("[sessions] load_pinned_sessions failed: {e}"),
        }
    }

    // ===================== 收起任务列表 =====================

    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden_sessions.read().contains_key(id)
    }

    pub fn hidden_at(&self, id: &str) -> Option<String> {
        self.hidden_sessions.read().get(id).cloned()
    }

    pub fn set_hidden(&self, id: &str, hidden: bool) {
        {
            let mut hidden_sessions = self.hidden_sessions.write();
            if hidden {
                hidden_sessions.insert(id.to_string(), Utc::now().to_rfc3339());
            } else {
                hidden_sessions.remove(id);
            }
        }
        if hidden {
            self.set_pinned(id, false);
        }
        self.save_hidden_sessions();
    }

    pub fn save_hidden_sessions(&self) {
        let file = super::paths::sessions_root().join("_hidden_sessions.json");
        let hidden_sessions = self.hidden_sessions.read();
        if hidden_sessions.is_empty() {
            let _ = std::fs::remove_file(&file);
            return;
        }
        let mut out: Vec<_> = hidden_sessions
            .iter()
            .map(|(id, hidden_at)| {
                serde_json::json!({
                    "id": id,
                    "hidden_at": hidden_at,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            a.get("id")
                .and_then(|v| v.as_str())
                .cmp(&b.get("id").and_then(|v| v.as_str()))
        });
        if let Ok(json) = serde_json::to_string_pretty(&out) {
            let _ = std::fs::write(file, json);
        }
    }

    pub fn load_hidden_sessions(&self) {
        let file = super::paths::sessions_root().join("_hidden_sessions.json");
        if !file.exists() {
            return;
        }
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => return,
        };
        match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(serde_json::Value::Array(items)) => {
                let mut hidden_sessions = HashMap::new();
                for item in items {
                    match item {
                        serde_json::Value::String(id) => {
                            hidden_sessions.insert(id, Utc::now().to_rfc3339());
                        }
                        serde_json::Value::Object(mut obj) => {
                            let id = obj.remove("id").and_then(|v| v.as_str().map(str::to_string));
                            let hidden_at = obj
                                .remove("hidden_at")
                                .and_then(|v| v.as_str().map(str::to_string))
                                .unwrap_or_else(|| Utc::now().to_rfc3339());
                            if let Some(id) = id {
                                hidden_sessions.insert(id, hidden_at);
                            }
                        }
                        _ => {}
                    }
                }
                *self.hidden_sessions.write() = hidden_sessions;
            }
            Ok(_) => eprintln!("[sessions] load_hidden_sessions failed: invalid shape"),
            Err(e) => eprintln!("[sessions] load_hidden_sessions failed: {e}"),
        }
    }

}

/// 生成 URL-safe session id（短 8 字节 timestamp + nanos hash）。
/// 上游 `validated_session_path` 只允许 `[A-Za-z0-9_-]`，所以走 base32-like 字符集。
fn generate_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    const ALPHA: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut n = nanos;
    let mut buf = String::with_capacity(13);
    for _ in 0..13 {
        buf.push(ALPHA[(n % 36) as usize] as char);
        n /= 36;
    }
    buf
}

fn looks_like_truncating_overwrite(existing: &[Message], incoming: &[Message]) -> bool {
    if incoming.len() >= existing.len() || existing.len() <= 2 {
        return false;
    }
    let check = incoming.len().min(2);
    if check == 0 {
        return true;
    }
    for idx in 0..check {
        if existing[idx] != incoming[idx] {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::paths::tests::ENV_LOCK;
    use deepseek_tui::models::ContentBlock;

    /// 借用 paths 模块的进程级 env 锁——避免与其他 mutate PINVOU3_HOME
    /// 的测试并行 race。返回带 guard 的 store；guard drop 后才解锁。
    fn isolated_store() -> (SessionStore, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = format!(
            "/tmp/pinvou3-sessions-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        std::env::set_var("PINVOU3_HOME", &tmp);
        let store = SessionStore::boot().expect("boot");
        // 注意：不 remove_var——锁还没 drop，下面的断言需要 PINVOU3_HOME 仍是这个值。
        (store, guard)
    }

    fn user_text(text: &str) -> Message {
        Message {
            role: "user".into(),
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        }
    }

    fn assistant_text(text: &str) -> Message {
        Message {
            role: "assistant".into(),
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        }
    }

    #[test]
    fn create_new_persists_and_lists() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let list = store.list().expect("list");
        assert!(list.iter().any(|m| m.id == s.metadata.id));
    }

    #[test]
    fn set_title_updates_metadata() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        store
            .set_title(&s.metadata.id, "改个名字".into())
            .expect("rename");
        let loaded = store.load(&s.metadata.id).expect("load");
        assert_eq!(loaded.metadata.title, "改个名字");
    }

    #[test]
    fn update_messages_rejects_unrelated_short_overwrite() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        store
            .update_messages(
                &s.metadata.id,
                vec![user_text("old 1"), assistant_text("old 2"), user_text("old 3")],
            )
            .expect("seed messages");

        let result = store.update_messages(
            &s.metadata.id,
            vec![user_text("new unrelated"), assistant_text("new answer")],
        );

        assert!(result.is_err(), "short unrelated overwrite is rejected");
        let loaded = store.load(&s.metadata.id).expect("load");
        assert_eq!(loaded.messages.len(), 3);
    }

    #[test]
    fn delete_removes_session() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        store.delete(&s.metadata.id).expect("delete");
        assert!(store.load(&s.metadata.id).is_err(), "load after delete");
    }

    #[test]
    fn active_id_tracks_set_active() {
        let (store, _g) = isolated_store();
        assert!(store.active_id().is_none());
        store.set_active(Some("abc".into()));
        assert_eq!(store.active_id().as_deref(), Some("abc"));
        store.set_active(None);
        assert!(store.active_id().is_none());
    }

    #[test]
    fn delete_active_clears_active_id() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        store.set_active(Some(s.metadata.id.clone()));
        store.delete(&s.metadata.id).expect("delete");
        assert!(store.active_id().is_none(), "delete active clears tracker");
    }

    #[test]
    fn delete_missing_session_file_is_idempotent() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let session_file = paths::sessions_root().join(format!("{}.json", s.metadata.id));
        let session_dir = paths::sessions_root().join(&s.metadata.id);
        std::fs::create_dir_all(&session_dir).expect("session dir");
        std::fs::remove_file(&session_file).expect("remove session file");

        store.delete(&s.metadata.id).expect("delete missing file");

        assert!(!session_dir.exists(), "stale session dir removed");
    }

    #[test]
    fn pinned_sessions_persist_and_delete_cleans() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");

        store.set_pinned(&s.metadata.id, true);
        assert!(store.is_pinned(&s.metadata.id));
        assert!(
            store.pinned_at(&s.metadata.id).is_some(),
            "pinning records pinned_at"
        );

        let reloaded = SessionStore::boot().expect("reboot");
        reloaded.load_pinned_sessions();
        assert!(reloaded.is_pinned(&s.metadata.id));
        assert!(
            reloaded.pinned_at(&s.metadata.id).is_some(),
            "pinned_at survives reload"
        );

        reloaded.delete(&s.metadata.id).expect("delete");
        assert!(!reloaded.is_pinned(&s.metadata.id));
        assert!(reloaded.pinned_at(&s.metadata.id).is_none());
    }

    #[test]
    fn pinned_sessions_loads_legacy_id_array() {
        let (_store, _g) = isolated_store();
        let file = super::paths::sessions_root().join("_pinned_sessions.json");
        std::fs::create_dir_all(super::paths::sessions_root()).expect("mkdir");
        std::fs::write(&file, r#"["legacy-session"]"#).expect("write legacy pins");

        let reloaded = SessionStore::boot().expect("reboot");
        reloaded.load_pinned_sessions();
        assert!(reloaded.is_pinned("legacy-session"));
        assert!(
            reloaded.pinned_at("legacy-session").is_some(),
            "legacy pins receive a migration timestamp"
        );
    }

    #[test]
    fn hidden_sessions_persist_restore_and_delete_cleans() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");

        store.set_hidden(&s.metadata.id, true);
        assert!(store.is_hidden(&s.metadata.id));
        assert!(
            store.hidden_at(&s.metadata.id).is_some(),
            "hiding records hidden_at"
        );

        let reloaded = SessionStore::boot().expect("reboot");
        reloaded.load_hidden_sessions();
        assert!(reloaded.is_hidden(&s.metadata.id));
        assert!(
            reloaded.hidden_at(&s.metadata.id).is_some(),
            "hidden_at survives reload"
        );

        reloaded.set_hidden(&s.metadata.id, false);
        assert!(!reloaded.is_hidden(&s.metadata.id));
        assert!(reloaded.hidden_at(&s.metadata.id).is_none());

        reloaded.set_hidden(&s.metadata.id, true);
        reloaded.delete(&s.metadata.id).expect("delete");
        assert!(!reloaded.is_hidden(&s.metadata.id));
        assert!(reloaded.hidden_at(&s.metadata.id).is_none());
    }

    #[test]
    fn hidden_sessions_loads_legacy_id_array() {
        let (_store, _g) = isolated_store();
        let file = super::paths::sessions_root().join("_hidden_sessions.json");
        std::fs::create_dir_all(super::paths::sessions_root()).expect("mkdir");
        std::fs::write(&file, r#"["legacy-hidden-session"]"#).expect("write legacy hidden");

        let reloaded = SessionStore::boot().expect("reboot");
        reloaded.load_hidden_sessions();
        assert!(reloaded.is_hidden("legacy-hidden-session"));
        assert!(
            reloaded.hidden_at("legacy-hidden-session").is_some(),
            "legacy hidden sessions receive a migration timestamp"
        );
    }

    #[test]
    fn hiding_session_clears_pinned_state() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");

        store.set_pinned(&s.metadata.id, true);
        assert!(store.is_pinned(&s.metadata.id));

        store.set_hidden(&s.metadata.id, true);
        assert!(store.is_hidden(&s.metadata.id));
        assert!(!store.is_pinned(&s.metadata.id));
        assert!(store.pinned_at(&s.metadata.id).is_none());

        let reloaded = SessionStore::boot().expect("reboot");
        reloaded.load_pinned_sessions();
        reloaded.load_hidden_sessions();
        assert!(reloaded.is_hidden(&s.metadata.id));
        assert!(!reloaded.is_pinned(&s.metadata.id));
    }

    #[test]
    fn generate_session_id_url_safe() {
        let id = generate_session_id();
        assert!(id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn pinvou_review_defaults_off() {
        let (store, _g) = isolated_store();
        assert!(!store.mode_state("s1").pinvou_review_enabled);
    }

    #[test]
    fn set_pinvou_review_persists() {
        let (store, _g) = isolated_store();
        store.set_pinvou_review("s1", true);
        assert!(store.mode_state("s1").pinvou_review_enabled);
        store.set_pinvou_review("s1", false);
        assert!(!store.mode_state("s1").pinvou_review_enabled);
    }

    #[test]
    fn set_mode_preserves_pinvou_review() {
        // 关键不变量:切 mode(set_mode)不能覆盖品悟开关。
        let (store, _g) = isolated_store();
        store.set_pinvou_review("s1", true);
        store.set_mode("s1", super::super::mode_state::SerializableMode::Yolo);
        let state = store.mode_state("s1");
        assert!(state.pinvou_review_enabled);
        assert!(matches!(
            state.mode,
            super::super::mode_state::SerializableMode::Yolo
        ));
    }

    /// 模式切换闭环(回归底座二态后的核心契约):流转命令 set_plan_mode_next(→Plan) /
    /// accept_plan / exit_plan_to_yolo(→Yolo) 实质都只调 set_mode,全程**只动 mode**——
    /// 品悟开关 / 挂载知识集 / 人格卡 / skill 绑定等正交状态必须原样保留。
    /// (discard_plan「算了」不在此列:放弃方案但留在当前 mode,不调 set_mode。)
    /// 防有人给流转命令加副作用,或把 set_mode 改成整体覆盖式写法时连带清掉这些字段。
    /// 比 set_mode_preserves_pinvou_review 更全(多步往返 + 四字段)。
    #[test]
    fn mode_switch_loop_preserves_orthogonal_state() {
        use super::super::mode_state::SerializableMode;
        let (store, _g) = isolated_store();
        let sid = "s-loop";

        // 起始默认 Yolo,挂满正交状态
        assert_eq!(store.mode_state(sid).mode, SerializableMode::Yolo);
        store.set_pinvou_review(sid, true);
        store.set_mounted_collection(sid, Some(42));
        store.set_active_persona(sid, Some("expert-x".into()));
        store.bind_skill(
            sid,
            ActiveSkillBinding {
                name: "h3c-ppt".into(),
                pending_instruction: None,
                phases: vec![],
                project_dir: None,
            },
        );

        // 闭环往返两轮:Yolo →(set_plan_mode_next)→ Plan →(accept/exit)→ Yolo
        for _ in 0..2 {
            store.set_mode(sid, SerializableMode::Plan);
            assert_eq!(store.mode_state(sid).mode, SerializableMode::Plan);
            store.set_mode(sid, SerializableMode::Yolo);
            assert_eq!(store.mode_state(sid).mode, SerializableMode::Yolo);
        }

        // 四个正交字段全保留
        let st = store.mode_state(sid);
        assert!(st.pinvou_review_enabled, "切 mode 清了品悟开关");
        assert_eq!(st.mounted_collection, Some(42), "切 mode 卸载了知识集");
        assert_eq!(
            st.active_persona.as_deref(),
            Some("expert-x"),
            "切 mode 清了人格"
        );
        assert_eq!(
            st.active_skill.map(|s| s.name),
            Some("h3c-ppt".to_string()),
            "切 mode 解绑了 skill"
        );
    }

    #[test]
    fn bind_skill_then_take_consumes_once_and_returns_binding() {
        let (store, _g) = isolated_store();
        store.bind_skill(
            "s1",
            ActiveSkillBinding {
                name: "h3c-ppt".into(),
                pending_instruction: Some("PREPEND".into()),
                phases: vec![],
                project_dir: None,
            },
        );
        let b = store.active_skill("s1").expect("bound");
        assert_eq!(b.name, "h3c-ppt");
        // pending_instruction 被 #[serde(skip)] 标记,active_skill 路径走的是
        // .clone() 不影响 pending_instruction(它仍存在原 entry 上),take 走另一路径
        assert_eq!(
            store.take_pending_skill_instruction("s1").as_deref(),
            Some("PREPEND")
        );
        assert!(store.take_pending_skill_instruction("s1").is_none());
        // 取走 instruction 后 active_skill 仍能返回 binding(name+phases),
        // 仅 pending_instruction 槽位被消费 — 关键:phases 不丢
        let b2 = store.active_skill("s1").expect("still bound");
        assert_eq!(b2.name, "h3c-ppt");
    }

    #[test]
    fn unbind_skill_clears_binding() {
        let (store, _g) = isolated_store();
        store.bind_skill(
            "s1",
            ActiveSkillBinding {
                name: "h3c-ppt".into(),
                pending_instruction: None,
                phases: vec![],
                project_dir: None,
            },
        );
        assert!(store.active_skill("s1").is_some());
        store.unbind_skill("s1");
        assert!(store.active_skill("s1").is_none());
    }

    #[test]
    fn bind_skill_preserves_mode() {
        // 绑定/解绑 skill 不能动 mode / pinvou_review_enabled。
        let (store, _g) = isolated_store();
        store.set_pinvou_review("s1", true);
        store.set_mode("s1", super::super::mode_state::SerializableMode::Plan);
        store.bind_skill(
            "s1",
            ActiveSkillBinding {
                name: "h3c-ppt".into(),
                pending_instruction: None,
                phases: vec![],
                project_dir: None,
            },
        );
        let state = store.mode_state("s1");
        assert!(state.pinvou_review_enabled);
        assert!(matches!(
            state.mode,
            super::super::mode_state::SerializableMode::Plan
        ));
        store.unbind_skill("s1");
        let state2 = store.mode_state("s1");
        assert!(state2.pinvou_review_enabled);
        assert!(matches!(
            state2.mode,
            super::super::mode_state::SerializableMode::Plan
        ));
    }
}
