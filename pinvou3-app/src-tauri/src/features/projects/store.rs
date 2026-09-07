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

/// 单文件持久化结构。schema_version 供未来结构演进识别(读到更新版本时
/// 拒绝加载,防止旧进程把新结构降级写坏)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ProjectsFile {
    pub schema_version: u32,
    pub projects: Vec<Project>,
    #[serde(default)]
    pub assignments: SessionAssignments,
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

#[derive(Debug, Default)]
struct StoreState {
    /// 恒按 (position, id) 有序,`list` 直接返回快照。
    projects: Vec<Project>,
    assignments: SessionAssignments,
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

/// root 的比较键:目录存在时用 fs::canonicalize(消 symlink),不存在时退回
/// 词法绝对化——目录被移走后 overlap 校验仍需可判定,且键对已存值幂等
/// (canonicalize(canonical p) == p)。
fn root_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| lexical_absolute(path))
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

/// 校验一组 roots 并返回 canonicalized 形态:
/// - 必须是绝对路径;
/// - 组内不得重复或互相嵌套;
/// - 不得与其它项目(skip_project_id 之外)的任何 root 重复或嵌套——自动
///   归组按 root 前缀匹配,跨项目重叠会让归属二义(Codex #22767 错归组的根源)。
fn validate_roots(
    projects: &[Project],
    skip_project_id: Option<&str>,
    roots: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let mut keys = Vec::with_capacity(roots.len());
    for root in roots {
        if !root.is_absolute() {
            bail!("project root must be absolute: {}", root.display());
        }
        let key = root_key(root);
        if keys.contains(&key) {
            bail!("duplicate project root: {}", root.display());
        }
        keys.push(key);
    }
    for (index, key) in keys.iter().enumerate() {
        for other in keys.iter().skip(index + 1) {
            if key.starts_with(other) || other.starts_with(key) {
                bail!(
                    "project roots must not nest: {} vs {}",
                    key.display(),
                    other.display()
                );
            }
        }
    }
    for project in projects {
        if Some(project.id.as_str()) == skip_project_id {
            continue;
        }
        for existing in &project.roots {
            for key in &keys {
                if key.starts_with(existing) || existing.starts_with(key) {
                    bail!(
                        "project root overlaps project '{}' ({} vs {})",
                        project.name,
                        existing.display(),
                        key.display()
                    );
                }
            }
        }
    }
    Ok(keys)
}

/// 原子落盘:tmp+rename。空状态删除文件,不留空壳。
fn persist_locked(state: &StoreState, path: &Path) -> Result<()> {
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
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&file)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename {}", path.display()))?;
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
        bail!(
            "projects schema {} is newer than supported {}",
            file.schema_version,
            SCHEMA_VERSION
        );
    }
    let mut projects = file.projects;
    projects.sort_by(|a, b| (a.position, &a.id).cmp(&(b.position, &b.id)));
    Ok(StoreState {
        projects,
        assignments: file.assignments,
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

    pub fn delete_project(&self, project_id: &str) -> Result<DeleteProjectReport> {
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
        let affected: Vec<String> = state
            .assignments
            .iter()
            .filter_map(|(session_id, assigned)| {
                (assigned.as_deref() == Some(project_id)).then(|| session_id.clone())
            })
            .collect();
        for session_id in &affected {
            state.assignments.remove(session_id);
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
                let mut keys =
                    validate_roots(&state.projects, Some(target_id), &[workspace.to_path_buf()])?;
                let Some(key) = keys.pop() else {
                    bail!("add_workspace_root produced no canonical key");
                };
                let project = &mut state.projects[index];
                let already_covered = project.roots.iter().any(|root| key.starts_with(root));
                if !already_covered {
                    project.roots.push(key.clone());
                    project.updated_at = Utc::now();
                    added_root = Some(key);
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

    /// 会话删除钩子:摘除其归属条目(含显式移出的 None 条目)。返回是否
    /// 发生变更;落盘失败仅记日志,内存态已前进,下次变更自愈。
    pub fn forget_session(&self, session_id: &str) -> bool {
        let mut state = self.state.write();
        if state.assignments.remove(session_id).is_none() {
            return false;
        }
        if let Err(error) = persist_locked(&state, &self.path) {
            eprintln!("[projects] persist after forget_session({session_id}) failed: {error:#}");
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
