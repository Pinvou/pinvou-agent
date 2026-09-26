//! 版本化 JSON store:3 个定时注册表的泛型持久化。
//!
//! Wave 1 1d 建立的 `VersionedRegistry` trait + `VersionedJsonStore<T>` 泛型,
//! 收敛 scheduled run read / model binding / UI metadata 三个同构 store。
//! 从 tasks.rs 抽离,通过 `use super::*` 复用 facade 的导入。

use std::path::Path;
use std::time::SystemTime;

use parking_lot::RwLock;

use super::*;

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
    /// anything. `None` means "unknown", which always forces a read.
    seen: Arc<RwLock<Option<FileStamp>>>,
}

/// Cheap change detector for the registry file: a `stat` is orders of
/// magnitude cheaper than read + parse + lock swap, and every writer of these
/// files (this handle and the `pinvou` CLI) goes through an atomic
/// write-and-rename, so a new payload always lands as a new inode with a fresh
/// mtime and length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    /// `None` when the file cannot be stat'ed at all (missing, or a metadata
    /// error) — treated as "unknown", never as "unchanged".
    fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
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
    /// repaired.
    Failed,
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
    fn read_from_disk(path: &Path) -> DiskRead<T> {
        match std::fs::read_to_string(path) {
            Ok(raw) => match serde_json::from_str::<T>(&raw) {
                Ok(registry) if registry.schema_version() == T::SUPPORTED_VERSION => {
                    DiskRead::Loaded(registry)
                }
                Ok(registry) if registry.schema_version() < T::SUPPORTED_VERSION => {
                    DiskRead::Migrated(registry.migrate())
                }
                Ok(registry) => {
                    Self::handle_invalid(
                        path,
                        &format!(
                            "schema v{} is newer than supported v{}",
                            registry.schema_version(),
                            T::SUPPORTED_VERSION
                        ),
                    );
                    DiskRead::Failed
                }
                Err(error) => {
                    Self::handle_invalid(path, &format!("invalid JSON: {error}"));
                    DiskRead::Failed
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                DiskRead::Loaded(T::default())
            }
            Err(error) => {
                log::warn!(
                    "Unable to read {} {}: {error}{}",
                    T::LABEL,
                    path.display(),
                    T::WARN_SUFFIX
                );
                DiskRead::Failed
            }
        }
    }

    pub(crate) fn open(path: PathBuf) -> Result<Self> {
        // Stat before read, matching `reload`: a foreign write landing between
        // the two leaves the stamp older than memory, which at worst costs one
        // extra read later — never the reverse (memory stale under a stamp
        // that matches).
        let stamp = FileStamp::of(&path);
        let (registry, migrated) = match Self::read_from_disk(&path) {
            // Fail open to an empty registry as before: existing startup
            // behaviour kept (there is no previous state to preserve here).
            DiskRead::Loaded(registry) => (registry, false),
            DiskRead::Migrated(registry) => (registry, true),
            DiskRead::Failed => (T::default(), false),
        };
        let store = Self {
            path: Arc::new(path),
            registry: Arc::new(RwLock::new(registry)),
            seen: Arc::new(RwLock::new(stamp)),
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
        }
    }

    /// Re-read the on-disk registry and swap it into the in-memory copy.
    ///
    /// Disk — not memory — is the authority: every mutation here persists while
    /// holding the write lock (and rolls back when the write fails), so the
    /// file is never behind this handle, while a *foreign* process (the
    /// `pinvou` CLI writes the same sidecars) can put it ahead.
    pub(crate) fn reload(&self) {
        // Stat before read: if a foreign write lands between the two, the
        // stamp ends up older than memory, which at worst costs this handle
        // one extra read later. Stating after the read produced the reverse —
        // memory behind the file while the stamp matched — which made
        // `reload_if_changed` skip the re-read that would have fixed it.
        let stamp = FileStamp::of(self.path.as_ref());
        let (registry, migrated) = match Self::read_from_disk(self.path.as_ref()) {
            DiskRead::Loaded(registry) => (registry, false),
            DiskRead::Migrated(registry) => (registry, true),
            // Unusable payload: keep the previous in-memory registry and do
            // NOT update the stamp. Swapping in `T::default()` here (the
            // pre-fix behaviour) resolved every miss to a plain chat task and
            // recorded the stamp so the damage never healed; with the stamp
            // unchanged, a later check re-reads once the file is repaired.
            DiskRead::Failed => return,
        };
        {
            // Scoped: persist_migrated() takes a read lock, and parking_lot
            // locks are not reentrant.
            *self.registry.write() = registry;
            *self.seen.write() = stamp;
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
        if let (Some(current), Some(seen)) = (current, *self.seen.read())
            && current == seen
        {
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
        let payload = serde_json::to_vec_pretty(registry)
            .with_context(|| format!("serialize {}", T::LABEL))?;
        let record_len = payload.len() as u64;
        deepseek_tui::utils::write_atomic(self.path.as_ref(), &payload)
            .with_context(|| format!("write {} {}", T::LABEL, self.path.display()))?;
        // Record what we just wrote so `reload_if_changed` does not mistake
        // this handle's own write for a foreign one and re-read it. The stamp
        // is taken from the file on disk *now* rather than derived from the
        // write itself: a foreign process going through `write_atomic` in the
        // same instant replaces the path with its own inode, and mtime
        // granularity cannot separate the two writes — a length check can. If
        // the current stamp does not describe our own payload, skip the
        // update: memory behind the file under a matching stamp is exactly
        // the state `reload_if_changed` exists to repair.
        let stamp = FileStamp::of(self.path.as_ref());
        let recorded = match stamp {
            Some(stamp) if stamp.len == record_len => Some(stamp),
            // Overwritten by a foreign writer (or unstat'able): "unknown"
            // forces the next check to re-read, which merges in the foreign
            // payload — never drops it.
            _ => None,
        };
        *self.seen.write() = recorded;
        Ok(())
    }

    /// Apply this store's quarantine policy to an invalid payload at `path`.
    pub(crate) fn handle_invalid(path: &Path, reason: &str) {
        match T::QUARANTINE {
            QuarantineStrategy::LogInPlace => {
                log::warn!(
                    "Ignoring invalid {} {} ({reason})",
                    T::LABEL,
                    path.display()
                );
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
                    Ok(()) => log::warn!(
                        "Quarantined {} {} to {} ({reason}){}",
                        T::LABEL,
                        path.display(),
                        quarantine_path.display(),
                        T::WARN_SUFFIX
                    ),
                    Err(error) => log::warn!(
                        "Invalid {} {} ({reason}) could not be quarantined: {error}{}",
                        T::LABEL,
                        path.display(),
                        T::WARN_SUFFIX
                    ),
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
        self.registry.read().tasks.values().cloned().collect()
    }

    pub(crate) fn runs_for(&self, automation_id: &str) -> Option<Vec<AutomationRunRecord>> {
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
    //! 测试与生产代码同文件:聚焦 `VersionedJsonStore` 的外部写者(CLI)、
    //! 损坏文件与 stat 顺序三类缺陷,复用 stores.rs 的私有可见性。
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
        std::fs::write(path, payload).expect("write kind registry");
    }

    fn memory_organize() -> String {
        SCHEDULED_TASK_KIND_MEMORY_ORGANIZE.to_string()
    }

    /// (a) 两个 store 实例共享一个文件:常驻句柄的写操作
    /// (compact / set_kind / remove)不得抹掉外部句柄已写入的变更。
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

    /// (b) reload 读到损坏文件:保留内存旧状态、不记录 stamp(之后重试),
    /// 文件修复后 reload_if_changed 能重新读到。
    #[test]
    fn reload_on_a_corrupt_file_keeps_state_and_retries_after_repair() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");
        let store = kind_store(&path);
        store
            .set_kind("t1", Some(memory_organize()))
            .expect("seed valid state");
        let stamp_before = *store.seen.read();

        // A crashed / hand-editing writer leaves an unusable file behind the
        // handle's back.
        std::fs::write(&path, "{ definitely-not-json").expect("write corrupt payload");
        store.reload();
        assert_eq!(
            store.kind_lookup_for("t1"),
            ScheduledTaskKindLookup::MemoryOrganize,
            "a failed read must keep the previous in-memory state"
        );
        assert_eq!(
            *store.seen.read(),
            stamp_before,
            "a failed read must not record the stamp; later checks must retry"
        );
        // Quarantine note: the kind store would rename the corrupt file away
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

    /// (c) stat-before-read by construction: the real window (外部写恰好落在
    /// stat 与 read 之间)无法在调用中途注入,所以用 reload 自己的原语、
    /// 按 reload 的顺序手工驱动同样的交错,断言由此产生的状态不会把后续
    /// 变更掩掉,也验证 stamp 落后于内存这一安全方向。
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
        match VersionedJsonStore::<ScheduledTaskKindRegistry>::read_from_disk(&path) {
            DiskRead::Loaded(registry) | DiskRead::Migrated(registry) => {
                *store.registry.write() = registry;
            }
            DiskRead::Failed => panic!("v2 must be readable"),
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
}
