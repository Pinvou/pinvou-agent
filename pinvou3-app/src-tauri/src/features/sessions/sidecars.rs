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
    crate::platform::filesystem::atomic_write_private(file, json.as_bytes())
        .with_context(|| format!("persist {file_name} failed"))
}

/// Whole-map save of the pinned / hidden sidecars: writes exactly the given
/// map. Only boot fixtures and tests should reach for this — production
/// mutation paths must go through [`apply_timestamped_id_mutation_locked`], because
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
///
/// The `_locked` suffix is the caller contract shared with the other sidecar
/// mutators (see `multi_agent_flags_io`): the durable read-modify-write must
/// run under the store's per-file io mutex (`pinned_sessions_io` /
/// `hidden_sessions_io`), or two concurrent mutators' RMWs interleave and
/// the later write lands over the earlier one's change (a purge erasing a
/// fresh pin, or a pin resurrecting an evicted id).
fn apply_timestamped_id_mutation_locked(
    file_name: &str,
    ts_key: &str,
    label: &str,
    upserts: &[(&str, String)],
    removes: &[&str],
) -> Result<()> {
    let file = crate::platform::paths::sessions_root().join(file_name);
    let mut entries = load_timestamped_id_map_for_mutation(file_name, ts_key, label)?;
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
        let _io = self.pinned_sessions_io.lock();
        apply_timestamped_id_mutation_locked(
            PINNED_SESSIONS_FILE,
            "pinned_at",
            "load_pinned_sessions",
            &[],
            ids,
        )
    }

    /// Retention-purge half of [`Self::set_hidden`].
    pub(crate) fn purge_hidden_ids(&self, ids: &[&str]) -> Result<()> {
        let _io = self.hidden_sessions_io.lock();
        apply_timestamped_id_mutation_locked(
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
/// [`apply_timestamped_id_mutation_locked`], this keeps entries written by other
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
        serde_json::from_str(&content).map_err(|error| {
            // Quarantine-then-refuse (same contract as the timestamped-map
            // mutation loader): this mutation fails, the evidence survives,
            // and the next mutation starts from "absent" instead of staying
            // bricked by the corrupt bytes.
            let note = match crate::platform::filesystem::quarantine_corrupt_file(&file) {
                Ok(quarantine) => {
                    format!("; corrupt bytes quarantined at {}", quarantine.display())
                }
                Err(quarantine_error) => format!("; quarantining failed ({quarantine_error})"),
            };
            anyhow::Error::new(error).context(format!("parse {file_name}{note}"))
        })?
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
    crate::platform::filesystem::atomic_write_private(&file, json.as_bytes())
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

/// Batched remove half for `_session_models.json`: retention purges evict N
/// sessions at once, so the durable file is re-read and rewritten once for
/// the whole batch instead of once per id (mirroring the batched mode-map
/// mutation). The `_locked` suffix is the caller contract: the durable RMW
/// must run under the store's `session_models_io` mutex, or it interleaves
/// with `set_session_model_id`'s own RMW and the later write lands over the
/// earlier one's change.
pub(crate) fn remove_session_models_locked(ids: &[&str]) -> Result<()> {
    mutate_json_map_file::<String, _>(SESSION_MODELS_FILE, |entries| {
        let mut changed = false;
        for id in ids {
            changed |= entries.remove(*id).is_some();
        }
        changed
    })
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
    parse_timestamped_id_map(&content, ts_key, label)
}

/// Parse half of [`load_timestamped_id_map`], shared with the mutation-path
/// loader so both sides agree on the accepted shapes.
fn parse_timestamped_id_map(
    content: &str,
    ts_key: &str,
    label: &str,
) -> Option<HashMap<String, String>> {
    match serde_json::from_str::<serde_json::Value>(content) {
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
            if parsed.is_empty() {
                // A zero-id map — including the literal `[]` — is treated as
                // corrupt, not "no pins": reading it as empty would refuse
                // the mutation path's protection and silently widen the
                // retention eviction set on the sweep path. The save path
                // never writes `[]` (an empty map deletes the file), so a
                // `[]` on disk means a hand edit or a foreign writer; the
                // refusal quarantines it once and the next mutation rebuilds.
                eprintln!("[sessions] {label} failed: array carries no usable ids");
                return None;
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

/// Mutation-path loader for the pinned / hidden sidecars. Unlike
/// [`load_timestamped_id_map`] — which degrades a torn/unreadable file to
/// `None` so retention sweeps can fall back to the boot-time map — a mutation
/// must distinguish "file absent" (start empty) from "file present but
/// unreadable/corrupt" (refuse): proceeding from an empty map and saving
/// would durably destroy every entry the file still holds, and the next sweep
/// would then enforce the narrowed file.
fn load_timestamped_id_map_for_mutation(
    file_name: &str,
    ts_key: &str,
    label: &str,
) -> Result<HashMap<String, String>> {
    let file = crate::platform::paths::sessions_root().join(file_name);
    if !file.exists() {
        return Ok(HashMap::new());
    }
    let content =
        std::fs::read_to_string(&file).with_context(|| format!("read {file_name} for mutation"))?;
    parse_timestamped_id_map(&content, ts_key, label)
        .ok_or_else(|| quarantine_corrupt_sidecar(&file, file_name))
}

/// Quarantine a corrupt sidecar file aside (platform helper: sub-second-
/// unique name, bytes preserved verbatim) and return the refusal error. The
/// current mutation fails, but the next one starts from "absent" instead of
/// being bricked by the corrupt bytes until someone deletes the file by
/// hand — the same quarantine-then-rebuild contract as the marketplace
/// consent file, without silently destroying the evidence.
fn quarantine_corrupt_sidecar(file: &std::path::Path, label: &str) -> anyhow::Error {
    let quarantined = crate::platform::filesystem::quarantine_corrupt_file(file)
        .map(|path| format!("; the corrupt bytes are quarantined at {}", path.display()))
        .unwrap_or_else(|error| format!("; quarantining failed ({error})"));
    anyhow::anyhow!(
        "{label} is unreadable or corrupt; refusing the id-level mutation to \
         protect the surviving entries{quarantined}"
    )
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
        // Same io-mutex contract as the pin/hidden sidecars: the cache write
        // guard serializes this method against itself, but the retention
        // purge's batch removal RMWs the same file without this method's
        // cache guard — the file mutex is what keeps the two RMWs from
        // interleaving.
        let _io = self.session_models_io.lock();
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
        // One critical section for the cache switch, the durable id-level
        // RMW and the compensated rollback: without the io mutex two
        // concurrent mutators (or the retention purge) interleave their file
        // RMWs and the later write lands over the earlier one's change.
        let _io = self.pinned_sessions_io.lock();
        let timestamp = Utc::now().to_rfc3339();
        let previous = self.pinned_sessions.read().get(id).cloned();
        {
            let mut pins = self.pinned_sessions.write();
            if pinned {
                pins.insert(id.to_string(), timestamp.clone());
            } else {
                pins.remove(id);
            }
        }
        let result = if pinned {
            apply_timestamped_id_mutation_locked(
                PINNED_SESSIONS_FILE,
                "pinned_at",
                "load_pinned_sessions",
                &[(id, timestamp.clone())],
                &[],
            )
        } else {
            apply_timestamped_id_mutation_locked(
                PINNED_SESSIONS_FILE,
                "pinned_at",
                "load_pinned_sessions",
                &[],
                &[id],
            )
        };
        if let Err(error) = result {
            // Roll the in-memory cache back to the durable state: the file is
            // the cross-process truth and a refused/failed persist must not
            // leave this process's snapshot claiming a state the file does
            // not hold (same contract as `set_session_model_id`).
            let mut pins = self.pinned_sessions.write();
            match previous {
                Some(previous) => {
                    pins.insert(id.to_string(), previous);
                }
                None => {
                    pins.remove(id);
                }
            }
            // Session ids stay out of the log line: the failing sidecar
            // file, named in the error context, identifies the write.
            eprintln!("[sessions] persist pin state failed: {error:#}");
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
            .and_then(|content| {
                parse_timestamped_id_map(&content, "pinned_at", "load_pinned_sessions")
            })
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
        // Same single-critical-section contract as `set_pinned` (its own io
        // mutex; the nested `set_pinned` below takes the pin mutex, never the
        // reverse, so the ordering is acyclic).
        let _io = self.hidden_sessions_io.lock();
        let timestamp = Utc::now().to_rfc3339();
        let previous = self.hidden_sessions.read().get(id).cloned();
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
            apply_timestamped_id_mutation_locked(
                HIDDEN_SESSIONS_FILE,
                "hidden_at",
                "load_hidden_sessions",
                &[(id, timestamp.clone())],
                &[],
            )
        } else {
            apply_timestamped_id_mutation_locked(
                HIDDEN_SESSIONS_FILE,
                "hidden_at",
                "load_hidden_sessions",
                &[],
                &[id],
            )
        };
        if let Err(error) = result {
            // Roll the hidden-cache entry back (same durable-truth contract
            // as `set_pinned`). The pin-clearing side effect above is
            // intentional and kept: hiding is the user's stated direction,
            // and a session that failed to hide stays unpinned either way.
            let mut hidden_sessions = self.hidden_sessions.write();
            match previous {
                Some(previous) => {
                    hidden_sessions.insert(id.to_string(), previous);
                }
                None => {
                    hidden_sessions.remove(id);
                }
            }
            eprintln!("[sessions] persist hidden state failed: {error:#}");
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
