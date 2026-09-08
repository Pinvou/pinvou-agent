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
    /// 来源标识:`Some("folder")` = 按文件夹自动物化的项目(Codex 客户端式
    /// 收编);`None` = 用户手工创建。仅作 UI 徽标与测试定位,不参与分组
    /// 判定与删除语义;用户改名/加根后保留原值,不做名字回写同步。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
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
}

const SCHEMA_VERSION: u32 = 1;

/// 删除项目的结果汇报:受影响会话 = 全体成员(显式归属 + 自动归组),一律写
/// 成显式移出留在未分组;会话本体永不删除。
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

/// `ensure_folder_roots` 的单根结果:前端/调用方据此区分新建、复用与冲突,
/// 冲突不阻断其余根。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EnsureFolderOutcome {
    /// 新建了同名文件夹项目(origin=folder)。
    Created { project: Project },
    /// 已有项目 root 覆盖该文件夹(等于或祖先),复用不动。
    Covered { project_id: String },
    /// 无法创建(非绝对路径、与既有 root 嵌套等);`reason` 可直接进日志。
    Failed { reason: String },
}

#[derive(Debug, Default)]
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
}

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
/// 退回词法绝对化——目录被移走后 overlap 校验仍需可判定,且形态对已存值
/// 幂等(canonicalize(canonical p) == p)。再经共享的 `platform_compat_path`
/// 归一,剥掉 Windows canonicalize 产生的 `\\?\` verbatim 前缀(非 Windows
/// 为恒等映射),与 `validate_codex_project_workspace` 的既有约定同源。
fn root_display(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| lexical_absolute(path));
    crate::platform::os::platform_compat_path(&canonical.to_string_lossy())
}

/// root 的比较键:展示形态经共享的 `filesystem_path_identity_key` 折叠——
/// Windows 折叠分隔符与大小写(`C:\Work` 与 `c:\work` 是同一 root),POSIX
/// 大小写敏感、原样保留。
///
/// 已知残留(接受的边缘):macOS 默认 APFS 大小写不敏感,但共享 helper 按
/// 「卷可能配置为大小写敏感」的约定不折叠大小写(见 platform/os/macos),
/// 同一目录换大小写写法仍算两个 root。按平台自行折叠会破坏大小写敏感卷,
/// 故在此记录而不折叠。
fn root_key(path: &Path) -> String {
    identity_key_of_display(&root_display(path))
}

/// 已存 root(写入时已是展示形态)的比较键:无需再触盘,只做键折叠。
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
        }
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

    /// 创建项目。roots 可为空(纯标签项目);非空时逐个过绝对性/重叠校验。
    ///
    /// 落盘失败时内存态已前进而磁盘滞后(persist 在锁内最后执行,失败向上
    /// 抛,不回滚内存);同一进程内立即重试 create 会先撞内存重叠校验(磁盘
    /// 还是旧内容)——已知语义,由下一次成功写盘自愈。
    pub fn create_project(&self, name: String, roots: Vec<PathBuf>) -> Result<Project> {
        self.create_project_with_origin(name, roots, None)
    }

    /// `create_project` 的带来源版本:ensure 收编通道传 `Some("folder")`。
    fn create_project_with_origin(
        &self,
        name: String,
        roots: Vec<PathBuf>,
        origin: Option<String>,
    ) -> Result<Project> {
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
            origin,
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

    /// 删除项目:成员会话(显式 Some(pid) + 命令层枚举的自动归组成员
    /// `expel_session_ids`)一律写成显式移出(None)——它们留在未分组,且不随
    /// 该文件夹的下一次自动物化复活;之后在该文件夹新建的会话没有归属条目,
    /// 照常自动归组。已有归属条目的 id(None=已移出/Some(其它)=显式归属它处)
    /// 不改写,tier-① 语义优先。会话永不删除。
    pub fn delete_project(
        &self,
        project_id: &str,
        expel_session_ids: &[String],
    ) -> Result<DeleteProjectReport> {
        let mut state = self.state.write();
        if !state.projects.iter().any(|project| project.id == project_id) {
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
            if !state.assignments.contains_key(session_id)
                && !affected.contains(session_id)
            {
                affected.push(session_id.clone());
            }
        }
        for session_id in &affected {
            state
                .assignments
                .insert(session_id.clone(), None);
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
        Ok(MoveSessionOutcome {
            project_id: project_id.map(str::to_string),
            added_root,
        })
    }

    /// 文件夹项目自动物化(Codex 客户端式收编,幂等):对每个输入文件夹,
    /// 已有项目 root 覆盖(等于或祖先)则复用,否则建 `origin=folder`、
    /// 名为目录 basename 的项目。与既有项目 root 嵌套等冲突逐根 Failed 上报,
    /// 不阻断其余根。输入按键去重;整批共享一次落盘。分组规则不变——新项目
    /// 的 roots 让既有 tier-② 根匹配自然收编该文件夹的会话,显式移出条目
    /// (删除项目时写入)仍压制,故删除后重建的项目只收新会话。
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
            let covered = state.projects.iter().find(|project| {
                project
                    .roots
                    .iter()
                    .any(|existing| key_is_same_or_nested(&key, &identity_key_of_display(existing)))
            });
            if let Some(project) = covered {
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
            // 同批先建的文件夹项目已在 state.projects 中,后续根与之重叠会被
            // validate 拦下,保证整批任何顺序执行结果一致。
            match validate_roots(&state.projects, None, std::slice::from_ref(root)) {
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
                    };
                    state.projects.push(project.clone());
                    state.projects.sort_by(|a, b| {
                        (a.position, &a.id).cmp(&(b.position, &b.id))
                    });
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

    /// 目录重绑定(修断链通道):把落在 `from` 前缀下的项目 root 平移到 `to`。
    /// `from` 按存储原值匹配(可能已在磁盘上消失),`to` 由命令层校验为存在
    /// 的 canonical 路径。改写后逐项目复验重叠约束——平移出的 root 可能撞上
    /// 其它项目的领地,此时整体报错回滚(内存态未落盘)。返回受影响项目 id。
    /// 幂等:无 root 命中即空操作。
    pub fn rebind_roots(&self, from: &Path, to: &Path) -> Result<Vec<String>> {
        let mut state = self.state.write();
        if from == to {
            return Ok(Vec::new());
        }
        // to 由命令层保证存在,这里统一成 canonical 键,与存储形态一致;
        // from 的匹配在折叠键上进行(Windows 折叠大小写/分隔符,仅大小写
        // 改名的目录不再漏配),后缀按组件数从原 root 切回,保留子目录
        // 原有大小写。改写在副本上进行,复验通过才提交内存态——重叠
        // 冲突时调用方看到的状态与盘面保持一致。
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
                validate_roots(&candidate, Some(&project.id), &project.roots)
                    .context("rebind produced overlapping project roots")?;
            }
            state.projects = candidate;
            persist_locked(&state, &self.path)?;
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
