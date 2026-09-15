//! Sidecar persistence for per-session auxiliary state.
//!
//! The durable `SavedSession` cannot grow new fields without changing the
//! upstream schema, so four independent JSON sidecars under
//! `~/.pinvou3/sessions/` capture cross-restart runtime state that must
//! survive a process bounce:
//!
//! - `_session_models.json` — session_id -> SavedModel.id override.
//! - `_pinned_sessions.json` — pinned conversation id list with timestamps.
//! - `_hidden_sessions.json` — collapsed conversation id list with timestamps.
//! - `_aux_sessions.json` — main session_id -> aux (`aux-` prefixed) session_id.
//!
//! Mode / pinvou_review / plan-phase remain in-memory only by design.

use std::collections::HashMap;
use std::io::ErrorKind;

use super::SessionStore;
use anyhow::{Context, Result};
use chrono::Utc;

const SESSION_MODELS_FILE: &str = "_session_models.json";
const PINNED_SESSIONS_FILE: &str = "_pinned_sessions.json";
const HIDDEN_SESSIONS_FILE: &str = "_hidden_sessions.json";
const AUX_SESSIONS_FILE: &str = "_aux_sessions.json";

impl SessionStore {
    pub fn session_model_id(&self, id: &str) -> Option<String> {
        self.session_model_override(id).or_else(|| {
            self.scheduled_profile(id)
                .and_then(|profile| profile.model_id)
        })
    }

    pub fn session_model_override(&self, id: &str) -> Option<String> {
        self.session_models.read().get(id).cloned()
    }

    pub fn set_session_model_id(&self, id: &str, model_id: Option<String>) -> Result<()> {
        let mut models = self.session_models.write();
        let previous = models.get(id).cloned();
        match model_id {
            Some(mid) => {
                models.insert(id.to_string(), mid);
            }
            None => {
                models.remove(id);
            }
        }
        if let Err(error) = Self::persist_session_models(&models) {
            match previous {
                Some(previous) => {
                    models.insert(id.to_string(), previous);
                }
                None => {
                    models.remove(id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn persist_session_models(models: &HashMap<String, String>) -> Result<()> {
        let file = crate::platform::paths::sessions_root().join("_session_models.json");
        if models.is_empty() {
            return match std::fs::remove_file(&file) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error).with_context(|| format!("remove {}", file.display())),
            };
        }
        let payload =
            serde_json::to_vec_pretty(models).context("serialize per-session model bindings")?;
        deepseek_tui::utils::write_atomic(&file, &payload)
            .with_context(|| format!("persist per-session model bindings to {}", file.display()))
    }

    pub fn save_session_models(&self) {
        if let Err(error) = Self::persist_session_models(&self.session_models.read()) {
            eprintln!("[sessions] save_session_models failed: {error:#}");
        }
    }

    pub fn load_session_models(&self) {
        let file = crate::platform::paths::sessions_root().join("_session_models.json");
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
        let file = crate::platform::paths::sessions_root().join("_pinned_sessions.json");
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
        let file = crate::platform::paths::sessions_root().join("_pinned_sessions.json");
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
        let file = crate::platform::paths::sessions_root().join("_hidden_sessions.json");
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
        let file = crate::platform::paths::sessions_root().join("_hidden_sessions.json");
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

    /// Auxiliary conversation mapping lookup: main session id → aux session
    /// (`aux-` prefixed) id.
    pub fn aux_session_id(&self, main_id: &str) -> Option<String> {
        self.aux_sessions.read().get(main_id).cloned()
    }

    /// Write/clear the main→aux mapping and persist it; on persist failure the
    /// in-memory state is rolled back — same transactional semantics as
    /// `set_session_model_id`: never leave an in-memory state that "looks
    /// successful but is lost on restart".
    pub fn set_aux_session(&self, main_id: &str, aux_id: Option<String>) -> Result<()> {
        // The pub write entry seals both ends of the mapping: keys must be
        // valid, unprefixed main-session ids, values must carry the aux-
        // prefix. The cascade-delete depth bound and the "aux- ids skip the
        // creation lock" argument both rest on keys/values keeping these
        // shapes; the invariant is guarded at load and creation and sealed
        // here at the single write API (same defense as
        // validate_scheduled_session_id on registry keys). Note: the
        // value==key (self-mapping) case cannot survive this validation —
        // the value must be aux- prefixed while the key must not be.
        super::validators::validate_session_id(main_id)?;
        if main_id.starts_with("aux-") || main_id.starts_with("sched-") {
            anyhow::bail!("Auxiliary mapping key must be an unprefixed main session id: {main_id}");
        }
        if let Some(aux_id) = &aux_id {
            super::validators::validate_session_id(aux_id)?;
            if !aux_id.starts_with("aux-") {
                anyhow::bail!("Auxiliary session id must start with 'aux-': {aux_id}");
            }
        }
        let mut aux_sessions = self.aux_sessions.write();
        let previous = aux_sessions.get(main_id).cloned();
        match aux_id {
            Some(aux_id) => {
                aux_sessions.insert(main_id.to_string(), aux_id);
            }
            None => {
                aux_sessions.remove(main_id);
            }
        }
        if let Err(error) = Self::persist_aux_sessions(&aux_sessions) {
            match previous {
                Some(previous) => {
                    aux_sessions.insert(main_id.to_string(), previous);
                }
                None => {
                    aux_sessions.remove(main_id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn persist_aux_sessions(aux_sessions: &HashMap<String, String>) -> Result<()> {
        let file = crate::platform::paths::sessions_root().join(AUX_SESSIONS_FILE);
        if aux_sessions.is_empty() {
            return match std::fs::remove_file(&file) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error).with_context(|| format!("remove {}", file.display())),
            };
        }
        let payload =
            serde_json::to_vec_pretty(aux_sessions).context("serialize aux session bindings")?;
        deepseek_tui::utils::write_atomic(&file, &payload)
            .with_context(|| format!("persist aux session bindings to {}", file.display()))
    }

    pub fn save_aux_sessions(&self) {
        if let Err(error) = Self::persist_aux_sessions(&self.aux_sessions.read()) {
            eprintln!("[sessions] save_aux_sessions failed: {error:#}");
        }
    }

    pub fn load_aux_sessions(&self) {
        let file = crate::platform::paths::sessions_root().join(AUX_SESSIONS_FILE);
        if !file.exists() {
            return;
        }
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => return,
        };
        match serde_json::from_str::<HashMap<String, String>>(&content) {
            Ok(map) => {
                // Corrupted-but-parseable entries must be dropped: a mapping
                // whose key/value is not a valid session id, or whose value
                // lacks the aux- prefix, would let get_or_create return the
                // main session as its own aux and write side-chat questions
                // straight into the main context (same load-time defense as
                // the sched- profile load); entries with aux-/sched- prefixed
                // keys or self-mappings would remove the "main→aux, one level"
                // depth bound from delete's cascade recursion (stack
                // overflow), so they are dropped too.
                // Duplicate values (two mains mapping to the same aux) are
                // dropped *deterministically*: iterating a HashMap would hand
                // ownership to per-process RandomState iteration order, so the
                // entries are sorted by (value, key) and the first claim wins
                // — which owner survives is stable across boots. The
                // duplicates become mapping-less orphans; startup
                // reconciliation rebuilds the one whose record backlink
                // matches and reclaims the rest.
                let mut entries: Vec<(String, String)> = map.into_iter().collect();
                entries.sort();
                let mut seen_aux_ids: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                let map: HashMap<String, String> = entries
                    .into_iter()
                    .filter(|(main_id, aux_id)| {
                        let valid = super::validators::validate_session_id(main_id).is_ok()
                            && super::validators::validate_session_id(aux_id).is_ok()
                            && aux_id.starts_with("aux-")
                            && !main_id.starts_with("aux-")
                            && !main_id.starts_with("sched-")
                            && main_id != aux_id
                            && seen_aux_ids.insert(aux_id.clone());
                        if !valid {
                            eprintln!("[sessions] drop invalid aux mapping {main_id} -> {aux_id}");
                        }
                        valid
                    })
                    .collect();
                *self.aux_sessions.write() = map;
            }
            Err(e) => eprintln!("[sessions] load_aux_sessions failed: {e}"),
        }
    }
}
