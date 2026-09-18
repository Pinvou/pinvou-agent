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
    /// 来源标识:`Some("folder")` = 按文件夹自动物化的项目;`None` = 用户手工
    /// 创建。仅作数据溯源(GET 返回、store 测试锚点),不参与分组判定与删除
    /// 语义;用户改名/加根后保留原值,不做名字回写同步。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// 项目记住的主文件夹(§9.2/§9.3):项目入口新建会话时的默认 cwd。必须是
    /// roots 成员;由创建流程/管理面板显式写入;root 更替把该根挤出 roots 时
    /// 降级为 None(见 `demote_stale_primary_root`),避免失效的主文件夹继续
    /// 充当项目入口 cwd。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_primary_root: Option<PathBuf>,
}

/// 归属映射值:`Some(project_id)` = 显式归属;`None` = 显式移出(跳过自动
/// 归组,直接回落隐式文件夹分组);无条目 = 未裁决,走自动归组。
pub type SessionAssignments = HashMap<String, Option<String>>;

/// 单文件持久化结构。schema_version 供未来结构演进识别:读到更新版本时
/// 按空状态降级启动,但置位拒绝后续写入(见 `StoreState::refuse_writes`),
/// 否则空状态 + 下次变更会把新结构文件降级覆盖写坏。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ProjectsFile {
    pub schema_version: u32,
    pub projects: Vec<Project>,
    #[serde(default)]
    pub assignments: SessionAssignments,
    /// 反物化排除表(§3,canonical 身份键):用户显式声明「此文件夹不再自动
    /// 建项目」;可在管理面板查看与撤销。旧文件缺该键时读为空表。
    #[serde(default)]
    pub never_materialize_roots: Vec<String>,
}

const SCHEMA_VERSION: u32 = 1;

/// 删除项目的结果汇报:受影响会话只被解绑(回落隐式分组),永不删除。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeleteProjectReport {
    pub affected_session_ids: Vec<String>,
}

/// 移动归属的结果:前端据此提示"已加入项目(并添加了文件夹 xx)"。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MoveSessionOutcome {
    pub project_id: Option<String>,
    /// 本次顺带加入目标项目的文件夹(canonicalized);未新增为 None。
    pub added_root: Option<PathBuf>,
}

/// `ensure_folder_roots` 的单根结果:调用方据此区分新建、复用与冲突,冲突不
/// 阻断其余根。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EnsureFolderOutcome {
    /// 新建了同名文件夹项目(origin=folder)。
    Created { project: Project },
    /// 已有物化项目锚定在该文件夹(origin=folder 且 roots 恰含此路径),复用不动。
    Covered { project_id: String },
    /// 无法创建(非绝对路径等);`reason` 可直接进日志。
    Failed { reason: String },
}

#[derive(Debug, Default)]
struct StoreState {
    /// 恒按 (position, id) 有序,`list` 直接返回快照。
    projects: Vec<Project>,
    assignments: SessionAssignments,
    /// 反物化排除表(canonical 身份键,已去重);ensure 跳过其中列出的根。
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

/// 降级已失效的「主文件夹」记忆:root 更替后,若 `last_primary_root` 已不再是
/// roots 成员,它会继续充当项目入口的默认 cwd(§9.3),而这个文件夹其实已经
/// 离开项目。成员判定与 `set_last_primary_root` 同用折叠键精确比较。
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

/// 差集两份 roots 快照:旧 roots 中未被任何新 root 覆盖(既不相等也不嵌套于
/// 其下)的即为本次被移除的根(§4 文件夹移除语义)。命令层据此枚举这些根下
/// 的自动归组成员,写成显式移出,避免分组/物化立刻把移除结果「翻回来」。
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

/// 组件感知的「等于或嵌套于」:键是正斜杠化的字符串,裸 `starts_with` 会把
/// `/a/bc` 误判进 `/a/b`,必须要求边界是分隔符。
fn key_is_same_or_nested(key: &str, base: &str) -> bool {
    if key == base {
        return true;
    }
    let base = base.strip_suffix('/').unwrap_or(base);
    if base.is_empty() {
        // POSIX 根 "/":一切绝对路径都嵌套其下。
        return key.starts_with('/');
    }
    key.starts_with(base) && key[base.len()..].starts_with('/')
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
    // 仅剩 tombstone(显式移出 None)时也保留文件:否则重启后文件消失,
    // 曾删除项目的文件夹会被下一次 ensure 重新物化,死而复生。
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
        never_materialize_roots: file.never_materialize_roots,
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
    /// convention of `REBIND_OLD_ROOT_EXISTS`/`REBIND_SESSIONS_BUSY`;
    /// localized frontend copy for this marker is not mapped yet (tracked in
    /// review #463), so the raw message is shown until it lands. The
    /// rejection path creates no guard; the token's Drop is the only clearing
    /// point, so error paths never leave a permanently closed gate.
    pub fn begin_rebind(&self) -> std::result::Result<RebindGate, String> {
        let mut flag = self.rebind_gate.lock();
        if *flag {
            return Err("REBIND_IN_PROGRESS: 另一个目录重绑定正在进行中，请稍后再试".to_string());
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
            return Err("REBIND_IN_PROGRESS: 另一个目录重绑定正在进行中，请稍后再试".to_string());
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

    /// 归属解析原料:显式归属条目(`None` = 显式移出)。
    pub fn assignment_of(&self, session_id: &str) -> Option<Option<String>> {
        self.state.read().assignments.get(session_id).cloned()
    }

    /// 全量归属快照(命令层随 list_projects 一并下发,前端分组解析用)。
    pub fn assignments_snapshot(&self) -> SessionAssignments {
        self.state.read().assignments.clone()
    }

    /// 显式归属到某项目的会话 id 列表(命令层统计成员数用)。
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

    /// 反物化排除表快照(canonical 键;命令层随 list_projects 下发,管理面板
    /// 据此展示与撤销)。
    pub fn never_materialize_roots(&self) -> Vec<String> {
        self.state.read().never_materialize_roots.clone()
    }

    /// 显式反物化(§3):`never = true` 把该文件夹(canonical 键)加入排除表,
    /// 之后 ensure 跳过它;`false` 撤销。幂等;返回更新后的表。只影响未来的自动
    /// 物化,既有项目与会话归属不受影响。
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
        if never {
            state.never_materialize_roots.push(key);
        } else {
            state.never_materialize_roots.retain(|entry| entry != &key);
        }
        persist_locked(&state, &self.path)?;
        Ok(state.never_materialize_roots.clone())
    }

    /// 创建项目。roots 可为空(纯标签项目);非空时逐个过绝对性/重叠校验。
    ///
    /// 落盘失败时内存态已前进而磁盘滞后(persist 在锁内最后执行,失败向上
    /// 抛,不回滚内存);同一进程内立即重试 create 会先撞内存重叠校验(磁盘
    /// 还是旧内容)——已知语义,由下一次成功写盘自愈。
    pub fn create_project(&self, name: String, roots: Vec<PathBuf>) -> Result<Project> {
        let name = validate_name(name)?;
        let mut state = self.state.write();
        let roots = validate_roots(&[], None, &roots)?;
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
            Some(roots) => Some(validate_roots(&[], None, &roots)?),
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

    /// 校验并归一化一份 roots 载荷而不改动状态。命令层在枚举「被移除根下的
    /// 成员」之前先跑一次,让差集比较发生在 canonical 形态之间:invoke 载荷只
    /// 做过键折叠(无 symlink/祖先解析),否则一次拼写不同但语义等价的 root
    /// 编辑(macOS `/var` vs `/private/var`、symlink 化的 home、autofs)会被误判
    /// 为移除,把该根下所有自动成员硬移出(评审 #484 B3)。
    pub fn normalize_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
        validate_roots(&[], None, roots)
    }

    /// root 更替 + 自动成员移出放在同一个 store 事务里(评审 #484 B3):一把锁、
    /// 一次落盘。此前更替与移出是两次 persist;若移出在新 roots 落盘后失败,命令
    /// 返回 Err,但重试时 `removed_roots` 已算成空(roots 已替换)从而永远跳过
    /// 移出——这些自动成员随后会被下一次 ensure 重新收编。`expel_session_ids`
    /// 是命令层枚举的「被移除根下的会话」;无归属条目的写成显式移出(None),
    /// 已有条目的不动(tier-① 语义,与 `expel_unassigned_sessions` 一致)。
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
        let roots = validate_roots(&state.projects, Some(project_id), &roots)?;
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

    /// 记录项目记住的主文件夹(§9.2):只接受 roots 成员(折叠键比较),外来
    /// 路径一律拒绝——主文件夹必须是项目势力范围内的目录。返回更新后的项目。
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

    /// 移除根时的成员移出(§4):命令层枚举的「被移除根下的会话」中,无归属
    /// 条目的写成显式移出(None)——留在未分组,阻止 tier-② 分组或 ensure
    /// 物化立刻把移除结果翻回来;已有条目的(显式归属本/他项目、已移出)不动,
    /// tier-① 语义优先。返回新写入的条目数。
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

    /// 删除项目:会话只被解绑(回落隐式分组),永不删除。全体成员——显式
    /// `Some(pid)` 条目加上命令层枚举的自动归组成员 `expel_session_ids`——一律
    /// 写成显式移出(None):留在未分组,且不会随文件夹的下一次自动物化复活;
    /// 之后在该文件夹新建的会话没有归属条目,照常自动归组。已有条目的 id
    /// (None = 已移出 / Some(其他) = 显式归属他处)不重写,tier-① 语义优先。
    ///
    /// 有意的边界语义(§9.9 跨项目重叠合法化后的交互,由测试锁定):tombstone 是
    /// 全局的——被删项目 A 的 root 若仍被存活项目 B 引用,该 root 下的自动成员
    /// 也被写成 None,不会被 B 的 tier-② 分组重新收编。删除是用户的显式声明
    /// ("这些会话退出分组"),自动复活会推翻它;要移入 B 需显式移动。
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
                let mut displays = validate_roots(&[], None, std::slice::from_ref(&owned_root))?;
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
        Ok(MoveSessionOutcome {
            project_id: project_id.map(str::to_string),
            added_root,
        })
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
    pub fn rebind_roots(&self, from: &Path, to: &Path) -> Result<Vec<String>> {
        let mut state = self.state.write();
        if from == to {
            return Ok(Vec::new());
        }
        // `to` is guaranteed to exist by the command layer; normalize it to
        // the canonical display form used for storage. `from` matching runs on
        // the folded identity key of its resolved display form (Windows folds
        // case/separators, so a case-only rename still matches), and the
        // suffix is cut by the resolved component count so a subdirectory
        // keeps its original casing. Rewrites happen on a candidate copy and
        // commit only after revalidation — on an overlap conflict the caller
        // observes state identical to disk.
        let to_key = root_display(to);
        let from_display = root_display(from);
        let from_key = identity_key_of_display(&from_display);
        let from_depth = from_display.components().count();
        let mut candidate = state.projects.clone();
        let mut affected_projects = Vec::new();
        for project in candidate.iter_mut() {
            let mut changed = false;
            for root in project.roots.iter_mut() {
                let root_key_str = identity_key_of_display(root);
                if !key_is_same_or_nested(&root_key_str, &from_key) {
                    continue;
                }
                let suffix: PathBuf = root.components().skip(from_depth).collect();
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
            // 跨项目重叠已合法化(§9.9):重绑定后只需保证组内不嵌套。
            for project in &candidate {
                validate_roots(&[], None, &project.roots)
                    .context("rebind produced nesting project roots")?;
            }
            state.projects = candidate;
            persist_locked(&state, &self.path)?;
        }
        Ok(affected_projects)
    }

    /// 文件夹项目自动物化(Codex 客户端式收编,幂等):对每个输入文件夹,复用
    /// 已锚定在它的物化项目,否则创建以目录 basename 命名的 `origin=folder`
    /// 项目(§9.9:被其它项目引用不算 covered,重叠合法)。排除表(§3)中的根
    /// 直接跳过。非绝对路径按根汇报 Failed,不阻断其余。输入按键去重;整批共用
    /// 一次落盘。分组规则不变——新项目的 roots 让既有 tier-② 根匹配自然收编
    /// 该文件夹下的会话,而显式移出条目(删除项目时写入)仍然压制,重建的项目
    /// 只会接收之后新建的会话。
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
            // 排除表(§3):用户已显式声明「此文件夹不建项目」——静默跳过
            // (与输入去重同形);tombstone 不会被复活。
            if state.never_materialize_roots.contains(&key) {
                continue;
            }
            // 锚定复用(§9.9):仅当已存在**锚定在该文件夹**的物化项目
            // (origin=folder 且 roots 恰含此路径)时复用;被其它项目当作附加/
            // 主根引用不算 covered——浏览通道始终以所选文件夹为家,重叠合法,
            // 于是会新建一个同名项目。
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
            // 同批内先前创建的文件夹项目已在 state.projects 中;相同根由
            // seen_keys 去重;锚定按精确路径判定,嵌套输入各自物化。
            match validate_roots(&[], None, std::slice::from_ref(root)) {
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

    /// 「对齐到项目」的归属解析(§6/§9.7):tier-① 显式归属(Some);显式移出
    /// (None 条目)阻断 tier-②;tier-② 用 workspace 的折叠键匹配 roots 前缀,
    /// 多个命中取 position 最小者(前端同款 tiebreak;projects 恒按
    /// (position, id) 排序,故 `find` 即最小)。
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

    /// 对齐用的钥匙串形态:主槽 = 会话自身 cwd(不换门,§9.2);附加根 = 项目
    /// roots 去掉 cwd,保持顺序(折叠键比较)。底座 normalize 会再归一化一次;
    /// 存储层按此形态写入,让「读到的」与「生效的」一致。
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
