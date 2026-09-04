//! 版本化 JSON store:3 个定时注册表的泛型持久化。
//!
//! Wave 1 1d 建立的 `VersionedRegistry` trait + `VersionedJsonStore<T>` 泛型,
//! 收敛 scheduled run read / model binding / UI metadata 三个同构 store。
//! 从 tasks.rs 抽离,通过 `use super::*` 复用 facade 的导入。

use std::path::Path;

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
}

impl<T: VersionedRegistry> VersionedJsonStore<T> {
    pub(crate) fn open(path: PathBuf) -> Result<Self> {
        let mut migrated = false;
        let registry = match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<T>(&raw) {
                Ok(registry) if registry.schema_version() == T::SUPPORTED_VERSION => registry,
                Ok(registry) if registry.schema_version() < T::SUPPORTED_VERSION => {
                    migrated = true;
                    registry.migrate()
                }
                Ok(registry) => {
                    Self::handle_invalid(
                        &path,
                        &format!(
                            "schema v{} is newer than supported v{}",
                            registry.schema_version(),
                            T::SUPPORTED_VERSION
                        ),
                    );
                    T::default()
                }
                Err(error) => {
                    Self::handle_invalid(&path, &format!("invalid JSON: {error}"));
                    T::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => T::default(),
            Err(error) => {
                log::warn!(
                    "Unable to read {} {}: {error}{}",
                    T::LABEL,
                    path.display(),
                    T::WARN_SUFFIX
                );
                T::default()
            }
        };
        let store = Self {
            path: Arc::new(path),
            registry: Arc::new(RwLock::new(registry)),
        };
        if migrated {
            if let Err(error) = store.persist(&store.registry.read()) {
                log::warn!(
                    "Unable to persist migrated {} {}: {error:#}",
                    T::LABEL,
                    store.path.display()
                );
            }
        }
        Ok(store)
    }

    pub(crate) fn persist(&self, registry: &T) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {} dir {}", T::LABEL, parent.display()))?;
        }
        let payload = serde_json::to_vec_pretty(registry)
            .with_context(|| format!("serialize {}", T::LABEL))?;
        deepseek_tui::utils::write_atomic(self.path.as_ref(), &payload)
            .with_context(|| format!("write {} {}", T::LABEL, self.path.display()))
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
            match previous {
                Some(archived) => {
                    registry.tasks.insert(automation_id, archived);
                }
                None => {
                    registry.tasks.remove(&automation_id);
                }
            }
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
            registry.tasks.insert(automation_id.to_string(), previous);
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
        let mut registry = self.registry.write();
        let previous = registry.tasks.insert(automation_id.clone(), archived);
        if let Err(error) = self.persist(&registry) {
            match previous {
                Some(previous) => {
                    registry.tasks.insert(automation_id, previous);
                }
                None => {
                    registry.tasks.remove(&automation_id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn remove_task(&self, automation_id: &str) -> Result<()> {
        let mut registry = self.registry.write();
        let Some(previous) = registry.tasks.remove(automation_id) else {
            return Ok(());
        };
        if let Err(error) = self.persist(&registry) {
            registry.tasks.insert(automation_id.to_string(), previous);
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
            match previous {
                Some(metadata) => {
                    registry.tasks.insert(automation_id.to_string(), metadata);
                }
                None => {
                    registry.tasks.remove(automation_id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn remove(&self, automation_id: &str) -> Result<()> {
        let mut registry = self.registry.write();
        let Some(previous) = registry.tasks.remove(automation_id) else {
            return Ok(());
        };
        if let Err(error) = self.persist(&registry) {
            registry.tasks.insert(automation_id.to_string(), previous);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn compact(&self, automation_ids: &HashSet<String>) -> Result<()> {
        let mut registry = self.registry.write();
        let before = registry.tasks.clone();
        registry.tasks.retain(|id, _| automation_ids.contains(id));
        if registry.tasks == before {
            return Ok(());
        }
        if let Err(error) = self.persist(&registry) {
            registry.tasks = before;
            return Err(error);
        }
        Ok(())
    }
}

pub(crate) type ScheduledTaskKindStore = VersionedJsonStore<ScheduledTaskKindRegistry>;

impl VersionedJsonStore<ScheduledTaskKindRegistry> {
    /// Reads the task kind. Only `memory_organize` is a supported kind for now; any other
    /// value left in the file is treated as an ordinary chat task (returns None),
    /// mirroring the creation-side allow-list.
    pub(crate) fn kind_for(&self, automation_id: &str) -> Option<String> {
        self.registry
            .read()
            .tasks
            .get(automation_id)
            .map(|entry| entry.kind.clone())
            .filter(|kind| kind == SCHEDULED_TASK_KIND_MEMORY_ORGANIZE)
    }

    /// None removes the task's kind record (back to an ordinary chat task).
    pub(crate) fn set_kind(&self, automation_id: &str, kind: Option<String>) -> Result<()> {
        if automation_id.trim().is_empty() {
            bail!("scheduled automation id cannot be empty");
        }
        let kind = kind
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
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
            match previous {
                Some(entry) => {
                    registry.tasks.insert(automation_id.to_string(), entry);
                }
                None => {
                    registry.tasks.remove(automation_id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn remove(&self, automation_id: &str) -> Result<()> {
        let mut registry = self.registry.write();
        let Some(previous) = registry.tasks.remove(automation_id) else {
            return Ok(());
        };
        if let Err(error) = self.persist(&registry) {
            registry.tasks.insert(automation_id.to_string(), previous);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn compact(&self, automation_ids: &HashSet<String>) -> Result<()> {
        let mut registry = self.registry.write();
        let before = registry.tasks.clone();
        registry.tasks.retain(|id, _| automation_ids.contains(id));
        if registry.tasks == before {
            return Ok(());
        }
        if let Err(error) = self.persist(&registry) {
            registry.tasks = before;
            return Err(error);
        }
        Ok(())
    }
}

pub(crate) type ScheduledTaskModelBindingStore =
    VersionedJsonStore<ScheduledTaskModelBindingRegistry>;

impl VersionedJsonStore<ScheduledTaskModelBindingRegistry> {
    pub(crate) fn model_id_for(&self, automation_id: &str, model: &str) -> Option<String> {
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
            match previous {
                Some(binding) => {
                    registry.tasks.insert(automation_id.to_string(), binding);
                }
                None => {
                    registry.tasks.remove(automation_id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn remove(&self, automation_id: &str) -> Result<()> {
        let mut registry = self.registry.write();
        let Some(previous) = registry.tasks.remove(automation_id) else {
            return Ok(());
        };
        if let Err(error) = self.persist(&registry) {
            registry.tasks.insert(automation_id.to_string(), previous);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn compact(&self, automation_ids: &HashSet<String>) -> Result<()> {
        let mut registry = self.registry.write();
        let before = registry.tasks.clone();
        registry.tasks.retain(|id, _| automation_ids.contains(id));
        if registry.tasks == before {
            return Ok(());
        }
        if let Err(error) = self.persist(&registry) {
            registry.tasks = before;
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

    pub(crate) fn remove_automation(&self, automation_id: &str) -> Result<()> {
        let mut registry = self.registry.write();
        let Some(removed) = registry.viewed_runs.remove(automation_id) else {
            return Ok(());
        };
        if let Err(error) = self.persist(&registry) {
            registry
                .viewed_runs
                .insert(automation_id.to_string(), removed);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn compact(
        &self,
        automation_id: &str,
        current_run_ids: &HashSet<String>,
    ) -> Result<()> {
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
