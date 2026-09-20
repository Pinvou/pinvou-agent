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

/// Durable write for the pinned / hidden sidecars: both files carry the same
/// shape (an array of `{ id, <ts_key> }` objects sorted by id) and the same
/// "empty map deletes the file" semantics. Persisted through tmp+rename:
/// these files are the cross-process truth for their consumers (the pinned
/// map gates the retention sweep), and a plain truncating write would let a
/// concurrent reader in another process observe an empty or partial file.
fn write_timestamped_id_map(
    file: &std::path::Path,
    entries: &HashMap<String, String>,
    file_name: &str,
    ts_key: &str,
) -> Result<()> {
    if entries.is_empty() {
        return match std::fs::remove_file(file) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(anyhow::Error::new(error).context(format!("remove {}", file.display())))
            }
        };
    }
    let mut out: Vec<_> = entries
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
    let json =
        serde_json::to_string_pretty(&out).context(format!("serialize {file_name} failed"))?;
    crate::platform::filesystem::atomic_write(file, json.as_bytes())
        .with_context(|| format!("persist {file_name} failed"))
}

/// Whole-map save of the pinned / hidden sidecars: writes exactly the given
/// map. Only boot fixtures and tests should reach for this — production
/// mutation paths must go through [`apply_timestamped_id_mutation`], because
/// the in-memory maps are boot-time snapshots and a wholesale write would
/// revert concurrent changes persisted by another process.
fn save_timestamped_id_map(map: &HashMap<String, String>, file_name: &str, ts_key: &str) {
    let file = crate::platform::paths::sessions_root().join(file_name);
    if let Err(error) = write_timestamped_id_map(&file, map, file_name, ts_key) {
        eprintln!("[sessions] persist {file_name} failed: {error:#}");
    }
}

/// Apply id-level upserts/removes directly to a pinned/hidden sidecar file,
/// leaving every entry this process never touched exactly as another process
/// wrote it. The in-memory maps are boot-time snapshots plus this process's
/// own changes, so rewriting them wholesale onto the shared store silently
/// reverts what a concurrent GUI/headless process persisted after this one
/// booted: a headless retention purge would erase a pin the GUI just added,
/// and a GUI save would resurrect an id the headless process just evicted.
/// The file stays the cross-process truth; the in-memory map is only this
/// process's read cache.
fn apply_timestamped_id_mutation(
    file_name: &str,
    ts_key: &str,
    label: &str,
    upserts: &[(&str, String)],
    removes: &[&str],
) -> Result<()> {
    let file = crate::platform::paths::sessions_root().join(file_name);
    let mut entries = load_timestamped_id_map(file_name, ts_key, label).unwrap_or_default();
    let mut changed = false;
    for (id, timestamp) in upserts {
        if entries.get(*id).map(String::as_str) != Some(timestamp.as_str()) {
            entries.insert((*id).to_string(), timestamp.clone());
            changed = true;
        }
    }
    for id in removes {
        changed |= entries.remove(*id).is_some();
    }
    if !changed {
        return Ok(());
    }
    write_timestamped_id_map(&file, &entries, file_name, ts_key)
}

impl SessionStore {
    /// Retention-purge half of [`Self::set_pinned`]: drops the given ids from
    /// the durable pin file without rewriting entries it does not own.
    pub(crate) fn purge_pinned_ids(&self, ids: &[&str]) -> Result<()> {
        apply_timestamped_id_mutation(
            PINNED_SESSIONS_FILE,
            "pinned_at",
            "load_pinned_sessions",
            &[],
            ids,
        )
    }

    /// Retention-purge half of [`Self::set_hidden`].
    pub(crate) fn purge_hidden_ids(&self, ids: &[&str]) -> Result<()> {
        apply_timestamped_id_mutation(
            HIDDEN_SESSIONS_FILE,
            "hidden_at",
            "load_hidden_sessions",
            &[],
            ids,
        )
    }
}

/// Read-modify-write a plain `HashMap<String, T>` sidecar file (the
/// per-session model / mode maps): applies `mutate` to the durable content
/// and persists only when it reports a change. Like
/// [`apply_timestamped_id_mutation`], this keeps entries written by other
/// processes after this one booted instead of reverting them with a stale
/// whole-map snapshot.
pub(crate) fn mutate_json_map_file<T, F>(file_name: &str, mutate: F) -> Result<()>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
    F: FnOnce(&mut HashMap<String, T>) -> bool,
{
    let file = crate::platform::paths::sessions_root().join(file_name);
    let mut entries: HashMap<String, T> = if file.exists() {
        let content =
            std::fs::read_to_string(&file).with_context(|| format!("read {file_name}"))?;
        serde_json::from_str(&content).with_context(|| format!("parse {file_name}"))?
    } else {
        HashMap::new()
    };
    if !mutate(&mut entries) {
        return Ok(());
    }
    if entries.is_empty() {
        return match std::fs::remove_file(&file) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(anyhow::Error::new(error).context(format!("remove {}", file.display())))
            }
        };
    }
    let json =
        serde_json::to_string_pretty(&entries).context(format!("serialize {file_name} failed"))?;
    crate::platform::filesystem::atomic_write(&file, json.as_bytes())
        .with_context(|| format!("persist {file_name} failed"))
}

/// Id-level mutation of `_session_models.json` (the per-session model
/// override map): upserts when `model_id` is `Some`, removes when `None`.
pub(crate) fn apply_session_model_mutation(id: &str, model_id: Option<&str>) -> Result<()> {
    mutate_json_map_file(SESSION_MODELS_FILE, |entries| match model_id {
        Some(mid) => entries.insert(id.to_string(), mid.to_string()).as_deref() != Some(mid),
        None => entries.remove(id).is_some(),
    })
    .context("persist per-session model bindings")
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
        if let Err(error) = apply_session_model_mutation(id, models.get(id).map(String::as_str)) {
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

    /// Whole-map save of the per-session model overrides. Only boot fixtures
    /// and tests should reach for this — production mutations go through
    /// [`Self::set_session_model_id`] / [`apply_session_model_mutation`] so
    /// entries persisted by another process after this one booted survive.
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
        let timestamp = Utc::now().to_rfc3339();
        {
            let mut pins = self.pinned_sessions.write();
            if pinned {
                pins.insert(id.to_string(), timestamp.clone());
            } else {
                pins.remove(id);
            }
        }
        let result = if pinned {
            apply_timestamped_id_mutation(
                PINNED_SESSIONS_FILE,
                "pinned_at",
                "load_pinned_sessions",
                &[(id, timestamp)],
                &[],
            )
        } else {
            apply_timestamped_id_mutation(
                PINNED_SESSIONS_FILE,
                "pinned_at",
                "load_pinned_sessions",
                &[],
                &[id],
            )
        };
        if let Err(error) = result {
            eprintln!("[sessions] persist pin state for {id} failed: {error:#}");
        }
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

    /// Durable pin protection for a retention sweep: the pin file is the
    /// cross-process truth, so the sweep re-reads it instead of trusting the
    /// map loaded at boot — a GUI pin made after this process started must
    /// still protect the session from this process's sweep. Any missing,
    /// unreadable, or unparseable file keeps the boot-time map: the save path
    /// deletes the file exactly when the map empties, and a torn read (an
    /// externally corrupted or legacy non-atomic file) must never widen the
    /// eviction set.
    pub(crate) fn durable_pinned_sessions(&self) -> std::collections::HashSet<String> {
        let file = crate::platform::paths::sessions_root().join(PINNED_SESSIONS_FILE);
        std::fs::read_to_string(&file)
            .ok()
            .and_then(|content| parse_pinned_sessions(&content))
            .map(|pins| pins.into_keys().collect())
            .unwrap_or_else(|| self.pinned_sessions.read().keys().cloned().collect())
    }

    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden_sessions.read().contains_key(id)
    }

    pub fn hidden_at(&self, id: &str) -> Option<String> {
        self.hidden_sessions.read().get(id).cloned()
    }

    pub fn set_hidden(&self, id: &str, hidden: bool) {
        let timestamp = Utc::now().to_rfc3339();
        {
            let mut hidden_sessions = self.hidden_sessions.write();
            if hidden {
                hidden_sessions.insert(id.to_string(), timestamp.clone());
            } else {
                hidden_sessions.remove(id);
            }
        }
        if hidden {
            self.set_pinned(id, false);
        }
        let result = if hidden {
            apply_timestamped_id_mutation(
                HIDDEN_SESSIONS_FILE,
                "hidden_at",
                "load_hidden_sessions",
                &[(id, timestamp)],
                &[],
            )
        } else {
            apply_timestamped_id_mutation(
                HIDDEN_SESSIONS_FILE,
                "hidden_at",
                "load_hidden_sessions",
                &[],
                &[id],
            )
        };
        if let Err(error) = result {
            eprintln!("[sessions] persist hidden state for {id} failed: {error:#}");
        }
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
}

/// Parse a `_pinned_sessions.json` payload: an array of bare session ids or
/// `{id, pinned_at}` objects. `None` on invalid JSON or a non-array payload
/// (callers keep their boot-time map instead of trusting a torn read).
fn parse_pinned_sessions(content: &str) -> Option<HashMap<String, String>> {
    let items = serde_json::from_str::<serde_json::Value>(content)
        .ok()?
        .as_array()?
        .to_vec();
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
    Some(pins)
}
