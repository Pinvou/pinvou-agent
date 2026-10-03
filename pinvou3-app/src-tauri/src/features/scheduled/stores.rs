//! 版本化 JSON store:3 个定时注册表的泛型持久化。
//!
//! Wave 1 1d 建立的 `VersionedRegistry` trait + `VersionedJsonStore<T>` 泛型,
//! 收敛 scheduled run read / model binding / UI metadata 三个同构 store。
//! 从 tasks.rs 抽离,通过 `use super::*` 复用 facade 的导入。

use std::path::Path;
use std::time::SystemTime;

use parking_lot::RwLock;

use super::*;
use crate::platform::filesystem::{FileIdentity, metadata_file_identity};

fn scheduled_run_read_state_schema_version() -> u32 {
    SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ScheduledRunReadRegistry {
    #[serde(default = "scheduled_run_read_state_schema_version")]
    pub(crate) schema_version: u32,
    #[serde(default)]
    pub(crate) viewed_runs: HashMap<String, HashSet<String>>,
}

impl Default for ScheduledRunReadRegistry {
    fn default() -> Self {
        Self {
            schema_version: SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION,
            viewed_runs: HashMap::new(),
        }
    }
}

fn scheduled_model_binding_schema_version() -> u32 {
    SCHEDULED_MODEL_BINDING_SCHEMA_VERSION
}

fn scheduled_task_kind_schema_version() -> u32 {
    SCHEDULED_TASK_KIND_SCHEMA_VERSION
}

fn scheduled_task_ui_metadata_schema_version() -> u32 {
    SCHEDULED_TASK_UI_METADATA_SCHEMA_VERSION
}

fn scheduled_history_archive_schema_version() -> u32 {
    SCHEDULED_HISTORY_ARCHIVE_SCHEMA_VERSION
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ScheduledTaskModelBinding {
    pub(crate) model_id: String,
    pub(crate) model: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ScheduledTaskModelBindingRegistry {
    #[serde(default = "scheduled_model_binding_schema_version")]
    pub(crate) schema_version: u32,
    #[serde(default)]
    pub(crate) tasks: HashMap<String, ScheduledTaskModelBinding>,
}

impl Default for ScheduledTaskModelBindingRegistry {
    fn default() -> Self {
        Self {
            schema_version: SCHEDULED_MODEL_BINDING_SCHEMA_VERSION,
            tasks: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ScheduledTaskKindEntry {
    pub(crate) kind: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ScheduledTaskKindRegistry {
    #[serde(default = "scheduled_task_kind_schema_version")]
    pub(crate) schema_version: u32,
    #[serde(default)]
    pub(crate) tasks: HashMap<String, ScheduledTaskKindEntry>,
}

impl Default for ScheduledTaskKindRegistry {
    fn default() -> Self {
        Self {
            schema_version: SCHEDULED_TASK_KIND_SCHEMA_VERSION,
            tasks: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ScheduledTaskUiMetadata {
    #[serde(default)]
    pub(crate) pinned: bool,
    #[serde(default)]
    pub(crate) pinned_at: Option<String>,
    pub(crate) updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ScheduledTaskUiMetadataRegistry {
    #[serde(default = "scheduled_task_ui_metadata_schema_version")]
    pub(crate) schema_version: u32,
    #[serde(default)]
    pub(crate) tasks: HashMap<String, ScheduledTaskUiMetadata>,
}

impl Default for ScheduledTaskUiMetadataRegistry {
    fn default() -> Self {
        Self {
            schema_version: SCHEDULED_TASK_UI_METADATA_SCHEMA_VERSION,
            tasks: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ArchivedScheduledTaskSnapshot {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
}

impl From<&AutomationRecord> for ArchivedScheduledTaskSnapshot {
    fn from(task: &AutomationRecord) -> Self {
        Self {
            id: task.id.clone(),
            name: task.name.clone(),
            model: task.model.clone(),
        }
    }
}

fn deserialize_archived_runs_lossy<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<AutomationRunRecord>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = <Vec<serde_json::Value> as serde::Deserialize>::deserialize(deserializer)?;
    let mut runs = Vec::with_capacity(values.len());
    for value in values {
        match serde_json::from_value(value) {
            Ok(run) => runs.push(run),
            Err(error) => {
                log::warn!("Ignoring invalid run in scheduled history archive: {error}");
            }
        }
    }
    Ok(runs)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ArchivedScheduledTask {
    pub(crate) task: ArchivedScheduledTaskSnapshot,
    #[serde(default, deserialize_with = "deserialize_archived_runs_lossy")]
    pub(crate) runs: Vec<AutomationRunRecord>,
    pub(crate) deleted_at: String,
}

fn archived_task_is_valid(key: &str, archived: &ArchivedScheduledTask) -> bool {
    !archived.task.id.trim().is_empty()
        && archived.task.id == key
        && archived
            .runs
            .iter()
            .all(|run| run.automation_id == archived.task.id)
}

fn deserialize_archived_tasks_lossy<'de, D>(
    deserializer: D,
) -> std::result::Result<HashMap<String, ArchivedScheduledTask>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values =
        <HashMap<String, serde_json::Value> as serde::Deserialize>::deserialize(deserializer)?;
    let mut tasks = HashMap::with_capacity(values.len());
    for (key, value) in values {
        match serde_json::from_value::<ArchivedScheduledTask>(value) {
            Ok(archived) if archived_task_is_valid(&key, &archived) => {
                tasks.insert(key, archived);
            }
            Ok(_) => {
                log::warn!(
                    "Ignoring inconsistent scheduled history archive entry for automation {key}"
                );
            }
            Err(error) => {
                log::warn!(
                    "Ignoring invalid scheduled history archive entry for automation {key}: {error}"
                );
            }
        }
    }
    Ok(tasks)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ScheduledHistoryArchiveRegistry {
    #[serde(default = "scheduled_history_archive_schema_version")]
    pub(crate) schema_version: u32,
    #[serde(default, deserialize_with = "deserialize_archived_tasks_lossy")]
    pub(crate) tasks: HashMap<String, ArchivedScheduledTask>,
}

impl Default for ScheduledHistoryArchiveRegistry {
    fn default() -> Self {
        Self {
            schema_version: SCHEDULED_HISTORY_ARCHIVE_SCHEMA_VERSION,
            tasks: HashMap::new(),
        }
    }
}

/// How [`VersionedJsonStore`] reacts to an unreadable / unsupported payload.
enum QuarantineStrategy {
    /// Emit a `warn!` and leave the offending file in place (UI metadata store).
    LogInPlace,
    /// Rename the file to `<name>.invalid-<ts>` and log (model-binding / read-state).
    Rename,
}

/// Per-store behaviour carried by the registry type itself.
///
/// The three scheduled registries share an identical open/persist skeleton but
/// differ in (a) schema version, (b) how an old version is migrated, (c) whether
/// invalid payloads are quarantined or merely logged, and (d) the human-readable
/// label/suffix used in diagnostics. This trait carries exactly those
/// differences so [`VersionedJsonStore<T>`] can stay generic without assuming
/// the three stores are textually identical.
pub(crate) trait VersionedRegistry:
    Default + serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static
{
    /// Schema version persisted in (and supported by) this registry.
    const SUPPORTED_VERSION: u32;
    /// Read back the schema version of a deserialised instance.
    fn schema_version(&self) -> u32;
    /// Migrate an older-version instance up to [`SUPPORTED_VERSION`].
    ///
    /// Infallible: every concrete store always produces a replacement (the
    /// read-state store deliberately resets to default, dropping viewed runs).
    fn migrate(self) -> Self;
    /// Quarantine policy for newer-than-supported / invalid-JSON payloads.
    const QUARANTINE: QuarantineStrategy;
    /// Human-readable label used in log/error messages for this store.
    const LABEL: &'static str;
    /// Fallback file-name stem when the on-disk path has no file component.
    const QUARANTINE_FALLBACK_NAME: &'static str;
    /// Store-specific suffix appended to quarantine / read-failure warnings.
    const WARN_SUFFIX: &'static str;
}

/// Versioned-JSON registry store with schema migration, quarantine, and atomic
/// writes. Collapses the three previously hand-rolled stores into one generic
/// core; per-store differences live on [`VersionedRegistry`].
#[derive(Clone)]
pub(crate) struct VersionedJsonStore<T: VersionedRegistry> {
    pub(crate) path: Arc<PathBuf>,
    pub(crate) registry: Arc<RwLock<T>>,
    /// Identity of the file contents this handle last read or wrote, used by
    /// [`Self::reload_if_changed`] to skip a re-read that cannot teach it
    /// anything. `None` means "unknown", which always forces a read;
    /// `Some(ABSENT)` prices in a confirmed absence (see the reload failure
    /// path), so repeated lookups cost one stat instead of a failed read.
    seen: Arc<RwLock<Option<FileStamp>>>,
    /// Set when a read QUARANTINED (renamed aside) this store's file under
    /// the Rename strategy: the canonical path is now absent while this
    /// handle's memory may be the only healthy copy left. A later absent-file
    /// read must then answer "keep memory", not "empty registry" — otherwise
    /// the next reload after a quarantine would install `T::default()` over
    /// the healthy registry and the next persist would write the emptying
    /// through. Cleared by a successful persist (the file is healthy again)
    /// and by a successful read of a present file.
    quarantined: Arc<std::sync::atomic::AtomicBool>,
}

/// Cheap change detector for the registry file: a `stat` is orders of
/// magnitude cheaper than read + parse + lock swap, and every writer of these
/// files (this handle and the `pinvou` CLI) goes through an atomic
/// write-and-rename, so a new payload always lands as a new inode with a
/// fresh mtime. On Unix the stamp carries that file identity: a foreign write
/// can preserve the byte length (same-shape payload) and, on coarse-mtime
/// filesystems, the timestamp, and only `dev`/`ino` tells the two files
/// apart, so it must take part in the equality check. Non-Unix keeps the
/// plain len+mtime behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    /// File identity: an atomic rename always replaces the inode, so equal
    /// len+mtime on a different inode is still a different write. `None` on
    /// platforms without a portable identity — those compare len+mtime only
    /// (the `cfg` lives in `platform::filesystem`, keeping this file
    /// unconditionally compiled).
    identity: Option<FileIdentity>,
}

impl FileStamp {
    /// Reserved sentinel for "this handle has priced in the file's absence":
    /// no real file can have `u64::MAX` length, so equality against a fresh
    /// `FileStamp::of` (which answers `None` for an absent path) can never
    /// be confused with a real stamp.
    const ABSENT: Self = Self {
        len: u64::MAX,
        modified: None,
        identity: None,
    };

    /// `None` when the file cannot be stat'ed at all (missing, or a metadata
    /// error) — treated as "unknown", never as "unchanged".
    fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            identity: metadata_file_identity(&meta),
        })
    }
}

/// The stamp to record after this handle wrote `payload` to `path`, or `None`
/// when the file on disk cannot be proven to carry it.
///
/// The proof cannot come from the stamp alone: a foreign write landing
/// between our `crate::platform::filesystem::atomic_write_private` and the stat replaces the
/// path with its own inode, and a same-length foreign payload is
/// indistinguishable from ours by length (and, within one mtime tick, by
/// timestamp) — recording it as "ours" would freeze this handle's stale
/// memory until the file changed again, which is exactly the state
/// `reload_if_changed` exists to repair. Reading the bytes back closes that
/// window: only a file whose content is our payload is recorded, and the
/// recorded stamp carries the file identity, so a same-length foreign write
/// is detected as changed by the next check and re-read. Any doubt — failed
/// stat, failed read, different bytes — records "unknown", which forces the
/// re-read that merges the foreign payload in instead of dropping it.
fn stamp_of_our_write(path: &Path, payload: &[u8]) -> Option<FileStamp> {
    let stamp = FileStamp::of(path)?;
    if stamp.len != payload.len() as u64 {
        // Different length: definitely not our payload, no read needed.
        return None;
    }
    (std::fs::read(path).ok().as_deref() == Some(payload)).then_some(stamp)
}

/// Outcome of one disk read of a store's file.
enum DiskRead<T> {
    /// A usable payload as-is (possibly the default because the file is
    /// absent — a missing sidecar is an empty registry, not a failure).
    Loaded(T),
    /// Usable after an on-read migration; the caller owns persisting the
    /// migrated form back.
    Migrated(T),
    /// Unusable (I/O error, invalid JSON, or a newer-than-supported schema).
    /// [`VersionedJsonStore::open`] fails open to an empty registry there
    /// (existing startup behaviour), while [`VersionedJsonStore::reload`]
    /// keeps its previous state so the next check retries once the file is
    /// repaired. `quarantined`: the unusable payload was RENAMED aside, so
    /// the canonical path is absent after this read — a later absent-file
    /// read must not read as an empty registry while memory may hold the
    /// only healthy copy.
    Failed { quarantined: bool },
}

impl<T: VersionedRegistry> VersionedJsonStore<T> {
    /// Read, parse and migrate the payload at `path`, applying this store's
    /// quarantine policy to an unusable one, and report whether the payload
    /// was migrated on read (the caller owns writing the migrated form back).
    ///
    /// Extracted so [`Self::open`] and [`Self::reload`] cannot drift: the
    /// reload path exists precisely because a foreign process may have
    /// rewritten the file, so it must honour the same version, migration and
    /// quarantine rules the initial read applies.
    fn read_from_disk(
        path: &Path,
        quarantined_flag: &std::sync::atomic::AtomicBool,
        had_seen_file: bool,
    ) -> DiskRead<T> {
        use std::sync::atomic::Ordering as AtomicOrdering;
        match std::fs::read_to_string(path) {
            Ok(raw) => match serde_json::from_str::<T>(&raw) {
                Ok(registry) if registry.schema_version() == T::SUPPORTED_VERSION => {
                    quarantined_flag.store(false, AtomicOrdering::Release);
                    DiskRead::Loaded(registry)
                }
                Ok(registry) if registry.schema_version() < T::SUPPORTED_VERSION => {
                    quarantined_flag.store(false, AtomicOrdering::Release);
                    DiskRead::Migrated(registry.migrate())
                }
                Ok(registry) => {
                    // Raise the flag BEFORE the rename inside handle_invalid:
                    // a concurrent same-handle reload whose read_to_string
                    // observes NotFound after the rename but samples the
                    // flag before this store would otherwise answer
                    // `Loaded(default)` and install it over healthy memory.
                    // Not cleared on a failed rename — the file either went
                    // away (a sibling quarantine's rename won the race, flag
                    // true is then the fact) or is still there (the next
                    // successful read clears the flag); both directions keep
                    // memory, which is the safe side.
                    if matches!(T::QUARANTINE, QuarantineStrategy::Rename) {
                        quarantined_flag.store(true, AtomicOrdering::Release);
                    }
                    let quarantined = Self::handle_invalid(
                        path,
                        &format!(
                            "schema v{} is newer than supported v{}",
                            registry.schema_version(),
                            T::SUPPORTED_VERSION
                        ),
                    );
                    DiskRead::Failed { quarantined }
                }
                Err(error) => {
                    // Same pre-rename flag discipline as the schema arm above.
                    if matches!(T::QUARANTINE, QuarantineStrategy::Rename) {
                        quarantined_flag.store(true, AtomicOrdering::Release);
                    }
                    let quarantined = Self::handle_invalid(path, &format!("invalid JSON: {error}"));
                    DiskRead::Failed { quarantined }
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // An absent file is an empty registry EXCEPT when this handle
                // has proven the file real before: a previous read that
                // quarantined it away (the flag), or a foreign process
                // removing/quarantining it after this handle loaded it (the
                // CLI quarantines a corrupt sidecar on EVERY command,
                // read-only ones included). In both cases memory may be the
                // only healthy copy, and installing the default over it
                // (and writing that through on the next persist) would
                // silently empty the store. Keep memory instead; the next
                // mutator's persist rewrites the healthy registry and heals
                // the file (which also clears the flag).
                if quarantined_flag.load(AtomicOrdering::Acquire) || had_seen_file {
                    DiskRead::Failed { quarantined: true }
                } else {
                    DiskRead::Loaded(T::default())
                }
            }
            Err(error) => {
                log::warn!(
                    "Unable to read {} {}: {error}{}",
                    T::LABEL,
                    path.display(),
                    T::WARN_SUFFIX
                );
                DiskRead::Failed { quarantined: false }
            }
        }
    }

    pub(crate) fn open(path: PathBuf) -> Result<Self> {
        // Stat before read, matching `reload`: a foreign write landing between
        // the two leaves the stamp older than memory, which at worst costs one
        // extra read later — never the reverse (memory stale under a stamp
        // that matches).
        let stamp = FileStamp::of(&path);
        let quarantined_flag = std::sync::atomic::AtomicBool::new(false);
        // Startup has not seen a file yet, so an absent file legitimately
        // boots the empty default (fail-open); only established handles keep
        // memory over a confirmed absence.
        let (registry, migrated) = match Self::read_from_disk(&path, &quarantined_flag, false) {
            // Fail open to an empty registry as before: existing startup
            // behaviour kept (there is no previous state to preserve here).
            DiskRead::Loaded(registry) => (registry, false),
            DiskRead::Migrated(registry) => (registry, true),
            DiskRead::Failed { .. } => (T::default(), false),
        };
        let store = Self {
            path: Arc::new(path),
            registry: Arc::new(RwLock::new(registry)),
            seen: Arc::new(RwLock::new(stamp)),
            quarantined: Arc::new(std::sync::atomic::AtomicBool::new(
                quarantined_flag.load(std::sync::atomic::Ordering::Acquire),
            )),
        };
        if migrated {
            // persist() refreshes `seen` from the file it just wrote; on a
            // failed write the pre-read stamp stays, which only forces a
            // (cheap) re-read later.
            store.persist_migrated();
        }
        Ok(store)
    }

    /// Handle over an in-memory registry that was never read from `path`, for
    /// tests that plant a deliberately unwritable path. `seen` stays unknown so
    /// any later read is forced rather than skipped as unchanged.
    #[cfg(test)]
    pub(crate) fn from_registry(path: PathBuf, registry: T) -> Self {
        Self {
            path: Arc::new(path),
            registry: Arc::new(RwLock::new(registry)),
            seen: Arc::new(RwLock::new(None)),
            quarantined: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Re-read the on-disk registry and swap it into the in-memory copy.
    ///
    /// Disk — not memory — is the authority: every mutation here persists while
    /// holding the write lock (and rolls back when the write fails), so the
    /// file is never behind this handle, while a *foreign* process (the
    /// `pinvou` CLI writes the same sidecars) can put it ahead.
    pub(crate) fn reload(&self) {
        self.reload_with_gap(|| ());
    }

    /// [`Self::reload`] with a seam between the disk read and the locked
    /// swap, so a test can interleave this handle's own mutator commit in
    /// the exact window the lock swap must not lose.
    fn reload_with_gap(&self, gap: impl FnOnce()) {
        // Stat before read: if a foreign write lands between the two, the
        // stamp ends up older than memory, which at worst costs this handle
        // one extra read later. Stating after the read produced the reverse —
        // memory behind the file while the stamp matched — which made
        // `reload_if_changed` skip the re-read that would have fixed it.
        let stamp = FileStamp::of(self.path.as_ref());
        let had_seen_file = self.seen.read().is_some();
        let (registry, migrated) =
            match Self::read_from_disk(self.path.as_ref(), &self.quarantined, had_seen_file) {
                DiskRead::Loaded(registry) => (registry, false),
                DiskRead::Migrated(registry) => (registry, true),
                // Unusable payload: keep the previous in-memory registry and do
                // NOT update the stamp. Swapping in `T::default()` here (the
                // pre-fix behaviour) resolved every miss to a plain chat task and
                // recorded the stamp so the damage never healed; with the stamp
                // unchanged, a later check re-reads once the file is repaired.
                DiskRead::Failed { quarantined } => {
                    if quarantined {
                        // The canonical file was renamed away: until a persist
                        // (or a repaired external write) makes the path exist
                        // again, an absent-file read must keep this memory, not
                        // install the empty default over it.
                        self.quarantined
                            .store(true, std::sync::atomic::Ordering::Release);
                        // Price the confirmed absence in: repeated lookups then
                        // cost one stat (answered by the ABSENT sentinel in
                        // `reload_if_changed`) instead of a failed read-and-swap
                        // each. A foreign write that recreates the file yields a
                        // real stamp, which never equals the sentinel.
                        if FileStamp::of(self.path.as_ref()).is_none() {
                            *self.seen.write() = Some(FileStamp::ABSENT);
                        }
                    }
                    return;
                }
            };
        gap();
        {
            // The stat / read / swap above is not atomic against this
            // handle's own mutators. A mutator whose write lock is taken
            // after our read can already have applied its change in memory,
            // persisted it, and advanced `seen` — installing the pre-read
            // payload over that would regress the registry to the stale
            // file AND set `seen` back to a stamp the file has moved past,
            // the exact lost-update this store's reload discipline exists
            // to prevent. So the swap re-stats under the write lock and
            // installs only a payload the file still provably carries: a
            // file identical to the pre-read stamp means no mutator (or
            // foreign writer) committed while we read. Anything else drops
            // the read — a mutator has already advanced memory and `seen`,
            // and the foreign-writer case pays one redundant re-read on the
            // next check, never a skipped one.
            //
            // Scoped short hold: persist_migrated() below takes a read
            // lock, and parking_lot locks are not reentrant.
            let mut state = self.registry.write();
            if FileStamp::of(self.path.as_ref()) != stamp {
                return;
            }
            *state = registry;
            *self.seen.write() = stamp;
            self.quarantined
                .store(false, std::sync::atomic::Ordering::Release);
        }
        if migrated {
            self.persist_migrated();
        }
    }

    /// [`Self::reload`], but only when the file looks different from the one
    /// this handle last saw.
    ///
    /// Lookup misses are the common case, not the rare one — every ordinary
    /// chat task misses the kind registry — so an unconditional reload would
    /// turn a task listing into one read-and-parse per row. A `stat` per miss
    /// keeps the foreign-writer guarantee at a fraction of the cost. The
    /// comparison fails open: an unreadable stamp on either side forces the
    /// read, so the worst case is the behaviour we would have had anyway.
    pub(crate) fn reload_if_changed(&self) {
        let current = FileStamp::of(self.path.as_ref());
        let seen = *self.seen.read();
        let unchanged = match (current, seen) {
            (Some(current), Some(seen)) => current == seen,
            // An absence this handle has already priced in (its own or a
            // foreign quarantine kept memory over it): a re-read cannot
            // teach it anything, and the sentinel answers without the
            // failed read the old shape paid on every miss.
            (None, Some(seen)) => seen == FileStamp::ABSENT,
            _ => false,
        };
        if unchanged {
            return;
        }
        self.reload();
    }

    /// Best-effort write-back of a registry that was migrated on read; a
    /// failure only means the migration is redone next time, so it warns.
    fn persist_migrated(&self) {
        if let Err(error) = self.persist(&self.registry.read()) {
            log::warn!(
                "Unable to persist migrated {} {}: {error:#}",
                T::LABEL,
                self.path.display()
            );
        }
    }

    pub(crate) fn persist(&self, registry: &T) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {} dir {}", T::LABEL, parent.display()))?;
        }
        // Foreign-write collision guard: `seen` is the stamp this handle
        // observed at its last reload or persist, and `registry` was mutated
        // on top of that snapshot. Every mutator reloads first, so a stamp
        // mismatch here means a foreign writer (the CLI or a second app
        // instance) landed between that reload and this persist — renaming
        // this whole-file payload now would silently destroy the foreign
        // write, the exact lost update the reload discipline otherwise
        // prevents. Refuse instead: the mutator's rollback keeps memory
        // consistent, the error tells the caller to retry, and the retry's
        // reload merges the foreign content before re-applying. (The
        // Windows non-identity corner stays as disclosed: len+mtime stamps
        // can alias a same-length same-tick write.)
        let current = FileStamp::of(self.path.as_ref());
        let seen = *self.seen.read();
        let unchanged = match (current, seen) {
            (Some(current), Some(seen)) => current == seen,
            // Absence priced in explicitly (post-quarantine keep) or never
            // seen at all (fresh open on a not-yet-created file): this
            // handle may create. A file that APPEARED in the window is the
            // (Some, _) arm below and still refuses.
            (None, Some(seen)) => seen == FileStamp::ABSENT,
            (None, None) => true,
            _ => false,
        };
        if !unchanged {
            return Err(anyhow::anyhow!(
                "{} changed on disk after this handle last read it (a concurrent \
                 writer landed between the reload and this persist); refusing to \
                 overwrite it — retry the operation to re-apply it on the merged state",
                T::LABEL
            ));
        }
        let payload = serde_json::to_vec_pretty(registry)
            .with_context(|| format!("serialize {}", T::LABEL))?;
        crate::platform::filesystem::atomic_write_private(self.path.as_ref(), &payload)
            .with_context(|| format!("write {} {}", T::LABEL, self.path.display()))?;
        // Record what we just wrote so `reload_if_changed` does not mistake
        // this handle's own write for a foreign one and re-read it — and so
        // a foreign write that DID land in the window is never recorded in
        // its place. See `stamp_of_our_write`: only a file proven (by
        // content) to carry our payload is recorded, identity included;
        // anything else stays "unknown" and forces the next check to
        // re-read, which merges the foreign payload in — never drops it.
        *self.seen.write() = stamp_of_our_write(self.path.as_ref(), &payload);
        self.quarantined
            .store(false, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    /// Apply this store's quarantine policy to an invalid payload at `path`.
    /// Applies this store's quarantine policy and reports whether the
    /// canonical file was renamed AWAY (Rename strategy, rename succeeded) —
    /// the fact the absent-file read arm needs to keep memory over an empty
    /// default.
    pub(crate) fn handle_invalid(path: &Path, reason: &str) -> bool {
        match T::QUARANTINE {
            QuarantineStrategy::LogInPlace => {
                log::warn!(
                    "Ignoring invalid {} {} ({reason})",
                    T::LABEL,
                    path.display()
                );
                false
            }
            QuarantineStrategy::Rename => {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let file_name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(T::QUARANTINE_FALLBACK_NAME);
                let quarantine_path =
                    path.with_file_name(format!("{file_name}.invalid-{timestamp}"));
                match std::fs::rename(path, &quarantine_path) {
                    Ok(()) => {
                        log::warn!(
                            "Quarantined {} {} to {} ({reason}){}",
                            T::LABEL,
                            path.display(),
                            quarantine_path.display(),
                            T::WARN_SUFFIX
                        );
                        true
                    }
                    Err(error) => {
                        log::warn!(
                            "Invalid {} {} ({reason}) could not be quarantined: {error}{}",
                            T::LABEL,
                            path.display(),
                            T::WARN_SUFFIX
                        );
                        false
                    }
                }
            }
        }
    }
}

/// Registries whose state is a per-automation `tasks` map keyed by automation id
/// (UI metadata / task kinds / model bindings / history archive). Carries the
/// map access needed by the shared remove / compact / rollback helpers below so
/// the four stores cannot drift apart again.
pub(crate) trait ScheduledTasksMapRegistry {
    type Entry: Clone;

    fn tasks_map(&mut self) -> &mut HashMap<String, Self::Entry>;

    /// Restore the previous entry after a failed persist: re-insert the old
    /// value, or remove the key when there was none. Shared by set_pinned /
    /// set_kind / set / archive_task / restore_task rollback tails.
    fn restore_entry(
        map: &mut HashMap<String, Self::Entry>,
        automation_id: &str,
        previous: Option<Self::Entry>,
    ) {
        match previous {
            Some(entry) => {
                map.insert(automation_id.to_string(), entry);
            }
            None => {
                map.remove(automation_id);
            }
        }
    }
}

impl<T> VersionedJsonStore<T>
where
    T: VersionedRegistry + ScheduledTasksMapRegistry,
{
    /// Remove one automation's entry; a missing key is a no-op (idempotent
    /// delete), and a failed persist rolls the in-memory map back so it never
    /// diverges from disk.
    ///
    /// The sidecar is co-owned by the `pinvou` CLI, which rewrites the whole
    /// file on its own writes; `remove` must therefore merge with whatever the
    /// foreign process left on disk before persisting, or its own write-back
    /// quietly drops the foreign entries.
    pub(crate) fn remove(&self, automation_id: &str) -> Result<()> {
        self.reload_if_changed();
        let mut registry = self.registry.write();
        let Some(previous) = registry.tasks_map().remove(automation_id) else {
            return Ok(());
        };
        if let Err(error) = self.persist(&registry) {
            registry
                .tasks_map()
                .insert(automation_id.to_string(), previous);
            return Err(error);
        }
        Ok(())
    }

    /// Keep only the given automation ids; unchanged maps skip the disk write,
    /// and a failed persist restores the pre-compact snapshot.
    ///
    /// The GUI task-list poll fires this every few seconds against a sidecar
    /// the `pinvou` CLI also rewrites; reloading only when the file's stamp
    /// moved keeps the poll a `stat` when nothing changed, while a CLI write
    /// in the window is merged into the compacted map instead of being
    /// overwritten by this handle's stale copy.
    pub(crate) fn compact(&self, automation_ids: &HashSet<String>) -> Result<()>
    where
        T::Entry: PartialEq,
    {
        self.reload_if_changed();
        let mut registry = self.registry.write();
        let before = registry.tasks_map().clone();
        registry
            .tasks_map()
            .retain(|id, _| automation_ids.contains(id));
        if *registry.tasks_map() == before {
            return Ok(());
        }
        if let Err(error) = self.persist(&registry) {
            *registry.tasks_map() = before;
            return Err(error);
        }
        Ok(())
    }
}

impl VersionedRegistry for ScheduledTaskUiMetadataRegistry {
    const SUPPORTED_VERSION: u32 = SCHEDULED_TASK_UI_METADATA_SCHEMA_VERSION;
    const QUARANTINE: QuarantineStrategy = QuarantineStrategy::LogInPlace;
    const LABEL: &'static str = "scheduled task UI metadata";
    const QUARANTINE_FALLBACK_NAME: &'static str = "scheduled-task-ui-metadata.json";
    const WARN_SUFFIX: &'static str = "";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn migrate(self) -> Self {
        Self {
            schema_version: SCHEDULED_TASK_UI_METADATA_SCHEMA_VERSION,
            tasks: self.tasks,
        }
    }
}

impl ScheduledTasksMapRegistry for ScheduledTaskUiMetadataRegistry {
    type Entry = ScheduledTaskUiMetadata;

    fn tasks_map(&mut self) -> &mut HashMap<String, Self::Entry> {
        &mut self.tasks
    }
}

impl VersionedRegistry for ScheduledHistoryArchiveRegistry {
    const SUPPORTED_VERSION: u32 = SCHEDULED_HISTORY_ARCHIVE_SCHEMA_VERSION;
    const QUARANTINE: QuarantineStrategy = QuarantineStrategy::Rename;
    const LABEL: &'static str = "scheduled history archive";
    const QUARANTINE_FALLBACK_NAME: &'static str = "history-archive.json";
    const WARN_SUFFIX: &'static str = "; deleted-task run history may be unavailable";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn migrate(self) -> Self {
        Self {
            schema_version: SCHEDULED_HISTORY_ARCHIVE_SCHEMA_VERSION,
            tasks: self.tasks,
        }
    }
}

impl VersionedRegistry for ScheduledTaskKindRegistry {
    const SUPPORTED_VERSION: u32 = SCHEDULED_TASK_KIND_SCHEMA_VERSION;
    const QUARANTINE: QuarantineStrategy = QuarantineStrategy::Rename;
    const LABEL: &'static str = "scheduled task kind";
    const QUARANTINE_FALLBACK_NAME: &'static str = "task-kinds.json";
    const WARN_SUFFIX: &'static str = "; scheduled tasks will run as ordinary chat tasks";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn migrate(self) -> Self {
        Self {
            schema_version: SCHEDULED_TASK_KIND_SCHEMA_VERSION,
            tasks: self.tasks,
        }
    }
}

impl ScheduledTasksMapRegistry for ScheduledTaskKindRegistry {
    type Entry = ScheduledTaskKindEntry;

    fn tasks_map(&mut self) -> &mut HashMap<String, Self::Entry> {
        &mut self.tasks
    }
}

impl VersionedRegistry for ScheduledTaskModelBindingRegistry {
    const SUPPORTED_VERSION: u32 = SCHEDULED_MODEL_BINDING_SCHEMA_VERSION;
    const QUARANTINE: QuarantineStrategy = QuarantineStrategy::Rename;
    const LABEL: &'static str = "scheduled model binding state";
    const QUARANTINE_FALLBACK_NAME: &'static str = "model-bindings.json";
    const WARN_SUFFIX: &'static str = "; scheduled tasks will fall back to wire model names";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn migrate(self) -> Self {
        Self {
            schema_version: SCHEDULED_MODEL_BINDING_SCHEMA_VERSION,
            tasks: self.tasks,
        }
    }
}

impl VersionedRegistry for ScheduledRunReadRegistry {
    const SUPPORTED_VERSION: u32 = SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION;
    const QUARANTINE: QuarantineStrategy = QuarantineStrategy::Rename;
    const LABEL: &'static str = "scheduled run read state";
    const QUARANTINE_FALLBACK_NAME: &'static str = "scheduled-run-read-state.json";
    const WARN_SUFFIX: &'static str = "; treating all runs as unread";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Older read-state schemas are dropped: viewed-run tracking resets on
    /// upgrade, matching the legacy hand-rolled store.
    fn migrate(self) -> Self {
        Self::default()
    }
}

impl ScheduledTasksMapRegistry for ScheduledTaskModelBindingRegistry {
    type Entry = ScheduledTaskModelBinding;

    fn tasks_map(&mut self) -> &mut HashMap<String, Self::Entry> {
        &mut self.tasks
    }
}

impl ScheduledTasksMapRegistry for ScheduledHistoryArchiveRegistry {
    type Entry = ArchivedScheduledTask;

    fn tasks_map(&mut self) -> &mut HashMap<String, Self::Entry> {
        &mut self.tasks
    }
}

pub(crate) type ScheduledTaskUiMetadataStore = VersionedJsonStore<ScheduledTaskUiMetadataRegistry>;

pub(crate) type ScheduledHistoryArchiveStore = VersionedJsonStore<ScheduledHistoryArchiveRegistry>;

impl VersionedJsonStore<ScheduledHistoryArchiveRegistry> {
    pub(crate) fn archive_task(
        &self,
        task: ArchivedScheduledTaskSnapshot,
        runs: Vec<AutomationRunRecord>,
    ) -> Result<()> {
        if task.id.trim().is_empty() {
            bail!("scheduled automation id cannot be empty");
        }
        if let Some(run) = runs.iter().find(|run| run.automation_id != task.id) {
            bail!(
                "scheduled run {} belongs to automation {}, not {}",
                run.id,
                run.automation_id,
                task.id
            );
        }
        let automation_id = task.id.clone();
        self.reload_if_changed();
        let mut registry = self.registry.write();
        let previous = registry.tasks.get(&automation_id).cloned();
        registry.tasks.insert(
            automation_id.clone(),
            ArchivedScheduledTask {
                task,
                runs,
                deleted_at: chrono::Utc::now().to_rfc3339(),
            },
        );
        if let Err(error) = self.persist(&registry) {
            <ScheduledHistoryArchiveRegistry as ScheduledTasksMapRegistry>::restore_entry(
                &mut registry.tasks,
                &automation_id,
                previous,
            );
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn archived_tasks(&self) -> Vec<ArchivedScheduledTask> {
        // Same stamp-gated read as the other cross-surface paths: a CLI
        // `scheduled delete` archives runs into this file under its own
        // lock, and without the reload a GUI sidebar would keep showing the
        // pre-archive list until an unrelated GUI mutation happened to
        // reload. Unchanged is one `stat`.
        self.reload_if_changed();
        self.registry.read().tasks.values().cloned().collect()
    }

    pub(crate) fn runs_for(&self, automation_id: &str) -> Option<Vec<AutomationRunRecord>> {
        self.reload_if_changed();
        self.registry
            .read()
            .tasks
            .get(automation_id)
            .map(|archived| archived.runs.clone())
    }

    pub(crate) fn find_run(
        &self,
        automation_id: &str,
        session_id: &str,
    ) -> Option<AutomationRunRecord> {
        self.reload_if_changed();
        self.registry
            .read()
            .tasks
            .get(automation_id)
            .and_then(|archived| {
                archived
                    .runs
                    .iter()
                    .find(|run| run.thread_id.as_deref() == Some(session_id))
                    .cloned()
            })
    }

    pub(crate) fn remove_run(
        &self,
        automation_id: &str,
        run_id: &str,
    ) -> Result<Option<RemovedArchivedRun>> {
        self.reload_if_changed();
        let mut registry = self.registry.write();
        let Some(previous) = registry.tasks.get(automation_id).cloned() else {
            return Ok(None);
        };
        let mut updated = previous.clone();
        updated.runs.retain(|run| run.id != run_id);
        if updated.runs.len() == previous.runs.len() {
            return Ok(None);
        }
        let remaining = updated.runs.clone();
        if remaining.is_empty() {
            registry.tasks.remove(automation_id);
        } else {
            registry.tasks.insert(automation_id.to_string(), updated);
        }
        if let Err(error) = self.persist(&registry) {
            <ScheduledHistoryArchiveRegistry as ScheduledTasksMapRegistry>::restore_entry(
                &mut registry.tasks,
                automation_id,
                Some(previous),
            );
            return Err(error);
        }
        Ok(Some(RemovedArchivedRun {
            archived_task: previous,
            remaining_runs: remaining,
        }))
    }

    pub(crate) fn restore_task(&self, archived: ArchivedScheduledTask) -> Result<()> {
        let automation_id = archived.task.id.clone();
        if !archived_task_is_valid(&automation_id, &archived) {
            bail!("invalid scheduled history archive entry for {automation_id}");
        }
        self.reload_if_changed();
        let mut registry = self.registry.write();
        let previous = registry.tasks.insert(automation_id.clone(), archived);
        if let Err(error) = self.persist(&registry) {
            <ScheduledHistoryArchiveRegistry as ScheduledTasksMapRegistry>::restore_entry(
                &mut registry.tasks,
                &automation_id,
                previous,
            );
            return Err(error);
        }
        Ok(())
    }
}

pub(crate) struct RemovedArchivedRun {
    pub(crate) archived_task: ArchivedScheduledTask,
    pub(crate) remaining_runs: Vec<AutomationRunRecord>,
}

impl VersionedJsonStore<ScheduledTaskUiMetadataRegistry> {
    pub(crate) fn metadata_for(&self, automation_id: &str) -> (bool, Option<String>) {
        self.registry
            .read()
            .tasks
            .get(automation_id)
            .filter(|metadata| metadata.pinned)
            .map(|metadata| (true, metadata.pinned_at.clone()))
            .unwrap_or((false, None))
    }

    pub(crate) fn set_pinned(&self, automation_id: &str, pinned: bool) -> Result<()> {
        if automation_id.trim().is_empty() {
            bail!("scheduled automation id cannot be empty");
        }
        let now = chrono::Utc::now().to_rfc3339();
        self.reload_if_changed();
        let mut registry = self.registry.write();
        let previous = registry.tasks.get(automation_id).cloned();
        if pinned {
            registry.tasks.insert(
                automation_id.to_string(),
                ScheduledTaskUiMetadata {
                    pinned: true,
                    pinned_at: Some(now.clone()),
                    updated_at: now,
                },
            );
        } else {
            registry.tasks.remove(automation_id);
        }
        if let Err(error) = self.persist(&registry) {
            <ScheduledTaskUiMetadataRegistry as ScheduledTasksMapRegistry>::restore_entry(
                &mut registry.tasks,
                automation_id,
                previous,
            );
            return Err(error);
        }
        Ok(())
    }
}

pub(crate) type ScheduledTaskKindStore = VersionedJsonStore<ScheduledTaskKindRegistry>;

/// Executor-facing lookup result for one automation's stored kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScheduledTaskKindLookup {
    /// No kind entry: an ordinary chat task (also the default for tasks that
    /// predate the kind sidecar).
    Chat,
    /// Created as a memory-organize task; the executor runs it app-side.
    MemoryOrganize,
    /// An entry exists but its value is not a kind this build supports
    /// (hand-edited sidecar, or a task written by a different app version).
    /// The executor must fail such a run instead of degrading it to a chat
    /// task: the stored prompt was authored for its kind, and running it as an
    /// unattended full-permission agent conversation is the unsafe direction.
    Unsupported(String),
}

impl VersionedJsonStore<ScheduledTaskKindRegistry> {
    /// Reads the task kind for DTO display. Only `memory_organize` is a
    /// supported kind for now; any other value left in the file surfaces as
    /// None (an ordinary chat task), mirroring the creation-side allow-list.
    /// The executor uses [`Self::kind_lookup_for`] instead, which distinguishes
    /// an unsupported value from no entry at all.
    pub(crate) fn kind_for(&self, automation_id: &str) -> Option<String> {
        match self.kind_lookup_for(automation_id) {
            ScheduledTaskKindLookup::MemoryOrganize => {
                Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE.to_string())
            }
            ScheduledTaskKindLookup::Chat | ScheduledTaskKindLookup::Unsupported(_) => None,
        }
    }

    /// Executor-facing tri-state lookup; see [`ScheduledTaskKindLookup`].
    ///
    /// A miss consults the disk before it answers `Chat`. This registry is read
    /// once at `open()`, but the same file is co-owned by a foreign process:
    /// `pinvou scheduled create --kind memory-organize` writes a kind record
    /// while the app is running, and the foundation's sweep re-reads task
    /// *definitions* from disk on every tick — so the scheduler will happily
    /// fire a task whose kind this handle has never seen. Answering `Chat`
    /// there is the unsafe direction (the executor would run the
    /// kind-specific prompt as an unattended full-permission Yolo
    /// conversation), so a miss pays a `stat` and re-reads only when the file
    /// actually changed; a hit stays lock-only with no IO, which is the hot
    /// path for every already-known task.
    pub(crate) fn kind_lookup_for(&self, automation_id: &str) -> ScheduledTaskKindLookup {
        if let Some(lookup) = self.stored_kind(automation_id) {
            return lookup;
        }
        self.reload_if_changed();
        self.stored_kind(automation_id)
            .unwrap_or(ScheduledTaskKindLookup::Chat)
    }

    /// Classify the in-memory entry, or None when this handle has no record
    /// for the id at all (the only case the disk can still contradict).
    fn stored_kind(&self, automation_id: &str) -> Option<ScheduledTaskKindLookup> {
        self.registry
            .read()
            .tasks
            .get(automation_id)
            .map(|entry| match entry.kind.as_str() {
                SCHEDULED_TASK_KIND_MEMORY_ORGANIZE => ScheduledTaskKindLookup::MemoryOrganize,
                other => ScheduledTaskKindLookup::Unsupported(other.to_string()),
            })
    }

    /// None removes the task's kind record (back to an ordinary chat task).
    pub(crate) fn set_kind(&self, automation_id: &str, kind: Option<String>) -> Result<()> {
        if automation_id.trim().is_empty() {
            bail!("scheduled automation id cannot be empty");
        }
        let kind = kind
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.reload_if_changed();
        let mut registry = self.registry.write();
        let previous = registry.tasks.get(automation_id).cloned();
        match kind {
            Some(kind) => {
                registry.tasks.insert(
                    automation_id.to_string(),
                    ScheduledTaskKindEntry {
                        kind,
                        updated_at: chrono::Utc::now().to_rfc3339(),
                    },
                );
            }
            None => {
                registry.tasks.remove(automation_id);
            }
        }
        if let Err(error) = self.persist(&registry) {
            <ScheduledTaskKindRegistry as ScheduledTasksMapRegistry>::restore_entry(
                &mut registry.tasks,
                automation_id,
                previous,
            );
            return Err(error);
        }
        Ok(())
    }
}

pub(crate) type ScheduledTaskModelBindingStore =
    VersionedJsonStore<ScheduledTaskModelBindingRegistry>;

impl VersionedJsonStore<ScheduledTaskModelBindingRegistry> {
    /// Same foreign-writer miss path as the task-kind store's
    /// `kind_lookup_for`: `pinvou scheduled create/update --model-id` rebinds
    /// this sidecar while the app is running, and a handle opened before that
    /// would silently drop the binding and run the task on the bare wire model
    /// name. That is a correctness bug rather than a safety one, so the fix
    /// stays the same size: a miss re-reads once, a hit never touches the disk.
    pub(crate) fn model_id_for(&self, automation_id: &str, model: &str) -> Option<String> {
        if let Some(model_id) = self.bound_model_id(automation_id, model) {
            return Some(model_id);
        }
        self.reload_if_changed();
        self.bound_model_id(automation_id, model)
    }

    fn bound_model_id(&self, automation_id: &str, model: &str) -> Option<String> {
        self.registry
            .read()
            .tasks
            .get(automation_id)
            .filter(|binding| binding.model == model)
            .map(|binding| binding.model_id.clone())
    }

    pub(crate) fn set(
        &self,
        automation_id: &str,
        model_id: Option<String>,
        model: Option<String>,
    ) -> Result<()> {
        if automation_id.trim().is_empty() {
            bail!("scheduled automation id cannot be empty");
        }
        let model_id = model_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let model = model
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.reload_if_changed();
        let mut registry = self.registry.write();
        let previous = registry.tasks.get(automation_id).cloned();
        match (model_id, model) {
            (Some(model_id), Some(model)) => {
                registry.tasks.insert(
                    automation_id.to_string(),
                    ScheduledTaskModelBinding {
                        model_id,
                        model,
                        updated_at: chrono::Utc::now().to_rfc3339(),
                    },
                );
            }
            _ => {
                registry.tasks.remove(automation_id);
            }
        }
        if let Err(error) = self.persist(&registry) {
            <ScheduledTaskModelBindingRegistry as ScheduledTasksMapRegistry>::restore_entry(
                &mut registry.tasks,
                automation_id,
                previous,
            );
            return Err(error);
        }
        Ok(())
    }
}

pub(crate) type ScheduledRunReadStore = VersionedJsonStore<ScheduledRunReadRegistry>;

impl VersionedJsonStore<ScheduledRunReadRegistry> {
    pub(crate) fn is_viewed(&self, automation_id: &str, run_id: &str) -> bool {
        self.registry
            .read()
            .viewed_runs
            .get(automation_id)
            .is_some_and(|runs| runs.contains(run_id))
    }

    pub(crate) fn mark_viewed(&self, automation_id: &str, run_id: &str) -> Result<()> {
        if automation_id.trim().is_empty() || run_id.trim().is_empty() {
            bail!("scheduled automation and run ids cannot be empty");
        }
        self.reload_if_changed();
        let mut registry = self.registry.write();
        let inserted = registry
            .viewed_runs
            .entry(automation_id.to_string())
            .or_default()
            .insert(run_id.to_string());
        if !inserted {
            return Ok(());
        }
        if let Err(error) = self.persist(&registry) {
            if let Some(runs) = registry.viewed_runs.get_mut(automation_id) {
                runs.remove(run_id);
                if runs.is_empty() {
                    registry.viewed_runs.remove(automation_id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn compact(
        &self,
        automation_id: &str,
        current_run_ids: &HashSet<String>,
    ) -> Result<()> {
        self.reload_if_changed();
        let mut registry = self.registry.write();
        let Some(existing) = registry.viewed_runs.get(automation_id).cloned() else {
            return Ok(());
        };
        let retained = existing
            .iter()
            .filter(|run_id| current_run_ids.contains(*run_id))
            .cloned()
            .collect::<HashSet<_>>();
        if retained == existing {
            return Ok(());
        }
        if retained.is_empty() {
            registry.viewed_runs.remove(automation_id);
        } else {
            registry
                .viewed_runs
                .insert(automation_id.to_string(), retained);
        }
        if let Err(error) = self.persist(&registry) {
            registry
                .viewed_runs
                .insert(automation_id.to_string(), existing);
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod foreign_writer_tests {
    //! Tests live in the same file as the production code: they target the
    //! three defect classes of `VersionedJsonStore` (foreign writers such as
    //! the CLI, corrupt files, and stat ordering) and reuse stores.rs's
    //! private visibility.
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_home() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-scheduled-store-tests-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn kind_store(path: &std::path::Path) -> ScheduledTaskKindStore {
        ScheduledTaskKindStore::open(path.to_path_buf()).expect("open kind store")
    }

    fn kind_entry_json() -> serde_json::Value {
        serde_json::json!({
            "kind": SCHEDULED_TASK_KIND_MEMORY_ORGANIZE,
            "updated_at": "2026-01-01T00:00:00Z",
        })
    }

    /// A kind-registry payload with the given tasks, written straight to
    /// `path` without going through any handle — what a foreign process does.
    fn write_kind_registry(path: &std::path::Path, tasks: serde_json::Value) {
        let payload = serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": SCHEDULED_TASK_KIND_SCHEMA_VERSION,
            "tasks": tasks,
        }))
        .expect("serialize kind registry");
        // Stage via temp+rename like every real writer (the foundation's
        // write_atomic, the CLI's write_json_atomic): an in-place write
        // keeps the inode, so a test that means to prove identity-based
        // detection would only ever prove mtime detection.
        let staging = path.with_extension("json.tmp-fixture");
        std::fs::write(&staging, payload).expect("write staging kind registry");
        std::fs::rename(&staging, path).expect("rename kind registry");
    }

    fn memory_organize() -> String {
        SCHEDULED_TASK_KIND_MEMORY_ORGANIZE.to_string()
    }

    /// A foreign write that lands between this handle's last read and its
    /// persist must not be silently destroyed by the whole-file rename: the
    /// persist re-checks the file's stamp and refuses. The refusal is
    /// recoverable — the next mutation's reload merges the foreign content
    /// and the re-applied write lands beside it.
    #[test]
    fn persist_refuses_when_a_foreign_write_landed_since_the_last_read() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");
        write_kind_registry(&path, serde_json::json!({ "t1": kind_entry_json() }));
        let store = kind_store(&path);
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize
        );

        // The foreign writer lands between our read and our persist.
        write_kind_registry(
            &path,
            serde_json::json!({ "t1": kind_entry_json(), "t2": kind_entry_json() }),
        );
        let error = store
            .persist(&store.registry.read())
            .expect_err("a stale persist must refuse instead of clobbering");
        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("changed on disk"),
            "the error must name the collision: {rendered}"
        );

        // The foreign content is intact on disk, and a retry (whose reload
        // merges it) lands without destroying it.
        store
            .persist(&store.registry.read())
            .expect_err("the stamp has not moved; the persist must still refuse");
        assert_eq!(
            store.kind_lookup_for("t2"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "the lookup miss reloads and merges the foreign write"
        );
        store
            .persist(&store.registry.read())
            .expect("the merged persist must go through");
        let fresh = kind_store(&path);
        assert_eq!(
            fresh.kind_lookup_for("t2"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "the merged persist must carry the foreign entry"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// The absent corner: a handle that priced in the file's absence must
    /// refuse to create it if a foreign writer created one in the window —
    /// creating would install this handle's payload over foreign content.
    #[test]
    fn persist_refuses_when_a_file_appeared_in_the_absent_window() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");
        let store = kind_store(&path);
        write_kind_registry(&path, serde_json::json!({ "t1": kind_entry_json() }));

        let error = store
            .persist(&store.registry.read())
            .expect_err("creating over an appeared file must refuse");
        assert!(
            format!("{error:#}").contains("changed on disk"),
            "{error:#}"
        );

        // The retry path merges the appeared content first.
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "the lookup miss reloads the appeared file"
        );
        store
            .persist(&store.registry.read())
            .expect("the merged persist must go through");
        let fresh = kind_store(&path);
        assert_eq!(
            fresh.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "the appeared content must survive the merge"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// Own writes must not trip the collision guard: back-to-back mutations
    /// on one handle (each persist advances `seen` to the stamp of the
    /// payload it proved) go through without a reload in between.
    #[test]
    fn sequential_own_persists_pass_the_collision_guard() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");
        let store = kind_store(&path);
        store
            .set_kind("t1", Some(memory_organize()))
            .expect("first persist creates the file");
        store
            .set_kind("t2", Some(memory_organize()))
            .expect("the second persist must not read as a collision");
        let fresh = kind_store(&path);
        assert_eq!(
            fresh.kind_lookup_for("t2"),
            ScheduledTaskKindLookup::MemoryOrganize
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// (g) A quarantined store must not degrade to the empty default once
    /// the file is gone: the rename-aside quarantine leaves the canonical
    /// path ABSENT while this handle's memory may be the only healthy copy.
    /// A later reload seeing the absent file must keep memory (the pre-fix
    /// behaviour installed `T::default()` over the healthy registry, and the
    /// next persist wrote that emptying through); the next mutator's persist
    /// rewrites the healthy registry and heals the file.
    #[test]
    fn quarantined_store_keeps_memory_after_the_file_is_gone() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");
        write_kind_registry(&path, serde_json::json!({ "t1": kind_entry_json() }));
        let store = kind_store(&path);
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize
        );

        // Corrupt on disk, then reload: the quarantine renames the file away
        // and memory keeps the healthy registry.
        std::fs::write(&path, b"{not json").unwrap();
        store.reload();
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "a corrupt file must not empty a healthy memory"
        );
        assert!(
            !path.exists(),
            "the Rename strategy must have removed the canonical file"
        );

        // The absent-file reload: the pre-fix behaviour answered
        // Loaded(default) here and swapped the empty registry in.
        store.reload();
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "an absent file after a quarantine must keep memory, not the \
             empty default"
        );

        // The next mutator's persist rewrites the healthy registry: the file
        // is healed and a fresh handle reads the same content.
        store
            .set_kind("t2", Some(memory_organize()))
            .expect("healing persist");
        assert_eq!(
            store.kind_lookup_for("t2"),
            ScheduledTaskKindLookup::MemoryOrganize
        );
        let fresh = kind_store(&path);
        assert_eq!(
            fresh.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "the healed file carries the healthy registry, not the default"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// A FOREIGN quarantine must not defeat the keep-memory guarantee: the
    /// CLI renames a corrupt sidecar aside on every command (read-only ones
    /// included), so this handle can observe the absence without ever having
    /// quarantined anything itself. Pre-fix, the absent-file read answered
    /// `Loaded(default)` for that case and the next persist wiped the only
    /// healthy copy; the keep now rides on the handle having proven the file
    /// real before, not on whose quarantine won.
    #[test]
    fn foreign_quarantine_before_reload_keeps_healthy_memory() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");
        write_kind_registry(&path, serde_json::json!({ "t1": kind_entry_json() }));
        let store = kind_store(&path);
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize
        );

        // The foreign CLI flow: corrupt on disk, then quarantine the corrupt
        // payload aside — the canonical path is absent again before this
        // handle's next reload.
        std::fs::write(&path, b"{not json").unwrap();
        let invalid = path.with_extension(format!(
            "json.invalid-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        std::fs::rename(&path, &invalid).expect("foreign quarantine rename");
        assert!(!path.exists(), "the foreign quarantine removed the file");

        // This handle's mutator reloads first: it must keep memory (never
        // install the default), and its persist must heal the file with the
        // FULL healthy registry, not the empty default.
        store
            .set_kind("t2", Some(memory_organize()))
            .expect("mutator after a foreign quarantine");
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "a foreign quarantine must not empty healthy memory"
        );
        let fresh = kind_store(&path);
        assert_eq!(
            fresh.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "the healed file carries the healthy registry, not the default"
        );
        assert_eq!(
            fresh.kind_lookup_for("t2"),
            ScheduledTaskKindLookup::MemoryOrganize
        );

        // The priced-in absence keeps repeated lookups at one stat: reload
        // again while the (healed) file is untouched — the stamp comparison,
        // not the sentinel, must short-circuit here because the file exists
        // again with a real stamp.
        store.reload();
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize
        );

        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_file(&invalid);
    }

    /// (a) Two store instances share one file: a resident handle's writes
    /// (compact / set_kind / remove) must not erase changes a foreign
    /// handle has already written.
    #[test]
    fn two_handles_over_one_file_do_not_erase_each_others_writes() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");

        // Seed one entry via a disposable handle, then open the two long-lived
        // handles over the same file: `gui` is the running app's (its poll
        // fires compact every few seconds), `cli` is the foreign process.
        kind_store(&path)
            .set_kind("doomed", Some(memory_organize()))
            .expect("seed kind entry");
        let gui = kind_store(&path);
        let cli = kind_store(&path);

        // The foreign handle rewrites the sidecar: drops "doomed", writes a
        // kind for a task the app never created.
        cli.remove("doomed").expect("foreign delete");
        cli.set_kind("cli-task", Some(memory_organize()))
            .expect("foreign create");

        // The GUI poll compacts for its listing, which includes the
        // foreign-created task. Pre-fix, the GUI persisted its stale
        // in-memory map and erased the foreign kind write.
        gui.compact(&HashSet::from(["cli-task".to_string()]))
            .expect("gui compact");
        assert_eq!(
            gui.kind_lookup_for("cli-task"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "compact must not rewrite the file from a stale in-memory map"
        );
        let after_compact = kind_store(&path);
        assert_eq!(
            after_compact.kind_lookup_for("cli-task"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "a fresh handle must read the foreign kind from disk"
        );

        // A GUI-side set_kind must merge with, not overwrite, the foreign entry.
        gui.set_kind("gui-task", Some(memory_organize()))
            .expect("gui set kind");
        let after_set = kind_store(&path);
        assert_eq!(
            after_set.kind_lookup_for("cli-task"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "set_kind must not erase the foreign entry"
        );
        assert_eq!(
            after_set.kind_lookup_for("gui-task"),
            ScheduledTaskKindLookup::MemoryOrganize
        );

        // A GUI-side remove must not take the foreign entry with it.
        gui.remove("gui-task").expect("gui remove");
        let after_remove = kind_store(&path);
        assert_eq!(
            after_remove.kind_lookup_for("cli-task"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "remove must not erase the foreign entry"
        );
        assert_eq!(
            after_remove.kind_lookup_for("gui-task"),
            ScheduledTaskKindLookup::Chat,
            "only the GUI's own entry is removed"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// (b) A reload that reads a corrupt file keeps the in-memory state and
    /// `reload_if_changed` picks the file up again once it is repaired. The
    /// quarantine renames the corrupt file away, so the failed read prices
    /// the confirmed ABSENCE in (`FileStamp::ABSENT`) — repeated checks then
    /// cost one stat instead of a failed read each — while a repaired file
    /// carries a real stamp that never equals the sentinel, so the retry is
    /// preserved. Before the sentinel, the failed read left the pre-corruption
    /// stamp in place, which made every subsequent check re-read the (absent)
    /// file; the sentinel is the priced-in version of the same retry promise.

    #[test]
    fn reload_on_a_corrupt_file_keeps_state_and_retries_after_repair() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");
        let store = kind_store(&path);
        store
            .set_kind("t1", Some(memory_organize()))
            .expect("seed valid state");

        // A crashed / hand-editing writer leaves an unusable file behind the
        // handle's back.
        std::fs::write(&path, "{ definitely-not-json").expect("write corrupt payload");
        store.reload();
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "a failed read must keep the previous in-memory state"
        );
        // The quarantine removed the canonical file; the absence is priced in.
        assert_eq!(
            *store.seen.read(),
            Some(FileStamp::ABSENT),
            "a quarantined-away file must be recorded as priced-in absence"
        );
        assert!(
            !path.exists(),
            "the Rename strategy must have removed the corrupt file"
        );
        // While the file stays absent, repeated checks answer from memory
        // without re-reading (the sentinel short-circuits) — and the kind
        // lookup must still see the healthy registry.
        store.reload();
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "the priced-in absence must keep memory on repeat checks"
        );
        // Quarantine note: the kind store renamed the corrupt file away
        // (Rename strategy) inside handle_invalid; the in-memory state still
        // must not degrade while the file is unusable.

        // Repaired behind the handle's back: the next miss must re-read and
        // pick the new content up instead of failing forever.
        write_kind_registry(
            &path,
            serde_json::json!({
                "t1": kind_entry_json(),
                "t2": kind_entry_json(),
            }),
        );
        assert_eq!(
            store.kind_lookup_for("t2"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "a repaired file must be picked up on the next check"
        );
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// (c) stat-before-read by construction: the real window (a foreign write
    /// landing exactly between the stat and the read) cannot be injected
    /// mid-call, so the test drives the same interleaving manually with
    /// reload's own primitives in reload's own order, asserting the resulting
    /// state does not mask subsequent changes and that the stamp lags memory
    /// in the safe direction.

    #[test]
    fn reload_stats_the_file_before_reading_it() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");

        // v1 on disk (only t1); a handle loaded on it.
        write_kind_registry(&path, serde_json::json!({ "t1": kind_entry_json() }));
        let store = kind_store(&path);
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize
        );

        // Step 1 — reload's stat, taken BEFORE its read.
        let stamp = FileStamp::of(&path).expect("v1 must be stat'able");
        // Step 2 — the racing write lands here: v2 adds t2.
        write_kind_registry(
            &path,
            serde_json::json!({ "t1": kind_entry_json(), "t2": kind_entry_json() }),
        );
        // Step 3 — reload's read of v2, then the swap, in reload's order.
        let flag = std::sync::atomic::AtomicBool::new(false);
        match VersionedJsonStore::<ScheduledTaskKindRegistry>::read_from_disk(&path, &flag, false) {
            DiskRead::Loaded(registry) | DiskRead::Migrated(registry) => {
                *store.registry.write() = registry;
            }
            DiskRead::Failed { .. } => panic!("v2 must be readable"),
        }
        *store.seen.write() = Some(stamp);

        // The interleaved state: memory carries the racing write (v2) while
        // `seen` still describes v1 — never the reverse, which is what
        // statting after the read produced.
        assert_eq!(
            store.kind_lookup_for("t2"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "the write that landed between stat and read must be in memory"
        );

        // A further foreign write must not be masked by the older recorded
        // stamp: the next check compares unequal and re-reads.
        write_kind_registry(
            &path,
            serde_json::json!({
                "t1": kind_entry_json(),
                "t2": kind_entry_json(),
                "t3": kind_entry_json(),
            }),
        );
        assert_eq!(
            store.kind_lookup_for("t3"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "the stale recorded stamp must force a re-read, not a skip"
        );

        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn two_same_length_foreign_writes_between_reloads_are_both_seen() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");

        // v1 seeds the handle's stamp: {t1}.
        write_kind_registry(&path, serde_json::json!({ "t1": kind_entry_json() }));
        // Platforms without identity (length+mtime is the only signal) cannot
        // deterministically distinguish two same-length writes at the same
        // instant; this case is only meaningful where identity exists (the
        // Windows build compiles but does not execute it).
        let seeded_identity = std::fs::metadata(&path)
            .ok()
            .and_then(|meta| metadata_file_identity(&meta));
        if seeded_identity.is_none() {
            return;
        }
        let store = kind_store(&path);
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize
        );
        // The store's `seen` stamp describes v1 right now; capture the same
        // stat for the aliasing guard below.
        let seeded_stamp = FileStamp::of(&path);

        // Two foreign writes, equal byte length, different key: renaming the
        // key keeps the payload shape (and thus length) identical while the
        // content differs.
        write_kind_registry(&path, serde_json::json!({ "ta": kind_entry_json() }));
        let first_write = FileStamp::of(&path);
        write_kind_registry(&path, serde_json::json!({ "tb": kind_entry_json() }));
        let second_write = FileStamp::of(&path);
        // The identity signal can be aliased by the environment itself: an
        // atomic rename hands the freed tmp inode number to the NEXT write's
        // tmp file, and a coarse mtime tick hides the ordering — two writes
        // (or a write and the seeded v1, whose two-char keys share the byte
        // length) then carry an identical stamp, and the store's "no change"
        // verdict is correct for the signal it has. That is the same aliasing
        // the no-identity guard above acknowledges, one layer up: the test can
        // only demand second-write visibility where the environment actually
        // provides a signal distinguishing the second write from BOTH the
        // first one and the seeded v1 the handle recorded.
        if let (Some(first), Some(second)) = (first_write, second_write) {
            if first == second || Some(second) == seeded_stamp {
                return;
            }
        } else {
            return;
        }

        // The next miss must re-read and land on the SECOND write, not skip
        // because every version shares the same byte length.
        assert_eq!(
            store.kind_lookup_for("tb"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "the second same-length foreign write must be visible"
        );
        assert_eq!(
            store.kind_lookup_for("ta"),
            ScheduledTaskKindLookup::Chat,
            "memory must reflect the second write, not the first"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// (e) A stamp whose len+mtime match but whose identity (dev/ino)
    /// differs must not be treated as "unchanged" — an atomic rename always
    /// changes the inode, so identity must participate in the equality
    /// comparison; on coarse-mtime filesystems only identity can distinguish
    /// two same-length writes at the same instant. The seen identity field is
    /// forged directly, so the case runs identically on platforms with and
    /// without identity (on the latter a real stamp is always None and the
    /// identity comparison never fires).
    #[test]
    fn file_identity_participates_in_stamp_comparison() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");
        write_kind_registry(&path, serde_json::json!({ "t1": kind_entry_json() }));
        let store = kind_store(&path);

        // Foreign rewrite (a new inode via the same atomic-rename shape every
        // writer uses), then forge a `seen` that matches the new file's
        // len+mtime but carries a different identity — exactly what a
        // coarse-mtime filesystem would hand a len+mtime-only comparison.
        write_kind_registry(&path, serde_json::json!({ "t2": kind_entry_json() }));
        let current = FileStamp::of(&path).expect("stat the rewritten file");
        let forged_identity = current.identity.unwrap_or(FileIdentity {
            device: 0,
            inode: 0,
        });
        *store.seen.write() = Some(FileStamp {
            len: current.len,
            modified: current.modified,
            identity: Some(FileIdentity {
                device: forged_identity.device.wrapping_add(1),
                inode: forged_identity.inode,
            }),
        });
        assert_ne!(*store.seen.read(), Some(current));

        assert_eq!(
            store.kind_lookup_for("t2"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "an identity mismatch must force the re-read even at equal len+mtime"
        );
        // The reload records the real stamp, so the next check skips again.
        assert_eq!(*store.seen.read(), Some(current));

        let _ = std::fs::remove_dir_all(dir);
    }

    /// (f) The persist-side "is the on-disk content our write?" judgment:
    /// the disclosed freeze scenario is a same-length foreign write landing
    /// exactly between write_atomic and the stamp being recorded — neither
    /// length nor even mtime can catch it, only a content comparison can.
    /// Only a stamp proven to be our write is recorded; anything else —
    /// mismatch or un-stat-able — records unknown, forcing a re-read on the
    /// next check, never treating it as "unchanged".
    #[test]
    fn persist_records_only_a_proven_own_write() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");

        // A clean write records its own stamp.
        let ours = b"{\n  \"payload\": ours\n}";
        std::fs::write(&path, ours).expect("seed our payload");
        assert!(
            stamp_of_our_write(&path, ours).is_some(),
            "a file carrying our payload must be recorded as ours"
        );

        // The disclosed race, driven manually: our write lands, then a
        // same-length foreign payload replaces it before the stamp is
        // recorded. The length check alone would bless the foreign file.
        let foreign = b"{\n  \"payload\": user\n}";
        assert_eq!(foreign.len(), ours.len(), "fixture must be same-length");
        std::fs::write(&path, foreign).expect("foreign same-length clobber");
        assert!(
            stamp_of_our_write(&path, ours).is_none(),
            "a same-length foreign clobber must not be recorded as ours"
        );

        // Unstat'able stays "unknown", never "unchanged" (existing
        // semantics).
        std::fs::remove_file(&path).expect("remove the file");
        assert!(stamp_of_our_write(&path, ours).is_none());

        let _ = std::fs::remove_dir_all(dir);
    }

    /// (f) The lock-swap half of the reload race, closed in the round-21
    /// review (M1): a reload that has already read the file when a mutator
    /// on this handle commits must not afterwards install the stale payload
    /// over the mutator's state. The seam `reload_with_gap` exposes makes
    /// this interleaving — impossible to hit through the public surface on
    /// demand — deterministic.
    #[test]
    fn reload_does_not_overwrite_a_mutator_that_committed_during_its_read() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");
        // v2 is what the file carries by the time reload's read returns.
        write_kind_registry(&path, serde_json::json!({ "t1": kind_entry_json() }));
        let store = kind_store(&path);
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize
        );

        // The gap replays the pre-fix interleaving end to end: reload stats
        // and reads v2; while its swap waits on the write lock, the mutator
        // takes the lock first, applies its change, persists v3 and advances
        // `seen`. reload's swap then runs — with the fix it drops the stale
        // v2 read; pre-fix it would have regressed memory to v2 and dragged
        // `seen` back to a stamp the file had already moved past.
        let mut pre_read_stamp = None;
        store.reload_with_gap(|| {
            // The file still carries v2 here: this is exactly the stamp
            // reload took before its read, and the one the pre-fix swap
            // would have written into `seen`.
            pre_read_stamp = FileStamp::of(&path);
            store
                .set_kind("t2", Some(memory_organize()))
                .expect("mutator commits inside the read-to-swap window");
        });

        // The mutator's committed change must survive in memory …
        assert_eq!(
            store.kind_lookup_for("t2"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "the mutator that committed during the read must not be overwritten"
        );
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize
        );
        // … and `seen` must not end up describing the state reload read,
        // which the file has already moved past — that is the half of the
        // race that turns a transient regression into a masked one.
        assert_ne!(
            *store.seen.read(),
            pre_read_stamp,
            "`seen` must carry the mutator's own record, not the stale pre-read stamp"
        );
        // … and the file on disk must still carry the mutator's v3 payload —
        // nothing rewrote it behind the mutator's back.
        let on_disk = std::fs::read_to_string(&path).expect("read the kind registry");
        assert!(
            on_disk.contains("\"t2\""),
            "the mutator's persisted payload must remain on disk"
        );
        assert!(
            on_disk.contains("\"t1\""),
            "the pre-read payload's entry must be merged, not clobbered"
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
