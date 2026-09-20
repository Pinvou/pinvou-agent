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
}

/// 归属映射值:`Some(project_id)` = 显式归属;`None` = 显式移出(跳过自动
/// 归组,直接回落隐式文件夹分组);无条目 = 未裁决,走自动归组。
pub type SessionAssignments = HashMap<String, Option<String>>;

/// assignments 里显式归属 `project_id` 的会话 id（命令层统计与 delete_project
/// 的解绑清理共用同一实现；吃裸 map，便于写路径在已持 state 写锁时复用）。
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
}

const SCHEMA_VERSION: u32 = 1;

/// 移动归属的结果:前端据此提示"已加入项目(并添加了文件夹 xx)"。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MoveSessionOutcome {
    /// 本次顺带加入目标项目的文件夹(canonicalized);未新增为 None。
    pub added_root: Option<PathBuf>,
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
                write!(f, "rebind produced overlapping project roots: {error}")
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
/// token's RAII semantics with [`RebindGate`].
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

/// root 的展示形态:目录存在时用 fs::canonicalize(消 symlink),不存在时
/// 经最深已存在祖先解析(见 `resolve_through_existing_ancestor`)——目录被
/// 移走后 overlap 校验仍需可判定,且形态对已存值幂等(canonicalize(
/// canonical p) == p)。再经共享的 `platform_compat_path` 归一,剥掉
/// Windows canonicalize 产生的 `\\?\` verbatim 前缀(非 Windows 为恒等
/// 映射),与 `validate_codex_project_workspace` 的既有约定同源。
pub(crate) fn root_display(path: &Path) -> PathBuf {
    let canonical =
        std::fs::canonicalize(path).unwrap_or_else(|_| resolve_through_existing_ancestor(path));
    crate::platform::os::platform_compat_path(&canonical.to_string_lossy())
}

/// canonicalize 不做部分解析:不存在的叶子会让 symlink 化的祖先(macOS 的
/// `/var` → `/private/var`)保持原写法,与已存 root 的键域错位——covered-skip
/// 与跨项目重叠拒绝会双双失明(评审 #471 Major)。沿祖先上溯到第一个存在的
/// 目录,对它 canonicalize,再把不存在的尾巴词法接回,候选由此键入其父目录
/// 的势力范围;symlink 链同样被 canonicalize 逐级消化。全链不存在(如未挂载
/// 卷)退回词法绝对化,维持「目录被移走后仍可判定」的原语义。
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

/// 校验一组 roots 并返回展示形态(canonicalized):重的判定与嵌套判定都在
/// 身份键上进行(Windows 折叠大小写/分隔符后可判定)。
/// - 必须是绝对路径;
/// - 组内不得重复或互相嵌套;
/// - 不得与其它项目(skip_project_id 之外)的任何 root 重复或嵌套——自动
///   归组按 root 前缀匹配,跨项目重叠会让归属二义(Codex #22767 错归组的根源)。
fn validate_roots(
    projects: &[Project],
    skip_project_id: Option<&str>,
    roots: &[PathBuf],
) -> Result<Vec<PathBuf>> {
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
    for project in projects {
        if Some(project.id.as_str()) == skip_project_id {
            continue;
        }
        for existing in &project.roots {
            let existing_key = identity_key_of_display(existing);
            for (key, display) in keys.iter().zip(displays.iter()) {
                if key_is_same_or_nested(key, &existing_key)
                    || key_is_same_or_nested(&existing_key, key)
                {
                    bail!(
                        "project root overlaps project '{}' ({} vs {})",
                        project.name,
                        existing.display(),
                        display.display()
                    );
                }
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
    if state.projects.is_empty() && state.assignments.is_empty() {
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
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create project store dir {}", parent.display()))?;
    crate::platform::filesystem::atomic_write(path, &serde_json::to_vec_pretty(&file)?)
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
    Ok(StoreState {
        projects,
        assignments: file.assignments,
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

    /// 仅测试用断言原料（生产路径走 `list` / `assignments_snapshot`）。
    #[cfg(test)]
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

    /// 创建项目。roots 可为空(纯标签项目);非空时逐个过绝对性/重叠校验。
    ///
    /// 落盘失败时内存态已前进而磁盘滞后(persist 在锁内最后执行,失败向上
    /// 抛,不回滚内存);同一进程内立即重试 create 会先撞内存重叠校验(磁盘
    /// 还是旧内容)——已知语义,由下一次成功写盘自愈。
    pub fn create_project(&self, name: String, roots: Vec<PathBuf>) -> Result<Project> {
        let name = validate_name(name)?;
        let mut state = self.state.write();
        let roots = validate_roots(&state.projects, None, &roots)?;
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
            Some(roots) => Some(validate_roots(&state.projects, Some(project_id), &roots)?),
            None => None,
        };
        let project = &mut state.projects[index];
        if let Some(name) = name {
            project.name = name;
        }
        if let Some(roots) = roots {
            project.roots = roots;
        }
        project.updated_at = Utc::now();
        let updated = project.clone();
        persist_locked(&state, &self.path)?;
        Ok(updated)
    }

    pub fn delete_project(&self, project_id: &str) -> Result<()> {
        let mut state = self.state.write();
        let Some(index) = state
            .projects
            .iter()
            .position(|project| project.id == project_id)
        else {
            bail!("project not found: {project_id}");
        };
        state.projects.remove(index);
        // 只清 Some(pid) 条目;显式移出条目(None)的语义是"不进任何项目",
        // 与项目存亡无关,保留。被清掉的会话回落自动/隐式分组。
        let affected = explicit_assignments_of(&state.assignments, project_id);
        for session_id in &affected {
            state.assignments.remove(session_id);
        }
        persist_locked(&state, &self.path)?;
        Ok(())
    }

    /// 移动会话归属(纯逻辑层写;不触碰会话的工作目录绑定)。
    ///
    /// - `project_id = Some`:归属目标;`add_workspace_root` 可顺带把会话的
    ///   工作目录加为目标 root(已被现有 root 覆盖时幂等跳过),同一把锁内
    ///   与归属一并落盘,避免半提交。
    /// - `project_id = None`:写显式移出条目,阻止自动归组把会话"复活"回
    ///   原项目。
    pub fn move_session_to_project(
        &self,
        session_id: &str,
        project_id: Option<&str>,
        add_workspace_root: Option<&Path>,
    ) -> Result<MoveSessionOutcome> {
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
            if let Some(workspace) = add_workspace_root {
                if !workspace.is_absolute() {
                    bail!(
                        "add_workspace_root must be absolute: {}",
                        workspace.display()
                    );
                }
                // 单元素集组内校验退化为此路径自身的绝对性;跨项目重叠在此
                // 一并拦截(错误信息指向冲突项目)。
                let owned_root = workspace.to_path_buf();
                let mut displays = validate_roots(
                    &state.projects,
                    Some(target_id),
                    std::slice::from_ref(&owned_root),
                )?;
                let Some(display) = displays.pop() else {
                    bail!("add_workspace_root produced no canonical key");
                };
                let key = identity_key_of_display(&display);
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
        Ok(MoveSessionOutcome { added_root })
    }

    /// Pure candidate computation shared by [`plan_rebind_roots`] (the
    /// non-committing pre-flight) and [`rebind_roots`] (the commit): translates
    /// every root under `from` onto `to` and reports the affected project ids.
    /// Matching and the suffix cut both run in the resolved display domain.
    fn rebind_root_candidates(
        projects: &[Project],
        from: &Path,
        to: &Path,
    ) -> (Vec<Project>, Vec<String>) {
        // `to` is guaranteed to exist by the command layer; normalize it to
        // the canonical display form used for storage. `from` matching runs on
        // the folded identity key of its resolved display form (Windows folds
        // case/separators, so a case-only rename still matches), and the
        // suffix is cut by the resolved component count so a subdirectory
        // keeps its original casing.
        let to_display = root_display(to);
        let from_display = root_display(from);
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
                    to_display.clone()
                } else {
                    to_display.join(suffix)
                };
                changed = true;
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
    /// an overlap conflict — must be detected BEFORE any session binding is
    /// touched. Validates the same candidate `rebind_roots` will commit and
    /// returns the project ids it would affect; nothing is written or
    /// persisted. `rebind_roots` revalidates under its write lock, so a
    /// concurrent project mutation cannot slip past the invariant.
    /// Scoped revalidation (review #463 round-10 minor 5): only the projects
    /// this rebind actually touches are validated against the whole
    /// candidate — a pre-existing overlap between two untouched legacy
    /// projects (load_state revalidates nothing) must not hard-block an
    /// unrelated rebind with a conflict whose copy cannot help.
    fn validate_rebind_candidates(
        candidate: &[Project],
        affected_projects: &[String],
    ) -> Result<()> {
        for project in candidate.iter().filter(|project| {
            affected_projects
                .iter()
                .any(|affected| affected == &project.id)
        }) {
            validate_roots(candidate, Some(&project.id), &project.roots)?;
        }
        Ok(())
    }

    pub fn plan_rebind_roots(&self, from: &Path, to: &Path) -> Result<Vec<String>> {
        if from == to {
            return Ok(Vec::new());
        }
        let (candidate, affected_projects) = {
            let state = self.state.read();
            Self::rebind_root_candidates(&state.projects, from, to)
        };
        if affected_projects.is_empty() {
            return Ok(Vec::new());
        }
        Self::validate_rebind_candidates(&candidate, &affected_projects)
            .context("rebind produced overlapping project roots")?;
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
    /// After rewriting, overlap invariants are revalidated per project — a
    /// translated root may collide with another project's territory, in which
    /// case the whole rebind fails and rolls back (memory untouched, nothing
    /// persisted). Returns the affected project ids.
    ///
    /// Idempotent: no matching root is an empty Ok, not an error. The retry
    /// contract depends on this — a rerun after a partially failed run finds
    /// the roots already moved and must converge to a no-op while the command
    /// layer retries the remaining session writes. A `from` that never had
    /// any root is indistinguishable from a completed retry at this layer;
    /// the entry normalization in the command layer (resolving `from` once
    /// for all three storage lanes) is what prevents a silent half-migration.
    pub fn rebind_roots(&self, from: &Path, to: &Path) -> Result<Vec<String>, RebindRootsError> {
        let mut state = self.state.write();
        if from == to {
            return Ok(Vec::new());
        }
        // Rewrites happen on a candidate copy and commit only after
        // revalidation — on an overlap conflict the caller observes state
        // identical to disk.
        let (candidate, affected_projects) =
            Self::rebind_root_candidates(&state.projects, from, to);
        if !affected_projects.is_empty() {
            Self::validate_rebind_candidates(&candidate, &affected_projects).map_err(|error| {
                RebindRootsError::Overlap(
                    error.context("rebind produced overlapping project roots"),
                )
            })?;
            // Persist FIRST, commit the in-memory candidate only on success
            // (round-8 review M2, mirroring the codex lane): committing
            // before the write let a persist failure leave memory at `to`
            // over a disk still holding `from` — an in-process retry then
            // found no `from`-roots and reported success while the persisted
            // file stayed unmigrated.
            let mut persisted = state.clone();
            persisted.projects = candidate;
            persist_locked(&persisted, &self.path).map_err(RebindRootsError::Persist)?;
            state.projects = persisted.projects;
        }
        Ok(affected_projects)
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
