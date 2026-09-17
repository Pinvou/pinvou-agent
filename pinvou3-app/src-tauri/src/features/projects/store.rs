//! ProjectStore: persistence, validation, and atomic writes for project
//! definitions and the session-assignment map.
//!
//! Storage is a single JSON file (`~/.pinvou3/projects/projects.json`):
//! project list and assignment map live in the same file, and one tmp+rename
//! atomic write covers both views. The empty state leaves no file (sidecar
//! family convention); a corrupt file boots as empty and the next mutation
//! self-heals by overwriting it — assignments are pure preference data, so
//! losing them is equivalent to falling back to implicit folder grouping and
//! needs no `code-session.json`-style dual-insurance sidecar.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// Project entity: name + folder territory (roots, canonicalized absolute
/// paths) + ordering position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub roots: Vec<PathBuf>,
    /// Manual sidebar ordering position; new projects append to the end
    /// (max+1), same position semantics as Codex.
    pub position: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Origin marker: `Some("folder")` = project auto-materialized from a
    /// folder (Codex-client-style adoption); `None` = created manually by the
    /// user. Only used as a UI badge and test anchor; it plays no part in
    /// grouping decisions or delete semantics; renames/root edits keep the
    /// original value (no name write-back sync).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// The project's remembered primary folder (§9.2/§9.3): default cwd when
    /// creating a session from the project channel. Must be a roots member;
    /// written explicitly by the creation flow / management panel; demoted to
    /// None when a root replacement drops it from roots (see
    /// `demote_stale_primary_root`) so a stale primary folder cannot keep
    /// serving as the project channel's cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_primary_root: Option<PathBuf>,
}

/// Assignment map value: `Some(project_id)` = explicit assignment; `None` =
/// explicit move-out (skip auto-grouping, fall back to implicit folder
/// grouping); no entry = undecided, auto-grouping applies.
pub type SessionAssignments = HashMap<String, Option<String>>;

/// Single-file persistence shape. schema_version identifies future structural
/// evolution: reading a newer version degrades to the empty state but sets
/// the write-refusal flag (see `StoreState::refuse_writes`) — otherwise the
/// empty state plus the next mutation would downgrade-overwrite and corrupt
/// the newer file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ProjectsFile {
    pub schema_version: u32,
    pub projects: Vec<Project>,
    #[serde(default)]
    pub assignments: SessionAssignments,
    /// Anti-materialization exclusion list (§3, canonical identity keys):
    /// the user's explicit "never auto-create a project for this folder";
    /// visible and revocable (management panel). Old files without the key
    /// read as an empty list.
    #[serde(default)]
    pub never_materialize_roots: Vec<String>,
}

const SCHEMA_VERSION: u32 = 1;

/// Delete-project report: affected sessions = all members (explicitly
/// assigned + auto-grouped), each written as an explicit move-out and left in
/// Ungrouped; sessions themselves are never deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeleteProjectReport {
    pub affected_session_ids: Vec<String>,
}

/// Move-assignment outcome: the frontend uses it to prompt "added to project
/// (and added folder xx)".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MoveSessionOutcome {
    pub project_id: Option<String>,
    /// Folder additionally adopted into the target project this time
    /// (canonicalized); None if nothing was added.
    pub added_root: Option<PathBuf>,
}

/// Per-root result of `ensure_folder_roots`: lets the frontend/caller
/// distinguish creation, reuse, and conflict; a conflict does not block the
/// remaining roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EnsureFolderOutcome {
    /// Created a folder project named after the directory (origin=folder).
    Created { project: Project },
    /// A materialized project anchored at this folder already exists
    /// (origin=folder and roots contain exactly this path); reused untouched.
    Covered { project_id: String },
    /// Could not create (non-absolute path, etc.); `reason` is log-ready.
    Failed { reason: String },
}

#[derive(Debug, Default)]
struct StoreState {
    /// Always ordered by (position, id); `list` returns the snapshot as-is.
    projects: Vec<Project>,
    assignments: SessionAssignments,
    /// Anti-materialization exclusion list (canonical identity keys,
    /// deduplicated); ensure skips listed roots.
    never_materialize_roots: Vec<String>,
    /// Set when the on-disk file has a higher schema_version than this
    /// process supports: all subsequent writes are refused so a downgraded
    /// process cannot overwrite the newer structure.
    refuse_writes: bool,
}

/// Project-layer storage. Fields are `Arc`-wrapped so the whole value clones
/// into Tauri State and into the delete-hook closure.
#[derive(Clone)]
pub struct ProjectStore {
    state: Arc<RwLock<StoreState>>,
    path: Arc<PathBuf>,
}

/// Project ids combine a nanosecond timestamp with a process-local monotonic
/// counter: the timestamp gives cross-process uniqueness, the counter covers
/// collisions from back-to-back creations within one clock granularity
/// (coarse on Windows).
fn generate_project_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const ALPHA: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let encode = |mut n: u128, len: usize| {
        let mut buf = String::with_capacity(len);
        for _ in 0..len {
            buf.push(ALPHA[(n % 36) as usize] as char);
            n /= 36;
        }
        buf
    };
    format!("prj-{}{}", encode(nanos, 13), encode(u128::from(count), 3))
}

fn validate_name(raw: String) -> Result<String> {
    let name = raw.trim().to_string();
    if name.is_empty() {
        bail!("project name must not be empty");
    }
    Ok(name)
}

/// Demote a remembered primary root that survived the removal of its own
/// root: after a root replacement, a `last_primary_root` that is no longer a
/// roots member would keep serving as the project channel's default cwd
/// (§9.3) even though the folder left the project. Membership uses the same
/// folded-key exact match as `set_last_primary_root`.
fn demote_stale_primary_root(project: &mut Project) {
    let Some(primary) = &project.last_primary_root else {
        return;
    };
    let key = identity_key_of_display(primary);
    if !project
        .roots
        .iter()
        .any(|root| identity_key_of_display(root) == key)
    {
        project.last_primary_root = None;
    }
}

/// Display form of a root: `fs::canonicalize` when the directory exists
/// (resolves symlinks); when it does not, canonicalize the nearest existing
/// ancestor and re-append the missing suffix (see `canonicalize_via_ancestor`
/// — a purely lexical fallback would diverge from stored values on macOS).
/// Overlap checks must remain decidable after a folder is moved away, and the
/// form is idempotent for existing values (canonicalize(canonical p) == p).
/// The shared `platform_compat_path` then strips the `\\?\` verbatim prefix
/// produced by Windows canonicalize (identity elsewhere), the same convention
/// as `validate_codex_project_workspace`.
fn root_display(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| canonicalize_via_ancestor(path));
    crate::platform::os::platform_compat_path(&canonical.to_string_lossy())
}

/// Fallback when the path does not exist: canonicalize the nearest existing
/// ancestor, then re-append the missing suffix component by component
/// (review #464 MAJOR 3): purely lexical absolutization diverges on macOS
/// from already-canonicalized stored values (/var/folders vs
/// /private/var/folders) — folded identity keys no longer nest and covered
/// detection breaks. Only when no ancestor exists at all (hand-crafted
/// corrupt state) does it fall back to the lexical form.
fn canonicalize_via_ancestor(path: &Path) -> PathBuf {
    let abs = lexical_absolute(path);
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = abs.as_path();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(cursor) {
            let mut rebuilt = canonical;
            for component in missing.iter().rev() {
                rebuilt.push(component);
            }
            return rebuilt;
        }
        match (cursor.file_name(), cursor.parent()) {
            (Some(name), Some(parent)) => {
                missing.push(name.to_os_string());
                cursor = parent;
            }
            _ => return abs,
        }
    }
}

/// Comparison key for roots: the display form folded through the shared
/// `filesystem_path_identity_key` — Windows folds separators and case
/// (`C:\Work` and `c:\work` are the same root); POSIX is case-sensitive and
/// preserved verbatim.
///
/// Known residue (accepted edge): macOS APFS is case-insensitive by default,
/// but the shared helper deliberately does not fold case ("the volume may be
/// configured case-sensitive", see platform/os/macos), so the same directory
/// spelled in different case still counts as two roots. Folding per platform
/// would break case-sensitive volumes, so this is documented rather than
/// folded.
fn root_key(path: &Path) -> String {
    identity_key_of_display(&root_display(path))
}

/// Comparison key for an already-stored root (written in display form):
/// key folding only, no disk access needed.
fn identity_key_of_display(path: &Path) -> String {
    crate::platform::os::filesystem_path_identity_key(&path.to_string_lossy())
}

/// Component-aware "equal to or nested under": keys are forward-slashed
/// strings, so a bare `starts_with` would misfile `/a/bc` under `/a/b`; the
/// boundary must be a separator.
fn key_is_same_or_nested(key: &str, base: &str) -> bool {
    if key == base {
        return true;
    }
    let base = base.strip_suffix('/').unwrap_or(base);
    if base.is_empty() {
        // POSIX root "/": every absolute path nests under it.
        return key.starts_with('/');
    }
    key.starts_with(base) && key[base.len()..].starts_with('/')
}

/// Diff two roots snapshots: old roots not covered by any new root (neither
/// equal to nor nested under one) are the roots removed this time (§4
/// folder-removal semantics). The command layer enumerates auto-grouped
/// members under them to write explicit move-outs, preventing grouping /
/// materialization from immediately "overturning" the removal.
pub fn removed_roots(old: &[PathBuf], new: &[PathBuf]) -> Vec<PathBuf> {
    let new_keys: Vec<String> = new
        .iter()
        .map(|root| identity_key_of_display(root))
        .collect();
    old.iter()
        .filter(|root| {
            let key = identity_key_of_display(root);
            !new_keys
                .iter()
                .any(|new_key| key_is_same_or_nested(&key, new_key))
        })
        .cloned()
        .collect()
}

/// Disk-free absolutization: `.` dropped, `..` pops one level, prefix kept
/// (Unix root / Windows drive).
fn lexical_absolute(path: &Path) -> PathBuf {
    let mut stack: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match stack.last() {
                Some(Component::Normal(_)) => {
                    stack.pop();
                }
                _ => stack.push(component),
            },
            other => stack.push(other),
        }
    }
    stack.into_iter().collect()
}

/// Validate a set of roots and return their display forms (canonicalized):
/// duplicate and nesting checks run on identity keys (decidable after Windows
/// case/separator folding).
/// - must be absolute paths;
/// - no duplicates or mutual nesting within the set.
///
/// Cross-project overlap has been legal since the 2026-09-11 decision (§9.9
/// root-overlap legalization): the same physical folder may be referenced by
/// multiple projects; auto-grouping ties are broken by the frontend via
/// position, and the backend no longer rejects.
fn validate_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut displays = Vec::with_capacity(roots.len());
    let mut keys = Vec::with_capacity(roots.len());
    for root in roots {
        if !root.is_absolute() {
            bail!("project root must be absolute: {}", root.display());
        }
        let display = root_display(root);
        let key = identity_key_of_display(&display);
        if keys.contains(&key) {
            bail!("duplicate project root: {}", root.display());
        }
        displays.push(display);
        keys.push(key);
    }
    for (index, (key, display)) in keys.iter().zip(displays.iter()).enumerate() {
        for (other, other_display) in keys.iter().zip(displays.iter()).skip(index + 1) {
            if key_is_same_or_nested(key, other) || key_is_same_or_nested(other, key) {
                bail!(
                    "project roots must not nest: {} vs {}",
                    display.display(),
                    other_display.display()
                );
            }
        }
    }
    Ok(displays)
}

/// Atomic persist (shared `atomic_write`: fsync + unique tmp name + backup
/// semantics). The empty state removes the file, leaving no empty shell.
/// Writes are refused after reading a newer schema (see `refuse_writes`).
fn persist_locked(state: &StoreState, path: &Path) -> Result<()> {
    if state.refuse_writes {
        bail!(
            "projects store on disk uses a newer schema; refusing to overwrite {}",
            path.display()
        );
    }
    if state.projects.is_empty()
        && state.assignments.is_empty()
        && state.never_materialize_roots.is_empty()
    {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
        };
    }
    let file = ProjectsFile {
        schema_version: SCHEMA_VERSION,
        projects: state.projects.clone(),
        assignments: state.assignments.clone(),
        never_materialize_roots: state.never_materialize_roots.clone(),
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create project store dir {}", parent.display()))?;
    crate::platform::filesystem::atomic_write(path, &serde_json::to_vec_pretty(&file)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Read and validate the structure. Missing file = empty state (first boot);
/// parse failures propagate, and `from_paths` decides the degradation policy
/// (log + empty state, boot is never blocked).
fn load_state(path: &Path) -> Result<StoreState> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(StoreState::default());
        }
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let file: ProjectsFile =
        serde_json::from_str(&content).with_context(|| format!("parse {}", path.display()))?;
    if file.schema_version > SCHEMA_VERSION {
        eprintln!(
            "[projects] {} uses schema {} newer than supported {}; writes are refused for this process",
            path.display(),
            file.schema_version,
            SCHEMA_VERSION
        );
        return Ok(StoreState {
            refuse_writes: true,
            ..StoreState::default()
        });
    }
    let mut projects = file.projects;
    projects.sort_by(|a, b| (a.position, &a.id).cmp(&(b.position, &b.id)));
    Ok(StoreState {
        projects,
        assignments: file.assignments,
        never_materialize_roots: file.never_materialize_roots,
        refuse_writes: false,
    })
}

impl ProjectStore {
    /// Open the default path (`~/.pinvou3/projects/projects.json`).
    pub fn boot() -> Self {
        Self::from_paths(crate::platform::paths::projects_store_path())
    }

    /// Entry point for tests/secondary instances: explicit persistence file.
    pub fn from_paths(path: PathBuf) -> Self {
        let state = match load_state(&path) {
            Ok(state) => state,
            Err(error) => {
                eprintln!("[projects] load {} failed: {error:#}", path.display());
                StoreState::default()
            }
        };
        Self {
            state: Arc::new(RwLock::new(state)),
            path: Arc::new(path),
        }
    }

    /// Project snapshot ordered by (position, id).
    pub fn list(&self) -> Vec<Project> {
        self.state.read().projects.clone()
    }

    pub fn get(&self, project_id: &str) -> Option<Project> {
        self.state
            .read()
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .cloned()
    }

    /// Assignment-resolution input: the explicit assignment entry (`None` =
    /// explicit move-out).
    pub fn assignment_of(&self, session_id: &str) -> Option<Option<String>> {
        self.state.read().assignments.get(session_id).cloned()
    }

    /// Full assignment snapshot (sent with list_projects by the command
    /// layer; feeds the frontend's group resolution).
    pub fn assignments_snapshot(&self) -> SessionAssignments {
        self.state.read().assignments.clone()
    }

    /// Snapshot of the anti-materialization exclusion list (canonical keys;
    /// sent with list_projects by the command layer; the management panel
    /// renders/revokes from it).
    pub fn never_materialize_roots(&self) -> Vec<String> {
        self.state.read().never_materialize_roots.clone()
    }

    /// Explicit anti-materialization (§3): `never = true` adds the folder
    /// (canonical key) to the exclusion list so ensure skips it afterwards;
    /// `false` revokes. Idempotent; returns the updated list. Affects only
    /// future auto-materialization; existing projects and session assignments
    /// are untouched.
    pub fn set_never_materialize(&self, root: &Path, never: bool) -> Result<Vec<String>> {
        if !root.is_absolute() {
            bail!(
                "never-materialize root must be absolute: {}",
                root.display()
            );
        }
        let key = root_key(root);
        let mut state = self.state.write();
        let contains = state.never_materialize_roots.contains(&key);
        if never == contains {
            return Ok(state.never_materialize_roots.clone());
        }
        if never {
            state.never_materialize_roots.push(key);
        } else {
            state.never_materialize_roots.retain(|entry| entry != &key);
        }
        persist_locked(&state, &self.path)?;
        Ok(state.never_materialize_roots.clone())
    }

    /// Session ids explicitly assigned to a project (member counting for the
    /// command layer).
    pub fn assigned_session_ids(&self, project_id: &str) -> Vec<String> {
        self.state
            .read()
            .assignments
            .iter()
            .filter_map(|(session_id, assigned)| {
                (assigned.as_deref() == Some(project_id)).then(|| session_id.clone())
            })
            .collect()
    }

    /// Create a project. roots may be empty (pure-label project); non-empty
    /// roots each pass the absoluteness/intra-set nesting validation
    /// (cross-project overlap is legal since §9.9).
    ///
    /// If the persist fails, the in-memory state has advanced while disk lags
    /// (persist runs last inside the lock; failures propagate without rolling
    /// back memory); an immediate in-process retry of create walks the same
    /// validate-and-persist path and self-heals on the next successful write.
    pub fn create_project(&self, name: String, roots: Vec<PathBuf>) -> Result<Project> {
        let name = validate_name(name)?;
        let roots = validate_roots(&roots)?;
        let mut state = self.state.write();
        let now = Utc::now();
        let position = state
            .projects
            .iter()
            .map(|project| project.position)
            .max()
            .unwrap_or(-1)
            + 1;
        let project = Project {
            id: generate_project_id(),
            name,
            roots,
            position,
            created_at: now,
            updated_at: now,
            origin: None,
            last_primary_root: None,
        };
        state.projects.push(project.clone());
        state
            .projects
            .sort_by(|a, b| (a.position, &a.id).cmp(&(b.position, &b.id)));
        persist_locked(&state, &self.path)?;
        Ok(project)
    }

    pub fn update_project(
        &self,
        project_id: &str,
        name: Option<String>,
        roots: Option<Vec<PathBuf>>,
    ) -> Result<Project> {
        let name = name.map(validate_name).transpose()?;
        let mut state = self.state.write();
        let Some(index) = state
            .projects
            .iter()
            .position(|project| project.id == project_id)
        else {
            bail!("project not found: {project_id}");
        };
        let roots = match roots {
            Some(roots) => Some(validate_roots(&roots)?),
            None => None,
        };
        let project = &mut state.projects[index];
        if let Some(name) = name {
            project.name = name;
        }
        if let Some(roots) = roots {
            project.roots = roots;
            demote_stale_primary_root(project);
        }
        project.updated_at = Utc::now();
        let updated = project.clone();
        persist_locked(&state, &self.path)?;
        Ok(updated)
    }

    /// Normalize and validate a roots payload without mutating state. Command
    /// layers run this before enumerating removed-root members so the diff
    /// compares canonical form against canonical form: the raw invoke payload
    /// is only key-folded (no symlink/ancestor resolution), so a no-op root
    /// edit spelled differently (macOS `/var` vs `/private/var`, symlinked
    /// home, autofs) would otherwise misjudge the edit as a removal and
    /// hard-expel every auto member under that root (review #484 B3).
    pub fn normalize_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
        validate_roots(roots)
    }

    /// Root replacement + auto-member expulsion in a single store transaction
    /// (review #484 B3): one lock, one persist. Previously the root replace
    /// and the expel were two persists; if the expel failed after the new
    /// roots had landed, the command returned Err but a retry recomputed
    /// `removed_roots` as empty (roots already replaced) and skipped the expel
    /// forever — the auto members would then be re-adopted by the next
    /// ensure. `expel_session_ids` is the command layer's enumeration of
    /// sessions under the removed roots; entry-less ids are written as
    /// explicit move-out (None), existing entries are untouched (tier-①
    /// semantics, same as `expel_unassigned_sessions`).
    pub fn update_project_and_expel(
        &self,
        project_id: &str,
        name: Option<String>,
        roots: Vec<PathBuf>,
        expel_session_ids: &[String],
    ) -> Result<Project> {
        let name = name.map(validate_name).transpose()?;
        let mut state = self.state.write();
        let Some(index) = state
            .projects
            .iter()
            .position(|project| project.id == project_id)
        else {
            bail!("project not found: {project_id}");
        };
        let roots = validate_roots(&roots)?;
        {
            let project = &mut state.projects[index];
            if let Some(name) = name {
                project.name = name;
            }
            project.roots = roots;
            demote_stale_primary_root(project);
            project.updated_at = Utc::now();
        }
        for session_id in expel_session_ids {
            if !state.assignments.contains_key(session_id) {
                state.assignments.insert(session_id.clone(), None);
            }
        }
        let updated = state.projects[index].clone();
        persist_locked(&state, &self.path)?;
        Ok(updated)
    }

    /// Record the project's remembered primary folder (§9.2): only roots
    /// members are accepted (folded-key comparison), foreign paths are
    /// rejected — the primary folder must be a directory inside the project's
    /// territory. Returns the updated project.
    pub fn set_last_primary_root(&self, project_id: &str, root: &Path) -> Result<Project> {
        let mut state = self.state.write();
        let Some(index) = state
            .projects
            .iter()
            .position(|project| project.id == project_id)
        else {
            bail!("project not found: {project_id}");
        };
        let display = root_display(root);
        let key = identity_key_of_display(&display);
        let is_member = state.projects[index]
            .roots
            .iter()
            .any(|existing| identity_key_of_display(existing) == key);
        if !is_member {
            bail!(
                "primary root must be one of the project roots: {}",
                root.display()
            );
        }
        let project = &mut state.projects[index];
        if project.last_primary_root.as_deref() == Some(display.as_path()) {
            return Ok(project.clone());
        }
        project.last_primary_root = Some(display);
        project.updated_at = Utc::now();
        let updated = project.clone();
        persist_locked(&state, &self.path)?;
        Ok(updated)
    }

    /// Delete a project: member sessions (explicit Some(pid) + the command
    /// layer's enumerated auto-grouped members `expel_session_ids`) are all
    /// written as explicit move-outs (None) — they stay in Ungrouped and do
    /// not revive with the folder's next auto-materialization; sessions
    /// created later in that folder have no assignment entry and auto-group
    /// as usual. Ids with existing entries (None = already moved out /
    /// Some(other) = explicitly assigned elsewhere) are not rewritten;
    /// tier-① semantics win. Sessions are never deleted.
    ///
    /// Intentional edge semantic (interaction after the §9.9 root-overlap
    /// legalization, locked by tests): the tombstone is global — if a removed
    /// project A's root is still referenced by a surviving project B, the
    /// auto members under that root are also written as None and will not be
    /// re-adopted by B's tier-② grouping. Deletion is the user's explicit
    /// statement ("these sessions leave grouping"), so automatic revival
    /// would overturn it; moving a session into B requires an explicit move.
    pub fn delete_project(
        &self,
        project_id: &str,
        expel_session_ids: &[String],
    ) -> Result<DeleteProjectReport> {
        let mut state = self.state.write();
        if !state
            .projects
            .iter()
            .any(|project| project.id == project_id)
        {
            bail!("project not found: {project_id}");
        }
        state.projects.retain(|project| project.id != project_id);
        let mut affected: Vec<String> = state
            .assignments
            .iter()
            .filter(|(_, assigned)| assigned.as_deref() == Some(project_id))
            .map(|(session_id, _)| session_id.clone())
            .collect();
        for session_id in expel_session_ids {
            if !state.assignments.contains_key(session_id) && !affected.contains(session_id) {
                affected.push(session_id.clone());
            }
        }
        for session_id in &affected {
            state.assignments.insert(session_id.clone(), None);
        }
        persist_locked(&state, &self.path)?;
        Ok(DeleteProjectReport {
            affected_session_ids: affected,
        })
    }

    /// Member expulsion for root removal (§4): among the command layer's
    /// enumerated "sessions under the removed roots", entry-less sessions are
    /// written as explicit move-outs (None) — they stay in Ungrouped,
    /// preventing tier-② grouping or ensure materialization from immediately
    /// overturning the removal; sessions with existing entries (explicitly
    /// assigned to this/another project, already moved out) are untouched,
    /// tier-① semantics win. Returns the number of newly written entries.
    pub fn expel_unassigned_sessions(&self, session_ids: &[String]) -> Result<usize> {
        let mut state = self.state.write();
        let mut changed = 0usize;
        for session_id in session_ids {
            if !state.assignments.contains_key(session_id) {
                state.assignments.insert(session_id.clone(), None);
                changed += 1;
            }
        }
        if changed > 0 {
            persist_locked(&state, &self.path)?;
        }
        Ok(changed)
    }

    /// Move a session's assignment (pure logical-layer write; never touches
    /// the session's working-directory binding).
    ///
    /// - `project_id = Some`: assignment target; `add_workspace_root` may
    ///   additionally adopt the session's working directory as a target root
    ///   (idempotently skipped when already covered by an existing root),
    ///   persisted together with the assignment under the same lock to avoid
    ///   a half-commit.
    /// - `project_id = None`: writes an explicit move-out entry so
    ///   auto-grouping cannot "revive" the session back into its original
    ///   project.
    pub fn move_session_to_project(
        &self,
        session_id: &str,
        project_id: Option<&str>,
        add_workspace_root: Option<&Path>,
    ) -> Result<MoveSessionOutcome> {
        let mut state = self.state.write();
        let mut added_root = None;
        if let Some(target_id) = project_id {
            if !state.projects.iter().any(|project| project.id == target_id) {
                bail!("project not found: {target_id}");
            }
            if let Some(workspace) = add_workspace_root {
                if !workspace.is_absolute() {
                    bail!(
                        "add_workspace_root must be absolute: {}",
                        workspace.display()
                    );
                }
                // Cross-project overlap is legalized (§9.9); only absoluteness
                // and canonicalize remain here; intra-set coverage/adoption is
                // handled below per the existing invariants.
                let owned_root = workspace.to_path_buf();
                let mut displays = validate_roots(std::slice::from_ref(&owned_root))?;
                let Some(display) = displays.pop() else {
                    bail!("add_workspace_root produced no canonical key");
                };
                let key = identity_key_of_display(&display);
                let Some(index) = state
                    .projects
                    .iter()
                    .position(|project| project.id == target_id)
                else {
                    bail!("project not found: {target_id}");
                };
                let project = &mut state.projects[index];
                // Intra-set check in both directions: workspace covered by an
                // existing root → idempotent skip; workspace is an ancestor of
                // existing roots → adopt the covered descendants (mirroring
                // auto-grouping's longest-root match). Letting an ancestor
                // through otherwise would break the intra-set no-nesting
                // invariant and make the next update's full validation report
                // "must not nest" forever, wedging the project's roots until
                // the file is repaired by hand.
                let existing_keys: Vec<String> = project
                    .roots
                    .iter()
                    .map(|root| identity_key_of_display(root))
                    .collect();
                let covered: Vec<usize> = existing_keys
                    .iter()
                    .enumerate()
                    .filter(|(_, existing_key)| key_is_same_or_nested(existing_key, &key))
                    .map(|(index, _)| index)
                    .collect();
                let already_covered = existing_keys
                    .iter()
                    .any(|existing_key| key_is_same_or_nested(&key, existing_key));
                if !covered.is_empty() && !already_covered {
                    project.roots = project
                        .roots
                        .drain(..)
                        .enumerate()
                        .filter(|(index, _)| !covered.contains(index))
                        .map(|(_, root)| root)
                        .collect();
                    project.roots.push(display.clone());
                    project.updated_at = Utc::now();
                    added_root = Some(display);
                } else if !already_covered {
                    project.roots.push(display.clone());
                    project.updated_at = Utc::now();
                    added_root = Some(display);
                }
            }
            state
                .assignments
                .insert(session_id.to_string(), Some(target_id.to_string()));
        } else {
            if let Some(workspace) = add_workspace_root {
                bail!(
                    "add_workspace_root requires a target project: {}",
                    workspace.display()
                );
            }
            state.assignments.insert(session_id.to_string(), None);
        }
        persist_locked(&state, &self.path)?;
        Ok(MoveSessionOutcome {
            project_id: project_id.map(str::to_string),
            added_root,
        })
    }

    /// Folder-project auto-materialization (Codex-client-style adoption,
    /// idempotent): for each input folder, reuse an existing materialized
    /// project anchored at it, otherwise create an `origin=folder` project
    /// named after the directory basename (§9.9: being referenced by another
    /// project does not count as covered; overlap is legal). Roots in the
    /// exclusion list (§3) are skipped. Non-absolute paths are reported
    /// per-root as Failed without blocking the rest. Input is deduplicated by
    /// key; the whole batch shares a single persist. Grouping rules are
    /// unchanged — a new project's roots let the existing tier-② root match
    /// naturally adopt the folder's sessions, while explicit move-out entries
    /// (written on project deletion) still suppress, so a recreated project
    /// only picks up new sessions.
    pub fn ensure_folder_roots(&self, roots: &[PathBuf]) -> Result<Vec<EnsureFolderOutcome>> {
        let mut state = self.state.write();
        let mut outcomes = Vec::with_capacity(roots.len());
        let mut seen_keys: Vec<String> = Vec::with_capacity(roots.len());
        let mut created_any = false;
        for root in roots {
            if !root.is_absolute() {
                outcomes.push(EnsureFolderOutcome::Failed {
                    reason: format!("folder root must be absolute: {}", root.display()),
                });
                continue;
            }
            let display = root_display(root);
            let key = identity_key_of_display(&display);
            if seen_keys.contains(&key) {
                continue;
            }
            seen_keys.push(key.clone());
            // Exclusion list (§3): the user explicitly said "never create a
            // project for this folder" — skip without producing an outcome
            // (same as input dedup); tombstones do not revive.
            if state.never_materialize_roots.contains(&key) {
                continue;
            }
            // Anchored reuse (§9.9): reuse only when a materialized project
            // **anchored at this folder** already exists (origin=folder and
            // roots contain exactly this path); being referenced by another
            // project as an attached/primary root does not count as covered —
            // the browse channel is always homed at the chosen folder,
            // overlap is legal, and a new same-named project is created.
            let anchored = state.projects.iter().find(|project| {
                project.origin.as_deref() == Some("folder")
                    && project
                        .roots
                        .iter()
                        .any(|existing| identity_key_of_display(existing) == key)
            });
            if let Some(project) = anchored {
                outcomes.push(EnsureFolderOutcome::Covered {
                    project_id: project.id.clone(),
                });
                continue;
            }
            let name = display
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| display.to_string_lossy().into_owned());
            // Folder projects created earlier in the same batch are already in
            // state.projects; later identical roots are deduped by seen_keys;
            // anchoring is judged by exact path, so nested inputs each
            // materialize.
            match validate_roots(std::slice::from_ref(root)) {
                Ok(displays) => {
                    let now = Utc::now();
                    let position = state
                        .projects
                        .iter()
                        .map(|project| project.position)
                        .max()
                        .unwrap_or(-1)
                        + 1;
                    let project = Project {
                        id: generate_project_id(),
                        name,
                        roots: displays,
                        position,
                        created_at: now,
                        updated_at: now,
                        origin: Some("folder".to_string()),
                        last_primary_root: None,
                    };
                    state.projects.push(project.clone());
                    state
                        .projects
                        .sort_by(|a, b| (a.position, &a.id).cmp(&(b.position, &b.id)));
                    created_any = true;
                    outcomes.push(EnsureFolderOutcome::Created { project });
                }
                Err(error) => outcomes.push(EnsureFolderOutcome::Failed {
                    reason: format!("{error:#}"),
                }),
            }
        }
        if created_any {
            persist_locked(&state, &self.path)?;
        }
        Ok(outcomes)
    }

    /// Directory rebind (broken-link repair channel): shift project roots
    /// under the `from` prefix to `to`. `from` matches against stored values
    /// (it may already be gone from disk); `to` is validated by the command
    /// layer as an existing canonical path. After rewriting, intra-set
    /// constraints are revalidated per project (the shift can create nesting
    /// within one project); failures roll back with an error (in-memory state
    /// not persisted); cross-project overlap is legal (§9.9) and not
    /// rechecked. Returns affected project ids.
    /// Idempotent: no matching root is a no-op.
    pub fn rebind_roots(&self, from: &Path, to: &Path) -> Result<Vec<String>> {
        let mut state = self.state.write();
        if from == to {
            return Ok(Vec::new());
        }
        // `to` is guaranteed by the command layer to exist; it is unified
        // into a canonical key here, consistent with the stored form; `from`
        // matching runs on folded keys (Windows folds case/separators, so a
        // case-only rename no longer misses), and the suffix is cut back from
        // the original root by component count, preserving subdirectory
        // casing. Rewriting happens on a copy and the in-memory state is only
        // committed after revalidation passes — on validation failure the
        // caller observes state consistent with disk.
        let to_key = root_display(to);
        let from_key = root_key(from);
        let mut candidate = state.projects.clone();
        let mut affected_projects = Vec::new();
        for project in candidate.iter_mut() {
            let mut changed = false;
            for root in project.roots.iter_mut() {
                let root_key_str = identity_key_of_display(root);
                if !key_is_same_or_nested(&root_key_str, &from_key) {
                    continue;
                }
                let suffix: PathBuf = root.components().skip(from.components().count()).collect();
                *root = if suffix.as_os_str().is_empty() {
                    to_key.clone()
                } else {
                    to_key.join(suffix)
                };
                changed = true;
            }
            if changed {
                project.updated_at = Utc::now();
                affected_projects.push(project.id.clone());
            }
        }
        if !affected_projects.is_empty() {
            for project in &candidate {
                validate_roots(&project.roots).context("rebind produced invalid project roots")?;
            }
            state.projects = candidate;
            persist_locked(&state, &self.path)?;
        }
        Ok(affected_projects)
    }

    /// Assignment resolution for "align to project" (§6/§9.7): tier-①
    /// explicit assignment (Some); an explicit move-out (None entry) blocks
    /// tier-②; tier-② matches the workspace's folded key against root
    /// prefixes, and multiple hits are adopted by the smallest position
    /// (same tiebreak as the frontend; projects are always ordered by
    /// (position, id), so `find` yields the smallest).
    pub fn resolve_session_project(&self, session_id: &str, workspace: &Path) -> Option<Project> {
        let state = self.state.read();
        match state.assignments.get(session_id) {
            Some(Some(project_id)) => {
                return state
                    .projects
                    .iter()
                    .find(|project| &project.id == project_id)
                    .cloned();
            }
            Some(None) => return None,
            None => {}
        }
        let key = identity_key_of_display(&root_display(workspace));
        state
            .projects
            .iter()
            .find(|project| {
                project
                    .roots
                    .iter()
                    .any(|root| key_is_same_or_nested(&key, &identity_key_of_display(root)))
            })
            .cloned()
    }

    /// Keychain shape for alignment: the primary slot = the session's own cwd
    /// (no door change, §9.2); additional roots = project roots minus cwd,
    /// order preserved (folded-key comparison). The foundation's normalize
    /// will normalize again; the storage layer writes it this way so "what is
    /// read" matches "what takes effect".
    pub fn keychain_for_workspace(cwd: &Path, project_roots: &[PathBuf]) -> Vec<PathBuf> {
        let cwd_display = root_display(cwd);
        let cwd_key = identity_key_of_display(&cwd_display);
        let mut out = vec![cwd_display];
        for root in project_roots {
            if identity_key_of_display(root) != cwd_key {
                out.push(root.clone());
            }
        }
        out
    }

    /// Session-delete hook: drop its assignment entry (including explicit
    /// move-out None entries). Returns whether anything changed; a persist
    /// failure is only logged — the in-memory state has advanced and the next
    /// mutation self-heals.
    pub fn forget_session(&self, session_id: &str) -> bool {
        let mut state = self.state.write();
        if state.assignments.remove(session_id).is_none() {
            return false;
        }
        if let Err(error) = persist_locked(&state, &self.path) {
            // No session id: the sidebar assignment map is not sensitive, but
            // CodeQL (deny gate) alerts on identifiers in logs, and the error
            // chain suffices for troubleshooting.
            eprintln!("[projects] persist after forget_session failed: {error:#}");
        }
        true
    }

    /// Boot reconciliation: prune assignment entries whose sessions no longer
    /// exist (a session may have been retired by the retention policy before
    /// the delete hook was registered). Returns the number pruned.
    pub fn retain_sessions(&self, existing: &std::collections::HashSet<String>) -> usize {
        let mut state = self.state.write();
        let before = state.assignments.len();
        state
            .assignments
            .retain(|session_id, _| existing.contains(session_id));
        let pruned = before.saturating_sub(state.assignments.len());
        if pruned > 0 {
            if let Err(error) = persist_locked(&state, &self.path) {
                eprintln!("[projects] persist after retain_sessions failed: {error:#}");
            }
        }
        pruned
    }
}
