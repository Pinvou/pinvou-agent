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
//! Mode / plan-phase remain in-memory only by design.

use std::collections::HashMap;
use std::io::ErrorKind;

use super::SessionStore;
use anyhow::{Context, Result};
use chrono::Utc;

const SESSION_MODELS_FILE: &str = "_session_models.json";
const PINNED_SESSIONS_FILE: &str = "_pinned_sessions.json";
const HIDDEN_SESSIONS_FILE: &str = "_hidden_sessions.json";
const AUX_SESSIONS_FILE: &str = "_aux_sessions.json";

/// Shared save core for the pinned / hidden sidecars: both files carry the
/// same shape (an array of `{ id, <ts_key> }` objects sorted by id) and the
/// same "empty map deletes the file" semantics. Failures are silently ignored
/// (best-effort persistence, matching the historical per-file implementations).
fn save_timestamped_id_map(map: &HashMap<String, String>, file_name: &str, ts_key: &str) {
    let file = crate::platform::paths::sessions_root().join(file_name);
    if map.is_empty() {
        let _ = std::fs::remove_file(&file);
        return;
    }
    let mut out: Vec<_> = map
        .iter()
        .map(|(id, timestamp)| {
            serde_json::json!({
                "id": id,
                (ts_key): timestamp,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.get("id")
            .and_then(|v| v.as_str())
            .cmp(&b.get("id").and_then(|v| v.as_str()))
    });
    if let Ok(json) = serde_json::to_string_pretty(&out) {
        let _ = deepseek_tui::utils::write_atomic(&file, json.as_bytes());
    }
}

/// Shared load core for the pinned / hidden sidecars. `None` = nothing to load
/// (missing / unreadable file or invalid shape, the latter logged with
/// `label` so the historical per-file diagnostics stay unchanged). Bare string
/// entries are re-stamped with the current time, matching the legacy format
/// that stored a plain id list.
fn load_timestamped_id_map(
    file_name: &str,
    ts_key: &str,
    label: &str,
) -> Option<HashMap<String, String>> {
    let file = crate::platform::paths::sessions_root().join(file_name);
    if !file.exists() {
        return None;
    }
    let content = match std::fs::read_to_string(&file) {
        Ok(c) => c,
        Err(_) => return None,
    };
    match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(serde_json::Value::Array(items)) => {
            let mut parsed = HashMap::new();
            for item in items {
                match item {
                    serde_json::Value::String(id) => {
                        parsed.insert(id, Utc::now().to_rfc3339());
                    }
                    serde_json::Value::Object(mut obj) => {
                        let id = obj
                            .remove("id")
                            .and_then(|v| v.as_str().map(str::to_string));
                        let timestamp = obj
                            .remove(ts_key)
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_else(|| Utc::now().to_rfc3339());
                        if let Some(id) = id {
                            parsed.insert(id, timestamp);
                        }
                    }
                    _ => {}
                }
            }
            Some(parsed)
        }
        Ok(_) => {
            eprintln!("[sessions] {label} failed: invalid shape");
            None
        }
        Err(e) => {
            eprintln!("[sessions] {label} failed: {e}");
            None
        }
    }
}

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
        let file = crate::platform::paths::sessions_root().join(SESSION_MODELS_FILE);
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
        let pins = self.pinned_sessions.read();
        save_timestamped_id_map(&pins, PINNED_SESSIONS_FILE, "pinned_at");
    }

    pub fn load_pinned_sessions(&self) {
        if let Some(pins) =
            load_timestamped_id_map(PINNED_SESSIONS_FILE, "pinned_at", "load_pinned_sessions")
        {
            *self.pinned_sessions.write() = pins;
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
        let hidden_sessions = self.hidden_sessions.read();
        save_timestamped_id_map(&hidden_sessions, HIDDEN_SESSIONS_FILE, "hidden_at");
    }

    pub fn load_hidden_sessions(&self) {
        if let Some(hidden_sessions) =
            load_timestamped_id_map(HIDDEN_SESSIONS_FILE, "hidden_at", "load_hidden_sessions")
        {
            *self.hidden_sessions.write() = hidden_sessions;
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
        // The loaded-flag invariant is sealed at the single write API too: with
        // `aux_sessions_loaded` false the in-memory map is artificially empty
        // (the sidecar read failed this boot), and one write here would
        // persist that map minus nothing plus the new entry — wiping every
        // existing binding on disk (the mod.rs field comment's disaster).
        // Every current caller runs under the flag (get-or-create checks,
        // the reconciliation skips, creation runs after the check), but the
        // guard keeps the write unreachable for future callers instead of
        // relying on that discipline.
        if !self
            .aux_sessions_loaded
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            anyhow::bail!("Aux session bindings were not loaded this boot; refusing to write");
        }
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
        if super::validators::is_aux_session_id(main_id)
            || super::validators::is_sched_session_id(main_id)
        {
            anyhow::bail!("Auxiliary mapping key must be an unprefixed main session id: {main_id}");
        }
        if let Some(aux_id) = &aux_id {
            super::validators::validate_session_id(aux_id)?;
            if !super::validators::is_aux_session_id(aux_id) {
                anyhow::bail!("Auxiliary session id must start with 'aux-': {aux_id}");
            }
        }
        let mut aux_sessions = self.aux_sessions.write();
        // Values are unique across the map (round-20 minor-12): two mains must
        // never resolve to the same aux — a doubled mapping makes one parent's
        // panel read the other's transcript. The check lives at this single
        // write API so the invariant cannot depend on caller discipline.
        if let Some(aux_id) = &aux_id {
            if aux_sessions
                .iter()
                .any(|(key, value)| value == aux_id && key != main_id)
            {
                anyhow::bail!(
                    "Auxiliary session is already bound to another main session: {aux_id}"
                );
            }
        }
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
                // Name the sidecar, never its absolute path: `save_aux_sessions`
                // prints the chain, and the sessions root embeds the host home
                // directory (same no-host-paths stance as the read-failure log
                // below).
                Err(error) => Err(error).with_context(|| format!("remove {AUX_SESSIONS_FILE}")),
            };
        }
        let payload =
            serde_json::to_vec_pretty(aux_sessions).context("serialize aux session bindings")?;
        deepseek_tui::utils::write_atomic(&file, &payload)
            .with_context(|| format!("persist aux session bindings to {AUX_SESSIONS_FILE}"))
    }

    pub fn save_aux_sessions(&self) {
        if let Err(error) = Self::persist_aux_sessions(&self.aux_sessions.read()) {
            eprintln!("[sessions] save_aux_sessions failed: {error:#}");
        }
    }

    pub fn load_aux_sessions(&self) {
        let file = crate::platform::paths::sessions_root().join(AUX_SESSIONS_FILE);
        // Absent sidecar = nothing to load, and that is a healthy state (the
        // flag still flips true): only a read failure on an existing file
        // must leave the "not loaded" mark that fails get_or_create and the
        // startup reconciliation closed. NotFound-only (round-23 should-fix
        // 3): the previous `!file.exists()` pre-probe conflated a transient
        // stat fault (EACCES under sync/AV clients, EIO) with "absent" and
        // flipped the loaded flag over a healthy sidecar — the boot then ran
        // the reconciliation against an artificially empty map and the
        // rebuild could overwrite it, the exact exists() conflation class
        // `durable_session_record_is_absent` eradicated elsewhere. One read,
        // classified by error kind, same taxonomy.
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.aux_sessions_loaded
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                return;
            }
            // A silent swallow here chains into real transcript loss: the
            // empty in-memory map makes the next set_aux_session overwrite
            // the sidecar with only the fresh mapping, and the next boot's
            // reconciliation then deletes the pre-existing aux records as
            // ambiguous duplicates. Log (same level as the parse-error
            // branch below) and leave aux_sessions_loaded false so the
            // writers/reconciler fail closed this boot.
            Err(error) => {
                eprintln!("[sessions] load_aux_sessions read failed: {error}");
                return;
            }
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
                // entries are sorted by (key, value) and the first claim wins
                // — which owner survives is stable across boots. The winning
                // owner keeps its mapping; the losers become mapping-less
                // orphans, whose records the startup reconciliation rebuilds
                // from the backlink when unambiguous or reclaims otherwise.
                let mut entries: Vec<(String, String)> = map.into_iter().collect();
                entries.sort();
                let mut seen_aux_ids: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                let map: HashMap<String, String> = entries
                    .into_iter()
                    .filter(|(main_id, aux_id)| {
                        let valid = super::validators::validate_session_id(main_id).is_ok()
                            && super::validators::validate_session_id(aux_id).is_ok()
                            && super::validators::is_aux_session_id(aux_id)
                            && !super::validators::is_aux_session_id(main_id)
                            && !super::validators::is_sched_session_id(main_id)
                            && main_id != aux_id
                            && seen_aux_ids.insert(aux_id.clone());
                        if !valid {
                            // No raw ids in logs (the CodeQL cleartext-logging
                            // stance this PR settled on); the entry kinds still
                            // identify the dropped shape for debugging.
                            eprintln!("[sessions] drop invalid aux mapping entry");
                        }
                        valid
                    })
                    .collect();
                *self.aux_sessions.write() = map;
            }
            // A parse failure is NOT the fail-closed case: the sidecar's
            // content is provably unrecoverable, so the records+backlinks on
            // disk carry all remaining truth, and orphan classification (with
            // its backlink-first rebuild) is safe to run. The corrupted file
            // is only ever overwritten once a fresh, validated mapping exists
            // (the empty-map write is suppressed by the not-loaded flag only
            // for the read-failure case, where the file may be intact).
            Err(e) => {
                eprintln!("[sessions] load_aux_sessions failed: {e}");
                self.aux_sessions_loaded
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                return;
            }
        }
        self.aux_sessions_loaded
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}
