//! ProjectStore:项目定义与会话归属映射的持久化、校验与原子写。
//!
//! 存储形态为单文件 JSON(`~/.pinvou3/projects/projects.json`):项目列表与
//! 归属映射同文件,tmp+rename 原子写一次覆盖两个视图。空状态不留文件
//! (sidecar 家族惯例);文件损坏时按空状态启动、下次变更自愈覆盖——归属是
//! 纯偏好数据,丢失等价回落隐式文件夹分组,无需 `code-session.json` 式的
//! 双保险 sidecar。

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

/// 项目实体:命名 + 文件夹势力范围(roots,canonicalized 绝对路径)+ 排序位。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub roots: Vec<PathBuf>,
    /// 侧栏手动排序位;新项目追加到末尾(max+1),同 Codex 的 position 语义。
    pub position: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Origin marker: `Some("folder")` = a project auto-materialized from a
    /// folder; `None` = created manually by the user. Provenance only (returned by
    /// GET, a store test anchor): no role in grouping or deletion semantics; kept
    /// across renames/added roots, with no name write-back sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// The project's remembered primary folder (§9.2/§9.3): the default cwd when
    /// creating a session at the project entry. Must be a roots member; written
    /// explicitly by the creation flow / manage panel; demoted to None when root
    /// changes push it out of roots (see `demote_stale_primary_root`), so a stale
    /// primary folder cannot keep serving as the project-entry cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_primary_root: Option<PathBuf>,
}

/// 归属映射值:`Some(project_id)` = 显式归属;`None` = 显式移出(跳过自动
/// 归组,直接回落隐式文件夹分组);无条目 = 未裁决,走自动归组。
pub type SessionAssignments = HashMap<String, Option<String>>;

/// Session ids explicitly assigned to `project_id` in assignments (one
/// implementation shared by the command layer's member counting and
/// delete_project's tombstone cleanup; takes the bare map so write paths
/// already holding the state write lock can reuse it).
fn explicit_assignments_of(assignments: &SessionAssignments, project_id: &str) -> Vec<String> {
    assignments
        .iter()
        .filter_map(|(session_id, assigned)| {
            (assigned.as_deref() == Some(project_id)).then(|| session_id.clone())
        })
        .collect()
}

/// 单文件持久化结构。schema_version 供未来结构演进识别:读到更新版本时
/// 按空状态降级启动,但置位拒绝后续写入(见 `StoreState::refuse_writes`),
/// 否则空状态 + 下次变更会把新结构文件降级覆盖写坏。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ProjectsFile {
    pub schema_version: u32,
    pub projects: Vec<Project>,
    #[serde(default)]
    pub assignments: SessionAssignments,
    /// Anti-materialization exclusion table (§3, canonical identity keys): the
    /// explicit "no more auto projects for this folder" declaration, viewable and
    /// revocable in the manage panel; older files lacking the key read it as an
    /// empty table.
    #[serde(default)]
    pub never_materialize_roots: Vec<String>,
}

// 2 (review #484 round-13 M1, requested in round 12): the persisted shape
// gained `Project.origin`, `Project.last_primary_root`, and
// `ProjectsFile.never_materialize_roots` under schema 1. Serde defaults make
// the 1→2 read free; the bump makes a pre-PR binary (update rollback,
// dual-version machine) refuse to write instead of silently stripping the
// exclusion list / origin / primary-root memory on its next persist.
const SCHEMA_VERSION: u32 = 2;

/// Report of a project deletion: members are all written as explicit
/// move-outs (tombstones, `Some(None)`) — they stay ungrouped and are not
/// revived by the folder's next auto-materialization; the sessions themselves
/// are never deleted. `affected_session_ids` currently has no frontend consumer
/// (handleDeleteProject discards the return value); it is a reserved field on
/// the protocol surface for a later prompt feature — until a consumer lands, do
/// not claim "the frontend will prompt" on its basis.
/// Entries are emitted in deterministic (sorted) order so cross-process
/// consumers (the CLI prints the report verbatim) get stable output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeleteProjectReport {
    pub affected_session_ids: Vec<String>,
}

/// Outcome of moving a session's project membership; the frontend uses it to
/// show "moved to the project (and added folder xx)".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MoveSessionOutcome {
    /// The project the session now belongs to; an explicit move-out is
    /// `None` as well.
    pub project_id: Option<String>,
    /// Folder that joined the target project as a side effect
    /// (canonicalized); `None` when nothing new joined.
    pub added_root: Option<PathBuf>,
}

/// Per-root outcome of `ensure_folder_roots`: callers distinguish created,
/// reused, and failed; a failure does not block the remaining roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EnsureFolderOutcome {
    /// Created a same-named folder project (origin=folder).
    Created { project: Project },
    /// A materialized project is already anchored on this folder (origin=folder,
    /// roots contains exactly this path); reused untouched.
    Covered { project_id: String },
    /// Cannot create (non-absolute path etc.); `reason` can go straight into the
    /// log.
    Failed { reason: String },
}

/// Round-8 review M3: a rebind failure must distinguish a genuine overlap
/// conflict (the localized `REBIND_ROOTS_CONFLICT` marker + retry dialog)
/// from an infrastructure failure — laundering a persist error into the
/// conflict marker told the user to resolve a "conflict" that no resolution
/// fixes, and combined with commit-before-persist the retry then
/// false-succeeded.
#[derive(Debug)]
pub enum RebindRootsError {
    Overlap(anyhow::Error),
    /// The candidate was valid but could not be persisted (restored, review
    /// #463 round-11 B2/T16): the command layer surfaces this with the typed
    /// `REBIND_ROOTS_PERSIST` marker instead of an untyped failure.
    Persist(anyhow::Error),
    Other(anyhow::Error),
}

impl std::fmt::Display for RebindRootsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RebindRootsError::Overlap(error) => {
                // The classification context already carries the sentence;
                // printing it again would duplicate it in {error:#} chains.
                write!(f, "{error}")
            }
            RebindRootsError::Persist(error) | RebindRootsError::Other(error) => {
                write!(f, "{error}")
            }
        }
    }
}

impl std::error::Error for RebindRootsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        let inner: &(dyn std::error::Error + 'static) = match self {
            RebindRootsError::Overlap(error)
            | RebindRootsError::Persist(error)
            | RebindRootsError::Other(error) => &**error,
        };
        inner.source()
    }
}

impl From<anyhow::Error> for RebindRootsError {
    fn from(error: anyhow::Error) -> Self {
        RebindRootsError::Other(error)
    }
}

#[derive(Debug, Default, Clone)]
struct StoreState {
    /// 恒按 (position, id) 有序,`list` 直接返回快照。
    projects: Vec<Project>,
    assignments: SessionAssignments,
    /// Anti-materialization exclusion table (canonical identity keys,
    /// deduplicated); ensure skips the roots it lists.
    never_materialize_roots: Vec<String>,
    /// 读到高于本进程 schema_version 的文件时置位:后续写入全部拒绝,
    /// 防止降级进程把新结构覆盖写坏。
    refuse_writes: bool,
}

/// 项目层存储。字段 `Arc` 包裹,整值克隆进 Tauri State 与删除钩子闭包。
#[derive(Clone)]
pub struct ProjectStore {
    state: Arc<RwLock<StoreState>>,
    path: Arc<PathBuf>,
    /// Process-local critical-section flag for directory rebinds (Minor 10):
    /// check-and-set completes under one lock, and the guard's Drop clears it.
    /// A rebind writes across three stores (projects store / session store /
    /// sidecars); two concurrent calls would interleave their write phases —
    /// each write is atomic and reruns converge, but serialization avoids the
    /// intermediate-state reports of the interleaved window.
    rebind_gate: Arc<parking_lot::Mutex<bool>>,
}

/// RAII token of `begin_rebind`: while held, other rebind calls and every
/// fenced project writer are rejected; Drop clears the flag. It holds only the
/// `Arc<Mutex<bool>>`, not a lock guard, so it is Send-safe across await
/// points; clearing happens in Drop, so error paths cannot leave a permanently
/// closed gate.
#[derive(Debug)]
pub struct RebindGate {
    flag: Arc<parking_lot::Mutex<bool>>,
}

impl Drop for RebindGate {
    fn drop(&mut self) {
        *self.flag.lock() = false;
    }
}

/// Same flag, entered from the writer side (`rebind_fence`). A distinct name
/// keeps intent legible at the call sites — a root-accepting writer is not
/// "beginning a rebind", it is refusing to commit into one — while sharing the
/// token's RAII semantics with `RebindGate` (not intra-doc linked: the gate
/// type is not re-exported past this module, so a link cannot resolve).
pub type RebindFence = RebindGate;

/// 进程内单调计数叠加纳秒时间戳生成项目 id:时间戳保证跨进程唯一,
/// 计数兜底同一时钟粒度(Windows 较粗)内连续创建的碰撞。
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

/// Demote a stale "primary folder" memory: after root changes, a
/// `last_primary_root` that is no longer a roots member would keep acting as
/// the project entry's default cwd (§9.3) even though the folder has already
/// left the project. Membership is judged by exact folded-key comparison, the
/// same as `set_last_primary_root`.
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

/// Diff two roots snapshots: old roots covered by no new root (neither equal
/// nor nested under one) are the roots removed this time (the §4
/// folder-removal semantics). The command layer uses this to enumerate the
/// auto-grouped members under those roots and write them as explicit
/// move-outs, so grouping/materialization cannot immediately "flip the removal
/// back".
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

/// Whether a session workspace is still covered by the (edited) new roots —
/// equal to or nested under one of them. A narrowing edit (old `[A]` → new
/// `[A/sub]`) can leave sessions under the removed old root `A` still inside
/// the new root's territory; those sessions are not move-out targets: a tier-①
/// explicit move-out is a user statement, not a side effect of editing the root
/// set (review #484 round-6 — `removed_roots` only checks "old root not covered
/// by the new roots"; copying that enumeration would also write members still
/// under a new root into explicit move-outs, permanently blocking tier-②
/// re-adoption).
pub fn workspace_covered_by_roots(workspace: &Path, roots: &[PathBuf]) -> bool {
    let key = identity_key_of_display(workspace);
    roots
        .iter()
        .any(|root| key_is_same_or_nested(&key, &identity_key_of_display(root)))
}

/// root 的展示形态:目录存在时用 fs::canonicalize(消 symlink),不存在时
/// 经最深已存在祖先解析(见 `resolve_through_existing_ancestor`)——目录被
/// After a directory moves away, the intra-set invariant checks must stay
/// decidable, and the form is idempotent over stored values (canonicalize(
/// canonical p) == p). The shared `platform_compat_path` normalization then
/// strips the `\\?\` verbatim prefix Windows canonicalize produces (identity
/// on other platforms) — the same convention as
/// `validate_codex_project_workspace`.
pub(crate) fn root_display(path: &Path) -> PathBuf {
    let canonical =
        std::fs::canonicalize(path).unwrap_or_else(|_| resolve_through_existing_ancestor(path));
    crate::platform::os::platform_compat_path(&canonical.to_string_lossy())
}

/// canonicalize does no partial resolution: a nonexistent leaf would leave
/// a symlinked ancestor (macOS `/var` vs `/private/var`) in its raw
/// spelling, misaligning the key domain against stored roots — both the
/// covered-skip and the intra-set dedup/nesting checks would go blind
/// (review #471 Major; the cross-project overlap rejection left with §9.9
/// and no longer consumes this). Walk up to the first existing ancestor,
/// canonicalize it, and lexically re-attach the missing tail so the
/// candidate keys into its parent's territory; symlink chains are digested
/// level by level. A fully missing path (e.g. an unmounted volume) falls
/// back to lexical absolutization, preserving the "still decidable after
/// the directory moved away" semantics.
fn resolve_through_existing_ancestor(path: &Path) -> PathBuf {
    let lexical = lexical_absolute(path);
    let mut missing: Vec<PathBuf> = Vec::new();
    let mut cursor: &Path = &lexical;
    loop {
        if cursor.exists() {
            if let Ok(mut base) = std::fs::canonicalize(cursor) {
                for component in missing.iter().rev() {
                    base.push(component);
                }
                return base;
            }
            break;
        }
        match (cursor.file_name(), cursor.parent()) {
            (Some(name), Some(parent)) => {
                missing.push(PathBuf::from(name));
                cursor = parent;
            }
            // 根/前缀等无可剥离的普通组件:保持词法形态。
            _ => break,
        }
    }
    lexical
}

/// Command-entry normalization of a rebind `from` (review #463 B1): the three
/// storage lanes match in different domains (this store resolves symlinked
/// ancestors; the codex/session lanes fold lexically), so an alias caller
/// (macOS `/var/x` vs the stored `/private/var/x`) would half-migrate.
/// Resolving once at the command entry pins every lane to the same form.
/// Idempotent for paths already in display form.
pub fn rebind_source_display(from: &Path) -> PathBuf {
    root_display(from)
}

/// Comparison key of a root: the display form folded through the shared
/// `filesystem_path_identity_key` — Windows folds separators and case
/// (`C:\Work` and `c:\work` are the same root); POSIX is case-sensitive and
/// kept as-is. Stored roots are already in display form at write time, so no
/// disk touch is needed — only the key fold.
///
/// Known residual (accepted edge): macOS defaults to a case-insensitive
/// APFS, but the shared helper deliberately does not fold case (a volume may
/// be configured case-sensitive; see platform/os/macos), so the same
/// directory spelled in two cases still counts as two roots. Folding here
/// would break case-sensitive volumes, so it is documented instead.
fn identity_key_of_display(path: &Path) -> String {
    crate::platform::os::filesystem_path_identity_key(&path.to_string_lossy())
}

/// Lock-free pure form of tier-② assignment resolution: the workspace's
/// folded key is prefix-matched same-or-nested against every project's roots;
/// projects stay sorted by (position, id), so `find` yields the smallest
/// position. Shared by `resolve_session_project` and the expulsion filter of
/// `update_project_and_expel`, keeping "the assignment read" and "the move-out
/// decision" on the same rule (review #484 round-11 M4).
/// Pre-computed per-project root key index for the prekeyed tier-② matcher
/// (round-33 MAJOR-1: the per-root key fold is pure string work on stored
/// display forms — no fs I/O — so building it under the read lock is fine).
fn root_key_index(state: &StoreState) -> Vec<Vec<String>> {
    state
        .projects
        .iter()
        .map(|project| {
            project
                .roots
                .iter()
                .map(|root| identity_key_of_display(root))
                .collect()
        })
        .collect()
}

fn tier2_project_of_prekeyed(
    state: &StoreState,
    root_keys: &[Vec<String>],
    key: &str,
) -> Option<Project> {
    state
        .projects
        .iter()
        .zip(root_keys)
        .find(|(_, keys)| keys.iter().any(|root| key_is_same_or_nested(key, root)))
        .map(|(project, _)| project.clone())
}

/// Batch form of `tier2_project_of` (round-19 minor 3): the expulsion filter
/// used to resolve tier-② per candidate, recomputing every project root's
/// folded key each time — precompute each project's root keys into an index
/// once, and only prefix comparisons remain per candidate. The rule is
/// byte-for-byte identical to the singular form. Round-21 should-fix 8: key
/// computation (one disk canonicalize) is split into `tier2_key_of` so the
/// expulsion path can finish the candidate keys **before** taking the store
/// write lock — previously 1000 candidates meant 1000 canonicalizes under the
/// write lock.
fn tier2_key_of(workspace: &Path) -> String {
    identity_key_of_display(&root_display(workspace))
}

/// Component-aware "same as, or nested under" on folded keys. The rule itself
/// now lives in `platform::os::path_identity_is_same_or_nested` (review #463
/// round-8 elegance: project-root validation, the codex rebind suffix matcher
/// and the command-layer nesting rejection each carried a hand-written copy,
/// and they had already drifted); only the projects-layer call site stays
/// here.
fn key_is_same_or_nested(key: &str, base: &str) -> bool {
    crate::platform::os::path_identity_is_same_or_nested(key, base)
}

/// 不触盘的绝对化:`.` 丢弃、`..` 回退一层、保留前缀(Unix 根 / Windows 盘符)。
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

/// Alias-equality for the rebind no-op guards (review #463 round-18 minor 8):
/// folded identity keys, so a case/spelling variant of the same directory is
/// the same path even when the raw spellings differ.
pub(crate) fn paths_are_alias_equal(a: &Path, b: &Path) -> bool {
    let key_a = crate::platform::os::filesystem_path_identity_key(&a.to_string_lossy());
    let key_b = crate::platform::os::filesystem_path_identity_key(&b.to_string_lossy());
    key_a.trim_end_matches('/') == key_b.trim_end_matches('/')
}

/// Root-cause prefixes of the overlap-family errors `validate_roots`
/// produces: single-sourced here and consumed by both the bail! sites below
/// and the command layer's preflight REBIND_ROOTS_CONFLICT partition, so a
/// wording change cannot silently degrade the conflict copy — the wording
/// pin drives every class through the real validator. Under §9.9 only the
/// intra-set classes (nest/duplicate) are producible; the overlap constant
/// stays in the partition list so a regression that re-legalizes nesting
/// still lands in the retry dialog, not the generic failure copy.
pub const ROOTS_NEST_CONFLICT: &str = "project roots must not nest";
pub const ROOTS_OVERLAP_CONFLICT: &str = "project root overlaps";
pub const ROOTS_DUPLICATE_CONFLICT: &str = "duplicate project root";
pub const REBIND_ROOTS_CONFLICT_PREFIXES: &[&str] = &[
    ROOTS_NEST_CONFLICT,
    ROOTS_OVERLAP_CONFLICT,
    ROOTS_DUPLICATE_CONFLICT,
];

/// Validate a set of roots and return their display form (canonicalized): the
/// duplicate and nesting checks both run on identity keys (decidable on Windows
/// after case/separator folding).
/// - must be absolute paths;
/// - no duplicates or mutual nesting within the set.
///
/// Cross-project overlap is out of scope here (§9.9 ruled it legal): the
/// `projects`/`skip_project_id` parameters were removed with §9.9 — historically
/// this spot rejected cross-project overlaps (the root cause of the
/// "Codex #22767 mis-grouping"); §9.9 dissolved the ambiguity with the
/// deterministic (position, id) assignment rule, and the legalization is locked
/// by tests such as `create_allows_overlap_with_other_projects`.
fn validate_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    // Round-21 M8: the same root-set cap the create intake enforces. Create
    // hard-caps at 64, but this validator gates the align/update/rebind
    // chains too — without it an over-64 project persisted a >64-entry
    // keychain snapshot (`keychain_for_workspace` copies all roots) while
    // the base silently truncates at delivery (store/UI ≠ engine, narrowing
    // direction), and `compose_workspace_roots`'s canonicalize+stat loop ran
    // unbounded per spawn before that truncation. The base's pub const is
    // used directly so the two caps cannot drift apart.
    if roots.len() > codewhale_core::MAX_WORKSPACE_ROOTS {
        bail!(
            "project declares {} roots; the cap is {} — an over-cap project \
             would persist a keychain the engine truncates at delivery",
            roots.len(),
            codewhale_core::MAX_WORKSPACE_ROOTS
        );
    }
    let mut displays = Vec::with_capacity(roots.len());
    let mut keys = Vec::with_capacity(roots.len());
    for root in roots {
        if !root.is_absolute() {
            bail!("project root must be absolute: {}", root.display());
        }
        // Round-11 M1 (review of the aligned keychain door): a project root
        // that normalizes to the filesystem root flows into member keychains
        // via `keychain_for_workspace` and makes the whole filesystem
        // writable. Checked on the canonical form too — a symlink whose
        // target is `/` must not pass on its lexical spelling. Ancestor
        // relationships to a session's cwd are `align`'s declared purpose
        // (§9.7) and stay allowed here.
        //
        // Round-18 M1b: the check also runs on the DISPLAY/identity form —
        // the form that actually lands in the store. A `..` spelling over a
        // non-existent prefix (`/pinvou3-ghost/..`) fails canonicalize, so
        // both raw and canonical fallback keep a Normal component and used
        // to pass, while `root_display`'s deepest-existing-ancestor
        // resolution folded the spelling down to `/` and stored the whole
        // filesystem as the project root.
        let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        let display = root_display(root);
        for spelling in [root.as_path(), canonical.as_path(), display.as_path()] {
            if !spelling
                .components()
                .any(|component| matches!(component, std::path::Component::Normal(_)))
            {
                bail!(
                    "project root {} normalizes to the filesystem root; \
                     a project whose root is the filesystem would make every member session's keychain the whole filesystem",
                    root.display()
                );
            }
        }
        let key = identity_key_of_display(&display);
        if keys.contains(&key) {
            bail!("{ROOTS_DUPLICATE_CONFLICT}: {}", root.display());
        }
        displays.push(display);
        keys.push(key);
    }
    for (index, (key, display)) in keys.iter().zip(displays.iter()).enumerate() {
        for (other, other_display) in keys.iter().zip(displays.iter()).skip(index + 1) {
            if key_is_same_or_nested(key, other) || key_is_same_or_nested(other, key) {
                bail!(
                    "{ROOTS_NEST_CONFLICT}: {} vs {}",
                    display.display(),
                    other_display.display()
                );
            }
        }
    }
    Ok(displays)
}

/// 原子落盘(共享 `atomic_write`:fsync + 唯一 tmp 名 + 备份语义)。空状态
/// 删除文件,不留空壳。读到更新 schema 时拒绝写入(见 `refuse_writes`)。
fn persist_locked(state: &StoreState, path: &Path) -> Result<()> {
    if state.refuse_writes {
        bail!(
            "projects store on disk uses a newer schema; refusing to overwrite {}",
            path.display()
        );
    }
    // Keep the file when only tombstones (explicit move-out Nones) remain:
    // otherwise it disappears on restart and the deleted projects' folders are
    // re-materialized by the next ensure, rising from the dead.
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
    crate::platform::filesystem::atomic_write_private(path, &serde_json::to_vec_pretty(&file)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// 读取并校验结构。文件不存在 = 空状态(首启);解析失败向上抛,由
/// `from_paths` 决定降级策略(日志 + 空状态,不阻断启动)。
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
    // The exclusion table's StoreState invariant is "canonical keys,
    // deduplicated" (writes go through set_never_materialize, which folds and
    // dedupes); a hand-edited or legacy file can carry duplicates, so re-pin
    // the invariant at load (review #484 n4).
    let mut never_materialize_roots = file.never_materialize_roots;
    let mut seen_keys = std::collections::HashSet::new();
    never_materialize_roots.retain(|key| seen_keys.insert(key.clone()));
    Ok(StoreState {
        projects,
        assignments: file.assignments,
        never_materialize_roots,
        refuse_writes: false,
    })
}

impl ProjectStore {
    /// 打开默认路径(`~/.pinvou3/projects/projects.json`)。
    pub fn boot() -> Self {
        Self::from_paths(crate::platform::paths::projects_store_path())
    }

    /// 测试/二级实例入口:显式指定持久化文件。
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
            rebind_gate: Arc::new(parking_lot::Mutex::new(false)),
        }
    }

    /// Enters the directory-rebind critical section (Minor 10): check-and-set
    /// is atomic; an already-set flag is rejected with the typed
    /// `REBIND_IN_PROGRESS` marker. The marker follows the stable-prefix
    /// convention of `REBIND_OLD_ROOT_EXISTS`/`REBIND_SESSIONS_BUSY` and is
    /// mapped to trilingual `uiProjects` copy in the frontend
    /// (`classifyRebindError`); the prose after the marker is backend
    /// diagnostics only and never reaches the user. The rejection path creates
    /// no guard; the token's Drop is the only clearing point, so error paths
    /// never leave a permanently closed gate.
    ///
    /// Known trade-off (review #463 round-8 minor): the gate is process-local
    /// and in-memory, so a second app instance is not serialized against this
    /// one — the two could interleave the same three write phases. Each
    /// individual write is atomic and a rerun converges, so the severity stays
    /// low; a cross-process lock would need a lock file and is out of scope.
    pub fn begin_rebind(&self) -> std::result::Result<RebindGate, String> {
        let mut flag = self.rebind_gate.lock();
        if *flag {
            return Err(
                "REBIND_IN_PROGRESS: another directory rebind is already running".to_string(),
            );
        }
        *flag = true;
        drop(flag);
        Ok(RebindGate {
            flag: Arc::clone(&self.rebind_gate),
        })
    }

    /// Enters the read/observational side of the same critical section: while a
    /// directory rebind is running, the root-accepting project writers must not
    /// commit. They validate the caller's roots against the *current* project
    /// table and add suffixes, so a write interleaved with an in-flight rebind
    /// can re-add a `from`-prefixed root or bind a session under `from` after
    /// the rebind's candidate snapshot — either way reintroducing the broken
    /// link the rebind is repairing (review #464 round-6 finding 6). Rejection
    /// reuses the `REBIND_IN_PROGRESS` marker, so the frontend's existing
    /// mapping covers it; the token is dropped as soon as the writer committed,
    /// which keeps the window to the write itself rather than the whole
    /// command.
    pub fn rebind_fence(&self) -> std::result::Result<RebindFence, String> {
        let mut flag = self.rebind_gate.lock();
        if *flag {
            return Err(
                "REBIND_IN_PROGRESS: another directory rebind is already running".to_string(),
            );
        }
        // Hold the gate for the writer's duration: mutually exclusive with
        // both `begin_rebind` and other fenced writers (check-and-set).
        *flag = true;
        drop(flag);
        Ok(RebindFence {
            flag: Arc::clone(&self.rebind_gate),
        })
    }

    /// 按 (position, id) 有序返回项目快照。
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

    /// 归属解析原料:显式归属条目(`None` = 显式移出)。仅测试用（生产路径走
    /// `assignments_snapshot`）。
    #[cfg(test)]
    pub fn assignment_of(&self, session_id: &str) -> Option<Option<String>> {
        self.state.read().assignments.get(session_id).cloned()
    }

    /// 全量归属快照(命令层随 list_projects 一并下发,前端分组解析用)。
    pub fn assignments_snapshot(&self) -> SessionAssignments {
        self.state.read().assignments.clone()
    }

    /// 显式归属到某项目的会话 id 列表(命令层统计成员数用)。
    pub fn assigned_session_ids(&self, project_id: &str) -> Vec<String> {
        explicit_assignments_of(&self.state.read().assignments, project_id)
    }

    /// Snapshot of the anti-materialization exclusion table (canonical keys;
    /// shipped by the command layer with list_projects; the manage panel shows and
    /// revokes from it).
    pub fn never_materialize_roots(&self) -> Vec<String> {
        self.state.read().never_materialize_roots.clone()
    }

    /// Display form of the exclusion table (for the manage panel's display and
    /// revocation). The table itself stores folded identity keys (on Windows a
    /// lower-case fold — shipping it directly would display `C:/Users/X` as
    /// `c:/users/x`), so the display form is reverse-looked up within known
    /// territory: project roots are the common source of exclusion entries; entries
    /// not found (hand-picked arbitrary folders) are returned as-is — the key
    /// remains the authoritative identity form, and the revocation path
    /// set_never_materialize folds before comparing, so both sides agree.
    pub fn never_materialize_display_roots(&self) -> Vec<String> {
        // Round-33 MAJOR-1: the canonicalize (root_display) runs OUTSIDE the
        // lock — snapshot the keys and the projects' root spellings under one
        // read, then resolve display forms lock-free (a hung automount must
        // not freeze every projects read).
        let (keys, root_spellings): (Vec<String>, Vec<std::path::PathBuf>) = {
            let state = self.state.read();
            (
                state.never_materialize_roots.clone(),
                state
                    .projects
                    .iter()
                    .flat_map(|project| project.roots.iter().cloned())
                    .collect(),
            )
        };
        keys.iter()
            .map(|key| {
                root_spellings
                    .iter()
                    .find(|root| &identity_key_of_display(root) == key)
                    .map(|root| root_display(root).to_string_lossy().to_string())
                    .unwrap_or_else(|| key.clone())
            })
            .collect()
    }

    /// Explicit anti-materialization (§3): `never = true` adds the folder
    /// (canonical key) to the exclusion table and later ensures skip it; `false`
    /// revokes. Idempotent; returns the updated table. Only future
    /// auto-materialization is affected — existing projects and session assignments
    /// are untouched.
    ///
    /// Persist FIRST, commit memory only on success (review #484 round-8 M1,
    /// same convention as `update_project_and_expel`): the idempotent
    /// early-return reads the same memory a commit-first order would have
    /// advanced, so a failed persist used to make every in-process retry
    /// return `Ok` over a disk still missing the entry.
    pub fn set_never_materialize(&self, root: &Path, never: bool) -> Result<Vec<String>> {
        if !root.is_absolute() {
            bail!(
                "never-materialize root must be absolute: {}",
                root.display()
            );
        }
        let key = identity_key_of_display(&root_display(root));
        let mut state = self.state.write();
        let contains = state.never_materialize_roots.contains(&key);
        if never == contains {
            return Ok(state.never_materialize_roots.clone());
        }
        let mut persisted = state.clone();
        if never {
            persisted.never_materialize_roots.push(key);
        } else {
            persisted
                .never_materialize_roots
                .retain(|entry| entry != &key);
        }
        persist_locked(&persisted, &self.path)?;
        *state = persisted;
        Ok(state.never_materialize_roots.clone())
    }

    /// Create a project. roots may be empty (a pure-label project); when
    /// non-empty, each root passes the absoluteness / intra-set invariant checks
    /// (dedup/nesting; cross-project overlap was ruled legal by §9.9 and is out of
    /// scope).
    ///
    /// Persist FIRST, commit memory only on success (round-21 M4, the same
    /// convention as `update_project_and_expel`): the old commit-first order
    /// advanced the project into memory before the disk write, and its docs
    /// defended that order with "a retry would first hit the in-memory overlap
    /// check" — once §9.9 deleted the cross-project overlap check, that
    /// justification no longer holds: a failed persist leaves a ghost project in
    /// memory, any later successful write in the same process commits it, and an
    /// immediate retry mints a duplicate project with the same roots. The persisted
    /// snapshot is committed back into memory only after the write succeeds; on
    /// failure memory and disk stay consistent.
    pub fn create_project(&self, name: String, roots: Vec<PathBuf>) -> Result<Project> {
        let name = validate_name(name)?;
        // Round-33 MAJOR-1: the per-root canonicalize runs BEFORE the write
        // lock (validate_roots touches fs for every spelling).
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
        let mut persisted = state.clone();
        persisted.projects.push(project.clone());
        persisted
            .projects
            .sort_by(|a, b| (a.position, &a.id).cmp(&(b.position, &b.id)));
        persist_locked(&persisted, &self.path)?;
        *state = persisted;
        Ok(project)
    }

    /// Update name/roots. Production roots changes go through
    /// `update_project_and_expel`'s persist-first line; in practice this method
    /// only carries renames.
    ///
    /// Known residual (round-32 minor 1, wording corrected): this rename
    /// path and `move_session_to_project` are the last commit-first
    /// mutators — NOT a convention; create_project itself went persist-first
    /// with a rollback pin in round-21 M4. A persist failure leaves memory
    /// ahead of disk until the next successful write; §9.9 deleted the
    /// cross-project check that once made a retry meaningful, so the
    /// durable fix is the six siblings' persist-first conversion (recorded
    /// as the next-wave pick).
    pub fn update_project(
        &self,
        project_id: &str,
        name: Option<String>,
        roots: Option<Vec<PathBuf>>,
    ) -> Result<Project> {
        let name = name.map(validate_name).transpose()?;
        // Round-34 minor 1: the roots validation (per-root canonicalize)
        // runs BEFORE the write lock — the last writer violating the
        // no-fs-under-lock discipline every sibling got (latent today: the
        // only production caller passes roots: None, but the method is pub).
        let roots = match roots {
            Some(roots) => Some(validate_roots(&roots)?),
            None => None,
        };
        let mut state = self.state.write();
        let Some(index) = state
            .projects
            .iter()
            .position(|project| project.id == project_id)
        else {
            bail!("project not found: {project_id}");
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

    /// Validates and normalizes a roots payload without touching state. The
    /// command layer runs this before enumerating "sessions under removed
    /// roots" so the diff compares canonical forms on both sides: the invoke
    /// payload only gets key folding (no symlink/ancestor resolution), and
    /// without this step a no-op edit spelled differently (macOS `/var` vs
    /// `/private/var`, symlinked home, autofs) would be misjudged as a
    /// removal, hard move-outs included (review #484 B3).
    pub fn normalize_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
        validate_roots(roots)
    }

    /// Root replacement + auto-member expulsion in a single store transaction
    /// (review #484 B3): one lock, one persist. Previously the two were two
    /// persists; if expulsion failed after the new roots landed, the command
    /// returned Err but a retry computed `removed_roots` as empty (roots were
    /// already replaced) and skipped expulsion forever — the next ensure would
    /// silently re-adopt those members. `expel_candidates` enumerates the
    /// command layer's "sessions under removed roots" with each session's cwd;
    /// entry-less ones are written as explicit move-outs (None) ONLY when they
    /// resolve to no project after the edit — one that still resolves against
    /// the post-edit state (this project's remaining roots, or another project
    /// still listing the shared root under the §9.9 overlap legalization) must
    /// stay entry-less so tier-② keeps grouping it there; a global tombstone on
    /// a shared root would silently ungroup a project this edit never touched
    /// (review #484 round-11 M4). Ones with existing entries are untouched
    /// (tier-① semantics, matching `delete_project`'s expel path — deletion
    /// stays the user's explicit statement, §9.9).
    ///
    /// Persist FIRST, commit memory only on success (review #484 M3, same
    /// convention as `rebind_roots`): the previous order mutated the in-memory
    /// roots before the write, so a failed persist left memory at the new
    /// roots over a disk still holding the old ones — an in-process retry
    /// recomputed `removed` against the mutated memory as empty and skipped
    /// the expulsion permanently.
    pub fn update_project_and_expel(
        &self,
        project_id: &str,
        name: Option<String>,
        roots: Vec<PathBuf>,
        expel_candidates: &[(String, PathBuf)],
    ) -> Result<Project> {
        let name = name.map(validate_name).transpose()?;
        // Round-21 should-fix 8: pre-key the expel candidates OUTSIDE the
        // write lock — `root_display` is a disk canonicalize, and at 1000
        // expel candidates that used to be 1000 canonicalizes held under
        // the store write lock. The cwd spellings are caller-supplied and
        // lock-independent.
        let candidate_keys: Vec<(String, String)> = expel_candidates
            .iter()
            .map(|(session_id, cwd)| (session_id.clone(), tier2_key_of(cwd)))
            .collect();
        // Round-33 MAJOR-1: the roots validation (per-root canonicalize) runs
        // in the same pre-lock block as the candidate keys.
        let roots = validate_roots(&roots)?;
        let mut state = self.state.write();
        let Some(index) = state
            .projects
            .iter()
            .position(|project| project.id == project_id)
        else {
            bail!("project not found: {project_id}");
        };
        // §9.9: cross-project root overlap is legal; only the intra-set
        // invariants (absolute paths, duplicates, nesting) are revalidated
        // (pre-lock, round-33 MAJOR-1). A folder project overlapping a manual
        // project is a designed legal state, and editing the manual project's
        // roots from the manage panel must not be rejected because of it.
        let mut persisted = state.clone();
        {
            let project = &mut persisted.projects[index];
            if let Some(name) = name {
                project.name = name;
            }
            project.roots = roots;
            demote_stale_primary_root(project);
            project.updated_at = Utc::now();
        }
        // The expulsion filter's tier-② resolution shares one root-key index across
        // candidates (computed once on the post-EDIT persisted state — removed roots'
        // keys vanish with the snapshot, the same semantics as per-candidate
        // resolution).
        let root_keys: Vec<Vec<String>> = persisted
            .projects
            .iter()
            .map(|project| {
                project
                    .roots
                    .iter()
                    .map(|root| identity_key_of_display(root))
                    .collect()
            })
            .collect();
        for (session_id, candidate_key) in &candidate_keys {
            if persisted.assignments.contains_key(session_id) {
                continue;
            }
            // Round-21 should-fix 8: pure prefix matching under the write
            // lock — the per-candidate disk canonicalize ran before the
            // lock was taken (see candidate_keys below).
            if tier2_project_of_prekeyed(&persisted, &root_keys, candidate_key).is_none() {
                persisted.assignments.insert(session_id.clone(), None);
            }
        }
        let updated = persisted.projects[index].clone();
        persist_locked(&persisted, &self.path)?;
        *state = persisted;
        Ok(updated)
    }

    /// Record the project's remembered primary folder (§9.2): only roots members
    /// are accepted (folded-key comparison); foreign paths are always rejected —
    /// the primary folder must be a directory within the project's territory.
    /// Returns the updated project.
    ///
    /// Persist FIRST, commit memory only on success (review #484 round-8 M1,
    /// same convention as `update_project_and_expel`): the idempotent
    /// early-return reads the same memory a commit-first order would have
    /// advanced, so a failed persist used to make every in-process retry
    /// return the in-memory value without ever writing it.
    pub fn set_last_primary_root(&self, project_id: &str, root: &Path) -> Result<Project> {
        // Round-33 MAJOR-1: resolve the display form BEFORE the write lock
        // (this runs on every project-channel create; a hung ancestor
        // resolution must not hold the store).
        let display = root_display(root);
        let key = identity_key_of_display(&display);
        let mut state = self.state.write();
        let Some(index) = state
            .projects
            .iter()
            .position(|project| project.id == project_id)
        else {
            bail!("project not found: {project_id}");
        };
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
        if state.projects[index].last_primary_root.as_deref() == Some(display.as_path()) {
            return Ok(state.projects[index].clone());
        }
        let mut persisted = state.clone();
        {
            let project = &mut persisted.projects[index];
            project.last_primary_root = Some(display);
            project.updated_at = Utc::now();
        }
        let updated = persisted.projects[index].clone();
        persist_locked(&persisted, &self.path)?;
        *state = persisted;
        Ok(updated)
    }

    /// Delete a project: sessions are only unbound (falling back to implicit
    /// grouping), never deleted. All members — explicit `Some(pid)` entries plus
    /// the command-enumerated auto-grouped members `expel_session_ids` — are
    /// written as explicit move-outs (None): they stay ungrouped and are not
    /// revived by the folder's next auto-materialization; sessions created in that
    /// folder afterwards carry no assignment entry and auto-group as usual. Ids
    /// that already have an entry (None = already moved out / Some(other) =
    /// explicitly assigned elsewhere) are not rewritten — tier-① semantics take
    /// precedence.
    ///
    /// Persist FIRST, commit memory only on success (review #484 round-5 M3,
    /// same convention as `update_project_and_expel`/`rebind_roots`): the
    /// previous order mutated memory before the write, so a failed persist
    /// left the project deleted in memory but alive on disk — and an
    /// in-process retry then failed with "project not found", making the
    /// deletion unrecoverable without a restart.
    ///
    /// Deliberate boundary semantics (the interaction left by §9.9's
    /// cross-project-overlap legalization, locked by the test
    /// `delete_on_a_shared_root_tombstones_globally_across_surviving_projects`
    /// — round-19 minor 5 added the previously missing shared-root leg): the
    /// tombstone is global — when a root of deleted project A is still referenced
    /// by surviving project B, the auto-members under that root are written as None
    /// too and cannot be re-adopted by B's tier-② grouping. Deletion is the user's
    /// explicit statement ("these sessions leave the grouping"); automatic revival
    /// would overturn it — moving into B requires an explicit move.
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
        // The report serializes across processes (the CLI prints it
        // verbatim); make the order deterministic instead of exposing the
        // HashMap's iteration order (carried from the merged base).
        affected.sort();
        let mut persisted = state.clone();
        persisted
            .projects
            .retain(|project| project.id != project_id);
        for session_id in &affected {
            persisted.assignments.insert(session_id.clone(), None);
        }
        persist_locked(&persisted, &self.path)?;
        *state = persisted;
        Ok(DeleteProjectReport {
            affected_session_ids: affected,
        })
    }

    /// 移动会话归属(纯逻辑层写;不触碰会话的工作目录绑定)。
    ///
    /// - `project_id = Some`:归属目标;`add_workspace_root` 可顺带把会话的
    ///   工作目录加为目标 root(已被现有 root 覆盖时幂等跳过),同一把锁内
    ///   与归属一并落盘,避免半提交。
    /// - `project_id = None`:写显式移出条目,阻止自动归组把会话"复活"回
    ///   原项目。
    ///
    /// Known residual (round-32 minor 1, wording corrected): commit-first
    /// like `update_project` — NOT a convention; create_project went
    /// persist-first with a rollback pin in round-21 M4. A persist failure
    /// leaves the assignment (and any incidentally added root) in memory
    /// ahead of disk; the round-26 minor-8 shape (an in-memory-only root
    /// visible to align/rebind) is the sharpest edge until the persist-first
    /// conversion lands (next-wave pick).
    pub fn move_session_to_project(
        &self,
        session_id: &str,
        project_id: Option<&str>,
        add_workspace_root: Option<&Path>,
    ) -> Result<MoveSessionOutcome> {
        // Round-33 MAJOR-1: the add-root's canonicalize (validate_roots on
        // the single-element slice) is resolved BEFORE the write lock; the
        // absoluteness rejection moves with it (pure lexical check).
        let add_root_display: Option<std::path::PathBuf> = match add_workspace_root {
            Some(workspace) => {
                if !workspace.is_absolute() {
                    bail!(
                        "add_workspace_root must be absolute: {}",
                        workspace.display()
                    );
                }
                let owned_root = workspace.to_path_buf();
                let mut displays = validate_roots(std::slice::from_ref(&owned_root))?;
                let Some(display) = displays.pop() else {
                    bail!("add_workspace_root produced no canonical key");
                };
                Some(display)
            }
            None => None,
        };
        let mut state = self.state.write();
        let mut added_root = None;
        if let Some(target_id) = project_id {
            let Some(index) = state
                .projects
                .iter()
                .position(|project| project.id == target_id)
            else {
                bail!("project not found: {target_id}");
            };
            if let Some(display) = &add_root_display {
                // For a single-element set the intra-set checks reduce to the
                // absoluteness check (§9.9; resolved pre-lock, round-33
                // MAJOR-1).
                let key = identity_key_of_display(display);
                let project = &mut state.projects[index];
                // 组内方向双查:workspace 被现有 root 覆盖 → 幂等跳过;workspace
                // 是现有 root 的祖先 → 收编被覆盖的后代(镜像自动归组的最长
                // root 匹配),否则放行祖先会破坏组内不嵌套不变量,并让下一次
                // update 的全量校验永远报 "must not nest",项目 roots 卡死到
                // 手工修文件为止。
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
                    // Capture the absorbed paths before the drain: a covered
                    // descendant that served as the remembered primary is
                    // about to leave `roots`, and stranding the memory would
                    // break "new conversation" on every later project-row
                    // entry (demote per §9.2 — the channel falls back to the
                    // first roots entry).
                    let covered_paths: Vec<PathBuf> = project
                        .roots
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| covered.contains(index))
                        .map(|(_, root)| root.clone())
                        .collect();
                    project.roots = project
                        .roots
                        .drain(..)
                        .enumerate()
                        .filter(|(index, _)| !covered.contains(index))
                        .map(|(_, root)| root)
                        .collect();
                    project.roots.push(display.clone());
                    if let Some(primary) = project.last_primary_root.as_ref() {
                        let primary_key = identity_key_of_display(primary);
                        if covered_paths
                            .iter()
                            .any(|path| identity_key_of_display(path) == primary_key)
                        {
                            project.last_primary_root = None;
                        }
                    }
                    project.updated_at = Utc::now();
                    added_root = Some(display.clone());
                } else if !already_covered {
                    // Round-29 M4: the cap fires only on whole-set
                    // validation and this plain-push arm pushed
                    // unconditionally — a 64-root project grew a 65th root
                    // here, persisting an over-cap project whose keychain
                    // the engine truncates at delivery (store/UI ≠ engine),
                    // and any later rebind touching it mis-mapped the cap
                    // error to the Overlap nesting dialog. The absorb arm
                    // above swaps covered descendants for one root (net
                    // non-growth), so only this arm needs the gate.
                    if project.roots.len() + 1 > codewhale_core::MAX_WORKSPACE_ROOTS {
                        bail!(
                            "adding this folder would push the project past the {}-root cap",
                            codewhale_core::MAX_WORKSPACE_ROOTS
                        );
                    }
                    project.roots.push(display.clone());
                    project.updated_at = Utc::now();
                    added_root = Some(display.clone());
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

    /// Pure candidate computation shared by [`plan_rebind_roots`] (the
    /// non-committing pre-flight) and [`rebind_roots`] (the commit): translates
    /// every root under `from` onto `to` and reports the affected project ids.
    /// Matching and the suffix cut both run in the resolved display domain.
    fn rebind_root_candidates(
        projects: &[Project],
        from_display: &Path,
        to_display: &Path,
    ) -> (Vec<Project>, Vec<String>) {
        // Round-34 R1: the display forms are resolved by the CALLERS before
        // any store lock is taken (`root_display` canonicalizes with an
        // existing-ancestor walk — under the lock it was the one fs-I/O
        // family the round-33 discipline missed; a hung automount `to` froze
        // every projects read and write). The forms' meaning is unchanged:
        // `to` is guaranteed to exist by the command layer and lands here in
        // the canonical display form used for storage; `from` matching runs
        // on the folded identity key of its resolved display form (Windows
        // folds case/separators, so a case-only rename still matches), and
        // the suffix is cut by the resolved component count so a
        // subdirectory keeps its original casing.
        let mut candidate = projects.to_vec();
        let mut affected_projects = Vec::new();
        for project in candidate.iter_mut() {
            let mut changed = false;
            for root in project.roots.iter_mut() {
                // Shared containment + suffix cut (round-8 review should-fix
                // 9): one platform predicate serves all three lanes.
                let Some(suffix) =
                    crate::platform::os::path_relative_suffix_under(root, &from_display)
                else {
                    continue;
                };
                *root = if suffix.as_os_str().is_empty() {
                    to_display.to_path_buf()
                } else {
                    to_display.join(suffix)
                };
                changed = true;
            }
            // The remembered primary folder moves with its directory (§9.2):
            // it is the project channel's default cwd, so stranding a
            // `/from`-spelled memory would break "new conversation" on every
            // later project-row entry with a nonexistent-path rejection —
            // minted by the very command that repairs broken links.
            if let Some(primary) = project.last_primary_root.as_mut() {
                if let Some(suffix) =
                    crate::platform::os::path_relative_suffix_under(primary, &from_display)
                {
                    *primary = if suffix.as_os_str().is_empty() {
                        to_display.to_path_buf()
                    } else {
                        to_display.join(suffix)
                    };
                }
            }
            if changed {
                project.updated_at = Utc::now();
                affected_projects.push(project.id.clone());
            }
        }
        (candidate, affected_projects)
    }

    /// Non-committing pre-flight for the rebind command (review #463 round-8
    /// M3): the session lanes now run before the project roots so an
    /// interrupted run still leaves the old root registered (hence
    /// badge-retryable), which means a root rewrite that cannot succeed —
    /// an intra-set nesting conflict, the one failure mode §9.9 leaves —
    /// must be detected BEFORE any session binding is touched. Validates the same candidate `rebind_roots` will commit and
    /// returns the project ids it would affect; nothing is written or
    /// persisted. `rebind_roots` revalidates under its write lock, so a
    /// concurrent project mutation cannot slip past the invariant.
    /// The whole candidate is checked intra-set: intra-set rejection has
    /// been enforced by every writer since before #464, so stored roots
    /// cannot carry legacy intra-set nesting the way the (once-legal)
    /// cross-project overlaps of #463's round-18 could.
    pub fn plan_rebind_roots(&self, from: &Path, to: &Path) -> Result<Vec<String>> {
        // Store-level defense in depth (the #464-ledger absolute class,
        // restored on the §9.9 rebase): the command entry rejects a relative
        // destination outright, but the store API must not silently cwd-
        // resolve one into a project relocation either — root_display's
        // missing-path fallback would otherwise launder `to` into an
        // absolute spelling under the process CWD. Not a partition prefix,
        // so the command layer surfaces it as an ordinary error.
        if !to.is_absolute() {
            bail!("rebind destination must be absolute: {}", to.display());
        }
        if paths_are_alias_equal(from, to) {
            return Ok(Vec::new());
        }
        // Round-34 R1: resolve both display forms BEFORE the read lock.
        let to_display = root_display(to);
        let from_display = root_display(from);
        let (candidate, affected_projects) = {
            let state = self.state.read();
            Self::rebind_root_candidates(&state.projects, &from_display, &to_display)
        };
        if affected_projects.is_empty() {
            return Ok(Vec::new());
        }
        // Plan and commit must agree (review #484 M2): the commit legalizes
        // cross-project overlap (§9.9), so the pre-flight asserts exactly what
        // rebind_roots revalidates under its write lock — the intra-set
        // invariants per AFFECTED project. This keeps the only failure mode
        // the commit can still hit (a translated root nesting inside one
        // project) a detected-before-anything-moved condition instead of a
        // mid-run rollback. Round-26 MAJOR 1: the loop is scoped to affected
        // projects (the base's `validate_rebind_candidates` scoping, restored)
        // — validate_roots' reject set now also carries this PR's cap and
        // fs-root classes, `load_state` revalidates nothing, so iterating the
        // whole store let one legacy bad-root project brick every UNRELATED
        // rebind with a diagnostic that project never produced.
        for project in &candidate {
            if !affected_projects.contains(&project.id) {
                continue;
            }
            validate_roots(&project.roots).context("rebind produced nesting project roots")?;
        }
        Ok(affected_projects)
    }

    /// Directory rebind (broken-link repair): translate project roots under
    /// the `from` prefix onto `to`. Matching runs in the resolved display
    /// domain, and the suffix is cut from each stored root by the RESOLVED
    /// `from` component count (review #463 B1): for an alias `from` (macOS
    /// `/var/x` resolving to `/private/var/x`) the resolved form is one
    /// component deeper, and cutting by the raw argument's count would keep
    /// an extra component, rewriting the root to `<to>/x` instead of `<to>`.
    /// `to` is validated by the command layer as an existing canonical path.
    /// After rewriting, the intra-set no-nesting invariant is revalidated per
    /// project (§9.9: a translated root landing on another project's
    /// territory is legal) — a nesting conflict fails the whole rebind with
    /// memory untouched and nothing persisted. Returns the affected project ids.
    ///
    /// Layering note (review #463 round-20 minor 12; updated round-13):
    /// the nesting guard lives at the COMMAND layer — this store fn
    /// performs no nesting validation, and only the command calls it
    /// today, so the guard is deliberately not duplicated here. The
    /// empty-`from` hazard is refused one layer below as well: the shared
    /// `path_relative_suffix_under` returns `None` for an empty base, so a
    /// raw empty `from` translates no root even without the command guard.
    ///
    /// Idempotent: no matching root is an empty Ok, not an error. The retry
    /// contract depends on this — a rerun after a partially failed run finds
    /// the roots already moved and must converge to a no-op while the command
    /// layer retries the remaining session writes. A `from` that never had
    /// any root is indistinguishable from a completed retry at this layer;
    /// the entry normalization in the command layer (resolving `from` once
    /// for all three storage lanes) is what prevents a silent half-migration.
    pub fn rebind_roots(&self, from: &Path, to: &Path) -> Result<Vec<String>, RebindRootsError> {
        // Same store-level defense as the plan lane (the #464-ledger
        // absolute class, restored on the §9.9 rebase): classify as Other —
        // a relative destination is an infrastructure shape, not a conflict
        // a re-pick dialog can fix.
        if !to.is_absolute() {
            return Err(RebindRootsError::Other(anyhow::anyhow!(
                "rebind destination must be absolute: {}",
                to.display()
            )));
        }
        // Round-34 R1: resolve both display forms BEFORE the write lock
        // (same hoist as the plan lane; also dedups the candidates' double
        // resolve the exclusion translation used to repeat).
        let to_display = root_display(to);
        let from_display = root_display(from);
        let mut state = self.state.write();
        // Alias-equal display forms are the same no-op (review #463 round-18
        // minor 8): the raw compare let a case/spelling variant bump
        // `updated_at` and persist a null rewrite.
        if paths_are_alias_equal(from, to) {
            return Ok(Vec::new());
        }
        // Rewrites happen on a candidate copy and commit only after
        // revalidation — on an overlap conflict the caller observes state
        // identical to disk.
        let (candidate, affected_projects) =
            Self::rebind_root_candidates(&state.projects, &from_display, &to_display);
        if !affected_projects.is_empty() {
            // §9.9: cross-project overlap is legal; only the intra-set
            // invariants are revalidated here, and only for AFFECTED projects
            // (round-26 MAJOR 1 — see plan_rebind_roots for the full scoping
            // rationale; the base scoped revalidation the same way so a
            // legacy untouched project cannot hard-block an unrelated
            // rebind). A genuine nesting conflict stays classified as Overlap
            // (the localized REBIND_ROOTS_CONFLICT marker and the retry
            // dialog are the right UX for it); other failures surface as
            // Other.
            for project in &candidate {
                if !affected_projects.contains(&project.id) {
                    continue;
                }
                validate_roots(&project.roots).map_err(|error| {
                    RebindRootsError::Overlap(
                        error.context("rebind produced nesting project roots"),
                    )
                })?;
            }
        }
        // The exclusion table moves with its directories too (review #484
        // round-8 m5): a moved never-materialize folder must keep suppressing
        // the NEW path, or the next ensure re-materializes the ghost the user
        // excluded — minted by the very command that repairs broken links.
        let translated_exclusions = Self::translate_never_materialize_candidates(
            &state.never_materialize_roots,
            &from_display,
            &to_display,
        );
        if affected_projects.is_empty() && translated_exclusions.is_none() {
            return Ok(affected_projects);
        }
        // Persist FIRST, commit the in-memory candidate only on success
        // (round-8 review M2, mirroring the codex lane): committing
        // before the write let a persist failure leave memory at `to`
        // over a disk still holding `from` — an in-process retry then
        // found no `from`-roots and reported success while the persisted
        // file stayed unmigrated.
        let mut persisted = state.clone();
        if !affected_projects.is_empty() {
            persisted.projects = candidate;
        }
        if let Some(exclusions) = translated_exclusions {
            persisted.never_materialize_roots = exclusions;
        }
        // Round-17 SF-6 (merged base): a disk failure keeps the Persist
        // classification — only a genuine intra-set conflict may wear the
        // Overlap marker, and the command layer's copy partition (the
        // `rebindRootsPersist` key) hangs off this variant.
        persist_locked(&persisted, &self.path).map_err(RebindRootsError::Persist)?;
        state.projects = persisted.projects;
        state.never_materialize_roots = persisted.never_materialize_roots;
        Ok(affected_projects)
    }

    /// Pure translation of the exclusion table for a rebind (review #484
    /// round-8 m5): the table stores folded identity keys rather than display
    /// paths, so containment and the cut both run in the key domain — keys
    /// equal to or nested under `from`'s key move onto `to`'s key. `None` =
    /// nothing matched, no rewrite needed.
    fn translate_never_materialize_candidates(
        keys: &[String],
        from_display: &Path,
        to_display: &Path,
    ) -> Option<Vec<String>> {
        // Round-34 R1: the display forms arrive pre-resolved (the caller
        // holds no store lock when it computes them).
        let from_key = identity_key_of_display(from_display);
        let to_key = identity_key_of_display(to_display);
        let from_key = from_key.trim_end_matches('/');
        let to_key = to_key.trim_end_matches('/');
        let mut changed = false;
        let translated = keys
            .iter()
            .map(|key| {
                let trimmed = key.trim_end_matches('/');
                // Same folded-prefix boundary as
                // `path_identity_is_same_or_nested`: a trailing separator is
                // noise, and the remainder must start with one so `/a/bc`
                // never counts as nested in `/a/b`.
                let suffix = if trimmed == from_key {
                    Some("")
                } else if trimmed.starts_with(from_key)
                    && trimmed[from_key.len()..].starts_with('/')
                {
                    Some(&trimmed[from_key.len()..])
                } else {
                    None
                };
                match suffix {
                    None => key.clone(),
                    Some(suffix) => {
                        changed = true;
                        format!("{to_key}{suffix}")
                    }
                }
            })
            .collect::<Vec<_>>();
        if changed { Some(translated) } else { None }
    }

    /// 会话删除钩子:摘除其归属条目(含显式移出的 None 条目)。返回是否
    /// 发生变更;落盘失败仅记日志,内存态已前进,下次变更自愈。
    pub fn forget_session(&self, session_id: &str) -> bool {
        let mut state = self.state.write();
        if state.assignments.remove(session_id).is_none() {
            return false;
        }
        if let Err(error) = persist_locked(&state, &self.path) {
            // 不带 session id:侧栏归属映射非敏感数据,但 CodeQL 对日志落
            // 标识符告警(deny 门),且排查只需错误链不需要 id。
            eprintln!("[projects] persist after forget_session failed: {error:#}");
        }
        true
    }

    /// Auto-materialization of folder projects (Codex-client-style adoption,
    /// idempotent): for each input folder, reuse the materialized project already
    /// anchored on it, otherwise create an `origin=folder` project named after the
    /// directory basename (§9.9: reference by another project does not count as
    /// covered — overlap is legal). Roots listed in the exclusion table (§3) are
    /// skipped outright. Non-absolute paths are reported as Failed per root without
    /// blocking the rest. Inputs are deduplicated by key; the whole batch shares
    /// one persist. Grouping rules are unchanged — the new project's roots let the
    /// existing tier-② root matching naturally adopt the sessions under that
    /// folder, while the explicit move-out entries (written when the project was
    /// deleted) still suppress, so the rebuilt project only receives sessions
    /// created afterwards.
    ///
    /// Persist FIRST, commit memory only on success (review #484 round-8 M1,
    /// same convention as `update_project_and_expel`): the batch builds on a
    /// clone and the previous order pushed created projects into live memory
    /// before the write, so a failed persist let the next ensure hit the
    /// anchored-reuse branch and report `Covered` forever — a session's tier-①
    /// assignment then pointed at a project id that vanished on restart,
    /// blocking tier-② re-adoption.
    pub fn ensure_folder_roots(&self, roots: &[PathBuf]) -> Result<Vec<EnsureFolderOutcome>> {
        // Round-33 MAJOR-1: every per-root fs resolution (root_display) runs
        // BEFORE the write lock — the previous shape canonicalized per root
        // (and re-stat'ed inside the anchored-reuse probe) while holding the
        // store, so one hung automount input froze all projects I/O. The
        // locked pass below consumes the pre-resolved (display, key) pairs.
        // Round-34 R1: the sibling intake's reject set runs in the same
        // pre-lock pass — the conversion had silently dropped the
        // filesystem-root rejection the old in-lock validate_roots provided,
        // so an OS folder picker returning the root volume materialized an
        // origin=folder project rooted at `/` (tier-② adopted sessions into
        // it and align wrote whole-disk keychains). fs-root on all three
        // spellings, exactly like validate_roots.
        let mut pre_resolved: Vec<(std::path::PathBuf, String)> = Vec::with_capacity(roots.len());
        let mut outcomes_pre: Vec<EnsureFolderOutcome> = Vec::new();
        for root in roots {
            if !root.is_absolute() {
                outcomes_pre.push(EnsureFolderOutcome::Failed {
                    reason: format!("folder root must be absolute: {}", root.display()),
                });
                continue;
            }
            let rejects_fs_root = |spelling: &std::path::Path| {
                !spelling
                    .components()
                    .any(|component| matches!(component, std::path::Component::Normal(_)))
            };
            let display = root_display(root);
            if rejects_fs_root(root)
                || rejects_fs_root(&display)
                || root
                    .canonicalize()
                    .map(|canonical| rejects_fs_root(&canonical))
                    .unwrap_or(false)
            {
                outcomes_pre.push(EnsureFolderOutcome::Failed {
                    reason: format!(
                        "folder root {} normalizes to the filesystem root; a project rooted there would make every member session's keychain the whole filesystem",
                        root.display()
                    ),
                });
                continue;
            }
            let key = identity_key_of_display(&display);
            if pre_resolved.iter().any(|(_, existing)| existing == &key) {
                continue;
            }
            pre_resolved.push((display, key));
        }
        let mut state = self.state.write();
        let mut persisted = state.clone();
        let mut outcomes = outcomes_pre;
        let mut created_any = false;
        for (display, key) in pre_resolved {
            let display = display.clone();
            let key = key.clone();
            // Exclusion table (§3): the user has explicitly declared "no project for
            // this folder" — skipped silently (the same shape as input deduplication);
            // the tombstone is not revived.
            if persisted.never_materialize_roots.contains(&key) {
                continue;
            }
            // Anchored reuse (§9.9): reuse only when a materialized project **anchored
            // on this folder** already exists (origin=folder and roots contains exactly
            // this path); being referenced by another project as an extra/primary root
            // does not count as covered — the browse channel always homes on the selected
            // folder, overlap is legal, so a same-named project is created.
            let anchored = persisted.projects.iter().find(|project| {
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
            // Folder projects created earlier in the batch are already in
            // persisted.projects; identical roots are deduplicated by seen_keys;
            // anchoring is decided on the exact path, so nested inputs each materialize
            // their own project.
            // The display form was already validated + resolved pre-lock
            // (round-33 MAJOR-1); the in-lock revalidation is gone.
            {
                let displays = vec![display.clone()];
                {
                    let now = Utc::now();
                    let position = persisted
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
                    persisted.projects.push(project.clone());
                    persisted
                        .projects
                        .sort_by(|a, b| (a.position, &a.id).cmp(&(b.position, &b.id)));
                    created_any = true;
                    outcomes.push(EnsureFolderOutcome::Created { project });
                }
            }
        }
        if created_any {
            persist_locked(&persisted, &self.path)?;
            *state = persisted;
        }
        Ok(outcomes)
    }

    /// Assignment resolution for "align to project" (§6/§9.7): tier-① explicit
    /// assignment (Some); an explicit move-out (a None entry) blocks tier-②;
    /// tier-② matches the workspace's folded key against roots prefixes, multiple
    /// hits take the smallest position (the frontend's same tiebreak; projects stay
    /// sorted by (position, id), so `find` is already the minimum).
    pub fn resolve_session_project(&self, session_id: &str, workspace: &Path) -> Option<Project> {
        // Round-33 MAJOR-1: pre-key the workspace outside the lock — the
        // canonicalize inside tier2_project_of used to run under the read
        // lock (one hung input path froze every align/grouping read).
        let workspace_key = tier2_key_of(workspace);
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
        tier2_project_of_prekeyed(&state, &root_key_index(&state), &workspace_key)
    }

    /// Keychain shape for alignment: the main slot = the session's own cwd (no
    /// door switch, §9.2); additional roots = the project's roots minus the cwd,
    /// order preserved (folded-key comparison). The base's normalize will normalize
    /// once more; the store layer writes in this shape so "what is read" matches
    /// "what takes effect".
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

    /// 启动对账:剔除归属表中已不存在的会话条目(会话可能在删除钩子注册
    /// 前已被保留策略淘汰)。返回剔除数量。
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
