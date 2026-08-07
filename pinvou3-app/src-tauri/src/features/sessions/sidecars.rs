//! Sidecar persistence for per-session auxiliary state.
//!
//! The durable `SavedSession` cannot grow new fields without changing the
//! upstream schema, so four independent JSON sidecars under
//! `~/.pinvou3/sessions/` capture cross-restart runtime state that must
//! survive a process bounce:
//!
//! - `_skill_bindings.json` — session_id -> active skill binding
//!   (persisted against `mode_states`; see [`save_skill_bindings`] /
//!   [`load_skill_bindings`]).
//! - `_session_models.json` — session_id -> SavedModel.id override.
//! - `_pinned_sessions.json` — pinned conversation id list with timestamps.
//! - `_hidden_sessions.json` — collapsed conversation id list with timestamps.
//!
//! Mode / pinvou_review / plan-phase remain in-memory only by design.

use std::collections::HashMap;

use chrono::Utc;
use parking_lot::RwLock;

use super::SessionModeState;

use super::SessionStore;

const SKILL_BINDINGS_FILE: &str = "_skill_bindings.json";
const SESSION_MODELS_FILE: &str = "_session_models.json";
const PINNED_SESSIONS_FILE: &str = "_pinned_sessions.json";
const HIDDEN_SESSIONS_FILE: &str = "_hidden_sessions.json";

/// 持久化所有 skill binding 到磁盘。
pub(crate) fn save_skill_bindings(mode_states: &RwLock<HashMap<String, SessionModeState>>) {
    let bindings_file = crate::platform::paths::sessions_root().join(SKILL_BINDINGS_FILE);
    let m = mode_states.read();
    let bindings: HashMap<String, &super::ActiveSkillBinding> = m
        .iter()
        .filter_map(|(id, state)| state.active_skill.as_ref().map(|s| (id.clone(), s)))
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
pub(crate) fn load_skill_bindings(mode_states: &RwLock<HashMap<String, SessionModeState>>) {
    let bindings_file = crate::platform::paths::sessions_root().join(SKILL_BINDINGS_FILE);
    if !bindings_file.exists() {
        return;
    }
    let content = match std::fs::read_to_string(&bindings_file) {
        Ok(c) => c,
        Err(_) => return,
    };
    let bindings: HashMap<String, super::ActiveSkillBinding> = match serde_json::from_str(&content)
    {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[sessions] load_skill_bindings failed: {e}");
            return;
        }
    };
    let mut m = mode_states.write();
    for (id, binding) in bindings {
        let entry = m.entry(id).or_default();
        entry.active_skill = Some(binding);
    }
}

impl SessionStore {
    // ===================== per-session 模型绑定 =====================

    /// 取该 session 在输入栏应显示的模型 id。普通会话无绑定时返回 None；
    /// 定时会话首次打开时回退创建任务时的模型，用户手动切换后返回交互覆盖值。
    pub fn session_model_id(&self, id: &str) -> Option<String> {
        self.session_model_override(id).or_else(|| {
            self.scheduled_profile(id)
                .and_then(|profile| profile.model_id)
        })
    }

    /// 只读取用户在对话输入栏里选择的模型，不包含定时运行创建时的模型回退。
    pub fn session_model_override(&self, id: &str) -> Option<String> {
        self.session_models.read().get(id).cloned()
    }

    /// 设/清该 session 的模型 id 并落盘。`None` = 清除(回退全局默认)。
    pub fn set_session_model_id(
        &self,
        id: &str,
        model_id: Option<String>,
    ) -> Result<(), anyhow::Error> {
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
        Ok(())
    }

    /// 持久化 per-session 模型绑定到 `~/.pinvou3/sessions/_session_models.json`。
    pub fn save_session_models(&self) {
        let file = crate::platform::paths::sessions_root().join(SESSION_MODELS_FILE);
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
        let file = crate::platform::paths::sessions_root().join(SESSION_MODELS_FILE);
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
        let file = crate::platform::paths::sessions_root().join(PINNED_SESSIONS_FILE);
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
        let file = crate::platform::paths::sessions_root().join(PINNED_SESSIONS_FILE);
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
                            let id = obj
                                .remove("id")
                                .and_then(|v| v.as_str().map(str::to_string));
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
        let file = crate::platform::paths::sessions_root().join(HIDDEN_SESSIONS_FILE);
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
        let file = crate::platform::paths::sessions_root().join(HIDDEN_SESSIONS_FILE);
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
                            let id = obj
                                .remove("id")
                                .and_then(|v| v.as_str().map(str::to_string));
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
